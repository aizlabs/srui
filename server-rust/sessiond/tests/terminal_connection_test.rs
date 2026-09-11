//! Terminal profile handshake, PTY I/O, and reconnect independence from semantic resume (§21, §21.2).

#[allow(dead_code)]
mod common;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use srui_protocol::{
    srui_message, ClientHello, ClientResume, SessionContinuity, TerminalInput, TerminalResize,
    TERMINAL_LOCAL_TYPE_ID, TERMINAL_PROFILE_URI,
};
use srui_sdk::*;
use srui_sessiond::{handle_connection, Session, SessionError, TerminalSpec};
use tokio::io::duplex;
use tokio_util::codec::{FramedRead, FramedWrite};
use tokio_util::sync::CancellationToken;

use srui_protocol::{SruiCodec, SruiMessage};

const CLIENT: &[u8] = &[9, 9];

fn terminal_session() -> (Arc<Session>, NodeId, NodeId) {
    let session = Arc::new(Session::new("terminal-session"));
    let surface = NodeId::new(1);
    let term = NodeId::new(20);
    session
        .transaction(|ui| {
            Surface::builder(surface)
                .label("Terminal Host")
                .create(ui)?;
            Ok(())
        })
        .unwrap();
    session
        .create_terminal_node(
            term,
            surface,
            TerminalSpec {
                executable: "/bin/sh".into(),
                args: vec!["-i".to_string()],
                ring_capacity: 64 * 1024,
                ..TerminalSpec::default()
            },
        )
        .unwrap();
    (session, surface, term)
}

struct ServerGuard {
    session: Arc<Session>,
    shutdown: CancellationToken,
    server_task: Option<tokio::task::JoinHandle<Result<(), srui_sessiond::ConnectionError>>>,
}

impl ServerGuard {
    pub async fn join(
        &mut self,
    ) -> Result<Result<(), srui_sessiond::ConnectionError>, tokio::task::JoinError> {
        self.server_task
            .take()
            .expect("server_task already joined")
            .await
    }
}

impl Drop for ServerGuard {
    fn drop(&mut self) {
        self.shutdown.cancel();
        self.session.pty().shutdown();
        if let Some(task) = &self.server_task {
            task.abort();
        }
    }
}

async fn connect(
    session: Arc<Session>,
    profiles: Vec<String>,
    resume: Option<ClientResume>,
) -> (
    FramedRead<tokio::io::ReadHalf<tokio::io::DuplexStream>, SruiCodec>,
    FramedWrite<tokio::io::WriteHalf<tokio::io::DuplexStream>, SruiCodec>,
    ServerGuard,
) {
    let shutdown = CancellationToken::new();
    let (client_io, server_io) = duplex(1024 * 1024);
    let session_clone = session.clone();
    let shutdown_clone = shutdown.clone();
    let server_task =
        tokio::spawn(
            async move { handle_connection(server_io, session_clone, shutdown_clone).await },
        );
    let (read_half, write_half) = tokio::io::split(client_io);
    let read = FramedRead::new(read_half, SruiCodec::new());
    let mut write = FramedWrite::new(write_half, SruiCodec::new());
    let hello = match resume {
        Some(resume) => SruiMessage {
            msg: Some(srui_message::Msg::ClientResume(resume)),
        },
        None => SruiMessage {
            msg: Some(srui_message::Msg::ClientHello(ClientHello {
                core_version: "0.5.0".to_string(),
                profiles,
                limits: None,
                client_instance_id: CLIENT.to_vec(),
                client_metadata: Default::default(),
                known_resource_hashes: vec![],
            })),
        },
    };
    write.send(hello).await.expect("send handshake");
    let guard = ServerGuard {
        session,
        shutdown,
        server_task: Some(server_task),
    };
    (read, write, guard)
}

async fn recv(
    read: &mut FramedRead<tokio::io::ReadHalf<tokio::io::DuplexStream>, SruiCodec>,
) -> SruiMessage {
    tokio::time::timeout(Duration::from_secs(3), read.next())
        .await
        .expect("timeout")
        .expect("eof")
        .expect("decode")
}

fn terminal_profiles() -> Vec<String> {
    vec![
        "org.srui.standard-widgets/1".to_string(),
        "org.srui.terminal/1".to_string(),
    ]
}

fn assert_terminal_negotiation_readvertised(
    required_profiles: &[String],
    optional_profiles: &[String],
    extension_namespaces: &[srui_protocol::ExtensionNamespaceMapping],
) {
    assert!(required_profiles
        .iter()
        .any(|profile| profile == TERMINAL_PROFILE_URI));
    assert!(optional_profiles.is_empty());
    let terminal_mapping = extension_namespaces
        .iter()
        .find(|mapping| mapping.extension_uri == TERMINAL_PROFILE_URI)
        .expect("resume response must re-advertise the terminal namespace");
    assert_ne!(terminal_mapping.namespace_id, 0);
}

#[tokio::test]
async fn welcome_advertises_negotiated_terminal_namespace() {
    let (session, _, _) = terminal_session();
    let (mut read, _write, _task) = connect(session, terminal_profiles(), None).await;
    let welcome = match recv(&mut read).await.msg {
        Some(srui_message::Msg::ServerWelcome(w)) => w,
        other => panic!("expected welcome, got {other:?}"),
    };
    assert!(welcome
        .required_profiles
        .iter()
        .any(|p| p == TERMINAL_PROFILE_URI));
    let mapping = welcome
        .extension_namespaces
        .iter()
        .find(|m| m.extension_uri == TERMINAL_PROFILE_URI)
        .expect("terminal namespace");
    assert_ne!(mapping.namespace_id, 0);
}

#[tokio::test]
async fn snapshot_contains_extension_typeref() {
    let (session, _, term) = terminal_session();
    let namespace = session.terminal_namespace_id().unwrap();
    let (mut read, _write, _task) = connect(session, terminal_profiles(), None).await;
    let _welcome = recv(&mut read).await;
    let snapshot = match recv(&mut read).await.msg {
        Some(srui_message::Msg::Transaction(tx)) => tx,
        other => panic!("expected snapshot, got {other:?}"),
    };
    let created = snapshot.operations.iter().find_map(|op| match &op.op {
        Some(srui_protocol::operation::Op::CreateNode(create)) => create
            .node
            .as_ref()
            .filter(|node| node.node_id == term.get()),
        _ => None,
    });
    let node = created.expect("terminal create node");
    assert_eq!(node.node_id, term.get());
    let ty = node.r#type.as_ref().expect("type");
    assert_eq!(ty.namespace_id, namespace);
    assert_eq!(ty.local_id, TERMINAL_LOCAL_TYPE_ID);
    assert_ne!(ty.namespace_id, 0);
}

#[tokio::test]
async fn keystrokes_produce_real_pty_output() {
    let (session, _, term) = terminal_session();
    let (mut read, mut write, _task) = connect(session, terminal_profiles(), None).await;
    let _welcome = recv(&mut read).await;
    let _snapshot = recv(&mut read).await;
    write
        .send(SruiMessage {
            msg: Some(srui_message::Msg::TerminalInput(TerminalInput {
                stream_id: term.get(),
                data: b"printf 'SRUI_KEY_OK\\n'\n".to_vec(),
            })),
        })
        .await
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let msg = tokio::time::timeout(
            deadline.saturating_duration_since(tokio::time::Instant::now()),
            read.next(),
        )
        .await
        .expect("timeout waiting for terminal data")
        .expect("eof")
        .expect("decode");
        if let Some(srui_message::Msg::TerminalData(data)) = msg.msg {
            if data
                .data
                .windows(b"SRUI_KEY_OK".len())
                .any(|window| window == b"SRUI_KEY_OK")
            {
                assert!(!data.data.is_empty());
                return;
            }
        }
    }
}

#[tokio::test]
async fn resize_reaches_tiocswinsz() {
    let (session, _, term) = terminal_session();
    let (mut read, mut write, _task) = connect(session, terminal_profiles(), None).await;
    let _welcome = recv(&mut read).await;
    let _snapshot = recv(&mut read).await;
    write
        .send(SruiMessage {
            msg: Some(srui_message::Msg::TerminalResize(TerminalResize {
                stream_id: term.get(),
                columns: 91,
                rows: 33,
                pixel_width: 0,
                pixel_height: 0,
            })),
        })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(40)).await;
    write
        .send(SruiMessage {
            msg: Some(srui_message::Msg::TerminalInput(TerminalInput {
                stream_id: term.get(),
                data: b"stty size\n".to_vec(),
            })),
        })
        .await
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let msg = tokio::time::timeout(
            deadline.saturating_duration_since(tokio::time::Instant::now()),
            read.next(),
        )
        .await
        .expect("timeout waiting for stty")
        .expect("eof")
        .expect("decode");
        if let Some(srui_message::Msg::TerminalData(data)) = msg.msg {
            if data.data.windows(5).any(|window| window == b"33 91") {
                return;
            }
        }
    }
}

#[tokio::test]
async fn cold_revision_zero_resume_readvertises_terminal_negotiation_before_replay() {
    let (session, _, term) = terminal_session();
    session
        .pty()
        .input(term, b"printf 'SRUI_REPLAY_UNIQUE\\n'\n".to_vec())
        .unwrap();
    tokio::time::sleep(Duration::from_millis(80)).await;
    let (start, end) = session.pty().offsets(term).unwrap();
    assert!(end > start);

    let (mut read, _write, _task) = connect(
        session,
        terminal_profiles(),
        Some(ClientResume {
            core_version: "0.5.0".to_string(),
            profiles: terminal_profiles(),
            session_id: "terminal-session".to_string(),
            client_instance_id: CLIENT.to_vec(),
            last_applied_revision: 0,
            last_acked_event_seq: 0,
            terminal_stream_offsets: HashMap::from([(term.get(), 0)]),
            limits: None,
            known_resource_hashes: vec![],
            pending_text_edits: vec![],
        }),
    )
    .await;
    let first = recv(&mut read).await;
    match first.msg {
        Some(srui_message::Msg::ServerResumeOk(resume_ok)) => {
            assert_eq!(resume_ok.replay_from_revision, 0);
            assert_terminal_negotiation_readvertised(
                &resume_ok.required_profiles,
                &resume_ok.optional_profiles,
                &resume_ok.extension_namespaces,
            );
        }
        other => panic!("expected resume ok, got {other:?}"),
    }
    let mut replayed = Vec::new();
    let mut ranges: Vec<(u64, u64)> = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while tokio::time::Instant::now() < deadline {
        if let Ok(Some(Ok(msg))) =
            tokio::time::timeout(Duration::from_millis(200), read.next()).await
        {
            if let Some(srui_message::Msg::TerminalData(data)) = msg.msg {
                let end = data.byte_offset + data.data.len() as u64;
                for &(start, stop) in &ranges {
                    assert!(
                        end <= start || data.byte_offset >= stop,
                        "overlapping replay frames [{start},{stop}) and [{}, {end})",
                        data.byte_offset
                    );
                }
                ranges.push((data.byte_offset, end));
                replayed.extend_from_slice(&data.data);
                if replayed
                    .windows(b"SRUI_REPLAY_UNIQUE".len())
                    .any(|window| window == b"SRUI_REPLAY_UNIQUE")
                {
                    return;
                }
            }
        }
    }
    panic!(
        "did not observe unique replay in {:?}",
        String::from_utf8_lossy(&replayed)
    );
}

#[tokio::test]
async fn reconnect_beyond_retention_sends_terminal_resync_with_resume_ok() {
    let session = Arc::new(Session::new("tiny-ring"));
    let surface = NodeId::new(1);
    let term = NodeId::new(21);
    session
        .transaction(|ui| {
            Surface::builder(surface).label("Tiny").create(ui)?;
            Ok(())
        })
        .unwrap();
    session
        .create_terminal_node(
            term,
            surface,
            TerminalSpec {
                executable: "/bin/sh".into(),
                args: vec![
                    "-c".to_string(),
                    "printf '0123456789ABCDEFGHIJ'; sleep 30".to_string(),
                ],
                ring_capacity: 8,
                ..TerminalSpec::default()
            },
        )
        .unwrap();
    tokio::time::sleep(Duration::from_millis(80)).await;
    let (mut read, _write, _task) = connect(
        session,
        terminal_profiles(),
        Some(ClientResume {
            core_version: "0.5.0".to_string(),
            profiles: terminal_profiles(),
            session_id: "tiny-ring".to_string(),
            client_instance_id: CLIENT.to_vec(),
            last_applied_revision: 1,
            last_acked_event_seq: 0,
            terminal_stream_offsets: HashMap::from([(term.get(), 0)]),
            limits: None,
            known_resource_hashes: vec![],
            pending_text_edits: vec![],
        }),
    )
    .await;
    let mut saw_resume_ok = false;
    let mut saw_terminal_resync = false;
    for _ in 0..8 {
        match recv(&mut read).await.msg {
            Some(srui_message::Msg::ServerResumeOk(_)) => saw_resume_ok = true,
            Some(srui_message::Msg::TerminalResyncRequired(resync)) => {
                assert_eq!(resync.stream_id, term.get());
                saw_terminal_resync = true;
            }
            Some(srui_message::Msg::Transaction(_)) | Some(srui_message::Msg::TerminalData(_)) => {}
            other => panic!("unexpected {other:?}"),
        }
        if saw_resume_ok && saw_terminal_resync {
            return;
        }
    }
    panic!("expected ServerResumeOk plus TerminalResyncRequired");
}

#[tokio::test]
async fn unsupported_client_fails_before_required_extension_node() {
    let (session, _, _) = terminal_session();
    let (mut read, _write, mut guard) = connect(
        session,
        vec!["org.srui.standard-widgets/1".to_string()],
        None,
    )
    .await;
    let result = tokio::time::timeout(Duration::from_secs(2), async {
        let first = read.next().await;
        let join = guard.join().await;
        (first, join)
    })
    .await
    .expect("handshake should end");
    match result.1 {
        Ok(Err(err)) => {
            let text = err.to_string();
            assert!(
                text.contains("unsatisfied")
                    || text.contains("required")
                    || text.contains("terminal"),
                "{text}"
            );
        }
        other => panic!("expected negotiation failure, got {other:?}"),
    }
}

#[tokio::test]
async fn terminal_input_rejects_unknown_stream() {
    let (session, _, _) = terminal_session();
    let (mut read, mut write, mut guard) = connect(session, terminal_profiles(), None).await;
    let _welcome = recv(&mut read).await;
    let _snapshot = recv(&mut read).await;
    write
        .send(SruiMessage {
            msg: Some(srui_message::Msg::TerminalInput(TerminalInput {
                stream_id: 99,
                data: b"x".to_vec(),
            })),
        })
        .await
        .unwrap();
    let join = tokio::time::timeout(Duration::from_secs(2), guard.join())
        .await
        .expect("connection should close")
        .expect("join");
    assert!(join.is_err(), "unknown stream must be a protocol error");
}

/// Negative decode vector for the TERMINAL_INPUT path (§21, §26; CLAUDE.md decode-path rule).
/// `malformed_terminal_input_empty.bin` is protobuf-valid but carries no payload, and must be
/// refused at the wire boundary instead of reaching the PTY master.
#[tokio::test]
async fn malformed_empty_terminal_input_vector_is_rejected() {
    let vector = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../protocol/conformance-vectors/malformed_terminal_input_empty.bin");
    let bytes = std::fs::read(&vector).expect("read malformed_terminal_input_empty.bin");
    let malformed: SruiMessage =
        srui_protocol::decode_framed(&bytes).expect("vector must be protobuf-valid");
    match &malformed.msg {
        Some(srui_message::Msg::TerminalInput(input)) => {
            assert!(input.data.is_empty(), "vector must carry an empty payload");
        }
        other => panic!("expected TerminalInput vector, got {other:?}"),
    }

    let (session, _, _) = terminal_session();
    let (mut read, mut write, mut guard) = connect(session, terminal_profiles(), None).await;
    let _welcome = recv(&mut read).await;
    let _snapshot = recv(&mut read).await;
    write.send(malformed).await.unwrap();
    let join = tokio::time::timeout(Duration::from_secs(2), guard.join())
        .await
        .expect("connection should close")
        .expect("join");
    assert!(
        join.is_err(),
        "empty TerminalInput must be a protocol error"
    );
}

#[tokio::test]
async fn creating_terminal_marks_profile_required() {
    let session = Session::new("req");
    let surface = NodeId::new(1);
    session
        .transaction(|ui| {
            Surface::builder(surface).label("S").create(ui)?;
            Ok(())
        })
        .unwrap();
    session
        .create_terminal_node(NodeId::new(2), surface, TerminalSpec::interactive_shell())
        .unwrap();
    let hello = ClientHello {
        core_version: "0.5.0".to_string(),
        profiles: vec!["org.srui.standard-widgets/1".to_string()],
        limits: None,
        client_instance_id: vec![1],
        client_metadata: Default::default(),
        known_resource_hashes: vec![],
    };
    let err = session.bootstrap_fresh_client(&hello).unwrap_err();
    assert!(matches!(err, SessionError::Negotiation(_)));
}

#[test]
fn create_terminal_after_attach_is_rejected() {
    let session = Session::new("late-term");
    let surface = NodeId::new(1);
    session
        .transaction(|ui| {
            Surface::builder(surface).label("S").create(ui)?;
            Ok(())
        })
        .unwrap();
    let _guard = session.attach().expect("attach");
    let err = session
        .create_terminal_node(NodeId::new(2), surface, TerminalSpec::interactive_shell())
        .unwrap_err();
    assert!(
        matches!(err, SessionError::InvalidInput(_)),
        "late spawn must fail, got {err:?}"
    );
}

/// A resume must prove compatibility again before a replaced Terminal incarnation can export its
/// extension snapshot or subscribe the client (§11.1, §15, §4 inv. 13).
#[test]
fn replaced_incarnation_resume_rejects_missing_terminal_profile() {
    let (session, _surface, _term) = terminal_session();
    let resume = ClientResume {
        core_version: "0.5.0".to_string(),
        profiles: vec!["org.srui.standard-widgets/1".to_string()],
        session_id: "a-different-incarnation".to_string(),
        client_instance_id: CLIENT.to_vec(),
        last_applied_revision: 0,
        last_acked_event_seq: 0,
        terminal_stream_offsets: HashMap::new(),
        limits: None,
        known_resource_hashes: vec![],
        pending_text_edits: vec![],
    };

    let err = session.bootstrap_resume(&resume).unwrap_err();
    match err {
        SessionError::Negotiation(error) => {
            assert!(
                error.to_string().contains(TERMINAL_PROFILE_URI),
                "profile rejection must name Terminal, got {error}"
            );
        }
        other => panic!("expected Negotiation, got {other:?}"),
    }
}

#[test]
fn replaced_incarnation_resume_rejects_incompatible_core_version() {
    let (session, _surface, _term) = terminal_session();
    let resume = ClientResume {
        core_version: "0.4.9".to_string(),
        profiles: terminal_profiles(),
        session_id: "a-different-incarnation".to_string(),
        client_instance_id: CLIENT.to_vec(),
        last_applied_revision: 0,
        last_acked_event_seq: 0,
        terminal_stream_offsets: HashMap::new(),
        limits: None,
        known_resource_hashes: vec![],
        pending_text_edits: vec![],
    };

    let err = session.bootstrap_resume(&resume).unwrap_err();
    match err {
        SessionError::UnsupportedCoreVersion {
            requested,
            supported,
        } => {
            assert_eq!(requested, "0.4.9");
            assert_eq!(supported, "0.5.0");
        }
        other => panic!("expected UnsupportedCoreVersion, got {other:?}"),
    }
}

#[tokio::test]
async fn replaced_incarnation_terminal_resume_sends_negotiation_before_snapshot() {
    let (session, _surface, term) = terminal_session();
    let expected_session_id = session.session_id();
    let (mut read, _write, _task) = connect(
        session,
        Vec::new(),
        Some(ClientResume {
            core_version: "0.5.0".to_string(),
            profiles: terminal_profiles(),
            session_id: "a-different-incarnation".to_string(),
            client_instance_id: CLIENT.to_vec(),
            last_applied_revision: 0,
            last_acked_event_seq: 0,
            terminal_stream_offsets: HashMap::new(),
            limits: None,
            known_resource_hashes: vec![],
            pending_text_edits: vec![],
        }),
    )
    .await;

    let resync = match recv(&mut read).await.msg {
        Some(srui_message::Msg::ServerResyncRequired(resync)) => resync,
        other => panic!("expected negotiation-bearing resync first, got {other:?}"),
    };
    assert_eq!(resync.session_id, expected_session_id);
    assert_eq!(
        SessionContinuity::try_from(resync.continuity),
        Ok(SessionContinuity::Replaced)
    );
    assert_terminal_negotiation_readvertised(
        &resync.required_profiles,
        &resync.optional_profiles,
        &resync.extension_namespaces,
    );
    let terminal_namespace = resync
        .extension_namespaces
        .iter()
        .find(|mapping| mapping.extension_uri == TERMINAL_PROFILE_URI)
        .expect("terminal namespace")
        .namespace_id;

    let snapshot = match recv(&mut read).await.msg {
        Some(srui_message::Msg::Transaction(snapshot)) => snapshot,
        other => panic!("expected snapshot after negotiation metadata, got {other:?}"),
    };
    assert_eq!(snapshot.new_revision, resync.snapshot_revision);
    let terminal_node = snapshot
        .operations
        .iter()
        .find_map(|operation| match &operation.op {
            Some(srui_protocol::operation::Op::CreateNode(create)) => create
                .node
                .as_ref()
                .filter(|node| node.node_id == term.get()),
            _ => None,
        });
    let terminal_type = terminal_node
        .expect("replacement snapshot contains Terminal node")
        .r#type
        .as_ref()
        .expect("Terminal type");
    assert_eq!(terminal_type.namespace_id, terminal_namespace);
    assert_eq!(terminal_type.local_id, TERMINAL_LOCAL_TYPE_ID);
}

/// Detaching returns the session to `Detached`, but a client that already handshook can resume
/// without a second `ServerWelcome`. Adding the required Terminal profile in that window would
/// hand that client a node whose profile and namespace its negotiated set omits (§15, §21).
#[test]
fn create_terminal_after_a_completed_handshake_is_rejected_even_once_detached() {
    let session = Session::new("detached-term");
    let surface = NodeId::new(1);
    session
        .transaction(|ui| {
            Surface::builder(surface).label("S").create(ui)?;
            Ok(())
        })
        .unwrap();

    let hello = ClientHello {
        core_version: "0.5.0".to_string(),
        profiles: vec!["org.srui.standard-widgets/1".to_string()],
        limits: None,
        client_instance_id: vec![7],
        client_metadata: Default::default(),
        known_resource_hashes: vec![],
    };
    let guard = session.attach().expect("attach");
    let _bootstrap = session.bootstrap_fresh_client(&hello).expect("handshake");
    drop(guard);
    assert!(session.is_detached(), "guard drop returns to Detached");
    assert!(session.has_negotiated(), "the handshake is remembered");

    let err = session
        .create_terminal_node(NodeId::new(2), surface, TerminalSpec::interactive_shell())
        .unwrap_err();
    assert!(
        matches!(err, SessionError::InvalidInput(_)),
        "post-handshake spawn must fail, got {err:?}"
    );
    assert!(
        session.terminal_namespace_id().is_none(),
        "a refused spawn must not leave a terminal namespace behind"
    );
}

#[tokio::test]
async fn flooding_terminal_does_not_starve_counter_ack() {
    let session = Arc::new(Session::new("flood"));
    let (surface, _text, _progress, button) = common::setup_counter_session(&session);
    let term = NodeId::new(20);
    session
        .create_terminal_node(
            term,
            surface,
            TerminalSpec {
                executable: "/bin/sh".into(),
                args: vec![
                    "-c".to_string(),
                    "while true; do printf 'X'; done".to_string(),
                ],
                ring_capacity: 64 * 1024,
                ..TerminalSpec::default()
            },
        )
        .unwrap();
    let (mut read, mut write, _task) = connect(session, terminal_profiles(), None).await;
    let _welcome = recv(&mut read).await;
    let _snapshot = recv(&mut read).await;
    let activate = Event::activate(1, "act-1", 1, button).with_client_instance_id(CLIENT);
    write
        .send(SruiMessage {
            msg: Some(srui_message::Msg::Event(activate.to_wire())),
        })
        .await
        .unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let mut saw_ack = false;
    let mut saw_tx = false;
    while tokio::time::Instant::now() < deadline && !(saw_ack && saw_tx) {
        let msg = tokio::time::timeout(
            deadline.saturating_duration_since(tokio::time::Instant::now()),
            read.next(),
        )
        .await
        .expect("timeout")
        .expect("eof")
        .expect("decode");
        match msg.msg {
            Some(srui_message::Msg::ServerEventAck(_)) => saw_ack = true,
            Some(srui_message::Msg::Transaction(_)) => saw_tx = true,
            _ => {}
        }
    }
    assert!(saw_ack && saw_tx, "terminal flood starved semantic traffic");
}

#[tokio::test]
async fn semantic_journal_gap_still_replays_retained_terminal() {
    let session = Arc::new(Session::with_config(
        "journal-gap",
        srui_sessiond::SessionConfig {
            journal_capacity: 1,
            ..srui_sessiond::SessionConfig::default()
        },
    ));
    let surface = NodeId::new(1);
    let term = NodeId::new(22);
    session
        .transaction(|ui| {
            Surface::builder(surface).label("Gap").create(ui)?;
            Ok(())
        })
        .unwrap();
    session
        .create_terminal_node(
            term,
            surface,
            TerminalSpec {
                executable: "/bin/sh".into(),
                args: vec!["-i".to_string()],
                ring_capacity: 64 * 1024,
                ..TerminalSpec::default()
            },
        )
        .unwrap();
    session
        .pty()
        .input(term, b"printf 'SRUI_JOURNAL_GAP_OK\\n'\n".to_vec())
        .unwrap();
    tokio::time::sleep(Duration::from_millis(80)).await;

    let (mut read, _write, _task) = connect(
        session,
        terminal_profiles(),
        Some(ClientResume {
            core_version: "0.5.0".to_string(),
            profiles: terminal_profiles(),
            session_id: "journal-gap".to_string(),
            client_instance_id: CLIENT.to_vec(),
            last_applied_revision: 0,
            last_acked_event_seq: 0,
            terminal_stream_offsets: HashMap::from([(term.get(), 0)]),
            limits: None,
            known_resource_hashes: vec![],
            pending_text_edits: vec![],
        }),
    )
    .await;
    let mut saw_semantic_resync = false;
    let mut replayed = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(4);
    while tokio::time::Instant::now() < deadline {
        let Ok(Some(Ok(msg))) = tokio::time::timeout(Duration::from_millis(250), read.next()).await
        else {
            if saw_semantic_resync
                && replayed
                    .windows(b"SRUI_JOURNAL_GAP_OK".len())
                    .any(|window| window == b"SRUI_JOURNAL_GAP_OK")
            {
                return;
            }
            continue;
        };
        match msg.msg {
            Some(srui_message::Msg::ServerResyncRequired(resync)) => {
                assert_eq!(
                    SessionContinuity::try_from(resync.continuity),
                    Ok(SessionContinuity::SameSession)
                );
                assert_terminal_negotiation_readvertised(
                    &resync.required_profiles,
                    &resync.optional_profiles,
                    &resync.extension_namespaces,
                );
                saw_semantic_resync = true;
            }
            Some(srui_message::Msg::TerminalData(data)) => replayed.extend_from_slice(&data.data),
            _ => {}
        }
        if saw_semantic_resync
            && replayed
                .windows(b"SRUI_JOURNAL_GAP_OK".len())
                .any(|window| window == b"SRUI_JOURNAL_GAP_OK")
        {
            return;
        }
    }
    panic!(
        "expected semantic resync plus retained terminal replay, got resync={saw_semantic_resync} replay={:?}",
        String::from_utf8_lossy(&replayed)
    );
}

#[tokio::test]
async fn journal_gap_and_terminal_eviction_are_independent() {
    let session = Arc::new(Session::with_config(
        "both-gaps",
        srui_sessiond::SessionConfig {
            journal_capacity: 1,
            ..srui_sessiond::SessionConfig::default()
        },
    ));
    let surface = NodeId::new(1);
    let term = NodeId::new(23);
    session
        .transaction(|ui| {
            Surface::builder(surface).label("Both").create(ui)?;
            Ok(())
        })
        .unwrap();
    session
        .create_terminal_node(
            term,
            surface,
            TerminalSpec {
                executable: "/bin/sh".into(),
                args: vec![
                    "-c".to_string(),
                    "printf '0123456789ABCDEFGHIJ'; sleep 30".to_string(),
                ],
                ring_capacity: 8,
                ..TerminalSpec::default()
            },
        )
        .unwrap();
    tokio::time::sleep(Duration::from_millis(80)).await;
    let (mut read, _write, _task) = connect(
        session,
        terminal_profiles(),
        Some(ClientResume {
            core_version: "0.5.0".to_string(),
            profiles: terminal_profiles(),
            session_id: "both-gaps".to_string(),
            client_instance_id: CLIENT.to_vec(),
            last_applied_revision: 0,
            last_acked_event_seq: 0,
            terminal_stream_offsets: HashMap::from([(term.get(), 0)]),
            limits: None,
            known_resource_hashes: vec![],
            pending_text_edits: vec![],
        }),
    )
    .await;
    let mut saw_semantic = false;
    let mut saw_terminal = false;
    for _ in 0..10 {
        match recv(&mut read).await.msg {
            Some(srui_message::Msg::ServerResyncRequired(_)) => saw_semantic = true,
            Some(srui_message::Msg::TerminalResyncRequired(_)) => saw_terminal = true,
            Some(srui_message::Msg::Transaction(_))
            | Some(srui_message::Msg::TerminalData(_))
            | Some(srui_message::Msg::ServerResumeOk(_)) => {}
            other => panic!("unexpected {other:?}"),
        }
        if saw_semantic && saw_terminal {
            return;
        }
    }
    panic!("expected independent semantic and terminal resyncs");
}

#[tokio::test]
async fn terminal_input_and_resize_to_closed_stream_does_not_drop_connection() {
    let session = Arc::new(Session::new("closed-stream"));
    let (surface, _text, _progress, button) = common::setup_counter_session(&session);
    let term = NodeId::new(20);
    session
        .create_terminal_node(
            term,
            surface,
            TerminalSpec {
                executable: "/bin/sh".into(),
                args: vec![
                    "-c".to_string(),
                    "printf 'SRUI_EXIT_EARLY\\n'; exit 0".to_string(),
                ],
                ring_capacity: 64 * 1024,
                ..TerminalSpec::default()
            },
        )
        .unwrap();
    let (mut read, mut write, _guard) = connect(session.clone(), terminal_profiles(), None).await;
    let _welcome = recv(&mut read).await;
    let _snapshot = recv(&mut read).await;

    // Wait until child has exited and command sender closed
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Send input and resize to the closed stream; must not drop connection
    write
        .send(SruiMessage {
            msg: Some(srui_message::Msg::TerminalInput(TerminalInput {
                stream_id: term.get(),
                data: b"echo after exit\n".to_vec(),
            })),
        })
        .await
        .unwrap();

    write
        .send(SruiMessage {
            msg: Some(srui_message::Msg::TerminalResize(TerminalResize {
                stream_id: term.get(),
                columns: 100,
                rows: 40,
                pixel_width: 0,
                pixel_height: 0,
            })),
        })
        .await
        .unwrap();

    // Verify connection is still alive and semantic interaction succeeds
    let activate = Event::activate(1, "act-1", 1, button).with_client_instance_id(CLIENT);
    write
        .send(SruiMessage {
            msg: Some(srui_message::Msg::Event(activate.to_wire())),
        })
        .await
        .unwrap();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let mut saw_ack = false;
    while tokio::time::Instant::now() < deadline && !saw_ack {
        let msg = tokio::time::timeout(
            deadline.saturating_duration_since(tokio::time::Instant::now()),
            read.next(),
        )
        .await
        .expect("timeout")
        .expect("eof")
        .expect("decode");
        if let Some(srui_message::Msg::ServerEventAck(_)) = msg.msg {
            saw_ack = true;
        }
    }
    assert!(
        saw_ack,
        "connection was dropped after input to closed stream"
    );
}

#[tokio::test]
async fn failed_spawn_rolls_back_capabilities_and_namespace() {
    let session = Session::new("failed-spawn");
    let surface = NodeId::new(1);
    session
        .transaction(|ui| {
            Surface::builder(surface).label("S").create(ui)?;
            Ok(())
        })
        .unwrap();

    // Attempt to spawn an invalid executable
    let err = session.create_terminal_node(
        NodeId::new(2),
        surface,
        TerminalSpec {
            executable: "/nonexistent/binary/srui_fail".into(),
            ..TerminalSpec::default()
        },
    );
    assert!(
        err.is_err(),
        "expected spawn failure for nonexistent binary"
    );

    // Verify terminal_v1 is not in required capabilities
    assert!(
        session.terminal_namespace_id().is_none(),
        "extension namespace must be rolled back on spawn failure"
    );

    // Standard client without terminal profile must succeed
    let hello = ClientHello {
        core_version: "0.5.0".to_string(),
        profiles: vec!["org.srui.standard-widgets/1".to_string()],
        limits: None,
        client_instance_id: vec![1],
        client_metadata: Default::default(),
        known_resource_hashes: vec![],
    };
    let welcome = session.bootstrap_fresh_client(&hello);
    assert!(
        welcome.is_ok(),
        "standard client was refused due to corrupted required capabilities: {:?}",
        welcome.err()
    );
}

#[tokio::test]
async fn live_terminal_generation_during_catch_up_does_not_trigger_fallbehind() {
    let session = Arc::new(Session::new("catchup-drain"));
    let surface = NodeId::new(1);
    let term = NodeId::new(24);
    session
        .transaction(|ui| {
            Surface::builder(surface).label("Drain").create(ui)?;
            Ok(())
        })
        .unwrap();
    session
        .create_terminal_node(
            term,
            surface,
            TerminalSpec {
                executable: "/bin/sh".into(),
                args: vec![
                    "-c".to_string(),
                    "printf 'HISTORICAL_DATA\\n'; sleep 0.1; while true; do printf 'LIVE_STREAM_BURST\\n'; sleep 0.05; done".to_string(),
                ],
                ring_capacity: 512,
                ..TerminalSpec::default()
            },
        )
        .unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    let (mut read, _write, _guard) = connect(
        session,
        terminal_profiles(),
        Some(ClientResume {
            core_version: "0.5.0".to_string(),
            profiles: terminal_profiles(),
            session_id: "catchup-drain".to_string(),
            client_instance_id: CLIENT.to_vec(),
            last_applied_revision: 1,
            last_acked_event_seq: 0,
            terminal_stream_offsets: HashMap::from([(term.get(), 0)]),
            limits: None,
            known_resource_hashes: vec![],
            pending_text_edits: vec![],
        }),
    )
    .await;

    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let mut saw_resume_ok = false;
    let mut saw_live_data = false;
    while tokio::time::Instant::now() < deadline {
        let msg = tokio::time::timeout(
            deadline.saturating_duration_since(tokio::time::Instant::now()),
            read.next(),
        )
        .await
        .expect("timeout")
        .expect("eof")
        .expect("decode");

        match msg.msg {
            Some(srui_message::Msg::ServerResumeOk(_)) => saw_resume_ok = true,
            Some(srui_message::Msg::TerminalResyncRequired(resync)) => {
                panic!("unexpected fallbehind resync during catch-up: {resync:?}");
            }
            Some(srui_message::Msg::TerminalData(data))
                if data
                    .data
                    .windows(b"LIVE_STREAM_BURST".len())
                    .any(|w| w == b"LIVE_STREAM_BURST") =>
            {
                saw_live_data = true;
            }
            _ => {}
        }
        if saw_resume_ok && saw_live_data {
            return;
        }
    }
    assert!(
        saw_resume_ok && saw_live_data,
        "failed to receive live data smoothly"
    );
}
