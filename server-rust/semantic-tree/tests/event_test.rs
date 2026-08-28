use srui_semantic_tree::{
    resolve_standard_event, ClientInstanceId, Event, EventId, EventValidationError, ItemId,
    NodeId, PropertyRef, Revision, SemanticStore, Size, TypeRef, Value,
};

#[test]
fn test_event_construction_and_getters() {
    let client_id = ClientInstanceId::from_string("client-macos-1");
    let event_id = EventId::from_string("evt-12345");
    let event = Event::new(
        Some(client_id.clone()),
        42,
        event_id.clone(),
        Revision::new(10),
        NodeId::new(100),
        TypeRef::EVENT_ACTIVATE,
        [
            (PropertyRef::LABEL, Value::String("Custom".to_string())),
            (PropertyRef::VALUE, Value::SignedInt(99)),
        ],
    );

    assert_eq!(event.client_instance_id, Some(client_id));
    assert_eq!(event.event_seq, 42);
    assert_eq!(event.event_id, event_id);
    assert_eq!(event.observed_revision, Revision::new(10));
    assert_eq!(event.node_id, NodeId::new(100));
    assert_eq!(event.event_type, TypeRef::EVENT_ACTIVATE);
    assert_eq!(event.standard_name(), Some("ACTIVATE"));

    assert!(event.has_argument(PropertyRef::LABEL));
    assert!(event.has_argument(PropertyRef::VALUE));
    assert!(!event.has_argument(PropertyRef::TEXT));

    assert_eq!(event.value_arg(), Some(&Value::SignedInt(99)));
    assert_eq!(
        event.get_argument(PropertyRef::LABEL),
        Some(&Value::String("Custom".to_string()))
    );

    let display_str = format!("{}", event);
    assert!(display_str.contains("seq=42"));
    assert!(display_str.contains("evt-12345"));
    assert!(display_str.contains("ACTIVATE"));
}

#[test]
fn test_standard_event_factories() {
    // 1. ACTIVATE
    let activate = Event::activate(1, "act-1", 10, 101);
    assert_eq!(activate.event_type, TypeRef::EVENT_ACTIVATE);
    assert_eq!(activate.standard_name(), Some("ACTIVATE"));
    assert!(activate.arguments.is_empty());

    // 2. VALUE_CHANGED
    let val_event = Event::value_changed(2, "val-1", 10, 102, true);
    assert_eq!(val_event.event_type, TypeRef::EVENT_VALUE_CHANGED);
    assert_eq!(val_event.bool_arg(), Some(true));

    let float_event = Event::value_changed(3, "val-2", 10, 102, 0.75f64);
    assert_eq!(float_event.value_arg(), Some(&Value::Float64(0.75)));

    // 3. SELECTION_CHANGED
    let sel_event = Event::selection_changed(4, "sel-1", 10, 103, ItemId::new(500));
    assert_eq!(sel_event.event_type, TypeRef::EVENT_SELECTION_CHANGED);
    assert_eq!(sel_event.item_id_arg(), Some(ItemId::new(500)));

    // 4. TEXT_EDIT
    let text_event = Event::text_edit(5, "txt-1", 10, 104, "hello world");
    assert_eq!(text_event.event_type, TypeRef::EVENT_TEXT_EDIT);
    assert_eq!(text_event.text_arg(), Some("hello world"));

    // 5. EXPANSION_CHANGED
    let exp_event = Event::expansion_changed(6, "exp-1", 10, 105, true);
    assert_eq!(exp_event.event_type, TypeRef::EVENT_EXPANSION_CHANGED);
    assert_eq!(exp_event.bool_arg(), Some(true));

    // 6. VIEWPORT_CHANGED
    let vp_event = Event::viewport_changed(7, "vp-1", 10, 106, Size::new(1024.0, 768.0));
    assert_eq!(vp_event.event_type, TypeRef::EVENT_VIEWPORT_CHANGED);
    assert_eq!(
        vp_event.value_arg(),
        Some(&Value::Size(Size::new(1024.0, 768.0)))
    );
}

#[test]
fn test_event_id_and_client_instance_id_conversions() {
    let evt_str = EventId::from_string("event-abc");
    assert_eq!(format!("{}", evt_str), "EventId(event-abc)");
    assert_eq!(evt_str.as_bytes(), b"event-abc");
    assert_eq!(evt_str.len(), 9);
    assert!(!evt_str.is_empty());

    let evt_binary = EventId::from_slice(&[0xde, 0xad, 0xbe, 0xef]);
    assert_eq!(format!("{}", evt_binary), "EventId(0xdeadbeef)");

    let client_str = ClientInstanceId::from_string("client-xyz");
    assert_eq!(format!("{}", client_str), "ClientInstanceId(client-xyz)");
    assert_eq!(client_str.as_bytes(), b"client-xyz");

    let client_binary = ClientInstanceId::from_slice(&[0x01, 0x02]);
    assert_eq!(format!("{}", client_binary), "ClientInstanceId(0x0102)");
}

#[test]
fn test_event_validation_node_exists_in_store() {
    let mut store = SemanticStore::new();
    let surface_id = NodeId::new(1);
    let button_id = NodeId::new(2);

    store
        .create_node(surface_id, TypeRef::SURFACE, None, None, [])
        .unwrap();
    store
        .create_node(
            button_id,
            TypeRef::BUTTON,
            Some(surface_id),
            None,
            [(PropertyRef::LABEL, Value::String("OK".to_string()))],
        )
        .unwrap();

    // 1. Existing node validation succeeds
    let valid_event = Event::activate(1, "evt-1", 0, button_id);
    let node = valid_event.validate_node_exists(&store).unwrap();
    assert_eq!(node.id, button_id);

    // 2. Non-existent node validation fails with NodeNotFound
    let non_existent_id = NodeId::new(999);
    let invalid_event = Event::activate(2, "evt-2", 0, non_existent_id);
    let err = invalid_event.validate_node_exists(&store).unwrap_err();
    assert_eq!(err, EventValidationError::NodeNotFound(non_existent_id));
}

#[test]
fn test_event_validation_node_interactive() {
    let mut store = SemanticStore::new();
    let surface_id = NodeId::new(1);
    let enabled_btn = NodeId::new(2);
    let disabled_btn = NodeId::new(3);

    store
        .create_node(surface_id, TypeRef::SURFACE, None, None, [])
        .unwrap();
    store
        .create_node(
            enabled_btn,
            TypeRef::BUTTON,
            Some(surface_id),
            None,
            [(PropertyRef::ENABLED, Value::Bool(true))],
        )
        .unwrap();
    store
        .create_node(
            disabled_btn,
            TypeRef::BUTTON,
            Some(surface_id),
            None,
            [(PropertyRef::ENABLED, Value::Bool(false))],
        )
        .unwrap();

    let enabled_event = Event::activate(1, "evt-1", 0, enabled_btn);
    assert!(enabled_event.validate_node_interactive(&store).is_ok());

    let disabled_event = Event::activate(2, "evt-2", 0, disabled_btn);
    let err = disabled_event.validate_node_interactive(&store).unwrap_err();
    assert_eq!(err, EventValidationError::NodeDisabled(disabled_btn));
}

#[test]
fn test_event_validation_observed_revision() {
    let store_revision = Revision::new(5);

    // Past revision is valid (client was at rev 3 when clicking)
    let past_event = Event::activate(1, "evt-1", 3, 100);
    assert!(past_event.validate_observed_revision(store_revision).is_ok());

    // Current revision is valid
    let curr_event = Event::activate(2, "evt-2", 5, 100);
    assert!(curr_event.validate_observed_revision(store_revision).is_ok());

    // Future revision is invalid (client claims to have observed revision 6 while store is at 5)
    let future_event = Event::activate(3, "evt-3", 6, 100);
    let err = future_event
        .validate_observed_revision(store_revision)
        .unwrap_err();
    assert_eq!(
        err,
        EventValidationError::FutureRevision {
            observed: Revision::new(6),
            current: Revision::new(5),
        }
    );
}

#[test]
fn test_event_full_validation() {
    let mut store = SemanticStore::with_limits_and_revision(
        Default::default(),
        Revision::new(10),
    );
    let surface_id = NodeId::new(1);
    let button_id = NodeId::new(2);

    store
        .create_node(surface_id, TypeRef::SURFACE, None, None, [])
        .unwrap();
    store
        .create_node(
            button_id,
            TypeRef::BUTTON,
            Some(surface_id),
            None,
            [(PropertyRef::LABEL, Value::String("Submit".to_string()))],
        )
        .unwrap();

    // 1. Valid event
    let event = Event::activate(1, "evt-1", 10, button_id);
    assert!(event.validate(&store).is_ok());

    // 2. Future revision event fails
    let future_event = Event::activate(2, "evt-2", 11, button_id);
    assert!(matches!(
        future_event.validate(&store),
        Err(EventValidationError::FutureRevision { .. })
    ));

    // 3. Nonexistent target fails
    let missing_event = Event::activate(3, "evt-3", 10, NodeId::new(99));
    assert_eq!(
        missing_event.validate(&store),
        Err(EventValidationError::NodeNotFound(NodeId::new(99)))
    );
}

#[test]
fn test_event_type_ref_resolution_and_constants() {
    assert_eq!(TypeRef::EVENT_ACTIVATE.local_id, 1);
    assert_eq!(TypeRef::EVENT_VALUE_CHANGED.local_id, 2);
    assert_eq!(TypeRef::EVENT_SELECTION_CHANGED.local_id, 3);
    assert_eq!(TypeRef::EVENT_EXPANSION_CHANGED.local_id, 4);
    assert_eq!(TypeRef::EVENT_TEXT_EDIT.local_id, 5);
    assert_eq!(TypeRef::EVENT_VIEWPORT_CHANGED.local_id, 6);

    let resolved = resolve_standard_event("ACTIVATE").unwrap();
    assert_eq!(resolved, TypeRef::EVENT_ACTIVATE);

    let resolved_text = resolve_standard_event("TEXT_EDIT").unwrap();
    assert_eq!(resolved_text, TypeRef::EVENT_TEXT_EDIT);

    let unknown = resolve_standard_event("NON_EXISTENT");
    assert!(unknown.is_err());
}
