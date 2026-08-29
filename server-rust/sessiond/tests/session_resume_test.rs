//! # Session Resume and Replay Wire Tests (§18, §20.2, §21)
//!
//! Verifies `ClientResume` / `ServerResumeOk` handshake and journal replay over
//! length-prefixed wire framing via [`handle_connection`].

use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use tokio::io::duplex;
use tokio::sync::Barrier;
use tokio_util::codec::{FramedRead, FramedWrite};
use tokio_util::sync::CancellationToken;

use srui_protocol::{
    srui_message, ClientResume, SessionContinuity, SruiCodec, SruiMessage, Transaction,
};
use srui_sessiond::{handle_connection, ConnectionError, Session};

fn make_tx(base: u64) -> Transaction {
    Transaction {
        base_revision: base,
        new_revision: base + 1,
        priority: 1,
        operations: vec![],
    }
}

fn client_resume(session_id: &str, last_applied: u64) -> SruiMessage {
    SruiMessage {
        msg: Some(srui_message::Msg::ClientResume(ClientResume {
            session_id: session_id.to_string(),
            client_instance_id: vec![1, 2, 3],
            last_applied_revision: last_applied,
            last_acked_event_seq: 0,
            terminal_stream_offsets: Default::default(),
        })),
    }
}

struct ResumeConnection {
    read: FramedRead<tokio::io::ReadHalf<tokio::io::DuplexStream>, SruiCodec>,
    write: FramedWrite<tokio::io::WriteHalf<tokio::io::DuplexStream>, SruiCodec>,
    server_task: tokio::task::JoinHandle<Result<(), ConnectionError>>,
    shutdown: CancellationToken,
}

impl ResumeConnection {
    async fn open(session: Arc<Session>) -> Self {
        let shutdown = CancellationToken::new();
        let (client_io, server_io) = duplex(1024 * 1024);

        let session_clone = session.clone();
        let shutdown_clone = shutdown.clone();
        let server_task = tokio::spawn(async move {
            handle_connection(server_io, session_clone, shutdown_clone).await
        });

        let (client_read, client_write) = tokio::io::split(client_io);
        Self {
            read: FramedRead::new(client_read, SruiCodec::new()),
            write: FramedWrite::new(client_write, SruiCodec::new()),
            server_task,
            shutdown,
        }
    }

    async fn send_resume(&mut self, session_id: &str, last_applied: u64) {
        self.write
            .send(client_resume(session_id, last_applied))
            .await
            .expect("send ClientResume");
    }

    async fn expect_resume_ok(&mut self, expected_session_id: &str, replay_from: u64) {
        let msg = self.read.next().await.expect("resume ok frame").expect("decode");
        match msg.msg {
            Some(srui_message::Msg::ServerResumeOk(ok)) => {
                assert_eq!(ok.session_id, expected_session_id);
                assert_eq!(ok.replay_from_revision, replay_from);
                assert_eq!(ok.last_processed_event_seq, 0);
            }
            other => panic!("expected ServerResumeOk, got {:?}", other),
        }
    }

    async fn expect_no_message(&mut self, wait: Duration) {
        let next = tokio::time::timeout(wait, self.read.next()).await;
        match next {
            Ok(Some(Ok(_))) => panic!("expected no wire message during {wait:?}"),
            Ok(Some(Err(e))) => panic!("unexpected decode error: {e}"),
            Ok(None) => panic!("connection closed while expecting no message"),
            Err(_) => {}
        }
    }

    async fn close(self) {
        self.shutdown.cancel();
        let res = self.server_task.await.expect("server task join");
        assert!(res.is_ok(), "connection handler failed: {:?}", res);
    }
}

#[tokio::test]
async fn test_resume_at_current_revision_yields_empty_replay() {
    let session = Arc::new(Session::new("resume-current"));
    session.commit_transaction(make_tx(0)).unwrap();
    session.commit_transaction(make_tx(1)).unwrap();
    assert_eq!(session.current_revision(), 2);

    let mut conn = ResumeConnection::open(session).await;
    conn.send_resume("resume-current", 2).await;
    conn.expect_resume_ok("resume-current", 2).await;
    conn.expect_no_message(Duration::from_millis(100)).await;
    conn.close().await;
}

#[tokio::test]
async fn test_resume_at_earliest_retained_revision_replays_full_range() {
    let session = Arc::new(Session::new("resume-earliest"));

    // Journal capacity is 1024; one more commit evicts revision 0 from the replay window.
    for base in 0..1025 {
        session.commit_transaction(make_tx(base)).unwrap();
    }
    assert_eq!(session.current_revision(), 1025);

    let replayed = session
        .collect_replayed_transactions(1)
        .expect("replay from earliest retained revision");
    assert_eq!(replayed.len(), 1024);
    assert_eq!(replayed.first().unwrap().base_revision, 1);
    assert_eq!(replayed.last().unwrap().new_revision, 1025);

    let mut conn = ResumeConnection::open(session).await;
    conn.send_resume("resume-earliest", 1).await;
    conn.expect_resume_ok("resume-earliest", 1).await;

    let mut count = 0usize;
    let mut expected_base = 1u64;
    while expected_base < 1025 {
        let msg = conn.read.next().await.expect("replay frame").expect("decode");
        match msg.msg {
            Some(srui_message::Msg::Transaction(tx)) => {
                assert_eq!(tx.base_revision, expected_base);
                assert_eq!(tx.new_revision, expected_base + 1);
                expected_base += 1;
                count += 1;
            }
            other => panic!("expected replay transaction, got {:?}", other),
        }
    }
    assert_eq!(count, 1024);
    conn.expect_no_message(Duration::from_millis(100)).await;
    conn.close().await;
}

#[tokio::test]
async fn test_wrong_session_id_requires_replacement_resync() {
    let session = Arc::new(Session::new("resume-authoritative"));
    session.commit_transaction(make_tx(0)).unwrap();

    let mut conn = ResumeConnection::open(session).await;
    conn.send_resume("expired-session-id", 0).await;

    let msg = conn
        .read
        .next()
        .await
        .expect("resync-required frame")
        .expect("decode");
    match msg.msg {
        Some(srui_message::Msg::ServerResyncRequired(resync)) => {
            assert_eq!(resync.session_id, "resume-authoritative");
            assert_eq!(resync.snapshot_revision, 1);
            assert_eq!(
                SessionContinuity::try_from(resync.continuity),
                Ok(SessionContinuity::Replaced)
            );
            assert_eq!(resync.last_processed_event_seq, 0);
        }
        other => panic!("expected ServerResyncRequired, got {:?}", other),
    }

    let snapshot = conn
        .read
        .next()
        .await
        .expect("snapshot frame")
        .expect("decode");
    match snapshot.msg {
        Some(srui_message::Msg::Transaction(tx)) => {
            assert_eq!(tx.base_revision, 0);
            assert_eq!(tx.new_revision, 1);
        }
        other => panic!("expected snapshot transaction, got {:?}", other),
    }

    conn.close().await;
}

#[tokio::test]
async fn test_replayed_transactions_are_contiguous_and_ordered() {
    let session = Arc::new(Session::new("resume-contiguous"));
    for base in 0..5 {
        session.commit_transaction(make_tx(base)).unwrap();
    }

    let mut conn = ResumeConnection::open(session).await;
    conn.send_resume("resume-contiguous", 0).await;
    conn.expect_resume_ok("resume-contiguous", 0).await;

    let mut expected_base = 0u64;
    for _ in 0..5 {
        let msg = conn.read.next().await.expect("replay frame").expect("decode");
        match msg.msg {
            Some(srui_message::Msg::Transaction(tx)) => {
                assert_eq!(tx.base_revision, expected_base);
                assert_eq!(tx.new_revision, expected_base + 1);
                expected_base += 1;
            }
            other => panic!("expected replay transaction, got {:?}", other),
        }
    }
    assert_eq!(expected_base, 5);
    conn.close().await;
}

#[tokio::test]
async fn test_transaction_committed_during_replay_delivered_once_after_boundary() {
    let session = Arc::new(Session::new("resume-during-replay"));
    for base in 0..2 {
        session.commit_transaction(make_tx(base)).unwrap();
    }

    let barrier = Arc::new(Barrier::new(2));
    let session_for_commit = session.clone();
    let barrier_for_commit = barrier.clone();

    let shutdown = CancellationToken::new();
    let (client_io, server_io) = duplex(1024 * 1024);

    let session_clone = session.clone();
    let shutdown_clone = shutdown.clone();
    let server_task = tokio::spawn(async move {
        handle_connection(server_io, session_clone, shutdown_clone).await
    });

    let (client_read, client_write) = tokio::io::split(client_io);
    let mut read = FramedRead::new(client_read, SruiCodec::new());
    let mut write = FramedWrite::new(client_write, SruiCodec::new());

    write
        .send(client_resume("resume-during-replay", 0))
        .await
        .expect("send ClientResume");

    let commit_task = tokio::spawn(async move {
        barrier_for_commit.wait().await;
        session_for_commit
            .commit_transaction(make_tx(2))
            .expect("commit during replay");
    });

    // ServerResumeOk
    let msg = read.next().await.expect("resume ok").expect("decode");
    match msg.msg {
        Some(srui_message::Msg::ServerResumeOk(_)) => {}
        other => panic!("expected ServerResumeOk, got {:?}", other),
    }

    // First replayed transaction — release concurrent commit.
    let tx1 = read.next().await.expect("replay tx 0->1").expect("decode");
    match tx1.msg {
        Some(srui_message::Msg::Transaction(t)) => {
            assert_eq!(t.base_revision, 0);
            assert_eq!(t.new_revision, 1);
        }
        other => panic!("expected tx 0->1, got {:?}", other),
    }
    barrier.wait().await;

    // Second replayed transaction.
    let tx2 = read.next().await.expect("replay tx 1->2").expect("decode");
    match tx2.msg {
        Some(srui_message::Msg::Transaction(t)) => {
            assert_eq!(t.base_revision, 1);
            assert_eq!(t.new_revision, 2);
        }
        other => panic!("expected tx 1->2, got {:?}", other),
    }

    commit_task.await.expect("commit task");

    // Post-replay broadcast must deliver the concurrent commit exactly once.
    let tx3 = read.next().await.expect("post-replay tx 2->3").expect("decode");
    match tx3.msg {
        Some(srui_message::Msg::Transaction(t)) => {
            assert_eq!(t.base_revision, 2);
            assert_eq!(t.new_revision, 3);
        }
        other => panic!("expected tx 2->3 after replay boundary, got {:?}", other),
    }

    let duplicate = tokio::time::timeout(Duration::from_millis(100), read.next()).await;
    assert!(
        duplicate.is_err(),
        "transaction committed during replay must not be duplicated on the wire"
    );

    shutdown.cancel();
    let res = server_task.await.expect("server task join");
    assert!(res.is_ok());
}
