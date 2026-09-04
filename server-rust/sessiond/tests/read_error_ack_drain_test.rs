//! Read-error acknowledgement drain regression (§18.2).
//!
//! A framing error after a settled event must not discard its already accepted acknowledgement,
//! even when the physical writer is backpressured.

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
    srui_message, ClientHello, EventAckStatus, FramingError, SruiCodec, SruiMessage,
};
use srui_sdk::{Button, NodeId, ACTIVATE};
use srui_semantic_tree::Event;
use srui_sessiond::{handle_connection, ConnectionError, Session};

const CLIENT_ID: &[u8] = b"read-error-ack-client";
const DEADLOCK: Duration = Duration::from_secs(2);

struct StreamControl {
    accepting_writes: AtomicBool,
    inject_malformed: AtomicBool,
    write_blocked: Notify,
    write_block_attempts: AtomicU64,
    malformed_delivered: Notify,
    writer_waker: AtomicWaker,
    reader_waker: AtomicWaker,
}

impl StreamControl {
    fn new() -> Self {
        Self {
            accepting_writes: AtomicBool::new(true),
            inject_malformed: AtomicBool::new(false),
            write_blocked: Notify::new(),
            write_block_attempts: AtomicU64::new(0),
            malformed_delivered: Notify::new(),
            writer_waker: AtomicWaker::new(),
            reader_waker: AtomicWaker::new(),
        }
    }

    fn block_writes(&self) {
        self.accepting_writes.store(false, Ordering::Release);
    }

    fn release_writes(&self) {
        self.accepting_writes.store(true, Ordering::Release);
        self.writer_waker.wake();
    }

    async fn wait_until_write_blocked(&self, expected_attempts: u64) {
        let wait = async {
            loop {
                if self.write_block_attempts.load(Ordering::Acquire) >= expected_attempts {
                    return;
                }
                let notified = self.write_blocked.notified();
                if self.write_block_attempts.load(Ordering::Acquire) >= expected_attempts {
                    return;
                }
                notified.await;
            }
        };
        timeout(DEADLOCK, wait)
            .await
            .expect("server never retried the gated acknowledgement write");
    }

    fn write_block_attempts(&self) -> u64 {
        self.write_block_attempts.load(Ordering::Acquire)
    }

    async fn inject_malformed_frame(&self) {
        self.inject_malformed.store(true, Ordering::Release);
        self.reader_waker.wake();
        timeout(DEADLOCK, self.malformed_delivered.notified())
            .await
            .expect("server never read the injected malformed frame");
    }

    fn poll_write_permission(&self, cx: &mut Context<'_>) -> Poll<()> {
        if self.accepting_writes.load(Ordering::Acquire) {
            return Poll::Ready(());
        }

        self.writer_waker.register(cx.waker());
        if self.accepting_writes.load(Ordering::Acquire) {
            Poll::Ready(())
        } else {
            self.write_block_attempts.fetch_add(1, Ordering::AcqRel);
            self.write_blocked.notify_one();
            Poll::Pending
        }
    }
}

struct ControlledServerStream {
    inner: DuplexStream,
    control: Arc<StreamControl>,
}

impl AsyncRead for ControlledServerStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.control.inject_malformed.swap(false, Ordering::AcqRel) {
            buf.put_slice(&[0x80; 10]);
            self.control.malformed_delivered.notify_one();
            return Poll::Ready(Ok(()));
        }

        self.control.reader_waker.register(cx.waker());
        if self.control.inject_malformed.swap(false, Ordering::AcqRel) {
            buf.put_slice(&[0x80; 10]);
            self.control.malformed_delivered.notify_one();
            return Poll::Ready(Ok(()));
        }

        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for ControlledServerStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.control.poll_write_permission(cx).is_pending() {
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

async fn recv_frame(
    read: &mut FramedRead<tokio::io::ReadHalf<DuplexStream>, SruiCodec>,
) -> SruiMessage {
    timeout(DEADLOCK, read.next())
        .await
        .expect("frame timeout")
        .expect("server closed before frame")
        .expect("decode frame")
}

#[tokio::test]
async fn framing_error_drains_an_already_accepted_event_ack() {
    let session = Arc::new(Session::new("read-error-ack"));
    let button = NodeId::new(1);
    session
        .transaction(|ui| {
            Button::builder(button).label("Click").create(ui)?;
            Ok(())
        })
        .expect("seed button");

    let invocations = Arc::new(AtomicU64::new(0));
    let invocation_count = Arc::clone(&invocations);
    session.on(button, ACTIVATE, move |_, _| {
        invocation_count.fetch_add(1, Ordering::SeqCst);
    });

    let control = Arc::new(StreamControl::new());
    let (client, server) = duplex(1024 * 1024);
    let controlled_server = ControlledServerStream {
        inner: server,
        control: Arc::clone(&control),
    };
    let server_session = Arc::clone(&session);
    let server_task = tokio::spawn(async move {
        handle_connection(controlled_server, server_session, CancellationToken::new()).await
    });

    let (client_read, client_write) = tokio::io::split(client);
    let mut read = FramedRead::new(client_read, SruiCodec::new());
    let mut write = FramedWrite::new(client_write, SruiCodec::new());

    write
        .send(SruiMessage {
            msg: Some(srui_message::Msg::ClientHello(ClientHello {
                core_version: "0.4.0".into(),
                profiles: vec!["org.srui.standard-widgets/1".into()],
                limits: None,
                client_instance_id: CLIENT_ID.to_vec(),
                client_metadata: Default::default(),
                known_resource_hashes: vec![],
            })),
        })
        .await
        .expect("send hello");
    assert!(matches!(
        recv_frame(&mut read).await.msg,
        Some(srui_message::Msg::ServerWelcome(_))
    ));
    assert!(matches!(
        recv_frame(&mut read).await.msg,
        Some(srui_message::Msg::Transaction(_))
    ));

    control.block_writes();
    let event = Event::activate(1, "evt-before-framing-error", 1, button)
        .with_client_instance_id(CLIENT_ID);
    write
        .send(SruiMessage {
            msg: Some(srui_message::Msg::Event(event.to_wire())),
        })
        .await
        .expect("send event");

    control.wait_until_write_blocked(1).await;
    assert_eq!(invocations.load(Ordering::SeqCst), 1);
    control.inject_malformed_frame().await;
    let blocked_attempts = control.write_block_attempts();
    // Reproduce the outbound-disconnect preemption: the session queue closes while the accepted
    // control ACK is still blocked in the physical writer.
    session.close_outbound();
    // Wait until the blocked send is repolled with outbound cancellation ready. The broken path
    // drops the send in that poll; the drain path keeps it pending until the gate opens.
    control.wait_until_write_blocked(blocked_attempts + 1).await;
    control.release_writes();

    let ack = match recv_frame(&mut read).await.msg {
        Some(srui_message::Msg::ServerEventAck(ack)) => ack,
        other => panic!("expected ServerEventAck, got {other:?}"),
    };
    assert_eq!(ack.event_id, b"evt-before-framing-error");
    assert_eq!(ack.status(), EventAckStatus::Processed);

    let server_result = timeout(DEADLOCK, server_task)
        .await
        .expect("server did not finish after framing error")
        .expect("server task join");
    assert!(
        matches!(
            server_result,
            Err(ConnectionError::Framing(FramingError::DecodeError(ref message)))
                if message == "malformed or overlong varint length prefix (exceeds 10 bytes)"
        ),
        "the original framing error must remain the connection result: {server_result:?}"
    );
}

#[tokio::test]
async fn framing_error_drains_an_accepted_ack_after_outbound_lag() {
    let session = Arc::new(Session::with_outbound_queue_capacity(
        "read-error-ack-lag",
        1,
    ));
    let button = NodeId::new(1);
    session
        .transaction(|ui| {
            Button::builder(button).label("Click").create(ui)?;
            Ok(())
        })
        .expect("seed button");

    let invocations = Arc::new(AtomicU64::new(0));
    let invocation_count = Arc::clone(&invocations);
    session.on(button, ACTIVATE, move |_, _| {
        invocation_count.fetch_add(1, Ordering::SeqCst);
    });

    let control = Arc::new(StreamControl::new());
    let (client, server) = duplex(1024 * 1024);
    let controlled_server = ControlledServerStream {
        inner: server,
        control: Arc::clone(&control),
    };
    let server_session = Arc::clone(&session);
    let server_task = tokio::spawn(async move {
        handle_connection(controlled_server, server_session, CancellationToken::new()).await
    });

    let (client_read, client_write) = tokio::io::split(client);
    let mut read = FramedRead::new(client_read, SruiCodec::new());
    let mut write = FramedWrite::new(client_write, SruiCodec::new());

    write
        .send(SruiMessage {
            msg: Some(srui_message::Msg::ClientHello(ClientHello {
                core_version: "0.4.0".into(),
                profiles: vec!["org.srui.standard-widgets/1".into()],
                limits: None,
                client_instance_id: CLIENT_ID.to_vec(),
                client_metadata: Default::default(),
                known_resource_hashes: vec![],
            })),
        })
        .await
        .expect("send hello");
    assert!(matches!(
        recv_frame(&mut read).await.msg,
        Some(srui_message::Msg::ServerWelcome(_))
    ));
    assert!(matches!(
        recv_frame(&mut read).await.msg,
        Some(srui_message::Msg::Transaction(_))
    ));

    // Park one UI frame in the physical writer. This leaves the one-slot outbound queue empty so
    // two later structural transactions deterministically overflow it.
    control.block_writes();
    session
        .transaction(|ui| {
            Button::builder(NodeId::new(2))
                .label("in-flight")
                .create(ui)?;
            Ok(())
        })
        .expect("queue in-flight transaction");
    control.wait_until_write_blocked(1).await;

    let event = Event::activate(
        1,
        "evt-before-outbound-lag",
        session.current_revision(),
        button,
    )
    .with_client_instance_id(CLIENT_ID);
    write
        .send(SruiMessage {
            msg: Some(srui_message::Msg::Event(event.to_wire())),
        })
        .await
        .expect("send event");
    timeout(DEADLOCK, async {
        while invocations.load(Ordering::SeqCst) != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("event handler did not run");
    control.inject_malformed_frame().await;

    for id in 3..=4u64 {
        session
            .transaction(|ui| {
                Button::builder(NodeId::new(id))
                    .label(format!("overflow-{id}"))
                    .create(ui)?;
                Ok(())
            })
            .expect("commit structural overflow transaction");
    }
    control.release_writes();

    let in_flight = recv_frame(&mut read).await;
    assert!(matches!(
        in_flight.msg,
        Some(srui_message::Msg::Transaction(_))
    ));
    let ack = match recv_frame(&mut read).await.msg {
        Some(srui_message::Msg::ServerEventAck(ack)) => ack,
        other => panic!("expected ServerEventAck after outbound lag, got {other:?}"),
    };
    assert_eq!(ack.event_id, b"evt-before-outbound-lag");
    assert_eq!(ack.status(), EventAckStatus::Processed);

    let server_result = timeout(DEADLOCK, server_task)
        .await
        .expect("server did not finish after framing error")
        .expect("server task join");
    assert!(
        matches!(
            server_result,
            Err(ConnectionError::Framing(FramingError::DecodeError(ref message)))
                if message == "malformed or overlong varint length prefix (exceeds 10 bytes)"
        ),
        "outbound lag must not mask the original framing error: {server_result:?}"
    );
}
