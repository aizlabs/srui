use bytes::BytesMut;
use futures::{SinkExt, StreamExt};
use prost::Message;
use srui_protocol::{
    srui_message, ClientHello, ClientLimits, FramingError, SruiCodec, SruiMessage, Transaction,
};
use tokio::io::{duplex, AsyncWriteExt};
use tokio_util::codec::{Decoder, FramedRead, FramedWrite};

#[test]
fn test_decode_does_not_eagerly_reserve_full_frame() {
    let mut codec = SruiCodec::new();
    let mut buf = BytesMut::new();

    // Header claims 1 MiB payload but only a few payload bytes are present.
    let claimed_payload = 1024 * 1024;
    prost::encode_length_delimiter(claimed_payload, &mut buf).expect("encode length delimiter");
    buf.extend_from_slice(&[0u8; 10]);

    let capacity_before = buf.capacity();
    let result = codec.decode(&mut buf).expect("decode should not error");
    assert!(result.is_none(), "incomplete frame should return Ok(None)");

    let growth = buf.capacity().saturating_sub(capacity_before);
    assert!(
        buf.capacity() < 128 * 1024,
        "capacity {} should stay well below claimed frame size {}",
        buf.capacity(),
        claimed_payload
    );
    assert!(
        growth <= 64 * 1024 + 1024,
        "reserve growth {} should be capped near 64 KiB",
        growth
    );
}

#[test]
fn test_decode_chunked_large_frame_without_upfront_allocation() {
    let msg = SruiMessage {
        msg: Some(srui_message::Msg::Transaction(Transaction {
            base_revision: 1,
            new_revision: 2,
            priority: 1,
            operations: vec![],
        })),
    };

    let mut full_frame = BytesMut::new();
    msg.encode_length_delimited(&mut full_frame)
        .expect("encode frame");

    let mut codec = SruiCodec::new();
    let mut buf = BytesMut::new();
    let chunk_size = 64;

    for chunk in full_frame.chunks(chunk_size) {
        buf.extend_from_slice(chunk);
        let max_capacity_during_decode = buf.capacity();
        assert!(
            max_capacity_during_decode < full_frame.len() + 128 * 1024,
            "buffer should grow incrementally, not reserve full frame upfront"
        );

        if let Some(decoded) = codec.decode(&mut buf).expect("decode should not error") {
            assert_eq!(decoded, msg);
            return;
        }
    }

    panic!("frame should have decoded after all chunks were fed");
}

#[test]
fn test_malformed_overlong_varint_returns_decode_error() {
    let mut codec = SruiCodec::new();
    let mut buf = bytes::BytesMut::from(&[0x80u8; 11][..]);

    let err = codec
        .decode(&mut buf)
        .expect_err("11 bytes of continuation varint should not return Ok(None)");

    assert!(matches!(err, FramingError::DecodeError(_)));
}

#[tokio::test]
async fn test_async_codec_roundtrip() {
    let (client_io, server_io) = duplex(1024 * 1024);
    let mut writer = FramedWrite::new(client_io, SruiCodec::new());
    let mut reader = FramedRead::new(server_io, SruiCodec::new());

    let msg = SruiMessage {
        msg: Some(srui_message::Msg::ClientHello(ClientHello {
            core_version: "0.4.0".to_string(),
            profiles: vec!["core".to_string(), "widgets.standard".to_string()],
            limits: Some(ClientLimits {
                max_frame_size: 16 * 1024 * 1024,
                max_transaction_operations: 10_000,
                max_tree_depth: 128,
                max_node_count: 50_000,
                max_string_length: 1_000_000,
                max_resource_size: 50 * 1024 * 1024,
            }),
            client_instance_id: vec![1, 2, 3, 4],
            client_metadata: Default::default(),
        })),
    };

    writer.send(msg.clone()).await.expect("send frame");
    let received = reader
        .next()
        .await
        .expect("received frame")
        .expect("decoded ok");
    assert_eq!(msg, received);
}

#[tokio::test]
async fn test_async_codec_max_frame_size_enforced() {
    let (client_io, _server_io) = duplex(1024);
    let mut writer = FramedWrite::new(client_io, SruiCodec::with_max_frame_size(5));

    let msg = SruiMessage {
        msg: Some(srui_message::Msg::Transaction(Transaction {
            base_revision: 1,
            new_revision: 2,
            priority: 1,
            operations: vec![],
        })),
    };

    // Encoding should fail since encoded size > 5 bytes
    let err = writer
        .send(msg)
        .await
        .expect_err("should reject large frame on send");
    assert!(matches!(
        err,
        FramingError::FrameSizeLimitExceeded { limit: 5, .. }
    ));

    // Now test decode rejecting oversized frame
    let (mut client_raw, server_raw) = duplex(1024);
    let mut small_reader = FramedRead::new(server_raw, SruiCodec::with_max_frame_size(5));

    let valid_frame = srui_protocol::encode_framed(&SruiMessage {
        msg: Some(srui_message::Msg::Transaction(Transaction {
            base_revision: 10,
            new_revision: 11,
            priority: 1,
            operations: vec![],
        })),
    })
    .expect("encode normal frame");

    client_raw
        .write_all(&valid_frame)
        .await
        .expect("write frame");
    let decode_err = small_reader
        .next()
        .await
        .expect("read frame")
        .expect_err("should reject on decode");
    assert!(matches!(
        decode_err,
        FramingError::FrameSizeLimitExceeded { limit: 5, .. }
    ));
}

#[tokio::test]
async fn test_async_codec_cancel_safety_in_select() {
    let (mut client_raw, server_raw) = duplex(1024 * 1024);
    let mut reader = FramedRead::new(server_raw, SruiCodec::new());

    let msg = SruiMessage {
        msg: Some(srui_message::Msg::ClientHello(ClientHello {
            core_version: "0.4.0".to_string(),
            profiles: vec!["core".to_string()],
            limits: None,
            client_instance_id: vec![42],
            client_metadata: Default::default(),
        })),
    };

    let encoded_bytes = srui_protocol::encode_framed(&msg).expect("encode frame");
    assert!(encoded_bytes.len() > 6);

    // Split frame into two halves
    let split_point = encoded_bytes.len() / 2;
    let first_half = &encoded_bytes[..split_point];
    let second_half = &encoded_bytes[split_point..];

    // Write only the first half
    client_raw
        .write_all(first_half)
        .await
        .expect("write first half");

    // Race reader against a timeout: reader will be cancelled mid-frame!
    let timeout_result = tokio::select! {
        res = reader.next() => Some(res),
        _ = tokio::time::sleep(tokio::time::Duration::from_millis(50)) => None,
    };
    // The select timed out and dropped the reader.next() future
    assert!(timeout_result.is_none());

    // Write the remaining half of the frame
    client_raw
        .write_all(second_half)
        .await
        .expect("write second half");

    // Next read MUST succeed with the complete message because FramedRead preserved the buffer!
    let received = reader
        .next()
        .await
        .expect("read completed frame")
        .expect("decoded successfully without data loss");
    assert_eq!(msg, received);
}
