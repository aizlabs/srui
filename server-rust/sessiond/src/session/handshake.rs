//! Atomic client attach: catch-up plus bounded outbound subscribe under one lock section
//! (§15, §18, §18.1, §20.2, §21).
//!
//! # Lock Order
//!
//! Bootstrap holds the [`SessionInner`](super::SessionInner) lock across
//! [`OutboundHub::subscribe`](crate::outbound::OutboundHub::subscribe) and
//! `OutboundHub::is_client_stale`, so the session lock is always the outermost of the two; the hub
//! itself nests `subscribers -> SubscriberState -> stale_clients`. Nothing may take a hub lock
//! before the session lock, or the two orders deadlock.

use crate::outbound::OutboundReceiver;
use srui_protocol::{
    ClientHello, ClientLimits, ClientResume, ExtensionNamespaceMapping, ServerResumeOk,
    ServerResyncRequired, ServerWelcome, SessionContinuity, Transaction,
};
use srui_semantic_tree::{CapabilitySet, Profile, SemanticStore};

use super::snapshot::export_snapshot_transaction;
use super::{Session, SessionError};

/// Core protocol version this build speaks (§15).
pub const CORE_VERSION: &str = "0.4.0";

/// Whether `requested` names a core version this build can serve (§15, §4 inv. 13).
///
/// Compatibility is decided on `major.minor`; the patch level is free. An absent field decodes to
/// the proto3 default `""`, which is indistinguishable from "omitted" on the wire, so it is
/// refused rather than treated as "unspecified, therefore fine": a default must never be the thing
/// that authorizes a session.
fn core_version_is_compatible(requested: &str) -> bool {
    fn major_minor(version: &str) -> Option<(&str, &str)> {
        let mut parts = version.split('.');
        let major = parts.next()?;
        let minor = parts.next()?;
        let numeric = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        (numeric(major) && numeric(minor)).then_some((major, minor))
    }

    match (major_minor(requested), major_minor(CORE_VERSION)) {
        (Some(requested), Some(supported)) => requested == supported,
        _ => false,
    }
}

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
    // §15: `core_version` is part of the handshake, not decoration. Accepting an unknown core
    // version would let two peers that disagree about required semantics reach the data plane
    // (§4 inv. 13).
    if !core_version_is_compatible(&hello.core_version) {
        return Err(SessionError::UnsupportedCoreVersion {
            requested: hello.core_version.clone(),
            supported: CORE_VERSION.to_string(),
        });
    }

    let mut client_caps = CapabilitySet::new();
    for p_str in &hello.profiles {
        if let Ok(p) = Profile::parse(p_str) {
            client_caps.insert(p);
        }
    }

    let _negotiated = inner.capabilities.negotiate(&client_caps)?;

    let initial_revision = inner.store.revision().get();
    // Advertise the effective (server ∩ client) resource ceiling so peers agree on §15/§26 limits.
    let mut limits = inner.limits;
    limits.max_resource_size =
        negotiated_max_resource_size(inner.limits.max_resource_size, hello.limits.as_ref()) as u32;
    let welcome = ServerWelcome {
        core_version: CORE_VERSION.to_string(),
        required_profiles: inner.capabilities.required.to_string_vec(),
        optional_profiles: inner.capabilities.optional.to_string_vec(),
        session_id: inner.session_id.clone(),
        initial_revision,
        extension_namespaces: vec![ExtensionNamespaceMapping {
            extension_uri: "org.srui.standard-widgets".to_string(),
            namespace_id: 0,
        }],
        limits: Some(limits),
    };

    let store_clone = if initial_revision > 0 {
        Some(inner.store.clone_staging())
    } else {
        None
    };

    Ok((welcome, store_clone))
}

/// Intersects the client's advertised `max_resource_size` with the server ceiling (§15, §26).
///
/// A missing or zero client value means "no client preference" and keeps the server default.
fn negotiated_max_resource_size(server: u32, client_limits: Option<&ClientLimits>) -> u64 {
    let server = u64::from(server);
    let client = client_limits
        .map(|limits| u64::from(limits.max_resource_size))
        .unwrap_or(0);
    if client == 0 {
        server
    } else {
        server.min(client)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResyncCause {
    ReplacedIncarnation,
    OutboundQueueOverflow,
    JournalGap,
}

impl ResyncCause {
    fn continuity(self) -> SessionContinuity {
        match self {
            Self::ReplacedIncarnation => SessionContinuity::Replaced,
            Self::OutboundQueueOverflow | Self::JournalGap => SessionContinuity::SameSession,
        }
    }

    fn reason(self) -> &'static str {
        match self {
            Self::ReplacedIncarnation => "requested session incarnation is no longer available",
            Self::OutboundQueueOverflow => {
                "outbound transaction queue overflowed; full state resynchronization required (§20.2)"
            }
            Self::JournalGap => "client revision outside retained journal window",
        }
    }
}

impl Session {
    /// Atomically prepares a fresh client handshake and subscribes it to transactions (§15, §18, §20.2).
    ///
    /// Evaluates `ClientHello`, negotiates capabilities, constructs `ServerWelcome`, clones
    /// authoritative state when `initial_revision > 0`, and subscribes to outbound hub while
    /// holding the session lock so no concurrent transaction commit can be missed between catch-up
    /// clone and live distribution.
    ///
    /// # Revision-Zero Omission
    /// When `initial_revision == 0`, `snapshot` is `None` because an empty `0 -> 0` snapshot violates normal
    /// transaction invariants.
    ///
    /// # Errors
    /// Returns [`SessionError::Negotiation`] if capability negotiation fails against client profiles.
    /// Returns [`SessionError::LockPoisoned`] if an internal mutex is poisoned.
    /// Returns [`SessionError::OutboundClosed`] if the outbound hub has been closed.
    pub fn bootstrap_fresh_client(
        &self,
        hello: &ClientHello,
    ) -> Result<FreshClientBootstrap, SessionError> {
        self.bootstrap_fresh_client_with(hello, || {})
    }

    /// [`Self::bootstrap_fresh_client`] with a callback run immediately before subscription while
    /// the session lock is held, used by tests to interleave a commit with the catch-up clone.
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
        let max_frame_size = inner_guard.limits.max_frame_size as usize;
        let max_resource_size = negotiated_max_resource_size(
            inner_guard.limits.max_resource_size,
            hello.limits.as_ref(),
        );
        let retained_resources = inner_guard.resources.retained_entries();
        let transactions = self.outbound_hub.subscribe(
            hello.client_instance_id.clone(),
            self.outbound_queue_capacity,
            max_ops,
            max_frame_size,
            max_resource_size,
        )?;
        // Seed while still holding SessionInner so a resource published between snapshot
        // creation and live subscription cannot be missed (SessionInner -> OutboundHub order).
        // Task 26 re-sends the retained CAS on every attach (no client known-hash protocol yet).
        self.outbound_hub
            .seed_resources(&transactions, &retained_resources);
        drop(inner_guard);

        // Fails the handshake rather than emitting a catch-up transaction the client must reject
        // and would then re-request forever (§18, §26).
        let snapshot = match store_clone {
            Some(store) => Some(export_snapshot_transaction(&store).inspect_err(|error| {
                tracing::error!(%error, "refusing to send an unrepresentable catch-up snapshot");
            })?),
            None => None,
        };

        Ok(FreshClientBootstrap {
            welcome,
            snapshot,
            transactions,
        })
    }

    /// Atomically prepares a client resume and subscribes it to transactions (§18, §18.1, §20.2).
    ///
    /// Replaced session incarnations are evaluated first (§18.1). If the session incarnation matches
    /// but the client detached due to outbound queue overflow (§20.2), full state resync is forced.
    /// If neither applies and `last_applied_revision` is within the retained journal window,
    /// a replay is prepared; otherwise, same-session snapshot resync is returned.
    pub fn bootstrap_resume(
        &self,
        resume: &ClientResume,
    ) -> Result<ResumeClientBootstrap, SessionError> {
        self.bootstrap_resume_with(resume, || {})
    }

    /// [`Self::bootstrap_resume`] with a callback run immediately before subscription while the
    /// session lock is held, used by tests to interleave a commit with the replay collection.
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
                reason: String,
                last_processed_event_seq: u64,
            },
        }

        let inner_guard = self.inner.lock().map_err(|_| SessionError::LockPoisoned)?;
        let last_processed_event_seq = inner_guard
            .dedupe
            .last_contiguous_processed_seq(&resume.client_instance_id);

        // Evaluate cause: replaced incarnation takes precedence, then outbound overflow, then journal gap
        let resync_cause = if resume.session_id != inner_guard.session_id {
            Some(ResyncCause::ReplacedIncarnation)
        } else if self
            .outbound_hub
            .is_client_stale(&resume.client_instance_id)
        {
            Some(ResyncCause::OutboundQueueOverflow)
        } else if inner_guard
            .journal
            .iter_from(resume.last_applied_revision)
            .is_none()
        {
            Some(ResyncCause::JournalGap)
        } else {
            None
        };

        let plan = if let Some(cause) = resync_cause {
            ResumePlan::Resync {
                session_id: inner_guard.session_id.clone(),
                snapshot_revision: inner_guard.store.revision().get(),
                store_snapshot: inner_guard.store.clone_staging(),
                continuity: cause.continuity(),
                reason: cause.reason().to_string(),
                last_processed_event_seq,
            }
        } else {
            let iter = inner_guard
                .journal
                .iter_from(resume.last_applied_revision)
                .unwrap();
            ResumePlan::Replay {
                welcome_msg: ServerResumeOk {
                    session_id: inner_guard.session_id.clone(),
                    replay_from_revision: resume.last_applied_revision,
                    last_processed_event_seq,
                },
                replayed: iter.cloned().collect(),
            }
        };

        before_subscribe();

        let max_ops = inner_guard.limits.max_transaction_operations as usize;
        let max_frame_size = inner_guard.limits.max_frame_size as usize;
        // ClientResume carries no limits; use the server ceiling until a known-hash / renegotiate
        // path exists (Task 26).
        let max_resource_size = u64::from(inner_guard.limits.max_resource_size);
        let retained_resources = inner_guard.resources.retained_entries();
        let transactions = self.outbound_hub.subscribe(
            resume.client_instance_id.clone(),
            self.outbound_queue_capacity,
            max_ops,
            max_frame_size,
            max_resource_size,
        )?;
        // Re-seed retained resources on resume; clients should share one ResourceCache across
        // controller generations so duplicate transfers stay cheap (§14, §18).
        self.outbound_hub
            .seed_resources(&transactions, &retained_resources);
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
                reason,
                last_processed_event_seq,
            } => ResumeOutcome::Resync {
                resync_msg: ServerResyncRequired {
                    session_id,
                    snapshot_revision,
                    reason,
                    continuity: continuity as i32,
                    last_processed_event_seq,
                },
                // A resync the client cannot decode is worse than a refused resume: it strands the
                // client awaiting a snapshot that every retry reproduces byte-for-byte (§18, §26).
                snapshot_transaction: export_snapshot_transaction(&store_snapshot).inspect_err(
                    |error| {
                        tracing::error!(%error, "refusing to send an unrepresentable resync snapshot");
                    },
                )?,
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

    fn empty_tx(base: u64, new_rev: u64) -> Transaction {
        Transaction {
            base_revision: base,
            new_revision: new_rev,
            priority: 1,
            operations: vec![],
        }
    }

    #[test]
    fn test_replaced_incarnation_precedes_outbound_overflow_stale() {
        let session = Session::with_outbound_queue_capacity("live-incarnation", 1);
        let client = vec![9, 9];
        let _rx = session
            .subscribe_transactions(client.clone())
            .expect("subscribe");

        session
            .commit_transaction(empty_tx(0, 1))
            .expect("fill queue");
        session
            .commit_transaction(empty_tx(1, 2))
            .expect("overflow");
        assert!(session.outbound_hub().is_client_stale(&client));

        let resume = ClientResume {
            session_id: "old-incarnation".to_string(),
            client_instance_id: client,
            last_applied_revision: 0,
            last_acked_event_seq: 0,
            terminal_stream_offsets: Default::default(),
        };
        let bootstrap = session.bootstrap_resume(&resume).expect("resume");
        match bootstrap.outcome {
            ResumeOutcome::Resync { resync_msg, .. } => {
                assert_eq!(
                    SessionContinuity::try_from(resync_msg.continuity),
                    Ok(SessionContinuity::Replaced)
                );
                assert!(resync_msg.reason.contains("incarnation"));
            }
            other => panic!("expected Replaced resync, got {other:?}"),
        }
    }

    #[test]
    fn test_overflow_stale_survives_resume_bootstrap_until_snapshot_ack() {
        let session = Session::with_outbound_queue_capacity("overflow-session", 1);
        let client = vec![4, 2];
        let _rx = session
            .subscribe_transactions(client.clone())
            .expect("subscribe");

        session
            .commit_transaction(empty_tx(0, 1))
            .expect("fill queue");
        session
            .commit_transaction(empty_tx(1, 2))
            .expect("overflow");
        assert!(session.outbound_hub().is_client_stale(&client));

        let resume = ClientResume {
            session_id: session.session_id(),
            client_instance_id: client.clone(),
            last_applied_revision: 0,
            last_acked_event_seq: 0,
            terminal_stream_offsets: Default::default(),
        };
        let bootstrap = session.bootstrap_resume(&resume).expect("resume");
        match bootstrap.outcome {
            ResumeOutcome::Resync { resync_msg, .. } => {
                assert_eq!(
                    SessionContinuity::try_from(resync_msg.continuity),
                    Ok(SessionContinuity::SameSession)
                );
                assert!(resync_msg
                    .reason
                    .contains("outbound transaction queue overflowed"));
            }
            other => panic!("expected overflow resync, got {other:?}"),
        }
        assert!(
            session.outbound_hub().is_client_stale(&client),
            "stale must remain until the snapshot write completes"
        );
        session.clear_stale_client(&client);
        assert!(!session.outbound_hub().is_client_stale(&client));
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
            .expect("receive broadcast")
            .unwrap()
            .into_transaction()
            .expect("transaction");
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
            .expect("post-snapshot transaction")
            .unwrap()
            .into_transaction()
            .expect("transaction");
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
            .expect("post-replay transaction")
            .unwrap()
            .into_transaction()
            .expect("transaction");
        assert_eq!(streamed.base_revision, 1);
        assert_eq!(streamed.new_revision, 2);
    }
}
