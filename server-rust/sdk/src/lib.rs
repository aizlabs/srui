//! SRUI Server SDK
//!
//! Provides high-level typed builder, accessor, session, transaction, and event routing
//! abstractions on top of the core SRUI semantic tree and transaction engine (§7.1–§7.7, §12, §29).

pub mod session;
pub mod widgets;

pub use session::*;
pub use widgets::*;

// Re-export standard event types, identifiers, transactions, and wire helpers (§6.1, §7.6, §7.7, §12.1, §16)
pub use srui_semantic_tree::{
    decode_event, decode_message, decode_node_record, decode_operation, decode_transaction,
    decode_value, encode_event, encode_message, encode_node_record, encode_operation,
    encode_transaction, encode_value, ClientInstanceId, EnumToken, Event, EventId,
    EventValidationError, NodeRecord, Revision, Transaction, TxnError, WireError,
};

// =============================================================================
// Ergonomic PropertyRef Constants (§7.4, §29)
// =============================================================================

/// Property reference for `label` (§7.4).
pub const LABEL: PropertyRef = PropertyRef::LABEL;
/// Property reference for `accessible_description` (§7.4).
pub const ACCESSIBLE_DESCRIPTION: PropertyRef = PropertyRef::ACCESSIBLE_DESCRIPTION;
/// Property reference for `role` (§7.4, §7.5).
pub const ROLE: PropertyRef = PropertyRef::ROLE;
/// Property reference for `value_description` (§7.4).
pub const VALUE_DESCRIPTION: PropertyRef = PropertyRef::VALUE_DESCRIPTION;
/// Property reference for `actions` (§7.4).
pub const ACTIONS: PropertyRef = PropertyRef::ACTIONS;
/// Property reference for `action_key` (§7.7).
pub const ACTION_KEY: PropertyRef = PropertyRef::ACTION_KEY;
/// Property reference for `visibility` (§7.4).
pub const VISIBILITY: PropertyRef = PropertyRef::VISIBILITY;
/// Property reference for `enabled` (§7.4).
pub const ENABLED: PropertyRef = PropertyRef::ENABLED;
/// Property reference for `read_only` (§7.4).
pub const READ_ONLY: PropertyRef = PropertyRef::READ_ONLY;
/// Property reference for `busy` (§7.4).
pub const BUSY: PropertyRef = PropertyRef::BUSY;
/// Property reference for `selected` (§7.4).
pub const SELECTED: PropertyRef = PropertyRef::SELECTED;
/// Property reference for `validation_state` (§7.4).
pub const VALIDATION_STATE: PropertyRef = PropertyRef::VALIDATION_STATE;
/// Property reference for `text` (§7.4).
pub const TEXT: PropertyRef = PropertyRef::TEXT;
/// Property reference for `value` (§7.4).
pub const VALUE: PropertyRef = PropertyRef::VALUE;
/// Property reference for `placeholder` (§7.4).
pub const PLACEHOLDER: PropertyRef = PropertyRef::PLACEHOLDER;
/// Property reference for `resource` (§7.4, §14).
pub const RESOURCE: PropertyRef = PropertyRef::RESOURCE;
/// Property reference for `items` (§7.4).
pub const ITEMS: PropertyRef = PropertyRef::ITEMS;
/// Property reference for `model_ref` (§7.4, §8).
pub const MODEL_REF: PropertyRef = PropertyRef::MODEL_REF;
/// Property reference for `columns` (§7.4).
pub const COLUMNS: PropertyRef = PropertyRef::COLUMNS;
/// Property reference for `presentation_hint` (§7.2).
pub const PRESENTATION_HINT: PropertyRef = PropertyRef::PRESENTATION_HINT;
/// Property reference for `selection_mode` (§7.4).
pub const SELECTION_MODE: PropertyRef = PropertyRef::SELECTION_MODE;
/// Property reference for `horizontal_alignment` (§7.4).
pub const HORIZONTAL_ALIGNMENT: PropertyRef = PropertyRef::HORIZONTAL_ALIGNMENT;
/// Property reference for `vertical_alignment` (§7.4).
pub const VERTICAL_ALIGNMENT: PropertyRef = PropertyRef::VERTICAL_ALIGNMENT;
/// Property reference for `grow` (§7.4).
pub const GROW: PropertyRef = PropertyRef::GROW;
/// Property reference for `shrink` (§7.4).
pub const SHRINK: PropertyRef = PropertyRef::SHRINK;
/// Property reference for `minimum_size` (§7.4).
pub const MINIMUM_SIZE: PropertyRef = PropertyRef::MINIMUM_SIZE;
/// Property reference for `maximum_size` (§7.4).
pub const MAXIMUM_SIZE: PropertyRef = PropertyRef::MAXIMUM_SIZE;
/// Property reference for `preferred_size` (§7.4).
pub const PREFERRED_SIZE: PropertyRef = PropertyRef::PREFERRED_SIZE;
/// Property reference for `spacing_role` (§7.4).
pub const SPACING_ROLE: PropertyRef = PropertyRef::SPACING_ROLE;
/// Property reference for `padding_role` (§7.4).
pub const PADDING_ROLE: PropertyRef = PropertyRef::PADDING_ROLE;

// =============================================================================
// Ergonomic Event Type Constants (§7.6, §29)
// =============================================================================

/// Event type reference for momentary control activation (`ACTIVATE`, §7.6, §29).
pub const ACTIVATE: TypeRef = TypeRef::EVENT_ACTIVATE;
/// Event type reference for value change (`VALUE_CHANGED`, §7.6).
pub const VALUE_CHANGED: TypeRef = TypeRef::EVENT_VALUE_CHANGED;
/// Event type reference for selection change (`SELECTION_CHANGED`, §7.6).
pub const SELECTION_CHANGED: TypeRef = TypeRef::EVENT_SELECTION_CHANGED;
/// Event type reference for text edit (`TEXT_EDIT`, §7.6, §22.6).
pub const TEXT_EDIT: TypeRef = TypeRef::EVENT_TEXT_EDIT;
/// Event type reference for expansion toggle (`EXPANSION_CHANGED`, §7.6).
pub const EXPANSION_CHANGED: TypeRef = TypeRef::EVENT_EXPANSION_CHANGED;
/// Event type reference for viewport change (`VIEWPORT_CHANGED`, §7.6).
pub const VIEWPORT_CHANGED: TypeRef = TypeRef::EVENT_VIEWPORT_CHANGED;
