//! Process action authorization and the dry-run action contract (D1 §7.7
//! semantic input routing, §18.2 event deduplication, §27 server security
//! controls; D2 T24, T32; PX-028).
//!
//! Everything that decides whether a process may be acted on lives here, on
//! the server, and nothing here can act on one:
//!
//! * [`ActionIntent`] is the immutable record of one prepared action: an
//!   intent ID minted by the server, the exact [`ProcessKey`]s it targets, the
//!   operation, its parameters, the account and client instance it was
//!   prepared for, and when it was created and expires. Its fields are
//!   private and it has no setter: confirming acts on what was prepared, never
//!   on the selection the client holds when it confirms.
//! * [`ProcessActions`] is the server-only interface an authorized intent
//!   reaches. It is handed an [`Authorized`], which only [`ActionGate`] can
//!   construct, after it re-checked every target against the latest scan. The
//!   only implementation is [`DryRunExecutor`], which sends nothing: it has no
//!   way to reach a process (PX-008's audit holds for this module like any
//!   other). Handles, signals and termination are PX-029 and PX-030.
//! * [`ActionOutcome`] keeps the six results apart: refused, unsupported,
//!   requested, observed complete, failed and outcome unknown. An executor
//!   reports only what it did ([`Execution`]); it cannot report an observed
//!   completion, which only a later observation of the process can establish
//!   (PX-031).
//! * Read-only mode is enforced here, at both entry points, not only by the
//!   disabled buttons: a click that reaches a handler in read-only mode is
//!   refused whatever the store says about the button.
//!
//! Freshness is the intent's own, never the session's revision. A confirm
//! click is accepted when it was observed at or after the revision that
//! published this intent's confirmation (§7.7 `observed_revision`), by the
//! client instance that prepared it, before its expiry, while every target is
//! still the instance the latest scan confirmed, owned by the account and not
//! protected. Telemetry commits between the two clicks advance the revision
//! and change nothing here; a click observed before the confirmation was
//! published was a click on something else, and is refused. A confirm is
//! single-use: the intent is consumed whatever the result, and redelivery of
//! the same event is answered from the runtime's dedupe cache without
//! re-running this handler (§18.2).
//!
//! The handlers read no event argument. A click names only its button; the
//! target is the process the server resolved as selected when Prepare was
//! clicked, so no argument a client attaches can widen, narrow or change the
//! target set.

use crate::on_demand::{MonotonicClock, SuspendAwareClock};
use crate::refresh::{lock_view, ProcessView};
use crate::source::{Observed, ProcessKey};
use crate::{bounded, ACTIONS_ROW, ACTIONS_STATUS, ACTION_CANCEL, ACTION_CONFIRM, ACTION_PREPARE};
use srui_sdk::{Button, NodeId, Row, SpacingRole, Text, TextRole, ENABLED, LABEL, TEXT};
use srui_sessiond::{Session, SessionError};
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How long a prepared intent can be confirmed (PX-028): long enough to read
/// the confirmation, short enough that a forgotten one does not linger.
pub const DEFAULT_INTENT_LIFETIME: Duration = Duration::from_secs(30);
/// The longest actions status, before the cut mark.
pub const MAX_ACTIONS_STATUS_BYTES: usize = 1024;
/// The longest Confirm label, before the cut mark.
pub const MAX_CONFIRM_LABEL_BYTES: usize = 512;
/// The longest process description an intent carries.
const MAX_WHO_BYTES: usize = 256;
/// The PIDs no intent may target unless configured otherwise: PID 1, the
/// namespace's init. PX-030 adds the app's own support chain; this set is
/// never the only authorization boundary.
pub const DEFAULT_PROTECTED_PIDS: [u32; 1] = [1];

pub const PREPARE_LABEL: &str = "Prepare dry run: terminate selected";
pub const PREPARE_DESCRIPTION: &str =
    "Checks the process selected in the table on the server and prepares a dry-run request for it alone. Nothing is sent to any process.";
pub const CONFIRM_LABEL: &str = "Confirm";
pub const CONFIRM_DESCRIPTION: &str =
    "Confirms the prepared request for the process it names, which the server checks again first. In this build the request is recorded and never sent.";
pub const CANCEL_LABEL: &str = "Cancel";
pub const CANCEL_DESCRIPTION: &str = "Discards the prepared request without acting on it.";
pub const READ_ONLY_STATUS: &str =
    "Read-only: this server refuses every process action; nothing can be prepared or sent";
pub const DRY_RUN_STATUS: &str =
    "Dry run: Prepare checks the selected process and Confirm records the request; nothing is ever sent to a process";
/// The action controls' nodes: the row, three buttons and the status.
pub const ACTIONS_NODE_COUNT: usize = 5;

/// Whether this server may act on processes at all (PX-028).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionMode {
    /// Every action is refused on the server; the controls are disabled.
    ReadOnly,
    /// Actions are authorized exactly as a real one would be and handed to
    /// the dry-run executor, which sends nothing.
    DryRun,
}

impl ActionMode {
    /// The `--actions` argument: `read-only` or `dry-run`.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "read-only" => Some(Self::ReadOnly),
            "dry-run" => Some(Self::DryRun),
            _ => None,
        }
    }
}

/// What an intent asks for. The only operation is a graceful termination
/// request (PX-030 implements it for real); further families are later
/// tickets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActionOperation {
    Terminate,
}

impl ActionOperation {
    pub fn verb(self) -> &'static str {
        match self {
            Self::Terminate => "terminate",
        }
    }
}

/// Which processes an intent's targets were chosen from. Fixed by the
/// button on the server; never read from a client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetScope {
    /// The one process the server resolved as selected.
    SelectedProcess,
}

/// An intent's parameters, chosen by the server for the button clicked.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionParameters {
    scope: TargetScope,
}

impl ActionParameters {
    pub fn scope(&self) -> TargetScope {
        self.scope
    }
}

/// An intent's identity, minted by the server, increasing within one gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IntentId(u64);

impl IntentId {
    pub fn get(self) -> u64 {
        self.0
    }
}

/// Who an intent was prepared for: the account the server acts as (the
/// daemon's own, which §27 binds to the authenticated SSH user) and the
/// client instance whose click prepared it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionContext {
    account: u32,
    client_instance_id: Vec<u8>,
}

impl ActionContext {
    pub fn account(&self) -> u32 {
        self.account
    }

    pub fn client_instance_id(&self) -> &[u8] {
        &self.client_instance_id
    }
}

/// One process instance an intent targets, as the server admitted it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionTarget {
    key: ProcessKey,
    pid: u32,
    owner: u32,
    who: String,
}

impl ActionTarget {
    pub fn key(&self) -> &ProcessKey {
        &self.key
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    pub fn owner(&self) -> u32 {
        self.owner
    }

    /// "PID 4101 · worker".
    pub fn who(&self) -> &str {
        &self.who
    }
}

/// One prepared action (PX-028). Immutable: built once by
/// [`ActionGate::prepare`], read through getters, consumed by
/// [`ActionGate::confirm`] or [`ActionGate::cancel`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionIntent {
    id: IntentId,
    targets: Vec<ActionTarget>,
    operation: ActionOperation,
    parameters: ActionParameters,
    context: ActionContext,
    created_at: Duration,
    expires_at: Duration,
}

impl ActionIntent {
    pub fn id(&self) -> IntentId {
        self.id
    }

    pub fn targets(&self) -> &[ActionTarget] {
        &self.targets
    }

    pub fn operation(&self) -> ActionOperation {
        self.operation
    }

    pub fn parameters(&self) -> ActionParameters {
        self.parameters
    }

    pub fn context(&self) -> &ActionContext {
        &self.context
    }

    /// When it was prepared, on the gate's monotonic clock.
    pub fn created_at(&self) -> Duration {
        self.created_at
    }

    /// The first instant, on the gate's monotonic clock, at which it can no
    /// longer be confirmed.
    pub fn expires_at(&self) -> Duration {
        self.expires_at
    }

    /// "terminate PID 4101 · worker".
    fn subject(&self) -> String {
        let whom: Vec<&str> = self.targets.iter().map(|target| target.who()).collect();
        format!("{} {}", self.operation.verb(), whom.join(", "))
    }
}

/// What the latest scan says about one process instance (PX-028).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetState {
    /// The latest scan listed exactly this instance.
    Current {
        pid: Observed<u32>,
        owner: Observed<u32>,
        thread: bool,
        who: String,
    },
    /// A row kept from an earlier scan: the latest one did not confirm it.
    Unconfirmed,
    /// Only a last-observed record is left of it (PX-021).
    Historical,
    /// The latest scan lists another instance under its PID.
    Replaced,
    /// The server knows nothing of it.
    Unknown,
}

/// What the gate asks of the server's process view: never of the client.
pub trait ActionTargets {
    /// The account the server acts as, as the source states it.
    fn account(&self) -> Observed<u32>;
    /// Whether the view is paused, so its rows are not the latest scan.
    fn paused(&self) -> bool;
    /// The process instance the server resolved as selected.
    fn selected(&self) -> Option<ProcessKey>;
    /// What the latest scan says about `key`.
    fn state_of(&self, key: &ProcessKey) -> TargetState;
}

/// Why an action was refused (PX-028).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The server is read-only.
    ReadOnly,
    /// The source does not state the account the server acts as.
    AccountUnknown,
    /// The view is paused, so its rows are not the latest scan.
    Paused,
    /// No process row is selected.
    NothingSelected,
    /// A thread row is selected; actions target processes.
    NotAProcess,
    /// The scan could not read the process's PID.
    PidUnknown,
    /// The latest scan did not confirm the process: its row is kept from an
    /// earlier one.
    Unconfirmed,
    /// The process left; only its last-observed record remains.
    Historical,
    /// Another process now holds its PID.
    Replaced,
    /// The server knows nothing of the process.
    Unknown,
    /// The process is not the account's own: another owner, or one the scan
    /// could not read.
    NotOwned { owner: Observed<u32>, account: u32 },
    /// The process is protected.
    Protected(u32),
    /// Confirm or Cancel with no prepared intent.
    NoPendingIntent,
    /// Another client instance prepared the intent.
    OtherClient,
    /// The click was observed before the confirmation it would confirm was
    /// published.
    StaleConfirmation {
        observed: u64,
        confirmable_from: Option<u64>,
    },
    /// The intent expired.
    Expired,
    /// The account the server acts as is not the one it was prepared for.
    AccountChanged,
}

impl Refusal {
    pub fn describe(&self) -> String {
        match self {
            Self::ReadOnly => "this server is read-only and refuses every process action".into(),
            Self::AccountUnknown => "the account this server acts as is not known".into(),
            Self::Paused => {
                "the view is paused, so its rows are not the latest scan; resume it first".into()
            }
            Self::NothingSelected => "select a process row first".into(),
            Self::NotAProcess => "a thread is selected; actions target processes".into(),
            Self::PidUnknown => "the scan could not read the process's PID".into(),
            Self::Unconfirmed => {
                "the latest scan did not confirm the process; its row is kept from an earlier scan"
                    .into()
            }
            Self::Historical => "the process left; only its last-observed record remains".into(),
            Self::Replaced => "another process now holds its PID".into(),
            Self::Unknown => "the server does not know the process".into(),
            Self::NotOwned {
                owner: Observed::Known(owner),
                account,
            } => format!("the process is owned by UID {owner}, not this account (UID {account})"),
            Self::NotOwned {
                owner: Observed::Missing(reason),
                account,
            } => format!(
                "the process's owner is {}, so it is not known to be this account's (UID {account})",
                reason.describe()
            ),
            Self::Protected(pid) => format!("PID {pid} is protected"),
            Self::NoPendingIntent => "nothing is prepared".into(),
            Self::OtherClient => "another client prepared this request".into(),
            Self::StaleConfirmation { .. } => {
                "the click was made before this request's confirmation was shown".into()
            }
            Self::Expired => "the prepared request expired".into(),
            Self::AccountChanged => "the account changed since the request was prepared".into(),
        }
    }
}

/// What an executor did with an authorized intent. It has no way to say that
/// a process was observed to end: only observation can say that (PX-031).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Execution {
    Requested,
    Unsupported,
    Failed(String),
    OutcomeUnknown(String),
}

/// The typed result of one action (PX-028). The six never collapse into one
/// another: a request is not a completion, and a refusal is not a failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionOutcome {
    /// The server refused it; nothing reached the executor.
    Refused {
        intent: Option<IntentId>,
        reason: Refusal,
    },
    /// The operation is not available here.
    Unsupported {
        intent: Option<IntentId>,
        operation: ActionOperation,
    },
    /// The executor accepted the request. `dry_run`: it sent nothing.
    Requested { intent: IntentId, dry_run: bool },
    /// A later observation established the requested effect (PX-031).
    ObservedComplete { intent: IntentId },
    /// The executor reported that the request failed.
    Failed { intent: IntentId, reason: String },
    /// The executor cannot say whether the request took effect.
    OutcomeUnknown { intent: IntentId, reason: String },
}

impl ActionOutcome {
    /// The outcome of handing intent `intent` to an executor.
    fn of_execution(
        intent: IntentId,
        operation: ActionOperation,
        dry_run: bool,
        execution: Execution,
    ) -> Self {
        match execution {
            Execution::Requested => Self::Requested { intent, dry_run },
            Execution::Unsupported => Self::Unsupported {
                intent: Some(intent),
                operation,
            },
            Execution::Failed(reason) => Self::Failed { intent, reason },
            Execution::OutcomeUnknown(reason) => Self::OutcomeUnknown { intent, reason },
        }
    }

    /// "refused", "unsupported", "requested", "observed complete", "failed"
    /// or "outcome unknown".
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Refused { .. } => "refused",
            Self::Unsupported { .. } => "unsupported",
            Self::Requested { .. } => "requested",
            Self::ObservedComplete { .. } => "observed complete",
            Self::Failed { .. } => "failed",
            Self::OutcomeUnknown { .. } => "outcome unknown",
        }
    }

    /// The status line for this outcome, of the intent described by
    /// `subject` when there was one.
    fn describe(&self, subject: Option<&str>) -> String {
        let numbered = |intent: Option<IntentId>| match intent {
            Some(intent) => format!(" #{}", intent.get()),
            None => String::new(),
        };
        let about = subject.map_or(String::new(), |subject| format!(" ({subject})"));
        match self {
            Self::Refused { intent, reason } => format!(
                "Refused{}{about}: {}; nothing was requested",
                numbered(*intent),
                reason.describe()
            ),
            Self::Unsupported { intent, operation } => format!(
                "Unsupported{}: this server cannot {} a process; nothing was requested",
                numbered(*intent),
                operation.verb()
            ),
            Self::Requested {
                intent,
                dry_run: true,
            } => format!(
                "Dry run #{} requested{about}: nothing was sent to the process, and whether it ends is not observed",
                intent.get()
            ),
            Self::Requested {
                intent,
                dry_run: false,
            } => format!(
                "Requested #{}{about}: whether it took effect is not yet observed",
                intent.get()
            ),
            Self::ObservedComplete { intent } => {
                format!("Observed complete #{}{about}", intent.get())
            }
            Self::Failed { intent, reason } => {
                format!("Failed #{}{about}: {reason}", intent.get())
            }
            Self::OutcomeUnknown { intent, reason } => {
                format!("Outcome unknown #{}{about}: {reason}", intent.get())
            }
        }
    }
}

/// An intent the gate has just re-checked in full (PX-028). Only
/// [`ActionGate::confirm`] constructs one, so an executor can be reached by
/// nothing else.
#[derive(Debug)]
pub struct Authorized<'a> {
    intent: &'a ActionIntent,
}

impl Authorized<'_> {
    pub fn intent(&self) -> &ActionIntent {
        self.intent
    }
}

/// The server-only interface an authorized action reaches (PX-028).
pub trait ProcessActions: Send + Sync {
    /// Whether this executor offers `operation` at all.
    fn supports(&self, operation: ActionOperation) -> bool;
    /// Acts on `action`, whose every target the gate has just re-checked.
    fn execute(&self, action: &Authorized<'_>) -> Execution;
}

/// The only executor in this build: it reports each supported request as
/// requested and sends nothing. It holds no handle and calls nothing that
/// could reach a process.
#[derive(Debug, Clone)]
pub struct DryRunExecutor {
    supported: Vec<ActionOperation>,
}

impl Default for DryRunExecutor {
    fn default() -> Self {
        Self::supporting(&[ActionOperation::Terminate])
    }
}

impl DryRunExecutor {
    /// A dry-run executor that offers exactly `operations`.
    pub fn supporting(operations: &[ActionOperation]) -> Self {
        Self {
            supported: operations.to_vec(),
        }
    }
}

impl ProcessActions for DryRunExecutor {
    fn supports(&self, operation: ActionOperation) -> bool {
        self.supported.contains(&operation)
    }

    fn execute(&self, action: &Authorized<'_>) -> Execution {
        if self.supports(action.intent().operation()) {
            Execution::Requested
        } else {
            Execution::Unsupported
        }
    }
}

/// The action controls as the client holds them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionsView {
    pub prepare_enabled: bool,
    pub confirm_enabled: bool,
    pub cancel_enabled: bool,
    pub confirm_label: String,
    pub status: String,
}

/// A prepared intent and the revision that published its confirmation.
#[derive(Debug, Clone)]
struct Pending {
    intent: ActionIntent,
    /// `None` until the transaction publishing it commits.
    confirmable_from: Option<u64>,
}

/// What the status line reports.
#[derive(Debug, Clone)]
enum Report {
    Idle,
    Prepared,
    Cancelled(IntentId),
    Outcome {
        outcome: ActionOutcome,
        subject: Option<String>,
    },
}

/// The server's action authority (PX-028): the mode, the one pending intent,
/// and the executor an authorized intent reaches.
pub struct ActionGate {
    mode: ActionMode,
    executor: Arc<dyn ProcessActions>,
    clock: Arc<dyn MonotonicClock>,
    lifetime: Duration,
    protected: BTreeSet<u32>,
    next_intent: u64,
    pending: Option<Pending>,
    report: Report,
    published: Option<ActionsView>,
}

impl std::fmt::Debug for ActionGate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ActionGate")
            .field("mode", &self.mode)
            .field("pending", &self.pending)
            .field("report", &self.report)
            .finish_non_exhaustive()
    }
}

/// A gate shared by the action handlers.
pub type SharedActions = Arc<Mutex<ActionGate>>;

/// Locks a shared gate. Every state change is complete before the gate
/// publishes, so a panic while publishing leaves a consistent gate.
pub fn lock_actions(gate: &Mutex<ActionGate>) -> std::sync::MutexGuard<'_, ActionGate> {
    gate.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl ActionGate {
    /// A gate in `mode` over `executor`, on a suspend-aware clock, with the
    /// default lifetime and protected PIDs.
    pub fn new(mode: ActionMode, executor: Arc<dyn ProcessActions>) -> Self {
        Self {
            mode,
            executor,
            clock: Arc::new(SuspendAwareClock::new()),
            lifetime: DEFAULT_INTENT_LIFETIME,
            protected: DEFAULT_PROTECTED_PIDS.into_iter().collect(),
            next_intent: 1,
            pending: None,
            report: Report::Idle,
            published: None,
        }
    }

    /// This gate on `clock`, which intents are created and expire by.
    pub fn with_clock(mut self, clock: Arc<dyn MonotonicClock>) -> Self {
        self.clock = clock;
        self
    }

    /// This gate with intents that expire `lifetime` after preparation.
    pub fn with_lifetime(mut self, lifetime: Duration) -> Self {
        self.lifetime = lifetime;
        self
    }

    /// This gate protecting exactly `pids`.
    pub fn with_protected_pids(mut self, pids: impl IntoIterator<Item = u32>) -> Self {
        self.protected = pids.into_iter().collect();
        self
    }

    pub fn mode(&self) -> ActionMode {
        self.mode
    }

    /// The prepared intent, if any.
    pub fn pending(&self) -> Option<&ActionIntent> {
        self.pending.as_ref().map(|pending| &pending.intent)
    }

    /// The revision that published the pending intent's confirmation.
    pub fn confirmable_from(&self) -> Option<u64> {
        self.pending
            .as_ref()
            .and_then(|pending| pending.confirmable_from)
    }

    /// The outcome of the latest prepare or confirm that produced one.
    pub fn outcome(&self) -> Option<&ActionOutcome> {
        match &self.report {
            Report::Outcome { outcome, .. } => Some(outcome),
            _ => None,
        }
    }

    /// Read-only mode, checked by the server at every entry point.
    fn writable(&self) -> Result<(), Refusal> {
        match self.mode {
            ActionMode::ReadOnly => Err(Refusal::ReadOnly),
            ActionMode::DryRun => Ok(()),
        }
    }

    /// Admits `key` as a target for `account`: the latest scan's own
    /// instance, a process, with a known PID, owned by `account`, not
    /// protected.
    fn admit(
        &self,
        targets: &dyn ActionTargets,
        key: &ProcessKey,
        account: u32,
    ) -> Result<ActionTarget, Refusal> {
        let (pid, owner, thread, who) = match targets.state_of(key) {
            TargetState::Current {
                pid,
                owner,
                thread,
                who,
            } => (pid, owner, thread, who),
            TargetState::Unconfirmed => return Err(Refusal::Unconfirmed),
            TargetState::Historical => return Err(Refusal::Historical),
            TargetState::Replaced => return Err(Refusal::Replaced),
            TargetState::Unknown => return Err(Refusal::Unknown),
        };
        if thread || key.is_thread() {
            return Err(Refusal::NotAProcess);
        }
        let Observed::Known(pid) = pid else {
            return Err(Refusal::PidUnknown);
        };
        if owner != Observed::Known(account) {
            return Err(Refusal::NotOwned { owner, account });
        }
        if self.protected.contains(&pid) {
            return Err(Refusal::Protected(pid));
        }
        Ok(ActionTarget {
            key: key.clone(),
            pid,
            owner: account,
            who: bounded(&who, MAX_WHO_BYTES),
        })
    }

    /// The account the server acts as, when the source states it.
    fn account(targets: &dyn ActionTargets) -> Result<u32, Refusal> {
        match targets.account() {
            Observed::Known(account) => Ok(account),
            Observed::Missing(_) => Err(Refusal::AccountUnknown),
        }
    }

    /// Prepares `operation` on the process the server resolved as selected,
    /// for the client instance `client`, replacing any pending intent. Nothing
    /// is published: [`Self::publish`] does that, and only a published intent
    /// can be confirmed.
    pub fn prepare(
        &mut self,
        targets: &dyn ActionTargets,
        client: &[u8],
        operation: ActionOperation,
    ) -> Result<IntentId, ActionOutcome> {
        // A new Prepare replaces the old intent whatever its result.
        self.pending = None;
        let prepared = self.prepare_intent(targets, client, operation);
        match prepared {
            Ok(intent) => {
                let id = intent.id;
                self.pending = Some(Pending {
                    intent,
                    confirmable_from: None,
                });
                self.report = Report::Prepared;
                Ok(id)
            }
            Err(outcome) => {
                self.report = Report::Outcome {
                    outcome: outcome.clone(),
                    subject: None,
                };
                Err(outcome)
            }
        }
    }

    fn prepare_intent(
        &mut self,
        targets: &dyn ActionTargets,
        client: &[u8],
        operation: ActionOperation,
    ) -> Result<ActionIntent, ActionOutcome> {
        let refused = |reason| ActionOutcome::Refused {
            intent: None,
            reason,
        };
        self.writable().map_err(refused)?;
        if !self.executor.supports(operation) {
            return Err(ActionOutcome::Unsupported {
                intent: None,
                operation,
            });
        }
        let account = Self::account(targets).map_err(refused)?;
        if targets.paused() {
            return Err(refused(Refusal::Paused));
        }
        let key = targets
            .selected()
            .ok_or_else(|| refused(Refusal::NothingSelected))?;
        let target = self.admit(targets, &key, account).map_err(refused)?;
        let id = IntentId(self.next_intent);
        self.next_intent += 1;
        let created_at = self.clock.now().0;
        Ok(ActionIntent {
            id,
            targets: vec![target],
            operation,
            parameters: ActionParameters {
                scope: TargetScope::SelectedProcess,
            },
            context: ActionContext {
                account,
                client_instance_id: client.to_vec(),
            },
            created_at,
            expires_at: created_at.saturating_add(self.lifetime),
        })
    }

    /// Confirms the pending intent for the client instance `client`, whose
    /// click was observed at revision `observed`. The intent is consumed
    /// whatever the result; only a fully re-checked one reaches the executor.
    pub fn confirm(
        &mut self,
        targets: &dyn ActionTargets,
        client: &[u8],
        observed: u64,
    ) -> ActionOutcome {
        let pending = self.pending.take();
        let subject = pending.as_ref().map(|pending| pending.intent.subject());
        let outcome = match self.authorize(pending.as_ref(), targets, client, observed) {
            Ok(intent) => {
                let execution = self.executor.execute(&Authorized { intent });
                ActionOutcome::of_execution(
                    intent.id,
                    intent.operation,
                    self.mode == ActionMode::DryRun,
                    execution,
                )
            }
            Err(reason) => ActionOutcome::Refused {
                intent: pending.as_ref().map(|pending| pending.intent.id),
                reason,
            },
        };
        self.report = Report::Outcome {
            outcome: outcome.clone(),
            subject,
        };
        outcome
    }

    /// Every check a confirmation must pass, in full, at the moment of
    /// confirmation.
    fn authorize<'p>(
        &self,
        pending: Option<&'p Pending>,
        targets: &dyn ActionTargets,
        client: &[u8],
        observed: u64,
    ) -> Result<&'p ActionIntent, Refusal> {
        self.writable()?;
        let pending = pending.ok_or(Refusal::NoPendingIntent)?;
        let intent = &pending.intent;
        if intent.context.client_instance_id != client {
            return Err(Refusal::OtherClient);
        }
        // Intent-specific freshness: the click must have been made on this
        // intent's confirmation. Any later revision — a telemetry commit, a
        // sort — leaves it valid.
        match pending.confirmable_from {
            Some(from) if observed >= from => {}
            confirmable_from => {
                return Err(Refusal::StaleConfirmation {
                    observed,
                    confirmable_from,
                })
            }
        }
        if self.clock.now().0 >= intent.expires_at {
            return Err(Refusal::Expired);
        }
        if Self::account(targets)? != intent.context.account {
            return Err(Refusal::AccountChanged);
        }
        if targets.paused() {
            return Err(Refusal::Paused);
        }
        for target in &intent.targets {
            self.admit(targets, &target.key, intent.context.account)?;
        }
        Ok(intent)
    }

    /// Discards the pending intent. Returns what it discarded.
    pub fn cancel(&mut self) -> Option<IntentId> {
        let cancelled = self.pending.take().map(|pending| pending.intent.id);
        self.report = match cancelled {
            Some(id) => Report::Cancelled(id),
            None => Report::Outcome {
                outcome: ActionOutcome::Refused {
                    intent: None,
                    reason: Refusal::NoPendingIntent,
                },
                subject: None,
            },
        };
        cancelled
    }

    /// The controls this gate's state calls for.
    pub fn view(&self) -> ActionsView {
        let writable = self.writable().is_ok();
        let pending = self.pending.as_ref().map(|pending| &pending.intent);
        let status = match (&self.report, pending) {
            (Report::Prepared, Some(intent)) => format!(
                "Prepared #{}: {} (dry run). Confirm within {} s or Cancel; only this process can be acted on",
                intent.id.get(),
                intent.subject(),
                self.lifetime.as_secs()
            ),
            (Report::Cancelled(id), _) => {
                format!("Cancelled #{}: nothing was requested", id.get())
            }
            (Report::Outcome { outcome, subject }, _) => outcome.describe(subject.as_deref()),
            _ if writable => DRY_RUN_STATUS.to_string(),
            _ => READ_ONLY_STATUS.to_string(),
        };
        ActionsView {
            prepare_enabled: writable,
            confirm_enabled: writable && pending.is_some(),
            cancel_enabled: writable && pending.is_some(),
            confirm_label: match pending {
                Some(intent) => bounded(
                    &format!("Confirm dry run: {}", intent.subject()),
                    MAX_CONFIRM_LABEL_BYTES,
                ),
                None => CONFIRM_LABEL.to_string(),
            },
            status: bounded(&status, MAX_ACTIONS_STATUS_BYTES),
        }
    }

    /// Creates the action controls under `parent`, in one transaction, as
    /// this gate's state calls for.
    pub fn create_controls(
        &mut self,
        session: &Session,
        parent: NodeId,
    ) -> Result<(), SessionError> {
        let view = self.view();
        session.transaction(|ui| {
            Row::builder(ACTIONS_ROW)
                .parent(parent)
                .spacing_role(SpacingRole::Normal)
                .create(ui)?;
            for (button, label, description, enabled) in [
                (
                    ACTION_PREPARE,
                    PREPARE_LABEL,
                    PREPARE_DESCRIPTION,
                    view.prepare_enabled,
                ),
                (
                    ACTION_CONFIRM,
                    view.confirm_label.as_str(),
                    CONFIRM_DESCRIPTION,
                    view.confirm_enabled,
                ),
                (
                    ACTION_CANCEL,
                    CANCEL_LABEL,
                    CANCEL_DESCRIPTION,
                    view.cancel_enabled,
                ),
            ] {
                Button::builder(button)
                    .parent(ACTIONS_ROW)
                    .label(label)
                    .accessible_description(description)
                    .create(ui)?;
                ui.set(button, ENABLED, enabled)?;
            }
            Text::builder(ACTIONS_STATUS)
                .parent(ACTIONS_ROW)
                .text(view.status.as_str())
                .role(TextRole::Status)
                .create(ui)?;
            Ok(())
        })?;
        self.published = Some(view);
        Ok(())
    }

    /// Publishes what changed in the controls, in one transaction. A pending
    /// intent becomes confirmable from the revision of the commit that first
    /// shows it; one whose confirmation never committed is discarded, so a
    /// client can confirm only a confirmation it can have seen.
    pub fn publish(&mut self, session: &Session) -> Result<(), SessionError> {
        let target = self.view();
        if self.published.is_none() {
            // The controls were never created: nothing can be shown, so
            // nothing may stay confirmable.
            self.pending = None;
            return Ok(());
        }
        // Diffed against what the store holds, not what this gate last
        // wrote, so a control enabled by anything else is put back.
        let published = session.with_store(|store| {
            let property = |node: NodeId, property| {
                store
                    .get_node(node)
                    .and_then(|node| node.get_property(property))
                    .cloned()
            };
            let enabled = |node| property(node, ENABLED) != Some(srui_sdk::Value::Bool(false));
            let text = |node, name| match property(node, name) {
                Some(srui_sdk::Value::String(text)) => text,
                _ => String::new(),
            };
            ActionsView {
                prepare_enabled: enabled(ACTION_PREPARE),
                confirm_enabled: enabled(ACTION_CONFIRM),
                cancel_enabled: enabled(ACTION_CANCEL),
                confirm_label: text(ACTION_CONFIRM, LABEL),
                status: text(ACTIONS_STATUS, TEXT),
            }
        });
        if published == target {
            self.published = Some(target);
            if let Some(pending) = &mut self.pending {
                // Already shown, unchanged: confirmable from now on.
                pending
                    .confirmable_from
                    .get_or_insert(session.current_revision());
            }
            return Ok(());
        }
        let committed = session.transaction_with_result(|ui, _| {
            for (button, was, now) in [
                (
                    ACTION_PREPARE,
                    published.prepare_enabled,
                    target.prepare_enabled,
                ),
                (
                    ACTION_CONFIRM,
                    published.confirm_enabled,
                    target.confirm_enabled,
                ),
                (
                    ACTION_CANCEL,
                    published.cancel_enabled,
                    target.cancel_enabled,
                ),
            ] {
                if was != now {
                    ui.set(button, ENABLED, now)?;
                }
            }
            if published.confirm_label != target.confirm_label {
                ui.set(ACTION_CONFIRM, LABEL, target.confirm_label.as_str())?;
            }
            if published.status != target.status {
                ui.set(ACTIONS_STATUS, TEXT, target.status.as_str())?;
            }
            Ok(())
        });
        match committed {
            Ok(result) => {
                self.published = Some(target);
                if let Some(pending) = &mut self.pending {
                    pending
                        .confirmable_from
                        .get_or_insert(result.transaction.new_revision);
                }
                Ok(())
            }
            Err(error) => {
                if self
                    .pending
                    .as_ref()
                    .is_some_and(|pending| pending.confirmable_from.is_none())
                {
                    self.pending = None;
                }
                Err(error)
            }
        }
    }
}

/// Dispatches one `ACTIVATE` on Prepare (PX-028). Only the event's
/// `client_instance_id`, which the session bound to its connection, is
/// read; its arguments never are.
pub fn prepare_requested(
    gate: &Mutex<ActionGate>,
    view: &Mutex<ProcessView>,
    session: &Session,
    event: &srui_protocol::Event,
) {
    let mut gate = lock_actions(gate);
    {
        let view = lock_view(view);
        // A refusal is reported on the status line, not as an error.
        let _ = gate.prepare(
            &*view,
            &event.client_instance_id,
            ActionOperation::Terminate,
        );
    }
    if let Err(error) = gate.publish(session) {
        eprintln!("srtop: action controls not published: {error}");
    }
}

/// Dispatches one `ACTIVATE` on Confirm (PX-028). Only the event's
/// `client_instance_id` and `observed_revision` are read.
pub fn confirm_requested(
    gate: &Mutex<ActionGate>,
    view: &Mutex<ProcessView>,
    session: &Session,
    event: &srui_protocol::Event,
) {
    let mut gate = lock_actions(gate);
    {
        let view = lock_view(view);
        gate.confirm(&*view, &event.client_instance_id, event.observed_revision);
    }
    if let Err(error) = gate.publish(session) {
        eprintln!("srtop: action controls not published: {error}");
    }
}

/// Dispatches one `ACTIVATE` on Cancel (PX-028). Nothing the event carries
/// is read.
pub fn cancel_requested(gate: &Mutex<ActionGate>, session: &Session) {
    let mut gate = lock_actions(gate);
    gate.cancel();
    if let Err(error) = gate.publish(session) {
        eprintln!("srtop: action controls not published: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::{
        BootId, CreationToken, HostId, MissingReason, PidNamespaceId, SourceId, ThreadIdentity,
    };
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[derive(Debug, Default)]
    struct ManualClock(AtomicU64);

    impl ManualClock {
        fn advance(&self, by: Duration) {
            self.0.fetch_add(by.as_nanos() as u64, Ordering::SeqCst);
        }
    }

    impl MonotonicClock for ManualClock {
        fn now(&self) -> crate::on_demand::SourceTime {
            crate::on_demand::SourceTime(Duration::from_nanos(self.0.load(Ordering::SeqCst)))
        }
    }

    /// Records every intent that reaches it.
    #[derive(Default)]
    struct Recording {
        reached: Mutex<Vec<ActionIntent>>,
        answer: Mutex<Option<Execution>>,
    }

    impl ProcessActions for Recording {
        fn supports(&self, _: ActionOperation) -> bool {
            true
        }

        fn execute(&self, action: &Authorized<'_>) -> Execution {
            self.reached.lock().unwrap().push(action.intent().clone());
            self.answer
                .lock()
                .unwrap()
                .clone()
                .unwrap_or(Execution::Requested)
        }
    }

    fn key(pid: u32, token: &str) -> ProcessKey {
        ProcessKey {
            source: SourceId("actions-unit".into()),
            host: Observed::Known(HostId("host".into())),
            boot: Observed::Known(BootId("boot".into())),
            pid_namespace: Observed::Known(PidNamespaceId(1)),
            pid: Observed::Known(pid),
            creation: CreationToken::Opaque(token.into()),
            thread: None,
        }
    }

    #[derive(Default)]
    struct Fake {
        account: Option<u32>,
        paused: bool,
        selected: Option<ProcessKey>,
        states: HashMap<ProcessKey, TargetState>,
    }

    impl Fake {
        fn current(&mut self, key: &ProcessKey, owner: Observed<u32>) {
            let pid = key.pid.clone();
            self.states.insert(
                key.clone(),
                TargetState::Current {
                    pid,
                    owner,
                    thread: key.is_thread(),
                    who: "PID · worker".into(),
                },
            );
        }
    }

    impl ActionTargets for Fake {
        fn account(&self) -> Observed<u32> {
            match self.account {
                Some(account) => Observed::Known(account),
                None => Observed::Missing(MissingReason::Unavailable),
            }
        }
        fn paused(&self) -> bool {
            self.paused
        }
        fn selected(&self) -> Option<ProcessKey> {
            self.selected.clone()
        }
        fn state_of(&self, key: &ProcessKey) -> TargetState {
            self.states
                .get(key)
                .cloned()
                .unwrap_or(TargetState::Unknown)
        }
    }

    const ME: u32 = 1000;
    const CLIENT: &[u8] = b"unit-client";

    fn gate(recording: &Arc<Recording>, clock: &Arc<ManualClock>) -> ActionGate {
        ActionGate::new(
            ActionMode::DryRun,
            Arc::clone(recording) as Arc<dyn ProcessActions>,
        )
        .with_clock(Arc::clone(clock) as Arc<dyn MonotonicClock>)
    }

    fn mine(pid: u32) -> (Fake, ProcessKey) {
        let target = key(pid, "a");
        let mut fake = Fake {
            account: Some(ME),
            selected: Some(target.clone()),
            ..Fake::default()
        };
        fake.current(&target, Observed::Known(ME));
        (fake, target)
    }

    /// A prepared intent marked published at revision 10, as `publish` does.
    fn prepared(gate: &mut ActionGate, fake: &Fake) -> IntentId {
        let id = gate
            .prepare(fake, CLIENT, ActionOperation::Terminate)
            .expect("prepared");
        gate.pending.as_mut().unwrap().confirmable_from = Some(10);
        id
    }

    #[test]
    fn an_intent_records_exactly_what_was_prepared() {
        let recording = Arc::new(Recording::default());
        let clock = Arc::new(ManualClock::default());
        clock.advance(Duration::from_secs(5));
        let mut gate = gate(&recording, &clock);
        let (fake, target) = mine(4101);
        let id = prepared(&mut gate, &fake);
        let intent = gate.pending().unwrap().clone();
        assert_eq!(intent.id(), id);
        assert_eq!(
            intent.targets().iter().map(|t| t.key()).collect::<Vec<_>>(),
            vec![&target]
        );
        assert_eq!(intent.targets()[0].pid(), 4101);
        assert_eq!(intent.targets()[0].owner(), ME);
        assert_eq!(intent.operation(), ActionOperation::Terminate);
        assert_eq!(intent.parameters().scope(), TargetScope::SelectedProcess);
        assert_eq!(intent.context().account(), ME);
        assert_eq!(intent.context().client_instance_id(), CLIENT);
        assert_eq!(intent.created_at(), Duration::from_secs(5));
        assert_eq!(
            intent.expires_at(),
            Duration::from_secs(5) + DEFAULT_INTENT_LIFETIME
        );
        // A second prepare mints a new, larger ID and replaces the first.
        let second = prepared(&mut gate, &fake);
        assert!(second > id);
        assert_eq!(gate.pending().unwrap().id(), second);
    }

    #[test]
    fn every_ineligible_target_is_refused_and_never_reaches_the_executor() {
        let recording = Arc::new(Recording::default());
        let clock = Arc::new(ManualClock::default());
        type Change = Box<dyn Fn(&mut Fake, &ProcessKey)>;
        let cases: Vec<(&str, Change, Refusal)> = vec![
            (
                "unknown",
                Box::new(|fake, key| {
                    fake.states.remove(key);
                }),
                Refusal::Unknown,
            ),
            (
                "historical",
                Box::new(|fake, key| {
                    fake.states.insert(key.clone(), TargetState::Historical);
                }),
                Refusal::Historical,
            ),
            (
                "unconfirmed",
                Box::new(|fake, key| {
                    fake.states.insert(key.clone(), TargetState::Unconfirmed);
                }),
                Refusal::Unconfirmed,
            ),
            (
                "replaced",
                Box::new(|fake, key| {
                    fake.states.insert(key.clone(), TargetState::Replaced);
                }),
                Refusal::Replaced,
            ),
            (
                "another owner",
                Box::new(|fake, key| fake.current(key, Observed::Known(ME + 1))),
                Refusal::NotOwned {
                    owner: Observed::Known(ME + 1),
                    account: ME,
                },
            ),
            (
                "an unread owner",
                Box::new(|fake, key| fake.current(key, Observed::Missing(MissingReason::Denied))),
                Refusal::NotOwned {
                    owner: Observed::Missing(MissingReason::Denied),
                    account: ME,
                },
            ),
            (
                "paused",
                Box::new(|fake, _| fake.paused = true),
                Refusal::Paused,
            ),
            (
                "account unknown",
                Box::new(|fake, _| fake.account = None),
                Refusal::AccountUnknown,
            ),
        ];
        for (name, change, reason) in cases {
            // At prepare.
            let mut gate = gate(&recording, &clock);
            let (mut fake, target) = mine(4101);
            change(&mut fake, &target);
            assert_eq!(
                gate.prepare(&fake, CLIENT, ActionOperation::Terminate),
                Err(ActionOutcome::Refused {
                    intent: None,
                    reason: reason.clone()
                }),
                "prepare of {name}"
            );
            assert!(gate.pending().is_none(), "{name}");
            // At confirm: prepared while eligible, changed before the click.
            let (mut fake, target) = mine(4101);
            let id = prepared(&mut gate, &fake);
            change(&mut fake, &target);
            assert_eq!(
                gate.confirm(&fake, CLIENT, 10),
                ActionOutcome::Refused {
                    intent: Some(id),
                    reason
                },
                "confirm of {name}"
            );
            assert!(gate.pending().is_none(), "a refused intent is consumed");
        }
        // A thread and a protected PID are refused too.
        let mut gate = gate(&recording, &clock);
        let mut thread = key(4101, "a");
        thread.thread = Some(ThreadIdentity {
            tid: 4102,
            creation: CreationToken::Opaque("t".into()),
        });
        let mut fake = Fake {
            account: Some(ME),
            selected: Some(thread.clone()),
            ..Fake::default()
        };
        fake.current(&thread, Observed::Known(ME));
        assert!(matches!(
            gate.prepare(&fake, CLIENT, ActionOperation::Terminate),
            Err(ActionOutcome::Refused {
                reason: Refusal::NotAProcess,
                ..
            })
        ));
        let (fake, _) = mine(1);
        assert!(matches!(
            gate.prepare(&fake, CLIENT, ActionOperation::Terminate),
            Err(ActionOutcome::Refused {
                reason: Refusal::Protected(1),
                ..
            })
        ));
        let fake = Fake {
            account: Some(ME),
            ..Fake::default()
        };
        assert!(matches!(
            gate.prepare(&fake, CLIENT, ActionOperation::Terminate),
            Err(ActionOutcome::Refused {
                reason: Refusal::NothingSelected,
                ..
            })
        ));
        assert!(
            recording.reached.lock().unwrap().is_empty(),
            "no ineligible target reached the executor"
        );
    }

    #[test]
    fn freshness_is_the_intents_own_and_a_confirm_is_single_use() {
        let recording = Arc::new(Recording::default());
        let clock = Arc::new(ManualClock::default());
        let mut gate = gate(&recording, &clock);
        let (fake, target) = mine(4101);
        // Observed before the confirmation was published: refused.
        let id = prepared(&mut gate, &fake);
        assert_eq!(
            gate.confirm(&fake, CLIENT, 9),
            ActionOutcome::Refused {
                intent: Some(id),
                reason: Refusal::StaleConfirmation {
                    observed: 9,
                    confirmable_from: Some(10)
                }
            }
        );
        // Never published: refused.
        let id = gate
            .prepare(&fake, CLIENT, ActionOperation::Terminate)
            .unwrap();
        assert!(matches!(
            gate.confirm(&fake, CLIENT, 99),
            ActionOutcome::Refused { intent: Some(refused), reason: Refusal::StaleConfirmation { confirmable_from: None, .. } } if refused == id
        ));
        // Another client instance: refused.
        prepared(&mut gate, &fake);
        assert!(matches!(
            gate.confirm(&fake, b"someone-else", 10),
            ActionOutcome::Refused {
                reason: Refusal::OtherClient,
                ..
            }
        ));
        // Just before expiry, at a much later revision: requested.
        let id = prepared(&mut gate, &fake);
        clock.advance(DEFAULT_INTENT_LIFETIME - Duration::from_nanos(1));
        assert_eq!(
            gate.confirm(&fake, CLIENT, 10_000),
            ActionOutcome::Requested {
                intent: id,
                dry_run: true
            }
        );
        // Consumed: a second confirm has nothing to confirm.
        assert!(matches!(
            gate.confirm(&fake, CLIENT, 10_000),
            ActionOutcome::Refused {
                reason: Refusal::NoPendingIntent,
                ..
            }
        ));
        // At expiry exactly: refused.
        prepared(&mut gate, &fake);
        clock.advance(DEFAULT_INTENT_LIFETIME);
        assert!(matches!(
            gate.confirm(&fake, CLIENT, 10),
            ActionOutcome::Refused {
                reason: Refusal::Expired,
                ..
            }
        ));
        // Cancel discards without acting.
        prepared(&mut gate, &fake);
        assert!(gate.cancel().is_some());
        assert!(gate.pending().is_none());
        let reached = recording.reached.lock().unwrap();
        assert_eq!(reached.len(), 1, "only the one valid confirm reached it");
        assert_eq!(reached[0].targets()[0].key(), &target);
    }

    #[test]
    fn the_six_outcomes_stay_distinct_and_read_only_refuses_everything() {
        let recording = Arc::new(Recording::default());
        let clock = Arc::new(ManualClock::default());
        let mut gate = gate(&recording, &clock);
        let (fake, _) = mine(4101);
        for (answer, kind) in [
            (Execution::Requested, "requested"),
            (Execution::Unsupported, "unsupported"),
            (Execution::Failed("denied".into()), "failed"),
            (Execution::OutcomeUnknown("lost".into()), "outcome unknown"),
        ] {
            *recording.answer.lock().unwrap() = Some(answer);
            prepared(&mut gate, &fake);
            assert_eq!(gate.confirm(&fake, CLIENT, 10).kind(), kind);
            let status = gate.view().status;
            assert!(
                !status.to_lowercase().contains("complete"),
                "{kind} is never reported complete: {status}"
            );
        }
        let kinds: BTreeSet<&str> = [
            ActionOutcome::Refused {
                intent: None,
                reason: Refusal::ReadOnly,
            },
            ActionOutcome::Unsupported {
                intent: None,
                operation: ActionOperation::Terminate,
            },
            ActionOutcome::Requested {
                intent: IntentId(1),
                dry_run: true,
            },
            ActionOutcome::ObservedComplete {
                intent: IntentId(1),
            },
            ActionOutcome::Failed {
                intent: IntentId(1),
                reason: String::new(),
            },
            ActionOutcome::OutcomeUnknown {
                intent: IntentId(1),
                reason: String::new(),
            },
        ]
        .iter()
        .map(ActionOutcome::kind)
        .collect();
        assert_eq!(kinds.len(), 6);
        // An executor that does not offer the operation: unsupported at
        // prepare, before any target is examined.
        let mut gate = ActionGate::new(
            ActionMode::DryRun,
            Arc::new(DryRunExecutor::supporting(&[])),
        );
        assert_eq!(
            gate.prepare(&fake, CLIENT, ActionOperation::Terminate),
            Err(ActionOutcome::Unsupported {
                intent: None,
                operation: ActionOperation::Terminate
            })
        );
        // Read-only: refused at prepare and at confirm, controls disabled.
        let reached = recording.reached.lock().unwrap().len();
        let mut gate = ActionGate::new(
            ActionMode::ReadOnly,
            Arc::clone(&recording) as Arc<dyn ProcessActions>,
        );
        assert!(matches!(
            gate.prepare(&fake, CLIENT, ActionOperation::Terminate),
            Err(ActionOutcome::Refused {
                reason: Refusal::ReadOnly,
                ..
            })
        ));
        assert!(matches!(
            gate.confirm(&fake, CLIENT, 10),
            ActionOutcome::Refused {
                reason: Refusal::ReadOnly,
                ..
            }
        ));
        let view = gate.view();
        assert!(!view.prepare_enabled && !view.confirm_enabled && !view.cancel_enabled);
        assert_eq!(recording.reached.lock().unwrap().len(), reached);
    }

    /// The action controls, at their widest, fit the reserve the row ceiling
    /// charges for them in every catch-up snapshot.
    #[test]
    fn the_widest_action_controls_fit_their_snapshot_reserve() {
        use srui_sdk::Operation;
        let session = Session::mint();
        crate::initialize(&session).unwrap();
        let mut gate = ActionGate::new(ActionMode::DryRun, Arc::new(DryRunExecutor::default()));
        gate.create_controls(&session, crate::COLUMN).unwrap();
        session
            .transaction(|ui| {
                ui.set(
                    ACTIONS_STATUS,
                    TEXT,
                    bounded(
                        &"\u{20000}".repeat(MAX_ACTIONS_STATUS_BYTES / 4 + 1),
                        MAX_ACTIONS_STATUS_BYTES,
                    ),
                )?;
                ui.set(
                    ACTION_CONFIRM,
                    LABEL,
                    bounded(
                        &"\u{20000}".repeat(MAX_CONFIRM_LABEL_BYTES / 4 + 1),
                        MAX_CONFIRM_LABEL_BYTES,
                    ),
                )?;
                Ok(())
            })
            .unwrap();
        let encoded: usize = session.with_store(|store| {
            [
                ACTIONS_ROW,
                ACTION_PREPARE,
                ACTION_CONFIRM,
                ACTION_CANCEL,
                ACTIONS_STATUS,
            ]
            .iter()
            .map(|id| {
                let node = store.get_node(*id).unwrap();
                let operation = Operation::create_node(
                    *id,
                    node.node_type,
                    node.parent_id,
                    None,
                    node.properties
                        .iter()
                        .map(|(property, value)| (*property, value.clone())),
                );
                srui_protocol::framed_payload_len(&srui_protocol::SruiMessage {
                    msg: Some(srui_protocol::srui_message::Msg::Transaction(
                        srui_protocol::Transaction {
                            base_revision: 0,
                            new_revision: 0,
                            priority: 0,
                            operations: vec![operation.to_wire()],
                        },
                    )),
                })
            })
            .sum()
        });
        assert!(
            encoded <= crate::refresh::ACTIONS_RESERVE_BYTES,
            "the widest action controls encode to {encoded} bytes"
        );
        assert!(
            encoded > crate::refresh::ACTIONS_RESERVE_BYTES / 4,
            "measured against controls that really are wide: {encoded}"
        );
    }
}
