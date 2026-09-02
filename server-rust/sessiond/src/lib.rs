//! # SRUI Session Daemon Library
//!
//! Provides the core session manager and async connection handler for `srui-sessiond` (§20.2).

pub mod connection;
pub mod outbound;
pub mod session;

pub use connection::{handle_connection, ConnectionError, HANDSHAKE_TIMEOUT};
pub use outbound::{
    is_coalesceable_tx, EnqueueError, EnqueueOutcome, OutboundHub, OutboundMetrics, OutboundQueue,
    OutboundReceiver, OutboundRecvError, OutboundTryRecvError, DEFAULT_OUTBOUND_QUEUE_CAPACITY,
};
pub use session::{
    mint_session_id, AttachmentGuard, EventOutcome, FreshClientBootstrap, ResumeClientBootstrap,
    ResumeOutcome, Session, SessionConfig, SessionError, SessionState,
    TRANSACTION_BROADCAST_CAPACITY,
};
