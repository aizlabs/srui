//! PX-031 live: what srtop observes of a process after the kernel accepted
//! SIGTERM for it, on this host's real `/proc` and real PID file
//! descriptors, through the real event path, the real gate, the production
//! terminator and the view's own scans. Only workers this test spawns are
//! signalled, each a `sh` whose `comm` is confirmed first, each in a guard
//! that kills and reaps it on drop and each ending by itself after about
//! 60 s.
//!
//! * A worker that exits by itself, and is reaped, between Prepare and
//!   Confirm cannot be held: the request is refused, nothing is sent and
//!   nothing is watched.
//! * A worker that records SIGTERM and keeps running: the request is
//!   requested, and at the deadline the observed line says still running;
//!   it has received exactly one SIGTERM. Another worker exiting during the
//!   watch is not the target's exit.
//! * A worker that exits on SIGTERM and is reaped: exited after the
//!   request, never "terminated by srtop".
//! * A worker that exits on SIGTERM and is *not* reaped stays a zombie
//!   (state `Z`), which the scan still lists: still running at the deadline,
//!   with the zombie caveat, never exited.
//!
//! One test, in sequence, so `/proc/self/fd` accounts for every handle.
#![cfg(target_os = "linux")]

use srui_process_explorer::actions::{
    lock_actions, ActionGate, ActionMode, ActionOutcome, ObservedOutcome, ProcessActions,
    SharedActions, OBSERVED_IDLE,
};
use srui_process_explorer::catalogue::ProcessField;
use srui_process_explorer::procfs::ProcFsSource;
use srui_process_explorer::refresh::{lock_view, SharedView};
use srui_process_explorer::terminate::PidfdTerminator;
use srui_process_explorer::{
    install_actions, install_inspector, start_from_source, ACTIONS_OBSERVED, ACTIONS_STATUS,
    ACTION_PREPARE, MODEL, TABLE,
};
use srui_sdk::{ItemId, NodeId, PropertyRef, TypeRef, Value, ACTIVATE, COLUMNS, TEXT};
use srui_semantic_tree::{Event as DomainEvent, SemanticStore};
use srui_sessiond::{EventOutcome, Session};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const CLIENT: &[u8] = b"px031-live-client";
const WINDOW: Duration = Duration::from_secs(2);

/// Exits with status 0 on SIGTERM; ends by itself with 3 after about 60 s.
const EXITS_ON_TERM: &str =
    "trap 'exit 0' TERM; i=0; while [ $i -lt 600 ]; do sleep 0.1; i=$((i+1)); done; exit 3";
/// Records each SIGTERM in `$0` and keeps running; ends by itself.
const COUNTS_TERM: &str = "trap 'echo term >> \"$0\"' TERM; i=0; while [ $i -lt 600 ]; do sleep 0.1; i=$((i+1)); done; exit 3";
/// Exits with status 0 by itself once `$0` exists; ends by itself.
const EXITS_WHEN_TOLD: &str =
    "i=0; while [ ! -e \"$0\" ] && [ $i -lt 600 ]; do sleep 0.05; i=$((i+1)); done; exit 0";

/// One worker this test started: killed and reaped on drop if still
/// running.
struct Worker(Child);

impl Drop for Worker {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

impl Worker {
    fn start(script: &str, argument: Option<&Path>) -> Self {
        let mut command = Command::new("sh");
        command.arg("-c").arg(script);
        if let Some(argument) = argument {
            command.arg(argument);
        }
        let worker = Self(
            command
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("sh starts"),
        );
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(comm(worker.pid()), "sh", "the owned worker");
        worker
    }

    fn pid(&self) -> u32 {
        self.0.id()
    }

    fn running(&mut self) -> bool {
        self.0.try_wait().unwrap().is_none()
    }

    /// Reaps the worker once it exited, waiting at most `within`.
    fn reaped_within(&mut self, within: Duration) -> Option<std::process::ExitStatus> {
        let deadline = Instant::now() + within;
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                return Some(status);
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

fn comm(pid: u32) -> String {
    std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// The state code `/proc/<pid>/stat` reports, read without reaping.
fn state(pid: u32) -> Option<char> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat[stat.rfind(')')? + 2..].chars().next()
}

fn open_descriptors() -> usize {
    std::fs::read_dir("/proc/self/fd").unwrap().count()
}

fn lines(path: &Path) -> usize {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .count()
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
        let gate: SharedActions = Arc::new(Mutex::new(
            ActionGate::new(ActionMode::Terminate, terminator as Arc<dyn ProcessActions>)
                .with_observation_window(WINDOW),
        ));
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
            format!("px031-live-{}", self.seq),
            self.session.current_revision(),
            node,
            event,
            arguments,
        )
        .with_client_instance_id(CLIENT.to_vec())
        .to_wire()
    }

    fn processed(&mut self, node: NodeId) {
        let wire = self.wire(node, ACTIVATE, Vec::new());
        let outcome = self.session.process_event(&wire).unwrap();
        self.sync();
        assert!(
            matches!(outcome, EventOutcome::Processed { .. }),
            "{outcome:?}"
        );
    }

    fn select(&mut self, pid: u32) {
        // Sampled until the worker is the latest scan's confirmed row.
        self.sample();
        self.sample();
        let item = self.item_of(pid);
        let wire = self.wire(
            TABLE,
            TypeRef::EVENT_SELECTION_CHANGED,
            vec![(PropertyRef::VALUE, Value::ItemId(item))],
        );
        assert!(matches!(
            self.session.process_event(&wire).unwrap(),
            EventOutcome::Processed { .. }
        ));
        self.sync();
    }

    fn prepare(&mut self, pid: u32) -> NodeId {
        self.select(pid);
        self.processed(ACTION_PREPARE);
        let status = self.text(ACTIONS_STATUS);
        assert!(status.starts_with("Prepared #"), "{status}");
        lock_actions(&self.gate)
            .pending_panel()
            .expect("prepared")
            .confirm
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

    fn text(&self, node: NodeId) -> String {
        match self
            .replica
            .get_node(node)
            .and_then(|node| node.get_property(TEXT))
        {
            Some(Value::String(text)) => text.clone(),
            other => panic!("{other:?}"),
        }
    }

    fn outcome(&self) -> Option<ActionOutcome> {
        lock_actions(&self.gate).outcome().cloned()
    }

    fn observed(&self) -> Option<ObservedOutcome> {
        lock_actions(&self.gate).observed().cloned()
    }

    /// Samples every 100 ms, with the refresh loop's deadline timer, until
    /// the watch settles; bounded by twice the window.
    fn until_settled(&mut self, during: impl Fn()) -> ObservedOutcome {
        let deadline = Instant::now() + WINDOW * 2 + Duration::from_secs(2);
        loop {
            during();
            self.sample();
            lock_view(&self.view).observe_actions(&self.session);
            self.sync();
            match self.observed() {
                Some(ObservedOutcome::Watching) => {}
                Some(settled) => return settled,
                None => panic!("nothing is watched"),
            }
            assert!(Instant::now() < deadline, "the deadline never settled");
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn live_outcomes_are_observed_by_scans_and_never_assumed() {
    let terminator = Arc::new(PidfdTerminator::live());
    assert_eq!(terminator.unavailable(), None, "PID file descriptors");
    let before = open_descriptors();
    let directory =
        TempDir(std::env::temp_dir().join(format!("srtop-px031-live-{}", std::process::id())));
    std::fs::create_dir_all(&directory.0).unwrap();
    let mut h = Harness::start(Arc::clone(&terminator));

    // N exits by itself between Prepare and Confirm, and is reaped.
    let go = directory.0.join("n-go");
    let mut n = Worker::start(EXITS_WHEN_TOLD, Some(&go));
    let confirm = h.prepare(n.pid());
    std::fs::write(&go, b"").unwrap();
    let exit = n.reaped_within(Duration::from_secs(10)).expect("N exited");
    assert_eq!(exit.code(), Some(0), "N ended by itself");
    h.processed(confirm);
    assert!(
        matches!(h.outcome(), Some(ActionOutcome::Refused { .. })),
        "{}",
        h.text(ACTIONS_STATUS)
    );
    assert!(h.text(ACTIONS_STATUS).ends_with("nothing was requested"));
    assert_eq!(
        h.observed(),
        None,
        "nothing was sent, so nothing is watched"
    );
    assert_eq!(h.text(ACTIONS_OBSERVED), OBSERVED_IDLE);

    // I records SIGTERM and keeps running; U exits by itself meanwhile.
    let terms = directory.0.join("i-terms");
    let mut i = Worker::start(COUNTS_TERM, Some(&terms));
    let u_go = directory.0.join("u-go");
    let mut u = Worker::start(EXITS_WHEN_TOLD, Some(&u_go));
    let confirm = h.prepare(i.pid());
    h.processed(confirm);
    assert!(
        matches!(
            h.outcome(),
            Some(ActionOutcome::Requested { dry_run: false, .. })
        ),
        "{}",
        h.text(ACTIONS_STATUS)
    );
    std::fs::write(&u_go, b"").unwrap();
    u.reaped_within(Duration::from_secs(10)).expect("U exited");
    let settled = h.until_settled(|| {});
    assert_eq!(settled, ObservedOutcome::StillRunning);
    let line = h.text(ACTIONS_OBSERVED);
    assert!(
        line.starts_with(&format!(
            "Still running #2 (terminate PID {} · sh)",
            i.pid()
        )),
        "{line}"
    );
    assert!(!line.contains(&u.pid().to_string()), "{line}");
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(lines(&terms), 1, "exactly one SIGTERM; nothing escalated");
    assert!(i.running(), "I handled SIGTERM and runs");

    // C exits on SIGTERM and is reaped.
    let mut c = Worker::start(EXITS_ON_TERM, None);
    let confirm = h.prepare(c.pid());
    h.processed(confirm);
    let exit = c.reaped_within(Duration::from_secs(10)).expect("C exited");
    assert_eq!(exit.code(), Some(0), "C's own handler ended it");
    let settled = h.until_settled(|| {});
    assert!(
        matches!(settled, ObservedOutcome::ExitedAfterRequest { within } if within < WINDOW),
        "{settled:?}"
    );
    let line = h.text(ACTIONS_OBSERVED);
    assert!(
        line.starts_with(&format!(
            "Exited after the request #3 (terminate PID {} · sh)",
            c.pid()
        )) && line.ends_with("whether SIGTERM or something else ended it is not known"),
        "{line}"
    );

    // Z exits on SIGTERM and is not reaped during the window: a zombie.
    let mut z = Worker::start(EXITS_ON_TERM, None);
    let zombie = z.pid();
    let confirm = h.prepare(zombie);
    h.processed(confirm);
    let deadline = Instant::now() + Duration::from_secs(10);
    while state(zombie) != Some('Z') {
        assert!(Instant::now() < deadline, "Z became a zombie");
        std::thread::sleep(Duration::from_millis(20));
    }
    let settled = h.until_settled(|| {
        assert_eq!(state(zombie), Some('Z'), "still an unreaped zombie");
    });
    assert_eq!(
        settled,
        ObservedOutcome::StillRunning,
        "a listed zombie is never reported exited"
    );
    let line = h.text(ACTIONS_OBSERVED);
    assert!(
        line.starts_with(&format!("Still running #4 (terminate PID {zombie} · sh)"))
            && line.contains("a zombie stays listed"),
        "{line}"
    );
    let exit = z.reaped_within(Duration::from_secs(5)).expect("Z reaped");
    assert_eq!(exit.code(), Some(0));

    assert_eq!(lines(&terms), 1, "I received nothing more");
    assert_eq!(terminator.live_handles(), 0);
    assert_eq!(open_descriptors(), before, "no descriptor leaked");
    assert_eq!(comm(i.pid()), "sh");
    drop(i);
    println!(
        "PX-031 live: kernel {}; N exited by itself before Confirm: refused, not watched; I (counts TERM) one SIGTERM, still running at the {} s deadline; C exited after the request; Z a zombie, still listed; descriptors {before} before and after",
        std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .unwrap_or_default()
            .trim(),
        WINDOW.as_secs()
    );
}
