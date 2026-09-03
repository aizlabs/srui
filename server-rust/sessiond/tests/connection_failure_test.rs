//! Handshake failure matrix for `handle_connection` (§15, §18.1, §20.4).
//!
//! Covers timeout, shutdown, EOF, malformed framing, unexpected first messages,
//! capability negotiation edge cases, `ServerWelcome` field population, core-version
//! negotiation, and unresponsive clients that stop draining the outbound stream.

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
    CapabilitySet, NegotiationError, Profile, ServerCapabilities, DEFAULT_MAX_NODE_COUNT,
    DEFAULT_MAX_STRING_LENGTH, DEFAULT_MAX_TRANSACTION_OPERATIONS, DEFAULT_MAX_TREE_DEPTH,
};
use srui_sessiond::{
    handle_connection, ConnectionError, Session, SessionError, HANDSHAKE_TIMEOUT, WRITE_TIMEOUT,
};

/// A client that completes the handshake byte-for-byte and then never reads again.
///
/// `duplex(64)` is smaller than a `ServerWelcome`, so the very first outbound frame pends. That is
/// the state that used to wedge the connection task: `SinkExt::send` never completes, so the
/// `select!` is never re-armed and neither the shutdown token nor the broadcast `Lagged` signal is
/// ever polled again.
/// Returns the server task plus the client halves, which the caller must keep alive: dropping
/// them would close the socket and let the server observe EOF instead of a wedged write.
async fn spawn_unreadable_client(
    session: Arc<Session>,
    shutdown: CancellationToken,
) -> (
    tokio::task::JoinHandle<Result<(), ConnectionError>>,
    tokio::io::ReadHalf<tokio::io::DuplexStream>,
    FramedWrite<tokio::io::WriteHalf<tokio::io::DuplexStream>, SruiCodec>,
) {
    let (client_io, server_io) = duplex(64);
    let handle = spawn_server(server_io, session, shutdown).await;

    let (client_read, client_write) = tokio::io::split(client_io);
    let mut write = FramedWrite::new(client_write, SruiCodec::new());
    write
        .send(SruiMessage {
            msg: Some(srui_message::Msg::ClientHello(sample_client_hello(&[
                "org.srui.standard-widgets/1",
            ]))),
        })
        .await
        .expect("send ClientHello");

    (handle, client_read, write)
}

fn sample_client_hello(profiles: &[&str]) -> ClientHello {
    ClientHello {
        core_version: "0.4.0".to_string(),
        profiles: profiles.iter().map(|s| (*s).to_string()).collect(),
        limits: None,
        client_instance_id: vec![1, 2, 3],
        client_metadata: Default::default(),
    }
}

/// Mirrors the profile parsing loop in fresh-client handshake negotiation (§15).
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
    client_framed_write
        .send(tx_msg)
        .await
        .expect("send transaction");

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
    client_framed_write
        .send(event_msg)
        .await
        .expect("send event");

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
            assert_eq!(
                w.required_profiles,
                vec!["org.srui.standard-widgets/1".to_string()]
            );
            assert_eq!(
                w.optional_profiles,
                vec![
                    "org.srui.richtext/1".to_string(),
                    "org.srui.terminal/1".to_string(),
                ]
            );
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
            assert_eq!(limits.max_string_length, DEFAULT_MAX_STRING_LENGTH as u32);
            assert_eq!(limits.max_resource_size, 50 * 1024 * 1024);
        }
        other => panic!("expected ServerWelcome, got {:?}", other),
    }

    let snapshot = client_framed_read
        .next()
        .await
        .expect("hello catch-up snapshot")
        .expect("decode snapshot");
    match snapshot.msg {
        Some(srui_message::Msg::Transaction(tx)) => {
            assert_eq!(tx.base_revision, 0);
            assert_eq!(tx.new_revision, 1);
        }
        other => panic!("expected hello catch-up Transaction, got {:?}", other),
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

#[tokio::test]
async fn handshake_negotiation_failure_is_session_error() {
    let session = Arc::new(Session::new("test-session-negotiation-fail"));

    // Populate session with pre-existing state
    let tx = Transaction {
        base_revision: 0,
        new_revision: 1,
        priority: 1,
        operations: vec![],
    };
    session
        .commit_transaction(tx)
        .expect("commit initial transaction");
    assert_eq!(session.current_revision(), 1);

    let shutdown = CancellationToken::new();
    let (client_io, server_io) = duplex(4096);
    let handle = spawn_server(server_io, session.clone(), shutdown.clone()).await;

    let (client_read, client_write) = tokio::io::split(client_io);
    let mut client_framed_read = FramedRead::new(client_read, SruiCodec::new());
    let mut client_framed_write = FramedWrite::new(client_write, SruiCodec::new());

    // Send ClientHello with incompatible profiles
    let hello = SruiMessage {
        msg: Some(srui_message::Msg::ClientHello(sample_client_hello(&[
            "org.srui.terminal/1",
        ]))),
    };
    client_framed_write.send(hello).await.expect("send hello");

    // Server should terminate with ConnectionError::Session(SessionError::Negotiation(...))
    let server_result = handle.await.expect("server task join");
    match server_result {
        Err(ConnectionError::Session(SessionError::Negotiation(
            NegotiationError::UnsatisfiedRequiredProfiles { missing },
        ))) => {
            assert_eq!(missing, vec![Profile::standard_widgets_v1()]);
        }
        other => panic!("expected SessionError::Negotiation, got {:?}", other),
    }

    // Assert the peer receives no envelope before stream close
    let next_msg = client_framed_read.next().await;
    assert!(
        next_msg.is_none(),
        "peer must receive no envelopes when negotiation fails"
    );
}

// ---------------------------------------------------------------------------
// Unresponsive clients (§17, §20.4)
// ---------------------------------------------------------------------------

/// §20.4/§17: a client that stops reading must not be able to pin a connection task forever.
///
/// Before the fix, `framed_write.send(..).await` inside a `select!` branch body pended
/// indefinitely, so the shutdown token was never polled again, the [`AttachmentGuard`] was never
/// dropped, and the session stayed `ATTACHED` with a dead peer — which in turn made the daemon's
/// `join_next()` drain in `main` hang on `SIGTERM`.
#[tokio::test]
async fn a_blocked_write_is_interrupted_by_the_shutdown_token() {
    let session = Arc::new(Session::new("blocked-write-shutdown"));
    let shutdown = CancellationToken::new();
    let (handle, _client_read, _client_write) =
        spawn_unreadable_client(Arc::clone(&session), shutdown.clone()).await;

    // Let the server reach the wedged welcome write before asking it to stop.
    tokio::task::yield_now().await;
    shutdown.cancel();

    let result = tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("a cancelled connection must not outlive the shutdown signal")
        .expect("server task join");
    assert!(
        result.is_ok(),
        "a write interrupted by shutdown is a clean stop, got {result:?}"
    );
    assert_eq!(
        session.attached_count(),
        0,
        "the attachment guard must drop so the session returns to DETACHED (§17)"
    );
    assert!(session.is_detached());
}

/// §20.4: an excessively stale client is detached on its own, without a shutdown signal, so the
/// daemon never accumulates connection tasks parked in an unwritable socket.
#[tokio::test(start_paused = true)]
async fn a_client_that_never_reads_is_detached_by_the_write_deadline() {
    let session = Arc::new(Session::new("blocked-write-deadline"));
    let shutdown = CancellationToken::new();
    let (handle, _client_read, _client_write) =
        spawn_unreadable_client(Arc::clone(&session), shutdown).await;

    // Paused clock: tokio auto-advances to the write deadline once every task is idle, so this
    // asserts the deadline rather than waiting on wall-clock time.
    let result = handle.await.expect("server task join");
    assert!(
        matches!(result, Err(ConnectionError::WriteTimeout(timeout)) if timeout == WRITE_TIMEOUT),
        "expected WriteTimeout, got {result:?}"
    );
    assert_eq!(session.attached_count(), 0);
    assert!(session.is_detached());
}

// ---------------------------------------------------------------------------
// Core version negotiation (§15, §4 inv. 13)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_incompatible_core_version_fails_the_handshake() {
    for requested in ["", "1.0.0", "0.5.0", "garbage", "0"] {
        let session = Arc::new(Session::new("core-version"));
        let shutdown = CancellationToken::new();
        let (client_io, server_io) = duplex(4096);
        let handle = spawn_server(server_io, Arc::clone(&session), shutdown).await;

        let (client_read, client_write) = tokio::io::split(client_io);
        let mut framed_read = FramedRead::new(client_read, SruiCodec::new());
        let mut framed_write = FramedWrite::new(client_write, SruiCodec::new());

        let mut hello = sample_client_hello(&["org.srui.standard-widgets/1"]);
        hello.core_version = requested.to_string();
        framed_write
            .send(SruiMessage {
                msg: Some(srui_message::Msg::ClientHello(hello)),
            })
            .await
            .expect("send hello");

        let result = handle.await.expect("server task join");
        assert!(
            matches!(
                result,
                Err(ConnectionError::Session(
                    SessionError::UnsupportedCoreVersion { .. }
                ))
            ),
            "core_version {requested:?} must be refused, got {result:?}"
        );
        assert!(
            framed_read.next().await.is_none(),
            "a refused core version must not receive a WELCOME"
        );
    }
}

#[tokio::test]
async fn a_compatible_patch_level_is_accepted() {
    let session = Arc::new(Session::new("core-version-patch"));
    let shutdown = CancellationToken::new();
    let (client_io, server_io) = duplex(4096);
    let handle = spawn_server(server_io, Arc::clone(&session), shutdown.clone()).await;

    let (client_read, client_write) = tokio::io::split(client_io);
    let mut framed_read = FramedRead::new(client_read, SruiCodec::new());
    let mut framed_write = FramedWrite::new(client_write, SruiCodec::new());

    let mut hello = sample_client_hello(&["org.srui.standard-widgets/1"]);
    hello.core_version = "0.4.99".to_string();
    framed_write
        .send(SruiMessage {
            msg: Some(srui_message::Msg::ClientHello(hello)),
        })
        .await
        .expect("send hello");

    let welcome = framed_read
        .next()
        .await
        .expect("welcome frame")
        .expect("decode welcome");
    assert!(matches!(
        welcome.msg,
        Some(srui_message::Msg::ServerWelcome(_))
    ));

    shutdown.cancel();
    let _ = handle.await.expect("server task join");
}
