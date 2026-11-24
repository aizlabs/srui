//! PX-028 acceptance through the real event path: action authorization and
//! the dry-run action contract (D1 §§7.7, 18.2, 27; D2 T24, T32).
//!
//! Every click here is a wire `ACTIVATE` admitted, validated and dispatched by
//! the runtime's session (`Session::process_event`), every commit is applied by
//! a client replica, and the executor behind the gate records each intent that
//! reaches it, so "cannot reach the executor" is observed, not inferred:
//!
//! * a disabled control is refused by event validation, and read-only mode is
//!   refused by the server even when the button is enabled in the store;
//! * an unknown, historical, unconfirmed, replaced-incarnation, unowned or
//!   protected target never reaches the executor, at prepare or at confirm;
//! * forged event arguments neither widen nor change the target set, and a
//!   new selection after Prepare does not retarget the intent;
//! * telemetry commits between Prepare and Confirm leave a valid intent
//!   valid; an expired intent, a click made before the confirmation was
//!   published, another client's click and a paused view are refused;
//! * a resumed view refuses Prepare and Confirm, and publishes them disabled,
//!   until its first sample is published; then every check is made on that
//!   sample's rows (Codex review round 1);
//! * a dry-run request is reported as requested, never as observed complete,
//!   and redelivery of the same Confirm is answered from the dedupe cache
//!   without reaching the executor again.

use srui_process_explorer::actions::{
    lock_actions, ActionGate, ActionMode, ActionOutcome, Authorized, DryRunExecutor, Execution,
    IntentId, ProcessActions, Refusal, SharedActions, READ_ONLY_STATUS,
};
use srui_process_explorer::catalogue::ProcessField;
use srui_process_explorer::on_demand::{MonotonicClock, SourceTime};
use srui_process_explorer::owner::Accounts;
use srui_process_explorer::refresh::{lock_view, SharedView};
use srui_process_explorer::source::*;
use srui_process_explorer::{
    install_actions, install_inspector, install_sampling_handlers, start_from_source,
    ACTIONS_STATUS, ACTION_CANCEL, ACTION_CONFIRM, ACTION_PREPARE, MODEL, SAMPLING_PAUSE, TABLE,
};
use srui_sdk::{
    ItemId, NodeId, PropertyRef, TypeRef, Value, ACTIVATE, COLUMNS, ENABLED, LABEL, TEXT,
};
use srui_semantic_tree::{Event as DomainEvent, SemanticStore};
use srui_sessiond::{EventOutcome, Session};
use std::collections::BTreeSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

const CLIENT: &[u8] = b"px028-client";
const OTHER_CLIENT: &[u8] = b"px028-other-client";
const SOURCE: &str = "px028-fixture-v1";
const ME: u32 = 1000;
const SOMEONE: u32 = 1001;

fn key(pid: u32, token: &str) -> ProcessKey {
    ProcessKey {
        source: SourceId(SOURCE.into()),
        host: Observed::Known(HostId("px028-host".into())),
        boot: Observed::Known(BootId("px028-boot".into())),
        pid_namespace: Observed::Known(PidNamespaceId(7)),
        pid: Observed::Known(pid),
        creation: CreationToken::Opaque(token.into()),
        thread: None,
    }
}

/// One record: PID, creation token, owner and CPU ticks (so a later sample
/// with other ticks is a telemetry-only change).
fn record(pid: u32, token: &str, owner: u32, ticks: u64) -> ProcessRecord {
    ProcessRecord {
        kind: KIND_NOT_COLLECTED,
        lineage: Lineage::new(1, 100),
        owner: Observed::Known(owner),
        key: key(pid, token),
        display_name: DisplayName::from(token),
        resident: Observed::Known(4096),
        cpu: CpuUsage::Measured(CpuInterval {
            ticks,
            ticks_per_second: 100,
            elapsed: Duration::from_secs(1),
        }),
    }
}

/// `init` (PID 1, mine), `worker` (4101, mine), `other` (4102, someone
/// else's) and `spare` (4103, mine).
fn host(ticks: u64) -> Vec<ProcessRecord> {
    vec![
        record(1, "init", ME, 1),
        record(4101, "worker", ME, ticks),
        record(4102, "other", SOMEONE, 2),
        record(4103, "spare", ME, 3),
    ]
}

/// A source that publishes `steps` in turn, then repeats the last one. A
/// step in `unenumerable` cannot say what it missed, so every absent row is
/// kept from an earlier scan, unconfirmed.
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
        "Read-only · PX-028 fixture"
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
            completeness: if self.unenumerable.contains(&index) {
                Completeness::from_scan(SkippedRecords::unenumerable(), Vec::new())
            } else {
                Completeness::Complete
            },
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

/// The executor every test puts behind the gate: it records each intent's
/// targets and answers as the dry-run executor does.
#[derive(Default)]
struct Recording {
    reached: Mutex<Vec<(IntentId, Vec<ProcessKey>)>>,
    dry_run: DryRunExecutor,
}

impl ProcessActions for Recording {
    fn supports(&self, operation: srui_process_explorer::actions::ActionOperation) -> bool {
        self.dry_run.supports(operation)
    }

    fn execute(&self, action: &Authorized<'_>) -> Execution {
        self.reached.lock().unwrap().push((
            action.intent().id(),
            action
                .intent()
                .targets()
                .iter()
                .map(|target| target.key().clone())
                .collect(),
        ));
        self.dry_run.execute(action)
    }
}

impl Recording {
    fn reached(&self) -> Vec<(IntentId, Vec<ProcessKey>)> {
        self.reached.lock().unwrap().clone()
    }
}

struct Harness {
    session: Arc<Session>,
    view: SharedView,
    gate: SharedActions,
    executor: Arc<Recording>,
    clock: Arc<ManualClock>,
    replica: SemanticStore,
    seq: u64,
}

fn harness(mode: ActionMode, source: &mut Steps) -> Harness {
    let session = Arc::new(Session::mint());
    let (view, _) = start_from_source(&session, source).unwrap();
    let view: SharedView = Arc::new(Mutex::new(view));
    install_inspector(&session, &view, None);
    install_sampling_handlers(&session, &view);
    let executor = Arc::new(Recording::default());
    let clock = Arc::new(ManualClock::default());
    let gate: SharedActions = Arc::new(Mutex::new(
        ActionGate::new(mode, Arc::clone(&executor) as Arc<dyn ProcessActions>)
            .with_clock(Arc::clone(&clock) as Arc<dyn MonotonicClock>),
    ));
    install_actions(&session, &view, &gate).unwrap();
    let mut harness = Harness {
        session,
        view,
        gate,
        executor,
        clock,
        replica: SemanticStore::new(),
        seq: 0,
    };
    harness.sync();
    harness
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
            let revision = transaction.new_revision;
            self.replica
                .apply_wire_transaction(transaction)
                .unwrap_or_else(|error| panic!("a client refuses revision {revision}: {error}"));
        }
        assert_eq!(self.replica.revision().get(), self.revision());
    }

    /// Sends one wire event and returns the session's settlement.
    fn deliver(&mut self, wire: &srui_protocol::Event) -> EventOutcome {
        let outcome = self
            .session
            .process_event(wire)
            .expect("the event is admitted");
        self.sync();
        outcome
    }

    /// A wire event from `client` on `node`, observed at `observed`.
    fn wire(
        &mut self,
        client: &[u8],
        node: NodeId,
        event: TypeRef,
        observed: u64,
        arguments: Vec<(PropertyRef, Value)>,
    ) -> srui_protocol::Event {
        self.seq += 1;
        DomainEvent::new(
            None,
            self.seq,
            format!("px028-event-{}", self.seq),
            observed,
            node,
            event,
            arguments,
        )
        .with_client_instance_id(client.to_vec())
        .to_wire()
    }

    /// A click on `button` by [`CLIENT`], observed at the latest revision.
    fn click(&mut self, button: NodeId) -> EventOutcome {
        let observed = self.revision();
        let wire = self.wire(CLIENT, button, ACTIVATE, observed, Vec::new());
        self.deliver(&wire)
    }

    fn processed(&mut self, button: NodeId) {
        let outcome = self.click(button);
        assert!(
            matches!(outcome, EventOutcome::Processed { .. }),
            "{button:?} not processed: {outcome:?}"
        );
    }

    fn select(&mut self, pid: u64) {
        let item = self.item_of(pid);
        let observed = self.revision();
        let wire = self.wire(
            CLIENT,
            TABLE,
            TypeRef::EVENT_SELECTION_CHANGED,
            observed,
            vec![(PropertyRef::VALUE, Value::ItemId(item))],
        );
        assert!(matches!(
            self.deliver(&wire),
            EventOutcome::Processed { .. }
        ));
    }

    /// As if a client, or a bug, had every action button enabled: the store
    /// says so, so event validation lets a click through to its handler.
    fn enable_all(&mut self) {
        self.session
            .transaction(|ui| {
                for button in [ACTION_PREPARE, ACTION_CONFIRM, ACTION_CANCEL] {
                    ui.set(button, ENABLED, true)?;
                }
                Ok(())
            })
            .unwrap();
        self.sync();
    }

    /// The view's sampling epoch now: a sample taken now is published only
    /// while this is still the epoch.
    fn epoch(&self) -> u64 {
        lock_view(&self.view).sampling().epoch()
    }

    /// One sample from `source` handed to the view as the refresh loop hands
    /// it, as taken under `epoch` (`ProcessView::apply_sampled`).
    fn sample_under(&mut self, source: &mut Steps, epoch: u64) {
        let status = source.status_text().to_string();
        let snapshot = source.snapshot();
        lock_view(&self.view)
            .apply_sampled(&self.session, epoch, &status, &snapshot)
            .unwrap();
        self.sync();
    }

    fn sample(&mut self, source: &mut Steps) {
        lock_view(&self.view)
            .refresh(&self.session, source)
            .unwrap();
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
        self.text(ACTIONS_STATUS)
    }

    fn enabled(&self, node: NodeId) -> bool {
        self.property(node, ENABLED) != Some(Value::Bool(false))
    }

    fn outcome(&self) -> Option<ActionOutcome> {
        lock_actions(&self.gate).outcome().cloned()
    }

    fn refused(&self) -> Refusal {
        match self.outcome() {
            Some(ActionOutcome::Refused { reason, .. }) => reason,
            other => panic!("not refused: {other:?}"),
        }
    }

    fn item_of(&self, pid: u64) -> ItemId {
        let headings = match self.property(TABLE, COLUMNS) {
            Some(Value::List(headings)) => headings,
            other => panic!("headings: {other:?}"),
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
}

fn rejected_as_disabled(outcome: &EventOutcome) -> bool {
    matches!(
        outcome,
        EventOutcome::Rejected {
            error: srui_semantic_tree::EventValidationError::NodeDisabled(_),
            ..
        }
    )
}

#[test]
fn a_confirmed_dry_run_reaches_the_executor_with_exactly_the_prepared_instance() {
    let mut source = Steps::new(vec![host(10), host(20), host(30)]);
    let mut h = harness(ActionMode::DryRun, &mut source);
    assert!(h.enabled(ACTION_PREPARE));
    assert!(!h.enabled(ACTION_CONFIRM) && !h.enabled(ACTION_CANCEL));
    h.select(4101);
    // Prepare carries forged arguments naming other processes: an item ID,
    // PIDs as text and a list. `spare` (4103) is this account's own, so a
    // handler that read the argument would have an eligible target to add.
    // None is read.
    let other = h.item_of(4102);
    let spare = h.item_of(4103);
    let observed = h.revision();
    let forged = h.wire(
        CLIENT,
        ACTION_PREPARE,
        ACTIVATE,
        observed,
        vec![
            (PropertyRef::VALUE, Value::ItemId(spare)),
            (PropertyRef::TEXT, Value::String("4102 4103 1".into())),
            (
                PropertyRef::LABEL,
                Value::List(vec![Value::ItemId(other), Value::ItemId(spare)]),
            ),
        ],
    );
    assert!(matches!(h.deliver(&forged), EventOutcome::Processed { .. }));
    let prepared_at = h.revision();
    {
        let gate = lock_actions(&h.gate);
        let intent = gate.pending().expect("prepared");
        let keys: Vec<&ProcessKey> = intent.targets().iter().map(|t| t.key()).collect();
        assert_eq!(
            keys,
            vec![&key(4101, "worker")],
            "only the selected instance"
        );
        assert_eq!(gate.confirmable_from(), Some(prepared_at));
    }
    assert!(h
        .status()
        .starts_with("Prepared #1: terminate PID 4101 · worker (dry run)"));
    assert!(h.enabled(ACTION_CONFIRM) && h.enabled(ACTION_CANCEL));
    assert_eq!(
        h.property(ACTION_CONFIRM, LABEL),
        Some(Value::String(
            "Confirm dry run: terminate PID 4101 · worker".into()
        ))
    );
    // Another row is selected, and two telemetry commits land: the intent
    // still names 4101, and is still valid.
    h.select(4103);
    h.sample(&mut source);
    h.sample(&mut source);
    assert!(
        h.revision() > prepared_at + 1,
        "telemetry advanced the revision"
    );
    // Confirm, again with forged arguments.
    let observed = h.revision();
    let confirm = h.wire(
        CLIENT,
        ACTION_CONFIRM,
        ACTIVATE,
        observed,
        vec![(PropertyRef::VALUE, Value::ItemId(spare))],
    );
    assert!(matches!(
        h.deliver(&confirm),
        EventOutcome::Processed { .. }
    ));
    let reached = h.executor.reached();
    assert_eq!(reached.len(), 1);
    assert_eq!(reached[0].0.get(), 1);
    assert_eq!(reached[0].1, vec![key(4101, "worker")]);
    assert!(
        matches!(
            h.outcome(),
            Some(ActionOutcome::Requested { intent, dry_run: true }) if intent.get() == 1
        ),
        "requested, not observed complete: {:?}",
        h.outcome()
    );
    let status = h.status();
    assert!(
        status.starts_with("Dry run #1 requested (terminate PID 4101 · worker): nothing was sent"),
        "{status}"
    );
    assert!(
        !status.contains("complete") && !status.contains("ended"),
        "{status}"
    );
    assert!(!h.enabled(ACTION_CONFIRM), "a confirm is single-use");
    // Redelivery of the same Confirm is answered from the dedupe cache and
    // never reaches the executor again (§18.2).
    assert!(matches!(
        h.deliver(&confirm),
        EventOutcome::Duplicate { accepted: true, .. }
    ));
    // A new Confirm click is refused by validation: nothing is prepared.
    assert!(rejected_as_disabled(&h.click(ACTION_CONFIRM)));
    assert_eq!(h.executor.reached().len(), 1);
}

#[test]
fn disabled_controls_are_refused_by_event_validation() {
    // Read-only: every control is disabled, and a click is rejected before
    // any handler runs.
    let mut source = Steps::new(vec![host(10)]);
    let mut h = harness(ActionMode::ReadOnly, &mut source);
    assert_eq!(h.status(), READ_ONLY_STATUS);
    h.select(4101);
    for button in [ACTION_PREPARE, ACTION_CONFIRM, ACTION_CANCEL] {
        assert!(!h.enabled(button), "{button:?}");
        assert!(rejected_as_disabled(&h.click(button)), "{button:?}");
    }
    // Dry run: Confirm and Cancel are disabled until something is prepared,
    // and again once it is cancelled.
    let mut source = Steps::new(vec![host(10)]);
    let mut h = harness(ActionMode::DryRun, &mut source);
    h.select(4101);
    assert!(rejected_as_disabled(&h.click(ACTION_CONFIRM)));
    h.processed(ACTION_PREPARE);
    h.processed(ACTION_CANCEL);
    assert!(h
        .status()
        .starts_with("Cancelled #1: nothing was requested"));
    assert!(lock_actions(&h.gate).pending().is_none());
    assert!(rejected_as_disabled(&h.click(ACTION_CONFIRM)));
    assert!(rejected_as_disabled(&h.click(ACTION_CANCEL)));
    assert!(h.executor.reached().is_empty());
}

#[test]
fn read_only_mode_is_enforced_by_the_server_even_with_the_buttons_enabled() {
    let mut source = Steps::new(vec![host(10)]);
    let mut h = harness(ActionMode::ReadOnly, &mut source);
    h.select(4101);
    // As if a client, or a bug, had the buttons enabled: the store says so,
    // so event validation lets each click through to its handler.
    let enable = |h: &mut Harness| {
        h.session
            .transaction(|ui| {
                for button in [ACTION_PREPARE, ACTION_CONFIRM, ACTION_CANCEL] {
                    ui.set(button, ENABLED, true)?;
                }
                Ok(())
            })
            .unwrap();
        h.sync();
    };
    for button in [ACTION_PREPARE, ACTION_CONFIRM] {
        enable(&mut h);
        h.processed(button);
        assert_eq!(h.refused(), Refusal::ReadOnly, "{button:?}");
        assert!(lock_actions(&h.gate).pending().is_none());
        assert!(
            h.status().starts_with("Refused: this server is read-only"),
            "{}",
            h.status()
        );
        // The server republishes the controls it holds: disabled.
        for button in [ACTION_PREPARE, ACTION_CONFIRM, ACTION_CANCEL] {
            assert!(!h.enabled(button), "{button:?}");
        }
    }
    assert!(h.executor.reached().is_empty());
}

#[test]
fn stale_intents_are_refused_through_dispatch() {
    let mut source = Steps::new(vec![host(10), host(20)]);
    let mut h = harness(ActionMode::DryRun, &mut source);
    h.select(4101);

    // Expired: the lifetime passed between Prepare and Confirm.
    h.processed(ACTION_PREPARE);
    h.clock
        .advance(srui_process_explorer::actions::DEFAULT_INTENT_LIFETIME);
    h.processed(ACTION_CONFIRM);
    assert_eq!(h.refused(), Refusal::Expired);
    assert!(h
        .status()
        .starts_with("Refused #1 (terminate PID 4101 · worker): the prepared request expired"));
    assert!(!h.enabled(ACTION_CONFIRM));

    // A click made on an earlier confirmation: #2 is published, then #3
    // replaces it; a Confirm observed between the two is refused.
    h.processed(ACTION_PREPARE);
    let second = h.revision();
    h.processed(ACTION_PREPARE);
    assert_eq!(lock_actions(&h.gate).pending().unwrap().id().get(), 3);
    let stale = h.wire(CLIENT, ACTION_CONFIRM, ACTIVATE, second, Vec::new());
    assert!(matches!(h.deliver(&stale), EventOutcome::Processed { .. }));
    assert!(matches!(
        h.refused(),
        Refusal::StaleConfirmation { observed, .. } if observed == second
    ));

    // Another client instance's click.
    h.processed(ACTION_PREPARE);
    let observed = h.revision();
    let foreign = h.wire(OTHER_CLIENT, ACTION_CONFIRM, ACTIVATE, observed, Vec::new());
    assert!(matches!(
        h.deliver(&foreign),
        EventOutcome::Processed { .. }
    ));
    assert_eq!(h.refused(), Refusal::OtherClient);

    // A paused view: its rows are not the latest scan. Prepare and Confirm
    // are published disabled, and refused by the server when a click gets
    // through all the same.
    h.processed(ACTION_PREPARE);
    h.processed(SAMPLING_PAUSE);
    assert!(!h.enabled(ACTION_PREPARE) && !h.enabled(ACTION_CONFIRM));
    assert!(
        h.enabled(ACTION_CANCEL),
        "a pending intent can still be discarded"
    );
    assert!(rejected_as_disabled(&h.click(ACTION_CONFIRM)));
    h.enable_all();
    h.processed(ACTION_CONFIRM);
    assert_eq!(h.refused(), Refusal::Paused);
    h.enable_all();
    h.processed(ACTION_PREPARE);
    assert_eq!(h.refused(), Refusal::Paused);
    assert!(!h.enabled(ACTION_PREPARE), "republished disabled");
    h.processed(SAMPLING_PAUSE);
    // Resumed: on hold until the resume's sample is applied.
    assert!(!h.enabled(ACTION_PREPARE));
    h.sample(&mut source);
    assert!(h.enabled(ACTION_PREPARE));

    // And a valid one still goes through, exactly at the publishing revision.
    h.processed(ACTION_PREPARE);
    let published = h.revision();
    let exact = h.wire(CLIENT, ACTION_CONFIRM, ACTIVATE, published, Vec::new());
    assert!(matches!(h.deliver(&exact), EventOutcome::Processed { .. }));
    assert!(matches!(h.outcome(), Some(ActionOutcome::Requested { .. })));
    assert_eq!(
        h.executor.reached().len(),
        1,
        "only the last intent reached it"
    );
}

/// Codex review round 1 (PR #119): Resume view makes the view live at once,
/// but its rows and their confirmations stay the frozen ones until the
/// resume's sample is applied. In that window nothing may be prepared or
/// confirmed against them, and once the sample is published every check is
/// made on its rows.
#[test]
fn a_resumed_view_refuses_actions_until_its_first_sample_is_published() {
    let mut exited = host(20);
    exited.remove(1);
    let mut source = Steps::new(vec![host(10), exited.clone()]);
    let mut h = harness(ActionMode::DryRun, &mut source);
    h.select(4101);
    // Prepared before the pause, while 4101 was the latest scan's own.
    h.processed(ACTION_PREPARE);
    assert!(h.enabled(ACTION_CONFIRM));
    h.processed(SAMPLING_PAUSE);
    // 4101 exits while the view is paused; Resume view is applied, and the
    // sample it asked for has not been taken yet.
    h.processed(SAMPLING_PAUSE);
    assert!(lock_actions(&h.gate).pending().is_some());
    assert!(!h.enabled(ACTION_PREPARE), "Prepare on hold while resuming");
    assert!(!h.enabled(ACTION_CONFIRM), "Confirm on hold while resuming");
    assert!(h.enabled(ACTION_CANCEL));
    let status = h.status();
    assert!(
        status.contains("on hold: the view is resuming: waiting for a fresh sample"),
        "{status}"
    );
    assert!(rejected_as_disabled(&h.click(ACTION_CONFIRM)));
    assert!(rejected_as_disabled(&h.click(ACTION_PREPARE)));
    // A click that gets through anyway is refused by the server: the frozen
    // row still says 4101 is current, but that is not the latest scan.
    h.enable_all();
    h.processed(ACTION_CONFIRM);
    assert_eq!(h.refused(), Refusal::Resuming);
    assert!(h
        .status()
        .starts_with("Refused #1 (terminate PID 4101 · worker): the view is resuming: waiting for a fresh sample"));
    assert!(lock_actions(&h.gate).pending().is_none(), "consumed");
    h.enable_all();
    h.processed(ACTION_PREPARE);
    assert_eq!(h.refused(), Refusal::Resuming);
    assert!(lock_actions(&h.gate).pending().is_none());
    assert!(!h.enabled(ACTION_PREPARE) && !h.enabled(ACTION_CONFIRM));
    assert!(
        h.executor.reached().is_empty(),
        "nothing reached the executor"
    );

    // The resume's sample is published: the controls follow it, and every
    // check is made on its rows. 4101 exited during the pause.
    h.sample(&mut source);
    assert!(h.enabled(ACTION_PREPARE), "republished enabled");
    assert!(!h.enabled(ACTION_CONFIRM), "nothing is prepared");
    h.processed(ACTION_PREPARE);
    assert_eq!(h.refused(), Refusal::Historical);
    // A process the fresh sample confirmed can be prepared and confirmed.
    h.select(4103);
    h.processed(ACTION_PREPARE);
    h.processed(ACTION_CONFIRM);
    assert!(matches!(h.outcome(), Some(ActionOutcome::Requested { .. })));
    let reached = h.executor.reached();
    assert_eq!(reached.len(), 1);
    assert_eq!(reached[0].1, vec![key(4103, "spare")]);

    // An intent prepared before the pause and left pending is not
    // confirmable during the resume, and once the sample is published it is
    // re-checked on the fresh rows: its target exited, so it is refused.
    let mut source = Steps::new(vec![host(10), exited]);
    let mut h = harness(ActionMode::DryRun, &mut source);
    h.select(4101);
    h.processed(ACTION_PREPARE);
    h.processed(SAMPLING_PAUSE);
    h.processed(SAMPLING_PAUSE);
    assert!(rejected_as_disabled(&h.click(ACTION_CONFIRM)));
    h.sample(&mut source);
    assert!(
        h.enabled(ACTION_CONFIRM),
        "the pending intent is shown again"
    );
    h.processed(ACTION_CONFIRM);
    assert_eq!(h.refused(), Refusal::Historical);
    assert!(h.executor.reached().is_empty());
}

/// PX-028 round 2 verification (V3b): a scan the refresh loop began before
/// the pause returns after Resume view. It is discarded, so it neither ends
/// the resume nor makes the frozen rows actionable: only a sample taken
/// under the current epoch does.
#[test]
fn a_sample_discarded_for_its_epoch_does_not_end_the_resume_hold() {
    let mut exited = host(20);
    exited.remove(1);
    let mut source = Steps::new(vec![host(10), exited]);
    let mut h = harness(ActionMode::DryRun, &mut source);
    h.select(4101);
    h.processed(ACTION_PREPARE);
    let before_pause = h.epoch();
    h.processed(SAMPLING_PAUSE);
    h.processed(SAMPLING_PAUSE);
    assert_ne!(h.epoch(), before_pause);
    // The scan taken under the pre-pause epoch returns: discarded.
    h.sample_under(&mut source, before_pause);
    assert!(lock_view(&h.view).sampling().resuming(), "still resuming");
    assert!(!h.enabled(ACTION_PREPARE) && !h.enabled(ACTION_CONFIRM));
    assert!(
        h.status()
            .contains("on hold: the view is resuming: waiting for a fresh sample"),
        "{}",
        h.status()
    );
    // The frozen row still says 4101 is current; a click that gets through
    // is refused all the same.
    h.enable_all();
    h.processed(ACTION_CONFIRM);
    assert_eq!(h.refused(), Refusal::Resuming);
    h.enable_all();
    h.processed(ACTION_PREPARE);
    assert_eq!(h.refused(), Refusal::Resuming);
    assert!(!h.enabled(ACTION_PREPARE) && !h.enabled(ACTION_CONFIRM));
    assert!(h.executor.reached().is_empty());

    // The sample taken under the current epoch ends the resume: the
    // controls are republished, and the checks run on its rows.
    let current = h.epoch();
    h.sample_under(&mut source, current);
    assert!(!lock_view(&h.view).sampling().resuming());
    assert!(h.enabled(ACTION_PREPARE), "republished enabled");
    h.processed(ACTION_PREPARE);
    assert_eq!(h.refused(), Refusal::Historical, "4101 exited");
    h.select(4103);
    h.processed(ACTION_PREPARE);
    h.processed(ACTION_CONFIRM);
    assert!(matches!(h.outcome(), Some(ActionOutcome::Requested { .. })));
    let reached = h.executor.reached();
    assert_eq!(reached.len(), 1);
    assert_eq!(reached[0].1, vec![key(4103, "spare")]);
}

/// PX-028 round 2 verification (V5): the first scan after Resume view may
/// confirm nothing — a snapshot the app rejects, or a process list that
/// could not be read. It still ends the resume, because it is the latest
/// scan: the hold lifts, and what it says about every row is that it did not
/// confirm it, so a target is refused as unconfirmed. The next good scan
/// confirms the rows again.
#[test]
fn a_first_scan_that_confirms_nothing_ends_the_resume_and_leaves_every_row_unconfirmed() {
    // Two records with one process-instance identity: rejected whole.
    let duplicate = vec![
        record(4101, "worker", ME, 10),
        record(4101, "worker", ME, 11),
    ];
    let cases: Vec<(&str, Vec<ProcessRecord>, bool)> = vec![
        ("rejected snapshot", duplicate, false),
        ("unlistable process list", Vec::new(), true),
    ];
    for (name, step, unenumerable) in cases {
        let mut source = Steps::new(vec![host(10), step, host(30)]);
        if unenumerable {
            source.unenumerable.insert(1);
        }
        let mut h = harness(ActionMode::DryRun, &mut source);
        h.select(4101);
        h.processed(SAMPLING_PAUSE);
        h.processed(SAMPLING_PAUSE);
        assert!(!h.enabled(ACTION_PREPARE), "{name}: on hold while resuming");
        h.sample(&mut source);
        assert!(
            !lock_view(&h.view).sampling().resuming(),
            "{name}: the scan ends the resume"
        );
        assert!(h.enabled(ACTION_PREPARE), "{name}: the hold lifts");
        assert!(!h.status().contains("on hold"), "{name}: {}", h.status());
        h.processed(ACTION_PREPARE);
        assert_eq!(h.refused(), Refusal::Unconfirmed, "{name}");
        assert!(lock_actions(&h.gate).pending().is_none(), "{name}");
        h.sample(&mut source);
        h.processed(ACTION_PREPARE);
        assert!(lock_actions(&h.gate).pending().is_some(), "{name}");
        h.processed(ACTION_CONFIRM);
        assert!(
            matches!(h.outcome(), Some(ActionOutcome::Requested { .. })),
            "{name}: {:?}",
            h.outcome()
        );
        assert_eq!(h.executor.reached()[0].1, vec![key(4101, "worker")]);
    }
}

#[test]
fn ineligible_targets_never_reach_the_executor() {
    let mut replaced = host(10);
    replaced[1] = record(4101, "newcomer", ME, 10);
    let mut setuid = host(10);
    setuid[1] = record(4101, "worker", SOMEONE, 10);
    let mut exited = host(10);
    exited.remove(1);
    // Each case: the step to sample between Prepare and Confirm, whether it
    // is unenumerable, whether 4101 is still the selection, and the refusal.
    let cases: Vec<(&str, Vec<ProcessRecord>, bool, bool, Refusal)> = vec![
        (
            "historical",
            exited.clone(),
            false,
            true,
            Refusal::Historical,
        ),
        ("unknown", exited.clone(), false, false, Refusal::Unknown),
        ("unconfirmed", exited, true, true, Refusal::Unconfirmed),
        ("replaced", replaced, false, true, Refusal::Replaced),
        (
            "another owner",
            setuid,
            false,
            true,
            Refusal::NotOwned {
                owner: Observed::Known(SOMEONE),
                account: ME,
            },
        ),
    ];
    for (name, step, unenumerable, still_selected, reason) in cases {
        let mut source = Steps::new(vec![host(10), step]);
        if unenumerable {
            source.unenumerable.insert(1);
        }
        let mut h = harness(ActionMode::DryRun, &mut source);
        h.select(4101);
        h.processed(ACTION_PREPARE);
        if !still_selected {
            // Nothing watches 4101 any more, so it leaves no record.
            h.select(4103);
        }
        h.sample(&mut source);
        h.processed(ACTION_CONFIRM);
        assert_eq!(h.refused(), reason, "{name}");
        assert!(
            h.status().starts_with("Refused #1"),
            "{name}: {}",
            h.status()
        );
        assert!(
            h.executor.reached().is_empty(),
            "{name} reached the executor"
        );
    }

    // At Prepare: another account's process, and PID 1.
    let mut source = Steps::new(vec![host(10)]);
    let mut h = harness(ActionMode::DryRun, &mut source);
    h.select(4102);
    h.processed(ACTION_PREPARE);
    assert_eq!(
        h.refused(),
        Refusal::NotOwned {
            owner: Observed::Known(SOMEONE),
            account: ME
        }
    );
    assert!(h
        .status()
        .starts_with("Refused: the process is owned by UID 1001, not this account (UID 1000)"));
    assert!(!h.enabled(ACTION_CONFIRM));
    h.select(1);
    h.processed(ACTION_PREPARE);
    assert_eq!(h.refused(), Refusal::Protected(1));
    // An item no row has is never selected, so nothing is targeted.
    let observed = h.revision();
    let forged = h.wire(
        CLIENT,
        TABLE,
        TypeRef::EVENT_SELECTION_CHANGED,
        observed,
        vec![(PropertyRef::VALUE, Value::ItemId(ItemId::new(999_999)))],
    );
    h.deliver(&forged);
    h.processed(ACTION_PREPARE);
    assert_ne!(
        lock_actions(&h.gate)
            .pending()
            .map(|intent| intent.targets()[0].pid()),
        Some(999_999)
    );
    assert!(h.executor.reached().is_empty());
}

#[test]
fn the_dry_run_executor_is_what_main_installs_and_it_reports_requested() {
    // The production executor, behind the real handlers.
    let mut source = Steps::new(vec![host(10)]);
    let session = Arc::new(Session::mint());
    let (view, _) = start_from_source(&session, &mut source).unwrap();
    let view: SharedView = Arc::new(Mutex::new(view));
    install_inspector(&session, &view, None);
    let gate: SharedActions = Arc::new(Mutex::new(ActionGate::new(
        ActionMode::DryRun,
        Arc::new(DryRunExecutor::default()),
    )));
    install_actions(&session, &view, &gate).unwrap();
    let mut h = Harness {
        session,
        view,
        gate,
        executor: Arc::new(Recording::default()),
        clock: Arc::new(ManualClock::default()),
        replica: SemanticStore::new(),
        seq: 0,
    };
    h.sync();
    h.select(4101);
    h.processed(ACTION_PREPARE);
    h.processed(ACTION_CONFIRM);
    assert!(matches!(
        h.outcome(),
        Some(ActionOutcome::Requested { intent, dry_run: true }) if intent.get() == 1
    ));
}
