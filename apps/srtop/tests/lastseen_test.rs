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

use srui_process_explorer::inspector::{DetailRead, DetailRequest};
use srui_process_explorer::lastseen::{
    Departure, Exit, Unobserved, MAX_TOMBSTONES, MAX_TOMBSTONE_AGE,
};
use srui_process_explorer::procfs::MonotonicClock;
use srui_process_explorer::refresh::{ProcessView, TagRequest};
use srui_process_explorer::source::{
    BootId, CappedRecords, Completeness, CpuInterval, CpuUsage, CreationToken, DisplayName,
    EnumerationIssue, HostId, IssueScope, Lineage, MissingReason, Observed, PidLedger,
    PidNamespaceId, ProcessKey, ProcessRecord, ProcessSnapshot, ProcessSource, SkippedRecords,
    SnapshotTime, SourceId, SystemSample, TaskKind, ThreadIdentity, ThreadRecords, ThreadScan,
    ThreadsSource, KIND_NOT_COLLECTED, OWNER_NOT_COLLECTED,
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
    /// What each step says about threads; `NotProvided` past its end.
    threads: Vec<ThreadScan>,
    next: usize,
}

impl Script {
    fn new(steps: Vec<Step>) -> Self {
        Self {
            steps,
            threads: Vec::new(),
            next: 0,
        }
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
            threads: self
                .threads
                .get(index)
                .cloned()
                .unwrap_or(ThreadScan::NotProvided),
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
            "Selected process: PID 300 · worker · its last-observed record is no longer kept (records are kept at most 10 minutes, 64 records and 32 KiB, the oldest dropped first); select another row".to_string(),
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

// ---- Correction round 1 ----

/// F1: a historical tag that aged out is counted once, not twice; the
/// status says it is forgotten, on a sample that could not even read the
/// process list (N1).
#[test]
fn an_aged_out_historical_tag_is_counted_once_even_on_an_unreadable_scan() {
    let clock = Arc::new(FakeClock(Mutex::new(Duration::from_secs(10))));
    let injected: Arc<dyn MonotonicClock> = clock.clone();
    let mut h = Harness::with(
        vec![
            complete(vec![record(1, "init", "init"), record(300, "w", "worker")]),
            complete(vec![record(1, "init", "init")]),
            unreadable_root(),
        ],
        |view| view.with_clock(injected),
    );
    h.select(300, "worker");
    h.view.request_tags(&h.session, TagRequest::Toggle).unwrap();
    h.select(1, "init");
    h.refresh();
    assert!(h.view.tags_status().starts_with(&format!(
        "Tags: 0 tagged · 1 historical: PID 300 · worker exited, last observed {}",
        at(0)
    )));
    clock.set(Duration::from_secs(10) + MAX_TOMBSTONE_AGE);
    h.refresh();
    assert_eq!(h.view.historical_tags().len(), 0);
    assert_eq!(h.view.last_seen().forgotten_tags(), 1);
    assert_eq!(
        h.view.tags_status(),
        "Tags: 0 tagged · 1 historical, forgotten · Tagged PID 300 · worker",
        "one historical tag, counted once"
    );
    h.view.request_tags(&h.session, TagRequest::Clear).unwrap();
    assert!(h
        .view
        .tags_status()
        .ends_with("Cleared 0 tags and 1 historical; the selection is unchanged"));
}

/// N1: tombstones age on samples that could not read the process list too:
/// age is time on the injected clock, not a scan outcome.
#[test]
fn a_tombstone_expires_while_the_process_list_is_unreadable() {
    let clock = Arc::new(FakeClock(Mutex::new(Duration::from_secs(10))));
    let injected: Arc<dyn MonotonicClock> = clock.clone();
    let mut h = Harness::with(
        vec![
            complete(vec![record(1, "init", "init"), record(300, "w", "worker")]),
            complete(vec![record(1, "init", "init")]),
            unreadable_root(),
        ],
        |view| view.with_clock(injected),
    );
    h.select(300, "worker");
    h.refresh();
    assert_eq!(h.view.last_seen().len(), 1);
    clock.set(Duration::from_secs(10) + MAX_TOMBSTONE_AGE - Duration::from_nanos(1));
    h.refresh();
    assert_eq!(h.view.last_seen().len(), 1, "short of the bound");
    assert!(h.status().contains("exited"), "{}", h.status());
    clock.set(Duration::from_secs(10) + MAX_TOMBSTONE_AGE);
    h.refresh();
    assert!(h.view.last_seen().is_empty());
    assert!(
        h.status()
            .contains("its last-observed record is no longer kept"),
        "{}",
        h.status()
    );
}

/// F2: a process left out at the row limit reads "no longer observed";
/// listed again, its record goes (it was observed after all), and the
/// inspector, still on the old row, says so. Selected again and then really
/// exiting, it leaves a fresh record: the new verdict, the last scan that
/// listed it, its latest cells.
#[test]
fn an_instance_listed_again_after_eviction_gets_a_fresh_record_when_it_exits() {
    let mut grown = record(300, "w", "worker");
    grown.resident = Observed::Known(64 << 20);
    let mut h = Harness::with(
        vec![
            complete(vec![
                record(1, "init", "init"),
                record(300, "w", "worker"),
                record(301, "o", "other"),
            ]),
            unenumerable(vec![
                record(1, "init", "init"),
                record(400, "n", "new"),
                record(401, "m", "newer"),
            ]),
            complete(vec![
                record(1, "init", "init"),
                record(300, "w", "worker"),
                record(301, "o", "other"),
            ]),
            complete(vec![
                record(1, "init", "init"),
                grown,
                record(301, "o", "other"),
            ]),
            complete(vec![record(1, "init", "init"), record(301, "o", "other")]),
        ],
        |view| view.with_published_limit(3),
    );
    let first = h.select(300, "worker");
    h.refresh();
    let evicted = h.view.last_seen().get(&key(300, "w", host())).unwrap();
    assert_eq!(
        evicted.departure(),
        Departure::NotObserved(Unobserved::Evicted)
    );
    // Listed again: the record that said "no longer observed" goes.
    h.refresh();
    let again = h.item(300, "worker").expect("listed again");
    assert_ne!(again, first);
    assert!(h.view.last_seen().is_empty());
    assert_eq!(
        h.status(),
        "Selected process: PID 300 · worker · listed again after it was no longer observed, under a new row; select that row to inspect it"
    );
    h.select(300, "worker");
    h.refresh();
    let live = h.inspector()[3].clone();
    assert_eq!(live, "CPU: 25.0% · Resident: 64.0 MiB");
    // It really exits.
    h.refresh();
    let tombstone = h.view.last_seen().get(&key(300, "w", host())).unwrap();
    assert_eq!(tombstone.departure(), Departure::Exited(Exit::Unlisted));
    assert_eq!(
        tombstone.last_observed(),
        Some(SystemTime::UNIX_EPOCH + Duration::from_secs(EPOCH + 3))
    );
    assert_eq!(
        h.status(),
        format!(
            "Selected process: PID 300 · worker · exited: a scan that accounts for its PID no longer lists it · last observed {}; a historical record: no action applies to it",
            at(3)
        )
    );
    assert_eq!(
        h.inspector()[3],
        format!("Last sampled · {live} · last observed {}", at(3))
    );
}

/// Kills M8a: only watched processes leave records. Seventy unwatched
/// processes exiting in one scan with the target never push its record out.
#[test]
fn a_mass_exit_of_unwatched_processes_keeps_the_watched_record() {
    let mut first = vec![record(1, "init", "init"), record(300, "w", "worker")];
    first.extend((0..70u32).map(|n| record(2000 + n, &format!("u{n}"), &format!("u{n}"))));
    let mut h = Harness::new(vec![
        complete(first),
        complete(vec![record(1, "init", "init")]),
    ]);
    h.select(300, "worker");
    h.refresh();
    assert_eq!(h.view.last_seen().len(), 1);
    assert_eq!(
        h.view
            .last_seen()
            .get(&key(300, "w", host()))
            .map(|tombstone| tombstone.departure()),
        Some(Departure::Exited(Exit::Unlisted))
    );
    assert_eq!(h.view.last_seen().dropped(), 0);
}

/// Kills M12: a detail read that found the record gone establishes the exit
/// that a scan which only evicted the row cannot.
#[test]
fn a_detail_read_establishes_an_exit_the_scan_could_not() {
    let mut h = Harness::with(
        vec![
            complete(vec![
                record(1, "init", "init"),
                record(300, "w", "worker"),
                record(301, "o", "other"),
            ]),
            unenumerable(vec![
                record(1, "init", "init"),
                record(400, "n", "new"),
                record(401, "m", "newer"),
            ]),
        ],
        |view| view.with_published_limit(3),
    );
    h.select(300, "worker");
    let request = DetailRequest {
        key: key(300, "w", host()),
        generation: h.view.inspector().generation(),
    };
    h.view
        .details_read(&h.session, &request, DetailRead::Ended)
        .unwrap();
    assert!(h.status().contains("exited: its record was gone when its details were read · its row leaves with the next sample"), "{}", h.status());
    h.refresh();
    assert_eq!(
        h.view
            .last_seen()
            .get(&key(300, "w", host()))
            .map(|tombstone| tombstone.departure()),
        Some(Departure::Exited(Exit::DetailRead))
    );
    assert!(
        h.status()
            .contains("· exited: its record was gone when its details were read · last observed "),
        "{}",
        h.status()
    );
}

fn thread_record(pid: u32, ptoken: &str, tid: u32, ttoken: &str, name: &str) -> ProcessRecord {
    let mut thread = record(pid, ptoken, name);
    thread.key.thread = Some(ThreadIdentity {
        tid,
        creation: CreationToken::Opaque(ttoken.into()),
    });
    thread.kind = Observed::Known(TaskKind::Thread);
    thread.lineage = Lineage::new(pid, u64::from(tid));
    thread
}

/// The threads one scan read; `unreadable` names processes whose threads it
/// could not read.
fn threads_read(records: Vec<ProcessRecord>, unreadable: &[u32]) -> ThreadScan {
    let mut read = ThreadRecords::with_limit(16);
    read.records = records;
    for pid in unreadable {
        read.unreadable.record(*pid);
    }
    ThreadScan::Read(read)
}

fn server() -> ProcessRecord {
    let mut process = record(200, "srv", "server");
    process.kind = Observed::Known(TaskKind::Process {
        threads: Observed::Known(2),
    });
    process
}

fn t200() -> ProcessRecord {
    thread_record(200, "srv", 200, "t200", "server")
}

fn t201() -> ProcessRecord {
    thread_record(200, "srv", 201, "t201", "worker-1")
}

/// A view over `scans` with user threads shown, its first sample taken, and
/// `worker-1` (TID 201) selected.
fn threads_shown(
    scans: Vec<(Step, ThreadScan)>,
    adjust: impl FnOnce(ProcessView) -> ProcessView,
) -> Harness {
    let (steps, threads): (Vec<Step>, Vec<ThreadScan>) = scans.into_iter().unzip();
    let session = Session::mint();
    let mut source = Script::new(steps);
    source.threads = threads;
    let (view, _) = ProcessView::start(&session, &mut source).unwrap();
    let mut h = Harness {
        session,
        view: adjust(view),
        source,
    };
    h.view.set_user_threads(&h.session, Some(true)).unwrap();
    h.refresh();
    let item = h.session.with_store(|store| {
        store
            .get_model(MODEL)
            .unwrap()
            .items
            .values()
            .find(|item| matches!(&item.value, Value::List(cells) if cells[1] == Value::String("worker-1".into())))
            .map(|item| item.item_id)
    });
    h.view
        .select_item(&h.session, Some(item.expect("worker-1 is shown")))
        .unwrap();
    h
}

fn thread_departure(h: &Harness) -> Option<Departure> {
    h.view
        .last_seen()
        .iter()
        .find(|tombstone| tombstone.key().is_thread())
        .map(|tombstone| tombstone.departure())
}

/// Kills M11: a sample that read no threads says nothing about a thread.
#[test]
fn a_thread_dropped_by_a_sample_that_read_no_threads_is_not_exited() {
    let base = || complete(vec![record(1, "init", "init"), server()]);
    let mut h = threads_shown(
        vec![
            (base(), threads_read(vec![t200(), t201()], &[])),
            (base(), threads_read(vec![t200(), t201()], &[])),
            (base(), ThreadScan::NotProvided),
        ],
        |view| view,
    );
    h.refresh();
    assert_eq!(
        thread_departure(&h),
        Some(Departure::NotObserved(Unobserved::ThreadsNotRead))
    );
    assert!(
        h.status().contains(
            "no longer observed: the sample read no threads; whether it exited is unknown"
        ),
        "{}",
        h.status()
    );
}

/// Kills M10: a thread kept because its process's threads could not be
/// read, then left out at the row limit, is not exited.
#[test]
fn a_thread_kept_by_the_thread_ledger_then_left_out_is_not_exited() {
    let mut h = threads_shown(
        vec![
            (
                complete(vec![record(1, "init", "init"), server()]),
                threads_read(vec![t200(), t201()], &[]),
            ),
            (
                complete(vec![record(1, "init", "init"), server()]),
                threads_read(vec![t200(), t201()], &[]),
            ),
            (
                complete(vec![
                    record(1, "init", "init"),
                    server(),
                    record(400, "n", "new"),
                ]),
                threads_read(vec![t200()], &[200]),
            ),
        ],
        |view| view.with_published_limit(4),
    );
    h.refresh();
    assert_eq!(
        thread_departure(&h),
        Some(Departure::NotObserved(Unobserved::Evicted))
    );
}

/// Kills M13: a thread whose process is no longer observed is not exited.
#[test]
fn a_thread_of_a_process_no_longer_observed_is_not_exited() {
    let elsewhere = |mut record: ProcessRecord| {
        record.key.pid_namespace = Observed::Known(PidNamespaceId(99));
        record
    };
    let base = || vec![record(1, "init", "init"), server()];
    let mut h = threads_shown(
        vec![
            (complete(base()), threads_read(vec![t200(), t201()], &[])),
            (complete(base()), threads_read(vec![t200(), t201()], &[])),
            (
                complete(base().into_iter().map(elsewhere).collect()),
                threads_read(vec![elsewhere(t200()), elsewhere(t201())], &[]),
            ),
        ],
        |view| view,
    );
    h.refresh();
    assert_eq!(
        thread_departure(&h),
        Some(Departure::NotObserved(Unobserved::WithProcess))
    );
}

// ---- Correction round 2 ----

impl Harness {
    /// The tag cell of the shown row with `pid` and `name` (PX-020).
    fn tag_cell(&self, pid: u32, name: &str) -> Option<String> {
        self.session.with_store(|store| {
            store
                .get_model(MODEL)
                .unwrap()
                .items
                .values()
                .find_map(|item| match &item.value {
                    Value::List(cells)
                        if cells[0] == Value::UnsignedInt(u64::from(pid))
                            && cells[1] == Value::String(name.into()) =>
                    {
                        match cells.last() {
                            Some(Value::String(tag)) => Some(tag.clone()),
                            _ => None,
                        }
                    }
                    _ => None,
                })
        })
    }
}

/// F4: a tagged process left out at the row limit keeps a historical tag
/// that says "no longer observed"; when a scan lists the very same instance
/// again, the tag is live again, on its row, and Clear counts it as a tag.
#[test]
fn a_tag_whose_process_is_listed_again_is_live_again() {
    let mut h = Harness::with(
        vec![
            complete(vec![
                record(1, "init", "init"),
                record(300, "w", "worker"),
                record(301, "o", "other"),
            ]),
            unenumerable(vec![
                record(1, "init", "init"),
                record(400, "n", "new"),
                record(401, "m", "newer"),
            ]),
            complete(vec![
                record(1, "init", "init"),
                record(300, "w", "worker"),
                record(301, "o", "other"),
            ]),
        ],
        |view| view.with_published_limit(3),
    );
    h.select(300, "worker");
    h.view.request_tags(&h.session, TagRequest::Toggle).unwrap();
    h.select(1, "init");
    h.refresh();
    assert_eq!(h.view.tags().tagged(), 0);
    assert!(
        h.view.tags_status().starts_with(&format!(
            "Tags: 0 tagged · 1 historical: PID 300 · worker no longer observed, last observed {}",
            at(0)
        )),
        "{}",
        h.view.tags_status()
    );
    // Listed again under the same whole key: tagged again, on its new row.
    h.refresh();
    assert_eq!(h.view.tags().tagged(), 1);
    assert!(h.view.historical_tags().is_empty());
    assert_eq!(h.view.last_seen().forgotten_tags(), 0);
    assert!(h.view.tags().is_tagged(&key(300, "w", host())));
    assert_eq!(h.tag_cell(300, "worker").as_deref(), Some("tagged"));
    assert_eq!(
        h.view.tags_status(),
        "Tags: 1 tagged · Tagged again: PID 300 · worker is listed again after it was no longer observed"
    );
    h.view.request_tags(&h.session, TagRequest::Clear).unwrap();
    assert!(h
        .view
        .tags_status()
        .ends_with("Cleared 1 tag and 0 historical; the selection is unchanged"));
    assert_eq!(h.tag_cell(300, "worker").as_deref(), Some(""));
}

/// F4: a tag whose record says the process exited is never live again, even
/// when a source that reuses its own tokens lists the same key again.
#[test]
fn an_exited_tag_stays_historical_when_its_key_is_listed_again() {
    let mut h = Harness::new(vec![
        complete(vec![record(1, "init", "init"), record(300, "w", "worker")]),
        complete(vec![record(1, "init", "init")]),
        complete(vec![record(1, "init", "init"), record(300, "w", "worker")]),
    ]);
    h.select(300, "worker");
    h.view.request_tags(&h.session, TagRequest::Toggle).unwrap();
    h.select(1, "init");
    h.refresh();
    h.refresh();
    assert_eq!(h.view.tags().tagged(), 0);
    assert_eq!(h.view.historical_tags(), ["PID 300 · worker"]);
    assert_eq!(h.tag_cell(300, "worker").as_deref(), Some(""));
    assert!(h
        .view
        .tags_status()
        .starts_with("Tags: 0 tagged · 1 historical: PID 300 · worker exited"));
}

/// F5 (kills R6): a newer process under the PID is another instance; the
/// record of the one left out at the row limit survives it.
#[test]
fn a_newer_process_under_the_pid_never_drops_the_old_record() {
    let mut h = Harness::with(
        vec![
            complete(vec![
                record(1, "init", "init"),
                record(300, "w", "worker"),
                record(301, "o", "other"),
            ]),
            unenumerable(vec![
                record(1, "init", "init"),
                record(400, "n", "new"),
                record(401, "m", "newer"),
            ]),
            complete(vec![
                record(1, "init", "init"),
                record(300, "x", "successor"),
                record(301, "o", "other"),
            ]),
        ],
        |view| view.with_published_limit(3),
    );
    h.select(300, "worker");
    h.refresh();
    h.refresh();
    assert!(h.item(300, "successor").is_some());
    assert_eq!(
        h.view
            .last_seen()
            .get(&key(300, "w", host()))
            .map(|tombstone| tombstone.departure()),
        Some(Departure::NotObserved(Unobserved::Evicted)),
        "the earlier instance's record stands"
    );
    assert!(
        h.status()
            .starts_with("Selected process: PID 300 · worker · no longer observed: "),
        "{}",
        h.status()
    );
}

/// F5 (kills R7): a thread left out at the row limit and then listed again
/// was observed after all: its record goes.
#[test]
fn a_thread_listed_again_drops_its_no_longer_observed_record() {
    let mut h = threads_shown(
        vec![
            (
                complete(vec![record(1, "init", "init"), server()]),
                threads_read(vec![t200(), t201()], &[]),
            ),
            (
                complete(vec![record(1, "init", "init"), server()]),
                threads_read(vec![t200(), t201()], &[]),
            ),
            (
                complete(vec![
                    record(1, "init", "init"),
                    server(),
                    record(400, "n", "new"),
                ]),
                threads_read(vec![t200()], &[200]),
            ),
            (
                complete(vec![record(1, "init", "init"), server()]),
                threads_read(vec![t200(), t201()], &[]),
            ),
        ],
        |view| view.with_published_limit(4),
    );
    h.refresh();
    assert_eq!(
        thread_departure(&h),
        Some(Departure::NotObserved(Unobserved::Evicted))
    );
    h.refresh();
    assert_eq!(thread_departure(&h), None, "observed again");
    assert!(
        h.status().starts_with("Selected thread: TID 201 of PID 200 · worker-1 · listed again after it was no longer observed"),
        "{}",
        h.status()
    );
}

/// N4 (kills E5): a paused view takes no sample, so its records do not age
/// out until it is resumed.
#[test]
fn a_paused_view_expires_nothing_until_it_resumes() {
    let clock = Arc::new(FakeClock(Mutex::new(Duration::from_secs(10))));
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
    h.view.pause_view(&h.session).unwrap();
    clock.set(Duration::from_secs(10) + MAX_TOMBSTONE_AGE * 2);
    h.refresh();
    assert_eq!(h.view.last_seen().len(), 1, "paused: nothing expires");
    assert!(h.status().contains("exited"), "{}", h.status());
    h.view.resume_view(&h.session).unwrap();
    h.refresh();
    assert!(
        h.view.last_seen().is_empty(),
        "resumed: the first sample expires it"
    );
}

/// F4: a tag made live again counts against the tag limit; with no room
/// left, its record (no longer true) goes, the tag is counted forgotten, and
/// the status says so.
#[test]
fn a_relisted_tag_beyond_the_tag_limit_is_counted_forgotten() {
    let max = srui_process_explorer::tags::MAX_TAGGED as u32;
    let unknown = || Observed::Missing(MissingReason::Denied);
    let children = |host: &dyn Fn() -> Observed<HostId>| -> Vec<ProcessRecord> {
        (0..max - 1)
            .map(|n| record_on(10_000 + n, &format!("c{n}"), &format!("c{n}"), host()))
            .collect()
    };
    let mut first = vec![record(1, "init", "init"), record(300, "w", "worker")];
    first.extend(children(&host));
    // The host identity unread and PID 300 absent: no longer observed.
    let mut second = vec![record_on(1, "init", "init", unknown())];
    second.extend(children(&unknown));
    let mut h = Harness::new(vec![
        complete(first.clone()),
        complete(second),
        complete(first),
    ]);
    h.select(300, "worker");
    h.view.request_tags(&h.session, TagRequest::Toggle).unwrap();
    h.select(1, "init");
    h.refresh();
    assert_eq!(h.view.historical_tags(), ["PID 300 · worker"]);
    assert_eq!(
        h.view
            .last_seen()
            .get(&key(300, "w", host()))
            .map(|tombstone| tombstone.departure()),
        Some(Departure::NotObserved(Unobserved::IdentityUnconfirmed))
    );
    // Fill the tag set: init and its children are the limit.
    h.select(1, "init");
    h.view
        .request_tags(&h.session, TagRequest::Descendants)
        .unwrap();
    assert_eq!(h.view.tags().tagged(), max as usize);
    h.refresh();
    assert_eq!(
        h.view.tags().tagged(),
        max as usize,
        "no room: not tagged again"
    );
    assert!(h.view.historical_tags().is_empty());
    assert_eq!(h.view.last_seen().forgotten_tags(), 1);
    assert_eq!(h.tag_cell(300, "worker").as_deref(), Some(""));
    assert_eq!(
        h.view.tags_status(),
        format!("Tags: {max} tagged · 1 historical, forgotten · PID 300 · worker listed again, but {max} processes are already tagged, the limit: its historical tag is forgotten")
    );
}
