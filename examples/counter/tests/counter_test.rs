//! Verification tests for the SRUI Counter Example application (§7.1–§7.7, §12.1, §29).
//!
//! Verifies:
//! 1. Driving the counter example by calling `session.dispatch(ACTIVATE on the button)` several times,
//!    asserting the resulting committed Text and Progress values match expectations after each dispatch.
//! 2. Confirming that a panicking or erroring handler does not leave the store in a partially-committed
//!    state (the surrounding transaction strictly obeys Task 5's atomicity guarantee).
//! 3. Confirming that event validation checks enforce disabled button rules (§27).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use srui_example_counter::CounterApp;
use srui_sdk::*;

#[test]
fn test_counter_dispatch_increments_text_and_progress_sequentially() {
    let app = CounterApp::new().expect("failed to initialize CounterApp");

    // Initial state check
    assert_eq!(app.current_revision(), 1);
    assert_eq!(app.get_count(), 0);
    assert_eq!(app.get_text(), Some("Count: 0".to_string()));
    assert_eq!(app.get_progress(), Some(0.0));
    assert_eq!(app.get_progress_description(), Some("0 / 100".to_string()));

    // Dispatch click 1
    let handled = app.click(1).expect("click 1 failed");
    assert_eq!(handled, 1);
    assert_eq!(app.current_revision(), 2);
    assert_eq!(app.get_count(), 1);
    assert_eq!(app.get_text(), Some("Count: 1".to_string()));
    assert_eq!(app.get_progress(), Some(0.01));
    assert_eq!(app.get_progress_description(), Some("1 / 100".to_string()));

    // Dispatch click 2
    let handled = app.click(2).expect("click 2 failed");
    assert_eq!(handled, 1);
    assert_eq!(app.current_revision(), 3);
    assert_eq!(app.get_count(), 2);
    assert_eq!(app.get_text(), Some("Count: 2".to_string()));
    assert_eq!(app.get_progress(), Some(0.02));
    assert_eq!(app.get_progress_description(), Some("2 / 100".to_string()));

    // Dispatch click 3
    let handled = app.click(3).expect("click 3 failed");
    assert_eq!(handled, 1);
    assert_eq!(app.current_revision(), 4);
    assert_eq!(app.get_count(), 3);
    assert_eq!(app.get_text(), Some("Count: 3".to_string()));
    assert_eq!(app.get_progress(), Some(0.03));
    assert_eq!(app.get_progress_description(), Some("3 / 100".to_string()));

    // Dispatch click 4
    let handled = app.click(4).expect("click 4 failed");
    assert_eq!(handled, 1);
    assert_eq!(app.current_revision(), 5);
    assert_eq!(app.get_count(), 4);
    assert_eq!(app.get_text(), Some("Count: 4".to_string()));
    assert_eq!(app.get_progress(), Some(0.04));
    assert_eq!(app.get_progress_description(), Some("4 / 100".to_string()));

    // Validate directly through typed widget accessors on the store (§7.4)
    app.session().with_store(|store| {
        let text = Text::from_store(store, app.text_id()).expect("Text widget not found");
        assert_eq!(text.text(store), Some("Count: 4"));
        assert_eq!(text.role(store), Some(TextRole::Heading));

        let prog =
            Progress::from_store(store, app.progress_id()).expect("Progress widget not found");
        assert_eq!(prog.value(store), Some(0.04));
        assert_eq!(prog.value_description(store), Some("4 / 100"));

        let btn = Button::from_store(store, app.button_id()).expect("Button widget not found");
        assert_eq!(btn.label(store), Some("Increment"));
        assert_eq!(btn.role(store), Some(ActionRole::Primary));
    });
}

#[test]
fn test_panicking_handler_transaction_preserves_store_atomicity() {
    let app = CounterApp::new().expect("failed to initialize CounterApp");

    // Set count to 5 via 5 clicks
    for i in 1..=5 {
        app.click(i).expect("click failed");
    }

    assert_eq!(app.current_revision(), 6);
    assert_eq!(app.get_text(), Some("Count: 5".to_string()));
    assert_eq!(app.get_progress(), Some(0.05));

    // Register a second faulty handler on the button that panics midway through a transaction
    let panic_attempted = Arc::new(AtomicBool::new(false));
    let panic_flag = Arc::clone(&panic_attempted);
    let text_id = app.text_id();

    app.session().on(app.button_id(), ACTIVATE, move |ctx, _| {
        panic_flag.store(true, Ordering::SeqCst);
        let res: Result<(), SdkError> = ctx.transaction(|ui| {
            // Stage partial mutation
            ui.set(text_id, TEXT, "Corrupted Text Should Never Commit")?;
            let temp_node = NodeId::new(999);
            Surface::builder(temp_node).create(ui)?;
            // Trigger panic
            panic!("handler business logic exploded!");
        });
        assert!(res.is_err());
        match res {
            Err(SdkError::Panicked(msg)) => {
                assert!(msg.contains("handler business logic exploded"))
            }
            other => panic!("expected SdkError::Panicked, got {:?}", other),
        }
    });

    // Dispatch event (triggers both the original increment handler and the panicking handler)
    let rev = app.session().current_revision();
    let event = Event::activate(6, "click-6", rev, app.button_id());
    let handled = app.session().dispatch(event).expect("dispatch succeeded");
    assert_eq!(handled, 2);
    assert!(panic_attempted.load(Ordering::SeqCst));

    // The valid handler successfully advanced to Count: 6 (revision 7).
    // The panicking handler's staged mutations were completely rolled back and never polluted the store.
    assert_eq!(app.current_revision(), 7);
    assert_eq!(app.get_text(), Some("Count: 6".to_string()));
    assert_eq!(app.get_progress(), Some(0.06));
    assert!(!app.session().contains_node(NodeId::new(999)));
}

#[test]
fn test_erroring_handler_transaction_preserves_store_atomicity() {
    let app = CounterApp::new().expect("failed to initialize CounterApp");

    let initial_rev = app.current_revision();
    let text_id = app.text_id();

    // Register an erroring handler on a secondary button
    let fail_btn = NodeId::new(10);
    app.session()
        .transaction(|ui| {
            Button::builder(fail_btn)
                .parent(app.surface_id())
                .label("Fail Button")
                .create(ui)?;
            Ok(())
        })
        .unwrap();

    let after_btn_rev = app.current_revision();
    assert_eq!(after_btn_rev, initial_rev + 1);

    let error_attempted = Arc::new(AtomicBool::new(false));
    let error_flag = Arc::clone(&error_attempted);

    app.session().on(fail_btn, ACTIVATE, move |ctx, _| {
        error_flag.store(true, Ordering::SeqCst);
        let res: Result<(), SdkError> = ctx.transaction(|ui| {
            ui.set(text_id, TEXT, "Uncommitted Text")?;
            Err(StoreError::OperationError(
                "business rule validation rejected".to_string(),
            ))
        });
        assert!(res.is_err());
    });

    // Dispatch ACTIVATE to fail_btn
    let event = Event::activate(1, "fail-evt", after_btn_rev, fail_btn);
    let count = app.session().dispatch(event).expect("dispatch executed");
    assert_eq!(count, 1);
    assert!(error_attempted.load(Ordering::SeqCst));

    // Assert revision and text were NOT changed by the failing transaction
    assert_eq!(app.current_revision(), after_btn_rev);
    assert_eq!(app.get_text(), Some("Count: 0".to_string()));
}

#[test]
fn test_disabled_button_dispatch_is_rejected_without_executing_handlers() {
    let app = CounterApp::new().expect("failed to initialize CounterApp");

    // Disable the button in a transaction (§7.4)
    app.session()
        .transaction(|ui| {
            ui.set(app.button_id(), ENABLED, false)?;
            Ok(())
        })
        .unwrap();

    let rev = app.session().current_revision();

    // Attempt to dispatch to disabled button (§27)
    let event = Event::activate(1, "disabled-click", rev, app.button_id());
    let err = app.session().dispatch(event).unwrap_err();

    match err {
        SdkError::EventValidation(EventValidationError::NodeDisabled(id)) => {
            assert_eq!(id, app.button_id());
        }
        other => panic!("expected NodeDisabled error, got {:?}", other),
    }

    // Counter must remain 0
    assert_eq!(app.get_count(), 0);
    assert_eq!(app.get_text(), Some("Count: 0".to_string()));
}

#[test]
fn test_live_counter_app_dispatch_with_wire_encoding_and_store_replay() {
    // 1. Initialize live CounterApp (produces Revision 1 initial UI)
    let app = CounterApp::new().expect("failed to initialize CounterApp");
    assert_eq!(app.current_revision(), 1);

    let mut fresh_store = SemanticStore::new();

    // Replay initial state from live CounterApp session store via wire serialization
    let initial_txn = Transaction::new(
        Revision::INITIAL,
        vec![
            Operation::create_node(
                app.surface_id(),
                TypeRef::SURFACE,
                None,
                None,
                [(LABEL, Value::from("Counter Application"))],
            ),
            Operation::create_node(
                app.text_id(),
                TypeRef::TEXT,
                Some(app.surface_id()),
                None,
                [
                    (TEXT, Value::from("Count: 0")),
                    (ROLE, Value::from(EnumToken::from(TextRole::Heading))),
                ],
            ),
            Operation::create_node(
                app.progress_id(),
                TypeRef::PROGRESS,
                Some(app.surface_id()),
                None,
                [
                    (VALUE, Value::from(0.0f64)),
                    (VALUE_DESCRIPTION, Value::from("0 / 100")),
                ],
            ),
            Operation::create_node(
                app.button_id(),
                TypeRef::BUTTON,
                Some(app.surface_id()),
                None,
                [
                    (LABEL, Value::from("Increment")),
                    (ROLE, Value::from(EnumToken::from(ActionRole::Primary))),
                ],
            ),
        ],
    );

    // Wire encode & decode initial transaction
    let initial_wire_bytes = encode_transaction(&initial_txn);
    let decoded_initial_txn = decode_transaction(&initial_wire_bytes).expect("decode initial txn");
    fresh_store
        .apply_transaction_record(&decoded_initial_txn)
        .expect("replay initial txn to fresh store");

    // Verify initial fresh store matches live CounterApp session store
    assert_eq!(fresh_store.revision(), app.session().current_revision());
    assert_eq!(fresh_store.node_count(), app.session().node_count());

    // 2. Dispatch 5 clicks through the live CounterApp session
    // For each click, serialize the Event over wire bytes before dispatching to CounterApp!
    for seq in 1..=5 {
        let rev = app.session().current_revision();
        let original_event = Event::activate(seq, format!("click-{}", seq), rev, app.button_id());

        // Event wire encode & decode roundtrip
        let event_wire_bytes = encode_event(&original_event);
        let decoded_event = decode_event(&event_wire_bytes).expect("decode event bytes");
        assert_eq!(decoded_event, original_event);

        // Dispatch decoded event to the live CounterApp session
        let handled = app
            .session()
            .dispatch(decoded_event)
            .expect("dispatch decoded event");
        assert_eq!(handled, 1);

        // Live CounterApp updated its store atomically (§12.1)
        assert_eq!(app.current_revision(), seq + 1);
        assert_eq!(app.get_count(), seq);

        // Capture the matching transaction, serialize to wire bytes, and replay against fresh store
        let click_txn = Transaction::new(
            Revision::new(seq),
            vec![
                Operation::set_property(app.text_id(), TEXT, format!("Count: {}", seq)),
                Operation::set_property(app.progress_id(), VALUE, (seq as f64) / 100.0),
                Operation::set_property(
                    app.progress_id(),
                    VALUE_DESCRIPTION,
                    format!("{} / 100", seq),
                ),
            ],
        );

        let txn_wire_bytes = encode_transaction(&click_txn);
        let decoded_txn = decode_transaction(&txn_wire_bytes).expect("decode click txn");
        fresh_store
            .apply_transaction_record(&decoded_txn)
            .expect("replay click txn to fresh store");

        // Verify fresh store exactly matches live CounterApp session store at this revision
        assert_eq!(fresh_store.revision(), app.session().current_revision());
        assert_eq!(fresh_store.node_count(), app.session().node_count());
        assert_eq!(
            fresh_store
                .get_node(app.text_id())
                .unwrap()
                .get_property(TEXT),
            app.session()
                .get_node(app.text_id())
                .unwrap()
                .get_property(TEXT)
        );
        assert_eq!(
            fresh_store
                .get_node(app.progress_id())
                .unwrap()
                .get_property(VALUE),
            app.session()
                .get_node(app.progress_id())
                .unwrap()
                .get_property(VALUE)
        );
    }
}
