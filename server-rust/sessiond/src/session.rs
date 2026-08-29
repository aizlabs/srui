//! # Session State & Transaction Coordination
//!
//! Authoritative state owner managing [`SemanticStore`], [`TransactionJournal`],
//! and [`EventDeduplicator`] for a session (§6.3, §12, §18, §18.2, §20.2, §21, App. B).
//!
//! Conforms strictly to [`async-no-lock-await`](rules/async-no-lock-await.md):
//! internal locks are held only for fast in-memory operations and never across `.await` points.
//! Conforms to [`async-bounded-channel`](rules/async-bounded-channel.md):
//! transaction broadcast channels are strictly bounded.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::sync::broadcast;

use srui_event_dedupe::{EventDeduplicator, EventOutcomeRecord, EventSequenceError, RecordOutcome};
use srui_journal::{JournalError, TransactionJournal};
use srui_protocol::{
    ClientHello, ClientResume, Event, ExtensionNamespaceMapping, ServerLimits, ServerResumeOk,
    ServerResyncRequired, ServerWelcome, SessionContinuity, Transaction,
};
use srui_sdk::UiTransaction;
use srui_semantic_tree::{
    CapabilitySet, EventValidationError, NegotiationError, NodeId, Profile, PropertyRef,
    SemanticStore, ServerCapabilities, StoreError, TxnError, TypeRef, Value,
    DEFAULT_MAX_ITEMS_PER_MODEL_OPERATION, DEFAULT_MAX_NODE_COUNT, DEFAULT_MAX_STRING_LENGTH,
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

/// Outcome of a [`ClientResume`] handshake request.
#[derive(Debug, Clone)]
pub enum ResumeOutcome {
    /// The exact requested session survived: sends `ServerResumeOk` followed by replay.
    Replay {
        welcome_msg: ServerResumeOk,
        from_revision: u64,
    },
    /// Full snapshot required, either for a same-session journal gap or a replaced incarnation.
    Resync {
        resync_msg: ServerResyncRequired,
        snapshot_transaction: Transaction,
    },
}

struct SessionInner {
    session_id: String,
    store: SemanticStore,
    journal: TransactionJournal,
    dedupe: EventDeduplicator,
    capabilities: ServerCapabilities,
    limits: ServerLimits,
    handlers: HashMap<(NodeId, TypeRef), Vec<HandlerFn>>,
}

impl std::fmt::Debug for SessionInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionInner")
            .field("session_id", &self.session_id)
            .field("store", &self.store)
            .field("journal", &self.journal)
            .field("dedupe", &self.dedupe)
            .field("capabilities", &self.capabilities)
            .field("limits", &self.limits)
            .field("handler_count", &self.handlers.len())
            .finish()
    }
}

/// Authoritative session controller managing the distributed UI graph.
#[derive(Debug, Clone)]
pub struct Session {
    inner: Arc<Mutex<SessionInner>>,
    tx_broadcast: Arc<Mutex<Option<broadcast::Sender<Transaction>>>>,
}

impl Session {
    /// Creates a new `Session` with the given session ID and default standard capabilities.
    pub fn new(session_id: impl Into<String>) -> Self {
        Self::with_broadcast_capacity(session_id, TRANSACTION_BROADCAST_CAPACITY)
    }

    /// Creates a session with a custom transaction broadcast channel capacity.
    ///
    /// Intended for integration tests that exercise lag/resync behavior (§20.2).
    #[doc(hidden)]
    pub fn with_broadcast_capacity(session_id: impl Into<String>, capacity: usize) -> Self {
        let (tx_broadcast, _) = broadcast::channel(capacity);
        Self::with_broadcast_sender(session_id, tx_broadcast)
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
            store: SemanticStore::new(),
            journal: TransactionJournal::new(1024),
            dedupe: EventDeduplicator::default(),
            capabilities: ServerCapabilities::default(),
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
    pub fn with_capabilities(
        session_id: impl Into<String>,
        capabilities: ServerCapabilities,
    ) -> Self {
        let session = Self::new(session_id);
        session.set_capabilities(capabilities);
        session
    }

    /// Sets the server-side capabilities for this session (§15).
    pub fn set_capabilities(&self, capabilities: ServerCapabilities) {
        let mut guard = lock_or_recover(&self.inner);
        guard.capabilities = capabilities;
    }

    /// Returns the configured server capabilities for this session (§15).
    pub fn capabilities(&self) -> ServerCapabilities {
        let guard = lock_or_recover(&self.inner);
        guard.capabilities.clone()
    }

    /// Returns the session ID.
    pub fn session_id(&self) -> String {
        let guard = lock_or_recover(&self.inner);
        guard.session_id.clone()
    }

    /// Subscribes to committed transaction broadcasts (§20.2).
    ///
    /// Returns [`SessionError::BroadcastClosed`] once [`Session::close_transaction_broadcast`] has
    /// dropped the sender. That hook is test-only, but it is reachable from a live session, and a
    /// panic here would take down the connection-accept task rather than failing one connection.
    pub fn subscribe_transactions(&self) -> Result<broadcast::Receiver<Transaction>, SessionError> {
        lock_or_recover(&self.tx_broadcast)
            .as_ref()
            .map(|sender| sender.subscribe())
            .ok_or(SessionError::BroadcastClosed)
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

    /// Exports the full current semantic store state as a snapshot transaction (§18, §18.1).
    pub fn export_snapshot_transaction(&self) -> Result<Transaction, SessionError> {
        let guard = self.inner.lock().map_err(|_| SessionError::LockPoisoned)?;
        Ok(export_snapshot_transaction(&guard.store))
    }

    /// Evaluates a `ClientHello` handshake message, negotiates capabilities,
    /// and returns the `ServerWelcome` envelope (§15, §18).
    pub fn handle_hello(&self, hello: &ClientHello) -> Result<ServerWelcome, SessionError> {
        let guard = self.inner.lock().map_err(|_| SessionError::LockPoisoned)?;

        let mut client_caps = CapabilitySet::new();
        for p_str in &hello.profiles {
            if let Ok(p) = Profile::parse(p_str) {
                client_caps.insert(p);
            }
        }

        let _negotiated = guard.capabilities.negotiate(&client_caps)?;

        let welcome = ServerWelcome {
            core_version: "0.4.0".to_string(),
            required_profiles: guard.capabilities.required.to_string_vec(),
            optional_profiles: guard.capabilities.optional.to_string_vec(),
            session_id: guard.session_id.clone(),
            initial_revision: guard.store.revision().get(),
            extension_namespaces: vec![ExtensionNamespaceMapping {
                extension_uri: "org.srui.standard-widgets".to_string(),
                namespace_id: 0,
            }],
            limits: Some(guard.limits),
        };

        Ok(welcome)
    }

    /// Evaluates a `ClientResume` reconnection request (§20.2, §21, §32.5).
    pub fn handle_resume(&self, resume: &ClientResume) -> Result<ResumeOutcome, SessionError> {
        #[allow(clippy::large_enum_variant)]
        enum ResumePlan {
            Replay {
                session_id: String,
                from_revision: u64,
                last_processed_event_seq: u64,
            },
            Resync {
                session_id: String,
                snapshot_revision: u64,
                store_snapshot: SemanticStore,
                continuity: SessionContinuity,
                last_processed_event_seq: u64,
            },
        }

        let plan = {
            let guard = self.inner.lock().map_err(|_| SessionError::LockPoisoned)?;
            let last_processed_event_seq = guard
                .dedupe
                .last_contiguous_processed_seq(&resume.client_instance_id);

            // A session ID is an incarnation token, not a human-readable application name. A
            // mismatch means the requested session is gone, so old client intents must not be
            // replayed against this authoritative state.
            if resume.session_id != guard.session_id {
                ResumePlan::Resync {
                    session_id: guard.session_id.clone(),
                    snapshot_revision: guard.store.revision().get(),
                    store_snapshot: guard.store.clone_staging(),
                    continuity: SessionContinuity::Replaced,
                    last_processed_event_seq,
                }
            } else if guard
                .journal
                .iter_from(resume.last_applied_revision)
                .is_some()
            {
                ResumePlan::Replay {
                    session_id: guard.session_id.clone(),
                    from_revision: resume.last_applied_revision,
                    last_processed_event_seq,
                }
            } else {
                ResumePlan::Resync {
                    session_id: guard.session_id.clone(),
                    snapshot_revision: guard.store.revision().get(),
                    store_snapshot: guard.store.clone_staging(),
                    continuity: SessionContinuity::SameSession,
                    last_processed_event_seq,
                }
            }
        };

        match plan {
            ResumePlan::Replay {
                session_id,
                from_revision,
                last_processed_event_seq,
            } => {
                let welcome_msg = ServerResumeOk {
                    session_id,
                    replay_from_revision: from_revision,
                    last_processed_event_seq,
                };
                Ok(ResumeOutcome::Replay {
                    welcome_msg,
                    from_revision,
                })
            }
            ResumePlan::Resync {
                session_id,
                snapshot_revision,
                store_snapshot,
                continuity,
                last_processed_event_seq,
            } => {
                let snapshot_tx = export_snapshot_transaction(&store_snapshot);
                let reason = match continuity {
                    SessionContinuity::SameSession => {
                        "client revision outside retained journal window"
                    }
                    SessionContinuity::Replaced => {
                        "requested session incarnation is no longer available"
                    }
                    SessionContinuity::Unspecified => unreachable!("server always sets continuity"),
                };
                let resync_msg = ServerResyncRequired {
                    session_id,
                    snapshot_revision,
                    reason: reason.to_string(),
                    continuity: continuity as i32,
                    last_processed_event_seq,
                };
                Ok(ResumeOutcome::Resync {
                    resync_msg,
                    snapshot_transaction: snapshot_tx,
                })
            }
        }
    }
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
                    })
                }
                RecordOutcome::Pending {
                    last_processed_event_seq,
                } => {
                    return Ok(EventOutcome::Pending {
                        last_processed_event_seq,
                    })
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

/// Appends one `MODEL_RESET_RANGE` operation carrying `items` starting at `start_index` (§13, §26).
fn push_model_reset_range(
    ops: &mut Vec<srui_protocol::Operation>,
    model_id: u64,
    start_index: u64,
    items: Vec<srui_protocol::ModelItem>,
) {
    ops.push(srui_protocol::Operation {
        op: Some(srui_protocol::operation::Op::ModelResetRange(
            srui_protocol::ModelResetRangeOp {
                model_id,
                start_index,
                items,
                total_count: 0,
            },
        )),
    });
}

fn export_snapshot_transaction(store: &SemanticStore) -> Transaction {
    let mut ops = Vec::new();

    let mut model_ids: Vec<_> = store.model_ids().collect();
    model_ids.sort_by_key(|id| id.get());
    for model_id in model_ids {
        if let Some(model) = store.get_model(model_id) {
            ops.push(srui_protocol::Operation {
                op: Some(srui_protocol::operation::Op::CreateModel(
                    srui_protocol::CreateModelOp {
                        model_id: model.id.get(),
                        model_type: Some(model.model_type.into()),
                        item_count: model.item_count,
                    },
                )),
            });

            // §26: a model may cache up to `max_cached_items_per_model` (100_000) items, but a
            // single model operation may carry at most `max_items_per_model_operation` (10_000).
            // An unchunked range therefore produces a snapshot that every conforming client must
            // reject — and a rejected resync snapshot leaves the client waiting for a snapshot it
            // will reject again (§18).
            for range in model.cached_ranges() {
                let mut chunk_start = range.start;
                let mut chunk: Vec<srui_protocol::ModelItem> = Vec::new();

                for idx in range.start..range.start + range.length {
                    match model.get_item_by_index(idx) {
                        Some(item) => {
                            if chunk.is_empty() {
                                chunk_start = idx;
                            }
                            chunk.push(srui_protocol::ModelItem::from(item));
                            if chunk.len() == DEFAULT_MAX_ITEMS_PER_MODEL_OPERATION {
                                push_model_reset_range(
                                    &mut ops,
                                    model.id.get(),
                                    chunk_start,
                                    std::mem::take(&mut chunk),
                                );
                            }
                        }
                        // `items` are positional from `start_index`, so a hole must end the run
                        // rather than shift every later item down by one.
                        None => {
                            if !chunk.is_empty() {
                                push_model_reset_range(
                                    &mut ops,
                                    model.id.get(),
                                    chunk_start,
                                    std::mem::take(&mut chunk),
                                );
                            }
                        }
                    }
                }

                if !chunk.is_empty() {
                    push_model_reset_range(&mut ops, model.id.get(), chunk_start, chunk);
                }
            }
        }
    }

    fn visit_node(
        store: &SemanticStore,
        node_id: srui_semantic_tree::NodeId,
        child_index: u32,
        ops: &mut Vec<srui_protocol::Operation>,
    ) {
        if let Some(node) = store.get_node(node_id) {
            let record = srui_protocol::NodeRecord {
                node_id: node.id.get(),
                r#type: Some(node.node_type.into()),
                parent_id: node.parent_id.map(|p| p.get()).unwrap_or(0),
                child_index,
                properties: node
                    .properties
                    .iter()
                    .map(|(p, v)| srui_protocol::Property {
                        property: Some((*p).into()),
                        value: Some(v.clone().into()),
                    })
                    .collect(),
            };
            ops.push(srui_protocol::Operation {
                op: Some(srui_protocol::operation::Op::CreateNode(
                    srui_protocol::CreateNodeOp { node: Some(record) },
                )),
            });

            for (idx, &child_id) in node.ordered_children.iter().enumerate() {
                visit_node(store, child_id, idx as u32, ops);
            }
        }
    }

    for (idx, &root_id) in store.root_ids().iter().enumerate() {
        visit_node(store, root_id, idx as u32, &mut ops);
    }

    Transaction {
        base_revision: 0,
        new_revision: store.revision().get(),
        priority: 0,
        operations: ops,
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

    #[test]
    fn test_session_lifecycle() {
        let session = Session::new("test-session");
        assert_eq!(session.session_id(), "test-session");
        assert_eq!(session.current_revision(), 0);

        // ClientHello
        let hello = ClientHello {
            core_version: "0.4.0".to_string(),
            profiles: vec!["org.srui.standard-widgets/1".to_string()],
            limits: None,
            client_instance_id: vec![1, 2],
            client_metadata: Default::default(),
        };

        let welcome = session.handle_hello(&hello).expect("hello negotiated");
        assert_eq!(welcome.session_id, "test-session");
        assert_eq!(welcome.initial_revision, 0);

        // Commit transaction
        let tx = Transaction {
            base_revision: 0,
            new_revision: 1,
            priority: 1,
            operations: vec![],
        };

        let committed = session.commit_transaction(tx).expect("commit tx");
        assert_eq!(committed.new_revision, 1);
        assert_eq!(session.current_revision(), 1);

        // Resume replay
        let resume = ClientResume {
            session_id: "test-session".to_string(),
            client_instance_id: vec![1, 2],
            last_applied_revision: 0,
            last_acked_event_seq: 0,
            terminal_stream_offsets: Default::default(),
        };

        match session.handle_resume(&resume).expect("resume handled") {
            ResumeOutcome::Replay {
                welcome_msg,
                from_revision,
            } => {
                assert_eq!(welcome_msg.replay_from_revision, 0);
                assert_eq!(welcome_msg.last_processed_event_seq, 0);
                let replayed = session
                    .collect_replayed_transactions(from_revision)
                    .expect("replay available");
                assert_eq!(replayed.len(), 1);
                assert_eq!(replayed[0].new_revision, 1);
            }
            ResumeOutcome::Resync { .. } => panic!("expected replay, got resync"),
        }
    }

    #[test]
    fn test_handle_resume_resync_after_journal_eviction() {
        let session = Session::new("resync-test");

        // Journal capacity is 1024; commit 1025 txs to evict revision 0 from replay window.
        for rev in 0..1025 {
            let tx = Transaction {
                base_revision: rev,
                new_revision: rev + 1,
                priority: 1,
                operations: vec![],
            };
            session.commit_transaction(tx).expect("commit tx");
        }
        assert_eq!(session.current_revision(), 1025);

        let resume = ClientResume {
            session_id: "resync-test".to_string(),
            client_instance_id: vec![1],
            last_applied_revision: 0,
            last_acked_event_seq: 0,
            terminal_stream_offsets: Default::default(),
        };

        match session.handle_resume(&resume).expect("resume handled") {
            ResumeOutcome::Resync {
                resync_msg,
                snapshot_transaction,
            } => {
                assert_eq!(resync_msg.session_id, "resync-test");
                assert_eq!(resync_msg.snapshot_revision, 1025);
                assert_eq!(
                    SessionContinuity::try_from(resync_msg.continuity),
                    Ok(SessionContinuity::SameSession)
                );
                assert_eq!(resync_msg.last_processed_event_seq, 0);
                assert_eq!(snapshot_transaction.new_revision, 1025);
            }
            ResumeOutcome::Replay { .. } => panic!("expected resync, got replay"),
        }
    }
}
