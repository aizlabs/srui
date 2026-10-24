//! PX-068 acceptance: server-authoritative column choice for the process table.
//!
//! * The visible columns are a sequence the server owns: showing, hiding and
//!   reordering columns republishes the table's headings and every row's cells
//!   in exactly that sequence, and restoring defaults brings back the catalogued
//!   process list.
//! * A layout change is a change of presentation, not of identity: every
//!   process keeps its item ID, no revision a client can hold lacks a row it
//!   had selected, and the active sort survives even when its column is hidden.
//! * Later refreshes publish in the chosen layout.
//! * The chooser is usable: its published controls, activated through the
//!   session exactly as a client's clicks arrive, hide, show, reorder and
//!   restore the columns, and a replayed click changes nothing.
//! * The table and its model are never recreated by a layout change, so a
//!   client's native selection, anchored to the table's item IDs, survives it:
//!   a client selection that applies every committed transaction as the macOS
//!   table adapter does still holds the same process afterwards.
//!
//! These tests drive the published session through `ProcessView`, as the sort
//! and filter suites do, and through the session's event path. Chooser
//! controls are found by what a user reads on them, not by node ID: an enabled
//! `Button` whose label names the column (its sort-button name or its heading)
//! and the action ("hide", "show", "left"/"earlier", "right"/"later"), or, for
//! restoring, "restore"/"reset" with "default".

use srui_process_explorer::catalogue::{ProcessField, SortDirection, PROCESS_LIST};
use srui_process_explorer::refresh::ProcessView;
use srui_process_explorer::sort::SortOrder;
use srui_process_explorer::source::*;
use srui_process_explorer::{install_sort_handlers, start_from_source, MODEL, SORT_CPU, TABLE};
use srui_protocol::{operation, Event as WireEvent};
use srui_sdk::{
    Button, EnumToken, ItemId, NodeId, SelectionMode, Value, COLUMNS, ENABLED, LABEL,
    SELECTION_MODE,
};
use srui_semantic_tree::{Event as SemanticEvent, SemanticStore};
use srui_sessiond::{EventOutcome, Session};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

const SOURCE: &str = "columns-fixture-v1";

/// Hiding a wide field must not admit rows that restoring it would evict.
#[test]
fn wide_columns_keep_snapshot_delivery_membership_and_selection_after_restore() {
    let name = "\u{20000}".repeat(MAX_DISPLAY_NAME_CHARS);
    let population: Vec<_> = (1..=34_000)
        .map(|pid| record(pid, &name, 0, CpuUsage::Missing(MissingReason::Unavailable)))
        .collect();
    let (session, mut view, mut source) = started(vec![population]);
    let original = view.item_ids();
    assert!(original.len() > 20_000 && original.len() < 34_000);
    let selected = *original.last().unwrap();
    let mut client = ClientSelection::select(&session, selected);

    let assert_deliverable = || {
        let hello = srui_protocol::ClientHello {
            core_version: "0.5.0".into(),
            profiles: vec!["org.srui.standard-widgets/1".into()],
            client_instance_id: vec![1, 2, 3],
            ..Default::default()
        };
        let snapshot = session
            .bootstrap_fresh_client(&hello)
            .expect("a client without multi-envelope snapshots can attach")
            .snapshot
            .expect("the populated session has a snapshot");
        let bytes = srui_protocol::framed_payload_len(&srui_protocol::SruiMessage {
            msg: Some(srui_protocol::srui_message::Msg::Transaction(snapshot)),
        });
        assert!(bytes <= srui_protocol::DEFAULT_MAX_FRAME_SIZE);
    };
    assert_deliverable();
    view.set_columns(
        &session,
        &[ProcessField::Pid, ProcessField::Resident, ProcessField::Cpu],
    )
    .unwrap();
    assert_eq!(client.follow(&session), Some(selected));
    view.refresh(&session, &mut source).unwrap();
    assert_eq!(client.follow(&session), Some(selected));
    assert!(
        view.item_ids() == original,
        "hiding must not expand membership"
    );
    assert_deliverable();

    view.restore_default_columns(&session).unwrap();
    assert_eq!(client.follow(&session), Some(selected));
    assert!(view.item_ids() == original);
    assert_deliverable();
    view.refresh(&session, &mut source).unwrap();
    assert_eq!(client.follow(&session), Some(selected));
    assert!(
        view.item_ids() == original,
        "the next refresh must not evict rows"
    );
    assert_deliverable();
}

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

/// A share of one CPU measured over one second, in percent.
fn cpu(percent: u64) -> CpuUsage {
    CpuUsage::Measured(CpuInterval {
        ticks: percent,
        ticks_per_second: 100,
        elapsed: Duration::from_secs(1),
    })
}

fn record(pid: u32, name: &str, resident: u64, cpu: CpuUsage) -> ProcessRecord {
    ProcessRecord {
        key: key(pid, name),
        display_name: name.into(),
        resident: Observed::Known(resident),
        cpu,
    }
}

/// Three processes with distinct PIDs, names, memory and CPU, so every cell
/// names its column and its row unambiguously:
///
/// | PID | name | resident | CPU |
/// | --- | --- | --- | --- |
/// | 7 | `init` | 1.0 KiB | 5.0% |
/// | 42 | `shell` | 9 B | 50.0% |
/// | 300 | `daemon` | 2.0 KiB | 20.0% |
fn population() -> Vec<ProcessRecord> {
    vec![
        record(7, "init", 1024, cpu(5)),
        record(42, "shell", 9, cpu(50)),
        record(300, "daemon", 2048, cpu(20)),
    ]
}

/// The text of `field`'s cell for the fixture process called `name`, in
/// `population()`.
fn expected_cell(name: &str, field: ProcessField) -> String {
    let (pid, resident, cpu) = match name {
        "init" => ("7", "1.0 KiB", "5.0%"),
        "shell" => ("42", "9 B", "50.0%"),
        "daemon" => ("300", "2.0 KiB", "20.0%"),
        other => panic!("not a fixture process: {other}"),
    };
    match field {
        ProcessField::Pid => pid.into(),
        ProcessField::Name => name.into(),
        ProcessField::Resident => resident.into(),
        ProcessField::Cpu => cpu.into(),
    }
}

fn expected_cells(name: &str, layout: &[ProcessField]) -> Vec<String> {
    layout
        .iter()
        .map(|field| expected_cell(name, *field))
        .collect()
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

fn started(steps: Vec<Vec<ProcessRecord>>) -> (Session, ProcessView, Steps) {
    let session = Session::mint();
    let mut source = Steps { steps, next: 0 };
    let (view, _) = start_from_source(&session, &mut source).expect("the fixture publishes");
    (session, view, source)
}

fn text(cell: &Value) -> String {
    match cell {
        Value::UnsignedInt(number) => number.to_string(),
        Value::String(text) => text.clone(),
        other => panic!("unexpected cell {other:?}"),
    }
}

/// Every published row, in published order: its item ID and its cells as text.
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
                (item.item_id, cells.iter().map(text).collect())
            })
            .collect()
    })
}

/// The table's published column headings.
fn headings(session: &Session) -> Vec<String> {
    session.with_store(|store| {
        match store
            .get_node(TABLE)
            .expect("the table exists")
            .get_property(COLUMNS)
        {
            Some(Value::List(columns)) => columns.iter().map(text).collect(),
            other => panic!("the table has no column list: {other:?}"),
        }
    })
}

fn headings_of(layout: &[ProcessField]) -> Vec<String> {
    layout
        .iter()
        .map(|field| field.heading().to_string())
        .collect()
}

/// Asserts the table publishes exactly `layout`: its headings, and every row's
/// cells in that sequence, each the cell of the fixture process the row was
/// published for, under the item ID that process held in `by_name`.
fn assert_published_layout(
    session: &Session,
    layout: &[ProcessField],
    by_name: &HashMap<String, ItemId>,
) {
    assert_eq!(
        headings(session),
        headings_of(layout),
        "headings of {layout:?}"
    );
    let published = rows(session);
    assert_eq!(
        published.len(),
        by_name.len(),
        "every process is still published"
    );
    let names: HashMap<ItemId, &String> = by_name.iter().map(|(name, id)| (*id, name)).collect();
    for (item_id, cells) in published {
        let name = names
            .get(&item_id)
            .unwrap_or_else(|| panic!("{item_id:?} is not an item ID the process held"));
        assert_eq!(
            cells,
            expected_cells(name, layout),
            "row of {name} under {layout:?}"
        );
    }
}

/// Each process's item ID by name, read while the default layout (whose
/// second cell is the name) is published.
fn identities_by_default_name(session: &Session) -> HashMap<String, ItemId> {
    rows(session)
        .into_iter()
        .map(|(id, cells)| (cells[1].clone(), id))
        .collect()
}

/// Asserts no transaction committed after `revision` deleted a row it did not
/// reinsert under the same item ID: a client's selection, anchored to item IDs,
/// is never left without its row.
fn assert_no_row_lost_since(session: &Session, revision: u64) {
    for transaction in session
        .collect_replayed_transactions(revision)
        .expect("the journal holds this run")
    {
        let mut deleted = Vec::new();
        let mut inserted = Vec::new();
        for op in &transaction.operations {
            match &op.op {
                Some(operation::Op::ModelDelete(delete)) => {
                    deleted.extend(delete.item_ids.iter().copied())
                }
                Some(operation::Op::ModelInsert(insert)) => {
                    inserted.extend(insert.items.iter().map(|item| item.item_id))
                }
                _ => {}
            }
        }
        let inserted: HashSet<u64> = inserted.into_iter().collect();
        for id in deleted {
            assert!(
                inserted.contains(&id),
                "revision {} removed item {id} and did not put it back",
                transaction.new_revision
            );
        }
    }
}

fn names_in_order(session: &Session, name_column: usize) -> Vec<String> {
    rows(session)
        .into_iter()
        .map(|(_, cells)| cells[name_column].clone())
        .collect()
}

/// The layout the table publishes, read back from its headings.
fn published_layout(session: &Session) -> Vec<ProcessField> {
    headings(session)
        .iter()
        .map(|heading| {
            *PROCESS_LIST
                .iter()
                .find(|field| field.heading() == heading)
                .unwrap_or_else(|| panic!("{heading:?} is not a catalogued heading"))
        })
        .collect()
}

/// Asserts no transaction committed after `revision` deleted or recreated the
/// table node or recreated its model, and that the table still offers its
/// single native selection over `MODEL`: a client's selection lives on that
/// table and is anchored to that model's item IDs, so neither may be replaced
/// by a change of columns.
fn assert_table_and_model_kept_since(session: &Session, revision: u64) {
    for transaction in session
        .collect_replayed_transactions(revision)
        .expect("the journal holds this run")
    {
        for op in &transaction.operations {
            match &op.op {
                Some(operation::Op::DeleteNode(delete)) => assert_ne!(
                    delete.node_id,
                    TABLE.get(),
                    "revision {} deleted the table",
                    transaction.new_revision
                ),
                Some(operation::Op::CreateNode(create)) => assert_ne!(
                    create.node.as_ref().map(|node| node.node_id),
                    Some(TABLE.get()),
                    "revision {} recreated the table",
                    transaction.new_revision
                ),
                Some(operation::Op::CreateModel(create)) => assert_ne!(
                    create.model_id,
                    MODEL.get(),
                    "revision {} recreated the table's model",
                    transaction.new_revision
                ),
                _ => {}
            }
        }
    }
    session.with_store(|store| {
        let table = store.get_node(TABLE).expect("the table exists");
        assert_eq!(
            table.get_property(SELECTION_MODE).cloned(),
            Some(Value::EnumToken(EnumToken::from(SelectionMode::Single))),
            "the table still offers its native selection"
        );
        assert!(
            store.get_model_for_node(TABLE).is_some(),
            "the table is still bound to its model"
        );
    });
}

/// A native client's single table selection, kept the way the macOS table
/// adapter keeps it (`TableCollectionAdapter`): the client holds a replica of
/// the published tree, applies each committed transaction whole, and then
/// restores its selection by item ID. The selection is lost when its row is
/// gone, when the table is deleted or recreated (a new native table starts
/// with nothing selected), or when the table stops offering a single
/// selection.
struct ClientSelection {
    replica: SemanticStore,
    selected: Option<ItemId>,
}

impl ClientSelection {
    /// A client that has applied every committed transaction selects `item`,
    /// a row it is showing, in the native table.
    fn select(session: &Session, item: ItemId) -> Self {
        let mut client = Self {
            replica: SemanticStore::new(),
            selected: None,
        };
        client.follow(session);
        assert!(
            client.offers_selection(),
            "the client's table offers a native selection"
        );
        assert!(client.shows(item), "{item:?} is not a row the client shows");
        client.selected = Some(item);
        client
    }

    fn offers_selection(&self) -> bool {
        self.replica
            .get_node(TABLE)
            .and_then(|table| table.get_property(SELECTION_MODE))
            == Some(&Value::EnumToken(EnumToken::from(SelectionMode::Single)))
    }

    fn shows(&self, item: ItemId) -> bool {
        self.replica
            .get_model_for_node(TABLE)
            .is_some_and(|model| model.items.values().any(|row| row.item_id == item))
    }

    /// Applies, as the client does, every transaction committed since the
    /// client last caught up, and returns what is selected afterwards.
    fn follow(&mut self, session: &Session) -> Option<ItemId> {
        for transaction in session
            .collect_replayed_transactions(self.replica.revision().get())
            .expect("the journal holds this run")
        {
            let replaces_table = transaction.operations.iter().any(|op| match &op.op {
                Some(operation::Op::DeleteNode(delete)) => delete.node_id == TABLE.get(),
                Some(operation::Op::CreateNode(create)) => {
                    create.node.as_ref().map(|node| node.node_id) == Some(TABLE.get())
                }
                _ => false,
            });
            let revision = transaction.new_revision;
            self.replica
                .apply_wire_transaction(transaction)
                .unwrap_or_else(|error| {
                    panic!("the client cannot apply revision {revision}: {error}")
                });
            if let Some(selected) = self.selected {
                if replaces_table || !self.offers_selection() || !self.shows(selected) {
                    self.selected = None;
                }
            }
        }
        assert_eq!(
            self.replica.revision().get(),
            session.current_revision(),
            "the client caught up"
        );
        let replica_rows: Vec<ItemId> = self
            .replica
            .get_model(MODEL)
            .map(|model| model.items.values().map(|row| row.item_id).collect())
            .unwrap_or_default();
        assert_eq!(
            replica_rows,
            rows(session)
                .into_iter()
                .map(|(id, _)| id)
                .collect::<Vec<_>>(),
            "the client's rows are the rows the server publishes"
        );
        self.selected
    }
}

/// The names of the published rows, in published order, wherever the `Name`
/// column currently is.
fn names_in_published_order(session: &Session) -> Vec<String> {
    let column = published_layout(session)
        .iter()
        .position(|field| *field == ProcessField::Name)
        .expect("the Name column is shown");
    names_in_order(session, column)
}

/// Every button in the published tree: its node, its label, and whether it is
/// enabled.
fn buttons(session: &Session) -> Vec<(NodeId, String, bool)> {
    session.with_store(|store| {
        let mut found = Vec::new();
        let mut pending: Vec<NodeId> = store.root_ids().to_vec();
        while let Some(id) = pending.pop() {
            let node = store.get_node(id).expect("a listed node exists");
            if node.node_type == Button::NODE_TYPE {
                let label = match node.get_property(LABEL) {
                    Some(Value::String(label)) => label.clone(),
                    _ => String::new(),
                };
                let enabled = !matches!(node.get_property(ENABLED), Some(Value::Bool(false)));
                found.push((id, label, enabled));
            }
            if let Some(children) = store.children_of(id) {
                pending.extend(children.iter().copied());
            }
        }
        found
    })
}

fn names_column(label: &str, field: ProcessField) -> bool {
    label.contains(&field.button_name().to_lowercase())
        || label.contains(&field.heading().to_lowercase())
}

/// The enabled chooser control for `field` whose label states one of `verbs`.
fn chooser_control(session: &Session, field: ProcessField, verbs: &[&str]) -> NodeId {
    let all = buttons(session);
    let matching: Vec<NodeId> = all
        .iter()
        .filter(|(_, label, enabled)| {
            let label = label.to_lowercase();
            *enabled
                && !label.contains("sort")
                && names_column(&label, field)
                && verbs.iter().any(|verb| label.contains(verb))
        })
        .map(|(id, _, _)| *id)
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "expected exactly one enabled column chooser button for {field:?} labelled with \
         one of {verbs:?}; published buttons: {all:?}"
    );
    matching[0]
}

/// The enabled chooser control that restores the default columns.
fn restore_control(session: &Session) -> NodeId {
    let all = buttons(session);
    let matching: Vec<NodeId> = all
        .iter()
        .filter(|(_, label, enabled)| {
            let label = label.to_lowercase();
            *enabled
                && !label.contains("sort")
                && label.contains("default")
                && (label.contains("restore") || label.contains("reset"))
        })
        .map(|(id, _, _)| *id)
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "expected exactly one enabled column chooser button restoring the default \
         columns; published buttons: {all:?}"
    );
    matching[0]
}

const HIDE: &[&str] = &["hide"];
const SHOW: &[&str] = &["show"];
const EARLIER: &[&str] = &["left", "earlier"];
const LATER: &[&str] = &["right", "later"];

/// A client's `ACTIVATE` on `node`, as the wire carries it.
fn activate(node: NodeId, seq: u64, observed_revision: u64) -> WireEvent {
    WireEvent::from(
        &SemanticEvent::activate(seq, format!("column-click-{seq}"), observed_revision, node)
            .with_client_instance_id("px068-columns-test"),
    )
}

/// Sends a click on `node` through the session and asserts it was applied and
/// published before it was acknowledged.
fn click(session: &Session, node: NodeId, seq: u64) {
    let before = session.current_revision();
    let outcome = session
        .process_event(&activate(node, seq, before))
        .expect("the session accepts the event");
    let revision_after_effect = match outcome {
        EventOutcome::Processed {
            revision_after_effect,
            ..
        } => revision_after_effect,
        other => panic!("the click on {node:?} was not processed: {other:?}"),
    };
    assert!(
        revision_after_effect > before,
        "the chooser click on {node:?} published nothing"
    );
    assert_eq!(
        revision_after_effect,
        session.current_revision(),
        "the acknowledgement holds the new layout"
    );
}

fn position(layout: &[ProcessField], field: ProcessField) -> usize {
    layout
        .iter()
        .position(|shown| *shown == field)
        .unwrap_or_else(|| panic!("{field:?} is not in {layout:?}"))
}

/// Asserts `sub` keeps the relative order it has in `layout`.
fn keeps_relative_order(layout: &[ProcessField], sub: &[ProcessField]) {
    let filtered: Vec<ProcessField> = layout.iter().copied().filter(|f| sub.contains(f)).collect();
    assert_eq!(filtered, sub, "{layout:?} reordered {sub:?}");
}

#[test]
fn the_default_layout_is_the_catalogued_process_list() {
    let (session, view, _) = started(vec![population()]);
    assert_eq!(view.columns(), PROCESS_LIST.to_vec());
    let ids = identities_by_default_name(&session);
    assert_published_layout(&session, &PROCESS_LIST, &ids);
}

#[test]
fn hiding_a_column_drops_its_heading_and_its_cell_from_every_row() {
    let (session, mut view, _) = started(vec![population()]);
    let ids = identities_by_default_name(&session);
    let revision = session.current_revision();
    let layout = [ProcessField::Pid, ProcessField::Name, ProcessField::Cpu];
    view.set_columns(&session, &layout)
        .expect("hiding Resident publishes");
    assert!(
        session.current_revision() > revision,
        "a layout change is published"
    );
    assert_eq!(view.columns(), layout.to_vec());
    assert_published_layout(&session, &layout, &ids);
    assert_eq!(
        view.item_ids(),
        rows(&session).iter().map(|(id, _)| *id).collect::<Vec<_>>()
    );
    assert_no_row_lost_since(&session, revision);
    assert_table_and_model_kept_since(&session, revision);
}

#[test]
fn reordering_columns_projects_cells_in_the_new_sequence() {
    let (session, mut view, _) = started(vec![population()]);
    let ids = identities_by_default_name(&session);
    let revision = session.current_revision();
    let layout = [
        ProcessField::Cpu,
        ProcessField::Name,
        ProcessField::Pid,
        ProcessField::Resident,
    ];
    view.set_columns(&session, &layout)
        .expect("a reorder publishes");
    assert_eq!(view.columns(), layout.to_vec());
    assert_published_layout(&session, &layout, &ids);
    // A reorder alone moves no row: the rows keep their published order.
    assert_eq!(names_in_order(&session, 1), ["init", "shell", "daemon"]);
    assert_no_row_lost_since(&session, revision);
    assert_table_and_model_kept_since(&session, revision);
}

#[test]
fn a_hidden_column_can_be_shown_again_at_a_chosen_position() {
    let (session, mut view, _) = started(vec![population()]);
    let ids = identities_by_default_name(&session);
    let hidden = [ProcessField::Name, ProcessField::Cpu];
    view.set_columns(&session, &hidden)
        .expect("hiding publishes");
    assert_published_layout(&session, &hidden, &ids);

    let revision = session.current_revision();
    let shown = [
        ProcessField::Resident,
        ProcessField::Name,
        ProcessField::Pid,
        ProcessField::Cpu,
    ];
    view.set_columns(&session, &shown)
        .expect("showing publishes");
    assert_eq!(view.columns(), shown.to_vec());
    assert_published_layout(&session, &shown, &ids);
    assert_no_row_lost_since(&session, revision);
}

#[test]
fn restoring_defaults_brings_back_the_catalogued_layout() {
    let (session, mut view, _) = started(vec![population()]);
    let ids = identities_by_default_name(&session);
    view.set_columns(&session, &[ProcessField::Cpu, ProcessField::Pid])
        .expect("a custom layout publishes");
    assert_eq!(
        headings(&session),
        headings_of(&[ProcessField::Cpu, ProcessField::Pid])
    );

    let revision = session.current_revision();
    view.restore_default_columns(&session)
        .expect("restoring defaults publishes");
    assert_eq!(view.columns(), PROCESS_LIST.to_vec());
    assert_published_layout(&session, &PROCESS_LIST, &ids);
    assert_no_row_lost_since(&session, revision);
}

#[test]
fn a_refresh_publishes_new_samples_in_the_chosen_layout() {
    let mut later = population();
    // `init` climbs to 90% and `cron` (PID 55) arrives.
    later[0].cpu = cpu(90);
    later.push(record(55, "cron", 512, cpu(1)));
    let (session, mut view, mut source) = started(vec![population(), later]);
    let ids = identities_by_default_name(&session);
    let layout = [ProcessField::Name, ProcessField::Cpu];
    view.set_columns(&session, &layout)
        .expect("a layout publishes");

    view.refresh(&session, &mut source)
        .expect("the refresh publishes");
    assert_eq!(
        view.columns(),
        layout.to_vec(),
        "a refresh keeps the layout"
    );
    assert_eq!(headings(&session), headings_of(&layout));
    let published: HashMap<String, (ItemId, Vec<String>)> = rows(&session)
        .into_iter()
        .map(|(id, cells)| (cells[0].clone(), (id, cells)))
        .collect();
    assert_eq!(published.len(), 4);
    assert_eq!(
        published["init"],
        (ids["init"], vec!["init".into(), "90.0%".into()])
    );
    assert_eq!(
        published["shell"],
        (ids["shell"], vec!["shell".into(), "50.0%".into()])
    );
    assert_eq!(
        published["daemon"],
        (ids["daemon"], vec!["daemon".into(), "20.0%".into()])
    );
    assert_eq!(
        published["cron"].1,
        vec!["cron".to_string(), "1.0%".to_string()]
    );
    assert!(!ids.values().any(|id| *id == published["cron"].0));
}

#[test]
fn hiding_the_sorted_column_keeps_the_sort_and_every_identity() {
    let mut later = population();
    // `init` climbs from 5% to 90% while its column is hidden.
    later[0].cpu = cpu(90);
    let (session, mut view, mut source) = started(vec![population(), later]);
    let ids = identities_by_default_name(&session);
    let order = SortOrder {
        field: ProcessField::Cpu,
        direction: SortDirection::Descending,
    };
    view.set_sort(&session, order).expect("the sort publishes");
    assert_eq!(names_in_order(&session, 1), ["shell", "daemon", "init"]);
    // A client selects `daemon`.
    let selected = ids["daemon"];
    let mut client = ClientSelection::select(&session, selected);

    let revision = session.current_revision();
    let layout = [
        ProcessField::Pid,
        ProcessField::Name,
        ProcessField::Resident,
    ];
    view.set_columns(&session, &layout)
        .expect("hiding the sorted column publishes");
    assert_eq!(
        view.sort(),
        Some(order),
        "the hidden column still orders the rows"
    );
    assert_published_layout(&session, &layout, &ids);
    assert_eq!(names_in_order(&session, 1), ["shell", "daemon", "init"]);
    assert_no_row_lost_since(&session, revision);
    assert_eq!(
        client.follow(&session),
        Some(selected),
        "hiding the sorted column keeps the client's selection"
    );

    // The next sample still sorts by the hidden CPU share, under the same
    // item IDs.
    let revision = session.current_revision();
    view.refresh(&session, &mut source)
        .expect("the refresh publishes");
    assert_eq!(view.sort(), Some(order));
    assert_eq!(names_in_order(&session, 1), ["init", "shell", "daemon"]);
    assert_eq!(headings(&session), headings_of(&layout));
    let after: HashMap<String, ItemId> = rows(&session)
        .into_iter()
        .map(|(id, cells)| (cells[1].clone(), id))
        .collect();
    assert_eq!(after, ids);
    assert_no_row_lost_since(&session, revision);
    assert_eq!(
        client.follow(&session),
        Some(selected),
        "a refresh keeps the selection"
    );

    // Showing the column again shows the order it has kept all along.
    view.restore_default_columns(&session)
        .expect("restoring defaults publishes");
    assert_eq!(view.sort(), Some(order));
    assert_eq!(names_in_order(&session, 1), ["init", "shell", "daemon"]);
    assert_eq!(identities_by_default_name(&session), ids);
    assert_table_and_model_kept_since(&session, revision);
    assert_eq!(
        client.follow(&session),
        Some(selected),
        "restoring defaults keeps the selection"
    );
}

#[test]
fn reordering_columns_under_an_active_sort_keeps_the_order_and_every_identity() {
    let mut later = population();
    // `init` climbs from 5% to 90% after the reorder.
    later[0].cpu = cpu(90);
    let (session, mut view, mut source) = started(vec![population(), later]);
    let ids = identities_by_default_name(&session);
    let order = SortOrder {
        field: ProcessField::Cpu,
        direction: SortDirection::Descending,
    };
    view.set_sort(&session, order).expect("the sort publishes");
    assert_eq!(names_in_order(&session, 1), ["shell", "daemon", "init"]);
    let sorted_ids = view.item_ids();
    // A client selects `shell`.
    let selected = ids["shell"];
    let mut client = ClientSelection::select(&session, selected);

    let revision = session.current_revision();
    let layout = [
        ProcessField::Cpu,
        ProcessField::Resident,
        ProcessField::Name,
        ProcessField::Pid,
    ];
    view.set_columns(&session, &layout)
        .expect("a reorder under a sort publishes");
    assert_eq!(view.sort(), Some(order), "a reorder keeps the active sort");
    assert_published_layout(&session, &layout, &ids);
    assert_eq!(
        names_in_published_order(&session),
        ["shell", "daemon", "init"]
    );
    assert_eq!(view.item_ids(), sorted_ids, "a reorder moves no row");
    assert_eq!(
        client.follow(&session),
        Some(selected),
        "a reorder keeps the client's selection"
    );

    // The next sample is published in the new layout, still sorted by CPU,
    // under the same item IDs.
    view.refresh(&session, &mut source)
        .expect("the refresh publishes");
    assert_eq!(view.sort(), Some(order));
    assert_eq!(view.columns(), layout.to_vec());
    assert_eq!(headings(&session), headings_of(&layout));
    assert_eq!(
        names_in_published_order(&session),
        ["init", "shell", "daemon"]
    );
    let after: HashMap<String, ItemId> = rows(&session)
        .into_iter()
        .map(|(id, cells)| (cells[2].clone(), id))
        .collect();
    assert_eq!(after, ids);
    assert_eq!(rows(&session)[0].1, vec!["90.0%", "1.0 KiB", "init", "7"]);
    assert_no_row_lost_since(&session, revision);
    assert_table_and_model_kept_since(&session, revision);
    assert_eq!(
        client.follow(&session),
        Some(selected),
        "a refresh keeps the selection"
    );
}

#[test]
fn chooser_controls_hide_reorder_show_and_restore_through_the_session() {
    use srui_process_explorer::install_column_handlers;

    let (session, view, _) = started(vec![population()]);
    let view = Arc::new(Mutex::new(view));
    install_column_handlers(&session, &view);
    let ids = identities_by_default_name(&session);
    let revision = session.current_revision();
    // A client selects `init`.
    let selected = ids["init"];
    let mut client = ClientSelection::select(&session, selected);

    // Hide Resident.
    click(
        &session,
        chooser_control(&session, ProcessField::Resident, HIDE),
        1,
    );
    let hidden = [ProcessField::Pid, ProcessField::Name, ProcessField::Cpu];
    assert_published_layout(&session, &hidden, &ids);
    assert_eq!(view.lock().unwrap().columns(), hidden.to_vec());
    assert_eq!(
        client.follow(&session),
        Some(selected),
        "hiding keeps the selection"
    );

    // Move CPU earlier: it moves ahead of a column it followed, and the rest
    // keep their order.
    click(
        &session,
        chooser_control(&session, ProcessField::Cpu, EARLIER),
        2,
    );
    let moved = published_layout(&session);
    assert_eq!(moved.len(), 3, "a move neither shows nor hides: {moved:?}");
    assert!(!moved.contains(&ProcessField::Resident), "{moved:?}");
    assert!(
        position(&moved, ProcessField::Cpu) < 2,
        "CPU did not move: {moved:?}"
    );
    keeps_relative_order(&moved, &[ProcessField::Pid, ProcessField::Name]);
    assert_published_layout(&session, &moved, &ids);
    assert_eq!(view.lock().unwrap().columns(), moved);
    assert_eq!(
        client.follow(&session),
        Some(selected),
        "a move keeps the selection"
    );

    // Move PID later.
    let pid_before = position(&moved, ProcessField::Pid);
    click(
        &session,
        chooser_control(&session, ProcessField::Pid, LATER),
        3,
    );
    let moved_again = published_layout(&session);
    assert_eq!(moved_again.len(), 3, "{moved_again:?}");
    assert!(
        position(&moved_again, ProcessField::Pid) > pid_before,
        "PID did not move: {moved_again:?}"
    );
    assert_published_layout(&session, &moved_again, &ids);
    assert_eq!(view.lock().unwrap().columns(), moved_again);
    assert_eq!(
        client.follow(&session),
        Some(selected),
        "a move keeps the selection"
    );

    // Show Resident again: it returns, and the chosen order of the others is
    // kept.
    click(
        &session,
        chooser_control(&session, ProcessField::Resident, SHOW),
        4,
    );
    let shown = published_layout(&session);
    assert_eq!(shown.len(), 4, "{shown:?}");
    keeps_relative_order(&shown, &moved_again);
    assert_published_layout(&session, &shown, &ids);
    assert_eq!(view.lock().unwrap().columns(), shown);
    assert_eq!(
        client.follow(&session),
        Some(selected),
        "showing keeps the selection"
    );

    // Restore the defaults.
    click(&session, restore_control(&session), 5);
    assert_published_layout(&session, &PROCESS_LIST, &ids);
    assert_eq!(view.lock().unwrap().columns(), PROCESS_LIST.to_vec());
    assert_eq!(
        client.follow(&session),
        Some(selected),
        "restoring keeps the selection"
    );

    assert_no_row_lost_since(&session, revision);
    assert_table_and_model_kept_since(&session, revision);
}

#[test]
fn a_replayed_chooser_click_changes_nothing() {
    use srui_process_explorer::install_column_handlers;

    let (session, view, _) = started(vec![population()]);
    let view = Arc::new(Mutex::new(view));
    install_column_handlers(&session, &view);
    let ids = identities_by_default_name(&session);

    let hide = activate(
        chooser_control(&session, ProcessField::Cpu, HIDE),
        1,
        session.current_revision(),
    );
    assert!(matches!(
        session.process_event(&hide).unwrap(),
        EventOutcome::Processed { .. }
    ));
    let hidden = [
        ProcessField::Pid,
        ProcessField::Name,
        ProcessField::Resident,
    ];
    assert_published_layout(&session, &hidden, &ids);
    click(
        &session,
        chooser_control(&session, ProcessField::Cpu, SHOW),
        2,
    );
    let shown = published_layout(&session);
    assert!(shown.contains(&ProcessField::Cpu), "{shown:?}");
    assert_published_layout(&session, &shown, &ids);

    // A reconnecting client replays the older hide: the session answers it
    // from its settled result and the column stays shown.
    let revision = session.current_revision();
    assert!(matches!(
        session.process_event(&hide).unwrap(),
        EventOutcome::Duplicate { accepted: true, .. }
    ));
    assert_eq!(session.current_revision(), revision);
    assert_eq!(published_layout(&session), shown);
    assert_eq!(view.lock().unwrap().columns(), shown);
}

#[test]
fn chooser_clicks_under_an_active_sort_keep_the_sort_and_every_identity() {
    use srui_process_explorer::install_column_handlers;

    let mut later = population();
    // `init` climbs from 5% to 90% while CPU is hidden.
    later[0].cpu = cpu(90);
    let (session, view, mut source) = started(vec![population(), later]);
    let view = Arc::new(Mutex::new(view));
    install_sort_handlers(&session, &view);
    install_column_handlers(&session, &view);
    let ids = identities_by_default_name(&session);

    // The user sorts by CPU (descending, its catalogued default) and selects
    // `daemon` in the native table.
    click(&session, SORT_CPU, 1);
    let order = view.lock().unwrap().sort().expect("a sort is active");
    assert_eq!(order.field, ProcessField::Cpu);
    assert_eq!(
        names_in_published_order(&session),
        ["shell", "daemon", "init"]
    );
    let selected = ids["daemon"];
    let mut client = ClientSelection::select(&session, selected);
    let revision = session.current_revision();

    // Hiding the sorted column and moving Name keep the order, the identities
    // and the selection.
    click(
        &session,
        chooser_control(&session, ProcessField::Cpu, HIDE),
        2,
    );
    assert!(!published_layout(&session).contains(&ProcessField::Cpu));
    assert_eq!(
        client.follow(&session),
        Some(selected),
        "hiding the sorted column keeps the client's selection"
    );
    click(
        &session,
        chooser_control(&session, ProcessField::Name, EARLIER),
        3,
    );
    let layout = published_layout(&session);
    assert_eq!(position(&layout, ProcessField::Name), 0, "{layout:?}");
    assert_published_layout(&session, &layout, &ids);
    assert_eq!(view.lock().unwrap().sort(), Some(order));
    assert_eq!(
        names_in_published_order(&session),
        ["shell", "daemon", "init"]
    );
    assert_eq!(
        rows(&session)[1].0,
        selected,
        "the selected process kept its item ID"
    );
    assert_eq!(
        client.follow(&session),
        Some(selected),
        "a move keeps the selection"
    );

    // The next sample still sorts by the hidden CPU share.
    view.lock()
        .unwrap()
        .refresh(&session, &mut source)
        .expect("the refresh publishes");
    assert_eq!(view.lock().unwrap().sort(), Some(order));
    assert_eq!(published_layout(&session), layout);
    assert_eq!(
        names_in_published_order(&session),
        ["init", "shell", "daemon"]
    );
    assert_eq!(rows(&session)[2].0, selected);
    assert_no_row_lost_since(&session, revision);
    assert_table_and_model_kept_since(&session, revision);
    assert_eq!(
        client.follow(&session),
        Some(selected),
        "a refresh keeps the selection"
    );
}

#[test]
fn a_layout_naming_a_column_twice_is_rejected_and_publishes_nothing() {
    let (session, mut view, _) = started(vec![population()]);
    let ids = identities_by_default_name(&session);
    let revision = session.current_revision();
    let rejected = view.set_columns(
        &session,
        &[ProcessField::Pid, ProcessField::Name, ProcessField::Pid],
    );
    assert!(rejected.is_err(), "a duplicated column is not a layout");
    assert_eq!(
        session.current_revision(),
        revision,
        "nothing was committed"
    );
    assert_eq!(view.columns(), PROCESS_LIST.to_vec());
    assert_published_layout(&session, &PROCESS_LIST, &ids);
}
