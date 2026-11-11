//! PX-021 acceptance: a last-observed record (tombstone) for a watched
//! process that leaves the collection (D1 §§6.2, 20.4, 26; D2 T21).
//!
//! * An exiting worker leaves an inspectable tombstone: the inspector shows
//!   its identity, last sample, last CPU and resident memory and last-observed
//!   time, marked historical, and no request takes it as a target.
//! * An inaccessible scan never records an exit: an unreadable root keeps
//!   every row, a denied record keeps its row, and a scan whose identity
//!   could not be read says "no longer observed" of everything it replaced.
//! * A PID replacement is a distinct live item.
//! * Retention expires deterministically under a fake clock.

use srui_process_explorer::lastseen::{
    Departure, Exit, Unobserved, MAX_TOMBSTONES, MAX_TOMBSTONE_AGE,
};
use srui_process_explorer::procfs::MonotonicClock;
use srui_process_explorer::refresh::{ProcessView, TagRequest};
use srui_process_explorer::source::{
    BootId, CappedRecords, Completeness, CpuInterval, CpuUsage, CreationToken, DisplayName,
    EnumerationIssue, HostId, IssueScope, Lineage, MissingReason, Observed, PidLedger,
    PidNamespaceId, ProcessKey, ProcessRecord, ProcessSnapshot, ProcessSource, SkippedRecords,
    SnapshotTime, SourceId, SystemSample, ThreadScan, ThreadsSource, KIND_NOT_COLLECTED,
    OWNER_NOT_COLLECTED,
};
use srui_process_explorer::{MODEL, TREE_MODEL};
use srui_sdk::{ItemId, Value};
use srui_sessiond::Session;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

const SOURCE: &str = "lastseen-fixture";
const EPOCH: u64 = 1_800_000_000;

fn key(pid: u32, token: &str, host: Observed<HostId>) -> ProcessKey {
    ProcessKey {
        source: SourceId(SOURCE.into()),
        host,
        boot: Observed::Known(BootId("boot".into())),
        pid_namespace: Observed::Known(PidNamespaceId(7)),
        pid: Observed::Known(pid),
        creation: CreationToken::Opaque(token.into()),
        thread: None,
    }
}

fn host() -> Observed<HostId> {
    Observed::Known(HostId("host".into()))
}

fn record(pid: u32, token: &str, name: &str) -> ProcessRecord {
    record_on(pid, token, name, host())
}

fn record_on(pid: u32, token: &str, name: &str, host: Observed<HostId>) -> ProcessRecord {
    ProcessRecord {
        kind: KIND_NOT_COLLECTED,
        lineage: Lineage::new(if pid == 1 { 0 } else { 1 }, u64::from(pid)),
        owner: OWNER_NOT_COLLECTED,
        key: key(pid, token, host),
        display_name: DisplayName::from(name),
        resident: Observed::Known(u64::from(pid) << 12),
        cpu: CpuUsage::Measured(CpuInterval {
            ticks: 25,
            ticks_per_second: 100,
            elapsed: Duration::from_secs(1),
        }),
    }
}

/// One scan: its records and its completeness.
type Step = (Vec<ProcessRecord>, Completeness);

fn complete(records: Vec<ProcessRecord>) -> Step {
    (records, Completeness::Complete)
}

/// A scan whose process list could not be read at all.
fn unreadable_root() -> Step {
    (
        Vec::new(),
        Completeness::from_scan(
            SkippedRecords::unenumerable(),
            vec![EnumerationIssue {
                scope: IssueScope::Root,
                reason: MissingReason::Denied,
                detail: "the process list could not be read".into(),
            }],
        ),
    )
}

/// A scan that listed `denied` but could not read those records.
fn denied(records: Vec<ProcessRecord>, denied: &[u32]) -> Step {
    let mut skipped = PidLedger::with_limit(16);
    let mut issues = Vec::new();
    for pid in denied {
        skipped.record(*pid);
        issues.push(EnumerationIssue {
            scope: IssueScope::Process(*pid),
            reason: MissingReason::Denied,
            detail: "denied".into(),
        });
    }
    (records, Completeness::from_scan(skipped, issues))
}

/// A scan that read records but cannot say which ones it missed.
fn unenumerable(records: Vec<ProcessRecord>) -> Step {
    (
        records,
        Completeness::from_scan(SkippedRecords::unenumerable(), Vec::new()),
    )
}

/// Publishes `steps` in turn, one second apart, then repeats the last.
struct Script {
    steps: Vec<Step>,
    next: usize,
}

impl Script {
    fn new(steps: Vec<Step>) -> Self {
        Self { steps, next: 0 }
    }
}

impl ProcessSource for Script {
    fn status_text(&self) -> &str {
        "Read-only · Last-observed fixture"
    }

    fn snapshot(&mut self) -> ProcessSnapshot {
        let index = self.next.min(self.steps.len() - 1);
        let at = self.next as u64;
        self.next += 1;
        let (records, completeness) = self.steps[index].clone();
        ProcessSnapshot {
            source: SourceId(SOURCE.into()),
            sampled_at: SnapshotTime(SystemTime::UNIX_EPOCH + Duration::from_secs(EPOCH + at)),
            records,
            vanished: 0,
            capped: CappedRecords::none(),
            completeness,
            system: SystemSample::not_provided(),
            threads: ThreadScan::NotProvided,
        }
    }
}

/// The UTC text of step `step`'s sample time.
fn at(step: u64) -> String {
    format!("2027-01-15 08:00:{step:02} UTC")
}

#[derive(Debug)]
struct FakeClock(Mutex<Duration>);

impl FakeClock {
    fn set(&self, now: Duration) {
        *self.0.lock().unwrap() = now;
    }
}

impl MonotonicClock for FakeClock {
    fn now(&self) -> Duration {
        *self.0.lock().unwrap()
    }
}

struct Harness {
    session: Session,
    view: ProcessView,
    source: Script,
}

impl Harness {
    fn new(steps: Vec<Step>) -> Self {
        Self::with(steps, |view| view)
    }

    fn with(steps: Vec<Step>, adjust: impl FnOnce(ProcessView) -> ProcessView) -> Self {
        let session = Session::mint();
        let mut source = Script::new(steps);
        let (view, _) = ProcessView::start(&session, &mut source).unwrap();
        Self {
            session,
            view: adjust(view),
            source,
        }
    }

    fn refresh(&mut self) {
        self.view.refresh(&self.session, &mut self.source).unwrap();
    }

    /// The shown row whose PID cell is `pid` and whose name is `name`.
    fn item(&self, pid: u32, name: &str) -> Option<ItemId> {
        self.session.with_store(|store| {
            [MODEL, TREE_MODEL]
                .into_iter()
                .filter_map(|model| store.get_model(model))
                .flat_map(|model| model.items.values())
                .find(|item| {
                    matches!(&item.value, Value::List(cells)
                        if cells[0] == Value::UnsignedInt(u64::from(pid))
                            && cells[1] == Value::String(name.into()))
                })
                .map(|item| item.item_id)
        })
    }

    fn select(&mut self, pid: u32, name: &str) -> ItemId {
        let item = self
            .item(pid, name)
            .unwrap_or_else(|| panic!("PID {pid} {name} is shown"));
        self.view.select_item(&self.session, Some(item)).unwrap();
        item
    }

    fn inspector(&self) -> [String; 5] {
        let view = self.view.inspector_view();
        [
            view.status.clone(),
            view.process.clone(),
            view.details.clone(),
            view.metrics.clone(),
            view.command.clone(),
        ]
    }

    fn status(&self) -> String {
        self.view.inspector_view().status.clone()
    }
}

#[test]
fn an_exiting_worker_leaves_an_inspectable_tombstone_that_no_request_takes() {
    let mut h = Harness::new(vec![
        complete(vec![
            record(1, "init", "init"),
            record(300, "w", "worker"),
            record(301, "o", "other"),
        ]),
        complete(vec![record(1, "init", "init"), record(301, "o", "other")]),
    ]);
    let worker = h.select(300, "worker");
    h.view.request_tags(&h.session, TagRequest::Toggle).unwrap();
    h.view.set_follow(&h.session, Some(true)).unwrap();
    let live = h.inspector();
    assert_eq!(
        live[0],
        "Selected process: PID 300 · worker · this source provides no details"
    );
    assert!(h.view.last_seen().is_empty());

    // The worker exits: the next complete scan no longer lists PID 300.
    h.refresh();
    assert_eq!(h.item(300, "worker"), None, "its row left the collection");
    let [status, process, details, metrics, command] = h.inspector();
    assert_eq!(
        status,
        format!(
            "Selected process: PID 300 · worker · exited: a scan that accounts for its PID no longer lists it · last observed {}; a historical record: no action applies to it",
            at(0)
        )
    );
    // Its identity and last sample, as last held: the owner, the CPU share
    // and resident memory of the last scan that listed it.
    assert_eq!(process, live[1]);
    assert_eq!(
        metrics,
        format!("Last sampled · {} · last observed {}", live[3], at(0))
    );
    assert_eq!(
        details,
        "State, parent and start time: none read while it was observed"
    );
    assert_eq!(
        command,
        "Command line: not kept once the process left the list: arguments can carry secrets"
    );
    assert_eq!(h.view.inspector().selected(), Some(worker));

    let seen = h.view.last_seen();
    assert_eq!(seen.len(), 1);
    let tombstone = seen
        .get(&key(300, "w", host()))
        .expect("the worker's tombstone");
    assert_eq!(tombstone.departure(), Departure::Exited(Exit::Unlisted));
    assert_eq!(
        tombstone.last_observed(),
        Some(SystemTime::UNIX_EPOCH + Duration::from_secs(EPOCH))
    );
    assert_eq!(
        tombstone.left_at(),
        SystemTime::UNIX_EPOCH + Duration::from_secs(EPOCH + 1)
    );
    assert!(tombstone.is_tagged());

    // Follow stopped and says how it left; the tag is historical and says so.
    assert_eq!(
        h.view.follow_view().status,
        "Follow stopped: PID 300 · worker exited · a new process that reuses its PID is not followed; select a row and turn follow on again"
    );
    assert!(
        h.view.tags_status().starts_with(&format!(
            "Tags: 0 tagged · 1 historical: PID 300 · worker exited, last observed {}",
            at(0)
        )),
        "{}",
        h.view.tags_status()
    );

    // No request takes the tombstone as its target.
    h.view.request_tags(&h.session, TagRequest::Toggle).unwrap();
    assert!(
        h.view.tags_status().ends_with(
            "Nothing tagged: PID 300 · worker exited: a historical record is never tagged"
        ),
        "{}",
        h.view.tags_status()
    );
    h.view
        .request_tags(&h.session, TagRequest::Descendants)
        .unwrap();
    assert!(h
        .view
        .tags_status()
        .ends_with("a historical record is never tagged"));
    assert_eq!(h.view.tags().tagged(), 0);
    h.view.set_follow(&h.session, Some(true)).unwrap();
    assert_eq!(
        h.view.follow_view().status,
        "Follow: off · PID 300 · worker exited: a historical record is never followed; select a listed process first"
    );
    assert_eq!(h.view.follow().following(), None);

    // Later samples keep it, unchanged, while it is watched.
    h.refresh();
    assert_eq!(h.inspector()[0], status);
    assert_eq!(h.view.last_seen().len(), 1);
}

#[test]
fn an_inaccessible_scan_never_records_an_exit() {
    let unknown_host = || Observed::Missing(MissingReason::Denied);
    let mut h = Harness::new(vec![
        complete(vec![
            record(1, "init", "init"),
            record(300, "w", "worker"),
            record(301, "o", "other"),
        ]),
        // The process list itself could not be read.
        unreadable_root(),
        // PID 300's record is denied.
        denied(
            vec![record(1, "init", "init"), record(301, "o", "other")],
            &[300],
        ),
        // Every record read, but not the host identity, and PIDs 300 and 301
        // are not listed: the session's identity anchors `init` (PX-003), but
        // a listing whose host is unknown proves nothing about an absence.
        complete(vec![record_on(1, "init", "init", unknown_host())]),
    ]);
    h.select(301, "other");
    h.view.request_tags(&h.session, TagRequest::Toggle).unwrap();
    h.select(1, "init");
    h.view.request_tags(&h.session, TagRequest::Toggle).unwrap();
    h.select(300, "worker");

    h.refresh();
    assert!(
        h.view.last_seen().is_empty(),
        "an unreadable root records nothing"
    );
    assert!(h.item(300, "worker").is_some(), "every row is kept");
    assert!(!h.status().contains("exited") && !h.status().contains("observed"));
    assert_eq!(h.view.tags().tagged(), 2);

    h.refresh();
    assert!(h.view.last_seen().is_empty(), "a denied record is kept");
    assert_eq!(
        h.status(),
        "Selected process: PID 300 · worker · this source provides no details · row kept from an earlier scan"
    );

    // The identity could not be read: the absent rows leave, and neither is
    // said to have exited; `init` is the same instance, still listed.
    let init = h.item(1, "init");
    h.refresh();
    assert_eq!(
        h.item(1, "init"),
        init,
        "an anchored instance keeps its item"
    );
    let seen = h.view.last_seen();
    assert_eq!(seen.len(), 2, "the target and the tagged 301");
    for tombstone in seen.iter() {
        assert_eq!(
            tombstone.departure(),
            Departure::NotObserved(Unobserved::IdentityUnconfirmed),
            "{}",
            tombstone.subject()
        );
    }
    // PID 300 was last listed by the first scan: the next two only kept it.
    assert_eq!(
        h.status(),
        format!(
            "Selected process: PID 300 · worker · no longer observed: the scan could not confirm the host, boot and PID namespace it was listed in; whether it exited is unknown · last observed {}; a historical record: no action applies to it",
            at(0)
        )
    );
    let tags = h.view.tags_status().to_string();
    assert!(!tags.contains("exited"), "{tags}");
    assert!(
        tags.starts_with(&format!(
            "Tags: 1 tagged · 1 historical: PID 301 · other no longer observed, last observed {}",
            at(2)
        )),
        "{tags}"
    );
}

#[test]
fn a_row_left_out_at_the_row_limit_is_no_longer_observed() {
    let mut h = Harness::with(
        vec![
            complete(vec![
                record(1, "init", "init"),
                record(300, "w", "worker"),
                record(301, "o", "other"),
            ]),
            // Cannot say what it missed: 300 and 301 are kept, then left out
            // because three rows is the limit.
            unenumerable(vec![
                record(1, "init", "init"),
                record(400, "n", "new"),
                record(401, "m", "newer"),
            ]),
        ],
        |view| view.with_published_limit(3),
    );
    h.select(300, "worker");
    h.refresh();
    assert_eq!(h.item(300, "worker"), None);
    let tombstone = h
        .view
        .last_seen()
        .get(&key(300, "w", host()))
        .expect("the target's tombstone");
    assert_eq!(
        tombstone.departure(),
        Departure::NotObserved(Unobserved::Evicted)
    );
    assert!(
        h.status().contains(
            "· no longer observed: kept while scans could not account for it, then left out at the row limit; whether it exited is unknown · "
        ),
        "{}",
        h.status()
    );
}

#[test]
fn a_pid_replacement_is_a_distinct_live_item() {
    let mut h = Harness::new(vec![
        complete(vec![record(1, "init", "init"), record(300, "a", "first")]),
        complete(vec![record(1, "init", "init"), record(300, "b", "second")]),
    ]);
    let first = h.select(300, "first");
    h.refresh();
    let second = h.item(300, "second").expect("the newcomer is listed");
    assert_ne!(second, first, "a reused PID is another item");
    assert_eq!(h.item(300, "first"), None);
    assert_eq!(
        h.view
            .last_seen()
            .get(&key(300, "a", host()))
            .map(|tombstone| tombstone.departure()),
        Some(Departure::Exited(Exit::Replaced))
    );
    assert!(h.view.last_seen().get(&key(300, "b", host())).is_none());
    assert!(
        h.status().starts_with(
            "Selected process: PID 300 · first · exited: its PID now names a newer process · "
        ),
        "{}",
        h.status()
    );
    // The newcomer is a live process, described live.
    h.select(300, "second");
    assert_eq!(
        h.status(),
        "Selected process: PID 300 · second · this source provides no details"
    );
    assert_eq!(
        h.inspector()[3],
        "CPU: 25.0% · Resident: 1.1 MiB",
        "its own metrics"
    );
    // The first instance's tombstone is no longer watched: the next sample
    // drops it.
    assert_eq!(h.view.last_seen().len(), 1);
    h.refresh();
    assert!(h.view.last_seen().is_empty());
}

#[test]
fn retention_expires_deterministically_under_a_fake_clock() {
    let clock = Arc::new(FakeClock(Mutex::new(Duration::from_secs(100))));
    let injected: Arc<dyn MonotonicClock> = clock.clone();
    let mut h = Harness::with(
        vec![
            complete(vec![record(1, "init", "init"), record(300, "w", "worker")]),
            complete(vec![record(1, "init", "init")]),
        ],
        |view| view.with_clock(injected),
    );
    h.select(300, "worker");
    h.refresh();
    assert_eq!(h.view.last_seen().len(), 1);
    let left = Duration::from_secs(100);
    // One nanosecond short of the bound: kept.
    clock.set(left + MAX_TOMBSTONE_AGE - Duration::from_nanos(1));
    h.refresh();
    assert_eq!(h.view.last_seen().len(), 1);
    assert!(h.status().contains("exited"), "{}", h.status());
    // At the bound: dropped by the first sample, and the inspector says so.
    clock.set(left + MAX_TOMBSTONE_AGE);
    h.refresh();
    assert!(h.view.last_seen().is_empty());
    assert_eq!(h.view.last_seen().dropped(), 1);
    assert_eq!(
        h.inspector(),
        [
            "Selected process: PID 300 · worker · its last-observed record is no longer kept (kept at most 10 minutes); select another row".to_string(),
            "Name: worker · PID 300 · Owner: unavailable".to_string(),
            "State, parent and start time: none: the record is no longer kept".to_string(),
            "CPU and resident memory: none: the record is no longer kept".to_string(),
            "Command line: none: the record is no longer kept".to_string(),
        ]
    );
    assert_eq!(MAX_TOMBSTONE_AGE, Duration::from_secs(600));
}

#[test]
fn the_count_bound_keeps_the_newest_tombstones() {
    // MAX_TOMBSTONES + 1 tagged processes exit in one scan: the oldest
    // tombstone goes, counted as a forgotten historical tag.
    let count = MAX_TOMBSTONES as u32 + 1;
    let mut all = vec![record(1, "init", "init")];
    all.extend((0..count).map(|n| record(1000 + n, &format!("t{n}"), &format!("c{n}"))));
    let mut h = Harness::new(vec![
        complete(all),
        complete(vec![record(1, "init", "init")]),
    ]);
    h.select(1, "init");
    h.view
        .request_tags(&h.session, TagRequest::Descendants)
        .unwrap();
    h.select(1, "init");
    h.view.request_tags(&h.session, TagRequest::Toggle).unwrap();
    assert_eq!(h.view.tags().tagged(), count as usize);
    h.refresh();
    assert_eq!(h.view.last_seen().len(), MAX_TOMBSTONES);
    assert_eq!(h.view.last_seen().forgotten_tags(), 1);
    assert!(h
        .view
        .tags_status()
        .contains(&format!("{count} historical")));
    assert!(h.view.tags_status().contains("(1 forgotten)"));
}

#[test]
fn a_paused_view_records_nothing_until_it_resumes() {
    let mut h = Harness::new(vec![
        complete(vec![record(1, "init", "init"), record(300, "w", "worker")]),
        complete(vec![record(1, "init", "init")]),
    ]);
    h.select(300, "worker");
    h.view.pause_view(&h.session).unwrap();
    h.refresh();
    assert!(
        h.view.last_seen().is_empty(),
        "a paused view takes no sample"
    );
    assert!(h.item(300, "worker").is_some(), "its frozen row stands");
    h.view.resume_view(&h.session).unwrap();
    h.refresh();
    assert_eq!(h.view.last_seen().len(), 1);
    assert!(
        h.status().contains(&format!(
            "exited: a scan that accounts for its PID no longer lists it · last observed {}",
            at(0)
        )),
        "{}",
        h.status()
    );
}

#[test]
fn a_filtered_out_target_is_not_historical_until_it_leaves() {
    let mut h = Harness::new(vec![
        complete(vec![
            record(1, "init", "init"),
            record(300, "w", "worker"),
            record(301, "o", "other"),
        ]),
        complete(vec![
            record(1, "init", "init"),
            record(300, "w", "worker"),
            record(301, "o", "other"),
        ]),
        complete(vec![record(1, "init", "init"), record(301, "o", "other")]),
    ]);
    h.select(300, "worker");
    h.view.set_filter(&h.session, "other").unwrap();
    h.refresh();
    assert!(h.view.last_seen().is_empty(), "a filter ends nothing");
    assert!(
        h.status().contains("not shown (filtered out"),
        "{}",
        h.status()
    );
    h.refresh();
    assert_eq!(h.view.last_seen().len(), 1);
    assert!(h.status().contains("· exited: "), "{}", h.status());
}

#[test]
fn the_tree_shows_the_same_tombstone() {
    let mut h = Harness::new(vec![
        complete(vec![record(1, "init", "init"), record(300, "w", "worker")]),
        complete(vec![record(1, "init", "init")]),
    ]);
    h.view.set_view(&h.session, Some(true)).unwrap();
    let worker = h.select(300, "worker");
    h.refresh();
    assert_eq!(h.item(300, "worker"), None);
    assert!(!h.view.collapsed().contains(&worker));
    assert!(
        h.status()
            .starts_with("Selected process: PID 300 · worker · exited: "),
        "{}",
        h.status()
    );
}

#[test]
fn a_selected_thread_gets_a_tombstone_and_a_hidden_one_does_not() {
    let session = Session::mint();
    let mut source = ThreadsSource::default();
    let (mut view, _) = ProcessView::start(&session, &mut source).unwrap();
    view.set_user_threads(&session, Some(true)).unwrap();
    view.refresh(&session, &mut source).unwrap();
    let thread_item = |session: &Session, name: &str| {
        session.with_store(|store| {
            store
                .get_model(MODEL)
                .unwrap()
                .items
                .values()
                .find(|item| matches!(&item.value, Value::List(cells) if cells[1] == Value::String(name.into())))
                .map(|item| item.item_id)
        })
    };
    let worker = thread_item(&session, "worker-1").expect("worker-1 is shown");
    view.select_item(&session, Some(worker)).unwrap();
    view.refresh(&session, &mut source).unwrap();
    let seen = view.last_seen();
    assert_eq!(seen.len(), 1, "only the selected thread");
    let tombstone = seen.iter().next().unwrap();
    assert!(tombstone.key().is_thread());
    assert_eq!(tombstone.departure(), Departure::Exited(Exit::Unlisted));
    assert!(
        view.inspector_view().status.starts_with(
            "Selected thread: TID 201 of PID 200 · worker-1 · exited: a scan that read its process's threads no longer lists it · last observed "
        ),
        "{}",
        view.inspector_view().status
    );

    // A thread selected and then hidden with the user threads is cleared from
    // the inspector first, so the sample that drops it records nothing.
    let mut source = ThreadsSource::default();
    let session = Session::mint();
    let (mut view, _) = ProcessView::start(&session, &mut source).unwrap();
    view.set_user_threads(&session, Some(true)).unwrap();
    view.refresh(&session, &mut source).unwrap();
    let worker = thread_item(&session, "worker-2").expect("worker-2 is shown");
    view.select_item(&session, Some(worker)).unwrap();
    view.set_user_threads(&session, Some(false)).unwrap();
    view.refresh(&session, &mut source).unwrap();
    assert!(view.last_seen().is_empty(), "{:?}", view.last_seen().len());
}
