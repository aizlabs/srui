//! Multi-client transaction broadcast tests (§20.2).
//!
//! Verifies that attached clients receive identical transaction sequences,
//! remain isolated on disconnect, resync/close on broadcast lag, and exit cleanly
//! when the broadcast channel closes.

use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use tokio::io::duplex;
use tokio::sync::broadcast::error::RecvError;
use tokio::time::timeout;
use tokio_util::codec::{FramedRead, FramedWrite};
use tokio_util::sync::CancellationToken;

use srui_protocol::{
    srui_message, ClientHello, SruiCodec, SruiMessage, Transaction,
};
use srui_sessiond::{handle_connection, ConnectionError, Session, SessionError};

const TEST_BROADCAST_CAPACITY: usize = 2;

fn make_tx(base: u64) -> Transaction {
    Transaction {
        base_revision: base,
        new_revision: base + 1,
        priority: 1,
        operations: vec![],
    }
}

struct ClientConnection {
    read: FramedRead<tokio::io::ReadHalf<tokio::io::DuplexStream>, SruiCodec>,
    write: FramedWrite<tokio::io::WriteHalf<tokio::io::DuplexStream>, SruiCodec>,
    server_task: tokio::task::JoinHandle<Result<(), ConnectionError>>,
    shutdown: CancellationToken,
}

impl ClientConnection {
    async fn connect(session: Arc<Session>, duplex_capacity: usize) -> Self {
        let shutdown = CancellationToken::new();
        let (client_io, server_io) = duplex(duplex_capacity);

        let session_clone = session.clone();
        let shutdown_clone = shutdown.clone();
        let server_task = tokio::spawn(async move {
            handle_connection(server_io, session_clone, shutdown_clone).await
        });

        let (client_read, client_write) = tokio::io::split(client_io);
        let mut read = FramedRead::new(client_read, SruiCodec::new());
        let mut write = FramedWrite::new(client_write, SruiCodec::new());

        let hello = SruiMessage {
            msg: Some(srui_message::Msg::ClientHello(ClientHello {
                core_version: "0.4.0".to_string(),
                profiles: vec!["org.srui.standard-widgets/1".to_string()],
                limits: None,
                client_instance_id: vec![1, 2, 3, 4],
                client_metadata: Default::default(),
            })),
        };
        write.send(hello).await.expect("send ClientHello");

        let welcome = read.next().await.expect("welcome frame").expect("decode welcome");
        match welcome.msg {
            Some(srui_message::Msg::ServerWelcome(w)) => {
                assert_eq!(w.session_id, session.session_id());
            }
            other => panic!("expected ServerWelcome, got {:?}", other),
        }

        Self {
            read,
            write,
            server_task,
            shutdown,
        }
    }

    async fn recv_transaction(&mut self) -> Transaction {
        let msg = self
            .read
            .next()
            .await
            .expect("transaction frame")
            .expect("decode transaction");
        match msg.msg {
            Some(srui_message::Msg::Transaction(tx)) => tx,
            other => panic!("expected Transaction, got {:?}", other),
        }
    }

    async fn disconnect(self) -> Result<(), ConnectionError> {
        let Self {
            read,
            write,
            server_task,
            shutdown: _,
        } = self;
        drop(read);
        drop(write);
        server_task.await.expect("server task join")
    }

    async fn await_server(self) -> Result<(), ConnectionError> {
        self.server_task.await.expect("server task join")
    }

    async fn cancel_and_join(self) -> Result<(), ConnectionError> {
        self.shutdown.cancel();
        self.await_server().await
    }
}

fn commit_n(session: &Session, start_revision: u64, count: usize) -> u64 {
    let mut rev = start_revision;
    for _ in 0..count {
        session
            .commit_transaction(make_tx(rev))
            .expect("commit transaction");
        rev += 1;
    }
    rev
}

#[tokio::test]
async fn test_two_clients_receive_transactions_in_identical_order() {
    let session = Arc::new(Session::new("multi-client-order"));
    let mut client_a = ClientConnection::connect(session.clone(), 1024 * 1024).await;
    let mut client_b = ClientConnection::connect(session.clone(), 1024 * 1024).await;

    commit_n(&session, 0, 3);

    let mut revisions_a = Vec::new();
    let mut revisions_b = Vec::new();
    for _ in 0..3 {
        revisions_a.push(client_a.recv_transaction().await.new_revision);
        revisions_b.push(client_b.recv_transaction().await.new_revision);
    }

    assert_eq!(revisions_a, vec![1, 2, 3]);
    assert_eq!(revisions_b, revisions_a);

    client_a.cancel_and_join().await.expect("client A clean shutdown");
    client_b.cancel_and_join().await.expect("client B clean shutdown");
}

#[tokio::test]
async fn test_disconnecting_one_client_does_not_affect_the_other() {
    let session = Arc::new(Session::new("multi-client-isolation"));
    let mut surviving = ClientConnection::connect(session.clone(), 1024 * 1024).await;
    let mut departing = ClientConnection::connect(session.clone(), 1024 * 1024).await;

    commit_n(&session, 0, 1);
    assert_eq!(surviving.recv_transaction().await.new_revision, 1);
    let _ = departing.recv_transaction().await;

    let departing_result = departing.disconnect().await;
    assert!(
        departing_result.is_ok(),
        "departing connection should exit cleanly, got {:?}",
        departing_result
    );

    commit_n(&session, 1, 2);
    assert_eq!(surviving.recv_transaction().await.new_revision, 2);
    assert_eq!(surviving.recv_transaction().await.new_revision, 3);

    surviving
        .cancel_and_join()
        .await
        .expect("surviving client clean shutdown");
}

#[tokio::test]
async fn test_lagged_broadcast_closes_connection_for_resync() {
    let session = Arc::new(Session::with_broadcast_capacity(
        "multi-client-lagged",
        TEST_BROADCAST_CAPACITY,
    ));

    let shutdown = CancellationToken::new();
    let (client_io, server_io) = duplex(64);

    let session_clone = session.clone();
    let shutdown_clone = shutdown.clone();
    let server_task = tokio::spawn(async move {
        handle_connection(server_io, session_clone, shutdown_clone).await
    });

    let (client_read, client_write) = tokio::io::split(client_io);
    let mut framed_read = FramedRead::new(client_read, SruiCodec::new());
    let mut framed_write = FramedWrite::new(client_write, SruiCodec::new());

    let hello = SruiMessage {
        msg: Some(srui_message::Msg::ClientHello(ClientHello {
            core_version: "0.4.0".to_string(),
            profiles: vec!["org.srui.standard-widgets/1".to_string()],
            limits: None,
            client_instance_id: vec![9, 8, 7, 6],
            client_metadata: Default::default(),
        })),
    };
    framed_write.send(hello).await.expect("send ClientHello");
    let welcome = framed_read.next().await.expect("welcome frame").expect("decode");
    assert!(matches!(
        welcome.msg,
        Some(srui_message::Msg::ServerWelcome(_))
    ));

    // Flood commits while the client is not reading so the broadcast receiver falls behind.
    commit_n(
        &session,
        0,
        TEST_BROADCAST_CAPACITY + TEST_BROADCAST_CAPACITY + 1,
    );

    let server_result = timeout(Duration::from_secs(2), server_task)
        .await
        .expect("lagged connection should finish promptly")
        .expect("server task join");
    assert!(
        matches!(
            server_result,
            Err(ConnectionError::Session(SessionError::LaggedResyncRequired))
        ),
        "expected LaggedResyncRequired, got {:?}",
        server_result
    );

    drop(framed_write);
    drop(framed_read);
    shutdown.cancel();
}

#[tokio::test]
async fn test_transaction_broadcast_closed_exits_connection_cleanly() {
    let session = Arc::new(Session::new("multi-client-broadcast-closed"));
    let client = ClientConnection::connect(session.clone(), 1024 * 1024).await;

    session.close_transaction_broadcast();

    let server_result = timeout(Duration::from_secs(2), client.await_server())
        .await
        .expect("closed broadcast should finish promptly");
    assert!(
        server_result.is_ok(),
        "broadcast channel closed should exit cleanly, got {:?}",
        server_result
    );
}

/// A connection accepted after the broadcast sender was dropped must fail that one connection,
/// not panic the accept task (§20.2).
#[tokio::test]
async fn test_subscribe_after_broadcast_closed_reports_error() {
    let session = Session::new("broadcast-closed-subscribe");
    session.close_transaction_broadcast();

    assert!(matches!(
        session.subscribe_transactions(),
        Err(SessionError::BroadcastClosed)
    ));
}

#[tokio::test]
async fn test_transaction_broadcast_closed_notifies_subscribers() {
    let session = Session::new("broadcast-closed-subscriber");
    let mut rx = session.subscribe_transactions().expect("broadcast open");
    session.close_transaction_broadcast();

    assert!(matches!(rx.recv().await, Err(RecvError::Closed)));
}
