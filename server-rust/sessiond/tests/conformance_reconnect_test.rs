//! SRUI Reconnect Conformance Suite (§32 item 8).
//!
//! Implements: §18 (reconnect and continuity), §18.1 (journal retention), §18.2 (event dedupe and
//! settlement), §18.3 (pending text edits), §21 (terminal replay boundaries), §32.8.
//!
//! §32 item 8 lists "replay, snapshot, partial transaction discard, pending-edit reconciliation,
//! event dedupe" — and, since Tasks 23–24, the continuity and settlement protocol on top of that.
//! Those behaviors were previously proven across `session_resume_test.rs`,
//! `session_resync_test.rs`, `event_delivery_test.rs` and `retention_invariant_test.rs`, which
//! remain valuable fine-grained coverage. This file is the single *named, independently runnable*
//! suite that walks the §32.8 checklist end to end.
//!
//! The wire harness (`ResumeConnection`) lives in `tests/common/mod.rs` and is shared with
//! `session_resume_test.rs`, so each behavior is asserted against one harness rather than being
//! re-implemented per file.
//!
//! Scenario checklist (numbering matches the manifest entry for suite 8):
//!
//!  1. retained-journal replay
//!  2. snapshot resync outside retention
//!  3. failed/partial transactions are never journaled or replayed
//!  4. pending text-edit reconciliation across reconnect
//!  5. a lost ACK replays the cached outcome without repeating the side effect
//!  6. `SAME_SESSION` continuity keeps pending events alive
//!  7. `REPLACED` continuity abandons events and edits
//!  8. an unrecognized continuity fails closed
//!  9. resume-attempt supersession (run via the continuity suites — see the note below)
//! 10. overlapping delivery stays unacknowledged while the original is in flight
//! 11. sequence 2 settling before sequence 1 leaves the frontier at 0
//! 12. settling sequence 1 advances the frontier directly through sequence 2
//! 13. selective settlement removes sequence 2 but does not cross the sequence-1 gap
//! 14. wrong session / wrong client acknowledgements cannot settle the active outbox

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use srui_protocol::{Event as WireEvent, SessionContinuity};
use srui_sdk::{Button, NodeId, Surface, Text, ACTIVATE, LABEL};
use srui_semantic_tree::{Event, PropertyRef, Value};
use srui_sessiond::{EventOutcome, Session, SessionConfig};

#[allow(dead_code)]
mod common;

use common::{make_tx, ResumeConnection};

const ROOT: u64 = 1;
const TEXT_NODE: u64 = 2;
const BUTTON: u64 = 3;

fn counter_session(name: &str, config: SessionConfig) -> Arc<Session> {
    let session = Arc::new(Session::with_config(name, config));
    session
        .transaction(|ui| {
            Surface::builder(NodeId::new(ROOT)).create(ui)?;
            Text::builder(NodeId::new(TEXT_NODE))
                .parent(NodeId::new(ROOT))
                .label("v0")
                .create(ui)?;
            Button::builder(NodeId::new(BUTTON))
                .parent(NodeId::new(ROOT))
                .label("Go")
                .create(ui)?;
            Ok(())
        })
        .expect("initial tree");
    session
}

/// An `ACTIVATE` delivery from `client` at `seq`, built through the same SDK path applications
/// and the wire decoder use, so the suite exercises real event construction rather than a
/// hand-assembled protobuf.
fn event(client: &[u8], event_id: &str, seq: u64, node: u64) -> WireEvent {
    Event::activate(seq, event_id, 1u64, NodeId::new(node))
        .with_client_instance_id(client.to_vec())
        .to_wire()
}

fn last_seq(outcome: &EventOutcome) -> u64 {
    match outcome {
        EventOutcome::Processed {
            last_processed_event_seq,
            ..
        }
        | EventOutcome::Pending {
            last_processed_event_seq,
        }
        | EventOutcome::Duplicate {
            last_processed_event_seq,
            ..
        }
        | EventOutcome::Rejected {
            last_processed_event_seq,
            ..
        } => *last_processed_event_seq,
    }
}

// ==============================================================================
// 1–2. Replay inside retention; snapshot outside it (§18, §18.1)
// ==============================================================================

/// Scenario 1: a reconnect inside the retention window replays the exact missing range.
#[tokio::test]
async fn scenario_01_retained_journal_replay() {
    let session = Arc::new(Session::new("conformance-replay"));
    for base in 0..8 {
        session.commit_transaction(make_tx(base)).expect("commit");
    }

    let replayed = session
        .collect_replayed_transactions(3)
        .expect("replay from a retained revision");
    assert_eq!(replayed.len(), 5, "revisions 3..8 must replay");
    assert_eq!(replayed.first().unwrap().base_revision, 3);
    assert_eq!(replayed.last().unwrap().new_revision, 8);

    let mut conn = ResumeConnection::open(session).await;
    conn.send_resume("conformance-replay", 3).await;
    conn.expect_resume_ok("conformance-replay", 3).await;
    for base in 3..8 {
        conn.expect_transaction(base, base + 1).await;
    }
    conn.expect_no_message(Duration::from_millis(80)).await;
    conn.close().await;
}

/// Scenario 2: a reconnect *outside* retention gets a snapshot, and the continuity is still
/// `SAME_SESSION` — the incarnation survived, only the journal window did not (§18.1).
#[tokio::test]
async fn scenario_02_snapshot_resync_outside_retention() {
    let session = Arc::new(Session::with_config(
        "conformance-retention",
        SessionConfig {
            journal_capacity: 4,
            ..SessionConfig::default()
        },
    ));
    for base in 0..12 {
        session.commit_transaction(make_tx(base)).expect("commit");
    }

    assert!(
        session.collect_replayed_transactions(0).is_err(),
        "revision 0 must have fallen out of a 4-transaction journal"
    );

    let mut conn = ResumeConnection::open(session).await;
    conn.send_resume("conformance-retention", 0).await;
    conn.expect_resync("conformance-retention", SessionContinuity::SameSession, 12)
        .await;
    conn.close().await;
}

/// Scenario 3: a transaction refused mid-apply is never journaled, so it can never be replayed.
/// The store cannot roll back a committed revision, so a partially journaled failure would
/// strand every future reconnect.
#[tokio::test]
async fn scenario_03_partial_transaction_never_journaled_or_replayed() {
    let session = Arc::new(Session::new("conformance-partial"));
    session.commit_transaction(make_tx(0)).expect("commit");
    let revision_before = session.current_revision();
    let journal_before = session
        .collect_replayed_transactions(0)
        .expect("journal window");

    // A transaction whose second operation cannot apply: the node does not exist.
    let mut bad = make_tx(revision_before);
    bad.operations = vec![srui_protocol::Operation {
        op: Some(srui_protocol::operation::Op::SetProperty(
            srui_protocol::SetPropertyOp {
                node_id: 9_999,
                property: Some(srui_protocol::PropertyRef {
                    namespace_id: 0,
                    local_id: PropertyRef::LABEL.local_id,
                }),
                value: Some(srui_protocol::Value {
                    value: Some(srui_protocol::value::Value::StringValue("nope".into())),
                }),
            },
        )),
    }];

    session
        .commit_transaction(bad)
        .expect_err("commit against a missing node must be refused");

    assert_eq!(
        session.current_revision(),
        revision_before,
        "a refused commit must not consume a revision"
    );
    assert_eq!(
        session
            .collect_replayed_transactions(0)
            .expect("journal window"),
        journal_before,
        "a refused commit must leave the journal byte-for-byte unchanged"
    );
}

// ==============================================================================
// 4–5. Pending edits and the event result cache (§18.2, §18.3)
// ==============================================================================

/// Scenario 4: a client reconnects still holding unacknowledged text edits. §18.3 requires the
/// server to echo those exact edits back as `discarded_text_edits`, so the client can settle them
/// without opening a global `event_seq` gap — and the authoritative text wins.
///
/// Declaring the edits is the whole point: a resume with an empty `pending_text_edits` never
/// enters the reconciliation path at all.
#[tokio::test]
async fn scenario_04_pending_text_edits_are_reconciled_on_resync() {
    let session = counter_session(
        "conformance-pending-edit",
        SessionConfig {
            // Small journal so the reconnect below is forced onto the snapshot/resync path,
            // which is where §18.3 cancellation happens.
            journal_capacity: 2,
            ..SessionConfig::default()
        },
    );

    session
        .transaction(|ui| {
            ui.set(
                NodeId::new(TEXT_NODE),
                LABEL,
                Value::from("server-authoritative"),
            )?;
            Ok(())
        })
        .expect("server-side correction");
    for base in session.current_revision()..session.current_revision() + 6 {
        session.commit_transaction(make_tx(base)).expect("commit");
    }

    let pending = vec![srui_protocol::PendingTextEditRef {
        event_id: b"pending-edit-1".to_vec(),
        event_seq: 1,
        node_id: TEXT_NODE,
        edit_seq: 7,
    }];

    let mut conn = ResumeConnection::open(session.clone()).await;
    conn.send_resume_with_pending_edits("conformance-pending-edit", 0, pending.clone())
        .await;
    let discarded = conn
        .expect_resync_discarding("conformance-pending-edit", SessionContinuity::SameSession)
        .await;
    conn.close().await;

    assert_eq!(
        discarded, pending,
        "§18.3: a same-session forced resync must echo the client's declared pending text edits \
         so they can be settled without opening an event_seq gap"
    );

    assert_eq!(
        session.with_store(|store| store
            .get_node(srui_semantic_tree::NodeId::new(TEXT_NODE))
            .and_then(|n| n.get_property(PropertyRef::LABEL))
            .cloned()),
        Some(srui_semantic_tree::Value::String(
            "server-authoritative".to_string()
        )),
        "reconnect must converge on the authoritative text, never a replayed client edit"
    );
}

/// Scenario 5: the client never saw the ACK and retries. §18.2 requires the cached outcome to be
/// returned rather than the action running a second time.
#[test]
fn scenario_05_lost_ack_replays_cached_outcome_without_repeating_the_effect() {
    let session = counter_session("conformance-lost-ack", SessionConfig::default());
    let client = b"client-a".to_vec();

    let first = session.process_event(&event(&client, "evt-1", 1, BUTTON));
    let revision_after_first = session.current_revision();
    assert!(
        matches!(first, Ok(EventOutcome::Processed { .. })),
        "first delivery must be processed, got {first:?}"
    );

    let retry = session
        .process_event(&event(&client, "evt-1", 1, BUTTON))
        .expect("retry is answered");
    match retry {
        EventOutcome::Duplicate {
            accepted,
            revision_after_effect,
            ..
        } => {
            assert!(accepted, "the original attempt was accepted");
            assert_eq!(
                revision_after_effect, revision_after_first,
                "a duplicate must echo the original revision, not advance it"
            );
        }
        other => panic!("expected Duplicate from the result cache, got {other:?}"),
    }

    assert_eq!(
        session.current_revision(),
        revision_after_first,
        "replaying a settled event must not re-run its side effect"
    );
}

// ==============================================================================
// 6–9. Continuity outcomes and resume supersession (§18, Task 23)
// ==============================================================================

/// Scenario 6: `SAME_SESSION` means the incarnation survived, so a settled event's cached result
/// is still reachable after the reconnect.
#[tokio::test]
async fn scenario_06_same_session_continuity_preserves_event_history() {
    let session = counter_session(
        "conformance-same-session",
        SessionConfig {
            journal_capacity: 2,
            ..SessionConfig::default()
        },
    );
    let client = b"client-same".to_vec();
    session
        .process_event(&event(&client, "evt-keep", 1, BUTTON))
        .expect("event processed");

    for base in session.current_revision()..session.current_revision() + 6 {
        session.commit_transaction(make_tx(base)).expect("commit");
    }

    let mut conn = ResumeConnection::open(session.clone()).await;
    conn.send_resume("conformance-same-session", 0).await;
    conn.expect_resync(
        "conformance-same-session",
        SessionContinuity::SameSession,
        session.current_revision(),
    )
    .await;
    conn.close().await;

    let replay = session
        .process_event(&event(&client, "evt-keep", 1, BUTTON))
        .expect("replay answered");
    assert!(
        matches!(replay, EventOutcome::Duplicate { .. }),
        "a SAME_SESSION reconnect must keep the settled event window, got {replay:?}"
    );
}

/// Scenario 7: resuming a *different* session id is a replaced incarnation. The server must say
/// `REPLACED` — the client's pending events and edits belong to a session that no longer exists.
#[tokio::test]
async fn scenario_07_replaced_continuity_abandons_client_state() {
    let session = Arc::new(Session::new("conformance-live"));
    session.commit_transaction(make_tx(0)).expect("commit");

    let mut conn = ResumeConnection::open(session.clone()).await;
    conn.send_resume("conformance-dead-incarnation", 1).await;
    conn.expect_resync(
        "conformance-live",
        SessionContinuity::Replaced,
        session.current_revision(),
    )
    .await;
    conn.close().await;
}

/// Scenario 8: every resync states a continuity the client can act on. `UNSPECIFIED` must never
/// be sent — an unknown required semantic fails explicitly rather than degrading (§4 inv. 13).
///
/// `expect_resync` asserts this on every resync path; this test pins it as a named scenario over
/// both continuity outcomes so the guarantee cannot be quietly dropped from the harness.
#[tokio::test]
async fn scenario_08_unknown_continuity_fails_closed() {
    let session = Arc::new(Session::with_config(
        "conformance-continuity",
        SessionConfig {
            journal_capacity: 2,
            ..SessionConfig::default()
        },
    ));
    for base in 0..8 {
        session.commit_transaction(make_tx(base)).expect("commit");
    }

    // Same incarnation, journal gap -> SAME_SESSION.
    let mut conn = ResumeConnection::open(session.clone()).await;
    conn.send_resume("conformance-continuity", 0).await;
    conn.expect_resync("conformance-continuity", SessionContinuity::SameSession, 8)
        .await;
    conn.close().await;

    // Different incarnation -> REPLACED. Neither path may answer UNSPECIFIED.
    let mut conn = ResumeConnection::open(session.clone()).await;
    conn.send_resume("some-other-incarnation", 0).await;
    conn.expect_resync("conformance-continuity", SessionContinuity::Replaced, 8)
        .await;
    conn.close().await;
}

// Scenario 9 (resume-attempt supersession) is deliberately not re-implemented here.
//
// Two resume attempts against the same live incarnation both legitimately receive RESUME_OK and
// both keep receiving broadcasts, so a server-side "older attempt is superseded" assertion has
// nothing to observe — an earlier version of this file asserted exactly that and proved nothing.
//
// The behaviour is real, and it is proven where it lives:
//   * client-side attempt supersession — `SessionResumeContinuityTests` ("A superseded REPLACED
//     response cannot abandon intents bound to a newer attempt", "A superseded RESUME_OK never
//     replays, rebinds, or re-enables event allocation");
//   * server-side generation supersession — `text_edit_test.rs`, which drives
//     `EventValidationError::SupersededGeneration`.
//
// Both are listed as suite 8 runners in protocol/conformance-vectors/suites/manifest.json, so the
// named suite executes them rather than restating them.

// ==============================================================================
// 10–14. Event settlement and frontier ordering (§18.2, Task 24)
// ==============================================================================

/// Scenario 10: while the original delivery is still in flight, an overlapping copy of the same
/// `event_id` is answered `Pending` — explicitly non-terminal, so it produces no acknowledgement
/// and the client keeps waiting instead of assuming the action succeeded (§18.2).
#[test]
fn scenario_10_overlapping_delivery_is_not_acknowledged_while_in_flight() {
    let session = counter_session("conformance-in-flight", SessionConfig::default());
    let client = b"client-overlap".to_vec();

    // The handler blocks so the first delivery is provably still dispatching when the overlapping
    // copy arrives on another thread.
    let started = Arc::new(AtomicBool::new(false));
    let release = Arc::new((Mutex::new(false), Condvar::new()));
    let handler_started = Arc::clone(&started);
    let handler_release = Arc::clone(&release);
    session.on(NodeId::new(BUTTON), ACTIVATE, move |_, _| {
        handler_started.store(true, Ordering::SeqCst);
        let (lock, wake) = &*handler_release;
        let mut released = lock.lock().unwrap_or_else(|error| error.into_inner());
        while !*released {
            released = wake
                .wait(released)
                .unwrap_or_else(|error| error.into_inner());
        }
    });

    let original = {
        let session = session.clone();
        let client = client.clone();
        std::thread::spawn(move || session.process_event(&event(&client, "evt-overlap", 1, BUTTON)))
    };

    while !started.load(Ordering::SeqCst) {
        std::thread::yield_now();
    }

    let overlapping = session
        .process_event(&event(&client, "evt-overlap", 1, BUTTON))
        .expect("overlapping delivery is answered");
    assert!(
        matches!(overlapping, EventOutcome::Pending { .. }),
        "an overlapping delivery must be Pending while the original is in flight, got {overlapping:?}"
    );
    assert_eq!(
        last_seq(&overlapping),
        0,
        "a Pending outcome must not advance the settled frontier"
    );

    let (lock, wake) = &*release;
    *lock.lock().unwrap_or_else(|error| error.into_inner()) = true;
    wake.notify_all();

    let settled = original
        .join()
        .expect("original delivery thread")
        .expect("original delivery is answered");
    assert!(
        matches!(settled, EventOutcome::Processed { .. }),
        "the original delivery is the one that runs the action, got {settled:?}"
    );

    // Now that it is settled, the same replay is answered from the result cache instead.
    let replay = session
        .process_event(&event(&client, "evt-overlap", 1, BUTTON))
        .expect("post-settlement replay");
    assert!(
        matches!(replay, EventOutcome::Duplicate { .. }),
        "a settled event replays as Duplicate, got {replay:?}"
    );
}

/// Scenario 11: sequence 2 settles first. The frontier is the highest *contiguous* settled
/// sequence, so it stays at 0 while sequence 1 is still missing.
#[test]
fn scenario_11_out_of_order_settlement_leaves_the_frontier_behind_the_gap() {
    let session = counter_session("conformance-frontier-gap", SessionConfig::default());
    let client = b"client-frontier".to_vec();

    let second = session
        .process_event(&event(&client, "evt-2", 2, BUTTON))
        .expect("sequence 2 settles");
    assert_eq!(
        last_seq(&second),
        0,
        "settling sequence 2 before sequence 1 must not advance the frontier past the gap"
    );
}

/// Scenario 12: settling sequence 1 closes the gap, and the frontier jumps straight through the
/// already-settled sequence 2 in one step.
#[test]
fn scenario_12_closing_the_gap_advances_the_frontier_through_settled_sequences() {
    let session = counter_session("conformance-frontier-close", SessionConfig::default());
    let client = b"client-frontier".to_vec();

    let second = session
        .process_event(&event(&client, "evt-2", 2, BUTTON))
        .expect("sequence 2 settles");
    assert_eq!(last_seq(&second), 0);

    let first = session
        .process_event(&event(&client, "evt-1", 1, BUTTON))
        .expect("sequence 1 settles");
    assert_eq!(
        last_seq(&first),
        2,
        "settling sequence 1 must advance the frontier through the settled sequence 2"
    );
}

/// Scenario 13: settlement is per `event_id`. Settling sequence 2 for one client must not be
/// mistaken for progress across the sequence-1 gap, and a third sequence stays behind it too.
#[test]
fn scenario_13_selective_settlement_does_not_cross_the_gap() {
    let session = counter_session("conformance-frontier-selective", SessionConfig::default());
    let client = b"client-frontier".to_vec();

    let second = session
        .process_event(&event(&client, "evt-2", 2, BUTTON))
        .expect("sequence 2 settles");
    let third = session
        .process_event(&event(&client, "evt-3", 3, BUTTON))
        .expect("sequence 3 settles");

    assert_eq!(last_seq(&second), 0);
    assert_eq!(
        last_seq(&third),
        0,
        "sequences 2 and 3 are settled but sequence 1 is still missing, so the frontier holds at 0"
    );

    let first = session
        .process_event(&event(&client, "evt-1", 1, BUTTON))
        .expect("sequence 1 settles");
    assert_eq!(
        last_seq(&first),
        3,
        "closing the gap advances the frontier through every contiguous settled sequence"
    );
}

/// Scenario 14: acknowledgements are scoped to a client instance. Another client's identical
/// sequence numbers must not settle this client's outbox — dedupe windows are per client (§18.2).
#[test]
fn scenario_14_other_clients_cannot_settle_this_clients_outbox() {
    let session = counter_session("conformance-frontier-isolation", SessionConfig::default());
    let alice = b"client-alice".to_vec();
    let bob = b"client-bob".to_vec();

    // Bob settles sequences 1 and 2 completely.
    let bob_first = session
        .process_event(&event(&bob, "bob-1", 1, BUTTON))
        .expect("bob sequence 1");
    let bob_second = session
        .process_event(&event(&bob, "bob-2", 2, BUTTON))
        .expect("bob sequence 2");
    assert_eq!(last_seq(&bob_first), 1);
    assert_eq!(last_seq(&bob_second), 2);

    // Alice has only settled sequence 2; Bob's progress must not close her gap.
    let alice_second = session
        .process_event(&event(&alice, "alice-2", 2, BUTTON))
        .expect("alice sequence 2");
    assert_eq!(
        last_seq(&alice_second),
        0,
        "another client's settled sequences must not advance this client's frontier"
    );

    // And a replay of Bob's event under Alice's identity is a *fresh* event, not a duplicate.
    let cross = session
        .process_event(&event(&alice, "bob-1", 1, BUTTON))
        .expect("cross-client replay");
    assert!(
        matches!(cross, EventOutcome::Processed { .. }),
        "dedupe windows are per client instance, got {cross:?}"
    );
}
