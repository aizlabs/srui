//! SRUI Semantic Tree: in-memory core types, identifiers, value representations, and node store.
//!
//! Conforms to SRUI Specification v0.4:
//! - §4: Architectural invariants (§4 inv. 13 unknown required semantics fail explicitly)
//! - §6.1: Core object list ([`Event`], [`CapabilitySet`])
//! - §6.2: Node identity ([`NodeId`])
//! - §6.3: State ownership ([`SemanticStore`])
//! - §6.4: Type and property references, namespaces ([`TypeRef`], [`PropertyRef`], [`STANDARD_NAMESPACE_ID`])
//! - §6.5: Typed value representation ([`Value`], semantic tuples, lists, small records)
//! - §7.6: Standard events ([`Event`], [`STANDARD_EVENTS`])
//! - §7.7: Semantic input routing ([`Event`], [`EventId`])
//! - §12: Persistent object graph and mutation stream
//! - §12.1: Revisions and transactions ([`Revision`], [`Transaction`], [`Operation`])
//! - §12.2: Commits are state-consistency boundaries, not render frames
//! - §13: Core mutation operations ([`SemanticStore`])
//! - §14: Resource model (large blobs are resources, not properties)
//! - §15: Capability negotiation ([`CapabilitySet`], [`Profile`], [`ServerCapabilities`])
//! - §26: Mandatory limits ([`StoreLimits`])

pub mod capability;
pub mod event;
pub mod ids;
pub mod model;
pub mod store;
pub mod transaction;
pub mod value;
pub mod wire;

pub use capability::{
    CapabilitySet, NegotiationError, ParseProfileError, Profile, ServerCapabilities,
    PROFILE_CODING, PROFILE_MEDIA_SURFACE, PROFILE_RICHTEXT, PROFILE_STANDARD_WIDGETS,
    PROFILE_TERMINAL, PROFILE_VECTOR_SCENE,
};

pub use event::{ClientInstanceId, Event, EventId, EventValidationError};

pub use ids::{
    lookup_standard_enum, lookup_standard_enum_value, lookup_standard_event,
    lookup_standard_node_type, lookup_standard_operation, lookup_standard_property,
    resolve_standard_enum_value, resolve_standard_event, resolve_standard_node_type,
    resolve_standard_property, standard_enum_name, standard_enum_value_name, standard_event_name,
    standard_node_type_name, standard_operation_name, standard_property_name, ItemId, ModelId,
    NodeId, ParseResourceHashError, PropertyRef, RegistryLookupError, ResourceHash, TypeRef,
    StandardActionRole, StandardHorizontalAlignment, StandardImportance, StandardInputRole,
    StandardPaddingRole, StandardSelectionMode, StandardSpacingRole, StandardTextRole,
    StandardTogglePresentationHint, StandardValidationState, StandardVerticalAlignment,
    StandardVisibility, STANDARD_ENUMS, STANDARD_EVENTS, STANDARD_NAMESPACE_ID,
    STANDARD_NODE_TYPES, STANDARD_OPERATIONS, STANDARD_PROPERTIES,
};

pub use model::{Model, ModelItem};

pub use store::{
    Node, SemanticStore, StoreError, StoreLimits, DEFAULT_MAX_CACHED_ITEMS_PER_MODEL,
    DEFAULT_MAX_ITEMS_PER_MODEL_OPERATION, DEFAULT_MAX_MODEL_COUNT, DEFAULT_MAX_NODE_COUNT,
    DEFAULT_MAX_STRING_LENGTH, DEFAULT_MAX_TRANSACTION_OPERATIONS, DEFAULT_MAX_TREE_DEPTH,
};

pub use transaction::{Operation, Revision, Transaction, TxnError};

pub use value::{
    EdgeInsets, EnumToken, Point, Property, Range, Rect, Size, SmallRecord, Value,
    ValueConversionError,
};

pub use wire::{
    decode_event, decode_message, decode_node_record, decode_operation, decode_transaction,
    decode_value, encode_event, encode_message, encode_node_record, encode_operation,
    encode_transaction, encode_value, NodeRecord, WireError,
};

