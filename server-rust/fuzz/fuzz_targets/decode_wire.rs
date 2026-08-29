//! Fuzz target for semantic-tree wire decoders (§16).
//!
//! Invariants: no panic/hang on arbitrary protobuf payloads.

#![no_main]

use libfuzzer_sys::fuzz_target;
use srui_protocol::DEFAULT_MAX_FRAME_SIZE;
use srui_semantic_tree::{
    decode_event, decode_message, decode_node_record, decode_operation, decode_transaction,
    decode_value,
};

fuzz_target!(|data: &[u8]| {
    if data.len() > DEFAULT_MAX_FRAME_SIZE {
        return;
    }

    let _ = decode_transaction(data);
    let _ = decode_node_record(data);
    let _ = decode_value(data);
    let _ = decode_operation(data);
    let _ = decode_event(data);
    let _ = decode_message(data);
});
