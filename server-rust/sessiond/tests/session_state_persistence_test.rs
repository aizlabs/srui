//! # Session State & Persistence Tests (§17, §20.2, App. B)
//!
//! Verifies:
//! 1. Session state lifecycle: `DETACHED -> ATTACHED -> DETACHED` as connections attach and drop.
//! 2. Multi-client attachment counts: state remains `ATTACHED` while `attached_count > 0`, and
//!    transitions to `DETACHED` only when all connections disconnect.
//! 3. State survival across sequential connections: connection A drives counter mutations, transport
//!    is lost / dropped, sessiond keeps state intact in `DETACHED`, and connection B attaches to the
//!    same session_id and committed state.
//! 4. Incarnation token uniqueness: every minted `session_id` is an opaque, globally unique token
//!    that never collides across process restarts.
//! 5. Replaced continuity: resuming against a restarted session daemon with an old session ID receives
//!    `ServerResyncRequired` with `continuity: REPLACED` and the fresh session ID.

mod common;
use common::*;

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::duplex;
use tokio_util::sync::CancellationToken;

use srui_protocol::{ClientResume, EventAckStatus, SessionContinuity};
use srui_sdk::*;
use srui_sessiond::{handle_connection, mint_session_id, ResumeOutcome, Session, SessionState};

#[tokio::test]
async fn test_session_state_lifecycle_attached_detached() {
    let session = Arc::new(Session::mint());
    assert_eq!(session.state(), SessionState::Detached);
    assert_eq!(session.attached_count(), 0);
    assert!(session.is_detached());
    assert!(!session.is_attached());

    // Connect Client 1
    let (client1, session_id1, rev1) =
        TestClientConnection::connect_fresh(session.clone(), &[1, 1]).await;
    assert_eq!(session_id1, session.session_id());
    assert_eq!(rev1, 0);

    // Yield to let the connection task attach at transport connect (§17)
    tokio::task::yield_now().await;
    assert_eq!(session.state(), SessionState::Attached);
    assert_eq!(session.attached_count(), 1);
    assert!(session.is_attached());
    assert!(!session.is_detached());

    // Connect Client 2 concurrently
    let (client2, session_id2, rev2) =
        TestClientConnection::connect_fresh(session.clone(), &[2, 2]).await;
    assert_eq!(session_id2, session.session_id());
    assert_eq!(rev2, 0);

    tokio::task::yield_now().await;
    assert_eq!(session.state(), SessionState::Attached);
    assert_eq!(session.attached_count(), 2);

    // Disconnect Client 1 -> remains Attached because Client 2 is still connected
    client1.drop_abruptly().await;
    tokio::task::yield_now().await;
    assert_eq!(session.state(), SessionState::Attached);
    assert_eq!(session.attached_count(), 1);

    // Disconnect Client 2 -> transitions to Detached
    client2.drop_abruptly().await;
    tokio::task::yield_now().await;
    assert_eq!(session.state(), SessionState::Detached);
    assert_eq!(session.attached_count(), 0);
    assert!(session.is_detached());
}

#[tokio::test]
async fn test_sessiond_sequential_connections_state_survival() {
    let session = Arc::new(Session::mint());
    let initial_session_id = session.session_id();
    let (_surface, text_id, progress_id, button_id) = setup_counter_session(&session);

    assert_eq!(session.current_revision(), 1);
    assert_eq!(session.state(), SessionState::Detached);

    // -------------------------------------------------------------------------
    // Connection A attaches, performs 3 clicks, then abruptly terminates
    // -------------------------------------------------------------------------
    let (mut conn_a, conn_a_sid, conn_a_rev) =
        TestClientConnection::connect_fresh(session.clone(), &[10, 20]).await;
    assert_eq!(conn_a_sid, initial_session_id);
    assert_eq!(conn_a_rev, 1);

    tokio::task::yield_now().await;
    assert_eq!(session.state(), SessionState::Attached);
    assert_eq!(session.attached_count(), 1);

    for seq in 1..=3 {
        conn_a
            .send_activate(&[10, 20], seq, &format!("click-{}", seq), seq, button_id)
            .await;

        let ack = conn_a.recv_event_ack().await;
        assert_eq!(ack.status(), EventAckStatus::Processed);
        assert_eq!(ack.last_processed_event_seq, seq);

        let tx = conn_a.recv_transaction().await;
        assert_eq!(tx.base_revision, seq);
        assert_eq!(tx.new_revision, seq + 1);
    }

    // Verify session reached revision 4 with Count: 3
    assert_eq!(session.current_revision(), 4);
    session.with_store(|store| {
        let text = Text::from_store(store, text_id).expect("text exists");
        assert_eq!(text.text(store), Some("Count: 3"));
        let prog = Progress::from_store(store, progress_id).expect("progress exists");
        assert_eq!(prog.value(store), Some(0.03));
    });

    // Simulate transport loss: Connection A dies
    conn_a.drop_abruptly().await;
    tokio::task::yield_now().await;

    // Session transitions to DETACHED; application state is preserved!
    assert_eq!(session.state(), SessionState::Detached);
    assert_eq!(session.attached_count(), 0);
    assert_eq!(session.current_revision(), 4);

    // -------------------------------------------------------------------------
    // Connection B attaches, verifies session_id and Count: 3 survived, then clicks
    // -------------------------------------------------------------------------
    let (mut conn_b, conn_b_sid, conn_b_rev) =
        TestClientConnection::connect_fresh(session.clone(), &[30, 40]).await;
    assert_eq!(
        conn_b_sid, initial_session_id,
        "session_id must survive across disconnect"
    );
    assert_eq!(
        conn_b_rev, 4,
        "committed revision 4 must survive across disconnect"
    );

    tokio::task::yield_now().await;
    assert_eq!(session.state(), SessionState::Attached);
    assert_eq!(session.attached_count(), 1);

    // Perform click 1 from Connection B (seq 1, observed revision 4)
    conn_b
        .send_activate(&[30, 40], 1, "click-b-1", 4, button_id)
        .await;

    let ack_b = conn_b.recv_event_ack().await;
    assert_eq!(ack_b.status(), EventAckStatus::Processed);
    assert_eq!(ack_b.last_processed_event_seq, 1);

    let tx_b = conn_b.recv_transaction().await;
    assert_eq!(tx_b.base_revision, 4);
    assert_eq!(tx_b.new_revision, 5);

    // Verify counter incremented to Count: 4
    assert_eq!(session.current_revision(), 5);
    session.with_store(|store| {
        let text = Text::from_store(store, text_id).expect("text exists");
        assert_eq!(text.text(store), Some("Count: 4"));
        let prog = Progress::from_store(store, progress_id).expect("progress exists");
        assert_eq!(prog.value(store), Some(0.04));
    });

    conn_b.drop_abruptly().await;
    tokio::task::yield_now().await;
    assert_eq!(session.state(), SessionState::Detached);
}

#[tokio::test]
async fn test_session_attached_while_awaiting_handshake() {
    let session = Arc::new(Session::mint());
    assert_eq!(session.state(), SessionState::Detached);

    let shutdown = CancellationToken::new();
    let (client_io, server_io) = duplex(1024 * 1024);
    let session_clone = session.clone();
    let shutdown_clone = shutdown.clone();
    let server_task =
        tokio::spawn(
            async move { handle_connection(server_io, session_clone, shutdown_clone).await },
        );

    // Server attached at transport connect and is blocked waiting for ClientHello (§17, App. B).
    tokio::task::yield_now().await;
    assert_eq!(session.state(), SessionState::Attached);
    assert_eq!(session.attached_count(), 1);

    drop(client_io);
    let _ = server_task.await;
    tokio::task::yield_now().await;
    assert_eq!(session.state(), SessionState::Detached);
    assert_eq!(session.attached_count(), 0);
}

#[test]
fn test_mint_session_id_produces_unique_tokens_in_process() {
    let mut ids = HashSet::new();
    const ITERATIONS: usize = 1_000;

    for _ in 0..ITERATIONS {
        let id = mint_session_id();
        assert_eq!(id.len(), 32, "session_id must be a 32-character hex token");
        assert!(
            id.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "session_id must be lowercase hex"
        );
        assert!(
            ids.insert(id),
            "mint_session_id produced a colliding session_id!"
        );
    }
    assert_eq!(ids.len(), ITERATIONS);
}

#[tokio::test]
async fn test_sessiond_process_restart_mints_unique_session_ids() {
    let mut seen = HashSet::new();
    const ITERATIONS: usize = 5;

    for iteration in 0..ITERATIONS {
        let socket_path = std::path::PathBuf::from(format!(
            "/tmp/srui-t-{}-{iteration}.sock",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&socket_path);

        let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_srui-sessiond"))
            .arg("--socket")
            .arg(&socket_path)
            .arg("--app")
            .arg("counter")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::inherit())
            .spawn()
            .expect("spawn srui-sessiond");

        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !socket_path.exists() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for sessiond socket at {}",
                socket_path.display()
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        let session_id = read_session_id_from_counter_sessiond(&socket_path).await;
        assert_eq!(session_id.len(), 32);
        assert!(
            seen.insert(session_id.clone()),
            "sessiond restart {iteration} reused session_id {session_id}"
        );

        child.kill().await.expect("kill sessiond");
        let status = child.wait().await.expect("wait for sessiond");
        assert!(!status.success(), "sessiond should exit after kill");
        let _ = std::fs::remove_file(&socket_path);
    }

    assert_eq!(seen.len(), ITERATIONS);
}

#[tokio::test]
async fn test_process_restart_replaced_continuity_on_old_session_id_resume() {
    // Session 1 (simulating process run 1)
    let session1 = Arc::new(Session::mint());
    let old_session_id = session1.session_id();

    // Session 2 (simulating process run 2 after crash/restart)
    let session2 = Arc::new(Session::mint());
    let new_session_id = session2.session_id();
    assert_ne!(old_session_id, new_session_id);

    // Client attempts to resume with old_session_id against the new sessiond process
    let resume = ClientResume {
        session_id: old_session_id.clone(),
        client_instance_id: vec![5, 5, 5],
        last_applied_revision: 2,
        last_acked_event_seq: 1,
        terminal_stream_offsets: Default::default(),
        limits: None,
        known_resource_hashes: vec![],
        pending_text_edits: vec![],
    };

    let outcome = session2
        .bootstrap_resume(&resume)
        .expect("bootstrap resume")
        .outcome;

    match outcome {
        ResumeOutcome::Resync {
            resync_msg,
            snapshot_transaction,
        } => {
            assert_eq!(
                resync_msg.session_id, new_session_id,
                "server must declare the replacement session ID"
            );
            assert_eq!(
                SessionContinuity::try_from(resync_msg.continuity),
                Ok(SessionContinuity::Replaced),
                "server must declare continuity as REPLACED (§17, §18)"
            );
            assert_eq!(resync_msg.last_processed_event_seq, 0);
            assert_eq!(snapshot_transaction.base_revision, 0);
        }
        ResumeOutcome::Replay { .. } => {
            panic!("restarted session daemon must never replay against mismatched session_id!");
        }
    }
}

#[tokio::test]
async fn test_terminal_session_state_rejects_attachment() {
    let session = Arc::new(Session::mint());
    assert_eq!(session.state(), SessionState::Detached);

    session.terminate();
    assert_eq!(session.state(), SessionState::Terminating);
    assert!(session.attach().is_none());

    let session_expired = Arc::new(Session::mint());
    session_expired.expire();
    assert_eq!(session_expired.state(), SessionState::Expired);
    assert!(session_expired.attach().is_none());
}
