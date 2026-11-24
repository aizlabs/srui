//! Graceful termination through an identity-checked process handle (D1 §27
//! server security controls, §6.2; D2 T32; K2 `pidfd_send_signal(2)`, K3
//! `pidfd_open(2)`; PX-030).
//!
//! [`PidfdTerminator`] is the one production [`ProcessActions`] that can
//! reach a process, and this file is the only one besides
//! [`crate::handle`] that PX-008's audit lets name the handle module. It
//! holds exactly **one** [`ProcessHandles`] table, as PX-029 requires: a
//! [`HandleId`] is minted per table and [`ProcessHandles::settle`] takes no
//! key, so a second table could settle a handle it did not acquire.
//!
//! * **Acquired at Confirm, not at Prepare.** The gate re-checks the intent
//!   in full and then hands it here; the handle is opened, checked against
//!   the intent's [`ProcessKey`] after the open (PX-029), and settled at once.
//!   No descriptor is held while a person reads the confirmation, so Cancel,
//!   expiry, navigation and withdrawal have nothing to close and cannot leak
//!   one, and the window between the identity check and the signal is the
//!   two system calls. Binding at Prepare would add nothing: the key already
//!   pins the instance by its start time, which the post-open check compares.
//! * **The key is checked again before settling.** [`ProcessHandles::key_of`]
//!   must name the intent's own key, or the handle is closed unused
//!   ([`signal_held`]).
//! * **SIGTERM only, never by PID.** The only signal is
//!   [`HandleSignal::Terminate`]. Where PID file descriptors are unavailable
//!   — not Linux, `ENOSYS`, a filter's `EPERM`, `ENODEV` — the terminator
//!   reports itself [`ProcessActions::unavailable`] from its probe, the gate
//!   publishes Terminate disabled with that reason, and nothing falls back to
//!   `kill(2)`: this module names no such call and the audit refuses one.
//! * **Requested is not ended.** `Sent` means the kernel accepted SIGTERM for
//!   that process, which may handle or ignore it; a zombie accepts it too.
//!   Whether it exited is observed by the gate from the view's own scans
//!   (PX-031, [`crate::actions::ObservedOutcome`]), never reported here.

use crate::actions::{ActionOperation, Authorized, Execution, ProcessActions, Refusal};
use crate::handle::{AcquireError, HandleId, HandleSignal, ProcessHandles, SignalOutcome};
use crate::source::ProcessKey;
use std::sync::Mutex;

/// The real termination executor of `--actions terminate` (PX-030).
#[derive(Debug)]
pub struct PidfdTerminator {
    /// The one handle table.
    handles: Mutex<ProcessHandles>,
    /// Why handles cannot be opened on this host, from the probe at start.
    unavailable: Option<String>,
}

impl PidfdTerminator {
    /// A terminator over this host's PID file descriptors.
    pub fn live() -> Self {
        Self::with_handles(ProcessHandles::live())
    }

    /// A terminator over `handles`, probed once: a handle path the probe
    /// finds unsupported keeps termination unavailable for the whole run.
    pub fn with_handles(handles: ProcessHandles) -> Self {
        let unavailable = match handles.probe() {
            Err(AcquireError::Unsupported(why)) => Some(why.describe().to_string()),
            // Available, or a transient failure an acquisition will report.
            Ok(()) | Err(_) => None,
        };
        Self {
            handles: Mutex::new(handles),
            unavailable,
        }
    }

    /// How many handles the table holds now: zero between actions.
    pub fn live_handles(&self) -> usize {
        self.lock().live_count()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ProcessHandles> {
        self.handles
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl ProcessActions for PidfdTerminator {
    fn supports(&self, operation: ActionOperation) -> bool {
        operation == ActionOperation::Terminate && self.unavailable.is_none()
    }

    fn unavailable(&self) -> Option<String> {
        self.unavailable.clone()
    }

    fn execute(&self, action: &Authorized<'_>) -> Execution {
        let intent = action.intent();
        if intent.operation() != ActionOperation::Terminate {
            return Execution::Unsupported;
        }
        if self.unavailable.is_some() {
            return Execution::Unsupported;
        }
        let [target] = intent.targets() else {
            return Execution::Failed(
                "termination acts on exactly one process; nothing was sent".into(),
            );
        };
        let mut handles = self.lock();
        match handles.acquire(target.key()) {
            Ok(id) => signal_held(&mut handles, id, target.key()),
            Err(error) => not_acquired(error),
        }
    }
}

/// Settles `id` with SIGTERM, only when the table holds it for exactly
/// `key`, the intent's confirmed instance; otherwise closes it unused.
pub fn signal_held(handles: &mut ProcessHandles, id: HandleId, key: &ProcessKey) -> Execution {
    if handles.key_of(id) != Some(key) {
        handles.cancel(id);
        return Execution::Refused(Refusal::NotHeld(
            "the held process is not the confirmed one; nothing was sent".into(),
        ));
    }
    match handles.settle(id, HandleSignal::Terminate) {
        SignalOutcome::Sent => Execution::Requested,
        SignalOutcome::Ended => Execution::Failed(
            "the process exited before SIGTERM reached it; nothing was sent".into(),
        ),
        SignalOutcome::Denied => Execution::Failed(
            "the system refused the signal (no permission, or a system-call filter); nothing was sent"
                .into(),
        ),
        SignalOutcome::Unsupported(_) => Execution::Unsupported,
        SignalOutcome::Expired | SignalOutcome::NoHandle => {
            Execution::Failed("the process handle was no longer held; nothing was sent".into())
        }
        SignalOutcome::Failed(why) => Execution::Failed(format!("{why}; nothing was sent")),
    }
}

/// What a refused acquisition means for the request: nothing was sent.
fn not_acquired(error: AcquireError) -> Execution {
    match error {
        AcquireError::Unsupported(_) => Execution::Unsupported,
        AcquireError::Replaced => Execution::Refused(Refusal::Replaced),
        AcquireError::Ended
        | AcquireError::Unidentifiable(_)
        | AcquireError::OtherBoot
        | AcquireError::OtherNamespace
        | AcquireError::Unverified(_) => Execution::Refused(Refusal::NotHeld(error.describe())),
        AcquireError::DescriptorsExhausted { .. }
        | AcquireError::LimitReached { .. }
        | AcquireError::Failed(_) => {
            Execution::Failed(format!("{}; nothing was sent", error.describe()))
        }
    }
}
