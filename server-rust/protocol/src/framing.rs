use prost::Message;
use std::fmt;

/// Default maximum allowed wire frame size in bytes (16 MiB, §26).
pub const DEFAULT_MAX_FRAME_SIZE: usize = 16 * 1024 * 1024;

/// Errors returned by length-delimited wire framing operations (§16, §26).
#[derive(Debug)]
pub enum FramingError {
    /// The frame payload or decoded length prefix exceeds the configured maximum frame size limit (§26).
    FrameSizeLimitExceeded { limit: usize, actual: usize },
    /// Protobuf encoding failed.
    EncodeError(String),
    /// Protobuf decoding failed.
    DecodeError(String),
    /// Underlying I/O error.
    Io(std::io::Error),
}

impl fmt::Display for FramingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FrameSizeLimitExceeded { limit, actual } => write!(
                f,
                "wire frame size limit exceeded: max allowed is {} bytes, actual is {} bytes (§26)",
                limit, actual
            ),
            Self::EncodeError(msg) => write!(f, "framing encode error: {}", msg),
            Self::DecodeError(msg) => write!(f, "framing decode error: {}", msg),
            Self::Io(e) => write!(f, "framing I/O error: {}", e),
        }
    }
}

impl From<std::io::Error> for FramingError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl std::error::Error for FramingError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            _ => None,
        }
    }
}


/// Encodes a message with a varint length prefix, enforcing `DEFAULT_MAX_FRAME_SIZE` (§16, §26).
pub fn encode_framed<M: Message>(msg: &M) -> Result<Vec<u8>, FramingError> {
    encode_framed_with_limit(msg, DEFAULT_MAX_FRAME_SIZE)
}

/// Encodes a message with a varint length prefix, enforcing a custom `max_frame_size` limit (§16, §26).
pub fn encode_framed_with_limit<M: Message>(
    msg: &M,
    max_frame_size: usize,
) -> Result<Vec<u8>, FramingError> {
    let payload_len = msg.encoded_len();
    if payload_len > max_frame_size {
        return Err(FramingError::FrameSizeLimitExceeded {
            limit: max_frame_size,
            actual: payload_len,
        });
    }
    let mut buf = Vec::with_capacity(payload_len + 10);
    msg.encode_length_delimited(&mut buf)
        .map_err(|e| FramingError::EncodeError(e.to_string()))?;
    Ok(buf)
}

/// Decodes a length-delimited message, enforcing `DEFAULT_MAX_FRAME_SIZE` (§16, §26).
pub fn decode_framed<M: Message + Default>(buf: &[u8]) -> Result<M, FramingError> {
    decode_framed_with_limit(buf, DEFAULT_MAX_FRAME_SIZE)
}

/// Decodes a length-delimited message, enforcing a custom `max_frame_size` limit (§16, §26).
pub fn decode_framed_with_limit<M: Message + Default>(
    mut buf: &[u8],
    max_frame_size: usize,
) -> Result<M, FramingError> {
    let mut peek_buf = buf;
    if let Ok(varint_len) = prost::decode_length_delimiter(&mut peek_buf) {
        if varint_len > max_frame_size {
            return Err(FramingError::FrameSizeLimitExceeded {
                limit: max_frame_size,
                actual: varint_len,
            });
        }
    }
    M::decode_length_delimited(&mut buf)
        .map_err(|e| FramingError::DecodeError(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;

    #[test]
    fn test_length_delimited_framing_roundtrip() {
        let msg = SruiMessage {
            msg: Some(srui_message::Msg::Transaction(Transaction {
                base_revision: 10,
                new_revision: 11,
                priority: 1,
                operations: vec![],
            })),
        };

        let framed_bytes = encode_framed(&msg).expect("encode framed");
        assert!(!framed_bytes.is_empty());
        let decoded: SruiMessage = decode_framed(&framed_bytes).expect("decode framed");
        assert_eq!(msg, decoded);
    }

    #[test]
    fn test_max_frame_size_limit_enforced() {
        let msg = SruiMessage {
            msg: Some(srui_message::Msg::Transaction(Transaction {
                base_revision: 100,
                new_revision: 101,
                priority: 1,
                operations: vec![],
            })),
        };

        // Encoding with tiny limit should fail
        let err = encode_framed_with_limit(&msg, 2).expect_err("encode exceeding frame size");
        assert!(matches!(err, FramingError::FrameSizeLimitExceeded { limit: 2, actual } if actual > 2));

        // Decoding with tiny limit should fail
        let valid_bytes = encode_framed(&msg).unwrap();
        let decode_err = decode_framed_with_limit::<SruiMessage>(&valid_bytes, 2)
            .expect_err("decode exceeding frame size");
        assert!(matches!(decode_err, FramingError::FrameSizeLimitExceeded { limit: 2, .. }));
    }

    #[test]
    fn test_decode_framed_with_limit_ignores_buffer_padding() {
        let msg = SruiMessage {
            msg: Some(srui_message::Msg::Transaction(Transaction {
                base_revision: 10,
                new_revision: 11,
                priority: 1,
                operations: vec![],
            })),
        };

        let framed = encode_framed(&msg).expect("encode framed");
        assert!(framed.len() <= 100);

        let mut large_buf = vec![0u8; 1024 * 1024];
        large_buf[..framed.len()].copy_from_slice(&framed);

        let decoded: SruiMessage =
            decode_framed_with_limit(&large_buf, DEFAULT_MAX_FRAME_SIZE).expect("decode framed");
        assert_eq!(msg, decoded);
    }
}
