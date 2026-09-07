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
    node_record
        .encode(&mut node_bytes)
        .expect("encode NodeRecord");
    fs::write(out_dir.join("golden_node_record.bin"), &node_bytes)
        .expect("write golden_node_record.bin");
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
                                value: Some(value::Value::StringValue(
                                    "27 tests passed".to_string(),
                                )),
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
    transaction
        .encode(&mut tx_bytes)
        .expect("encode Transaction");
    fs::write(out_dir.join("golden_transaction.bin"), &tx_bytes)
        .expect("write golden_transaction.bin");
    println!("Wrote golden_transaction.bin ({} bytes)", tx_bytes.len());

    // 3. Construct golden Framed SruiMessage
    let framed_message = SruiMessage {
        msg: Some(srui_message::Msg::Transaction(transaction)),
    };
    let framed_bytes = encode_framed(&framed_message).expect("encode framed SruiMessage");
    fs::write(out_dir.join("golden_framed_message.bin"), &framed_bytes)
        .expect("write golden_framed_message.bin");
    println!(
        "Wrote golden_framed_message.bin ({} bytes)",
        framed_bytes.len()
    );

    // 4. Construct golden Framed ServerEventAck (§18.2)
    let event_ack = SruiMessage {
        msg: Some(srui_message::Msg::ServerEventAck(ServerEventAck {
            client_instance_id: b"c17".to_vec(),
            event_id: b"e123".to_vec(),
            last_processed_event_seq: 593,
            status: EventAckStatus::Processed as i32,
            revision_after_effect: 1843,
            reject_reason: String::new(),
            session_id: "s-91c".to_string(),
        })),
    };
    let event_ack_bytes = encode_framed(&event_ack).expect("encode framed ServerEventAck");
    fs::write(out_dir.join("golden_event_ack.bin"), &event_ack_bytes)
        .expect("write golden_event_ack.bin");
    println!(
        "Wrote golden_event_ack.bin ({} bytes)",
        event_ack_bytes.len()
    );

    // 5. Construct Malformed Fixture: Overlong Varint (11 bytes, exceeds 64-bit 10-byte limit)
    let overlong_varint_bytes = vec![
        0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x01,
    ];
    fs::write(
        out_dir.join("malformed_overlong_varint.bin"),
        &overlong_varint_bytes,
    )
    .expect("write malformed_overlong_varint.bin");
    println!(
        "Wrote malformed_overlong_varint.bin ({} bytes)",
        overlong_varint_bytes.len()
    );

    // 6. Construct Malformed Fixture: Truncated Frame (declares 100 bytes length, has only 4)
    let truncated_frame_bytes = vec![0x64, 0x01, 0x02, 0x03, 0x04];
    fs::write(
        out_dir.join("malformed_truncated_frame.bin"),
        &truncated_frame_bytes,
    )
    .expect("write malformed_truncated_frame.bin");
    println!(
        "Wrote malformed_truncated_frame.bin ({} bytes)",
        truncated_frame_bytes.len()
    );

    // 7. Construct golden framed ClientModelRangeRequest (§8, §22.7)
    let range_request = SruiMessage {
        msg: Some(srui_message::Msg::ClientModelRangeRequest(
            ClientModelRangeRequest {
                node_id: 7,
                model_id: 11,
                start_index: 128,
                count: 64,
                observed_revision: 5,
            },
        )),
    };
    let range_bytes = encode_framed(&range_request).expect("encode framed ClientModelRangeRequest");
    fs::write(
        out_dir.join("golden_client_model_range_request.bin"),
        &range_bytes,
    )
    .expect("write golden_client_model_range_request.bin");
    println!(
        "Wrote golden_client_model_range_request.bin ({} bytes)",
        range_bytes.len()
    );

    // 8. Construct golden framed TEXT_EDIT (§18.3, §22.6).
    let text_edit_event = SruiMessage {
        msg: Some(srui_message::Msg::Event(Event {
            client_instance_id: b"client-29".to_vec(),
            event_seq: 29,
            event_id: b"event-text-29".to_vec(),
            observed_revision: 41,
            node_id: 7,
            event_type: Some(TypeRef {
                namespace_id: STANDARD_NAMESPACE_ID,
                local_id: StandardEvent::EventTextEdit as u32,
            }),
            arguments: vec![Property {
                property: Some(PropertyRef {
                    namespace_id: STANDARD_NAMESPACE_ID,
                    local_id: StandardProperty::PropertyText as u32,
                }),
                value: Some(Value {
                    value: Some(value::Value::StringValue("composed text".to_string())),
                }),
            }],
            edit_seq: 3,
        })),
    };
    let text_edit_bytes = encode_framed(&text_edit_event).expect("encode framed TEXT_EDIT event");
    fs::write(out_dir.join("golden_text_edit_event.bin"), &text_edit_bytes)
        .expect("write golden_text_edit_event.bin");
    println!(
        "Wrote golden_text_edit_event.bin ({} bytes)",
        text_edit_bytes.len()
    );

    // 9. Construct protobuf-valid TEXT_EDIT missing its required positive edit_seq.
    let malformed_text_edit = SruiMessage {
        msg: Some(srui_message::Msg::Event(Event {
            client_instance_id: b"client-29".to_vec(),
            event_seq: 30,
            event_id: b"bad-text-zero".to_vec(),
            observed_revision: 41,
            node_id: 7,
            event_type: Some(TypeRef {
                namespace_id: STANDARD_NAMESPACE_ID,
                local_id: StandardEvent::EventTextEdit as u32,
            }),
            arguments: vec![Property {
                property: Some(PropertyRef {
                    namespace_id: STANDARD_NAMESPACE_ID,
                    local_id: StandardProperty::PropertyText as u32,
                }),
                value: Some(Value {
                    value: Some(value::Value::StringValue("rejected text".to_string())),
                }),
            }],
            edit_seq: 0,
        })),
    };
    let malformed_text_edit_bytes =
        encode_framed(&malformed_text_edit).expect("encode malformed framed TEXT_EDIT event");
    fs::write(
        out_dir.join("malformed_text_edit_zero_edit_seq.bin"),
        &malformed_text_edit_bytes,
    )
    .expect("write malformed_text_edit_zero_edit_seq.bin");
    println!(
        "Wrote malformed_text_edit_zero_edit_seq.bin ({} bytes)",
        malformed_text_edit_bytes.len()
    );

    // 10. Construct protobuf-valid ACTIVATE carrying a forbidden edit_seq.
    let malformed_activate = SruiMessage {
        msg: Some(srui_message::Msg::Event(Event {
            client_instance_id: b"client-29".to_vec(),
            event_seq: 31,
            event_id: b"bad-activate-seq".to_vec(),
            observed_revision: 41,
            node_id: 7,
            event_type: Some(TypeRef {
                namespace_id: STANDARD_NAMESPACE_ID,
                local_id: StandardEvent::EventActivate as u32,
            }),
            arguments: Vec::new(),
            edit_seq: 1,
        })),
    };
    let malformed_activate_bytes =
        encode_framed(&malformed_activate).expect("encode malformed framed ACTIVATE event");
    fs::write(
        out_dir.join("malformed_activate_nonzero_edit_seq.bin"),
        &malformed_activate_bytes,
    )
    .expect("write malformed_activate_nonzero_edit_seq.bin");
    println!(
        "Wrote malformed_activate_nonzero_edit_seq.bin ({} bytes)",
        malformed_activate_bytes.len()
    );

    // 11–14. Terminal compatibility envelopes (§21)
    let terminal_data = SruiMessage {
        msg: Some(srui_message::Msg::TerminalData(TerminalData {
            stream_id: 7,
            byte_offset: 4096,
            data: b"pty-ok".to_vec(),
        })),
    };
    let terminal_data_bytes = encode_framed(&terminal_data).expect("encode framed TerminalData");
    fs::write(
        out_dir.join("golden_terminal_data.bin"),
        &terminal_data_bytes,
    )
    .expect("write golden_terminal_data.bin");
    println!(
        "Wrote golden_terminal_data.bin ({} bytes)",
        terminal_data_bytes.len()
    );

    let terminal_input = SruiMessage {
        msg: Some(srui_message::Msg::TerminalInput(TerminalInput {
            stream_id: 7,
            data: b"ls\n".to_vec(),
        })),
    };
    let terminal_input_bytes = encode_framed(&terminal_input).expect("encode framed TerminalInput");
    fs::write(
        out_dir.join("golden_terminal_input.bin"),
        &terminal_input_bytes,
    )
    .expect("write golden_terminal_input.bin");
    println!(
        "Wrote golden_terminal_input.bin ({} bytes)",
        terminal_input_bytes.len()
    );

    let terminal_resize = SruiMessage {
        msg: Some(srui_message::Msg::TerminalResize(TerminalResize {
            stream_id: 7,
            columns: 80,
            rows: 24,
            pixel_width: 1280,
            pixel_height: 720,
        })),
    };
    let terminal_resize_bytes =
        encode_framed(&terminal_resize).expect("encode framed TerminalResize");
    fs::write(
        out_dir.join("golden_terminal_resize.bin"),
        &terminal_resize_bytes,
    )
    .expect("write golden_terminal_resize.bin");
    println!(
        "Wrote golden_terminal_resize.bin ({} bytes)",
        terminal_resize_bytes.len()
    );

    let terminal_resync = SruiMessage {
        msg: Some(srui_message::Msg::TerminalResyncRequired(
            TerminalResyncRequired {
                stream_id: 7,
                requested_offset: 100,
                retained_from_offset: 64,
                resume_at_offset: 240,
                reason: TerminalResyncReason::RetentionLoss as i32,
            },
        )),
    };
    let terminal_resync_bytes =
        encode_framed(&terminal_resync).expect("encode framed TerminalResyncRequired");
    fs::write(
        out_dir.join("golden_terminal_resync_required.bin"),
        &terminal_resync_bytes,
    )
    .expect("write golden_terminal_resync_required.bin");
    println!(
        "Wrote golden_terminal_resync_required.bin ({} bytes)",
        terminal_resync_bytes.len()
    );

    // 15. Protobuf-valid TERMINAL_INPUT carrying no payload: forbidden by §21 (the
    // server must never enqueue an empty write onto a PTY master).
    let malformed_terminal_input = SruiMessage {
        msg: Some(srui_message::Msg::TerminalInput(TerminalInput {
            stream_id: 20,
            data: Vec::new(),
        })),
    };
    let malformed_terminal_input_bytes =
        encode_framed(&malformed_terminal_input).expect("encode malformed framed TerminalInput");
    fs::write(
        out_dir.join("malformed_terminal_input_empty.bin"),
        &malformed_terminal_input_bytes,
    )
    .expect("write malformed_terminal_input_empty.bin");
    println!(
        "Wrote malformed_terminal_input_empty.bin ({} bytes)",
        malformed_terminal_input_bytes.len()
    );

    // 16. Protobuf-valid TERMINAL_DATA carrying no payload: forbidden by §21 (an empty
    // frame advances no offset and must be rejected instead of silently applied).
    let malformed_terminal_data = SruiMessage {
        msg: Some(srui_message::Msg::TerminalData(TerminalData {
            stream_id: 20,
            byte_offset: 4096,
            data: Vec::new(),
        })),
    };
    let malformed_terminal_data_bytes =
        encode_framed(&malformed_terminal_data).expect("encode malformed framed TerminalData");
    fs::write(
        out_dir.join("malformed_terminal_data_empty.bin"),
        &malformed_terminal_data_bytes,
    )
    .expect("write malformed_terminal_data_empty.bin");
    println!(
        "Wrote malformed_terminal_data_empty.bin ({} bytes)",
        malformed_terminal_data_bytes.len()
    );
}
