//! Resync end-to-end tests (§18, §20.2).
//!
//! Verifies journal-window eviction triggers `ServerResyncRequired` plus a snapshot transaction
//! that reconstructs the authoritative tree, collection models, and root ordering.

use std::sync::Arc;

use futures::{SinkExt, StreamExt};
use tokio::io::duplex;
use tokio_util::codec::{FramedRead, FramedWrite};
use tokio_util::sync::CancellationToken;

use srui_protocol::{
    srui_message, ClientHello, ClientResume, SessionContinuity, SruiCodec, SruiMessage,
    Transaction as WireTransaction,
};
use srui_sdk::*;
use srui_semantic_tree::{
    ItemId, ModelId, ModelItem, NodeId, Operation, ResyncSnapshot, Revision, SemanticStore,
    StoreLimits, TxnError, TypeRef, Value, DEFAULT_MAX_ITEMS_PER_MODEL_OPERATION,
};
use srui_sessiond::{handle_connection, ResumeOutcome, Session};

const JOURNAL_CAPACITY: u64 = 1024;

fn seed_session_with_tree_and_models(session: &Session) -> u64 {
    let model_id = ModelId::new(10);
    let item_a = ItemId::new(100);
    let item_b = ItemId::new(101);

    session
        .transaction(|ui| {
            ui.apply_op(&Operation::create_model(model_id, TypeRef::LIST, 1_000))?;
            ui.apply_op(&Operation::model_reset_range(
                model_id,
                0,
                [
                    ModelItem::with_value(item_a, Value::String("alpha".into())),
                    ModelItem::with_value(item_b, Value::String("beta".into())),
                ],
                None,
            ))?;

            Surface::builder(1).label("Second Root").create(ui)?;
            Surface::builder(2).label("First Root").create(ui)?;

            List::builder(3)
                .parent(2)
                .model_ref(model_id)
                .label("Services")
                .create(ui)?;

            Ok(())
        })
        .expect("seed tree and models");

    session.current_revision()
}

fn evict_journal_window(session: &Session, from_revision: u64) {
    let mut rev = from_revision;
    while rev < from_revision + JOURNAL_CAPACITY {
        let tx = WireTransaction {
            base_revision: rev,
            new_revision: rev + 1,
            priority: 0,
            operations: vec![],
        };
        session.commit_transaction(tx).expect("advance revision");
        rev += 1;
    }
}

/// Applies a resync snapshot the way a replica does: from explicit protocol context, through the
/// dedicated §18 entry point rather than the live-stream path (§12.1, §18).
fn apply_resync_snapshot(wire_tx: WireTransaction) -> Result<SemanticStore, TxnError> {
    assert_eq!(
        wire_tx.base_revision,
        Revision::INITIAL.get(),
        "snapshot must start from revision 0"
    );

    let snapshot = ResyncSnapshot::try_from(wire_tx)?;
    let mut replica =
        SemanticStore::with_limits_and_revision(StoreLimits::default(), Revision::INITIAL);
    replica.replace_from_snapshot(&snapshot)?;
    Ok(replica)
}

fn assert_stores_equivalent(expected: &SemanticStore, actual: &SemanticStore) {
    assert_eq!(expected.revision(), actual.revision());
    assert_eq!(expected.root_ids(), actual.root_ids());
    assert_eq!(expected.node_count(), actual.node_count());
    assert_eq!(expected.model_count(), actual.model_count());

    for &root in expected.root_ids() {
        assert_node_subtree_equivalent(expected, actual, root);
    }

    for model_id in expected.model_ids() {
        let expected_model = expected.get_model(model_id).expect("expected model");
        let actual_model = actual.get_model(model_id).expect("actual model");
        assert_eq!(expected_model.model_type, actual_model.model_type);
        assert_eq!(expected_model.item_count, actual_model.item_count);
        assert_eq!(
            expected_model.cached_item_count(),
            actual_model.cached_item_count()
        );
        for (idx, item) in expected_model.iter_cached_items() {
            assert_eq!(actual_model.get_item_by_index(*idx), Some(item));
        }
    }
}

fn assert_node_subtree_equivalent(
    expected: &SemanticStore,
    actual: &SemanticStore,
    node_id: NodeId,
) {
    let expected_node = expected.get_node(node_id).expect("expected node");
    let actual_node = actual.get_node(node_id).expect("actual node");
    assert_eq!(expected_node.node_type, actual_node.node_type);
    assert_eq!(expected_node.parent_id, actual_node.parent_id);
    assert_eq!(expected_node.ordered_children, actual_node.ordered_children);
    assert_eq!(expected_node.properties, actual_node.properties);

    for &child in &expected_node.ordered_children {
        assert_node_subtree_equivalent(expected, actual, child);
    }
}

#[test]
fn test_handle_resume_resync_snapshot_reconstructs_tree_and_models() {
    let session = Session::new("resync-snapshot-test");
    let seeded_revision = seed_session_with_tree_and_models(&session);
    evict_journal_window(&session, seeded_revision);

    let expected_revision = seeded_revision + JOURNAL_CAPACITY;
    assert_eq!(session.current_revision(), expected_revision);

    session.with_store(|store| {
        assert_eq!(store.root_ids(), &[NodeId::new(1), NodeId::new(2)]);
        assert_eq!(store.model_count(), 1);
    });

    let resume = srui_protocol::ClientResume {
        session_id: "resync-snapshot-test".to_string(),
        client_instance_id: vec![7],
        last_applied_revision: 0,
        last_acked_event_seq: 0,
        terminal_stream_offsets: Default::default(),
        limits: None,
        known_resource_hashes: vec![],
        pending_text_edits: vec![],
    };

    let (resync_msg, snapshot_tx) = match session
        .bootstrap_resume(&resume)
        .expect("resume handled")
        .outcome
    {
        ResumeOutcome::Resync {
            resync_msg,
            snapshot_transaction,
        } => (resync_msg, snapshot_transaction),
        ResumeOutcome::Replay { .. } => panic!("expected resync, got replay"),
    };

    assert_eq!(resync_msg.session_id, "resync-snapshot-test");
    assert_eq!(resync_msg.snapshot_revision, expected_revision);
    assert_eq!(
        SessionContinuity::try_from(resync_msg.continuity),
        Ok(SessionContinuity::SameSession)
    );
    assert_eq!(resync_msg.last_processed_event_seq, 0);
    assert_eq!(snapshot_tx.base_revision, 0);
    assert_eq!(snapshot_tx.new_revision, expected_revision);
    assert!(
        snapshot_tx
            .operations
            .iter()
            .any(|op| matches!(op.op, Some(srui_protocol::operation::Op::CreateModel(_)))),
        "snapshot must export collection models"
    );

    let replayed = apply_resync_snapshot(snapshot_tx).expect("apply resync snapshot");
    session.with_store(|store| assert_stores_equivalent(store, &replayed));
}

/// §26: a model may cache far more items than a single model operation may carry, so the snapshot
/// exporter must chunk a cached range at `max_items_per_model_operation`. An unchunked range makes
/// the snapshot undecodable for every conforming client, which then waits forever for a snapshot it
/// will reject again (§18).
#[test]
fn test_resync_snapshot_chunks_cached_ranges_within_item_limit() {
    const TOTAL_ITEMS: u64 = 25_000;
    let seed_chunk = DEFAULT_MAX_ITEMS_PER_MODEL_OPERATION as u64;

    let session = Session::new("resync-chunked-model");
    let model_id = ModelId::new(20);
    session
        .transaction(|ui| {
            ui.apply_op(&Operation::create_model(
                model_id,
                TypeRef::LIST,
                TOTAL_ITEMS,
            ))?;
            let mut start = 0u64;
            while start < TOTAL_ITEMS {
                let end = (start + seed_chunk).min(TOTAL_ITEMS);
                let items: Vec<ModelItem> = (start..end)
                    .map(|idx| ModelItem::with_value(ItemId::new(idx + 1), Value::UnsignedInt(idx)))
                    .collect();
                ui.apply_op(&Operation::model_reset_range(model_id, start, items, None))?;
                start = end;
            }

            Surface::builder(1).label("Root").create(ui)?;
            List::builder(2).parent(1).model_ref(model_id).create(ui)?;
            Ok(())
        })
        .expect("seed model larger than one model operation");

    // The seeding ops are adjacent, so the store holds a single 25_000-item cached range.
    session.with_store(|store| {
        let model = store.get_model(model_id).expect("seeded model");
        assert_eq!(model.cached_ranges().len(), 1);
        assert_eq!(model.cached_item_count(), TOTAL_ITEMS as usize);
    });

    let seeded_revision = session.current_revision();
    evict_journal_window(&session, seeded_revision);

    let resume = srui_protocol::ClientResume {
        session_id: "resync-chunked-model".to_string(),
        client_instance_id: vec![9],
        last_applied_revision: 0,
        last_acked_event_seq: 0,
        terminal_stream_offsets: Default::default(),
        limits: None,
        known_resource_hashes: vec![],
        pending_text_edits: vec![],
    };
    let snapshot_tx = match session
        .bootstrap_resume(&resume)
        .expect("resume handled")
        .outcome
    {
        ResumeOutcome::Resync {
            snapshot_transaction,
            ..
        } => snapshot_transaction,
        ResumeOutcome::Replay { .. } => panic!("expected resync, got replay"),
    };

    let mut exported_items = 0usize;
    let mut reset_ops = 0usize;
    for op in &snapshot_tx.operations {
        if let Some(srui_protocol::operation::Op::ModelResetRange(reset)) = &op.op {
            reset_ops += 1;
            exported_items += reset.items.len();
            assert!(
                reset.items.len() <= DEFAULT_MAX_ITEMS_PER_MODEL_OPERATION,
                "MODEL_RESET_RANGE carries {} items, over the §26 per-operation limit of {}",
                reset.items.len(),
                DEFAULT_MAX_ITEMS_PER_MODEL_OPERATION
            );
        }
    }
    assert_eq!(reset_ops, 3);
    assert_eq!(exported_items, TOTAL_ITEMS as usize);

    // The decisive assertion: a client enforcing the default §26 limits can apply the snapshot.
    let replayed = apply_resync_snapshot(snapshot_tx).expect("apply chunked resync snapshot");
    session.with_store(|store| assert_stores_equivalent(store, &replayed));
}

#[tokio::test]
async fn test_connection_resync_delivers_snapshot_matching_authoritative_store() {
    let session = Arc::new(Session::new("resync-e2e-session"));
    let seeded_revision = seed_session_with_tree_and_models(&session);
    evict_journal_window(&session, seeded_revision);
    let expected_revision = seeded_revision + JOURNAL_CAPACITY;

    let shutdown = CancellationToken::new();
    let (client_io, server_io) = duplex(1024 * 1024);

    let session_clone = session.clone();
    let shutdown_clone = shutdown.clone();
    let server_task =
        tokio::spawn(
            async move { handle_connection(server_io, session_clone, shutdown_clone).await },
        );

    let (client_read, client_write) = tokio::io::split(client_io);
    let mut framed_read = FramedRead::new(client_read, SruiCodec::new());
    let mut framed_write = FramedWrite::new(client_write, SruiCodec::new());

    let resume = SruiMessage {
        msg: Some(srui_message::Msg::ClientResume(ClientResume {
            session_id: "resync-e2e-session".to_string(),
            client_instance_id: vec![42],
            last_applied_revision: 0,
            last_acked_event_seq: 0,
            terminal_stream_offsets: Default::default(),
            limits: None,
            known_resource_hashes: vec![],
            pending_text_edits: vec![],
        })),
    };
    framed_write.send(resume).await.expect("send client resume");

    let resync_envelope = framed_read
        .next()
        .await
        .expect("receive resync required")
        .expect("decode resync required");
    let resync = match resync_envelope.msg {
        Some(srui_message::Msg::ServerResyncRequired(msg)) => msg,
        other => panic!("expected ServerResyncRequired, got {:?}", other),
    };
    assert_eq!(resync.session_id, "resync-e2e-session");
    assert_eq!(resync.snapshot_revision, expected_revision);

    let snapshot_envelope = framed_read
        .next()
        .await
        .expect("receive snapshot transaction")
        .expect("decode snapshot transaction");
    let snapshot_tx = match snapshot_envelope.msg {
        Some(srui_message::Msg::Transaction(tx)) => tx,
        other => panic!("expected snapshot Transaction, got {:?}", other),
    };

    assert_eq!(snapshot_tx.base_revision, 0);
    assert_eq!(snapshot_tx.new_revision, expected_revision);

    let replayed = apply_resync_snapshot(snapshot_tx).expect("apply resync snapshot");
    session.with_store(|store| {
        assert_stores_equivalent(store, &replayed);
        assert_eq!(store.root_ids(), &[NodeId::new(1), NodeId::new(2)]);
        let list = List::from_store(store, NodeId::new(3)).expect("list node");
        assert_eq!(list.model_ref(store), Some(ModelId::new(10)));
        assert_eq!(list.label(store), Some("Services"));
        let model = store.get_model(ModelId::new(10)).expect("list model");
        assert_eq!(
            model.get_item_by_index(0).unwrap().value,
            Value::String("alpha".into())
        );
        assert_eq!(
            model.get_item_by_index(1).unwrap().value,
            Value::String("beta".into())
        );
    });

    shutdown.cancel();
    drop(framed_write);
    drop(framed_read);
    let _ = server_task.await;
}

#[tokio::test]
async fn test_fresh_client_bootstrap_populated_session_and_immediate_commit() {
    let session = Session::new("bootstrap-populated-test");
    let seeded_revision = seed_session_with_tree_and_models(&session);
    assert!(seeded_revision > 0);

    let hello = ClientHello {
        core_version: "0.5.0".to_string(),
        profiles: vec!["org.srui.standard-widgets/1".to_string()],
        limits: None,
        client_instance_id: vec![1, 2, 3],
        client_metadata: Default::default(),
        known_resource_hashes: vec![],
    };

    let mut bootstrap = session
        .bootstrap_fresh_client(&hello)
        .expect("bootstrap succeeds");
    assert_eq!(bootstrap.welcome.session_id, "bootstrap-populated-test");
    assert_eq!(bootstrap.welcome.initial_revision, seeded_revision);

    let snapshot = bootstrap
        .snapshot
        .expect("snapshot must be present for nonzero revision");
    assert_eq!(bootstrap.welcome.initial_revision, snapshot.new_revision);
    assert_eq!(snapshot.base_revision, 0);
    assert_eq!(snapshot.new_revision, seeded_revision);

    // Verify snapshot operation presence for both nodes and models
    let has_create_model = snapshot
        .operations
        .iter()
        .any(|op| matches!(op.op, Some(srui_protocol::operation::Op::CreateModel(_))));
    let has_model_items = snapshot.operations.iter().any(|op| {
        matches!(
            op.op,
            Some(srui_protocol::operation::Op::ModelResetRange(_))
        )
    });
    let has_create_node = snapshot
        .operations
        .iter()
        .any(|op| matches!(op.op, Some(srui_protocol::operation::Op::CreateNode(_))));
    assert!(has_create_model, "snapshot must contain CreateModel ops");
    assert!(has_model_items, "snapshot must contain ModelResetRange ops");
    assert!(has_create_node, "snapshot must contain CreateNode ops");

    // Applying the snapshot to a fresh SemanticStore produces a store equivalent to the authoritative store
    let replayed = apply_resync_snapshot(snapshot.clone()).expect("apply snapshot to fresh store");
    session.with_store(|store| assert_stores_equivalent(store, &replayed));

    // A commit immediately after bootstrap is received from bootstrap.transactions with base_revision == snapshot.new_revision
    let next_tx = WireTransaction {
        base_revision: seeded_revision,
        new_revision: seeded_revision + 1,
        priority: 1,
        operations: vec![],
    };
    session
        .commit_transaction(next_tx)
        .expect("commit next transaction");

    let received = bootstrap
        .transactions
        .recv_class(srui_sessiond::LogicalChannelClass::Ui)
        .await
        .expect("receive transaction")
        .into_transaction()
        .expect("transaction");
    assert_eq!(received.base_revision, snapshot.new_revision);
    assert_eq!(received.new_revision, snapshot.new_revision + 1);
}

#[tokio::test]
async fn test_fresh_client_bootstrap_revision_zero_captures_first_transaction() {
    let session = Session::new("bootstrap-rev0-test");
    assert_eq!(session.current_revision(), 0);

    let hello = ClientHello {
        core_version: "0.5.0".to_string(),
        profiles: vec!["org.srui.standard-widgets/1".to_string()],
        limits: None,
        client_instance_id: vec![4, 5, 6],
        client_metadata: Default::default(),
        known_resource_hashes: vec![],
    };

    let mut bootstrap = session
        .bootstrap_fresh_client(&hello)
        .expect("bootstrap rev0");
    assert_eq!(bootstrap.welcome.initial_revision, 0);
    assert!(
        bootstrap.snapshot.is_none(),
        "revision-zero bootstrap must omit snapshot"
    );

    // Receiver captures the first 0 -> 1 transaction
    let first_tx = WireTransaction {
        base_revision: 0,
        new_revision: 1,
        priority: 1,
        operations: vec![],
    };
    session
        .commit_transaction(first_tx)
        .expect("commit first tx");

    let received = bootstrap
        .transactions
        .recv_class(srui_sessiond::LogicalChannelClass::Ui)
        .await
        .expect("receive first transaction")
        .into_transaction()
        .expect("transaction");
    assert_eq!(received.base_revision, 0);
    assert_eq!(received.new_revision, 1);
}

/// §26: a catch-up snapshot larger than one transaction can express must fail on the server.
///
/// The store's `max_node_count` is an order of magnitude above `max_transaction_operations`, so an
/// application doing nothing wrong can reach a state whose snapshot every conforming replica must
/// reject at decode. Emitting it anyway strands the client: a reconnect regenerates the identical
/// snapshot, so it fails, resumes, and fails again forever with no diagnosis on either side.
#[tokio::test]
async fn oversized_catch_up_snapshot_fails_the_handshake_instead_of_being_sent() {
    use srui_semantic_tree::DEFAULT_MAX_TRANSACTION_OPERATIONS;
    use srui_sessiond::SessionError;

    let session = Session::new("oversized-snapshot");
    let root = NodeId::new(1);
    session
        .transaction(|ui| Surface::builder(root).label("root").create(ui))
        .expect("root");

    // Built in legal chunks: every individual transaction respects §26, but the resulting store
    // needs more operations than one transaction may carry.
    let target = DEFAULT_MAX_TRANSACTION_OPERATIONS as u64 + 1;
    let mut next_id = 2u64;
    while next_id <= target {
        let chunk_end = (next_id + 999).min(target);
        session
            .transaction(|ui| {
                for id in next_id..=chunk_end {
                    Text::builder(NodeId::new(id))
                        .parent(root)
                        .text("x")
                        .create(ui)?;
                }
                Ok(())
            })
            .expect("chunked create");
        next_id = chunk_end + 1;
    }
    assert!(session.node_count() > DEFAULT_MAX_TRANSACTION_OPERATIONS);

    let hello = ClientHello {
        core_version: srui_sessiond::CORE_VERSION.to_string(),
        profiles: vec!["org.srui.standard-widgets/1".to_string()],
        limits: None,
        client_instance_id: vec![7],
        client_metadata: Default::default(),
        known_resource_hashes: vec![],
    };
    match session.bootstrap_fresh_client(&hello) {
        Err(SessionError::SnapshotUnrepresentable { limit, actual }) => {
            assert_eq!(limit, DEFAULT_MAX_TRANSACTION_OPERATIONS);
            assert!(actual > limit);
        }
        Ok(bootstrap) => panic!(
            "handshake produced a snapshot with {} operations, which every conforming replica \
             must reject (§26)",
            bootstrap
                .snapshot
                .map(|tx| tx.operations.len())
                .unwrap_or_default()
        ),
        Err(other) => panic!("expected SnapshotUnrepresentable, got {other:?}"),
    }

    // The same refusal must apply on the resync path, which is the one a stranded client retries.
    let resume = ClientResume {
        session_id: "a-different-incarnation".to_string(),
        client_instance_id: vec![7],
        last_applied_revision: 0,
        last_acked_event_seq: 0,
        terminal_stream_offsets: Default::default(),
        limits: None,
        known_resource_hashes: vec![],
        pending_text_edits: vec![],
    };
    assert!(matches!(
        session.bootstrap_resume(&resume),
        Err(SessionError::SnapshotUnrepresentable { .. })
    ));
}

#[test]
fn same_session_resync_cancels_declared_text_edits_before_snapshot() {
    use srui_protocol::PendingTextEditRef;
    use srui_semantic_tree::{EditSeq, Event as DomainEvent};
    use srui_sessiond::{EventOutcome, SessionConfig};

    let session = Session::with_config(
        "resync-cancel-text",
        SessionConfig {
            journal_capacity: 1,
            ..SessionConfig::default()
        },
    );
    session
        .transaction(|ui| {
            Surface::builder(1).create(ui)?;
            TextInput::builder(2).parent(1).value("snap").create(ui)?;
            Ok(())
        })
        .expect("seed editor");
    session
        .commit_transaction(WireTransaction {
            base_revision: 1,
            new_revision: 2,
            priority: 0,
            operations: vec![],
        })
        .expect("evict seed from journal");

    let pending = PendingTextEditRef {
        event_id: b"text-pending".to_vec(),
        event_seq: 1,
        node_id: 2,
        edit_seq: 4,
    };
    let resume = ClientResume {
        session_id: "resync-cancel-text".to_string(),
        client_instance_id: b"client-a".to_vec(),
        last_applied_revision: 0,
        last_acked_event_seq: 0,
        terminal_stream_offsets: Default::default(),
        limits: None,
        known_resource_hashes: vec![],
        pending_text_edits: vec![pending.clone()],
    };

    match session.bootstrap_resume(&resume).expect("resume").outcome {
        ResumeOutcome::Resync {
            resync_msg,
            snapshot_transaction,
        } => {
            assert_eq!(
                SessionContinuity::try_from(resync_msg.continuity),
                Ok(SessionContinuity::SameSession)
            );
            assert_eq!(resync_msg.discarded_text_edits, vec![pending.clone()]);
            assert_eq!(resync_msg.last_processed_event_seq, 1);
            assert_eq!(snapshot_transaction.new_revision, 2);
        }
        other => panic!("expected same-session resync, got {other:?}"),
    }

    let replay = DomainEvent::text_edit(1, "text-pending", 0u64, 2, "x", EditSeq::new(4).unwrap())
        .with_client_instance_id(b"client-a".as_slice())
        .to_wire();
    match session.process_event(&replay).expect("replay canceled") {
        EventOutcome::Duplicate {
            accepted: false,
            last_processed_event_seq,
            ..
        } => assert_eq!(last_processed_event_seq, 1),
        other => panic!("canceled TEXT_EDIT must be answered from the result cache, got {other:?}"),
    }
}

#[test]
fn replacement_resync_ignores_pending_text_edit_refs() {
    use srui_protocol::PendingTextEditRef;
    use srui_sessiond::SessionConfig;

    let session = Session::with_config(
        "live-incarnation",
        SessionConfig {
            journal_capacity: 1,
            ..SessionConfig::default()
        },
    );
    session
        .transaction(|ui| {
            Surface::builder(1).create(ui)?;
            Ok(())
        })
        .expect("seed");
    session
        .commit_transaction(WireTransaction {
            base_revision: 1,
            new_revision: 2,
            priority: 0,
            operations: vec![],
        })
        .expect("evict");

    let resume = ClientResume {
        session_id: "expired-incarnation".to_string(),
        client_instance_id: b"client-b".to_vec(),
        last_applied_revision: 0,
        last_acked_event_seq: 0,
        terminal_stream_offsets: Default::default(),
        limits: None,
        known_resource_hashes: vec![],
        pending_text_edits: vec![PendingTextEditRef {
            event_id: b"old-text".to_vec(),
            event_seq: 7,
            node_id: 9,
            edit_seq: 3,
        }],
    };

    match session
        .bootstrap_resume(&resume)
        .expect("replacement")
        .outcome
    {
        ResumeOutcome::Resync { resync_msg, .. } => {
            assert_eq!(
                SessionContinuity::try_from(resync_msg.continuity),
                Ok(SessionContinuity::Replaced)
            );
            assert!(resync_msg.discarded_text_edits.is_empty());
            assert_eq!(resync_msg.last_processed_event_seq, 0);
        }
        other => panic!("expected replacement resync, got {other:?}"),
    }
}

#[test]
fn same_session_resync_does_not_track_canceled_refs_for_missing_nodes() {
    use srui_protocol::PendingTextEditRef;
    use srui_semantic_tree::{EditSeq, Event as DomainEvent};
    use srui_sessiond::{EventOutcome, SessionConfig, MAX_TEXT_EDIT_STREAMS};

    let session = Session::with_config(
        "resync-cancel-missing",
        SessionConfig {
            journal_capacity: 1,
            ..SessionConfig::default()
        },
    );
    session
        .transaction(|ui| {
            Surface::builder(1).create(ui)?;
            TextInput::builder(2).parent(1).value("snap").create(ui)?;
            Ok(())
        })
        .expect("seed editor");
    session
        .commit_transaction(WireTransaction {
            base_revision: 1,
            new_revision: 2,
            priority: 0,
            operations: vec![],
        })
        .expect("evict seed from journal");

    let pending: Vec<PendingTextEditRef> = (0..MAX_TEXT_EDIT_STREAMS)
        .map(|i| PendingTextEditRef {
            event_id: format!("ghost-{i}").into_bytes(),
            event_seq: i as u64 + 1,
            node_id: 1_000 + i as u64,
            edit_seq: 1,
        })
        .collect();
    let resume = ClientResume {
        session_id: "resync-cancel-missing".to_string(),
        client_instance_id: b"client-ghost".to_vec(),
        last_applied_revision: 0,
        last_acked_event_seq: 0,
        terminal_stream_offsets: Default::default(),
        limits: None,
        known_resource_hashes: vec![],
        pending_text_edits: pending.clone(),
    };

    match session.bootstrap_resume(&resume).expect("resume").outcome {
        ResumeOutcome::Resync { resync_msg, .. } => {
            assert_eq!(resync_msg.discarded_text_edits.len(), MAX_TEXT_EDIT_STREAMS);
            assert_eq!(
                resync_msg.last_processed_event_seq,
                MAX_TEXT_EDIT_STREAMS as u64
            );
        }
        other => panic!("expected same-session resync, got {other:?}"),
    }

    let live = DomainEvent::text_edit(
        (MAX_TEXT_EDIT_STREAMS as u64) + 1,
        "live-editor",
        0u64,
        2,
        "ok",
        EditSeq::new(1).unwrap(),
    )
    .with_client_instance_id(b"client-ghost".as_slice())
    .to_wire();
    match session.process_event(&live).expect("live editor") {
        EventOutcome::Processed { .. } => {}
        other => {
            panic!("canceled missing-node refs must not exhaust the text tracker, got {other:?}")
        }
    }
}

#[test]
fn same_session_resync_settles_oversized_pending_event_id_with_bounded_identity() {
    use srui_protocol::PendingTextEditRef;
    use srui_semantic_tree::{EditSeq, Event as DomainEvent, MAX_EVENT_ID_BYTES};
    use srui_sessiond::{EventOutcome, SessionConfig};

    let session = Session::with_config(
        "resync-oversized-event-id",
        SessionConfig {
            journal_capacity: 1,
            ..SessionConfig::default()
        },
    );
    session
        .transaction(|ui| {
            Surface::builder(1).create(ui)?;
            TextInput::builder(2).parent(1).value("snap").create(ui)?;
            Ok(())
        })
        .expect("seed editor");
    session
        .commit_transaction(WireTransaction {
            base_revision: 1,
            new_revision: 2,
            priority: 0,
            operations: vec![],
        })
        .expect("evict seed from journal");

    let oversized_id = vec![0x41; MAX_EVENT_ID_BYTES + 1];
    let resume = ClientResume {
        session_id: "resync-oversized-event-id".to_string(),
        client_instance_id: b"client-oversized".to_vec(),
        last_applied_revision: 0,
        last_acked_event_seq: 0,
        terminal_stream_offsets: Default::default(),
        limits: None,
        known_resource_hashes: vec![],
        pending_text_edits: vec![PendingTextEditRef {
            event_id: oversized_id.clone(),
            event_seq: 1,
            node_id: 2,
            edit_seq: 1,
        }],
    };

    match session.bootstrap_resume(&resume).expect("resume").outcome {
        ResumeOutcome::Resync { resync_msg, .. } => {
            assert_eq!(resync_msg.discarded_text_edits, resume.pending_text_edits);
            assert_eq!(resync_msg.last_processed_event_seq, 1);
        }
        other => panic!("expected same-session resync, got {other:?}"),
    }

    let oversized_replay = DomainEvent::text_edit(
        1,
        "temporary",
        0u64,
        2,
        "discarded",
        EditSeq::new(1).unwrap(),
    )
    .with_client_instance_id(b"client-oversized".as_slice())
    .to_wire();
    let oversized_replay = srui_protocol::Event {
        event_id: oversized_id,
        ..oversized_replay
    };
    assert!(matches!(
        session
            .process_event(&oversized_replay)
            .expect("oversized replay returns settled rejection"),
        EventOutcome::Duplicate {
            accepted: false,
            last_processed_event_seq: 1,
            ..
        }
    ));

    let live = DomainEvent::text_edit(2, "next-valid", 0u64, 2, "ok", EditSeq::new(2).unwrap())
        .with_client_instance_id(b"client-oversized".as_slice())
        .to_wire();
    assert!(matches!(
        session.process_event(&live).expect("next valid event"),
        EventOutcome::Processed {
            last_processed_event_seq: 2,
            ..
        }
    ));
}

#[test]
fn same_session_resync_refuses_oversized_pending_text_edits() {
    use srui_protocol::PendingTextEditRef;
    use srui_sessiond::{SessionConfig, SessionError, MAX_TEXT_EDIT_STREAMS};

    let session = Session::with_config(
        "resync-pending-overflow",
        SessionConfig {
            journal_capacity: 1,
            ..SessionConfig::default()
        },
    );
    session
        .transaction(|ui| {
            Surface::builder(1).create(ui)?;
            TextInput::builder(2).parent(1).value("snap").create(ui)?;
            Ok(())
        })
        .expect("seed editor");
    session
        .commit_transaction(WireTransaction {
            base_revision: 1,
            new_revision: 2,
            priority: 0,
            operations: vec![],
        })
        .expect("evict seed from journal");

    let pending: Vec<PendingTextEditRef> = (0..=MAX_TEXT_EDIT_STREAMS)
        .map(|i| PendingTextEditRef {
            event_id: format!("overflow-{i}").into_bytes(),
            event_seq: i as u64 + 1,
            node_id: 2,
            edit_seq: 1,
        })
        .collect();
    let resume = ClientResume {
        session_id: "resync-pending-overflow".to_string(),
        client_instance_id: b"client-overflow".to_vec(),
        last_applied_revision: 0,
        last_acked_event_seq: 0,
        terminal_stream_offsets: Default::default(),
        limits: None,
        known_resource_hashes: vec![],
        pending_text_edits: pending,
    };

    match session.bootstrap_resume(&resume) {
        Err(SessionError::InvalidInput(message)) => {
            assert!(
                message.contains("pending_text_edits"),
                "diagnostic must name the field, got {message:?}"
            );
        }
        other => panic!("expected InvalidInput, got {other:?}"),
    }
}

#[test]
fn same_session_resync_validates_pending_text_edits_before_settling() {
    use srui_protocol::PendingTextEditRef;
    use srui_sessiond::{SessionConfig, SessionError};

    let session = Session::with_config(
        "resync-pending-partial",
        SessionConfig {
            journal_capacity: 1,
            ..SessionConfig::default()
        },
    );
    session
        .transaction(|ui| {
            Surface::builder(1).create(ui)?;
            TextInput::builder(2).parent(1).value("snap").create(ui)?;
            Ok(())
        })
        .expect("seed editor");
    session
        .commit_transaction(WireTransaction {
            base_revision: 1,
            new_revision: 2,
            priority: 0,
            operations: vec![],
        })
        .expect("evict seed from journal");

    let resume = ClientResume {
        session_id: "resync-pending-partial".to_string(),
        client_instance_id: b"client-partial".to_vec(),
        last_applied_revision: 0,
        last_acked_event_seq: 0,
        terminal_stream_offsets: Default::default(),
        limits: None,
        known_resource_hashes: vec![],
        pending_text_edits: vec![
            PendingTextEditRef {
                event_id: b"ok".to_vec(),
                event_seq: 1,
                node_id: 2,
                edit_seq: 1,
            },
            PendingTextEditRef {
                event_id: b"bad".to_vec(),
                event_seq: 2,
                node_id: 2,
                edit_seq: 0,
            },
        ],
    };

    match session.bootstrap_resume(&resume) {
        Err(SessionError::InvalidInput(message)) => {
            assert!(
                message.contains("edit_seq"),
                "diagnostic must name edit_seq, got {message:?}"
            );
        }
        other => panic!("expected InvalidInput, got {other:?}"),
    }

    let retry = ClientResume {
        pending_text_edits: vec![],
        ..resume
    };
    match session.bootstrap_resume(&retry).expect("retry").outcome {
        ResumeOutcome::Resync { resync_msg, .. } => {
            assert_eq!(resync_msg.last_processed_event_seq, 0);
            assert!(resync_msg.discarded_text_edits.is_empty());
        }
        other => panic!("expected same-session resync, got {other:?}"),
    }
}
