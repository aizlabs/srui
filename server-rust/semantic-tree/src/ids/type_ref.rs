use std::fmt;

/// Strongly-typed reference to a node type in a namespace registry (§6.4, §7.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TypeRef {
    pub namespace_id: u32,
    pub local_id: u32,
}

impl TypeRef {
    /// Constructs a new `TypeRef`.
    pub const fn new(namespace_id: u32, local_id: u32) -> Self {
        Self {
            namespace_id,
            local_id,
        }
    }

    /// Constructs a standard namespace (0) `TypeRef`.
    pub const fn standard(local_id: u32) -> Self {
        Self {
            namespace_id: 0,
            local_id,
        }
    }

    /// Returns `true` if this `TypeRef` belongs to the standard namespace (0).
    pub const fn is_standard(&self) -> bool {
        self.namespace_id == 0
    }

    /// Looks up the standard string name of this node type if it belongs to namespace 0.
    pub fn standard_name(&self) -> Option<&'static str> {
        if self.is_standard() {
            super::standard_node_type_name(self.local_id)
        } else {
            None
        }
    }

    /// Looks up the standard string name of this event type if it belongs to namespace 0 (§7.6).
    pub fn standard_event_name(&self) -> Option<&'static str> {
        if self.is_standard() {
            super::standard_event_name(self.local_id)
        } else {
            None
        }
    }

    /// Resolves a standard node type name to a standard `TypeRef` (§7.2).
    pub fn resolve_standard(name: &str) -> Result<Self, super::RegistryLookupError> {
        super::resolve_standard_node_type(name)
    }

    /// Resolves a standard event name to a standard `TypeRef` (§7.6).
    pub fn resolve_standard_event(name: &str) -> Result<Self, super::RegistryLookupError> {
        super::resolve_standard_event(name)
    }
}

impl fmt::Display for TypeRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_standard() {
            if let Some(name) = self.standard_name() {
                return write!(f, "TypeRef(standard:{}/{})", self.local_id, name);
            }
        }
        write!(f, "TypeRef({}:{})", self.namespace_id, self.local_id)
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
