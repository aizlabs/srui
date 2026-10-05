//! PX-007 acceptance: the system summary and data-freshness indicator, published
//! as standard Text and Progress nodes and refreshed as scalar properties only
//! when a client would see them change (§§6–8, 12, 22, 29).
use srui_process_explorer::source::*;
use srui_process_explorer::summary::{
    CPU_BAR, CPU_TEXT, FRESHNESS_TEXT, LAYOUT, MEMORY_BAR, MEMORY_TEXT, PROCESSES_TEXT, SUMMARY,
    SWAP_BAR, SWAP_TEXT,
};
use srui_process_explorer::{
    initialize_from_source, start_from_source, MODEL, SHELL_NODE_COUNT, STATUS,
};
use srui_sdk::{
    EnumToken, NodeId, Operation, PropertyRef, TextRole, TypeRef, Value, Visibility, RESOURCE,
    ROLE, TEXT, VALUE, VALUE_DESCRIPTION, VISIBILITY,
};
use srui_sessiond::Session;
use std::time::{Duration, SystemTime};

fn property(session: &Session, node: NodeId, property: PropertyRef) -> Option<Value> {
    session.with_store(|store| {
        store
            .get_node(node)
            .unwrap()
            .get_property(property)
            .cloned()
    })
}

fn text(session: &Session, node: NodeId) -> String {
    match property(session, node, TEXT) {
        Some(Value::String(text)) => text,
        other => panic!("{node:?} carries no text: {other:?}"),
    }
}

fn operations_since(session: &Session, revision: u64) -> Vec<Operation> {
    session
        .collect_replayed_transactions(revision)
        .expect("the journal holds this run")
        .into_iter()
        .flat_map(|transaction| transaction.operations)
        .map(|op| Operation::try_from(op).expect("a committed operation decodes"))
        .collect()
}

fn hidden() -> Value {
    Value::from(EnumToken::from(Visibility::Hidden))
}

/// The fixture snapshot, with `system` as its system-wide figures.
fn fake_with(system: SystemSample) -> ProcessSnapshot {
    let mut snapshot = FakeProcessSource.snapshot();
    snapshot.system = system;
    snapshot
}

/// A source that answers each sample with the next prepared snapshot, and the
/// last one for ever after.
struct Script(Vec<ProcessSnapshot>);

impl ProcessSource for Script {
    fn status_text(&self) -> &str {
        FAKE_STATUS_TEXT
    }

    fn snapshot(&mut self) -> ProcessSnapshot {
        if self.0.len() > 1 {
            self.0.remove(0)
        } else {
            self.0[0].clone()
        }
    }
}

/// A scan whose process list could not be read at all.
fn unlistable(at: u64) -> ProcessSnapshot {
    let mut snapshot = FakeProcessSource.snapshot();
    snapshot.records.clear();
    snapshot.sampled_at = SnapshotTime(SystemTime::UNIX_EPOCH + Duration::from_secs(at));
    snapshot.completeness = Completeness::from_scan(
        SkippedRecords::unenumerable(),
        vec![EnumerationIssue {
            scope: IssueScope::Root,
            reason: MissingReason::Denied,
            detail: "/proc: Permission denied (os error 13)".into(),
        }],
    );
    snapshot
}

/// The summary is standard Text and Progress widgets in Column and Row layout:
/// no custom graph, no image, no resource, no drawing instruction. The fake
/// source's totals and percentages are published as text and as bar fractions
/// that agree, and its host without swap is published as having none.
#[test]
fn the_summary_is_published_as_text_and_progress_and_nothing_else() {
    let session = Session::mint();
    initialize_from_source(&session, &mut FakeProcessSource).unwrap();
    session.with_store(|store| {
        assert_eq!(store.node_count(), SHELL_NODE_COUNT);
        for id in 1..=SHELL_NODE_COUNT as u64 {
            let node = store.get_node(NodeId::new(id)).unwrap();
            assert!(
                [
                    TypeRef::SURFACE,
                    TypeRef::COLUMN,
                    TypeRef::ROW,
                    TypeRef::TEXT,
                    TypeRef::PROGRESS,
                    TypeRef::TABLE,
                ]
                .contains(&node.node_type),
                "node {id} is a {:?}",
                node.node_type
            );
            assert!(!node.has_property(RESOURCE), "node {id} carries a resource");
        }
        for (id, node_type, parent) in LAYOUT {
            let node = store.get_node(id).unwrap();
            assert_eq!((node.node_type, node.parent_id), (node_type, Some(parent)));
        }
    });
    assert_eq!(
        text(&session, CPU_TEXT),
        "Overall CPU (100% = all 8 logical CPUs): 31.2%"
    );
    assert_eq!(
        text(&session, MEMORY_TEXT),
        "Memory: 4.0 GiB used of 16.0 GiB (25.0% of total)"
    );
    assert_eq!(text(&session, SWAP_TEXT), "Swap: none configured");
    assert_eq!(
        text(&session, PROCESSES_TEXT),
        "Processes visible to this reader: 3 listed · complete scan · no srtop filter"
    );
    assert_eq!(
        text(&session, FRESHNESS_TEXT),
        "Last successful sample: 2027-01-15 08:00:00 UTC (server clock) · source: \
         fake-processes-v1"
    );
    // Each bar fills exactly the truncated share its line prints.
    assert_eq!(
        property(&session, CPU_BAR, VALUE),
        Some(Value::Float64(0.312))
    );
    assert_eq!(
        property(&session, MEMORY_BAR, VALUE),
        Some(Value::Float64(0.25))
    );
    assert_eq!(
        property(&session, CPU_BAR, VALUE_DESCRIPTION),
        Some(Value::String("31.2% of all 8 logical CPUs".into()))
    );
    // No swap: no value, and a hidden bar rather than an empty one.
    assert_eq!(property(&session, SWAP_BAR, VALUE), None);
    assert_eq!(property(&session, SWAP_BAR, VISIBILITY), Some(hidden()));
}

/// A source that never changes produces no traffic once it is synchronized:
/// the fake source's sample time is a constant of the fixture, so the
/// freshness line it publishes never moves either.
#[test]
fn a_frozen_fake_source_publishes_nothing_after_its_first_transaction() {
    let session = Session::mint();
    let mut source = FakeProcessSource;
    let (mut view, _) = start_from_source(&session, &mut source).unwrap();
    for _ in 0..20 {
        let outcome = view.refresh(&session, &mut source).unwrap();
        assert!(!outcome.published(), "{outcome:?}");
    }
    assert_eq!(session.current_revision(), 1);
}

/// Only the summary properties a client would see change are published, as
/// scalar `SET_PROPERTY` operations in the refresh's own transaction: a change
/// of overall CPU alone sets that line and its bar, and nothing else.
#[test]
fn only_the_summary_lines_that_changed_reach_the_wire() {
    let busier = {
        let mut system = FakeProcessSource::system();
        system.cpu = SystemCpu::Measured(SystemCpuInterval {
            busy: 400,
            total: 800,
            cpus: Some(8),
        });
        system
    };
    let session = Session::mint();
    let mut source = Script(vec![
        fake_with(FakeProcessSource::system()),
        fake_with(busier),
    ]);
    let (mut view, _) = start_from_source(&session, &mut source).unwrap();
    let outcome = view.refresh(&session, &mut source).unwrap();
    assert_eq!(
        (
            outcome.transactions,
            outcome.summary,
            outcome.updated,
            outcome.status_changed
        ),
        (1, 3, 0, false),
        "{outcome:?}"
    );
    assert_eq!(
        operations_since(&session, 1),
        vec![
            Operation::set_property(CPU_BAR, VALUE_DESCRIPTION, "50.0% of all 8 logical CPUs"),
            Operation::set_property(CPU_BAR, VALUE, 0.5),
            Operation::set_property(
                CPU_TEXT,
                TEXT,
                "Overall CPU (100% = all 8 logical CPUs): 50.0%"
            ),
        ]
    );
    // The same figures again: nothing at all.
    let outcome = view.refresh(&session, &mut source).unwrap();
    assert!(!outcome.published(), "{outcome:?}");
}

/// Review round 1 (F2): one refresh plans its status first, then its summary,
/// then its row operations, so a failing scan explains itself first and the
/// summary of a scan is in place before any of its rows.
#[test]
fn a_refresh_publishes_its_status_then_its_summary_then_its_rows() {
    let busier = {
        let mut system = FakeProcessSource::system();
        system.cpu = SystemCpu::Measured(SystemCpuInterval {
            busy: 400,
            total: 800,
            cpus: Some(8),
        });
        system
    };
    // Process 4102 ended, the host got busier, and the source relabels itself:
    // a row deletion, summary lines and the status all change on one tick.
    let mut later = fake_with(busier);
    later.records.remove(1);
    let session = Session::mint();
    let mut source = Script(vec![FakeProcessSource.snapshot(), later]);
    let (mut view, _) = start_from_source(&session, &mut source).unwrap();
    let snapshot = source.snapshot();
    let outcome = view
        .apply(&session, "Read-only · Relabelled fake snapshot", &snapshot)
        .unwrap();
    assert!(outcome.status_changed && outcome.summary > 0 && outcome.deleted == 1);
    let ops = operations_since(&session, 1);
    assert!(
        matches!(
            ops[0],
            Operation::SetProperty {
                id: STATUS,
                property: TEXT,
                ..
            }
        ),
        "{ops:?}"
    );
    let summary_node = |id: &NodeId| LAYOUT.iter().any(|(node, _, _)| node == id);
    let summary_ops: Vec<usize> = ops
        .iter()
        .enumerate()
        .filter(|(_, op)| {
            matches!(op, Operation::SetProperty { id, .. } | Operation::ClearProperty { id, .. }
                if summary_node(id))
        })
        .map(|(index, _)| index)
        .collect();
    let first_row_operation = ops
        .iter()
        .position(|op| {
            matches!(
                op,
                Operation::ModelInsert { .. }
                    | Operation::ModelDelete { .. }
                    | Operation::ModelUpdate { .. }
            )
        })
        .expect("the refresh deletes a row");
    assert!(!summary_ops.is_empty(), "{ops:?}");
    assert!(
        summary_ops
            .iter()
            .all(|&index| 0 < index && index < first_row_operation),
        "the summary goes between the status and the rows: {ops:?}"
    );
}

/// The process count follows each successful scan, in the status line's own
/// numbers and words, and says what it counts: the processes this reader can
/// see, with no filter of srtop's own.
#[test]
fn the_process_count_follows_each_scan_in_the_status_lines_words() {
    let mut degraded = FakeProcessSource.snapshot();
    degraded.records.truncate(2);
    let mut skipped = SkippedRecords::with_limit(8);
    skipped.record(4103);
    degraded.completeness = Completeness::from_scan(
        skipped,
        vec![EnumerationIssue {
            scope: IssueScope::Process(4103),
            reason: MissingReason::Denied,
            detail: "4103/stat: Permission denied (os error 13)".into(),
        }],
    );
    let session = Session::mint();
    let mut source = Script(vec![FakeProcessSource.snapshot(), degraded]);
    let (mut view, _) = start_from_source(&session, &mut source).unwrap();
    assert_eq!(
        text(&session, PROCESSES_TEXT),
        "Processes visible to this reader: 3 listed · complete scan · no srtop filter"
    );
    view.refresh(&session, &mut source).unwrap();
    assert_eq!(
        text(&session, PROCESSES_TEXT),
        "Processes visible to this reader: 2 listed · 1 unreadable · incomplete scan · no srtop filter"
    );
    assert!(
        text(&session, STATUS).contains("incomplete scan · 2 processes listed · 1 unreadable"),
        "{}",
        text(&session, STATUS)
    );
    // An incomplete scan whose list was read is still a successful sample.
    assert!(text(&session, FRESHNESS_TEXT).starts_with("Last successful sample: "));
}

/// A collector that keeps failing leaves the UI usable: every node and row
/// stays, the summary keeps the last successful sample under a collector
/// error, the identical failures that follow publish nothing at all, and the
/// first good scan recovers in one transaction.
#[test]
fn a_collector_that_stays_broken_republishes_nothing_and_the_ui_stays_usable() {
    let session = Session::mint();
    let mut script = vec![FakeProcessSource.snapshot()];
    script.extend((1..=5).map(|tick| unlistable(1_800_000_000 + tick)));
    let mut recovered = FakeProcessSource.snapshot();
    recovered.sampled_at =
        SnapshotTime(SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_006));
    script.push(recovered);
    let mut source = Script(script);
    let (mut view, _) = start_from_source(&session, &mut source).unwrap();
    let memory = text(&session, MEMORY_TEXT);

    let first = view.refresh(&session, &mut source).unwrap();
    assert_eq!(first.transactions, 1);
    assert_eq!(first.retained, 3);
    let failed = text(&session, FRESHNESS_TEXT);
    assert_eq!(
        failed,
        "Collector error: could not list processes (permission denied) · last successful \
         sample: 2027-01-15 08:00:00 UTC (server clock) · source: fake-processes-v1"
    );
    assert_eq!(
        property(&session, FRESHNESS_TEXT, ROLE),
        Some(Value::from(EnumToken::from(TextRole::Warning)))
    );
    for _ in 0..4 {
        let again = view.refresh(&session, &mut source).unwrap();
        assert!(
            !again.published(),
            "an unchanged failure is not news: {again:?}"
        );
    }
    assert_eq!(text(&session, MEMORY_TEXT), memory, "the figures are kept");
    session.with_store(|store| {
        assert_eq!(store.node_count(), SHELL_NODE_COUNT);
        assert_eq!(store.get_model(MODEL).unwrap().item_count, 3);
        assert_eq!(store.children_of(SUMMARY).map(<[NodeId]>::len), Some(7));
    });

    let recovery = view.refresh(&session, &mut source).unwrap();
    assert_eq!(recovery.transactions, 1);
    assert_eq!(
        text(&session, FRESHNESS_TEXT),
        "Last successful sample: 2027-01-15 08:00:06 UTC (server clock) · source: \
         fake-processes-v1"
    );
    assert_eq!(
        property(&session, FRESHNESS_TEXT, ROLE),
        Some(Value::from(EnumToken::from(TextRole::Status)))
    );
}

/// A first scan that cannot list its processes publishes no figure: every
/// line says it was not sampled, every bar is hidden, and the freshness line
/// names the collector error and the source it was reading.
#[test]
fn a_first_scan_that_fails_publishes_no_figure() {
    let session = Session::mint();
    initialize_from_source(&session, &mut Script(vec![unlistable(1_800_000_000)])).unwrap();
    assert_eq!(
        text(&session, FRESHNESS_TEXT),
        "Collector error: could not list processes (permission denied) · no successful \
         sample yet · source: fake-processes-v1"
    );
    assert_eq!(text(&session, MEMORY_TEXT), "Memory: Not sampled");
    assert_eq!(
        text(&session, CPU_TEXT),
        "Overall CPU (100% = all logical CPUs): Not sampled"
    );
    for bar in [CPU_BAR, MEMORY_BAR, SWAP_BAR] {
        assert_eq!(property(&session, bar, VALUE), None);
        assert_eq!(property(&session, bar, VISIBILITY), Some(hidden()));
    }
}

/// The scripted sequence's every step carries its own sample time, which the
/// freshness line publishes, so one five-tick cycle costs exactly five
/// transactions: the four PX-004 counted, plus step 1, whose only change is
/// that time. A failed step keeps the time of the step before it.
#[test]
fn a_scripted_cycle_costs_one_transaction_per_tick() {
    let session = Session::mint();
    let mut source = ScriptedFakeSource::default();
    let (mut view, _) = start_from_source(&session, &mut source).unwrap();
    let mut times = Vec::new();
    for _ in 0..ScriptedFakeSource::STEPS * 2 {
        let outcome = view.refresh(&session, &mut source).unwrap();
        assert_eq!(outcome.transactions, 1, "{outcome:?}");
        times.push(text(&session, FRESHNESS_TEXT));
    }
    assert_eq!(
        session.current_revision(),
        1 + 2 * ScriptedFakeSource::STEPS as u64
    );
    // Steps 3 and 8 failed: they name the sample of steps 2 and 7.
    for (failed, last) in [(3, "08:00:02"), (8, "08:00:07")] {
        let line = &times[failed - 1];
        assert!(
            line.starts_with("Collector error: ") && line.contains(last),
            "step {failed}: {line}"
        );
    }
    assert!(times[9].starts_with("Last successful sample: 2027-01-15 08:00:10 UTC"));
}
