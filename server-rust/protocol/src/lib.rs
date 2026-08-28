//! SRUI Protocol Buffers Schema & Generated Types

pub mod proto {
    include!(concat!(env!("OUT_DIR"), "/srui.protocol.rs"));
}

pub use proto::*;

pub mod framing;
pub use framing::{decode_framed, encode_framed};

/// Namespace 0 is the permanently reserved SRUI standard registry namespace.
pub const STANDARD_NAMESPACE_ID: u32 = 0;
