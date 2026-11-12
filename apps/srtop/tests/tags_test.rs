//! PX-020 acceptance: stable multi-selection and descendant tagging (D1
//! §§6.2, 7.6, 8; D2 T28).
//!
//! * Tags are explicit, server-side and kept by process instance
//!   (`ProcessKey`), independent of the selection: they survive sorting,
//!   filtering, the column chooser and the view switch, and clearing them
//!   leaves the selection, the promoted `value`, follow and the inspector
//!   untouched. A tag request publishes only tag cells and the tags status.
//! * "Tag selected and descendants" freezes the membership the latest sample
//!   shows: a child started later is not added, a filtered descendant is
//!   tagged all the same and counted as hidden.
//! * A tagged process that leaves the collection keeps a bounded historical
//!   tag; a newer process reusing its PID inherits nothing.
//! * Threads are never tagged and never descendants; a hidden kernel thread's
//!   tag is counted; a row hidden under a collapsed parent cannot be tagged.
//! * A request beyond the tag limit is refused whole.
//! * Merged with main: a request acts on the process the inspector
//!   describes, so a user-cleared selection (PX-017-G02) tags nothing and
//!   only the selecting client's collapse takes the target (PX-018); a
//!   tagged row the row limit drops unconfirmed turns historical while the
//!   inspector never calls it ended (PX-013 review round).
//!
//! Every request goes through the real handler (`ACTIVATE` on the tag
//! buttons) and every commit is applied by a client replica.

use srui_process_explorer::catalogue::ProcessField;
use srui_process_explorer::refresh::{lock_view, ProcessView, SharedView};
use srui_process_explorer::source::*;
use srui_process_explorer::tags::{MAX_HISTORICAL_TAGS, MAX_TAGGED, TAGGED_CELL, TAG_HEADING};
use srui_process_explorer::{
    install_follow_handlers, install_inspector, install_tag_handlers, install_view_handlers,
    start_from_source, FOLLOW_STATUS, INSPECTOR_STATUS, MODEL, TABLE, TAGS_CLEAR, TAGS_STATUS,
    TAG_DESCENDANTS, TAG_TOGGLE, TREE, TREE_MODEL, VIEW_TOGGLE,
};
use srui_sdk::{
    ItemId, NodeId, Operation, PropertyRef, TypeRef, Value, COLUMNS, KEEP_VISIBLE, TEXT, VALUE,
};
use srui_semantic_tree::{Event as DomainEvent, SemanticStore};
use srui_sessiond::{EventOutcome, Session};
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

const CLIENT: &[u8] = b"tags-client-instance";
const SOURCE: &str = "tags-fixture-v1";

/// One record: PID, name, parent PID and start tick. The name and start tick
/// make the creation token, so a new name or start under an old PID is a new
/// process instance.
fn record(pid: u32, name: &str, parent: u32, started: u64) -> ProcessRecord {
    ProcessRecord {
        kind: KIND_NOT_COLLECTED,
        lineage: Lineage::new(parent, started),
        owner: OWNER_NOT_COLLECTED,
        key: ProcessKey {
            source: SourceId(SOURCE.into()),
            host: Observed::Known(HostId("tags-host".into())),
            boot: Observed::Known(BootId("tags-boot".into())),
            pid_namespace: Observed::Known(PidNamespaceId(7)),
            pid: Observed::Known(pid),
            creation: CreationToken::Opaque(format!("{name}-{started}")),
            thread: None,
        },
        display_name: DisplayName::from(name),
        resident: Observed::Known(4096),
        cpu: CpuUsage::Measured(CpuInterval {
            // Distinct shares, so a CPU sort orders every row.
            ticks: u64::from(pid % 97) + 1,
            ticks_per_second: 100,
            elapsed: Duration::from_secs(1),
        }),
    }
}

/// A source that publishes `steps` in turn, then repeats the last one. A
/// step in `unenumerable` is a scan that cannot say what it missed, so every
/// absent row is kept from an earlier scan (PX-004).
struct Steps {
    steps: Vec<Vec<ProcessRecord>>,
    unenumerable: BTreeSet<usize>,
    next: usize,
}

impl Steps {
    fn new(steps: Vec<Vec<ProcessRecord>>) -> Self {
        Self {
            steps,
            unenumerable: BTreeSet::new(),
            next: 0,
        }
    }
}

impl ProcessSource for Steps {
    fn status_text(&self) -> &str {
        "Read-only · Tags fixture"
    }

    fn snapshot(&mut self) -> ProcessSnapshot {
        let index = self.next.min(self.steps.len() - 1);
        self.next += 1;
        ProcessSnapshot {
            source: SourceId(SOURCE.into()),
            sampled_at: SnapshotTime(
                SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000 + self.next as u64),
            ),
            records: self.steps[index].clone(),
            vanished: 0,
            threads: ThreadScan::NotProvided,
            capped: CappedRecords::none(),
            completeness: if self.unenumerable.contains(&index) {
                Completeness::from_scan(SkippedRecords::unenumerable(), Vec::new())
            } else {
                Completeness::Complete
            },
            system: SystemSample::not_provided(),
        }
    }
}

/// `build` (10) with `cc` (11) and `ld` (12) under it and `as` (13) under
/// `cc`, beside `editor` (20), all under `init` (1).
fn family() -> Vec<ProcessRecord> {
    vec![
        record(1, "init", 0, 1),
        record(10, "build", 1, 10),
        record(11, "cc", 10, 11),
        record(12, "ld", 10, 12),
        record(13, "as", 11, 13),
        record(20, "editor", 1, 20),
    ]
}

struct Harness {
    session: Arc<Session>,
    view: SharedView,
    replica: SemanticStore,
    seq: u64,
}

fn harness(source: &mut impl ProcessSource) -> Harness {
    harness_limited(source, usize::MAX)
}

/// [`harness`] with the published-row ceiling lowered to `limit`.
fn harness_limited(source: &mut impl ProcessSource, limit: usize) -> Harness {
    let session = Arc::new(Session::mint());
    let (view, _) = start_from_source(&session, source).unwrap();
    let view = view.with_published_limit(limit);
    let view: SharedView = Arc::new(Mutex::new(view));
    install_tag_handlers(&session, &view);
    install_view_handlers(&session, &view);
    install_follow_handlers(&session, &view);
    install_inspector(&session, &view, None);
    let mut harness = Harness {
        session,
        view,
        replica: SemanticStore::new(),
        seq: 0,
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

    /// Applies every committed transaction as a client does.
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

    fn send(&mut self, wire: srui_protocol::Event) {
        let outcome = self.session.process_event(&wire).expect("event admitted");
        assert!(
            matches!(outcome, EventOutcome::Processed { .. }),
            "not processed: {outcome:?}"
        );
        self.sync();
    }

    fn event(&mut self, node: NodeId, event: TypeRef, arguments: Vec<(PropertyRef, Value)>) {
        self.seq += 1;
        let wire = DomainEvent::new(
            None,
            self.seq,
            format!("tags-event-{}", self.seq),
            self.revision(),
            node,
            event,
            arguments,
        )
        .with_client_instance_id(CLIENT.to_vec())
        .to_wire();
        self.send(wire);
    }

    /// A click on a tag button: an `ACTIVATE` that names only the button.
    fn click(&mut self, button: NodeId) {
        self.click_as(CLIENT, button);
    }

    /// [`Self::click`], by the client instance `client`.
    fn click_as(&mut self, client: &[u8], button: NodeId) {
        self.seq += 1;
        let wire = DomainEvent::activate(
            self.seq,
            format!("tags-click-{}", self.seq),
            self.revision(),
            button,
        )
        .with_client_instance_id(client.to_vec())
        .to_wire();
        self.send(wire);
    }

    /// `client` collapses or expands the tree row of `pid` locally and
    /// reports it with `EXPANSION_CHANGED` (PX-018).
    fn disclose_as(&mut self, client: &[u8], pid: u64, expanded: bool) {
        let item = self.item_of(pid);
        self.seq += 1;
        let wire = DomainEvent::expansion_changed(
            self.seq,
            format!("tags-disclosure-{}", self.seq),
            self.revision(),
            TREE,
            item,
            expanded,
        )
        .with_client_instance_id(client.to_vec())
        .to_wire();
        self.send(wire);
    }

    fn select(&mut self, pid: u64) {
        let node = self.shown_collection();
        let item = self.item_of(pid);
        self.event(
            node,
            TypeRef::EVENT_SELECTION_CHANGED,
            vec![(PropertyRef::VALUE, Value::ItemId(item))],
        );
    }

    fn show_tree(&mut self, on: bool) {
        self.event(
            VIEW_TOGGLE,
            TypeRef::EVENT_VALUE_CHANGED,
            vec![(PropertyRef::VALUE, Value::Bool(on))],
        );
    }

    fn follow(&mut self, on: bool) {
        self.event(
            srui_process_explorer::FOLLOW_TOGGLE,
            TypeRef::EVENT_VALUE_CHANGED,
            vec![(PropertyRef::VALUE, Value::Bool(on))],
        );
    }

    fn sample(&mut self, source: &mut impl ProcessSource) {
        self.view().refresh(&self.session, source).unwrap();
        self.sync();
    }

    fn with_view(&mut self, change: impl FnOnce(&mut ProcessView, &Session)) {
        {
            let session = Arc::clone(&self.session);
            let mut view = lock_view(&self.view);
            change(&mut view, &session);
        }
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

    fn status(&self) -> String {
        self.text(TAGS_STATUS)
    }

    fn shown_collection(&self) -> NodeId {
        if self.property(VIEW_TOGGLE, VALUE) == Some(Value::Bool(true)) {
            TREE
        } else {
            TABLE
        }
    }

    fn headings(&self, node: NodeId) -> Vec<String> {
        match self.property(node, COLUMNS) {
            Some(Value::List(headings)) => headings
                .iter()
                .map(|heading| match heading {
                    Value::String(text) => text.clone(),
                    other => panic!("a heading: {other:?}"),
                })
                .collect(),
            other => panic!("headings: {other:?}"),
        }
    }

    /// The shown collection's rows as the replica holds them: item, PID, name
    /// and tag cell, in published order.
    fn rows(&self) -> Vec<(ItemId, u64, String, String)> {
        let node = self.shown_collection();
        let model = if node == TREE { TREE_MODEL } else { MODEL };
        let headings = self.headings(node);
        let at = |heading: &str| {
            headings
                .iter()
                .position(|shown| shown == heading)
                .unwrap_or_else(|| panic!("no {heading} column in {headings:?}"))
        };
        let (pid, name, tag) = (
            at(ProcessField::Pid.heading()),
            at(ProcessField::Name.heading()),
            at(TAG_HEADING),
        );
        let model = self.replica.get_model(model).unwrap();
        (0..model.item_count)
            .map(|index| {
                let item = model.get_item_by_index(index).unwrap();
                let Value::List(cells) = &item.value else {
                    panic!("a row is a list of cells")
                };
                let text = |cell: &Value| match cell {
                    Value::String(text) => text.clone(),
                    Value::UnsignedInt(number) => number.to_string(),
                    other => format!("{other:?}"),
                };
                (
                    item.item_id,
                    text(&cells[pid]).parse().unwrap_or(0),
                    text(&cells[name]),
                    text(&cells[tag]),
                )
            })
            .collect()
    }

    fn item_of(&self, pid: u64) -> ItemId {
        self.rows()
            .into_iter()
            .find(|row| row.1 == pid)
            .unwrap_or_else(|| panic!("PID {pid} is shown"))
            .0
    }

    /// The PIDs whose shown rows' tag cells say tagged.
    fn tagged(&self) -> BTreeSet<u64> {
        self.rows()
            .into_iter()
            .filter(|row| {
                assert!(row.3.is_empty() || row.3 == TAGGED_CELL, "{row:?}");
                row.3 == TAGGED_CELL
            })
            .map(|row| row.1)
            .collect()
    }

    /// Every operation committed after `revision`.
    fn operations_since(&self, revision: u64) -> Vec<Operation> {
        self.session
            .collect_replayed_transactions(revision)
            .expect("the journal holds this run")
            .into_iter()
            .flat_map(|transaction| transaction.operations)
            .map(|op| Operation::try_from(op).expect("a committed operation decodes"))
            .collect()
    }
}

fn set(pids: &[u64]) -> BTreeSet<u64> {
    pids.iter().copied().collect()
}

/// The only operations a tag request may commit: the tag cells of rows that
/// changed, updated in place by identity, and the tags status.
fn assert_only_tag_cells_and_status(operations: &[Operation]) {
    assert!(!operations.is_empty(), "the request published nothing");
    for operation in operations {
        match operation {
            Operation::ModelUpdate { id, .. } => {
                assert!(*id == MODEL || *id == TREE_MODEL, "{operation:?}")
            }
            Operation::SetProperty { id, property, .. } => {
                assert_eq!((*id, *property), (TAGS_STATUS, TEXT), "{operation:?}")
            }
            other => panic!("a tag request committed {other:?}"),
        }
    }
}

#[test]
fn descendant_tags_are_frozen_survive_filters_and_turn_historical_when_a_process_ends() {
    let mut later = family();
    // A child `build` starts after the request.
    later.push(record(14, "late", 10, 14));
    // Then `cc` ends; a newer process reuses its PID; `as` is reparented to
    // init, the same instance.
    let mut reused: Vec<ProcessRecord> = later
        .iter()
        .filter(|record| record.key.pid != Observed::Known(11))
        .cloned()
        .collect();
    reused.push(record(11, "cc-next", 1, 30));
    for record in &mut reused {
        if record.key.pid == Observed::Known(13) {
            record.lineage = Lineage::new(1, 13);
        }
    }
    let mut source = Steps::new(vec![family(), later, reused]);
    let mut h = harness(&mut source);
    assert_eq!(
        h.headings(TABLE).last().map(String::as_str),
        Some(TAG_HEADING),
        "the tag column follows the chosen columns"
    );
    assert!(h.status().starts_with("Tags: none"), "{}", h.status());

    h.select(10);
    let inspector = h.text(INSPECTOR_STATUS);
    let revision = h.revision();
    h.click(TAG_DESCENDANTS);
    assert_eq!(h.tagged(), set(&[10, 11, 12, 13]));
    assert_eq!(
        h.status(),
        "Tags: 4 tagged · Tagged PID 10 · build and 3 sampled descendants; processes started later are not added"
    );
    assert_only_tag_cells_and_status(&h.operations_since(revision));
    assert_eq!(
        h.text(INSPECTOR_STATUS),
        inspector,
        "the selection is untouched"
    );

    // A filter hides three of them; they stay tagged and are counted.
    h.with_view(|view, session| {
        view.set_filter(session, "build|editor").unwrap();
    });
    assert_eq!(h.tagged(), set(&[10]));
    assert!(
        h.status()
            .starts_with("Tags: 4 tagged · 3 hidden by the filters · "),
        "{}",
        h.status()
    );
    h.with_view(|view, session| {
        view.set_filter(session, "").unwrap();
    });
    assert_eq!(h.tagged(), set(&[10, 11, 12, 13]));
    assert!(
        h.status().starts_with("Tags: 4 tagged · Tagged"),
        "{}",
        h.status()
    );

    // `late` started after the request: never added.
    h.sample(&mut source);
    assert!(h.rows().iter().any(|row| row.1 == 14), "late is shown");
    assert_eq!(h.tagged(), set(&[10, 11, 12, 13]));
    let cc = h.item_of(11);

    // `cc` ended and its PID was reused: the newcomer is not tagged, and
    // `cc`'s tag is historical. `as`, reparented, keeps its own.
    h.sample(&mut source);
    let newcomer = h.item_of(11);
    assert_ne!(newcomer, cc, "a reused PID is another process instance");
    assert_eq!(h.tagged(), set(&[10, 12, 13]));
    assert!(
        h.status()
            .starts_with("Tags: 3 tagged · 1 no longer listed: PID 11 · cc · "),
        "{}",
        h.status()
    );
    assert_eq!(h.view().tags().tagged(), 3);
    assert_eq!(
        h.view().tags().history().collect::<Vec<_>>(),
        ["PID 11 · cc"]
    );

    // Tagging the newcomer tags it alone, under its own identity.
    h.select(11);
    h.click(TAG_TOGGLE);
    assert_eq!(h.tagged(), set(&[10, 11, 12, 13]));
    assert!(
        h.status().ends_with("Tagged PID 11 · cc-next"),
        "{}",
        h.status()
    );

    // Clear removes every tag, live and historical, and nothing else.
    let inspector = h.text(INSPECTOR_STATUS);
    let revision = h.revision();
    h.click(TAGS_CLEAR);
    assert!(h.tagged().is_empty());
    assert_eq!(
        h.status(),
        "Tags: none · a tag marks a process instance on this server and changes no process · Cleared 4 tags and 1 historical; the selection is unchanged"
    );
    assert_eq!(h.view().tags().historical(), 0);
    assert_only_tag_cells_and_status(&h.operations_since(revision));
    assert_eq!(h.text(INSPECTOR_STATUS), inspector);
}

#[test]
fn tags_survive_sorting_the_column_chooser_and_the_view_switch_and_clearing_keeps_the_selection() {
    let mut source = Steps::new(vec![family()]);
    let mut h = harness(&mut source);
    h.select(12);
    h.click(TAG_TOGGLE);
    assert_eq!(h.tagged(), set(&[12]));
    assert!(h.status().ends_with("Tagged PID 12 · ld"), "{}", h.status());
    let ld = h.item_of(12);

    // Follow the selected `ld`; a search promotes `editor`.
    h.follow(true);
    assert_eq!(h.property(TABLE, KEEP_VISIBLE), Some(Value::ItemId(ld)));
    h.select(20);
    h.click(TAG_TOGGLE);
    assert_eq!(h.tagged(), set(&[12, 20]));
    h.with_view(|view, session| {
        view.set_search(session, "editor").unwrap();
    });
    let editor = h.item_of(20);
    assert_eq!(h.property(TABLE, VALUE), Some(Value::ItemId(editor)));

    // A CPU sort reorders rows; the tags stay on their instances.
    let order: Vec<u64> = h.rows().iter().map(|row| row.1).collect();
    h.with_view(|view, session| {
        view.click_sort(session, ProcessField::Cpu).unwrap();
    });
    assert_ne!(h.rows().iter().map(|row| row.1).collect::<Vec<_>>(), order);
    assert_eq!(h.tagged(), set(&[12, 20]));
    assert_eq!(h.item_of(12), ld);

    // The column chooser: the tag column still follows the chosen columns.
    h.with_view(|view, session| {
        view.set_columns(session, &[ProcessField::Name, ProcessField::Pid])
            .unwrap();
    });
    assert_eq!(h.headings(TABLE), ["Name", "PID", TAG_HEADING]);
    assert_eq!(h.tagged(), set(&[12, 20]));

    // The tree carries the same tags, its tag cell before Parent.
    h.show_tree(true);
    assert_eq!(h.headings(TREE), ["Name", "PID", TAG_HEADING, "Parent"]);
    assert_eq!(h.tagged(), set(&[12, 20]));
    h.show_tree(false);
    assert_eq!(h.tagged(), set(&[12, 20]));

    // Clearing changes no selection, promotion, follow or inspector.
    let selection = (
        h.property(TABLE, VALUE),
        h.property(TABLE, KEEP_VISIBLE),
        h.text(INSPECTOR_STATUS),
        h.text(FOLLOW_STATUS),
    );
    let revision = h.revision();
    h.click(TAGS_CLEAR);
    assert!(h.tagged().is_empty());
    assert_only_tag_cells_and_status(&h.operations_since(revision));
    assert_eq!(
        (
            h.property(TABLE, VALUE),
            h.property(TABLE, KEEP_VISIBLE),
            h.text(INSPECTOR_STATUS),
            h.text(FOLLOW_STATUS),
        ),
        selection
    );

    // Toggle tags and untags the selected `editor`; nothing selected tags
    // nothing.
    h.click(TAG_TOGGLE);
    assert_eq!(h.tagged(), set(&[20]));
    h.click(TAG_TOGGLE);
    assert!(h.tagged().is_empty());
    assert!(
        h.status().ends_with("Untagged PID 20 · editor"),
        "{}",
        h.status()
    );
    h.event(
        TABLE,
        TypeRef::EVENT_SELECTION_CHANGED,
        vec![(PropertyRef::VALUE, Value::Null)],
    );
    h.click(TAG_TOGGLE);
    h.click(TAG_DESCENDANTS);
    assert!(h.tagged().is_empty());
    assert!(
        h.status()
            .ends_with("Nothing tagged: select a process row first"),
        "{}",
        h.status()
    );
}

#[test]
fn threads_are_never_tagged_and_a_hidden_kernel_thread_tag_is_counted() {
    let mut source = ThreadsSource::default();
    let mut h = harness(&mut source);
    h.with_view(|view, session| {
        view.set_user_threads(session, Some(true)).unwrap();
    });
    h.sample(&mut source);
    // TID 201 of PID 200: a thread row.
    let thread = h
        .rows()
        .into_iter()
        .find(|row| row.2 == "worker-1")
        .expect("worker-1 is shown")
        .0;
    h.event(
        TABLE,
        TypeRef::EVENT_SELECTION_CHANGED,
        vec![(PropertyRef::VALUE, Value::ItemId(thread))],
    );
    h.click(TAG_TOGGLE);
    assert!(h.tagged().is_empty());
    assert!(
        h.status().ends_with(
            "TID 201 of PID 200 · worker-1 is a thread: tags mark processes, so select its process row"
        ),
        "{}",
        h.status()
    );
    h.click(TAG_DESCENDANTS);
    assert!(h.tagged().is_empty());

    // Its process and its sampled descendants: the process alone, never its
    // threads, whose rows share its PID.
    let server = h
        .rows()
        .into_iter()
        .find(|row| row.1 == 200 && row.2 == "server")
        .expect("server is shown")
        .0;
    h.event(
        TABLE,
        TypeRef::EVENT_SELECTION_CHANGED,
        vec![(PropertyRef::VALUE, Value::ItemId(server))],
    );
    h.click(TAG_DESCENDANTS);
    let tagged_rows: Vec<_> = h
        .rows()
        .into_iter()
        .filter(|row| row.3 == TAGGED_CELL)
        .collect();
    assert_eq!(tagged_rows.len(), 1, "{tagged_rows:?}");
    assert_eq!(tagged_rows[0].0, server);
    assert!(
        h.status()
            .ends_with("Tagged PID 200 · server and 0 sampled descendants; processes started later are not added"),
        "{}",
        h.status()
    );

    // A tagged kernel thread hidden by its checkbox is counted, not dropped.
    h.select(2);
    h.click(TAG_TOGGLE);
    assert_eq!(h.view().tags().tagged(), 2);
    h.with_view(|view, session| {
        view.set_kernel_threads(session, Some(false)).unwrap();
    });
    assert!(
        h.status()
            .starts_with("Tags: 2 tagged · 1 kernel thread hidden · "),
        "{}",
        h.status()
    );
    assert_eq!(h.view().tags().historical(), 0, "hiding is not ending");
    // Hiding user threads drops thread rows only; no tag changes.
    h.with_view(|view, session| {
        view.set_user_threads(session, Some(false)).unwrap();
    });
    h.sample(&mut source);
    h.with_view(|view, session| {
        view.set_kernel_threads(session, Some(true)).unwrap();
    });
    assert_eq!(h.view().tags().tagged(), 2);
    assert_eq!(h.view().tags().historical(), 0);
    assert_eq!(h.tagged(), set(&[2, 200]));
}

#[test]
fn a_row_hidden_under_a_collapsed_parent_cannot_be_tagged_and_is_not_counted_hidden() {
    let mut source = Steps::new(vec![family()]);
    let mut h = harness(&mut source);
    h.show_tree(true);
    h.select(11);
    h.click(TAG_TOGGLE);
    assert_eq!(h.tagged(), set(&[11]));
    // Collapsing `build` hides `cc`: the client dropped the selection, and so
    // did the inspector, so the shortcut tags nothing.
    let build = h.item_of(10);
    h.seq += 1;
    let wire = DomainEvent::expansion_changed(
        h.seq,
        format!("tags-disclosure-{}", h.seq),
        h.revision(),
        TREE,
        build,
        false,
    )
    .with_client_instance_id(CLIENT.to_vec())
    .to_wire();
    h.send(wire);
    assert_eq!(
        h.text(INSPECTOR_STATUS),
        srui_process_explorer::inspector::NO_SELECTION_STATUS
    );
    h.click(TAG_TOGGLE);
    assert_eq!(h.view().tags().tagged(), 1, "the hidden row stays tagged");
    assert!(
        h.status()
            .ends_with("Nothing tagged: select a process row first"),
        "{}",
        h.status()
    );
    // A collapsed row is in the model, not hidden by a filter.
    assert!(!h.status().contains("hidden"), "{}", h.status());
}

/// Merge with main (PX-018 × PX-020): disclosure is each client's own, so
/// another client's collapse never takes the selection a tag request acts
/// on. A tag request acts on the process the inspector describes, which
/// every client is shown, whichever client clicks. Only the selecting
/// client's collapse drops the selection, and then nothing is tagged.
#[test]
fn only_the_selecting_clients_collapse_takes_the_tag_target() {
    const OTHER: &[u8] = b"tags-other-client-instance";
    let mut source = Steps::new(vec![family()]);
    let mut h = harness(&mut source);
    h.show_tree(true);
    h.select(11);
    // The other client hides `cc` under `build` on its own screen only, and
    // a sample republishes the tree over that report.
    h.disclose_as(OTHER, 10, false);
    h.sample(&mut source);
    assert!(
        h.text(INSPECTOR_STATUS)
            .starts_with("Selected process: PID 11 · cc"),
        "{}",
        h.text(INSPECTOR_STATUS)
    );
    h.click_as(OTHER, TAG_TOGGLE);
    assert_eq!(h.view().tags().tagged(), 1);
    assert_eq!(h.tagged(), set(&[11]));
    assert!(h.status().ends_with("Tagged PID 11 · cc"), "{}", h.status());
    // The selecting client collapses `build` too: its selection is gone.
    h.disclose_as(CLIENT, 10, false);
    assert_eq!(
        h.text(INSPECTOR_STATUS),
        srui_process_explorer::inspector::NO_SELECTION_STATUS
    );
    h.click_as(OTHER, TAG_TOGGLE);
    assert_eq!(h.view().tags().tagged(), 1, "the hidden row stays tagged");
    assert!(
        h.status()
            .ends_with("Nothing tagged: select a process row first"),
        "{}",
        h.status()
    );
}

/// Merge with main (PX-017-G02 × PX-020): a selection the user cleared is
/// not a tag target, and clearing the tags leaves the cleared selection and
/// the follow status it stopped with as they are.
#[test]
fn a_cleared_selection_tags_nothing_and_clearing_tags_keeps_it_cleared() {
    let mut source = Steps::new(vec![family()]);
    let mut h = harness(&mut source);
    h.select(10);
    h.click(TAG_DESCENDANTS);
    assert_eq!(h.tagged(), set(&[10, 11, 12, 13]));
    h.follow(true);
    let node = h.shown_collection();
    h.event(node, TypeRef::EVENT_SELECTION_CHANGED, Vec::new());
    assert_eq!(
        h.text(INSPECTOR_STATUS),
        srui_process_explorer::inspector::CLEARED_SELECTION_STATUS
    );
    assert_eq!(
        h.text(FOLLOW_STATUS),
        srui_process_explorer::follow::SELECTION_CLEARED_FOLLOW_STATUS
    );
    for button in [TAG_TOGGLE, TAG_DESCENDANTS] {
        h.click(button);
        assert_eq!(h.tagged(), set(&[10, 11, 12, 13]), "nothing changed");
        assert!(
            h.status()
                .ends_with("Nothing tagged: select a process row first"),
            "{}",
            h.status()
        );
    }
    let promoted = h.property(node, VALUE);
    h.click(TAGS_CLEAR);
    assert!(h.tagged().is_empty());
    assert_eq!(
        h.text(INSPECTOR_STATUS),
        srui_process_explorer::inspector::CLEARED_SELECTION_STATUS,
        "clearing tags is not a selection"
    );
    assert_eq!(
        h.text(FOLLOW_STATUS),
        srui_process_explorer::follow::SELECTION_CLEARED_FOLLOW_STATUS
    );
    assert_eq!(h.property(node, VALUE), promoted);
}

/// Merge with main (PX-013 review round, PX-019 × PX-020): a tagged process
/// that retention kept unconfirmed and the row ceiling then dropped is no
/// longer listed, so its tag turns historical, worded as not listed, while
/// the inspector says it was dropped under the row limit, never ended. A
/// request on it names it as no longer shown. When a later scan lists the
/// same instance again, under a new item ID, the historical tag is not put
/// back on it.
#[test]
fn a_tagged_process_the_row_limit_drops_turns_historical_and_is_never_called_ended() {
    let first = vec![
        record(1, "init", 0, 1),
        record(10, "a", 1, 10),
        record(20, "b", 1, 20),
        record(30, "c", 1, 30),
    ];
    let second = vec![
        record(1, "init", 0, 1),
        record(10, "a", 1, 10),
        record(30, "c", 1, 30),
        record(40, "d", 1, 40),
    ];
    let mut source = Steps::new(vec![first.clone(), second, first]);
    source.unenumerable.insert(1);
    let mut h = harness_limited(&mut source, 4);
    h.select(20);
    let b = h.item_of(20);
    h.click(TAG_TOGGLE);
    assert_eq!(h.tagged(), set(&[20]));

    // An incomplete scan keeps `b` unconfirmed, and `d` takes its place
    // under the ceiling.
    h.sample(&mut source);
    assert!(
        h.rows().iter().all(|row| row.0 != b),
        "the ceiling dropped b"
    );
    assert_eq!(h.view().tags().tagged(), 0);
    assert_eq!(h.view().tags().historical(), 1);
    assert!(
        h.status()
            .starts_with("Tags: 0 tagged · 1 no longer listed: PID 20 · b"),
        "{}",
        h.status()
    );
    let inspector = h.text(INSPECTOR_STATUS);
    assert!(
        inspector.ends_with("not shown (dropped under the row limit)"),
        "{inspector}"
    );
    h.click(TAG_TOGGLE);
    assert!(
        h.status()
            .ends_with("Nothing tagged: PID 20 · b is no longer shown"),
        "{}",
        h.status()
    );
    assert_eq!(h.view().tags().tagged(), 0);

    // A complete scan lists `b` again, as a new row: untagged.
    h.sample(&mut source);
    let again = h.item_of(20);
    assert_ne!(again, b);
    assert!(h.tagged().is_empty(), "{:?}", h.tagged());
    assert_eq!(h.view().tags().historical(), 1);
    for text in [h.text(INSPECTOR_STATUS), h.status()] {
        assert!(
            !text.contains("ended"),
            "an eviction was called an exit: {text}"
        );
    }
}

/// `root` (1000) and `children` processes under it, then a step in which
/// every child has ended.
fn crowd(children: u32) -> Steps {
    let mut first = vec![record(1, "init", 0, 1), record(1000, "root", 1, 2)];
    first.extend((1..=children).map(|index| record(1000 + index, &format!("c{index}"), 1000, 3)));
    let second = vec![record(1, "init", 0, 1), record(1000, "root", 1, 2)];
    Steps::new(vec![first, second])
}

#[test]
fn historical_tags_are_bounded_newest_kept_and_cleared() {
    let children = MAX_HISTORICAL_TAGS as u32 + 6;
    let mut source = crowd(children);
    let mut h = harness(&mut source);
    h.select(1000);
    h.click(TAG_DESCENDANTS);
    assert_eq!(h.view().tags().tagged(), children as usize + 1);
    h.sample(&mut source);
    assert_eq!(h.view().tags().tagged(), 1);
    assert_eq!(h.view().tags().historical(), MAX_HISTORICAL_TAGS);
    // The oldest six were forgotten; the newest are named.
    let history: Vec<String> = h.view().tags().history().map(str::to_string).collect();
    assert_eq!(history.first().map(String::as_str), Some("PID 1007 · c7"));
    assert_eq!(history.last().map(String::as_str), Some("PID 1070 · c70"));
    assert_eq!(
        h.status(),
        "Tags: 1 tagged · 70 no longer listed: PID 1070 · c70, PID 1069 · c69, PID 1068 · c68 and 67 more (6 forgotten) · Tagged PID 1000 · root and 70 sampled descendants; processes started later are not added"
    );
    h.click(TAGS_CLEAR);
    assert_eq!(h.view().tags().historical(), 0);
    assert!(h
        .status()
        .ends_with("Cleared 1 tag and 70 historical; the selection is unchanged"));
}

#[test]
fn a_request_beyond_the_tag_limit_is_refused_whole() {
    // One more than may be tagged: refused, nothing tagged.
    let mut source = crowd(MAX_TAGGED as u32);
    let mut h = harness(&mut source);
    h.select(1000);
    h.click(TAG_DESCENDANTS);
    assert_eq!(h.view().tags().tagged(), 0);
    assert!(h.tagged().is_empty());
    assert!(
        h.status().ends_with(&format!(
            "Nothing tagged: PID 1000 · root and {MAX_TAGGED} sampled descendants would make {} tags, above the limit of {MAX_TAGGED}",
            MAX_TAGGED + 1
        )),
        "{}",
        h.status()
    );
    // Exactly the limit: tagged; then one more by Toggle is refused.
    let mut source = crowd(MAX_TAGGED as u32 - 1);
    let mut h = harness(&mut source);
    h.select(1000);
    h.click(TAG_DESCENDANTS);
    assert_eq!(h.view().tags().tagged(), MAX_TAGGED);
    h.select(1);
    h.click(TAG_TOGGLE);
    assert_eq!(h.view().tags().tagged(), MAX_TAGGED);
    assert!(!h.tagged().contains(&1));
    assert!(
        h.status().ends_with(&format!(
            "Nothing tagged: PID 1 · init not tagged, as {MAX_TAGGED} processes already are, the limit; untag or clear some first"
        )),
        "{}",
        h.status()
    );
    // Untagging is never refused.
    h.select(1000);
    h.click(TAG_TOGGLE);
    assert_eq!(h.view().tags().tagged(), MAX_TAGGED - 1);
}

#[test]
fn a_paused_view_tags_its_frozen_rows() {
    let mut later = family();
    later.push(record(14, "late", 10, 14));
    let mut source = Steps::new(vec![family(), later]);
    let mut h = harness(&mut source);
    h.with_view(|view, session| {
        view.pause_view(session).unwrap();
    });
    h.select(10);
    h.click(TAG_DESCENDANTS);
    assert_eq!(h.tagged(), set(&[10, 11, 12, 13]));
    // A sample reaching a paused view publishes nothing; resumed, `late`
    // appears untagged.
    h.sample(&mut source);
    h.with_view(|view, session| {
        view.resume_view(session).unwrap();
    });
    h.sample(&mut source);
    assert!(h.rows().iter().any(|row| row.1 == 14));
    assert_eq!(h.tagged(), set(&[10, 11, 12, 13]));
}

/// PID reuse never inherits a tag, also while the earlier instance is still
/// shown: a scan that cannot say what it missed keeps the ended `cc`, tagged,
/// beside the newcomer that reuses PID 11, which is not tagged. Once a
/// complete scan drops `cc`, its tag turns historical and the newcomer is
/// still untagged.
#[test]
fn a_newcomer_beside_a_kept_earlier_instance_of_its_pid_is_not_tagged() {
    let mut reused: Vec<ProcessRecord> = family()
        .into_iter()
        .filter(|record| record.key.pid != Observed::Known(11))
        .collect();
    reused.push(record(11, "cc-next", 1, 30));
    let mut source = Steps::new(vec![family(), reused.clone(), reused]);
    source.unenumerable.insert(1);
    let mut h = harness(&mut source);
    h.select(11);
    h.click(TAG_TOGGLE);
    let cc = h.item_of(11);
    h.sample(&mut source);
    let instances: Vec<_> = h.rows().into_iter().filter(|row| row.1 == 11).collect();
    assert_eq!(
        instances.len(),
        2,
        "both instances of PID 11 are shown: {instances:?}"
    );
    for (item, _, name, tag) in &instances {
        if *item == cc {
            assert_eq!((name.as_str(), tag.as_str()), ("cc", TAGGED_CELL));
        } else {
            assert_eq!((name.as_str(), tag.as_str()), ("cc-next", ""));
        }
    }
    assert!(
        h.status()
            .starts_with("Tags: 1 tagged · Tagged PID 11 · cc"),
        "{}",
        h.status()
    );
    h.sample(&mut source);
    assert!(h.tagged().is_empty());
    assert!(
        h.status()
            .starts_with("Tags: 0 tagged · 1 no longer listed: PID 11 · cc"),
        "{}",
        h.status()
    );
}
