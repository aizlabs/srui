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
    srui_message, ClientResume, SessionContinuity, SruiCodec, SruiMessage,
    Transaction as WireTransaction,
};
use srui_sdk::*;
use srui_semantic_tree::{
    ItemId, ModelId, ModelItem, NodeId, Operation, Revision, SemanticStore,
    StoreLimits, Transaction, TxnError, TypeRef, Value, DEFAULT_MAX_ITEMS_PER_MODEL_OPERATION,
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

            Surface::builder(1)
                .label("Second Root")
                .create(ui)?;
            Surface::builder(2)
                .label("First Root")
                .create(ui)?;

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

fn apply_resync_snapshot(wire_tx: WireTransaction) -> Result<SemanticStore, TxnError> {
    let txn = Transaction::try_from(wire_tx)?;
    assert_eq!(
        txn.base_revision,
        Revision::INITIAL,
        "snapshot must start from revision 0"
    );

    let limits = StoreLimits::default();
    let mut staged = SemanticStore::with_limits_and_revision(limits.clone(), Revision::INITIAL);
    for (idx, op) in txn.operations.iter().enumerate() {
        op.apply(&mut staged).map_err(|source| TxnError::OpFailed {
            op_index: idx,
            source,
        })?;
    }

    let mut committed = SemanticStore::with_limits_and_revision(limits, Revision::INITIAL);
    committed.commit_staging(staged, txn.new_revision);
    Ok(committed)
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
    };

    let (resync_msg, snapshot_tx) = match session.handle_resume(&resume).expect("resume handled") {
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
            .any(|op| matches!(
                op.op,
                Some(srui_protocol::operation::Op::CreateModel(_))
            )),
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
            ui.apply_op(&Operation::create_model(model_id, TypeRef::LIST, TOTAL_ITEMS))?;
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
    };
    let snapshot_tx = match session.handle_resume(&resume).expect("resume handled") {
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
    let server_task = tokio::spawn(async move {
        handle_connection(server_io, session_clone, shutdown_clone).await
    });

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
        assert_eq!(model.get_item_by_index(0).unwrap().value, Value::String("alpha".into()));
        assert_eq!(model.get_item_by_index(1).unwrap().value, Value::String("beta".into()));
    });

    shutdown.cancel();
    drop(framed_write);
    drop(framed_read);
    let _ = server_task.await;
}
