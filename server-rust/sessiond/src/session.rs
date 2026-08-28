//! # Session State & Transaction Coordination
//!
//! Authoritative state owner managing [`SemanticStore`], [`TransactionJournal`],
//! and [`EventDeduplicator`] for a session (§6.3, §12, §18, §20.2, §21).
//!
//! Conforms strictly to [`async-no-lock-await`](rules/async-no-lock-await.md):
//! internal locks are held only for fast in-memory operations and never across `.await` points.
//! Conforms to [`async-bounded-channel`](rules/async-bounded-channel.md):
//! transaction broadcast channels are strictly bounded.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use tokio::sync::broadcast;

use srui_event_dedupe::EventDeduplicator;
use srui_journal::{JournalError, TransactionJournal};
use srui_protocol::{
    ClientHello, ClientResume, Event, ExtensionNamespaceMapping, ServerLimits,
    ServerResumeOk, ServerResyncRequired, ServerWelcome, Transaction,
};
use srui_sdk::UiTransaction;
use srui_semantic_tree::{
    CapabilitySet, EventValidationError, NegotiationError, NodeId, Profile, PropertyRef,
    SemanticStore, ServerCapabilities, StoreError, TxnError, TypeRef, Value,
    DEFAULT_MAX_NODE_COUNT, DEFAULT_MAX_STRING_LENGTH, DEFAULT_MAX_TRANSACTION_OPERATIONS,
    DEFAULT_MAX_TREE_DEPTH,
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

    #[error("lock poisoned")]
    LockPoisoned,

    #[error("client lagged behind transaction broadcast; resync required")]
    LaggedResyncRequired,

    #[error("replay unavailable for requested revision")]
    ReplayUnavailable,

    #[error("transaction panicked: {0}")]
    Panicked(String),
}

fn lock_or_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// Outcome of a [`ClientResume`] handshake request.
#[derive(Debug, Clone)]
pub enum ResumeOutcome {
    /// Replay available: sends `ServerResumeOk` followed by the missing transaction sequence.
    Replay {
        welcome_msg: ServerResumeOk,
        from_revision: u64,
    },
    /// Replay window expired: client must receive full snapshot resync (§20.2).
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
    tx_broadcast: broadcast::Sender<Transaction>,
}

impl Session {
    /// Creates a new `Session` with the given session ID and default standard capabilities.
    pub fn new(session_id: impl Into<String>) -> Self {
        let (tx_broadcast, _) = broadcast::channel(TRANSACTION_BROADCAST_CAPACITY);
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
            tx_broadcast,
        }
    }

    /// Returns the session ID.
    pub fn session_id(&self) -> String {
        let guard = lock_or_recover(&self.inner);
        guard.session_id.clone()
    }

    /// Subscribes to committed transaction broadcasts (§20.2).
    pub fn subscribe_transactions(&self) -> broadcast::Receiver<Transaction> {
        self.tx_broadcast.subscribe()
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
            },
            Resync {
                session_id: String,
                snapshot_revision: u64,
                store_snapshot: SemanticStore,
            },
        }

        let plan = {
            let guard = self.inner.lock().map_err(|_| SessionError::LockPoisoned)?;

            if guard.journal.iter_from(resume.last_applied_revision).is_some() {
                ResumePlan::Replay {
                    session_id: guard.session_id.clone(),
                    from_revision: resume.last_applied_revision,
                }
            } else {
                ResumePlan::Resync {
                    session_id: guard.session_id.clone(),
                    snapshot_revision: guard.store.revision().get(),
                    store_snapshot: guard.store.clone_staging(),
                }
            }
        };

        match plan {
            ResumePlan::Replay {
                session_id,
                from_revision,
            } => {
                let welcome_msg = ServerResumeOk {
                    session_id,
                    replay_from_revision: from_revision,
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
            } => {
                let snapshot_tx = export_snapshot_transaction(&store_snapshot);
                let resync_msg = ServerResyncRequired {
                    session_id,
                    snapshot_revision,
                    reason: "client revision outside retained journal window".to_string(),
                };
                Ok(ResumeOutcome::Resync {
                    resync_msg,
                    snapshot_transaction: snapshot_tx,
                })
            }
        }
    }

    /// Registers an in-process semantic event handler for the given node and event type (§7.6, §29).
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
        let _ = self.tx_broadcast.send(tx);
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
        let _ = self.tx_broadcast.send(tx.clone());
        Ok(tx)
    }

    /// Processes an incoming client event: checks for deduplication,
    /// validates interactive status against the store, and dispatches to registered handlers (§7.7, §27, §29).
    ///
    /// Returns `Ok(true)` if newly accepted and valid, or `Ok(false)` if duplicate.
    pub fn process_event(&self, event: &Event) -> Result<bool, SessionError> {
        let matching_handlers = {
            let mut guard = self.inner.lock().map_err(|_| SessionError::LockPoisoned)?;

            // Deduplication check (§18.2, §32.4)
            if !guard.dedupe.record_event(event) {
                return Ok(false); // Duplicate event ignored
            }

            let node_id = srui_semantic_tree::NodeId::new(event.node_id);
            let obs_rev = srui_semantic_tree::Revision::new(event.observed_revision);

            if obs_rev > guard.store.revision() {
                return Err(SessionError::EventValidation(
                    EventValidationError::FutureRevision {
                        observed: obs_rev,
                        current: guard.store.revision(),
                    },
                ));
            }

            let node = guard
                .store
                .get_node(node_id)
                .ok_or(EventValidationError::NodeNotFound(node_id))?;

            if let Some(Value::Bool(false)) = node.get_property(PropertyRef::ENABLED) {
                return Err(SessionError::EventValidation(
                    EventValidationError::NodeDisabled(node_id),
                ));
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

        for handler in matching_handlers {
            handler(self, event);
        }

        Ok(true)
    }

    /// Returns the current revision of the store.
    pub fn current_revision(&self) -> u64 {
        let guard = lock_or_recover(&self.inner);
        guard.store.revision().get()
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

fn export_snapshot_transaction(store: &SemanticStore) -> Transaction {
    let mut ops = Vec::new();

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
            ResumeOutcome::Replay { welcome_msg, from_revision } => {
                assert_eq!(welcome_msg.replay_from_revision, 0);
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
                assert_eq!(snapshot_transaction.new_revision, 1025);
            }
            ResumeOutcome::Replay { .. } => panic!("expected resync, got replay"),
        }
    }
}
