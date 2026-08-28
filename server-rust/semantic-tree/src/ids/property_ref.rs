use std::fmt;

/// Strongly-typed reference to a property definition in a namespace registry (§6.4, §7.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PropertyRef {
    pub namespace_id: u32,
    pub local_id: u32,
}

impl PropertyRef {
    /// Constructs a new `PropertyRef`.
    pub const fn new(namespace_id: u32, local_id: u32) -> Self {
        Self {
            namespace_id,
            local_id,
        }
    }

    /// Constructs a standard namespace (0) `PropertyRef`.
    pub const fn standard(local_id: u32) -> Self {
        Self {
            namespace_id: 0,
            local_id,
        }
    }

    /// Returns `true` if this `PropertyRef` belongs to the standard namespace (0).
    pub const fn is_standard(&self) -> bool {
        self.namespace_id == 0
    }

    /// Looks up the standard string name of this property if it belongs to namespace 0.
    pub fn standard_name(&self) -> Option<&'static str> {
        if self.is_standard() {
            super::standard_property_name(self.local_id)
        } else {
            None
        }
    }

    /// Resolves a standard property name to a standard `PropertyRef`.
    pub fn resolve_standard(name: &str) -> Result<Self, super::RegistryLookupError> {
        super::resolve_standard_property(name)
    }
}

impl fmt::Display for PropertyRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_standard() {
            if let Some(name) = self.standard_name() {
                return write!(f, "PropertyRef(standard:{}/{})", self.local_id, name);
            }
        }
        write!(f, "PropertyRef({}:{})", self.namespace_id, self.local_id)
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
