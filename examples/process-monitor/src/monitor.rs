//! Runtime monitor orchestration and event handling (§7.6, §12.1, §27).

use std::sync::{Arc, Mutex, MutexGuard, Weak};

use srui_protocol::Event as WireEvent;
use srui_sdk::*;
use srui_semantic_tree::Event as SemanticEvent;
use srui_sessiond::{Session, SessionError};
use tracing::{debug, info, warn};

use crate::domain::{KILL_BUTTON_ID, PROCESS_TABLE_ID, SHOW_ALL_ID};
use crate::source::ProcessSource;
use crate::state::{MonitorState, TickPlan};
use crate::terminator::{ProcessTerminator, TerminateError};
use crate::ui::build_initial_ui;

pub(crate) fn lock_or_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// Result of a "Kill Selected" activation, reported for logging and tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KillOutcome {
    /// No row is selected in authoritative server state.
    NoSelection,
    /// The selected item id is unknown or stale.
    UnknownSelection(ItemId),
    /// The resolved PID is denylisted and was not signalled.
    Denied(u32),
    /// The live process at that PID has a different start time: identity is stale.
    StaleIdentity(u32),
    /// `SIGTERM` was delivered.
    Terminated(u32),
    /// The OS refused the signal.
    Failed(u32, TerminateError),
}

/// Live process monitor: authoritative state plus the session it publishes into.
pub struct Monitor {
    session: Arc<Session>,
    state: Mutex<MonitorState>,
    source: Mutex<Box<dyn ProcessSource>>,
    terminator: Box<dyn ProcessTerminator>,
}

impl std::fmt::Debug for Monitor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Monitor")
            .field("session_id", &self.session.session_id())
            .finish_non_exhaustive()
    }
}

impl Monitor {
    /// Samples once, publishes the initial graph, and registers semantic event handlers.
    pub fn start(
        session: Arc<Session>,
        mut source: Box<dyn ProcessSource>,
        terminator: Box<dyn ProcessTerminator>,
        effective_uid: u32,
    ) -> Result<Arc<Self>, SessionError> {
        let snapshot = source.sample();
        let mut state = MonitorState::new(effective_uid);
        let _ = state.plan_initial(&snapshot);
        build_initial_ui(&session, &state)?;

        let monitor = Arc::new(Self {
            session,
            state: Mutex::new(state),
            source: Mutex::new(source),
            terminator,
        });
        monitor.register_handlers();
        Ok(monitor)
    }

    /// The underlying session.
    pub fn session(&self) -> &Arc<Session> {
        &self.session
    }

    /// Runs `f` against authoritative application state (semantic inspection, §22.9).
    pub fn with_state<T>(&self, f: impl FnOnce(&MonitorState) -> T) -> T {
        f(&lock_or_recover(&self.state))
    }

    /// Samples the system and commits at most one transaction (§12.1). Returns the operation count.
    ///
    /// The sample is taken before the application-state lock is acquired, so no `sysinfo` refresh
    /// ever runs inside an open transaction.
    pub fn tick(&self) -> Result<usize, SessionError> {
        let snapshot = lock_or_recover(&self.source).sample();
        let mut state = lock_or_recover(&self.state);
        let plan = state.plan_sample(&snapshot);
        self.commit_plan(&mut state, plan)
    }

    fn commit_plan(&self, state: &mut MonitorState, plan: TickPlan) -> Result<usize, SessionError> {
        // An empty diff commits no transaction (§12.1), but authoritative state still advances:
        // a newly enumerated process that is currently filtered out must still be tracked so a
        // later `show_all` change can publish it.
        let count = plan.operations().len();
        if count > 0 {
            let operations = plan.operations().to_vec();
            self.session.transaction(move |ui| {
                for op in &operations {
                    ui.apply_op(op)?;
                }
                Ok(())
            })?;
        }
        state.commit(plan);
        Ok(count)
    }

    fn register_handlers(self: &Arc<Self>) {
        let selection_target = Arc::downgrade(self);
        self.session
            .on(PROCESS_TABLE_ID, SELECTION_CHANGED, move |_, event| {
                if let Some(monitor) = selection_target.upgrade() {
                    monitor.on_selection_changed(event);
                }
            });

        let toggle_target = Arc::downgrade(self);
        self.session
            .on(SHOW_ALL_ID, VALUE_CHANGED, move |_, event| {
                if let Some(monitor) = toggle_target.upgrade() {
                    monitor.on_show_all_changed(event);
                }
            });

        let kill_target: Weak<Self> = Arc::downgrade(self);
        self.session.on(KILL_BUTTON_ID, ACTIVATE, move |_, event| {
            if let Some(monitor) = kill_target.upgrade() {
                monitor.on_kill_activated(event);
            }
        });
    }

    /// Handles `SELECTION_CHANGED` on the process table (§7.6).
    ///
    /// Only the item id is trusted, and only if it currently exists in authoritative state. Row
    /// text, PID text, index, label, and `action_key` from the client are never consulted.
    pub fn on_selection_changed(&self, event: &WireEvent) {
        let Some(item) = decode_item_id(event) else {
            warn!("rejecting SELECTION_CHANGED without a usable item id argument");
            return;
        };
        let mut state = lock_or_recover(&self.state);
        let accepted = if !event.client_instance_id.is_empty() {
            state.select_for_client(&event.client_instance_id, item)
        } else {
            state.select(item)
        };
        if accepted {
            debug!("selection accepted for item {}", item.get());
        } else {
            warn!(
                "rejecting SELECTION_CHANGED for unknown or no longer visible item {}",
                item.get()
            );
        }
    }

    /// Handles `VALUE_CHANGED` on the "Show all processes" toggle (§7.6, §27).
    pub fn on_show_all_changed(&self, event: &WireEvent) {
        let Some(show_all) = decode_bool(event) else {
            warn!("rejecting VALUE_CHANGED without a boolean value argument");
            return;
        };
        let mut state = lock_or_recover(&self.state);
        let plan = state.plan_visibility(show_all);
        match self.commit_plan(&mut state, plan) {
            Ok(count) => info!("show_all set to {show_all} ({count} operations)"),
            Err(error) => {
                warn!("failed to apply show_all change: {error}");
                let current_show_all = state.show_all();
                let _ = self.session.transaction(move |ui| {
                    ui.set(SHOW_ALL_ID, VALUE, current_show_all)?;
                    Ok(())
                });
            }
        }
    }

    /// Handles `ACTIVATE` on the "Kill Selected" button (§7.6, §27).
    pub fn on_kill_activated(&self, event: &WireEvent) {
        let client_id = if !event.client_instance_id.is_empty() {
            Some(event.client_instance_id.as_slice())
        } else {
            None
        };
        match self.kill_selected_for_client(client_id) {
            KillOutcome::NoSelection => warn!("kill refused: no process is selected"),
            KillOutcome::UnknownSelection(item) => {
                warn!("kill refused: selected item {} is stale", item.get())
            }
            KillOutcome::Denied(pid) => warn!("kill refused: pid {pid} is denylisted"),
            KillOutcome::StaleIdentity(pid) => {
                warn!("kill refused: pid {pid} no longer matches the selected process identity")
            }
            KillOutcome::Terminated(pid) => info!("SIGTERM delivered to pid {pid}"),
            KillOutcome::Failed(pid, error) => warn!("kill of pid {pid} failed: {error}"),
        }
    }

    /// Resolves the selection through server-owned state and signals it, or refuses (§27).
    ///
    /// The PID is never read from client input: the selection is an [`ItemId`], resolved to a
    /// [`ProcessKey`] the server assigned, then revalidated against the live process start time
    /// immediately before signalling so a reused PID cannot be hit.
    pub fn kill_selected(&self) -> KillOutcome {
        self.kill_selected_for_client(None)
    }

    /// Resolves the selection for a specific client through server-owned state and signals it (§27).
    pub fn kill_selected_for_client(&self, client_id: Option<&[u8]>) -> KillOutcome {
        let target = {
            let state = lock_or_recover(&self.state);
            let selected = match client_id {
                Some(id) => state.selected_item_for_client(id),
                None => state.selected_item().or_else(|| {
                    if state.client_selections().len() == 1 {
                        state.client_selections().values().next().copied()
                    } else {
                        None
                    }
                }),
            };
            let Some(selected) = selected else {
                return KillOutcome::NoSelection;
            };
            let Some(key) = state.key_for_item(selected) else {
                return KillOutcome::UnknownSelection(selected);
            };
            if state.denylist().contains(&key.pid) {
                return KillOutcome::Denied(key.pid);
            }
            key
        };

        // Revalidate identity with no application-state lock held and no transaction open.
        let live_start_time = lock_or_recover(&self.source).start_time_of(target.pid);
        if live_start_time != Some(target.start_time) {
            return KillOutcome::StaleIdentity(target.pid);
        }

        match self.terminator.terminate_key(target) {
            Ok(()) => KillOutcome::Terminated(target.pid),
            Err(error) => KillOutcome::Failed(target.pid, error),
        }
    }
}

fn decode_semantic_event(event: &WireEvent) -> Option<SemanticEvent> {
    SemanticEvent::try_from(event.clone()).ok()
}

fn decode_item_id(event: &WireEvent) -> Option<ItemId> {
    let decoded = decode_semantic_event(event)?;
    match decoded.value_arg() {
        Some(Value::ItemId(item)) => Some(*item),
        Some(Value::UnsignedInt(raw)) => Some(ItemId::new(*raw)),
        _ => None,
    }
}

fn decode_bool(event: &WireEvent) -> Option<bool> {
    let decoded = decode_semantic_event(event)?;
    match decoded.value_arg() {
        Some(Value::Bool(value)) => Some(*value),
        _ => None,
    }
}
