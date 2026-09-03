//! Bounded SHA-256 content-addressed resource store (§14).

use std::collections::HashMap;
use std::sync::Arc;

use sha2::{Digest, Sha256};
use srui_semantic_tree::ResourceHash;
use thiserror::Error;

use crate::{
    DEFAULT_MAX_RESOURCE_BYTES, DEFAULT_MAX_RESOURCE_ENTRIES, DEFAULT_MAX_TOTAL_RESOURCE_BYTES,
};

/// Configurable ceilings for a [`ResourceStore`] (§14, §26).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceLimits {
    /// Maximum encoded bytes accepted for a single publish.
    pub max_resource_bytes: usize,
    /// Maximum number of distinct retained hashes.
    pub max_entries: usize,
    /// Maximum aggregate retained bytes across all entries.
    pub max_total_bytes: usize,
}

impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            max_resource_bytes: DEFAULT_MAX_RESOURCE_BYTES,
            max_entries: DEFAULT_MAX_RESOURCE_ENTRIES,
            max_total_bytes: DEFAULT_MAX_TOTAL_RESOURCE_BYTES,
        }
    }
}

/// Errors produced by resource publication and lookup (§14).
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ResourceError {
    /// Encoded payload exceeds the configured per-resource ceiling.
    #[error("resource encoded length {length} exceeds limit {limit}")]
    ResourceTooLarge { length: usize, limit: usize },

    /// CAS entry count would exceed the configured bound.
    #[error("resource store entry count would exceed limit {limit}")]
    EntryLimitExceeded { limit: usize },

    /// Aggregate retained bytes would exceed the configured bound.
    #[error("resource store total bytes would exceed limit {limit}")]
    TotalBytesLimitExceeded { limit: usize },
}

/// Immutable retained resource entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceEntry {
    /// Canonical content hash.
    pub hash: ResourceHash,
    /// Inferred or declared media type.
    pub media_type: String,
    /// Encoded payload length in bytes.
    pub encoded_length: u64,
    /// Immutable encoded bytes shared across subscribers.
    pub bytes: Arc<[u8]>,
}

/// Outcome of [`ResourceStore::publish_resource`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishOutcome {
    /// Canonical hash of the published bytes.
    pub hash: ResourceHash,
    /// `true` when this call inserted a new CAS entry; `false` on dedupe hit.
    pub inserted: bool,
    /// Retained entry (existing or newly inserted).
    pub entry: ResourceEntry,
}

/// Bounded content-addressed store of immutable resource bytes (§14).
///
/// Identical content always yields the same [`ResourceHash`]. An existing hash is
/// never overwritten; republishing returns the retained entry without duplicating
/// storage.
#[derive(Debug, Clone, Default)]
pub struct ResourceStore {
    limits: ResourceLimits,
    entries: HashMap<ResourceHash, ResourceEntry>,
    /// Insertion order for bounded enumeration / bootstrap seeding.
    order: Vec<ResourceHash>,
    total_bytes: usize,
}

impl ResourceStore {
    /// Creates an empty store with default limits.
    #[must_use]
    pub fn new() -> Self {
        Self::with_limits(ResourceLimits::default())
    }

    /// Creates an empty store with explicit limits.
    #[must_use]
    pub fn with_limits(limits: ResourceLimits) -> Self {
        Self {
            limits,
            entries: HashMap::new(),
            order: Vec::new(),
            total_bytes: 0,
        }
    }

    /// Returns the store's configured limits.
    #[must_use]
    pub fn limits(&self) -> ResourceLimits {
        self.limits
    }

    /// Number of distinct retained resources.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns `true` when no resources are retained.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Aggregate retained encoded bytes.
    #[must_use]
    pub fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    /// Publishes `bytes` into the CAS, returning the canonical hash.
    ///
    /// Oversized payloads are rejected before hashing or copying into store-owned
    /// memory. Identical content is deduplicated and never overwrites an existing
    /// hash.
    pub fn publish_resource(
        &mut self,
        bytes: impl AsRef<[u8]>,
    ) -> Result<PublishOutcome, ResourceError> {
        let slice = bytes.as_ref();
        if slice.len() > self.limits.max_resource_bytes {
            return Err(ResourceError::ResourceTooLarge {
                length: slice.len(),
                limit: self.limits.max_resource_bytes,
            });
        }

        let hash = hash_bytes(slice);
        if let Some(existing) = self.entries.get(&hash) {
            return Ok(PublishOutcome {
                hash,
                inserted: false,
                entry: existing.clone(),
            });
        }

        if self.entries.len() >= self.limits.max_entries {
            return Err(ResourceError::EntryLimitExceeded {
                limit: self.limits.max_entries,
            });
        }
        let next_total = self.total_bytes.checked_add(slice.len()).ok_or(
            ResourceError::TotalBytesLimitExceeded {
                limit: self.limits.max_total_bytes,
            },
        )?;
        if next_total > self.limits.max_total_bytes {
            return Err(ResourceError::TotalBytesLimitExceeded {
                limit: self.limits.max_total_bytes,
            });
        }

        let owned: Arc<[u8]> = Arc::from(slice.to_vec().into_boxed_slice());
        let entry = ResourceEntry {
            hash,
            media_type: infer_media_type(slice).to_string(),
            encoded_length: owned.len() as u64,
            bytes: Arc::clone(&owned),
        };
        self.entries.insert(hash, entry.clone());
        self.order.push(hash);
        self.total_bytes = next_total;
        Ok(PublishOutcome {
            hash,
            inserted: true,
            entry,
        })
    }

    /// Looks up a retained resource by hash.
    #[must_use]
    pub fn lookup(&self, hash: &ResourceHash) -> Option<&ResourceEntry> {
        self.entries.get(hash)
    }

    /// Returns `true` when `hash` is retained.
    #[must_use]
    pub fn contains(&self, hash: &ResourceHash) -> bool {
        self.entries.contains_key(hash)
    }

    /// Enumerates retained resources in insertion order (bounded by store size).
    pub fn iter(&self) -> impl Iterator<Item = &ResourceEntry> {
        self.order.iter().filter_map(|hash| self.entries.get(hash))
    }

    /// Collects retained entries for connection bootstrap seeding.
    #[must_use]
    pub fn retained_entries(&self) -> Vec<ResourceEntry> {
        self.iter().cloned().collect()
    }
}

/// SHA-256 digest of `bytes` as a [`ResourceHash`].
#[must_use]
pub fn hash_bytes(bytes: &[u8]) -> ResourceHash {
    let digest = Sha256::digest(bytes);
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&digest);
    ResourceHash::new(arr)
}

/// Infers a common raster MIME type from well-known signatures.
///
/// Unknown payloads become `application/octet-stream`. Fonts are intentionally
/// not given a decoding path in v0.1 (§14).
#[must_use]
pub fn infer_media_type(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n']) {
        return "image/png";
    }
    if bytes.len() >= 3 && bytes[0] == 0xFF && bytes[1] == 0xD8 && bytes[2] == 0xFF {
        return "image/jpeg";
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return "image/gif";
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return "image/webp";
    }
    if bytes.starts_with(b"BM") {
        return "image/bmp";
    }
    "application/octet-stream"
}
