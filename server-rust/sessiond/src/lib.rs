//! # SRUI Session Daemon Library
//!
//! Provides the core session manager and async connection handler for `srui-sessiond` (§20.2).

pub mod connection;
pub mod outbound;
pub mod session;

pub use connection::{handle_connection, ConnectionError, HANDSHAKE_TIMEOUT, WRITE_TIMEOUT};
pub use outbound::{
    logical_class_for_server_envelope, LogicalChannelClass, LogicalChannelScheduler, OutboundItem,
    OutboundReceiver, OutboundRecvError, DEFAULT_OUTBOUND_QUEUE_CAPACITY, SERVICE_CYCLE,
};
pub use session::{
    mint_session_id, run_model_range_worker, AttachmentGuard, EventOutcome, FreshClientBootstrap,
    ModelRangeError, ModelRangeFulfillment, ModelRangeProvider, ModelRangeQuery,
    ModelRangeRequestInbox, ResumeClientBootstrap, ResumeOutcome, Session, SessionConfig,
    SessionError, SessionState, CORE_VERSION, MAX_CLIENT_INSTANCE_ID_BYTES,
    MAX_RETAINED_CLIENT_STATE_BYTES,
};
