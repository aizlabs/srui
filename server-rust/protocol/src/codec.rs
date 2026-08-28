use bytes::BytesMut;
use prost::Message;
use tokio_util::codec::{Decoder, Encoder};

use crate::framing::{FramingError, DEFAULT_MAX_FRAME_SIZE};
use crate::SruiMessage;

/// Asynchronous length-delimited codec for [`SruiMessage`] frames (§16, §26).
///
/// Implements cancellation-safe decoding over Tokio async streams. When used with
/// [`tokio_util::codec::FramedRead`], partial frame chunks accumulated in internal
/// buffers survive across `tokio::select!` future cancellations without data loss.
#[derive(Debug, Clone)]
pub struct SruiCodec {
    max_frame_size: usize,
}

impl Default for SruiCodec {
    fn default() -> Self {
        Self::new()
    }
}

impl SruiCodec {
    /// Creates a new `SruiCodec` with the default maximum frame size (16 MiB, §26).
    #[must_use]
    pub fn new() -> Self {
        Self::with_max_frame_size(DEFAULT_MAX_FRAME_SIZE)
    }

    /// Creates a new `SruiCodec` with a custom maximum frame size limit (§26).
    #[must_use]
    pub const fn with_max_frame_size(max_frame_size: usize) -> Self {
        Self { max_frame_size }
    }

    /// Returns the configured maximum frame size limit in bytes.
    #[must_use]
    pub const fn max_frame_size(&self) -> usize {
        self.max_frame_size
    }
}

impl Decoder for SruiCodec {
    type Item = SruiMessage;
    type Error = FramingError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        if src.is_empty() {
            return Ok(None);
        }

        // Peek the length delimiter without consuming src
        let mut peek_buf = &src[..];
        let varint_len = match prost::decode_length_delimiter(&mut peek_buf) {
            Ok(len) => len,
            Err(_) => {
                // Not enough bytes to decode the varint length prefix yet; wait for more data
                return Ok(None);
            }
        };

        // Enforce maximum wire frame size limit (§26)
        if varint_len > self.max_frame_size {
            return Err(FramingError::FrameSizeLimitExceeded {
                limit: self.max_frame_size,
                actual: varint_len,
            });
        }

        let header_len = src.len() - peek_buf.len();
        let total_frame_len = match header_len.checked_add(varint_len) {
            Some(len) => len,
            None => {
                return Err(FramingError::FrameSizeLimitExceeded {
                    limit: self.max_frame_size,
                    actual: usize::MAX,
                });
            }
        };

        if src.len() < total_frame_len {
            // Reserve remaining required capacity to avoid incremental allocations
            src.reserve(total_frame_len - src.len());
            return Ok(None);
        }

        // Split off the complete frame bytes from the buffer
        let frame_bytes = src.split_to(total_frame_len);
        let mut msg_slice = &frame_bytes[header_len..];

        let msg = SruiMessage::decode(&mut msg_slice)
            .map_err(|e| FramingError::DecodeError(e.to_string()))?;

        Ok(Some(msg))
    }
}

impl Encoder<SruiMessage> for SruiCodec {
    type Error = FramingError;

    fn encode(&mut self, item: SruiMessage, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let payload_len = item.encoded_len();
        if payload_len > self.max_frame_size {
            return Err(FramingError::FrameSizeLimitExceeded {
                limit: self.max_frame_size,
                actual: payload_len,
            });
        }

        dst.reserve(payload_len + 10);
        item.encode_length_delimited(dst)
            .map_err(|e| FramingError::EncodeError(e.to_string()))?;

        Ok(())
    }
}
