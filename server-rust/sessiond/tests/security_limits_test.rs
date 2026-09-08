//! Task 32 server-side event identifier limits (§18.2, §26).

use srui_protocol::Event;
use srui_sdk::Button;
use srui_semantic_tree::{Event as DomainEvent, WireError, MAX_EVENT_ID_BYTES};
use srui_sessiond::{EventOutcome, Session};

fn event(sequence: u64, event_id: Vec<u8>) -> Event {
    Event {
        client_instance_id: b"event-limit-client".to_vec(),
        event_seq: sequence,
        event_id,
        observed_revision: 1,
        node_id: 1,
        event_type: Some(srui_semantic_tree::TypeRef::EVENT_ACTIVATE.into()),
        ..Event::default()
    }
}

#[test]
fn wire_conversion_refuses_oversized_event_id_before_domain_construction() {
    let error = DomainEvent::try_from(event(1, vec![0x41; MAX_EVENT_ID_BYTES + 1]))
        .expect_err("oversized event id must not construct a domain EventId");
    assert!(matches!(
        error,
        WireError::EventIdTooLong {
            actual,
            limit: MAX_EVENT_ID_BYTES
        } if actual == MAX_EVENT_ID_BYTES + 1
    ));
}

#[test]
fn oversized_event_id_is_settled_as_rejected_without_blocking_the_frontier() {
    let session = Session::new("event-id-limit");
    session
        .transaction(|ui| {
            Button::builder(1).create(ui)?;
            Ok(())
        })
        .expect("seed interactive node");
    let oversized = event(1, vec![0x41; MAX_EVENT_ID_BYTES + 1]);

    let rejected = session
        .process_event(&oversized)
        .expect("oversized event is a settled validation rejection");
    assert!(matches!(
        rejected,
        EventOutcome::Rejected {
            last_processed_event_seq: 1,
            ..
        }
    ));

    let later = session
        .process_event(&event(2, b"bounded-id".to_vec()))
        .expect("later bounded event remains processable");
    assert!(matches!(
        later,
        EventOutcome::Processed {
            last_processed_event_seq: 2,
            ..
        }
    ));

    // The internal marker is digest-bound, so replaying *this* identifier is a genuine replay and
    // is answered from the result cache, exactly as a valid identifier would be.
    let replay = session
        .process_event(&oversized)
        .expect("replayed oversized event returns its settled outcome");
    assert!(matches!(
        replay,
        EventOutcome::Duplicate {
            accepted: false,
            last_processed_event_seq: 2,
            ..
        }
    ));
}

/// Two *different* oversized identifiers at one `event_seq` must not collapse onto one internal
/// identity. A pair of valid identifiers in that position raises `SequenceAlreadyAssigned`;
/// malformed input has to fail the same way rather than being answered `Duplicate` (§4 inv. 13).
#[test]
fn distinct_oversized_event_ids_at_one_sequence_do_not_coalesce() {
    let session = Session::new("event-id-collision");
    session
        .transaction(|ui| {
            Button::builder(1).create(ui)?;
            Ok(())
        })
        .expect("seed interactive node");

    let first = session
        .process_event(&event(1, vec![0x41; MAX_EVENT_ID_BYTES + 1]))
        .expect("first oversized event settles");
    assert!(matches!(first, EventOutcome::Rejected { .. }));

    let conflict = session
        .process_event(&event(1, vec![0x42; MAX_EVENT_ID_BYTES + 9]))
        .expect_err("a different identifier at an assigned sequence is a client protocol error");
    assert!(
        conflict.to_string().contains("already assigned"),
        "expected the same conflict a pair of valid identifiers raises, got: {conflict}"
    );

    // The valid-identifier control: identical wire shape, identical outcome.
    let session = Session::new("event-id-control");
    session
        .transaction(|ui| {
            Button::builder(1).create(ui)?;
            Ok(())
        })
        .expect("seed interactive node");
    session
        .process_event(&event(1, b"bounded-a".to_vec()))
        .expect("first bounded event settles");
    let valid_conflict = session
        .process_event(&event(1, b"bounded-b".to_vec()))
        .expect_err("distinct valid identifiers at one sequence conflict");
    assert_eq!(valid_conflict.to_string(), conflict.to_string());
}

/// `srui.proto` declares `event_id` non-empty. Rust and Swift must refuse the same wire bytes,
/// so an absent identifier is a decode failure rather than a zero-length deduplication key.
#[test]
fn wire_conversion_refuses_empty_event_id() {
    let error = DomainEvent::try_from(event(1, Vec::new()))
        .expect_err("an empty event_id must not construct a domain EventId");
    assert!(
        matches!(error, WireError::MissingField("Event.event_id")),
        "unexpected error: {error}"
    );
}
