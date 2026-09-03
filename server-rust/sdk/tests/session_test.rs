//! Comprehensive conformance tests for the SRUI SDK Session API (§6.1, §7.6, §7.7, §12.1, §12.2, §27, §29).
//!
//! Verifies:
//! 1. `session.transaction(|ui| { ... })` ergonomic workflow matching §29.
//! 2. Typed widget mutators operate seamlessly on `ui` within transactions.
//! 3. Revision advances strictly monotonically by 1 on successful transaction commit.
//! 4. Atomicity guarantee: returning `Err` rolls back all staged mutations without side effects.
//! 5. Atomicity guarantee: panicking inside transaction closure rolls back all staged mutations cleanly.
//! 6. `session.on(node, EVENT, handler)` and `session.dispatch(event)` event routing.
//! 7. Server-side event validation (§27): missing nodes, disabled nodes, and future revisions are rejected.
//! 8. Nested transaction execution inside event handlers operates without deadlocks.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use srui_sdk::*;
use srui_semantic_tree::{EventValidationError, Size};

#[test]
fn test_session_initialization_and_metadata() {
    let session = Session::new("test-session-001");
    assert_eq!(session.session_id(), "test-session-001");
    assert_eq!(session.current_revision().get(), 0);
    assert_eq!(session.node_count(), 0);
    assert!(session.root_ids().is_empty());
}

#[test]
fn test_section29_transaction_and_on_syntax() {
    let session = Session::new("s29-example");
    let approve_button = NodeId::new(10);
    let progress = NodeId::new(20);
    let status = NodeId::new(30);

    // 1. §29 Session Transaction Example
    session
        .transaction(|ui| {
            Surface::builder(1).create(ui)?;
            Button::builder(approve_button)
                .parent(1)
                .label("Approve")
                .role(ActionRole::Primary)
                .create(ui)?;
            Progress::builder(progress).parent(1).create(ui)?;
            Text::builder(status).parent(1).create(ui)?;

            ui.set(progress, VALUE, 0.72)?;
            ui.set(status, TEXT, "Running tests")?;
            Ok(())
        })
        .expect("initial transaction failed");

    assert_eq!(session.current_revision().get(), 1);
    assert_eq!(session.node_count(), 4);

    // Verify properties in store
    session.with_store(|store| {
        let p = Progress::from_store(store, progress).unwrap();
        assert_eq!(p.value(store), Some(0.72));

        let t = Text::from_store(store, status).unwrap();
        assert_eq!(t.text(store), Some("Running tests"));

        let b = Button::from_store(store, approve_button).unwrap();
        assert_eq!(b.label(store), Some("Approve"));
    });

    // 2. §29 Event Handler Registration
    let action_executed = Arc::new(AtomicBool::new(false));
    let action_flag = Arc::clone(&action_executed);

    session.on(approve_button, ACTIVATE, move |ctx, event| {
        action_flag.store(true, Ordering::SeqCst);
        ctx.transaction(|ui| {
            ui.set(event.node_id, LABEL, "Approved!")?;
            ui.set(status, TEXT, "Deployment Approved")?;
            Ok(())
        })
        .expect("handler transaction failed");
    });

    assert_eq!(session.handler_count(approve_button, ACTIVATE), 1);

    // 3. Deliver ACTIVATE event via dispatch
    let event = Event::activate(1, "evt-001", 1, approve_button);
    let dispatched_count = session.dispatch(event).expect("dispatch failed");
    assert_eq!(dispatched_count, 1);
    assert!(action_executed.load(Ordering::SeqCst));

    // Revision should advance to 2
    assert_eq!(session.current_revision().get(), 2);

    // Verify updated state
    session.with_store(|store| {
        let b = Button::from_store(store, approve_button).unwrap();
        assert_eq!(b.label(store), Some("Approved!"));

        let t = Text::from_store(store, status).unwrap();
        assert_eq!(t.text(store), Some("Deployment Approved"));
    });
}

#[test]
fn test_transaction_atomicity_and_rollback_on_err() {
    let session = Session::new("rollback-test");
    let root = NodeId::new(1);
    let text = NodeId::new(2);

    session
        .transaction(|ui| {
            Surface::builder(root).label("Initial").create(ui)?;
            Text::builder(text)
                .parent(root)
                .text("Initial Text")
                .create(ui)?;
            Ok(())
        })
        .unwrap();

    assert_eq!(session.current_revision().get(), 1);
    assert_eq!(session.node_count(), 2);

    // Attempt fallible transaction that modifies existing property, creates node, then returns Err
    let new_node = NodeId::new(3);
    let result: Result<(), SdkError> = session.transaction(|ui| {
        ui.set(text, TEXT, "Mutated Text")?;
        Button::builder(new_node)
            .parent(root)
            .label("Temp")
            .create(ui)?;
        assert_eq!(ui.node_count(), 3); // visible in staging

        // Deliberate StoreError return
        Err(StoreError::OperationError(
            "simulated store rejection".into(),
        ))
    });

    assert!(result.is_err());
    match result {
        Err(SdkError::Store(StoreError::OperationError(msg))) => {
            assert!(msg.contains("simulated store rejection"))
        }
        other => panic!("expected SdkError::Store, got {:?}", other),
    }

    // Also test transaction_custom with custom error type
    let custom_res: Result<(), SdkError> = session.transaction_custom(|ui| {
        ui.set(text, TEXT, "Custom Fail").unwrap();
        Err("custom user error string")
    });
    assert!(custom_res.is_err());
    match custom_res {
        Err(SdkError::User(msg)) => assert_eq!(msg, "custom user error string"),
        other => panic!("expected SdkError::User, got {:?}", other),
    }

    // Revision must remain 1 and all mutations must be rolled back
    assert_eq!(session.current_revision().get(), 1);
    assert_eq!(session.node_count(), 2);
    assert!(!session.contains_node(new_node));

    session.with_store(|store| {
        let t = Text::from_store(store, text).unwrap();
        assert_eq!(t.text(store), Some("Initial Text"));
    });
}

#[test]
fn test_transaction_atomicity_and_rollback_on_panic() {
    let session = Session::new("panic-test");
    let root = NodeId::new(1);
    let btn = NodeId::new(2);

    session
        .transaction(|ui| {
            Surface::builder(root).create(ui)?;
            Button::builder(btn)
                .parent(root)
                .label("Original Label")
                .create(ui)?;
            Ok(())
        })
        .unwrap();

    assert_eq!(session.current_revision().get(), 1);
    assert_eq!(session.node_count(), 2);

    // Attempt transaction that mutates state and then panics
    let temp_node = NodeId::new(99);
    let result: Result<(), SdkError> = session.transaction(|ui| {
        ui.set(btn, LABEL, "Uncommitted Label").unwrap();
        Text::builder(temp_node).parent(root).create(ui).unwrap();
        panic!("fatal application crash simulation");
    });

    assert!(result.is_err());
    match result {
        Err(SdkError::Panicked(msg)) => assert!(msg.contains("fatal application crash simulation")),
        other => panic!("expected SdkError::Panicked, got {:?}", other),
    }

    // Assert store is intact: revision unchanged, partial mutations discarded
    assert_eq!(session.current_revision().get(), 1);
    assert_eq!(session.node_count(), 2);
    assert!(!session.contains_node(temp_node));

    session.with_store(|store| {
        let b = Button::from_store(store, btn).unwrap();
        assert_eq!(b.label(store), Some("Original Label"));
    });
}

#[test]
fn test_event_validation_rejections() {
    let session = Session::new("validation-test");
    let root = NodeId::new(1);
    let enabled_btn = NodeId::new(2);
    let disabled_btn = NodeId::new(3);

    session
        .transaction(|ui| {
            Surface::builder(root).create(ui)?;
            Button::builder(enabled_btn)
                .parent(root)
                .enabled(true)
                .create(ui)?;
            Button::builder(disabled_btn)
                .parent(root)
                .enabled(false)
                .create(ui)?;
            Ok(())
        })
        .unwrap();

    assert_eq!(session.current_revision().get(), 1);

    // 1. Dispatch to non-existent node
    let missing_node = NodeId::new(999);
    let event_missing = Event::activate(1, "e-missing", 1, missing_node);
    let err_missing = session.dispatch(event_missing).unwrap_err();
    match err_missing {
        SdkError::EventValidation(EventValidationError::NodeNotFound(id)) => {
            assert_eq!(id, missing_node)
        }
        other => panic!("expected NodeNotFound, got {:?}", other),
    }

    // 2. Dispatch to disabled node (§27)
    let event_disabled = Event::activate(2, "e-disabled", 1, disabled_btn);
    let err_disabled = session.dispatch(event_disabled).unwrap_err();
    match err_disabled {
        SdkError::EventValidation(EventValidationError::NodeDisabled(id)) => {
            assert_eq!(id, disabled_btn)
        }
        other => panic!("expected NodeDisabled, got {:?}", other),
    }

    // 3. Dispatch with future observed_revision (§7.7, §27)
    let event_future = Event::activate(3, "e-future", 99, enabled_btn);
    let err_future = session.dispatch(event_future).unwrap_err();
    match err_future {
        SdkError::EventValidation(EventValidationError::FutureRevision { observed, current }) => {
            assert_eq!(observed.get(), 99);
            assert_eq!(current.get(), 1);
        }
        other => panic!("expected FutureRevision, got {:?}", other),
    }
}

#[test]
fn test_multiple_handlers_and_clear_handlers() {
    let session = Session::new("multi-handler-test");
    let btn = NodeId::new(1);

    session
        .transaction(|ui| {
            Button::builder(btn).label("Click").create(ui)?;
            Ok(())
        })
        .unwrap();

    let counter1 = Arc::new(AtomicU64::new(0));
    let counter2 = Arc::new(AtomicU64::new(0));

    let c1 = Arc::clone(&counter1);
    session.on(btn, ACTIVATE, move |_, _| {
        c1.fetch_add(1, Ordering::SeqCst);
    });

    let c2 = Arc::clone(&counter2);
    session.on(btn, ACTIVATE, move |_, _| {
        c2.fetch_add(10, Ordering::SeqCst);
    });

    assert_eq!(session.handler_count(btn, ACTIVATE), 2);

    let event = Event::activate(1, "e1", 1, btn);
    let count = session.dispatch(event).unwrap();
    assert_eq!(count, 2);
    assert_eq!(counter1.load(Ordering::SeqCst), 1);
    assert_eq!(counter2.load(Ordering::SeqCst), 10);

    // Clear handlers
    session.clear_handlers();
    assert_eq!(session.handler_count(btn, ACTIVATE), 0);

    let event2 = Event::activate(2, "e2", 1, btn);
    let count2 = session.dispatch(event2).unwrap();
    assert_eq!(count2, 0);
    assert_eq!(counter1.load(Ordering::SeqCst), 1); // unchanged
}

#[test]
fn test_transaction_sync_and_complex_mutations() {
    let session = Session::new("sync-test");
    let surface = NodeId::new(1);
    let row = NodeId::new(2);
    let tgl = NodeId::new(3);

    session
        .transaction_sync(|ui| {
            Surface::builder(surface)
                .preferred_size(Size::new(800.0, 600.0))
                .create(ui)
                .unwrap();
            Row::builder(row).parent(surface).create(ui).unwrap();
            Toggle::switch(tgl)
                .parent(row)
                .value(true)
                .create(ui)
                .unwrap();
        })
        .unwrap();

    assert_eq!(session.current_revision().get(), 1);
    assert_eq!(session.node_count(), 3);

    session.with_store(|store| {
        let t = Toggle::from_store(store, tgl).unwrap();
        assert_eq!(t.value(store), Some(true));
        assert_eq!(
            t.presentation_hint(store),
            Some(TogglePresentationHint::Switch)
        );
    });

    // Test delete and move within transaction
    session
        .transaction(|ui| {
            ui.set(tgl, VALUE, false)?;
            let col = NodeId::new(4);
            Column::builder(col).parent(surface).create(ui)?;
            ui.move_node(tgl, Some(col), None)?;
            Ok(())
        })
        .unwrap();

    assert_eq!(session.current_revision().get(), 2);
    assert_eq!(session.node_count(), 4);

    session.with_store(|store| {
        let t = Toggle::from_store(store, tgl).unwrap();
        assert_eq!(t.value(store), Some(false));
        assert_eq!(store.parent_of(tgl), Some(Some(NodeId::new(4))));
    });
}

#[test]
fn test_publish_resource_returns_canonical_hash_and_dedupes() {
    let session = Session::new("resource-api");
    let bytes = b"sdk-resource-payload";
    let first = session.publish_resource(bytes).expect("publish").hash;
    let second = session.publish_resource(bytes).expect("republish");
    assert!(!second.inserted);
    assert_eq!(second.hash, first);
    let entry = session.lookup_resource(&first).expect("lookup");
    assert_eq!(entry.bytes.as_ref(), bytes);
}

#[test]
fn test_publish_resource_rejects_oversize_before_retention() {
    let session = Session::new("resource-limit");
    let over = vec![0u8; DEFAULT_MAX_RESOURCE_BYTES + 1];
    let err = session.publish_resource(&over).expect_err("oversize");
    assert!(matches!(
        err,
        SdkError::Resource(ResourceError::ResourceTooLarge { .. })
    ));
    assert!(session
        .lookup_resource(&ResourceHash::new([0u8; 32]))
        .is_none());
}

#[test]
fn test_image_widget_accepts_published_resource_hash() {
    let session = Session::new("image-resource");
    let hash = session.publish_resource(b"tiny-image-bytes").unwrap().hash;
    session
        .transaction(|ui| {
            Surface::builder(1).create(ui)?;
            Image::builder(2).parent(1).resource(hash).create(ui)?;
            Ok(())
        })
        .unwrap();
    session.with_store(|store| {
        let image = Image::from_store(store, NodeId::new(2)).unwrap();
        assert_eq!(image.resource(store), Some(hash));
    });
}
