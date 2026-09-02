//! Outbound transaction backpressure and coalescing integration tests (§20.2).
//!
//! Verifies:
//! 1. Scalar property coalescing under blocked client writes with continuous revision spans.
//! 2. Lossless structural barriers preventing coalescing across non-scalar mutations.
//! 3. Bounded peak queue depth assertions under 1,000 rapid updates.
//! 4. Unmergeable structural queue saturation forcing client detachment and Task 23 snapshot resync.

use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use tokio::io::duplex;
use tokio::time::timeout;
use tokio_util::codec::{FramedRead, FramedWrite};
use tokio_util::sync::CancellationToken;

use srui_protocol::{
    srui_message, ClientHello, ClientResume, SessionContinuity, SruiCodec, SruiMessage,
};
use srui_sdk::{NodeId, Row, Surface, Text, LABEL};
use srui_semantic_tree::{PropertyRef, SemanticStore};
use srui_sessiond::{handle_connection, ConnectionError, Session, SessionConfig, SessionError};

#[tokio::test]
async fn test_throttled_1000_updates_coalesce_with_structural_barrier() {
    const QUEUE_CAPACITY: usize = 16;
    let session = Arc::new(Session::with_outbound_queue_capacity(
        "outbound-coalesce-test",
        QUEUE_CAPACITY,
    ));

    let shutdown = CancellationToken::new();
    // Tiny duplex buffer so server write quickly blocks when client stops reading
    let (client_io, server_io) = duplex(128);

    let session_clone = Arc::clone(&session);
    let shutdown_clone = shutdown.clone();
    let server_task =
        tokio::spawn(
            async move { handle_connection(server_io, session_clone, shutdown_clone).await },
        );

    let (client_read, client_write) = tokio::io::split(client_io);
    let mut framed_read = FramedRead::new(client_read, SruiCodec::new());
    let mut framed_write = FramedWrite::new(client_write, SruiCodec::new());

    // Phase 1: Complete initial handshake
    let hello = SruiMessage {
        msg: Some(srui_message::Msg::ClientHello(ClientHello {
            core_version: "0.4.0".to_string(),
            profiles: vec!["org.srui.standard-widgets/1".to_string()],
            limits: None,
            client_instance_id: vec![1, 2, 3, 4],
            client_metadata: Default::default(),
        })),
    };
    framed_write.send(hello).await.expect("send ClientHello");

    let welcome = framed_read
        .next()
        .await
        .expect("welcome frame")
        .expect("decode welcome");
    assert!(matches!(
        welcome.msg,
        Some(srui_message::Msg::ServerWelcome(_))
    ));

    // Set up initial UI graph: Surface (root 1) and Text (node 2)
    let root = NodeId::new(1);
    let text_node = NodeId::new(2);
    let row_node = NodeId::new(3);

    session
        .transaction(|ui| {
            Surface::builder(root).create(ui)?;
            Text::builder(text_node)
                .parent(root)
                .label("v0")
                .create(ui)?;
            Ok(())
        })
        .expect("initial setup transaction");

    // Client intentionally does NOT read from socket during the flood to back up the duplex buffer
    // 1. First 500 scalar updates to node 2 label
    for i in 1..=500 {
        let val = format!("v{i}");
        session
            .transaction(|ui| {
                ui.set(text_node, LABEL, val)?;
                Ok(())
            })
            .expect("scalar update 1..500");
    }

    // 2. Structural barrier: insert a Row node midway
    session
        .transaction(|ui| {
            Row::builder(row_node).parent(root).create(ui)?;
            Ok(())
        })
        .expect("structural row transaction");

    // 3. Second 500 scalar updates to node 2 label
    for i in 501..=1000 {
        let val = format!("v{i}");
        session
            .transaction(|ui| {
                ui.set(text_node, LABEL, val)?;
                Ok(())
            })
            .expect("scalar update 501..1000");
    }

    assert_eq!(session.current_revision(), 1002);

    // Now drain and apply all incoming messages on the client side
    let mut client_store = SemanticStore::new();
    let mut received_tx_count = 0;
    let mut continuous_ranges = true;

    // Read until client catches up to revision 1002
    while client_store.revision().get() < 1002 {
        let frame = timeout(Duration::from_secs(3), framed_read.next())
            .await
            .expect("read frame within timeout")
            .expect("stream not finished")
            .expect("frame decoded cleanly");

        match frame.msg {
            Some(srui_message::Msg::Transaction(tx)) => {
                received_tx_count += 1;
                if tx.base_revision != client_store.revision().get() {
                    continuous_ranges = false;
                }
                client_store
                    .apply_wire_transaction(tx)
                    .expect("apply wire transaction to client replica");
            }
            other => panic!("expected Transaction message, got {:?}", other),
        }
    }

    // Assert continuous revision ranges across the entire stream
    assert!(
        continuous_ranges,
        "received transactions must have continuous revision ranges"
    );
    assert_eq!(client_store.revision().get(), 1002);

    // Assert final scalar value matches 1,000th update
    let node = client_store.get_node(text_node).expect("text node exists");
    assert_eq!(
        node.get_property(PropertyRef::LABEL),
        Some(&srui_semantic_tree::Value::String("v1000".to_string()))
    );

    // Assert structural row exists in client replica
    assert!(
        client_store.contains_node(row_node),
        "structural row node must be preserved through coalesced stream"
    );

    // Assert materially fewer messages than 1,000 (structural barrier separates coalesced chunks)
    assert!(
        received_tx_count <= 10,
        "expected materially fewer messages than 1000 due to coalescing, got {received_tx_count}"
    );

    // Clean shutdown
    shutdown.cancel();
    drop(framed_write);
    drop(framed_read);
    let _ = server_task.await;
}

#[tokio::test]
async fn test_structural_saturation_causes_detachment_and_forces_resync() {
    const SMALL_CAPACITY: usize = 2;
    let config = SessionConfig {
        outbound_queue_capacity: SMALL_CAPACITY,
        journal_capacity: 100,
        ..SessionConfig::default()
    };
    let session = Arc::new(Session::with_config("structural-saturation-test", config));

    let client_instance = vec![42, 99];
    let shutdown = CancellationToken::new();
    let (client_io, server_io) = duplex(64);

    let session_clone = Arc::clone(&session);
    let shutdown_clone = shutdown.clone();
    let server_task =
        tokio::spawn(
            async move { handle_connection(server_io, session_clone, shutdown_clone).await },
        );

    let (client_read, client_write) = tokio::io::split(client_io);
    let mut framed_read = FramedRead::new(client_read, SruiCodec::new());
    let mut framed_write = FramedWrite::new(client_write, SruiCodec::new());

    // Complete fresh handshake
    let hello = SruiMessage {
        msg: Some(srui_message::Msg::ClientHello(ClientHello {
            core_version: "0.4.0".to_string(),
            profiles: vec!["org.srui.standard-widgets/1".to_string()],
            limits: None,
            client_instance_id: client_instance.clone(),
            client_metadata: Default::default(),
        })),
    };
    framed_write.send(hello).await.expect("send ClientHello");

    let welcome = framed_read
        .next()
        .await
        .expect("welcome frame")
        .expect("decode welcome");
    assert!(matches!(
        welcome.msg,
        Some(srui_message::Msg::ServerWelcome(_))
    ));

    // Stop reading from client stream to freeze duplex socket buffer.
    // Issue uncoalesceable structural transactions exceeding SMALL_CAPACITY (2).
    for i in 1..=6 {
        let node_id = NodeId::new(100 + i);
        session
            .transaction(|ui| {
                Surface::builder(node_id).create(ui)?;
                Ok(())
            })
            .expect("commit structural transaction");
    }

    // The connection handler must detach with LaggedResyncRequired
    let server_result = timeout(Duration::from_secs(3), server_task)
        .await
        .expect("server task should exit promptly on overflow")
        .expect("server task join");

    assert!(
        matches!(
            server_result,
            Err(ConnectionError::Session(SessionError::LaggedResyncRequired))
        ),
        "expected LaggedResyncRequired on structural saturation, got {:?}",
        server_result
    );

    drop(framed_write);
    drop(framed_read);

    // -------------------------------------------------------------------------
    // Reconnect & Assert Forced Snapshot Resync (Task 23 / Step 5)
    // -------------------------------------------------------------------------
    let (reconnect_client_io, reconnect_server_io) = duplex(1024 * 1024);
    let reconnect_shutdown = CancellationToken::new();
    let session_clone2 = Arc::clone(&session);
    let reconnect_shutdown_clone = reconnect_shutdown.clone();
    let reconnect_server_task = tokio::spawn(async move {
        handle_connection(
            reconnect_server_io,
            session_clone2,
            reconnect_shutdown_clone,
        )
        .await
    });

    let (r_read, r_write) = tokio::io::split(reconnect_client_io);
    let mut r_framed_read = FramedRead::new(r_read, SruiCodec::new());
    let mut r_framed_write = FramedWrite::new(r_write, SruiCodec::new());

    // Client attempts to resume from revision 0 (which is within journal retention!)
    let resume = SruiMessage {
        msg: Some(srui_message::Msg::ClientResume(ClientResume {
            session_id: session.session_id(),
            client_instance_id: client_instance.clone(),
            last_applied_revision: 0,
            last_acked_event_seq: 0,
            terminal_stream_offsets: Default::default(),
        })),
    };
    r_framed_write
        .send(resume)
        .await
        .expect("send ClientResume");

    // Server must respond with SERVER_RESYNC_REQUIRED (not ServerResumeOk / Replay!)
    let resync_frame = r_framed_read
        .next()
        .await
        .expect("resync frame")
        .expect("decode");

    match resync_frame.msg {
        Some(srui_message::Msg::ServerResyncRequired(resync)) => {
            assert_eq!(resync.session_id, session.session_id());
            assert_eq!(resync.snapshot_revision, session.current_revision());
            assert_eq!(
                SessionContinuity::try_from(resync.continuity),
                Ok(SessionContinuity::SameSession)
            );
            assert!(
                resync
                    .reason
                    .contains("outbound transaction queue overflowed"),
                "expected outbound overflow reason, got {:?}",
                resync.reason
            );
        }
        other => panic!(
            "expected ServerResyncRequired for stale client, got {:?}",
            other
        ),
    }

    // Followed immediately by the snapshot transaction
    let snapshot_frame = r_framed_read
        .next()
        .await
        .expect("snapshot transaction frame")
        .expect("decode");

    match snapshot_frame.msg {
        Some(srui_message::Msg::Transaction(snapshot_tx)) => {
            assert_eq!(snapshot_tx.base_revision, 0);
            assert_eq!(snapshot_tx.new_revision, session.current_revision());
        }
        other => panic!("expected snapshot Transaction, got {:?}", other),
    }

    reconnect_shutdown.cancel();
    drop(r_framed_write);
    drop(r_framed_read);
    let _ = reconnect_server_task.await;
}
