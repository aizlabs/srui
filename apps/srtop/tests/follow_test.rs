//! PX-017 acceptance: follow selected process and a stable viewport (D1
//! §§6.2–6.3, 7.6, 7.7, 8, 22.5, 22.7; D2 T28).
//!
//! * Follow is an explicit semantic setting: a standard `Toggle` whose
//!   `VALUE_CHANGED` the server resolves against its own selection, and a
//!   status line that says whom follow is on, or why it is off.
//! * Off, no sample promotes anything: ordinary selection preservation is the
//!   client's, anchored to item IDs, and never scrolls.
//! * On, a sample, sort or filter that moves the followed process to another
//!   row ends with its promotion (PX-011-G01), after every row operation; one
//!   that does not move it promotes nothing, so a pan is kept. No row is ever
//!   created for it.
//! * Follow stops, and says why, when the process ends or is hidden; a new
//!   process that reuses its PID is never followed.
//! * Selecting another row follows it; a selection a later promotion replaced
//!   on the client (by its `observed_revision`) is promoted back.

use srui_process_explorer::catalogue::ProcessField;
use srui_process_explorer::follow::{FOLLOW_LABEL, OFF_STATUS};
use srui_process_explorer::refresh::{lock_view, ProcessView, SharedView};
use srui_process_explorer::source::{FollowSource, ProcessSource};
use srui_process_explorer::{
    install_follow_handlers, install_inspector, start_from_source, FOLLOW_STATUS, FOLLOW_TOGGLE,
    INSPECTOR_STATUS, MODEL, SORT_CPU, SORT_PID, TABLE,
};
use srui_protocol::Event as WireEvent;
use srui_sdk::{
    ItemId, NodeId, Operation, PropertyRef, TypeRef, Value, ENABLED, LABEL, TEXT, VALUE,
};
use srui_semantic_tree::Event as DomainEvent;
use srui_sessiond::{EventOutcome, Session};
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

const CLIENT: &[u8] = b"follow-client-instance";

struct Harness {
    session: Arc<Session>,
    view: SharedView,
    source: FollowSource,
    seq: u64,
}

fn harness() -> Harness {
    let session = Arc::new(Session::mint());
    let mut source = FollowSource::default();
    let (view, _) = start_from_source(&session, &mut source).unwrap();
    let view: SharedView = Arc::new(Mutex::new(view));
    install_follow_handlers(&session, &view);
    install_inspector(&session, &view, None);
    Harness {
        session,
        view,
        source,
        seq: 0,
    }
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

/// The item IDs every `SET_PROPERTY(TABLE, VALUE)` since `revision` promoted.
fn promotions_since(session: &Session, revision: u64) -> Vec<ItemId> {
    operations_since(session, revision)
        .into_iter()
        .filter_map(|op| match op {
            Operation::SetProperty {
                id,
                property,
                value: Value::ItemId(item),
            } if id == TABLE && property == VALUE => Some(item),
            _ => None,
        })
        .collect()
}

impl Harness {
    fn view(&self) -> std::sync::MutexGuard<'_, ProcessView> {
        lock_view(&self.view)
    }

    fn revision(&self) -> u64 {
        self.session.current_revision()
    }

    /// One sample: the next step of the script.
    fn sample(&mut self) {
        let mut view = lock_view(&self.view);
        view.refresh(&self.session, &mut self.source).unwrap();
    }

    fn sort(&self, button: NodeId) {
        let field = if button == SORT_CPU {
            ProcessField::Cpu
        } else {
            ProcessField::Pid
        };
        self.view().click_sort(&self.session, field).unwrap();
    }

    /// The rows a client holds, in published order, as (item ID, PID, name).
    fn rows(&self) -> Vec<(ItemId, u64, String)> {
        self.session.with_store(|store| {
            let model = store.get_model(MODEL).unwrap();
            (0..model.item_count)
                .map(|index| {
                    let item = model.get_item_by_index(index).unwrap();
                    let Value::List(cells) = &item.value else {
                        panic!("a row is a list of cells")
                    };
                    let Value::UnsignedInt(pid) = cells[ProcessField::Pid.index()] else {
                        panic!("a known PID")
                    };
                    let Value::String(name) = &cells[ProcessField::Name.index()] else {
                        panic!("a name")
                    };
                    (item.item_id, pid, name.clone())
                })
                .collect()
        })
    }

    fn row_of(&self, item: ItemId) -> Option<usize> {
        self.rows().iter().position(|(id, _, _)| *id == item)
    }

    fn item_of(&self, pid: u64) -> ItemId {
        self.rows()
            .into_iter()
            .find(|(_, row_pid, _)| *row_pid == pid)
            .map(|(id, _, _)| id)
            .unwrap_or_else(|| panic!("no shown row has PID {pid}"))
    }

    fn identities(&self) -> BTreeSet<ItemId> {
        self.rows().into_iter().map(|(id, _, _)| id).collect()
    }

    fn property(&self, node: NodeId, property: PropertyRef) -> Option<Value> {
        self.session.with_store(|store| {
            store
                .get_node(node)
                .and_then(|n| n.get_property(property))
                .cloned()
        })
    }

    fn status(&self) -> String {
        match self.property(FOLLOW_STATUS, TEXT) {
            Some(Value::String(text)) => text,
            other => panic!("no follow status: {other:?}"),
        }
    }

    fn checked(&self) -> bool {
        self.property(FOLLOW_TOGGLE, VALUE) == Some(Value::Bool(true))
    }

    fn promoted(&self) -> Option<ItemId> {
        self.session.with_store(|store| {
            store
                .get_node(TABLE)
                .and_then(|node| node.promoted_selection())
        })
    }

    fn send(
        &mut self,
        observed: u64,
        node: NodeId,
        event: TypeRef,
        arguments: Vec<(PropertyRef, Value)>,
    ) {
        self.seq += 1;
        let wire: WireEvent = DomainEvent::new(
            None,
            self.seq,
            format!("follow-event-{}", self.seq),
            observed,
            node,
            event,
            arguments,
        )
        .with_client_instance_id(CLIENT.to_vec())
        .to_wire();
        let outcome = self.session.process_event(&wire).expect("event admitted");
        assert!(
            matches!(outcome, EventOutcome::Processed { .. }),
            "not processed: {outcome:?}"
        );
    }

    /// A user's selection, made while the client held `observed`.
    fn select_at(&mut self, observed: u64, item: ItemId) {
        self.send(
            observed,
            TABLE,
            TypeRef::EVENT_SELECTION_CHANGED,
            vec![(PropertyRef::VALUE, Value::ItemId(item))],
        );
    }

    fn select(&mut self, item: ItemId) {
        self.select_at(self.revision(), item);
    }

    /// The checkbox's click: the client flips it and sends its value.
    fn toggle(&mut self, on: bool) {
        self.send(
            self.revision(),
            FOLLOW_TOGGLE,
            TypeRef::EVENT_VALUE_CHANGED,
            vec![(PropertyRef::VALUE, Value::Bool(on))],
        );
    }

    /// Sorts by CPU (busiest first), selects `tracked` and turns follow on.
    fn follow_tracked(&mut self) -> ItemId {
        self.sort(SORT_CPU);
        let tracked = self.item_of(FollowSource::TRACKED_PID.into());
        assert_eq!(self.row_of(tracked), Some(FollowSource::RANKS[0]));
        self.select(tracked);
        let before = self.revision();
        self.toggle(true);
        assert!(self.checked());
        assert_eq!(
            promotions_since(&self.session, before),
            vec![tracked],
            "turning follow on reveals the process at once"
        );
        tracked
    }
}

/// The follow controls are a standard checkbox and a status line, off at the
/// start, and the toggle is enabled in a collecting view.
#[test]
fn follow_is_an_explicit_off_by_default_setting() {
    let h = harness();
    assert_eq!(
        h.property(FOLLOW_TOGGLE, LABEL),
        Some(Value::String(FOLLOW_LABEL.into()))
    );
    assert_eq!(h.property(FOLLOW_TOGGLE, ENABLED), Some(Value::Bool(true)));
    assert!(!h.checked());
    assert_eq!(h.status(), OFF_STATUS);
    assert_eq!(h.view().follow().following(), None);
    assert_eq!(h.view().follow_view().status, OFF_STATUS);
}

/// Off, a selected process is kept selected by the client under its item ID
/// (§22.5) and no sample ever promotes it, however far it moves: the viewport
/// is the user's.
#[test]
fn non_follow_mode_never_promotes_on_a_sample() {
    let mut h = harness();
    h.sort(SORT_CPU);
    let tracked = h.item_of(FollowSource::TRACKED_PID.into());
    h.select(tracked);
    let identities = h.identities();
    let start = h.revision();
    for step in 1..FollowSource::REUSED_STEP {
        h.sample();
        assert_eq!(
            h.row_of(tracked),
            Some(FollowSource::RANKS[step]),
            "step {step}"
        );
        assert_eq!(h.identities(), identities, "step {step}: no row created");
    }
    assert!(h.revision() > start, "the samples published");
    assert_eq!(
        promotions_since(&h.session, start),
        Vec::<ItemId>::new(),
        "follow off: no sample promotes"
    );
    assert_eq!(h.promoted(), None);
    assert!(!h.checked());
    assert_eq!(h.status(), OFF_STATUS);
}

/// On, every sample that moves the followed process ends with its promotion,
/// naming its final place; a sample that does not move it promotes nothing,
/// so a pan is kept; no row is ever created; and a sort that moves it reveals
/// it too.
#[test]
fn cpu_resorting_keeps_the_followed_identity_revealed_without_creating_rows() {
    let mut h = harness();
    let tracked = h.follow_tracked();
    let identities = h.identities();
    assert!(
        h.status()
            .starts_with("Following PID 6010 · tracked · row 1 of 60 · "),
        "{}",
        h.status()
    );
    for step in 1..FollowSource::REUSED_STEP {
        let before = h.revision();
        h.sample();
        let rank = FollowSource::RANKS[step];
        assert_eq!(h.row_of(tracked), Some(rank), "step {step}");
        assert_eq!(
            h.identities(),
            identities,
            "step {step}: no row created or replaced"
        );
        let moved = rank != FollowSource::RANKS[step - 1];
        let operations = operations_since(&h.session, before);
        let promotions = promotions_since(&h.session, before);
        if moved {
            assert_eq!(promotions, vec![tracked], "step {step}: one promotion");
            // The promotion comes after every row operation, and its
            // one-item range is the row the model already holds there.
            let set = operations
                .iter()
                .rposition(|op| matches!(op, Operation::SetProperty { id, property, .. } if *id == TABLE && *property == VALUE))
                .unwrap();
            let last_row_op = operations
                .iter()
                .rposition(|op| {
                    matches!(
                        op,
                        Operation::ModelInsert { .. }
                            | Operation::ModelDelete { .. }
                            | Operation::ModelUpdate { .. }
                    )
                })
                .unwrap();
            assert!(last_row_op < set, "step {step}: promoted after the move");
            let reset = operations
                .iter()
                .find_map(|op| match op {
                    Operation::ModelResetRange {
                        start_index, items, ..
                    } => Some((
                        *start_index,
                        items.iter().map(|item| item.item_id).collect::<Vec<_>>(),
                    )),
                    _ => None,
                })
                .expect("the promotion carries its row");
            assert_eq!(reset, (rank as u64, vec![tracked]), "step {step}");
            assert!(
                h.status().starts_with(&format!(
                    "Following PID 6010 · tracked · row {} of 60 · ",
                    rank + 1
                )),
                "step {step}: {}",
                h.status()
            );
        } else {
            assert_eq!(
                promotions,
                Vec::<ItemId>::new(),
                "step {step}: not moved, nothing promoted"
            );
            assert!(
                h.revision() > before,
                "step {step}: the sample itself published"
            );
        }
        assert_eq!(h.promoted(), Some(tracked));
        assert!(h.checked());
        assert_eq!(h.view().follow().following(), Some(tracked));
    }
    // A sort that moves it reveals it as well.
    let before = h.revision();
    h.sort(SORT_PID);
    assert_eq!(h.row_of(tracked), Some(9));
    assert_eq!(promotions_since(&h.session, before), vec![tracked]);
    assert_eq!(h.identities(), identities);
}

/// When the followed process ends, follow stops, says so, and never moves to
/// the new process that reuses its PID in the same sample, or later.
#[test]
fn a_disappeared_process_never_transfers_follow_to_its_reused_pid() {
    let mut h = harness();
    let tracked = h.follow_tracked();
    for _ in 1..FollowSource::REUSED_STEP {
        h.sample();
    }
    assert_eq!(h.view().follow().following(), Some(tracked));
    let before = h.revision();
    h.sample();
    let reused = h.item_of(FollowSource::TRACKED_PID.into());
    assert_ne!(reused, tracked, "the reused PID is another process");
    assert_eq!(
        h.row_of(reused),
        Some(FollowSource::RANKS[FollowSource::REUSED_STEP])
    );
    assert_eq!(h.row_of(tracked), None);
    assert_eq!(h.view().follow().following(), None);
    assert!(!h.checked(), "the checkbox is off");
    assert_eq!(
        h.status(),
        "Follow stopped: PID 6010 · tracked has ended · a new process that reuses its PID is not followed; select a row and turn follow on again"
    );
    assert_eq!(promotions_since(&h.session, before), Vec::<ItemId>::new());
    assert_eq!(h.promoted(), None, "settlement cleared the promotion");
    match h.property(INSPECTOR_STATUS, TEXT) {
        Some(Value::String(text)) => assert!(text.contains("this process has ended"), "{text}"),
        other => panic!("{other:?}"),
    }
    let after = h.revision();
    h.sample();
    assert_eq!(promotions_since(&h.session, after), Vec::<ItemId>::new());
    assert_eq!(h.view().follow().following(), None);
}

/// A filter that hides the followed process stops follow with its reason;
/// it is never moved to a shown row.
#[test]
fn a_filter_that_hides_the_followed_process_stops_follow() {
    let mut h = harness();
    let tracked = h.follow_tracked();
    let before = h.revision();
    h.view().set_filter(&h.session, "task").unwrap();
    assert_eq!(h.row_of(tracked), None);
    assert_eq!(h.view().follow().following(), None);
    assert!(!h.checked());
    assert_eq!(
        h.status(),
        "Follow stopped: PID 6010 · tracked is no longer shown (filtered out or beyond the row limit); select a row and turn follow on again"
    );
    assert_eq!(promotions_since(&h.session, before), Vec::<ItemId>::new());
    // Showing it again does not restart follow.
    h.view().set_filter(&h.session, "").unwrap();
    assert!(h.row_of(tracked).is_some());
    assert_eq!(h.view().follow().following(), None);
    assert!(!h.checked());
}

/// Turning follow on needs a shown selected process; a refusal is explained
/// and publishes the toggle's value, so the client's checkbox goes back. A
/// request without a boolean changes nothing and states the value too.
#[test]
fn turning_follow_on_without_a_shown_selection_is_refused_and_unchecks_the_box() {
    let mut h = harness();
    let before = h.revision();
    h.toggle(true);
    assert!(!h.checked());
    assert_eq!(
        h.status(),
        "Follow: off · nothing is selected; select a row, then turn follow on"
    );
    let operations = operations_since(&h.session, before);
    assert!(
        operations.iter().any(|op| matches!(op, Operation::SetProperty { id, property, value: Value::Bool(false) } if *id == FOLLOW_TOGGLE && *property == VALUE)),
        "the refusal states the value: {operations:?}"
    );
    assert_eq!(promotions_since(&h.session, before), Vec::<ItemId>::new());

    let tracked = h.item_of(FollowSource::TRACKED_PID.into());
    h.select(tracked);
    h.view().set_filter(&h.session, "task").unwrap();
    h.toggle(true);
    assert!(!h.checked());
    assert_eq!(
        h.status(),
        "Follow: off · PID 6010 · tracked is not shown (filtered out or beyond the row limit); select a shown row first"
    );
    h.view().set_filter(&h.session, "").unwrap();

    // No boolean: nothing changes, the held value is stated.
    let before = h.revision();
    h.send(
        h.revision(),
        FOLLOW_TOGGLE,
        TypeRef::EVENT_VALUE_CHANGED,
        vec![],
    );
    assert!(!h.checked());
    assert!(h.revision() > before);
    assert_eq!(h.view().follow().following(), None);

    // Off by the user: back to the plain off status.
    h.select(tracked);
    h.toggle(true);
    assert_eq!(h.view().follow().following(), Some(tracked));
    h.toggle(false);
    assert!(!h.checked());
    assert_eq!(h.status(), OFF_STATUS);
    assert_eq!(h.view().follow().following(), None);
    let before = h.revision();
    for _ in 0..3 {
        h.sample();
    }
    assert_eq!(promotions_since(&h.session, before), Vec::<ItemId>::new());
}

/// Selecting another row follows it, without a promotion: the client already
/// shows it selected. A selection observed before the latest promotion was
/// replaced by it on the client, so the server promotes the user's row back.
#[test]
fn selecting_another_row_follows_it_and_a_replaced_selection_is_restored() {
    let mut h = harness();
    let tracked = h.follow_tracked();
    let promoted_at = h.view().last_promotion();
    assert_eq!(promoted_at, h.revision());
    let task_03 = h.item_of(6004);
    assert_eq!(h.row_of(task_03), Some(4));
    let before = h.revision();
    h.select(task_03);
    assert_eq!(h.view().follow().following(), Some(task_03));
    assert_eq!(promotions_since(&h.session, before), Vec::<ItemId>::new());
    assert!(
        h.status().starts_with("Following PID 6004 · task-03 · "),
        "{}",
        h.status()
    );

    // `tracked` leaves the top for row 56, so task-03 changes row too, one
    // up: follow promotes task-03, never `tracked`.
    let before = h.revision();
    h.sample();
    assert_eq!(h.row_of(task_03), Some(3));
    assert_eq!(promotions_since(&h.session, before), vec![task_03]);

    // The last row keeps its place through the unchanged step: nothing is
    // promoted.
    let task_59 = h.item_of(6060);
    h.select(task_59);
    let before = h.revision();
    h.sample();
    assert_eq!(h.row_of(task_59), Some(59));
    assert_eq!(promotions_since(&h.session, before), Vec::<ItemId>::new());

    // Back on tracked; the next sample that moves it promotes it.
    h.select(tracked);
    let before = h.revision();
    h.sample();
    assert_eq!(h.row_of(tracked), Some(FollowSource::RANKS[3]));
    assert_eq!(promotions_since(&h.session, before), vec![tracked]);
    let promoted_at = h.view().last_promotion();

    // The user clicked task-05 before applying that promotion: the client then
    // replaced the click with it, so the server promotes task-05 back.
    let task_05 = h.item_of(6006);
    let before = h.revision();
    h.select_at(promoted_at - 1, task_05);
    assert_eq!(h.view().follow().following(), Some(task_05));
    assert_eq!(promotions_since(&h.session, before), vec![task_05]);
    assert_eq!(h.promoted(), Some(task_05));

    // A click made after the latest promotion is already what the client shows.
    let task_07 = h.item_of(6008);
    let before = h.revision();
    let latest = h.view().last_promotion();
    h.select_at(latest, task_07);
    assert_eq!(h.view().follow().following(), Some(task_07));
    assert_eq!(promotions_since(&h.session, before), Vec::<ItemId>::new());

    // A selection that names no shown row stops follow.
    h.select(ItemId::new(9_999_999));
    assert_eq!(h.view().follow().following(), None);
    assert!(!h.checked());
    assert!(
        h.status()
            .starts_with("Follow stopped: the selection named no row"),
        "{}",
        h.status()
    );
}

/// With follow off, an early selection is never promoted back: nothing is
/// promoted on a selection at all.
#[test]
fn non_follow_mode_never_promotes_on_a_selection() {
    let mut h = harness();
    let tracked = h.item_of(FollowSource::TRACKED_PID.into());
    h.select(tracked);
    let before = h.revision();
    h.select_at(0, h.item_of(6004));
    assert_eq!(promotions_since(&h.session, before), Vec::<ItemId>::new());
}

/// The fixture moves only `tracked`, to the scripted rank, and reuses its PID
/// for a new instance at the scripted step.
#[test]
fn the_follow_fixture_moves_one_process_and_then_reuses_its_pid() {
    let mut source = FollowSource::default();
    let mut tokens = Vec::new();
    for step in 0..FollowSource::RANKS.len() + 1 {
        let snapshot = source.snapshot();
        assert_eq!(snapshot.records.len(), FollowSource::RECORDS);
        let tracked = snapshot
            .records
            .iter()
            .find(|record| {
                record.key.pid
                    == srui_process_explorer::source::Observed::Known(FollowSource::TRACKED_PID)
            })
            .unwrap();
        tokens.push(tracked.key.creation.clone());
        assert_eq!(tracked.display_name.as_str(), "tracked", "step {step}");
    }
    assert!(tokens[..FollowSource::REUSED_STEP]
        .iter()
        .all(|token| *token == tokens[0]));
    assert!(tokens[FollowSource::REUSED_STEP..]
        .iter()
        .all(|token| *token != tokens[0]));
}
