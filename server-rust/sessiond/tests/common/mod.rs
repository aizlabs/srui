//! Shared integration test harness and helpers for `sessiond` (§17, §20.2, §18, §32.8).

use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use tokio::io::duplex;
use tokio_util::codec::{FramedRead, FramedWrite};
use tokio_util::sync::CancellationToken;

use srui_protocol::{
    srui_message, ClientHello, ClientResume, SessionContinuity, SruiCodec, SruiMessage, Transaction,
};
use srui_sdk::*;
use srui_sessiond::{handle_connection, ConnectionError, Session};

/// Sets up the standard built-in counter application on the provided session.
pub fn setup_counter_session(session: &Arc<Session>) -> (NodeId, NodeId, NodeId, NodeId) {
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

    (surface_id, text_id, progress_id, button_id)
}

/// In-memory duplex connection harness simulating a connected client.
pub struct TestClientConnection {
    pub read: FramedRead<tokio::io::ReadHalf<tokio::io::DuplexStream>, SruiCodec>,
    pub write: FramedWrite<tokio::io::WriteHalf<tokio::io::DuplexStream>, SruiCodec>,
    pub server_task: tokio::task::JoinHandle<Result<(), ConnectionError>>,
}

impl TestClientConnection {
    pub async fn connect_fresh(
        session: Arc<Session>,
        client_instance_id: &[u8],
    ) -> (Self, String, u64) {
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
                core_version: "0.5.0".to_string(),
                profiles: vec!["org.srui.standard-widgets/1".to_string()],
                limits: None,
                client_instance_id: client_instance_id.to_vec(),
                client_metadata: Default::default(),
                known_resource_hashes: vec![],
            })),
        };
        write.send(hello).await.expect("send ClientHello");

        let welcome_frame = read
            .next()
            .await
            .expect("welcome frame")
            .expect("decode welcome");

        let (session_id, initial_rev) = match welcome_frame.msg {
            Some(srui_message::Msg::ServerWelcome(w)) => (w.session_id, w.initial_revision),
            other => panic!("expected ServerWelcome, got {:?}", other),
        };

        if initial_rev > 0 {
            let snapshot_frame = read
                .next()
                .await
                .expect("snapshot frame")
                .expect("decode snapshot");
            match snapshot_frame.msg {
                Some(srui_message::Msg::Transaction(tx)) => {
                    assert_eq!(tx.base_revision, 0);
                    assert_eq!(tx.new_revision, initial_rev);
                }
                other => panic!("expected snapshot Transaction, got {:?}", other),
            }
        }

        (
            Self {
                read,
                write,
                server_task,
            },
            session_id,
            initial_rev,
        )
    }

    pub async fn send_activate(
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
        self.write
            .send(envelope)
            .await
            .expect("send activate event");
    }

    pub async fn recv_event_ack(&mut self) -> srui_protocol::ServerEventAck {
        let msg = tokio::time::timeout(Duration::from_secs(2), self.read.next())
            .await
            .expect("timeout waiting for event ack")
            .expect("stream ended")
            .expect("decode event ack");
        match msg.msg {
            Some(srui_message::Msg::ServerEventAck(ack)) => ack,
            other => panic!("expected ServerEventAck, got {:?}", other),
        }
    }

    pub async fn recv_transaction(&mut self) -> srui_protocol::Transaction {
        let msg = tokio::time::timeout(Duration::from_secs(2), self.read.next())
            .await
            .expect("timeout waiting for transaction")
            .expect("stream ended")
            .expect("decode transaction");
        match msg.msg {
            Some(srui_message::Msg::Transaction(tx)) => tx,
            other => panic!("expected Transaction, got {:?}", other),
        }
    }

    pub async fn drop_abruptly(self) {
        drop(self.read);
        drop(self.write);
        let _ = self.server_task.await;
    }
}

/// Connects to a running sessiond Unix domain socket with retries and returns the minted session ID.
pub async fn read_session_id_from_counter_sessiond(socket_path: &std::path::Path) -> String {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let stream = loop {
        match tokio::net::UnixStream::connect(socket_path).await {
            Ok(stream) => break stream,
            Err(err) => {
                if tokio::time::Instant::now() >= deadline {
                    panic!(
                        "failed to connect to socket {}: {err}",
                        socket_path.display()
                    );
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    };
    let (read_half, write_half) = tokio::io::split(stream);
    let mut read = FramedRead::new(read_half, SruiCodec::new());
    let mut write = FramedWrite::new(write_half, SruiCodec::new());

    let hello = SruiMessage {
        msg: Some(srui_message::Msg::ClientHello(ClientHello {
            core_version: "0.5.0".to_string(),
            profiles: vec!["org.srui.standard-widgets/1".to_string()],
            limits: None,
            client_instance_id: vec![7, 7],
            client_metadata: Default::default(),
            known_resource_hashes: vec![],
        })),
    };
    write.send(hello).await.expect("send ClientHello");

    let welcome_frame = read
        .next()
        .await
        .expect("welcome frame")
        .expect("decode welcome");
    let session_id = match welcome_frame.msg {
        Some(srui_message::Msg::ServerWelcome(w)) => w.session_id,
        other => panic!("expected ServerWelcome, got {:?}", other),
    };

    // Counter bootstrap sends a catch-up snapshot when initial_revision > 0.
    let snapshot_frame = read
        .next()
        .await
        .expect("snapshot frame")
        .expect("decode snapshot");
    match snapshot_frame.msg {
        Some(srui_message::Msg::Transaction(tx)) => {
            assert_eq!(tx.base_revision, 0);
            assert_eq!(tx.new_revision, 1);
        }
        other => panic!("expected snapshot Transaction, got {:?}", other),
    }

    session_id
}

// ==============================================================================
// Resume / replay wire harness (§18, §18.1, §32.8)
//
// Shared by `session_resume_test.rs` (fine-grained unit coverage) and
// `conformance_reconnect_test.rs` (the named §32 item 8 suite) so each reconnect behavior is
// asserted once against one harness rather than re-implemented per file.
// ==============================================================================

pub fn make_tx(base: u64) -> Transaction {
    Transaction {
        base_revision: base,
        new_revision: base + 1,
        priority: 1,
        operations: vec![],
    }
}

pub fn client_resume(session_id: &str, last_applied: u64) -> SruiMessage {
    SruiMessage {
        msg: Some(srui_message::Msg::ClientResume(ClientResume {
            session_id: session_id.to_string(),
            client_instance_id: vec![1, 2, 3],
            last_applied_revision: last_applied,
            last_acked_event_seq: 0,
            terminal_stream_offsets: Default::default(),
            limits: None,
            known_resource_hashes: vec![],
            pending_text_edits: vec![],
        })),
    }
}

pub struct ResumeConnection {
    pub read: FramedRead<tokio::io::ReadHalf<tokio::io::DuplexStream>, SruiCodec>,
    pub write: FramedWrite<tokio::io::WriteHalf<tokio::io::DuplexStream>, SruiCodec>,
    server_task: tokio::task::JoinHandle<Result<(), ConnectionError>>,
    shutdown: CancellationToken,
}

impl ResumeConnection {
    pub async fn open(session: Arc<Session>) -> Self {
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

    pub async fn send_resume(&mut self, session_id: &str, last_applied: u64) {
        self.write
            .send(client_resume(session_id, last_applied))
            .await
            .expect("send ClientResume");
    }

    pub async fn expect_resume_ok(&mut self, expected_session_id: &str, replay_from: u64) {
        let msg = self
            .read
            .next()
            .await
            .expect("resume ok frame")
            .expect("decode");
        match msg.msg {
            Some(srui_message::Msg::ServerResumeOk(ok)) => {
                assert_eq!(ok.session_id, expected_session_id);
                assert_eq!(ok.replay_from_revision, replay_from);
                assert_eq!(ok.last_processed_event_seq, 0);
            }
            other => panic!("expected ServerResumeOk, got {:?}", other),
        }
    }

    /// Reads one `ServerResyncRequired` and asserts its continuity decision (§18).
    pub async fn expect_resync(
        &mut self,
        expected_session_id: &str,
        expected_continuity: SessionContinuity,
        expected_snapshot_revision: u64,
    ) {
        let msg = self
            .read
            .next()
            .await
            .expect("resync-required frame")
            .expect("decode");
        match msg.msg {
            Some(srui_message::Msg::ServerResyncRequired(resync)) => {
                assert_eq!(resync.session_id, expected_session_id);
                assert_eq!(resync.snapshot_revision, expected_snapshot_revision);
                assert_eq!(
                    SessionContinuity::try_from(resync.continuity),
                    Ok(expected_continuity),
                    "the server always states a recognized continuity (§18, §4 inv. 13)"
                );
                assert_ne!(
                    SessionContinuity::try_from(resync.continuity),
                    Ok(SessionContinuity::Unspecified)
                );
            }
            other => panic!("expected ServerResyncRequired, got {:?}", other),
        }
    }

    pub async fn expect_transaction(&mut self, base_revision: u64, new_revision: u64) {
        let msg = self
            .read
            .next()
            .await
            .expect("transaction frame")
            .expect("decode");
        match msg.msg {
            Some(srui_message::Msg::Transaction(tx)) => {
                assert_eq!(tx.base_revision, base_revision);
                assert_eq!(tx.new_revision, new_revision);
            }
            other => panic!("expected transaction, got {:?}", other),
        }
    }

    pub async fn expect_no_message(&mut self, wait: Duration) {
        let next = tokio::time::timeout(wait, self.read.next()).await;
        match next {
            Ok(Some(Ok(_))) => panic!("expected no wire message during {wait:?}"),
            Ok(Some(Err(e))) => panic!("unexpected decode error: {e}"),
            Ok(None) => panic!("connection closed while expecting no message"),
            Err(_) => {}
        }
    }

    pub async fn close(self) {
        self.shutdown.cancel();
        let res = self.server_task.await.expect("server task join");
        assert!(res.is_ok(), "connection handler failed: {:?}", res);
    }
}
