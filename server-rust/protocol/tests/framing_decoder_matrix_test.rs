use bytes::BytesMut;
use srui_protocol::{
    decode_framed, encode_framed, srui_message, FramingError, SruiCodec, SruiMessage, Transaction,
    DEFAULT_MAX_FRAME_SIZE,
};
use std::fs;
use std::path::Path;
use tokio_util::codec::Decoder;

fn golden_framed_bytes() -> Vec<u8> {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    fs::read(
        Path::new(manifest_dir)
            .join("../../protocol/conformance-vectors/golden_framed_message.bin"),
    )
    .expect("read golden_framed_message.bin")
}

fn golden_message() -> SruiMessage {
    decode_framed(&golden_framed_bytes()).expect("decode golden framed message")
}

fn sample_message() -> SruiMessage {
    SruiMessage {
        msg: Some(srui_message::Msg::Transaction(Transaction {
            base_revision: 1,
            new_revision: 2,
            priority: 1,
            operations: vec![],
        })),
    }
}

fn build_synthetic_frame(declared_payload_len: usize, payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::new();
    prost::encode_length_delimiter(declared_payload_len, &mut frame)
        .expect("encode length delimiter");
    frame.extend_from_slice(payload);
    frame
}

fn varint_prefix_len(value: usize) -> usize {
    let mut buf = Vec::new();
    prost::encode_length_delimiter(value, &mut buf).expect("encode length delimiter");
    buf.len()
}

fn feed_all_chunks<F>(frame: &[u8], chunk_size: usize, mut on_chunk: F) -> Option<SruiMessage>
where
    F: FnMut(&mut SruiCodec, &mut BytesMut) -> Result<Option<SruiMessage>, FramingError>,
{
    let mut codec = SruiCodec::new();
    let mut buf = BytesMut::new();
    let mut last = None;

    for chunk in frame.chunks(chunk_size) {
        buf.extend_from_slice(chunk);
        if let Some(msg) = on_chunk(&mut codec, &mut buf).expect("decode should not panic") {
            last = Some(msg);
        }
    }

    if buf.is_empty() {
        last
    } else if let Some(msg) = on_chunk(&mut codec, &mut buf).expect("final decode should not panic")
    {
        Some(msg)
    } else {
        last
    }
}

fn decode_by_split_points(frame: &[u8]) -> SruiMessage {
    let expected = golden_message();
    for split in 1..frame.len() {
        let mut codec = SruiCodec::new();
        let mut buf = BytesMut::new();

        buf.extend_from_slice(&frame[..split]);
        assert!(
            codec.decode(&mut buf).expect("first chunk").is_none(),
            "split at {split} should be incomplete after first chunk"
        );

        buf.extend_from_slice(&frame[split..]);
        let decoded = codec
            .decode(&mut buf)
            .expect("second chunk decode")
            .expect("split at {split} should complete on second chunk");
        assert_eq!(decoded, expected, "split at {split}");
        assert!(
            buf.is_empty(),
            "split at {split} should consume entire frame"
        );
    }
    expected
}

#[test]
fn test_golden_frame_split_at_every_byte_boundary() {
    let frame = golden_framed_bytes();
    assert!(frame.len() > 2, "golden frame should be nontrivial");
    decode_by_split_points(&frame);
}

#[test]
fn test_golden_frame_one_byte_feeds() {
    let frame = golden_framed_bytes();
    let expected = golden_message();

    let decoded = feed_all_chunks(&frame, 1, |codec, buf| codec.decode(buf))
        .expect("one-byte feeds should eventually decode golden frame");
    assert_eq!(decoded, expected);
}

#[test]
fn test_coalesced_golden_frames_decode_in_wire_order() {
    let frame = golden_framed_bytes();
    let expected = golden_message();
    let mut coalesced = frame.clone();
    coalesced.extend_from_slice(&frame);

    let mut codec = SruiCodec::new();
    let mut buf = BytesMut::from(&coalesced[..]);

    let first = codec
        .decode(&mut buf)
        .expect("first frame decode")
        .expect("first coalesced frame");
    assert_eq!(first, expected);

    let second = codec
        .decode(&mut buf)
        .expect("second frame decode")
        .expect("second coalesced frame");
    assert_eq!(second, expected);
    assert!(buf.is_empty());
}

#[test]
fn test_varint_length_prefix_boundaries_wait_for_payload() {
    for declared_len in [1usize, 127, 128] {
        let frame = build_synthetic_frame(declared_len, &vec![0u8; declared_len]);
        let prefix_len = varint_prefix_len(declared_len);

        let mut codec = SruiCodec::new();
        let mut buf = BytesMut::from(&frame[..prefix_len]);
        assert!(
            codec
                .decode(&mut buf)
                .expect("prefix-only decode")
                .is_none(),
            "declared length {declared_len} should wait for payload"
        );

        buf.extend_from_slice(&frame[prefix_len..]);
        assert!(
            matches!(codec.decode(&mut buf), Err(FramingError::DecodeError(_))),
            "declared length {declared_len} with zero-filled payload should fail protobuf decode"
        );
    }

    let empty_frame = build_synthetic_frame(0, &[]);
    let mut codec = SruiCodec::new();
    let mut buf = BytesMut::from(&empty_frame[..]);
    assert_eq!(
        codec
            .decode(&mut buf)
            .expect("length-0 frame decode")
            .expect("length-0 frame"),
        SruiMessage::default(),
        "declared length 0 is complete with only the prefix byte"
    );
}

#[test]
fn test_max_declared_length_accepted_and_max_plus_one_rejected() {
    let mut codec = SruiCodec::new();
    let mut at_limit = Vec::new();
    prost::encode_length_delimiter(DEFAULT_MAX_FRAME_SIZE, &mut at_limit)
        .expect("encode max length delimiter");
    let mut over_limit = Vec::new();
    prost::encode_length_delimiter(DEFAULT_MAX_FRAME_SIZE + 1, &mut over_limit)
        .expect("encode max+1 length delimiter");

    let mut buf = BytesMut::from(&at_limit[..]);
    assert!(
        codec.decode(&mut buf).expect("max prefix decode").is_none(),
        "max declared length should wait for payload"
    );

    buf.clear();
    buf.extend_from_slice(&over_limit);
    let err = codec
        .decode(&mut buf)
        .expect_err("max+1 declared length should fail immediately");
    assert!(matches!(
        err,
        FramingError::FrameSizeLimitExceeded {
            limit,
            actual
        } if limit == DEFAULT_MAX_FRAME_SIZE && actual == DEFAULT_MAX_FRAME_SIZE + 1
    ));
}

#[test]
fn test_incomplete_varint_prefix_returns_none() {
    let mut codec = SruiCodec::new();
    let mut buf = BytesMut::from(&[0x80][..]); // prefix for declared length 128

    assert!(
        codec.decode(&mut buf).expect("incomplete prefix").is_none(),
        "single-byte continuation prefix should wait for more data"
    );
    assert_eq!(buf.len(), 1, "incomplete prefix bytes should be retained");
}

#[test]
fn test_overlong_varint_returns_decode_error_without_consuming() {
    let mut codec = SruiCodec::new();
    let mut buf = BytesMut::from(&[0x80u8; 11][..]);

    let err = codec.decode(&mut buf).expect_err("overlong varint");
    assert!(matches!(err, FramingError::DecodeError(_)));
    assert_eq!(
        buf.len(),
        11,
        "overlong varint bytes should remain in buffer"
    );
}

#[test]
fn test_declared_length_exceeds_available_payload_waits_then_recovers() {
    let truncated = [0x64, 0x01, 0x02, 0x03, 0x04]; // declares 100, provides 4
    let valid = encode_framed(&sample_message()).expect("encode valid frame");

    let mut codec = SruiCodec::new();
    let mut buf = BytesMut::from(&truncated[..]);
    assert!(
        codec.decode(&mut buf).expect("truncated frame").is_none(),
        "declared length greater than payload should wait"
    );

    buf.clear();
    buf.extend_from_slice(&valid);
    let decoded = codec
        .decode(&mut buf)
        .expect("recovery decode")
        .expect("valid frame after truncated wait");
    assert_eq!(decoded, sample_message());
}

#[test]
fn test_decoder_recovers_after_overlong_varint() {
    let valid = encode_framed(&sample_message()).expect("encode valid frame");

    let mut codec = SruiCodec::new();
    let mut buf = BytesMut::from(&[0x80u8; 11][..]);
    assert!(matches!(
        codec.decode(&mut buf),
        Err(FramingError::DecodeError(_))
    ));

    buf.clear();
    buf.extend_from_slice(&valid);
    let decoded = codec
        .decode(&mut buf)
        .expect("recovery decode")
        .expect("valid frame after overlong varint");
    assert_eq!(decoded, sample_message());
}

#[test]
fn test_decoder_recovers_after_max_plus_one_declared_length() {
    let valid = encode_framed(&sample_message()).expect("encode valid frame");
    let mut over_limit = Vec::new();
    prost::encode_length_delimiter(DEFAULT_MAX_FRAME_SIZE + 1, &mut over_limit)
        .expect("encode max+1 length delimiter");

    let mut codec = SruiCodec::new();
    let mut buf = BytesMut::from(&over_limit[..]);
    assert!(matches!(
        codec.decode(&mut buf),
        Err(FramingError::FrameSizeLimitExceeded { .. })
    ));

    buf.clear();
    buf.extend_from_slice(&valid);
    let decoded = codec
        .decode(&mut buf)
        .expect("recovery decode")
        .expect("valid frame after max+1 rejection");
    assert_eq!(decoded, sample_message());
}

#[test]
fn test_decoder_recovers_after_invalid_protobuf_payload() {
    let valid = encode_framed(&sample_message()).expect("encode valid frame");
    let invalid = build_synthetic_frame(1, &[0x08]); // tag 1, wire type 0, no value

    let mut codec = SruiCodec::new();
    let mut buf = BytesMut::from(&invalid[..]);
    assert!(matches!(
        codec.decode(&mut buf),
        Err(FramingError::DecodeError(_))
    ));
    assert!(
        buf.is_empty(),
        "invalid frame should be consumed from buffer"
    );

    buf.extend_from_slice(&valid);
    let decoded = codec
        .decode(&mut buf)
        .expect("recovery decode")
        .expect("valid frame after invalid protobuf");
    assert_eq!(decoded, sample_message());
}

#[test]
fn test_decoder_recovers_after_incomplete_prefix_without_clearing() {
    let frame = golden_framed_bytes();
    let expected = golden_message();
    let prefix_len = varint_prefix_len(104); // golden payload length from conformance vector

    let mut codec = SruiCodec::new();
    let mut buf = BytesMut::from(&frame[..prefix_len - 1]);
    assert!(
        codec.decode(&mut buf).expect("partial prefix").is_none(),
        "partial varint prefix should wait"
    );

    buf.extend_from_slice(&frame[(prefix_len - 1)..]);
    let decoded = codec
        .decode(&mut buf)
        .expect("completion decode")
        .expect("golden frame after prefix completion");
    assert_eq!(decoded, expected);
}
