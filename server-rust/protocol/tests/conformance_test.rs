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

fn create_authored_node_record() -> NodeRecord {
    NodeRecord {
        node_id: 42,
        r#type: Some(TypeRef {
            namespace_id: STANDARD_NAMESPACE_ID,
            local_id: StandardNodeType::NodeTypeButton as u32,
        }),
        parent_id: 1,
        child_index: 0,
        properties: vec![
            Property {
                property: Some(PropertyRef {
                    namespace_id: STANDARD_NAMESPACE_ID,
                    local_id: StandardProperty::PropertyLabel as u32,
                }),
                value: Some(Value {
                    value: Some(value::Value::StringValue("Delete".to_string())),
                }),
            },
            Property {
                property: Some(PropertyRef {
                    namespace_id: STANDARD_NAMESPACE_ID,
                    local_id: StandardProperty::PropertyRole as u32,
                }),
                value: Some(Value {
                    value: Some(value::Value::EnumValue(EnumValue {
                        enum_id: StandardEnum::EnumActionRole as u32,
                        value_id: ActionRole::Destructive as u32,
                    })),
                }),
            },
            Property {
                property: Some(PropertyRef {
                    namespace_id: STANDARD_NAMESPACE_ID,
                    local_id: StandardProperty::PropertyEnabled as u32,
                }),
                value: Some(Value {
                    value: Some(value::Value::BoolValue(true)),
                }),
            },
        ],
    }
}

fn create_authored_transaction() -> Transaction {
    Transaction {
        base_revision: 104,
        new_revision: 105,
        priority: 1,
        operations: vec![
            Operation {
                op: Some(operation::Op::CreateNode(CreateNodeOp {
                    node: Some(NodeRecord {
                        node_id: 19,
                        r#type: Some(TypeRef {
                            namespace_id: STANDARD_NAMESPACE_ID,
                            local_id: StandardNodeType::NodeTypeText as u32,
                        }),
                        parent_id: 2,
                        child_index: 3,
                        properties: vec![Property {
                            property: Some(PropertyRef {
                                namespace_id: STANDARD_NAMESPACE_ID,
                                local_id: StandardProperty::PropertyText as u32,
                            }),
                            value: Some(Value {
                                value: Some(value::Value::StringValue("27 tests passed".to_string())),
                            }),
                        }],
                    }),
                })),
            },
            Operation {
                op: Some(operation::Op::SetProperty(SetPropertyOp {
                    node_id: 4,
                    property: Some(PropertyRef {
                        namespace_id: STANDARD_NAMESPACE_ID,
                        local_id: StandardProperty::PropertyValue as u32,
                    }),
                    value: Some(Value {
                        value: Some(value::Value::FloatValue(0.71)),
                    }),
                })),
            },
            Operation {
                op: Some(operation::Op::BatchPropertySet(BatchPropertySetOp {
                    node_id: 19,
                    properties: vec![Property {
                        property: Some(PropertyRef {
                            namespace_id: STANDARD_NAMESPACE_ID,
                            local_id: StandardProperty::PropertyMinimumSize as u32,
                        }),
                        value: Some(Value {
                            value: Some(value::Value::SizeValue(SizeVal {
                                width: 120.0,
                                height: 24.0,
                            })),
                        }),
                    }],
                })),
            },
        ],
    }
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

#[test]
fn test_direct_encode_golden_node_record_matches_wire_bytes() {
    let (vectors_dir, spec) = load_expected_spec();
    let node_spec = &spec["vectors"]["golden_node_record"];
    let filename = node_spec["file"].as_str().unwrap();
    let expected_hex = node_spec["hex"].as_str().unwrap();
    let fixture_bytes = fs::read(vectors_dir.join(filename)).unwrap();

    let authored_node = create_authored_node_record();
    let mut encoded = Vec::new();
    authored_node.encode(&mut encoded).unwrap();

    assert_eq!(to_hex(&encoded), expected_hex);
    assert_eq!(encoded, fixture_bytes);
}

#[test]
fn test_direct_encode_golden_transaction_matches_wire_bytes() {
    let (vectors_dir, spec) = load_expected_spec();
    let tx_spec = &spec["vectors"]["golden_transaction"];
    let filename = tx_spec["file"].as_str().unwrap();
    let expected_hex = tx_spec["hex"].as_str().unwrap();
    let fixture_bytes = fs::read(vectors_dir.join(filename)).unwrap();

    let authored_tx = create_authored_transaction();
    let mut encoded = Vec::new();
    authored_tx.encode(&mut encoded).unwrap();

    assert_eq!(to_hex(&encoded), expected_hex);
    assert_eq!(encoded, fixture_bytes);
}

#[test]
fn test_cross_language_rust_vs_swift_byte_equality() {
    let (vectors_dir, _) = load_expected_spec();

    // 1. Rust-encoded NodeRecord must match golden fixture bit-for-bit
    let rust_node = create_authored_node_record();
    let mut rust_node_bytes = Vec::new();
    rust_node.encode(&mut rust_node_bytes).unwrap();
    let golden_node_bytes = fs::read(vectors_dir.join("golden_node_record.bin")).unwrap();
    assert_eq!(
        rust_node_bytes, golden_node_bytes,
        "Rust-encoded NodeRecord does not match golden bytes"
    );

    // 2. Rust-encoded Transaction must match golden fixture bit-for-bit
    let rust_tx = create_authored_transaction();
    let mut rust_tx_bytes = Vec::new();
    rust_tx.encode(&mut rust_tx_bytes).unwrap();
    let golden_tx_bytes = fs::read(vectors_dir.join("golden_transaction.bin")).unwrap();
    assert_eq!(
        rust_tx_bytes, golden_tx_bytes,
        "Rust-encoded Transaction does not match golden bytes"
    );

    // 3. Rust-encoded Framed SruiMessage must match golden fixture bit-for-bit
    let rust_framed_msg = SruiMessage {
        msg: Some(srui_message::Msg::Transaction(create_authored_transaction())),
    };
    let rust_framed_bytes = encode_framed(&rust_framed_msg).unwrap();
    let golden_framed_bytes = fs::read(vectors_dir.join("golden_framed_message.bin")).unwrap();
    assert_eq!(
        rust_framed_bytes, golden_framed_bytes,
        "Rust-encoded Framed SruiMessage does not match golden bytes"
    );
}

#[test]
fn test_decode_golden_framed_message_against_expected_json() {
    let (vectors_dir, spec) = load_expected_spec();
    let framed_spec = &spec["vectors"]["golden_framed_message"];

    let filename = framed_spec["file"].as_str().expect("file name");
    let expected_hex = framed_spec["hex"].as_str().expect("hex");
    let expected_byte_len = framed_spec["byte_length"].as_u64().expect("byte_length") as usize;
    let expected = &framed_spec["expected"];

    let fixture_path = vectors_dir.join(filename);
    let bytes = fs::read(&fixture_path)
        .unwrap_or_else(|e| panic!("Failed to read fixture from {:?}: {}", fixture_path, e));

    // 1. Assert raw bytes match canonical specification
    assert_eq!(bytes.len(), expected_byte_len, "Fixture byte length mismatch");
    assert_eq!(to_hex(&bytes), expected_hex, "Fixture hex mismatch");

    // 2. Decode framed message and assert against expected JSON spec
    let decoded: SruiMessage = decode_framed(&bytes[..]).expect("Decode framed SruiMessage");
    match decoded.msg {
        Some(srui_message::Msg::Transaction(ref tx)) => {
            assert_eq!(tx.base_revision, expected["base_revision"].as_u64().unwrap());
            assert_eq!(tx.new_revision, expected["new_revision"].as_u64().unwrap());
            assert_eq!(tx.priority, expected["priority"].as_u64().unwrap() as u32);
            assert_eq!(tx.operations.len(), expected["operation_count"].as_u64().unwrap() as usize);
        }
        other => panic!("Expected Transaction in framed message, got {:?}", other),
    }

    // 3. Re-encode framed and verify identical wire bytes
    let roundtrip = encode_framed(&decoded).expect("re-encode framed");
    assert_eq!(roundtrip, bytes, "Roundtrip re-encode framed mismatch");
}

#[test]
fn test_direct_encode_golden_framed_message_matches_wire_bytes() {
    let (vectors_dir, spec) = load_expected_spec();
    let framed_spec = &spec["vectors"]["golden_framed_message"];
    let filename = framed_spec["file"].as_str().unwrap();
    let expected_hex = framed_spec["hex"].as_str().unwrap();
    let fixture_bytes = fs::read(vectors_dir.join(filename)).unwrap();

    let msg = SruiMessage {
        msg: Some(srui_message::Msg::Transaction(create_authored_transaction())),
    };
    let encoded = encode_framed(&msg).expect("encode framed");

    assert_eq!(to_hex(&encoded), expected_hex);
    assert_eq!(encoded, fixture_bytes);
}

#[test]
fn test_length_delimited_framing_conformance() {
    let msg = SruiMessage {
        msg: Some(srui_message::Msg::Transaction(create_authored_transaction())),
    };

    let framed_bytes = encode_framed(&msg).expect("encode framed");
    assert!(!framed_bytes.is_empty());

    let decoded: SruiMessage = decode_framed(&framed_bytes).expect("decode framed");
    match decoded.msg {
        Some(srui_message::Msg::Transaction(tx)) => {
            assert_eq!(tx.base_revision, 104);
            assert_eq!(tx.new_revision, 105);
            assert_eq!(tx.operations.len(), 3);
        }
        other => panic!("Expected Transaction payload, got {:?}", other),
    }
}
