//! # SRUI Session Daemon Library
//!
//! Provides the core session manager and async connection handler for `srui-sessiond` (§20.2).

pub mod connection;
pub mod outbound;
pub mod session;

pub use connection::{handle_connection, ConnectionError, HANDSHAKE_TIMEOUT, WRITE_TIMEOUT};
pub use outbound::{
    OutboundItem, OutboundReceiver, OutboundRecvError, DEFAULT_OUTBOUND_QUEUE_CAPACITY,
};
pub use session::{
    mint_session_id, AttachmentGuard, EventOutcome, FreshClientBootstrap, ResumeClientBootstrap,
    ResumeOutcome, Session, SessionConfig, SessionError, SessionState, CORE_VERSION,
    MAX_CLIENT_INSTANCE_ID_BYTES, MAX_RETAINED_CLIENT_STATE_BYTES,
};
