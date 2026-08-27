use prost::Message;
use srui_protocol::*;
use std::fs;
use std::path::Path;

#[test]
fn test_decode_golden_node_record() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let fixture_path = Path::new(manifest_dir)
        .join("../../protocol/conformance-vectors/golden_node_record.bin");
    let bytes = fs::read(&fixture_path)
        .unwrap_or_else(|e| panic!("Failed to read golden_node_record.bin from {:?}: {}", fixture_path, e));

    let node = NodeRecord::decode(&bytes[..]).expect("Failed to decode NodeRecord from golden fixture");

    assert_eq!(node.node_id, 42);
    let type_ref = node.r#type.expect("type_ref must be present");
    assert_eq!(type_ref.namespace_id, STANDARD_NAMESPACE_ID);
    assert_eq!(type_ref.local_id, StandardNodeType::NodeTypeButton as u32);
    assert_eq!(node.parent_id, 1);
    assert_eq!(node.child_index, 0);
    assert_eq!(node.properties.len(), 3);

    // Property 0: Label = "Delete"
    let p0 = &node.properties[0];
    let p0_ref = p0.property.as_ref().expect("p0 property ref");
    assert_eq!(p0_ref.namespace_id, STANDARD_NAMESPACE_ID);
    assert_eq!(p0_ref.local_id, StandardProperty::PropertyLabel as u32);
    match &p0.value.as_ref().expect("p0 value").value {
        Some(value::Value::StringValue(s)) => assert_eq!(s, "Delete"),
        other => panic!("Expected StringValue, got {:?}", other),
    }

    // Property 1: Role = ActionRole::Destructive (enum_id=2, value_id=3)
    let p1 = &node.properties[1];
    let p1_ref = p1.property.as_ref().expect("p1 property ref");
    assert_eq!(p1_ref.namespace_id, STANDARD_NAMESPACE_ID);
    assert_eq!(p1_ref.local_id, StandardProperty::PropertyRole as u32);
    match &p1.value.as_ref().expect("p1 value").value {
        Some(value::Value::EnumValue(ev)) => {
            assert_eq!(ev.enum_id, 2);
            assert_eq!(ev.value_id, ActionRole::Destructive as u32);
        }
        other => panic!("Expected EnumValue, got {:?}", other),
    }

    // Property 2: Enabled = true
    let p2 = &node.properties[2];
    let p2_ref = p2.property.as_ref().expect("p2 property ref");
    assert_eq!(p2_ref.namespace_id, STANDARD_NAMESPACE_ID);
    assert_eq!(p2_ref.local_id, StandardProperty::PropertyEnabled as u32);
    match &p2.value.as_ref().expect("p2 value").value {
        Some(value::Value::BoolValue(b)) => assert_eq!(*b, true),
        other => panic!("Expected BoolValue, got {:?}", other),
    }
}

#[test]
fn test_decode_golden_transaction() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let fixture_path = Path::new(manifest_dir)
        .join("../../protocol/conformance-vectors/golden_transaction.bin");
    let bytes = fs::read(&fixture_path)
        .unwrap_or_else(|e| panic!("Failed to read golden_transaction.bin from {:?}: {}", fixture_path, e));

    let tx = Transaction::decode(&bytes[..]).expect("Failed to decode Transaction from golden fixture");

    assert_eq!(tx.base_revision, 104);
    assert_eq!(tx.new_revision, 105);
    assert_eq!(tx.priority, 1);
    assert_eq!(tx.operations.len(), 3);

    // Op 0: CREATE_NODE (Text node with text "27 tests passed")
    match &tx.operations[0].op {
        Some(operation::Op::CreateNode(op)) => {
            let node = op.node.as_ref().expect("create_node node");
            assert_eq!(node.node_id, 19);
            let t = node.r#type.as_ref().expect("node type");
            assert_eq!(t.namespace_id, STANDARD_NAMESPACE_ID);
            assert_eq!(t.local_id, StandardNodeType::NodeTypeText as u32);
            assert_eq!(node.parent_id, 2);
            assert_eq!(node.child_index, 3);
            assert_eq!(node.properties.len(), 1);
            let p = &node.properties[0];
            assert_eq!(p.property.as_ref().unwrap().local_id, StandardProperty::PropertyText as u32);
            match &p.value.as_ref().unwrap().value {
                Some(value::Value::StringValue(s)) => assert_eq!(s, "27 tests passed"),
                other => panic!("Expected StringValue, got {:?}", other),
            }
        }
        other => panic!("Expected CreateNode op, got {:?}", other),
    }

    // Op 1: SET_PROPERTY (node 4, Value property = 0.71)
    match &tx.operations[1].op {
        Some(operation::Op::SetProperty(op)) => {
            assert_eq!(op.node_id, 4);
            let p_ref = op.property.as_ref().expect("property ref");
            assert_eq!(p_ref.namespace_id, STANDARD_NAMESPACE_ID);
            assert_eq!(p_ref.local_id, StandardProperty::PropertyValue as u32);
            match &op.value.as_ref().expect("value").value {
                Some(value::Value::FloatValue(f)) => {
                    assert!((f - 0.71).abs() < 1e-6);
                }
                other => panic!("Expected FloatValue, got {:?}", other),
            }
        }
        other => panic!("Expected SetProperty op, got {:?}", other),
    }

    // Op 2: BATCH_PROPERTY_SET (node 19, MinimumSize = 120.0 x 24.0)
    match &tx.operations[2].op {
        Some(operation::Op::BatchPropertySet(op)) => {
            assert_eq!(op.node_id, 19);
            assert_eq!(op.properties.len(), 1);
            let p = &op.properties[0];
            let p_ref = p.property.as_ref().expect("property ref");
            assert_eq!(p_ref.namespace_id, STANDARD_NAMESPACE_ID);
            assert_eq!(p_ref.local_id, StandardProperty::PropertyMinimumSize as u32);
            match &p.value.as_ref().expect("value").value {
                Some(value::Value::SizeValue(size)) => {
                    assert_eq!(size.width, 120.0);
                    assert_eq!(size.height, 24.0);
                }
                other => panic!("Expected SizeValue, got {:?}", other),
            }
        }
        other => panic!("Expected BatchPropertySet op, got {:?}", other),
    }
}
