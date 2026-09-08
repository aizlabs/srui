//! Task 32 server-side event identifier limits (§18.2, §26).

use srui_protocol::Event;
use srui_semantic_tree::{Event as DomainEvent, WireError, MAX_EVENT_ID_BYTES};
use srui_sessiond::{EventOutcome, Session};

fn event(sequence: u64, event_id: Vec<u8>) -> Event {
    Event {
        client_instance_id: b"event-limit-client".to_vec(),
        event_seq: sequence,
        event_id,
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
        EventOutcome::Rejected {
            last_processed_event_seq: 2,
            ..
        }
    ));

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
