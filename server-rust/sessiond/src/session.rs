//! # Session State & Transaction Coordination
//!
//! Authoritative state owner managing [`SemanticStore`], [`TransactionJournal`],
//! and [`EventDeduplicator`] for a session (§6.3, §12, §18, §18.2, §20.2, §21, App. B).
//!
//! Conforms strictly to [`async-no-lock-await`](rules/async-no-lock-await.md):
//! internal locks are held only for fast in-memory operations and never across `.await` points.
//! Conforms to [`async-bounded-channel`](rules/async-bounded-channel.md):
//! transaction broadcast channels are strictly bounded.

mod handshake;
mod snapshot;

pub use handshake::{FreshClientBootstrap, ResumeClientBootstrap, ResumeOutcome};

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::sync::broadcast;

/// Lifecycle states of an authoritative semantic session (§17, App. B).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SessionState {
    /// No active transport attachment; application and semantic store remain alive (§17).
    Detached,
    /// At least one transport connection is actively attached and streaming (§17).
    Attached,
    /// Session is terminating (§17; triggering policy stubbed in Task 22).
    Terminating,
    /// Session has expired and is discarded (§17; triggering policy stubbed in Task 22).
    Expired,
}

/// Mints an opaque, globally unique 128-bit session incarnation token (§17).
///
/// Returns a 32-character lowercase hex string with 128 bits of cryptographic entropy.
/// A server process restart MUST mint a fresh token and MUST NOT reuse tokens across runs (§17).
pub fn mint_session_id() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("failed to generate random bytes for session_id");
    let mut s = String::with_capacity(32);
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(s, "{:02x}", b);
    }
    s
}

use srui_event_dedupe::{EventDeduplicator, EventOutcomeRecord, EventSequenceError, RecordOutcome};
use srui_journal::{JournalError, TransactionJournal};
use srui_protocol::{Event, ServerLimits, Transaction};
use srui_sdk::UiTransaction;
use srui_semantic_tree::{
    EventValidationError, NegotiationError, NodeId, PropertyRef, SemanticStore, ServerCapabilities,
    StoreError, TxnError, TypeRef, Value, DEFAULT_MAX_NODE_COUNT, DEFAULT_MAX_STRING_LENGTH,
    DEFAULT_MAX_TRANSACTION_OPERATIONS, DEFAULT_MAX_TREE_DEPTH,
};
use thiserror::Error;

/// Capacity of the transaction broadcast channel (§20.2).
pub const TRANSACTION_BROADCAST_CAPACITY: usize = 128;

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

    #[error("client lagged behind transaction broadcast; resync required")]
    LaggedResyncRequired,

    #[error("replay unavailable for requested revision")]
    ReplayUnavailable,

    #[error("transaction broadcast channel is closed")]
    BroadcastClosed,

    #[error("transaction panicked: {0}")]
    Panicked(String),
}

fn lock_or_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn subscribe_tx_broadcast(
    tx_broadcast: &Mutex<Option<broadcast::Sender<Transaction>>>,
) -> Result<broadcast::Receiver<Transaction>, SessionError> {
    tx_broadcast
        .lock()
        .map_err(|_| SessionError::LockPoisoned)?
        .as_ref()
        .map(|sender| sender.subscribe())
        .ok_or(SessionError::BroadcastClosed)
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

pub(crate) struct SessionInner {
    pub(crate) session_id: String,
    pub(crate) state: SessionState,
    pub(crate) attached_connections: usize,
    pub(crate) store: SemanticStore,
    pub(crate) journal: TransactionJournal,
    pub(crate) dedupe: EventDeduplicator,
    pub(crate) capabilities: ServerCapabilities,
    pub(crate) limits: ServerLimits,
    pub(crate) handlers: HashMap<(NodeId, TypeRef), Vec<HandlerFn>>,
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
            .field("handler_count", &self.handlers.len())
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

impl Drop for AttachmentGuard {
    fn drop(&mut self) {
        self.session.detach_internal();
    }
}

/// Authoritative session controller managing the distributed UI graph.
#[derive(Debug, Clone)]
pub struct Session {
    pub(crate) inner: Arc<Mutex<SessionInner>>,
    pub(crate) tx_broadcast: Arc<Mutex<Option<broadcast::Sender<Transaction>>>>,
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

    /// Creates a new `Session` with the given session ID and default standard capabilities.
    pub fn new(session_id: impl Into<String>) -> Self {
        Self::with_capabilities(session_id, ServerCapabilities::standard_widgets())
    }

    /// Creates a session with a custom transaction broadcast channel capacity.
    ///
    /// Intended for integration tests that exercise lag/resync behavior (§20.2).
    #[doc(hidden)]
    pub fn with_broadcast_capacity(session_id: impl Into<String>, capacity: usize) -> Self {
        let (tx_broadcast, _) = broadcast::channel(capacity);
        Self::with_broadcast_sender(
            session_id,
            tx_broadcast,
            ServerCapabilities::standard_widgets(),
        )
    }

    /// Drops the transaction broadcast sender so attached subscribers observe
    /// [`broadcast::error::RecvError::Closed`] (§20.2).
    #[doc(hidden)]
    pub fn close_transaction_broadcast(&self) {
        lock_or_recover(&self.tx_broadcast).take();
    }

    fn with_broadcast_sender(
        session_id: impl Into<String>,
        tx_broadcast: broadcast::Sender<Transaction>,
        capabilities: ServerCapabilities,
    ) -> Self {
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
            journal: TransactionJournal::new(1024),
            dedupe: EventDeduplicator::default(),
            capabilities,
            limits,
            handlers: HashMap::new(),
        };

        Self {
            inner: Arc::new(Mutex::new(inner)),
            tx_broadcast: Arc::new(Mutex::new(Some(tx_broadcast))),
        }
    }

    fn broadcast_sender(&self) -> Option<broadcast::Sender<Transaction>> {
        lock_or_recover(&self.tx_broadcast).clone()
    }

    /// Creates a new `Session` with the given session ID and custom server capabilities (§15).
    #[must_use]
    pub fn with_capabilities(
        session_id: impl Into<String>,
        capabilities: ServerCapabilities,
    ) -> Self {
        let (tx_broadcast, _) = broadcast::channel(TRANSACTION_BROADCAST_CAPACITY);
        Self::with_broadcast_sender(session_id, tx_broadcast, capabilities)
    }

    /// Returns the session ID.
    pub fn session_id(&self) -> String {
        let guard = lock_or_recover(&self.inner);
        guard.session_id.clone()
    }

    /// Returns the current lifecycle state of the session (§17).
    pub fn state(&self) -> SessionState {
        let guard = lock_or_recover(&self.inner);
        guard.state
    }

    /// Returns the number of currently attached transport connections (§17).
    pub fn attached_count(&self) -> usize {
        let guard = lock_or_recover(&self.inner);
        guard.attached_connections
    }

    /// Returns `true` if at least one transport connection is attached (§17).
    pub fn is_attached(&self) -> bool {
        self.state() == SessionState::Attached
    }

    /// Returns `true` if the session is detached from all transports (§17).
    pub fn is_detached(&self) -> bool {
        self.state() == SessionState::Detached
    }

    /// Attaches a transport connection to this session (§17, App. B).
    ///
    /// Increments the attached connection count and transitions `DETACHED -> ATTACHED`.
    /// Returns an [`AttachmentGuard`] that automatically decrements the count and transitions
    /// back to `DETACHED` when dropped.
    #[must_use]
    pub fn attach(&self) -> AttachmentGuard {
        {
            let mut guard = lock_or_recover(&self.inner);
            guard.attached_connections = guard.attached_connections.saturating_add(1);
            if guard.state == SessionState::Detached {
                guard.state = SessionState::Attached;
                tracing::info!(
                    "Session {} transitioned to ATTACHED (active attachments: {})",
                    guard.session_id,
                    guard.attached_connections
                );
            }
        }
        AttachmentGuard {
            session: self.clone(),
        }
    }

    /// Internal helper to detach a transport connection (§17, App. B).
    pub(crate) fn detach_internal(&self) {
        let mut guard = lock_or_recover(&self.inner);
        guard.attached_connections = guard.attached_connections.saturating_sub(1);
        if guard.attached_connections == 0 && guard.state == SessionState::Attached {
            guard.state = SessionState::Detached;
            tracing::info!(
                "Session {} transitioned to DETACHED (active attachments: 0); retaining application state",
                guard.session_id
            );
        }
    }

    /// Marks the session as terminating (§17; triggering policy stubbed in Task 22).
    pub fn terminate(&self) {
        let mut guard = lock_or_recover(&self.inner);
        guard.state = SessionState::Terminating;
        tracing::info!("Session {} marked as TERMINATING", guard.session_id);
    }

    /// Marks the session as expired (§17; triggering policy stubbed in Task 22).
    pub fn expire(&self) {
        let mut guard = lock_or_recover(&self.inner);
        guard.state = SessionState::Expired;
        tracing::info!("Session {} marked as EXPIRED", guard.session_id);
    }

    /// Subscribes to committed transaction broadcasts (§20.2).
    ///
    /// Returns [`SessionError::BroadcastClosed`] once [`Session::close_transaction_broadcast`] has
    /// dropped the sender. That hook is test-only, but it is reachable from a live session, and a
    /// panic here would take down the connection-accept task rather than failing one connection.
    pub fn subscribe_transactions(&self) -> Result<broadcast::Receiver<Transaction>, SessionError> {
        subscribe_tx_broadcast(&self.tx_broadcast)
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
        let (val, tx) = {
            let mut guard = self.inner.lock().map_err(|_| SessionError::LockPoisoned)?;
            let base_revision = guard.store.revision();
            let max_ops = guard.store.limits().max_transaction_operations;
            let mut ui = UiTransaction::new(guard.store.clone_staging(), max_ops);

            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(&mut ui)));

            match result {
                Ok(Ok(val)) => {
                    let (staged, ops) = ui.into_staged_and_ops();
                    let new_rev = base_revision.next();
                    guard.store.commit_staging(staged, new_rev);

                    let tx_domain = srui_semantic_tree::Transaction::new(base_revision, ops);
                    let tx_wire: Transaction = tx_domain.into();
                    guard.journal.record(tx_wire.clone())?;
                    (val, tx_wire)
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

        // Broadcast to attached bridges outside of the mutex lock (§20.2, async-no-lock-await)
        if let Some(tx_broadcast) = self.broadcast_sender() {
            let _ = tx_broadcast.send(tx);
        }
        Ok(val)
    }

    /// Applies a wire transaction to the store, logs it to the journal,
    /// and broadcasts it to attached client streams without holding locks across await.
    pub fn commit_transaction(&self, tx: Transaction) -> Result<Transaction, SessionError> {
        // Fast in-memory critical section (async-no-lock-await)
        {
            let mut guard = self.inner.lock().map_err(|_| SessionError::LockPoisoned)?;
            guard.store.apply_wire_transaction(tx.clone())?;
            guard.journal.record(tx.clone())?;
        }

        // Broadcast to attached bridges outside of the mutex lock
        if let Some(tx_broadcast) = self.broadcast_sender() {
            let _ = tx_broadcast.send(tx.clone());
        }
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
        let matching_handlers = {
            let mut guard = self.inner.lock().map_err(|_| SessionError::LockPoisoned)?;

            // Deduplication check (§18.2, §32.4). A settled replay is answered from the result
            // cache; an in-flight replay remains unacknowledged so the client cannot mistake it
            // for a completed action.
            match guard.dedupe.admit_event(event)? {
                RecordOutcome::Duplicate {
                    prior,
                    last_processed_event_seq,
                } => {
                    return Ok(EventOutcome::Duplicate {
                        accepted: prior.accepted,
                        revision_after_effect: prior.revision_after_effect,
                        last_processed_event_seq,
                        reject_reason: prior.reject_reason,
                    });
                }
                RecordOutcome::Pending {
                    last_processed_event_seq,
                } => {
                    return Ok(EventOutcome::Pending {
                        last_processed_event_seq,
                    });
                }
                RecordOutcome::Fresh { .. } => {}
            }

            let node_id = srui_semantic_tree::NodeId::new(event.node_id);
            let obs_rev = srui_semantic_tree::Revision::new(event.observed_revision);
            let current_rev = guard.store.revision();

            let validation = if obs_rev > current_rev {
                Err(EventValidationError::FutureRevision {
                    observed: obs_rev,
                    current: current_rev,
                })
            } else {
                match guard.store.get_node(node_id) {
                    None => Err(EventValidationError::NodeNotFound(node_id)),
                    Some(node) => {
                        if let Some(Value::Bool(false)) = node.get_property(PropertyRef::ENABLED) {
                            Err(EventValidationError::NodeDisabled(node_id))
                        } else {
                            Ok(())
                        }
                    }
                }
            };

            if let Err(error) = validation {
                let max_string_length = guard.store.limits().max_string_length;
                let last_processed_event_seq = guard.dedupe.settle_event(
                    event,
                    EventOutcomeRecord {
                        accepted: false,
                        revision_after_effect: current_rev.get(),
                        reject_reason: bound_diagnostic_string(
                            error.to_string(),
                            max_string_length,
                        ),
                    },
                );
                return Ok(EventOutcome::Rejected {
                    error,
                    revision_after_effect: current_rev.get(),
                    last_processed_event_seq,
                });
            }

            let event_type = event
                .event_type
                .map(srui_semantic_tree::TypeRef::from)
                .unwrap_or(srui_semantic_tree::TypeRef::new(0, 0));

            guard
                .handlers
                .get(&(node_id, event_type))
                .cloned()
                .unwrap_or_default()
        }; // Lock released here!

        let dispatch_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            for handler in matching_handlers {
                handler(self, event);
            }
        }));

        if let Err(panic_payload) = dispatch_result {
            let mut guard = self.inner.lock().map_err(|_| SessionError::LockPoisoned)?;
            guard.dedupe.abandon_event(event);
            drop(guard);

            let panic_msg = if let Some(s) = panic_payload.downcast_ref::<&str>() {
                s.to_string()
            } else if let Some(s) = panic_payload.downcast_ref::<String>() {
                s.clone()
            } else {
                "unknown panic".to_string()
            };
            return Err(SessionError::Panicked(panic_msg));
        }

        // Sampled after dispatch so the ack reports the revision the side effect produced
        // (App. B `semantic_revision_after_effect`).
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
            (revision_after_effect, last_processed_event_seq)
        };

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
}
