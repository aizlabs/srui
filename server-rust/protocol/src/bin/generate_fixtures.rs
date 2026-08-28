use prost::Message;
use srui_protocol::*;
use std::fs;
use std::path::Path;

fn main() {
    if std::env::var("SRUI_WRITE_FIXTURES").as_deref() != Ok("1") {
        eprintln!(
            "Refusing to overwrite conformance vectors.\n\
             Set SRUI_WRITE_FIXTURES=1 if you intentionally need to regenerate golden bytes."
        );
        std::process::exit(1);
    }

    let out_dir = Path::new("../../protocol/conformance-vectors");
    fs::create_dir_all(out_dir).expect("create conformance-vectors dir");

    // 1. Construct golden NodeRecord
    let node_record = NodeRecord {
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
                        enum_id: 2, // ActionRole
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
    };

    let mut node_bytes = Vec::new();
    node_record.encode(&mut node_bytes).expect("encode NodeRecord");
    fs::write(out_dir.join("golden_node_record.bin"), &node_bytes).expect("write golden_node_record.bin");
    println!("Wrote golden_node_record.bin ({} bytes)", node_bytes.len());

    // 2. Construct golden Transaction
    let transaction = Transaction {
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
    };

    let mut tx_bytes = Vec::new();
    transaction.encode(&mut tx_bytes).expect("encode Transaction");
    fs::write(out_dir.join("golden_transaction.bin"), &tx_bytes).expect("write golden_transaction.bin");
    println!("Wrote golden_transaction.bin ({} bytes)", tx_bytes.len());

    // 3. Construct golden Framed SruiMessage
    let framed_message = SruiMessage {
        msg: Some(srui_message::Msg::Transaction(transaction)),
    };
    let framed_bytes = encode_framed(&framed_message).expect("encode framed SruiMessage");
    fs::write(out_dir.join("golden_framed_message.bin"), &framed_bytes).expect("write golden_framed_message.bin");
    println!("Wrote golden_framed_message.bin ({} bytes)", framed_bytes.len());
}
