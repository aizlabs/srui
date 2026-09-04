//! Deterministic fixed-size resource chunk iteration (§14, §19.2).

use std::sync::Arc;

/// Encoded payload size per resource chunk (16 KiB).
///
/// Stays strictly below the documented 16–32 KiB range ceiling so framing,
/// envelope, and hash overhead never push a chunk frame into the next class.
pub const CHUNK_PAYLOAD_SIZE: usize = 16_384;

/// One contiguous slice of a published resource's encoded bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceChunkView {
    /// Byte offset of `data` within the full encoded resource.
    pub byte_offset: u64,
    /// Chunk payload bytes (at most [`CHUNK_PAYLOAD_SIZE`]).
    pub data: Arc<[u8]>,
}

/// Iterator yielding fixed-size contiguous chunks of `bytes`.
#[derive(Debug, Clone)]
pub struct ResourceChunkIter {
    bytes: Arc<[u8]>,
    offset: usize,
}

impl ResourceChunkIter {
    /// Creates an iterator over `bytes` using [`CHUNK_PAYLOAD_SIZE`] slices.
    #[must_use]
    pub fn new(bytes: Arc<[u8]>) -> Self {
        Self { bytes, offset: 0 }
    }

    /// Total encoded length being iterated.
    #[must_use]
    pub fn encoded_length(&self) -> u64 {
        self.bytes.len() as u64
    }

    /// Next byte offset that will be emitted, or `encoded_length` when exhausted.
    #[must_use]
    pub fn next_offset(&self) -> u64 {
        self.offset as u64
    }
}

impl Iterator for ResourceChunkIter {
    type Item = ResourceChunkView;

    fn next(&mut self) -> Option<Self::Item> {
        if self.offset >= self.bytes.len() {
            return None;
        }
        let start = self.offset;
        let end = (start + CHUNK_PAYLOAD_SIZE).min(self.bytes.len());
        let data: Arc<[u8]> = Arc::from(&self.bytes[start..end]);
        self.offset = end;
        Some(ResourceChunkView {
            byte_offset: start as u64,
            data,
        })
    }
}

/// Collects all chunk payloads for `bytes` with checked contiguous offsets.
///
/// Empty resources yield no chunks; metadata alone announces `encoded_length = 0`.
#[must_use]
pub fn chunk_payloads(bytes: Arc<[u8]>) -> Vec<ResourceChunkView> {
    ResourceChunkIter::new(bytes).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_resource_yields_no_chunks() {
        let chunks = chunk_payloads(Arc::from(Vec::<u8>::new().into_boxed_slice()));
        assert!(chunks.is_empty());
    }

    #[test]
    fn chunks_are_contiguous_and_bounded() {
        let len = CHUNK_PAYLOAD_SIZE * 2 + 100;
        let bytes: Arc<[u8]> = Arc::from(vec![0xABu8; len].into_boxed_slice());
        let chunks = chunk_payloads(Arc::clone(&bytes));
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].byte_offset, 0);
        assert_eq!(chunks[0].data.len(), CHUNK_PAYLOAD_SIZE);
        assert_eq!(chunks[1].byte_offset, CHUNK_PAYLOAD_SIZE as u64);
        assert_eq!(chunks[1].data.len(), CHUNK_PAYLOAD_SIZE);
        assert_eq!(chunks[2].byte_offset, (CHUNK_PAYLOAD_SIZE * 2) as u64);
        assert_eq!(chunks[2].data.len(), 100);

        let mut reconstructed = Vec::with_capacity(len);
        let mut expected_offset = 0u64;
        for chunk in &chunks {
            assert_eq!(chunk.byte_offset, expected_offset);
            assert!(chunk.data.len() <= CHUNK_PAYLOAD_SIZE);
            reconstructed.extend_from_slice(&chunk.data);
            expected_offset += chunk.data.len() as u64;
        }
        assert_eq!(reconstructed.as_slice(), bytes.as_ref());
        assert_eq!(expected_offset, len as u64);
    }
}
