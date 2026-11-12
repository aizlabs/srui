//! PX-018 acceptance: parent/child process tree navigation (D1 §§6.2, 8,
//! 22.5, 22.7, 26; D2 T28), on the hierarchical model-backed `Tree` of
//! PX-018-G01 and the `keep_visible` hint of PX-017-G01.
//!
//! * The same process instances, under the same item IDs, are projected into
//!   one `Tree` model whose index space is the forest's preorder; no node is
//!   created per row, and the hierarchy is generated on the server.
//! * A parent counts only when the same generation holds exactly one record
//!   with its PID that started no later than the child and is shown;
//!   anything else is an explicit root whose Parent cell says why. Cycles are
//!   broken deterministically, and chains deeper than the depth limit start a
//!   new root.
//! * Sorting orders each level and never flattens the hierarchy; the row
//!   ceiling keeps a preorder prefix.
//! * The selection and follow carry over between the table and the tree; a
//!   collapse that hides the selected row clears the inspector, and follow
//!   says when its process is hidden under a collapsed parent.
//! * Every tree edit is index-based and one transaction, which a replica
//!   applying the journal accepts revision by revision.

use srui_process_explorer::catalogue::ProcessField;
use srui_process_explorer::follow::SELECTION_CLEARED_FOLLOW_STATUS;
use srui_process_explorer::inspector::{CLEARED_SELECTION_STATUS, NO_SELECTION_STATUS};
use srui_process_explorer::refresh::{lock_view, ProcessView, SharedView, ViewMode};
use srui_process_explorer::source::*;
use srui_process_explorer::{
    install_follow_handlers, install_inspector, install_search_handlers, install_view_handlers,
    start_from_source, FOLLOW_STATUS, INSPECTOR_STATUS, LIST_SHORTCUT, MODEL, PID_GO, TABLE, TREE,
    TREE_MODEL, VIEW_STATUS, VIEW_TOGGLE,
};
use srui_protocol::{operation, ClientModelRangeRequest, DEFAULT_MAX_FRAME_SIZE};
use srui_sdk::{
    ItemId, NodeId, PropertyRef, TypeRef, Value, COLUMNS, KEEP_VISIBLE, TEXT, VALUE, VISIBILITY,
};
use srui_semantic_tree::{EnumToken, Event as DomainEvent, SemanticStore, StandardVisibility};
use srui_sessiond::{EventOutcome, ModelRangeFulfillment, Session};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

const CLIENT: &[u8] = b"tree-client-instance";
/// Another client instance attached to the same session.
const OTHER: &[u8] = b"tree-other-client-instance";

/// One row of the published tree, as a client reads it off the model.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TreeRow {
    item: ItemId,
    pid: u64,
    name: String,
    depth: u64,
    descendants: u64,
    parent: String,
}

struct Harness {
    session: Arc<Session>,
    view: SharedView,
    replica: SemanticStore,
    seq: u64,
    /// The event sequence of every client but [`CLIENT`].
    other_seqs: HashMap<Vec<u8>, u64>,
}

fn harness(source: &mut impl ProcessSource) -> Harness {
    harness_with(source, |view| view)
}

fn harness_with(
    source: &mut impl ProcessSource,
    adjust: impl FnOnce(ProcessView) -> ProcessView,
) -> Harness {
    let session = Arc::new(Session::mint());
    let (view, _) = start_from_source(&session, source).unwrap();
    let view: SharedView = Arc::new(Mutex::new(adjust(view)));
    install_view_handlers(&session, &view);
    install_follow_handlers(&session, &view);
    install_search_handlers(&session, &view);
    install_inspector(&session, &view, None);
    let mut harness = Harness {
        session,
        view,
        replica: SemanticStore::new(),
        seq: 0,
        other_seqs: HashMap::new(),
    };
    harness.sync();
    harness
}

impl Harness {
    fn view(&self) -> std::sync::MutexGuard<'_, ProcessView> {
        lock_view(&self.view)
    }

    fn revision(&self) -> u64 {
        self.session.current_revision()
    }

    /// Applies, as a client does, every transaction committed since the
    /// replica last caught up: each one must be accepted, tree shape included.
    fn sync(&mut self) {
        for transaction in self
            .session
            .collect_replayed_transactions(self.replica.revision().get())
            .expect("the journal holds this run")
        {
            let revision = transaction.new_revision;
            self.replica
                .apply_wire_transaction(transaction)
                .unwrap_or_else(|error| panic!("a client refuses revision {revision}: {error}"));
        }
        assert_eq!(self.replica.revision().get(), self.revision());
    }

    fn send(&mut self, node: NodeId, event: TypeRef, arguments: Vec<(PropertyRef, Value)>) {
        self.send_from(CLIENT, node, event, arguments);
    }

    /// The next event sequence number of `client`, each client's own.
    fn next_seq(&mut self, client: &[u8]) -> u64 {
        let seq = if client == CLIENT {
            &mut self.seq
        } else {
            self.other_seqs.entry(client.to_vec()).or_default()
        };
        *seq += 1;
        *seq
    }

    /// One event from `client`, at the current revision.
    fn send_from(
        &mut self,
        client: &[u8],
        node: NodeId,
        event: TypeRef,
        arguments: Vec<(PropertyRef, Value)>,
    ) {
        let seq = self.next_seq(client);
        let wire = DomainEvent::new(
            None,
            seq,
            format!("tree-event-{seq}"),
            self.revision(),
            node,
            event,
            arguments,
        )
        .with_client_instance_id(client.to_vec())
        .to_wire();
        let outcome = self.session.process_event(&wire).expect("event admitted");
        assert!(
            matches!(outcome, EventOutcome::Processed { .. }),
            "not processed: {outcome:?}"
        );
        self.sync();
    }

    /// The user clicks the view switch.
    fn show_tree(&mut self, on: bool) {
        self.send(
            VIEW_TOGGLE,
            TypeRef::EVENT_VALUE_CHANGED,
            vec![(PropertyRef::VALUE, Value::Bool(on))],
        );
    }

    fn select(&mut self, node: NodeId, item: ItemId) {
        self.select_from(CLIENT, node, item);
    }

    fn select_from(&mut self, client: &[u8], node: NodeId, item: ItemId) {
        self.send_from(
            client,
            node,
            TypeRef::EVENT_SELECTION_CHANGED,
            vec![(PropertyRef::VALUE, Value::ItemId(item))],
        );
    }

    fn disclose(&mut self, item: ItemId, expanded: bool) {
        self.disclose_from(CLIENT, item, expanded);
    }

    fn disclose_from(&mut self, client: &[u8], item: ItemId, expanded: bool) {
        let seq = self.next_seq(client);
        let wire = DomainEvent::expansion_changed(
            seq,
            format!("tree-disclosure-{seq}"),
            self.revision(),
            TREE,
            item,
            expanded,
        )
        .with_client_instance_id(client.to_vec())
        .to_wire();
        let outcome = self.session.process_event(&wire).expect("event admitted");
        assert!(matches!(outcome, EventOutcome::Processed { .. }));
        self.sync();
    }

    fn follow(&mut self, on: bool) {
        self.send(
            srui_process_explorer::FOLLOW_TOGGLE,
            TypeRef::EVENT_VALUE_CHANGED,
            vec![(PropertyRef::VALUE, Value::Bool(on))],
        );
    }

    /// A fresh client instance attaches to the running session: it takes
    /// the catch-up snapshot, which carries rows and hints but no disclosure,
    /// so its outline starts with every item expanded.
    fn attach(&mut self, client: &[u8]) {
        let hello = srui_protocol::ClientHello {
            core_version: "0.5.0".to_string(),
            profiles: vec!["org.srui.standard-widgets/1".to_string()],
            limits: None,
            client_instance_id: client.to_vec(),
            client_metadata: Default::default(),
            known_resource_hashes: vec![],
        };
        let snapshot = self
            .session
            .bootstrap_fresh_client(&hello)
            .expect("a fresh client attaches")
            .snapshot
            .expect("a catch-up snapshot");
        let snapshot = srui_semantic_tree::ResyncSnapshot::try_from(snapshot)
            .expect("the catch-up snapshot decodes");
        SemanticStore::new()
            .replace_from_snapshot(&snapshot)
            .expect("the fresh client accepts the snapshot");
    }

    fn sample(&mut self, source: &mut impl ProcessSource) {
        self.view().refresh(&self.session, source).unwrap();
        self.sync();
    }

    fn property(&self, node: NodeId, property: PropertyRef) -> Option<Value> {
        self.replica
            .get_node(node)
            .and_then(|node| node.get_property(property))
            .cloned()
    }

    fn text(&self, node: NodeId) -> String {
        match self.property(node, TEXT) {
            Some(Value::String(text)) => text,
            other => panic!("no text on {node:?}: {other:?}"),
        }
    }

    fn item_property(&self, node: NodeId, property: PropertyRef) -> Option<ItemId> {
        match self.property(node, property) {
            Some(Value::ItemId(item)) => Some(item),
            _ => None,
        }
    }

    fn shown(&self, node: NodeId) -> bool {
        self.property(node, VISIBILITY)
            != Some(Value::EnumToken(EnumToken::from(
                StandardVisibility::Collapsed,
            )))
    }

    /// The tree as the replica holds it, in preorder.
    fn tree(&self) -> Vec<TreeRow> {
        let model = self.replica.get_model(TREE_MODEL).unwrap();
        (0..model.item_count)
            .map(|index| {
                let item = model
                    .get_item_by_index(index)
                    .expect("every tree row is cached");
                let Value::List(cells) = &item.value else {
                    panic!("a row is a list of cells")
                };
                let number = |property| match item.get_property(property) {
                    Some(Value::UnsignedInt(value)) => *value,
                    other => panic!("{property:?} is {other:?}"),
                };
                TreeRow {
                    item: item.item_id,
                    pid: match cells[ProcessField::Pid.index()] {
                        Value::UnsignedInt(pid) => pid,
                        _ => 0,
                    },
                    name: match &cells[ProcessField::Name.index()] {
                        Value::String(name) => name.clone(),
                        other => panic!("a name: {other:?}"),
                    },
                    depth: number(PropertyRef::TREE_DEPTH),
                    descendants: number(PropertyRef::DESCENDANT_COUNT),
                    parent: match cells.last() {
                        Some(Value::String(parent)) => parent.clone(),
                        other => panic!("a parent cell: {other:?}"),
                    },
                }
            })
            .collect()
    }

    /// The table's rows as (item, PID), in published order.
    fn table(&self) -> Vec<(ItemId, u64)> {
        let model = self.replica.get_model(MODEL).unwrap();
        (0..model.item_count)
            .map(|index| {
                let item = model.get_item_by_index(index).unwrap();
                let Value::List(cells) = &item.value else {
                    panic!("a row is a list of cells")
                };
                let pid = match cells[ProcessField::Pid.index()] {
                    Value::UnsignedInt(pid) => pid,
                    _ => 0,
                };
                (item.item_id, pid)
            })
            .collect()
    }

    fn item_of(&self, pid: u64) -> ItemId {
        if let Some(row) = self.tree().into_iter().find(|row| row.pid == pid) {
            return row.item;
        }
        self.table()
            .into_iter()
            .find(|(_, row_pid)| *row_pid == pid)
            .map(|(item, _)| item)
            .unwrap_or_else(|| panic!("PID {pid} is not shown"))
    }

    /// Every model operation on the tree's model since `revision`.
    fn tree_operations_since(&self, revision: u64) -> Vec<Vec<operation::Op>> {
        self.session
            .collect_replayed_transactions(revision)
            .unwrap()
            .into_iter()
            .map(|transaction| {
                transaction
                    .operations
                    .into_iter()
                    .filter_map(|op| op.op)
                    .filter(|op| match op {
                        operation::Op::ModelInsert(insert) => insert.model_id == TREE_MODEL.get(),
                        operation::Op::ModelDelete(delete) => delete.model_id == TREE_MODEL.get(),
                        operation::Op::ModelUpdate(update) => update.model_id == TREE_MODEL.get(),
                        operation::Op::ModelResetRange(reset) => reset.model_id == TREE_MODEL.get(),
                        _ => false,
                    })
                    .collect::<Vec<_>>()
            })
            .filter(|ops| !ops.is_empty())
            .collect()
    }
}

/// (PID, depth, descendants, parent cell) of every tree row, in preorder.
fn shape(rows: &[TreeRow]) -> Vec<(u64, u64, u64, String)> {
    rows.iter()
        .map(|row| (row.pid, row.depth, row.descendants, row.parent.clone()))
        .collect()
}

/// Each row's parent PID as the preorder's depths say, or `None` for a root.
fn parents(rows: &[TreeRow]) -> BTreeMap<u64, Option<u64>> {
    let mut open: Vec<u64> = Vec::new();
    rows.iter()
        .map(|row| {
            open.truncate(row.depth as usize);
            let parent = open.last().copied();
            open.push(row.pid);
            (row.pid, parent)
        })
        .collect()
}

fn s(text: &str) -> String {
    text.to_string()
}

/// A fixed source of `records`, whose scan says what it could not read.
struct Fixed {
    records: Vec<ProcessRecord>,
    completeness: Completeness,
    capped: CappedRecords,
}

impl Fixed {
    fn new(records: Vec<ProcessRecord>) -> Self {
        Self {
            records,
            completeness: Completeness::Complete,
            capped: CappedRecords::none(),
        }
    }
}

impl ProcessSource for Fixed {
    fn status_text(&self) -> &str {
        "Read-only · Fixed tree fixture"
    }

    fn snapshot(&mut self) -> ProcessSnapshot {
        ProcessSnapshot {
            source: SourceId("tree-fixture-v1".into()),
            sampled_at: SnapshotTime(SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000)),
            records: self.records.clone(),
            vanished: 0,
            capped: self.capped.clone(),
            completeness: self.completeness.clone(),
            system: FakeProcessSource::system(),
        }
    }
}

fn record(pid: u32, name: &str, lineage: Lineage, ticks: u64) -> ProcessRecord {
    ProcessRecord {
        lineage,
        owner: OWNER_NOT_COLLECTED,
        key: ProcessKey {
            source: SourceId("tree-fixture-v1".into()),
            host: Observed::Known(HostId("tree-host".into())),
            boot: Observed::Known(BootId("tree-boot".into())),
            pid_namespace: Observed::Known(PidNamespaceId(4_026_531_836)),
            pid: Observed::Known(pid),
            creation: CreationToken::Opaque(format!("{name}-{pid}")),
        },
        display_name: DisplayName::from(name),
        resident: Observed::Known(4096),
        cpu: CpuUsage::Measured(CpuInterval {
            ticks,
            ticks_per_second: 100,
            elapsed: Duration::from_secs(1),
        }),
    }
}

#[test]
fn the_tree_projects_the_same_identities_with_parents_from_the_same_generation() {
    let mut source = TreeSource::default();
    let mut h = harness(&mut source);
    let table = h.table();
    assert_eq!(table.len(), 12);
    assert!(h.shown(TABLE) && !h.shown(TREE));
    assert_eq!(
        h.tree().len(),
        0,
        "the tree's model is empty while the table is shown"
    );
    let nodes = h.replica.node_count();
    let before = h.revision();
    h.show_tree(true);
    assert_eq!(h.revision(), before + 1, "the switch is one transaction");
    assert_eq!(h.view().view_mode(), ViewMode::Tree);
    assert_eq!(h.replica.node_count(), nodes, "no node is created per row");
    assert!(!h.shown(TABLE) && h.shown(TREE));
    assert_eq!(h.table().len(), 0, "the hidden table's model is emptied");
    assert_eq!(h.property(VIEW_TOGGLE, VALUE), Some(Value::Bool(true)));
    // The list-focus shortcut moved with the shown collection.
    assert_eq!(h.property(TABLE, PropertyRef::SHORTCUT), None);
    assert_eq!(
        h.property(TREE, PropertyRef::SHORTCUT),
        Some(Value::String(LIST_SHORTCUT.into()))
    );
    let tree = h.tree();
    // The same processes under the same item IDs.
    let tree_ids: HashSet<ItemId> = tree.iter().map(|row| row.item).collect();
    let table_ids: HashSet<ItemId> = table.iter().map(|(item, _)| *item).collect();
    assert_eq!(tree_ids, table_ids);
    assert_eq!(
        shape(&tree),
        vec![
            (1, 0, 7, s("none: started by the kernel")),
            (100, 1, 5, s("1")),
            (200, 2, 4, s("100")),
            (300, 3, 2, s("200")),
            (310, 4, 0, s("300")),
            (311, 4, 0, s("300")),
            (301, 3, 0, s("200")),
            (700, 1, 0, s("1")),
            (500, 0, 0, s("root: parent 9999 not in this scan")),
            (600, 0, 0, s("root: parent 700 started later (PID reused)")),
            (800, 0, 1, s("root: cycle through parent 801 broken here")),
            (801, 1, 0, s("800")),
        ]
    );
    // The headings were the tree's from the start: the table's and Parent.
    assert_eq!(
        h.property(TREE, COLUMNS),
        Some(Value::List(vec![
            Value::String("PID".into()),
            Value::String("Name".into()),
            Value::String("Resident".into()),
            Value::String("CPU (100% = 1 CPU)".into()),
            Value::String("Parent".into()),
        ]))
    );
    let status = h.text(VIEW_STATUS);
    assert!(
        status.starts_with("View: process tree · 12 processes under 4 roots · 3 without a shown parent (1 parent not in this scan or unread, 1 parent PID reused, 1 cycle or depth limit)"),
        "{status}"
    );
    // Switching back restores the table, same identities, and empties the tree.
    h.show_tree(false);
    assert_eq!(h.table(), table);
    assert_eq!(h.tree().len(), 0);
    assert!(h.shown(TABLE) && !h.shown(TREE));
    assert_eq!(
        h.property(TABLE, PropertyRef::SHORTCUT),
        Some(Value::String(LIST_SHORTCUT.into()))
    );
    assert_eq!(h.property(TREE, PropertyRef::SHORTCUT), None);
    assert_eq!(
        h.text(VIEW_STATUS),
        srui_process_explorer::refresh::TABLE_VIEW_STATUS
    );
}

#[test]
fn reparenting_after_a_parent_exits_attaches_the_children_to_init_by_index_ranges() {
    let mut source = TreeSource::default();
    let mut h = harness(&mut source);
    h.show_tree(true);
    let cc = h.item_of(310);
    let ld = h.item_of(311);
    let before = h.revision();
    h.sample(&mut source);
    assert_eq!(source.step(), TreeSource::REPARENTED_STEP + 1);
    let tree = h.tree();
    assert_eq!(
        shape(&tree),
        vec![
            (1, 0, 6, s("none: started by the kernel")),
            (100, 1, 2, s("1")),
            (200, 2, 1, s("100")),
            (301, 3, 0, s("200")),
            (310, 1, 0, s("1")),
            (311, 1, 0, s("1")),
            (700, 1, 0, s("1")),
            (500, 0, 0, s("root: parent 9999 not in this scan")),
            (600, 0, 0, s("root: parent 700 started later (PID reused)")),
            (800, 0, 1, s("root: cycle through parent 801 broken here")),
            (801, 1, 0, s("800")),
        ]
    );
    // The reparented processes kept their item IDs.
    assert_eq!(h.item_of(310), cc);
    assert_eq!(h.item_of(311), ld);
    // Every tree edit is by index, one transaction per sample: a sparse
    // replica never sees an identity delete it cannot place.
    let edits = h.tree_operations_since(before);
    assert_eq!(edits.len(), 1, "one transaction carries the tree edit");
    for op in edits.iter().flatten() {
        match op {
            operation::Op::ModelDelete(delete) => {
                assert!(
                    delete.item_ids.is_empty(),
                    "a tree delete is an index range"
                );
                assert!(delete.count > 0);
            }
            operation::Op::ModelUpdate(update) => {
                assert_ne!(update.index, u64::MAX, "a tree update names its index")
            }
            operation::Op::ModelInsert(_) => {}
            other => panic!("unexpected tree operation {other:?}"),
        }
    }
    // An unchanged sample publishes nothing at all, or only its freshness.
    let idle = h.revision();
    h.sample(&mut source);
    assert!(h.tree_operations_since(idle).is_empty());
}

#[test]
fn a_reused_pid_is_never_a_parent_but_a_parent_started_at_the_same_tick_is() {
    let mut source = Fixed::new(vec![
        record(10, "parent", Lineage::new(0, 100), 1),
        // Started at the same tick as its parent: a parent that started no
        // later than the child counts.
        record(11, "twin", Lineage::new(10, 100), 1),
        // Its parent PID 12 is held by a process that started after it.
        record(13, "older-child", Lineage::new(12, 150), 1),
        record(12, "newer", Lineage::new(10, 200), 1),
        // Neither start time read: never attached.
        record(
            14,
            "unverified",
            Lineage {
                parent: Observed::Known(10),
                started: Observed::Missing(MissingReason::Denied),
            },
            1,
        ),
        // Its own parent PID unread.
        record(
            15,
            "unread",
            Lineage {
                parent: Observed::Missing(MissingReason::Denied),
                started: Observed::Known(300),
            },
            1,
        ),
        // Two records hold PID 16 in one generation: the parent is ambiguous.
        record(17, "ambiguous", Lineage::new(16, 900), 1),
        record(16, "dup-a", Lineage::new(10, 400), 1),
        record(16, "dup-b", Lineage::new(10, 401), 1),
    ]);
    let mut h = harness(&mut source);
    h.show_tree(true);
    let parents_of: BTreeMap<String, String> = h
        .tree()
        .into_iter()
        .map(|row| (row.name, row.parent))
        .collect();
    assert_eq!(parents_of["twin"], "10");
    assert_eq!(
        parents_of["older-child"],
        "root: parent 12 started later (PID reused)"
    );
    assert_eq!(parents_of["newer"], "10");
    assert_eq!(
        parents_of["unverified"],
        "root: parent 10 unverified (start time unread)"
    );
    assert_eq!(
        parents_of["unread"],
        "root: parent unread (permission denied)"
    );
    assert_eq!(parents_of["ambiguous"], "root: parent 16 ambiguous");
}

#[test]
fn missing_parents_are_explicit_roots_that_say_why() {
    let mut skipped = SkippedRecords::with_limit(8);
    skipped.record(21);
    let mut capped = CappedRecords::with_limit(8);
    capped.record(22);
    let mut source = Fixed {
        records: vec![
            record(1, "init", Lineage::new(0, 1), 1),
            record(30, "gone-parent", Lineage::new(20, 30), 1),
            record(31, "denied-parent", Lineage::new(21, 31), 1),
            record(32, "capped-parent", Lineage::new(22, 32), 1),
            record(33, "no-lineage", Lineage::NOT_COLLECTED, 1),
        ],
        completeness: Completeness::from_scan(skipped, Vec::new()),
        capped,
    };
    let mut h = harness(&mut source);
    h.show_tree(true);
    let parents_of: BTreeMap<String, String> = h
        .tree()
        .into_iter()
        .map(|row| (row.name, row.parent))
        .collect();
    assert_eq!(parents_of["init"], "none: started by the kernel");
    assert_eq!(
        parents_of["gone-parent"],
        "root: parent 20 not in this scan"
    );
    assert_eq!(parents_of["denied-parent"], "root: parent 21 unreadable");
    assert_eq!(
        parents_of["capped-parent"],
        "root: parent 22 beyond the record limit"
    );
    assert_eq!(
        parents_of["no-lineage"],
        "root: parent unread (unavailable)"
    );
    // A scan that cannot name what it missed may hide a parent unseen.
    let mut lost = SkippedRecords::with_limit(0);
    lost.unnamed();
    let mut source = Fixed {
        records: vec![record(30, "gone-parent", Lineage::new(20, 30), 1)],
        completeness: Completeness::from_scan(lost, Vec::new()),
        capped: CappedRecords::none(),
    };
    let mut h = harness(&mut source);
    h.show_tree(true);
    assert_eq!(
        h.tree()[0].parent,
        "root: parent 20 not seen (incomplete scan)"
    );
}

#[test]
fn a_row_kept_from_an_earlier_scan_is_neither_placed_nor_a_parent() {
    let mut source = Fixed::new(vec![
        record(1, "init", Lineage::new(0, 1), 1),
        record(40, "kept", Lineage::new(1, 40), 1),
        record(41, "child-of-kept", Lineage::new(40, 41), 1),
    ]);
    let mut h = harness(&mut source);
    h.show_tree(true);
    assert_eq!(h.tree()[2].parent, "40");
    // PID 40 becomes unreadable: its row is kept from the earlier scan.
    let mut skipped = SkippedRecords::with_limit(4);
    skipped.record(40);
    source.records.remove(1);
    source.completeness = Completeness::from_scan(skipped, Vec::new());
    h.sample(&mut source);
    let parents_of: BTreeMap<String, String> = h
        .tree()
        .into_iter()
        .map(|row| (row.name, row.parent))
        .collect();
    assert_eq!(parents_of["kept"], "root: row kept from an earlier scan");
    assert_eq!(
        parents_of["child-of-kept"],
        "root: parent 40 kept from an earlier scan"
    );
}

#[test]
fn malformed_cycles_are_broken_deterministically_whatever_the_sort() {
    let mut source = Fixed::new(vec![
        // A three-cycle whose start times tie, and a self-parent.
        record(70, "c-70", Lineage::new(72, 5), 3),
        record(71, "c-71", Lineage::new(70, 5), 1),
        record(72, "c-72", Lineage::new(71, 5), 2),
        record(80, "self", Lineage::new(80, 5), 4),
        record(81, "under-cycle", Lineage::new(71, 6), 5),
    ]);
    let mut h = harness(&mut source);
    h.show_tree(true);
    let first = parents(&h.tree());
    assert_eq!(
        first[&70], None,
        "the cycle is broken at its first identity"
    );
    assert_eq!(first[&72], Some(71));
    assert_eq!(first[&71], Some(70));
    assert_eq!(first[&81], Some(71));
    assert_eq!(first[&80], None);
    let reasons: BTreeMap<u64, String> = h
        .tree()
        .into_iter()
        .map(|row| (row.pid, row.parent))
        .collect();
    assert_eq!(reasons[&70], "root: cycle through parent 72 broken here");
    assert_eq!(reasons[&80], "root: cycle through parent 80 broken here");
    // The same break under both CPU orders.
    for _ in 0..2 {
        h.view().click_sort(&h.session, ProcessField::Cpu).unwrap();
        h.sync();
        assert_eq!(parents(&h.tree()), first);
    }
}

#[test]
fn deep_chains_fit_the_depth_limit_and_a_deeper_one_starts_a_new_root() {
    let chain = |length: u32| {
        Fixed::new(
            (0..length)
                .map(|level| {
                    let pid = 1000 + level;
                    let parent = if level == 0 { 0 } else { pid - 1 };
                    record(
                        pid,
                        &format!("level-{level}"),
                        Lineage::new(parent, u64::from(level)),
                        1,
                    )
                })
                .collect(),
        )
    };
    // At the limit: five levels hold a chain of five exactly.
    let mut at = chain(5);
    let mut h = harness_with(&mut at, |view| view.with_tree_levels(5));
    h.show_tree(true);
    let depths: Vec<u64> = h.tree().iter().map(|row| row.depth).collect();
    assert_eq!(depths, vec![0, 1, 2, 3, 4]);
    assert_eq!(h.tree()[0].descendants, 4);
    // Past it: the sixth process starts a new root, which says why.
    let mut past = chain(7);
    let mut h = harness_with(&mut past, |view| view.with_tree_levels(5));
    h.show_tree(true);
    let tree = h.tree();
    assert_eq!(
        shape(&tree),
        vec![
            (1000, 0, 4, s("none: started by the kernel")),
            (1001, 1, 3, s("1000")),
            (1002, 2, 2, s("1001")),
            (1003, 3, 1, s("1002")),
            (1004, 4, 0, s("1003")),
            (1005, 0, 1, s("root: parent 1004 at the depth limit")),
            (1006, 1, 0, s("1005")),
        ]
    );
    // The default limit, which every client enforces: 256 levels hold a chain
    // of 256, and the 257th starts a new root; the store accepts every commit.
    let mut deep = chain(300);
    let mut h = harness(&mut deep);
    assert_eq!(
        h.view().tree_levels(),
        srui_semantic_tree::DEFAULT_MAX_MODEL_TREE_DEPTH
    );
    h.show_tree(true);
    let tree = h.tree();
    assert_eq!(tree.len(), 300);
    assert_eq!(tree[255].depth, 255);
    assert_eq!(tree[255].descendants, 0);
    assert_eq!(tree[256].depth, 0);
    assert_eq!(tree[256].parent, "root: parent 1255 at the depth limit");
    assert_eq!(tree.iter().map(|row| row.depth).max(), Some(255));
}

#[test]
fn sorting_orders_each_parents_children_and_never_flattens_the_hierarchy() {
    let mut source = TreeSource::default();
    let mut h = harness(&mut source);
    h.show_tree(true);
    let before = parents(&h.tree());
    let ids: HashSet<ItemId> = h.tree().iter().map(|row| row.item).collect();
    h.view().click_sort(&h.session, ProcessField::Cpu).unwrap();
    h.sync();
    let sorted = h.tree();
    assert_eq!(parents(&sorted), before, "every parent is kept");
    assert_eq!(
        sorted.iter().map(|row| row.item).collect::<HashSet<_>>(),
        ids,
        "every item keeps its ID"
    );
    // Busiest first at every level.
    assert_eq!(
        sorted.iter().map(|row| row.pid).collect::<Vec<_>>(),
        vec![500, 600, 1, 700, 100, 200, 301, 300, 310, 311, 800, 801]
    );
    assert!(sorted.iter().any(|row| row.depth > 0));
    // And back the other way.
    h.view().click_sort(&h.session, ProcessField::Cpu).unwrap();
    h.sync();
    assert_eq!(parents(&h.tree()), before);
    assert_eq!(
        h.tree().iter().map(|row| row.pid).collect::<Vec<_>>(),
        vec![800, 801, 1, 100, 200, 300, 311, 310, 301, 700, 600, 500]
    );
}

#[test]
fn the_row_ceiling_keeps_a_preorder_prefix_and_never_orphans_a_shown_process() {
    let mut source = TreeSource::default();
    let mut h = harness_with(&mut source, |view| view.with_published_limit(5));
    h.show_tree(true);
    assert_eq!(
        shape(&h.tree()),
        vec![
            (1, 0, 4, s("none: started by the kernel")),
            (100, 1, 3, s("1")),
            (200, 2, 2, s("100")),
            (300, 3, 1, s("200")),
            (310, 4, 0, s("300")),
        ]
    );
    // The next sample states what the ceiling left out, as in the table.
    h.sample(&mut source);
    assert_eq!(h.tree().len(), 5);
    let status = h.text(srui_process_explorer::STATUS);
    assert!(
        status.contains("6 rows beyond the publishable size limit"),
        "{status}"
    );
}

#[test]
fn the_selection_and_follow_carry_over_between_the_table_and_the_tree() {
    let mut source = TreeSource::default();
    let mut h = harness(&mut source);
    // Without follow: the selection survives the switch both ways.
    let shell = h.item_of(200);
    h.select(TABLE, shell);
    h.show_tree(true);
    assert_eq!(h.item_property(TREE, VALUE), Some(shell));
    assert_eq!(h.item_property(TABLE, VALUE), None);
    assert!(h.text(INSPECTOR_STATUS).contains("PID 200"));
    h.show_tree(false);
    assert_eq!(h.item_property(TABLE, VALUE), Some(shell));
    assert_eq!(h.item_property(TREE, VALUE), None);
    // With follow: the hint moves to the shown collection, same item ID.
    let cc = h.item_of(310);
    h.select(TABLE, cc);
    h.follow(true);
    assert_eq!(h.item_property(TABLE, KEEP_VISIBLE), Some(cc));
    // A follow retarget withdrew the earlier promotion of another process.
    assert_ne!(h.item_property(TABLE, VALUE), Some(shell));
    h.show_tree(true);
    assert_eq!(h.item_property(TREE, KEEP_VISIBLE), Some(cc));
    assert_eq!(h.item_property(TREE, VALUE), Some(cc));
    assert_eq!(h.item_property(TABLE, KEEP_VISIBLE), None);
    assert_eq!(h.item_property(TABLE, VALUE), None);
    assert_eq!(h.view().follow().following(), Some(cc));
    let status = h.text(FOLLOW_STATUS);
    assert!(
        status.starts_with("Following PID 310 · cc · item 5 of 12 in the tree"),
        "{status}"
    );
    // A selection the hidden table reports is not the user's: ignored.
    let revision = h.revision();
    h.select(TABLE, shell);
    assert_eq!(h.revision(), revision, "nothing published");
    assert_eq!(h.view().follow().following(), Some(cc));
    // A selection in the tree retargets follow, as in the table.
    let ld = h.item_of(311);
    h.select(TREE, ld);
    assert_eq!(h.item_property(TREE, KEEP_VISIBLE), Some(ld));
    assert_eq!(h.view().follow().following(), Some(ld));
    // A sample that moves it changes no hint: the client reveals it.
    let hinted = h.revision();
    h.sample(&mut source);
    let ops = h
        .session
        .collect_replayed_transactions(hinted)
        .unwrap()
        .into_iter()
        .flat_map(|transaction| transaction.operations)
        .filter_map(|op| op.op)
        .filter(|op| matches!(op, operation::Op::SetProperty(set) if set.node_id == TREE.get()))
        .count();
    assert_eq!(ops, 0, "a sample sets nothing on the tree node");
    assert_eq!(h.item_property(TREE, KEEP_VISIBLE), Some(ld));
    // Back to the table: follow carries over again.
    h.show_tree(false);
    assert_eq!(h.item_property(TABLE, KEEP_VISIBLE), Some(ld));
    assert_eq!(h.item_property(TREE, KEEP_VISIBLE), None);
    assert!(h
        .text(FOLLOW_STATUS)
        .starts_with("Following PID 311 · ld · row"));
}

#[test]
fn a_collapse_that_hides_the_selection_clears_the_inspector_and_follow_says_where_it_is() {
    let mut source = TreeSource::default();
    let mut h = harness(&mut source);
    h.show_tree(true);
    let cc = h.item_of(310);
    let make = h.item_of(300);
    let session = h.item_of(100);
    h.select(TREE, cc);
    h.follow(true);
    assert!(h.text(INSPECTOR_STATUS).contains("PID 310"));
    let rows = h.tree();
    // Collapsing an unrelated parent changes nothing but what the server knows.
    let loop_a = h.item_of(800);
    let quiet = h.revision();
    h.disclose(loop_a, false);
    assert_eq!(h.revision(), quiet, "nothing a client sees changed");
    // Collapsing `session`, which hides `cc` two levels down.
    let before = h.revision();
    h.disclose(session, false);
    assert_eq!(h.text(INSPECTOR_STATUS), NO_SELECTION_STATUS);
    let status = h.text(FOLLOW_STATUS);
    assert!(
        status.starts_with(
            "Following PID 310 · cc · hidden under collapsed PID 100 · session: expand it"
        ),
        "{status}"
    );
    assert_eq!(h.view().follow().following(), Some(cc), "follow stays on");
    assert_eq!(h.tree(), rows, "disclosure publishes no row");
    assert!(h.tree_operations_since(before).is_empty());
    assert!(h.view().collapsed_by(CLIENT).contains(&session));
    // Expanding `session` while `make` is still open shows it again.
    h.disclose(session, true);
    assert!(h
        .text(FOLLOW_STATUS)
        .starts_with("Following PID 310 · cc · item 5 of 12"));
    // A collapse of `make` names the innermost collapsed parent that hides it.
    h.disclose(make, false);
    assert!(h
        .text(FOLLOW_STATUS)
        .contains("hidden under collapsed PID 300 · make"));
    // A disclosure report for an item without children, or one the tree does
    // not hold, is ignored.
    let ignored = h.revision();
    h.disclose(cc, false);
    h.disclose(ItemId::new(9_999_999), false);
    assert_eq!(h.revision(), ignored);
    // When `make` ends, it leaves the collapsed set with its row.
    h.sample(&mut source);
    assert!(!h.view().collapsed_by(CLIENT).contains(&make));
    assert!(h
        .text(FOLLOW_STATUS)
        .starts_with("Following PID 310 · cc · item"));
}

#[test]
fn search_promotes_its_match_in_the_tree_and_the_filters_make_hidden_parents_roots() {
    let mut source = TreeSource::default();
    let mut h = harness(&mut source);
    h.show_tree(true);
    h.view().set_search(&h.session, "ld").unwrap();
    h.sync();
    let ld = h.item_of(311);
    assert_eq!(h.item_property(TREE, VALUE), Some(ld));
    assert_eq!(h.item_property(TABLE, VALUE), None);
    assert!(h.text(INSPECTOR_STATUS).contains("PID 311"));
    h.view().set_filter(&h.session, "cc | ld | init").unwrap();
    h.sync();
    assert_eq!(
        shape(&h.tree()),
        vec![
            (1, 0, 0, s("none: started by the kernel")),
            (310, 0, 0, s("root: parent 300 hidden by the filters")),
            (311, 0, 0, s("root: parent 300 hidden by the filters")),
            // "ld" is in "reused-child" too.
            (600, 0, 0, s("root: parent 700 started later (PID reused)")),
        ]
    );
    assert_eq!(h.item_of(311), ld, "a filter keeps item IDs");
    assert!(h
        .text(VIEW_STATUS)
        .contains("2 parent hidden by the filters"));
    h.view().set_filter(&h.session, "").unwrap();
    h.sync();
    assert_eq!(h.tree().len(), 12);
}

#[test]
fn a_paused_view_switches_over_its_frozen_rows_and_publishes_no_sample() {
    let mut source = TreeSource::default();
    let mut h = harness(&mut source);
    h.view().pause_view(&h.session).unwrap();
    h.sync();
    h.show_tree(true);
    let frozen = h.tree();
    assert_eq!(frozen.len(), 12);
    let before = h.revision();
    h.sample(&mut source);
    assert_eq!(h.revision(), before, "a paused view publishes no sample");
    h.show_tree(false);
    assert_eq!(h.table().len(), 12);
    h.show_tree(true);
    assert_eq!(shape(&h.tree()), shape(&frozen));
    h.view().resume_view(&h.session).unwrap();
    h.sample(&mut source);
    assert_eq!(h.tree().len(), 11, "the resumed sample converges");
}

#[test]
fn a_request_without_a_boolean_or_for_the_shown_view_changes_nothing_but_the_switch() {
    let mut source = TreeSource::default();
    let mut h = harness(&mut source);
    let table = h.table();
    h.show_tree(false);
    assert_eq!(h.table(), table);
    assert_eq!(h.property(VIEW_TOGGLE, VALUE), Some(Value::Bool(false)));
    h.send(VIEW_TOGGLE, TypeRef::EVENT_VALUE_CHANGED, vec![]);
    assert_eq!(h.view().view_mode(), ViewMode::Table);
    assert_eq!(h.table(), table);
}

#[test]
fn a_fresh_client_attaching_in_the_tree_view_gets_one_collection_and_its_hint() {
    let mut source = TreeSource::default();
    let mut h = harness(&mut source);
    let cc = h.item_of(310);
    h.select(TABLE, cc);
    h.follow(true);
    h.show_tree(true);
    let hello = srui_protocol::ClientHello {
        core_version: "0.5.0".to_string(),
        profiles: vec!["org.srui.standard-widgets/1".to_string()],
        limits: None,
        client_instance_id: vec![9, 9, 9],
        client_metadata: Default::default(),
        known_resource_hashes: vec![],
    };
    let snapshot = h
        .session
        .bootstrap_fresh_client(&hello)
        .expect("a fresh client attaches")
        .snapshot
        .expect("a catch-up snapshot");
    let frame = srui_protocol::framed_payload_len(&srui_protocol::SruiMessage {
        msg: Some(srui_protocol::srui_message::Msg::Transaction(
            snapshot.clone(),
        )),
    });
    assert!(frame <= DEFAULT_MAX_FRAME_SIZE);
    let mut fresh = SemanticStore::new();
    let snapshot = srui_semantic_tree::ResyncSnapshot::try_from(snapshot)
        .expect("the catch-up snapshot decodes");
    fresh
        .replace_from_snapshot(&snapshot)
        .expect("the fresh client accepts the snapshot, tree shape included");
    assert_eq!(fresh.get_model(MODEL).unwrap().item_count, 0);
    assert_eq!(fresh.get_model(TREE_MODEL).unwrap().item_count, 12);
    assert_eq!(
        fresh.get_node(TREE).unwrap().keep_visible(),
        Some(cc),
        "the newly mounted tree reveals the followed process"
    );
    assert_eq!(fresh.get_node(TABLE).unwrap().keep_visible(), None);
}

#[tokio::test]
async fn the_server_holds_every_published_tree_row_so_a_range_request_needs_nothing_sent() {
    let mut source = TreeSource::default();
    let mut h = harness(&mut source);
    h.show_tree(true);
    let outcome = h
        .session
        .fulfill_model_range_request(ClientModelRangeRequest {
            node_id: TREE.get(),
            model_id: TREE_MODEL.get(),
            start_index: 0,
            count: 12,
            observed_revision: h.revision(),
        })
        .await
        .expect("a well-formed range request");
    assert_eq!(outcome, ModelRangeFulfillment::AlreadyCached);
}

#[test]
fn ten_thousand_processes_arrange_and_refresh_inside_the_scale_budget() {
    let mut source = SyntheticProcessSource::new(10_000);
    let mut h = harness(&mut source);
    let started = Instant::now();
    h.show_tree(true);
    let switched = started.elapsed();
    assert_eq!(h.tree().len(), 10_000);
    assert!(
        h.tree().iter().any(|row| row.depth >= 5),
        "a real hierarchy"
    );
    let mut slowest = Duration::ZERO;
    for _ in 0..5 {
        let tick = Instant::now();
        h.view().refresh(&h.session, &mut source).unwrap();
        slowest = slowest.max(tick.elapsed());
    }
    h.sync();
    assert!(
        slowest < Duration::from_secs(3),
        "a 10,000-process tree tick took {slowest:?}, beyond PX-010-G01's 3 s budget"
    );
    for transaction in h.session.collect_replayed_transactions(0).unwrap() {
        let frame = srui_protocol::framed_payload_len(&srui_protocol::SruiMessage {
            msg: Some(srui_protocol::srui_message::Msg::Transaction(transaction)),
        });
        assert!(frame <= DEFAULT_MAX_FRAME_SIZE);
    }
    // Under a CPU sort every sample reorders siblings: the tree edit resends
    // the rows between the first and the last that moved (lc-debt in
    // `plan_tree`), still one transaction inside the budget and one frame.
    h.view().click_sort(&h.session, ProcessField::Cpu).unwrap();
    let sorted_from = h.revision();
    let mut slowest_sorted = Duration::ZERO;
    for _ in 0..5 {
        let tick = Instant::now();
        h.view().refresh(&h.session, &mut source).unwrap();
        slowest_sorted = slowest_sorted.max(tick.elapsed());
    }
    h.sync();
    assert!(
        slowest_sorted < Duration::from_secs(3),
        "{slowest_sorted:?}"
    );
    let largest = h
        .session
        .collect_replayed_transactions(sorted_from)
        .unwrap()
        .into_iter()
        .map(|transaction| {
            srui_protocol::framed_payload_len(&srui_protocol::SruiMessage {
                msg: Some(srui_protocol::srui_message::Msg::Transaction(transaction)),
            })
        })
        .max()
        .unwrap_or(0);
    assert!(largest <= DEFAULT_MAX_FRAME_SIZE);
    let roots = h.tree().iter().filter(|row| row.depth == 0).count();
    println!(
        "PX-018 10,000-process tree: switch={switched:?} slowest tick={slowest:?} \
         slowest CPU-sorted tick={slowest_sorted:?} largest CPU-sorted frame={largest} B roots={roots}"
    );
}

#[test]
fn a_column_layout_change_re_lays_out_both_collections_and_keeps_the_tree() {
    let mut source = TreeSource::default();
    let mut h = harness(&mut source);
    h.show_tree(true);
    let before = h.tree();
    h.view()
        .set_columns(&h.session, &[ProcessField::Name, ProcessField::Pid])
        .unwrap();
    h.sync();
    let headings = |node| match h.property(node, COLUMNS) {
        Some(Value::List(headings)) => headings,
        other => panic!("headings: {other:?}"),
    };
    assert_eq!(
        headings(TREE),
        vec![
            Value::String("Name".into()),
            Value::String("PID".into()),
            Value::String("Parent".into()),
        ]
    );
    assert_eq!(
        headings(TABLE),
        vec![Value::String("Name".into()), Value::String("PID".into())]
    );
    let model = h.replica.get_model(TREE_MODEL).unwrap();
    let first = model.get_item_by_index(0).unwrap();
    assert_eq!(
        first.value,
        Value::List(vec![
            Value::String("init".into()),
            Value::UnsignedInt(1),
            Value::String("none: started by the kernel".into()),
        ])
    );
    let after: Vec<(ItemId, u64, u64)> = (0..model.item_count)
        .map(|index| {
            let item = model.get_item_by_index(index).unwrap();
            let number = |property| match item.get_property(property) {
                Some(Value::UnsignedInt(value)) => *value,
                _ => panic!("tree properties kept"),
            };
            (
                item.item_id,
                number(PropertyRef::TREE_DEPTH),
                number(PropertyRef::DESCENDANT_COUNT),
            )
        })
        .collect();
    assert_eq!(
        after,
        before
            .iter()
            .map(|row| (row.item, row.depth, row.descendants))
            .collect::<Vec<_>>(),
        "the hierarchy and every item ID are kept"
    );
}

/// `count` processes whose rows all cost the same bytes, in the table and in
/// the tree: the widest name, the same cells, and every one a kernel root, so
/// the publishable byte ceiling keeps a count that depends on the view alone.
fn uniform(count: u32) -> Fixed {
    let name = "\u{20000}".repeat(MAX_DISPLAY_NAME_CHARS);
    Fixed::new(
        (0..count)
            .map(|offset| record(10_000 + offset, &name, Lineage::new(0, 1), 1))
            .collect(),
    )
}

/// The rows a fresh view of `count` uniform processes publishes in the table,
/// and in the tree it switches to.
fn capacity(count: u32) -> (usize, usize) {
    let mut source = uniform(count);
    let mut h = harness(&mut source);
    let table = h.table().len();
    h.show_tree(true);
    (table, h.tree().len())
}

fn status(h: &Harness) -> String {
    h.text(srui_process_explorer::STATUS)
}

fn filter_status(h: &Harness) -> String {
    h.text(srui_process_explorer::FILTER_STATUS)
}

/// The statuses the switch commit itself set: only that one transaction.
fn switch_statuses(h: &Harness, from: u64) -> Vec<(u64, String)> {
    let transactions = h.session.collect_replayed_transactions(from).unwrap();
    assert_eq!(transactions.len(), 1, "the switch is one transaction");
    transactions[0]
        .operations
        .iter()
        .filter_map(|op| match &op.op {
            Some(operation::Op::SetProperty(set)) => {
                let text = match set.value.as_ref()?.value.as_ref()? {
                    srui_protocol::value::Value::StringValue(text) => text.clone(),
                    _ => return None,
                };
                Some((set.node_id, text))
            }
            _ => None,
        })
        .collect()
}

/// F1 (PX-018 verification): a tree row costs more bytes than a table row
/// (its Parent cell and two item properties), so a switch to the tree can
/// leave rows out at the byte ceiling that the table showed. The switch
/// commit itself counts them in the status line and the filter status, paused
/// or not, and the switch back, a filter and its reset restate the count for
/// the rows they publish. Exactly at the tree's ceiling nothing is left out.
#[test]
fn a_switch_to_the_tree_at_the_byte_ceiling_counts_the_rows_it_leaves_out() {
    // Far more than the tree's ceiling and inside the table's: the probe finds
    // the tree's ceiling for this row shape without restating the budget.
    let (table, at) = capacity(22_000);
    assert_eq!(table, 22_000, "the table shows every probe row");
    assert!(at < 22_000, "the tree's rows cost more: {at}");
    let at = u32::try_from(at).unwrap();
    for paused in [false, true] {
        // Exactly at the ceiling: the tree shows every row and says nothing.
        let mut source = uniform(at);
        let mut h = harness(&mut source);
        if paused {
            h.view().pause_view(&h.session).unwrap();
            h.sync();
        }
        let clean = status(&h);
        h.show_tree(true);
        assert_eq!(h.tree().len(), at as usize, "paused={paused}");
        assert_eq!(status(&h), clean, "nothing to count, paused={paused}");
        assert!(!status(&h).contains("beyond the publishable size limit"));
        assert_eq!(
            filter_status(&h),
            format!("Filter: none · showing all {at} rows")
        );

        // One row past it: the switch commit counts the one row it left out.
        let mut source = uniform(at + 1);
        let mut h = harness(&mut source);
        assert_eq!(h.table().len(), at as usize + 1);
        if paused {
            h.view().pause_view(&h.session).unwrap();
            h.sync();
        }
        let before = h.revision();
        h.show_tree(true);
        assert_eq!(h.tree().len(), at as usize, "paused={paused}");
        let expected = format!("{clean} · 1 row beyond the publishable size limit");
        let shown_of = format!(
            "Filter: none · showing {at} of {} rows; 1 beyond the publishable size limit",
            at + 1
        );
        assert_eq!(status(&h), expected, "paused={paused}");
        assert_eq!(filter_status(&h), shown_of);
        let set = switch_statuses(&h, before);
        assert!(set.contains(&(srui_process_explorer::STATUS.get(), expected.clone())));
        assert!(set.contains(&(srui_process_explorer::FILTER_STATUS.get(), shown_of)));
        // A filter while in the tree publishes other rows, and recounts them.
        h.view()
            .set_filter(&h.session, "nothing-matches-this")
            .unwrap();
        h.sync();
        assert_eq!(
            status(&h),
            clean,
            "a filter that leaves out nothing, paused={paused}"
        );
        h.view().set_filter(&h.session, "").unwrap();
        h.sync();
        assert_eq!(
            status(&h),
            expected,
            "the reset counts again, paused={paused}"
        );
        assert_eq!(h.tree().len(), at as usize);
        // A sort republishes the same rows and keeps the count.
        h.view().click_sort(&h.session, ProcessField::Cpu).unwrap();
        h.sync();
        assert_eq!(status(&h), expected);
        // The switch back shows every row again, and says so in its commit.
        let back = h.revision();
        h.show_tree(false);
        assert_eq!(h.table().len(), at as usize + 1);
        assert_eq!(status(&h), clean, "paused={paused}");
        let set = switch_statuses(&h, back);
        assert!(set.contains(&(srui_process_explorer::STATUS.get(), clean.clone())));
        assert_eq!(
            filter_status(&h),
            format!(
                "Committed filter (gen 2): none · showing all {} rows",
                at + 1
            )
        );
        if !paused {
            // A sample in the tree counts the same rows as the switch did.
            h.show_tree(true);
            h.sample(&mut source);
            assert_eq!(status(&h), expected);
        }
    }
}

/// A fixed source whose records a test edits between samples, each with its
/// own name, PID, parent and start tick.
fn family(records: &[(u32, &str, u32, u64)]) -> Fixed {
    Fixed::new(
        records
            .iter()
            .map(|&(pid, name, parent, started)| {
                record(pid, name, Lineage::new(parent, started), u64::from(pid))
            })
            .collect(),
    )
}

/// Codex review round 1, finding 1: the client keeps a collapsed item while
/// its model holds it, children or not (`OutlineCollectionAdapter
/// .pruneCollapsedItems`), so a collapsed process that loses its last child
/// and later gains one hides that child again. The server mirrors exactly
/// that: it keeps the collapsed item through the leaf phase, so a followed
/// process reparented under it later is reported hidden under it, and the
/// inspector, whose row the client no longer shows, is cleared.
#[test]
fn a_collapsed_process_that_loses_its_last_child_stays_collapsed_for_the_next_one() {
    let mut source = family(&[
        (10, "alpha", 0, 10),
        (11, "beta", 10, 11),
        (20, "foxtrot", 0, 50),
    ]);
    let mut h = harness(&mut source);
    h.show_tree(true);
    let alpha = h.item_of(10);
    let foxtrot = h.item_of(20);
    h.select(TREE, foxtrot);
    h.follow(true);
    h.disclose(alpha, false);
    assert!(h.view().collapsed_by(CLIENT).contains(&alpha));
    // `beta` ends: `alpha` has no child left, and stays collapsed, as on the
    // client.
    source
        .records
        .retain(|record| record.key.pid != Observed::Known(11));
    h.sample(&mut source);
    assert_eq!(
        h.tree()
            .iter()
            .find(|row| row.pid == 10)
            .unwrap()
            .descendants,
        0
    );
    assert!(
        h.view().collapsed_by(CLIENT).contains(&alpha),
        "a collapsed leaf stays collapsed while the tree holds it"
    );
    // `foxtrot` is reparented under `alpha`, which the client still keeps
    // collapsed: it is hidden there.
    for record in &mut source.records {
        if record.key.pid == Observed::Known(20) {
            record.lineage = Lineage::new(10, 50);
        }
    }
    h.sample(&mut source);
    assert_eq!(
        h.tree().iter().find(|row| row.pid == 20).unwrap().parent,
        "10"
    );
    assert_eq!(
        h.view().follow().following(),
        Some(foxtrot),
        "follow stays on"
    );
    let status = h.text(FOLLOW_STATUS);
    assert!(
        status.starts_with("Following PID 20 · foxtrot · hidden under collapsed PID 10 · alpha"),
        "{status}"
    );
    assert_eq!(h.text(INSPECTOR_STATUS), NO_SELECTION_STATUS);
    // Once `alpha` leaves the tree, it leaves the collapsed set too.
    source
        .records
        .retain(|record| record.key.pid != Observed::Known(10));
    h.sample(&mut source);
    assert!(!h.view().collapsed_by(CLIENT).contains(&alpha));
}

/// Codex review round 1, finding 2: a refresh, or a republish, that places
/// the selected process under a parent the client keeps collapsed hides its
/// row there, and the client restores no selection to a hidden row and
/// reports nothing (`OutlineCollectionAdapter.update` restores the selection
/// with selection events suppressed). The inspector is cleared in that same
/// plan instead of describing a process the client no longer shows selected.
#[test]
fn a_selected_process_moved_under_a_collapsed_parent_clears_the_inspector() {
    // A sample: `xray` is reparented under the collapsed `alpha`.
    let mut source = family(&[
        (10, "alpha", 0, 10),
        (11, "beta", 10, 11),
        (30, "xray", 0, 60),
    ]);
    let mut h = harness(&mut source);
    h.show_tree(true);
    let alpha = h.item_of(10);
    let xray = h.item_of(30);
    h.select(TREE, xray);
    h.disclose(alpha, false);
    assert!(h.text(INSPECTOR_STATUS).contains("PID 30"));
    for record in &mut source.records {
        if record.key.pid == Observed::Known(30) {
            record.lineage = Lineage::new(10, 60);
        }
    }
    let before = h.revision();
    h.sample(&mut source);
    assert_eq!(
        h.tree().iter().find(|row| row.pid == 30).unwrap().parent,
        "10"
    );
    assert_eq!(h.text(INSPECTOR_STATUS), NO_SELECTION_STATUS);
    assert_eq!(
        h.session
            .collect_replayed_transactions(before)
            .unwrap()
            .len(),
        1,
        "cleared in the sample's own commit"
    );

    // A republish: resetting a filter shows `mid` again, which puts the
    // selected `xray` back under the collapsed `alpha`.
    let mut source = family(&[
        (10, "alpha", 0, 10),
        (11, "beta", 10, 11),
        (12, "mid", 10, 12),
        (13, "xray", 12, 13),
    ]);
    let mut h = harness(&mut source);
    h.show_tree(true);
    h.view()
        .set_filter(&h.session, "alpha | beta | xray")
        .unwrap();
    h.sync();
    assert_eq!(
        h.tree().iter().find(|row| row.pid == 13).unwrap().parent,
        "root: parent 12 hidden by the filters"
    );
    let alpha = h.item_of(10);
    let xray = h.item_of(13);
    h.select(TREE, xray);
    h.follow(true);
    h.disclose(alpha, false);
    assert!(
        h.text(INSPECTOR_STATUS).contains("PID 13"),
        "xray is a shown root"
    );
    h.view().set_filter(&h.session, "").unwrap();
    h.sync();
    assert_eq!(h.tree().iter().find(|row| row.pid == 13).unwrap().depth, 2);
    assert_eq!(h.text(INSPECTOR_STATUS), NO_SELECTION_STATUS);
    assert!(h
        .text(FOLLOW_STATUS)
        .starts_with("Following PID 13 · xray · hidden under collapsed PID 10 · alpha"));
    // A switch starts a newly filled tree expanded: nothing is hidden there.
    h.show_tree(false);
    h.select(TABLE, xray);
    h.show_tree(true);
    assert!(h.view().collapsed_by(CLIENT).is_empty());
    assert!(h.text(INSPECTOR_STATUS).contains("PID 13"));
    assert!(h
        .text(FOLLOW_STATUS)
        .starts_with("Following PID 13 · xray · item 4 of 4 in the tree"));
}

/// Verification of Codex review round 1 (F1): a search or a PID jump that
/// promotes a row hidden under a collapsed parent reveals it on the client,
/// which expands exactly those parents and selects the row at once, and only
/// then reports each with `EXPANSION_CHANGED(true)`
/// (`OutlineCollectionAdapter.expandCollapsedAncestors`). The server's mirror
/// expands them when the promotion is planned, so a sample that lands before
/// those reports keeps the inspector on the match, and the reports, when
/// they come, change nothing.
#[test]
fn a_sample_between_a_search_reveal_and_its_expansion_report_keeps_the_match() {
    let mut source = family(&[
        (10, "alpha", 0, 10),
        (11, "beta", 10, 11),
        (30, "xray", 0, 60),
    ]);
    let mut h = harness(&mut source);
    h.show_tree(true);
    let alpha = h.item_of(10);
    let beta = h.item_of(11);
    h.disclose(alpha, false);
    h.view().jump_to_pid(&h.session, "11").unwrap();
    h.sync();
    assert_eq!(h.item_property(TREE, VALUE), Some(beta), "promoted");
    assert!(h.text(INSPECTOR_STATUS).contains("PID 11"));
    assert!(
        !h.view().collapsed_by(CLIENT).contains(&alpha),
        "the reveal expanded alpha in the mirror"
    );
    // The client's `EXPANSION_CHANGED(true)` is still in flight.
    h.sample(&mut source);
    assert!(
        h.text(INSPECTOR_STATUS).contains("PID 11"),
        "{}",
        h.text(INSPECTOR_STATUS)
    );
    let quiet = h.revision();
    h.disclose(alpha, true);
    assert_eq!(h.revision(), quiet, "the late report changes nothing");
    assert!(h.text(INSPECTOR_STATUS).contains("PID 11"));
    // The same for a search, with follow on: the followed match is shown.
    h.disclose(alpha, false);
    assert_eq!(h.text(INSPECTOR_STATUS), NO_SELECTION_STATUS);
    h.view().set_search(&h.session, "beta").unwrap();
    h.sync();
    h.follow(true);
    h.sample(&mut source);
    assert!(h.text(INSPECTOR_STATUS).contains("PID 11"));
    assert!(h
        .text(FOLLOW_STATUS)
        .starts_with("Following PID 11 · beta · item 2 of 3 in the tree"));
}

/// Codex review round 2: the client reports a disclosure change only for an
/// item with children in the revision it acted on, but a sample can take the
/// item's last child before the report arrives. The server applies the
/// report all the same, as the client did: when `alpha`, expanded on the
/// client, gains a child again, that child is shown, so the inspector keeps
/// the selected `xray` and follow reports it in the tree.
#[test]
fn a_disclosure_report_for_an_item_that_became_a_leaf_is_still_mirrored() {
    let mut source = family(&[
        (10, "alpha", 0, 10),
        (11, "beta", 10, 11),
        (30, "xray", 0, 60),
    ]);
    let mut h = harness(&mut source);
    h.show_tree(true);
    let alpha = h.item_of(10);
    let xray = h.item_of(30);
    h.select(TREE, xray);
    h.follow(true);
    h.disclose(alpha, false);
    // The client expands `alpha` again; before its report arrives, a sample
    // takes `alpha`'s last child.
    source
        .records
        .retain(|record| record.key.pid != Observed::Known(11));
    h.sample(&mut source);
    assert_eq!(
        h.tree()
            .iter()
            .find(|row| row.pid == 10)
            .unwrap()
            .descendants,
        0
    );
    h.disclose(alpha, true);
    assert!(
        !h.view().collapsed_by(CLIENT).contains(&alpha),
        "the late report is mirrored"
    );
    // `xray` is reparented under `alpha`, which the client shows expanded.
    for record in &mut source.records {
        if record.key.pid == Observed::Known(30) {
            record.lineage = Lineage::new(10, 60);
        }
    }
    h.sample(&mut source);
    assert_eq!(
        h.tree().iter().find(|row| row.pid == 30).unwrap().parent,
        "10"
    );
    assert!(
        h.text(INSPECTOR_STATUS).contains("PID 30"),
        "{}",
        h.text(INSPECTOR_STATUS)
    );
    assert!(h
        .text(FOLLOW_STATUS)
        .starts_with("Following PID 30 · xray · item 2 of 2 in the tree"));
    // The symmetric case: the client collapses `alpha` while `xray` is under
    // it, and before that report arrives a sample moves `xray` back to the
    // top. The collapse is mirrored all the same, and hides `xray` when it
    // returns under `alpha`.
    let set_parent = |source: &mut Fixed, parent: u32| {
        for record in &mut source.records {
            if record.key.pid == Observed::Known(30) {
                record.lineage = Lineage::new(parent, 60);
            }
        }
    };
    set_parent(&mut source, 0);
    h.sample(&mut source);
    assert_eq!(
        h.tree()
            .iter()
            .find(|row| row.pid == 10)
            .unwrap()
            .descendants,
        0
    );
    h.disclose(alpha, false);
    assert!(
        h.view().collapsed_by(CLIENT).contains(&alpha),
        "the late collapse is mirrored"
    );
    set_parent(&mut source, 10);
    h.sample(&mut source);
    assert_eq!(h.text(INSPECTOR_STATUS), NO_SELECTION_STATUS);
    assert!(h
        .text(FOLLOW_STATUS)
        .starts_with("Following PID 30 · xray · hidden under collapsed PID 10 · alpha"));
}

/// Codex review round 3: a `SELECTION_CHANGED` the client sent just before a
/// sample moved the selected row under a parent it keeps collapsed arrives
/// after that sample committed. The client has since dropped the selection
/// (`OutlineCollectionAdapter.update` restores it only to a shown row, and a
/// later expand restores nothing), so the delayed report leaves the
/// inspector cleared and does not retarget follow.
#[test]
fn a_delayed_selection_of_a_row_now_hidden_by_a_collapsed_parent_is_not_applied() {
    let mut source = family(&[
        (10, "alpha", 0, 10),
        (11, "beta", 10, 11),
        (30, "xray", 0, 60),
        (40, "zulu", 0, 70),
    ]);
    let mut h = harness(&mut source);
    h.show_tree(true);
    let alpha = h.item_of(10);
    let xray = h.item_of(30);
    let zulu = h.item_of(40);
    h.select(TREE, zulu);
    h.follow(true);
    h.disclose(alpha, false);
    // The user clicks `xray` while it is a shown root; before that report
    // arrives, a sample reparents it under the collapsed `alpha`.
    let observed = h.revision();
    for record in &mut source.records {
        if record.key.pid == Observed::Known(30) {
            record.lineage = Lineage::new(10, 60);
        }
    }
    h.sample(&mut source);
    assert_eq!(
        h.tree().iter().find(|row| row.pid == 30).unwrap().parent,
        "10"
    );
    // The delayed report, made at the earlier revision.
    h.seq += 1;
    let wire = DomainEvent::new(
        None,
        h.seq,
        format!("tree-event-{}", h.seq),
        observed,
        TREE,
        TypeRef::EVENT_SELECTION_CHANGED,
        vec![(PropertyRef::VALUE, Value::ItemId(xray))],
    )
    .with_client_instance_id(CLIENT.to_vec())
    .to_wire();
    let outcome = h.session.process_event(&wire).expect("event admitted");
    assert!(
        matches!(outcome, EventOutcome::Processed { .. }),
        "{outcome:?}"
    );
    h.sync();
    assert_eq!(h.text(INSPECTOR_STATUS), NO_SELECTION_STATUS);
    assert_eq!(
        h.view().follow().following(),
        Some(zulu),
        "follow stays on zulu"
    );
    assert!(h.text(FOLLOW_STATUS).starts_with("Following PID 40 · zulu"));
    // It stays cleared through the next sample.
    h.sample(&mut source);
    assert_eq!(h.text(INSPECTOR_STATUS), NO_SELECTION_STATUS);
    // Once `alpha` is expanded, `xray` can be selected again.
    h.disclose(alpha, true);
    h.select(TREE, xray);
    assert!(h.text(INSPECTOR_STATUS).contains("PID 30"));
}

/// Codex review round 4: disclosure is each client's own. Client A collapses
/// `alpha`, which hides the selected and followed `beta` there. A fresh
/// client then attaches to the same running session: its outline starts all
/// expanded and the snapshot carries no disclosure, so it shows `alpha`'s
/// children and reports nothing. Its selection of a child it shows is its
/// own: the inspector describes it, follow retargets to it and does not call
/// it hidden, and a sample keeps it, whatever A keeps collapsed.
#[test]
fn a_fresh_client_does_not_inherit_another_clients_collapsed_parent() {
    let mut source = family(&[
        (10, "alpha", 0, 10),
        (11, "beta", 10, 11),
        (12, "gamma", 10, 12),
        (30, "xray", 0, 60),
    ]);
    let mut h = harness(&mut source);
    h.show_tree(true);
    let alpha = h.item_of(10);
    let beta = h.item_of(11);
    let gamma = h.item_of(12);
    h.select(TREE, beta);
    h.follow(true);
    h.disclose(alpha, false);
    assert_eq!(h.text(INSPECTOR_STATUS), NO_SELECTION_STATUS);
    assert!(h
        .text(FOLLOW_STATUS)
        .starts_with("Following PID 11 · beta · hidden under collapsed PID 10 · alpha"));

    h.attach(OTHER);
    h.select_from(OTHER, TREE, gamma);
    let inspector = h.text(INSPECTOR_STATUS);
    assert!(
        inspector.starts_with("Selected process: PID 12 · gamma"),
        "{inspector}"
    );
    assert_eq!(h.view().follow().following(), Some(gamma));
    let follow = h.text(FOLLOW_STATUS);
    assert!(
        follow.starts_with("Following PID 12 · gamma · item 3 of 4 in the tree"),
        "{follow}"
    );
    // A sample re-evaluates the selection against the selecting client's
    // disclosure, not A's.
    h.sample(&mut source);
    assert!(h
        .text(INSPECTOR_STATUS)
        .starts_with("Selected process: PID 12 · gamma"));
    // The process A could no longer see is selectable on the fresh client.
    h.select_from(OTHER, TREE, beta);
    assert!(h
        .text(INSPECTOR_STATUS)
        .starts_with("Selected process: PID 11 · beta"));
    assert!(h
        .text(FOLLOW_STATUS)
        .starts_with("Following PID 11 · beta · item 2 of 4 in the tree"));
    // A's mirror is A's alone.
    assert!(h.view().collapsed_by(CLIENT).contains(&alpha));
    assert!(h.view().collapsed_by(OTHER).is_empty());
}

/// Codex review round 4, two clients at once: a collapse on one client never
/// clears the inspector or hides follow for a selection another client made
/// and still shows; the selecting client's own collapse does.
#[test]
fn concurrent_clients_each_hide_only_their_own_selection() {
    let mut source = family(&[
        (10, "alpha", 0, 10),
        (11, "beta", 10, 11),
        (12, "gamma", 10, 12),
        (30, "xray", 0, 60),
    ]);
    let mut h = harness(&mut source);
    h.show_tree(true);
    let alpha = h.item_of(10);
    let beta = h.item_of(11);
    // The other client selects and follows `beta`.
    h.select_from(OTHER, TREE, beta);
    h.follow(true);
    // This client collapses `alpha`: nothing the other client sees changes.
    let quiet = h.revision();
    h.disclose(alpha, false);
    assert_eq!(h.revision(), quiet, "nothing published");
    assert!(h
        .text(INSPECTOR_STATUS)
        .starts_with("Selected process: PID 11 · beta"));
    assert!(h
        .text(FOLLOW_STATUS)
        .starts_with("Following PID 11 · beta · item 2 of 4 in the tree"));
    h.sample(&mut source);
    assert!(h
        .text(INSPECTOR_STATUS)
        .starts_with("Selected process: PID 11 · beta"));
    // The other client collapses `alpha` too: its own selection is hidden.
    h.disclose_from(OTHER, alpha, false);
    assert_eq!(h.text(INSPECTOR_STATUS), NO_SELECTION_STATUS);
    assert!(h
        .text(FOLLOW_STATUS)
        .starts_with("Following PID 11 · beta · hidden under collapsed PID 10 · alpha"));
    // It expands it again and reselects; this client's collapse still
    // leaves that selection alone.
    h.disclose_from(OTHER, alpha, true);
    h.select_from(OTHER, TREE, beta);
    assert!(h
        .text(INSPECTOR_STATUS)
        .starts_with("Selected process: PID 11 · beta"));
    assert!(h
        .text(FOLLOW_STATUS)
        .starts_with("Following PID 11 · beta · item 2 of 4 in the tree"));
}

/// Codex review round 4: a Go to PID promotion reveals its match on every
/// client, and is the clicking client's selection: another client's later
/// collapse does not clear it.
#[test]
fn a_promotion_is_the_selection_of_the_client_that_asked_for_it() {
    let mut source = family(&[
        (10, "alpha", 0, 10),
        (11, "beta", 10, 11),
        (30, "xray", 0, 60),
    ]);
    let mut h = harness(&mut source);
    h.show_tree(true);
    let alpha = h.item_of(10);
    let beta = h.item_of(11);
    h.disclose(alpha, false);
    h.view().set_pid_input(&h.session, "11").unwrap();
    h.sync();
    h.send_from(OTHER, PID_GO, TypeRef::EVENT_ACTIVATE, vec![]);
    assert_eq!(h.item_property(TREE, VALUE), Some(beta), "promoted");
    assert!(h
        .text(INSPECTOR_STATUS)
        .starts_with("Selected process: PID 11 · beta"));
    // This client re-collapses `alpha` after the reveal: the promotion was
    // the other client's, which still shows it.
    h.disclose(alpha, false);
    assert!(h
        .text(INSPECTOR_STATUS)
        .starts_with("Selected process: PID 11 · beta"));
}

/// PX-017-G02 merged with PX-018: a user deselect (`SELECTION_CHANGED` with
/// no `value`) in the shown tree clears the inspector and stops follow, as
/// in the table; one from the hidden table, sent before the switch reached
/// that client, is ignored like a selection from it. A collapse is not a
/// deselect: follow stays on and says where the process is hidden.
#[test]
fn a_user_deselect_in_the_tree_stops_follow_and_one_from_the_hidden_table_is_ignored() {
    let mut source = family(&[
        (10, "alpha", 0, 10),
        (11, "beta", 10, 11),
        (30, "xray", 0, 60),
    ]);
    let mut h = harness(&mut source);
    h.show_tree(true);
    let alpha = h.item_of(10);
    let beta = h.item_of(11);
    h.select(TREE, beta);
    h.follow(true);
    h.disclose(alpha, false);
    assert_eq!(h.text(INSPECTOR_STATUS), NO_SELECTION_STATUS);
    assert_eq!(
        h.view().follow().following(),
        Some(beta),
        "a collapse is not a deselect"
    );
    h.disclose(alpha, true);
    h.select(TREE, beta);
    h.send(TABLE, TypeRef::EVENT_SELECTION_CHANGED, vec![]);
    assert!(h
        .text(INSPECTOR_STATUS)
        .starts_with("Selected process: PID 11 · beta"));
    assert_eq!(h.view().follow().following(), Some(beta));
    h.send(TREE, TypeRef::EVENT_SELECTION_CHANGED, vec![]);
    assert_eq!(h.text(INSPECTOR_STATUS), CLEARED_SELECTION_STATUS);
    assert_eq!(h.view().follow().following(), None);
    assert_eq!(h.text(FOLLOW_STATUS), SELECTION_CLEARED_FOLLOW_STATUS);
}
