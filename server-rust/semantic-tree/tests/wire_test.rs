//! Comprehensive tests for Wire Protocol serialization and conversion bridge (§16).
//!
//! # Verification Requirements:
//! 1. Drive the Task 10 Counter example through a sequence of transactions, serialize each committed
//!    transaction to protobuf bytes, deserialize it back, and confirm replaying the deserialized ops
//!    against a fresh store produces the same resulting state as the original.
//! 2. Decode the Task 2 golden fixture bytes (`golden_node_record.bin`, `golden_transaction.bin`,
//!    `golden_framed_message.bin`) through this new path and confirm it produces the expected in-memory
//!    `NodeRecord` and `Transaction` structures matching Task 2 golden fixtures.
//! 3. Test roundtrip serialization for all 17 [`Value`] variants, all standard/custom [`Event`] types,
//!    all 12 [`Operation`] variants, and [`NodeRecord`].

use std::fs;
use std::path::Path;

use srui_protocol::framing::decode_framed;
use srui_protocol::{StandardEnum, StandardNodeType};
use srui_semantic_tree::*;

fn load_fixture_bytes(filename: &str) -> Vec<u8> {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let path = Path::new(manifest_dir)
        .join("../../protocol/conformance-vectors")
        .join(filename);
    fs::read(&path).unwrap_or_else(|e| panic!("Failed to read fixture {:?}: {}", path, e))
}

// =============================================================================
// Verification 1: Task 10 Counter Example Transaction Serialization & Replay
// =============================================================================

#[test]
fn test_counter_example_transaction_serialization_and_fresh_store_replay() {
    // 1. Authoritative original session store driven through multiple transactions (§12.1, §29)
    let mut original_store = SemanticStore::new();
    let mut fresh_store = SemanticStore::new();

    let surface_id = NodeId::new(1);
    let text_id = NodeId::new(2);
    let progress_id = NodeId::new(3);
    let button_id = NodeId::new(4);

    // --- Transaction 1: Initial Counter UI Construction (Revision 0 -> 1) ---
    let txn1 = Transaction::new(
        Revision::INITIAL,
        vec![
            Operation::create_node(
                surface_id,
                TypeRef::SURFACE,
                None,
                None,
                [(PropertyRef::LABEL, Value::from("Counter Application"))],
            ),
            Operation::create_node(
                text_id,
                TypeRef::TEXT,
                Some(surface_id),
                None,
                [
                    (PropertyRef::TEXT, Value::from("Count: 0")),
                    (
                        PropertyRef::ROLE,
                        Value::from(EnumToken::new(
                            StandardEnum::EnumTextRole as u32,
                            StandardTextRole::Heading as u32,
                        )),
                    ),
                ],
            ),
            Operation::create_node(
                progress_id,
                TypeRef::PROGRESS,
                Some(surface_id),
                None,
                [
                    (PropertyRef::VALUE, Value::from(0.0f64)),
                    (PropertyRef::VALUE_DESCRIPTION, Value::from("0 / 100")),
                ],
            ),
            Operation::create_node(
                button_id,
                TypeRef::BUTTON,
                Some(surface_id),
                None,
                [
                    (PropertyRef::LABEL, Value::from("Increment")),
                    (
                        PropertyRef::ROLE,
                        Value::from(EnumToken::new(
                            StandardEnum::EnumActionRole as u32,
                            StandardActionRole::Primary as u32,
                        )),
                    ),
                ],
            ),
        ],
    );

    // Apply to original store
    original_store
        .apply_transaction_record(&txn1)
        .expect("original txn 1 apply");

    // Serialize txn1 to Protobuf wire bytes
    let txn1_wire_bytes = encode_transaction(&txn1);
    assert!(!txn1_wire_bytes.is_empty());

    // Deserialize txn1 from Protobuf wire bytes
    let deserialized_txn1 = decode_transaction(&txn1_wire_bytes).expect("decode txn 1 bytes");
    assert_eq!(deserialized_txn1, txn1);

    // Replay deserialized transaction against fresh store
    fresh_store
        .apply_transaction_record(&deserialized_txn1)
        .expect("fresh store replay txn 1");

    // Verify fresh store state matches original store state at revision 1
    assert_eq!(fresh_store.revision(), original_store.revision());
    assert_eq!(fresh_store.node_count(), original_store.node_count());
    assert_eq!(fresh_store.root_ids(), original_store.root_ids());
    assert_eq!(
        fresh_store
            .get_node(text_id)
            .unwrap()
            .get_property(PropertyRef::TEXT),
        Some(&Value::from("Count: 0"))
    );
    assert_eq!(
        fresh_store
            .get_node(progress_id)
            .unwrap()
            .get_property(PropertyRef::VALUE),
        Some(&Value::from(0.0f64))
    );
    assert_eq!(
        fresh_store
            .get_node(progress_id)
            .unwrap()
            .get_property(PropertyRef::VALUE_DESCRIPTION),
        Some(&Value::from("0 / 100"))
    );

    // --- Transactions 2..=6: Simulate 5 Button Clicks (Revision 1 -> 6) ---
    for count in 1..=5 {
        let base_rev = Revision::new(count);
        let progress_val = (count as f64) / 100.0;
        let count_text = format!("Count: {}", count);
        let progress_desc = format!("{} / 100", count);

        let click_txn = Transaction::new(
            base_rev,
            vec![
                Operation::set_property(
                    text_id,
                    PropertyRef::TEXT,
                    Value::from(count_text.clone()),
                ),
                Operation::set_property(progress_id, PropertyRef::VALUE, Value::from(progress_val)),
                Operation::set_property(
                    progress_id,
                    PropertyRef::VALUE_DESCRIPTION,
                    Value::from(progress_desc.clone()),
                ),
            ],
        );

        // Apply to original store
        original_store
            .apply_transaction_record(&click_txn)
            .expect("original click txn apply");

        // Serialize to wire bytes
        let bytes = encode_transaction(&click_txn);

        // Deserialize back
        let deserialized = decode_transaction(&bytes).expect("deserialize click txn");
        assert_eq!(deserialized, click_txn);

        // Replay against fresh store
        fresh_store
            .apply_transaction_record(&deserialized)
            .expect("fresh store replay click txn");

        // Assert full convergence between original and fresh store
        assert_eq!(fresh_store.revision(), original_store.revision());
        assert_eq!(fresh_store.revision(), Revision::new(count + 1));
        assert_eq!(fresh_store.node_count(), 4);
        assert_eq!(
            fresh_store
                .get_node(text_id)
                .unwrap()
                .get_property(PropertyRef::TEXT),
            Some(&Value::from(count_text))
        );
        assert_eq!(
            fresh_store
                .get_node(progress_id)
                .unwrap()
                .get_property(PropertyRef::VALUE),
            Some(&Value::from(progress_val))
        );
        assert_eq!(
            fresh_store
                .get_node(progress_id)
                .unwrap()
                .get_property(PropertyRef::VALUE_DESCRIPTION),
            Some(&Value::from(progress_desc))
        );
    }

    // Final comprehensive verification: every node in fresh store matches original store exactly
    for &node_id in &[surface_id, text_id, progress_id, button_id] {
        let orig_node = original_store.get_node(node_id).unwrap();
        let fresh_node = fresh_store.get_node(node_id).unwrap();
        assert_eq!(orig_node, fresh_node);
    }
}

// =============================================================================
// Verification 2: Task 2 Golden Fixtures Decoding Through New Path
// =============================================================================

#[test]
fn test_decode_task2_golden_node_record_fixture() {
    let bytes = load_fixture_bytes("golden_node_record.bin");
    assert_eq!(
        bytes.len(),
        48,
        "Task 2 golden NodeRecord byte length is 48"
    );

    // Decode through new path: decode_node_record(&bytes)
    let node_record = decode_node_record(&bytes).expect("decode golden_node_record.bin");

    // Assert in-memory structure matches Task 2 expected specification
    assert_eq!(node_record.node_id, NodeId::new(42));
    assert_eq!(
        node_record.node_type,
        TypeRef::standard(StandardNodeType::NodeTypeButton as u32)
    );
    assert_eq!(node_record.parent_id, Some(NodeId::new(1)));
    assert_eq!(node_record.child_index, Some(0));
    assert_eq!(node_record.properties.len(), 3);

    // Property 0: label = "Delete"
    assert_eq!(
        node_record.get_property(PropertyRef::LABEL),
        Some(&Value::String("Delete".to_string()))
    );

    // Property 1: role = ActionRole::Destructive (enum_id=2, value_id=3)
    assert_eq!(
        node_record.get_property(PropertyRef::ROLE),
        Some(&Value::EnumToken(EnumToken::new(
            StandardEnum::EnumActionRole as u32,
            StandardActionRole::Destructive as u32,
        )))
    );

    // Property 2: enabled = true
    assert_eq!(
        node_record.get_property(PropertyRef::ENABLED),
        Some(&Value::Bool(true))
    );

    // Re-encode back to protobuf bytes and assert bit-for-bit equality with Task 2 golden fixture
    let roundtrip_bytes = encode_node_record(&node_record);
    assert_eq!(
        roundtrip_bytes, bytes,
        "Re-encoded NodeRecord must match golden fixture bytes"
    );
}

#[test]
fn test_decode_task2_golden_transaction_fixture() {
    let bytes = load_fixture_bytes("golden_transaction.bin");
    assert_eq!(
        bytes.len(),
        102,
        "Task 2 golden Transaction byte length is 102"
    );

    // Decode through new path: decode_transaction(&bytes)
    let txn = decode_transaction(&bytes).expect("decode golden_transaction.bin");

    // Assert in-memory structure matches Task 2 expected specification
    assert_eq!(txn.base_revision, Revision::new(104));
    assert_eq!(txn.new_revision, Revision::new(105));
    assert_eq!(txn.priority, 1);
    assert_eq!(txn.operations.len(), 3);

    // Op 0: CreateNode (Text, node_id=19, parent=2, index=3, text="27 tests passed")
    match &txn.operations[0] {
        Operation::CreateNode {
            id,
            node_type,
            parent_id,
            child_index,
            properties,
        } => {
            assert_eq!(*id, NodeId::new(19));
            assert_eq!(
                *node_type,
                TypeRef::standard(StandardNodeType::NodeTypeText as u32)
            );
            assert_eq!(*parent_id, Some(NodeId::new(2)));
            assert_eq!(*child_index, Some(3));
            assert_eq!(properties.len(), 1);
            assert_eq!(properties[0].0, PropertyRef::TEXT);
            assert_eq!(
                properties[0].1,
                Value::String("27 tests passed".to_string())
            );
        }
        other => panic!("Expected CreateNode for op 0, got {:?}", other),
    }

    // Op 1: SetProperty (node_id=4, property=VALUE, value=0.71)
    match &txn.operations[1] {
        Operation::SetProperty {
            id,
            property,
            value,
        } => {
            assert_eq!(*id, NodeId::new(4));
            assert_eq!(*property, PropertyRef::VALUE);
            assert_eq!(*value, Value::Float64(0.71));
        }
        other => panic!("Expected SetProperty for op 1, got {:?}", other),
    }

    // Op 2: BatchPropertySet (node_id=19, properties=[(MINIMUM_SIZE, Size(120, 24))])
    match &txn.operations[2] {
        Operation::BatchPropertySet { id, properties } => {
            assert_eq!(*id, NodeId::new(19));
            assert_eq!(properties.len(), 1);
            assert_eq!(properties[0].0, PropertyRef::MINIMUM_SIZE);
            assert_eq!(properties[0].1, Value::Size(Size::new(120.0, 24.0)));
        }
        other => panic!("Expected BatchPropertySet for op 2, got {:?}", other),
    }

    // Re-encode back to protobuf bytes and assert bit-for-bit equality with Task 2 golden fixture
    let roundtrip_bytes = encode_transaction(&txn);
    assert_eq!(
        roundtrip_bytes, bytes,
        "Re-encoded Transaction must match golden fixture bytes"
    );
}

#[test]
fn test_decode_task2_golden_framed_message_fixture() {
    let bytes = load_fixture_bytes("golden_framed_message.bin");
    assert_eq!(
        bytes.len(),
        105,
        "Task 2 golden Framed SruiMessage byte length is 105"
    );

    // Decode framed top-level SruiMessage
    let msg: srui_protocol::SruiMessage =
        decode_framed(&bytes[..]).expect("decode golden_framed_message.bin");

    // Extract transaction payload
    match msg.msg {
        Some(srui_protocol::srui_message::Msg::Transaction(wire_txn)) => {
            let txn = Transaction::try_from(wire_txn).expect("convert wire transaction");
            assert_eq!(txn.base_revision, Revision::new(104));
            assert_eq!(txn.new_revision, Revision::new(105));
            assert_eq!(txn.priority, 1);
            assert_eq!(txn.operations.len(), 3);
        }
        other => panic!(
            "Expected Transaction payload in framed message, got {:?}",
            other
        ),
    }
}

// =============================================================================
// Value Roundtrip: All 17 Dynamic Variants (§6.5, §16)
// =============================================================================

#[test]
fn test_all_17_value_variants_wire_byte_roundtrip() {
    let variants: Vec<Value> = vec![
        Value::Null,
        Value::Bool(true),
        Value::SignedInt(-9223372036854775807),
        Value::UnsignedInt(18446744073709551615),
        Value::Float64(std::f64::consts::E),
        Value::String("SRUI Semantic UI Protocol v0.4".to_string()),
        Value::NodeId(NodeId::new(42)),
        Value::ItemId(ItemId::new(999)),
        Value::ResourceHash(ResourceHash::new([0xfe; 32])),
        Value::EnumToken(EnumToken::new(2, 3)),
        Value::Size(Size::new(1920.0, 1080.0)),
        Value::Point(Point::new(350.5, 720.25)),
        Value::Range(Range::new(10, 250)),
        Value::Rect(Rect::new(100.0, 200.0, 800.0, 600.0)),
        Value::EdgeInsets(EdgeInsets::new(10.0, 15.0, 20.0, 25.0)),
        Value::List(vec![
            Value::String("item-1".to_string()),
            Value::SignedInt(42),
            Value::Bool(false),
        ]),
        Value::Record(SmallRecord::new(
            TypeRef::standard(1),
            vec![
                Property::new(
                    PropertyRef::LABEL,
                    Value::String("Record Title".to_string()),
                ),
                Property::new(PropertyRef::VALUE, Value::Float64(0.95)),
            ],
        )),
    ];

    assert_eq!(variants.len(), 17, "Must test exactly 17 Value variants");

    for (idx, val) in variants.into_iter().enumerate() {
        // Serialize to bytes
        let bytes = encode_value(&val);
        assert!(
            !bytes.is_empty(),
            "Encoded Value variant {} must not be empty",
            idx + 1
        );

        // Inherent method roundtrip
        let val_bytes_inherent = val.to_wire_bytes();
        assert_eq!(bytes, val_bytes_inherent);

        // Deserialize back
        let back = decode_value(&bytes).unwrap_or_else(|e| {
            panic!(
                "Failed to decode value variant #{}: {:?}: {}",
                idx + 1,
                val,
                e
            )
        });

        assert_eq!(val, back, "Roundtrip mismatch on variant #{}", idx + 1);
    }
}

// =============================================================================
// Event Roundtrip: Standard & Custom Events (§7.6, §7.7, §16)
// =============================================================================

#[test]
fn test_standard_and_custom_events_wire_byte_roundtrip() {
    let client_id = ClientInstanceId::from_string("client-macos-arm64-1");
    let event_id = EventId::from_string("evt-uuid-712-42");

    let events: Vec<Event> = vec![
        // 1. ACTIVATE
        Event::activate(1, "act-1", 100, 10),
        // 2. VALUE_CHANGED (bool)
        Event::value_changed(2, "val-1", 100, 11, true),
        // 3. VALUE_CHANGED (float)
        Event::value_changed(3, "val-2", 100, 12, 0.72f64),
        // 4. SELECTION_CHANGED (ItemId)
        Event::selection_changed(4, "sel-1", 100, 13, ItemId::new(500)),
        // 5. TEXT_EDIT
        Event::text_edit(5, "txt-1", 100, 14, "New input text"),
        // 6. EXPANSION_CHANGED
        Event::expansion_changed(6, "exp-1", 100, 15, true),
        // 7. VIEWPORT_CHANGED
        Event::viewport_changed(7, "vp-1", 100, 16, Size::new(1440.0, 900.0)),
        // 8. Rich Custom Event with ClientInstanceId and multiple arguments
        Event::new(
            Some(client_id.clone()),
            8,
            event_id.clone(),
            Revision::new(104),
            NodeId::new(42),
            TypeRef::new(1, 100), // Extension namespace 1
            [
                (
                    PropertyRef::LABEL,
                    Value::String("Custom Action".to_string()),
                ),
                (PropertyRef::VALUE, Value::Float64(123.456)),
                (PropertyRef::ENABLED, Value::Bool(true)),
                (PropertyRef::new(1, 1), Value::Point(Point::new(10.0, 20.0))),
            ],
        ),
    ];

    for (idx, event) in events.into_iter().enumerate() {
        // Serialize to wire bytes
        let bytes = encode_event(&event);
        assert!(
            !bytes.is_empty(),
            "Encoded Event #{} must not be empty",
            idx + 1
        );

        // Inherent method roundtrip
        let bytes_inherent = event.to_wire_bytes();
        assert_eq!(bytes, bytes_inherent);

        // Deserialize back
        let back = decode_event(&bytes)
            .unwrap_or_else(|e| panic!("Failed to decode event #{}: {:?}: {}", idx + 1, event, e));

        assert_eq!(event, back, "Roundtrip mismatch on event #{}", idx + 1);
    }
}

// =============================================================================
// Operation Roundtrip: All 12 Variants (§13, §16)
// =============================================================================

#[test]
fn test_all_12_operations_wire_byte_roundtrip() {
    let ops: Vec<Operation> = vec![
        // 1. CreateNode
        Operation::create_node(
            NodeId::new(10),
            TypeRef::BUTTON,
            Some(NodeId::new(1)),
            Some(2),
            [
                (PropertyRef::LABEL, Value::from("Click")),
                (PropertyRef::ENABLED, Value::from(true)),
            ],
        ),
        // 2. DeleteNode
        Operation::delete_node(NodeId::new(20)),
        // 3. SetProperty
        Operation::set_property(NodeId::new(30), PropertyRef::TEXT, "Hello"),
        // 4. ClearProperty
        Operation::clear_property(NodeId::new(40), PropertyRef::LABEL),
        // 5. MoveNode
        Operation::move_node(NodeId::new(50), Some(NodeId::new(5)), Some(1)),
        // 6. ReorderChildren
        Operation::reorder_children(
            NodeId::new(60),
            [NodeId::new(3), NodeId::new(1), NodeId::new(2)],
        ),
        // 7. BatchPropertySet
        Operation::batch_property_set(
            NodeId::new(70),
            [
                (PropertyRef::LABEL, Value::from("Batch")),
                (PropertyRef::VALUE, Value::from(100u64)),
            ],
        ),
        // 8. CreateModel
        Operation::create_model(ModelId::new(80), TypeRef::LIST, 500_000),
        // 9. ModelInsert
        Operation::model_insert(
            ModelId::new(80),
            10,
            vec![
                ModelItem::new(
                    ItemId::new(1001),
                    Value::String("Row 1".to_string()),
                    [(PropertyRef::LABEL, Value::from("Item A"))],
                ),
                ModelItem::with_value(ItemId::new(1002), Value::SignedInt(42)),
            ],
        ),
        // 10. ModelDelete
        Operation::model_delete_range(ModelId::new(80), 5, 2),
        // 11. ModelUpdate
        Operation::model_update(
            ModelId::new(80),
            Some(0),
            vec![ModelItem::with_value(ItemId::new(1001), "Updated Row 1")],
        ),
        // 12. ModelResetRange
        Operation::model_reset_range(
            ModelId::new(80),
            0,
            vec![ModelItem::with_value(ItemId::new(2001), "Replaced 0")],
            Some(500_001),
        ),
    ];

    assert_eq!(
        ops.len(),
        12,
        "Must test all 12 standard operation variants"
    );

    for (idx, op) in ops.into_iter().enumerate() {
        // Serialize to wire bytes
        let bytes = encode_operation(&op);
        assert!(
            !bytes.is_empty(),
            "Encoded Operation #{} must not be empty",
            idx + 1
        );

        // Inherent method roundtrip
        let bytes_inherent = op.to_wire_bytes();
        assert_eq!(bytes, bytes_inherent);

        // Deserialize back
        let back = decode_operation(&bytes)
            .unwrap_or_else(|e| panic!("Failed to decode operation #{}: {:?}: {}", idx + 1, op, e));

        assert_eq!(op, back, "Roundtrip mismatch on operation #{}", idx + 1);
    }
}

// =============================================================================
// NodeRecord <-> Operation::CreateNode Conversions
// =============================================================================

#[test]
fn test_node_record_conversions_and_operations() {
    let rec = NodeRecord::new(
        NodeId::new(42),
        TypeRef::BUTTON,
        Some(NodeId::new(1)),
        Some(0),
        vec![
            Property::new(PropertyRef::LABEL, Value::String("Submit".to_string())),
            Property::new(PropertyRef::ENABLED, Value::Bool(true)),
        ],
    );

    assert_eq!(
        rec.get_property(PropertyRef::LABEL),
        Some(&Value::String("Submit".to_string()))
    );
    assert_eq!(
        rec.get_property(PropertyRef::ENABLED),
        Some(&Value::Bool(true))
    );
    assert!(rec.has_property(PropertyRef::LABEL));
    assert!(!rec.has_property(PropertyRef::TEXT));

    // Convert to Operation::CreateNode
    let op: Operation = rec.clone().into();
    match &op {
        Operation::CreateNode {
            id,
            node_type,
            parent_id,
            child_index,
            properties,
        } => {
            assert_eq!(*id, NodeId::new(42));
            assert_eq!(*node_type, TypeRef::BUTTON);
            assert_eq!(*parent_id, Some(NodeId::new(1)));
            assert_eq!(*child_index, Some(0));
            assert_eq!(properties.len(), 2);
        }
        _ => panic!("Expected CreateNode"),
    }

    // Convert Operation back to NodeRecord
    let back_rec = NodeRecord::try_from(op).expect("op to NodeRecord");
    assert_eq!(rec, back_rec);

    // Protobuf wire byte roundtrip
    let wire_bytes = rec.to_wire_bytes();
    let decoded_rec = NodeRecord::from_wire_bytes(&wire_bytes).expect("decode NodeRecord bytes");
    assert_eq!(rec, decoded_rec);
}

// =============================================================================
// Wire Error Handling & Malformed Payload Rejection
// =============================================================================

#[test]
fn test_malformed_protobuf_bytes_rejected_cleanly() {
    // 1. Truncated / invalid varint bytes
    let malformed_bytes = vec![0xff, 0xff, 0xff];
    assert!(decode_transaction(&malformed_bytes).is_err());
    assert!(decode_event(&malformed_bytes).is_err());
    assert!(decode_value(&malformed_bytes).is_err());
    assert!(decode_node_record(&malformed_bytes).is_err());
    assert!(decode_operation(&malformed_bytes).is_err());

    // 2. Missing required field in wire Event
    let wire_event_missing_type = srui_protocol::Event {
        client_instance_id: vec![],
        event_seq: 1,
        event_id: vec![1, 2, 3],
        observed_revision: 0,
        node_id: 10,
        event_type: None, // Missing required type
        arguments: vec![],
    };
    let err = Event::try_from(wire_event_missing_type).expect_err("must fail without event_type");
    assert!(matches!(err, WireError::MissingField("Event.event_type")));

    // 3. Missing required field in wire NodeRecord
    let wire_node_missing_type = srui_protocol::NodeRecord {
        node_id: 10,
        r#type: None, // Missing required type
        parent_id: 0,
        child_index: 0,
        properties: vec![],
    };
    let err_node =
        NodeRecord::try_from(wire_node_missing_type).expect_err("must fail without type");
    assert!(matches!(
        err_node,
        WireError::MissingField("NodeRecord.type")
    ));

    // 4. Invalid resource hash length in Value
    let wire_val_bad_hash = srui_protocol::Value {
        value: Some(srui_protocol::value::Value::ResourceHash(vec![1, 2, 3])), // only 3 bytes, not 32
    };
    let err_val =
        Value::try_from(wire_val_bad_hash).expect_err("must fail with invalid hash length");
    assert!(matches!(
        err_val,
        ValueConversionError::InvalidResourceHashLength(3)
    ));
}
