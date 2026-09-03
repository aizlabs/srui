//! Resource delivery, corruption, and UI-before-resource scheduling tests (§14, §19.2).

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::{SinkExt, StreamExt};
use tokio::io::duplex;
use tokio::time::{sleep, timeout};
use tokio_util::codec::{FramedRead, FramedWrite};
use tokio_util::sync::CancellationToken;

use srui_protocol::{
    operation::Op, srui_message, value::Value as WireValInner, SruiCodec, SruiMessage, Transaction,
};
use srui_resources::CHUNK_PAYLOAD_SIZE;
use srui_sdk::*;
use srui_sessiond::{handle_connection, Session};

/// Deterministic valid 1×1 RGB PNG (69 bytes); shared with Swift ResourceCacheTests.
fn tiny_png() -> Vec<u8> {
    vec![
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
        0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x60,
        0x60, 0x60, 0x00, 0x00, 0x00, 0x04, 0x00, 0x01, 0xF6, 0x17, 0x38, 0x55, 0x00, 0x00, 0x00,
        0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ]
}

async fn connect_client(
    session: Arc<Session>,
) -> (
    FramedRead<tokio::io::ReadHalf<tokio::io::DuplexStream>, SruiCodec>,
    FramedWrite<tokio::io::WriteHalf<tokio::io::DuplexStream>, SruiCodec>,
    CancellationToken,
    tokio::task::JoinHandle<()>,
) {
    let (client, server) = duplex(1024 * 1024);
    let shutdown = CancellationToken::new();
    let shutdown_server = shutdown.clone();
    let server_task = tokio::spawn(async move {
        let _ = handle_connection(server, session, shutdown_server).await;
    });

    let (read_half, write_half) = tokio::io::split(client);
    let mut framed_read = FramedRead::new(read_half, SruiCodec::new());
    let mut framed_write = FramedWrite::new(write_half, SruiCodec::new());

    let hello = SruiMessage {
        msg: Some(srui_message::Msg::ClientHello(srui_protocol::ClientHello {
            core_version: "0.4.0".into(),
            profiles: vec!["org.srui.standard-widgets/1".into()],
            limits: None,
            client_instance_id: vec![1, 2, 3],
            client_metadata: Default::default(),
        })),
    };
    framed_write.send(hello).await.expect("send hello");
    let welcome = timeout(Duration::from_secs(2), framed_read.next())
        .await
        .expect("welcome timeout")
        .expect("welcome eof")
        .expect("welcome frame");
    assert!(matches!(
        welcome.msg,
        Some(srui_message::Msg::ServerWelcome(_))
    ));

    (framed_read, framed_write, shutdown, server_task)
}

#[tokio::test]
async fn resource_metadata_and_chunks_arrive_after_publish() {
    let session = Arc::new(Session::new("resource-wire"));
    let png = tiny_png();
    let outcome = session.publish_resource(&png).expect("publish");
    assert!(outcome.inserted);

    let (mut read, _write, shutdown, server_task) = connect_client(Arc::clone(&session)).await;

    let mut saw_meta = false;
    let mut reconstructed = Vec::new();
    let mut expected_offset = 0u64;
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let msg = timeout(Duration::from_millis(200), read.next())
            .await
            .ok()
            .flatten()
            .and_then(|r| r.ok());
        let Some(msg) = msg else { break };
        match msg.msg {
            Some(srui_message::Msg::ResourceMetadata(meta)) => {
                assert_eq!(meta.resource_hash, outcome.hash.0.to_vec());
                assert_eq!(meta.encoded_length, png.len() as u64);
                assert_eq!(meta.media_type, "image/png");
                saw_meta = true;
            }
            Some(srui_message::Msg::ResourceChunk(chunk)) => {
                assert_eq!(chunk.resource_hash, outcome.hash.0.to_vec());
                assert_eq!(chunk.byte_offset, expected_offset);
                assert!(chunk.data.len() <= CHUNK_PAYLOAD_SIZE);
                expected_offset += chunk.data.len() as u64;
                reconstructed.extend_from_slice(&chunk.data);
            }
            Some(srui_message::Msg::Transaction(_)) => {}
            other => panic!("unexpected message {other:?}"),
        }
        if saw_meta && reconstructed.len() == png.len() {
            break;
        }
    }

    assert!(saw_meta, "expected resource metadata");
    assert_eq!(reconstructed, png);

    shutdown.cancel();
    let _ = server_task.await;
}

#[tokio::test]
async fn corruption_fixture_preserves_advertised_hash_on_wire() {
    // Server always sends authentic bytes; corruption is a client-side concern.
    // This fixture documents the wire shape a corruption test mutates: flip one
    // chunk byte while keeping the advertised metadata hash unchanged.
    let session = Arc::new(Session::new("resource-corrupt-shape"));
    let mut bytes = vec![0xAAu8; CHUNK_PAYLOAD_SIZE + 50];
    bytes[0..8].copy_from_slice(&[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n']);
    let outcome = session.publish_resource(&bytes).expect("publish");

    let (mut read, _write, shutdown, server_task) = connect_client(Arc::clone(&session)).await;

    let mut advertised_hash = None;
    let mut first_chunk = None;
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let msg = timeout(Duration::from_millis(200), read.next())
            .await
            .ok()
            .flatten()
            .and_then(|r| r.ok());
        let Some(msg) = msg else { break };
        match msg.msg {
            Some(srui_message::Msg::ResourceMetadata(meta)) => {
                advertised_hash = Some(meta.resource_hash);
            }
            Some(srui_message::Msg::ResourceChunk(chunk)) if first_chunk.is_none() => {
                first_chunk = Some(chunk);
            }
            _ => {}
        }
        if advertised_hash.is_some() && first_chunk.is_some() {
            break;
        }
    }

    let hash = advertised_hash.expect("metadata");
    let mut chunk = first_chunk.expect("chunk");
    assert_eq!(hash, outcome.hash.0.to_vec());
    assert_eq!(chunk.resource_hash, hash);
    // Simulate on-the-wire corruption: mutate payload, keep advertised hash.
    chunk.data[0] ^= 0xFF;
    assert_ne!(chunk.data.as_slice(), &bytes[..chunk.data.len()]);
    assert_eq!(chunk.resource_hash, hash);

    shutdown.cancel();
    let _ = server_task.await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ui_transaction_interleaves_ahead_of_remaining_resource_chunks() {
    let session = Arc::new(Session::new("resource-latency"));

    // Multi-chunk payload so a full transfer at 10 ms/frame exceeds one second.
    let chunk_count = 120usize;

    // Small duplex so framed writes backpressure: a newly queued SET_PROPERTY then waits
    // behind at most the resource chunk whose socket write has already begun (§19.2).
    let (client, server) = duplex(512);
    let shutdown = CancellationToken::new();
    let shutdown_server = shutdown.clone();
    let session_server = Arc::clone(&session);
    let server_task = tokio::spawn(async move {
        let _ = handle_connection(server, session_server, shutdown_server).await;
    });

    let (read_half, write_half) = tokio::io::split(client);
    let mut framed_read = FramedRead::new(read_half, SruiCodec::new());
    let mut framed_write = FramedWrite::new(write_half, SruiCodec::new());

    framed_write
        .send(SruiMessage {
            msg: Some(srui_message::Msg::ClientHello(srui_protocol::ClientHello {
                core_version: "0.4.0".into(),
                profiles: vec!["org.srui.standard-widgets/1".into()],
                limits: None,
                client_instance_id: vec![9, 9, 9],
                client_metadata: Default::default(),
            })),
        })
        .await
        .unwrap();
    let _ = framed_read.next().await; // welcome

    // Seed UI before the resource transfer begins.
    session
        .transaction(|ui| {
            Surface::builder(1).create(ui)?;
            Text::builder(2).parent(1).text("before").create(ui)?;
            Ok(())
        })
        .unwrap();
    // Drain the UI snapshot/transaction frames.
    for _ in 0..4 {
        let _ = timeout(Duration::from_millis(200), framed_read.next()).await;
    }

    let mut payload = vec![0u8; CHUNK_PAYLOAD_SIZE * chunk_count];
    payload[0..8].copy_from_slice(&[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n']);
    session.publish_resource(&payload).expect("publish large");

    let mut saw_first_chunk = false;
    let mut resource_frames_after_first_before_tx = 0usize;
    let mut set_prop_at: Option<Instant> = None;
    let mut tx_latency = None;

    loop {
        let msg = timeout(Duration::from_secs(5), framed_read.next())
            .await
            .expect("frame timeout")
            .expect("eof")
            .expect("decode");

        // Throttle client draining by 10 ms per frame.
        sleep(Duration::from_millis(10)).await;

        match msg.msg {
            Some(srui_message::Msg::ResourceChunk(_)) => {
                if !saw_first_chunk {
                    saw_first_chunk = true;
                    set_prop_at = Some(Instant::now());
                    session
                        .transaction(|ui| {
                            ui.set(2, TEXT, "after-first-chunk")?;
                            Ok(())
                        })
                        .unwrap();
                } else if tx_latency.is_none() {
                    resource_frames_after_first_before_tx += 1;
                }
            }
            Some(srui_message::Msg::Transaction(tx)) => {
                let is_text_update = tx.operations.iter().any(|op| {
                    matches!(
                        &op.op,
                        Some(Op::SetProperty(sp)) if {
                            let prop = sp.property.as_ref().map(|p| p.local_id);
                            let val = sp.value.as_ref().and_then(|v| match &v.value {
                                Some(WireValInner::StringValue(s)) => Some(s.as_str()),
                                _ => None,
                            });
                            prop == Some(PropertyRef::TEXT.local_id)
                                && val == Some("after-first-chunk")
                        }
                    )
                });
                if is_text_update {
                    tx_latency = Some(set_prop_at.expect("set prop time").elapsed());
                    break;
                }
            }
            _ => {}
        }
    }

    let latency = tx_latency.expect("SET_PROPERTY transaction must arrive");
    assert!(
        resource_frames_after_first_before_tx <= 1,
        "at most one further resource chunk may precede the transaction, got {resource_frames_after_first_before_tx}"
    );
    assert!(
        latency < Duration::from_millis(250),
        "UI latency under resource load must stay below 250 ms, got {latency:?}"
    );

    let full_transfer_estimate = Duration::from_millis(10 * (1 + chunk_count as u64));
    assert!(
        full_transfer_estimate > Duration::from_secs(1),
        "fixture must be large enough that a full drain exceeds 1s ({full_transfer_estimate:?})"
    );

    shutdown.cancel();
    let _ = server_task.await;
}

#[tokio::test]
async fn republish_does_not_duplicate_transfer_to_attached_client() {
    let session = Arc::new(Session::new("resource-dedupe-wire"));
    let (mut read, _write, shutdown, server_task) = connect_client(Arc::clone(&session)).await;

    let bytes = b"dedupe-bytes";
    let first = session.publish_resource(bytes).unwrap();
    assert!(first.inserted);

    // Wait for metadata.
    let mut saw = false;
    for _ in 0..20 {
        if let Ok(Some(Ok(msg))) = timeout(Duration::from_millis(100), read.next()).await {
            if matches!(msg.msg, Some(srui_message::Msg::ResourceMetadata(_))) {
                saw = true;
                break;
            }
        }
    }
    assert!(saw);

    let second = session.publish_resource(bytes).unwrap();
    assert!(!second.inserted);

    // No additional metadata should arrive promptly.
    let extra = timeout(Duration::from_millis(150), read.next()).await;
    if let Ok(Some(Ok(msg))) = extra {
        assert!(
            !matches!(msg.msg, Some(srui_message::Msg::ResourceMetadata(_))),
            "deduped publish must not re-announce metadata"
        );
    }

    shutdown.cancel();
    let _ = server_task.await;
}

#[tokio::test]
async fn oversized_for_client_ceiling_is_not_transferred() {
    let session = Arc::new(Session::new("resource-client-limit"));
    let png = tiny_png();
    session.publish_resource(&png).expect("publish");

    let (client, server) = duplex(1024 * 1024);
    let shutdown = CancellationToken::new();
    let shutdown_server = shutdown.clone();
    let session_server = Arc::clone(&session);
    let server_task = tokio::spawn(async move {
        let _ = handle_connection(server, session_server, shutdown_server).await;
    });

    let (read_half, write_half) = tokio::io::split(client);
    let mut framed_read = FramedRead::new(read_half, SruiCodec::new());
    let mut framed_write = FramedWrite::new(write_half, SruiCodec::new());

    // Advertise a ceiling below the published PNG so the server must skip transfer (§15, §26).
    let hello = SruiMessage {
        msg: Some(srui_message::Msg::ClientHello(srui_protocol::ClientHello {
            core_version: "0.4.0".into(),
            profiles: vec!["org.srui.standard-widgets/1".into()],
            limits: Some(srui_protocol::ClientLimits {
                max_frame_size: 0,
                max_transaction_operations: 0,
                max_tree_depth: 0,
                max_node_count: 0,
                max_string_length: 0,
                max_resource_size: 8,
            }),
            client_instance_id: vec![9, 9, 9],
            client_metadata: Default::default(),
        })),
    };
    framed_write.send(hello).await.expect("send hello");
    let welcome = timeout(Duration::from_secs(2), framed_read.next())
        .await
        .expect("welcome timeout")
        .expect("welcome eof")
        .expect("welcome frame");
    match welcome.msg {
        Some(srui_message::Msg::ServerWelcome(w)) => {
            let limits = w.limits.expect("welcome limits");
            assert_eq!(limits.max_resource_size, 8);
        }
        other => panic!("expected welcome, got {other:?}"),
    }

    let mut saw_resource = false;
    for _ in 0..20 {
        if let Ok(Some(Ok(msg))) = timeout(Duration::from_millis(50), framed_read.next()).await {
            match msg.msg {
                Some(srui_message::Msg::ResourceMetadata(_))
                | Some(srui_message::Msg::ResourceChunk(_)) => {
                    saw_resource = true;
                    break;
                }
                _ => {}
            }
        }
    }
    assert!(
        !saw_resource,
        "resources above the negotiated client ceiling must not be transferred"
    );

    shutdown.cancel();
    let _ = server_task.await;
}

// Silence unused import warning if Transaction not used in all cfgs.
#[allow(dead_code)]
fn _keep_transaction_ty(_: &Transaction) {}
