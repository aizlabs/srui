//! Authoritative `TEXT_EDIT` processing: sequence watermarks, policy, and generation barriers
//! (§18.3, §22.6, §26, §27).

use std::sync::{Arc, Barrier};
use std::thread;

use srui_protocol::Event as WireEvent;
use srui_sdk::{Surface, TextInput};
use srui_semantic_tree::{
    EditSeq, Event as DomainEvent, EventValidationError, NodeId, PropertyRef,
    StandardValidationState, Value,
};
use srui_sessiond::{EventOutcome, Session, TextEditDecision, MAX_TEXT_EDIT_STREAMS};

const CLIENT: &[u8] = b"text-client";
const EDITOR: u64 = 2;

fn seed_editor(session: &Session) {
    session
        .transaction(|ui| {
            Surface::builder(1).label("Editor").create(ui)?;
            TextInput::builder(EDITOR).parent(1).value("").create(ui)?;
            Ok(())
        })
        .expect("seed text input");
}

fn edit_seq(n: u64) -> EditSeq {
    EditSeq::new(n).expect("positive edit_seq")
}

fn text_edit(event_seq: u64, event_id: &str, text: &str, seq: u64) -> WireEvent {
    DomainEvent::text_edit(event_seq, event_id, 0u64, EDITOR, text, edit_seq(seq))
        .with_client_instance_id(CLIENT.to_vec())
        .to_wire()
}

fn editor_value(session: &Session) -> String {
    session.with_store(|store| {
        store
            .get_node(NodeId::new(EDITOR))
            .and_then(|node| {
                node.get_property(PropertyRef::VALUE)
                    .and_then(Value::as_string)
            })
            .unwrap_or("")
            .to_string()
    })
}

fn editor_validation(session: &Session) -> Option<StandardValidationState> {
    session.with_store(|store| {
        store
            .get_node(NodeId::new(EDITOR))
            .and_then(|node| node.get_property(PropertyRef::VALIDATION_STATE))
            .and_then(Value::as_enum_token)
            .and_then(|token| StandardValidationState::try_from(token).ok())
    })
}

#[test]
fn duplicate_gap_and_stale_edit_sequences() {
    let session = Session::new("text-seq");
    seed_editor(&session);

    let first = text_edit(1, "e1", "one", 1);
    match session.process_event(&first).expect("seq 1") {
        EventOutcome::Processed { .. } => {}
        other => panic!("expected processed, got {other:?}"),
    }
    assert_eq!(editor_value(&session), "one");

    match session.process_event(&first).expect("duplicate 1") {
        EventOutcome::Duplicate { accepted: true, .. } => {}
        other => panic!("expected idempotent duplicate, got {other:?}"),
    }
    assert_eq!(editor_value(&session), "one");

    let gap = text_edit(2, "e3", "three", 3);
    match session.process_event(&gap).expect("seq 3") {
        EventOutcome::Processed { .. } => {}
        other => panic!("expected gap accepted, got {other:?}"),
    }
    assert_eq!(editor_value(&session), "three");

    let stale = text_edit(3, "e2", "two", 2);
    match session.process_event(&stale).expect("seq 2") {
        EventOutcome::Rejected {
            error:
                EventValidationError::StaleEditSeq {
                    observed: 2,
                    watermark: 3,
                },
            ..
        } => {}
        other => panic!("expected stale rejection, got {other:?}"),
    }
    assert_eq!(editor_value(&session), "three");
}

#[test]
fn older_validation_cannot_publish_after_newer_generation() {
    let session = Session::new("text-generation");
    seed_editor(&session);

    let first_entered = Arc::new(Barrier::new(2));
    let release_old = Arc::new(Barrier::new(2));

    session.on_text_edit({
        let first_entered = Arc::clone(&first_entered);
        let release_old = Arc::clone(&release_old);
        move |_, request| {
            if request.edit_seq.get() == 1 {
                first_entered.wait();
                release_old.wait();
            }
            TextEditDecision::Accept
        }
    });

    let session_old = session.clone();
    let old =
        thread::spawn(move || session_old.process_event(&text_edit(1, "old", "old-value", 1)));

    first_entered.wait();

    let newer = session
        .process_event(&text_edit(2, "new", "new-value", 2))
        .expect("newer edit");
    match newer {
        EventOutcome::Processed { .. } => {}
        other => panic!("expected newer edit processed, got {other:?}"),
    }
    assert_eq!(editor_value(&session), "new-value");

    release_old.wait();
    let older = old.join().expect("old thread").expect("old result");
    match older {
        EventOutcome::Rejected {
            error: EventValidationError::StaleEditSeq { .. },
            ..
        } => {}
        other => panic!("expected older generation rejected, got {other:?}"),
    }
    assert_eq!(
        editor_value(&session),
        "new-value",
        "older validator must not overwrite the newer published value"
    );
}

#[test]
fn reject_publishes_error_validation_and_authoritative_value() {
    let session = Session::new("text-reject");
    seed_editor(&session);
    session.on_text_edit(|_, _| TextEditDecision::Reject {
        value: Some("corrected".into()),
        reason: "not allowed".into(),
    });

    match session
        .process_event(&text_edit(1, "bad", "nope", 1))
        .expect("reject")
    {
        EventOutcome::Rejected {
            error: EventValidationError::PolicyRejected(reason),
            ..
        } => assert_eq!(reason, "not allowed"),
        other => panic!("expected policy rejection, got {other:?}"),
    }
    assert_eq!(editor_value(&session), "corrected");
    assert_eq!(
        editor_validation(&session),
        Some(StandardValidationState::Error)
    );
}

#[test]
fn default_policy_accepts_and_marks_valid() {
    let session = Session::new("text-accept");
    seed_editor(&session);
    match session
        .process_event(&text_edit(1, "ok", "hello", 1))
        .expect("accept")
    {
        EventOutcome::Processed { .. } => {}
        other => panic!("expected processed, got {other:?}"),
    }
    assert_eq!(editor_value(&session), "hello");
    assert_eq!(
        editor_validation(&session),
        Some(StandardValidationState::Valid)
    );
}

#[test]
fn deleted_editor_reclaims_monotonicity_state() {
    let session = Session::new("text-reclaim");
    seed_editor(&session);
    session
        .process_event(&text_edit(1, "first", "keep", 1))
        .expect("first edit");

    session
        .transaction(|ui| {
            ui.delete(NodeId::new(EDITOR))?;
            Ok(())
        })
        .expect("delete editor");

    match session
        .process_event(&text_edit(2, "gone", "stale", 2))
        .expect("deleted target")
    {
        EventOutcome::Rejected {
            error: EventValidationError::NodeNotFound(id),
            ..
        } => assert_eq!(id, NodeId::new(EDITOR)),
        other => panic!("deleted editor must reject TEXT_EDIT, got {other:?}"),
    }

    // NodeIds are never reused in a session (§6.2). Reclaiming the stream must free the
    // tracker slot so a *new* editor identity can start at edit_seq 1.
    const NEW_EDITOR: u64 = 3;
    session
        .transaction(|ui| {
            TextInput::builder(NEW_EDITOR)
                .parent(1)
                .value("")
                .create(ui)?;
            Ok(())
        })
        .expect("create replacement editor");

    let fresh = DomainEvent::text_edit(3, "again", 0u64, NEW_EDITOR, "fresh", edit_seq(1))
        .with_client_instance_id(CLIENT.to_vec())
        .to_wire();
    match session.process_event(&fresh).expect("new editor stream") {
        EventOutcome::Processed { .. } => {}
        other => panic!("new editor must accept edit_seq 1, got {other:?}"),
    }
    assert_eq!(
        session.with_store(|store| {
            store
                .get_node(NodeId::new(NEW_EDITOR))
                .and_then(|node| {
                    node.get_property(PropertyRef::VALUE)
                        .and_then(Value::as_string)
                })
                .unwrap_or("")
                .to_string()
        }),
        "fresh"
    );
}

#[test]
fn tracker_full_refuses_a_new_editor_stream() {
    let session = Session::new("text-full");
    session
        .transaction(|ui| {
            Surface::builder(1).create(ui)?;
            for id in 0..MAX_TEXT_EDIT_STREAMS {
                TextInput::builder((id as u64) + 2)
                    .parent(1)
                    .value("")
                    .create(ui)?;
            }
            Ok(())
        })
        .expect("seed full set of editors");

    for id in 0..MAX_TEXT_EDIT_STREAMS {
        let node = (id as u64) + 2;
        let event = DomainEvent::text_edit(
            id as u64 + 1,
            format!("id-{id}"),
            0u64,
            node,
            "x",
            edit_seq(1),
        )
        .with_client_instance_id(CLIENT.to_vec())
        .to_wire();
        match session.process_event(&event).expect("fill") {
            EventOutcome::Processed { .. } => {}
            other => panic!("expected processed while filling tracker, got {other:?}"),
        }
    }

    session
        .transaction(|ui| {
            TextInput::builder((MAX_TEXT_EDIT_STREAMS as u64) + 2)
                .parent(1)
                .value("")
                .create(ui)?;
            Ok(())
        })
        .expect("one more editor");

    let overflow = DomainEvent::text_edit(
        MAX_TEXT_EDIT_STREAMS as u64 + 1,
        "overflow",
        0u64,
        (MAX_TEXT_EDIT_STREAMS as u64) + 2,
        "x",
        edit_seq(1),
    )
    .with_client_instance_id(CLIENT.to_vec())
    .to_wire();
    match session.process_event(&overflow).expect("overflow") {
        EventOutcome::Rejected {
            error: EventValidationError::TextTrackerFull { limit },
            ..
        } => assert_eq!(limit, MAX_TEXT_EDIT_STREAMS),
        other => panic!("expected tracker full, got {other:?}"),
    }

    session
        .transaction(|ui| {
            ui.delete(NodeId::new(2))?;
            Ok(())
        })
        .expect("reclaim one editor stream");

    let replacement_id = (MAX_TEXT_EDIT_STREAMS as u64) + 2;
    let retry = DomainEvent::text_edit(
        MAX_TEXT_EDIT_STREAMS as u64 + 2,
        "after-reclaim",
        0u64,
        replacement_id,
        "reclaimed",
        edit_seq(1),
    )
    .with_client_instance_id(CLIENT.to_vec())
    .to_wire();
    match session.process_event(&retry).expect("after reclaim") {
        EventOutcome::Processed { .. } => {}
        other => panic!("deleting an editor must free a tracker slot, got {other:?}"),
    }
    assert_eq!(
        session.with_store(|store| {
            store
                .get_node(NodeId::new(replacement_id))
                .and_then(|node| {
                    node.get_property(PropertyRef::VALUE)
                        .and_then(Value::as_string)
                })
                .unwrap_or("")
                .to_string()
        }),
        "reclaimed"
    );
}

#[test]
fn non_text_event_with_edit_seq_is_rejected() {
    let session = Session::new("text-invalid-seq");
    seed_editor(&session);
    let mut activate =
        DomainEvent::activate(1, "act", 0u64, EDITOR).with_client_instance_id(CLIENT.to_vec());
    activate.edit_seq = Some(edit_seq(1));
    match session
        .process_event(&activate.to_wire())
        .expect("activate")
    {
        EventOutcome::Rejected {
            error: EventValidationError::InvalidEditSeq,
            ..
        } => {}
        other => panic!("expected InvalidEditSeq, got {other:?}"),
    }
}

#[test]
fn correct_policy_publishes_supplied_value() {
    let session = Session::new("text-correct");
    seed_editor(&session);
    session.on_text_edit(|_, request| TextEditDecision::Correct {
        value: request.value.trim().to_string(),
        validation: StandardValidationState::Warning,
    });
    match session
        .process_event(&text_edit(1, "trim", "  hi  ", 1))
        .expect("correct")
    {
        EventOutcome::Processed { .. } => {}
        other => panic!("expected processed correction, got {other:?}"),
    }
    assert_eq!(editor_value(&session), "hi");
    assert_eq!(
        editor_validation(&session),
        Some(StandardValidationState::Warning)
    );
}
