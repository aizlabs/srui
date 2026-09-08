//! Event delivery and deduplication over the wire (§7.6, §7.7, §18.2, §27, §29, §32.4).
//!
//! Verifies `handle_connection` event path through [`Session::process_event`]:
//! handler dispatch, per-client-instance dedupe, validation rejections, revision stability,
//! handler re-entry into `Session::transaction()`, and `clear_handlers`.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use tokio::io::duplex;
use tokio::time::timeout;
use tokio_util::codec::{FramedRead, FramedWrite};
use tokio_util::sync::CancellationToken;

use srui_protocol::{
    srui_message, ClientHello, ClientResume, EventAckStatus, ServerEventAck, ServerResumeOk,
    SruiCodec, SruiMessage,
};
use srui_sdk::{Button, NodeId, ACTIVATE, LABEL, TEXT_EDIT};
use srui_semantic_tree::Event;
use srui_sessiond::{handle_connection, ConnectionError, EventOutcome, Session, SessionError};

const CLIENT_A: &[u8] = b"client-instance-a";
const CLIENT_B: &[u8] = b"client-instance-b";

type ClientWrite = FramedWrite<tokio::io::WriteHalf<tokio::io::DuplexStream>, SruiCodec>;
type ClientRead = FramedRead<tokio::io::ReadHalf<tokio::io::DuplexStream>, SruiCodec>;

async fn connect_client(
    session: Arc<Session>,
    shutdown: CancellationToken,
    client_instance_id: &[u8],
) -> (
    ClientWrite,
    ClientRead,
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

    let hello = SruiMessage {
        msg: Some(srui_message::Msg::ClientHello(ClientHello {
            core_version: "0.5.0".to_string(),
            profiles: vec!["org.srui.standard-widgets/1".to_string()],
            limits: None,
            client_instance_id: client_instance_id.to_vec(),
            client_metadata: Default::default(),
            known_resource_hashes: vec![],
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
            if w.initial_revision > 0 {
                let snapshot = client_framed_read
                    .next()
                    .await
                    .expect("hello catch-up snapshot")
                    .expect("decode snapshot");
                match snapshot.msg {
                    Some(srui_message::Msg::Transaction(tx)) => {
                        assert_eq!(tx.base_revision, 0);
                        assert_eq!(tx.new_revision, w.initial_revision);
                    }
                    other => panic!("expected hello catch-up Transaction, got {other:?}"),
                }
            }
        }
        other => panic!("expected ServerWelcome, got {:?}", other),
    }

    (client_framed_write, client_framed_read, server_task)
}

/// Reconnects an existing `client_instance_id` through `CLIENT RESUME` (§18, §18.2).
///
/// `last_acked_event_seq` is only a client retention hint; the server answers with its own
/// contiguous settled frontier.
async fn resume_client(
    session: Arc<Session>,
    shutdown: CancellationToken,
    client_instance_id: &[u8],
    last_applied_revision: u64,
    last_acked_event_seq: u64,
) -> (
    ClientWrite,
    ClientRead,
    tokio::task::JoinHandle<Result<(), ConnectionError>>,
    ServerResumeOk,
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

    let resume = SruiMessage {
        msg: Some(srui_message::Msg::ClientResume(ClientResume {
            session_id: session.session_id(),
            client_instance_id: client_instance_id.to_vec(),
            last_applied_revision,
            last_acked_event_seq,
            terminal_stream_offsets: Default::default(),
            limits: None,
            known_resource_hashes: vec![],
            pending_text_edits: vec![],
        })),
    };
    client_framed_write.send(resume).await.expect("send resume");

    let resume_msg = timeout(Duration::from_secs(2), client_framed_read.next())
        .await
        .expect("timed out waiting for resume response")
        .expect("receive resume response")
        .expect("decode resume response");
    let resume_ok = match resume_msg.msg {
        Some(srui_message::Msg::ServerResumeOk(ok)) => ok,
        other => panic!("expected ServerResumeOk, got {other:?}"),
    };

    (
        client_framed_write,
        client_framed_read,
        server_task,
        resume_ok,
    )
}

fn wire_activate_event(
    client_instance_id: &[u8],
    event_seq: u64,
    event_id: &str,
    observed_revision: u64,
    node_id: NodeId,
) -> SruiMessage {
    let event = Event::activate(event_seq, event_id, observed_revision, node_id)
        .with_client_instance_id(client_instance_id);
    SruiMessage {
        msg: Some(srui_message::Msg::Event(event.to_wire())),
    }
}

async fn wait_until(timeout: Duration, mut condition: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + timeout;
    while !condition() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for condition"
        );
        tokio::task::yield_now().await;
    }
}

async fn drain_one_transaction(read: &mut ClientRead) {
    let msg = timeout(Duration::from_secs(2), read.next())
        .await
        .expect("timed out waiting for transaction broadcast")
        .expect("stream ended while waiting for transaction")
        .expect("decode transaction frame");
    match msg.msg {
        Some(srui_message::Msg::Transaction(_)) => {}
        other => panic!("expected Transaction envelope, got {:?}", other),
    }
}

/// Reads the `SERVER EVENT_ACK` settling the event just sent on this connection (§18.2).
///
/// The ack always precedes any transaction the handler commits: the handler runs inline in
/// `process_event`, so the commit is merely queued on the broadcast channel while the ack is
/// written straight back to this connection.
async fn recv_event_ack(read: &mut ClientRead) -> ServerEventAck {
    let msg = timeout(Duration::from_secs(2), read.next())
        .await
        .expect("timed out waiting for event ack")
        .expect("stream ended while waiting for event ack")
        .expect("decode event ack frame");
    match msg.msg {
        Some(srui_message::Msg::ServerEventAck(ack)) => ack,
        other => panic!("expected ServerEventAck envelope, got {:?}", other),
    }
}

/// Closes the client end and asserts the connection loop exited cleanly rather than with an error.
///
/// A rejected event must leave the stream usable (§18.2), so the only way this connection ends is
/// the client hanging up.
async fn assert_connection_survived(
    write: ClientWrite,
    read: ClientRead,
    server_task: tokio::task::JoinHandle<Result<(), ConnectionError>>,
) {
    drop(write);
    drop(read);
    let result = timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task timed out after a rejected event")
        .expect("server task join");
    assert!(
        result.is_ok(),
        "a rejected event must not tear down the connection, got {:?}",
        result
    );
}

async fn assert_no_pending_frame(read: &mut ClientRead) {
    let pending = timeout(Duration::from_millis(50), read.next())
        .await
        .ok()
        .and_then(|r| r.transpose().ok());
    assert!(pending.is_none(), "unexpected frame: {:?}", pending);
}

async fn send_activate(
    write: &mut ClientWrite,
    client_instance_id: &[u8],
    event_seq: u64,
    event_id: &str,
    observed_revision: u64,
    node_id: NodeId,
) {
    write
        .send(wire_activate_event(
            client_instance_id,
            event_seq,
            event_id,
            observed_revision,
            node_id,
        ))
        .await
        .expect("send event");
}

fn seed_button(session: &Session, btn: NodeId) {
    session
        .transaction(|ui| {
            Button::builder(btn).label("Initial").create(ui)?;
            Ok(())
        })
        .expect("seed button");
    assert_eq!(session.current_revision(), 1);
}

#[tokio::test]
async fn test_valid_event_invokes_handler_once() {
    let session = Arc::new(Session::new("valid-event-once"));
    let btn = NodeId::new(1);
    seed_button(&session, btn);

    let invocations = Arc::new(AtomicU64::new(0));
    let counter = Arc::clone(&invocations);
    session.on(btn, ACTIVATE, move |_, _| {
        counter.fetch_add(1, Ordering::SeqCst);
    });

    let shutdown = CancellationToken::new();
    let (mut client_write, _client_read, _server_task) =
        connect_client(session.clone(), shutdown, CLIENT_A).await;

    send_activate(&mut client_write, CLIENT_A, 1, "evt-valid", 1, btn).await;
    wait_until(Duration::from_secs(2), || {
        invocations.load(Ordering::SeqCst) == 1
    })
    .await;

    assert_eq!(invocations.load(Ordering::SeqCst), 1);
    assert_eq!(session.current_revision(), 1);
}

#[tokio::test]
async fn test_duplicate_event_id_same_client_no_repeat_side_effects() {
    let session = Arc::new(Session::new("duplicate-event"));
    let btn = NodeId::new(1);
    seed_button(&session, btn);

    let invocations = Arc::new(AtomicU64::new(0));
    let counter = Arc::clone(&invocations);
    session.on(btn, ACTIVATE, move |ctx, _| {
        counter.fetch_add(1, Ordering::SeqCst);
        ctx.transaction(|ui| {
            ui.set(btn, LABEL, "Clicked")?;
            Ok(())
        })
        .expect("handler transaction");
    });

    let shutdown = CancellationToken::new();
    let (mut client_write, mut client_read, _server_task) =
        connect_client(session.clone(), shutdown, CLIENT_A).await;

    send_activate(&mut client_write, CLIENT_A, 1, "evt-dup", 1, btn).await;
    wait_until(Duration::from_secs(2), || {
        invocations.load(Ordering::SeqCst) == 1
    })
    .await;
    let first_ack = recv_event_ack(&mut client_read).await;
    assert_eq!(first_ack.status(), EventAckStatus::Processed);
    assert_eq!(first_ack.revision_after_effect, 2);
    drain_one_transaction(&mut client_read).await;

    send_activate(&mut client_write, CLIENT_A, 1, "evt-dup", 1, btn).await;
    // The replay is settled from the result cache with the *prior* outcome (§18.2), and no
    // transaction follows because the handler did not run again.
    let dup_ack = recv_event_ack(&mut client_read).await;
    assert_eq!(dup_ack.status(), EventAckStatus::Duplicate);
    assert_eq!(dup_ack.event_id, b"evt-dup");
    assert_eq!(dup_ack.revision_after_effect, 2);
    assert_eq!(dup_ack.last_processed_event_seq, 1);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_no_pending_frame(&mut client_read).await;

    assert_eq!(invocations.load(Ordering::SeqCst), 1);
    assert_eq!(session.current_revision(), 2);

    session.with_store(|store| {
        let b = Button::from_store(store, btn).unwrap();
        assert_eq!(b.label(store), Some("Clicked"));
    });
}

#[tokio::test]
async fn test_lost_ack_is_answered_from_the_result_cache_after_reconnect() {
    let session = Arc::new(Session::new("lost-ack-reconnect"));
    let btn = NodeId::new(1);
    seed_button(&session, btn);

    let invocations = Arc::new(AtomicU64::new(0));
    let counter = Arc::clone(&invocations);
    session.on(btn, ACTIVATE, move |ctx, _| {
        counter.fetch_add(1, Ordering::SeqCst);
        ctx.transaction(|ui| {
            ui.set(btn, LABEL, "Clicked")?;
            Ok(())
        })
        .expect("handler transaction");
    });

    let shutdown = CancellationToken::new();
    let (mut write_1, read_1, task_1) =
        connect_client(session.clone(), shutdown.clone(), CLIENT_A).await;

    send_activate(&mut write_1, CLIENT_A, 1, "evt-lost-ack", 1, btn).await;
    wait_until(Duration::from_secs(2), || {
        invocations.load(Ordering::SeqCst) == 1
    })
    .await;

    // The connection dies before the client can read the acknowledgement: from the client's point
    // of view the event is still unsettled, so its retry set keeps it (§18.2).
    drop(write_1);
    drop(read_1);
    let _ = timeout(Duration::from_secs(2), task_1)
        .await
        .expect("first connection did not finish");

    let (mut write_2, mut read_2, task_2, resume_ok) = resume_client(
        session.clone(),
        shutdown.clone(),
        CLIENT_A,
        1,
        // The client never saw the ack, so its own retention hint is still 0. The server's
        // frontier is authoritative and reports the settlement it actually performed.
        0,
    )
    .await;
    assert_eq!(resume_ok.session_id, session.session_id());
    assert_eq!(resume_ok.last_processed_event_seq, 1);
    drain_one_transaction(&mut read_2).await;

    // The retry carries byte-identical identity, so it is answered from the result cache rather
    // than clicking the button a second time.
    send_activate(&mut write_2, CLIENT_A, 1, "evt-lost-ack", 1, btn).await;
    let replay_ack = recv_event_ack(&mut read_2).await;
    assert_eq!(replay_ack.status(), EventAckStatus::Duplicate);
    assert_eq!(replay_ack.event_id, b"evt-lost-ack");
    assert_eq!(replay_ack.client_instance_id, CLIENT_A);
    assert_eq!(replay_ack.revision_after_effect, 2);
    assert_eq!(replay_ack.last_processed_event_seq, 1);
    assert_no_pending_frame(&mut read_2).await;

    assert_eq!(invocations.load(Ordering::SeqCst), 1);
    assert_eq!(session.current_revision(), 2);

    shutdown.cancel();
    assert!(task_2.await.expect("server task join").is_ok());
}

#[tokio::test]
async fn test_two_event_ids_for_the_same_handler_both_execute() {
    let session = Arc::new(Session::new("distinct-event-ids"));
    let btn = NodeId::new(1);
    seed_button(&session, btn);

    let invocations = Arc::new(AtomicU64::new(0));
    let counter = Arc::clone(&invocations);
    session.on(btn, ACTIVATE, move |_, _| {
        counter.fetch_add(1, Ordering::SeqCst);
    });

    let shutdown = CancellationToken::new();
    let (mut client_write, mut client_read, _server_task) =
        connect_client(session.clone(), shutdown, CLIENT_A).await;

    // Deduplication is keyed on `event_id`, not on the target node: two distinct presses of the
    // same button are two distinct actions (§18.2).
    send_activate(&mut client_write, CLIENT_A, 1, "evt-press-1", 1, btn).await;
    let first_ack = recv_event_ack(&mut client_read).await;
    assert_eq!(first_ack.status(), EventAckStatus::Processed);
    assert_eq!(first_ack.event_id, b"evt-press-1");
    assert_eq!(first_ack.last_processed_event_seq, 1);

    send_activate(&mut client_write, CLIENT_A, 2, "evt-press-2", 1, btn).await;
    let second_ack = recv_event_ack(&mut client_read).await;
    assert_eq!(second_ack.status(), EventAckStatus::Processed);
    assert_eq!(second_ack.event_id, b"evt-press-2");
    assert_eq!(second_ack.last_processed_event_seq, 2);

    assert_eq!(invocations.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn test_event_client_instance_must_match_handshake() {
    let session = Arc::new(Session::new("bound-client-instance"));
    let btn = NodeId::new(1);
    seed_button(&session, btn);

    let shutdown = CancellationToken::new();
    let (mut write_b, _read_b, task_b) =
        connect_client(session.clone(), shutdown.clone(), CLIENT_B).await;

    // A connection handshaken as B cannot poison A's cumulative sequence high-water mark.
    send_activate(&mut write_b, CLIENT_A, 10_000, "evt-spoof", 1, btn).await;
    let mismatch = timeout(Duration::from_secs(2), task_b)
        .await
        .expect("server did not reject mismatched client instance")
        .expect("server task join");
    assert!(matches!(
        mismatch,
        Err(ConnectionError::ClientInstanceMismatch)
    ));

    let (mut write_a, mut read_a, task_a) =
        connect_client(session.clone(), shutdown.clone(), CLIENT_A).await;
    send_activate(&mut write_a, CLIENT_A, 1, "evt-a", 1, btn).await;
    let ack = recv_event_ack(&mut read_a).await;
    assert_eq!(ack.status(), EventAckStatus::Processed);
    assert_eq!(ack.last_processed_event_seq, 1);

    shutdown.cancel();
    assert!(task_a.await.expect("server task join").is_ok());
}

#[tokio::test]
async fn test_different_client_instances_dedupe_isolated() {
    let session = Arc::new(Session::new("multi-client-dedupe"));
    let btn = NodeId::new(1);
    seed_button(&session, btn);

    let invocations = Arc::new(AtomicU64::new(0));
    let counter = Arc::clone(&invocations);
    session.on(btn, ACTIVATE, move |_, _| {
        counter.fetch_add(1, Ordering::SeqCst);
    });

    let shutdown = CancellationToken::new();

    let (mut write_a, mut read_a, _task_a) =
        connect_client(session.clone(), shutdown.clone(), CLIENT_A).await;
    let (mut write_b, mut read_b, _task_b) =
        connect_client(session.clone(), shutdown.clone(), CLIENT_B).await;

    // Same event ID from two client instances must both dispatch.
    send_activate(&mut write_a, CLIENT_A, 1, "shared-event-id", 1, btn).await;
    send_activate(&mut write_b, CLIENT_B, 1, "shared-event-id", 1, btn).await;
    wait_until(Duration::from_secs(2), || {
        invocations.load(Ordering::SeqCst) == 2
    })
    .await;

    assert_eq!(invocations.load(Ordering::SeqCst), 2);

    // Each connection is acked for its own event, and the sequence high-water mark is scoped to
    // the `client_instance_id` just like the dedupe window (§18, §18.2).
    let ack_a = recv_event_ack(&mut read_a).await;
    assert_eq!(ack_a.status(), EventAckStatus::Processed);
    assert_eq!(ack_a.client_instance_id, CLIENT_A);
    assert_eq!(ack_a.last_processed_event_seq, 1);

    let ack_b = recv_event_ack(&mut read_b).await;
    assert_eq!(ack_b.status(), EventAckStatus::Processed);
    assert_eq!(ack_b.client_instance_id, CLIENT_B);
    assert_eq!(ack_b.last_processed_event_seq, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_in_flight_replay_is_not_acknowledged_as_settled() {
    let session = Arc::new(Session::new("in-flight-replay"));
    let btn = NodeId::new(1);
    seed_button(&session, btn);

    let started = Arc::new(AtomicBool::new(false));
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    let handler_started = Arc::clone(&started);
    let handler_release = Arc::clone(&release);
    session.on(btn, ACTIVATE, move |_, _| {
        handler_started.store(true, Ordering::SeqCst);
        let (lock, wake) = &*handler_release;
        let mut released = lock.lock().unwrap_or_else(|error| error.into_inner());
        while !*released {
            released = wake
                .wait(released)
                .unwrap_or_else(|error| error.into_inner());
        }
    });

    let shutdown = CancellationToken::new();
    let (mut write_a, mut read_a, task_a) =
        connect_client(session.clone(), shutdown.clone(), CLIENT_A).await;
    let (mut write_overlap, mut read_overlap, task_overlap) =
        connect_client(session.clone(), shutdown.clone(), CLIENT_A).await;

    send_activate(&mut write_a, CLIENT_A, 1, "evt-in-flight", 1, btn).await;
    wait_until(Duration::from_secs(2), || started.load(Ordering::SeqCst)).await;

    send_activate(&mut write_overlap, CLIENT_A, 1, "evt-in-flight", 1, btn).await;
    // This following event proves the overlap connection processed the replay. If the replay had
    // received a bogus terminal ack, it would be the next frame instead of this probe's ack.
    send_activate(
        &mut write_overlap,
        CLIENT_A,
        2,
        "evt-probe",
        1,
        NodeId::new(999),
    )
    .await;
    let probe_ack = recv_event_ack(&mut read_overlap).await;
    assert_eq!(probe_ack.event_id, b"evt-probe");
    assert_eq!(probe_ack.status(), EventAckStatus::Rejected);
    // Sequence 1 is still in flight, so settling sequence 2 cannot cross the gap.
    assert_eq!(probe_ack.last_processed_event_seq, 0);
    assert_no_pending_frame(&mut read_overlap).await;

    let (lock, wake) = &*release;
    *lock.lock().unwrap_or_else(|error| error.into_inner()) = true;
    wake.notify_all();

    let first_ack = recv_event_ack(&mut read_a).await;
    assert_eq!(first_ack.status(), EventAckStatus::Processed);
    assert_eq!(first_ack.revision_after_effect, 1);
    // Once sequence 1 settles, the contiguous frontier jumps across already-settled sequence 2.
    assert_eq!(first_ack.last_processed_event_seq, 2);

    send_activate(&mut write_overlap, CLIENT_A, 1, "evt-in-flight", 1, btn).await;
    let settled_replay = recv_event_ack(&mut read_overlap).await;
    assert_eq!(settled_replay.status(), EventAckStatus::Duplicate);
    assert_eq!(settled_replay.revision_after_effect, 1);
    assert_eq!(settled_replay.last_processed_event_seq, 2);

    shutdown.cancel();
    assert!(task_a.await.expect("server task join").is_ok());
    assert!(task_overlap.await.expect("server task join").is_ok());
}

#[tokio::test]
async fn test_ack_sequence_advances_monotonically_across_events() {
    let session = Arc::new(Session::new("ack-seq-monotonic"));
    let btn = NodeId::new(1);
    seed_button(&session, btn);

    let shutdown = CancellationToken::new();
    let (mut client_write, mut client_read, _server_task) =
        connect_client(session.clone(), shutdown, CLIENT_A).await;

    for seq in 1..=3u64 {
        send_activate(
            &mut client_write,
            CLIENT_A,
            seq,
            &format!("evt-{seq}"),
            1,
            btn,
        )
        .await;
        let ack = recv_event_ack(&mut client_read).await;
        assert_eq!(ack.status(), EventAckStatus::Processed);
        assert_eq!(ack.event_id, format!("evt-{seq}").as_bytes());
        assert_eq!(ack.last_processed_event_seq, seq);
    }
}

#[tokio::test]
async fn test_missing_node_event_rejected_without_mutation() {
    let session = Arc::new(Session::new("missing-node"));
    let btn = NodeId::new(1);
    seed_button(&session, btn);

    let invocations = Arc::new(AtomicU64::new(0));
    let counter = Arc::clone(&invocations);
    session.on(btn, ACTIVATE, move |_, _| {
        counter.fetch_add(1, Ordering::SeqCst);
    });

    let shutdown = CancellationToken::new();
    let (mut client_write, mut client_read, server_task) =
        connect_client(session.clone(), shutdown, CLIENT_A).await;

    let missing = NodeId::new(999);
    send_activate(&mut client_write, CLIENT_A, 1, "evt-missing", 1, missing).await;

    // §18.2: the event is settled as REJECTED, not answered by closing the connection — a torn
    // connection would be resumed and the same invalid event replayed forever.
    let ack = recv_event_ack(&mut client_read).await;
    assert_eq!(ack.status(), EventAckStatus::Rejected);
    assert_eq!(ack.event_id, b"evt-missing");
    assert_eq!(ack.last_processed_event_seq, 1);
    assert!(
        ack.reject_reason.contains("999"),
        "reject_reason should name the missing node, got {:?}",
        ack.reject_reason
    );

    let original_reason = ack.reject_reason.clone();
    send_activate(&mut client_write, CLIENT_A, 1, "evt-missing", 1, missing).await;
    let replay_ack = recv_event_ack(&mut client_read).await;
    assert_eq!(replay_ack.status(), EventAckStatus::Rejected);
    assert_eq!(replay_ack.reject_reason, original_reason);

    assert_eq!(invocations.load(Ordering::SeqCst), 0);
    assert_eq!(session.current_revision(), 1);
    assert_eq!(session.node_count(), 1);

    assert_connection_survived(client_write, client_read, server_task).await;
}

#[tokio::test]
async fn malformed_events_are_rejected_without_tearing_down_the_connection() {
    let session = Arc::new(Session::new("malformed-events"));
    let btn = NodeId::new(1);
    seed_button(&session, btn);

    let invocations = Arc::new(AtomicU64::new(0));
    let counter = Arc::clone(&invocations);
    session.on(btn, ACTIVATE, move |_, _| {
        counter.fetch_add(1, Ordering::SeqCst);
    });

    let shutdown = CancellationToken::new();
    let (mut client_write, mut client_read, server_task) =
        connect_client(session.clone(), shutdown, CLIENT_A).await;

    let mut missing_event_type = Event::activate(1, "missing-type", 1, btn)
        .with_client_instance_id(CLIENT_A)
        .to_wire();
    missing_event_type.event_type = None;

    let mut missing_edit_seq = Event::activate(2, "missing-edit-seq", 1, btn)
        .with_client_instance_id(CLIENT_A)
        .to_wire();
    missing_edit_seq.event_type = Some(TEXT_EDIT.into());
    missing_edit_seq.edit_seq = 0;

    let mut unexpected_edit_seq = Event::activate(3, "unexpected-edit-seq", 1, btn)
        .with_client_instance_id(CLIENT_A)
        .to_wire();
    unexpected_edit_seq.edit_seq = 1;

    let mut undecodable_argument = Event::activate(4, "undecodable-argument", 1, btn)
        .with_client_instance_id(CLIENT_A)
        .to_wire();
    undecodable_argument
        .arguments
        .push(srui_protocol::Property {
            property: Some(LABEL.into()),
            value: Some(srui_protocol::Value { value: None }),
        });

    let mut oversized_event_id = Event::activate(5, "placeholder", 1, btn)
        .with_client_instance_id(CLIENT_A)
        .to_wire();
    oversized_event_id.event_id = vec![0x41; srui_semantic_tree::MAX_EVENT_ID_BYTES + 1];

    let malformed = [
        missing_event_type,
        missing_edit_seq,
        unexpected_edit_seq,
        undecodable_argument,
        oversized_event_id,
    ];
    for (index, event) in malformed.iter().enumerate() {
        client_write
            .send(SruiMessage {
                msg: Some(srui_message::Msg::Event(event.clone())),
            })
            .await
            .expect("send malformed event");
        let ack = recv_event_ack(&mut client_read).await;
        assert_eq!(ack.status(), EventAckStatus::Rejected);
        assert_eq!(ack.last_processed_event_seq, (index + 1) as u64);
        if event.event_id.len() > srui_semantic_tree::MAX_EVENT_ID_BYTES {
            assert!(ack.event_id.len() <= srui_semantic_tree::MAX_EVENT_ID_BYTES);
            assert_ne!(ack.event_id, event.event_id);
        } else {
            assert_eq!(ack.event_id, event.event_id);
        }
        assert!(
            ack.reject_reason.contains("malformed event:"),
            "unexpected rejection: {:?}",
            ack.reject_reason
        );

        if index == 0 {
            client_write
                .send(SruiMessage {
                    msg: Some(srui_message::Msg::Event(event.clone())),
                })
                .await
                .expect("replay malformed event");
            let replay_ack = recv_event_ack(&mut client_read).await;
            assert_eq!(replay_ack.status(), EventAckStatus::Rejected);
            assert_eq!(replay_ack.reject_reason, ack.reject_reason);
            assert_eq!(replay_ack.last_processed_event_seq, 1);
        }
    }

    send_activate(
        &mut client_write,
        CLIENT_A,
        6,
        "valid-after-malformed",
        1,
        btn,
    )
    .await;
    let ack = recv_event_ack(&mut client_read).await;
    assert_eq!(ack.status(), EventAckStatus::Processed);
    assert_eq!(ack.last_processed_event_seq, 6);
    assert_eq!(invocations.load(Ordering::SeqCst), 1);

    assert_connection_survived(client_write, client_read, server_task).await;
}

#[tokio::test]
async fn test_disabled_node_event_rejected_without_mutation() {
    let session = Arc::new(Session::new("disabled-node"));
    let enabled_btn = NodeId::new(1);
    let disabled_btn = NodeId::new(2);

    session
        .transaction(|ui| {
            Button::builder(enabled_btn).label("Enabled").create(ui)?;
            Button::builder(disabled_btn)
                .label("Disabled")
                .enabled(false)
                .create(ui)?;
            Ok(())
        })
        .expect("seed buttons");

    let invocations = Arc::new(AtomicU64::new(0));
    let counter = Arc::clone(&invocations);
    session.on(disabled_btn, ACTIVATE, move |_, _| {
        counter.fetch_add(1, Ordering::SeqCst);
    });

    let shutdown = CancellationToken::new();
    let (mut client_write, mut client_read, server_task) =
        connect_client(session.clone(), shutdown, CLIENT_A).await;

    send_activate(
        &mut client_write,
        CLIENT_A,
        1,
        "evt-disabled",
        1,
        disabled_btn,
    )
    .await;

    let ack = recv_event_ack(&mut client_read).await;
    assert_eq!(ack.status(), EventAckStatus::Rejected);
    assert_eq!(ack.event_id, b"evt-disabled");
    assert!(
        ack.reject_reason.contains("disabled"),
        "reject_reason should name the refusal, got {:?}",
        ack.reject_reason
    );

    assert_eq!(invocations.load(Ordering::SeqCst), 0);
    assert_eq!(session.current_revision(), 1);
    assert_eq!(session.node_count(), 2);

    assert_connection_survived(client_write, client_read, server_task).await;
}

#[tokio::test]
async fn test_future_revision_event_rejected_without_mutation() {
    let session = Arc::new(Session::new("future-revision"));
    let btn = NodeId::new(1);
    seed_button(&session, btn);

    let invocations = Arc::new(AtomicU64::new(0));
    let counter = Arc::clone(&invocations);
    session.on(btn, ACTIVATE, move |_, _| {
        counter.fetch_add(1, Ordering::SeqCst);
    });

    let shutdown = CancellationToken::new();
    let (mut client_write, mut client_read, server_task) =
        connect_client(session.clone(), shutdown, CLIENT_A).await;

    send_activate(&mut client_write, CLIENT_A, 1, "evt-future", 99, btn).await;

    let ack = recv_event_ack(&mut client_read).await;
    assert_eq!(ack.status(), EventAckStatus::Rejected);
    assert_eq!(ack.event_id, b"evt-future");
    assert_eq!(ack.revision_after_effect, 1);
    assert!(
        ack.reject_reason.contains("99"),
        "reject_reason should name the future revision, got {:?}",
        ack.reject_reason
    );

    assert_eq!(invocations.load(Ordering::SeqCst), 0);
    assert_eq!(session.current_revision(), 1);

    assert_connection_survived(client_write, client_read, server_task).await;
}

#[tokio::test]
async fn test_rejected_events_do_not_advance_revision() {
    let session = Arc::new(Session::new("rejected-no-revision-bump"));
    let btn = NodeId::new(1);
    seed_button(&session, btn);

    // Handler would advance revision if invoked.
    session.on(btn, ACTIVATE, move |ctx, _| {
        ctx.transaction(|ui| {
            ui.set(btn, LABEL, "Mutated")?;
            Ok(())
        })
        .expect("handler transaction");
    });

    let baseline_revision = session.current_revision();
    let baseline_journal = session
        .collect_replayed_transactions(0)
        .expect("journal replay");

    let shutdown = CancellationToken::new();
    let (mut client_write, mut client_read, server_task) =
        connect_client(session.clone(), shutdown, CLIENT_A).await;

    send_activate(
        &mut client_write,
        CLIENT_A,
        1,
        "evt-future-reject",
        baseline_revision + 50,
        btn,
    )
    .await;

    assert_eq!(
        recv_event_ack(&mut client_read).await.status(),
        EventAckStatus::Rejected
    );
    assert_connection_survived(client_write, client_read, server_task).await;

    assert_eq!(session.current_revision(), baseline_revision);
    let journal_after = session
        .collect_replayed_transactions(0)
        .expect("journal replay after rejection");
    assert_eq!(journal_after, baseline_journal);

    session.with_store(|store| {
        let b = Button::from_store(store, btn).unwrap();
        assert_eq!(b.label(store), Some("Initial"));
    });
}

#[tokio::test]
async fn test_handler_reentry_transaction_no_deadlock() {
    let session = Arc::new(Session::new("handler-reentry"));
    let btn = NodeId::new(1);
    seed_button(&session, btn);

    session.on(btn, ACTIVATE, move |ctx, _| {
        ctx.transaction(|ui| {
            ui.set(btn, LABEL, "Handled")?;
            Ok(())
        })
        .expect("nested transaction in handler");
    });

    let shutdown = CancellationToken::new();
    let (mut client_write, mut client_read, server_task) =
        connect_client(session.clone(), shutdown, CLIENT_A).await;

    send_activate(&mut client_write, CLIENT_A, 1, "evt-reentry", 1, btn).await;
    assert_eq!(
        recv_event_ack(&mut client_read).await.status(),
        EventAckStatus::Processed
    );
    drain_one_transaction(&mut client_read).await;

    assert_eq!(session.current_revision(), 2);
    session.with_store(|store| {
        let b = Button::from_store(store, btn).unwrap();
        assert_eq!(b.label(store), Some("Handled"));
    });

    // Connection must remain healthy (no deadlock during handler re-entry).
    drop(client_write);
    drop(client_read);
    let server_result = timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task timed out after handler re-entry")
        .expect("server task join");
    assert!(
        server_result.is_ok(),
        "server connection failed after handler re-entry: {:?}",
        server_result
    );
}

#[test]
fn test_handler_panic_abandons_in_flight_admission_for_retry() {
    let session = Session::new("handler-panic-retry");
    let btn = NodeId::new(1);
    seed_button(&session, btn);
    session.on(btn, ACTIVATE, |_, _| panic!("handler failed"));

    let event = Event::activate(1, "evt-panic", 1, btn)
        .with_client_instance_id(CLIENT_A)
        .to_wire();
    assert!(matches!(
        session.process_event(&event),
        Err(SessionError::Panicked(message)) if message == "handler failed"
    ));

    session.clear_handlers();
    let invocations = Arc::new(AtomicU64::new(0));
    let counter = Arc::clone(&invocations);
    session.on(btn, ACTIVATE, move |_, _| {
        counter.fetch_add(1, Ordering::SeqCst);
    });

    assert!(matches!(
        session.process_event(&event),
        Ok(EventOutcome::Processed {
            last_processed_event_seq: 1,
            ..
        })
    ));
    assert_eq!(invocations.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn fallible_handler_error_is_rejected_without_tearing_down_the_connection() {
    let session = Arc::new(Session::new("fallible-handler-rejection"));
    let btn = NodeId::new(1);
    seed_button(&session, btn);
    session.on_result(btn, ACTIVATE, |_, _| {
        Err(SessionError::InvalidInput("handler failed".to_string()))
    });

    let shutdown = CancellationToken::new();
    let (mut client_write, mut client_read, server_task) =
        connect_client(session, shutdown, CLIENT_A).await;

    send_activate(&mut client_write, CLIENT_A, 1, "evt-error", 1, btn).await;
    let first_ack = recv_event_ack(&mut client_read).await;
    assert_eq!(first_ack.status(), EventAckStatus::Rejected);
    assert!(first_ack.reject_reason.contains("handler failed"));

    send_activate(&mut client_write, CLIENT_A, 1, "evt-error", 1, btn).await;
    let replay_ack = recv_event_ack(&mut client_read).await;
    assert_eq!(replay_ack.status(), EventAckStatus::Rejected);
    assert_eq!(replay_ack.reject_reason, first_ack.reject_reason);
    assert_eq!(replay_ack.last_processed_event_seq, 1);

    assert_connection_survived(client_write, client_read, server_task).await;
}

#[tokio::test]
async fn test_clearing_handlers_stops_dispatch() {
    let session = Arc::new(Session::new("clear-handlers"));
    let btn = NodeId::new(1);
    seed_button(&session, btn);

    let invocations = Arc::new(AtomicU64::new(0));
    let counter = Arc::clone(&invocations);
    session.on(btn, ACTIVATE, move |_, _| {
        counter.fetch_add(1, Ordering::SeqCst);
    });

    session.clear_handlers();
    assert_eq!(session.handler_count(btn, ACTIVATE), 0);

    let shutdown = CancellationToken::new();
    let (mut client_write, _client_read, _server_task) =
        connect_client(session.clone(), shutdown, CLIENT_A).await;

    send_activate(&mut client_write, CLIENT_A, 1, "evt-after-clear", 1, btn).await;
    tokio::time::sleep(Duration::from_millis(50)).await;

    assert_eq!(invocations.load(Ordering::SeqCst), 0);
    assert_eq!(session.current_revision(), 1);
}
