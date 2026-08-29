//! Remote authority and active-session message validation (§12, §18, §20.2).

use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use tokio::io::duplex;
use tokio::sync::broadcast::error::TryRecvError;
use tokio::time::timeout;
use tokio_util::codec::{FramedRead, FramedWrite};
use tokio_util::sync::CancellationToken;

use srui_protocol::{
    srui_message, ClientHello, ClientResume, CreateNodeOp, NodeRecord, Operation, ServerResumeOk,
    ServerResyncRequired, ServerWelcome, SruiCodec, SruiMessage, Transaction, TypeRef,
};
use srui_sdk::{NodeId, Surface};
use srui_sessiond::{handle_connection, ConnectionError, Session};

fn client_hello_message() -> SruiMessage {
    SruiMessage {
        msg: Some(srui_message::Msg::ClientHello(ClientHello {
            core_version: "0.4.0".to_string(),
            profiles: vec!["org.srui.standard-widgets/1".to_string()],
            limits: None,
            client_instance_id: vec![1, 2, 3, 4],
            client_metadata: Default::default(),
        })),
    }
}

async fn connect_client(
    session: Arc<Session>,
    shutdown: CancellationToken,
) -> (
    FramedWrite<tokio::io::WriteHalf<tokio::io::DuplexStream>, SruiCodec>,
    FramedRead<tokio::io::ReadHalf<tokio::io::DuplexStream>, SruiCodec>,
    tokio::task::JoinHandle<Result<(), ConnectionError>>,
) {
    let (client_io, server_io) = duplex(1024 * 1024);
    let session_clone = session.clone();
    let shutdown_clone = shutdown.clone();
    let server_task =
        tokio::spawn(
            async move { handle_connection(server_io, session_clone, shutdown_clone).await },
        );

    let (client_read, client_write) = tokio::io::split(client_io);
    let mut client_framed_read = FramedRead::new(client_read, SruiCodec::new());
    let mut client_framed_write = FramedWrite::new(client_write, SruiCodec::new());
    client_framed_write
        .send(client_hello_message())
        .await
        .expect("send hello");
    let welcome_msg = client_framed_read
        .next()
        .await
        .expect("receive welcome")
        .expect("decode welcome");
    match welcome_msg.msg {
        Some(srui_message::Msg::ServerWelcome(w)) => {
            assert_eq!(w.session_id, session.session_id());
        }
        other => panic!("expected ServerWelcome, got {other:?}"),
    }
    (client_framed_write, client_framed_read, server_task)
}

/// §4 inv. 13: only unknown *required* semantics fail closed. prost decodes an envelope whose
/// oneof field number this build does not know to `None`, exactly like an empty envelope, so
/// treating that as fatal would drop connections from clients speaking a newer protocol.
#[tokio::test]
async fn test_unrecognized_active_session_envelope_is_ignored() {
    let session = Arc::new(Session::new("unknown-envelope"));
    let shutdown = CancellationToken::new();
    session
        .transaction(|ui| {
            Surface::builder(NodeId::new(1)).label("baseline").create(ui)?;
            Ok(())
        })
        .expect("baseline transaction");

    let (mut client_write, mut client_read, server_task) =
        connect_client(session.clone(), shutdown.clone()).await;
    client_write
        .send(SruiMessage::default())
        .await
        .expect("send unrecognized envelope");

    // The connection must still be serving: a transaction committed afterwards reaches this client.
    session
        .transaction(|ui| {
            Surface::builder(NodeId::new(2))
                .label("still connected")
                .create(ui)?;
            Ok(())
        })
        .expect("post-unknown-envelope transaction");
    let message = timeout(Duration::from_secs(2), client_read.next())
        .await
        .expect("receive timed out")
        .expect("stream closed by unrecognized envelope")
        .expect("frame decode");
    assert!(matches!(
        message.msg,
        Some(srui_message::Msg::Transaction(_))
    ));
    assert_eq!(session.current_revision(), 2);

    shutdown.cancel();
    server_task.await.expect("join").expect("clean shutdown");
}

#[tokio::test]
async fn test_client_transaction_rejected_without_mutating_authority() {
    let session = Arc::new(Session::new("authority-test"));
    let shutdown = CancellationToken::new();
    let surface_id = NodeId::new(1);
    let mut broadcast_rx = session.subscribe_transactions().expect("broadcast open");
    session
        .transaction(|ui| {
            Surface::builder(surface_id)
                .label("Authority baseline")
                .create(ui)?;
            Ok(())
        })
        .expect("server baseline transaction");

    let baseline_revision = session.current_revision();
    let baseline_node_count = session.node_count();
    let baseline_journal = session
        .collect_replayed_transactions(0)
        .expect("journal replay available");
    assert_eq!(baseline_revision, 1);
    assert_eq!(baseline_node_count, 1);
    assert_eq!(baseline_journal.len(), 1);
    broadcast_rx
        .recv()
        .await
        .expect("baseline transaction broadcast");

    let (mut client_write, _client_read, server_task) =
        connect_client(session.clone(), shutdown).await;
    client_write
        .send(SruiMessage {
            msg: Some(srui_message::Msg::Transaction(Transaction {
                base_revision: baseline_revision,
                new_revision: baseline_revision + 1,
                priority: 1,
                operations: vec![Operation {
                    op: Some(srui_protocol::operation::Op::CreateNode(CreateNodeOp {
                        node: Some(NodeRecord {
                            node_id: 99,
                            r#type: Some(TypeRef {
                                namespace_id: 0,
                                local_id: 1,
                            }),
                            parent_id: 0,
                            child_index: 0,
                            properties: vec![],
                        }),
                    })),
                }],
            })),
        })
        .await
        .expect("send client transaction");

    let server_result = server_task.await.expect("server task join");
    assert!(matches!(
        server_result,
        Err(ConnectionError::ClientTransactionRejected)
    ));
    assert_eq!(session.current_revision(), baseline_revision);
    assert_eq!(session.node_count(), baseline_node_count);
    assert!(session.contains_node(surface_id));
    assert!(!session.contains_node(NodeId::new(99)));
    assert_eq!(
        session
            .collect_replayed_transactions(0)
            .expect("journal replay still available"),
        baseline_journal
    );
    assert!(matches!(broadcast_rx.try_recv(), Err(TryRecvError::Empty)));
}

#[tokio::test]
async fn test_client_transaction_on_pristine_session_rejected() {
    let session = Arc::new(Session::new("authority-pristine"));
    let shutdown = CancellationToken::new();
    assert_eq!(session.current_revision(), 0);
    assert_eq!(session.node_count(), 0);
    let baseline_journal = session
        .collect_replayed_transactions(0)
        .expect("empty journal replay");
    assert!(baseline_journal.is_empty());
    let mut broadcast_rx = session.subscribe_transactions().expect("broadcast open");

    let (mut client_write, _client_read, server_task) =
        connect_client(session.clone(), shutdown).await;
    client_write
        .send(SruiMessage {
            msg: Some(srui_message::Msg::Transaction(Transaction {
                base_revision: 0,
                new_revision: 1,
                priority: 1,
                operations: vec![],
            })),
        })
        .await
        .expect("send client transaction");

    let server_result = timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task timed out")
        .expect("server task join");
    assert!(matches!(
        server_result,
        Err(ConnectionError::ClientTransactionRejected)
    ));
    assert_eq!(session.current_revision(), 0);
    assert_eq!(session.node_count(), 0);
    assert_eq!(
        session
            .collect_replayed_transactions(0)
            .expect("journal replay still available"),
        baseline_journal
    );
    assert!(matches!(broadcast_rx.try_recv(), Err(TryRecvError::Empty)));
}

#[tokio::test]
async fn test_active_session_rejects_handshake_server_and_empty_messages() {
    let cases = [
        (
            client_hello_message(),
            "ClientHello is valid only during handshake",
        ),
        (
            SruiMessage {
                msg: Some(srui_message::Msg::ClientResume(ClientResume::default())),
            },
            "ClientResume is valid only during handshake",
        ),
        (
            SruiMessage {
                msg: Some(srui_message::Msg::ServerWelcome(ServerWelcome::default())),
            },
            "server-only or unsupported message during active session",
        ),
        (
            SruiMessage {
                msg: Some(srui_message::Msg::ServerResumeOk(ServerResumeOk::default())),
            },
            "server-only or unsupported message during active session",
        ),
        (
            SruiMessage {
                msg: Some(srui_message::Msg::ServerResyncRequired(
                    ServerResyncRequired::default(),
                )),
            },
            "server-only or unsupported message during active session",
        ),
    ];

    for (case_index, (message, expected_error)) in cases.into_iter().enumerate() {
        let session = Arc::new(Session::new(format!("active-message-{case_index}")));
        let baseline_id = NodeId::new(1);
        session
            .transaction(|ui| {
                Surface::builder(baseline_id).label("baseline").create(ui)?;
                Ok(())
            })
            .expect("baseline transaction");
        let baseline_journal = session
            .collect_replayed_transactions(0)
            .expect("baseline journal");

        let shutdown = CancellationToken::new();
        let (_companion_write, mut companion_read, companion_task) =
            connect_client(session.clone(), shutdown.clone()).await;
        let (mut offender_write, _offender_read, offender_task) =
            connect_client(session.clone(), shutdown.clone()).await;
        offender_write
            .send(message)
            .await
            .expect("send invalid active message");

        let offender_result = timeout(Duration::from_secs(2), offender_task)
            .await
            .expect("offender task timed out")
            .expect("offender task join");
        assert!(
            matches!(
                offender_result,
                Err(ConnectionError::UnexpectedMessage(actual)) if actual == expected_error
            ),
            "case {case_index}: expected UnexpectedMessage({expected_error:?}), got {offender_result:?}"
        );
        assert_eq!(session.current_revision(), 1);
        assert_eq!(session.node_count(), 1);
        assert_eq!(
            session
                .collect_replayed_transactions(0)
                .expect("journal after rejection"),
            baseline_journal
        );

        session
            .transaction(|ui| {
                Surface::builder(NodeId::new(2))
                    .label("companion still live")
                    .create(ui)?;
                Ok(())
            })
            .expect("post-rejection server transaction");
        let companion_message = timeout(Duration::from_secs(2), companion_read.next())
            .await
            .expect("companion receive timed out")
            .expect("companion stream closed")
            .expect("companion frame decode");
        assert!(matches!(
            companion_message.msg,
            Some(srui_message::Msg::Transaction(_))
        ));

        shutdown.cancel();
        companion_task
            .await
            .expect("companion join")
            .expect("companion shutdown");
    }
}
