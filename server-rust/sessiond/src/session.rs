//! # Session State & Transaction Coordination
//!
//! Authoritative state owner managing [`SemanticStore`], [`TransactionJournal`],
//! and [`EventDeduplicator`] for a session (§6.3, §12, §18, §18.2, §18.3, §20.2, §21, App. B).
//!
//! Conforms strictly to [`async-no-lock-await`](rules/async-no-lock-await.md):
//! internal locks are held only for fast in-memory operations and never across `.await` points.
//! Conforms to [`async-bounded-channel`](rules/async-bounded-channel.md):
//! per-connection outbound transaction queues are strictly bounded.

mod handshake;
mod model_range;
mod snapshot;
pub(crate) mod terminal;
mod text_edit;

pub use handshake::{
    FreshClientBootstrap, ResumeClientBootstrap, ResumeOutcome, CORE_VERSION,
    MAX_CLIENT_INSTANCE_ID_BYTES,
};
pub use model_range::{
    run_model_range_worker, ModelRangeError, ModelRangeFulfillment, ModelRangeProvider,
    ModelRangeQuery, ModelRangeRequestInbox,
};
pub use terminal::TerminalAttach;
pub use text_edit::{TextEditDecision, TextEditRequest, TextEditTracker, MAX_TEXT_EDIT_STREAMS};

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::outbound::{OutboundHub, OutboundReceiver, DEFAULT_OUTBOUND_QUEUE_CAPACITY};

/// Lifecycle states of an authoritative semantic session (§17, App. B).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SessionState {
    /// No active transport attachment; application and semantic store remain alive (§17).
    #[default]
    Detached,
    /// At least one transport connection is actively attached and streaming (§17).
    Attached,
    /// Session is terminating (§17; triggering policy stubbed in Task 22).
    Terminating,
    /// Session has expired and is discarded (§17; triggering policy stubbed in Task 22).
    Expired,
}

impl SessionState {
    /// Returns `true` if this state is terminal (`Terminating` or `Expired`).
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Terminating | Self::Expired)
    }
}

impl std::fmt::Display for SessionState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Detached => write!(f, "DETACHED"),
            Self::Attached => write!(f, "ATTACHED"),
            Self::Terminating => write!(f, "TERMINATING"),
            Self::Expired => write!(f, "EXPIRED"),
        }
    }
}

/// Ceiling for [`Session::retained_client_state_bytes`], composed from the caps that produce it
/// (§15, §20.2, §26).
///
/// Written as the product of the entry caps and the per-identifier byte cap rather than as a
/// literal, so that raising any one of the three moves the budget with it instead of silently
/// invalidating the invariant test.
pub const MAX_RETAINED_CLIENT_STATE_BYTES: usize = (handshake::MAX_CLIENT_RESOURCE_CEILINGS
    + crate::outbound::MAX_TRACKED_STALE_CLIENTS
    + text_edit::MAX_TEXT_EDIT_STREAMS)
    * handshake::MAX_CLIENT_INSTANCE_ID_BYTES;

const HEX_CHARS: &[u8; 16] = b"0123456789abcdef";

/// Mints an opaque, globally unique 128-bit session incarnation token (§17).
///
/// Returns a 32-character lowercase hex string with 128 bits of cryptographic entropy.
/// A server process restart MUST mint a fresh token and MUST NOT reuse tokens across runs (§17).
#[must_use]
pub fn mint_session_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("failed to generate random bytes for session_id");
    let mut s = String::with_capacity(32);
    for &b in &bytes {
        s.push(HEX_CHARS[(b >> 4) as usize] as char);
        s.push(HEX_CHARS[(b & 0x0f) as usize] as char);
    }
    s
}

use srui_event_dedupe::{EventDeduplicator, EventOutcomeRecord, EventSequenceError, RecordOutcome};
use srui_journal::{JournalError, TransactionJournal, DEFAULT_MAX_JOURNAL_ENTRIES};
use srui_protocol::{Event, ServerLimits, Transaction};
use srui_resources::{PublishOutcome, ResourceEntry, ResourceStore};
use srui_sdk::UiTransaction;
use srui_semantic_tree::{
    AuthoritativeCommit, Event as DomainEvent, EventValidationError, NegotiationError, NodeId,
    Operation, ResourceHash, SemanticStore, ServerCapabilities, StoreError, TxnError, TypeRef,
    DEFAULT_MAX_EVENT_ID_BYTES, DEFAULT_MAX_NODE_COUNT, DEFAULT_MAX_STRING_LENGTH,
    DEFAULT_MAX_TRANSACTION_OPERATIONS, DEFAULT_MAX_TREE_DEPTH,
};
use thiserror::Error;

/// Type alias for event handler callbacks in `sessiond` (§29).
pub type HandlerFn = Arc<dyn Fn(&Session, &Event) + Send + Sync + 'static>;

/// Errors produced by session state operations.
#[derive(Debug, Error)]
pub enum SessionError {
    #[error("store error: {0}")]
    Store(#[from] StoreError),

    #[error("transaction error: {0}")]
    Transaction(#[from] TxnError),

    #[error("journal error: {0}")]
    Journal(#[from] JournalError),

    #[error("event validation error: {0}")]
    EventValidation(#[from] EventValidationError),

    #[error("capability negotiation error: {0}")]
    Negotiation(#[from] NegotiationError),

    #[error("invalid client event sequence: {0}")]
    EventSequence(#[from] EventSequenceError),

    #[error("lock poisoned")]
    LockPoisoned,

    #[error("client outbound queue overflowed; resync required")]
    LaggedResyncRequired,

    #[error("replay unavailable for requested revision")]
    ReplayUnavailable,

    #[error("outbound transaction queue or hub is closed")]
    OutboundClosed,

    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("invalid configuration: {0}")]
    InvalidConfiguration(String),

    #[error("catch-up snapshot needs {actual} operations but a transaction may carry at most {limit} (§26)")]
    SnapshotUnrepresentable { limit: usize, actual: usize },

    #[error("unsupported core protocol version {requested:?}; this server speaks {supported:?}")]
    UnsupportedCoreVersion {
        requested: String,
        supported: String,
    },

    #[error("session is in a terminal state ({0:?})")]
    TerminalState(SessionState),

    #[error("transaction panicked: {0}")]
    Panicked(String),

    #[error("resource error: {0}")]
    Resource(#[from] srui_resources::ResourceError),
}

/// Takes `mutex`, recovering from poisoning instead of propagating the panic.
///
/// A panic while a lock is held must fail the connection that panicked, not every later use of the
/// daemon's shared state.
pub(crate) fn lock_or_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// Truncates a diagnostic string to the negotiated §26 `max_string_length` (UTF-8 safe).
pub(crate) fn bound_diagnostic_string(mut value: String, max_len: usize) -> String {
    if value.len() <= max_len {
        return value;
    }
    let mut end = max_len;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
    value
}

/// Outcome of one client event (§18.2).
///
/// `Processed`, `Duplicate`, and `Rejected` are terminal and become `SERVER EVENT_ACK`; `Pending`
/// is explicitly non-terminal and produces no acknowledgement. `last_processed_event_seq` is the
/// highest contiguous settled sequence for the event's `client_instance_id`; it never crosses
/// an in-flight or missing sequence (§18.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventOutcome {
    /// Newly admitted, validated, and dispatched to handlers.
    Processed {
        revision_after_effect: u64,
        last_processed_event_seq: u64,
    },
    /// The same event is still being dispatched by another connection.
    Pending { last_processed_event_seq: u64 },
    /// Replay of an already-settled `event_id`: answered from the result cache without re-running
    /// the action (§18.2, App. B). `accepted` echoes whether the original attempt was processed.
    Duplicate {
        accepted: bool,
        revision_after_effect: u64,
        last_processed_event_seq: u64,
        reject_reason: String,
    },
    /// Refused by event validation. The event is settled, not retried.
    Rejected {
        error: EventValidationError,
        revision_after_effect: u64,
        last_processed_event_seq: u64,
    },
}

enum EventAdmission {
    Fresh,
    Existing(EventOutcome),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HandlerDispatchKind {
    Ordinary,
    CommittedTextEdit,
}

pub(crate) fn panic_payload_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic".to_string()
    }
}

pub(crate) struct SessionInner {
    pub(crate) session_id: String,
    pub(crate) state: SessionState,
    pub(crate) attached_connections: usize,
    pub(crate) store: SemanticStore,
    pub(crate) journal: TransactionJournal,
    pub(crate) dedupe: EventDeduplicator,
    pub(crate) capabilities: ServerCapabilities,
    pub(crate) limits: ServerLimits,
    pub(crate) resources: ResourceStore,
    /// Negotiated per-`client_instance_id` resource ceilings from ClientHello (§15, §26).
    ///
    /// Retained so ClientResume (which carries no limits) can reuse the last negotiated value.
    pub(crate) client_resource_ceilings: HashMap<Vec<u8>, u64>,
    pub(crate) handlers: HashMap<(NodeId, TypeRef), Vec<HandlerFn>>,
    /// Sparse-collection window providers keyed by [`srui_semantic_tree::ModelId`] (§8, §22.7).
    pub(crate) model_range_providers: HashMap<srui_semantic_tree::ModelId, ModelRangeProvider>,
    pub(crate) text_edit_tracker: text_edit::TextEditTracker,
    pub(crate) text_edit_policy: Option<text_edit::TextEditPolicy>,
    /// Session-stable extension URI → namespace_id table advertised on every welcome (§15, §21).
    pub(crate) extension_namespaces: Vec<srui_protocol::ExtensionNamespaceMapping>,
    /// Set once any client has completed a handshake against this session (§15, §21).
    ///
    /// Detaching clears [`SessionState::Attached`] but not this flag: a client that already
    /// negotiated can resume without a second `ServerWelcome`, so capability-changing operations
    /// stay illegal for the rest of the session's life, not just while a transport is attached.
    pub(crate) has_negotiated: bool,
}

impl std::fmt::Debug for SessionInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionInner")
            .field("session_id", &self.session_id)
            .field("state", &self.state)
            .field("attached_connections", &self.attached_connections)
            .field("store", &self.store)
            .field("journal", &self.journal)
            .field("dedupe", &self.dedupe)
            .field("capabilities", &self.capabilities)
            .field("limits", &self.limits)
            .field("resources", &self.resources)
            .field(
                "client_resource_ceilings",
                &self.client_resource_ceilings.len(),
            )
            .field("handler_count", &self.handlers.len())
            .field(
                "model_range_provider_count",
                &self.model_range_providers.len(),
            )
            .field("text_edit_streams", &self.text_edit_tracker.len())
            .field("extension_namespaces", &self.extension_namespaces)
            .finish()
    }
}

/// RAII guard representing an active transport attachment to a [`Session`] (§17).
///
/// When dropped (e.g. upon transport EOF, error, cancellation, or bridge death),
/// automatically decrements the session's attached connection count and transitions
/// `ATTACHED -> DETACHED` when the last connection drops (§17).
#[derive(Debug)]
pub struct AttachmentGuard {
    session: Session,
}

impl AttachmentGuard {
    /// Returns a reference to the attached [`Session`].
    #[must_use]
    pub fn session(&self) -> &Session {
        &self.session
    }
}

impl Drop for AttachmentGuard {
    fn drop(&mut self) {
        self.session.detach_internal();
    }
}

/// Construction parameters for a [`Session`] (§15, §18.1, §20.2).
///
/// # Journal Retention (§18.1)
/// `journal_capacity` is the **maximum retained transaction count**, the §18.1 retention policy
/// this implementation uses. A resume whose `last_applied_revision` falls outside that window
/// is answered `RESYNC_REQUIRED{continuity = SAME_SESSION}` instead of a journal replay (§18).
#[derive(Debug, Clone)]
pub struct SessionConfig {
    /// Server-side capability set offered during negotiation (§15).
    pub capabilities: ServerCapabilities,
    /// Maximum number of committed transactions retained for reconnect replay (§18.1).
    /// Must be positive; zero is rejected at session construction (same policy as
    /// `srui-sessiond --journal-capacity`).
    pub journal_capacity: usize,
    /// Capacity of the bounded per-connection outbound transaction queue (§20.2).
    /// Must be positive; zero is rejected at session construction, like `journal_capacity`.
    pub outbound_queue_capacity: usize,
    /// Maximum encoded client event identifier length (§7.7, §18.2, §26).
    /// Must be positive so deduplication keys always have a finite bound.
    pub max_event_id_bytes: usize,
}
impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            capabilities: ServerCapabilities::standard_widgets(),
            journal_capacity: DEFAULT_MAX_JOURNAL_ENTRIES,
            outbound_queue_capacity: DEFAULT_OUTBOUND_QUEUE_CAPACITY,
            max_event_id_bytes: DEFAULT_MAX_EVENT_ID_BYTES,
        }
    }
}

/// Authoritative session controller managing the distributed UI graph.
#[derive(Debug, Clone)]
pub struct Session {
    pub(crate) inner: Arc<Mutex<SessionInner>>,
    pub(crate) outbound_hub: Arc<OutboundHub>,
    pub(crate) outbound_queue_capacity: usize,
    pub(crate) max_event_id_bytes: usize,
    /// PTY streams live outside `SessionInner` so blocking I/O never holds the semantic mutex (§21).
    pub(crate) pty: Arc<srui_pty::PTYManager>,
}

impl Default for Session {
    /// Creates a session with a freshly minted incarnation token and standard capabilities (§17).
    fn default() -> Self {
        Self::mint()
    }
}

impl Session {
    /// Creates a new `Session` with a freshly minted, globally unique incarnation token (§17).
    #[must_use]
    pub fn mint() -> Self {
        Self::mint_with_capabilities(ServerCapabilities::standard_widgets())
    }

    /// Creates a new `Session` with a freshly minted, globally unique incarnation token and custom capabilities (§15, §17).
    #[must_use]
    pub fn mint_with_capabilities(capabilities: ServerCapabilities) -> Self {
        Self::with_capabilities(mint_session_id(), capabilities)
    }

    /// Creates a new `Session` with a freshly minted incarnation token and an explicit
    /// configuration, including the §18.1 journal retention window (§17, §18.1).
    #[must_use]
    pub fn mint_with_config(config: SessionConfig) -> Self {
        Self::with_config(mint_session_id(), config)
    }

    /// Creates a new `Session` with the given session ID and default standard capabilities.
    pub fn new(session_id: impl Into<String>) -> Self {
        Self::with_capabilities(session_id, ServerCapabilities::standard_widgets())
    }

    /// Creates a session with a custom per-connection outbound transaction queue capacity.
    ///
    /// Intended for integration tests that exercise lag/resync behavior (§20.2).
    #[must_use]
    pub fn with_outbound_queue_capacity(session_id: impl Into<String>, capacity: usize) -> Self {
        Self::with_config(
            session_id,
            SessionConfig {
                outbound_queue_capacity: capacity,
                ..SessionConfig::default()
            },
        )
    }

    /// Drops all active outbound queues so attached subscribers observe
    /// [`OutboundRecvError::Closed`] (§20.2).
    pub fn close_outbound(&self) {
        self.outbound_hub.close();
    }

    /// Creates a session with the given session ID and an explicit [`SessionConfig`] (§15, §18.1, §20.2).
    ///
    /// # Panics
    ///
    /// Panics when `journal_capacity`, `outbound_queue_capacity`, or `max_event_id_bytes` is
    /// zero. All are refused rather than clamped: zero would either disable a security bound or
    /// make the corresponding queue unable to retain a single valid entry.
    #[must_use]
    pub fn with_config(session_id: impl Into<String>, config: SessionConfig) -> Self {
        assert!(
            config.journal_capacity > 0,
            "SessionConfig::journal_capacity must be a positive integer (§18.1); \
             got 0. Use the default ({DEFAULT_MAX_JOURNAL_ENTRIES}) or pass an explicit window."
        );
        assert!(
            config.outbound_queue_capacity > 0,
            "SessionConfig::outbound_queue_capacity must be a positive integer (§20.2); \
             got 0. Use the default ({DEFAULT_OUTBOUND_QUEUE_CAPACITY}) or pass an explicit capacity."
        );
        assert!(
            config.max_event_id_bytes > 0,
            "SessionConfig::max_event_id_bytes must be a positive integer (§26); \
             got 0. Use the default ({DEFAULT_MAX_EVENT_ID_BYTES}) or pass an explicit limit."
        );
        let limits = ServerLimits {
            max_frame_size: 16 * 1024 * 1024,
            max_transaction_operations: DEFAULT_MAX_TRANSACTION_OPERATIONS as u32,
            max_tree_depth: DEFAULT_MAX_TREE_DEPTH as u32,
            max_node_count: DEFAULT_MAX_NODE_COUNT as u32,
            max_string_length: DEFAULT_MAX_STRING_LENGTH as u32,
            max_resource_size: 50 * 1024 * 1024,
        };

        let inner = SessionInner {
            session_id: session_id.into(),
            state: SessionState::Detached,
            attached_connections: 0,
            store: SemanticStore::new(),
            journal: TransactionJournal::new(config.journal_capacity),
            dedupe: EventDeduplicator::default(),
            capabilities: config.capabilities,
            limits,
            resources: ResourceStore::new(),
            client_resource_ceilings: HashMap::new(),
            handlers: HashMap::new(),
            model_range_providers: HashMap::new(),
            text_edit_tracker: text_edit::TextEditTracker::default(),
            text_edit_policy: None,
            extension_namespaces: vec![terminal::standard_namespace_mapping()],
            has_negotiated: false,
        };

        Self {
            inner: Arc::new(Mutex::new(inner)),
            outbound_hub: Arc::new(OutboundHub::new()),
            outbound_queue_capacity: config.outbound_queue_capacity,
            max_event_id_bytes: config.max_event_id_bytes,
            pty: Arc::new(srui_pty::PTYManager::default()),
        }
    }

    /// Creates a new `Session` with the given session ID and custom server capabilities (§15).
    #[must_use]
    pub fn with_capabilities(
        session_id: impl Into<String>,
        capabilities: ServerCapabilities,
    ) -> Self {
        Self::with_config(
            session_id,
            SessionConfig {
                capabilities,
                ..SessionConfig::default()
            },
        )
    }

    /// Returns the session ID.
    #[must_use]
    pub fn session_id(&self) -> String {
        let guard = lock_or_recover(&self.inner);
        guard.session_id.clone()
    }

    /// Returns the current lifecycle state of the session (§17).
    #[must_use]
    pub fn state(&self) -> SessionState {
        let guard = lock_or_recover(&self.inner);
        guard.state
    }

    /// Returns the number of currently attached transport connections (§17).
    #[must_use]
    pub fn attached_count(&self) -> usize {
        let guard = lock_or_recover(&self.inner);
        guard.attached_connections
    }

    /// Returns `true` if at least one transport connection is attached (§17).
    #[must_use]
    pub fn is_attached(&self) -> bool {
        self.state() == SessionState::Attached
    }

    /// Returns `true` once any client has completed a handshake against this session (§15, §21).
    ///
    /// Unlike [`Session::is_attached`], this never returns to `false`: a detached client can
    /// resume without a second `ServerWelcome`, so its negotiated capability set outlives the
    /// transport.
    #[must_use]
    pub fn has_negotiated(&self) -> bool {
        let guard = lock_or_recover(&self.inner);
        guard.has_negotiated
    }

    /// Returns `true` if the session is detached from all transports (§17).
    #[must_use]
    pub fn is_detached(&self) -> bool {
        self.state() == SessionState::Detached
    }

    /// Attaches a transport connection to this session (§17, App. B).
    ///
    /// Increments the attached connection count and transitions `DETACHED -> ATTACHED`.
    /// Returns `None` if the session is in a terminal state (`TERMINATING` or `EXPIRED`).
    /// Returns an [`AttachmentGuard`] that automatically decrements the count and transitions
    /// back to `DETACHED` when dropped.
    #[must_use]
    pub fn attach(&self) -> Option<AttachmentGuard> {
        let mut guard = lock_or_recover(&self.inner);
        if guard.state.is_terminal() {
            tracing::warn!(
                session_id = %guard.session_id,
                state = ?guard.state,
                "Refusing attachment to terminal session"
            );
            return None;
        }
        guard.attached_connections = guard.attached_connections.saturating_add(1);
        if guard.state == SessionState::Detached {
            guard.state = SessionState::Attached;
            tracing::info!(
                session_id = %guard.session_id,
                active_attachments = guard.attached_connections,
                "Session transitioned to ATTACHED"
            );
        }
        Some(AttachmentGuard {
            session: self.clone(),
        })
    }

    /// Internal helper to detach a transport connection (§17, App. B).
    pub(crate) fn detach_internal(&self) {
        let mut guard = lock_or_recover(&self.inner);
        guard.attached_connections = guard.attached_connections.saturating_sub(1);
        if guard.attached_connections == 0 && guard.state == SessionState::Attached {
            guard.state = SessionState::Detached;
            tracing::info!(
                session_id = %guard.session_id,
                "Session transitioned to DETACHED; retaining application state"
            );
        }
    }

    /// Marks the session as terminating (§17; triggering policy stubbed in Task 22).
    pub fn terminate(&self) {
        let mut guard = lock_or_recover(&self.inner);
        guard.state = SessionState::Terminating;
        tracing::info!(session_id = %guard.session_id, "Session marked as TERMINATING");
        drop(guard);
        self.shutdown_terminals();
    }

    /// Marks the session as expired (§17; triggering policy stubbed in Task 22).
    pub fn expire(&self) {
        let mut guard = lock_or_recover(&self.inner);
        guard.state = SessionState::Expired;
        tracing::info!(session_id = %guard.session_id, "Session marked as EXPIRED");
        drop(guard);
        self.shutdown_terminals();
    }

    /// Publishes immutable `bytes` into the session resource CAS and, when newly
    /// inserted, schedules transfer to every attached client (§14, §19.2).
    ///
    /// Repeated publication of identical content returns the same hash without
    /// duplicating transfer work. The CAS survives detach/resume for the session
    /// incarnation.
    pub fn publish_resource(
        &self,
        bytes: impl AsRef<[u8]>,
    ) -> Result<PublishOutcome, SessionError> {
        let outcome = {
            let mut guard = self.inner.lock().map_err(|_| SessionError::LockPoisoned)?;
            // Never evict hashes still referenced by the authoritative tree (§14).
            let protected = guard.store.referenced_resource_hashes();
            guard
                .resources
                .publish_resource_protecting(bytes, &protected)?
        };
        if outcome.inserted {
            self.outbound_hub.publish_resource(&outcome.entry);
        }
        Ok(outcome)
    }

    /// Looks up a retained resource by hash (§14).
    pub fn lookup_resource(&self, hash: &ResourceHash) -> Option<ResourceEntry> {
        let guard = lock_or_recover(&self.inner);
        guard.resources.lookup(hash).cloned()
    }

    /// Subscribes to committed transactions for the specified `client_instance_id` (§20.2).
    ///
    /// Holds [`SessionInner`] through subscribe + retained-resource seeding so a concurrent
    /// [`Session::publish_resource`] cannot insert between the snapshot and the live
    /// subscription (same invariant as handshake bootstrap).
    ///
    /// Returns [`SessionError::OutboundClosed`] once [`Session::close_outbound`] has closed the hub.
    pub fn subscribe_transactions(
        &self,
        client_instance_id: Vec<u8>,
    ) -> Result<OutboundReceiver, SessionError> {
        let guard = self.inner.lock().map_err(|_| SessionError::LockPoisoned)?;
        let max_ops = guard.limits.max_transaction_operations as usize;
        let max_frame_size = guard.limits.max_frame_size as usize;
        let max_resource_size = guard.limits.max_resource_size as u64;
        let capacity = self.outbound_queue_capacity;
        let retained = guard.resources.retained_entries();
        // SessionInner -> OutboundHub: do not drop `guard` until after seed_resources.
        let receiver = self.outbound_hub.subscribe(
            client_instance_id,
            capacity,
            max_ops,
            max_frame_size,
            max_resource_size,
        )?;
        self.outbound_hub.seed_resources(&receiver, &retained);
        drop(guard);
        Ok(receiver)
    }

    /// Clears the overflow stale marker after a catch-up snapshot has been written (§20.2).
    pub(crate) fn clear_stale_client(&self, client_instance_id: &[u8]) {
        self.outbound_hub.clear_stale_client(client_instance_id);
    }

    /// Client-supplied bytes retained across every long-lived per-client table (§15, §20.2, §26).
    ///
    /// Each of those tables caps its entry *count*; a count cap says nothing about the size of
    /// what an entry holds, which is precisely how an unbounded `client_instance_id` could grow
    /// the daemon without breaching any declared limit. Exposing the byte total is what makes the
    /// retention invariant assertable: no test can check a budget nothing computes.
    ///
    /// Fixed-size components (a `u64` ceiling, a `usize` depth) are deliberately excluded — they
    /// are already bounded by the entry caps. Only client-controlled, variable-size bytes count.
    pub fn retained_client_state_bytes(&self) -> Result<usize, SessionError> {
        let guard = self.inner.lock().map_err(|_| SessionError::LockPoisoned)?;
        let ceiling_key_bytes: usize = guard.client_resource_ceilings.keys().map(Vec::len).sum();
        let text_edit_key_bytes = guard.text_edit_tracker.retained_client_id_bytes();
        drop(guard);
        Ok(ceiling_key_bytes
            + text_edit_key_bytes
            + self.outbound_hub.retained_stale_client_bytes())
    }

    /// Returns a reference to the session's outbound transaction hub.
    #[cfg(test)]
    pub(crate) fn outbound_hub(&self) -> &Arc<OutboundHub> {
        &self.outbound_hub
    }

    /// Returns the configured outbound transaction queue capacity.
    #[must_use]
    pub fn outbound_queue_capacity(&self) -> usize {
        self.outbound_queue_capacity
    }

    /// Collects transactions to replay starting at `from_revision` using a borrowed journal iterator.
    pub fn collect_replayed_transactions(
        &self,
        from_revision: u64,
    ) -> Result<Vec<Transaction>, SessionError> {
        let guard = self.inner.lock().map_err(|_| SessionError::LockPoisoned)?;
        guard
            .journal
            .iter_from(from_revision)
            .map(|iter| iter.cloned().collect())
            .ok_or(SessionError::ReplayUnavailable)
    }

    /// Registers an event handler for `node` and `event_type` (§29).
    pub fn on<F>(&self, node: impl Into<NodeId>, event_type: TypeRef, handler: F)
    where
        F: Fn(&Session, &Event) + Send + Sync + 'static,
    {
        let mut guard = lock_or_recover(&self.inner);
        guard
            .handlers
            .entry((node.into(), event_type))
            .or_default()
            .push(Arc::new(handler));
    }

    /// Returns the number of registered handlers for a specific node and event type.
    pub fn handler_count(&self, node: impl Into<NodeId>, event_type: TypeRef) -> usize {
        let guard = lock_or_recover(&self.inner);
        guard
            .handlers
            .get(&(node.into(), event_type))
            .map(|v| v.len())
            .unwrap_or(0)
    }

    /// Clears all registered event handlers from this session.
    pub fn clear_handlers(&self) {
        let mut guard = lock_or_recover(&self.inner);
        guard.handlers.clear();
    }

    /// Opens an atomic semantic transaction advancing the graph from revision `N` to `N + 1` (§12.1, §29).
    ///
    /// Applies mutations speculatively on [`UiTransaction`], commits atomically to [`SemanticStore`],
    /// logs the transaction in [`TransactionJournal`], and broadcasts the resulting [`Transaction`]
    /// to all attached client connections.
    pub fn transaction<T, F>(&self, f: F) -> Result<T, SessionError>
    where
        F: FnOnce(&mut UiTransaction) -> Result<T, StoreError>,
    {
        let val = {
            let mut guard = self.inner.lock().map_err(|_| SessionError::LockPoisoned)?;
            let base_revision = guard.store.revision();
            let max_ops = guard.store.limits().max_transaction_operations;
            let mut ui = UiTransaction::new(guard.store.clone_staging(), max_ops);

            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&mut ui)));

            match result {
                Ok(Ok(val)) => {
                    let (staged, ops) = ui.into_staged_and_ops();
                    let deletes_nodes = ops
                        .iter()
                        .any(|op| matches!(op, Operation::DeleteNode { .. }));
                    let commit = AuthoritativeCommit::new(base_revision, ops);

                    // Journal admission is decided before the store mutates: `append` below cannot
                    // fail, so the store and the journal advance together or neither does
                    // (§12.1, §18.1).
                    let permit = guard.journal.prepare(&commit)?;
                    let tx_wire = permit.transaction().clone();
                    guard.store.commit_staging(staged, commit.new_revision());
                    guard.journal.append(permit);
                    if deletes_nodes {
                        let SessionInner {
                            store,
                            text_edit_tracker,
                            ..
                        } = &mut *guard;
                        text_edit_tracker.reclaim_missing_nodes(store);
                    }
                    // Published under `inner` so delivery order equals commit order (§12.1);
                    // see `publish_committed` for why this is not an `async-no-lock-await`
                    // violation.
                    self.publish_committed(&tx_wire);
                    let missing_terminals = if deletes_nodes {
                        self.pty
                            .live_stream_ids()
                            .into_iter()
                            .filter(|id| guard.store.get_node(*id).is_none())
                            .collect()
                    } else {
                        Vec::new()
                    };
                    drop(guard);
                    self.close_terminals_for_deleted_nodes(&missing_terminals);
                    val
                }
                Ok(Err(store_err)) => return Err(SessionError::Store(store_err)),
                Err(panic_payload) => {
                    let panic_msg = if let Some(s) = panic_payload.downcast_ref::<&str>() {
                        s.to_string()
                    } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                        s.clone()
                    } else {
                        "unknown panic".to_string()
                    };
                    return Err(SessionError::Panicked(panic_msg));
                }
            }
        };

        Ok(val)
    }

    /// Publishes one committed transaction to every attached connection (§12.1, §20.2).
    ///
    /// # Locking
    ///
    /// The caller MUST still hold `inner`. The mutex that orders commits is the only thing that
    /// can order publications: releasing it first lets two committers interleave as
    /// `commit(N) | commit(N+1) | publish(N+1) | publish(N)`, which delivers a revision gap that
    /// every conforming replica must reject as divergence even though the journal is correct.
    /// Lock order stays `inner -> subscriber`, matching the handshake bootstrap paths, which
    /// subscribe while holding `inner`.
    ///
    /// This adds no `await`: [`OutboundHub::publish`] is synchronous and never blocks — a full
    /// queue marks the subscriber stale for forced resync rather than waiting — so holding `inner`
    /// across it does not violate `async-no-lock-await`.
    pub(crate) fn publish_committed(&self, tx: &Transaction) {
        self.outbound_hub.publish(tx);
    }

    /// Applies a wire transaction to the store, logs it to the journal,
    /// and publishes it to attached client streams without holding locks across await.
    ///
    /// Only an authoritative commit — `new_revision == base_revision + 1` — may be committed. A
    /// coalesced delivery span is refused here by type: it belongs to a replica's stream, never to
    /// authoritative state (§12.1, §20.4).
    pub fn commit_transaction(&self, tx: Transaction) -> Result<Transaction, SessionError> {
        let commit = AuthoritativeCommit::try_from(tx)?;

        // Fast in-memory critical section (async-no-lock-await)
        let tx = {
            let mut guard = self.inner.lock().map_err(|_| SessionError::LockPoisoned)?;

            // Everything that can fail happens first: staging the operations and admitting the
            // transaction to the journal. The two mutations that follow are infallible, so the
            // store cannot end up ahead of the journal and wedge every later commit on this
            // session with `NonContiguousRevision` (§12.1, §18.1, §20.2).
            let staged = guard.store.prepare_commit(&commit)?;
            let permit = guard.journal.prepare(&commit)?;
            let tx_wire = permit.transaction().clone();
            let deletes_nodes = tx_wire
                .operations
                .iter()
                .any(|op| matches!(op.op, Some(srui_protocol::operation::Op::DeleteNode(_))));

            guard.store.commit_prepared(staged);
            guard.journal.append(permit);
            if deletes_nodes {
                let SessionInner {
                    store,
                    text_edit_tracker,
                    ..
                } = &mut *guard;
                text_edit_tracker.reclaim_missing_nodes(store);
            }
            // Published under `inner` so delivery order equals commit order (§12.1).
            self.publish_committed(&tx_wire);
            let missing_terminals = if deletes_nodes {
                self.pty
                    .live_stream_ids()
                    .into_iter()
                    .filter(|id| guard.store.get_node(*id).is_none())
                    .collect()
            } else {
                Vec::new()
            };
            drop(guard);
            self.close_terminals_for_deleted_nodes(&missing_terminals);
            tx_wire
        };

        Ok(tx)
    }

    /// Processes an incoming client event: checks for deduplication,
    /// validates interactive status against the store, and dispatches to registered handlers (§7.7, §27, §29).
    ///
    /// Returns the settled [`EventOutcome`], which the connection turns into a `SERVER EVENT_ACK`
    /// (§18.2). A validation failure is a *rejection of that event*, not a protocol violation: it
    /// is reported as [`EventOutcome::Rejected`] rather than an `Err`, because tearing the
    /// connection down would make the client reconnect and replay the same invalid event forever.
    /// Infrastructure failures and invalid receive-window sequences remain `Err`.
    pub fn process_event(&self, event: &Event) -> Result<EventOutcome, SessionError> {
        if event.event_id.len() > self.max_event_id_bytes {
            return Err(SessionError::InvalidInput(format!(
                "event_id is {} bytes; configured maximum is {} bytes (§26)",
                event.event_id.len(),
                self.max_event_id_bytes
            )));
        }

        // Cloning and decoding peer-controlled arguments may be proportional to the frame size,
        // so perform that work before entering the session-wide critical section. Admission is
        // still evaluated first semantically: receive-window errors win over a captured decode
        // failure, and malformed replays remain idempotent through the settled result cache.
        let domain = DomainEvent::try_from(event.clone());

        let mut guard = self.inner.lock().map_err(|_| SessionError::LockPoisoned)?;
        match Self::admit_event(&mut guard, event)? {
            EventAdmission::Fresh => {}
            EventAdmission::Existing(outcome) => return Ok(outcome),
        }

        let domain = match domain {
            Ok(domain) => domain,
            Err(wire_error) => {
                let error =
                    EventValidationError::PolicyRejected(format!("malformed event: {wire_error}"));
                return Ok(Self::settle_rejected_event(&mut guard, event, error));
            }
        };

        if domain.event_type == TypeRef::EVENT_TEXT_EDIT {
            return self.process_admitted_text_edit(event, domain, guard);
        }

        let validation = domain
            .validate_observed_revision(guard.store.revision())
            .and_then(|()| domain.validate_node_interactive(&guard.store).map(|_| ()));
        if let Err(error) = validation {
            return Ok(Self::settle_rejected_event(&mut guard, event, error));
        }

        let matching_handlers = guard
            .handlers
            .get(&(domain.node_id, domain.event_type))
            .cloned()
            .unwrap_or_default();
        drop(guard);

        self.dispatch_admitted_event(event, &matching_handlers, HandlerDispatchKind::Ordinary)
    }

    fn admit_event(
        inner: &mut SessionInner,
        event: &Event,
    ) -> Result<EventAdmission, SessionError> {
        let admission = match inner.dedupe.admit_event(event)? {
            RecordOutcome::Duplicate {
                prior,
                last_processed_event_seq,
            } => EventAdmission::Existing(EventOutcome::Duplicate {
                accepted: prior.accepted,
                revision_after_effect: prior.revision_after_effect,
                last_processed_event_seq,
                reject_reason: prior.reject_reason,
            }),
            RecordOutcome::Pending {
                last_processed_event_seq,
            } => EventAdmission::Existing(EventOutcome::Pending {
                last_processed_event_seq,
            }),
            RecordOutcome::Fresh { .. } => EventAdmission::Fresh,
        };
        Ok(admission)
    }

    pub(crate) fn settle_rejected_event(
        inner: &mut SessionInner,
        event: &Event,
        error: EventValidationError,
    ) -> EventOutcome {
        let revision_after_effect = inner.store.revision().get();
        let max_string_length = inner.store.limits().max_string_length;
        let last_processed_event_seq = inner.dedupe.settle_event(
            event,
            EventOutcomeRecord {
                accepted: false,
                revision_after_effect,
                reject_reason: bound_diagnostic_string(error.to_string(), max_string_length),
            },
        );
        EventOutcome::Rejected {
            error,
            revision_after_effect,
            last_processed_event_seq,
        }
    }

    pub(crate) fn dispatch_admitted_event(
        &self,
        event: &Event,
        handlers: &[HandlerFn],
        kind: HandlerDispatchKind,
    ) -> Result<EventOutcome, SessionError> {
        let dispatch = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            for handler in handlers {
                handler(self, event);
            }
        }));

        let panic_payload = match (kind, dispatch) {
            (HandlerDispatchKind::Ordinary, Err(panic_payload)) => {
                let mut guard = self.inner.lock().map_err(|_| SessionError::LockPoisoned)?;
                guard.dedupe.abandon_event(event);
                drop(guard);
                return Err(SessionError::Panicked(panic_payload_message(
                    panic_payload.as_ref(),
                )));
            }
            (_, dispatch) => dispatch.err(),
        };

        // Sample after dispatch so the ACK includes every handler transaction. A TEXT_EDIT is
        // already authoritative at this point, so even a notification-handler panic must settle
        // it as accepted before the infrastructure failure is surfaced.
        let (revision_after_effect, last_processed_event_seq) = {
            let mut guard = self.inner.lock().map_err(|_| SessionError::LockPoisoned)?;
            let revision_after_effect = guard.store.revision().get();
            let last_processed_event_seq = guard.dedupe.settle_event(
                event,
                EventOutcomeRecord {
                    accepted: true,
                    revision_after_effect,
                    reject_reason: String::new(),
                },
            );
            if kind == HandlerDispatchKind::CommittedTextEdit {
                guard
                    .text_edit_tracker
                    .finish_committed_handler_dispatch(event);
            }
            (revision_after_effect, last_processed_event_seq)
        };

        if let Some(panic_payload) = panic_payload {
            return Err(SessionError::Panicked(panic_payload_message(
                panic_payload.as_ref(),
            )));
        }

        Ok(EventOutcome::Processed {
            revision_after_effect,
            last_processed_event_seq,
        })
    }

    /// Returns the current committed semantic revision.
    pub fn current_revision(&self) -> u64 {
        let guard = lock_or_recover(&self.inner);
        guard.store.revision().get()
    }

    /// Highest revision admitted to the journal (§18.1).
    ///
    /// Equal to [`Self::current_revision`] after every commit: the prepared-permit path makes the
    /// append infallible precisely so the two cannot drift (§12.1).
    pub fn journal_latest_revision(&self) -> u64 {
        let guard = lock_or_recover(&self.inner);
        guard.journal.latest_revision()
    }

    /// Negotiated §26 string bound for wire diagnostics such as `reject_reason`.
    pub fn max_string_length(&self) -> usize {
        let guard = lock_or_recover(&self.inner);
        guard.store.limits().max_string_length
    }

    /// Returns the total number of active nodes currently in the store (§6.2).
    pub fn node_count(&self) -> usize {
        let guard = lock_or_recover(&self.inner);
        guard.store.node_count()
    }

    /// Returns `true` if an active node exists with the given ID.
    pub fn contains_node(&self, node: impl Into<NodeId>) -> bool {
        let guard = lock_or_recover(&self.inner);
        guard.store.contains_node(node.into())
    }

    /// Returns a clone of the node with the given ID, if it exists in the store.
    pub fn get_node(&self, node: impl Into<NodeId>) -> Option<srui_semantic_tree::Node> {
        let guard = lock_or_recover(&self.inner);
        guard.store.get_node(node.into()).cloned()
    }

    /// Returns a list of top-level root node IDs.
    pub fn root_ids(&self) -> Vec<NodeId> {
        let guard = lock_or_recover(&self.inner);
        guard.store.root_ids().to_vec()
    }

    /// Executes a read-only query closure against the committed [`SemanticStore`].
    pub fn with_store<T, F>(&self, f: F) -> T
    where
        F: FnOnce(&SemanticStore) -> T,
    {
        let guard = lock_or_recover(&self.inner);
        f(&guard.store)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use srui_semantic_tree::{PropertyRef, Value};

    impl Session {
        fn poison_lock_for_test(&self) {
            let inner = Arc::clone(&self.inner);
            let _ = std::panic::catch_unwind(|| {
                let _guard = inner.lock().unwrap();
                panic!("test lock poison");
            });
        }
    }

    #[test]
    fn test_session_transaction_records_widget_builder_operations() {
        use srui_sdk::{NodeId, Surface};

        let session = Session::new("widget-ops-test");
        session
            .transaction(|ui| {
                Surface::builder(1)
                    .label("Counter Application")
                    .create(ui)?;
                Ok(())
            })
            .expect("widget builder transaction");

        let replayed = session
            .collect_replayed_transactions(0)
            .expect("journal replay available");
        assert_eq!(replayed.len(), 1);
        assert_eq!(replayed[0].base_revision, 0);
        assert_eq!(replayed[0].new_revision, 1);
        assert_eq!(
            replayed[0].operations.len(),
            1,
            "widget builder mutations must be recorded as wire operations"
        );

        session.with_store(|store| {
            let surface = Surface::from_store(store, NodeId::new(1)).expect("surface exists");
            assert_eq!(surface.label(store), Some("Counter Application"));
        });
    }

    #[test]
    fn test_getters_survive_poisoned_lock() {
        let session = Session::new("poison-test");
        session.poison_lock_for_test();
        assert_eq!(session.session_id(), "poison-test");
        assert_eq!(session.current_revision(), 0);
    }

    #[test]
    fn test_session_config_rejects_zero_journal_capacity() {
        let result = std::panic::catch_unwind(|| {
            let _ = Session::with_config(
                "zero-capacity",
                SessionConfig {
                    journal_capacity: 0,
                    ..SessionConfig::default()
                },
            );
        });
        assert!(
            result.is_err(),
            "zero journal_capacity must be rejected (§18.1)"
        );
    }

    /// A zero outbound queue capacity is refused on the same terms as a zero journal window, instead
    /// of being silently clamped to a capacity that cannot hold a transaction (§20.2).
    #[test]
    fn test_session_config_rejects_zero_outbound_queue_capacity() {
        let result = std::panic::catch_unwind(|| {
            let _ = Session::with_config(
                "zero-outbound",
                SessionConfig {
                    outbound_queue_capacity: 0,
                    ..SessionConfig::default()
                },
            );
        });
        assert!(
            result.is_err(),
            "zero outbound_queue_capacity must be rejected (§20.2)"
        );
    }

    /// A coalesced delivery span is refused at the type boundary, before anything is staged, so the
    /// store cannot advance past a journal that will not record it and wedge every later commit
    /// with `NonContiguousRevision` (§12.1, §18.1, §20.4).
    #[test]
    fn test_commit_transaction_rejects_inadmissible_span_without_advancing_store() {
        use srui_sdk::Surface;
        use srui_semantic_tree::{Operation, Revision, Transaction as DomainTransaction};

        let session = Session::new("span-guard");
        session
            .transaction(|ui| {
                Surface::builder(NodeId::new(1)).create(ui)?;
                Ok(())
            })
            .expect("initial surface");
        assert_eq!(session.current_revision(), 1);

        let coalesced = DomainTransaction::with_revisions(
            Revision::new(1),
            Revision::new(5),
            [Operation::SetProperty {
                id: NodeId::new(1),
                property: PropertyRef::LABEL,
                value: Value::String("coalesced".to_string()),
            }],
            0,
        );
        let wire: Transaction = (&coalesced).into();

        let err = session
            .commit_transaction(wire)
            .expect_err("a multi-revision span is not an authoritative commit");
        assert!(
            matches!(
                err,
                SessionError::Transaction(TxnError::InvalidNewRevision {
                    expected: e,
                    actual: a,
                }) if e == Revision::new(2) && a == Revision::new(5)
            ),
            "expected refusal as a non-authoritative form, got {err:?}"
        );
        assert_eq!(
            session.current_revision(),
            1,
            "a refused transaction must leave the store on its committed revision"
        );

        session
            .transaction(|ui| {
                ui.set(NodeId::new(1), PropertyRef::LABEL, "after")?;
                Ok(())
            })
            .expect("session must remain committable after a refused transaction");
        assert_eq!(session.current_revision(), 2);
    }

    /// `ClientHello.client_instance_id` is a proto3 `bytes` field with no documented non-empty
    /// requirement; omitting it must not drop the socket without a diagnostic (§15, §18).
    #[test]
    fn test_bootstrap_fresh_client_accepts_empty_client_instance_id() {
        let session = Session::new("empty-instance-id");
        let hello = srui_protocol::ClientHello {
            core_version: "0.5.0".to_string(),
            profiles: vec!["org.srui.standard-widgets/1".to_string()],
            limits: None,
            client_instance_id: Vec::new(),
            client_metadata: Default::default(),
            known_resource_hashes: vec![],
        };

        let bootstrap = session
            .bootstrap_fresh_client(&hello)
            .expect("empty client_instance_id must complete the handshake");
        assert_eq!(bootstrap.welcome.session_id, "empty-instance-id");
        assert!(bootstrap.transactions.termination().is_none());
    }

    /// `client_instance_id` is client-supplied and is retained as a key by the remembered-ceiling
    /// table, the stale-client record, and every subscriber. Those tables cap their entry count,
    /// not the key size, so an oversized identifier must be refused at the handshake (§15, §26).
    #[test]
    fn test_bootstrap_fresh_client_rejects_oversized_client_instance_id() {
        use super::handshake::MAX_CLIENT_INSTANCE_ID_BYTES;

        let session = Session::new("oversized-instance-id");
        let hello = srui_protocol::ClientHello {
            core_version: "0.5.0".to_string(),
            profiles: vec!["org.srui.standard-widgets/1".to_string()],
            limits: None,
            client_instance_id: vec![7u8; MAX_CLIENT_INSTANCE_ID_BYTES + 1],
            client_metadata: Default::default(),
            known_resource_hashes: vec![],
        };

        match session.bootstrap_fresh_client(&hello) {
            Err(SessionError::InvalidInput(message)) => {
                assert!(
                    message.contains("client_instance_id"),
                    "diagnostic must name the offending field, got {message:?}"
                );
            }
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    /// The resume path keys the same tables, so it must refuse the identifier the fresh path does.
    #[test]
    fn test_bootstrap_resume_rejects_oversized_client_instance_id() {
        use super::handshake::MAX_CLIENT_INSTANCE_ID_BYTES;

        let session = Session::new("oversized-instance-id-resume");
        let resume = srui_protocol::ClientResume {
            session_id: "oversized-instance-id-resume".to_string(),
            client_instance_id: vec![7u8; MAX_CLIENT_INSTANCE_ID_BYTES + 1],
            last_applied_revision: 0,
            last_acked_event_seq: 0,
            terminal_stream_offsets: Default::default(),
            limits: None,
            known_resource_hashes: vec![],
            pending_text_edits: vec![],
        };

        match session.bootstrap_resume(&resume) {
            Err(SessionError::InvalidInput(message)) => {
                assert!(
                    message.contains("client_instance_id"),
                    "diagnostic must name the offending field, got {message:?}"
                );
            }
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[test]
    fn transaction_reclaims_only_nodes_absent_after_reparent_then_delete() {
        let session = Session::new("post-commit-delete-reclamation");
        let root = NodeId::new(1);
        let deleted_editor = NodeId::new(2);
        let surviving_editor = NodeId::new(3);
        session
            .transaction(|ui| {
                ui.create_node(root, TypeRef::SURFACE, None, None, std::iter::empty())?;
                ui.create_node(
                    deleted_editor,
                    TypeRef::TEXT_INPUT,
                    Some(root),
                    None,
                    std::iter::empty(),
                )?;
                ui.create_node(
                    surviving_editor,
                    TypeRef::TEXT_INPUT,
                    Some(deleted_editor),
                    None,
                    std::iter::empty(),
                )?;
                Ok(())
            })
            .expect("seed editor subtree");

        let client = b"client";
        {
            let mut inner = session.inner.lock().unwrap();
            inner
                .text_edit_tracker
                .reserve(
                    client,
                    deleted_editor,
                    srui_semantic_tree::EditSeq::new(5).unwrap(),
                )
                .unwrap();
            inner.text_edit_tracker.mark_terminal(
                client,
                deleted_editor,
                srui_semantic_tree::EditSeq::new(5).unwrap(),
            );
            inner
                .text_edit_tracker
                .reserve(
                    client,
                    surviving_editor,
                    srui_semantic_tree::EditSeq::new(7).unwrap(),
                )
                .unwrap();
            inner.text_edit_tracker.mark_terminal(
                client,
                surviving_editor,
                srui_semantic_tree::EditSeq::new(7).unwrap(),
            );
        }

        session
            .transaction(|ui| {
                ui.move_node(surviving_editor, Some(root), None)?;
                ui.delete(deleted_editor)?;
                Ok(())
            })
            .expect("reparent editor before deleting its old parent");

        assert!(!session.contains_node(deleted_editor));
        assert!(session.contains_node(surviving_editor));
        let inner = session.inner.lock().unwrap();
        assert_eq!(
            inner
                .text_edit_tracker
                .last_terminal_of(client, deleted_editor),
            None
        );
        assert_eq!(
            inner
                .text_edit_tracker
                .last_terminal_of(client, surviving_editor),
            Some(7)
        );
    }

    #[test]
    fn test_outbound_hub_accessible() {
        let session = Session::new("test-outbound-hub");
        assert!(!session.outbound_hub().is_closed());
    }
}
