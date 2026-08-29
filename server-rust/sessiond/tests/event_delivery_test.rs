//! Event delivery and deduplication over the wire (§7.6, §7.7, §18.2, §27, §29, §32.4).
//!
//! Verifies `handle_connection` event path through [`Session::process_event`]:
//! handler dispatch, per-client-instance dedupe, validation rejections, revision stability,
//! handler re-entry into `Session::transaction()`, and `clear_handlers`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use tokio::io::duplex;
use tokio::time::timeout;
use tokio_util::codec::{FramedRead, FramedWrite};
use tokio_util::sync::CancellationToken;

use srui_protocol::{srui_message, ClientHello, SruiCodec, SruiMessage};
use srui_sdk::{Button, NodeId, ACTIVATE, LABEL};
use srui_semantic_tree::{Event, EventValidationError};
use srui_sessiond::{handle_connection, ConnectionError, Session, SessionError};

const CLIENT_A: &[u8] = b"client-instance-a";
const CLIENT_B: &[u8] = b"client-instance-b";

type ClientWrite =
    FramedWrite<tokio::io::WriteHalf<tokio::io::DuplexStream>, SruiCodec>;
type ClientRead = FramedRead<tokio::io::ReadHalf<tokio::io::DuplexStream>, SruiCodec>;

async fn connect_client(
    session: Arc<Session>,
    shutdown: CancellationToken,
    client_instance_id: &[u8],
) -> (ClientWrite, ClientRead, tokio::task::JoinHandle<Result<(), ConnectionError>>) {
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
            client_instance_id: client_instance_id.to_vec(),
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
    wait_until(Duration::from_secs(2), || invocations.load(Ordering::SeqCst) == 1).await;

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
    wait_until(Duration::from_secs(2), || invocations.load(Ordering::SeqCst) == 1).await;
    drain_one_transaction(&mut client_read).await;

    send_activate(&mut client_write, CLIENT_A, 2, "evt-dup", 1, btn).await;
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

    let (mut write_a, _read_a, _task_a) =
        connect_client(session.clone(), shutdown.clone(), CLIENT_A).await;
    let (mut write_b, _read_b, _task_b) =
        connect_client(session.clone(), shutdown.clone(), CLIENT_B).await;

    // Same event ID from two client instances must both dispatch.
    send_activate(&mut write_a, CLIENT_A, 1, "shared-event-id", 1, btn).await;
    send_activate(&mut write_b, CLIENT_B, 1, "shared-event-id", 1, btn).await;
    wait_until(Duration::from_secs(2), || invocations.load(Ordering::SeqCst) == 2).await;

    assert_eq!(invocations.load(Ordering::SeqCst), 2);
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
    let (mut client_write, _client_read, server_task) =
        connect_client(session.clone(), shutdown, CLIENT_A).await;

    let missing = NodeId::new(999);
    send_activate(&mut client_write, CLIENT_A, 1, "evt-missing", 1, missing).await;

    let server_result = server_task.await.expect("server task join");
    match server_result {
        Err(ConnectionError::Session(SessionError::EventValidation(
            EventValidationError::NodeNotFound(id),
        ))) => assert_eq!(id, missing),
        other => panic!("expected NodeNotFound session error, got {:?}", other),
    }

    assert_eq!(invocations.load(Ordering::SeqCst), 0);
    assert_eq!(session.current_revision(), 1);
    assert_eq!(session.node_count(), 1);
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
    let (mut client_write, _client_read, server_task) =
        connect_client(session.clone(), shutdown, CLIENT_A).await;

    send_activate(&mut client_write, CLIENT_A, 1, "evt-disabled", 1, disabled_btn).await;

    let server_result = server_task.await.expect("server task join");
    match server_result {
        Err(ConnectionError::Session(SessionError::EventValidation(
            EventValidationError::NodeDisabled(id),
        ))) => assert_eq!(id, disabled_btn),
        other => panic!("expected NodeDisabled session error, got {:?}", other),
    }

    assert_eq!(invocations.load(Ordering::SeqCst), 0);
    assert_eq!(session.current_revision(), 1);
    assert_eq!(session.node_count(), 2);
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
    let (mut client_write, _client_read, server_task) =
        connect_client(session.clone(), shutdown, CLIENT_A).await;

    send_activate(&mut client_write, CLIENT_A, 1, "evt-future", 99, btn).await;

    let server_result = server_task.await.expect("server task join");
    match server_result {
        Err(ConnectionError::Session(SessionError::EventValidation(
            EventValidationError::FutureRevision { observed, current },
        ))) => {
            assert_eq!(observed.get(), 99);
            assert_eq!(current.get(), 1);
        }
        other => panic!("expected FutureRevision session error, got {:?}", other),
    }

    assert_eq!(invocations.load(Ordering::SeqCst), 0);
    assert_eq!(session.current_revision(), 1);
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
    let (mut client_write, _client_read, server_task) =
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

    let _ = server_task.await.expect("server task join");

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
