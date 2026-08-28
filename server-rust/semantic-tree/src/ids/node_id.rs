use std::fmt;

/// Strongly-typed identifier for a node in the semantic graph (§6.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u64);

impl NodeId {
    /// Creates a new `NodeId`.
    pub const fn new(id: u64) -> Self {
        Self(id)
    }

    /// Returns the underlying `u64` identifier.
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NodeId({})", self.0)
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

/// Strongly-typed identifier for an item within a collection / list / table (§6.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ItemId(pub u64);

impl ItemId {
    /// Creates a new `ItemId`.
    pub const fn new(id: u64) -> Self {
        Self(id)
    }

    /// Returns the underlying `u64` identifier.
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for ItemId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ItemId({})", self.0)
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

/// Strongly-typed identifier for a collection model in the semantic graph (§6.2, §8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModelId(pub u64);

impl ModelId {
    /// Creates a new `ModelId`.
    pub const fn new(id: u64) -> Self {
        Self(id)
    }

    /// Returns the underlying `u64` identifier.
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for ModelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ModelId({})", self.0)
    }
}

impl From<u64> for ModelId {
    fn from(id: u64) -> Self {
        Self(id)
    }
}

impl From<ModelId> for u64 {
    fn from(id: ModelId) -> Self {
        id.0
    }
}


/// 256-bit SHA-256 binary digest identifying content-addressed resources (§7.4, §8, §18).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ResourceHash(pub [u8; 32]);

impl ResourceHash {
    /// Creates a new `ResourceHash` from raw 32 bytes.
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Returns the raw 32-byte slice.
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// Returns lowercase hex string representation.
    pub fn to_hex(&self) -> String {
        let mut hex = String::with_capacity(64);
        for byte in &self.0 {
            use std::fmt::Write;
            let _ = write!(hex, "{:02x}", byte);
        }
        hex
    }

    /// Parses a 64-character hex string (or optional `sha256:` prefixed) into a `ResourceHash`.
    pub fn from_hex(s: &str) -> Result<Self, ParseResourceHashError> {
        let clean = s.strip_prefix("sha256:").unwrap_or(s);
        if clean.len() != 64 {
            return Err(ParseResourceHashError::InvalidLength(clean.len()));
        }

        let mut bytes = [0u8; 32];
        for i in 0..32 {
            let byte_hex = &clean[i * 2..i * 2 + 2];
            bytes[i] = u8::from_str_radix(byte_hex, 16)
                .map_err(|_| ParseResourceHashError::InvalidHexCharacter)?;
        }
        Ok(Self(bytes))
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

/// Error parsing a resource hash from string format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseResourceHashError {
    InvalidLength(usize),
    InvalidHexCharacter,
}

impl fmt::Display for ParseResourceHashError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLength(len) => {
                write!(f, "expected 64 hex characters, got length {}", len)
            }
            Self::InvalidHexCharacter => write!(f, "invalid hex character in resource hash"),
        }
    }
}

impl std::error::Error for ParseResourceHashError {}
