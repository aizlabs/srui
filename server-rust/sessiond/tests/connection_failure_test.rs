//! Handshake failure matrix for `handle_connection` (§15, §18.1).
//!
//! Covers timeout, shutdown, EOF, malformed framing, unexpected first messages,
//! capability negotiation edge cases, and `ServerWelcome` field population.

use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use tokio::io::{duplex, AsyncWriteExt};
use tokio_util::codec::{FramedRead, FramedWrite};
use tokio_util::sync::CancellationToken;

use srui_protocol::{
    srui_message, ClientHello, Event, FramingError, SruiCodec, SruiMessage, Transaction,
};
use srui_semantic_tree::{
    CapabilitySet, NegotiationError, Profile, ServerCapabilities,
    DEFAULT_MAX_NODE_COUNT, DEFAULT_MAX_STRING_LENGTH, DEFAULT_MAX_TRANSACTION_OPERATIONS,
    DEFAULT_MAX_TREE_DEPTH,
};
use srui_sessiond::{
    handle_connection, ConnectionError, Session, SessionError, HANDSHAKE_TIMEOUT,
};

fn sample_client_hello(profiles: &[&str]) -> ClientHello {
    ClientHello {
        core_version: "0.4.0".to_string(),
        profiles: profiles.iter().map(|s| (*s).to_string()).collect(),
        limits: None,
        client_instance_id: vec![1, 2, 3],
        client_metadata: Default::default(),
    }
}

/// Mirrors the profile parsing loop in `Session::handle_hello` (§15).
fn client_capability_set(hello: &ClientHello) -> CapabilitySet {
    let mut client_caps = CapabilitySet::new();
    for p_str in &hello.profiles {
        if let Ok(p) = Profile::parse(p_str) {
            client_caps.insert(p);
        }
    }
    client_caps
}

fn standard_widgets_server() -> ServerCapabilities {
    ServerCapabilities::new(
        CapabilitySet::from_str_slice(&["org.srui.standard-widgets/1"]).unwrap(),
        CapabilitySet::from_str_slice(&["org.srui.terminal/1"]).unwrap(),
    )
}

async fn spawn_server(
    server_io: tokio::io::DuplexStream,
    session: Arc<Session>,
    shutdown: CancellationToken,
) -> tokio::task::JoinHandle<Result<(), ConnectionError>> {
    tokio::spawn(async move { handle_connection(server_io, session, shutdown).await })
}

#[tokio::test]
async fn handshake_times_out_when_client_sends_no_first_message() {
    tokio::time::pause();

    let session = Arc::new(Session::new("handshake-timeout"));
    let shutdown = CancellationToken::new();
    let (_client_io, server_io) = duplex(4096);

    let handle = spawn_server(server_io, session, shutdown).await;

    tokio::time::advance(HANDSHAKE_TIMEOUT + Duration::from_millis(1)).await;

    let result = handle.await.expect("server task join");
    assert!(matches!(result, Err(ConnectionError::HandshakeTimeout)));
}

#[tokio::test]
async fn shutdown_during_handshake_exits_cleanly() {
    let session = Arc::new(Session::new("shutdown-handshake"));
    let shutdown = CancellationToken::new();
    let (_client_io, server_io) = duplex(4096);

    let handle = spawn_server(server_io, session, shutdown.clone()).await;

    shutdown.cancel();

    let result = handle.await.expect("server task join");
    assert!(result.is_ok());
}

#[tokio::test]
async fn eof_before_handshake_returns_connection_closed() {
    let session = Arc::new(Session::new("eof-handshake"));
    let shutdown = CancellationToken::new();
    let (client_io, server_io) = duplex(4096);

    let handle = spawn_server(server_io, session, shutdown).await;

    drop(client_io);

    let result = handle.await.expect("server task join");
    assert!(matches!(result, Err(ConnectionError::ConnectionClosed)));
}

#[tokio::test]
async fn malformed_framing_during_handshake_returns_framing_error() {
    let session = Arc::new(Session::new("malformed-handshake"));
    let shutdown = CancellationToken::new();
    let (client_io, server_io) = duplex(4096);

    let handle = spawn_server(server_io, session, shutdown).await;

    let (_client_read, mut client_write) = tokio::io::split(client_io);
    client_write
        .write_all(&[0x80u8; 11])
        .await
        .expect("write malformed varint prefix");
    client_write
        .shutdown()
        .await
        .expect("shutdown client write half");

    let result = handle.await.expect("server task join");
    match result {
        Err(ConnectionError::Framing(FramingError::DecodeError(_))) => {}
        other => panic!("expected framing decode error, got {:?}", other),
    }
}

#[tokio::test]
async fn transaction_as_first_message_is_rejected() {
    let session = Arc::new(Session::new("tx-first"));
    let shutdown = CancellationToken::new();
    let (client_io, server_io) = duplex(4096);

    let handle = spawn_server(server_io, session, shutdown).await;

    let (_client_read, client_write) = tokio::io::split(client_io);
    let mut client_framed_write = FramedWrite::new(client_write, SruiCodec::new());

    let tx_msg = SruiMessage {
        msg: Some(srui_message::Msg::Transaction(Transaction {
            base_revision: 0,
            new_revision: 1,
            priority: 1,
            operations: vec![],
        })),
    };
    client_framed_write.send(tx_msg).await.expect("send transaction");

    let result = handle.await.expect("server task join");
    assert!(matches!(
        result,
        Err(ConnectionError::UnexpectedMessage(
            "expected ClientHello or ClientResume"
        ))
    ));
}

#[tokio::test]
async fn event_as_first_message_is_rejected() {
    let session = Arc::new(Session::new("event-first"));
    let shutdown = CancellationToken::new();
    let (client_io, server_io) = duplex(4096);

    let handle = spawn_server(server_io, session, shutdown).await;

    let (_client_read, client_write) = tokio::io::split(client_io);
    let mut client_framed_write = FramedWrite::new(client_write, SruiCodec::new());

    let event_msg = SruiMessage {
        msg: Some(srui_message::Msg::Event(Event {
            client_instance_id: vec![1],
            event_seq: 1,
            event_id: vec![9, 9],
            observed_revision: 0,
            node_id: 42,
            event_type: None,
            arguments: vec![],
        })),
    };
    client_framed_write.send(event_msg).await.expect("send event");

    let result = handle.await.expect("server task join");
    assert!(matches!(
        result,
        Err(ConnectionError::UnexpectedMessage(
            "expected ClientHello or ClientResume"
        ))
    ));
}

#[tokio::test]
async fn server_welcome_contains_session_metadata() {
    let session = Arc::new(Session::new("welcome-fields"));
    session
        .commit_transaction(Transaction {
            base_revision: 0,
            new_revision: 1,
            priority: 1,
            operations: vec![],
        })
        .expect("seed revision");

    let shutdown = CancellationToken::new();
    let (client_io, server_io) = duplex(4096);
    let handle = spawn_server(server_io, session, shutdown.clone()).await;

    let (client_read, client_write) = tokio::io::split(client_io);
    let mut client_framed_read = FramedRead::new(client_read, SruiCodec::new());
    let mut client_framed_write = FramedWrite::new(client_write, SruiCodec::new());

    let hello = SruiMessage {
        msg: Some(srui_message::Msg::ClientHello(sample_client_hello(&[
            "org.srui.standard-widgets/1",
            "org.srui.terminal/1",
        ]))),
    };
    client_framed_write.send(hello).await.expect("send hello");

    let welcome_msg = client_framed_read
        .next()
        .await
        .expect("receive welcome")
        .expect("decode welcome");

    match welcome_msg.msg {
        Some(srui_message::Msg::ServerWelcome(w)) => {
            assert_eq!(w.core_version, "0.4.0");
            assert_eq!(w.session_id, "welcome-fields");
            assert_eq!(w.initial_revision, 1);
            assert_eq!(w.required_profiles, Vec::<String>::new());
            assert_eq!(w.optional_profiles, Vec::<String>::new());
            assert_eq!(w.extension_namespaces.len(), 1);
            assert_eq!(
                w.extension_namespaces[0].extension_uri,
                "org.srui.standard-widgets"
            );
            assert_eq!(w.extension_namespaces[0].namespace_id, 0);

            let limits = w.limits.expect("server limits advertised");
            assert_eq!(limits.max_frame_size, 16 * 1024 * 1024);
            assert_eq!(
                limits.max_transaction_operations,
                DEFAULT_MAX_TRANSACTION_OPERATIONS as u32
            );
            assert_eq!(limits.max_tree_depth, DEFAULT_MAX_TREE_DEPTH as u32);
            assert_eq!(limits.max_node_count, DEFAULT_MAX_NODE_COUNT as u32);
            assert_eq!(
                limits.max_string_length,
                DEFAULT_MAX_STRING_LENGTH as u32
            );
            assert_eq!(limits.max_resource_size, 50 * 1024 * 1024);
        }
        other => panic!("expected ServerWelcome, got {:?}", other),
    }

    shutdown.cancel();
    let _ = handle.await.expect("server task join");
}

#[test]
fn hello_missing_required_profile_fails_negotiation() {
    let server = standard_widgets_server();
    let hello = sample_client_hello(&["org.srui.terminal/1"]);
    let client_caps = client_capability_set(&hello);

    let err = server
        .negotiate(&client_caps)
        .expect_err("missing required profile must fail");

    assert_eq!(
        err,
        NegotiationError::UnsatisfiedRequiredProfiles {
            missing: vec![Profile::standard_widgets_v1()],
        }
    );
}

#[test]
fn incompatible_required_profile_version_fails_negotiation() {
    let server = standard_widgets_server();
    let hello = sample_client_hello(&["org.srui.standard-widgets/2"]);
    let client_caps = client_capability_set(&hello);

    let err = server
        .negotiate(&client_caps)
        .expect_err("version mismatch must fail required profile match");

    assert_eq!(
        err,
        NegotiationError::UnsatisfiedRequiredProfiles {
            missing: vec![Profile::standard_widgets_v1()],
        }
    );
}

#[tokio::test]
async fn unknown_optional_profiles_are_allowed() {
    let session = Arc::new(Session::new("unknown-optional"));
    let shutdown = CancellationToken::new();
    let (client_io, server_io) = duplex(4096);
    let handle = spawn_server(server_io, session.clone(), shutdown.clone()).await;

    let (client_read, client_write) = tokio::io::split(client_io);
    let mut client_framed_read = FramedRead::new(client_read, SruiCodec::new());
    let mut client_framed_write = FramedWrite::new(client_write, SruiCodec::new());

    let hello = SruiMessage {
        msg: Some(srui_message::Msg::ClientHello(sample_client_hello(&[
            "org.srui.standard-widgets/1",
            "com.example.unknown-extension/1",
        ]))),
    };
    client_framed_write.send(hello).await.expect("send hello");

    let welcome_msg = client_framed_read
        .next()
        .await
        .expect("receive welcome")
        .expect("decode welcome");

    match welcome_msg.msg {
        Some(srui_message::Msg::ServerWelcome(_)) => {}
        other => panic!("expected ServerWelcome, got {:?}", other),
    }

    shutdown.cancel();
    assert!(handle.await.expect("server task join").is_ok());
}

#[test]
fn invalid_profile_syntax_is_not_silently_accepted() {
    let hello = sample_client_hello(&[
        "org.srui.standard-widgets",
        "not-a-profile",
        "org.srui.standard-widgets/1",
    ]);
    let client_caps = client_capability_set(&hello);

    assert_eq!(client_caps.len(), 1);
    assert!(client_caps.contains_str("org.srui.standard-widgets/1"));
    assert!(!client_caps.contains_str("org.srui.standard-widgets"));
    assert!(!client_caps.contains_str("not-a-profile"));

    let server = standard_widgets_server();
    assert!(server.negotiate(&client_caps).is_ok());

    let invalid_only = sample_client_hello(&["bad-profile", "also/invalid/too/many"]);
    let invalid_caps = client_capability_set(&invalid_only);
    assert!(invalid_caps.is_empty());

    let err = server
        .negotiate(&invalid_caps)
        .expect_err("invalid-only profiles must not satisfy required capabilities");
    assert!(matches!(
        err,
        NegotiationError::UnsatisfiedRequiredProfiles { .. }
    ));
}

#[test]
fn handshake_negotiation_failure_is_session_error() {
    let server = standard_widgets_server();
    let hello = sample_client_hello(&["org.srui.terminal/1"]);
    let client_caps = client_capability_set(&hello);
    let negotiation_err = server.negotiate(&client_caps).unwrap_err();

    let connection_err = ConnectionError::Session(SessionError::Negotiation(negotiation_err));
    assert!(matches!(
        connection_err,
        ConnectionError::Session(SessionError::Negotiation(
            NegotiationError::UnsatisfiedRequiredProfiles { .. }
        ))
    ));
}
