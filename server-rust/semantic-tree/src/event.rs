//! Client-originated semantic events and validation helpers (§6.1, §7.6, §7.7, §16, §18.2, §27).
//!
//! # Architecture & Protocol Invariants
//!
//! - **§6.1 Core Objects**: An `Event` represents a client-originated semantic user action.
//! - **§7.6 Standard Events**: Controls emit high-level semantic events (`ACTIVATE`, `VALUE_CHANGED`,
//!   `SELECTION_CHANGED`, `EXPANSION_CHANGED`, `TEXT_EDIT`, `VIEWPORT_CHANGED`) rather than raw pointer coordinates.
//! - **§7.7 Semantic Input Routing**: Events report *what happened to which semantic node*, carrying
//!   `event_seq`, a retry-safe `event_id`, `observed_revision`, target `node_id`, `event_type`, and
//!   event-specific arguments.
//! - **§18.2 Retry Safety & Deduplication**: Every event that can cause side effects contains a stable
//!   `event_id` unique within session lifetime, enabling retry-safe delivery across network disconnects.
//! - **§27 Server Validation**: The server validates every client event against the current graph
//!   (verifying node existence, interactive/enabled status, and revision freshness) before dispatching to handlers.

use crate::ids::{ItemId, NodeId, PropertyRef, TypeRef};
use crate::store::{Node, SemanticStore};
use crate::transaction::Revision;
use crate::value::{Size, Value};
use std::collections::HashMap;
use std::fmt;

/// Globally unique event identifier for deduplication and retry-safety (§7.7, §16, §18.2).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EventId(pub Vec<u8>);

impl EventId {
    /// Constructs a new `EventId` from a raw byte vector.
    pub const fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    /// Constructs an `EventId` by cloning a byte slice.
    pub fn from_slice(slice: &[u8]) -> Self {
        Self(slice.to_vec())
    }

    /// Constructs an `EventId` from a UTF-8 string or identifier.
    pub fn from_string(s: impl Into<String>) -> Self {
        Self(s.into().into_bytes())
    }

    /// Returns lowercase hex string representation of the identifier.
    pub fn to_hex(&self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut hex = String::with_capacity(self.0.len() * 2);
        for &byte in &self.0 {
            hex.push(HEX[(byte >> 4) as usize] as char);
            hex.push(HEX[(byte & 0x0f) as usize] as char);
        }
        hex
    }

    /// Returns the underlying byte slice.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Returns `true` if the event identifier is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Returns the byte length of the identifier.
    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl fmt::Display for EventId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Ok(s) = std::str::from_utf8(&self.0) {
            if s.chars().all(|c| !c.is_control()) {
                return write!(f, "EventId({})", s);
            }
        }
        write!(f, "EventId(0x{})", self.to_hex())
    }
}

impl From<Vec<u8>> for EventId {
    fn from(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }
}

impl From<&[u8]> for EventId {
    fn from(slice: &[u8]) -> Self {
        Self(slice.to_vec())
    }
}

impl From<&str> for EventId {
    fn from(s: &str) -> Self {
        Self(s.as_bytes().to_vec())
    }
}

impl From<String> for EventId {
    fn from(s: String) -> Self {
        Self(s.into_bytes())
    }
}

/// Ephemeral client instance identifier distinguishing reconnecting client attachments (§16, §18).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ClientInstanceId(pub Vec<u8>);

impl ClientInstanceId {
    /// Constructs a new `ClientInstanceId` from a raw byte vector.
    pub const fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    /// Constructs a `ClientInstanceId` by cloning a byte slice.
    pub fn from_slice(slice: &[u8]) -> Self {
        Self(slice.to_vec())
    }

    /// Constructs a `ClientInstanceId` from a UTF-8 string.
    pub fn from_string(s: impl Into<String>) -> Self {
        Self(s.into().into_bytes())
    }

    /// Returns lowercase hex string representation of the identifier.
    pub fn to_hex(&self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut hex = String::with_capacity(self.0.len() * 2);
        for &byte in &self.0 {
            hex.push(HEX[(byte >> 4) as usize] as char);
            hex.push(HEX[(byte & 0x0f) as usize] as char);
        }
        hex
    }

    /// Returns the underlying byte slice.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Returns `true` if the identifier is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Returns the byte length of the identifier.
    pub fn len(&self) -> usize {
        self.0.len()
    }
}

impl fmt::Display for ClientInstanceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Ok(s) = std::str::from_utf8(&self.0) {
            if s.chars().all(|c| !c.is_control()) {
                return write!(f, "ClientInstanceId({})", s);
            }
        }
        write!(f, "ClientInstanceId(0x{})", self.to_hex())
    }
}

impl From<Vec<u8>> for ClientInstanceId {
    fn from(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }
}

impl From<&[u8]> for ClientInstanceId {
    fn from(slice: &[u8]) -> Self {
        Self(slice.to_vec())
    }
}

impl From<&str> for ClientInstanceId {
    fn from(s: &str) -> Self {
        Self(s.as_bytes().to_vec())
    }
}

impl From<String> for ClientInstanceId {
    fn from(s: String) -> Self {
        Self(s.into_bytes())
    }
}

/// Client-originated semantic event representing a user interaction (§6.1, §7.6, §7.7, §16).
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    /// Optional client instance identifier (§16, §18).
    pub client_instance_id: Option<ClientInstanceId>,
    /// Monotonically increasing per-client event sequence number (§7.7, §16).
    pub event_seq: u64,
    /// Globally unique, retry-safe event identifier for deduplication (§7.7, §16, §18.2).
    pub event_id: EventId,
    /// Authoritative tree revision observed by client when generating this event (§7.7, §16).
    pub observed_revision: Revision,
    /// Target semantic node identifier in the UI graph (§6.2, §7.7).
    pub node_id: NodeId,
    /// Event type reference in a namespace registry (§6.4, §7.6, §7.7).
    pub event_type: TypeRef,
    /// Event payload arguments mapped by property reference (§7.6, §7.7, §16).
    pub arguments: HashMap<PropertyRef, Value>,
}

impl Event {
    /// Constructs a generic semantic event.
    pub fn new(
        client_instance_id: Option<ClientInstanceId>,
        event_seq: u64,
        event_id: impl Into<EventId>,
        observed_revision: impl Into<Revision>,
        node_id: impl Into<NodeId>,
        event_type: TypeRef,
        arguments: impl IntoIterator<Item = (PropertyRef, Value)>,
    ) -> Self {
        Self {
            client_instance_id,
            event_seq,
            event_id: event_id.into(),
            observed_revision: observed_revision.into(),
            node_id: node_id.into(),
            event_type,
            arguments: arguments.into_iter().collect(),
        }
    }

    /// Convenience constructor for momentary control activation (`ACTIVATE`, §7.6, §7.7).
    pub fn activate(
        event_seq: u64,
        event_id: impl Into<EventId>,
        observed_revision: impl Into<Revision>,
        node_id: impl Into<NodeId>,
    ) -> Self {
        Self::new(
            None,
            event_seq,
            event_id,
            observed_revision,
            node_id,
            TypeRef::EVENT_ACTIVATE,
            std::iter::empty(),
        )
    }

    /// Convenience constructor for value change events (`VALUE_CHANGED`, §7.6).
    pub fn value_changed(
        event_seq: u64,
        event_id: impl Into<EventId>,
        observed_revision: impl Into<Revision>,
        node_id: impl Into<NodeId>,
        value: impl Into<Value>,
    ) -> Self {
        Self::new(
            None,
            event_seq,
            event_id,
            observed_revision,
            node_id,
            TypeRef::EVENT_VALUE_CHANGED,
            [(PropertyRef::VALUE, value.into())],
        )
    }

    /// Convenience constructor for selection change events (`SELECTION_CHANGED`, §7.6).
    pub fn selection_changed(
        event_seq: u64,
        event_id: impl Into<EventId>,
        observed_revision: impl Into<Revision>,
        node_id: impl Into<NodeId>,
        item_id: impl Into<ItemId>,
    ) -> Self {
        Self::new(
            None,
            event_seq,
            event_id,
            observed_revision,
            node_id,
            TypeRef::EVENT_SELECTION_CHANGED,
            [(PropertyRef::VALUE, Value::ItemId(item_id.into()))],
        )
    }

    /// Convenience constructor for text edit events (`TEXT_EDIT`, §7.6, §22.6).
    pub fn text_edit(
        event_seq: u64,
        event_id: impl Into<EventId>,
        observed_revision: impl Into<Revision>,
        node_id: impl Into<NodeId>,
        text: impl Into<String>,
    ) -> Self {
        Self::new(
            None,
            event_seq,
            event_id,
            observed_revision,
            node_id,
            TypeRef::EVENT_TEXT_EDIT,
            [(PropertyRef::TEXT, Value::String(text.into()))],
        )
    }

    /// Convenience constructor for expansion change events (`EXPANSION_CHANGED`, §7.6).
    pub fn expansion_changed(
        event_seq: u64,
        event_id: impl Into<EventId>,
        observed_revision: impl Into<Revision>,
        node_id: impl Into<NodeId>,
        expanded: bool,
    ) -> Self {
        Self::new(
            None,
            event_seq,
            event_id,
            observed_revision,
            node_id,
            TypeRef::EVENT_EXPANSION_CHANGED,
            [(PropertyRef::VALUE, Value::Bool(expanded))],
        )
    }

    /// Convenience constructor for viewport change events (`VIEWPORT_CHANGED`, §7.6).
    pub fn viewport_changed(
        event_seq: u64,
        event_id: impl Into<EventId>,
        observed_revision: impl Into<Revision>,
        node_id: impl Into<NodeId>,
        size: Size,
    ) -> Self {
        Self::new(
            None,
            event_seq,
            event_id,
            observed_revision,
            node_id,
            TypeRef::EVENT_VIEWPORT_CHANGED,
            [(PropertyRef::VALUE, Value::Size(size))],
        )
    }

    /// Sets the client instance identifier on this event.
    pub fn with_client_instance_id(mut self, client_instance_id: impl Into<ClientInstanceId>) -> Self {
        self.client_instance_id = Some(client_instance_id.into());
        self
    }

    /// Adds an argument to the event payload.
    pub fn with_argument(mut self, property: PropertyRef, value: impl Into<Value>) -> Self {
        self.arguments.insert(property, value.into());
        self
    }

    /// Returns a reference to an argument value if present.
    pub fn get_argument(&self, prop: PropertyRef) -> Option<&Value> {
        self.arguments.get(&prop)
    }

    /// Returns `true` if the event contains the specified argument property.
    pub fn has_argument(&self, prop: PropertyRef) -> bool {
        self.arguments.contains_key(&prop)
    }

    /// Returns the primary `Value` argument (from [`PropertyRef::VALUE`]) if present.
    pub fn value_arg(&self) -> Option<&Value> {
        self.get_argument(PropertyRef::VALUE)
    }

    /// Returns the text string argument (from [`PropertyRef::TEXT`]) if present.
    pub fn text_arg(&self) -> Option<&str> {
        self.get_argument(PropertyRef::TEXT).and_then(|v| v.as_string())
    }

    /// Returns the boolean argument (from [`PropertyRef::VALUE`]) if present.
    pub fn bool_arg(&self) -> Option<bool> {
        self.get_argument(PropertyRef::VALUE).and_then(|v| v.as_bool())
    }

    /// Returns the item ID argument (from [`PropertyRef::VALUE`]) if present.
    pub fn item_id_arg(&self) -> Option<ItemId> {
        self.get_argument(PropertyRef::VALUE).and_then(|v| v.as_item_id())
    }

    /// Returns the standard event name if this event's type belongs to standard namespace 0 (§7.6).
    pub fn standard_name(&self) -> Option<&'static str> {
        self.event_type.standard_event_name()
    }

    /// Validates that the event's target `node_id` exists in the provided [`SemanticStore`].
    pub fn validate_node_exists<'a>(&self, store: &'a SemanticStore) -> Result<&'a Node, EventValidationError> {
        store
            .get_node(self.node_id)
            .ok_or(EventValidationError::NodeNotFound(self.node_id))
    }

    /// Validates that the event's target `node_id` exists and is interactive (`enabled != false`) in the store (§7.4, §7.7, §27).
    pub fn validate_node_interactive<'a>(&self, store: &'a SemanticStore) -> Result<&'a Node, EventValidationError> {
        let node = self.validate_node_exists(store)?;
        if let Some(Value::Bool(false)) = node.get_property(PropertyRef::ENABLED) {
            return Err(EventValidationError::NodeDisabled(self.node_id));
        }
        Ok(node)
    }

    /// Validates that the event's `observed_revision` is not from an unseen future revision (§7.7, §12.1).
    pub fn validate_observed_revision(&self, store_revision: Revision) -> Result<(), EventValidationError> {
        if self.observed_revision > store_revision {
            return Err(EventValidationError::FutureRevision {
                observed: self.observed_revision,
                current: store_revision,
            });
        }
        Ok(())
    }

    /// Performs complete semantic validation against the store: checks node existence, node enabled state, and revision validity (§7.7, §27).
    pub fn validate<'a>(&self, store: &'a SemanticStore) -> Result<&'a Node, EventValidationError> {
        self.validate_observed_revision(store.revision())?;
        self.validate_node_interactive(store)
    }
}

impl fmt::Display for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let type_label = self
            .standard_name()
            .map(|n| n.to_string())
            .unwrap_or_else(|| self.event_type.to_string());
        write!(
            f,
            "Event(seq={}, id={}, node={}, type={}, rev={}, args={})",
            self.event_seq,
            self.event_id,
            self.node_id,
            type_label,
            self.observed_revision,
            self.arguments.len()
        )
    }
}

/// Error encountered during semantic event validation against a [`SemanticStore`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventValidationError {
    /// Target `NodeId` does not exist in the store (§7.7).
    NodeNotFound(NodeId),
    /// Target node has `enabled = false` and cannot accept interactive events (§7.4, §27).
    NodeDisabled(NodeId),
    /// Event references an observed revision that is in the future relative to the store (§7.7, §12.1).
    FutureRevision {
        observed: Revision,
        current: Revision,
    },
    /// Event is missing an expected argument property.
    MissingArgument(PropertyRef),
}

impl fmt::Display for EventValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NodeNotFound(id) => write!(f, "event target node {} does not exist in store", id),
            Self::NodeDisabled(id) => write!(f, "event target node {} is disabled", id),
            Self::FutureRevision { observed, current } => write!(
                f,
                "event observed revision {} is in the future relative to store revision {}",
                observed, current
            ),
            Self::MissingArgument(prop) => write!(f, "event is missing required argument property {}", prop),
        }
    }
}

impl std::error::Error for EventValidationError {}
