//! PX-030 live: confirmed graceful termination of one process on this host's
//! real `/proc` and real PID file descriptors (`pidfd_open`,
//! `pidfd_send_signal`), through the real event path, the real gate and the
//! production terminator and support chain. Only workers this test spawns are
//! ever signalled: each is a `sh` whose `comm` is confirmed from
//! `/proc/<pid>/comm` before it is selected, and each is reaped here. The
//! support-chain and PID 1 checks are exercised on this test process, its
//! parent and PID 1, which are refused before anything is opened.
//!
//! One test, in sequence, so the descriptor count of `/proc/self/fd` accounts
//! for every handle this binary opens.
#![cfg(target_os = "linux")]

use srui_process_explorer::actions::{
    lock_actions, ActionGate, ActionMode, ActionOutcome, ProcessActions, Refusal, SharedActions,
    SupportRole,
};
use srui_process_explorer::catalogue::ProcessField;
use srui_process_explorer::procfs::ProcFsSource;
use srui_process_explorer::refresh::{lock_view, SharedView};
use srui_process_explorer::terminate::PidfdTerminator;
use srui_process_explorer::{
    install_actions, install_inspector, start_from_source, ACTIONS_STATUS, ACTION_CANCEL,
    ACTION_PREPARE, MODEL, TABLE,
};
use srui_sdk::{ItemId, NodeId, PropertyRef, TypeRef, Value, ACTIVATE, COLUMNS, TEXT};
use srui_semantic_tree::{Event as DomainEvent, SemanticStore};
use srui_sessiond::{EventOutcome, Session};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const CLIENT: &[u8] = b"px030-live-client";

/// A worker that exits with status 0 when it receives SIGTERM. Each worker
/// ends by itself with status 3 after about 60 s (600 sleeps of 0.1 s), a
/// backstop should this test die without reaping it.
const EXITS_ON_TERM: &str =
    "trap 'exit 0' TERM; i=0; while [ $i -lt 600 ]; do sleep 0.1; i=$((i+1)); done; exit 3";
/// A worker that records each SIGTERM in `$0` and keeps running.
const COUNTS_TERM: &str = "trap 'echo term >> \"$0\"' TERM; i=0; while [ $i -lt 600 ]; do sleep 0.1; i=$((i+1)); done; exit 3";

/// One worker this test started: killed and reaped on drop if it is still
/// running (PX-030 verification round 1, F2), so a failed assertion leaves
/// no `sh` behind. It is this test's own unreaped child, so its PID cannot
/// name any other process.
struct Worker(Child);

impl std::ops::Deref for Worker {
    type Target = Child;
    fn deref(&self) -> &Child {
        &self.0
    }
}

impl std::ops::DerefMut for Worker {
    fn deref_mut(&mut self) -> &mut Child {
        &mut self.0
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

fn worker(script: &str, argument: Option<&Path>) -> Worker {
    let mut command = Command::new("sh");
    command.arg("-c").arg(script);
    if let Some(argument) = argument {
        command.arg(argument);
    }
    let child = Worker(
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("sh starts"),
    );
    // Let the trap be installed before anything is sent.
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        comm(child.id()),
        "sh",
        "the owned worker is the sh this test started"
    );
    child
}

fn comm(pid: u32) -> String {
    std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn parent_of(pid: u32) -> u32 {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    let after = &stat[stat.rfind(')').unwrap() + 2..];
    after.split(' ').nth(1).unwrap().parse().unwrap()
}

fn open_descriptors() -> usize {
    std::fs::read_dir("/proc/self/fd").unwrap().count()
}

/// Waits for an owned worker to exit by itself, at most `within`.
fn exited_within(child: &mut Child, within: Duration) -> Option<std::process::ExitStatus> {
    let deadline = Instant::now() + within;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return Some(status);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

struct Harness {
    session: Arc<Session>,
    view: SharedView,
    gate: SharedActions,
    source: ProcFsSource,
    replica: SemanticStore,
    seq: u64,
}

impl Harness {
    fn start(terminator: Arc<PidfdTerminator>) -> Self {
        let session = Arc::new(Session::mint());
        let mut source = ProcFsSource::live();
        let (view, _) = start_from_source(&session, &mut source).unwrap();
        let view: SharedView = Arc::new(Mutex::new(view));
        install_inspector(&session, &view, None);
        let gate: SharedActions = Arc::new(Mutex::new(ActionGate::new(
            ActionMode::Terminate,
            terminator as Arc<dyn ProcessActions>,
        )));
        install_actions(&session, &view, &gate).unwrap();
        let mut harness = Self {
            session,
            view,
            gate,
            source,
            replica: SemanticStore::new(),
            seq: 0,
        };
        harness.sync();
        harness
    }

    fn sync(&mut self) {
        for transaction in self
            .session
            .collect_replayed_transactions(self.replica.revision().get())
            .unwrap()
        {
            self.replica.apply_wire_transaction(transaction).unwrap();
        }
    }

    fn sample(&mut self) {
        lock_view(&self.view)
            .refresh(&self.session, &mut self.source)
            .unwrap();
        self.sync();
    }

    fn wire(
        &mut self,
        node: NodeId,
        event: TypeRef,
        arguments: Vec<(PropertyRef, Value)>,
    ) -> srui_protocol::Event {
        self.seq += 1;
        DomainEvent::new(
            None,
            self.seq,
            format!("px030-live-{}", self.seq),
            self.session.current_revision(),
            node,
            event,
            arguments,
        )
        .with_client_instance_id(CLIENT.to_vec())
        .to_wire()
    }

    fn deliver(&mut self, wire: &srui_protocol::Event) -> EventOutcome {
        let outcome = self.session.process_event(wire).unwrap();
        self.sync();
        outcome
    }

    fn processed(&mut self, node: NodeId) {
        let wire = self.wire(node, ACTIVATE, Vec::new());
        let outcome = self.deliver(&wire);
        assert!(
            matches!(outcome, EventOutcome::Processed { .. }),
            "{outcome:?}"
        );
    }

    fn select(&mut self, pid: u32) {
        let item = self.item_of(pid);
        let wire = self.wire(
            TABLE,
            TypeRef::EVENT_SELECTION_CHANGED,
            vec![(PropertyRef::VALUE, Value::ItemId(item))],
        );
        assert!(matches!(
            self.deliver(&wire),
            EventOutcome::Processed { .. }
        ));
    }

    fn item_of(&self, pid: u32) -> ItemId {
        let headings = match self
            .replica
            .get_node(TABLE)
            .and_then(|node| node.get_property(COLUMNS))
        {
            Some(Value::List(headings)) => headings.clone(),
            other => panic!("headings: {other:?}"),
        };
        let at = headings
            .iter()
            .position(|heading| heading == &Value::String(ProcessField::Pid.heading().into()))
            .unwrap();
        let model = self.replica.get_model(MODEL).unwrap();
        (0..model.item_count)
            .map(|index| model.get_item_by_index(index).unwrap())
            .find(|item| match &item.value {
                Value::List(cells) => match &cells[at] {
                    Value::UnsignedInt(shown) => *shown == u64::from(pid),
                    Value::String(shown) => shown == &pid.to_string(),
                    _ => false,
                },
                _ => false,
            })
            .unwrap_or_else(|| panic!("PID {pid} is listed"))
            .item_id
    }

    fn status(&self) -> String {
        match self
            .replica
            .get_node(ACTIONS_STATUS)
            .and_then(|node| node.get_property(TEXT))
        {
            Some(Value::String(text)) => text.clone(),
            other => panic!("{other:?}"),
        }
    }

    fn outcome(&self) -> Option<ActionOutcome> {
        lock_actions(&self.gate).outcome().cloned()
    }

    fn confirm(&self) -> NodeId {
        lock_actions(&self.gate)
            .pending_panel()
            .expect("prepared")
            .confirm
    }
}

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn live_terminate_reaches_exactly_the_confirmed_owned_worker() {
    let terminator = Arc::new(PidfdTerminator::live());
    assert_eq!(
        terminator.unavailable(),
        None,
        "PID file descriptors are available on this kernel"
    );
    let before = open_descriptors();
    let directory =
        TempDir(std::env::temp_dir().join(format!("srtop-px030-live-{}", std::process::id())));
    std::fs::create_dir_all(&directory.0).unwrap();
    let terms = directory.0.join("terms");

    // Two owned workers: A exits on SIGTERM, B records each SIGTERM.
    let mut a = worker(EXITS_ON_TERM, None);
    let mut b = worker(COUNTS_TERM, Some(&terms));
    let mut h = Harness::start(Arc::clone(&terminator));

    // The support chain and PID 1, refused before any handle: this test
    // process is the server here, and its parent is its ancestor.
    let me = std::process::id();
    h.select(me);
    h.processed(ACTION_PREPARE);
    assert_eq!(
        h.outcome(),
        Some(ActionOutcome::Refused {
            intent: None,
            reason: Refusal::SupportChain {
                pid: me,
                role: SupportRole::ThisServer
            }
        }),
        "{}",
        h.status()
    );
    let parent = parent_of(me);
    if owned_by_me(parent) && parent != 1 {
        h.select(parent);
        h.processed(ACTION_PREPARE);
        assert_eq!(
            h.outcome(),
            Some(ActionOutcome::Refused {
                intent: None,
                reason: Refusal::SupportChain {
                    pid: parent,
                    role: SupportRole::ServerAncestor
                }
            }),
            "{}",
            h.status()
        );
    }
    // PID 1: protected when this account owns it (a container run as this
    // user), and not this account's otherwise.
    h.select(1);
    h.processed(ACTION_PREPARE);
    let expected = if owned_by_me(1) {
        Refusal::Protected(1)
    } else {
        match h.outcome() {
            Some(ActionOutcome::Refused {
                reason: reason @ Refusal::NotOwned { .. },
                ..
            }) => reason,
            other => panic!("PID 1: {other:?}"),
        }
    };
    assert_eq!(
        h.outcome(),
        Some(ActionOutcome::Refused {
            intent: None,
            reason: expected
        })
    );
    let pid1 = h.status();

    // Prepare A, select B, confirm: A's intent is withdrawn; nothing is sent.
    h.select(a.id());
    h.processed(ACTION_PREPARE);
    let a_confirm = h.confirm();
    let held = h.wire(a_confirm, ACTIVATE, Vec::new());
    h.select(b.id());
    assert!(matches!(h.deliver(&held), EventOutcome::Rejected { .. }));
    // Cancel: nothing is sent.
    h.processed(ACTION_PREPARE);
    h.processed(ACTION_CANCEL);
    std::thread::sleep(Duration::from_millis(300));
    assert!(a.try_wait().unwrap().is_none(), "A received nothing");
    assert!(b.try_wait().unwrap().is_none(), "B received nothing");
    assert!(!terms.exists(), "B recorded no SIGTERM");
    assert_eq!(open_descriptors(), before, "no handle was opened");

    // B confirmed: one SIGTERM, which B handles and survives; the same
    // event redelivered sends nothing new.
    h.processed(ACTION_PREPARE);
    let b_confirm = h.confirm();
    let confirm = h.wire(b_confirm, ACTIVATE, Vec::new());
    assert!(matches!(
        h.deliver(&confirm),
        EventOutcome::Processed { .. }
    ));
    assert!(
        matches!(
            h.outcome(),
            Some(ActionOutcome::Requested { dry_run: false, .. })
        ),
        "{}",
        h.status()
    );
    assert!(matches!(
        h.deliver(&confirm),
        EventOutcome::Duplicate { accepted: true, .. }
    ));
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(
        std::fs::read_to_string(&terms)
            .unwrap_or_default()
            .lines()
            .count(),
        1,
        "exactly one SIGTERM reached B"
    );
    assert!(
        b.try_wait().unwrap().is_none(),
        "B handles SIGTERM and keeps running"
    );
    assert!(a.try_wait().unwrap().is_none(), "A received nothing");
    assert_eq!(
        open_descriptors(),
        before,
        "the handle was closed after its signal"
    );

    // A, selected and confirmed by name: it handles SIGTERM and exits 0.
    h.select(a.id());
    h.processed(ACTION_PREPARE);
    let status = h.status();
    assert!(status.starts_with("Prepared #"), "{status}");
    let a_confirm = h.confirm();
    h.processed(a_confirm);
    let exit = exited_within(&mut a, Duration::from_secs(10)).expect("A exited after SIGTERM");
    assert_eq!(
        exit.code(),
        Some(0),
        "A's own SIGTERM handler ended it: {exit:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&terms)
            .unwrap_or_default()
            .lines()
            .count(),
        1,
        "B received nothing more"
    );
    assert_eq!(terminator.live_handles(), 0);
    assert_eq!(open_descriptors(), before, "no descriptor leaked");

    // The next scan sees A gone; the status still says requested, never
    // that the action completed (observing that is PX-031's).
    h.sample();
    let after = h.status();
    assert!(!after.contains("complete"), "{after}");

    // B is the test's own: end it here.
    assert_eq!(comm(b.id()), "sh");
    drop(b);
    println!(
        "PX-030 live: kernel {}; A (sh, exits on TERM) terminated through the gate with exit 0; B (sh, counts TERM) received exactly one SIGTERM across a duplicate delivery; this process (PID {me}) and its parent (PID {parent}) refused as the support chain; PID 1: {pid1}; descriptors {before} before and after",
        std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .unwrap_or_default()
            .trim()
    );
}

fn owned_by_me(pid: u32) -> bool {
    use std::os::unix::fs::MetadataExt;
    let mine = std::fs::metadata("/proc/self").unwrap().uid();
    std::fs::metadata(format!("/proc/{pid}")).is_ok_and(|meta| meta.uid() == mine)
}
