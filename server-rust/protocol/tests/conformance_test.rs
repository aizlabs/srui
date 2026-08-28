use prost::Message;
use serde_json::Value as JsonValue;
use srui_protocol::*;
use std::fs;
use std::path::Path;

fn load_expected_spec() -> (std::path::PathBuf, JsonValue) {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let spec_path = Path::new(manifest_dir).join("../../protocol/conformance-vectors/expected.json");
    let text = fs::read_to_string(&spec_path)
        .unwrap_or_else(|e| panic!("Failed to read expected.json from {:?}: {}", spec_path, e));
    let json: JsonValue = serde_json::from_str(&text).expect("valid JSON in expected.json");
    (spec_path.parent().unwrap().to_path_buf(), json)
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

#[test]
fn test_decode_golden_node_record_against_expected_json() {
    let (vectors_dir, spec) = load_expected_spec();
    let node_spec = &spec["vectors"]["golden_node_record"];

    let filename = node_spec["file"].as_str().expect("file name");
    let expected_hex = node_spec["hex"].as_str().expect("hex");
    let expected_byte_len = node_spec["byte_length"].as_u64().expect("byte_length") as usize;
    let expected = &node_spec["expected"];

    let fixture_path = vectors_dir.join(filename);
    let bytes = fs::read(&fixture_path)
        .unwrap_or_else(|e| panic!("Failed to read fixture from {:?}: {}", fixture_path, e));

    // 1. Assert raw bytes match canonical specification
    assert_eq!(bytes.len(), expected_byte_len, "Fixture byte length mismatch");
    assert_eq!(to_hex(&bytes), expected_hex, "Fixture hex mismatch");

    // 2. Decode and assert against JSON oracle
    let node = NodeRecord::decode(&bytes[..]).expect("Decode NodeRecord");

    assert_eq!(node.node_id, expected["node_id"].as_u64().unwrap());
    assert_eq!(node.parent_id, expected["parent_id"].as_u64().unwrap());
    assert_eq!(node.child_index, expected["child_index"].as_u64().unwrap() as u32);

    let type_ref = node.r#type.expect("type_ref");
    assert_eq!(type_ref.namespace_id, expected["type"]["namespace_id"].as_u64().unwrap() as u32);
    assert_eq!(type_ref.local_id, expected["type"]["local_id"].as_u64().unwrap() as u32);

    let expected_props = expected["properties"].as_array().expect("properties array");
    assert_eq!(node.properties.len(), expected_props.len());

    // Property 0: label
    let p0 = &node.properties[0];
    let p0_ref = p0.property.as_ref().unwrap();
    assert_eq!(p0_ref.local_id, expected_props[0]["property"]["local_id"].as_u64().unwrap() as u32);
    match &p0.value.as_ref().unwrap().value {
        Some(value::Value::StringValue(s)) => {
            assert_eq!(s, expected_props[0]["value"]["string_value"].as_str().unwrap());
        }
        other => panic!("Expected StringValue, got {:?}", other),
    }

    // Property 1: role
    let p1 = &node.properties[1];
    let p1_ref = p1.property.as_ref().unwrap();
    assert_eq!(p1_ref.local_id, expected_props[1]["property"]["local_id"].as_u64().unwrap() as u32);
    match &p1.value.as_ref().unwrap().value {
        Some(value::Value::EnumValue(ev)) => {
            let ev_spec = &expected_props[1]["value"]["enum_value"];
            assert_eq!(ev.enum_id, ev_spec["enum_id"].as_u64().unwrap() as u32);
            assert_eq!(ev.value_id, ev_spec["value_id"].as_u64().unwrap() as u32);
        }
        other => panic!("Expected EnumValue, got {:?}", other),
    }

    // Property 2: enabled
    let p2 = &node.properties[2];
    let p2_ref = p2.property.as_ref().unwrap();
    assert_eq!(p2_ref.local_id, expected_props[2]["property"]["local_id"].as_u64().unwrap() as u32);
    match &p2.value.as_ref().unwrap().value {
        Some(value::Value::BoolValue(b)) => {
            assert_eq!(*b, expected_props[2]["value"]["bool_value"].as_bool().unwrap());
        }
        other => panic!("Expected BoolValue, got {:?}", other),
    }

    // 3. Re-encode and verify identical wire bytes
    let mut roundtrip = Vec::new();
    node.encode(&mut roundtrip).unwrap();
    assert_eq!(roundtrip, bytes, "Roundtrip re-encode mismatch");
}

#[test]
fn test_decode_golden_transaction_against_expected_json() {
    let (vectors_dir, spec) = load_expected_spec();
    let tx_spec = &spec["vectors"]["golden_transaction"];

    let filename = tx_spec["file"].as_str().expect("file name");
    let expected_hex = tx_spec["hex"].as_str().expect("hex");
    let expected_byte_len = tx_spec["byte_length"].as_u64().expect("byte_length") as usize;
    let expected = &tx_spec["expected"];

    let fixture_path = vectors_dir.join(filename);
    let bytes = fs::read(&fixture_path)
        .unwrap_or_else(|e| panic!("Failed to read fixture from {:?}: {}", fixture_path, e));

    // 1. Assert raw bytes match canonical specification
    assert_eq!(bytes.len(), expected_byte_len, "Fixture byte length mismatch");
    assert_eq!(to_hex(&bytes), expected_hex, "Fixture hex mismatch");

    // 2. Decode and assert against JSON oracle
    let tx = Transaction::decode(&bytes[..]).expect("Decode Transaction");

    assert_eq!(tx.base_revision, expected["base_revision"].as_u64().unwrap());
    assert_eq!(tx.new_revision, expected["new_revision"].as_u64().unwrap());
    assert_eq!(tx.priority, expected["priority"].as_u64().unwrap() as u32);

    let expected_ops = expected["operations"].as_array().expect("operations array");
    assert_eq!(tx.operations.len(), expected_ops.len());

    // Op 0: CREATE_NODE
    match &tx.operations[0].op {
        Some(operation::Op::CreateNode(create_op)) => {
            let exp_create = &expected_ops[0]["create_node"]["node"];
            let node = create_op.node.as_ref().unwrap();
            assert_eq!(node.node_id, exp_create["node_id"].as_u64().unwrap());
            assert_eq!(node.parent_id, exp_create["parent_id"].as_u64().unwrap());
            assert_eq!(node.child_index, exp_create["child_index"].as_u64().unwrap() as u32);
            assert_eq!(node.r#type.as_ref().unwrap().local_id, exp_create["type"]["local_id"].as_u64().unwrap() as u32);

            let exp_prop = &exp_create["properties"][0];
            let p0 = &node.properties[0];
            assert_eq!(p0.property.as_ref().unwrap().local_id, exp_prop["property"]["local_id"].as_u64().unwrap() as u32);
            match &p0.value.as_ref().unwrap().value {
                Some(value::Value::StringValue(s)) => {
                    assert_eq!(s, exp_prop["value"]["string_value"].as_str().unwrap());
                }
                other => panic!("Expected StringValue, got {:?}", other),
            }
        }
        other => panic!("Expected CreateNode op, got {:?}", other),
    }

    // Op 1: SET_PROPERTY
    match &tx.operations[1].op {
        Some(operation::Op::SetProperty(set_op)) => {
            let exp_set = &expected_ops[1]["set_property"];
            assert_eq!(set_op.node_id, exp_set["node_id"].as_u64().unwrap());
            assert_eq!(set_op.property.as_ref().unwrap().local_id, exp_set["property"]["local_id"].as_u64().unwrap() as u32);
            match &set_op.value.as_ref().unwrap().value {
                Some(value::Value::FloatValue(f)) => {
                    let expected_f = exp_set["value"]["float_value"].as_f64().unwrap();
                    assert!((f - expected_f).abs() < 1e-6);
                }
                other => panic!("Expected FloatValue, got {:?}", other),
            }
        }
        other => panic!("Expected SetProperty op, got {:?}", other),
    }

    // Op 2: BATCH_PROPERTY_SET
    match &tx.operations[2].op {
        Some(operation::Op::BatchPropertySet(batch_op)) => {
            let exp_batch = &expected_ops[2]["batch_property_set"];
            assert_eq!(batch_op.node_id, exp_batch["node_id"].as_u64().unwrap());
            let exp_prop = &exp_batch["properties"][0];
            let p0 = &batch_op.properties[0];
            assert_eq!(p0.property.as_ref().unwrap().local_id, exp_prop["property"]["local_id"].as_u64().unwrap() as u32);
            match &p0.value.as_ref().unwrap().value {
                Some(value::Value::SizeValue(size)) => {
                    let exp_size = &exp_prop["value"]["size_value"];
                    assert_eq!(size.width, exp_size["width"].as_f64().unwrap());
                    assert_eq!(size.height, exp_size["height"].as_f64().unwrap());
                }
                other => panic!("Expected SizeValue, got {:?}", other),
            }
        }
        other => panic!("Expected BatchPropertySet op, got {:?}", other),
    }

    // 3. Re-encode and verify identical wire bytes
    let mut roundtrip = Vec::new();
    tx.encode(&mut roundtrip).unwrap();
    assert_eq!(roundtrip, bytes, "Roundtrip re-encode mismatch");
}
