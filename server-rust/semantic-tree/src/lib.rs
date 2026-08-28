//! SRUI Semantic Tree: in-memory core types, identifiers, and value representations.
//!
//! Conforms to SRUI Specification v0.4:
//! - §6.2: Node identity ([`NodeId`])
//! - §6.4: Type and property references, namespaces ([`TypeRef`], [`PropertyRef`], [`STANDARD_NAMESPACE_ID`])
//! - §6.5: Typed value representation ([`Value`], semantic tuples, lists, small records)
//! - §14: Resource model (large blobs are resources, not properties)

pub mod ids;
pub mod value;

pub use ids::{
    lookup_standard_enum, lookup_standard_event, lookup_standard_node_type, lookup_standard_operation,
    lookup_standard_property, resolve_standard_node_type, resolve_standard_property,
    standard_enum_name, standard_event_name, standard_node_type_name, standard_operation_name,
    standard_property_name, ItemId, NodeId, ParseResourceHashError, PropertyRef,
    RegistryLookupError, ResourceHash, TypeRef, STANDARD_ENUMS, STANDARD_EVENTS,
    STANDARD_NAMESPACE_ID, STANDARD_NODE_TYPES, STANDARD_OPERATIONS, STANDARD_PROPERTIES,
};

pub use value::{
    EdgeInsets, EdgeInsetsVal, EnumToken, EnumValue, Point, PointVal, Property, Range, RangeVal,
    Rect, RectVal, Size, SizeVal, SmallRecord, Value, ValueConversionError,
};
