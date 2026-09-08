//! Task 32 server-side event identifier limits (§26).

use srui_protocol::Event;
use srui_semantic_tree::DEFAULT_MAX_EVENT_ID_BYTES;
use srui_sessiond::{Session, SessionConfig, SessionError};

#[test]
fn oversized_event_id_is_rejected_before_deduplication_or_dispatch() {
    let session = Session::new("event-id-limit");
    let event = Event {
        event_id: vec![0x41; DEFAULT_MAX_EVENT_ID_BYTES + 1],
        ..Event::default()
    };

    let error = session
        .process_event(&event)
        .expect_err("oversized event id must fail closed");
    assert!(matches!(error, SessionError::InvalidInput(_)));
    assert!(error.to_string().contains("event_id"));
}

#[test]
fn event_id_limit_is_locally_configurable_but_must_remain_finite() {
    let session = Session::with_config(
        "tight-event-id-limit",
        SessionConfig {
            max_event_id_bytes: 4,
            ..SessionConfig::default()
        },
    );
    let event = Event {
        event_id: vec![0x41; 5],
        ..Event::default()
    };
    assert!(matches!(
        session.process_event(&event),
        Err(SessionError::InvalidInput(_))
    ));
}

#[test]
#[should_panic(expected = "max_event_id_bytes must be a positive integer")]
fn zero_event_id_limit_is_rejected_at_construction() {
    let _ = Session::with_config(
        "invalid-event-id-limit",
        SessionConfig {
            max_event_id_bytes: 0,
            ..SessionConfig::default()
        },
    );
}
