//! PX-009 acceptance: server-authoritative sorting of the published process
//! collection (D1 §§6.2, 7.6, 8; D2 T28).
//!
//! * Fixtures cover ties, missing values, numeric versus lexical ordering and
//!   changing CPU rankings, each checked on the collection a client holds.
//! * Selection remains on the same item after reordering: a client anchors its
//!   native selection to item IDs, so every process keeps its item ID through
//!   every reorder, and no committed transaction removes a row it does not put
//!   back — no revision a client can hold is missing a selected row.
//! * Stale query results cannot overwrite a newer sort request: a click is
//!   applied while a scan is still running, and that scan's result is published
//!   in the newer order; an older click replayed after a newer one changes
//!   nothing.
//!
//! The comparator's own cases are unit tests in `src/sort.rs`, and the move
//! planner's in `src/refresh.rs`; these tests drive the published session.

use srui_process_explorer::catalogue::{ProcessField, SortDirection};
use srui_process_explorer::refresh::{poll, ProcessView, Refreshed, MIN_REFRESH_INTERVAL};
use srui_process_explorer::sort::SortOrder;
use srui_process_explorer::source::*;
use srui_process_explorer::{
    initialize, install_sort_handlers, start_from_source, MODEL, SORT_CPU, SORT_NAME, SORT_PID,
    SORT_RESIDENT, TABLE,
};
use srui_protocol::{operation, Event as WireEvent};
use srui_sdk::{EnumToken, ItemId, NodeId, SelectionMode, Value, ENABLED, LABEL, SELECTION_MODE};
use srui_semantic_tree::Event as SemanticEvent;
use srui_sessiond::{EventOutcome, Session};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

const SOURCE: &str = "sort-fixture-v1";

fn key(pid: Option<u32>, token: &str) -> ProcessKey {
    ProcessKey {
        source: SourceId(SOURCE.into()),
        host: Observed::Known(HostId("fixture-host".into())),
        boot: Observed::Known(BootId("fixture-boot".into())),
        pid_namespace: Observed::Known(PidNamespaceId(4_026_531_836)),
        pid: pid.map_or(
            Observed::Missing(MissingReason::Unavailable),
            Observed::Known,
        ),
        creation: CreationToken::Opaque(token.into()),
    }
}

/// A share of one CPU measured over one second, in percent.
fn cpu(percent: u64) -> CpuUsage {
    CpuUsage::Measured(CpuInterval {
        ticks: percent,
        ticks_per_second: 100,
        elapsed: Duration::from_secs(1),
    })
}

fn record(
    pid: Option<u32>,
    token: &str,
    name: &str,
    resident: Observed<u64>,
    cpu: CpuUsage,
) -> ProcessRecord {
    ProcessRecord {
        key: key(pid, token),
        display_name: name.into(),
        resident,
        cpu,
    }
}

/// Six processes, in the order a procfs scan lists them (ascending PID, the
/// one whose PID could not be observed last):
///
/// | PID | name | resident | CPU |
/// | --- | --- | --- | --- |
/// | 9 | `zsh` | 9 B | 2.0% |
/// | 10 | `Xorg` | 1.0 KiB | 10.0% |
/// | 11 | `worker` | 1.0 KiB | unavailable |
/// | 100 | `a10` | denied | warming up |
/// | 1000 | `a9` | 2.0 KiB | 10.0% |
/// | unavailable | `a9` | 4.0 KiB | 0.0% (a measured zero) |
///
/// Their PIDs' texts sort `10 < 100 < 1000 < 11 < 9` and their memory cells'
/// `1.0 KiB < 2.0 KiB < 4.0 KiB < 9 B`, the opposite of their numbers; `a10`
/// sorts before `a9` and `Xorg` before every lower-case name; two names, two
/// memory values and two CPU shares tie; and every column has a value that is
/// not present.
fn population() -> Vec<ProcessRecord> {
    vec![
        record(Some(9), "zsh", "zsh", Observed::Known(9), cpu(2)),
        record(Some(10), "xorg", "Xorg", Observed::Known(1024), cpu(10)),
        record(
            Some(11),
            "worker",
            "worker",
            Observed::Known(1024),
            CpuUsage::Missing(MissingReason::Unavailable),
        ),
        record(
            Some(100),
            "a10",
            "a10",
            Observed::Missing(MissingReason::Denied),
            CpuUsage::WarmingUp,
        ),
        record(Some(1000), "a9-known", "a9", Observed::Known(2048), cpu(10)),
        record(None, "a9-hidden", "a9", Observed::Known(4096), cpu(0)),
    ]
}

fn snapshot(records: Vec<ProcessRecord>, second: u64) -> ProcessSnapshot {
    ProcessSnapshot {
        source: SourceId(SOURCE.into()),
        sampled_at: SnapshotTime(
            SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000 + second),
        ),
        records,
        vanished: 0,
        capped: CappedRecords::none(),
        completeness: Completeness::Complete,
        system: SystemSample::not_provided(),
    }
}

/// A source that publishes `steps` in turn, then repeats the last one.
struct Steps {
    steps: Vec<Vec<ProcessRecord>>,
    next: usize,
}

impl Steps {
    fn new(steps: Vec<Vec<ProcessRecord>>) -> Self {
        Self { steps, next: 0 }
    }
}

impl ProcessSource for Steps {
    fn status_text(&self) -> &str {
        FAKE_STATUS_TEXT
    }

    fn snapshot(&mut self) -> ProcessSnapshot {
        let index = self.next.min(self.steps.len() - 1);
        self.next += 1;
        snapshot(self.steps[index].clone(), self.next as u64)
    }
}

/// One published row: its item ID and its cells as text.
fn rows(session: &Session) -> Vec<(ItemId, Vec<String>)> {
    session.with_store(|store| {
        let model = store.get_model(MODEL).expect("the collection model exists");
        assert_eq!(model.item_count, model.items.len() as u64);
        model
            .items
            .values()
            .map(|item| {
                let Value::List(cells) = &item.value else {
                    panic!("a row is a list of cells")
                };
                let cells = cells
                    .iter()
                    .map(|cell| match cell {
                        Value::UnsignedInt(number) => number.to_string(),
                        Value::String(text) => text.clone(),
                        other => panic!("unexpected cell {other:?}"),
                    })
                    .collect();
                (item.item_id, cells)
            })
            .collect()
    })
}

/// The published PID cells, in order: each names one fixture process.
fn pids(session: &Session) -> Vec<String> {
    rows(session)
        .into_iter()
        .map(|(_, cells)| cells[0].clone())
        .collect()
}

/// Each process's item ID, by its PID cell.
fn identities(session: &Session) -> HashMap<String, ItemId> {
    rows(session)
        .into_iter()
        .map(|(id, cells)| (cells[0].clone(), id))
        .collect()
}

fn label(session: &Session, node: NodeId) -> String {
    session.with_store(
        |store| match store.get_node(node).unwrap().get_property(LABEL) {
            Some(Value::String(label)) => label.clone(),
            other => panic!("sort button {node:?} has no label: {other:?}"),
        },
    )
}

fn labels(session: &Session) -> [String; 4] {
    [SORT_PID, SORT_NAME, SORT_RESIDENT, SORT_CPU].map(|node| label(session, node))
}

fn order(field: ProcessField, direction: SortDirection) -> SortOrder {
    SortOrder { field, direction }
}

fn started(steps: Vec<Vec<ProcessRecord>>) -> (Session, ProcessView, Steps) {
    let session = Session::mint();
    let mut source = Steps::new(steps);
    let (view, _) = start_from_source(&session, &mut source).expect("the fixture publishes");
    (session, view, source)
}

/// The item IDs one committed transaction deleted and inserted, and whether it
/// carried any other model operation.
fn model_changes(transaction: &srui_protocol::Transaction) -> (Vec<u64>, Vec<u64>, usize) {
    let mut deleted = Vec::new();
    let mut inserted = Vec::new();
    let mut other = 0;
    for op in &transaction.operations {
        match &op.op {
            Some(operation::Op::ModelDelete(delete)) => {
                deleted.extend(delete.item_ids.iter().copied())
            }
            Some(operation::Op::ModelInsert(insert)) => {
                inserted.extend(insert.items.iter().map(|item| item.item_id))
            }
            Some(operation::Op::ModelUpdate(_)) | Some(operation::Op::ModelResetRange(_)) => {
                other += 1
            }
            _ => {}
        }
    }
    (deleted, inserted, other)
}

/// Asserts that every transaction committed after `revision` is a pure move:
/// it reinserts exactly the rows it deletes, under the same item IDs, and
/// changes no row's value. A client applies each transaction whole, so no
/// revision it can hold lacks a row — a selection anchored to an item ID is
/// restored onto that same item after each one.
fn assert_only_moves_since(session: &Session, revision: u64) -> usize {
    let transactions = session
        .collect_replayed_transactions(revision)
        .expect("the journal holds this run");
    let mut moved = 0;
    for transaction in &transactions {
        let (mut deleted, mut inserted, other) = model_changes(transaction);
        assert_eq!(other, 0, "a reorder re-sends no value");
        assert_eq!(
            deleted.len(),
            deleted.iter().collect::<HashSet<_>>().len(),
            "a row is moved once per transaction"
        );
        deleted.sort_unstable();
        inserted.sort_unstable();
        assert_eq!(
            deleted, inserted,
            "revision {} removed a row it did not put back",
            transaction.new_revision
        );
        moved += deleted.len();
    }
    moved
}

/// A client's `ACTIVATE` on `node`, as the wire carries it.
fn activate(node: NodeId, seq: u64, observed_revision: u64) -> WireEvent {
    WireEvent::from(
        &SemanticEvent::activate(seq, format!("sort-click-{seq}"), observed_revision, node)
            .with_client_instance_id("px009-sort-test"),
    )
}

fn processed(outcome: EventOutcome) -> u64 {
    match outcome {
        EventOutcome::Processed {
            revision_after_effect,
            ..
        } => revision_after_effect,
        other => panic!("the click was not processed: {other:?}"),
    }
}

#[test]
fn every_column_sorts_both_ways_by_its_typed_values_with_missing_last() {
    let (session, mut view, _) = started(vec![population()]);
    // Until the first click the rows are in their source's order, and no
    // button claims an order.
    assert_eq!(
        pids(&session),
        ["9", "10", "11", "100", "1000", "Unavailable"]
    );
    assert_eq!(
        labels(&session),
        [
            "Sort by PID",
            "Sort by Name",
            "Sort by Resident",
            "Sort by CPU"
        ]
    );
    assert_eq!(view.sort(), None);
    let identities_before = identities(&session);

    for (field, direction, expected) in [
        // Numbers by value: 9 < 10 < 11 < 100 < 1000, never their text order
        // 10 < 100 < 1000 < 11 < 9. The unobserved PID is last both ways.
        (
            ProcessField::Pid,
            SortDirection::Descending,
            ["1000", "100", "11", "10", "9", "Unavailable"],
        ),
        (
            ProcessField::Pid,
            SortDirection::Ascending,
            ["9", "10", "11", "100", "1000", "Unavailable"],
        ),
        // Names by character: `Xorg` before lower case, `a10` before `a9`;
        // the two `a9`s tie and keep their identity order (the observed PID
        // first) in both directions.
        (
            ProcessField::Name,
            SortDirection::Ascending,
            ["10", "100", "1000", "Unavailable", "11", "9"],
        ),
        (
            ProcessField::Name,
            SortDirection::Descending,
            ["9", "11", "1000", "Unavailable", "100", "10"],
        ),
        // Bytes by value: 9 B below 1.0 KiB, whose text sorts first. The two
        // 1.0 KiB rows tie and keep identity order both ways; the denied
        // value is last both ways, never a zero.
        (
            ProcessField::Resident,
            SortDirection::Descending,
            ["Unavailable", "1000", "10", "11", "9", "100"],
        ),
        (
            ProcessField::Resident,
            SortDirection::Ascending,
            ["9", "10", "11", "1000", "Unavailable", "100"],
        ),
        // CPU by measured share: the measured zero is a value; the unavailable
        // and warming-up shares are last both ways, in identity order.
        (
            ProcessField::Cpu,
            SortDirection::Descending,
            ["10", "1000", "9", "Unavailable", "11", "100"],
        ),
        (
            ProcessField::Cpu,
            SortDirection::Ascending,
            ["Unavailable", "9", "10", "1000", "11", "100"],
        ),
    ] {
        let revision = session.current_revision();
        let outcome = view.set_sort(&session, order(field, direction)).unwrap();
        assert_eq!(pids(&session), expected, "{field:?} {direction:?}");
        assert_eq!(view.sort(), Some(order(field, direction)));
        // Only rows moved, under their own item IDs, and nothing else.
        let moved = assert_only_moves_since(&session, revision);
        assert_eq!(outcome.moved, moved);
        assert_eq!(
            (outcome.inserted, outcome.deleted, outcome.updated),
            (0, 0, 0)
        );
        assert_eq!(
            identities(&session),
            identities_before,
            "{field:?} {direction:?}"
        );
        assert_eq!(
            view.item_ids(),
            rows(&session).iter().map(|(id, _)| *id).collect::<Vec<_>>()
        );
    }
    // The cells themselves never changed: sorting publishes order, not values.
    let mut cells: Vec<Vec<String>> = rows(&session).into_iter().map(|(_, cells)| cells).collect();
    cells.sort();
    assert!(cells.contains(&vec![
        "100".into(),
        "a10".into(),
        "Denied".into(),
        "Warming up".into()
    ]));
    assert!(cells.contains(&vec![
        "Unavailable".into(),
        "a9".into(),
        "4.0 KiB".into(),
        "0.0%".into()
    ]));
}

#[test]
fn a_reorder_moves_only_the_rows_that_change_place() {
    let (session, mut view, _) = started(vec![population()]);
    view.set_sort(&session, order(ProcessField::Pid, SortDirection::Ascending))
        .unwrap();
    // The source order already was PID ascending: the label changes, no row
    // moves.
    assert_eq!(
        pids(&session),
        ["9", "10", "11", "100", "1000", "Unavailable"]
    );
    assert_eq!(label(&session, SORT_PID), "Sort by PID \u{2191}");
    // Name ascending keeps the longest run already in order (100, 1000,
    // Unavailable after 10) and moves the other two.
    let outcome = view
        .set_sort(
            &session,
            order(ProcessField::Name, SortDirection::Ascending),
        )
        .unwrap();
    assert_eq!(
        pids(&session),
        ["10", "100", "1000", "Unavailable", "11", "9"]
    );
    assert_eq!(outcome.moved, 2);
    assert_eq!(outcome.transactions, 1, "labels and moves commit together");
    assert_eq!(outcome.controls, 2, "the old and the new active button");
    assert_eq!(
        labels(&session),
        [
            "Sort by PID",
            "Sort by Name \u{2191}",
            "Sort by Resident",
            "Sort by CPU"
        ]
    );
}

#[test]
fn a_request_for_the_published_order_publishes_nothing() {
    let (session, mut view, _) = started(vec![population()]);
    view.set_sort(
        &session,
        order(ProcessField::Cpu, SortDirection::Descending),
    )
    .unwrap();
    let revision = session.current_revision();
    let outcome = view
        .set_sort(
            &session,
            order(ProcessField::Cpu, SortDirection::Descending),
        )
        .unwrap();
    assert_eq!(outcome, Refreshed::default());
    assert_eq!(session.current_revision(), revision);
    // An unchanged snapshot under an active sort publishes nothing either.
    let mut repeat = Steps::new(vec![population()]);
    let refreshed = view.refresh(&session, &mut repeat).unwrap();
    assert_eq!(refreshed.transactions, 0);
    assert_eq!(session.current_revision(), revision);
    assert_eq!(
        (
            refreshed.moved,
            refreshed.inserted,
            refreshed.deleted,
            refreshed.updated
        ),
        (0, 0, 0, 0)
    );
}

#[test]
fn changing_cpu_rankings_move_rows_under_their_own_item_ids() {
    let first = population();
    // The shell climbs from 2% to 50%.
    let mut climbed = population();
    climbed[0].cpu = cpu(50);
    // Then `worker` is measured at 80%, `Xorg` ends, and `make` arrives at 30%.
    let mut churned = climbed.clone();
    churned[2].cpu = cpu(80);
    churned.remove(1);
    churned.insert(
        1,
        record(Some(12), "make", "make", Observed::Known(512), cpu(30)),
    );
    // Then `make` ends too, and `worker` can no longer be measured.
    let mut fallen = churned.clone();
    fallen.remove(1);
    fallen[1].cpu = CpuUsage::Missing(MissingReason::Unavailable);
    let (session, mut view, mut source) = started(vec![first, climbed, churned, fallen]);
    view.set_sort(
        &session,
        order(ProcessField::Cpu, SortDirection::Descending),
    )
    .unwrap();
    assert_eq!(
        pids(&session),
        ["10", "1000", "9", "Unavailable", "11", "100"]
    );
    let before = identities(&session);
    // A client's selection, anchored to the shell's item ID.
    let selected = before["9"];

    // The shell climbs past two rows: it alone moves, carrying its new value,
    // so its row is sent once and nothing else is touched.
    let revision = session.current_revision();
    let outcome = view.refresh(&session, &mut source).unwrap();
    assert_eq!(
        pids(&session),
        ["9", "10", "1000", "Unavailable", "11", "100"]
    );
    assert_eq!(
        (
            outcome.moved,
            outcome.inserted,
            outcome.deleted,
            outcome.updated
        ),
        (1, 0, 0, 0)
    );
    assert_eq!(identities(&session), before);
    assert_eq!(
        rows(&session)[0],
        (
            selected,
            vec!["9".into(), "zsh".into(), "9 B".into(), "50.0%".into()]
        )
    );
    for transaction in session.collect_replayed_transactions(revision).unwrap() {
        let (mut deleted, mut inserted, _) = model_changes(&transaction);
        deleted.sort_unstable();
        inserted.sort_unstable();
        assert_eq!(
            deleted, inserted,
            "a moved row is never missing from a revision"
        );
    }

    // A process ends, one arrives at its sorted place, and `worker` — now
    // measured — leaves the missing group for the top.
    let outcome = view.refresh(&session, &mut source).unwrap();
    assert_eq!(
        pids(&session),
        ["11", "9", "12", "1000", "Unavailable", "100"]
    );
    assert_eq!(
        (
            outcome.moved,
            outcome.inserted,
            outcome.deleted,
            outcome.updated
        ),
        (1, 1, 1, 0)
    );
    let after = identities(&session);
    for pid in ["9", "11", "100", "1000", "Unavailable"] {
        assert_eq!(after[pid], before[pid], "{pid} kept its item ID");
    }
    assert!(!after.contains_key("10"));
    assert!(
        !before.values().any(|id| *id == after["12"]),
        "a new process gets a new ID"
    );
    assert_eq!(
        rows(&session)[1].0,
        selected,
        "the selected process is still the same item"
    );

    // `make` ends and `worker` drops back into the missing group, last.
    view.refresh(&session, &mut source).unwrap();
    assert_eq!(pids(&session), ["9", "1000", "Unavailable", "11", "100"]);
    assert_eq!(identities(&session)["9"], selected);
}

#[test]
fn a_click_is_applied_and_published_before_it_is_acknowledged() {
    let (session, view, _) = started(vec![population()]);
    let view = Arc::new(Mutex::new(view));
    install_sort_handlers(&session, &view);
    let revision = session.current_revision();
    // The first click on CPU applies its catalogued default, descending.
    let acknowledged = processed(
        session
            .process_event(&activate(SORT_CPU, 1, revision))
            .unwrap(),
    );
    assert_eq!(
        acknowledged,
        session.current_revision(),
        "the acknowledgement holds the new order"
    );
    assert!(acknowledged > revision);
    assert_eq!(
        pids(&session),
        ["10", "1000", "9", "Unavailable", "11", "100"]
    );
    assert_eq!(label(&session, SORT_CPU), "Sort by CPU \u{2193}");
    // The second reverses it, and the missing values stay last.
    processed(
        session
            .process_event(&activate(SORT_CPU, 2, acknowledged))
            .unwrap(),
    );
    assert_eq!(
        pids(&session),
        ["Unavailable", "9", "10", "1000", "11", "100"]
    );
    assert_eq!(label(&session, SORT_CPU), "Sort by CPU \u{2191}");
    // Another column takes its own default and releases the old one.
    processed(
        session
            .process_event(&activate(SORT_RESIDENT, 3, session.current_revision()))
            .unwrap(),
    );
    assert_eq!(
        pids(&session),
        ["Unavailable", "1000", "10", "11", "9", "100"]
    );
    assert_eq!(
        labels(&session),
        [
            "Sort by PID",
            "Sort by Name",
            "Sort by Resident \u{2193}",
            "Sort by CPU"
        ]
    );
    assert_eq!(
        view.lock().unwrap().sort(),
        Some(order(ProcessField::Resident, SortDirection::Descending))
    );
}

#[test]
fn an_older_click_replayed_after_a_newer_one_changes_nothing() {
    let (session, view, _) = started(vec![population()]);
    let view = Arc::new(Mutex::new(view));
    install_sort_handlers(&session, &view);
    let name = activate(SORT_NAME, 1, session.current_revision());
    processed(session.process_event(&name).unwrap());
    processed(
        session
            .process_event(&activate(SORT_PID, 2, session.current_revision()))
            .unwrap(),
    );
    let newest = pids(&session);
    assert_eq!(newest, ["9", "10", "11", "100", "1000", "Unavailable"]);
    let revision = session.current_revision();
    // A reconnecting client replays the older click: the session answers it
    // from its settled result and runs nothing (§18.2).
    assert!(matches!(
        session.process_event(&name).unwrap(),
        EventOutcome::Duplicate { accepted: true, .. }
    ));
    assert_eq!(session.current_revision(), revision);
    assert_eq!(pids(&session), newest);
    assert_eq!(label(&session, SORT_PID), "Sort by PID \u{2191}");
}

#[test]
fn a_snapshot_sampled_before_a_click_is_published_in_the_newer_order() {
    let mut climbed = population();
    climbed[0].cpu = cpu(50);
    let (session, mut view, mut source) = started(vec![population(), climbed]);
    // The scan is taken first, under no sort at all...
    let stale = source.snapshot();
    // ...a click lands while it is outstanding...
    view.click_sort(&session, ProcessField::Cpu).unwrap();
    assert_eq!(
        pids(&session),
        ["10", "1000", "9", "Unavailable", "11", "100"]
    );
    // ...and the scan's result is published in the click's order.
    view.apply(&session, FAKE_STATUS_TEXT, &stale).unwrap();
    assert_eq!(
        pids(&session),
        ["9", "10", "1000", "Unavailable", "11", "100"]
    );
    assert_eq!(label(&session, SORT_CPU), "Sort by CPU \u{2193}");
    assert_eq!(
        view.sort(),
        Some(order(ProcessField::Cpu, SortDirection::Descending))
    );
}

/// The fixture's first snapshot is free; every later one announces itself on
/// `entered` and waits for the test to hand it its records.
struct Gated {
    first: Option<Vec<ProcessRecord>>,
    entered: tokio::sync::mpsc::UnboundedSender<()>,
    release: std::sync::mpsc::Receiver<Vec<ProcessRecord>>,
    last: Vec<ProcessRecord>,
    taken: u64,
}

impl ProcessSource for Gated {
    fn status_text(&self) -> &str {
        FAKE_STATUS_TEXT
    }

    fn snapshot(&mut self) -> ProcessSnapshot {
        let records = match self.first.take() {
            Some(records) => records,
            None => {
                // A closed channel only means the test has finished.
                let _ = self.entered.send(());
                self.release.recv().unwrap_or_else(|_| self.last.clone())
            }
        };
        self.last = records.clone();
        self.taken += 1;
        snapshot(records, self.taken)
    }
}

/// Waits until the loop starts its next scan. The time limit is a safety net
/// only: every step of the test waits on this event, never on a clock.
async fn next_scan(entered: &mut tokio::sync::mpsc::UnboundedReceiver<()>) {
    tokio::time::timeout(Duration::from_secs(60), entered.recv())
        .await
        .expect("the loop samples")
        .expect("the source is alive");
}

/// The production loop, a click through the session while its scan is still
/// running, and the scan's result afterwards: the result is published in the
/// click's order, never in the order in force when the scan began.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_click_during_a_running_scan_is_never_overwritten_by_that_scan() {
    let session = Arc::new(Session::mint());
    let (entered_tx, mut entered) = tokio::sync::mpsc::unbounded_channel();
    let (release, release_rx) = std::sync::mpsc::channel();
    let mut source = Gated {
        first: Some(population()),
        entered: entered_tx,
        release: release_rx,
        last: Vec::new(),
        taken: 0,
    };
    // As `main` does.
    let (view, _) = start_from_source(&session, &mut source).unwrap();
    let view = Arc::new(Mutex::new(view));
    install_sort_handlers(&session, &view);
    let shutdown = tokio_util::sync::CancellationToken::new();
    let polling = tokio::spawn(poll(
        Arc::clone(&view),
        source,
        Arc::clone(&session),
        MIN_REFRESH_INTERVAL,
        shutdown.child_token(),
    ));
    next_scan(&mut entered).await;
    // The loop's scan is running. A click arrives and is published at once,
    // over the rows the client already holds.
    let revision = session.current_revision();
    processed(
        session
            .process_event(&activate(SORT_CPU, 1, revision))
            .unwrap(),
    );
    assert_eq!(
        pids(&session),
        ["10", "1000", "9", "Unavailable", "11", "100"]
    );
    // The scan now returns its result — the shell climbed to 50% — and the
    // next scan starting proves the loop has published it.
    let mut climbed = population();
    climbed[0].cpu = cpu(50);
    release.send(climbed).unwrap();
    next_scan(&mut entered).await;
    assert_eq!(
        pids(&session),
        ["9", "10", "1000", "Unavailable", "11", "100"]
    );
    assert_eq!(label(&session, SORT_CPU), "Sort by CPU \u{2193}");
    assert_eq!(
        view.lock().unwrap().sort(),
        Some(order(ProcessField::Cpu, SortDirection::Descending))
    );
    shutdown.cancel();
    drop(release);
    tokio::time::timeout(Duration::from_secs(60), polling)
        .await
        .expect("the loop stops")
        .expect("the loop does not panic");
}

#[test]
fn sort_controls_are_standard_buttons_enabled_only_with_a_collection() {
    let (session, _view, _) = started(vec![population()]);
    for node in [SORT_PID, SORT_NAME, SORT_RESIDENT, SORT_CPU] {
        assert_eq!(
            session.with_store(|store| store
                .get_node(node)
                .unwrap()
                .get_property(ENABLED)
                .cloned()),
            Some(Value::Bool(true))
        );
    }
    // The table offers a native single selection, which the client anchors to
    // item IDs; the server installs nothing that acts on it.
    assert_eq!(
        session.with_store(|store| store
            .get_node(TABLE)
            .unwrap()
            .get_property(SELECTION_MODE)
            .cloned()),
        Some(Value::EnumToken(EnumToken::from(SelectionMode::Single)))
    );

    // An empty shell samples nothing: its buttons are disabled, and a click on
    // one is refused by event validation before any handler runs.
    let empty = Session::mint();
    initialize(&empty).unwrap();
    let (view, _) = {
        // A view of another session stands in for the handler's target: it
        // must never be reached.
        let other = Session::mint();
        start_from_source(&other, &mut Steps::new(vec![population()])).unwrap()
    };
    let view = Arc::new(Mutex::new(view));
    install_sort_handlers(&empty, &view);
    let outcome = empty
        .process_event(&activate(SORT_PID, 1, empty.current_revision()))
        .unwrap();
    assert!(
        matches!(outcome, EventOutcome::Rejected { .. }),
        "{outcome:?}"
    );
    assert_eq!(view.lock().unwrap().sort(), None, "no handler ran");
    assert_eq!(label(&empty, SORT_PID), "Sort by PID");
}
