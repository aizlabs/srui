//! Per-connection resource transfer cursors (§14, §19.2).
//!
//! Tracks which retained resources still need metadata/chunk delivery on one
//! outbound subscriber. Exactly one resource frame is emitted per selection when
//! no transaction is pending, so UI traffic can interleave after at most the
//! chunk whose socket write has already begun.

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

    fn next_frame(&mut self) -> Option<ResourceOutboundFrame> {
        if !self.metadata_sent {
            self.metadata_sent = true;
            return Some(ResourceOutboundFrame::Metadata(ResourceMetadata {
                resource_hash: self.hash.0.to_vec(),
                media_type: self.media_type.clone(),
                encoded_length: self.bytes.len() as u64,
                decoded_width: 0,
                decoded_height: 0,
                priority: ResourcePriority::Normal as i32,
            }));
        }

        if self.next_offset >= self.bytes.len() {
            return None;
        }

        let start = self.next_offset;
        let end = (start + CHUNK_PAYLOAD_SIZE).min(self.bytes.len());
        let data = self.bytes[start..end].to_vec();
        self.next_offset = end;
        Some(ResourceOutboundFrame::Chunk(ResourceChunk {
            resource_hash: self.hash.0.to_vec(),
            byte_offset: start as u64,
            data,
        }))
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

    /// Emits the next metadata or chunk frame, advancing the cursor by at most one frame.
    pub(crate) fn pop_frame(&mut self) -> Option<ResourceOutboundFrame> {
        loop {
            if self.active.is_none() {
                let next = self.pending.pop_front()?;
                self.active = Some(ActiveTransfer::from_entry(next));
            }

            let active = self.active.as_mut()?;
            if let Some(frame) = active.next_frame() {
                if active.is_complete() {
                    self.active = None;
                }
                return Some(frame);
            }

            // Empty resource: metadata already sent, no chunks.
            self.active = None;
        }
    }
}
