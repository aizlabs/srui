//! Bounded absolute-offset output ring (§21.2).
//!
//! Stores bytes in a `VecDeque` of chunks covering the half-open range
//! `[retained_start, next_offset)`. Partial eviction splits the oldest chunk.

use std::collections::VecDeque;

use srui_protocol::MAX_TERMINAL_OUTPUT_FRAME_BYTES;
use thiserror::Error;

/// Errors from ring mutation or slice extraction.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum RingError {
    #[error("terminal output offset overflow")]
    OffsetOverflow,
    #[error(
        "requested range [{start}, {end}) is outside retained [{retained_start}, {next_offset})"
    )]
    RangeUnavailable {
        start: u64,
        end: u64,
        retained_start: u64,
        next_offset: u64,
    },
}

/// Bounded byte ring with absolute offsets.
#[derive(Debug)]
pub struct OutputRing {
    chunks: VecDeque<Vec<u8>>,
    retained_start: u64,
    next_offset: u64,
    stored_bytes: usize,
    capacity: usize,
}

impl OutputRing {
    /// Creates an empty ring that retains at most `capacity` bytes.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "output ring capacity must be positive");
        Self {
            chunks: VecDeque::new(),
            retained_start: 0,
            next_offset: 0,
            stored_bytes: 0,
            capacity,
        }
    }

    #[must_use]
    pub fn retained_start(&self) -> u64 {
        self.retained_start
    }

    #[must_use]
    pub fn next_offset(&self) -> u64 {
        self.next_offset
    }

    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    #[must_use]
    pub fn stored_bytes(&self) -> usize {
        self.stored_bytes
    }

    /// Appends `data` at `next_offset` and evicts from the front to stay within capacity.
    pub fn append(&mut self, data: &[u8]) -> Result<u64, RingError> {
        if data.is_empty() {
            return Ok(self.next_offset);
        }
        let added = u64::try_from(data.len()).map_err(|_| RingError::OffsetOverflow)?;
        self.next_offset = self
            .next_offset
            .checked_add(added)
            .ok_or(RingError::OffsetOverflow)?;
        self.chunks.push_back(data.to_vec());
        self.stored_bytes = self.stored_bytes.saturating_add(data.len());
        self.evict_to_capacity();
        Ok(self.next_offset)
    }

    fn evict_to_capacity(&mut self) {
        while self.stored_bytes > self.capacity {
            let Some(front) = self.chunks.front_mut() else {
                break;
            };
            let overflow = self.stored_bytes - self.capacity;
            if overflow >= front.len() {
                let removed = self.chunks.pop_front().expect("front exists");
                self.stored_bytes -= removed.len();
                self.retained_start = self
                    .retained_start
                    .checked_add(removed.len() as u64)
                    .expect("retained_start cannot overflow while next_offset is valid");
            } else {
                front.drain(..overflow);
                self.stored_bytes -= overflow;
                self.retained_start = self
                    .retained_start
                    .checked_add(overflow as u64)
                    .expect("retained_start cannot overflow while next_offset is valid");
            }
        }
    }

    /// Copies `[start, end)` into a new buffer. `end` may equal `start`.
    pub fn copy_range(&self, start: u64, end: u64) -> Result<Vec<u8>, RingError> {
        if start > end {
            return Err(RingError::RangeUnavailable {
                start,
                end,
                retained_start: self.retained_start,
                next_offset: self.next_offset,
            });
        }
        if start < self.retained_start || end > self.next_offset {
            return Err(RingError::RangeUnavailable {
                start,
                end,
                retained_start: self.retained_start,
                next_offset: self.next_offset,
            });
        }
        if start == end {
            return Ok(Vec::new());
        }
        let mut out = Vec::with_capacity((end - start) as usize);
        let mut cursor = self.retained_start;
        for chunk in &self.chunks {
            let chunk_end = cursor + chunk.len() as u64;
            if chunk_end <= start {
                cursor = chunk_end;
                continue;
            }
            if cursor >= end {
                break;
            }
            let from = start.saturating_sub(cursor) as usize;
            let to = (end.min(chunk_end) - cursor) as usize;
            out.extend_from_slice(&chunk[from..to]);
            cursor = chunk_end;
            if cursor >= end {
                break;
            }
        }
        Ok(out)
    }

    /// Frames `[start, end)` into bounded `TerminalData` payloads.
    ///
    /// Slices directly from retained chunks; does not assemble an intermediate
    /// contiguous copy of the whole range.
    pub fn frame_range(&self, start: u64, end: u64) -> Result<Vec<(u64, Vec<u8>)>, RingError> {
        if start > end || start < self.retained_start || end > self.next_offset {
            return Err(RingError::RangeUnavailable {
                start,
                end,
                retained_start: self.retained_start,
                next_offset: self.next_offset,
            });
        }
        if start == end {
            return Ok(Vec::new());
        }

        let mut frames = Vec::new();
        let mut frame = Vec::new();
        let mut frame_offset = start;
        let mut cursor = self.retained_start;
        for chunk in &self.chunks {
            let chunk_end = cursor + chunk.len() as u64;
            if chunk_end <= start {
                cursor = chunk_end;
                continue;
            }
            if cursor >= end {
                break;
            }
            let from = start.saturating_sub(cursor) as usize;
            let to = (end.min(chunk_end) - cursor) as usize;
            let mut remaining = &chunk[from..to];
            while !remaining.is_empty() {
                let space = MAX_TERMINAL_OUTPUT_FRAME_BYTES - frame.len();
                let take = remaining.len().min(space);
                if frame.is_empty() && take == MAX_TERMINAL_OUTPUT_FRAME_BYTES {
                    frames.push((frame_offset, remaining[..take].to_vec()));
                    frame_offset = frame_offset
                        .checked_add(take as u64)
                        .ok_or(RingError::OffsetOverflow)?;
                } else {
                    frame.extend_from_slice(&remaining[..take]);
                    if frame.len() == MAX_TERMINAL_OUTPUT_FRAME_BYTES {
                        let filled = std::mem::take(&mut frame);
                        frames.push((frame_offset, filled));
                        frame_offset = frame_offset
                            .checked_add(MAX_TERMINAL_OUTPUT_FRAME_BYTES as u64)
                            .ok_or(RingError::OffsetOverflow)?;
                    }
                }
                remaining = &remaining[take..];
            }
            cursor = chunk_end;
            if cursor >= end {
                break;
            }
        }
        if !frame.is_empty() {
            frames.push((frame_offset, frame));
        }
        Ok(frames)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mid_chunk_replay_returns_exact_suffix() {
        let mut ring = OutputRing::new(64);
        ring.append(b"abcdef").unwrap();
        assert_eq!(ring.copy_range(2, 5).unwrap(), b"cde");
        assert_eq!(ring.copy_range(0, 6).unwrap(), b"abcdef");
    }

    #[test]
    fn partial_eviction_splits_oldest_chunk() {
        let mut ring = OutputRing::new(8);
        ring.append(b"abcdef").unwrap();
        ring.append(b"ghij").unwrap();
        assert_eq!(ring.stored_bytes(), 8);
        assert_eq!(ring.retained_start(), 2);
        assert_eq!(ring.next_offset(), 10);
        assert_eq!(ring.copy_range(2, 10).unwrap(), b"cdefghij");
        assert!(ring.copy_range(0, 2).is_err());
    }

    #[test]
    fn frame_range_slices_across_chunks_without_requiring_contiguous_copy() {
        let mut ring = OutputRing::new(64);
        ring.append(b"abcdef").unwrap();
        ring.append(b"ghijkl").unwrap();
        let frames = ring.frame_range(2, 10).unwrap();
        let joined: Vec<u8> = frames.into_iter().flat_map(|(_, data)| data).collect();
        assert_eq!(joined, b"cdefghij");
    }

    #[test]
    fn frames_respect_output_limit() {
        let mut ring = OutputRing::new(64 * 1024);
        let payload = vec![b'x'; MAX_TERMINAL_OUTPUT_FRAME_BYTES + 8];
        ring.append(&payload).unwrap();
        let frames = ring.frame_range(0, payload.len() as u64).unwrap();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].0, 0);
        assert_eq!(frames[0].1.len(), MAX_TERMINAL_OUTPUT_FRAME_BYTES);
        assert_eq!(frames[1].0, MAX_TERMINAL_OUTPUT_FRAME_BYTES as u64);
        assert_eq!(frames[1].1.len(), 8);
        assert_eq!(
            frames.iter().map(|(_, data)| data.len()).sum::<usize>(),
            payload.len()
        );
    }

    #[test]
    fn offset_overflow_is_checked() {
        let mut ring = OutputRing::new(8);
        ring.next_offset = u64::MAX - 1;
        assert!(matches!(
            ring.append(&[1, 2, 3]),
            Err(RingError::OffsetOverflow)
        ));
    }
}
