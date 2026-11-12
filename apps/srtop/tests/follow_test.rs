//! PX-017 acceptance: follow selected process and a stable viewport (D1
//! §§6.2–6.3, 7.6, 8, 22.5, 22.7; D2 T28), on the standard `keep_visible`
//! hint (PX-017-G01).
//!
//! * Follow is an explicit semantic setting: a standard `Toggle` whose
//!   `VALUE_CHANGED` the server resolves against its own selection, and a
//!   status line that says whom follow is on, or why it is off.
//! * Follow never promotes. It names the followed item in the table's
//!   `keep_visible` hint: once at turn-on and once per retarget, never for a
//!   sample, a sort or a filter, because the client reveals the hinted item
//!   wherever a commit moves it. So no reply ever replaces a newer local
//!   selection: rapid arrows and clicks end on the last press.
//! * Off, nothing is hinted and nothing is promoted: ordinary selection
//!   preservation is the client's, anchored to item IDs, and never scrolls.
//! * While follow is on the promoted `value` names the followed process or
//!   nothing, so a fresh client never restores a selection of another
//!   process, and its viewport is revealed to the followed one.
//! * Follow stops, clears its hint and says why when the process ends or is
//!   hidden; a new process that reuses its PID is never followed.

use srui_process_explorer::catalogue::ProcessField;
use srui_process_explorer::follow::{FOLLOW_LABEL, OFF_STATUS, SELECTION_CLEARED_FOLLOW_STATUS};
use srui_process_explorer::refresh::{lock_view, ProcessView, SharedView};
use srui_process_explorer::source::{FollowSource, ProcessSource, FOLLOW_STATUS_TEXT};
use srui_process_explorer::{
    install_follow_handlers, install_inspector, start_from_source, FOLLOW_STATUS, FOLLOW_TOGGLE,
    INSPECTOR_STATUS, MODEL, SORT_CPU, SORT_PID, TABLE,
};
use srui_protocol::Event as WireEvent;
use srui_sdk::{
    ItemId, NodeId, Operation, PropertyRef, TypeRef, Value, ENABLED, KEEP_VISIBLE, LABEL,
    MODEL_REF, TEXT, VALUE,
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

/// The transactions committed since `revision`, as a client receives them.
fn transactions_since(session: &Session, revision: u64) -> Vec<Vec<Operation>> {
    session
        .collect_replayed_transactions(revision)
        .expect("the journal holds this run")
        .into_iter()
        .map(|transaction| {
            transaction
                .operations
                .into_iter()
                .map(|op| Operation::try_from(op).expect("a committed operation decodes"))
                .collect()
        })
        .collect()
}

/// Operations committed since `revision`, as a client receives them.
fn operations_since(session: &Session, revision: u64) -> Vec<Operation> {
    transactions_since(session, revision)
        .into_iter()
        .flatten()
        .collect()
}

/// The item IDs every `SET_PROPERTY(TABLE, property)` in `operations` names.
fn sets_of(operations: &[Operation], property: PropertyRef) -> Vec<ItemId> {
    operations
        .iter()
        .filter_map(|op| match op {
            Operation::SetProperty {
                id,
                property: set,
                value: Value::ItemId(item),
            } if *id == TABLE && *set == property => Some(*item),
            _ => None,
        })
        .collect()
}

/// How many `CLEAR_PROPERTY(TABLE, property)` are in `operations`.
fn clears_of(operations: &[Operation], property: PropertyRef) -> usize {
    operations
        .iter()
        .filter(|op| matches!(op, Operation::ClearProperty { id, property: cleared } if *id == TABLE && *cleared == property))
        .count()
}

/// The item IDs every `SET_PROPERTY(TABLE, VALUE)` since `revision` promoted.
fn promotions_since(session: &Session, revision: u64) -> Vec<ItemId> {
    sets_of(&operations_since(session, revision), VALUE)
}

/// How many promotions were withdrawn (`CLEAR_PROPERTY(TABLE, VALUE)`) since
/// `revision`.
fn withdrawals_since(session: &Session, revision: u64) -> usize {
    clears_of(&operations_since(session, revision), VALUE)
}

/// The item IDs every `SET_PROPERTY(TABLE, KEEP_VISIBLE)` since `revision`
/// hinted.
fn hints_since(session: &Session, revision: u64) -> Vec<ItemId> {
    sets_of(&operations_since(session, revision), KEEP_VISIBLE)
}

/// How many hints were cleared since `revision`.
fn hint_clears_since(session: &Session, revision: u64) -> usize {
    clears_of(&operations_since(session, revision), KEEP_VISIBLE)
}

/// The table as a client that attaches now receives it in its catch-up
/// snapshot: its promoted selection, which the generic client restores as
/// the selected row, and its `keep_visible` hint, which it reveals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Fresh {
    selection: Option<ItemId>,
    hint: Option<ItemId>,
}

fn fresh_client(session: &Session) -> Fresh {
    let hello = srui_protocol::ClientHello {
        core_version: "0.5.0".to_string(),
        profiles: vec!["org.srui.standard-widgets/1".to_string()],
        limits: None,
        client_instance_id: vec![7, 7, 7],
        client_metadata: Default::default(),
        known_resource_hashes: vec![],
    };
    let snapshot = session
        .bootstrap_fresh_client(&hello)
        .expect("a fresh client attaches")
        .snapshot
        .expect("a populated session sends a catch-up snapshot");
    let mut fresh = Fresh {
        selection: None,
        hint: None,
    };
    for operation in snapshot.operations {
        match Operation::try_from(operation).expect("a snapshot operation decodes") {
            Operation::CreateNode { id, properties, .. } if id == TABLE => {
                for (property, value) in properties {
                    match value {
                        Value::ItemId(item) if property == VALUE => fresh.selection = Some(item),
                        Value::ItemId(item) if property == KEEP_VISIBLE => fresh.hint = Some(item),
                        _ => {}
                    }
                }
            }
            Operation::SetProperty {
                id,
                property,
                value: Value::ItemId(item),
            } if id == TABLE => {
                if property == VALUE {
                    fresh.selection = Some(item);
                } else if property == KEEP_VISIBLE {
                    fresh.hint = Some(item);
                }
            }
            Operation::ClearProperty { id, property } if id == TABLE => {
                if property == VALUE {
                    fresh.selection = None;
                } else if property == KEEP_VISIBLE {
                    fresh.hint = None;
                }
            }
            _ => {}
        }
    }
    fresh
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

    fn pid_of(&self, item: ItemId) -> Option<u64> {
        self.rows()
            .into_iter()
            .find(|(id, _, _)| *id == item)
            .map(|(_, pid, _)| pid)
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
        match self.property(TABLE, VALUE) {
            Some(Value::ItemId(item)) => Some(item),
            _ => None,
        }
    }

    fn hinted(&self) -> Option<ItemId> {
        match self.property(TABLE, KEEP_VISIBLE) {
            Some(Value::ItemId(item)) => Some(item),
            _ => None,
        }
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

    /// The user leaves the table with nothing selected: `SELECTION_CHANGED`
    /// with no `value` (PX-017-G02).
    fn clear_selection(&mut self) {
        self.send(
            self.revision(),
            TABLE,
            TypeRef::EVENT_SELECTION_CHANGED,
            vec![],
        );
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

    /// Sorts by CPU (busiest first), selects `tracked` by a click and turns
    /// follow on, asserting nothing about how follow reached the wire.
    fn start_following_tracked(&mut self) -> ItemId {
        self.sort(SORT_CPU);
        let tracked = self.item_of(FollowSource::TRACKED_PID.into());
        assert_eq!(self.row_of(tracked), Some(FollowSource::RANKS[0]));
        self.select(tracked);
        self.toggle(true);
        assert!(self.checked());
        assert_eq!(self.view().follow().following(), Some(tracked));
        tracked
    }

    /// [`Self::start_following_tracked`], asserting that turning on hinted
    /// `tracked` once and promoted nothing.
    fn follow_tracked(&mut self) -> ItemId {
        let before = self.revision();
        let tracked = self.start_following_tracked();
        assert_eq!(
            promotions_since(&self.session, before),
            Vec::<ItemId>::new(),
            "turning follow on promotes nothing"
        );
        assert_eq!(
            hints_since(&self.session, before),
            vec![tracked],
            "turning follow on hints the process once"
        );
        assert_eq!(self.hinted(), Some(tracked));
        tracked
    }
}

/// The follow controls are a standard checkbox and a status line, off at the
/// start, and the toggle is enabled in a collecting view. Nothing is hinted.
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
    assert_eq!(h.hinted(), None);
}

/// Off, a selected process is kept selected by the client under its item ID
/// (§22.5) and no sample ever promotes or hints it, however far it moves: the
/// viewport is the user's.
#[test]
fn non_follow_mode_never_promotes_or_hints_on_a_sample() {
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
    assert_eq!(hints_since(&h.session, start), Vec::<ItemId>::new());
    assert_eq!(h.promoted(), None);
    assert_eq!(h.hinted(), None);
    assert!(!h.checked());
    assert_eq!(h.status(), OFF_STATUS);
}

/// Turning follow on follows the user's own selection: one hint, in one
/// commit with the checkbox, and no promotion or withdrawal, since nothing
/// else is promoted. A fresh client then restores no selection and is
/// revealed to the followed process.
#[test]
fn turning_follow_on_hints_the_users_selection_and_promotes_nothing() {
    let mut h = harness();
    h.sort(SORT_CPU);
    let tracked = h.item_of(FollowSource::TRACKED_PID.into());
    h.select(tracked);
    let before = h.revision();
    h.toggle(true);
    let transactions = transactions_since(&h.session, before);
    assert_eq!(transactions.len(), 1, "one commit: {transactions:?}");
    let operations = &transactions[0];
    assert_eq!(sets_of(operations, KEEP_VISIBLE), vec![tracked]);
    assert_eq!(sets_of(operations, VALUE), Vec::<ItemId>::new());
    assert_eq!(clears_of(operations, VALUE), 0);
    assert!(operations.iter().any(|op| matches!(op, Operation::SetProperty { id, property, value: Value::Bool(true) } if *id == FOLLOW_TOGGLE && *property == VALUE)));
    // The hint carries its row, so a sparse replica can place it.
    assert!(operations.iter().any(|op| matches!(op, Operation::ModelResetRange { start_index: 0, items, .. } if items.len() == 1 && items[0].item_id == tracked)));
    assert_eq!(h.hinted(), Some(tracked));
    assert_eq!(h.promoted(), None);
    assert_eq!(
        fresh_client(&h.session),
        Fresh {
            selection: None,
            hint: Some(tracked)
        }
    );
    assert!(
        h.status()
            .starts_with("Following PID 6010 · tracked · row 1 of 60 · "),
        "{}",
        h.status()
    );
}

/// On, no sample, sort or filter that moves the followed process puts
/// anything of follow's on the wire: the hint stays on its item and the
/// client reveals it wherever it moves. No row is ever created, and the
/// status names its row.
#[test]
fn cpu_resorting_keeps_the_followed_identity_hinted_without_creating_rows() {
    let mut h = harness();
    let tracked = h.follow_tracked();
    let identities = h.identities();
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
        assert!(h.revision() > before, "step {step}: the sample published");
        assert_eq!(
            promotions_since(&h.session, before),
            Vec::<ItemId>::new(),
            "step {step}: no promotion"
        );
        assert_eq!(
            hints_since(&h.session, before),
            Vec::<ItemId>::new(),
            "step {step}: the hint is not set again"
        );
        assert_eq!(hint_clears_since(&h.session, before), 0, "step {step}");
        assert_eq!(h.hinted(), Some(tracked), "step {step}");
        assert_eq!(h.promoted(), None, "step {step}");
        assert!(
            h.status().starts_with(&format!(
                "Following PID 6010 · tracked · row {} of 60 · ",
                rank + 1
            )),
            "step {step}: {}",
            h.status()
        );
        assert!(h.checked());
        assert_eq!(h.view().follow().following(), Some(tracked));
    }
    // A sort that moves it changes nothing of follow's either.
    let before = h.revision();
    h.sort(SORT_PID);
    assert_eq!(h.row_of(tracked), Some(9));
    assert_eq!(promotions_since(&h.session, before), Vec::<ItemId>::new());
    assert_eq!(hints_since(&h.session, before), Vec::<ItemId>::new());
    assert_eq!(h.hinted(), Some(tracked));
    assert_eq!(h.identities(), identities);
}

/// When the followed process ends, follow stops, says so, clears its hint and
/// never moves to the new process that reuses its PID in the same sample, or
/// later.
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
    assert_eq!(hints_since(&h.session, before), Vec::<ItemId>::new());
    assert_eq!(h.hinted(), None, "the hint is cleared");
    assert_eq!(
        fresh_client(&h.session),
        Fresh {
            selection: None,
            hint: None
        }
    );
    match h.property(INSPECTOR_STATUS, TEXT) {
        Some(Value::String(text)) => assert!(text.contains("this process has ended"), "{text}"),
        other => panic!("{other:?}"),
    }
    let after = h.revision();
    h.sample();
    assert_eq!(promotions_since(&h.session, after), Vec::<ItemId>::new());
    assert_eq!(hints_since(&h.session, after), Vec::<ItemId>::new());
    assert_eq!(h.view().follow().following(), None);
}

/// A filter that hides the followed process stops follow with its reason and
/// clears its hint; it is never moved to a shown row.
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
    assert_eq!(hints_since(&h.session, before), Vec::<ItemId>::new());
    assert_eq!(h.hinted(), None);
    // Showing it again does not restart follow.
    let before = h.revision();
    h.view().set_filter(&h.session, "").unwrap();
    assert!(h.row_of(tracked).is_some());
    assert_eq!(h.view().follow().following(), None);
    assert!(!h.checked());
    assert_eq!(hints_since(&h.session, before), Vec::<ItemId>::new());
    assert_eq!(h.hinted(), None);
}

/// The row ceiling that leaves the followed process out stops follow the
/// same way, and clears its hint.
#[test]
fn the_row_ceiling_that_hides_the_followed_process_stops_follow() {
    let mut h = harness();
    let tracked = h.follow_tracked();
    // Step 1 moves `tracked` to row 56 of 60; a ceiling of 50 then leaves it
    // out of the published rows.
    {
        let mut view = h.view.lock().unwrap();
        let placeholder = ProcessView::start(&Session::mint(), &mut FollowSource::default())
            .unwrap()
            .0;
        let following = std::mem::replace(&mut *view, placeholder);
        *view = following.with_published_limit(50);
    }
    let before = h.revision();
    h.sample();
    assert_eq!(h.row_of(tracked), None, "beyond the row ceiling");
    assert_eq!(h.view().follow().following(), None);
    assert!(
        h.status().starts_with("Follow stopped: PID 6010 · tracked is no longer shown (filtered out or beyond the row limit)"),
        "{}",
        h.status()
    );
    assert_eq!(promotions_since(&h.session, before), Vec::<ItemId>::new());
    assert_eq!(h.hinted(), None);
}

/// PX-017-G02: the user deselects the followed row, which the client reports
/// as a `SELECTION_CHANGED` with no `value`. Follow stops, says the selection
/// was cleared, and clears its hint; it never moves to another row and
/// promotes nothing.
#[test]
fn clearing_the_selection_while_following_stops_follow_with_a_reason() {
    let mut h = harness();
    let tracked = h.follow_tracked();
    let before = h.revision();
    h.clear_selection();
    assert_eq!(h.view().follow().following(), None);
    assert!(!h.checked());
    assert_eq!(h.status(), SELECTION_CLEARED_FOLLOW_STATUS);
    assert_eq!(hint_clears_since(&h.session, before), 1);
    assert_eq!(h.hinted(), None);
    assert_eq!(promotions_since(&h.session, before), Vec::<ItemId>::new());
    assert_eq!(h.view().inspector().selected(), None);
    assert!(h.row_of(tracked).is_some(), "the process is still shown");
    // A sample does not restart follow.
    let before = h.revision();
    h.sample();
    assert_eq!(h.view().follow().following(), None);
    assert_eq!(hints_since(&h.session, before), Vec::<ItemId>::new());
}

/// PX-017-G02, from Codex review of PR #101: select a row, deselect it, then
/// turn follow on. The server knows nothing is selected, so turning on is
/// refused and the deselected process is never followed or hinted.
#[test]
fn turning_follow_on_after_the_selection_was_cleared_is_refused() {
    let mut h = harness();
    let tracked = h.item_of(FollowSource::TRACKED_PID.into());
    h.select(tracked);
    assert_eq!(h.view().inspector().selected(), Some(tracked));
    h.clear_selection();
    assert_eq!(h.view().inspector().selected(), None);
    let before = h.revision();
    h.toggle(true);
    assert!(!h.checked());
    assert_eq!(h.view().follow().following(), None);
    assert_eq!(
        h.status(),
        "Follow: off · nothing is selected; select a row, then turn follow on"
    );
    assert_eq!(hints_since(&h.session, before), Vec::<ItemId>::new());
    assert_eq!(promotions_since(&h.session, before), Vec::<ItemId>::new());
    assert_eq!(h.hinted(), None);
}

/// Turning follow on needs a shown selected process; a refusal is explained,
/// hints nothing and publishes the toggle's value, so the client's checkbox
/// goes back. A request without a boolean changes nothing and states the
/// value too. Turning follow off clears the hint.
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
    assert_eq!(hints_since(&h.session, before), Vec::<ItemId>::new());

    let tracked = h.item_of(FollowSource::TRACKED_PID.into());
    h.select(tracked);
    h.view().set_filter(&h.session, "task").unwrap();
    let before = h.revision();
    h.toggle(true);
    assert!(!h.checked());
    assert_eq!(
        h.status(),
        "Follow: off · PID 6010 · tracked is not shown (filtered out or beyond the row limit); select a shown row first"
    );
    assert_eq!(hints_since(&h.session, before), Vec::<ItemId>::new());
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
    assert_eq!(h.hinted(), None);

    // Off by the user: back to the plain off status, and the hint is gone.
    h.select(tracked);
    h.toggle(true);
    assert_eq!(h.view().follow().following(), Some(tracked));
    assert_eq!(h.hinted(), Some(tracked));
    let before = h.revision();
    h.toggle(false);
    assert!(!h.checked());
    assert_eq!(h.status(), OFF_STATUS);
    assert_eq!(h.view().follow().following(), None);
    assert_eq!(hint_clears_since(&h.session, before), 1);
    assert_eq!(h.hinted(), None);
    let before = h.revision();
    for _ in 0..3 {
        h.sample();
    }
    assert_eq!(promotions_since(&h.session, before), Vec::<ItemId>::new());
    assert_eq!(hints_since(&h.session, before), Vec::<ItemId>::new());
}

/// Selecting another row follows it: one hint per retarget, no promotion, and
/// nothing for a reselection of the followed row or for a sample that moves
/// it. A selection that names no shown row stops follow and clears the hint.
#[test]
fn selecting_another_row_retargets_the_hint_and_never_promotes() {
    let mut h = harness();
    let tracked = h.follow_tracked();
    let task_03 = h.item_of(6004);
    assert_eq!(h.row_of(task_03), Some(4));
    let identities = h.identities();
    let before = h.revision();
    h.select(task_03);
    assert_eq!(h.view().follow().following(), Some(task_03));
    assert_eq!(promotions_since(&h.session, before), Vec::<ItemId>::new());
    assert_eq!(hints_since(&h.session, before), vec![task_03]);
    assert_eq!(
        withdrawals_since(&h.session, before),
        0,
        "nothing to withdraw"
    );
    assert_eq!(h.hinted(), Some(task_03));
    assert_eq!(h.identities(), identities);
    assert!(
        h.status().starts_with("Following PID 6004 · task-03 · "),
        "{}",
        h.status()
    );
    // Reselecting it touches nothing; each retarget hints once.
    let before = h.revision();
    h.select(task_03);
    assert_eq!(hints_since(&h.session, before), Vec::<ItemId>::new());
    let task_04 = h.item_of(6005);
    h.select(task_04);
    h.select(task_03);
    assert_eq!(promotions_since(&h.session, before), Vec::<ItemId>::new());
    assert_eq!(hints_since(&h.session, before), vec![task_04, task_03]);

    // `tracked` leaves the top for row 56, so task-03 changes row too, one
    // up: nothing of follow's is committed, and `tracked` is never hinted.
    let before = h.revision();
    h.sample();
    assert_eq!(h.row_of(task_03), Some(3));
    assert_eq!(promotions_since(&h.session, before), Vec::<ItemId>::new());
    assert_eq!(hints_since(&h.session, before), Vec::<ItemId>::new());
    assert_eq!(h.hinted(), Some(task_03));

    // A click after a reply the client had not yet applied changes nothing:
    // there is no reply that could replace it.
    let task_05 = h.item_of(6006);
    let before = h.revision();
    h.select_at(before.saturating_sub(3), task_05);
    assert_eq!(h.view().follow().following(), Some(task_05));
    assert_eq!(promotions_since(&h.session, before), Vec::<ItemId>::new());
    assert_eq!(hints_since(&h.session, before), vec![task_05]);
    assert_ne!(h.hinted(), Some(tracked));

    // A selection that names no shown row stops follow and clears the hint.
    let before = h.revision();
    h.select(ItemId::new(9_999_999));
    assert_eq!(h.view().follow().following(), None);
    assert!(!h.checked());
    assert!(
        h.status()
            .starts_with("Follow stopped: the selection named no row"),
        "{}",
        h.status()
    );
    assert_eq!(hint_clears_since(&h.session, before), 1);
    assert_eq!(h.hinted(), None);
}

/// PR #98 review (`follow.rs:225`): after following A and clicking B, the
/// table's value still named A, so a relaunched client restored A while
/// follow described B. A promotion that names another process is withdrawn
/// whenever follow turns on or retargets, in the same commit as the hint; a
/// search promotion of the process follow moves to is kept.
#[test]
fn follow_withdraws_a_promotion_that_names_another_process() {
    let mut h = harness();
    h.sort(SORT_CPU);
    // A search promotes task-05 (PX-011), and then the user clicks `tracked`:
    // the value still names task-05.
    h.view().set_search(&h.session, "task-05").unwrap();
    let task_05 = h.item_of(6006);
    assert_eq!(h.promoted(), Some(task_05));
    let tracked = h.item_of(FollowSource::TRACKED_PID.into());
    h.select(tracked);
    assert_eq!(
        h.promoted(),
        Some(task_05),
        "follow off: unchanged (PX-011)"
    );

    // Turning on withdraws it, in the commit that sets the hint.
    let before = h.revision();
    h.toggle(true);
    let together: Vec<_> = transactions_since(&h.session, before)
        .into_iter()
        .filter(|operations| {
            clears_of(operations, VALUE) == 1 && sets_of(operations, KEEP_VISIBLE) == vec![tracked]
        })
        .collect();
    assert_eq!(together.len(), 1, "withdrawn with the hint");
    assert_eq!(promotions_since(&h.session, before), Vec::<ItemId>::new());
    assert_eq!(h.promoted(), None);
    assert_eq!(
        fresh_client(&h.session),
        Fresh {
            selection: None,
            hint: Some(tracked)
        }
    );

    // A search match while following is promoted by the search, and follow
    // moves to it: the value and the hint name the same process.
    let before = h.revision();
    h.view().set_search(&h.session, "task-03").unwrap();
    let task_03 = h.item_of(6004);
    assert_eq!(promotions_since(&h.session, before), vec![task_03]);
    assert_eq!(hints_since(&h.session, before), vec![task_03]);
    assert_eq!(withdrawals_since(&h.session, before), 0);
    assert_eq!(
        fresh_client(&h.session),
        Fresh {
            selection: Some(task_03),
            hint: Some(task_03)
        }
    );

    // Clicking another row while following withdraws the search's
    // promotion, in the commit that moves the hint.
    let task_07 = h.item_of(6008);
    let before = h.revision();
    h.select(task_07);
    let transactions = transactions_since(&h.session, before);
    assert_eq!(transactions.len(), 1);
    assert_eq!(clears_of(&transactions[0], VALUE), 1);
    assert_eq!(sets_of(&transactions[0], KEEP_VISIBLE), vec![task_07]);
    assert_eq!(promotions_since(&h.session, before), Vec::<ItemId>::new());
    assert_eq!(
        fresh_client(&h.session),
        Fresh {
            selection: None,
            hint: Some(task_07)
        }
    );
    // Nothing is withdrawn twice.
    let before = h.revision();
    h.select(task_05);
    assert_eq!(withdrawals_since(&h.session, before), 0);
}

/// A fresh client never restores a selection of a process follow is not on:
/// after turn-on, a retarget, a burst of retargets, a search match and a
/// click after it, the snapshot's promoted value names the followed process
/// or nothing.
#[test]
fn a_fresh_client_never_restores_a_selection_of_another_process() {
    let mut h = harness();
    let tracked = h.start_following_tracked();
    let check = |h: &Harness, step: &str| {
        let following = h.view().follow().following();
        assert!(following.is_some(), "{step}: following");
        let fresh = fresh_client(&h.session);
        assert!(
            fresh.selection.is_none() || fresh.selection == following,
            "{step}: a fresh client restores {:?} (PID {:?}) while follow is on {:?} (PID {:?})",
            fresh.selection,
            fresh.selection.and_then(|item| h.pid_of(item)),
            following,
            following.and_then(|item| h.pid_of(item)),
        );
    };
    check(&h, "turn-on");
    let task_03 = h.item_of(6004);
    h.select(task_03);
    check(&h, "retarget");
    // `tracked` moves to row 56; the followed row shifts up one.
    h.sample();
    check(&h, "a sample after the retarget");
    for pid in 6005..6015 {
        if pid == u64::from(FollowSource::TRACKED_PID) {
            continue;
        }
        h.select(h.item_of(pid));
        check(&h, &format!("burst, PID {pid}"));
    }
    h.sample();
    check(&h, "a sample after the burst");
    h.view().set_search(&h.session, "task-20").unwrap();
    check(&h, "search");
    h.select(tracked);
    check(&h, "a click after the search");
    h.sample();
    check(&h, "a sample after the click");
}

/// A fresh client is revealed to the followed process: the snapshot's
/// `keep_visible` hint names it after turn-on, a retarget, a burst, a search
/// match and a sample, and names nothing once follow is off.
#[test]
fn a_fresh_client_is_revealed_to_the_followed_process() {
    let mut h = harness();
    let tracked = h.start_following_tracked();
    let check = |h: &Harness, step: &str| {
        let following = h.view().follow().following();
        assert_eq!(fresh_client(&h.session).hint, following, "{step}");
    };
    check(&h, "turn-on");
    assert_eq!(fresh_client(&h.session).hint, Some(tracked));
    h.select(h.item_of(6004));
    check(&h, "retarget");
    for pid in 6005..6009 {
        h.select(h.item_of(pid));
    }
    check(&h, "burst");
    h.sample();
    check(&h, "sample");
    h.view().set_search(&h.session, "task-20").unwrap();
    check(&h, "search");
    assert_eq!(fresh_client(&h.session).hint, Some(h.item_of(6021)));
    h.toggle(false);
    assert_eq!(fresh_client(&h.session).hint, None, "off");
}

/// What a generic client holds for the table, as
/// `CollectionSelectionPromotion.swift` and `CollectionKeepVisible.swift`
/// apply it: a promotion replaces the selection; a withdrawal deselects only
/// an untouched promoted row and otherwise keeps the user's later local
/// selection (PX-011-G01); the hint never touches the selection
/// (PX-017-G01).
#[derive(Debug, Default)]
struct ClientTable {
    revision: u64,
    rows: Vec<ItemId>,
    selected: Option<ItemId>,
    promoted: Option<ItemId>,
    local_since_promotion: bool,
    hinted: Option<ItemId>,
    hint_sets: usize,
}

impl ClientTable {
    fn select_locally(&mut self, item: ItemId) {
        self.selected = Some(item);
        self.local_since_promotion = true;
    }

    fn apply(&mut self, operations: &[Operation]) {
        for operation in operations {
            match operation {
                Operation::SetProperty {
                    id,
                    property,
                    value: Value::ItemId(item),
                } if *id == TABLE && *property == VALUE => {
                    self.selected = Some(*item);
                    self.promoted = Some(*item);
                    self.local_since_promotion = false;
                }
                Operation::ClearProperty { id, property } if *id == TABLE && *property == VALUE => {
                    let previous = self.promoted.take();
                    if !self.local_since_promotion
                        && previous.is_some()
                        && self.selected == previous
                    {
                        self.selected = None;
                    }
                }
                Operation::SetProperty {
                    id,
                    property,
                    value: Value::ItemId(item),
                } if *id == TABLE && *property == KEEP_VISIBLE => {
                    self.hinted = Some(*item);
                    self.hint_sets += 1;
                }
                Operation::ClearProperty { id, property }
                    if *id == TABLE && *property == KEEP_VISIBLE =>
                {
                    self.hinted = None;
                }
                _ => {}
            }
        }
    }
}

/// One thing that happens in [`simulate`].
#[derive(Debug, Clone, Copy)]
enum Happening {
    /// The user presses Down: the row after the client's selection.
    Down,
    /// The user clicks the row holding this PID.
    Click(u64),
    /// The server takes one sample.
    Sample,
}

#[derive(Debug, Default)]
struct Outcome {
    /// The item each press or click selected, in order.
    pressed: Vec<ItemId>,
    /// Deliveries that changed the client's selection.
    changed_by_commit: usize,
    /// Deliveries after which the client's selection was a row pressed
    /// before the latest press.
    backward_jumps: usize,
    /// Presses whose starting row was not the row the previous press chose.
    displaced_presses: usize,
    final_selected: Option<ItemId>,
}

/// Runs `script` (time in ms, what happens) against a real view with
/// `latency` ms each way: every client selection reaches the server
/// `latency` after it is made, carrying the revision the client held, and
/// every commit reaches the client `latency` after it is made. The client
/// starts with `selected` selected. Returns the client's table and the
/// outcome.
fn simulate(
    h: &mut Harness,
    selected: ItemId,
    script: &[(u64, Happening)],
    latency: u64,
) -> (ClientTable, Outcome) {
    #[derive(Debug)]
    enum Event {
        User(Happening),
        Server(ItemId, u64),
        Deliver(u64, Vec<Operation>, Vec<ItemId>),
    }
    let mut client = ClientTable {
        revision: h.revision(),
        rows: h.rows().into_iter().map(|(id, _, _)| id).collect(),
        selected: Some(selected),
        promoted: h.promoted(),
        hinted: h.hinted(),
        ..ClientTable::default()
    };
    let mut queue: Vec<(u64, usize, Event)> = script
        .iter()
        .enumerate()
        .map(|(order, (time, what))| (*time, order, Event::User(*what)))
        .collect();
    let mut order = queue.len();
    let mut outcome = Outcome::default();
    while let Some(next) = (0..queue.len()).min_by_key(|index| (queue[*index].0, queue[*index].1)) {
        let (time, _, event) = queue.remove(next);
        let mut deliver = |h: &mut Harness, before: u64, queue: &mut Vec<(u64, usize, Event)>| {
            if h.revision() == before {
                return;
            }
            let operations = operations_since(&h.session, before);
            let rows = h.rows().into_iter().map(|(id, _, _)| id).collect();
            queue.push((
                time + latency,
                order,
                Event::Deliver(h.revision(), operations, rows),
            ));
            order += 1;
        };
        match event {
            Event::User(Happening::Sample) => {
                let before = h.revision();
                h.sample();
                deliver(h, before, &mut queue);
            }
            Event::User(action) => {
                if outcome
                    .pressed
                    .last()
                    .is_some_and(|last| client.selected != Some(*last))
                {
                    outcome.displaced_presses += 1;
                }
                let current = client
                    .selected
                    .and_then(|item| client.rows.iter().position(|row| *row == item));
                let target = match action {
                    Happening::Down => {
                        current.map_or(0, |row| (row + 1).min(client.rows.len() - 1))
                    }
                    Happening::Click(pid) => h.row_of(h.item_of(pid)).unwrap(),
                    Happening::Sample => unreachable!(),
                };
                let item = client.rows[target];
                client.select_locally(item);
                outcome.pressed.push(item);
                queue.push((time + latency, order, Event::Server(item, client.revision)));
                order += 1;
            }
            Event::Server(item, observed) => {
                let before = h.revision();
                h.select_at(observed, item);
                deliver(h, before, &mut queue);
            }
            Event::Deliver(revision, operations, rows) => {
                let before = client.selected;
                client.rows = rows;
                client.apply(&operations);
                client.revision = revision;
                if client.selected != before {
                    outcome.changed_by_commit += 1;
                    let earlier = &outcome.pressed[..outcome.pressed.len().saturating_sub(1)];
                    if client.selected.is_some_and(|now| earlier.contains(&now)) {
                        outcome.backward_jumps += 1;
                    }
                }
            }
        }
    }
    outcome.final_selected = client.selected;
    (client, outcome)
}

/// Rapid arrow keys while following, with and without samples moving rows
/// mid-burst (the CPU sort is active, and the first sample moves `tracked`
/// from the top to row 56, shifting every row between): whatever the key
/// repeat and the round trip, no commit changes the client's selection, so
/// every press moves on from the last one and the burst ends on the last
/// press. The server follows that row, hints each retarget once, promotes
/// nothing, and a fresh client restores no other process and is revealed to
/// the last one.
#[test]
fn rapid_selections_while_following_end_on_the_last_press_with_samples_mid_burst() {
    const PRESSES: u64 = 20;
    let mut cases = Vec::new();
    for interval in [30, 80] {
        for rtt in [50, 100] {
            cases.push((interval, rtt, Vec::new()));
            // Samples land at press 5 and press 12, while earlier presses are
            // still in flight.
            cases.push((interval, rtt, vec![5 * interval + 1, 12 * interval + 1]));
        }
    }
    // Faster and slower edges.
    cases.push((1, 50, vec![3, 9]));
    cases.push((10, 100, vec![25, 95]));
    cases.push((30, 10, vec![61]));
    let mut measured = Vec::new();
    for (interval, rtt, samples) in cases {
        let mut h = harness();
        let tracked = h.start_following_tracked();
        let mut script: Vec<(u64, Happening)> = (0..PRESSES)
            .map(|press| (press * interval, Happening::Down))
            .collect();
        script.extend(samples.iter().map(|at| (*at, Happening::Sample)));
        let start = h.revision();
        let (client, outcome) = simulate(&mut h, tracked, &script, rtt / 2);
        let label = format!(
            "key repeat {interval} ms, RTT {rtt} ms, samples at {samples:?} ms, {PRESSES} presses"
        );
        let last = *outcome.pressed.last().unwrap();
        let promotions = promotions_since(&h.session, start);
        let hints = hints_since(&h.session, start);
        let withdrawals = withdrawals_since(&h.session, start);
        let fresh = fresh_client(&h.session);
        println!(
            "PX-017 rapid selections {label}: final PID {:?} last PID {:?} changedByCommit={} backwardJumps={} displacedPresses={} promotions={} hintSets={} withdrawals={} fresh={fresh:?}",
            outcome.final_selected.and_then(|item| h.pid_of(item)),
            h.pid_of(last),
            outcome.changed_by_commit,
            outcome.backward_jumps,
            outcome.displaced_presses,
            promotions.len(),
            hints.len(),
            withdrawals,
        );
        // The samples really moved rows under the burst.
        let moved = h.row_of(tracked) != Some(FollowSource::RANKS[0]);
        assert_eq!(moved, !samples.is_empty(), "{label}");
        let following = h.view().follow().following();
        measured.push((
            label,
            outcome,
            last,
            h.pid_of(last),
            following,
            promotions,
            hints,
            withdrawals,
            client.hinted,
            fresh,
        ));
    }
    // The selection is the user's, in every case, before anything else.
    for (label, outcome, last, last_pid, following, ..) in &measured {
        assert_eq!(outcome.changed_by_commit, 0, "{label}: {outcome:?}");
        let distinct: BTreeSet<ItemId> = outcome.pressed.iter().copied().collect();
        assert_eq!(
            distinct.len(),
            PRESSES as usize,
            "{label}: each press moved on"
        );
        assert_eq!(outcome.backward_jumps, 0, "{label}: {outcome:?}");
        assert_eq!(outcome.displaced_presses, 0, "{label}: {outcome:?}");
        assert_eq!(outcome.final_selected, Some(*last), "{label}: {outcome:?}");
        // Every press is the next row: o_0 … o_19, whose PIDs skip `tracked`.
        assert_eq!(*last_pid, Some(6021), "{label}");
        assert_eq!(*following, Some(*last), "{label}");
    }
    // Then the wire: no promotion, one hint per retarget, and a fresh client
    // that restores nothing and is revealed to the last press.
    for (label, outcome, last, _, _, promotions, hints, withdrawals, hinted, fresh) in &measured {
        assert_eq!(
            *promotions,
            Vec::<ItemId>::new(),
            "{label}: follow never promotes"
        );
        assert_eq!(*hints, outcome.pressed, "{label}: one hint per retarget");
        assert_eq!(*withdrawals, 0, "{label}");
        assert_eq!(*hinted, Some(*last), "{label}");
        assert_eq!(
            *fresh,
            Fresh {
                selection: None,
                hint: Some(*last)
            },
            "{label}"
        );
    }
}

/// With follow off the same bursts are untouched as well: nothing is hinted
/// or promoted at all.
#[test]
fn rapid_selections_with_follow_off_hint_and_promote_nothing() {
    for (interval, rtt) in [(30, 50), (80, 100)] {
        let mut h = harness();
        h.sort(SORT_CPU);
        let tracked = h.item_of(FollowSource::TRACKED_PID.into());
        h.select(tracked);
        let mut script: Vec<(u64, Happening)> = (0..20)
            .map(|press| (press * interval, Happening::Down))
            .collect();
        script.push((5 * interval + 1, Happening::Sample));
        let start = h.revision();
        let (_, outcome) = simulate(&mut h, tracked, &script, rtt / 2);
        assert_eq!(outcome.changed_by_commit, 0);
        assert_eq!(outcome.final_selected, outcome.pressed.last().copied());
        assert_eq!(promotions_since(&h.session, start), Vec::<ItemId>::new());
        assert_eq!(hints_since(&h.session, start), Vec::<ItemId>::new());
        assert_eq!(h.hinted(), None);
        println!(
            "PX-017 rapid selections with follow off, key repeat {interval} ms, RTT {rtt} ms: promotions=0 hintSets=0 changedByCommit={}",
            outcome.changed_by_commit
        );
    }
}

/// A click that crosses a sample moving the followed process ends on the
/// click, for this client and a fresh one, at every latency.
#[test]
fn a_click_crossing_a_sample_ends_on_the_click() {
    for latency in [5, 25, 50, 100] {
        let mut h = harness();
        let tracked = h.start_following_tracked();
        // The click on task-59 at 0 ms; the sample that moves `tracked` to
        // row 56 at 1 ms, before the click reaches the server.
        let script = [(0, Happening::Click(6060)), (1, Happening::Sample)];
        let start = h.revision();
        let (client, outcome) = simulate(&mut h, tracked, &script, latency);
        let task_59 = h.item_of(6060);
        assert_eq!(
            client.selected,
            Some(task_59),
            "latency {latency}: {outcome:?}"
        );
        assert_eq!(outcome.changed_by_commit, 0, "latency {latency}");
        assert_eq!(h.view().follow().following(), Some(task_59));
        assert_eq!(
            fresh_client(&h.session),
            Fresh {
                selection: None,
                hint: Some(task_59)
            }
        );
        assert_ne!(client.selected, Some(tracked));
        assert_eq!(promotions_since(&h.session, start), Vec::<ItemId>::new());
        assert_eq!(hints_since(&h.session, start), vec![task_59]);
    }
}

/// Follow and Pause view (PX-015): a paused view publishes no sample, so
/// nothing of follow's is committed for one; pause and resume alone commit
/// nothing of follow's; a user sort while paused that moves the followed row
/// keeps the hint on it (the client reveals it because it moved) and still
/// promotes nothing.
#[test]
fn follow_commits_nothing_while_paused_and_a_paused_sort_keeps_the_hint() {
    let mut h = harness();
    let tracked = h.follow_tracked();
    // Step 1: `tracked` moves to row 56.
    let before = h.revision();
    h.sample();
    assert_eq!(h.row_of(tracked), Some(55));
    assert_eq!(promotions_since(&h.session, before), Vec::<ItemId>::new());

    // Pause; a sample that reaches the paused view (step 3 would move the
    // row to 41) is discarded.
    let before = h.revision();
    h.view().pause_view(&h.session).unwrap();
    assert!(h.view().frozen().is_some());
    let paused = snapshot_at(3);
    h.view()
        .apply(&h.session, FOLLOW_STATUS_TEXT, &paused)
        .unwrap();
    assert_eq!(h.row_of(tracked), Some(55), "a paused view keeps its rows");
    assert!(h.checked());

    // Resume; the first sample (step 2) keeps the row at 56.
    h.view().resume_view(&h.session).unwrap();
    let resumed = snapshot_at(2);
    h.view()
        .apply(&h.session, FOLLOW_STATUS_TEXT, &resumed)
        .unwrap();
    assert_eq!(h.row_of(tracked), Some(55));
    assert_eq!(
        promotions_since(&h.session, before),
        Vec::<ItemId>::new(),
        "pause and resume alone promote nothing"
    );
    assert_eq!(hints_since(&h.session, before), Vec::<ItemId>::new());
    assert_eq!(hint_clears_since(&h.session, before), 0);

    // Pause again; a sort by PID while paused moves the row to 10: the hint
    // stays on it, nothing is promoted or hinted again.
    h.view().pause_view(&h.session).unwrap();
    let before = h.revision();
    h.sort(SORT_PID);
    assert_eq!(h.row_of(tracked), Some(9));
    assert!(h.revision() > before, "the sort committed while paused");
    assert_eq!(promotions_since(&h.session, before), Vec::<ItemId>::new());
    assert_eq!(hints_since(&h.session, before), Vec::<ItemId>::new());
    assert_eq!(h.hinted(), Some(tracked));
    let before = h.revision();
    h.view().resume_view(&h.session).unwrap();
    h.view()
        .apply(&h.session, FOLLOW_STATUS_TEXT, &snapshot_at(3))
        .unwrap();
    assert_eq!(h.row_of(tracked), Some(9));
    h.sort(SORT_CPU);
    assert_eq!(h.row_of(tracked), Some(FollowSource::RANKS[3]));
    assert_eq!(promotions_since(&h.session, before), Vec::<ItemId>::new());
    assert_eq!(hints_since(&h.session, before), Vec::<ItemId>::new());
    assert_eq!(h.view().follow().following(), Some(tracked));
    assert_eq!(h.hinted(), Some(tracked));
}

/// A hint that fails to commit is repaired by the next plan, which sets it
/// even though the followed row did not move: the turn-on below fails
/// because the table briefly has no `model_ref`, so the store refuses
/// `keep_visible` (`invalid_keep_visible`, PX-017-G01).
#[test]
fn a_hint_that_did_not_commit_is_set_by_the_next_plan() {
    let mut h = harness();
    h.sort(SORT_CPU);
    let tracked = h.item_of(FollowSource::TRACKED_PID.into());
    h.select(tracked);
    let model_ref = h.property(TABLE, MODEL_REF).expect("a model_ref");
    h.session
        .transaction(|ui| {
            ui.clear(TABLE, MODEL_REF)?;
            Ok(())
        })
        .unwrap();
    let before = h.revision();
    h.toggle(true);
    assert_eq!(h.revision(), before, "the turn-on did not commit");
    assert_eq!(h.view().follow().following(), Some(tracked));
    assert_eq!(h.hinted(), None);
    h.session
        .transaction(|ui| {
            ui.set(TABLE, MODEL_REF, model_ref.clone())?;
            Ok(())
        })
        .unwrap();
    let before = h.revision();
    // Step 1 moves `tracked`; the repair names it at its new row.
    h.sample();
    assert_eq!(hints_since(&h.session, before), vec![tracked]);
    assert_eq!(promotions_since(&h.session, before), Vec::<ItemId>::new());
    assert_eq!(h.hinted(), Some(tracked));
    assert!(h.checked(), "the checkbox is published too");
    let before = h.revision();
    h.sample();
    assert_eq!(hints_since(&h.session, before), Vec::<ItemId>::new());
}

/// The fixture's snapshot for `step`, independent of the harness's source.
fn snapshot_at(step: usize) -> srui_process_explorer::source::ProcessSnapshot {
    let mut source = FollowSource::default();
    for _ in 0..step {
        source.snapshot();
    }
    source.snapshot()
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
