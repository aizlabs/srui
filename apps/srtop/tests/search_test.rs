//! PX-011 acceptance: find-next/find-previous and jump to PID for srtop (D1
//! §§6.2–6.3, 7.6, 8, 12.1, 22.5, 29).
//!
//! * Search is distinct from filtering: it locates matches and never hides,
//!   moves or changes a row. Three identical names cycle predictably forward
//!   and backward while the row count and every row stay exactly as they were.
//! * No match, a missing PID and a malformed PID each publish a visible
//!   explanation, and the navigation controls that could do nothing are
//!   disabled (refused by event validation, not merely discouraged).
//! * The current match is held by item ID. A PID that is reused selects the new
//!   live record only through an explicit new search: a refresh never retargets
//!   the old selection.
//! * Matches are counted over the whole server-side collection, not over cached
//!   or published rows only: matches the filter or the row ceiling keep out of
//!   the model are counted and stated.

use srui_process_explorer::catalogue::{ProcessField, SortDirection};
use srui_process_explorer::refresh::ProcessView;
use srui_process_explorer::search::{
    SearchStep, IDLE_STATUS, MAX_PID_INPUT_BYTES, MAX_SEARCH_STATUS_BYTES,
};
use srui_process_explorer::sort::SortOrder;
use srui_process_explorer::source::*;
use srui_process_explorer::{
    install_filter_handlers, install_search_handlers, start_from_source, MODEL, PID_GO, PID_INPUT,
    SEARCH_INPUT, SEARCH_NEXT, SEARCH_PREVIOUS, SEARCH_STATUS, TABLE,
};
use srui_protocol::Event as WireEvent;
use srui_sdk::{ItemId, NodeId, Operation, Value, ENABLED, TEXT, VALUE};
use srui_semantic_tree::{EditSeq, Event as DomainEvent};
use srui_sessiond::{EventOutcome, Session};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const SOURCE: &str = "search-fixture-v1";
const CLIENT: &[u8] = b"search-client-instance";

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

fn record(pid: u32, token: &str, name: &str, cmd: &[u8]) -> ProcessRecord {
    ProcessRecord {
        key: key(pid, token),
        display_name: DisplayName::from(name).with_command(cmd),
        resident: Observed::Known(4096),
        cpu: CpuUsage::Measured(CpuInterval {
            ticks: 1,
            ticks_per_second: 100,
            elapsed: Duration::from_secs(1),
        }),
    }
}

/// A source whose records and completeness the test replaces between samples.
struct Fixture {
    records: Vec<ProcessRecord>,
    completeness: Completeness,
}

impl Fixture {
    fn new(records: Vec<ProcessRecord>) -> Self {
        Self {
            records,
            completeness: Completeness::Complete,
        }
    }
}

impl ProcessSource for Fixture {
    fn status_text(&self) -> &str {
        "fixture source ok"
    }

    fn snapshot(&mut self) -> ProcessSnapshot {
        ProcessSnapshot {
            sampled_at: SnapshotTime(std::time::UNIX_EPOCH + Duration::from_secs(1_800_000_000)),
            source: SourceId(SOURCE.into()),
            records: self.records.clone(),
            vanished: 0,
            capped: CappedRecords::none(),
            system: SystemSample::not_provided(),
            completeness: self.completeness.clone(),
        }
    }
}

/// Five processes, three of them named exactly `worker`.
fn three_workers() -> Vec<ProcessRecord> {
    vec![
        record(1, "init", "systemd", b"/sbin/init"),
        record(100, "worker-a", "worker", b"worker --job=a"),
        record(101, "worker-b", "worker", b"worker --job=b"),
        record(102, "worker-c", "worker", b"worker --job=c"),
        record(200, "nginx", "nginx", b"nginx: master process"),
    ]
}

/// Every row the client holds, in published order: item ID and cells.
fn rows(session: &Session) -> Vec<(ItemId, Value)> {
    session.with_store(|store| {
        let model = store.get_model(MODEL).expect("the collection model exists");
        assert_eq!(model.item_count, model.items.len() as u64);
        model
            .items
            .values()
            .map(|item| (item.item_id, item.value.clone()))
            .collect()
    })
}

fn item_of(session: &Session, pid: u64) -> ItemId {
    rows(session)
        .into_iter()
        .find(
            |(_, value)| matches!(value, Value::List(cells) if cells[0] == Value::UnsignedInt(pid)),
        )
        .map(|(id, _)| id)
        .unwrap_or_else(|| panic!("no published row has PID {pid}"))
}

fn text(session: &Session, node: NodeId) -> String {
    session.with_store(
        |store| match store.get_node(node).and_then(|n| n.get_property(TEXT)) {
            Some(Value::String(text)) => text.clone(),
            other => panic!("node {node:?} has no text: {other:?}"),
        },
    )
}

fn value(session: &Session, node: NodeId) -> String {
    session.with_store(
        |store| match store.get_node(node).and_then(|n| n.get_property(VALUE)) {
            Some(Value::String(text)) => text.clone(),
            _ => String::new(),
        },
    )
}

fn enabled(session: &Session, node: NodeId) -> bool {
    session.with_store(|store| {
        store.get_node(node).and_then(|n| n.get_property(ENABLED)) == Some(&Value::Bool(true))
    })
}

/// The search status the client holds, which must also be the view's record.
fn status(session: &Session, view: &ProcessView) -> String {
    let held = text(session, SEARCH_STATUS);
    assert_eq!(
        held,
        view.search_status(),
        "the view records what committed"
    );
    held
}

/// Operations committed since `revision`, as a client receives them.
fn operations_since(session: &Session, revision: u64) -> Vec<Operation> {
    session
        .collect_replayed_transactions(revision)
        .expect("the journal holds this run")
        .into_iter()
        .flat_map(|transaction| transaction.operations)
        .map(|op| Operation::try_from(op).expect("a committed operation decodes"))
        .collect()
}

/// The table's promoted selection as the store holds it (PX-011-G01).
fn promoted(session: &Session) -> Option<ItemId> {
    session.with_store(|store| {
        store
            .get_node(TABLE)
            .and_then(|node| node.promoted_selection())
    })
}

/// Whether any operation since `revision` cleared the table's promotion.
fn promotion_cleared_since(session: &Session, revision: u64) -> bool {
    operations_since(session, revision).iter().any(|op| {
        matches!(op, Operation::ClearProperty { id, property } if *id == TABLE && *property == VALUE)
    })
}

/// Every operation since `revision` touches only the search controls and the
/// table's promoted selection: no row is added, removed, moved or changed. A
/// promotion carries its row in a one-item `MODEL_RESET_RANGE` whose item is
/// exactly the row the model already holds at that index.
fn assert_only_search_controls_changed(
    session: &Session,
    revision: u64,
    before: &[(ItemId, Value)],
) {
    for op in operations_since(session, revision) {
        match op {
            Operation::SetProperty {
                id,
                property,
                value,
            } if id == TABLE => {
                assert_eq!(
                    property, VALUE,
                    "search set only the table's promoted selection"
                );
                assert!(matches!(value, Value::ItemId(_)), "{value:?}");
            }
            Operation::ModelResetRange {
                id: model,
                start_index,
                items,
                ..
            } => {
                assert_eq!(model, MODEL);
                assert_eq!(items.len(), 1, "a promotion carries exactly its own row");
                let (id, value) = &before[start_index as usize];
                assert_eq!((items[0].item_id, &items[0].value), (*id, value));
            }
            Operation::SetProperty { id, .. } => assert!(
                [
                    SEARCH_STATUS,
                    SEARCH_PREVIOUS,
                    SEARCH_NEXT,
                    PID_GO,
                    SEARCH_INPUT,
                    PID_INPUT
                ]
                .contains(&id),
                "search touched node {id:?}"
            ),
            other => panic!("search published something other than its controls: {other:?}"),
        }
    }
}

#[test]
fn three_identical_names_cycle_forward_and_backward_without_changing_rows() {
    let session = Session::mint();
    let mut source = Fixture::new(three_workers());
    let (mut view, _) = start_from_source(&session, &mut source).unwrap();
    assert_eq!(status(&session, &view), IDLE_STATUS);
    assert!(!enabled(&session, SEARCH_NEXT) && !enabled(&session, SEARCH_PREVIOUS));
    let before = rows(&session);
    assert_eq!(before.len(), 5);
    let revision = session.current_revision();
    let workers = [
        item_of(&session, 100),
        item_of(&session, 101),
        item_of(&session, 102),
    ];

    view.set_search(&session, "worker").unwrap();
    let expect = |view: &ProcessView, nth: usize| {
        assert_eq!(view.search().target(), Some(workers[nth - 1]));
        let pid = 99 + nth;
        let row = nth + 1;
        assert_eq!(
            status(&session, view),
            format!(
                "Search: \"worker\": 3 matches in 5 shown rows · match {nth} of 3 at row {row} of 5 · PID {pid} · worker"
            )
        );
        assert_eq!(rows(&session), before, "search never changes a row");
        assert_eq!(view.row_count(), 5);
        // The match is promoted to the table's selection, by item ID.
        assert_eq!(promoted(&session), Some(workers[nth - 1]));
        assert_eq!(view.promoted_selection(), Some(workers[nth - 1]));
    };
    expect(&view, 1);
    assert!(enabled(&session, SEARCH_NEXT) && enabled(&session, SEARCH_PREVIOUS));
    // Forward: 1 → 2 → 3 → wraps to 1.
    for nth in [2, 3, 1] {
        view.step_search(&session, SearchStep::Next).unwrap();
        expect(&view, nth);
    }
    // Backward: 1 → wraps to 3 → 2 → 1.
    for nth in [3, 2, 1] {
        view.step_search(&session, SearchStep::Previous).unwrap();
        expect(&view, nth);
    }
    assert_only_search_controls_changed(&session, revision, &before);

    // A query is case-folded and `|`-split by the filter's own rules.
    view.set_search(&session, "NGINX|systemd").unwrap();
    assert!(status(&session, &view).starts_with(
        "Search: \"NGINX\" | \"systemd\": 2 matches in 5 shown rows · match 1 of 2 at row 1 of 5 · PID 1"
    ));
    assert_eq!(view.search().generation().get(), 2);
    assert_eq!(rows(&session), before);
}

#[test]
fn the_cycle_follows_the_published_order_and_the_target_keeps_its_identity() {
    let session = Session::mint();
    let mut source = Fixture::new(three_workers());
    let (mut view, _) = start_from_source(&session, &mut source).unwrap();
    let first = item_of(&session, 100);
    let middle = item_of(&session, 101);
    view.set_search(&session, "worker").unwrap();
    assert_eq!(view.search().target(), Some(first));
    assert!(status(&session, &view).ends_with("match 1 of 3 at row 2 of 5 · PID 100 · worker"));
    // Sorting by PID descending moves PID 100 from row 2 to row 4, and from the
    // first match to the last: the search is re-published with the sort, and
    // the target and its promotion stay on the same item ID.
    view.set_sort(
        &session,
        SortOrder {
            field: ProcessField::Pid,
            direction: SortDirection::Descending,
        },
    )
    .unwrap();
    assert_eq!(view.search().target(), Some(first));
    assert_eq!(
        promoted(&session),
        Some(first),
        "a sort keeps the promotion"
    );
    assert_eq!(
        status(&session, &view),
        "Search: \"worker\": 3 matches in 5 shown rows · match 3 of 3 at row 4 of 5 · PID 100 · worker"
    );
    // Next now follows the new order: from the last match it wraps to PID 102
    // at row 2; Previous comes back.
    view.step_search(&session, SearchStep::Next).unwrap();
    assert_eq!(view.search().target(), Some(item_of(&session, 102)));
    view.step_search(&session, SearchStep::Previous).unwrap();
    assert_eq!(view.search().target(), Some(first));
    view.step_search(&session, SearchStep::Previous).unwrap();
    assert_eq!(view.search().target(), Some(middle));
    assert_eq!(promoted(&session), Some(middle));

    // The filter hides the target: the search says so and moves nowhere, and
    // the SDK's settlement clears the promotion in the commit that removed
    // its row.
    let revision = session.current_revision();
    view.set_filter(&session, "nginx").unwrap();
    assert_eq!(view.search().target(), Some(middle));
    assert_eq!(promoted(&session), None);
    assert_eq!(view.promoted_selection(), None);
    assert!(promotion_cleared_since(&session, revision));
    assert_eq!(
        status(&session, &view),
        "Search: no match for \"worker\" in 1 shown row; search hides no rows · 3 more matches not shown (filtered out or beyond the row limit) · PID 101 · worker is no longer shown; the search moved to no other process"
    );
    assert!(!enabled(&session, SEARCH_NEXT) && !enabled(&session, SEARCH_PREVIOUS));
    // Clearing the filter publishes the very same item again: the same process
    // instance, not a retarget.
    view.set_filter(&session, "").unwrap();
    assert_eq!(view.search().target(), Some(middle));
    assert!(status(&session, &view).contains("match 2 of 3 at row 3 of 5 · PID 101"));
    // A row shown again is not re-promoted by a refresh or a filter change:
    // only an explicit search moves the native selection.
    assert_eq!(promoted(&session), None);
    view.step_search(&session, SearchStep::Next).unwrap();
    assert_eq!(promoted(&session), Some(first));
}

/// The native fixture's three identical names, as the native test reads them:
/// the promotion cycles 10 → 30 → 50 → 10 and back, the rows untouched.
#[test]
fn the_repeated_name_fixture_cycles_its_three_workers() {
    let session = Session::mint();
    let (mut view, _) = start_from_source(&session, &mut RepeatedNamesSource).unwrap();
    let before = rows(&session);
    assert_eq!(before.len(), RepeatedNamesSource::RECORDS);
    let workers = RepeatedNamesSource::WORKER_ROWS.map(|row| before[row].0);
    view.set_search(&session, "worker").unwrap();
    let mut seen = vec![promoted(&session)];
    for _ in 0..3 {
        view.step_search(&session, SearchStep::Next).unwrap();
        seen.push(promoted(&session));
    }
    for _ in 0..3 {
        view.step_search(&session, SearchStep::Previous).unwrap();
        seen.push(promoted(&session));
    }
    let [a, b, c] = workers.map(Some);
    assert_eq!(seen, [a, b, c, a, c, b, a]);
    assert!(status(&session, &view).ends_with("match 1 of 3 at row 10 of 60 · PID 5010 · worker"));
    // A refresh of the constant fixture publishes nothing and keeps the promotion.
    let revision = session.current_revision();
    view.refresh(&session, &mut RepeatedNamesSource).unwrap();
    assert_eq!(session.current_revision(), revision);
    assert_eq!(promoted(&session), a);
    assert_eq!(rows(&session), before);
}

#[test]
fn next_and_previous_from_a_target_that_is_no_longer_shown_continue_from_its_place() {
    let session = Session::mint();
    let mut source = Fixture::new(three_workers());
    let (mut view, _) = start_from_source(&session, &mut source).unwrap();
    let middle = item_of(&session, 101);
    view.set_search(&session, "worker").unwrap();
    view.step_search(&session, SearchStep::Next).unwrap();
    assert_eq!(view.search().target(), Some(middle));
    assert_eq!(promoted(&session), Some(middle));
    // PID 101 exits: rows are now PID 1, 100, 102, 200, and the target last
    // held row 3 (index 2), which PID 102 now occupies.
    source.records.remove(2);
    let revision = session.current_revision();
    view.refresh(&session, &mut source).unwrap();
    assert_eq!(view.search().target(), Some(middle));
    assert_eq!(
        promoted(&session),
        None,
        "settlement clears a promotion whose row ended"
    );
    assert!(promotion_cleared_since(&session, revision));
    assert!(status(&session, &view)
        .ends_with("PID 101 · worker is no longer shown; the search moved to no other process"));
    // Next continues at the place it held: PID 102, the row now there.
    view.step_search(&session, SearchStep::Next).unwrap();
    assert_eq!(view.search().target(), Some(item_of(&session, 102)));
    assert_eq!(promoted(&session), Some(item_of(&session, 102)));

    // Previous from a lost target goes to the match before its place.
    view.step_search(&session, SearchStep::Previous).unwrap();
    assert_eq!(view.search().target(), Some(item_of(&session, 100)));
    view.step_search(&session, SearchStep::Next).unwrap();
    let last = item_of(&session, 102);
    assert_eq!(view.search().target(), Some(last));
    source.records.remove(2);
    view.refresh(&session, &mut source).unwrap();
    // PID 102 last held row 3 (index 2), now nginx's; no match is at or after
    // it, so Next wraps to the first match.
    view.step_search(&session, SearchStep::Next).unwrap();
    assert_eq!(view.search().target(), Some(item_of(&session, 100)));
}

#[test]
fn a_search_that_selects_nothing_leaves_the_promotion_as_it_was() {
    let session = Session::mint();
    let mut source = Fixture::new(three_workers());
    let (mut view, _) = start_from_source(&session, &mut source).unwrap();
    view.jump_to_pid(&session, "101").unwrap();
    let held = item_of(&session, 101);
    assert_eq!(promoted(&session), Some(held));
    let revision = session.current_revision();
    for nothing in ["zzz", ""] {
        view.set_search(&session, nothing).unwrap();
        assert_eq!(promoted(&session), Some(held), "{nothing:?}");
    }
    view.step_search(&session, SearchStep::Next).unwrap();
    view.jump_to_pid(&session, "9999").unwrap();
    view.set_pid_input(&session, "12a").unwrap();
    assert_eq!(promoted(&session), Some(held));
    assert!(!promotion_cleared_since(&session, revision));
    assert!(operations_since(&session, revision)
        .iter()
        .all(|op| !matches!(
            op,
            Operation::SetProperty { id, .. } if *id == TABLE
        )));
    // Going to the PID already promoted is a new promotion: it reveals again.
    view.jump_to_pid(&session, "101").unwrap();
    assert!(operations_since(&session, revision).iter().any(|op| matches!(
        op,
        Operation::SetProperty { id, value: Value::ItemId(item), .. } if *id == TABLE && *item == held
    )));
}

#[test]
fn no_match_a_missing_pid_and_a_malformed_pid_are_each_explained() {
    let session = Session::mint();
    let mut source = Fixture::new(three_workers());
    let (mut view, _) = start_from_source(&session, &mut source).unwrap();
    let before = rows(&session);

    view.set_search(&session, "zzz").unwrap();
    assert_eq!(
        status(&session, &view),
        "Search: no match for \"zzz\" in 5 shown rows; search hides no rows"
    );
    assert_eq!(view.search().target(), None);
    assert!(!enabled(&session, SEARCH_NEXT) && !enabled(&session, SEARCH_PREVIOUS));
    // A step with nothing to step to moves nothing.
    view.step_search(&session, SearchStep::Next).unwrap();
    assert_eq!(view.search().target(), None);
    assert_eq!(rows(&session), before, "no match never empties the table");
    view.set_search(&session, "").unwrap();
    assert_eq!(status(&session, &view), IDLE_STATUS);

    // A PID no shown process has.
    view.set_pid_input(&session, "9999").unwrap();
    assert!(enabled(&session, PID_GO));
    assert_eq!(
        status(&session, &view),
        IDLE_STATUS,
        "typing a PID searches nothing until Go to PID"
    );
    view.go_to_pid(&session).unwrap();
    assert_eq!(
        status(&session, &view),
        "Search: no shown process has PID 9999"
    );
    assert_eq!(view.search().target(), None);

    // A malformed PID cannot be submitted, and says why.
    for malformed in ["12a", "-1", "4294967296", "1 2", "0x10"] {
        view.set_pid_input(&session, malformed).unwrap();
        assert!(!enabled(&session, PID_GO), "{malformed:?}");
        assert!(
            status(&session, &view).ends_with("PID field: digits only"),
            "{malformed:?}: {}",
            status(&session, &view)
        );
    }
    // A PID the filter hides is explained as hidden, not as absent.
    view.set_filter(&session, "nginx").unwrap();
    view.jump_to_pid(&session, "100").unwrap();
    assert_eq!(
        status(&session, &view),
        "Search: PID 100 is not shown: filtered out or beyond the row limit"
    );
    // And an existing PID is found where it is published.
    view.set_filter(&session, "").unwrap();
    view.jump_to_pid(&session, " 102 ").unwrap();
    assert_eq!(view.search().target(), Some(item_of(&session, 102)));
    assert_eq!(
        status(&session, &view),
        "Search: PID 102 found at row 4 of 5 · worker"
    );
    assert_eq!(rows(&session), before);
}

#[test]
fn a_reused_pid_selects_the_new_record_only_through_an_explicit_new_search() {
    let session = Session::mint();
    let mut source = Fixture::new(vec![
        record(1, "init", "systemd", b"/sbin/init"),
        record(300, "job-old", "job", b"job --run=1"),
        record(400, "other", "other", b"other"),
    ]);
    let (mut view, _) = start_from_source(&session, &mut source).unwrap();
    let old = item_of(&session, 300);
    view.jump_to_pid(&session, "300").unwrap();
    assert_eq!(view.search().target(), Some(old));
    assert_eq!(
        status(&session, &view),
        "Search: PID 300 found at row 2 of 3 · job"
    );

    // The process exits and its PID is reused by a new instance.
    source.records[1] = record(300, "job-new", "job", b"job --run=2");
    view.refresh(&session, &mut source).unwrap();
    let new = item_of(&session, 300);
    assert_ne!(new, old, "a reused PID is a different process instance");
    // The search stays on the old instance, which is gone, and says so. Its
    // promotion was cleared with the row; the new instance never inherits it.
    assert_eq!(view.search().target(), Some(old));
    assert_eq!(promoted(&session), None);
    assert_eq!(
        status(&session, &view),
        "Search: PID 300 · job is no longer shown; the search moved to no other process · a different process now has PID 300: Go to PID selects it"
    );
    // Another refresh still does not retarget.
    view.refresh(&session, &mut source).unwrap();
    assert_eq!(view.search().target(), Some(old));
    assert_eq!(promoted(&session), None);

    // Only an explicit new search selects the new live record.
    view.go_to_pid(&session).unwrap();
    assert_eq!(view.search().target(), Some(new));
    assert_eq!(promoted(&session), Some(new));
    assert_eq!(
        status(&session, &view),
        "Search: PID 300 found at row 2 of 3 · job"
    );

    // The same holds for a query target: the reuse does not move it either.
    view.set_search(&session, "job").unwrap();
    assert_eq!(view.search().target(), Some(new));
    source.records[1] = record(300, "job-third", "job", b"job --run=3");
    view.refresh(&session, &mut source).unwrap();
    let third = item_of(&session, 300);
    assert_eq!(view.search().target(), Some(new));
    assert_eq!(promoted(&session), None);
    assert!(status(&session, &view)
        .contains("PID 300 · job is no longer shown; the search moved to no other process"));
    assert!(!status(&session, &view).contains("Go to PID selects it"));
    // Next is explicit: it continues from the place the old target held.
    view.step_search(&session, SearchStep::Next).unwrap();
    assert_eq!(view.search().target(), Some(third));
    assert_eq!(promoted(&session), Some(third));
}

#[test]
fn a_pid_shown_only_from_an_earlier_scan_is_not_selected() {
    let session = Session::mint();
    let mut source = Fixture::new(vec![
        record(1, "init", "systemd", b"/sbin/init"),
        record(300, "job", "job", b"job"),
    ]);
    let (mut view, _) = start_from_source(&session, &mut source).unwrap();
    // The next scan could not read PID 300: its row is retained, unconfirmed.
    let mut skipped = SkippedRecords::with_limit(1);
    skipped.record(300);
    source.records.truncate(1);
    source.completeness = Completeness::from_scan(
        skipped,
        vec![EnumerationIssue {
            scope: IssueScope::Process(300),
            reason: MissingReason::Denied,
            detail: "denied".into(),
        }],
    );
    view.refresh(&session, &mut source).unwrap();
    assert_eq!(rows(&session).len(), 2, "the unreadable row is retained");
    view.jump_to_pid(&session, "300").unwrap();
    assert_eq!(view.search().target(), None);
    assert_eq!(promoted(&session), None, "a retained row is never promoted");
    assert_eq!(
        status(&session, &view),
        "Search: PID 300 is shown only from an earlier scan and was not selected"
    );
    // When a scan confirms it again, the status says a new search would find
    // it, and still selects nothing on its own.
    source.records.push(record(300, "job", "job", b"job"));
    source.completeness = Completeness::Complete;
    view.refresh(&session, &mut source).unwrap();
    assert_eq!(view.search().target(), None);
    assert_eq!(
        status(&session, &view),
        "Search: PID 300 was not shown when searched and is now: Go to PID selects it"
    );
    view.go_to_pid(&session).unwrap();
    assert_eq!(view.search().target(), Some(item_of(&session, 300)));
}

#[test]
fn matches_are_counted_over_the_whole_collection_not_the_published_rows() {
    let session = Session::mint();
    let mut source = Fixture::new(three_workers());
    let (view, _) = start_from_source(&session, &mut source).unwrap();
    // Publish only the first two rows: PID 1 and the first worker.
    let mut view = view.with_published_limit(2);
    view.refresh(&session, &mut source).unwrap();
    assert_eq!(rows(&session).len(), 2);
    view.set_search(&session, "worker").unwrap();
    assert_eq!(
        status(&session, &view),
        "Search: \"worker\": 1 match in 2 shown rows · 2 more matches not shown (filtered out or beyond the row limit) · match 1 of 1 at row 2 of 2 · PID 100 · worker"
    );
    // Next cycles over the matches the client can be shown, and never claims
    // a row it does not hold.
    view.step_search(&session, SearchStep::Next).unwrap();
    assert_eq!(view.search().target(), Some(item_of(&session, 100)));
    view.jump_to_pid(&session, "102").unwrap();
    assert_eq!(
        status(&session, &view),
        "Search: \"worker\": 1 match in 2 shown rows · 2 more matches not shown (filtered out or beyond the row limit) · PID 102 is not shown: filtered out or beyond the row limit"
    );
}

#[test]
fn search_state_and_status_stay_inside_their_bounds() {
    let session = Session::mint();
    let wide = "\u{20000}".repeat(MAX_DISPLAY_NAME_CHARS);
    let mut source = Fixture::new(vec![
        record(7, "wide-a", &wide, b"x"),
        record(8, "wide-b", &wide, b"x"),
    ]);
    let (mut view, _) = start_from_source(&session, &mut source).unwrap();
    // The widest query the filter's rules keep: sixteen long terms.
    let term = "\u{20000}".repeat(7);
    let query = vec![term.as_str(); 40].join("|");
    view.set_search(&session, &query).unwrap();
    let shown = status(&session, &view);
    assert!(shown.len() <= MAX_SEARCH_STATUS_BYTES, "{}", shown.len());
    assert!(shown.contains("query bounded"), "{shown}");
    assert!(value(&session, SEARCH_INPUT).len() <= 512);
    view.set_search(&session, &wide).unwrap();
    view.set_pid_input(&session, &"7".repeat(4 * MAX_PID_INPUT_BYTES))
        .unwrap();
    assert!(value(&session, PID_INPUT).len() <= MAX_PID_INPUT_BYTES);
    assert!(view.search().pid_input().len() <= MAX_PID_INPUT_BYTES);
    assert!(status(&session, &view).len() <= MAX_SEARCH_STATUS_BYTES);
}

fn text_edit(seq: u64, node: NodeId, text: &str) -> WireEvent {
    DomainEvent::text_edit(
        seq,
        format!("search-edit-{seq}"),
        0u64,
        node.get(),
        text,
        EditSeq::new(seq).expect("positive edit_seq"),
    )
    .with_client_instance_id(CLIENT.to_vec())
    .to_wire()
}

fn activate(seq: u64, node: NodeId, observed: u64) -> WireEvent {
    DomainEvent::activate(seq, format!("search-click-{seq}"), observed, node)
        .with_client_instance_id(CLIENT.to_vec())
        .to_wire()
}

fn processed(outcome: EventOutcome) {
    assert!(
        matches!(outcome, EventOutcome::Processed { .. }),
        "not processed: {outcome:?}"
    );
}

#[test]
fn the_search_controls_are_driven_by_standard_client_events() {
    let session = Arc::new(Session::mint());
    let mut source = Fixture::new(three_workers());
    let (view, _) = start_from_source(&session, &mut source).unwrap();
    let view = Arc::new(Mutex::new(view));
    install_filter_handlers(&session, &view);
    install_search_handlers(&session, &view);
    let before = rows(&session);

    // Next is disabled before any search, so event validation refuses it.
    let refused = session
        .process_event(&activate(1, SEARCH_NEXT, session.current_revision()))
        .expect("event admitted");
    assert!(
        matches!(refused, EventOutcome::Rejected { .. }),
        "{refused:?}"
    );
    processed(
        session
            .process_event(&text_edit(2, SEARCH_INPUT, "worker"))
            .unwrap(),
    );
    assert_eq!(value(&session, SEARCH_INPUT), "worker");
    processed(
        session
            .process_event(&activate(3, SEARCH_NEXT, session.current_revision()))
            .unwrap(),
    );
    processed(
        session
            .process_event(&activate(4, SEARCH_NEXT, session.current_revision()))
            .unwrap(),
    );
    processed(
        session
            .process_event(&activate(5, SEARCH_PREVIOUS, session.current_revision()))
            .unwrap(),
    );
    assert!(text(&session, SEARCH_STATUS).contains("match 2 of 3 at row 3 of 5 · PID 101"));
    assert_eq!(rows(&session), before);

    // The PID is read from the field the server kept, never from the click.
    processed(
        session
            .process_event(&text_edit(6, PID_INPUT, "200"))
            .unwrap(),
    );
    processed(
        session
            .process_event(&activate(7, PID_GO, session.current_revision()))
            .unwrap(),
    );
    assert!(
        text(&session, SEARCH_STATUS).ends_with("PID 200 found at row 5 of 5 · nginx"),
        "{}",
        text(&session, SEARCH_STATUS)
    );
    assert_eq!(
        view.lock().unwrap().search().target(),
        Some(item_of(&session, 200))
    );
    assert_eq!(promoted(&session), Some(item_of(&session, 200)));
    // A malformed PID disables Go to PID; a click is then refused.
    processed(
        session
            .process_event(&text_edit(8, PID_INPUT, "20x"))
            .unwrap(),
    );
    let refused = session
        .process_event(&activate(9, PID_GO, session.current_revision()))
        .unwrap();
    assert!(
        matches!(refused, EventOutcome::Rejected { .. }),
        "{refused:?}"
    );
    assert_eq!(rows(&session), before);
}
