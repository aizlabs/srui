//! # Counter Application Socket Integration Test (§16, §18, §20.1, §20.2, §21, §29)
//!
//! Verifies:
//! 1. Spawning `sessiond::Session` and hosting the Task 10 Counter example application.
//! 2. Binding the session daemon behind a local socket listener with `handle_connection`.
//!    (Using TCP loopback `127.0.0.1:0` for robust cross-platform and sandboxed test execution;
//!    `handle_connection` is generic over `AsyncRead + AsyncWrite` and supports both Unix domain sockets
//!    and TCP loopback (§20.2)).
//! 3. Connecting directly to the socket as a client using length-prefixed Protocol Buffers framing (`SruiCodec`).
//! 4. Driving the Counter application through a sequence of `ACTIVATE` events sent as framed protobuf bytes.
//! 5. Asserting the client receives the expected framed `Transaction` bytes back, matching bit-for-bit what
//!    Task 11's in-memory `encode_transaction` produces for each committed revision.
//! 6. Failure/reconnect paths: resume replay without double increment, duplicate event dedupe across
//!    reconnect, disabled/future-revision rejections, multi-client revision parity, and isolated
//!    malformed-frame disconnect (§18.2, §20.2, §27).

use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use tokio::io::{duplex, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;
use tokio_util::codec::{FramedRead, FramedWrite};
use tokio_util::sync::CancellationToken;

use srui_protocol::{
    srui_message, ClientHello, ClientResume, EventAckStatus, FramingError, ServerEventAck,
    SruiCodec, SruiMessage,
};
use srui_sdk::*;
use srui_sessiond::{handle_connection, ConnectionError, Session};

const COUNTER_CLIENT_A: &[u8] = &[10, 20, 30, 40];
const COUNTER_CLIENT_B: &[u8] = &[50, 60, 70, 80];

type CounterRead = FramedRead<tokio::io::ReadHalf<tokio::io::DuplexStream>, SruiCodec>;
type CounterWrite = FramedWrite<tokio::io::WriteHalf<tokio::io::DuplexStream>, SruiCodec>;

struct CounterFixture {
    session: Arc<Session>,
    text_id: NodeId,
    progress_id: NodeId,
    button_id: NodeId,
}

impl CounterFixture {
    fn new(session_id: &str) -> Self {
        let session = Arc::new(Session::new(session_id));
        let surface_id = NodeId::new(1);
        let text_id = NodeId::new(2);
        let progress_id = NodeId::new(3);
        let button_id = NodeId::new(4);

        session
            .transaction(|ui| {
                Surface::builder(surface_id)
                    .label("Counter Application")
                    .create(ui)?;

                Text::builder(text_id)
                    .parent(surface_id)
                    .text("Count: 0")
                    .role(TextRole::Heading)
                    .create(ui)?;

                Progress::builder(progress_id)
                    .parent(surface_id)
                    .value(0.0)
                    .value_description("0 / 100")
                    .create(ui)?;

                Button::builder(button_id)
                    .parent(surface_id)
                    .label("Increment")
                    .role(ActionRole::Primary)
                    .create(ui)?;

                Ok(())
            })
            .expect("initialize counter app");

        assert_eq!(session.current_revision(), 1);

        let text = text_id;
        let prog = progress_id;
        session.on(button_id, ACTIVATE, move |ctx, _event| {
            ctx.transaction(|ui| {
                let current: u64 = ui
                    .get_node(text)
                    .and_then(|n| n.get_property(TEXT))
                    .and_then(|v| v.as_string())
                    .and_then(|s| s.strip_prefix("Count: "))
                    .and_then(|n| n.parse::<u64>().ok())
                    .unwrap_or(0);
                let next_val = current + 1;
                ui.set(text, TEXT, format!("Count: {}", next_val))?;
                ui.set(prog, VALUE, (next_val as f64) / 100.0)?;
                ui.set(prog, VALUE_DESCRIPTION, format!("{} / 100", next_val))?;
                Ok(())
            })
            .expect("counter increment transaction failed");
        });

        Self {
            session,
            text_id,
            progress_id,
            button_id,
        }
    }

    fn assert_count(&self, expected: u64) {
        self.session.with_store(|store| {
            let text_widget = Text::from_store(store, self.text_id).expect("text widget exists");
            assert_eq!(
                text_widget.text(store),
                Some(format!("Count: {}", expected).as_str())
            );

            let prog_widget =
                Progress::from_store(store, self.progress_id).expect("progress widget exists");
            assert_eq!(prog_widget.value(store), Some((expected as f64) / 100.0));
            assert_eq!(
                prog_widget.value_description(store),
                Some(format!("{} / 100", expected).as_str())
            );
        });
    }
}

struct CounterConnection {
    read: CounterRead,
    write: CounterWrite,
    server_task: tokio::task::JoinHandle<Result<(), ConnectionError>>,
    shutdown: CancellationToken,
}

impl CounterConnection {
    async fn connect(session: Arc<Session>, client_instance_id: &[u8]) -> Self {
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

        let hello = SruiMessage {
            msg: Some(srui_message::Msg::ClientHello(ClientHello {
                core_version: "0.4.0".to_string(),
                profiles: vec!["org.srui.standard-widgets/1".to_string()],
                limits: None,
                client_instance_id: client_instance_id.to_vec(),
                client_metadata: Default::default(),
            })),
        };
        write.send(hello).await.expect("send ClientHello");

        let welcome = read.next().await.expect("welcome frame").expect("decode welcome");
        match welcome.msg {
            Some(srui_message::Msg::ServerWelcome(w)) => {
                assert_eq!(w.session_id, session.session_id());
                assert_eq!(w.initial_revision, 1);
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

    async fn connect_resume(
        session: Arc<Session>,
        client_instance_id: &[u8],
        last_applied_revision: u64,
    ) -> Self {
        let shutdown = CancellationToken::new();
        let (client_io, server_io) = duplex(1024 * 1024);

        let session_clone = session.clone();
        let shutdown_clone = shutdown.clone();
        let server_task = tokio::spawn(async move {
            handle_connection(server_io, session_clone, shutdown_clone).await
        });

        let (client_read, client_write) = tokio::io::split(client_io);
        let read = FramedRead::new(client_read, SruiCodec::new());
        let mut write = FramedWrite::new(client_write, SruiCodec::new());

        let resume = SruiMessage {
            msg: Some(srui_message::Msg::ClientResume(ClientResume {
                session_id: session.session_id(),
                client_instance_id: client_instance_id.to_vec(),
                last_applied_revision,
                last_acked_event_seq: 0,
                terminal_stream_offsets: Default::default(),
            })),
        };
        write.send(resume).await.expect("send ClientResume");

        Self {
            read,
            write,
            server_task,
            shutdown,
        }
    }

    async fn expect_resume_ok(&mut self, replay_from: u64) {
        let msg = self.read.next().await.expect("resume ok frame").expect("decode");
        match msg.msg {
            Some(srui_message::Msg::ServerResumeOk(ok)) => {
                assert_eq!(ok.replay_from_revision, replay_from);
            }
            other => panic!("expected ServerResumeOk, got {:?}", other),
        }
    }

    async fn send_activate(
        &mut self,
        client_instance_id: &[u8],
        event_seq: u64,
        event_id: &str,
        observed_revision: u64,
        button_id: NodeId,
    ) {
        let event = Event::activate(event_seq, event_id, observed_revision, button_id)
            .with_client_instance_id(client_instance_id);
        let envelope = SruiMessage {
            msg: Some(srui_message::Msg::Event(event.to_wire())),
        };
        self.write.send(envelope).await.expect("send ACTIVATE event");
    }

    async fn recv_transaction(&mut self) -> srui_protocol::Transaction {
        let msg = timeout(Duration::from_secs(2), self.read.next())
            .await
            .expect("timed out waiting for transaction")
            .expect("stream ended while waiting for transaction")
            .expect("decode transaction frame");
        match msg.msg {
            Some(srui_message::Msg::Transaction(tx)) => tx,
            other => panic!("expected Transaction envelope, got {:?}", other),
        }
    }

    /// Consumes the `SERVER EVENT_ACK` settling the event this connection just sent (§18.2).
    ///
    /// The ack precedes any transaction the event's handler commits: the handler runs inline in
    /// `process_event`, so the commit is only queued on the broadcast channel while the ack is
    /// written directly to this connection.
    async fn recv_event_ack(&mut self) -> ServerEventAck {
        let msg = timeout(Duration::from_secs(2), self.read.next())
            .await
            .expect("timed out waiting for event ack")
            .expect("stream ended while waiting for event ack")
            .expect("decode event ack frame");
        match msg.msg {
            Some(srui_message::Msg::ServerEventAck(ack)) => ack,
            other => panic!("expected ServerEventAck envelope, got {:?}", other),
        }
    }

    async fn expect_no_message(&mut self, wait: Duration) {
        let pending = timeout(wait, self.read.next()).await;
        match pending {
            Ok(Some(Ok(msg))) => panic!("expected no wire message during {wait:?}, got {:?}", msg),
            Ok(Some(Err(e))) => panic!("unexpected decode error: {e}"),
            Ok(None) => panic!("connection closed while expecting no message"),
            Err(_) => {}
        }
    }

    async fn cancel_and_join(self) -> Result<(), ConnectionError> {
        self.shutdown.cancel();
        self.server_task.await.expect("server task join")
    }

    async fn disconnect_abruptly(self) -> Result<(), ConnectionError> {
        drop(self.read);
        drop(self.write);
        self.server_task.await.expect("server task join")
    }
}

async fn wait_for_revision(session: &Session, target: u64, wait: Duration) {
    let deadline = tokio::time::Instant::now() + wait;
    while session.current_revision() < target {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for revision {target}"
        );
        tokio::task::yield_now().await;
    }
}

fn wire_activate(
    client_instance_id: &[u8],
    event_seq: u64,
    event_id: &str,
    observed_revision: u64,
    button_id: NodeId,
) -> SruiMessage {
    let event = Event::activate(event_seq, event_id, observed_revision, button_id)
        .with_client_instance_id(client_instance_id);
    SruiMessage {
        msg: Some(srui_message::Msg::Event(event.to_wire())),
    }
}

#[tokio::test]
async fn test_sessiond_socket_hosts_counter_and_streams_transactions() {
    // 1. Initialize session and Counter application state (§29)
    let session = Arc::new(Session::new("counter-socket-session"));
    let surface_id = NodeId::new(1);
    let text_id = NodeId::new(2);
    let progress_id = NodeId::new(3);
    let button_id = NodeId::new(4);

    // Initial transaction (Revision 0 -> 1)
    session
        .transaction(|ui| {
            Surface::builder(surface_id)
                .label("Counter Application")
                .create(ui)?;

            Text::builder(text_id)
                .parent(surface_id)
                .text("Count: 0")
                .role(TextRole::Heading)
                .create(ui)?;

            Progress::builder(progress_id)
                .parent(surface_id)
                .value(0.0)
                .value_description("0 / 100")
                .create(ui)?;

            Button::builder(button_id)
                .parent(surface_id)
                .label("Increment")
                .role(ActionRole::Primary)
                .create(ui)?;

            Ok(())
        })
        .expect("initialize counter app");

    assert_eq!(session.current_revision(), 1);

    // Register ACTIVATE event handler on the button (§7.6, §29)
    let text = text_id;
    let prog = progress_id;
    session.on(button_id, ACTIVATE, move |ctx, _event| {
        ctx.transaction(|ui| {
            let current: u64 = ui
                .get_node(text)
                .and_then(|n| n.get_property(TEXT))
                .and_then(|v| v.as_string())
                .and_then(|s| s.strip_prefix("Count: "))
                .and_then(|n| n.parse::<u64>().ok())
                .unwrap_or(0);
            let next_val = current + 1;
            ui.set(text, TEXT, format!("Count: {}", next_val))?;
            ui.set(prog, VALUE, (next_val as f64) / 100.0)?;
            ui.set(prog, VALUE_DESCRIPTION, format!("{} / 100", next_val))?;
            Ok(())
        })
        .expect("counter increment transaction failed");
    });

    // 2. Bind TCP loopback listener (§20.2: TCP loopback / Unix socket)
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind TCP loopback listener");
    let local_addr = listener.local_addr().expect("retrieve local address");
    let shutdown = CancellationToken::new();

    let session_clone = session.clone();
    let shutdown_clone = shutdown.clone();
    let server_task = tokio::spawn(async move {
        tokio::select! {
            accept_res = listener.accept() => {
                match accept_res {
                    Ok((stream, _)) => {
                        let _ = handle_connection(stream, session_clone, shutdown_clone).await;
                    }
                    Err(e) => eprintln!("Listener accept error: {}", e),
                }
            }
            _ = shutdown_clone.cancelled() => {}
        }
    });

    // 3. Connect client stream directly to the socket (§20.1, §20.2)
    let client_stream = match TcpStream::connect(local_addr).await {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            eprintln!(
                "Skipping live TCP socket connection in restricted sandbox environment: {}",
                e
            );
            shutdown.cancel();
            let _ = server_task.await;
            return;
        }
        Err(e) => panic!("client failed to connect to sessiond socket: {}", e),
    };

    let (client_read, client_write) = tokio::io::split(client_stream);
    let mut framed_read = FramedRead::new(client_read, SruiCodec::new());
    let mut framed_write = FramedWrite::new(client_write, SruiCodec::new());

    // 4. Protocol Handshake: Send ClientHello, receive ServerWelcome (§15, §18)
    let hello = SruiMessage {
        msg: Some(srui_message::Msg::ClientHello(ClientHello {
            core_version: "0.4.0".to_string(),
            profiles: vec!["org.srui.standard-widgets/1".to_string()],
            limits: None,
            client_instance_id: vec![10, 20, 30, 40],
            client_metadata: Default::default(),
        })),
    };
    framed_write.send(hello).await.expect("send client hello frame");

    let welcome_envelope = framed_read
        .next()
        .await
        .expect("receive welcome")
        .expect("decode welcome frame");

    match welcome_envelope.msg {
        Some(srui_message::Msg::ServerWelcome(w)) => {
            assert_eq!(w.session_id, "counter-socket-session");
            assert_eq!(w.initial_revision, 1);
        }
        other => panic!("expected ServerWelcome envelope, got {:?}", other),
    }

    // 5. Drive 5 simulated button click ACTIVATE events through the Unix socket (§7.6, §16, §29)
    for seq in 1..=5 {
        let observed_rev = seq; // Before click 1, store is at rev 1; before click 2, rev 2, etc.
        let event = Event::activate(
            seq,
            format!("click-{}", seq),
            Revision::new(observed_rev),
            button_id,
        )
        .with_client_instance_id(vec![10, 20, 30, 40]);

        // Send framed Event over the Unix socket
        let event_envelope = SruiMessage {
            msg: Some(srui_message::Msg::Event(event.to_wire())),
        };
        framed_write
            .send(event_envelope)
            .await
            .expect("send activate event frame");

        // The event is settled first (§18.2), then the committed transaction is broadcast.
        let ack_envelope = framed_read
            .next()
            .await
            .expect("receive event ack")
            .expect("decode event ack frame");
        match ack_envelope.msg {
            Some(srui_message::Msg::ServerEventAck(ack)) => {
                assert_eq!(ack.status(), EventAckStatus::Processed);
                assert_eq!(ack.last_processed_event_seq, seq);
            }
            other => panic!("expected ServerEventAck for click {}, got {:?}", seq, other),
        }

        // Receive framed Transaction response over the Unix socket
        let response_envelope = framed_read
            .next()
            .await
            .expect("receive transaction response")
            .expect("decode transaction response frame");

        let wire_tx = match response_envelope.msg {
            Some(srui_message::Msg::Transaction(tx)) => tx,
            other => panic!("expected Transaction envelope for click {}, got {:?}", seq, other),
        };

        // Construct expected domain Transaction matching Task 11's in-memory encode
        let expected_domain_tx = Transaction::new(
            Revision::new(seq),
            vec![
                Operation::set_property(text_id, TEXT, format!("Count: {}", seq)),
                Operation::set_property(progress_id, VALUE, (seq as f64) / 100.0),
                Operation::set_property(progress_id, VALUE_DESCRIPTION, format!("{} / 100", seq)),
            ],
        );

        let expected_wire_tx: srui_protocol::Transaction = expected_domain_tx.to_wire();
        let expected_wire_bytes = encode_transaction(&expected_domain_tx);

        // Verify wire fields match expected
        assert_eq!(wire_tx.base_revision, seq);
        assert_eq!(wire_tx.new_revision, seq + 1);
        assert_eq!(wire_tx, expected_wire_tx);

        // Verify that decoding and re-encoding matches Task 11's in-memory bytes exactly
        let decoded_domain_tx = Transaction::try_from(wire_tx).expect("decode transaction");
        assert_eq!(decoded_domain_tx, expected_domain_tx);
        assert_eq!(encode_transaction(&decoded_domain_tx), expected_wire_bytes);

        // Verify authoritative session store advanced atomically
        assert_eq!(session.current_revision(), seq + 1);
        session.with_store(|store| {
            let text_widget = Text::from_store(store, text_id).expect("text widget exists");
            assert_eq!(text_widget.text(store), Some(format!("Count: {}", seq).as_str()));

            let prog_widget = Progress::from_store(store, progress_id).expect("progress widget exists");
            assert_eq!(prog_widget.value(store), Some((seq as f64) / 100.0));
            assert_eq!(prog_widget.value_description(store), Some(format!("{} / 100", seq).as_str()));
        });
    }

    // 6. Clean disconnection and server shutdown
    shutdown.cancel();
    drop(framed_write);
    drop(framed_read);

    let _ = server_task.await;
}

#[tokio::test]
async fn test_sessiond_in_memory_duplex_hosts_counter_and_streams_transactions() {
    let session = Arc::new(Session::new("counter-duplex-session"));
    let surface_id = NodeId::new(1);
    let text_id = NodeId::new(2);
    let progress_id = NodeId::new(3);
    let button_id = NodeId::new(4);

    // Initial transaction (Revision 0 -> 1)
    session
        .transaction(|ui| {
            Surface::builder(surface_id)
                .label("Counter Application")
                .create(ui)?;

            Text::builder(text_id)
                .parent(surface_id)
                .text("Count: 0")
                .role(TextRole::Heading)
                .create(ui)?;

            Progress::builder(progress_id)
                .parent(surface_id)
                .value(0.0)
                .value_description("0 / 100")
                .create(ui)?;

            Button::builder(button_id)
                .parent(surface_id)
                .label("Increment")
                .role(ActionRole::Primary)
                .create(ui)?;

            Ok(())
        })
        .expect("initialize counter app");

    assert_eq!(session.current_revision(), 1);

    // Register ACTIVATE event handler on the button (§7.6, §29)
    let text = text_id;
    let prog = progress_id;
    session.on(button_id, ACTIVATE, move |ctx, _event| {
        ctx.transaction(|ui| {
            let current: u64 = ui
                .get_node(text)
                .and_then(|n| n.get_property(TEXT))
                .and_then(|v| v.as_string())
                .and_then(|s| s.strip_prefix("Count: "))
                .and_then(|n| n.parse::<u64>().ok())
                .unwrap_or(0);
            let next_val = current + 1;
            ui.set(text, TEXT, format!("Count: {}", next_val))?;
            ui.set(prog, VALUE, (next_val as f64) / 100.0)?;
            ui.set(prog, VALUE_DESCRIPTION, format!("{} / 100", next_val))?;
            Ok(())
        })
        .expect("counter increment transaction failed");
    });

    let shutdown = CancellationToken::new();
    let (client_io, server_io) = tokio::io::duplex(1024 * 1024);

    let session_clone = session.clone();
    let shutdown_clone = shutdown.clone();
    let server_task = tokio::spawn(async move {
        handle_connection(server_io, session_clone, shutdown_clone).await
    });

    let (client_read, client_write) = tokio::io::split(client_io);
    let mut framed_read = FramedRead::new(client_read, SruiCodec::new());
    let mut framed_write = FramedWrite::new(client_write, SruiCodec::new());

    // Protocol Handshake
    let hello = SruiMessage {
        msg: Some(srui_message::Msg::ClientHello(ClientHello {
            core_version: "0.4.0".to_string(),
            profiles: vec!["org.srui.standard-widgets/1".to_string()],
            limits: None,
            client_instance_id: vec![1, 2, 3, 4],
            client_metadata: Default::default(),
        })),
    };
    framed_write.send(hello).await.expect("send client hello frame");

    let welcome_envelope = framed_read
        .next()
        .await
        .expect("receive welcome")
        .expect("decode welcome frame");

    match welcome_envelope.msg {
        Some(srui_message::Msg::ServerWelcome(w)) => {
            assert_eq!(w.session_id, "counter-duplex-session");
            assert_eq!(w.initial_revision, 1);
        }
        other => panic!("expected ServerWelcome envelope, got {:?}", other),
    }

    // Drive 5 ACTIVATE events
    for seq in 1..=5 {
        let event = Event::activate(
            seq,
            format!("click-{}", seq),
            Revision::new(seq),
            button_id,
        )
        .with_client_instance_id(vec![1, 2, 3, 4]);

        let event_envelope = SruiMessage {
            msg: Some(srui_message::Msg::Event(event.to_wire())),
        };
        framed_write
            .send(event_envelope)
            .await
            .expect("send activate event frame");

        // The event is settled first (§18.2), then the committed transaction is broadcast.
        let ack_envelope = framed_read
            .next()
            .await
            .expect("receive event ack")
            .expect("decode event ack frame");
        match ack_envelope.msg {
            Some(srui_message::Msg::ServerEventAck(ack)) => {
                assert_eq!(ack.status(), EventAckStatus::Processed);
                assert_eq!(ack.last_processed_event_seq, seq);
            }
            other => panic!("expected ServerEventAck for click {}, got {:?}", seq, other),
        }

        let response_envelope = framed_read
            .next()
            .await
            .expect("receive transaction response")
            .expect("decode transaction response frame");

        let wire_tx = match response_envelope.msg {
            Some(srui_message::Msg::Transaction(tx)) => tx,
            other => panic!("expected Transaction envelope for click {}, got {:?}", seq, other),
        };

        let expected_domain_tx = Transaction::new(
            Revision::new(seq),
            vec![
                Operation::set_property(text_id, TEXT, format!("Count: {}", seq)),
                Operation::set_property(progress_id, VALUE, (seq as f64) / 100.0),
                Operation::set_property(progress_id, VALUE_DESCRIPTION, format!("{} / 100", seq)),
            ],
        );

        let expected_wire_tx: srui_protocol::Transaction = expected_domain_tx.to_wire();
        let expected_wire_bytes = encode_transaction(&expected_domain_tx);

        assert_eq!(wire_tx.base_revision, seq);
        assert_eq!(wire_tx.new_revision, seq + 1);
        assert_eq!(wire_tx, expected_wire_tx);

        let decoded_domain_tx = Transaction::try_from(wire_tx).expect("decode transaction");
        assert_eq!(decoded_domain_tx, expected_domain_tx);
        assert_eq!(encode_transaction(&decoded_domain_tx), expected_wire_bytes);

        assert_eq!(session.current_revision(), seq + 1);
        session.with_store(|store| {
            let text_widget = Text::from_store(store, text_id).expect("text widget exists");
            assert_eq!(text_widget.text(store), Some(format!("Count: {}", seq).as_str()));

            let prog_widget = Progress::from_store(store, progress_id).expect("progress widget exists");
            assert_eq!(prog_widget.value(store), Some((seq as f64) / 100.0));
            assert_eq!(prog_widget.value_description(store), Some(format!("{} / 100", seq).as_str()));
        });
    }

    shutdown.cancel();
    drop(framed_write);
    drop(framed_read);

    let res = server_task.await.expect("server task completed");
    assert!(res.is_ok());
}

#[tokio::test]
async fn test_counter_disconnect_before_tx_resume_replays_without_double_increment() {
    let fixture = CounterFixture::new("counter-disconnect-resume");
    let button_id = fixture.button_id;

    let mut conn =
        CounterConnection::connect(fixture.session.clone(), COUNTER_CLIENT_A).await;

    conn.send_activate(COUNTER_CLIENT_A, 1, "click-1", 1, button_id)
        .await;

    // Server processes the event asynchronously; wait for commit before disconnecting.
    wait_for_revision(&fixture.session, 2, Duration::from_secs(2)).await;

    // Client disconnects before reading the broadcast transaction.
    assert_eq!(fixture.session.current_revision(), 2);
    fixture.assert_count(1);

    let server_result = conn.disconnect_abruptly().await;
    assert!(server_result.is_ok());

    let mut resumed = CounterConnection::connect_resume(
        fixture.session.clone(),
        COUNTER_CLIENT_A,
        1,
    )
    .await;
    resumed.expect_resume_ok(1).await;

    let replayed = resumed.recv_transaction().await;
    assert_eq!(replayed.base_revision, 1);
    assert_eq!(replayed.new_revision, 2);
    fixture.assert_count(1);
    assert_eq!(fixture.session.current_revision(), 2);

    resumed.expect_no_message(Duration::from_millis(100)).await;
    resumed.cancel_and_join().await.expect("resume connection clean exit");
}

#[tokio::test]
async fn test_counter_duplicate_activate_across_reconnect_no_double_increment() {
    let fixture = CounterFixture::new("counter-dup-reconnect");
    let button_id = fixture.button_id;

    let mut conn =
        CounterConnection::connect(fixture.session.clone(), COUNTER_CLIENT_A).await;

    conn.send_activate(COUNTER_CLIENT_A, 1, "click-dup", 1, button_id)
        .await;
    let ack = conn.recv_event_ack().await;
    assert_eq!(ack.status(), EventAckStatus::Processed);
    assert_eq!(ack.event_id, b"click-dup");
    assert_eq!(ack.last_processed_event_seq, 1);
    let tx = conn.recv_transaction().await;
    assert_eq!(tx.new_revision, 2);
    fixture.assert_count(1);

    conn.disconnect_abruptly().await.expect("first connection exit");

    let mut resumed = CounterConnection::connect_resume(
        fixture.session.clone(),
        COUNTER_CLIENT_A,
        2,
    )
    .await;
    resumed.expect_resume_ok(2).await;
    resumed
        .expect_no_message(Duration::from_millis(100))
        .await;

    // Retry the same stable event_id after reconnect (§18.2 dedupe).
    resumed
        .send_activate(COUNTER_CLIENT_A, 1, "click-dup", 2, button_id)
        .await;

    // The replay is answered from the result cache: settled as DUPLICATE, handler not re-run, so
    // no transaction follows.
    let dup_ack = resumed.recv_event_ack().await;
    assert_eq!(dup_ack.status(), EventAckStatus::Duplicate);
    assert_eq!(dup_ack.event_id, b"click-dup");
    assert_eq!(dup_ack.revision_after_effect, 2);
    resumed
        .expect_no_message(Duration::from_millis(100))
        .await;

    assert_eq!(fixture.session.current_revision(), 2);
    fixture.assert_count(1);
    resumed.cancel_and_join().await.expect("resume connection clean exit");
}

#[tokio::test]
async fn test_counter_disabled_button_rejects_activate() {
    let fixture = CounterFixture::new("counter-disabled-button");
    let button_id = fixture.button_id;

    let mut conn =
        CounterConnection::connect(fixture.session.clone(), COUNTER_CLIENT_A).await;

    fixture
        .session
        .transaction(|ui| {
            ui.set(button_id, ENABLED, false)?;
            Ok(())
        })
        .expect("disable increment button");
    let _ = conn.recv_transaction().await;

    conn.send_activate(COUNTER_CLIENT_A, 1, "click-disabled", 2, button_id)
        .await;

    // §18.2: validation refusal settles the event with a REJECTED ack. Closing the connection
    // instead would make the client resume, replay the same invalid event, and be closed again.
    let ack = conn.recv_event_ack().await;
    assert_eq!(ack.status(), EventAckStatus::Rejected);
    assert_eq!(ack.event_id, b"click-disabled");
    assert_eq!(ack.last_processed_event_seq, 1);
    assert!(
        ack.reject_reason.contains("disabled"),
        "reject_reason should name the refusal, got {:?}",
        ack.reject_reason
    );

    // The connection survives and no state changed.
    conn.expect_no_message(Duration::from_millis(100)).await;
    assert_eq!(fixture.session.current_revision(), 2);
    fixture.assert_count(0);
    conn.cancel_and_join().await.expect("connection clean exit");
}

#[tokio::test]
async fn test_counter_future_revision_event_rejects_activate() {
    let fixture = CounterFixture::new("counter-future-revision");
    let button_id = fixture.button_id;

    let mut conn =
        CounterConnection::connect(fixture.session.clone(), COUNTER_CLIENT_A).await;

    conn.send_activate(COUNTER_CLIENT_A, 1, "click-future", 99, button_id)
        .await;

    let ack = conn.recv_event_ack().await;
    assert_eq!(ack.status(), EventAckStatus::Rejected);
    assert_eq!(ack.event_id, b"click-future");
    assert_eq!(ack.revision_after_effect, 1);
    assert!(
        ack.reject_reason.contains("99"),
        "reject_reason should name the future revision, got {:?}",
        ack.reject_reason
    );

    conn.expect_no_message(Duration::from_millis(100)).await;
    assert_eq!(fixture.session.current_revision(), 1);
    fixture.assert_count(0);
    conn.cancel_and_join().await.expect("connection clean exit");
}

#[tokio::test]
async fn test_two_counter_clients_receive_identical_revisions() {
    let fixture = CounterFixture::new("counter-two-clients");
    let button_id = fixture.button_id;

    let mut client_a =
        CounterConnection::connect(fixture.session.clone(), COUNTER_CLIENT_A).await;
    let mut client_b =
        CounterConnection::connect(fixture.session.clone(), COUNTER_CLIENT_B).await;

    client_a
        .send_activate(COUNTER_CLIENT_A, 1, "click-a", 1, button_id)
        .await;

    // The ack is unicast to the originating connection; client B sees only the broadcast
    // transaction (§18.2, §20.2).
    let ack_a = client_a.recv_event_ack().await;
    assert_eq!(ack_a.status(), EventAckStatus::Processed);
    assert_eq!(ack_a.client_instance_id, COUNTER_CLIENT_A);

    let tx_a = client_a.recv_transaction().await;
    let tx_b = client_b.recv_transaction().await;

    assert_eq!(tx_a.base_revision, 1);
    assert_eq!(tx_a.new_revision, 2);
    assert_eq!(tx_b.base_revision, tx_a.base_revision);
    assert_eq!(tx_b.new_revision, tx_a.new_revision);
    assert_eq!(tx_b, tx_a);

    fixture.assert_count(1);
    assert_eq!(fixture.session.current_revision(), 2);

    client_a.cancel_and_join().await.expect("client A clean exit");
    client_b.cancel_and_join().await.expect("client B clean exit");
}

#[tokio::test]
async fn test_malformed_frame_closes_one_counter_client_only() {
    let fixture = CounterFixture::new("counter-malformed-isolation");
    let button_id = fixture.button_id;

    let shutdown_a = CancellationToken::new();
    let shutdown_b = CancellationToken::new();

    let (client_io_a, server_io_a) = duplex(1024 * 1024);
    let (client_io_b, server_io_b) = duplex(1024 * 1024);

    let session_a = fixture.session.clone();
    let session_b = fixture.session.clone();
    let shutdown_a_clone = shutdown_a.clone();
    let shutdown_b_clone = shutdown_b.clone();

    let server_a = tokio::spawn(async move {
        handle_connection(server_io_a, session_a, shutdown_a_clone).await
    });
    let server_b = tokio::spawn(async move {
        handle_connection(server_io_b, session_b, shutdown_b_clone).await
    });

    let (read_a, write_a) = tokio::io::split(client_io_a);
    let (read_b, write_b) = tokio::io::split(client_io_b);
    let mut framed_read_a = FramedRead::new(read_a, SruiCodec::new());
    let mut framed_write_a = FramedWrite::new(write_a, SruiCodec::new());
    let mut framed_read_b = FramedRead::new(read_b, SruiCodec::new());
    let mut framed_write_b = FramedWrite::new(write_b, SruiCodec::new());

    for (framed_write, client_id) in [
        (&mut framed_write_a, COUNTER_CLIENT_A),
        (&mut framed_write_b, COUNTER_CLIENT_B),
    ] {
        let hello = SruiMessage {
            msg: Some(srui_message::Msg::ClientHello(ClientHello {
                core_version: "0.4.0".to_string(),
                profiles: vec!["org.srui.standard-widgets/1".to_string()],
                limits: None,
                client_instance_id: client_id.to_vec(),
                client_metadata: Default::default(),
            })),
        };
        framed_write.send(hello).await.expect("send ClientHello");
    }

    for framed_read in [&mut framed_read_a, &mut framed_read_b] {
        let welcome = framed_read
            .next()
            .await
            .expect("welcome frame")
            .expect("decode welcome");
        assert!(matches!(
            welcome.msg,
            Some(srui_message::Msg::ServerWelcome(_))
        ));
    }

    framed_write_b
        .send(wire_activate(COUNTER_CLIENT_B, 1, "click-b", 1, button_id))
        .await
        .expect("send ACTIVATE on surviving client");

    let ack_b = timeout(Duration::from_secs(2), framed_read_b.next())
        .await
        .expect("timed out waiting for client B event ack")
        .expect("client B stream ended")
        .expect("decode client B event ack");
    assert!(matches!(
        ack_b.msg,
        Some(srui_message::Msg::ServerEventAck(_))
    ));

    let tx_b = timeout(Duration::from_secs(2), framed_read_b.next())
        .await
        .expect("timed out waiting for client B transaction")
        .expect("client B stream ended")
        .expect("decode client B transaction");
    match tx_b.msg {
        Some(srui_message::Msg::Transaction(tx)) => {
            assert_eq!(tx.base_revision, 1);
            assert_eq!(tx.new_revision, 2);
        }
        other => panic!("expected Transaction on client B, got {:?}", other),
    }
    fixture.assert_count(1);

    // Corrupt client A's stream after handshake; client B must remain healthy.
    framed_write_a
        .get_mut()
        .write_all(&[0x80u8; 11])
        .await
        .expect("write malformed frame prefix on client A");

    let server_a_result = timeout(Duration::from_secs(2), server_a)
        .await
        .expect("client A server task timed out")
        .expect("client A server task join");
    match server_a_result {
        Err(ConnectionError::Framing(FramingError::DecodeError(_))) => {}
        other => panic!("expected framing decode error on client A, got {:?}", other),
    }

    framed_write_b
        .send(wire_activate(COUNTER_CLIENT_B, 2, "click-b-2", 2, button_id))
        .await
        .expect("send second ACTIVATE on surviving client");

    let ack_b2 = timeout(Duration::from_secs(2), framed_read_b.next())
        .await
        .expect("timed out waiting for second client B event ack")
        .expect("client B stream ended")
        .expect("decode second client B event ack");
    assert!(matches!(
        ack_b2.msg,
        Some(srui_message::Msg::ServerEventAck(_))
    ));

    let tx_b2 = timeout(Duration::from_secs(2), framed_read_b.next())
        .await
        .expect("timed out waiting for second client B transaction")
        .expect("client B stream ended")
        .expect("decode second client B transaction");
    match tx_b2.msg {
        Some(srui_message::Msg::Transaction(tx)) => {
            assert_eq!(tx.base_revision, 2);
            assert_eq!(tx.new_revision, 3);
        }
        other => panic!("expected second Transaction on client B, got {:?}", other),
    }
    fixture.assert_count(2);

    shutdown_b.cancel();
    drop(framed_write_b);
    drop(framed_read_b);
    assert!(
        server_b.await.expect("client B server task join").is_ok(),
        "surviving client connection should exit cleanly"
    );
}

