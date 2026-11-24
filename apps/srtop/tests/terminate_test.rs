//! PX-030 acceptance through the real event path, with the real terminator
//! (`PidfdTerminator`) over a deterministic fake of the kernel's PID file
//! descriptors and `/proc` (D1 §§6.2, 7.7, 18.2, 27; D2 T24, T32; K2).
//!
//! Every click is a wire event the session admits, validates and dispatches;
//! every commit is applied by a client replica. The fake kernel numbers each
//! process instance, binds a descriptor to the instance that held the PID at
//! the open, delivers a signal only to that instance while it lives, and
//! counts every open and close, so "nothing was sent" and "no handle leaked"
//! are observed, not inferred:
//!
//! * a confirmed Terminate sends SIGTERM to exactly the confirmed instance,
//!   once; redelivery of the same event sends nothing new (§18.2);
//! * prepare A, select B, confirm: A's intent is withdrawn and its Confirm
//!   names no node; nothing is opened or sent until B is prepared and
//!   confirmed itself;
//! * Cancel and an expired intent open nothing and send nothing;
//! * a PID that another process took after the scan is refused after the
//!   open, and its newcomer receives nothing; one that exited is refused;
//! * the support chain and configured PIDs are refused before any handle;
//! * where PID file descriptors are unavailable, Terminate… is published
//!   disabled with the reason, and nothing is ever attempted by PID;
//! * hundreds of prepare/cancel/withdraw/confirm cycles leave the live node
//!   count, the handler table and the descriptor count unchanged;
//! * a handle is settled only for the key it was acquired for.

use srui_process_explorer::actions::{
    lock_actions, ActionGate, ActionMode, ActionOutcome, Execution, PanelIds, ProcessActions,
    Refusal, SharedActions, SupportChain, SupportRole, TERMINATE_LABEL, TERMINATE_STATUS,
};
use srui_process_explorer::catalogue::ProcessField;
use srui_process_explorer::handle::{HandleSignal, Pidfd, PidfdSystem, ProcessHandles};
use srui_process_explorer::on_demand::{MonotonicClock, SourceTime};
use srui_process_explorer::owner::Accounts;
use srui_process_explorer::refresh::{lock_view, SharedView};
use srui_process_explorer::source::*;
use srui_process_explorer::terminate::{signal_held, PidfdTerminator};
use srui_process_explorer::{
    install_actions, install_inspector, install_sampling_handlers, start_from_source,
    ACTIONS_STATUS, ACTION_CANCEL, ACTION_PREPARE, MODEL, TABLE,
};
use srui_sdk::{
    ItemId, NodeId, PropertyRef, TypeRef, Value, ACTIVATE, COLUMNS, ENABLED, LABEL, TEXT,
};
use srui_semantic_tree::{Event as DomainEvent, EventValidationError, SemanticStore};
use srui_sessiond::{EventOutcome, Session};
use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

const CLIENT: &[u8] = b"px030-client";
const SOURCE: &str = "px030-fixture-v1";
const BOOT: &str = "px030-boot";
const NAMESPACE: u64 = 7;
const ME: u32 = 1000;
const SOMEONE: u32 = 1001;
const ESRCH: i32 = 3;
const SIGTERM: i32 = 15;

fn key(pid: u32, start: u64) -> ProcessKey {
    ProcessKey {
        source: SourceId(SOURCE.into()),
        host: Observed::Known(HostId("px030-host".into())),
        boot: Observed::Known(BootId(BOOT.into())),
        pid_namespace: Observed::Known(PidNamespaceId(NAMESPACE)),
        pid: Observed::Known(pid),
        creation: CreationToken::LinuxBootTicks(start),
        thread: None,
    }
}

fn record(pid: u32, start: u64, name: &str, owner: u32, ticks: u64) -> ProcessRecord {
    ProcessRecord {
        kind: KIND_NOT_COLLECTED,
        lineage: Lineage::new(1, 100),
        owner: Observed::Known(owner),
        key: key(pid, start),
        display_name: DisplayName::from(name),
        resident: Observed::Known(4096),
        cpu: CpuUsage::Measured(CpuInterval {
            ticks,
            ticks_per_second: 100,
            elapsed: Duration::from_secs(1),
        }),
    }
}

/// `init` (1), `worker` A (4101, mine), `other` (4102, someone else's) and
/// `spare` B (4103, mine).
fn host(ticks: u64) -> Vec<ProcessRecord> {
    vec![
        record(1, 1, "init", ME, 1),
        record(4101, 500, "worker", ME, ticks),
        record(4102, 600, "other", SOMEONE, 2),
        record(4103, 700, "spare", ME, 3),
    ]
}

struct Steps {
    steps: Vec<Vec<ProcessRecord>>,
    next: usize,
}

impl ProcessSource for Steps {
    fn status_text(&self) -> &str {
        "Read-only · PX-030 fixture"
    }

    fn accounts(&self) -> Accounts {
        Accounts::fixed(ME, &[(ME, "alice"), (SOMEONE, "bob")])
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
            completeness: Completeness::Complete,
            system: SystemSample::not_provided(),
        }
    }
}

#[derive(Debug, Default)]
struct ManualClock(AtomicU64);

impl ManualClock {
    fn advance(&self, by: Duration) {
        self.0.fetch_add(by.as_nanos() as u64, Ordering::SeqCst);
    }
}

impl MonotonicClock for ManualClock {
    fn now(&self) -> SourceTime {
        SourceTime(Duration::from_nanos(self.0.load(Ordering::SeqCst)))
    }
}

/// The fake kernel's state: the live instance under each PID (its start
/// time), every signal delivered, and every descriptor opened and closed.
#[derive(Debug, Default)]
struct State {
    processes: HashMap<u32, u64>,
    delivered: Vec<(u32, u64, i32)>,
    opens: usize,
    open_now: usize,
    /// Answer every open with `ENOSYS` (a kernel before 5.3).
    no_system_call: bool,
}

#[derive(Debug, Clone)]
struct Kernel {
    state: Arc<Mutex<State>>,
    available: bool,
}

impl Kernel {
    fn new(available: bool) -> Self {
        let mut state = State::default();
        for record in host(0) {
            let (Observed::Known(pid), CreationToken::LinuxBootTicks(start)) =
                (record.key.pid.clone(), record.key.creation.clone())
            else {
                unreachable!()
            };
            state.processes.insert(pid, start);
        }
        Self {
            state: Arc::new(Mutex::new(state)),
            available,
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap()
    }

    fn delivered(&self) -> Vec<(u32, u64, i32)> {
        self.state().delivered.clone()
    }

    fn opens(&self) -> usize {
        self.state().opens
    }

    fn open_now(&self) -> usize {
        self.state().open_now
    }
}

#[derive(Debug)]
struct FakePidfd {
    state: Arc<Mutex<State>>,
    pid: u32,
    start: u64,
}

impl Pidfd for FakePidfd {
    fn send(&self, signal: HandleSignal) -> io::Result<()> {
        let mut state = self.state.lock().unwrap();
        if state.processes.get(&self.pid) != Some(&self.start) {
            return Err(io::Error::from_raw_os_error(ESRCH));
        }
        state
            .delivered
            .push((self.pid, self.start, signal.number()));
        Ok(())
    }
}

impl Drop for FakePidfd {
    fn drop(&mut self) {
        self.state.lock().unwrap().open_now -= 1;
    }
}

impl PidfdSystem for Kernel {
    fn available(&self) -> bool {
        self.available
    }

    fn open(&self, pid: u32) -> io::Result<Box<dyn Pidfd>> {
        let mut state = self.state();
        if state.no_system_call {
            return Err(io::ErrorKind::Unsupported.into());
        }
        let Some(start) = state.processes.get(&pid).copied() else {
            return Err(io::Error::from_raw_os_error(ESRCH));
        };
        state.opens += 1;
        state.open_now += 1;
        Ok(Box::new(FakePidfd {
            state: Arc::clone(&self.state),
            pid,
            start,
        }))
    }

    fn read_stat(&self, pid: u32) -> io::Result<Vec<u8>> {
        let state = self.state();
        let Some(start) = state.processes.get(&pid) else {
            return Err(io::ErrorKind::NotFound.into());
        };
        Ok(
            format!("{pid} (w) S 1 1 1 0 -1 0 0 0 0 0 1 1 0 0 20 0 1 0 {start} 4096 10\n")
                .into_bytes(),
        )
    }

    fn boot_id(&self) -> io::Result<String> {
        Ok(BOOT.into())
    }

    fn pid_namespace(&self) -> io::Result<u64> {
        Ok(NAMESPACE)
    }
}

fn handles(kernel: &Kernel, clock: &Arc<ManualClock>) -> ProcessHandles {
    ProcessHandles::new(
        Arc::new(kernel.clone()),
        Arc::clone(clock) as Arc<dyn MonotonicClock>,
        SourceId(SOURCE.into()),
    )
}

struct Harness {
    session: Arc<Session>,
    view: SharedView,
    gate: SharedActions,
    kernel: Kernel,
    terminator: Arc<PidfdTerminator>,
    clock: Arc<ManualClock>,
    replica: SemanticStore,
    seq: u64,
    slots: u32,
}

fn harness_with(
    source: &mut Steps,
    kernel: Kernel,
    configure: impl FnOnce(ActionGate) -> ActionGate,
) -> Harness {
    let session = Arc::new(Session::mint());
    let (view, _) = start_from_source(&session, source).unwrap();
    let view: SharedView = Arc::new(Mutex::new(view));
    install_inspector(&session, &view, None);
    install_sampling_handlers(&session, &view);
    let clock = Arc::new(ManualClock::default());
    let terminator = Arc::new(PidfdTerminator::with_handles(handles(&kernel, &clock)));
    let gate = configure(
        ActionGate::new(
            ActionMode::Terminate,
            Arc::clone(&terminator) as Arc<dyn ProcessActions>,
        )
        .with_clock(Arc::clone(&clock) as Arc<dyn MonotonicClock>),
    );
    let slots = gate.panel_slots();
    let gate: SharedActions = Arc::new(Mutex::new(gate));
    install_actions(&session, &view, &gate).unwrap();
    let mut harness = Harness {
        session,
        view,
        gate,
        kernel,
        terminator,
        clock,
        replica: SemanticStore::new(),
        seq: 0,
        slots,
    };
    harness.sync();
    harness
}

fn harness(source: &mut Steps) -> Harness {
    harness_with(source, Kernel::new(true), |gate| gate)
}

fn steps(steps: Vec<Vec<ProcessRecord>>) -> Steps {
    Steps { steps, next: 0 }
}

impl Harness {
    fn revision(&self) -> u64 {
        self.session.current_revision()
    }

    fn sync(&mut self) {
        for transaction in self
            .session
            .collect_replayed_transactions(self.replica.revision().get())
            .expect("the journal holds this run")
        {
            self.replica
                .apply_wire_transaction(transaction)
                .expect("a client applies every commit");
        }
    }

    fn deliver(&mut self, wire: &srui_protocol::Event) -> EventOutcome {
        let outcome = self
            .session
            .process_event(wire)
            .expect("the event is admitted");
        self.sync();
        outcome
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
            format!("px030-event-{}", self.seq),
            self.revision(),
            node,
            event,
            arguments,
        )
        .with_client_instance_id(CLIENT.to_vec())
        .to_wire()
    }

    fn click(&mut self, node: NodeId) -> EventOutcome {
        let wire = self.wire(node, ACTIVATE, Vec::new());
        self.deliver(&wire)
    }

    fn processed(&mut self, node: NodeId) {
        let outcome = self.click(node);
        assert!(
            matches!(outcome, EventOutcome::Processed { .. }),
            "{node:?}: {outcome:?}"
        );
    }

    fn select(&mut self, pid: u64) {
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

    fn sample(&mut self, source: &mut Steps) {
        lock_view(&self.view)
            .refresh(&self.session, source)
            .unwrap();
        self.sync();
    }

    fn panel(&self) -> Option<PanelIds> {
        lock_actions(&self.gate).pending_panel()
    }

    fn property(&self, node: NodeId, property: PropertyRef) -> Option<Value> {
        self.replica
            .get_node(node)
            .and_then(|node| node.get_property(property))
            .cloned()
    }

    fn text(&self, node: NodeId, property: PropertyRef) -> String {
        match self.property(node, property) {
            Some(Value::String(text)) => text,
            other => panic!("no text on {node:?}: {other:?}"),
        }
    }

    fn status(&self) -> String {
        self.text(ACTIONS_STATUS, TEXT)
    }

    fn enabled(&self, node: NodeId) -> bool {
        self.property(node, ENABLED) != Some(Value::Bool(false))
    }

    fn exists(&self, node: NodeId) -> bool {
        self.replica.get_node(node).is_some()
    }

    fn outcome(&self) -> Option<ActionOutcome> {
        lock_actions(&self.gate).outcome().cloned()
    }

    fn withdrawn(&self) -> Option<Refusal> {
        lock_actions(&self.gate)
            .withdrawn()
            .map(|(_, reason)| reason.clone())
    }

    fn item_of(&self, pid: u64) -> ItemId {
        let Some(Value::List(headings)) = self.property(TABLE, COLUMNS) else {
            panic!("headings")
        };
        let at = headings
            .iter()
            .position(|heading| heading == &Value::String(ProcessField::Pid.heading().into()))
            .expect("a PID column");
        let model = self.replica.get_model(MODEL).unwrap();
        (0..model.item_count)
            .map(|index| model.get_item_by_index(index).unwrap())
            .find(|item| match &item.value {
                Value::List(cells) => match &cells[at] {
                    Value::UnsignedInt(shown) => *shown == pid,
                    Value::String(shown) => shown == &pid.to_string(),
                    _ => false,
                },
                _ => false,
            })
            .unwrap_or_else(|| panic!("PID {pid} is shown"))
            .item_id
    }

    /// Every handler the session holds for the action controls: Prepare,
    /// Cancel and one Confirm per panel slot.
    fn action_handlers(&self) -> usize {
        let mut count = self.session.handler_count(ACTION_PREPARE, ACTIVATE)
            + self.session.handler_count(ACTION_CANCEL, ACTIVATE);
        for slot in 0..self.slots {
            count += self
                .session
                .handler_count(PanelIds::of(slot).confirm, ACTIVATE);
        }
        count
    }
}

fn gone(outcome: &EventOutcome) -> bool {
    matches!(
        outcome,
        EventOutcome::Rejected {
            error: EventValidationError::NodeNotFound(_),
            ..
        }
    )
}

#[test]
fn a_confirmed_terminate_sends_sigterm_to_exactly_the_confirmed_instance_once() {
    let mut source = steps(vec![host(10), host(20)]);
    let mut h = harness(&mut source);
    assert_eq!(h.status(), TERMINATE_STATUS);
    assert_eq!(
        h.property(ACTION_PREPARE, LABEL),
        Some(Value::String(TERMINATE_LABEL.into()))
    );
    assert_eq!(h.kernel.opens(), 0, "the start-time probe opened nothing");
    h.select(4101);
    h.processed(ACTION_PREPARE);
    let panel = h.panel().unwrap();
    assert_eq!(
        h.text(panel.question, TEXT),
        "Terminate PID 4101 · worker (UID 1000)? Confirm sends it SIGTERM, which asks it to exit; it may handle or ignore it."
    );
    assert_eq!(
        h.text(panel.confirm, LABEL),
        "Confirm: terminate PID 4101 · worker"
    );
    assert_eq!(
        h.kernel.opens(),
        0,
        "no handle is held while a person reads it"
    );
    // Telemetry between the clicks changes nothing.
    h.sample(&mut source);
    assert_eq!(h.panel(), Some(panel));
    let confirm = h.wire(panel.confirm, ACTIVATE, Vec::new());
    assert!(matches!(
        h.deliver(&confirm),
        EventOutcome::Processed { .. }
    ));
    assert_eq!(h.kernel.delivered(), vec![(4101, 500, SIGTERM)]);
    assert!(
        matches!(
            h.outcome(),
            Some(ActionOutcome::Requested { intent, dry_run: false }) if intent.get() == 1
        ),
        "requested, not observed: {:?}",
        h.outcome()
    );
    let status = h.status();
    assert!(
        status.starts_with("Requested #1 (terminate PID 4101 · worker): SIGTERM was sent"),
        "{status}"
    );
    assert!(!status.contains("complete"), "{status}");
    assert_eq!(
        h.kernel.open_now(),
        0,
        "the handle was closed after its signal"
    );
    assert_eq!(h.terminator.live_handles(), 0);
    assert!(!h.exists(panel.row), "the panel is deleted");
    // The same event again: the settled result, nothing re-sent (§18.2).
    assert!(matches!(
        h.deliver(&confirm),
        EventOutcome::Duplicate { accepted: true, .. }
    ));
    // A new click on the consumed panel names no node.
    assert!(gone(&h.click(panel.confirm)));
    assert_eq!(h.kernel.delivered().len(), 1, "one request, one signal");
    assert_eq!(h.kernel.opens(), 1);
}

/// The acceptance case: prepare termination for A, select B, and confirm.
/// The old intent is rejected: selecting B withdraws A's intent and deletes
/// its panel, so A's Confirm names no node and nothing is opened or sent. B
/// is acted on only once B is prepared and confirmed by name.
#[test]
fn prepare_a_select_b_and_confirm_rejects_the_old_intent() {
    let mut source = steps(vec![host(10)]);
    let mut h = harness(&mut source);
    h.select(4101);
    h.processed(ACTION_PREPARE);
    let a = h.panel().unwrap();
    let a_confirm = h.wire(a.confirm, ACTIVATE, Vec::new());
    h.select(4103);
    assert_eq!(h.withdrawn(), Some(Refusal::Navigated));
    assert!(!h.exists(a.row));
    assert!(gone(&h.deliver(&a_confirm)), "A's confirmation is gone");
    assert_eq!(h.kernel.opens(), 0);
    assert!(h.kernel.delivered().is_empty());
    // B, explicitly.
    h.processed(ACTION_PREPARE);
    let b = h.panel().unwrap();
    assert_ne!(b, a);
    assert_eq!(
        h.text(b.confirm, LABEL),
        "Confirm: terminate PID 4103 · spare"
    );
    h.processed(b.confirm);
    assert_eq!(h.kernel.delivered(), vec![(4103, 700, SIGTERM)], "only B");
}

#[test]
fn cancel_and_expired_intents_open_and_send_nothing() {
    let mut source = steps(vec![host(10), host(20)]);
    let mut h = harness(&mut source);
    h.select(4101);
    // Cancel.
    h.processed(ACTION_PREPARE);
    let cancelled = h.panel().unwrap();
    h.processed(ACTION_CANCEL);
    assert!(h
        .status()
        .starts_with("Cancelled #1: nothing was requested"));
    assert!(gone(&h.click(cancelled.confirm)));
    // Expired, clicked before any sample withdrew it: refused at Confirm.
    h.processed(ACTION_PREPARE);
    let expired = h.panel().unwrap();
    h.clock
        .advance(srui_process_explorer::actions::DEFAULT_INTENT_LIFETIME);
    h.processed(expired.confirm);
    assert!(matches!(
        h.outcome(),
        Some(ActionOutcome::Refused {
            reason: Refusal::Expired,
            ..
        })
    ));
    // Expired and sampled: withdrawn, panel deleted.
    h.processed(ACTION_PREPARE);
    let withdrawn = h.panel().unwrap();
    h.clock
        .advance(srui_process_explorer::actions::DEFAULT_INTENT_LIFETIME);
    h.sample(&mut source);
    assert_eq!(h.withdrawn(), Some(Refusal::Expired));
    assert!(gone(&h.click(withdrawn.confirm)));
    assert_eq!(h.kernel.opens(), 0, "no handle was ever opened");
    assert!(h.kernel.delivered().is_empty());
}

#[test]
fn a_pid_taken_or_left_after_the_scan_is_refused_and_nothing_is_sent() {
    let mut source = steps(vec![host(10)]);
    let mut h = harness(&mut source);
    // Another process takes PID 4101 after the scan that listed A.
    h.select(4101);
    h.processed(ACTION_PREPARE);
    h.kernel.state().processes.insert(4101, 999);
    let confirm = h.panel().unwrap().confirm;
    h.processed(confirm);
    assert!(
        matches!(
            h.outcome(),
            Some(ActionOutcome::Refused {
                intent: Some(_),
                reason: Refusal::Replaced
            })
        ),
        "{:?}",
        h.outcome()
    );
    assert_eq!(h.kernel.opens(), 1, "opened, re-checked, refused");
    assert_eq!(h.kernel.open_now(), 0, "and closed");
    assert!(
        h.kernel.delivered().is_empty(),
        "the newcomer received nothing"
    );
    // A process that exited after the scan.
    h.select(4103);
    h.processed(ACTION_PREPARE);
    h.kernel.state().processes.remove(&4103);
    let confirm = h.panel().unwrap().confirm;
    h.processed(confirm);
    assert!(
        matches!(
            h.outcome(),
            Some(ActionOutcome::Refused {
                reason: Refusal::NotHeld(_),
                ..
            })
        ),
        "{:?}",
        h.outcome()
    );
    assert!(h.kernel.delivered().is_empty());
    assert_eq!(h.kernel.open_now(), 0);
}

/// A support chain naming fixed PIDs, as the live one names srtop, its
/// ancestors and its clients' transports.
struct Chain(Vec<(u32, SupportRole)>);

impl SupportChain for Chain {
    fn protects(&self, _: &ProcessKey, pid: u32) -> Result<(), Refusal> {
        match self.0.iter().find(|(member, _)| *member == pid) {
            Some((pid, role)) => Err(Refusal::SupportChain {
                pid: *pid,
                role: *role,
            }),
            None => Ok(()),
        }
    }
}

#[test]
fn the_support_chain_and_configured_pids_are_refused_before_any_handle() {
    let mut source = steps(vec![host(10)]);
    let mut h = harness_with(&mut source, Kernel::new(true), |gate| {
        gate.with_support_chain(Arc::new(Chain(vec![(4103, SupportRole::ThisServer)])))
            .with_protected_pids([1, 4101])
    });
    h.select(4103);
    h.processed(ACTION_PREPARE);
    assert!(
        h.status().starts_with(
            "Refused: PID 4103 is this server (srtop) itself, which serves this session"
        ),
        "{}",
        h.status()
    );
    h.select(4101);
    h.processed(ACTION_PREPARE);
    assert!(h.status().starts_with("Refused: PID 4101 is protected"));
    h.select(1);
    h.processed(ACTION_PREPARE);
    assert!(h.status().starts_with("Refused: PID 1 is protected"));
    assert!(h.panel().is_none());
    assert_eq!(h.kernel.opens(), 0);
}

#[test]
fn without_pid_file_descriptors_terminate_stays_disabled_and_nothing_falls_back() {
    for (name, kernel) in [
        ("not Linux", Kernel::new(false)),
        ("ENOSYS", {
            let kernel = Kernel::new(true);
            kernel.state().no_system_call = true;
            kernel
        }),
    ] {
        let mut source = steps(vec![host(10)]);
        let mut h = harness_with(&mut source, kernel, |gate| gate);
        let status = h.status();
        assert!(
            status.starts_with("Terminate unavailable: ")
                && status.ends_with("; nothing can be sent to any process"),
            "{name}: {status}"
        );
        assert!(!h.enabled(ACTION_PREPARE), "{name}");
        h.select(4101);
        assert!(matches!(
            h.click(ACTION_PREPARE),
            EventOutcome::Rejected {
                error: EventValidationError::NodeDisabled(_),
                ..
            }
        ));
        // A click that gets through anyway: unsupported, nothing prepared.
        h.session
            .transaction(|ui| {
                ui.set(ACTION_PREPARE, ENABLED, true)?;
                Ok(())
            })
            .unwrap();
        h.sync();
        h.processed(ACTION_PREPARE);
        assert!(
            matches!(h.outcome(), Some(ActionOutcome::Unsupported { .. })),
            "{name}: {:?}",
            h.outcome()
        );
        assert!(h.panel().is_none() && !h.enabled(ACTION_PREPARE), "{name}");
        assert!(h.kernel.delivered().is_empty(), "{name}");
        assert_eq!(h.kernel.opens(), 0, "{name}");
    }
}

/// Fresh node IDs per intent, from a bounded scheme, and no leak: hundreds
/// of prepare/cancel, prepare/withdraw, prepare/expire and
/// prepare/confirm cycles leave the live node count, the handler table and
/// the descriptor count exactly as they were, and never reuse a node ID.
#[test]
fn repeated_cycles_leak_no_node_handler_or_handle() {
    let mut source = steps(vec![host(10)]);
    let mut h = harness_with(&mut source, Kernel::new(true), |gate| {
        gate.with_panel_slots(512)
    });
    h.select(4101);
    let nodes = h.session.node_count();
    let handlers = h.action_handlers();
    assert_eq!(
        handlers,
        2 + 512,
        "Prepare, Cancel and one Confirm per slot"
    );
    let mut seen = std::collections::BTreeSet::new();
    let check = |h: &mut Harness, cycle: usize| {
        assert_eq!(h.session.node_count(), nodes, "cycle {cycle}: nodes");
        assert_eq!(h.action_handlers(), handlers, "cycle {cycle}: handlers");
        assert_eq!(h.kernel.open_now(), 0, "cycle {cycle}: descriptors");
        assert_eq!(h.terminator.live_handles(), 0, "cycle {cycle}: handles");
    };
    let mut prepare = |h: &mut Harness| {
        h.processed(ACTION_PREPARE);
        let panel = h.panel().unwrap();
        assert_eq!(h.session.node_count(), nodes + 3, "one panel at a time");
        assert!(seen.insert(panel.row.0), "a node ID is never reused");
        panel
    };
    for cycle in 0..150 {
        let panel = prepare(&mut h);
        h.processed(ACTION_CANCEL);
        assert!(!h.exists(panel.row));
        check(&mut h, cycle);
    }
    for cycle in 150..250 {
        let panel = prepare(&mut h);
        h.select(4103);
        h.select(4101);
        assert!(!h.exists(panel.row));
        check(&mut h, cycle);
    }
    for cycle in 250..350 {
        let panel = prepare(&mut h);
        h.clock
            .advance(srui_process_explorer::actions::DEFAULT_INTENT_LIFETIME);
        h.sample(&mut source);
        assert!(!h.exists(panel.row));
        check(&mut h, cycle);
    }
    // A new Prepare replaces a pending one and its panel.
    for _ in 350..450 {
        prepare(&mut h);
        check_one_panel(&h, nodes);
    }
    h.processed(ACTION_CANCEL);
    check(&mut h, 450);
    assert_eq!(h.kernel.opens(), 0, "no cycle so far reached a handle");
    // Confirmed cycles: each opens one descriptor and closes it.
    for cycle in 450..500 {
        let panel = prepare(&mut h);
        h.processed(panel.confirm);
        check(&mut h, cycle);
        // The worker handles SIGTERM in the fixture by staying: it is still
        // listed, so it can be prepared again.
    }
    assert_eq!(h.kernel.delivered().len(), 50);
    assert_eq!(h.kernel.opens(), 50);
    assert_eq!(seen.len(), 500);
}

fn check_one_panel(h: &Harness, nodes: usize) {
    assert_eq!(
        h.session.node_count(),
        nodes + 3,
        "the old panel was replaced"
    );
}

/// PX-029's requirement on PX-030: a handle is settled only for the key it
/// was acquired for. The terminator acquires and settles in one table; a
/// handle the table holds for another key is closed unused.
#[test]
fn a_handle_is_settled_only_for_the_key_it_was_acquired_for() {
    let kernel = Kernel::new(true);
    let clock = Arc::new(ManualClock::default());
    let mut table = handles(&kernel, &clock);
    let b = key(4103, 700);
    let a = key(4101, 500);
    let id = table.acquire(&b).unwrap();
    assert_eq!(
        signal_held(&mut table, id, &a),
        Execution::Refused(Refusal::NotHeld(
            "the held process is not the confirmed one; nothing was sent".into()
        ))
    );
    assert_eq!(table.live_count(), 0, "closed unused");
    assert_eq!(kernel.open_now(), 0);
    assert!(
        kernel.delivered().is_empty(),
        "neither A nor B received anything"
    );
    let id = table.acquire(&a).unwrap();
    assert_eq!(signal_held(&mut table, id, &a), Execution::Requested);
    assert_eq!(kernel.delivered(), vec![(4101, 500, SIGTERM)]);
}
