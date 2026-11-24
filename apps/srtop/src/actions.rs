//! Process action authorization, the dry-run action contract and confirmed
//! graceful termination (D1 §6.2 node identity, §7.7 semantic input routing,
//! §18.2 event deduplication, §27 server security controls; D2 T24, T32;
//! PX-028, PX-030).
//!
//! Everything that decides whether a process may be acted on lives here, on
//! the server:
//!
//! * [`ActionIntent`] is the immutable record of one prepared action: an
//!   intent ID minted by the server, the exact [`ProcessKey`]s it targets, the
//!   operation, its parameters, the account and client instance it was
//!   prepared for, and when it was created and expires. Its fields are
//!   private and it has no setter: confirming acts on what was prepared, never
//!   on the selection the client holds when it confirms.
//! * [`ProcessActions`] is the server-only interface an authorized intent
//!   reaches. It is handed an [`Authorized`], which only [`ActionGate`] can
//!   construct, after it re-checked every target against the latest scan.
//!   Production has exactly two implementations, which PX-008's audit pins:
//!   [`DryRunExecutor`], which sends nothing, and
//!   [`crate::terminate::PidfdTerminator`] (PX-030), which sends SIGTERM
//!   through an identity-checked PID file descriptor ([`crate::handle`]) and
//!   never by PID.
//! * [`ActionOutcome`] keeps the six results apart: refused, unsupported,
//!   requested, observed complete, failed and outcome unknown. An executor
//!   reports only what it did ([`Execution`]); it cannot report an observed
//!   completion, which only a later observation of the process can establish
//!   (PX-031). A sent SIGTERM is *requested*: the process may handle or ignore
//!   it, and whether it exited is not observed here.
//! * Read-only mode is enforced here, at both entry points, not only by the
//!   disabled buttons: a click that reaches a handler in read-only mode is
//!   refused whatever the store says about the button.
//!
//! **The confirmation panel (PX-030).** Prepare ("Terminate…") builds the
//! intent, and the gate publishes an inline confirmation for it from standard
//! widgets: a `Row` holding a `Text` that names the exact process and its
//! owner, and a `Button` that confirms exactly that intent. The three nodes
//! are fresh for every intent and never reused (§6.2): the `n`-th intent of a
//! run (from 0) takes [`PanelIds::of`]`(n)`, IDs `PANEL_ID_BASE + 3n` to
//! `PANEL_ID_BASE + 3n + 2`, and a run shows at most
//! [`DEFAULT_CONFIRMATION_PANELS`] of them; past that Prepare is refused until
//! srtop restarts. The runtime has no way to remove an event handler, so the
//! panel handlers are all registered once, before the socket is served
//! ([`crate::install_actions`]), each bound to its slot: a click on slot `n`
//! can confirm only the intent shown in slot `n`, never a later one. A
//! withdrawn panel is deleted in the commit that withdraws it, so a click
//! still in flight names a node that no longer exists and event validation
//! rejects it (§7.4). Repeated prepare/cancel cycles leave the live node
//! count, the handler table and the handle table as they were.
//!
//! **What withdraws an intent (PX-030).** Cancel; expiry; navigation — the
//! server's resolved selection no longer naming the intent's process
//! instance (another row selected, the selection cleared, or a filter,
//! search or view switch that moves it); read-only mode; and the target no
//! longer being the latest scan's eligible instance (it exited, another
//! process took its PID, its owner changed, it became protected). Confirm
//! checks every one of them itself; the view also asks the gate to
//! [`ActionGate::revalidate`] on a selection change and on every sample, so
//! the panel is withdrawn as soon as it is stale. A paused or resuming view
//! holds the intent, its Confirm disabled, rather than withdrawing it; expiry
//! and navigation still withdraw it.
//!
//! **Protected targets.** PID 1, the namespace's init; any PID configured
//! with `--protect-pid`; and, for this host's live processes, the support
//! chain the server derives from its own process tree ([`SupportChain`],
//! [`crate::support`]): srtop itself and its ancestors, and each connected
//! client's transport (the socket peer, the SSH bridge) and its ancestors
//! (the SSH session serving it). None of this is the only boundary: the
//! owner, identity and freshness checks apply to every target first.
//!
//! Freshness is the intent's own, never the session's revision. A confirm
//! click is accepted when it was observed at or after the revision that
//! published this intent's confirmation (§7.7 `observed_revision`), by the
//! client instance that prepared it, before its expiry, while the selection
//! still names it and every target is still the instance the latest scan
//! confirmed, owned by the account and not protected. Telemetry commits
//! between the two clicks advance the revision and change nothing here. A
//! confirm is single-use: the intent is consumed whatever the result, its
//! panel deleted, and redelivery of the same event is answered from the
//! runtime's dedupe cache without re-running this handler (§18.2).
//!
//! The handlers read no event argument. A click names only its button; the
//! target is the process the server resolved as selected when Prepare was
//! clicked, so no argument a client attaches can widen, narrow or change the
//! target set.

use crate::on_demand::{MonotonicClock, SuspendAwareClock};
use crate::refresh::{lock_view, ProcessView};
use crate::source::{Observed, ProcessKey};
use crate::{bounded, ACTIONS_ROW, ACTIONS_STATUS, ACTION_CANCEL, ACTION_PREPARE};
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
/// The longest confirmation question, before the cut mark (PX-030).
pub const MAX_PANEL_TEXT_BYTES: usize = 512;
/// The longest process description an intent carries.
const MAX_WHO_BYTES: usize = 256;
/// The PIDs no intent may target, whatever the source: PID 1, the
/// namespace's init. `--protect-pid` adds to it, and the live support chain
/// is checked besides (PX-030); this set is never the only authorization
/// boundary.
pub const DEFAULT_PROTECTED_PIDS: [u32; 1] = [1];
/// The most PIDs `--protect-pid` may name (PX-030).
pub const MAX_CONFIGURED_PROTECTED_PIDS: usize = 64;

/// The first node ID of the confirmation panels (PX-030), far above every
/// fixed node of the shell.
pub const PANEL_ID_BASE: u64 = 1_000_000;
/// The nodes one confirmation panel takes: its row, question and Confirm.
pub const PANEL_IDS: u64 = 3;
/// How many confirmation panels one run of srtop shows at most (PX-030):
/// every intent takes fresh node IDs, which the session never reuses (§6.2),
/// and every panel's Confirm handler is registered at start. Past this many
/// prepared intents, Prepare is refused until srtop restarts.
pub const DEFAULT_CONFIRMATION_PANELS: u32 = 4096;

pub const PREPARE_LABEL: &str = "Prepare dry run: terminate selected";
pub const PREPARE_DESCRIPTION: &str =
    "Checks the process selected in the table on the server and prepares a dry-run request for it alone. Nothing is sent to any process.";
/// The prepare button of `--actions terminate` (PX-030).
pub const TERMINATE_LABEL: &str = "Terminate…";
pub const TERMINATE_DESCRIPTION: &str =
    "Checks the process selected in the table on the server and asks you to confirm it by name below. Nothing is sent until you confirm.";
pub const CONFIRM_DESCRIPTION: &str =
    "Confirms the prepared request for the process it names, which the server checks again first.";
pub const CANCEL_LABEL: &str = "Cancel";
pub const CANCEL_DESCRIPTION: &str = "Discards the prepared request without acting on it.";
pub const READ_ONLY_STATUS: &str =
    "Read-only: this server refuses every process action; nothing can be prepared or sent";
pub const DRY_RUN_STATUS: &str =
    "Dry run: Prepare checks the selected process and Confirm records the request; nothing is ever sent to a process";
/// The idle status of `--actions terminate` (PX-030).
pub const TERMINATE_STATUS: &str =
    "Terminate… checks the selected process and asks you to confirm it by name; Confirm sends it SIGTERM, a request it may handle or ignore";
/// The fixed action controls' nodes: the row, Prepare, Cancel and the
/// status. A pending intent adds its panel's [`PANEL_IDS`] nodes.
pub const ACTIONS_NODE_COUNT: usize = 4;

/// Whether this server may act on processes at all (PX-028, PX-030).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionMode {
    /// Every action is refused on the server; the controls are disabled.
    ReadOnly,
    /// Actions are authorized exactly as a real one would be and handed to
    /// the dry-run executor, which sends nothing.
    DryRun,
    /// Actions are authorized and a confirmed one is handed to the real
    /// executor, which sends SIGTERM through a process handle (PX-030).
    Terminate,
}

impl ActionMode {
    /// The `--actions` argument: `read-only`, `dry-run` or `terminate`.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "read-only" => Some(Self::ReadOnly),
            "dry-run" => Some(Self::DryRun),
            "terminate" => Some(Self::Terminate),
            _ => None,
        }
    }
}

/// The three nodes of one confirmation panel (PX-030): slot `n` is
/// `PANEL_ID_BASE + 3n` (the row), `+ 1` (the question) and `+ 2` (Confirm).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PanelIds {
    pub row: NodeId,
    pub question: NodeId,
    pub confirm: NodeId,
}

impl PanelIds {
    pub fn of(slot: u32) -> Self {
        let row = PANEL_ID_BASE + PANEL_IDS * u64::from(slot);
        Self {
            row: NodeId::new(row),
            question: NodeId::new(row + 1),
            confirm: NodeId::new(row + 2),
        }
    }
}

/// What an intent asks for. The only operation is a graceful termination
/// request (SIGTERM); further families are later tickets.
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

    /// "Terminate".
    fn title(self) -> &'static str {
        match self {
            Self::Terminate => "Terminate",
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
/// [`ActionGate::confirm`], [`ActionGate::cancel`] or a withdrawal.
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
        format!("{} {}", self.operation.verb(), self.whom())
    }

    /// "PID 4101 · worker".
    fn whom(&self) -> String {
        let whom: Vec<&str> = self.targets.iter().map(|target| target.who()).collect();
        whom.join(", ")
    }

    /// Whether the server's resolved selection still names this intent's
    /// process (PX-030): navigating away withdraws it.
    fn still_selected(&self, targets: &dyn ActionTargets) -> bool {
        match self.parameters.scope {
            TargetScope::SelectedProcess => {
                let selected = targets.selected();
                self.targets
                    .iter()
                    .all(|target| selected.as_ref() == Some(&target.key))
            }
        }
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
    /// Whether the view was resumed and has not yet published a sample, so
    /// its rows are still the frozen ones, not the latest scan.
    fn resuming(&self) -> bool;
    /// The process instance the server resolved as selected.
    fn selected(&self) -> Option<ProcessKey>;
    /// What the latest scan says about `key`.
    fn state_of(&self, key: &ProcessKey) -> TargetState;
}

/// Which part of the chain that serves this session a process is (PX-030).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupportRole {
    /// srtop itself: the daemon that holds the session.
    ThisServer,
    /// A process srtop descends from.
    ServerAncestor,
    /// A connected client's transport: the process at the other end of its
    /// socket (the SSH bridge).
    ClientTransport,
    /// A process a client's transport descends from (its SSH session).
    TransportAncestor,
}

impl SupportRole {
    fn describe(self) -> &'static str {
        match self {
            Self::ThisServer => "this server (srtop) itself",
            Self::ServerAncestor => "an ancestor of this server",
            Self::ClientTransport => "a connected client's transport (its SSH bridge)",
            Self::TransportAncestor => {
                "an ancestor of a connected client's transport (its SSH session)"
            }
        }
    }
}

/// The processes that serve this session, which no action may target
/// (PX-030). Derived on the server from its own process tree and its own
/// connections, never from anything a client names.
pub trait SupportChain: Send + Sync {
    /// `Err` when `key`, PID `pid`, is part of the chain, or when the chain
    /// cannot be established for the processes `key` belongs to.
    fn protects(&self, key: &ProcessKey, pid: u32) -> Result<(), Refusal>;
}

/// Why an action was refused (PX-028, PX-030).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The server is read-only.
    ReadOnly,
    /// The source does not state the account the server acts as.
    AccountUnknown,
    /// The view is paused, so its rows are not the latest scan.
    Paused,
    /// The view was resumed and waits for its first sample: its rows, and
    /// what they confirm, are still those of before the pause.
    Resuming,
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
    /// The process is protected: PID 1 or a configured PID.
    Protected(u32),
    /// The process serves this session (PX-030).
    SupportChain { pid: u32, role: SupportRole },
    /// The chain that serves this session could not be established, so no
    /// process of this host may be acted on (PX-030).
    SupportChainUnknown(String),
    /// Confirm or Cancel with no prepared intent.
    NoPendingIntent,
    /// A click on a confirmation that is no longer the one shown (PX-030).
    Withdrawn,
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
    /// The server's resolved selection no longer names the intent's process
    /// (PX-030).
    Navigated,
    /// This run has shown every confirmation panel it may (PX-030).
    PanelsExhausted { limit: u32 },
    /// The executor could not hold the process for signalling, so nothing was
    /// sent (PX-030).
    NotHeld(String),
}

impl Refusal {
    pub fn describe(&self) -> String {
        match self {
            Self::ReadOnly => "this server is read-only and refuses every process action".into(),
            Self::AccountUnknown => "the account this server acts as is not known".into(),
            Self::Paused => {
                "the view is paused, so its rows are not the latest scan; resume it first".into()
            }
            Self::Resuming => {
                "the view is resuming: waiting for a fresh sample; its rows are still those of before the pause"
                    .into()
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
            Self::SupportChain { pid, role } => {
                format!("PID {pid} is {}, which serves this session", role.describe())
            }
            Self::SupportChainUnknown(why) => format!(
                "the processes serving this session could not be established ({why}), so no process of this host can be acted on"
            ),
            Self::NoPendingIntent => "nothing is prepared".into(),
            Self::Withdrawn => "this confirmation was withdrawn".into(),
            Self::OtherClient => "another client prepared this request".into(),
            Self::StaleConfirmation { .. } => {
                "the click was made before this request's confirmation was shown".into()
            }
            Self::Expired => "the prepared request expired".into(),
            Self::AccountChanged => "the account changed since the request was prepared".into(),
            Self::Navigated => "the selection moved off the process this request names".into(),
            Self::PanelsExhausted { limit } => format!(
                "this server has shown the {limit} confirmations it allows in one run; restart srtop to prepare more"
            ),
            Self::NotHeld(why) => format!("the process could not be held for signalling: {why}"),
        }
    }
}

/// Why no action can be prepared or confirmed on `targets` now, whatever the
/// target (PX-028): the view is paused, or resumed and still showing the
/// frozen rows until its first sample is applied. `None` when its rows are
/// the latest scan. The one rule the gate checks at Prepare and Confirm and
/// publishes its controls' enabled state by.
pub fn hold_of(targets: &dyn ActionTargets) -> Option<Refusal> {
    if targets.paused() {
        Some(Refusal::Paused)
    } else if targets.resuming() {
        Some(Refusal::Resuming)
    } else {
        None
    }
}

/// What an executor did with an authorized intent. It has no way to say that
/// a process was observed to end: only observation can say that (PX-031).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Execution {
    Requested,
    Unsupported,
    /// The executor refused before sending anything (PX-030): the process
    /// could not be held, or is not the instance confirmed.
    Refused(Refusal),
    Failed(String),
    OutcomeUnknown(String),
}

/// The typed result of one action (PX-028). The six never collapse into one
/// another: a request is not a completion, and a refusal is not a failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionOutcome {
    /// The server refused it; nothing was sent.
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
            Execution::Refused(reason) => Self::Refused {
                intent: Some(intent),
                reason,
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
                "Requested #{}{about}: SIGTERM was sent; the process may handle or ignore it, and whether it exits is not yet observed",
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
    /// Why this executor can act on nothing on this host, when it cannot
    /// (PX-030): the controls are then published disabled with this reason.
    fn unavailable(&self) -> Option<String> {
        None
    }
    /// Acts on `action`, whose every target the gate has just re-checked.
    fn execute(&self, action: &Authorized<'_>) -> Execution;
}

/// The executor that reports each supported request as requested and sends
/// nothing (PX-028). It holds no handle and calls nothing that could reach a
/// process; `--actions dry-run` uses it.
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

/// One confirmation panel as the client holds it (PX-030).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelView {
    pub slot: u32,
    pub question: String,
    pub confirm_label: String,
    pub confirm_enabled: bool,
}

/// The action controls as the client holds them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionsView {
    pub prepare_enabled: bool,
    pub cancel_enabled: bool,
    pub status: String,
    /// The pending intent's confirmation panel, when one is pending.
    pub panel: Option<PanelView>,
}

/// A prepared intent, its panel slot, and the revision that published it.
#[derive(Debug, Clone)]
struct Pending {
    intent: ActionIntent,
    /// The panel this intent alone is confirmed through (PX-030).
    slot: u32,
    /// `None` until the transaction publishing it commits.
    confirmable_from: Option<u64>,
}

/// What the status line reports.
#[derive(Debug, Clone)]
enum Report {
    Idle,
    Prepared,
    Cancelled(IntentId),
    /// Withdrawn before it was confirmed (PX-030).
    Withdrawn {
        intent: IntentId,
        subject: String,
        reason: Refusal,
    },
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
    /// The processes serving this session (PX-030).
    support: Arc<dyn SupportChain>,
    next_intent: u64,
    /// How many confirmation panels this gate may show, and the next unused
    /// one (PX-030). A slot is never used twice.
    panel_slots: u32,
    next_slot: u32,
    pending: Option<Pending>,
    report: Report,
    /// Why the view's rows are not the latest scan, as the view last said
    /// ([`Self::hold_for`]): Prepare and Confirm are published disabled while
    /// this is `Some`. The checks themselves always ask the view again.
    hold: Option<Refusal>,
    /// Where the controls live, once created.
    parent: Option<NodeId>,
    published: Option<ActionsView>,
    /// The slot of the panel this gate last committed to the store.
    shown: Option<u32>,
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
    /// default lifetime, protected PIDs and panel count, and this process's
    /// own support chain (no connection registered).
    pub fn new(mode: ActionMode, executor: Arc<dyn ProcessActions>) -> Self {
        Self {
            mode,
            executor,
            clock: Arc::new(SuspendAwareClock::new()),
            lifetime: DEFAULT_INTENT_LIFETIME,
            protected: DEFAULT_PROTECTED_PIDS.into_iter().collect(),
            support: Arc::new(crate::support::ServerSupportChain::new()),
            next_intent: 1,
            panel_slots: DEFAULT_CONFIRMATION_PANELS,
            next_slot: 0,
            pending: None,
            report: Report::Idle,
            hold: None,
            parent: None,
            published: None,
            shown: None,
        }
    }

    /// Records whether `targets`' rows are the latest scan, so the controls
    /// are published for it. Changes no intent: Prepare and Confirm check
    /// `targets` again themselves.
    pub fn hold_for(&mut self, targets: &dyn ActionTargets) {
        self.hold = hold_of(targets);
    }

    /// Why the controls are on hold, as last recorded.
    pub fn hold(&self) -> Option<&Refusal> {
        self.hold.as_ref()
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

    /// This gate refusing the processes `support` names (PX-030).
    pub fn with_support_chain(mut self, support: Arc<dyn SupportChain>) -> Self {
        self.support = support;
        self
    }

    /// This gate showing at most `slots` confirmation panels (PX-030).
    pub fn with_panel_slots(mut self, slots: u32) -> Self {
        self.panel_slots = slots;
        self
    }

    pub fn mode(&self) -> ActionMode {
        self.mode
    }

    /// How many confirmation panels this gate may show: one Confirm handler
    /// is registered for each (PX-030).
    pub fn panel_slots(&self) -> u32 {
        self.panel_slots
    }

    /// The PIDs this gate protects in every source.
    pub fn protected_pids(&self) -> &BTreeSet<u32> {
        &self.protected
    }

    /// The prepared intent, if any.
    pub fn pending(&self) -> Option<&ActionIntent> {
        self.pending.as_ref().map(|pending| &pending.intent)
    }

    /// The panel the pending intent is confirmed through (PX-030).
    pub fn pending_panel(&self) -> Option<PanelIds> {
        self.pending
            .as_ref()
            .map(|pending| PanelIds::of(pending.slot))
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

    /// Why the latest intent was withdrawn, when it was (PX-030).
    pub fn withdrawn(&self) -> Option<(IntentId, &Refusal)> {
        match &self.report {
            Report::Withdrawn { intent, reason, .. } => Some((*intent, reason)),
            _ => None,
        }
    }

    /// Read-only mode, checked by the server at every entry point.
    fn writable(&self) -> Result<(), Refusal> {
        match self.mode {
            ActionMode::ReadOnly => Err(Refusal::ReadOnly),
            ActionMode::DryRun | ActionMode::Terminate => Ok(()),
        }
    }

    /// Admits `key` as a target for `account`: the latest scan's own
    /// instance, a process, with a known PID, owned by `account`, not
    /// protected and not part of the chain serving this session.
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
        self.support.protects(key, pid)?;
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
        // A new Prepare replaces the old intent whatever its result: a
        // refused one leaves nothing pending, and the old panel goes with the
        // next publication.
        self.pending = None;
        let prepared = self.prepare_intent(targets, client, operation);
        match prepared {
            Ok((intent, slot)) => {
                let id = intent.id;
                self.pending = Some(Pending {
                    intent,
                    slot,
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
    ) -> Result<(ActionIntent, u32), ActionOutcome> {
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
        if let Some(hold) = hold_of(targets) {
            return Err(refused(hold));
        }
        let key = targets
            .selected()
            .ok_or_else(|| refused(Refusal::NothingSelected))?;
        let target = self.admit(targets, &key, account).map_err(refused)?;
        if self.next_slot >= self.panel_slots {
            return Err(refused(Refusal::PanelsExhausted {
                limit: self.panel_slots,
            }));
        }
        // Fresh node IDs for this intent's panel, never used again (§6.2).
        let slot = self.next_slot;
        self.next_slot += 1;
        let id = IntentId(self.next_intent);
        self.next_intent += 1;
        let created_at = self.clock.now().0;
        Ok((
            ActionIntent {
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
            },
            slot,
        ))
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

    /// A click on the Confirm of panel `slot` (PX-030): it confirms the
    /// pending intent only when that intent is the one shown in `slot`. Any
    /// other click is refused and changes nothing: the pending intent, if
    /// any, is another panel's.
    pub fn confirm_panel(
        &mut self,
        targets: &dyn ActionTargets,
        client: &[u8],
        observed: u64,
        slot: u32,
    ) -> ActionOutcome {
        if self.pending.as_ref().map(|pending| pending.slot) != Some(slot) {
            return ActionOutcome::Refused {
                intent: None,
                reason: Refusal::Withdrawn,
            };
        }
        self.confirm(targets, client, observed)
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
        // Navigation withdraws an intent (PX-030): confirming acts only on
        // the process the selection still names.
        if !intent.still_selected(targets) {
            return Err(Refusal::Navigated);
        }
        if let Some(hold) = hold_of(targets) {
            return Err(hold);
        }
        for target in &intent.targets {
            self.admit(targets, &target.key, intent.context.account)?;
        }
        Ok(intent)
    }

    /// Withdraws the pending intent when it can no longer be confirmed
    /// (PX-030): expired, navigated away from, the server read-only, or its
    /// target no longer the latest scan's eligible instance. A paused or
    /// resuming view keeps it on hold instead, unless it expired or the
    /// selection moved. The panel goes with the next publication.
    pub fn revalidate(&mut self, targets: &dyn ActionTargets) {
        let Some(pending) = &self.pending else {
            return;
        };
        let Some(reason) = self.withdrawal(&pending.intent, targets) else {
            return;
        };
        if let Some(pending) = self.pending.take() {
            self.report = Report::Withdrawn {
                intent: pending.intent.id,
                subject: pending.intent.subject(),
                reason,
            };
        }
    }

    /// Why `intent` can no longer be confirmed, if it cannot.
    fn withdrawal(&self, intent: &ActionIntent, targets: &dyn ActionTargets) -> Option<Refusal> {
        if let Err(reason) = self.writable() {
            return Some(reason);
        }
        if self.clock.now().0 >= intent.expires_at {
            return Some(Refusal::Expired);
        }
        if !intent.still_selected(targets) {
            return Some(Refusal::Navigated);
        }
        if hold_of(targets).is_some() {
            return None;
        }
        match Self::account(targets) {
            Err(reason) => return Some(reason),
            Ok(account) if account != intent.context.account => {
                return Some(Refusal::AccountChanged)
            }
            Ok(_) => {}
        }
        intent.targets.iter().find_map(|target| {
            self.admit(targets, &target.key, intent.context.account)
                .err()
        })
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

    /// The prepare button's label in this mode.
    pub fn prepare_label(&self) -> &'static str {
        match self.mode {
            ActionMode::Terminate => TERMINATE_LABEL,
            ActionMode::ReadOnly | ActionMode::DryRun => PREPARE_LABEL,
        }
    }

    fn prepare_description(&self) -> &'static str {
        match self.mode {
            ActionMode::Terminate => TERMINATE_DESCRIPTION,
            ActionMode::ReadOnly | ActionMode::DryRun => PREPARE_DESCRIPTION,
        }
    }

    /// The question the panel asks about `intent`: the exact process and
    /// its owner, and what Confirm does.
    fn question(&self, intent: &ActionIntent) -> String {
        let owner = intent
            .targets
            .first()
            .map_or(String::new(), |target| format!(" (UID {})", target.owner));
        match self.mode {
            ActionMode::DryRun | ActionMode::ReadOnly => format!(
                "Dry run: {} {}{owner}? Confirm records the request; nothing is sent to the process.",
                intent.operation.verb(),
                intent.whom()
            ),
            ActionMode::Terminate => format!(
                "{} {}{owner}? Confirm sends it SIGTERM, which asks it to exit; it may handle or ignore it.",
                intent.operation.title(),
                intent.whom()
            ),
        }
    }

    fn confirm_label(&self, intent: &ActionIntent) -> String {
        match self.mode {
            ActionMode::DryRun | ActionMode::ReadOnly => {
                format!("Confirm dry run: {}", intent.subject())
            }
            ActionMode::Terminate => format!("Confirm: {}", intent.subject()),
        }
    }

    /// The controls this gate's state calls for.
    pub fn view(&self) -> ActionsView {
        let writable = self.writable().is_ok();
        let unavailable = self.executor.unavailable();
        let pending = self.pending.as_ref();
        let dry_run = if self.mode == ActionMode::DryRun {
            " (dry run)"
        } else {
            ""
        };
        let status = match (&self.report, pending) {
            (Report::Prepared, Some(pending)) if self.hold.is_some() => format!(
                "Prepared #{}: {}{dry_run}, on hold: {}; Confirm is unavailable until it is current, and Cancel discards it",
                pending.intent.id.get(),
                pending.intent.subject(),
                self.hold.as_ref().map(Refusal::describe).unwrap_or_default()
            ),
            (Report::Prepared, Some(pending)) => format!(
                "Prepared #{}: {}{dry_run}. Confirm within {} s or Cancel; only this process can be acted on, and selecting another withdraws it",
                pending.intent.id.get(),
                pending.intent.subject(),
                self.lifetime.as_secs()
            ),
            (Report::Cancelled(id), _) => {
                format!("Cancelled #{}: nothing was requested", id.get())
            }
            (
                Report::Withdrawn {
                    intent,
                    subject,
                    reason,
                },
                _,
            ) => format!(
                "Withdrawn #{} ({subject}): {}; nothing was requested",
                intent.get(),
                reason.describe()
            ),
            (Report::Outcome { outcome, subject }, _) => outcome.describe(subject.as_deref()),
            _ if writable => match (&unavailable, &self.hold) {
                (Some(why), _) => format!(
                    "{} unavailable: {why}; nothing can be sent to any process",
                    ActionOperation::Terminate.title()
                ),
                (None, Some(hold)) => format!("Actions on hold: {}", hold.describe()),
                (None, None) if self.mode == ActionMode::Terminate => TERMINATE_STATUS.to_string(),
                (None, None) => DRY_RUN_STATUS.to_string(),
            },
            _ => READ_ONLY_STATUS.to_string(),
        };
        // Paused or resuming, nothing can be prepared or confirmed: the rows
        // are not the latest scan. Cancel only discards, so it stays.
        let actionable = writable && self.hold.is_none() && unavailable.is_none();
        ActionsView {
            prepare_enabled: actionable,
            cancel_enabled: writable && pending.is_some(),
            status: bounded(&status, MAX_ACTIONS_STATUS_BYTES),
            panel: pending.map(|pending| PanelView {
                slot: pending.slot,
                question: bounded(&self.question(&pending.intent), MAX_PANEL_TEXT_BYTES),
                confirm_label: bounded(
                    &self.confirm_label(&pending.intent),
                    MAX_CONFIRM_LABEL_BYTES,
                ),
                confirm_enabled: actionable,
            }),
        }
    }

    /// Creates the fixed action controls under `parent`, in one transaction,
    /// as this gate's state calls for. Confirmation panels are created under
    /// the same parent, after them, as intents are prepared.
    pub fn create_controls(
        &mut self,
        session: &Session,
        parent: NodeId,
    ) -> Result<(), SessionError> {
        let view = self.view();
        let (prepare_label, prepare_description) =
            (self.prepare_label(), self.prepare_description());
        session.transaction(|ui| {
            Row::builder(ACTIONS_ROW)
                .parent(parent)
                .spacing_role(SpacingRole::Normal)
                .create(ui)?;
            for (button, label, description, enabled) in [
                (
                    ACTION_PREPARE,
                    prepare_label,
                    prepare_description,
                    view.prepare_enabled,
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
            if let Some(panel) = &view.panel {
                create_panel(ui, parent, panel)?;
            }
            Ok(())
        })?;
        self.parent = Some(parent);
        self.shown = view.panel.as_ref().map(|panel| panel.slot);
        self.published = Some(view);
        Ok(())
    }

    /// Publishes what changed in the controls, in one transaction. A pending
    /// intent becomes confirmable from the revision of the commit that first
    /// shows its panel; one whose panel never committed is discarded, so a
    /// client can confirm only a confirmation it can have seen. A withdrawn,
    /// cancelled or confirmed intent's panel is deleted in the same commit.
    pub fn publish(&mut self, session: &Session) -> Result<(), SessionError> {
        let target = self.view();
        self.publish_view(session, target)
    }

    /// [`Self::publish`], only when the controls this gate's state calls for
    /// differ from the ones it last published, or that publication failed:
    /// what a sample's commit asks for, so an unchanged gate reads nothing.
    pub fn publish_if_changed(&mut self, session: &Session) -> Result<(), SessionError> {
        let target = self.view();
        if self.published.as_ref() == Some(&target) {
            return Ok(());
        }
        self.publish_view(session, target)
    }

    fn publish_view(&mut self, session: &Session, target: ActionsView) -> Result<(), SessionError> {
        let Some(parent) = self.parent else {
            // The controls were never created: nothing can be shown, so
            // nothing may stay confirmable.
            self.pending = None;
            return Ok(());
        };
        let shown = self.shown;
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
                cancel_enabled: enabled(ACTION_CANCEL),
                status: text(ACTIONS_STATUS, TEXT),
                panel: shown.and_then(|slot| {
                    let ids = PanelIds::of(slot);
                    store.get_node(ids.row)?;
                    Some(PanelView {
                        slot,
                        question: text(ids.question, TEXT),
                        confirm_label: text(ids.confirm, LABEL),
                        confirm_enabled: enabled(ids.confirm),
                    })
                }),
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
                    ACTION_CANCEL,
                    published.cancel_enabled,
                    target.cancel_enabled,
                ),
            ] {
                if was != now {
                    ui.set(button, ENABLED, now)?;
                }
            }
            if published.status != target.status {
                ui.set(ACTIONS_STATUS, TEXT, target.status.as_str())?;
            }
            match (&published.panel, &target.panel) {
                (Some(was), Some(now)) if was.slot == now.slot => {
                    let ids = PanelIds::of(now.slot);
                    if was.confirm_enabled != now.confirm_enabled {
                        ui.set(ids.confirm, ENABLED, now.confirm_enabled)?;
                    }
                    if was.confirm_label != now.confirm_label {
                        ui.set(ids.confirm, LABEL, now.confirm_label.as_str())?;
                    }
                    if was.question != now.question {
                        ui.set(ids.question, TEXT, now.question.as_str())?;
                    }
                }
                (was, now) => {
                    // Another intent, or none: the old panel goes, with its
                    // button, and a new one gets fresh node IDs.
                    if let Some(was) = was {
                        ui.delete(PanelIds::of(was.slot).row)?;
                    }
                    if let Some(now) = now {
                        create_panel(ui, parent, now)?;
                    }
                }
            }
            Ok(())
        });
        match committed {
            Ok(result) => {
                self.shown = target.panel.as_ref().map(|panel| panel.slot);
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

/// Creates one confirmation panel (PX-030) under `parent`: a `Row` with the
/// question naming the process and the Confirm button bound to its slot.
fn create_panel(
    ui: &mut srui_sdk::UiTransaction,
    parent: NodeId,
    panel: &PanelView,
) -> Result<(), srui_semantic_tree::StoreError> {
    let ids = PanelIds::of(panel.slot);
    Row::builder(ids.row)
        .parent(parent)
        .spacing_role(SpacingRole::Normal)
        .create(ui)?;
    Text::builder(ids.question)
        .parent(ids.row)
        .text(panel.question.as_str())
        .role(TextRole::Warning)
        .create(ui)?;
    Button::builder(ids.confirm)
        .parent(ids.row)
        .label(panel.confirm_label.as_str())
        .accessible_description(CONFIRM_DESCRIPTION)
        .create(ui)?;
    ui.set(ids.confirm, ENABLED, panel.confirm_enabled)?;
    Ok(())
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
    // The view, then the gate: the order every holder of both takes them.
    let view = lock_view(view);
    let mut gate = lock_actions(gate);
    // A refusal is reported on the status line, not as an error.
    let _ = gate.prepare(
        &*view,
        &event.client_instance_id,
        ActionOperation::Terminate,
    );
    gate.hold_for(&*view);
    if let Err(error) = gate.publish(session) {
        eprintln!("srtop: action controls not published: {error}");
    }
}

/// Dispatches one `ACTIVATE` on the Confirm of panel `slot` (PX-028,
/// PX-030). Only the event's `client_instance_id` and `observed_revision`
/// are read.
pub fn confirm_requested(
    gate: &Mutex<ActionGate>,
    view: &Mutex<ProcessView>,
    session: &Session,
    event: &srui_protocol::Event,
    slot: u32,
) {
    // The view, then the gate: the order every holder of both takes them.
    let view = lock_view(view);
    let mut gate = lock_actions(gate);
    gate.confirm_panel(
        &*view,
        &event.client_instance_id,
        event.observed_revision,
        slot,
    );
    gate.hold_for(&*view);
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
        resuming: bool,
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
        fn resuming(&self) -> bool {
            self.resuming
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
                "resuming",
                Box::new(|fake, _| fake.resuming = true),
                Refusal::Resuming,
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
        assert!(!view.prepare_enabled && !view.cancel_enabled && view.panel.is_none());
        assert_eq!(recording.reached.lock().unwrap().len(), reached);
    }

    #[test]
    fn a_failed_publication_of_the_controls_is_retried_by_the_next_call() {
        // PX-028 round 2 verification (V6). No transient store failure can
        // be made in one session: a deleted node's ID is never reused
        // (§6.2) and store limits are fixed per session. So the failing
        // commit goes to a session without the controls, through the real
        // `publish_if_changed` and its error branch, and the next call, with
        // the gate's state unchanged, goes to the session that has them.
        let session = Session::mint();
        crate::initialize(&session).unwrap();
        let recording = Arc::new(Recording::default());
        let clock = Arc::new(ManualClock::default());
        let mut gate = gate(&recording, &clock);
        gate.create_controls(&session, crate::COLUMN).unwrap();
        let enabled = |session: &Session, node: NodeId| {
            session.with_store(|store| {
                store
                    .get_node(node)
                    .and_then(|node| node.get_property(ENABLED))
                    .cloned()
            }) != Some(srui_sdk::Value::Bool(false))
        };
        assert!(enabled(&session, ACTION_PREPARE));
        // A prepared intent's panel is shown, its Confirm enabled.
        let (mut fake, _) = mine(4101);
        gate.prepare(&fake, CLIENT, ActionOperation::Terminate)
            .unwrap();
        gate.publish(&session).unwrap();
        let panel = gate.pending_panel().unwrap();
        assert!(enabled(&session, panel.confirm));
        fake.resuming = true;
        gate.hold_for(&fake);
        let elsewhere = Session::mint();
        assert!(
            gate.publish_if_changed(&elsewhere).is_err(),
            "the controls do not exist there"
        );
        assert!(enabled(&session, ACTION_PREPARE), "nothing reached it yet");
        assert!(gate.pending().is_some(), "a shown intent is kept");
        let before = session.current_revision();
        gate.hold_for(&fake);
        gate.publish_if_changed(&session).unwrap();
        assert!(session.current_revision() > before, "retried");
        assert!(!enabled(&session, ACTION_PREPARE));
        assert!(!enabled(&session, panel.confirm));
        // Published now: an unchanged gate commits nothing more.
        let after = session.current_revision();
        gate.publish_if_changed(&session).unwrap();
        assert_eq!(session.current_revision(), after);
    }

    /// The action controls, at their widest, fit the reserve the row ceiling
    /// charges for them in every catch-up snapshot: the fixed controls and a
    /// confirmation panel (PX-030), each text at its bound with the cut mark.
    #[test]
    fn the_widest_action_controls_fit_their_snapshot_reserve() {
        use srui_sdk::Operation;
        let session = Session::mint();
        crate::initialize(&session).unwrap();
        let mut gate = ActionGate::new(ActionMode::Terminate, Arc::new(DryRunExecutor::default()));
        gate.create_controls(&session, crate::COLUMN).unwrap();
        let (fake, _) = mine(4101);
        gate.prepare(&fake, CLIENT, ActionOperation::Terminate)
            .unwrap();
        gate.publish(&session).unwrap();
        let panel = gate.pending_panel().unwrap();
        let wide = |limit: usize| bounded(&"\u{20000}".repeat(limit / 4 + 1), limit);
        session
            .transaction(|ui| {
                ui.set(ACTIONS_STATUS, TEXT, wide(MAX_ACTIONS_STATUS_BYTES))?;
                ui.set(panel.question, TEXT, wide(MAX_PANEL_TEXT_BYTES))?;
                ui.set(panel.confirm, LABEL, wide(MAX_CONFIRM_LABEL_BYTES))?;
                Ok(())
            })
            .unwrap();
        let encoded: usize = session.with_store(|store| {
            [
                ACTIONS_ROW,
                ACTION_PREPARE,
                ACTION_CANCEL,
                ACTIONS_STATUS,
                panel.row,
                panel.question,
                panel.confirm,
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
            encoded > crate::refresh::ACTIONS_RESERVE_BYTES / 2,
            "measured against controls that really are wide: {encoded}"
        );
    }

    // ----------------------------------------------------------------------
    // PX-030
    // ----------------------------------------------------------------------

    #[test]
    fn confirm_acts_only_while_the_selection_names_the_intents_process() {
        let recording = Arc::new(Recording::default());
        let clock = Arc::new(ManualClock::default());
        let mut gate = gate(&recording, &clock);
        let (mut fake, a) = mine(4101);
        let b = key(4103, "b");
        fake.current(&b, Observed::Known(ME));
        // Prepared for A; B is selected; Confirm: refused, nothing reached.
        let id = prepared(&mut gate, &fake);
        fake.selected = Some(b.clone());
        assert_eq!(
            gate.confirm(&fake, CLIENT, 10),
            ActionOutcome::Refused {
                intent: Some(id),
                reason: Refusal::Navigated
            }
        );
        // The selection cleared: refused too.
        fake.selected = Some(a.clone());
        prepared(&mut gate, &fake);
        fake.selected = None;
        assert!(matches!(
            gate.confirm(&fake, CLIENT, 10),
            ActionOutcome::Refused {
                reason: Refusal::Navigated,
                ..
            }
        ));
        // Back on A: only A is ever acted on.
        fake.selected = Some(a.clone());
        prepared(&mut gate, &fake);
        assert!(matches!(
            gate.confirm(&fake, CLIENT, 10),
            ActionOutcome::Requested { .. }
        ));
        let reached = recording.reached.lock().unwrap();
        assert_eq!(reached.len(), 1);
        assert_eq!(reached[0].targets()[0].key(), &a);
    }

    #[test]
    fn revalidation_withdraws_a_stale_intent_and_holds_one_while_paused() {
        let recording = Arc::new(Recording::default());
        let clock = Arc::new(ManualClock::default());
        type Change = Box<dyn Fn(&mut Fake, &ProcessKey, &ManualClock)>;
        let cases: Vec<(&str, Change, Refusal)> = vec![
            (
                "navigated",
                Box::new(|fake, _, _| fake.selected = Some(key(4103, "b"))),
                Refusal::Navigated,
            ),
            (
                "expired",
                Box::new(|_, _, clock| clock.advance(DEFAULT_INTENT_LIFETIME)),
                Refusal::Expired,
            ),
            (
                "replaced",
                Box::new(|fake, key, _| {
                    fake.states.insert(key.clone(), TargetState::Replaced);
                }),
                Refusal::Replaced,
            ),
            (
                "exited",
                Box::new(|fake, key, _| {
                    fake.states.insert(key.clone(), TargetState::Historical);
                }),
                Refusal::Historical,
            ),
            (
                "owner changed",
                Box::new(|fake, key, _| fake.current(key, Observed::Known(ME + 1))),
                Refusal::NotOwned {
                    owner: Observed::Known(ME + 1),
                    account: ME,
                },
            ),
        ];
        for (name, change, reason) in cases {
            let mut gate = gate(&recording, &clock);
            let (mut fake, target) = mine(4101);
            let id = prepared(&mut gate, &fake);
            gate.revalidate(&fake);
            assert_eq!(gate.pending().map(ActionIntent::id), Some(id), "{name}");
            change(&mut fake, &target, &clock);
            gate.revalidate(&fake);
            assert!(gate.pending().is_none(), "{name} withdraws it");
            assert_eq!(gate.withdrawn(), Some((id, &reason)), "{name}");
            let view = gate.view();
            assert!(view.panel.is_none(), "{name}: the panel goes");
            assert!(
                view.status.starts_with(&format!("Withdrawn #{}", id.get()))
                    && view.status.ends_with("nothing was requested"),
                "{name}: {}",
                view.status
            );
        }
        // Paused: held, not withdrawn; Confirm disabled; still navigation
        // and expiry withdraw it.
        let mut gate = gate(&recording, &clock);
        let (mut fake, target) = mine(4101);
        prepared(&mut gate, &fake);
        fake.paused = true;
        fake.states.insert(target.clone(), TargetState::Unconfirmed);
        gate.hold_for(&fake);
        gate.revalidate(&fake);
        assert!(gate.pending().is_some(), "held while paused");
        assert!(!gate.view().panel.unwrap().confirm_enabled);
        fake.selected = None;
        gate.revalidate(&fake);
        assert_eq!(
            gate.withdrawn().map(|(_, why)| why),
            Some(&Refusal::Navigated)
        );
        assert!(recording.reached.lock().unwrap().is_empty());
    }

    #[test]
    fn a_panels_confirm_confirms_only_the_intent_shown_in_its_slot() {
        let recording = Arc::new(Recording::default());
        let clock = Arc::new(ManualClock::default());
        let mut gate = gate(&recording, &clock);
        let (fake, _) = mine(4101);
        prepared(&mut gate, &fake);
        let first = gate.pending_panel().unwrap();
        let second_id = prepared(&mut gate, &fake);
        let second = gate.pending_panel().unwrap();
        assert_ne!(first, second, "a new intent takes fresh node IDs");
        // The first panel's Confirm is not the second's: refused, and the
        // second stays pending.
        assert_eq!(
            gate.confirm_panel(&fake, CLIENT, 10, 0),
            ActionOutcome::Refused {
                intent: None,
                reason: Refusal::Withdrawn
            }
        );
        assert_eq!(gate.pending().map(ActionIntent::id), Some(second_id));
        assert!(recording.reached.lock().unwrap().is_empty());
        assert!(matches!(
            gate.confirm_panel(&fake, CLIENT, 10, 1),
            ActionOutcome::Requested { intent, .. } if intent == second_id
        ));
        // Consumed: the same slot again confirms nothing.
        assert!(matches!(
            gate.confirm_panel(&fake, CLIENT, 10, 1),
            ActionOutcome::Refused {
                reason: Refusal::Withdrawn,
                ..
            }
        ));
        assert_eq!(recording.reached.lock().unwrap().len(), 1);
    }

    /// PX-030 verification round 1 (F4): a new Prepare replaces the pending
    /// intent whatever its result. A refused one leaves nothing pending, and
    /// no panel, even where the old intent would otherwise be kept (a paused
    /// view holds a pending intent; the selection has not moved).
    #[test]
    fn a_refused_prepare_leaves_no_earlier_intent_pending() {
        let recording = Arc::new(Recording::default());
        let clock = Arc::new(ManualClock::default());
        let mut gate = gate(&recording, &clock);
        let (mut fake, _) = mine(4101);
        let first = prepared(&mut gate, &fake);
        fake.paused = true;
        gate.hold_for(&fake);
        gate.revalidate(&fake);
        assert_eq!(gate.pending().map(ActionIntent::id), Some(first), "held");
        assert!(matches!(
            gate.prepare(&fake, CLIENT, ActionOperation::Terminate),
            Err(ActionOutcome::Refused {
                reason: Refusal::Paused,
                ..
            })
        ));
        assert!(gate.pending().is_none(), "the refused Prepare replaced it");
        assert!(gate.view().panel.is_none());
        // The old panel's slot confirms nothing.
        fake.paused = false;
        assert!(matches!(
            gate.confirm_panel(&fake, CLIENT, 10, 0),
            ActionOutcome::Refused {
                reason: Refusal::Withdrawn,
                ..
            }
        ));
        assert!(recording.reached.lock().unwrap().is_empty());
    }

    #[test]
    fn panel_ids_are_fresh_bounded_and_clear_of_the_shell() {
        let recording = Arc::new(Recording::default());
        let clock = Arc::new(ManualClock::default());
        let mut gate = gate(&recording, &clock).with_panel_slots(3);
        let (fake, _) = mine(4101);
        let mut seen = BTreeSet::new();
        for _ in 0..3 {
            prepared(&mut gate, &fake);
            let ids = gate.pending_panel().unwrap();
            for id in [ids.row, ids.question, ids.confirm] {
                assert!(id.0 >= PANEL_ID_BASE, "{id:?} is clear of the shell");
                assert!(seen.insert(id.0), "{id:?} is never reused");
            }
            assert!(gate.cancel().is_some());
        }
        assert!(matches!(
            gate.prepare(&fake, CLIENT, ActionOperation::Terminate),
            Err(ActionOutcome::Refused {
                reason: Refusal::PanelsExhausted { limit: 3 },
                ..
            })
        ));
        assert!(gate.view().status.contains("restart srtop"));
        // The default bound's last panel ends where the scheme says.
        let last = PanelIds::of(DEFAULT_CONFIRMATION_PANELS - 1);
        assert_eq!(
            last.confirm.0,
            PANEL_ID_BASE + PANEL_IDS * u64::from(DEFAULT_CONFIRMATION_PANELS) - 1
        );
        for fixed in [ACTIONS_ROW, ACTION_PREPARE, ACTION_CANCEL, ACTIONS_STATUS] {
            assert!(fixed.0 < PANEL_ID_BASE);
        }
    }

    /// A support chain that names fixed PIDs, for any source.
    struct Chain(Mutex<Result<Vec<(u32, SupportRole)>, String>>);

    impl SupportChain for Chain {
        fn protects(&self, _: &ProcessKey, pid: u32) -> Result<(), Refusal> {
            match &*self.0.lock().unwrap() {
                Err(why) => Err(Refusal::SupportChainUnknown(why.clone())),
                Ok(members) => match members.iter().find(|(member, _)| *member == pid) {
                    Some((pid, role)) => Err(Refusal::SupportChain {
                        pid: *pid,
                        role: *role,
                    }),
                    None => Ok(()),
                },
            }
        }
    }

    #[test]
    fn the_support_chain_and_configured_pids_are_refused_at_prepare_and_confirm() {
        let recording = Arc::new(Recording::default());
        let clock = Arc::new(ManualClock::default());
        let chain = Arc::new(Chain(Mutex::new(Ok(vec![(
            4102,
            SupportRole::ClientTransport,
        )]))));
        let mut gate = gate(&recording, &clock)
            .with_protected_pids([1, 4104])
            .with_support_chain(Arc::clone(&chain) as Arc<dyn SupportChain>);
        let (fake, _) = mine(4102);
        assert_eq!(
            gate.prepare(&fake, CLIENT, ActionOperation::Terminate),
            Err(ActionOutcome::Refused {
                intent: None,
                reason: Refusal::SupportChain {
                    pid: 4102,
                    role: SupportRole::ClientTransport
                }
            })
        );
        assert!(gate
            .view()
            .status
            .contains("PID 4102 is a connected client's transport (its SSH bridge)"));
        let (fake, _) = mine(4104);
        assert!(matches!(
            gate.prepare(&fake, CLIENT, ActionOperation::Terminate),
            Err(ActionOutcome::Refused {
                reason: Refusal::Protected(4104),
                ..
            })
        ));
        // Prepared while not in the chain; a client connects whose transport
        // it is before Confirm: refused there.
        let (fake, _) = mine(4101);
        let id = prepared(&mut gate, &fake);
        *chain.0.lock().unwrap() = Ok(vec![(4101, SupportRole::TransportAncestor)]);
        assert_eq!(
            gate.confirm(&fake, CLIENT, 10),
            ActionOutcome::Refused {
                intent: Some(id),
                reason: Refusal::SupportChain {
                    pid: 4101,
                    role: SupportRole::TransportAncestor
                }
            }
        );
        // A chain that cannot be read refuses everything.
        *chain.0.lock().unwrap() = Err("unreadable".into());
        assert!(matches!(
            gate.prepare(&fake, CLIENT, ActionOperation::Terminate),
            Err(ActionOutcome::Refused {
                reason: Refusal::SupportChainUnknown(_),
                ..
            })
        ));
        // The owner check comes first: the chain is not the only boundary.
        *chain.0.lock().unwrap() = Ok(Vec::new());
        let mut other = Fake {
            account: Some(ME),
            ..Fake::default()
        };
        let theirs = key(4105, "t");
        other.current(&theirs, Observed::Known(ME + 1));
        other.selected = Some(theirs);
        assert!(matches!(
            gate.prepare(&other, CLIENT, ActionOperation::Terminate),
            Err(ActionOutcome::Refused {
                reason: Refusal::NotOwned { .. },
                ..
            })
        ));
        assert!(recording.reached.lock().unwrap().is_empty());
    }

    /// An executor that cannot act on this host (PX-030).
    struct Unavailable;

    impl ProcessActions for Unavailable {
        fn supports(&self, _: ActionOperation) -> bool {
            false
        }
        fn unavailable(&self) -> Option<String> {
            Some("this system has no PID file descriptors".into())
        }
        fn execute(&self, _: &Authorized<'_>) -> Execution {
            panic!("an unavailable executor is never reached")
        }
    }

    #[test]
    fn an_unavailable_executor_keeps_terminate_disabled_with_its_reason() {
        let mut gate = ActionGate::new(ActionMode::Terminate, Arc::new(Unavailable));
        let view = gate.view();
        assert!(!view.prepare_enabled && !view.cancel_enabled);
        assert_eq!(
            view.status,
            "Terminate unavailable: this system has no PID file descriptors; nothing can be sent to any process"
        );
        assert_eq!(gate.prepare_label(), TERMINATE_LABEL);
        let (fake, _) = mine(4101);
        assert_eq!(
            gate.prepare(&fake, CLIENT, ActionOperation::Terminate),
            Err(ActionOutcome::Unsupported {
                intent: None,
                operation: ActionOperation::Terminate
            })
        );
        assert!(gate.pending().is_none());
    }

    #[test]
    fn a_terminate_gate_names_the_process_and_reports_a_sent_signal_as_requested() {
        let recording = Arc::new(Recording::default());
        let clock = Arc::new(ManualClock::default());
        let mut gate = ActionGate::new(
            ActionMode::Terminate,
            Arc::clone(&recording) as Arc<dyn ProcessActions>,
        )
        .with_clock(Arc::clone(&clock) as Arc<dyn MonotonicClock>);
        let (fake, _) = mine(4101);
        let id = prepared(&mut gate, &fake);
        let view = gate.view();
        let panel = view.panel.expect("a panel");
        assert_eq!(
            panel.question,
            "Terminate PID · worker (UID 1000)? Confirm sends it SIGTERM, which asks it to exit; it may handle or ignore it."
        );
        assert_eq!(panel.confirm_label, "Confirm: terminate PID · worker");
        assert!(panel.confirm_enabled);
        assert!(!view.status.contains("dry run"), "{}", view.status);
        assert_eq!(
            gate.confirm(&fake, CLIENT, 10),
            ActionOutcome::Requested {
                intent: id,
                dry_run: false
            }
        );
        let status = gate.view().status;
        assert!(
            status.starts_with("Requested #1 (terminate PID · worker): SIGTERM was sent"),
            "{status}"
        );
        assert!(
            !status.contains("complete") && !status.contains("exited."),
            "a sent signal is not an ended process: {status}"
        );
        // An executor's refusal is a refusal, with the intent's number.
        *recording.answer.lock().unwrap() =
            Some(Execution::Refused(Refusal::NotHeld("gone".into())));
        let id = prepared(&mut gate, &fake);
        assert_eq!(
            gate.confirm(&fake, CLIENT, 10),
            ActionOutcome::Refused {
                intent: Some(id),
                reason: Refusal::NotHeld("gone".into())
            }
        );
        assert!(gate
            .view()
            .status
            .starts_with("Refused #2 (terminate PID · worker): the process could not be held for signalling: gone; nothing was requested"));
    }
}
