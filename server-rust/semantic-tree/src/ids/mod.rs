//! Semantic graph identifier types and standard registry constants.

pub mod node_id;
pub mod property_ref;
pub mod type_ref;

pub use node_id::{ItemId, ModelId, NodeId, ParseResourceHashError, ResourceHash};
pub use property_ref::PropertyRef;
pub use srui_protocol::STANDARD_NAMESPACE_ID;
pub use type_ref::TypeRef;

use crate::value::EnumToken;
use std::fmt;

// Include auto-generated lookup tables and constants from registry.yaml (§7.2, §7.4, §7.5)
include!(concat!(env!("OUT_DIR"), "/registry_tables.rs"));

/// Error returned when resolving a registry symbol name fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryLookupError {
    UnknownNodeType(String),
    UnknownProperty(String),
    UnknownEnum(String),
    UnknownEvent(String),
    UnknownOperation(String),
}

impl fmt::Display for RegistryLookupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownNodeType(name) => write!(f, "unknown standard node type: {:?}", name),
            Self::UnknownProperty(name) => write!(f, "unknown standard property: {:?}", name),
            Self::UnknownEnum(name) => write!(f, "unknown standard enum: {:?}", name),
            Self::UnknownEvent(name) => write!(f, "unknown standard event: {:?}", name),
            Self::UnknownOperation(name) => write!(f, "unknown standard operation: {:?}", name),
        }
    }
}

impl std::error::Error for RegistryLookupError {}

/// Resolves a standard node type name to a standard namespace 0 `TypeRef`.
pub fn resolve_standard_node_type(name: &str) -> Result<TypeRef, RegistryLookupError> {
    lookup_standard_node_type(name)
        .map(TypeRef::standard)
        .ok_or_else(|| RegistryLookupError::UnknownNodeType(name.to_string()))
}

/// Resolves a standard property name to a standard namespace 0 `PropertyRef`.
pub fn resolve_standard_property(name: &str) -> Result<PropertyRef, RegistryLookupError> {
    lookup_standard_property(name)
        .map(PropertyRef::standard)
        .ok_or_else(|| RegistryLookupError::UnknownProperty(name.to_string()))
}

/// Resolves a standard event name to a standard namespace 0 `TypeRef`.
pub fn resolve_standard_event(name: &str) -> Result<TypeRef, RegistryLookupError> {
    lookup_standard_event(name)
        .map(TypeRef::standard)
        .ok_or_else(|| RegistryLookupError::UnknownEvent(name.to_string()))
}
