//! SRUI PTY manager, bounded output ring, and per-stream process ownership (§21, §21.2).
//!
//! Terminal bytes never enter the semantic store. A falling-behind subscriber receives
//! only [`TerminalResyncRequired`](srui_protocol::TerminalResyncRequired) and does not
//! mark the semantic outbound subscriber stale.

mod manager;
mod ring;
mod spec;
pub(crate) mod stream;

pub use manager::{
    PTYManager, PTYManagerConfig, PTYManagerError, SubscribeOutcome, TerminalSubscription,
};
pub use ring::{OutputRing, RingError};
pub use spec::{TerminalSpec, DEFAULT_RING_CAPACITY, DEFAULT_TERM};
pub use stream::SubscribeSnapshot;
pub use stream::{StreamCommand, TerminalEvent, TerminalStreamError};

pub use srui_protocol::{
    MAX_TERMINAL_COLUMNS, MAX_TERMINAL_INPUT_BYTES, MAX_TERMINAL_OUTPUT_FRAME_BYTES,
    MAX_TERMINAL_PIXEL_DIMENSION, MAX_TERMINAL_RESUME_MAP_ENTRIES, MAX_TERMINAL_ROWS,
    TERMINAL_LOCAL_TYPE_ID, TERMINAL_PROFILE_URI,
};
