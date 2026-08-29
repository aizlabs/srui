//! Fuzz target for length-delimited framing decoders (§16, §26).
//!
//! Invariants: no panic/hang; decode paths enforce frame-size limits (bounded memory).

#![no_main]

use bytes::BytesMut;
use libfuzzer_sys::fuzz_target;
use srui_protocol::{decode_framed, SruiCodec, SruiMessage, DEFAULT_MAX_FRAME_SIZE};
use tokio_util::codec::Decoder;

fuzz_target!(|data: &[u8]| {
    if data.len() > DEFAULT_MAX_FRAME_SIZE + 16 {
        return;
    }

    let _ = decode_framed::<SruiMessage>(data);

    let mut codec = SruiCodec::new();
    let mut buf = BytesMut::from(data);
    let _ = codec.decode(&mut buf);

    for chunk_size in [1usize, 3, 7, 13] {
        let mut codec = SruiCodec::new();
        let mut buf = BytesMut::new();
        for chunk in data.chunks(chunk_size) {
            buf.extend_from_slice(chunk);
            match codec.decode(&mut buf) {
                Ok(_) | Err(_) => {}
            }
        }
    }
});
