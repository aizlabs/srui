//! PX-013 acceptance: the selected-process inspector (D1 §§6.2–6.3, 7.6, 8,
//! 12, 22.5, 29; D2 T9–T11, T16, T21).
//!
//! * A selection is untrusted input: its `ItemId` is resolved against the rows
//!   the server publishes, and an unknown, stale or malformed one is refused
//!   with a defined state and no detail read at all.
//! * Details are read asynchronously, tagged with the target's `ProcessKey`
//!   and selection generation, and a slow read of an earlier selection never
//!   overwrites a later one.
//! * The inspector stays on its process instance when sorting moves its row,
//!   and a reused PID's new record is never shown.
//! * It follows a search promotion like a click, a later click wins, and a
//!   filter that hides the target (and so clears the promotion) shows the
//!   target as not shown, never another process.
//! * No selection, loading, permission denied, ended and not provided are
//!   distinct, and the detail worker is bounded and ends with its view.

use srui_process_explorer::catalogue::ProcessField;
use srui_process_explorer::command::CommandContext;
use srui_process_explorer::inspector::{
    DetailRead, DetailReadsIdle, DetailSource, ProcessDetails, ProcessState, StartTime,
    CLEARED_SELECTION_STATUS, MAX_INSPECTOR_LINE_BYTES, NO_SELECTION_STATUS,
};
use srui_process_explorer::owner::Accounts;
use srui_process_explorer::refresh::{lock_view, ProcessView, SharedView};
use srui_process_explorer::source::*;
use srui_process_explorer::{
    install_inspector, install_search_handlers, start_from_source, INSPECTOR_DETAILS,
    INSPECTOR_METRICS, INSPECTOR_PROCESS, INSPECTOR_STATUS, MODEL, TABLE,
};
use srui_protocol::Event as WireEvent;
use srui_sdk::{ItemId, NodeId, Operation, PropertyRef, TypeRef, Value, TEXT};
use srui_semantic_tree::Event as DomainEvent;
use srui_sessiond::{EventOutcome, Session};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime};

const SOURCE: &str = "inspector-fixture-v1";
const CLIENT: &[u8] = b"inspector-client-instance";
const BOOT: u64 = 1_799_990_000;
const WAIT: Duration = Duration::from_secs(10);

fn key(pid: u32, token: &str) -> ProcessKey {
    ProcessKey {
        source: SourceId(SOURCE.into()),
        host: Observed::Known(HostId("fixture-host".into())),
        boot: Observed::Known(BootId("fixture-boot".into())),
        pid_namespace: Observed::Known(PidNamespaceId(4_026_531_836)),
        pid: Observed::Known(pid),
        creation: CreationToken::Opaque(token.into()),
    }
}

fn record(pid: u32, token: &str, name: &str, owner: Observed<u32>, resident: u64) -> ProcessRecord {
    ProcessRecord {
        owner,
        key: key(pid, token),
        display_name: DisplayName::from(name),
        resident: Observed::Known(resident),
        cpu: CpuUsage::Measured(CpuInterval {
            ticks: 25,
            ticks_per_second: 100,
            elapsed: Duration::from_secs(1),
        }),
    }
}

/// Four processes: alpha (UID 1000, the fixture's own account), beta (UID
/// 1001), gamma (owner denied) and delta.
fn four() -> Vec<ProcessRecord> {
    vec![
        record(100, "a", "alpha", Observed::Known(1000), 1 << 20),
        record(200, "b", "beta", Observed::Known(1001), 2048),
        record(
            300,
            "c",
            "gamma",
            Observed::Missing(MissingReason::Denied),
            0,
        ),
        record(400, "d", "delta", Observed::Known(1000), 4096),
    ]
}

struct Fixture {
    records: Vec<ProcessRecord>,
    completeness: Completeness,
}

impl ProcessSource for Fixture {
    fn status_text(&self) -> &str {
        "inspector fixture"
    }

    fn accounts(&self) -> Accounts {
        Accounts::fixed(1000, &[(1000, "alice")])
    }

    fn snapshot(&mut self) -> ProcessSnapshot {
        ProcessSnapshot {
            sampled_at: SnapshotTime(SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000)),
            source: SourceId(SOURCE.into()),
            records: self.records.clone(),
            vanished: 0,
            capped: CappedRecords::none(),
            system: SystemSample::not_provided(),
            completeness: self.completeness.clone(),
        }
    }
}

fn read(state: u8, parent: u32, after_boot_seconds: u64) -> DetailRead {
    DetailRead::Read(ProcessDetails {
        state: Observed::Known(ProcessState::from_code(state).unwrap()),
        parent: Observed::Known(parent),
        started: Observed::Known(StartTime::from_ticks(
            after_boot_seconds * 100,
            100,
            Observed::Known(BOOT),
        )),
        command: CommandContext::missing(MissingReason::Unavailable),
    })
}

/// A deterministic detail source that counts every read, in order, and can
/// hold one token's read until the test releases it.
struct Probe {
    calls: Mutex<Vec<String>>,
    entered: Condvar,
    held: Mutex<Option<(String, mpsc::Receiver<()>)>>,
}

impl Probe {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            entered: Condvar::new(),
            held: Mutex::new(None),
        })
    }

    /// Holds the next read of `token` until the returned sender is used or
    /// dropped.
    fn hold(&self, token: &str) -> mpsc::Sender<()> {
        let (release, wait) = mpsc::channel();
        *self.held.lock().unwrap() = Some((token.to_string(), wait));
        release
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    /// Waits until the worker has entered `count` reads.
    fn wait_for_calls(&self, count: usize) {
        let deadline = Instant::now() + WAIT;
        let mut calls = self.calls.lock().unwrap();
        while calls.len() < count {
            let left = deadline
                .checked_duration_since(Instant::now())
                .expect("the worker entered the read in time");
            calls = self.entered.wait_timeout(calls, left).unwrap().0;
        }
    }
}

impl DetailSource for Probe {
    fn read_details(&self, target: &ProcessKey) -> DetailRead {
        let CreationToken::Opaque(token) = &target.creation else {
            panic!("the fixture mints only opaque tokens")
        };
        self.calls.lock().unwrap().push(token.clone());
        self.entered.notify_all();
        let held = {
            let mut held = self.held.lock().unwrap();
            match held.take() {
                Some((wanted, wait)) if wanted == *token => Some(wait),
                other => {
                    *held = other;
                    None
                }
            }
        };
        if let Some(wait) = held {
            let _ = wait.recv();
        }
        match token.as_str() {
            "a" => read(b'S', 1, 4_000),
            "b" => read(b'R', 100, 9_000),
            "c" => DetailRead::Unreadable(MissingReason::Denied),
            "d" => read(b'I', 2, 100),
            // An instance the source finds gone, or whose PID now names another
            // instance, while its row is still published.
            "e" => DetailRead::Ended,
            // The record of a reused PID: if it were ever shown, the test
            // would see a zombie.
            _ => read(b'Z', 999, 1),
        }
    }
}

struct Harness {
    session: Arc<Session>,
    view: SharedView,
    source: Fixture,
    idle: Option<DetailReadsIdle>,
}

fn start(records: Vec<ProcessRecord>, probe: Option<&Arc<Probe>>) -> Harness {
    start_limited(records, probe, usize::MAX)
}

/// Asks the account-name cache (PX-012) about every owner in `records` and
/// waits for its worker to answer them all, before anything is selected. The
/// inspector's owner text then names each owner as the fixture database
/// answers it (`alice`, or `no account name`) on its first publication and
/// every later one, instead of depending on whether that worker answered
/// before or after a given commit. No test here is about name-lookup delay;
/// that is PX-012's `a_delayed_name_lookup_never_stalls_publication`.
fn settle_names(view: &ProcessView, records: &[ProcessRecord]) {
    let names = view.owner_filter().names();
    for record in records {
        if let Observed::Known(uid) = record.owner {
            let _ = names.name(uid);
        }
    }
    assert!(names.wait_idle(WAIT), "the fixture names were answered");
}

/// [`start`] with the published-row ceiling lowered to `limit`.
fn start_limited(records: Vec<ProcessRecord>, probe: Option<&Arc<Probe>>, limit: usize) -> Harness {
    let session = Arc::new(Session::mint());
    let mut source = Fixture {
        records,
        completeness: Completeness::Complete,
    };
    let (view, _) = start_from_source(&session, &mut source).unwrap();
    settle_names(&view, &source.records);
    let view = view.with_published_limit(limit);
    let view: SharedView = Arc::new(Mutex::new(view));
    install_search_handlers(&session, &view);
    let details = probe.map(|probe| Arc::clone(probe) as Arc<dyn DetailSource>);
    let idle = install_inspector(&session, &view, details);
    Harness {
        session,
        view,
        source,
        idle,
    }
}

impl Harness {
    fn view(&self) -> std::sync::MutexGuard<'_, ProcessView> {
        lock_view(&self.view)
    }

    fn idle(&self) {
        assert!(
            self.idle.as_ref().expect("a detail worker").wait_idle(WAIT),
            "the detail worker settled"
        );
    }

    fn refresh(&mut self) {
        let mut view = lock_view(&self.view);
        view.refresh(&self.session, &mut self.source).unwrap();
    }

    fn item_of(&self, pid: u64) -> ItemId {
        self.rows()
            .into_iter()
            .find(|(_, value)| {
                matches!(value, Value::List(cells) if cells[ProcessField::Pid.index()] == Value::UnsignedInt(pid))
            })
            .map(|(id, _)| id)
            .unwrap_or_else(|| panic!("no published row has PID {pid}"))
    }

    fn rows(&self) -> Vec<(ItemId, Value)> {
        self.session.with_store(|store| {
            store
                .get_model(MODEL)
                .unwrap()
                .items
                .values()
                .map(|item| (item.item_id, item.value.clone()))
                .collect()
        })
    }

    /// Sends a `SELECTION_CHANGED` carrying exactly `arguments`, as a client
    /// would, and expects it processed.
    fn select_with(&self, seq: u64, arguments: Vec<(PropertyRef, Value)>) {
        let event: WireEvent = DomainEvent::new(
            None,
            seq,
            format!("inspector-select-{seq}"),
            self.session.current_revision(),
            TABLE,
            TypeRef::EVENT_SELECTION_CHANGED,
            arguments,
        )
        .with_client_instance_id(CLIENT.to_vec())
        .to_wire();
        let outcome = self.session.process_event(&event).expect("event admitted");
        assert!(
            matches!(outcome, EventOutcome::Processed { .. }),
            "not processed: {outcome:?}"
        );
    }

    fn select(&self, seq: u64, item: ItemId) {
        self.select_with(seq, vec![(PropertyRef::VALUE, Value::ItemId(item))]);
    }

    fn text(&self, node: NodeId) -> String {
        self.session.with_store(|store| {
            match store.get_node(node).and_then(|n| n.get_property(TEXT)) {
                Some(Value::String(text)) => text.clone(),
                other => panic!("node {node:?} has no text: {other:?}"),
            }
        })
    }

    /// The four inspector lines the client holds, which must also be the
    /// view's record of them.
    fn lines(&self) -> [String; 4] {
        let held = [
            self.text(INSPECTOR_STATUS),
            self.text(INSPECTOR_PROCESS),
            self.text(INSPECTOR_DETAILS),
            self.text(INSPECTOR_METRICS),
        ];
        let view = self.view();
        let recorded = view.inspector_view();
        assert_eq!(
            [
                &recorded.status,
                &recorded.process,
                &recorded.details,
                &recorded.metrics
            ],
            [&held[0], &held[1], &held[2], &held[3]],
            "the view records what committed"
        );
        held
    }

    /// Every inspector text committed since `revision`.
    fn inspector_texts_since(&self, revision: u64) -> Vec<String> {
        self.session
            .collect_replayed_transactions(revision)
            .unwrap()
            .into_iter()
            .flat_map(|transaction| transaction.operations)
            .map(|op| Operation::try_from(op).unwrap())
            .filter_map(|op| match op {
                Operation::SetProperty {
                    id,
                    value: Value::String(text),
                    ..
                } if [
                    INSPECTOR_STATUS,
                    INSPECTOR_PROCESS,
                    INSPECTOR_DETAILS,
                    INSPECTOR_METRICS,
                ]
                .contains(&id) =>
                {
                    Some(text)
                }
                _ => None,
            })
            .collect()
    }
}

/// The started line for a fixture `after_boot` seconds after [`BOOT`].
fn started(after_boot: u64) -> String {
    srui_process_explorer::metric::format_utc(
        SystemTime::UNIX_EPOCH + Duration::from_secs(BOOT + after_boot),
    )
    .unwrap()
}

#[test]
fn a_slow_read_of_an_earlier_selection_never_overwrites_a_later_one() {
    let probe = Probe::new();
    let release = probe.hold("a");
    let harness = start(four(), Some(&probe));
    let (a, b) = (harness.item_of(100), harness.item_of(200));

    harness.select(1, a);
    assert_eq!(
        harness.lines()[0],
        "Selected process: PID 100 · alpha · loading details"
    );
    // The worker is inside A's read, which the test holds.
    probe.wait_for_calls(1);
    let before_b = harness.session.current_revision();
    harness.select(2, b);
    let [status, process, details, metrics] = harness.lines();
    assert_eq!(status, "Selected process: PID 200 · beta · loading details");
    assert!(
        process.starts_with("Name: beta · PID 200 · Owner: UID 1001"),
        "{process}"
    );
    assert_eq!(details, "State, parent and start time: loading");
    // CPU and resident memory are the server's own row, shown at once.
    assert_eq!(metrics, "CPU: 25.0% · Resident: 2.0 KiB");
    assert_eq!(probe.calls(), ["a"], "B waits behind A's read");

    // A's read now completes, late: it answers an earlier generation and is
    // dropped; B's read follows and is published.
    release.send(()).unwrap();
    harness.idle();
    let [status, _, details, _] = harness.lines();
    assert_eq!(status, "Selected process: PID 200 · beta");
    assert_eq!(
        details,
        format!(
            "State: R (running) · Parent: PID 100 · Started: {} (server clock)",
            started(9_000)
        )
    );
    assert_eq!(probe.calls(), ["a", "b"]);
    assert_eq!(harness.view().inspector().stale_results(), 1);
    assert_eq!(harness.view().inspector().generation().get(), 2);
    assert_eq!(harness.view().inspector().selected(), Some(b));
    // Nothing of A was ever committed after B was selected.
    for text in harness.inspector_texts_since(before_b) {
        assert!(
            !text.contains("alpha")
                && !text.contains("Selected process: PID 100")
                && !text.contains("sleeping")
                && !text.contains(&started(4_000)),
            "A's details reached the client after B was selected: {text}"
        );
    }
}

#[test]
fn the_inspector_stays_on_its_process_when_sorting_moves_its_row_and_never_shows_a_reused_pid() {
    let probe = Probe::new();
    let mut harness = start(four(), Some(&probe));
    let b = harness.item_of(200);
    harness.select(1, b);
    harness.idle();
    let selected = harness.lines();
    assert_eq!(selected[0], "Selected process: PID 200 · beta");
    // The fixture database has no entry for UID 1001, answered before the
    // selection (`settle_names`), so this line cannot change under the sort.
    assert_eq!(
        selected[1],
        "Name: beta · PID 200 · Owner: UID 1001 (no account name)"
    );
    let ids_before: Vec<ItemId> = harness.rows().iter().map(|(id, _)| *id).collect();
    assert_eq!(ids_before.iter().position(|id| *id == b), Some(1));

    // PID descending moves B from row 1 to row 2 under its own item ID; the
    // inspector neither changes nor reads again.
    let revision = harness.session.current_revision();
    harness
        .view()
        .click_sort(&harness.session, ProcessField::Pid)
        .unwrap();
    harness
        .view()
        .click_sort(&harness.session, ProcessField::Pid)
        .unwrap();
    let ids_after: Vec<ItemId> = harness.rows().iter().map(|(id, _)| *id).collect();
    assert_eq!(ids_after.iter().position(|id| *id == b), Some(2));
    assert_eq!(harness.lines(), selected);
    assert_eq!(
        harness.inspector_texts_since(revision),
        Vec::<String>::new()
    );
    assert_eq!(probe.calls(), ["b"]);

    // A refresh that changes B's memory moves its metrics, still for B, and
    // reads B's details again for that sample.
    harness.source.records[1].resident = Observed::Known(3 << 20);
    harness.refresh();
    harness.idle();
    let [status, _, _, metrics] = harness.lines();
    assert_eq!(status, "Selected process: PID 200 · beta");
    assert_eq!(metrics, "CPU: 25.0% · Resident: 3.0 MiB");
    assert_eq!(probe.calls(), ["b", "b"]);

    // B ends and a new process takes PID 200 in the same scan: a new key, so
    // a new item. The inspector says B ended and never reads or shows the
    // newcomer.
    harness.source.records[1] = record(200, "b2", "newcomer", Observed::Known(0), 1);
    harness.refresh();
    harness.idle();
    let newcomer = harness.item_of(200);
    assert_ne!(newcomer, b);
    let [status, process, details, metrics] = harness.lines();
    assert_eq!(
        status,
        "Selected process: PID 200 · beta · this process has ended; select another row"
    );
    assert!(process.starts_with("Name: beta · PID 200"), "{process}");
    assert_eq!(
        details,
        "State, parent and start time: none: the process has ended"
    );
    assert_eq!(
        metrics,
        "CPU and resident memory: none: the process has ended"
    );
    assert_eq!(probe.calls(), ["b", "b"], "the newcomer is never read");
    assert_eq!(harness.view().inspector().selected(), Some(b));

    // A detail read that finds the instance gone, or another instance under
    // its PID, is an end too, while the row is still published; it is not
    // read again on later samples.
    harness
        .source
        .records
        .push(record(500, "e", "epsilon", Observed::Known(1000), 1));
    harness.refresh();
    let e = harness.item_of(500);
    harness.select(2, e);
    harness.idle();
    assert_eq!(
        harness.lines()[0],
        "Selected process: PID 500 · epsilon · this process has ended; select another row"
    );
    harness.refresh();
    harness.idle();
    assert_eq!(probe.calls(), ["b", "b", "e"]);
}

/// A sort goes through the full republication path (PX-012 review round), so
/// it can change which rows the published-row ceiling keeps. A sort that drops
/// the selected row shows that same process as not shown, never the row that
/// took its place, and a sort that brings it back describes it again.
#[test]
fn a_sort_that_drops_the_selected_row_under_the_ceiling_shows_it_as_not_shown() {
    let probe = Probe::new();
    let mut harness = start_limited(four(), Some(&probe), 2);
    // The lowered ceiling applies from the next publication: the leading two
    // rows, PIDs 100 and 200, are kept.
    harness.refresh();
    let kept: Vec<ItemId> = harness.rows().iter().map(|(id, _)| *id).collect();
    assert_eq!(kept, [harness.item_of(100), harness.item_of(200)]);
    let b = harness.item_of(200);
    harness.select(1, b);
    harness.idle();
    let selected = harness.lines();
    assert_eq!(selected[0], "Selected process: PID 200 · beta");
    let calls = probe.calls();

    // PID ascending keeps the same window; descending keeps 400 and 300, and
    // drops B under the ceiling in the same plan.
    let revision = harness.session.current_revision();
    harness
        .view()
        .click_sort(&harness.session, ProcessField::Pid)
        .unwrap();
    assert_eq!(harness.lines()[0], selected[0]);
    harness
        .view()
        .click_sort(&harness.session, ProcessField::Pid)
        .unwrap();
    let shown: Vec<ItemId> = harness.rows().iter().map(|(id, _)| *id).collect();
    assert_eq!(shown.len(), 2);
    assert!(!shown.contains(&b), "the ceiling dropped B");
    let [status, process, details, metrics] = harness.lines();
    assert_eq!(
        status,
        "Selected process: PID 200 · beta · not shown (filtered out or beyond the row limit)"
    );
    assert!(
        process.starts_with("Name: beta · PID 200 · Owner: UID 1001"),
        "{process}"
    );
    assert_eq!(details, "State, parent and start time: not shown");
    assert_eq!(metrics, "CPU and resident memory: not shown");
    assert_eq!(harness.view().inspector().selected(), Some(b));
    for text in harness.inspector_texts_since(revision) {
        for other in ["alpha", "gamma", "delta", "PID 300", "PID 400"] {
            assert!(
                !text.contains(other),
                "another process reached the inspector: {text}"
            );
        }
    }

    // Ascending again brings B back under its own item ID, described again.
    harness
        .view()
        .click_sort(&harness.session, ProcessField::Pid)
        .unwrap();
    assert_eq!(harness.item_of(200), b);
    assert_eq!(harness.lines()[0], "Selected process: PID 200 · beta");
    assert_eq!(probe.calls(), calls, "no sort read anything");
}

/// PX-013 review round, F1: a refresh that changes the selected instance's
/// owner (another UID, then denied) publishes the new owner, and no later
/// publication — a detail completion, a re-selection, a sort — ever brings an
/// older owner back. The owner lines committed for B are therefore exactly
/// the sequence of the owners the scans read, never oscillating.
#[test]
fn the_inspectors_owner_is_always_the_latest_scans_and_never_reverts() {
    let probe = Probe::new();
    let mut harness = start(four(), Some(&probe));
    let b = harness.item_of(200);
    harness.select(1, b);
    harness.idle();
    let pending = "Name: beta · PID 200 · Owner: UID 1001 (no account name)";
    assert_eq!(harness.lines()[1], pending);
    let revision = harness.session.current_revision();
    let mut seq = 2;
    let mut settle_and_check = |harness: &mut Harness, expected: &str| {
        assert_eq!(harness.lines()[1], expected, "after the refresh");
        // The refresh asked for B's details again; its completion publishes.
        harness.idle();
        assert_eq!(harness.lines()[1], expected, "after a detail completion");
        harness.select(seq, b);
        seq += 1;
        harness.idle();
        assert_eq!(harness.lines()[1], expected, "after a re-selection");
        harness
            .view()
            .click_sort(&harness.session, ProcessField::Name)
            .unwrap();
        assert_eq!(harness.lines()[1], expected, "after a sort");
        harness.idle();
        assert_eq!(harness.lines()[1], expected, "after everything settled");
        assert_eq!(harness.view().inspector().selected(), Some(b));
    };

    // The same instance (same key, same item) now runs as the fixture's own
    // account.
    let alice = "Name: beta · PID 200 · Owner: UID 1000 (alice)";
    let i = harness
        .source
        .records
        .iter()
        .position(|r| r.display_name.as_str() == "beta")
        .unwrap();
    harness.source.records[i].owner = Observed::Known(1000);
    harness.refresh();
    assert_eq!(harness.item_of(200), b);
    settle_and_check(&mut harness, alice);

    // Then its owner can no longer be read.
    let denied = "Name: beta · PID 200 · Owner: permission denied";
    harness.source.records[i].owner = Observed::Missing(MissingReason::Denied);
    harness.refresh();
    assert_eq!(harness.item_of(200), b);
    settle_and_check(&mut harness, denied);

    // A last change, with no re-selection after it, and then B ends: the
    // inspector shows the last scan's owner of it, never the one it had when
    // it was last selected.
    let other = "Name: beta · PID 200 · Owner: UID 4242 (no account name)";
    {
        let view = harness.view();
        let names = view.owner_filter().names();
        let _ = names.name(4242);
        assert!(names.wait_idle(WAIT));
    }
    harness.source.records[i].owner = Observed::Known(4242);
    harness.refresh();
    harness.idle();
    assert_eq!(harness.lines()[1], other);
    harness.source.records.remove(i);
    harness.refresh();
    assert_eq!(
        harness.lines()[..2],
        [
            "Selected process: PID 200 · beta · this process has ended; select another row"
                .to_string(),
            other.to_string(),
        ]
    );

    let mut owners: Vec<String> = harness
        .inspector_texts_since(revision)
        .into_iter()
        .filter(|text| text.starts_with("Name: beta"))
        .collect();
    owners.dedup();
    assert_eq!(
        owners,
        [alice, denied, other],
        "the owner never reverted or oscillated"
    );
}

/// PX-013 review round, F2: an incomplete scan that cannot account for the
/// selected process keeps its row, and when this scan's newly confirmed rows
/// leave no room for it under the published-row ceiling, retention drops it.
/// Nothing established an exit, so the inspector says it is not confirmed and
/// not shown, keeps its identity and never says "ended". Only a later scan
/// entitled to account for it may establish that.
#[test]
fn a_retained_row_the_ceiling_evicts_is_never_called_ended() {
    let probe = Probe::new();
    let mut harness = start_limited(four(), Some(&probe), 4);
    let b = harness.item_of(200);
    harness.select(1, b);
    harness.idle();
    assert_eq!(harness.lines()[0], "Selected process: PID 200 · beta");
    let revision = harness.session.current_revision();

    // An incomplete scan that cannot name what it missed: B is absent but
    // retained, and a new confirmed process takes the fourth place.
    let beta = harness.source.records.remove(1);
    harness
        .source
        .records
        .push(record(500, "f", "epsilon", Observed::Known(1000), 1));
    harness.source.completeness = Completeness::from_scan(
        SkippedRecords::unenumerable(),
        vec![EnumerationIssue {
            scope: IssueScope::Entry,
            reason: MissingReason::Denied,
            detail: "scripted unreadable entry".into(),
        }],
    );
    harness.refresh();
    harness.idle();
    assert_eq!(harness.rows().len(), 4);
    assert!(
        harness.rows().iter().all(|(id, _)| *id != b),
        "the ceiling dropped B"
    );
    let [status, process, details, metrics] = harness.lines();
    assert_eq!(
        status,
        "Selected process: PID 200 · beta · not confirmed by the latest scan; not shown (dropped under the row limit)"
    );
    assert_eq!(
        process,
        "Name: beta · PID 200 · Owner: UID 1001 (no account name)"
    );
    assert_eq!(details, "State, parent and start time: not shown");
    assert_eq!(metrics, "CPU and resident memory: not shown");
    assert_eq!(harness.view().inspector().selected(), Some(b));
    // The same incomplete scan again changes nothing.
    harness.refresh();
    assert!(harness.lines()[0].contains("not confirmed by the latest scan"));

    // A complete scan lists B again: the allocator forgot its item when it was
    // dropped, so it is a new row, which the inspector does not adopt.
    harness.source.completeness = Completeness::Complete;
    harness.source.records.insert(1, beta);
    harness.source.records.pop();
    harness.refresh();
    let again = harness.item_of(200);
    assert_ne!(again, b);
    assert_eq!(
        harness.lines()[0],
        "Selected process: PID 200 · beta · listed again as a new row after the row limit dropped it unconfirmed; select it again"
    );
    assert_eq!(harness.view().inspector().selected(), Some(b));
    for text in harness.inspector_texts_since(revision) {
        assert!(
            !text.contains("has ended"),
            "an eviction was called an exit: {text}"
        );
    }
    assert_eq!(probe.calls(), ["b"], "nothing was read for a row not shown");

    // A complete scan that no longer lists it accounts for its absence: now
    // the process has ended.
    harness.source.records.remove(1);
    harness.refresh();
    assert_eq!(
        harness.lines()[0],
        "Selected process: PID 200 · beta · this process has ended; select another row"
    );
}

#[test]
fn an_unknown_stale_or_malformed_selection_is_refused_without_a_detail_read() {
    let probe = Probe::new();
    let mut harness = start(four(), Some(&probe));

    // An item this server never published.
    harness.select(1, ItemId::new(999_999));
    let [status, process, details, metrics] = harness.lines();
    assert_eq!(
        status,
        "Selected process: none · item 999999 is not a row this server shows; nothing was read"
    );
    assert_eq!(
        [process, details, metrics],
        [String::new(), String::new(), String::new()]
    );

    // Not an item at all: a PID-shaped number and a string.
    for (seq, arguments) in [
        (2, vec![(PropertyRef::VALUE, Value::UnsignedInt(200))]),
        (3, vec![(PropertyRef::VALUE, Value::String("200".into()))]),
    ] {
        harness.select_with(seq, arguments);
        assert_eq!(
            harness.lines()[0],
            "Selected process: none · the selection named no row; nothing was read"
        );
    }

    // No value at all: the user left the table with nothing selected
    // (PX-017-G02). Nothing is read for it either.
    harness.select_with(4, vec![]);
    assert_eq!(harness.lines()[0], CLEARED_SELECTION_STATUS);

    // A row the filter hides is not published, so its item is refused too.
    let hidden = harness.item_of(300);
    harness
        .view()
        .set_filter(&harness.session, "alpha")
        .unwrap();
    harness.select(5, hidden);
    assert_eq!(
        harness.lines()[0],
        format!(
            "Selected process: none · item {} is not a row this server shows; nothing was read",
            hidden.get()
        )
    );
    harness.view().set_filter(&harness.session, "").unwrap();

    // A stale item: delta was published, then ended.
    let d = harness.item_of(400);
    harness.source.records.pop();
    harness.refresh();
    harness.select(6, d);
    assert!(harness.lines()[0].contains(&format!("item {} is not a row", d.get())));

    harness.idle();
    assert_eq!(
        probe.calls(),
        Vec::<String>::new(),
        "no refused selection read anything"
    );
    assert_eq!(harness.view().inspector().generation().get(), 6);
    assert_eq!(harness.view().inspector().selected(), None);
}

#[test]
fn the_inspector_follows_a_search_promotion_and_a_later_click_wins() {
    let probe = Probe::new();
    let harness = start(four(), Some(&probe));
    let (a, d) = (harness.item_of(100), harness.item_of(400));
    let promoted = || {
        harness.session.with_store(|store| {
            store
                .get_node(TABLE)
                .and_then(|node| node.promoted_selection())
        })
    };

    harness.view().set_search(&harness.session, "beta").unwrap();
    harness.idle();
    assert_eq!(promoted(), Some(harness.item_of(200)));
    assert_eq!(harness.lines()[0], "Selected process: PID 200 · beta");

    // A user's later click wins; the promotion itself is the server's and is
    // not touched by a click.
    harness.select(1, a);
    harness.idle();
    assert_eq!(harness.lines()[0], "Selected process: PID 100 · alpha");
    assert_eq!(promoted(), Some(harness.item_of(200)));

    // A later promotion wins over the click.
    harness
        .view()
        .set_search(&harness.session, "delta")
        .unwrap();
    harness.idle();
    assert_eq!(promoted(), Some(d));
    let [status, _, details, _] = harness.lines();
    assert_eq!(status, "Selected process: PID 400 · delta");
    assert!(
        details.starts_with("State: I (idle) · Parent: PID 2"),
        "{details}"
    );

    // A filter that hides delta clears the promotion in the same commit and
    // shows delta as not shown: never another process.
    let revision = harness.session.current_revision();
    harness
        .view()
        .set_filter(&harness.session, "alpha")
        .unwrap();
    assert_eq!(promoted(), None);
    assert_eq!(
        harness.lines(),
        [
            "Selected process: PID 400 · delta · not shown (filtered out or beyond the row limit)"
                .to_string(),
            "Name: delta · PID 400 · Owner: UID 1000 (alice)".to_string(),
            "State, parent and start time: not shown".to_string(),
            "CPU and resident memory: not shown".to_string(),
        ]
    );
    for text in harness.inspector_texts_since(revision) {
        assert!(
            !text.contains("alpha"),
            "another process reached the inspector: {text}"
        );
    }
    // Shown again, it is the same instance with the details read for it.
    harness.view().set_filter(&harness.session, "").unwrap();
    assert_eq!(harness.lines()[0], "Selected process: PID 400 · delta");
    assert_eq!(probe.calls(), ["b", "a", "d"]);
}

#[test]
fn no_selection_loading_denied_ended_and_not_provided_are_distinct() {
    let probe = Probe::new();
    let mut harness = start(four(), Some(&probe));
    assert_eq!(
        harness.lines(),
        [
            NO_SELECTION_STATUS.to_string(),
            String::new(),
            String::new(),
            String::new()
        ]
    );

    // Permission denied: the details, and separately the owner.
    let c = harness.item_of(300);
    harness.select(1, c);
    harness.idle();
    let [status, process, details, metrics] = harness.lines();
    assert_eq!(
        status,
        "Selected process: PID 300 · gamma · details: permission denied"
    );
    assert_eq!(process, "Name: gamma · PID 300 · Owner: permission denied");
    assert_eq!(details, "State, parent and start time: permission denied");
    assert_eq!(metrics, "CPU: 25.0% · Resident: 0 B");

    // A named owner: the cache answers on its own worker, and the next
    // publication shows the name.
    let a = harness.item_of(100);
    harness.select(2, a);
    harness.idle();
    assert!(harness.view().owner_filter().names().wait_idle(WAIT));
    harness.refresh();
    harness.idle();
    let [status, process, details, _] = harness.lines();
    assert_eq!(status, "Selected process: PID 100 · alpha");
    assert_eq!(process, "Name: alpha · PID 100 · Owner: UID 1000 (alice)");
    assert_eq!(
        details,
        format!(
            "State: S (sleeping) · Parent: PID 1 · Started: {} (server clock)",
            started(4_000)
        )
    );

    // Ended: the row leaves the collection.
    harness.source.records.remove(0);
    harness.refresh();
    assert_eq!(
        harness.lines()[0],
        "Selected process: PID 100 · alpha · this process has ended; select another row"
    );

    // A source without details still shows the row's own fields.
    let plain = start(four(), None);
    plain.select(1, plain.item_of(200));
    let [status, process, details, metrics] = plain.lines();
    assert_eq!(
        status,
        "Selected process: PID 200 · beta · this source provides no details"
    );
    assert!(
        process.starts_with("Name: beta · PID 200 · Owner: UID 1001"),
        "{process}"
    );
    assert_eq!(
        details,
        "State, parent and start time: not provided by this source"
    );
    assert_eq!(metrics, "CPU: 25.0% · Resident: 2.0 KiB");

    // Every line stays inside its bound, whatever the name.
    let wide = "\u{20000}".repeat(MAX_DISPLAY_NAME_CHARS);
    let widest = start(
        vec![record(
            u32::MAX,
            "w",
            &wide,
            Observed::Known(u32::MAX),
            u64::MAX,
        )],
        None,
    );
    widest.select(1, widest.item_of(u64::from(u32::MAX)));
    for line in widest.lines() {
        assert!(
            line.len() <= MAX_INSPECTOR_LINE_BYTES + '…'.len_utf8(),
            "{}",
            line.len()
        );
    }
}

#[test]
fn the_detail_worker_is_bounded_and_ends_with_its_view() {
    let probe = Probe::new();
    let release = probe.hold("a");
    let harness = start(four(), Some(&probe));
    let items: Vec<ItemId> = [100, 200, 300, 400, 200, 300]
        .into_iter()
        .map(|pid| harness.item_of(pid))
        .collect();
    harness.select(1, items[0]);
    probe.wait_for_calls(1);
    // While A's read is held, five more selections leave one request waiting:
    // each replaces the one before it.
    for (seq, item) in (2..).zip(&items[1..]) {
        harness.select(seq, *item);
    }
    assert_eq!(harness.idle.as_ref().unwrap().superseded(), 4);
    release.send(()).unwrap();
    harness.idle();
    assert_eq!(
        probe.calls(),
        ["a", "c"],
        "only the read in progress and the last one ran"
    );
    assert_eq!(
        harness.lines()[0],
        "Selected process: PID 300 · gamma · details: permission denied"
    );
    assert_eq!(harness.view().inspector().stale_results(), 1);

    // The worker holds the view and the session only weakly: dropping them
    // ends it, and it lets go of its source.
    let Harness {
        session,
        view,
        idle,
        ..
    } = harness;
    drop(view);
    drop(session);
    drop(idle);
    let deadline = Instant::now() + WAIT;
    while Arc::strong_count(&probe) > 1 {
        assert!(
            Instant::now() < deadline,
            "the detail worker ended with its view"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
