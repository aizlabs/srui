//! Identifier newtypes and namespace-qualified references.
//!
//! Conforms to SRUI Specification v0.4:
//! - §6.2: Node identity (`NodeId`)
//! - §6.4: Type and property references, namespaces (`TypeRef`, `PropertyRef`, `STANDARD_NAMESPACE_ID`)
//! - §6.5: Value references (`ItemId`, `ResourceHash`)
//!
//! Includes build-time lookup helpers generated from `protocol/registry.yaml`.

use std::fmt;
use std::ops::Deref;

/// Namespace 0 is permanently reserved for the SRUI standard registry (§6.4).
pub const STANDARD_NAMESPACE_ID: u32 = 0;

/// Session-scoped unique identifier for a semantic node (§6.2).
///
/// Rules per §6.2:
/// - `NodeId` is session-scoped.
/// - A node ID MUST NOT be reused during the same session.
/// - Stable IDs make event routing, replay, accessibility identity, and reconnect deterministic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct NodeId(pub u64);

impl NodeId {
    /// Creates a new `NodeId`.
    pub const fn new(id: u64) -> Self {
        Self(id)
    }

    /// Returns the raw `u64` identifier.
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl From<u64> for NodeId {
    fn from(id: u64) -> Self {
        Self(id)
    }
}

impl From<NodeId> for u64 {
    fn from(id: NodeId) -> Self {
        id.0
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// Stable collection item identifier within a collection model (§6.5, §8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct ItemId(pub u64);

impl ItemId {
    /// Creates a new `ItemId`.
    pub const fn new(id: u64) -> Self {
        Self(id)
    }

    /// Returns the raw `u64` identifier.
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl From<u64> for ItemId {
    fn from(id: u64) -> Self {
        Self(id)
    }
}

impl From<ItemId> for u64 {
    fn from(id: ItemId) -> Self {
        id.0
    }
}

impl fmt::Display for ItemId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "item:{}", self.0)
    }
}

/// Content-addressed 32-byte SHA-256 identifier for immutable binary resources (§6.5, §14).
///
/// Large binary content is a Resource, not a Value (per §6.5). Resources are addressed
/// strictly by their 32-byte cryptographic SHA-256 hash.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct ResourceHash(pub [u8; 32]);

impl ResourceHash {
    /// Creates a `ResourceHash` from a 32-byte array.
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Returns a reference to the underlying 32-byte array.
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Consumes the wrapper and returns the 32-byte array.
    pub const fn into_bytes(self) -> [u8; 32] {
        self.0
    }

    /// Formats the hash as a standard lowercase 64-character hex string.
    pub fn to_hex(&self) -> String {
        let mut s = String::with_capacity(64);
        for byte in &self.0 {
            use std::fmt::Write;
            let _ = write!(s, "{:02x}", byte);
        }
        s
    }

    /// Parses a 64-character hex string (with optional `"sha256:"` prefix) into a `ResourceHash`.
    pub fn from_hex(hex_str: &str) -> Result<Self, ParseResourceHashError> {
        let clean = hex_str.strip_prefix("sha256:").unwrap_or(hex_str);
        if clean.len() != 64 {
            return Err(ParseResourceHashError::InvalidLength(clean.len()));
        }
        let mut bytes = [0u8; 32];
        for i in 0..32 {
            let chunk = &clean[i * 2..i * 2 + 2];
            bytes[i] = u8::from_str_radix(chunk, 16)
                .map_err(|_| ParseResourceHashError::InvalidHexCharacter)?;
        }
        Ok(Self(bytes))
    }
}

impl fmt::Debug for ResourceHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ResourceHash(sha256:{})", self.to_hex())
    }
}

impl fmt::Display for ResourceHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "sha256:{}", self.to_hex())
    }
}

impl From<[u8; 32]> for ResourceHash {
    fn from(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl From<ResourceHash> for [u8; 32] {
    fn from(hash: ResourceHash) -> Self {
        hash.0
    }
}

impl TryFrom<&[u8]> for ResourceHash {
    type Error = ParseResourceHashError;

    fn try_from(slice: &[u8]) -> Result<Self, Self::Error> {
        if slice.len() != 32 {
            return Err(ParseResourceHashError::InvalidLength(slice.len()));
        }
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(slice);
        Ok(Self(bytes))
    }
}

impl TryFrom<Vec<u8>> for ResourceHash {
    type Error = ParseResourceHashError;

    fn try_from(vec: Vec<u8>) -> Result<Self, Self::Error> {
        Self::try_from(vec.as_slice())
    }
}

impl AsRef<[u8]> for ResourceHash {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl AsRef<[u8; 32]> for ResourceHash {
    fn as_ref(&self) -> &[u8; 32] {
        &self.0
    }
}

impl Deref for ResourceHash {
    type Target = [u8; 32];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Error returned when parsing a [`ResourceHash`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseResourceHashError {
    /// The input length did not match the expected 32 bytes or 64 hex characters.
    InvalidLength(usize),
    /// A non-hexadecimal character was encountered.
    InvalidHexCharacter,
}

impl fmt::Display for ParseResourceHashError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLength(len) => write!(
                f,
                "invalid resource hash length: expected 32 bytes / 64 hex chars, got {}",
                len
            ),
            Self::InvalidHexCharacter => write!(f, "invalid hex character in resource hash string"),
        }
    }
}

impl std::error::Error for ParseResourceHashError {}

/// Compact reference to a semantic node type or record type (§6.4).
///
/// Standard types reside in `namespace_id = 0` (matching `protocol/registry.yaml`).
/// Extension types use session-negotiated namespace IDs (`namespace_id > 0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct TypeRef {
    /// Namespace identifier (`0` for standard registry, assigned ID for extensions).
    pub namespace_id: u32,
    /// Local type identifier within the namespace.
    pub local_id: u32,
}

impl TypeRef {
    /// Constructs a `TypeRef` with given namespace and local IDs.
    pub const fn new(namespace_id: u32, local_id: u32) -> Self {
        Self {
            namespace_id,
            local_id,
        }
    }

    /// Constructs a standard `TypeRef` in namespace 0.
    pub const fn standard(local_id: u32) -> Self {
        Self {
            namespace_id: STANDARD_NAMESPACE_ID,
            local_id,
        }
    }

    /// Returns `true` if this reference belongs to standard namespace 0.
    pub const fn is_standard(&self) -> bool {
        self.namespace_id == STANDARD_NAMESPACE_ID
    }

    /// Resolves a standard node type name (e.g. `"Button"`, `"Surface"`) to a standard `TypeRef`.
    ///
    /// Returns `Ok(TypeRef)` if recognized in namespace 0, or `Err(RegistryLookupError)` otherwise.
    pub fn resolve_standard(name: &str) -> Result<Self, RegistryLookupError> {
        lookup_standard_node_type(name)
            .map(Self::standard)
            .ok_or_else(|| RegistryLookupError::UnknownNodeType(name.to_string()))
    }

    /// Resolves a standard node type name to an `Option<TypeRef>`.
    pub fn from_standard_name(name: &str) -> Option<Self> {
        lookup_standard_node_type(name).map(Self::standard)
    }

    /// Looks up the standard name for this `TypeRef` if it belongs to namespace 0.
    pub fn standard_name(&self) -> Option<&'static str> {
        if self.is_standard() {
            standard_node_type_name(self.local_id)
        } else {
            None
        }
    }
}

impl fmt::Display for TypeRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(name) = self.standard_name() {
            write!(f, "TypeRef(0:{} \"{}\")", self.local_id, name)
        } else {
            write!(f, "TypeRef({}:{})", self.namespace_id, self.local_id)
        }
    }
}

impl From<srui_protocol::TypeRef> for TypeRef {
    fn from(wire: srui_protocol::TypeRef) -> Self {
        Self {
            namespace_id: wire.namespace_id,
            local_id: wire.local_id,
        }
    }
}

impl From<TypeRef> for srui_protocol::TypeRef {
    fn from(type_ref: TypeRef) -> Self {
        Self {
            namespace_id: type_ref.namespace_id,
            local_id: type_ref.local_id,
        }
    }
}

/// Compact reference to a semantic property (§6.4).
///
/// Standard properties reside in `namespace_id = 0` (matching `protocol/registry.yaml`).
/// Extension properties use session-negotiated namespace IDs (`namespace_id > 0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct PropertyRef {
    /// Namespace identifier (`0` for standard registry, assigned ID for extensions).
    pub namespace_id: u32,
    /// Local property identifier within the namespace.
    pub local_id: u32,
}

impl PropertyRef {
    /// Constructs a `PropertyRef` with given namespace and local IDs.
    pub const fn new(namespace_id: u32, local_id: u32) -> Self {
        Self {
            namespace_id,
            local_id,
        }
    }

    /// Constructs a standard `PropertyRef` in namespace 0.
    pub const fn standard(local_id: u32) -> Self {
        Self {
            namespace_id: STANDARD_NAMESPACE_ID,
            local_id,
        }
    }

    /// Returns `true` if this reference belongs to standard namespace 0.
    pub const fn is_standard(&self) -> bool {
        self.namespace_id == STANDARD_NAMESPACE_ID
    }

    /// Resolves a standard property name (e.g. `"label"`, `"value"`) to a standard `PropertyRef`.
    ///
    /// Returns `Ok(PropertyRef)` if recognized in namespace 0, or `Err(RegistryLookupError)` otherwise.
    pub fn resolve_standard(name: &str) -> Result<Self, RegistryLookupError> {
        lookup_standard_property(name)
            .map(Self::standard)
            .ok_or_else(|| RegistryLookupError::UnknownProperty(name.to_string()))
    }

    /// Resolves a standard property name to an `Option<PropertyRef>`.
    pub fn from_standard_name(name: &str) -> Option<Self> {
        lookup_standard_property(name).map(Self::standard)
    }

    /// Looks up the standard name for this `PropertyRef` if it belongs to namespace 0.
    pub fn standard_name(&self) -> Option<&'static str> {
        if self.is_standard() {
            standard_property_name(self.local_id)
        } else {
            None
        }
    }
}

// Include build-time generated lookup tables and constants from protocol/registry.yaml.
include!(concat!(env!("OUT_DIR"), "/registry_tables.rs"));

impl fmt::Display for PropertyRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(name) = self.standard_name() {
            write!(f, "PropertyRef(0:{} \"{}\")", self.local_id, name)
        } else {
            write!(f, "PropertyRef({}:{})", self.namespace_id, self.local_id)
        }
    }
}

impl From<srui_protocol::PropertyRef> for PropertyRef {
    fn from(wire: srui_protocol::PropertyRef) -> Self {
        Self {
            namespace_id: wire.namespace_id,
            local_id: wire.local_id,
        }
    }
}

impl From<PropertyRef> for srui_protocol::PropertyRef {
    fn from(prop_ref: PropertyRef) -> Self {
        Self {
            namespace_id: prop_ref.namespace_id,
            local_id: prop_ref.local_id,
        }
    }
}

/// Standalone helper to resolve a standard node type name to a standard `TypeRef`.
pub fn resolve_standard_node_type(name: &str) -> Result<TypeRef, RegistryLookupError> {
    TypeRef::resolve_standard(name)
}

/// Standalone helper to resolve a standard property name to a standard `PropertyRef`.
pub fn resolve_standard_property(name: &str) -> Result<PropertyRef, RegistryLookupError> {
    PropertyRef::resolve_standard(name)
}

/// Error returned when resolving an unknown identifier against the standard registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistryLookupError {
    /// The specified node type name was not found in the standard registry.
    UnknownNodeType(String),
    /// The specified property name was not found in the standard registry.
    UnknownProperty(String),
}

impl fmt::Display for RegistryLookupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownNodeType(name) => write!(
                f,
                "unknown standard node type: {:?} (not in registry.yaml namespace 0)",
                name
            ),
            Self::UnknownProperty(name) => write!(
                f,
                "unknown standard property: {:?} (not in registry.yaml namespace 0)",
                name
            ),
        }
    }
}

impl std::error::Error for RegistryLookupError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn test_node_id_newtype() {
        let id = NodeId::new(42);
        assert_eq!(id.get(), 42);
        assert_eq!(u64::from(id), 42);
        assert_eq!(NodeId::from(42), id);
        assert_eq!(format!("{}", id), "#42");
    }

    #[test]
    fn test_item_id_newtype() {
        let id = ItemId::new(999);
        assert_eq!(id.get(), 999);
        assert_eq!(u64::from(id), 999);
        assert_eq!(ItemId::from(999), id);
        assert_eq!(format!("{}", id), "item:999");
    }

    #[test]
    fn test_resource_hash() {
        let raw = [0xabu8; 32];
        let hash = ResourceHash::new(raw);
        assert_eq!(hash.as_bytes(), &raw);
        assert_eq!(hash.into_bytes(), raw);
        assert_eq!(hash[0], 0xab);

        let hex = hash.to_hex();
        assert_eq!(hex.len(), 64);
        assert_eq!(hex, "abababababababababababababababababababababababababababababababab");

        let parsed = ResourceHash::from_hex(&hex).expect("parse hex");
        assert_eq!(parsed, hash);

        let prefixed = format!("sha256:{}", hex);
        let parsed_prefixed = ResourceHash::from_hex(&prefixed).expect("parse prefixed");
        assert_eq!(parsed_prefixed, hash);

        assert!(ResourceHash::from_hex("short").is_err());
        assert!(ResourceHash::from_hex("invalid_characters_zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz").is_err());
    }

    #[test]
    fn test_type_ref_equality_and_hashing() {
        let t1 = TypeRef::new(0, 11);
        let t2 = TypeRef::standard(11);
        let t3 = TypeRef::BUTTON;
        let t_ext = TypeRef::new(1, 11);

        assert_eq!(t1, t2);
        assert_eq!(t2, t3);
        assert_ne!(t1, t_ext);

        let mut set = HashSet::new();
        set.insert(t1);
        assert!(set.contains(&t2));
        assert!(set.contains(&t3));
        assert!(!set.contains(&t_ext));
    }

    #[test]
    fn test_property_ref_equality_and_hashing() {
        let p1 = PropertyRef::new(0, 1);
        let p2 = PropertyRef::standard(1);
        let p3 = PropertyRef::LABEL;
        let p_ext = PropertyRef::new(2, 1);

        assert_eq!(p1, p2);
        assert_eq!(p2, p3);
        assert_ne!(p1, p_ext);

        let mut set = HashSet::new();
        set.insert(p1);
        assert!(set.contains(&p2));
        assert!(set.contains(&p3));
        assert!(!set.contains(&p_ext));
    }

    #[test]
    fn test_resolve_known_registry_node_types() {
        assert_eq!(TypeRef::resolve_standard("Button").unwrap(), TypeRef::BUTTON);
        assert_eq!(TypeRef::resolve_standard("Button").unwrap().local_id, 11);
        assert_eq!(TypeRef::resolve_standard("Surface").unwrap(), TypeRef::SURFACE);
        assert_eq!(TypeRef::resolve_standard("Surface").unwrap().local_id, 1);
        assert_eq!(TypeRef::resolve_standard("Text").unwrap(), TypeRef::TEXT);
        assert_eq!(TypeRef::resolve_standard("Text").unwrap().local_id, 9);
        assert_eq!(TypeRef::resolve_standard("Toolbar").unwrap(), TypeRef::TOOLBAR);
        assert_eq!(TypeRef::resolve_standard("Toolbar").unwrap().local_id, 27);

        assert_eq!(TypeRef::BUTTON.standard_name(), Some("Button"));
        assert_eq!(TypeRef::SURFACE.standard_name(), Some("Surface"));
    }

    #[test]
    fn test_resolve_known_registry_properties() {
        assert_eq!(PropertyRef::resolve_standard("label").unwrap(), PropertyRef::LABEL);
        assert_eq!(PropertyRef::resolve_standard("label").unwrap().local_id, 1);
        assert_eq!(PropertyRef::resolve_standard("value").unwrap(), PropertyRef::VALUE);
        assert_eq!(PropertyRef::resolve_standard("value").unwrap().local_id, 13);
        assert_eq!(PropertyRef::resolve_standard("enabled").unwrap(), PropertyRef::ENABLED);
        assert_eq!(PropertyRef::resolve_standard("enabled").unwrap().local_id, 7);
        assert_eq!(PropertyRef::resolve_standard("selection_mode").unwrap(), PropertyRef::SELECTION_MODE);
        assert_eq!(PropertyRef::resolve_standard("selection_mode").unwrap().local_id, 30);

        assert_eq!(PropertyRef::LABEL.standard_name(), Some("label"));
        assert_eq!(PropertyRef::VALUE.standard_name(), Some("value"));
    }

    #[test]
    fn test_resolve_unknown_name_fails_clearly() {
        let node_res = TypeRef::resolve_standard("NonExistentWidget");
        assert!(matches!(node_res, Err(RegistryLookupError::UnknownNodeType(name)) if name == "NonExistentWidget"));

        let prop_res = PropertyRef::resolve_standard("non_existent_property");
        assert!(matches!(prop_res, Err(RegistryLookupError::UnknownProperty(name)) if name == "non_existent_property"));
    }

    #[test]
    fn test_wire_proto_conversion() {
        let type_ref = TypeRef::BUTTON;
        let wire_type: srui_protocol::TypeRef = type_ref.into();
        assert_eq!(wire_type.namespace_id, 0);
        assert_eq!(wire_type.local_id, 11);
        let back_type: TypeRef = wire_type.into();
        assert_eq!(back_type, type_ref);

        let prop_ref = PropertyRef::LABEL;
        let wire_prop: srui_protocol::PropertyRef = prop_ref.into();
        assert_eq!(wire_prop.namespace_id, 0);
        assert_eq!(wire_prop.local_id, 1);
        let back_prop: PropertyRef = wire_prop.into();
        assert_eq!(back_prop, prop_ref);
    }
}
