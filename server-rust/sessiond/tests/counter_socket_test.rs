//! # Counter Application Socket Integration Test (§16, §20.1, §20.2, §29)
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

use std::sync::Arc;

use futures::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::codec::{FramedRead, FramedWrite};
use tokio_util::sync::CancellationToken;

use srui_protocol::{
    srui_message, ClientHello, SruiCodec, SruiMessage,
};
use srui_sdk::*;
use srui_sessiond::{handle_connection, Session};

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
        );

        // Send framed Event over the Unix socket
        let event_envelope = SruiMessage {
            msg: Some(srui_message::Msg::Event(event.to_wire())),
        };
        framed_write
            .send(event_envelope)
            .await
            .expect("send activate event frame");

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
        );

        let event_envelope = SruiMessage {
            msg: Some(srui_message::Msg::Event(event.to_wire())),
        };
        framed_write
            .send(event_envelope)
            .await
            .expect("send activate event frame");

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

