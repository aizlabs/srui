//! Session-owned terminal namespace table and PTY binding (§15, §21, §21.2).

use std::collections::HashMap;

use srui_protocol::{
    srui_message, ExtensionNamespaceMapping, SruiMessage, TerminalResyncReason,
    MAX_TERMINAL_RESUME_MAP_ENTRIES, TERMINAL_LOCAL_TYPE_ID, TERMINAL_PROFILE_URI,
};
use srui_pty::{PTYManager, SubscribeOutcome, TerminalEvent, TerminalSpec, TerminalSubscription};
use srui_sdk::StoreMut;
use srui_semantic_tree::{NodeId, Profile, TypeRef};

use super::{lock_or_recover, Session, SessionError};
use crate::outbound::LogicalChannelClass;

/// Handshake-time terminal attach: catch-up frames plus live subscriptions.
pub struct TerminalAttach {
    /// Replay (`terminalNormal`) or local resync (`terminalHigh`) sent after the semantic decision.
    pub catch_up: Vec<(LogicalChannelClass, SruiMessage)>,
    /// Live cursors; new output after the captured cut.
    pub live: Vec<TerminalSubscription>,
}

impl std::fmt::Debug for TerminalAttach {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerminalAttach")
            .field("catch_up_len", &self.catch_up.len())
            .field("live_len", &self.live.len())
            .finish()
    }
}

impl TerminalAttach {
    #[must_use]
    pub fn empty() -> Self {
        Self {
            catch_up: Vec::new(),
            live: Vec::new(),
        }
    }
}

pub(crate) fn standard_namespace_mapping() -> ExtensionNamespaceMapping {
    ExtensionNamespaceMapping {
        extension_uri: "org.srui.standard-widgets".to_string(),
        namespace_id: 0,
    }
}

struct TerminalTypeRollback {
    was_required: bool,
    was_optional: bool,
    created_namespace: Option<u32>,
}

impl Session {
    /// Returns the session-stable extension namespace table, including namespace 0.
    #[must_use]
    pub fn extension_namespaces(&self) -> Vec<ExtensionNamespaceMapping> {
        let guard = lock_or_recover(&self.inner);
        guard.extension_namespaces.clone()
    }

    /// Namespace assigned to `org.srui.terminal/1`, if this session has created a Terminal node.
    #[must_use]
    pub fn terminal_namespace_id(&self) -> Option<u32> {
        let guard = lock_or_recover(&self.inner);
        guard
            .extension_namespaces
            .iter()
            .find(|mapping| mapping.extension_uri == TERMINAL_PROFILE_URI)
            .map(|mapping| mapping.namespace_id)
    }

    /// Shared PTY manager. Lives outside [`super::SessionInner`] so PTY I/O never holds it.
    #[must_use]
    pub fn pty(&self) -> &PTYManager {
        &self.pty
    }

    /// Creates a Terminal extension node and binds `node_id` to a PTY stream (§21).
    ///
    /// Marks `org.srui.terminal/1` required (no semantic fallback), allocates a stable
    /// session namespace, and keeps the PTY alive across detach.
    ///
    /// v1 is handshake-scoped: live pumps and `extension_namespaces` are installed only
    /// when a client attaches. Calling this after [`Session::is_attached`] is therefore
    /// refused — spawn terminals before `handle_connection` accepts a transport.
    pub fn create_terminal_node(
        &self,
        node_id: NodeId,
        parent: NodeId,
        spec: TerminalSpec,
    ) -> Result<TypeRef, SessionError> {
        if self.is_attached() {
            return Err(SessionError::InvalidInput(
                "v1 create_terminal_node must run before any client attaches; \
                 live pumps and extension_namespaces are handshake-only"
                    .to_string(),
            ));
        }
        let (type_ref, rollback) = self.prepare_terminal_type()?;
        if let Err(error) = self.pty.spawn(node_id, spec) {
            self.rollback_terminal_type(rollback);
            return Err(SessionError::InvalidInput(error.to_string()));
        }
        let spawned = self.transaction(|ui| {
            StoreMut::create_node(ui, node_id, type_ref, Some(parent), None, [])?;
            Ok(type_ref)
        });
        if spawned.is_err() {
            let _ = self.pty.close(node_id);
            self.rollback_terminal_type(rollback);
        }
        spawned
    }

    fn prepare_terminal_type(&self) -> Result<(TypeRef, TerminalTypeRollback), SessionError> {
        let mut guard = self.inner.lock().map_err(|_| SessionError::LockPoisoned)?;
        let profile = Profile::terminal_v1();
        let was_required = guard.capabilities.required.contains(&profile);
        let was_optional = guard.capabilities.optional.remove(&profile);
        guard.capabilities.required.insert(profile);
        let (namespace_id, created_namespace) = if let Some(existing) = guard
            .extension_namespaces
            .iter()
            .find(|mapping| mapping.extension_uri == TERMINAL_PROFILE_URI)
            .map(|mapping| mapping.namespace_id)
        {
            (existing, None)
        } else {
            let allocated = next_extension_namespace(&guard.extension_namespaces);
            guard.extension_namespaces.push(ExtensionNamespaceMapping {
                extension_uri: TERMINAL_PROFILE_URI.to_string(),
                namespace_id: allocated,
            });
            (allocated, Some(allocated))
        };
        let rollback = TerminalTypeRollback {
            was_required,
            was_optional,
            created_namespace,
        };
        Ok((
            TypeRef::new(namespace_id, TERMINAL_LOCAL_TYPE_ID),
            rollback,
        ))
    }

    fn rollback_terminal_type(&self, rollback: TerminalTypeRollback) {
        let Ok(mut guard) = self.inner.lock() else { return };
        if !self.pty.live_stream_ids().is_empty() {
            return;
        }
        let profile = Profile::terminal_v1();
        if !rollback.was_required {
            guard.capabilities.required.remove(&profile);
        }
        if rollback.was_optional {
            guard.capabilities.optional.insert(profile);
        }
        if let Some(ns_id) = rollback.created_namespace {
            guard
                .extension_namespaces
                .retain(|mapping| mapping.namespace_id != ns_id);
        }
    }

    /// Subscribes every live stream. Replaced incarnations ignore advertised offsets.
    pub fn attach_terminals(
        &self,
        offsets: &HashMap<u64, u64>,
        ignore_offsets: bool,
    ) -> Result<TerminalAttach, SessionError> {
        if offsets.len() > MAX_TERMINAL_RESUME_MAP_ENTRIES {
            return Err(SessionError::InvalidInput(format!(
                "terminal_stream_offsets has {} entries; at most {MAX_TERMINAL_RESUME_MAP_ENTRIES} are accepted (§21, §26)",
                offsets.len()
            )));
        }
        let effective = if ignore_offsets {
            HashMap::new()
        } else {
            offsets.clone()
        };
        let outcomes = self
            .pty
            .subscribe_all(&effective)
            .map_err(|error| SessionError::InvalidInput(error.to_string()))?;
        Ok(attach_from_outcomes(outcomes))
    }

    pub(crate) fn close_terminals_for_deleted_nodes(&self, deleted: &[NodeId]) {
        self.pty.close_many(deleted.iter().copied());
    }

    pub(crate) fn shutdown_terminals(&self) {
        self.pty.shutdown();
    }
}

fn next_extension_namespace(existing: &[ExtensionNamespaceMapping]) -> u32 {
    let used: std::collections::HashSet<u32> = existing.iter().map(|m| m.namespace_id).collect();
    (1..=u32::MAX)
        .find(|id| !used.contains(id))
        .expect("extension namespace space exhausted")
}

fn attach_from_outcomes(outcomes: Vec<SubscribeOutcome>) -> TerminalAttach {
    let mut catch_up = Vec::new();
    let mut live = Vec::new();
    for outcome in outcomes {
        let class = if outcome.is_replay() {
            LogicalChannelClass::TerminalNormal
        } else {
            LogicalChannelClass::TerminalHigh
        };
        for event in outcome.catch_up_events() {
            catch_up.push((class, event_to_message(event)));
        }
        live.push(outcome.subscription);
    }
    TerminalAttach { catch_up, live }
}

pub(crate) fn event_to_message(event: TerminalEvent) -> SruiMessage {
    match event {
        TerminalEvent::Data(data) => SruiMessage {
            msg: Some(srui_message::Msg::TerminalData(data)),
        },
        TerminalEvent::Resync(resync) => SruiMessage {
            msg: Some(srui_message::Msg::TerminalResyncRequired(resync)),
        },
    }
}

pub(crate) fn live_class_for_event(event: &TerminalEvent) -> LogicalChannelClass {
    match event {
        TerminalEvent::Data(_) | TerminalEvent::Resync(_) => LogicalChannelClass::TerminalHigh,
    }
}

/// Live output and local resync share `terminalHigh`; historical replay uses `terminalNormal`.
#[allow(dead_code)]
pub(crate) fn live_event_class(reason: Option<TerminalResyncReason>) -> LogicalChannelClass {
    let _ = reason;
    LogicalChannelClass::TerminalHigh
}
