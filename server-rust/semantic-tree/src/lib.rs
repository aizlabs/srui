//! SRUI Semantic Tree: in-memory core types, identifiers, value representations, and node store.
//!
//! Conforms to SRUI Specification v0.4:
//! - §6.2: Node identity ([`NodeId`])
//! - §6.3: State ownership ([`SemanticStore`])
//! - §6.4: Type and property references, namespaces ([`TypeRef`], [`PropertyRef`], [`STANDARD_NAMESPACE_ID`])
//! - §6.5: Typed value representation ([`Value`], semantic tuples, lists, small records)
//! - §12: Persistent object graph and mutation stream
//! - §12.1: Revisions and transactions ([`Revision`], [`Transaction`], [`Operation`])
//! - §12.2: Commits are state-consistency boundaries, not render frames
//! - §13: Core mutation operations ([`SemanticStore`])
//! - §14: Resource model (large blobs are resources, not properties)
//! - §26: Mandatory limits ([`StoreLimits`])

pub mod ids;
pub mod model;
pub mod store;
pub mod transaction;
pub mod value;

pub use ids::{
    lookup_standard_enum, lookup_standard_event, lookup_standard_node_type, lookup_standard_operation,
    lookup_standard_property, resolve_standard_node_type, resolve_standard_property,
    standard_enum_name, standard_event_name, standard_node_type_name, standard_operation_name,
    standard_property_name, ItemId, ModelId, NodeId, ParseResourceHashError, PropertyRef,
    RegistryLookupError, ResourceHash, TypeRef, StandardActionRole, StandardHorizontalAlignment,
    StandardImportance, StandardInputRole, StandardPaddingRole, StandardSelectionMode,
    StandardSpacingRole, StandardTextRole, StandardTogglePresentationHint, StandardValidationState,
    StandardVerticalAlignment, StandardVisibility, STANDARD_ENUMS, STANDARD_EVENTS,
    STANDARD_NAMESPACE_ID, STANDARD_NODE_TYPES, STANDARD_OPERATIONS, STANDARD_PROPERTIES,
};

pub use model::{Model, ModelItem};

pub use store::{
    Node, SemanticStore, StoreError, StoreLimits, DEFAULT_MAX_NODE_COUNT,
    DEFAULT_MAX_STRING_LENGTH, DEFAULT_MAX_TRANSACTION_OPERATIONS, DEFAULT_MAX_TREE_DEPTH,
};

pub use transaction::{Operation, Revision, Transaction, TxnError};

pub use value::{
    EdgeInsets, EnumToken, Point, Property, Range, Rect, Size, SmallRecord, Value,
    ValueConversionError,
};

