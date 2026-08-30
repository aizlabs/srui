//! # SRUI Session Daemon Library
//!
//! Provides the core session manager and async connection handler for `srui-sessiond` (§20.2).

pub mod connection;
pub mod session;

pub use connection::{handle_connection, ConnectionError, HANDSHAKE_TIMEOUT};
pub use session::{
    mint_session_id, AttachmentGuard, EventOutcome, FreshClientBootstrap, ResumeClientBootstrap,
    ResumeOutcome, Session, SessionError, SessionState, TRANSACTION_BROADCAST_CAPACITY,
};
