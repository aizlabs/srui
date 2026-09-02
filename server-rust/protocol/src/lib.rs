//! SRUI Protocol Buffers Schema & Generated Types

pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/srui.protocol.rs"));
}

pub use proto::*;

pub mod framing;
pub use framing::{
    decode_framed, decode_framed_with_limit, encode_framed, encode_framed_with_limit,
    framed_payload_len, FramingError, DEFAULT_MAX_FRAME_SIZE,
};

#[cfg(feature = "async-codec")]
pub mod codec;
#[cfg(feature = "async-codec")]
pub use codec::SruiCodec;

/// Namespace 0 is the permanently reserved SRUI standard registry namespace.
pub const STANDARD_NAMESPACE_ID: u32 = 0;
