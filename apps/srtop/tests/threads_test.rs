//! PX-019 acceptance: user threads and kernel-thread visibility (D1 §§6.2,
//! 6.3, 7.6, 8, 12, 22, 29; D2 T9–T11, T16, T21; H3, S1).
//!
//! * A thread is keyed by its process instance and its own TID and creation
//!   token, so a thread that ends and a newcomer under the same TID get
//!   different item IDs while every other row keeps its own.
//! * "Show user threads" (off by default) and "Show kernel threads" (on by
//!   default) are independent; the filters choose processes, and a process's
//!   threads are shown with it, after it in the table and under it in the
//!   tree.
//! * A process row is the whole process and a thread row that thread alone:
//!   nothing counts a thread as a process or adds the two, and a thread row
//!   repeats no memory figure.
//! * A source without per-thread records, and a process whose threads could
//!   not be read, say so; neither is ever zero threads.
//! * The thread bound and the row ceiling are tested at and past the bound.
//!
//! The procfs cases run on fixture trees on any platform; the live case, a
//! bounded multithreaded worker owned by this test, runs on Linux.

use srui_process_explorer::catalogue::ProcessField;
use srui_process_explorer::inspector::DetailRead;
use srui_process_explorer::procfs::ProcFsSource;
use srui_process_explorer::refresh::{lock_view, ProcessView, SharedView};
use srui_process_explorer::source::*;
use srui_process_explorer::summary::PROCESSES_TEXT;
use srui_process_explorer::{
    install_follow_handlers, install_inspector, install_thread_handlers, install_view_handlers,
    start_from_source, FILTER_STATUS, FOLLOW_STATUS, INSPECTOR_STATUS, KERNEL_THREADS_TOGGLE,
    MODEL, OWNER_STATUS, TABLE, THREADS_STATUS, TREE_MODEL, USER_THREADS_TOGGLE, VIEW_STATUS,
    VIEW_TOGGLE,
};
use srui_sdk::{ItemId, NodeId, PropertyRef, TypeRef, Value, TEXT, VALUE};
use srui_semantic_tree::{Event as DomainEvent, SemanticStore};
use srui_sessiond::{EventOutcome, Session};
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const CLIENT: &[u8] = b"threads-client-instance";

/// One published row as a client reads it: item, PID cell, name, resident
/// and CPU cells, and the parent cell and depth in the tree.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Shown {
    item: ItemId,
    pid: String,
    name: String,
    resident: String,
    cpu: String,
    parent: Option<String>,
    depth: Option<u64>,
}

struct Harness {
    session: Arc<Session>,
    view: SharedView,
    replica: SemanticStore,
    seq: u64,
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
    install_thread_handlers(&session, &view);
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

    /// Applies every committed transaction as a client does: each must be
    /// accepted.
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
    }

    fn send(&mut self, node: NodeId, event: TypeRef, arguments: Vec<(PropertyRef, Value)>) {
        self.seq += 1;
        let wire = DomainEvent::new(
            None,
            self.seq,
            format!("threads-event-{}", self.seq),
            self.session.current_revision(),
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
        self.sync();
    }

    /// The user clicks a checkbox.
    fn toggle(&mut self, node: NodeId, on: bool) {
        self.send(
            node,
            TypeRef::EVENT_VALUE_CHANGED,
            vec![(PropertyRef::VALUE, Value::Bool(on))],
        );
    }

    fn select(&mut self, node: NodeId, item: ItemId) {
        self.send(
            node,
            TypeRef::EVENT_SELECTION_CHANGED,
            vec![(PropertyRef::VALUE, Value::ItemId(item))],
        );
    }

    fn sample(&mut self, source: &mut impl ProcessSource) {
        self.view().refresh(&self.session, source).unwrap();
        self.sync();
    }

    fn text(&self, node: NodeId) -> String {
        match self
            .replica
            .get_node(node)
            .and_then(|node| node.get_property(TEXT))
        {
            Some(Value::String(text)) => text.clone(),
            other => panic!("no text on {node:?}: {other:?}"),
        }
    }

    fn value(&self, node: NodeId) -> Option<Value> {
        self.replica
            .get_node(node)
            .and_then(|node| node.get_property(VALUE))
            .cloned()
    }

    /// The rows of `model` as the replica holds them, in index order.
    fn rows_of(&self, model: srui_sdk::ModelId) -> Vec<Shown> {
        let model = self.replica.get_model(model).unwrap();
        (0..model.item_count)
            .map(|index| {
                let item = model.get_item_by_index(index).expect("every row is cached");
                let Value::List(cells) = &item.value else {
                    panic!("a row is a list of cells")
                };
                let text = |field: ProcessField| match &cells[field.index()] {
                    Value::String(text) => text.clone(),
                    Value::UnsignedInt(value) => value.to_string(),
                    other => format!("{other:?}"),
                };
                let tree = item.get_property(PropertyRef::TREE_DEPTH).is_some();
                Shown {
                    item: item.item_id,
                    pid: text(ProcessField::Pid),
                    name: text(ProcessField::Name),
                    resident: text(ProcessField::Resident),
                    cpu: text(ProcessField::Cpu),
                    parent: tree.then(|| match cells.last() {
                        Some(Value::String(parent)) => parent.clone(),
                        other => panic!("a parent cell: {other:?}"),
                    }),
                    depth: match item.get_property(PropertyRef::TREE_DEPTH) {
                        Some(Value::UnsignedInt(depth)) => Some(*depth),
                        _ => None,
                    },
                }
            })
            .collect()
    }

    fn table(&self) -> Vec<Shown> {
        self.rows_of(MODEL)
    }

    fn tree(&self) -> Vec<Shown> {
        self.rows_of(TREE_MODEL)
    }

    fn pids(&self) -> Vec<String> {
        self.table().into_iter().map(|row| row.pid).collect()
    }

    fn item(&self, pid: &str) -> ItemId {
        self.table()
            .into_iter()
            .chain(self.tree())
            .find(|row| row.pid == pid)
            .unwrap_or_else(|| panic!("{pid} is not shown"))
            .item
    }
}

fn s(text: &str) -> String {
    text.to_string()
}

const SERVER_THREADS: [&str; 3] = [
    "200 (thread of 200)",
    "201 (thread of 200)",
    "202 (thread of 200)",
];

#[test]
fn user_threads_start_hidden_and_are_counted_by_the_kernels_count_not_as_rows() {
    let mut source = ThreadsSource::default();
    let mut h = harness(&mut source);
    assert_eq!(h.pids(), ["1", "2", "3", "200", "300"]);
    assert_eq!(h.value(USER_THREADS_TOGGLE), Some(Value::Bool(false)));
    assert_eq!(h.value(KERNEL_THREADS_TOGGLE), Some(Value::Bool(true)));
    assert_eq!(
        h.text(THREADS_STATUS),
        "User threads hidden · 6 threads in 3 user processes by the kernel's count · \
         Kernel threads shown (2)"
    );
    // The source is not asked for threads while they are hidden.
    h.sample(&mut source);
    assert_eq!(h.pids(), ["1", "2", "3", "200", "300"]);
    assert!(!h.view().threads_requested());
}

#[test]
fn showing_user_threads_adds_each_thread_after_its_process_with_its_own_counters() {
    let mut source = ThreadsSource::default();
    let mut h = harness(&mut source);
    let server = h.item("200");
    h.toggle(USER_THREADS_TOGGLE, true);
    assert_eq!(h.value(USER_THREADS_TOGGLE), Some(Value::Bool(true)));
    assert!(h.view().threads_requested());
    assert!(
        h.text(THREADS_STATUS)
            .starts_with("User threads shown from the next sample"),
        "{}",
        h.text(THREADS_STATUS)
    );
    h.sample(&mut source);
    let rows = h.table();
    assert_eq!(
        rows.iter().map(|row| row.pid.as_str()).collect::<Vec<_>>(),
        [
            "1",
            "2",
            "3",
            "200",
            SERVER_THREADS[0],
            SERVER_THREADS[1],
            SERVER_THREADS[2],
            "300"
        ]
    );
    assert_eq!(rows[3].item, server, "the process row keeps its item ID");
    // Each thread's own CPU, and no memory figure repeated per thread.
    let threads = &rows[4..7];
    assert_eq!(
        threads
            .iter()
            .map(|row| (row.name.as_str(), row.cpu.as_str(), row.resident.as_str()))
            .collect::<Vec<_>>(),
        [
            ("server", "10.0%", "See process"),
            ("worker-1", "60.0%", "See process"),
            ("worker-2", "30.0%", "See process"),
        ]
    );
    assert_eq!(
        rows[3].cpu, "100.0%",
        "the process row is the whole process"
    );
    assert_eq!(rows[3].resident, "800.0 KiB");
    // Every row has its own item ID.
    let distinct: BTreeSet<u64> = rows.iter().map(|row| row.item.get()).collect();
    assert_eq!(distinct.len(), rows.len());
    assert_eq!(
        h.text(THREADS_STATUS),
        "User threads: 3 thread rows under 1 process · a thread row's CPU is that thread's \
         own, a process row's the whole process: never add both · threads of 1 process \
         unreadable (1 permission denied) · Kernel threads shown (2)"
    );
}

#[test]
fn processes_are_never_counted_twice_and_thread_rows_split_their_process() {
    let mut source = ThreadsSource::default();
    let mut h = harness(&mut source);
    let before = h.text(PROCESSES_TEXT);
    h.toggle(USER_THREADS_TOGGLE, true);
    h.sample(&mut source);
    // The summary counts the scan's process records, never a thread row.
    let after = h.text(PROCESSES_TEXT);
    assert!(after.contains(": 5 listed"), "{after}");
    assert_eq!(before, after);
    // The user filter counts processes.
    assert!(
        h.text(OWNER_STATUS).contains("showing all 5 processes"),
        "{}",
        h.text(OWNER_STATUS)
    );
    // The table counts rows, threads included, and says so.
    assert_eq!(h.text(FILTER_STATUS), "Filter: none · showing all 8 rows");
    // A process's thread rows split its CPU between them: the process row is
    // the sum, so adding the two would count it twice.
    for step in [0, ThreadsSource::CHURN_STEP] {
        let process = ThreadsSource::records(step)
            .into_iter()
            .find(|record| record.key.pid == Observed::Known(200))
            .unwrap();
        let CpuUsage::Measured(whole) = process.cpu else {
            panic!()
        };
        let threads: u64 = ThreadsSource::threads(step)
            .records
            .iter()
            .map(|thread| match thread.cpu {
                CpuUsage::Measured(own) => own.ticks,
                _ => panic!(),
            })
            .sum();
        assert_eq!(whole.ticks, threads, "step {step}");
        // Every thread record states no memory of its own.
        assert!(ThreadsSource::threads(step)
            .records
            .iter()
            .all(|thread| thread.resident == Observed::Missing(MissingReason::Unavailable)));
    }
}

#[test]
fn a_thread_that_ends_and_a_reused_tid_get_new_identities_and_nothing_else_moves() {
    let mut source = ThreadsSource::default();
    let mut h = harness(&mut source);
    h.toggle(USER_THREADS_TOGGLE, true);
    h.sample(&mut source);
    let before: HashMap<String, ItemId> = h
        .table()
        .into_iter()
        .map(|row| (row.pid, row.item))
        .collect();
    // The churn step: worker-1 (TID 201) ended, worker-3 (TID 203) started,
    // and TID 202 now names a new thread under a new creation token.
    assert_eq!(source.step(), ThreadsSource::CHURN_STEP);
    h.sample(&mut source);
    let rows = h.table();
    assert_eq!(
        rows.iter()
            .map(|row| (row.pid.as_str(), row.name.as_str()))
            .collect::<Vec<_>>(),
        [
            ("1", "init"),
            ("2", "kthreadd"),
            ("3", "kworker/0:1"),
            ("200", "server"),
            ("200 (thread of 200)", "server"),
            ("202 (thread of 200)", "worker-2b"),
            ("203 (thread of 200)", "worker-3"),
            ("300", "locked"),
        ]
    );
    let after: HashMap<String, ItemId> = rows.into_iter().map(|row| (row.pid, row.item)).collect();
    for kept in ["1", "2", "3", "200", "300", SERVER_THREADS[0]] {
        assert_eq!(before[kept], after[kept], "{kept} keeps its item ID");
    }
    assert_ne!(
        before[SERVER_THREADS[2]], after[SERVER_THREADS[2]],
        "a reused TID is a new thread"
    );
    assert!(
        !after.contains_key(SERVER_THREADS[1]),
        "an ended thread's row is gone"
    );
    let old: BTreeSet<ItemId> = before.values().copied().collect();
    assert!(
        !old.contains(&after["203 (thread of 200)"]),
        "a new thread gets a new ID"
    );
    assert!(!old.contains(&after["202 (thread of 200)"]));
}

#[test]
fn the_kernel_thread_switch_is_independent_of_the_user_thread_switch() {
    let mut source = ThreadsSource::default();
    let mut h = harness(&mut source);
    h.toggle(KERNEL_THREADS_TOGGLE, false);
    assert_eq!(h.value(KERNEL_THREADS_TOGGLE), Some(Value::Bool(false)));
    assert_eq!(h.pids(), ["1", "200", "300"]);
    assert!(h
        .text(THREADS_STATUS)
        .ends_with("Kernel threads hidden (2)"));
    // Nothing was sampled to hide them, and no process row was replaced.
    h.toggle(USER_THREADS_TOGGLE, true);
    h.sample(&mut source);
    assert_eq!(
        h.pids(),
        [
            "1",
            "200",
            SERVER_THREADS[0],
            SERVER_THREADS[1],
            SERVER_THREADS[2],
            "300"
        ]
    );
    h.toggle(KERNEL_THREADS_TOGGLE, true);
    assert_eq!(
        h.pids(),
        [
            "1",
            "2",
            "3",
            "200",
            SERVER_THREADS[0],
            SERVER_THREADS[1],
            SERVER_THREADS[2],
            "300"
        ]
    );
    // Hiding user threads leaves the kernel's alone and removes every thread
    // row at once; the next sample no longer reads them.
    h.toggle(USER_THREADS_TOGGLE, false);
    assert_eq!(h.pids(), ["1", "2", "3", "200", "300"]);
    assert!(!h.view().threads_requested());
    h.sample(&mut source);
    assert_eq!(h.pids(), ["1", "2", "3", "200", "300"]);
    // A request that changes nothing puts the checkbox back.
    h.send(KERNEL_THREADS_TOGGLE, TypeRef::EVENT_VALUE_CHANGED, vec![]);
    assert_eq!(h.value(KERNEL_THREADS_TOGGLE), Some(Value::Bool(true)));
}

#[test]
fn filters_choose_processes_and_threads_follow_them() {
    let mut source = ThreadsSource::default();
    let mut h = harness(&mut source);
    h.toggle(USER_THREADS_TOGGLE, true);
    h.sample(&mut source);
    // A thread's own name never shows it without its process.
    h.view().set_filter(&h.session, "worker-").unwrap();
    h.sync();
    assert!(h.pids().is_empty(), "{:?}", h.pids());
    // Its process's name shows the process and its threads.
    h.view().set_filter(&h.session, "server").unwrap();
    h.sync();
    assert_eq!(
        h.pids(),
        [
            "200",
            SERVER_THREADS[0],
            SERVER_THREADS[1],
            SERVER_THREADS[2]
        ]
    );
    assert!(
        h.text(FILTER_STATUS).contains("4 of 8 rows matched"),
        "{}",
        h.text(FILTER_STATUS)
    );
    assert!(
        h.text(OWNER_STATUS).contains("1 of 5 processes match"),
        "{}",
        h.text(OWNER_STATUS)
    );
    // The summary's match count is of processes.
    assert!(
        h.text(PROCESSES_TEXT).contains("1 matching srtop filter"),
        "{}",
        h.text(PROCESSES_TEXT)
    );
}

#[test]
fn sorting_orders_processes_and_each_processs_threads_among_themselves() {
    let mut source = ThreadsSource::default();
    let mut h = harness(&mut source);
    h.toggle(USER_THREADS_TOGGLE, true);
    h.sample(&mut source);
    let before: HashMap<String, ItemId> = h
        .table()
        .into_iter()
        .map(|row| (row.pid, row.item))
        .collect();
    h.view().click_sort(&h.session, ProcessField::Cpu).unwrap();
    h.sync();
    assert_eq!(
        h.pids(),
        [
            "200",
            SERVER_THREADS[1],
            SERVER_THREADS[2],
            SERVER_THREADS[0],
            "300",
            "1",
            "3",
            "2"
        ]
    );
    let after: HashMap<String, ItemId> = h
        .table()
        .into_iter()
        .map(|row| (row.pid, row.item))
        .collect();
    assert_eq!(before, after, "a sort moves rows and keeps every item ID");
    // The next sample keeps them grouped in that order.
    h.sample(&mut source);
    assert_eq!(
        h.pids(),
        [
            "200",
            "203 (thread of 200)",
            "202 (thread of 200)",
            SERVER_THREADS[0],
            "300",
            "1",
            "3",
            "2"
        ]
    );
}

#[test]
fn the_tree_places_each_thread_under_its_process_as_its_first_children() {
    let mut source = ThreadsSource::default();
    let mut h = harness(&mut source);
    h.toggle(USER_THREADS_TOGGLE, true);
    h.sample(&mut source);
    let ids: HashMap<String, ItemId> = h
        .table()
        .into_iter()
        .map(|row| (row.pid, row.item))
        .collect();
    h.toggle(VIEW_TOGGLE, true);
    let tree = h.tree();
    assert_eq!(
        tree.iter()
            .map(|row| (
                row.pid.as_str(),
                row.depth.unwrap(),
                row.parent.clone().unwrap()
            ))
            .collect::<Vec<_>>(),
        [
            ("1", 0, s("none: started by the kernel")),
            ("200", 1, s("1")),
            (SERVER_THREADS[0], 2, s("thread of 200")),
            (SERVER_THREADS[1], 2, s("thread of 200")),
            (SERVER_THREADS[2], 2, s("thread of 200")),
            ("300", 1, s("1")),
            ("2", 0, s("none: started by the kernel")),
            ("3", 1, s("2")),
        ]
    );
    // The same item IDs as in the table.
    for row in &tree {
        assert_eq!(ids[&row.pid], row.item, "{}", row.pid);
    }
    assert!(
        h.text(VIEW_STATUS)
            .starts_with("View: process tree · 5 processes and 3 threads under 2 roots"),
        "{}",
        h.text(VIEW_STATUS)
    );
    // Thread churn in the tree is accepted by a replica, revision by revision.
    h.sample(&mut source);
    assert_eq!(
        h.tree()
            .iter()
            .map(|row| row.pid.as_str())
            .collect::<Vec<_>>(),
        [
            "1",
            "200",
            SERVER_THREADS[0],
            "202 (thread of 200)",
            "203 (thread of 200)",
            "300",
            "2",
            "3"
        ]
    );
}

#[test]
fn the_inspector_and_follow_name_a_thread_and_see_it_end() {
    let mut source = ThreadsSource::default();
    let mut h = harness(&mut source);
    h.toggle(USER_THREADS_TOGGLE, true);
    h.sample(&mut source);
    let worker = h.item(SERVER_THREADS[1]);
    h.select(TABLE, worker);
    assert!(
        h.text(INSPECTOR_STATUS)
            .starts_with("Selected thread: TID 201 of PID 200 · worker-1"),
        "{}",
        h.text(INSPECTOR_STATUS)
    );
    h.toggle(srui_process_explorer::FOLLOW_TOGGLE, true);
    assert!(
        h.text(FOLLOW_STATUS)
            .starts_with("Following TID 201 of PID 200 · worker-1"),
        "{}",
        h.text(FOLLOW_STATUS)
    );
    // The thread ends: the inspector says so, and never shows TID 202's
    // newcomer or any other row.
    h.sample(&mut source);
    assert!(
        h.text(INSPECTOR_STATUS).contains("this thread has ended"),
        "{}",
        h.text(INSPECTOR_STATUS)
    );
    // Go to PID selects the process, never one of its threads.
    {
        let mut view = h.view();
        view.set_pid_input(&h.session, "200").unwrap();
        view.go_to_pid(&h.session).unwrap();
    }
    h.sync();
    let process = h.item("200");
    assert_eq!(h.view().search().target(), Some(process));
}

#[test]
fn hiding_user_threads_clears_an_inspector_that_described_one() {
    let mut source = ThreadsSource::default();
    let mut h = harness(&mut source);
    h.toggle(USER_THREADS_TOGGLE, true);
    h.sample(&mut source);
    h.select(TABLE, h.item(SERVER_THREADS[2]));
    assert!(h
        .text(INSPECTOR_STATUS)
        .starts_with("Selected thread: TID 202"));
    h.toggle(USER_THREADS_TOGGLE, false);
    assert_eq!(
        h.text(INSPECTOR_STATUS),
        srui_process_explorer::inspector::NO_SELECTION_STATUS
    );
}

#[test]
fn a_source_without_per_thread_records_says_so_and_never_shows_zero_threads() {
    let mut source = FakeProcessSource;
    let mut h = harness(&mut source);
    assert_eq!(
        h.text(THREADS_STATUS),
        "User threads hidden · this source states no thread counts · Kernel threads: this \
         source does not say which processes are kernel threads"
    );
    h.toggle(USER_THREADS_TOGGLE, true);
    h.sample(&mut source);
    assert_eq!(h.pids().len(), 3);
    assert!(
        h.text(THREADS_STATUS).starts_with(
            "User threads unavailable: this source reads no per-thread records, so rows show \
             whole processes only"
        ),
        "{}",
        h.text(THREADS_STATUS)
    );
    assert!(!h.text(THREADS_STATUS).contains("0 thread"));
}

#[test]
fn the_view_reads_no_thread_while_paused_and_says_when_it_will() {
    let mut source = ThreadsSource::default();
    let mut h = harness(&mut source);
    h.view().pause_view(&h.session).unwrap();
    h.sync();
    h.toggle(USER_THREADS_TOGGLE, true);
    assert_eq!(
        h.text(THREADS_STATUS),
        "User threads shown from the next sample · the view is paused: Resume view reads them \
         · Kernel threads shown (2)"
    );
    assert_eq!(h.pids(), ["1", "2", "3", "200", "300"]);
}

/// A fixed source whose threads name a process it did not list, or whose
/// process record names a thread.
struct Malformed(fn() -> ProcessSnapshot);

impl ProcessSource for Malformed {
    fn status_text(&self) -> &str {
        "Read-only · Malformed thread fixture"
    }

    fn snapshot(&mut self) -> ProcessSnapshot {
        (self.0)()
    }
}

fn threads_snapshot(records: Vec<ProcessRecord>, threads: ThreadRecords) -> ProcessSnapshot {
    ProcessSnapshot {
        source: SourceId(ThreadsSource::SOURCE.into()),
        sampled_at: SnapshotTime(SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000)),
        records,
        vanished: 0,
        capped: CappedRecords::none(),
        completeness: Completeness::Complete,
        system: FakeProcessSource::system(),
        threads: ThreadScan::Read(threads),
    }
}

#[test]
fn a_thread_without_its_process_or_a_process_naming_a_thread_is_refused_whole() {
    let orphan = || {
        let records = ThreadsSource::records(0)
            .into_iter()
            .filter(|record| record.key.pid != Observed::Known(200))
            .collect();
        threads_snapshot(records, ThreadsSource::threads(0))
    };
    let posing = || {
        let mut records = ThreadsSource::records(0);
        records[0].kind = Observed::Known(TaskKind::Thread);
        threads_snapshot(records, ThreadsSource::threads(0))
    };
    let unnamed = || {
        let mut threads = ThreadsSource::threads(0);
        threads.records[0].key.thread = None;
        threads_snapshot(ThreadsSource::records(0), threads)
    };
    for (case, make) in [
        ("orphan", orphan as fn() -> ProcessSnapshot),
        ("posing", posing),
        ("unnamed", unnamed),
    ] {
        let mut source = ThreadsSource::default();
        let mut h = harness(&mut source);
        let before = h.table();
        h.sample(&mut Malformed(make));
        assert_eq!(
            h.table(),
            before,
            "{case}: nothing is published from a refused scan"
        );
        let status = h.text(srui_process_explorer::STATUS);
        assert!(status.contains("snapshot rejected"), "{case}: {status}");
    }
}

#[test]
fn the_row_ceiling_keeps_a_process_before_its_threads_at_and_past_the_bound() {
    for (limit, shown) in [
        (
            8usize,
            vec![
                "1",
                "2",
                "3",
                "200",
                SERVER_THREADS[0],
                SERVER_THREADS[1],
                SERVER_THREADS[2],
                "300",
            ],
        ),
        (
            7,
            vec![
                "1",
                "2",
                "3",
                "200",
                SERVER_THREADS[0],
                SERVER_THREADS[1],
                SERVER_THREADS[2],
            ],
        ),
        (5, vec!["1", "2", "3", "200", SERVER_THREADS[0]]),
        (4, vec!["1", "2", "3", "200"]),
    ] {
        let mut source = ThreadsSource::default();
        let mut h = harness_with(&mut source, |view| view.with_published_limit(limit));
        h.toggle(USER_THREADS_TOGGLE, true);
        h.sample(&mut source);
        assert_eq!(h.pids(), shown, "limit {limit}");
        let filter = h.text(FILTER_STATUS);
        if limit < 8 {
            assert_eq!(
                filter,
                format!(
                    "Filter: none · showing {limit} of 8 rows; {} beyond the publishable size limit",
                    8 - limit
                ),
            );
        } else {
            assert_eq!(filter, "Filter: none · showing all 8 rows");
        }
        // The tree keeps a preorder prefix: no thread without its process.
        h.toggle(VIEW_TOGGLE, true);
        let tree = h.tree();
        assert_eq!(tree.len(), limit.min(8), "limit {limit}");
        for row in tree.iter().filter(|row| row.pid.contains("thread")) {
            assert_eq!(row.parent.as_deref(), Some("thread of 200"));
        }
    }
}

// ---- procfs fixture trees -------------------------------------------------

/// `PF_KTHREAD`, transcribed from include/linux/sched.h.
const PF_KTHREAD: u64 = 0x0020_0000;

struct Tree(PathBuf);

impl Tree {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = PathBuf::from(format!(
            "/tmp/srtop-threads-{}-{nonce}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        fs::create_dir_all(path.join("sys/kernel/random")).unwrap();
        fs::write(path.join("sys/kernel/hostname"), "fixture-host\n").unwrap();
        fs::write(path.join("sys/kernel/random/boot_id"), "fixture-boot\n").unwrap();
        fs::create_dir_all(path.join("self/ns")).unwrap();
        std::os::unix::fs::symlink("pid:[4026531836]", path.join("self/ns/pid")).unwrap();
        // AT_PAGESZ 4096, AT_CLKTCK 100, AT_NULL.
        let mut auxv = Vec::new();
        for word in [6u64, 4096, 17, 100, 0, 0] {
            auxv.extend_from_slice(&(word as usize).to_ne_bytes());
        }
        fs::write(path.join("self/auxv"), auxv).unwrap();
        Self(path)
    }

    /// A process record: `stat`, `statm` and `status`.
    #[allow(clippy::too_many_arguments)]
    fn process(
        &self,
        pid: u32,
        comm: &str,
        flags: u64,
        threads: u32,
        start: u64,
        cpu: u64,
    ) -> &Self {
        let dir = self.0.join(pid.to_string());
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("stat"),
            stat_line(pid, comm, flags, cpu, threads, start),
        )
        .unwrap();
        fs::write(dir.join("statm"), "10 4 2 1 0 3 0\n").unwrap();
        fs::write(
            dir.join("status"),
            "Name:\tx\nUid:\t1000\t1000\t1000\t1000\n",
        )
        .unwrap();
        self
    }

    /// One thread of `pid`: its `task/<tid>/stat`.
    fn thread(&self, pid: u32, tid: u32, comm: &str, start: u64, cpu: u64) -> &Self {
        let dir = self.0.join(format!("{pid}/task/{tid}"));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("stat"), stat_line(tid, comm, 0, cpu, 1, start)).unwrap();
        self
    }

    fn remove_thread(&self, pid: u32, tid: u32) -> &Self {
        fs::remove_dir_all(self.0.join(format!("{pid}/task/{tid}"))).unwrap();
        self
    }

    fn deny_threads(&self, pid: u32) -> &Self {
        let task = self.0.join(format!("{pid}/task"));
        fs::create_dir_all(&task).unwrap();
        fs::set_permissions(&task, fs::Permissions::from_mode(0o000)).unwrap();
        self
    }

    fn allow_threads(&self, pid: u32) -> &Self {
        let task = self.0.join(format!("{pid}/task"));
        fs::set_permissions(&task, fs::Permissions::from_mode(0o755)).unwrap();
        self
    }

    /// The usual tree: init with its one thread, a kernel thread, `server`
    /// with three threads, a process with no `task` directory and one whose
    /// `task` directory is refused.
    fn usual() -> Self {
        let tree = Self::new();
        tree.process(1, "init", 0, 1, 10, 5)
            .thread(1, 1, "init", 10, 5)
            .process(2, "kthreadd", PF_KTHREAD, 1, 20, 0)
            .process(100, "server", 0, 3, 100, 90)
            .thread(100, 100, "server", 100, 10)
            .thread(100, 101, "worker-1", 110, 50)
            .thread(100, 102, "worker-2", 120, 30)
            .process(200, "bare", 0, 1, 200, 0)
            .process(300, "locked", 0, 2, 300, 0)
            .deny_threads(300);
        tree
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        if let Ok(entries) = fs::read_dir(&self.0) {
            for entry in entries.flatten() {
                let _ = fs::set_permissions(
                    entry.path().join("task"),
                    fs::Permissions::from_mode(0o755),
                );
            }
        }
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A `stat` line with `flags` (field 9), `cpu` user ticks (field 14),
/// `threads` (field 20) and `start` (field 22).
fn stat_line(pid: u32, comm: &str, flags: u64, cpu: u64, threads: u32, start: u64) -> String {
    let mut fields: Vec<String> = (4..=21).map(|field| (field * 10).to_string()).collect();
    let mut set = |field: usize, value: String| fields[field - 4] = value;
    set(4, "1".into());
    set(9, flags.to_string());
    set(14, cpu.to_string());
    set(15, "0".into());
    set(20, threads.to_string());
    format!("{pid} ({comm}) S {} {start} 4096 9\n", fields.join(" "))
}

fn tids(records: &[ProcessRecord]) -> Vec<(u32, u32)> {
    records
        .iter()
        .map(|record| {
            let Observed::Known(pid) = record.key.pid else {
                panic!()
            };
            (pid, record.key.thread.as_ref().unwrap().tid)
        })
        .collect()
}

#[test]
fn procfs_reads_threads_only_when_asked_and_names_what_it_could_not_read() {
    let tree = Tree::usual();
    let mut source = ProcFsSource::with_root(&tree.0);
    // Not asked: nothing under `task` is read.
    let quiet = source.snapshot();
    assert_eq!(quiet.threads, ThreadScan::NotRequested);
    assert_eq!(quiet.records.len(), 5);
    let kinds: Vec<_> = quiet
        .records
        .iter()
        .map(|record| record.kind.clone())
        .collect();
    assert_eq!(
        kinds,
        [
            Observed::Known(TaskKind::Process {
                threads: Observed::Known(1)
            }),
            Observed::Known(TaskKind::KernelThread),
            Observed::Known(TaskKind::Process {
                threads: Observed::Known(3)
            }),
            Observed::Known(TaskKind::Process {
                threads: Observed::Known(1)
            }),
            Observed::Known(TaskKind::Process {
                threads: Observed::Known(2)
            }),
        ]
    );
    source.collect_threads(true);
    let first = source.snapshot();
    let read = first.threads.read().expect("threads were read").clone();
    assert_eq!(
        tids(&read.records),
        [(1, 1), (100, 100), (100, 101), (100, 102)]
    );
    // A process's own records are untouched: never a thread among them, and
    // the scan's process count is the same.
    assert_eq!(first.records.len(), 5);
    assert!(first
        .records
        .iter()
        .all(|record| record.key.thread.is_none()));
    // `bare` has no `task` directory and `locked` refuses it; the kernel
    // thread's is never listed.
    assert_eq!(read.unreadable.count(), 2);
    assert_eq!(read.unreadable.pids(), &BTreeSet::from([200, 300]));
    assert_eq!(read.denied, 1);
    assert_eq!(read.capped.count(), 0);
    let worker = &read.records[2];
    let server = first
        .records
        .iter()
        .find(|r| r.key.pid == Observed::Known(100))
        .unwrap();
    assert_eq!(
        worker.key,
        ProcessKey {
            thread: Some(ThreadIdentity {
                tid: 101,
                creation: CreationToken::LinuxBootTicks(110)
            }),
            ..server.key.clone()
        }
    );
    assert!(worker.key.is_thread_of(&server.key));
    assert_eq!(worker.display_name.as_str(), "worker-1");
    assert_eq!(worker.kind, Observed::Known(TaskKind::Thread));
    assert_eq!(worker.owner, Observed::Known(1000));
    assert_eq!(
        worker.resident,
        Observed::Missing(MissingReason::Unavailable)
    );
    assert_eq!(
        worker.cpu,
        CpuUsage::WarmingUp,
        "the first sample of a thread"
    );
    // Their earlier thread rows are kept, not deleted.
    let retention = first.retention();
    assert!(retention
        .uncertain_pids()
        .unwrap()
        .is_superset(&BTreeSet::from([200, 300])));
    // The next scan measures each thread on its own counters; TID 102 now
    // names a new thread, which warms up rather than inheriting its counter.
    tree.thread(100, 101, "worker-1", 110, 80)
        .remove_thread(100, 102)
        .thread(100, 102, "worker-2b", 150, 200);
    let second = source.snapshot();
    let read = second.threads.read().unwrap();
    assert!(matches!(read.records[2].cpu, CpuUsage::Measured(interval) if interval.ticks == 30));
    assert_eq!(read.records[3].cpu, CpuUsage::WarmingUp);
    assert_ne!(
        read.records[3].key,
        first.threads.records()[3].key,
        "a reused TID is a new thread instance"
    );
    assert_eq!(source.thread_baselines(), 4);
    // Hidden again: no thread is read and no thread baseline is kept.
    source.collect_threads(false);
    assert_eq!(source.snapshot().threads, ThreadScan::NotRequested);
    assert_eq!(source.thread_baselines(), 0);
}

#[test]
fn the_thread_bound_names_the_processes_it_left_unread_at_and_past_the_bound() {
    let tree = Tree::usual();
    tree.allow_threads(300);
    fs::remove_dir_all(tree.0.join("300/task")).unwrap();
    tree.thread(300, 300, "locked", 300, 0)
        .thread(300, 301, "locked-2", 301, 0);
    // Six threads in all: 1, 100, 101, 102, 300, 301. The root is read in
    // directory order, so which processes the bound leaves unread depends on
    // it; what is read and what is named never do.
    let all: HashMap<u32, usize> = HashMap::from([(1, 1), (100, 3), (300, 2)]);
    for limit in [7usize, 6, 5, 4, 2, 1, 0] {
        let mut source = ProcFsSource::with_root(&tree.0).with_thread_limit(limit);
        source.collect_threads(true);
        let snapshot = source.snapshot();
        let threads = snapshot.threads.read().unwrap();
        assert_eq!(threads.records.len(), limit.min(6), "limit {limit}");
        let mut read: HashMap<u32, usize> = HashMap::new();
        for (pid, _) in tids(&threads.records) {
            *read.entry(pid).or_default() += 1;
        }
        // Exactly the processes with a thread left unread are named.
        let short: BTreeSet<u32> = all
            .iter()
            .filter(|(pid, count)| read.get(pid).copied().unwrap_or(0) < **count)
            .map(|(pid, _)| *pid)
            .collect();
        assert_eq!(threads.capped.pids(), &short, "limit {limit}");
        assert_eq!(limit >= 6, short.is_empty(), "limit {limit}");
        // A capped process's thread rows are kept, never deleted.
        for pid in &short {
            assert!(snapshot.retention().uncertain_pids().unwrap().contains(pid));
        }
        // `bare` still has no `task` directory, and is never capped.
        assert_eq!(
            threads.unreadable.pids(),
            &BTreeSet::from([200]),
            "limit {limit}"
        );
    }
}

#[test]
fn thread_rows_of_a_process_whose_threads_became_unreadable_are_kept() {
    let tree = Tree::usual();
    let mut source = ProcFsSource::with_root(&tree.0);
    let mut h = harness(&mut source);
    h.toggle(USER_THREADS_TOGGLE, true);
    h.sample(&mut source);
    let before = h.table();
    assert_eq!(
        before
            .iter()
            .map(|row| row.pid.as_str())
            .collect::<Vec<_>>(),
        [
            "1",
            "1 (thread of 1)",
            "2",
            "100",
            "100 (thread of 100)",
            "101 (thread of 100)",
            "102 (thread of 100)",
            "200",
            "300"
        ]
    );
    tree.deny_threads(100);
    h.sample(&mut source);
    let ids = |rows: Vec<Shown>| -> Vec<(String, ItemId)> {
        rows.into_iter().map(|row| (row.pid, row.item)).collect()
    };
    assert_eq!(
        ids(h.table()),
        ids(before),
        "no thread is claimed to have ended"
    );
    assert!(
        h.text(THREADS_STATUS)
            .contains("threads of 3 processes unreadable (2 permission denied)"),
        "{}",
        h.text(THREADS_STATUS)
    );
    // Readable again, and one thread has ended: only its row goes.
    tree.allow_threads(100).remove_thread(100, 101);
    h.sample(&mut source);
    assert_eq!(
        h.pids(),
        [
            "1",
            "1 (thread of 1)",
            "2",
            "100",
            "100 (thread of 100)",
            "102 (thread of 100)",
            "200",
            "300"
        ]
    );
}

#[test]
fn the_detail_reader_reads_a_thread_and_reports_a_reused_tid_as_ended() {
    let tree = Tree::usual();
    let mut source = ProcFsSource::with_root(&tree.0);
    source.collect_threads(true);
    let snapshot = source.snapshot();
    let worker = snapshot.threads.records()[2].key.clone();
    let details = source.details().unwrap();
    let DetailRead::Read(read) = details.read_details(&worker) else {
        panic!("the thread is readable")
    };
    assert_eq!(
        read.state.clone(),
        Observed::Known(srui_process_explorer::inspector::ProcessState::from_code(b'S').unwrap())
    );
    tree.remove_thread(100, 101)
        .thread(100, 101, "newcomer", 999, 0);
    assert_eq!(details.read_details(&worker), DetailRead::Ended);
    tree.remove_thread(100, 101);
    assert_eq!(details.read_details(&worker), DetailRead::Ended);
}

// ---- live Linux ------------------------------------------------------------

#[cfg(target_os = "linux")]
mod live {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Child, ChildStdin, Command, Stdio};
    use std::sync::mpsc;
    use std::time::Instant;

    /// `<test pid>:<threads>`: set only for a worker this test spawned.
    const WORKER_ENV: &str = "SRTOP_THREAD_WORKER";
    const WORKER_TEST: &str = "live::thread_worker";
    const READY: &str = "srtop thread worker ready:";
    const REPLACED: &str = "srtop thread worker replaced:";
    /// How many threads the worker starts besides libtest's own.
    const THREADS: usize = 4;
    const LIFETIME: Duration = Duration::from_secs(60);
    const TIMEOUT: Duration = Duration::from_secs(30);

    /// A bounded, disposable worker owned by this test: this test binary,
    /// re-executed to run [`thread_worker`] alone, with [`THREADS`] named
    /// threads. One line on its stdin ends one thread and starts another.
    struct Worker {
        child: Child,
        stdin: ChildStdin,
        lines: mpsc::Receiver<String>,
    }

    impl Worker {
        fn spawn() -> (Self, Vec<u32>) {
            let program = std::env::current_exe().unwrap();
            let mut child = Command::new(program)
                .args([
                    "--exact",
                    WORKER_TEST,
                    "--ignored",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env(WORKER_ENV, format!("{}:{THREADS}", std::process::id()))
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
                .expect("the test owns this worker");
            let stdin = child.stdin.take().unwrap();
            let stdout = child.stdout.take().unwrap();
            let (sender, lines) = mpsc::channel();
            std::thread::spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    let _ = sender.send(line);
                }
            });
            let mut worker = Self {
                child,
                stdin,
                lines,
            };
            let tids = worker.wait_for(READY);
            (worker, tids)
        }

        fn pid(&self) -> u32 {
            self.child.id()
        }

        /// The numbers after `marker` on the line that carries it.
        fn wait_for(&mut self, marker: &str) -> Vec<u32> {
            let deadline = Instant::now() + TIMEOUT;
            loop {
                let line = self
                    .lines
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .unwrap_or_else(|error| panic!("the worker never said {marker:?}: {error:?}"));
                if let Some(at) = line.find(marker) {
                    return line[at + marker.len()..]
                        .split(|c: char| !c.is_ascii_digit())
                        .filter(|part| !part.is_empty())
                        .map(|part| part.parse().unwrap())
                        .collect();
                }
            }
        }

        /// Ends one worker thread and starts another: (ended TID, new TID).
        fn replace(&mut self) -> (u32, u32) {
            writeln!(self.stdin, "replace").unwrap();
            self.stdin.flush().unwrap();
            let tids = self.wait_for(REPLACED);
            (tids[0], tids[1])
        }
    }

    impl Drop for Worker {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    fn own_tid() -> u32 {
        let link = fs::read_link("/proc/thread-self").expect("Linux names this thread");
        link.file_name().unwrap().to_str().unwrap().parse().unwrap()
    }

    /// The body of a [`Worker`]; a no-op unless this test's own parent set
    /// [`WORKER_ENV`]. It ends after [`LIFETIME`], when its stdin closes, or
    /// when its parent is gone.
    #[test]
    #[ignore = "the body of a test-owned worker process, which Worker::spawn runs"]
    fn thread_worker() {
        let request = std::env::var(WORKER_ENV).unwrap_or_default();
        let Some((parent, threads)) = request.split_once(':') else {
            return;
        };
        if parent.parse::<u32>().ok() != Some(std::os::unix::process::parent_id()) {
            return;
        }
        let threads: usize = threads.parse().unwrap();
        let start = |name: String| {
            let (stop, stopped) = mpsc::channel::<()>();
            let (told, tid) = mpsc::channel::<u32>();
            let handle = std::thread::Builder::new()
                .name(name)
                .spawn(move || {
                    told.send(own_tid()).unwrap();
                    let _ = stopped.recv_timeout(LIFETIME);
                })
                .unwrap();
            (tid.recv().unwrap(), stop, handle)
        };
        let mut running: Vec<_> = (0..threads)
            .map(|i| start(format!("srtop-thr-{i}")))
            .collect();
        let list = running
            .iter()
            .map(|(tid, ..)| tid.to_string())
            .collect::<Vec<_>>()
            .join(",");
        println!("{READY} {list}");
        let started = Instant::now();
        let mut lines = std::io::stdin().lock().lines();
        while started.elapsed() < LIFETIME {
            match lines.next() {
                Some(Ok(line)) if line == "replace" => {
                    let (ended, stop, handle) = running.remove(0);
                    stop.send(()).unwrap();
                    handle.join().unwrap();
                    let fresh = start("srtop-thr-new".to_string());
                    println!("{REPLACED} {ended} {}", fresh.0);
                    running.push(fresh);
                }
                Some(Ok(_)) => {}
                _ => break,
            }
        }
        for (_, stop, handle) in running {
            let _ = stop.send(());
            let _ = handle.join();
        }
    }

    /// The worker's rows, as (PID cell, name, item), in published order.
    fn worker_rows(h: &Harness, pid: u32) -> Vec<(String, String, ItemId)> {
        let process = pid.to_string();
        let suffix = format!("(thread of {pid})");
        h.table()
            .into_iter()
            .filter(|row| row.pid == process || row.pid.ends_with(&suffix))
            .map(|row| (row.pid, row.name, row.item))
            .collect()
    }

    /// The kernel's own thread count for `pid`, from `stat` field 20.
    fn kernel_count(pid: u32) -> usize {
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
        let after = &stat[stat.rfind(')').unwrap() + 2..];
        after.split(' ').nth(17).unwrap().parse().unwrap()
    }

    #[test]
    fn a_live_multithreaded_worker_shows_its_threads_and_a_replaced_thread_is_new() {
        let (mut worker, workers) = Worker::spawn();
        let pid = worker.pid();
        assert_eq!(workers.len(), THREADS);
        let mut source = ProcFsSource::live();
        let mut h = harness(&mut source);
        h.toggle(USER_THREADS_TOGGLE, true);
        h.sample(&mut source);
        let rows = worker_rows(&h, pid);
        let count = kernel_count(pid);
        // The process row, then one row per thread the kernel counts: the
        // main thread (TID = PID), libtest's test thread and the workers.
        assert_eq!(rows[0].0, pid.to_string(), "{rows:?}");
        assert_eq!(rows.len(), 1 + count, "{rows:?}");
        assert!(count > THREADS, "{count}");
        assert_eq!(
            rows[1].0,
            format!("{pid} (thread of {pid})"),
            "the main thread first"
        );
        for (index, tid) in workers.iter().enumerate() {
            let row = rows
                .iter()
                .find(|row| row.0 == format!("{tid} (thread of {pid})"))
                .unwrap_or_else(|| panic!("TID {tid} is shown: {rows:?}"));
            assert_eq!(row.1, format!("srtop-thr-{index}"));
        }
        // No double count: the summary counts processes, the scan's own.
        let processes = h.text(PROCESSES_TEXT);
        let listed = h
            .table()
            .iter()
            .filter(|row| !row.pid.contains("thread"))
            .count();
        assert!(
            processes.contains(&format!(": {listed} listed")),
            "{processes} vs {listed}"
        );
        // One thread ends and another starts.
        let before: HashMap<String, ItemId> =
            rows.iter().map(|row| (row.0.clone(), row.2)).collect();
        let (ended, fresh) = worker.replace();
        h.sample(&mut source);
        let after: HashMap<String, ItemId> = worker_rows(&h, pid)
            .into_iter()
            .map(|row| (row.0, row.2))
            .collect();
        let ended_cell = format!("{ended} (thread of {pid})");
        let fresh_cell = format!("{fresh} (thread of {pid})");
        assert!(
            !after.contains_key(&ended_cell),
            "the ended thread's row is gone"
        );
        let new_id = after[&fresh_cell];
        assert!(
            !before.values().any(|id| *id == new_id),
            "the new thread has a new item ID"
        );
        for (cell, id) in &before {
            if *cell != ended_cell && *cell != fresh_cell {
                assert_eq!(after.get(cell), Some(id), "{cell} keeps its item ID");
            }
        }
        println!(
            "PX-019 live: pid={pid} kernel_count={count} rows={} ended={ended} fresh={fresh} status={}",
            rows.len(),
            h.text(THREADS_STATUS)
        );
    }
}

#[test]
fn a_thread_count_the_kernel_did_not_write_is_unread_never_zero() {
    use srui_process_explorer::procfs::parse_stat;
    let line = |threads: &str| {
        format!("5 (x) S 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 {threads} 18 200 1 2\n")
    };
    assert_eq!(
        parse_stat(5, line("7").as_bytes()).unwrap().threads,
        Observed::Known(7)
    );
    for bad in ["-1", "x", "4294967296"] {
        assert_eq!(
            parse_stat(5, line(bad).as_bytes()).unwrap().threads,
            Observed::Missing(MissingReason::Unavailable),
            "{bad}"
        );
    }
}
