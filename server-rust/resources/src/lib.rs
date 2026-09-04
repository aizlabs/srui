//! SRUI Resource CAS (§14, §19.2)
//!
//! Content-addressed immutable binary store used by the server runtime to retain
//! and chunk-deliver resources. Spec sections implemented:
//! - §14 Resource model: SHA-256 addressing, media type, encoded length, optional
//!   decoded dimensions, priority; chunked transfer so large images cannot block
//!   latency-sensitive UI/control traffic.
//! - §19.2 Priority classes: resource payloads are low-priority relative to UI;
//!   chunk payloads stay within the documented 16–32 KiB range.

mod chunk;
mod store;

pub use chunk::{chunk_payloads, ResourceChunkIter, CHUNK_PAYLOAD_SIZE};
pub use store::{
    infer_media_type, PublishOutcome, ResourceEntry, ResourceError, ResourceLimits, ResourceStore,
};

/// Default encoded-byte ceiling for a single resource (§14, §26).
pub const DEFAULT_MAX_RESOURCE_BYTES: usize = 50 * 1024 * 1024;

/// Default maximum number of distinct retained resources in one CAS.
pub const DEFAULT_MAX_RESOURCE_ENTRIES: usize = 1024;

/// Default aggregate retained byte budget across all resources in one CAS.
pub const DEFAULT_MAX_TOTAL_RESOURCE_BYTES: usize = 256 * 1024 * 1024;
