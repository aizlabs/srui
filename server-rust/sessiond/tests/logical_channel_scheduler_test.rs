//! End-to-end logical-channel scheduler harness (§18.2, §19.2).
//!
//! Correctness is asserted with frame counts and gated reads. `timeout` is only a deadlock guard;
//! wall-clock sleeps are not used.

use std::collections::HashSet;
use std::io;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use futures::task::AtomicWaker;
use futures::{SinkExt, StreamExt};
use tokio::io::{duplex, AsyncRead, AsyncWrite, DuplexStream, ReadBuf};
use tokio::sync::Notify;
use tokio::time::timeout;
use tokio_util::codec::{FramedRead, FramedWrite};
use tokio_util::sync::CancellationToken;

use srui_protocol::{
    operation::Op, srui_message, value::Value as WireValInner, EventAckStatus, SruiCodec,
    SruiMessage,
};
use srui_resources::CHUNK_PAYLOAD_SIZE;
use srui_sdk::{Button, NodeId, Surface, Text, ACTIVATE, TEXT};
use srui_semantic_tree::Event;
use srui_sessiond::{
    handle_connection, logical_class_for_server_envelope, LogicalChannelClass, Session,
};

type ClientRead = FramedRead<tokio::io::ReadHalf<tokio::io::DuplexStream>, SruiCodec>;
type ClientWrite = FramedWrite<tokio::io::WriteHalf<tokio::io::DuplexStream>, SruiCodec>;

const CLIENT_ID: &[u8] = b"scheduler-client";
const DEADLOCK: Duration = Duration::from_secs(5);

struct ServerWriteGate {
    accepting_writes: AtomicBool,
    blocked: Notify,
    writer_waker: AtomicWaker,
}

impl ServerWriteGate {
    fn new() -> Self {
        Self {
            accepting_writes: AtomicBool::new(true),
            blocked: Notify::new(),
            writer_waker: AtomicWaker::new(),
        }
    }

    fn block(&self) {
        self.accepting_writes.store(false, Ordering::Release);
    }

    fn release(&self) {
        self.accepting_writes.store(true, Ordering::Release);
        self.writer_waker.wake();
    }

    async fn wait_until_blocked(&self) {
        timeout(DEADLOCK, self.blocked.notified())
            .await
            .expect("server never attempted the gated resource write");
    }

    fn poll_permission(&self, cx: &mut Context<'_>) -> Poll<()> {
        if self.accepting_writes.load(Ordering::Acquire) {
            return Poll::Ready(());
        }

        self.writer_waker.register(cx.waker());
        if self.accepting_writes.load(Ordering::Acquire) {
            Poll::Ready(())
        } else {
            self.blocked.notify_one();
            Poll::Pending
        }
    }
}

struct WriteGatedStream {
    inner: DuplexStream,
    gate: Arc<ServerWriteGate>,
}

impl AsyncRead for WriteGatedStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for WriteGatedStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.gate.poll_permission(cx).is_pending() {
            return Poll::Pending;
        }
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

async fn recv_frame(read: &mut ClientRead) -> SruiMessage {
    timeout(DEADLOCK, read.next())
        .await
        .expect("deadlock waiting for frame")
        .expect("eof while waiting for frame")
        .expect("decode frame")
}

fn classify(msg: &SruiMessage) -> LogicalChannelClass {
    logical_class_for_server_envelope(msg)
}

async fn connect(
    session: Arc<Session>,
    duplex_bytes: usize,
    client_instance_id: &[u8],
) -> (
    ClientWrite,
    ClientRead,
    CancellationToken,
    tokio::task::JoinHandle<()>,
) {
    let (client, server) = duplex(duplex_bytes);
    let shutdown = CancellationToken::new();
    let shutdown_server = shutdown.clone();
    let server_task = tokio::spawn(async move {
        let _ = handle_connection(server, session, shutdown_server).await;
    });

    let (read_half, write_half) = tokio::io::split(client);
    let mut read = FramedRead::new(read_half, SruiCodec::new());
    let mut write = FramedWrite::new(write_half, SruiCodec::new());

    write
        .send(SruiMessage {
            msg: Some(srui_message::Msg::ClientHello(srui_protocol::ClientHello {
                core_version: "0.4.0".into(),
                profiles: vec!["org.srui.standard-widgets/1".into()],
                limits: None,
                client_instance_id: client_instance_id.to_vec(),
                client_metadata: Default::default(),
                known_resource_hashes: vec![],
            })),
        })
        .await
        .expect("send hello");

    let welcome = recv_frame(&mut read).await;
    assert!(matches!(
        welcome.msg,
        Some(srui_message::Msg::ServerWelcome(_))
    ));
    assert_eq!(classify(&welcome), LogicalChannelClass::Control);

    (write, read, shutdown, server_task)
}

async fn connect_with_write_gate(
    session: Arc<Session>,
    duplex_bytes: usize,
    client_instance_id: &[u8],
) -> (
    ClientWrite,
    ClientRead,
    CancellationToken,
    tokio::task::JoinHandle<()>,
    Arc<ServerWriteGate>,
) {
    let (client, server) = duplex(duplex_bytes);
    let gate = Arc::new(ServerWriteGate::new());
    let gated_server = WriteGatedStream {
        inner: server,
        gate: Arc::clone(&gate),
    };
    let shutdown = CancellationToken::new();
    let shutdown_server = shutdown.clone();
    let server_task = tokio::spawn(async move {
        handle_connection(gated_server, session, shutdown_server)
            .await
            .expect("gated server connection failed");
    });

    let (read_half, write_half) = tokio::io::split(client);
    let mut read = FramedRead::new(read_half, SruiCodec::new());
    let mut write = FramedWrite::new(write_half, SruiCodec::new());

    write
        .send(SruiMessage {
            msg: Some(srui_message::Msg::ClientHello(srui_protocol::ClientHello {
                core_version: "0.4.0".into(),
                profiles: vec!["org.srui.standard-widgets/1".into()],
                limits: None,
                client_instance_id: client_instance_id.to_vec(),
                client_metadata: Default::default(),
                known_resource_hashes: vec![],
            })),
        })
        .await
        .expect("send hello");

    let welcome = recv_frame(&mut read).await;
    assert!(matches!(
        welcome.msg,
        Some(srui_message::Msg::ServerWelcome(_))
    ));
    assert_eq!(classify(&welcome), LogicalChannelClass::Control);

    (write, read, shutdown, server_task, gate)
}

async fn drain_handshake_snapshot(read: &mut ClientRead, initial_revision: u64) {
    if initial_revision == 0 {
        return;
    }
    let snapshot = recv_frame(read).await;
    match snapshot.msg {
        Some(srui_message::Msg::Transaction(tx)) => {
            assert_eq!(tx.new_revision, initial_revision);
            assert_eq!(
                classify(&SruiMessage {
                    msg: Some(srui_message::Msg::Transaction(tx)),
                }),
                LogicalChannelClass::Ui
            );
        }
        other => panic!("expected handshake snapshot, got {other:?}"),
    }
}

fn wire_activate(
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

fn png_prefix(bytes: &mut [u8]) {
    let prefix = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
    bytes[..prefix.len()].copy_from_slice(&prefix);
}

fn is_probe_text(tx: &srui_protocol::Transaction, expected: &str) -> bool {
    tx.operations.iter().any(|op| {
        matches!(
            &op.op,
            Some(Op::SetProperty(sp)) if {
                let val = sp.value.as_ref().and_then(|v| match &v.value {
                    Some(WireValInner::StringValue(s)) => Some(s.as_str()),
                    _ => None,
                });
                val == Some(expected)
            }
        )
    })
}

async fn wait_until(mut condition: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + DEADLOCK;
    while !condition() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "deadlock waiting for condition"
        );
        tokio::task::yield_now().await;
    }
}

/// Resource saturation, then control/input/UI probes: at most the in-flight resource frame
/// precedes the probes, acks stay distinct control envelopes, and handlers run before the
/// transfer completes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resource_backlog_yields_to_control_input_and_ui_on_the_wire() {
    let session = Arc::new(Session::new("scheduler-wire-probes"));
    let button = NodeId::new(10);
    let text = NodeId::new(11);
    session
        .transaction(|ui| {
            Surface::builder(1).create(ui)?;
            Text::builder(text).parent(1).text("before").create(ui)?;
            Button::builder(button).parent(1).label("go").create(ui)?;
            Ok(())
        })
        .unwrap();

    let invocations = Arc::new(AtomicU64::new(0));
    let counter = Arc::clone(&invocations);
    session.on(button, ACTIVATE, move |_, _| {
        counter.fetch_add(1, Ordering::SeqCst);
    });

    // Small duplex: a 16 KiB resource chunk cannot fully leave the write future until the client
    // reads, so at most the already-started resource frame can precede newly queued control/UI.
    let (mut write, mut read, shutdown, server_task) =
        connect(Arc::clone(&session), 8 * 1024, CLIENT_ID).await;
    drain_handshake_snapshot(&mut read, session.current_revision()).await;

    let mut payload = vec![0u8; CHUNK_PAYLOAD_SIZE * 200];
    png_prefix(&mut payload);
    session
        .publish_resource(&payload)
        .expect("publish resource");

    let first_resource = recv_frame(&mut read).await;
    assert_eq!(classify(&first_resource), LogicalChannelClass::Resource);

    write
        .send(wire_activate(
            CLIENT_ID,
            1,
            "evt-a",
            session.current_revision(),
            button,
        ))
        .await
        .unwrap();
    write
        .send(wire_activate(
            CLIENT_ID,
            2,
            "evt-b",
            session.current_revision(),
            button,
        ))
        .await
        .unwrap();
    wait_until(|| invocations.load(Ordering::SeqCst) == 2).await;
    assert!(
        invocations.load(Ordering::SeqCst) == 2,
        "event handlers must run before the resource transfer completes"
    );

    session
        .transaction(|ui| {
            ui.set(text, TEXT, "probe-ui")?;
            Ok(())
        })
        .unwrap();

    let mut acks = Vec::new();
    let mut saw_ui_probe = false;
    let mut resource_after_first = 0usize;
    let mut frames_after_inflight: Vec<LogicalChannelClass> = Vec::new();
    let mut inflight_consumed = false;

    loop {
        let msg = recv_frame(&mut read).await;
        let class = classify(&msg);
        match &msg.msg {
            Some(srui_message::Msg::ResourceMetadata(_) | srui_message::Msg::ResourceChunk(_)) => {
                if !inflight_consumed {
                    inflight_consumed = true;
                } else {
                    resource_after_first += 1;
                    frames_after_inflight.push(class);
                }
            }
            Some(srui_message::Msg::ServerEventAck(ack)) => {
                inflight_consumed = true;
                frames_after_inflight.push(class);
                assert_eq!(
                    logical_class_for_server_envelope(&msg),
                    LogicalChannelClass::Control
                );
                acks.push(ack.clone());
            }
            Some(srui_message::Msg::Transaction(tx)) if is_probe_text(tx, "probe-ui") => {
                inflight_consumed = true;
                frames_after_inflight.push(class);
                saw_ui_probe = true;
            }
            Some(srui_message::Msg::Transaction(_)) => {
                inflight_consumed = true;
                frames_after_inflight.push(class);
            }
            other => panic!("unexpected envelope {other:?}"),
        }

        if acks.len() >= 2 && saw_ui_probe {
            break;
        }
        assert!(
            frames_after_inflight.len() < 64,
            "probes did not arrive: acks={} ui={saw_ui_probe} frames={frames_after_inflight:?}",
            acks.len()
        );
    }

    assert_eq!(acks.len(), 2, "each settled event gets its own EVENT_ACK");
    assert_ne!(acks[0].event_id, acks[1].event_id);
    let ids = [acks[0].event_id.as_slice(), acks[1].event_id.as_slice()];
    assert!(ids.contains(&b"evt-a".as_slice()));
    assert!(ids.contains(&b"evt-b".as_slice()));
    for ack in &acks {
        assert_eq!(ack.status(), EventAckStatus::Processed);
    }
    assert!(saw_ui_probe);

    let first_control = frames_after_inflight
        .iter()
        .position(|c| *c == LogicalChannelClass::Control)
        .expect("control ack");
    let first_ui = frames_after_inflight
        .iter()
        .position(|c| *c == LogicalChannelClass::Ui)
        .expect("ui probe");
    assert!(
        resource_after_first == 0
            || frames_after_inflight
                .iter()
                .take(first_control.max(first_ui) + 1)
                .filter(|c| **c == LogicalChannelClass::Resource)
                .count()
                == 0,
        "no second resource token may precede control/input/UI probes, got {frames_after_inflight:?}"
    );
    let ack_span = frames_after_inflight
        .iter()
        .enumerate()
        .filter_map(|(idx, class)| (*class == LogicalChannelClass::Control).then_some(idx))
        .collect::<Vec<_>>();
    assert!(ack_span.len() >= 2);
    assert!(
        ack_span[1] - ack_span[0] <= LogicalChannelClass::Control.max_service_gap(),
        "acks must arrive within the control service bound, indices {ack_span:?}"
    );
    assert!(
        ack_span[0] < LogicalChannelClass::Control.max_service_gap(),
        "first ack must arrive within the control bound after the in-flight resource frame"
    );

    shutdown.cancel();
    let _ = timeout(DEADLOCK, server_task).await;
}

/// A saturated control/UI flood still gives resource and UI their documented service slots.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resource_progresses_under_control_and_ui_flood() {
    const CONTROL_FLOOD_EVENTS: u64 = 96;
    const UI_FLOOD_TRANSACTIONS: u64 = 48;
    const SATURATED_CONTROL_HANDLERS: u64 = 65;

    let session = Arc::new(Session::new("scheduler-flood"));
    let button = NodeId::new(3);
    session
        .transaction(|ui| {
            Surface::builder(1).create(ui)?;
            Button::builder(button)
                .parent(1)
                .label("flood")
                .create(ui)?;
            Ok(())
        })
        .unwrap();

    let invocations = Arc::new(AtomicU64::new(0));
    let invocation_counter = Arc::clone(&invocations);
    session.on(button, ACTIVATE, move |_, _| {
        invocation_counter.fetch_add(1, Ordering::SeqCst);
    });

    let (write, mut read, shutdown, server_task, write_gate) =
        connect_with_write_gate(Arc::clone(&session), 16 * 1024, CLIENT_ID).await;
    drain_handshake_snapshot(&mut read, session.current_revision()).await;

    write_gate.block();

    let mut payload = vec![0u8; CHUNK_PAYLOAD_SIZE * 32];
    png_prefix(&mut payload);
    session
        .publish_resource(&payload)
        .expect("publish resource");
    write_gate.wait_until_blocked().await;

    // CreateNode is structural, so every transaction remains a distinct UI envelope instead of
    // coalescing into one scalar tail item. The count stays below the 128-item subscriber bound.
    let ui_base_revision = session.current_revision();
    let ui_flood = {
        let session = Arc::clone(&session);
        tokio::spawn(async move {
            for i in 0..UI_FLOOD_TRANSACTIONS {
                session
                    .transaction(|ui| {
                        Text::builder(10_000 + i)
                            .parent(1)
                            .text(format!("ui-flood-{i}"))
                            .create(ui)?;
                        Ok(())
                    })
                    .expect("commit structural UI flood transaction");
            }
        })
    };
    timeout(DEADLOCK, ui_flood)
        .await
        .expect("UI flood producer did not complete")
        .expect("UI flood producer panicked");
    assert_eq!(
        session.current_revision(),
        ui_base_revision + UI_FLOOD_TRANSACTIONS,
        "every structural UI flood transaction must commit"
    );

    let event_revision = session.current_revision();
    let control_flood = tokio::spawn(async move {
        let mut write = write;
        for seq in 1..=CONTROL_FLOOD_EVENTS {
            write
                .send(wire_activate(
                    CLIENT_ID,
                    seq,
                    &format!("e{seq}"),
                    event_revision,
                    button,
                ))
                .await
                .expect("send control-flood event");
        }
        write
    });

    // With the outbound write gated, handlers 1..=64 fill the bounded ACK channel and handler 65
    // blocks while enqueuing its ACK. This proves control is continuously ready before measuring
    // resource/UI service, instead of relying on task timing.
    wait_until(|| invocations.load(Ordering::SeqCst) == SATURATED_CONTROL_HANDLERS).await;
    assert_eq!(
        invocations.load(Ordering::SeqCst),
        SATURATED_CONTROL_HANDLERS
    );

    write_gate.release();

    let first = recv_frame(&mut read).await;
    assert_eq!(
        classify(&first),
        LogicalChannelClass::Resource,
        "the deterministically gated write must be the first resource frame"
    );

    let mut saw_metadata = false;
    let mut reconstructed = 0usize;
    match first.msg {
        Some(srui_message::Msg::ResourceMetadata(meta)) => {
            saw_metadata = true;
            assert_eq!(meta.encoded_length, payload.len() as u64);
        }
        Some(srui_message::Msg::ResourceChunk(chunk)) => {
            reconstructed += chunk.data.len();
        }
        other => panic!("expected gated resource envelope, got {other:?}"),
    }

    let mut ack_ids = HashSet::new();
    let mut ui_frames = 0u64;
    let mut resource_distance = 0usize;
    let mut ui_distance = 0usize;

    while ack_ids.len() < CONTROL_FLOOD_EVENTS as usize
        || ui_frames < UI_FLOOD_TRANSACTIONS
        || reconstructed < payload.len()
    {
        let msg = recv_frame(&mut read).await;
        let class = classify(&msg);

        if reconstructed < payload.len() {
            resource_distance += 1;
            if class == LogicalChannelClass::Resource {
                assert!(
                    resource_distance <= LogicalChannelClass::Resource.max_service_gap(),
                    "resource service distance {resource_distance} exceeded bound {}",
                    LogicalChannelClass::Resource.max_service_gap()
                );
                resource_distance = 0;
            } else {
                assert!(
                    resource_distance < LogicalChannelClass::Resource.max_service_gap(),
                    "resource missed its next service slot after {} dispatched frames",
                    LogicalChannelClass::Resource.max_service_gap()
                );
            }
        }

        if ui_frames < UI_FLOOD_TRANSACTIONS {
            ui_distance += 1;
            if class == LogicalChannelClass::Ui {
                assert!(
                    ui_distance <= LogicalChannelClass::Ui.max_service_gap(),
                    "UI service distance {ui_distance} exceeded bound {}",
                    LogicalChannelClass::Ui.max_service_gap()
                );
                ui_distance = 0;
            } else {
                assert!(
                    ui_distance < LogicalChannelClass::Ui.max_service_gap(),
                    "UI missed its next service slot after {} dispatched frames",
                    LogicalChannelClass::Ui.max_service_gap()
                );
            }
        }

        match msg.msg {
            Some(srui_message::Msg::ResourceMetadata(meta)) => {
                assert!(!saw_metadata, "resource metadata sent more than once");
                saw_metadata = true;
                assert_eq!(meta.encoded_length, payload.len() as u64);
            }
            Some(srui_message::Msg::ResourceChunk(chunk)) => {
                reconstructed += chunk.data.len();
                assert!(
                    reconstructed <= payload.len(),
                    "resource chunks exceeded the published payload"
                );
            }
            Some(srui_message::Msg::ServerEventAck(ack)) => {
                assert_eq!(ack.status(), EventAckStatus::Processed);
                assert!(
                    ack_ids.insert(ack.event_id),
                    "control flood returned a duplicate EVENT_ACK"
                );
            }
            Some(srui_message::Msg::Transaction(_)) => {
                ui_frames += 1;
                assert!(
                    ui_frames <= UI_FLOOD_TRANSACTIONS,
                    "received more UI frames than structural transactions"
                );
            }
            other => panic!("unexpected envelope {other:?}"),
        }
    }

    assert!(saw_metadata, "resource metadata was not delivered");
    assert_eq!(reconstructed, payload.len());
    assert_eq!(ui_frames, UI_FLOOD_TRANSACTIONS);
    assert_eq!(ack_ids.len(), CONTROL_FLOOD_EVENTS as usize);
    assert_eq!(
        invocations.load(Ordering::SeqCst),
        CONTROL_FLOOD_EVENTS,
        "every control-flood event handler must complete"
    );

    let _write = timeout(DEADLOCK, control_flood)
        .await
        .expect("control flood producer did not complete")
        .expect("control flood producer panicked");

    shutdown.cancel();
    timeout(DEADLOCK, server_task)
        .await
        .expect("gated server task did not stop")
        .expect("gated server task panicked");
}

/// UI still interleaves ahead of remaining resource chunks (regression from the timing-only test).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ui_transaction_interleaves_ahead_of_remaining_resource_chunks() {
    let session = Arc::new(Session::new("scheduler-ui-interleave"));
    session
        .transaction(|ui| {
            Surface::builder(1).create(ui)?;
            Text::builder(2).parent(1).text("before").create(ui)?;
            Ok(())
        })
        .unwrap();

    let (mut _write, mut read, shutdown, server_task) =
        connect(Arc::clone(&session), 8 * 1024, CLIENT_ID).await;
    drain_handshake_snapshot(&mut read, session.current_revision()).await;

    let mut payload = vec![0u8; CHUNK_PAYLOAD_SIZE * 80];
    png_prefix(&mut payload);
    session.publish_resource(&payload).expect("publish large");

    let mut saw_first_chunk = false;
    let mut resource_after_first_before_tx = 0usize;

    loop {
        let msg = recv_frame(&mut read).await;
        match msg.msg {
            Some(srui_message::Msg::ResourceChunk(_)) => {
                if !saw_first_chunk {
                    saw_first_chunk = true;
                    session
                        .transaction(|ui| {
                            ui.set(2, TEXT, "after-first-chunk")?;
                            Ok(())
                        })
                        .unwrap();
                } else {
                    resource_after_first_before_tx += 1;
                }
            }
            Some(srui_message::Msg::Transaction(tx)) if is_probe_text(&tx, "after-first-chunk") => {
                break;
            }
            _ => {}
        }
        assert!(
            resource_after_first_before_tx < 32,
            "UI probe never arrived"
        );
    }

    assert!(
        resource_after_first_before_tx <= 1,
        "at most one further resource chunk may precede the transaction, got {resource_after_first_before_tx}"
    );

    shutdown.cancel();
    let _ = timeout(DEADLOCK, server_task).await;
}
