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

/// Profile URI stored in [`ExtensionNamespaceMapping::extension_uri`] for the
/// Terminal compatibility profile (§21). Local type ID [`TERMINAL_LOCAL_TYPE_ID`]
/// within the session-assigned namespace is `Terminal`.
pub const TERMINAL_PROFILE_URI: &str = "org.srui.terminal/1";

/// Local type ID of `Terminal` inside a negotiated `org.srui.terminal/1` namespace.
pub const TERMINAL_LOCAL_TYPE_ID: u32 = 1;

/// Maximum `TerminalInput.data` length (§21, §26).
pub const MAX_TERMINAL_INPUT_BYTES: usize = 65_536;

/// Maximum `TerminalData.data` length; empty frames are forbidden (§21, §26).
pub const MAX_TERMINAL_OUTPUT_FRAME_BYTES: usize = 16_384;

/// Maximum `ClientResume.terminal_stream_offsets` entries (§21, §26).
pub const MAX_TERMINAL_RESUME_MAP_ENTRIES: usize = 256;

/// Inclusive upper bound for `TerminalResize.columns` (§21, §26).
pub const MAX_TERMINAL_COLUMNS: u32 = 512;

/// Inclusive upper bound for `TerminalResize.rows` (§21, §26).
pub const MAX_TERMINAL_ROWS: u32 = 512;

/// Inclusive upper bound for terminal pixel dimensions (§21, §26).
pub const MAX_TERMINAL_PIXEL_DIMENSION: u32 = 16_384;
