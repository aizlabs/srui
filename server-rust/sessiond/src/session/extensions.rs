//! Session-stable optional extension profile registration (§6.4, §11, §15).
//!
//! The registration API is deliberately optional-only: required extensions without a fallback
//! need profile-specific lifecycle code like Terminal, while Task 31 only needs a placeholder
//! extension whose Standard Widget subtree remains usable by a base client.

use std::collections::HashSet;

use srui_protocol::{ExtensionNamespaceMapping, TERMINAL_PROFILE_URI};
use srui_semantic_tree::Profile;

use super::{Session, SessionError};

/// Allocates the lowest unused nonzero session namespace.
pub(crate) fn next_extension_namespace(
    existing: &[ExtensionNamespaceMapping],
) -> Result<u32, SessionError> {
    let used: HashSet<u32> = existing
        .iter()
        .map(|mapping| mapping.namespace_id)
        .collect();
    (1..=u32::MAX).find(|id| !used.contains(id)).ok_or_else(|| {
        SessionError::InvalidInput("extension namespace space exhausted".to_string())
    })
}

impl Session {
    /// Registers a fallback-capable optional extension before any client negotiates (§11.1, §15).
    ///
    /// Repeated registration is idempotent. Standard widgets keep namespace 0, and Terminal uses
    /// [`Session::create_terminal_node`] because its PTY/profile changes need rollback together.
    pub fn register_optional_extension_profile(
        &self,
        profile: Profile,
    ) -> Result<u32, SessionError> {
        if profile == Profile::standard_widgets_v1() {
            return Err(SessionError::InvalidInput(
                "org.srui.standard-widgets/1 is permanently assigned to namespace 0".to_string(),
            ));
        }
        if profile == Profile::terminal_v1() {
            return Err(SessionError::InvalidInput(format!(
                "{TERMINAL_PROFILE_URI} must be registered through create_terminal_node"
            )));
        }

        let mut guard = self.inner.lock().map_err(|_| SessionError::LockPoisoned)?;
        if guard.attached_connections > 0 || guard.has_negotiated {
            return Err(SessionError::InvalidInput(
                "extension profiles must be registered before any client attaches or handshakes"
                    .to_string(),
            ));
        }
        if guard.capabilities.required.contains(&profile) {
            return Err(SessionError::InvalidInput(format!(
                "profile {profile} is already required and cannot also be optional"
            )));
        }

        let uri = profile.to_string();
        if let Some(namespace_id) = guard
            .extension_namespaces
            .iter()
            .find(|mapping| mapping.extension_uri == uri)
            .map(|mapping| mapping.namespace_id)
        {
            guard.capabilities.optional.insert(profile);
            return Ok(namespace_id);
        }

        let namespace_id = next_extension_namespace(&guard.extension_namespaces)?;
        guard.capabilities.optional.insert(profile);
        guard.extension_namespaces.push(ExtensionNamespaceMapping {
            extension_uri: uri,
            namespace_id,
        });
        Ok(namespace_id)
    }
}
