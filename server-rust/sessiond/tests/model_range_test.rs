//! Collection range-request fulfillment over the active session (§8, §12.1, §22.7).

#[allow(dead_code)]
mod common;
use common::TestClientConnection;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use tokio::io::duplex;
use tokio::time::timeout;
use tokio_util::codec::{FramedRead, FramedWrite};
use tokio_util::sync::CancellationToken;

use srui_protocol::{
    operation::Op, srui_message, ClientModelRangeRequest, EventAckStatus, SruiCodec, SruiMessage,
};
use srui_sdk::{Button, NodeId, Surface, Table, ACTIVATE};
use srui_semantic_tree::{ItemId, ModelId, ModelItem, Operation, TypeRef};
use srui_sessiond::{
    handle_connection, ConnectionError, ModelRangeError, ModelRangeFulfillment, ModelRangeProvider,
    ModelRangeQuery, Session,
};

const CLIENT_ID: &[u8] = b"range-client";

fn item_at(index: u64) -> ModelItem {
    ModelItem::with_value(ItemId::new(index + 1), format!("row-{index}"))
}

fn counting_provider(calls: Arc<AtomicUsize>) -> ModelRangeProvider {
    Arc::new(move |query: ModelRangeQuery| {
        let calls = Arc::clone(&calls);
        Box::pin(async move {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok((0..query.count)
                .map(|offset| item_at(query.start_index + offset))
                .collect())
        })
    })
}

fn seed_table(session: &Session, item_count: u64) -> (NodeId, ModelId) {
    let node_id = NodeId::new(2);
    let model_id = ModelId::new(7);
    session
        .transaction(|ui| {
            Surface::builder(NodeId::new(1))
                .label("surface")
                .create(ui)?;
            ui.apply_op(&Operation::create_model(
                model_id,
                TypeRef::TABLE,
                item_count,
            ))?;
            Table::builder(node_id)
                .parent(NodeId::new(1))
                .model_ref(model_id)
                .columns(["Index", "Label"])
                .create(ui)?;
            Ok(())
        })
        .expect("seed table");
    (node_id, model_id)
}

fn range_request(
    node_id: NodeId,
    model_id: ModelId,
    start: u64,
    count: u64,
    revision: u64,
) -> SruiMessage {
    SruiMessage {
        msg: Some(srui_message::Msg::ClientModelRangeRequest(
            ClientModelRangeRequest {
                node_id: node_id.get(),
                model_id: model_id.get(),
                start_index: start,
                count,
                observed_revision: revision,
            },
        )),
    }
}

fn is_reset_range(msg: &SruiMessage, start: u64) -> bool {
    match &msg.msg {
        Some(srui_message::Msg::Transaction(tx)) => tx.operations.iter().any(|op| {
            matches!(
                &op.op,
                Some(Op::ModelResetRange(reset)) if reset.start_index == start
            )
        }),
        _ => false,
    }
}

#[tokio::test]
async fn valid_request_commits_one_reset_range_revision() {
    let session = Arc::new(Session::new("range-valid"));
    let (node_id, model_id) = seed_table(&session, 1_000);
    let calls = Arc::new(AtomicUsize::new(0));
    session.register_model_range_provider(model_id, counting_provider(Arc::clone(&calls)));
    let revision = session.current_revision();

    let (mut client, _, _) =
        TestClientConnection::connect_fresh(Arc::clone(&session), CLIENT_ID).await;
    client
        .write
        .send(range_request(node_id, model_id, 10, 2, revision))
        .await
        .expect("send range request");

    let msg = timeout(Duration::from_secs(2), client.read.next())
        .await
        .expect("timeout")
        .expect("eof")
        .expect("decode");
    assert!(
        is_reset_range(&msg, 10),
        "expected MODEL_RESET_RANGE, got {msg:?}"
    );
    if let Some(srui_message::Msg::Transaction(tx)) = msg.msg {
        assert_eq!(tx.base_revision, revision);
        assert_eq!(tx.new_revision, revision + 1);
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(session.current_revision(), revision + 1);
}

#[tokio::test]
async fn invalid_requests_never_invoke_the_provider() {
    let session = Arc::new(Session::new("range-invalid"));
    let (node_id, model_id) = seed_table(&session, 100);
    let calls = Arc::new(AtomicUsize::new(0));
    session.register_model_range_provider(model_id, counting_provider(Arc::clone(&calls)));
    let revision = session.current_revision();

    let (mut client, _, _) =
        TestClientConnection::connect_fresh(Arc::clone(&session), CLIENT_ID).await;
    let cases = [
        range_request(NodeId::new(0), model_id, 0, 1, revision),
        range_request(node_id, ModelId::new(0), 0, 1, revision),
        range_request(node_id, model_id, 0, 0, revision),
        range_request(node_id, model_id, 99, 2, revision),
        range_request(node_id, model_id, 0, 10_001, revision),
        range_request(node_id, model_id, 0, 1, revision.saturating_sub(1)),
        range_request(NodeId::new(99), model_id, 0, 1, revision),
        range_request(node_id, ModelId::new(99), 0, 1, revision),
        SruiMessage {
            msg: Some(srui_message::Msg::ClientModelRangeRequest(
                ClientModelRangeRequest {
                    node_id: node_id.get(),
                    model_id: model_id.get(),
                    start_index: u64::MAX,
                    count: 1,
                    observed_revision: revision,
                },
            )),
        },
    ];
    for case in cases {
        client.write.send(case).await.expect("send invalid");
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(session.current_revision(), revision);
    assert!(
        timeout(Duration::from_millis(100), client.read.next())
            .await
            .is_err(),
        "invalid requests must not produce a transaction"
    );
}

#[tokio::test]
async fn duplicate_inflight_requests_are_coalesced() {
    let session = Arc::new(Session::new("range-coalesce"));
    let (node_id, model_id) = seed_table(&session, 1_000);
    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let release_rx = Arc::new(tokio::sync::Mutex::new(Some(release_rx)));
    let calls = Arc::new(AtomicUsize::new(0));
    let starts = Arc::new(std::sync::Mutex::new(Vec::<u64>::new()));
    session.register_model_range_provider(
        model_id,
        Arc::new({
            let calls = Arc::clone(&calls);
            let starts = Arc::clone(&starts);
            move |query: ModelRangeQuery| {
                let calls = Arc::clone(&calls);
                let starts = Arc::clone(&starts);
                let release_rx = Arc::clone(&release_rx);
                Box::pin(async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    starts.lock().expect("starts").push(query.start_index);
                    if let Some(rx) = release_rx.lock().await.take() {
                        let _ = rx.await;
                        // Fail without committing so the coalesced follow-up keeps a
                        // matching observed_revision and can actually run.
                        return Err(ModelRangeError::Provider(
                            "in-flight request abandoned for coalescing test".into(),
                        ));
                    }
                    Ok((0..query.count)
                        .map(|offset| item_at(query.start_index + offset))
                        .collect())
                })
            }
        }),
    );
    let revision = session.current_revision();
    let (mut client, _, _) =
        TestClientConnection::connect_fresh(Arc::clone(&session), CLIENT_ID).await;

    client
        .write
        .send(range_request(node_id, model_id, 0, 8, revision))
        .await
        .unwrap();
    timeout(Duration::from_secs(2), async {
        while calls.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("provider entered");

    client
        .write
        .send(range_request(node_id, model_id, 128, 8, revision))
        .await
        .unwrap();
    client
        .write
        .send(range_request(node_id, model_id, 256, 8, revision))
        .await
        .unwrap();
    // Let the read loop enqueue both follow-ups so the inbox can replace 128 with 256
    // before the gated first call returns.
    tokio::time::sleep(Duration::from_millis(50)).await;
    release_tx.send(()).unwrap();

    let filled = timeout(Duration::from_secs(2), client.read.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        is_reset_range(&filled, 256),
        "latest pending request must win, got {filled:?}"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        starts.lock().expect("starts").as_slice(),
        &[0, 256],
        "the replaced 128-window must never reach the provider"
    );
    assert_eq!(session.current_revision(), revision + 1);
    assert!(
        timeout(Duration::from_millis(150), client.read.next())
            .await
            .is_err(),
        "coalesced follow-up must commit exactly one revision"
    );
}

#[tokio::test]
async fn gated_provider_does_not_block_activate_ack() {
    let session = Arc::new(Session::new("range-gated-ack"));
    let (node_id, model_id) = seed_table(&session, 64);
    let button = NodeId::new(3);
    session
        .transaction(|ui| {
            Button::builder(button)
                .parent(NodeId::new(1))
                .label("go")
                .create(ui)?;
            Ok(())
        })
        .unwrap();
    session.on(button, ACTIVATE, move |_, _| {});

    let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
    let release_rx = Arc::new(tokio::sync::Mutex::new(Some(release_rx)));
    session.register_model_range_provider(
        model_id,
        Arc::new(move |query: ModelRangeQuery| {
            let release_rx = Arc::clone(&release_rx);
            Box::pin(async move {
                if let Some(rx) = release_rx.lock().await.take() {
                    let _ = rx.await;
                }
                Ok((0..query.count)
                    .map(|offset| item_at(query.start_index + offset))
                    .collect())
            })
        }),
    );
    let revision = session.current_revision();
    let (mut client, _, _) =
        TestClientConnection::connect_fresh(Arc::clone(&session), CLIENT_ID).await;

    client
        .write
        .send(range_request(node_id, model_id, 0, 4, revision))
        .await
        .unwrap();
    client
        .send_activate(CLIENT_ID, 1, "e1", revision, button)
        .await;
    let ack = timeout(Duration::from_secs(2), client.recv_event_ack())
        .await
        .expect("activate ACK must not wait on the range provider");
    assert_eq!(ack.status, EventAckStatus::Processed as i32);

    release_tx.send(()).unwrap();
    let tx = timeout(Duration::from_secs(2), client.read.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(is_reset_range(&tx, 0));
}

#[tokio::test]
async fn proactive_push_uses_the_same_fulfillment_path() {
    let session = Arc::new(Session::new("range-push"));
    let (node_id, model_id) = seed_table(&session, 128);
    let calls = Arc::new(AtomicUsize::new(0));
    session.register_model_range_provider(model_id, counting_provider(Arc::clone(&calls)));
    let (mut client, _, _) =
        TestClientConnection::connect_fresh(Arc::clone(&session), CLIENT_ID).await;

    let outcome = session
        .push_visible_model_range(node_id, model_id, 0, 8)
        .await
        .unwrap();
    assert!(matches!(outcome, ModelRangeFulfillment::Committed { .. }));
    let msg = timeout(Duration::from_secs(2), client.read.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(is_reset_range(&msg, 0));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn range_transaction_is_included_in_snapshot_and_replay() {
    let session = Arc::new(Session::new("range-replay"));
    let (node_id, model_id) = seed_table(&session, 64);
    session
        .register_model_range_provider(model_id, counting_provider(Arc::new(AtomicUsize::new(0))));
    let before = session.current_revision();
    session
        .push_visible_model_range(node_id, model_id, 0, 4)
        .await
        .unwrap();

    let replayed = session
        .collect_replayed_transactions(before)
        .expect("replay");
    assert_eq!(replayed.len(), 1);
    assert!(replayed[0]
        .operations
        .iter()
        .any(|op| { matches!(op.op, Some(Op::ModelResetRange(_))) }));
    session.with_store(|store| {
        let model = store.get_model(model_id).expect("model");
        assert!(model.get_item_by_index(0).is_some());
        assert!(model.get_item_by_index(3).is_some());
    });

    let from_start = session
        .collect_replayed_transactions(0)
        .expect("full replay");
    assert!(from_start.iter().any(|tx| {
        tx.operations
            .iter()
            .any(|op| matches!(op.op, Some(Op::ModelResetRange(_))))
    }));

    let (_, _, initial_rev) =
        TestClientConnection::connect_fresh(Arc::clone(&session), b"snapshot-client").await;
    assert_eq!(initial_rev, session.current_revision());
}

#[tokio::test]
async fn range_transaction_follows_earlier_ui_revision() {
    let session = Arc::new(Session::new("range-fifo"));
    let (node_id, model_id) = seed_table(&session, 64);
    session
        .register_model_range_provider(model_id, counting_provider(Arc::new(AtomicUsize::new(0))));
    let (mut client, _, _) =
        TestClientConnection::connect_fresh(Arc::clone(&session), CLIENT_ID).await;

    let before = session.current_revision();
    session
        .transaction(|ui| {
            ui.set(NodeId::new(1), srui_sdk::LABEL, "moved")?;
            Ok(())
        })
        .unwrap();
    session
        .push_visible_model_range(node_id, model_id, 0, 4)
        .await
        .unwrap();

    let first = timeout(Duration::from_secs(2), client.read.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let second = timeout(Duration::from_secs(2), client.read.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    match (&first.msg, &second.msg) {
        (
            Some(srui_message::Msg::Transaction(earlier)),
            Some(srui_message::Msg::Transaction(later)),
        ) => {
            assert_eq!(earlier.base_revision, before);
            assert_eq!(earlier.new_revision, before + 1);
            assert_eq!(later.base_revision, before + 1);
            assert_eq!(later.new_revision, before + 2);
            assert!(
                !is_reset_range(&first, 0),
                "earlier UI revision must not be overtaken by the range fill"
            );
            assert!(is_reset_range(&second, 0));
        }
        other => panic!("expected two transactions, got {other:?}"),
    }
}

#[tokio::test]
async fn pre_handshake_range_request_is_rejected() {
    let session = Arc::new(Session::new("range-pre-handshake"));
    let shutdown = CancellationToken::new();
    let (client_io, server_io) = duplex(64 * 1024);
    let handle = tokio::spawn({
        let session = Arc::clone(&session);
        async move { handle_connection(server_io, session, shutdown).await }
    });
    let (client_read, client_write) = tokio::io::split(client_io);
    let mut write = FramedWrite::new(client_write, SruiCodec::new());
    let mut read = FramedRead::new(client_read, SruiCodec::new());
    write
        .send(range_request(NodeId::new(2), ModelId::new(7), 0, 1, 0))
        .await
        .unwrap();
    let result = timeout(Duration::from_secs(2), handle)
        .await
        .expect("server join")
        .expect("join");
    assert!(
        matches!(
            result,
            Err(ConnectionError::UnexpectedMessage(
                "expected ClientHello or ClientResume"
            ))
        ),
        "got {result:?}"
    );
    assert!(read.next().await.is_none());
}
