//! # Session State & Transaction Coordination
//!
//! Authoritative state owner managing [`SemanticStore`], [`TransactionJournal`],
//! and [`EventDeduplicator`] for a session (§6.3, §12, §18, §20.2, §21).
//!
//! Conforms strictly to [`async-no-lock-await`](rules/async-no-lock-await.md):
//! internal locks are held only for fast in-memory operations and never across `.await` points.
//! Conforms to [`async-bounded-channel`](rules/async-bounded-channel.md):
//! transaction broadcast channels are strictly bounded.

use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

use srui_event_dedupe::EventDeduplicator;
use srui_journal::{JournalError, TransactionJournal};
use srui_protocol::{
    ClientHello, ClientResume, Event, ExtensionNamespaceMapping, ServerLimits,
    ServerResumeOk, ServerResyncRequired, ServerWelcome, Transaction,
};
use srui_semantic_tree::{
    CapabilitySet, EventValidationError, NegotiationError, Profile, PropertyRef, SemanticStore,
    ServerCapabilities, TxnError, Value, DEFAULT_MAX_NODE_COUNT, DEFAULT_MAX_STRING_LENGTH,
    DEFAULT_MAX_TRANSACTION_OPERATIONS, DEFAULT_MAX_TREE_DEPTH,
};
use thiserror::Error;

/// Capacity of the transaction broadcast channel (§20.2).
pub const TRANSACTION_BROADCAST_CAPACITY: usize = 128;

/// Errors produced by session state operations.
#[derive(Debug, Error)]
pub enum SessionError {
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
}

/// Outcome of a [`ClientResume`] handshake request.
#[derive(Debug, Clone)]
pub enum ResumeOutcome {
    /// Replay available: sends `ServerResumeOk` followed by the missing transaction sequence.
    Replay {
        welcome_msg: ServerResumeOk,
        replayed_transactions: Vec<Transaction>,
    },
    /// Replay window expired: client must receive full snapshot resync (§20.2).
    Resync {
        resync_msg: ServerResyncRequired,
        snapshot_transaction: Transaction,
    },
}

#[derive(Debug)]
struct SessionInner {
    session_id: String,
    store: SemanticStore,
    journal: TransactionJournal,
    dedupe: EventDeduplicator,
    capabilities: ServerCapabilities,
    limits: ServerLimits,
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
        };

        Self {
            inner: Arc::new(Mutex::new(inner)),
            tx_broadcast,
        }
    }

    /// Returns the session ID.
    pub fn session_id(&self) -> String {
        let guard = self.inner.lock().unwrap();
        guard.session_id.clone()
    }

    /// Subscribes to committed transaction broadcasts (§20.2).
    pub fn subscribe_transactions(&self) -> broadcast::Receiver<Transaction> {
        self.tx_broadcast.subscribe()
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
            limits: Some(guard.limits.clone()),
        };

        Ok(welcome)
    }

    /// Evaluates a `ClientResume` reconnection request (§20.2, §21, §32.5).
    pub fn handle_resume(&self, resume: &ClientResume) -> Result<ResumeOutcome, SessionError> {
        let guard = self.inner.lock().map_err(|_| SessionError::LockPoisoned)?;

        if let Some(replayed) = guard.journal.replay_from(resume.last_applied_revision) {
            let welcome_msg = ServerResumeOk {
                session_id: guard.session_id.clone(),
                replay_from_revision: resume.last_applied_revision,
            };
            Ok(ResumeOutcome::Replay {
                welcome_msg,
                replayed_transactions: replayed,
            })
        } else {
            // Replay window expired -> full state snapshot required (§20.2)
            let snapshot_tx = export_snapshot_transaction(&guard.store);
            let resync_msg = ServerResyncRequired {
                session_id: guard.session_id.clone(),
                snapshot_revision: guard.store.revision().get(),
                reason: "client revision outside retained journal window".to_string(),
            };
            Ok(ResumeOutcome::Resync {
                resync_msg,
                snapshot_transaction: snapshot_tx,
            })
        }
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
    /// validates interactive status against the store, and dispatches it.
    ///
    /// Returns `Ok(true)` if newly accepted and valid, or `Ok(false)` if duplicate.
    pub fn process_event(&self, event: &Event) -> Result<bool, SessionError> {
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

        Ok(true)
    }

    /// Returns the current revision of the store.
    pub fn current_revision(&self) -> u64 {
        let guard = self.inner.lock().unwrap();
        guard.store.revision().get()
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
            ResumeOutcome::Replay { welcome_msg, replayed_transactions } => {
                assert_eq!(welcome_msg.replay_from_revision, 0);
                assert_eq!(replayed_transactions.len(), 1);
                assert_eq!(replayed_transactions[0].new_revision, 1);
            }
            ResumeOutcome::Resync { .. } => panic!("expected replay, got resync"),
        }
    }
}
