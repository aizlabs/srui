use crate::outbound::OutboundReceiver;
use srui_protocol::{
    ClientHello, ClientResume, ExtensionNamespaceMapping, ServerResumeOk, ServerResyncRequired,
    ServerWelcome, SessionContinuity, Transaction,
};
use srui_semantic_tree::{CapabilitySet, Profile, SemanticStore};

use super::snapshot::export_snapshot_transaction;
use super::{Session, SessionError};

/// Result of an atomic fresh-client handshake bootstrap (§15, §18, §20.2).
#[derive(Debug)]
pub struct FreshClientBootstrap {
    /// Negotiated welcome parameters sent to the fresh client (§15).
    pub welcome: ServerWelcome,
    /// Catch-up snapshot transaction for populated sessions, or `None` if revision is 0 (§18).
    pub snapshot: Option<Transaction>,
    /// Bounded outbound receiver capturing every subsequent transaction committed to the session (§20.2).
    pub transactions: OutboundReceiver,
}

/// Result of an atomic resume handshake bootstrap (§20.2, §21, §32.5).
#[derive(Debug)]
pub struct ResumeClientBootstrap {
    /// Replay or resync catch-up collected under the same lock as [`Self::transactions`].
    pub outcome: ResumeOutcome,
    /// Bounded outbound receiver capturing every subsequent transaction committed to the session (§20.2).
    pub transactions: OutboundReceiver,
}

/// Outcome of a [`ClientResume`] handshake request.
#[derive(Debug, Clone)]
pub enum ResumeOutcome {
    /// The exact requested session survived: sends `ServerResumeOk` followed by replay.
    Replay {
        welcome_msg: ServerResumeOk,
        replayed: Vec<Transaction>,
    },
    /// Full snapshot required, either for a same-session journal gap or a replaced incarnation.
    Resync {
        resync_msg: ServerResyncRequired,
        snapshot_transaction: Transaction,
    },
}

fn negotiate_hello(
    inner: &super::SessionInner,
    hello: &ClientHello,
) -> Result<(ServerWelcome, Option<SemanticStore>), SessionError> {
    let mut client_caps = CapabilitySet::new();
    for p_str in &hello.profiles {
        if let Ok(p) = Profile::parse(p_str) {
            client_caps.insert(p);
        }
    }

    let _negotiated = inner.capabilities.negotiate(&client_caps)?;

    let initial_revision = inner.store.revision().get();
    let welcome = ServerWelcome {
        core_version: "0.4.0".to_string(),
        required_profiles: inner.capabilities.required.to_string_vec(),
        optional_profiles: inner.capabilities.optional.to_string_vec(),
        session_id: inner.session_id.clone(),
        initial_revision,
        extension_namespaces: vec![ExtensionNamespaceMapping {
            extension_uri: "org.srui.standard-widgets".to_string(),
            namespace_id: 0,
        }],
        limits: Some(inner.limits),
    };

    let store_clone = if initial_revision > 0 {
        Some(inner.store.clone_staging())
    } else {
        None
    };

    Ok((welcome, store_clone))
}

fn resync_reason(continuity: SessionContinuity) -> &'static str {
    match continuity {
        SessionContinuity::SameSession => "client revision outside retained journal window",
        SessionContinuity::Replaced => "requested session incarnation is no longer available",
        SessionContinuity::Unspecified => unreachable!("server always sets continuity"),
    }
}

impl Session {
    /// Atomically prepares a fresh client handshake and subscribes it to transactions (§15, §18, §20.2).
    ///
    /// Evaluates `ClientHello`, negotiates capabilities, constructs `ServerWelcome`, clones
    /// authoritative state when `initial_revision > 0`, and subscribes to `tx_broadcast` while
    /// holding the session lock so no concurrent transaction commit can be missed between catch-up
    /// capture and subscription. Snapshot encoding happens after both locks are dropped.
    ///
    /// # Lock Order
    /// Acquires `inner -> tx_broadcast`. All other session operations acquire at most one of these mutexes,
    /// preserving strict deadlock freedom.
    ///
    /// # Revision-Zero Omission
    /// When `initial_revision == 0`, `snapshot` is `None` because an empty `0 -> 0` snapshot violates normal
    /// transaction invariants.
    ///
    /// # Errors
    /// Returns [`SessionError::Negotiation`] if capability negotiation fails against client profiles.
    /// Returns [`SessionError::LockPoisoned`] if an internal mutex is poisoned.
    /// Returns [`SessionError::BroadcastClosed`] if the transaction broadcast sender has been closed.
    pub fn bootstrap_fresh_client(
        &self,
        hello: &ClientHello,
    ) -> Result<FreshClientBootstrap, SessionError> {
        self.bootstrap_fresh_client_with(hello, || {})
    }

    pub(crate) fn bootstrap_fresh_client_with<F>(
        &self,
        hello: &ClientHello,
        before_subscribe: F,
    ) -> Result<FreshClientBootstrap, SessionError>
    where
        F: FnOnce(),
    {
        let inner_guard = self.inner.lock().map_err(|_| SessionError::LockPoisoned)?;
        let (welcome, store_clone) = negotiate_hello(&inner_guard, hello)?;

        before_subscribe();

        let max_ops = inner_guard.limits.max_transaction_operations as usize;
        let transactions = self.outbound_hub.subscribe(
            hello.client_instance_id.clone(),
            self.outbound_queue_capacity,
            max_ops,
        )?;
        drop(inner_guard);

        Ok(FreshClientBootstrap {
            welcome,
            snapshot: store_clone.map(|store| export_snapshot_transaction(&store)),
            transactions,
        })
    }

    /// Atomically prepares a resume handshake and subscribes it to transactions (§20.2, §21, §32.5).
    ///
    /// Collects journal replay or a store clone for resync, then subscribes to `outbound_hub` while
    /// still holding `inner`, so a concurrent commit cannot land between catch-up capture and subscription.
    /// Resync snapshot encoding happens after both locks are dropped.
    ///
    /// # Errors
    /// Returns [`SessionError::LockPoisoned`] if an internal mutex is poisoned.
    /// Returns [`SessionError::BroadcastClosed`] if the transaction hub has been closed.
    pub fn bootstrap_resume(
        &self,
        resume: &ClientResume,
    ) -> Result<ResumeClientBootstrap, SessionError> {
        self.bootstrap_resume_with(resume, || {})
    }

    pub(crate) fn bootstrap_resume_with<F>(
        &self,
        resume: &ClientResume,
        before_subscribe: F,
    ) -> Result<ResumeClientBootstrap, SessionError>
    where
        F: FnOnce(),
    {
        #[allow(clippy::large_enum_variant)]
        enum ResumePlan {
            Replay {
                welcome_msg: ServerResumeOk,
                replayed: Vec<Transaction>,
            },
            Resync {
                session_id: String,
                snapshot_revision: u64,
                store_snapshot: SemanticStore,
                continuity: SessionContinuity,
                last_processed_event_seq: u64,
            },
        }

        let inner_guard = self.inner.lock().map_err(|_| SessionError::LockPoisoned)?;
        let last_processed_event_seq = inner_guard
            .dedupe
            .last_contiguous_processed_seq(&resume.client_instance_id);

        let is_stale = self
            .outbound_hub
            .is_client_stale(&resume.client_instance_id);

        // A session ID is an incarnation token, not a human-readable application name. A
        // mismatch means the requested session is gone, so old client intents must not be
        // replayed against this authoritative state. If the client detached due to outbound
        // queue overflow, force snapshot resync even if journal replay is otherwise available.
        let plan = if is_stale {
            ResumePlan::Resync {
                session_id: inner_guard.session_id.clone(),
                snapshot_revision: inner_guard.store.revision().get(),
                store_snapshot: inner_guard.store.clone_staging(),
                continuity: SessionContinuity::SameSession,
                last_processed_event_seq,
            }
        } else if resume.session_id != inner_guard.session_id {
            ResumePlan::Resync {
                session_id: inner_guard.session_id.clone(),
                snapshot_revision: inner_guard.store.revision().get(),
                store_snapshot: inner_guard.store.clone_staging(),
                continuity: SessionContinuity::Replaced,
                last_processed_event_seq,
            }
        } else if let Some(iter) = inner_guard.journal.iter_from(resume.last_applied_revision) {
            ResumePlan::Replay {
                welcome_msg: ServerResumeOk {
                    session_id: inner_guard.session_id.clone(),
                    replay_from_revision: resume.last_applied_revision,
                    last_processed_event_seq,
                },
                replayed: iter.cloned().collect(),
            }
        } else {
            ResumePlan::Resync {
                session_id: inner_guard.session_id.clone(),
                snapshot_revision: inner_guard.store.revision().get(),
                store_snapshot: inner_guard.store.clone_staging(),
                continuity: SessionContinuity::SameSession,
                last_processed_event_seq,
            }
        };

        before_subscribe();

        let max_ops = inner_guard.limits.max_transaction_operations as usize;
        let transactions = self.outbound_hub.subscribe(
            resume.client_instance_id.clone(),
            self.outbound_queue_capacity,
            max_ops,
        )?;
        drop(inner_guard);

        let outcome = match plan {
            ResumePlan::Replay {
                welcome_msg,
                replayed,
            } => ResumeOutcome::Replay {
                welcome_msg,
                replayed,
            },
            ResumePlan::Resync {
                session_id,
                snapshot_revision,
                store_snapshot,
                continuity,
                last_processed_event_seq,
            } => ResumeOutcome::Resync {
                resync_msg: ServerResyncRequired {
                    session_id,
                    snapshot_revision,
                    reason: resync_reason(continuity).to_string(),
                    continuity: continuity as i32,
                    last_processed_event_seq,
                },
                snapshot_transaction: export_snapshot_transaction(&store_snapshot),
            },
        };

        Ok(ResumeClientBootstrap {
            outcome,
            transactions,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier, TryLockError};
    use std::thread;

    fn sample_hello() -> ClientHello {
        ClientHello {
            core_version: "0.4.0".to_string(),
            profiles: vec!["org.srui.standard-widgets/1".to_string()],
            limits: None,
            client_instance_id: vec![1, 2],
            client_metadata: Default::default(),
        }
    }

    #[test]
    fn test_session_lifecycle() {
        let session = Session::new("test-session");
        assert_eq!(session.session_id(), "test-session");
        assert_eq!(session.current_revision(), 0);

        let hello = sample_hello();
        let bootstrap = session
            .bootstrap_fresh_client(&hello)
            .expect("hello negotiated");
        assert_eq!(bootstrap.welcome.session_id, "test-session");
        assert_eq!(bootstrap.welcome.initial_revision, 0);
        assert!(bootstrap.snapshot.is_none());

        let tx = Transaction {
            base_revision: 0,
            new_revision: 1,
            priority: 1,
            operations: vec![],
        };

        let committed = session.commit_transaction(tx).expect("commit tx");
        assert_eq!(committed.new_revision, 1);
        assert_eq!(session.current_revision(), 1);

        let bootstrap_after = session
            .bootstrap_fresh_client(&hello)
            .expect("hello after commit");
        assert_eq!(bootstrap_after.welcome.initial_revision, 1);
        let snapshot = bootstrap_after.snapshot.expect("hello catch-up snapshot");
        assert_eq!(snapshot.base_revision, 0);
        assert_eq!(snapshot.new_revision, 1);

        let resume = ClientResume {
            session_id: "test-session".to_string(),
            client_instance_id: vec![1, 2],
            last_applied_revision: 0,
            last_acked_event_seq: 0,
            terminal_stream_offsets: Default::default(),
        };

        match session
            .bootstrap_resume(&resume)
            .expect("resume handled")
            .outcome
        {
            ResumeOutcome::Replay {
                welcome_msg,
                replayed,
            } => {
                assert_eq!(welcome_msg.replay_from_revision, 0);
                assert_eq!(welcome_msg.last_processed_event_seq, 0);
                assert_eq!(replayed.len(), 1);
                assert_eq!(replayed[0].new_revision, 1);
            }
            ResumeOutcome::Resync { .. } => panic!("expected replay, got resync"),
        }
    }

    #[test]
    fn test_bootstrap_resume_resync_after_journal_eviction() {
        let session = Session::new("resync-test");

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

        match session
            .bootstrap_resume(&resume)
            .expect("resume handled")
            .outcome
        {
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

    #[test]
    fn test_bootstrap_fresh_client() {
        let session = Session::new("test-bootstrap-session");
        let hello = sample_hello();

        let mut bootstrap0 = session
            .bootstrap_fresh_client(&hello)
            .expect("bootstrap rev0");
        assert_eq!(bootstrap0.welcome.session_id, "test-bootstrap-session");
        assert_eq!(bootstrap0.welcome.initial_revision, 0);
        assert!(bootstrap0.snapshot.is_none());

        let tx = Transaction {
            base_revision: 0,
            new_revision: 1,
            priority: 1,
            operations: vec![],
        };
        session.commit_transaction(tx).expect("commit tx");

        let rec_tx = bootstrap0
            .transactions
            .try_recv()
            .expect("receive broadcast");
        assert_eq!(rec_tx.new_revision, 1);

        let bootstrap1 = session
            .bootstrap_fresh_client(&hello)
            .expect("bootstrap rev1");
        assert_eq!(bootstrap1.welcome.initial_revision, 1);
        let snapshot = bootstrap1.snapshot.expect("snapshot for rev1");
        assert_eq!(snapshot.base_revision, 0);
        assert_eq!(snapshot.new_revision, 1);
    }

    fn seed_revision_one(session: &Session) {
        session
            .commit_transaction(Transaction {
                base_revision: 0,
                new_revision: 1,
                priority: 1,
                operations: vec![],
            })
            .expect("seed revision");
    }

    #[test]
    fn test_bootstrap_fresh_client_holds_catch_up_until_subscription() {
        let session = Arc::new(Session::new("test-bootstrap-no-gap"));
        seed_revision_one(&session);

        let hello = ClientHello {
            core_version: "0.4.0".to_string(),
            profiles: vec!["org.srui.standard-widgets/1".to_string()],
            limits: None,
            client_instance_id: vec![7, 8],
            client_metadata: Default::default(),
        };

        let snapshot_captured = Arc::new(Barrier::new(2));
        let allow_subscription = Arc::new(Barrier::new(2));
        let bootstrap_thread = {
            let session = Arc::clone(&session);
            let snapshot_captured = Arc::clone(&snapshot_captured);
            let allow_subscription = Arc::clone(&allow_subscription);
            thread::spawn(move || {
                session.bootstrap_fresh_client_with(&hello, || {
                    snapshot_captured.wait();
                    allow_subscription.wait();
                })
            })
        };

        snapshot_captured.wait();
        assert!(
            matches!(session.inner.try_lock(), Err(TryLockError::WouldBlock)),
            "bootstrap must retain the authoritative state lock after catch-up capture"
        );

        let commit_started = Arc::new(Barrier::new(2));
        let commit_thread = {
            let session = Arc::clone(&session);
            let commit_started = Arc::clone(&commit_started);
            thread::spawn(move || {
                assert!(
                    matches!(session.inner.try_lock(), Err(TryLockError::WouldBlock)),
                    "the competing commit must observe bootstrap holding the state lock"
                );
                commit_started.wait();
                session.commit_transaction(Transaction {
                    base_revision: 1,
                    new_revision: 2,
                    priority: 1,
                    operations: vec![],
                })
            })
        };

        commit_started.wait();
        allow_subscription.wait();

        let mut bootstrap = bootstrap_thread
            .join()
            .expect("bootstrap thread")
            .expect("bootstrap succeeds");
        commit_thread
            .join()
            .expect("commit thread")
            .expect("commit succeeds");

        let snapshot = bootstrap.snapshot.expect("revision-one snapshot");
        assert_eq!(snapshot.new_revision, 1);

        let streamed = bootstrap
            .transactions
            .try_recv()
            .expect("post-snapshot transaction");
        assert_eq!(streamed.base_revision, snapshot.new_revision);
        assert_eq!(streamed.new_revision, snapshot.new_revision + 1);
    }

    #[test]
    fn test_bootstrap_resume_holds_catch_up_until_subscription() {
        let session = Arc::new(Session::new("test-resume-no-gap"));
        seed_revision_one(&session);

        let resume = ClientResume {
            session_id: "test-resume-no-gap".to_string(),
            client_instance_id: vec![7, 8],
            last_applied_revision: 0,
            last_acked_event_seq: 0,
            terminal_stream_offsets: Default::default(),
        };

        let catch_up_captured = Arc::new(Barrier::new(2));
        let allow_subscription = Arc::new(Barrier::new(2));
        let bootstrap_thread = {
            let session = Arc::clone(&session);
            let catch_up_captured = Arc::clone(&catch_up_captured);
            let allow_subscription = Arc::clone(&allow_subscription);
            thread::spawn(move || {
                session.bootstrap_resume_with(&resume, || {
                    catch_up_captured.wait();
                    allow_subscription.wait();
                })
            })
        };

        catch_up_captured.wait();
        assert!(
            matches!(session.inner.try_lock(), Err(TryLockError::WouldBlock)),
            "resume bootstrap must retain the authoritative state lock after catch-up capture"
        );

        let commit_started = Arc::new(Barrier::new(2));
        let commit_thread = {
            let session = Arc::clone(&session);
            let commit_started = Arc::clone(&commit_started);
            thread::spawn(move || {
                assert!(
                    matches!(session.inner.try_lock(), Err(TryLockError::WouldBlock)),
                    "the competing commit must observe resume bootstrap holding the state lock"
                );
                commit_started.wait();
                session.commit_transaction(Transaction {
                    base_revision: 1,
                    new_revision: 2,
                    priority: 1,
                    operations: vec![],
                })
            })
        };

        commit_started.wait();
        allow_subscription.wait();

        let mut bootstrap = bootstrap_thread
            .join()
            .expect("bootstrap thread")
            .expect("bootstrap succeeds");
        commit_thread
            .join()
            .expect("commit thread")
            .expect("commit succeeds");

        match bootstrap.outcome {
            ResumeOutcome::Replay { replayed, .. } => {
                assert_eq!(replayed.len(), 1);
                assert_eq!(replayed[0].new_revision, 1);
            }
            ResumeOutcome::Resync { .. } => panic!("expected replay, got resync"),
        }

        let streamed = bootstrap
            .transactions
            .try_recv()
            .expect("post-replay transaction");
        assert_eq!(streamed.base_revision, 1);
        assert_eq!(streamed.new_revision, 2);
    }
}
