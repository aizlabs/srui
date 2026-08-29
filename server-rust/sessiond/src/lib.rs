//! # SRUI Session Daemon Library
//!
//! Provides the core session manager and async connection handler for `srui-sessiond` (§20.2).

pub mod connection;
pub mod session;

pub use connection::{handle_connection, ConnectionError, HANDSHAKE_TIMEOUT};
pub use session::{
    EventOutcome, FreshClientBootstrap, ResumeOutcome, Session, SessionError,
    TRANSACTION_BROADCAST_CAPACITY,
};
