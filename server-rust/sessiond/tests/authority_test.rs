//! Remote authority: client-originated transactions must not mutate server state (§12, §20.2).

use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use tokio::io::duplex;
use tokio::sync::broadcast::error::TryRecvError;
use tokio::time::timeout;
use tokio_util::codec::{FramedRead, FramedWrite};
use tokio_util::sync::CancellationToken;

use srui_protocol::{
    srui_message, ClientHello, CreateNodeOp, NodeRecord, Operation, SruiCodec, SruiMessage,
    Transaction, TypeRef,
};
use srui_sdk::{NodeId, Surface};
use srui_sessiond::{handle_connection, ConnectionError, Session};

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
    let server_task = tokio::spawn(async move {
        handle_connection(server_io, session_clone, shutdown_clone).await
    });

    let (client_read, client_write) = tokio::io::split(client_io);
    let mut client_framed_read = FramedRead::new(client_read, SruiCodec::new());
    let mut client_framed_write = FramedWrite::new(client_write, SruiCodec::new());

    let hello = SruiMessage {
        msg: Some(srui_message::Msg::ClientHello(ClientHello {
            core_version: "0.4.0".to_string(),
            profiles: vec!["org.srui.standard-widgets/1".to_string()],
            limits: None,
            client_instance_id: vec![1, 2, 3, 4],
            client_metadata: Default::default(),
        })),
    };
    client_framed_write.send(hello).await.expect("send hello");

    let welcome_msg = client_framed_read
        .next()
        .await
        .expect("receive welcome")
        .expect("decode welcome");
    match welcome_msg.msg {
        Some(srui_message::Msg::ServerWelcome(w)) => {
            assert_eq!(w.session_id, session.session_id());
        }
        other => panic!("expected ServerWelcome, got {:?}", other),
    }

    (
        client_framed_write,
        client_framed_read,
        server_task,
    )
}

#[tokio::test]
async fn test_client_transaction_rejected_without_mutating_authority() {
    let session = Arc::new(Session::new("authority-test"));
    let shutdown = CancellationToken::new();
    let surface_id = NodeId::new(1);

    let mut broadcast_rx = session.subscribe_transactions();

    // Establish authoritative baseline: one committed server transaction with store content.
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

    // Drain the broadcast emitted by the baseline commit.
    broadcast_rx
        .recv()
        .await
        .expect("baseline transaction broadcast");

    let (mut client_write, _client_read, server_task) =
        connect_client(session.clone(), shutdown.clone()).await;

    // Client attempts to advance revision and mutate the store.
    let client_tx = SruiMessage {
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
    };
    client_write
        .send(client_tx)
        .await
        .expect("send client transaction");

    let server_result = server_task.await.expect("server task join");
    assert!(
        matches!(server_result, Err(ConnectionError::ClientTransactionRejected)),
        "expected ClientTransactionRejected, got {:?}",
        server_result
    );

    // Revision, journal, and store must be unchanged.
    assert_eq!(session.current_revision(), baseline_revision);
    assert_eq!(session.node_count(), baseline_node_count);
    assert!(session.contains_node(surface_id));
    assert!(!session.contains_node(NodeId::new(99)));

    let journal_after = session
        .collect_replayed_transactions(0)
        .expect("journal replay still available");
    assert_eq!(journal_after, baseline_journal);

    // Rejected client transaction must not be broadcast.
    assert!(matches!(
        broadcast_rx.try_recv(),
        Err(TryRecvError::Empty)
    ));
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

    let mut broadcast_rx = session.subscribe_transactions();

    let (mut client_write, _client_read, server_task) =
        connect_client(session.clone(), shutdown.clone()).await;

    let client_tx = SruiMessage {
        msg: Some(srui_message::Msg::Transaction(Transaction {
            base_revision: 0,
            new_revision: 1,
            priority: 1,
            operations: vec![],
        })),
    };
    client_write
        .send(client_tx)
        .await
        .expect("send client transaction");

    let server_result = timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task timed out")
        .expect("server task join");
    assert!(
        matches!(server_result, Err(ConnectionError::ClientTransactionRejected)),
        "expected ClientTransactionRejected, got {:?}",
        server_result
    );

    assert_eq!(session.current_revision(), 0);
    assert_eq!(session.node_count(), 0);

    let journal_after = session
        .collect_replayed_transactions(0)
        .expect("journal replay still available");
    assert_eq!(journal_after, baseline_journal);

    assert!(matches!(
        broadcast_rx.try_recv(),
        Err(TryRecvError::Empty)
    ));
}
