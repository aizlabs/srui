//! Per-connection resource transfer cursors (§14, §19.2).
//!
//! Tracks which retained resources still need metadata/chunk delivery on one
//! outbound subscriber. The logical-channel scheduler treats both metadata and
//! chunk frames as `resource` traffic and emits at most one frame per selection
//! ([`crate::outbound::LogicalChannelScheduler`]). A chunk whose socket write has
//! already begun is not preemptible; newly ready control, input, and UI frames are
//! selected before another resource frame.

use std::collections::VecDeque;
use std::sync::Arc;

use srui_protocol::{ResourceChunk, ResourceMetadata, ResourcePriority};
use srui_resources::{ResourceEntry, CHUNK_PAYLOAD_SIZE};
use srui_semantic_tree::ResourceHash;

/// One outbound frame that is not a semantic transaction (§14, §19.2).
#[derive(Debug, Clone, PartialEq)]
pub enum ResourceOutboundFrame {
    Metadata(ResourceMetadata),
    Chunk(ResourceChunk),
}

#[derive(Debug)]
struct ActiveTransfer {
    hash: ResourceHash,
    media_type: String,
    bytes: Arc<[u8]>,
    metadata_sent: bool,
    next_offset: usize,
}

impl ActiveTransfer {
    fn from_entry(entry: ResourceEntry) -> Self {
        Self {
            hash: entry.hash,
            media_type: entry.media_type,
            bytes: entry.bytes,
            metadata_sent: false,
            next_offset: 0,
        }
    }

    fn next_frame(&mut self) -> ResourceOutboundFrame {
        if !self.metadata_sent {
            self.metadata_sent = true;
            return ResourceOutboundFrame::Metadata(ResourceMetadata {
                resource_hash: self.hash.0.to_vec(),
                media_type: self.media_type.clone(),
                encoded_length: self.bytes.len() as u64,
                decoded_width: 0,
                decoded_height: 0,
                priority: ResourcePriority::Normal as i32,
            });
        }

        debug_assert!(self.next_offset < self.bytes.len());
        let start = self.next_offset;
        let end = (start + CHUNK_PAYLOAD_SIZE).min(self.bytes.len());
        let data = self.bytes[start..end].to_vec();
        self.next_offset = end;
        ResourceOutboundFrame::Chunk(ResourceChunk {
            resource_hash: self.hash.0.to_vec(),
            byte_offset: start as u64,
            data,
        })
    }

    fn is_complete(&self) -> bool {
        self.metadata_sent && self.next_offset >= self.bytes.len()
    }
}

/// FIFO of resources awaiting transfer on one connection, plus the active cursor.
#[derive(Debug, Default)]
pub(crate) struct ResourceTransferQueue {
    pending: VecDeque<ResourceEntry>,
    active: Option<ActiveTransfer>,
}

impl ResourceTransferQueue {
    pub(crate) fn clear(&mut self) {
        self.pending.clear();
        self.active = None;
    }

    /// Enqueues `entry` unless the same hash is already queued or actively transferring.
    pub(crate) fn enqueue(&mut self, entry: ResourceEntry) {
        let hash = entry.hash;
        if self
            .active
            .as_ref()
            .is_some_and(|active| active.hash == hash)
        {
            return;
        }
        if self.pending.iter().any(|pending| pending.hash == hash) {
            return;
        }
        self.pending.push_back(entry);
    }

    /// Returns `true` when a metadata/chunk frame can still be emitted.
    pub(crate) fn has_work(&self) -> bool {
        self.active.is_some() || !self.pending.is_empty()
    }

    /// Emits the next metadata or chunk frame, advancing the cursor by at most one frame.
    pub(crate) fn pop_frame(&mut self) -> Option<ResourceOutboundFrame> {
        if self.active.is_none() {
            let next = self.pending.pop_front()?;
            self.active = Some(ActiveTransfer::from_entry(next));
        }

        let active = self.active.as_mut()?;
        let frame = active.next_frame();
        if active.is_complete() {
            self.active = None;
        }
        Some(frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use srui_semantic_tree::ResourceHash;

    fn entry(tag: u8, bytes: &[u8]) -> ResourceEntry {
        ResourceEntry {
            hash: ResourceHash::new([tag; 32]),
            media_type: "application/octet-stream".into(),
            encoded_length: bytes.len() as u64,
            bytes: Arc::from(bytes),
        }
    }

    #[test]
    fn empty_resource_emits_metadata_only() {
        let mut queue = ResourceTransferQueue::default();
        queue.enqueue(entry(1, b""));
        match queue.pop_frame() {
            Some(ResourceOutboundFrame::Metadata(meta)) => {
                assert_eq!(meta.encoded_length, 0);
                assert_eq!(meta.resource_hash, [1u8; 32]);
            }
            other => panic!("expected metadata, got {other:?}"),
        }
        assert!(!queue.has_work());
        assert!(queue.pop_frame().is_none());
    }

    #[test]
    fn non_empty_resource_emits_metadata_then_chunks() {
        let mut queue = ResourceTransferQueue::default();
        let payload = vec![0xABu8; CHUNK_PAYLOAD_SIZE + 3];
        queue.enqueue(entry(2, &payload));

        match queue.pop_frame() {
            Some(ResourceOutboundFrame::Metadata(meta)) => {
                assert_eq!(meta.encoded_length, payload.len() as u64);
            }
            other => panic!("expected metadata, got {other:?}"),
        }
        match queue.pop_frame() {
            Some(ResourceOutboundFrame::Chunk(chunk)) => {
                assert_eq!(chunk.byte_offset, 0);
                assert_eq!(chunk.data.len(), CHUNK_PAYLOAD_SIZE);
            }
            other => panic!("expected first chunk, got {other:?}"),
        }
        match queue.pop_frame() {
            Some(ResourceOutboundFrame::Chunk(chunk)) => {
                assert_eq!(chunk.byte_offset, CHUNK_PAYLOAD_SIZE as u64);
                assert_eq!(chunk.data, vec![0xABu8; 3]);
            }
            other => panic!("expected tail chunk, got {other:?}"),
        }
        assert!(!queue.has_work());
    }
}
