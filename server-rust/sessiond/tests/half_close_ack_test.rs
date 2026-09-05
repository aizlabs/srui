//! Half-close acknowledgement delivery regression (§18.2).
//!
//! A client may finish its outbound stream while continuing to read. A settled event
//! acknowledgement queued before that clean EOF must still be written.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use tokio::io::duplex;
use tokio::time::timeout;
use tokio_util::codec::{FramedRead, FramedWrite};
use tokio_util::sync::CancellationToken;

use srui_protocol::{srui_message, ClientHello, EventAckStatus, SruiCodec, SruiMessage};
use srui_sdk::{Button, NodeId, ACTIVATE};
use srui_semantic_tree::Event;
use srui_sessiond::{handle_connection, Session};

const CLIENT_ID: &[u8] = b"half-close-ack-client";
const DEADLOCK: Duration = Duration::from_secs(2);

#[tokio::test]
async fn clean_inbound_eof_flushes_settled_event_ack() {
    let session = Arc::new(Session::new("half-close-ack"));
    let button = NodeId::new(1);
    session
        .transaction(|ui| {
            Button::builder(button).label("Click").create(ui)?;
            Ok(())
        })
        .expect("seed button");

    let invocations = Arc::new(AtomicU64::new(0));
    let invocation_count = Arc::clone(&invocations);
    session.on(button, ACTIVATE, move |_, _| {
        invocation_count.fetch_add(1, Ordering::SeqCst);
    });

    let (client, server) = duplex(1024 * 1024);
    let server_session = Arc::clone(&session);
    let server_task = tokio::spawn(async move {
        handle_connection(server, server_session, CancellationToken::new()).await
    });

    let (client_read, client_write) = tokio::io::split(client);
    let mut read = FramedRead::new(client_read, SruiCodec::new());
    let mut write = FramedWrite::new(client_write, SruiCodec::new());

    write
        .send(SruiMessage {
            msg: Some(srui_message::Msg::ClientHello(ClientHello {
                core_version: "0.5.0".into(),
                profiles: vec!["org.srui.standard-widgets/1".into()],
                limits: None,
                client_instance_id: CLIENT_ID.to_vec(),
                client_metadata: Default::default(),
                known_resource_hashes: vec![],
            })),
        })
        .await
        .expect("send hello");

    let welcome = timeout(DEADLOCK, read.next())
        .await
        .expect("welcome timeout")
        .expect("server closed before welcome")
        .expect("decode welcome");
    assert!(matches!(
        welcome.msg,
        Some(srui_message::Msg::ServerWelcome(_))
    ));

    let snapshot = timeout(DEADLOCK, read.next())
        .await
        .expect("snapshot timeout")
        .expect("server closed before snapshot")
        .expect("decode snapshot");
    assert!(matches!(
        snapshot.msg,
        Some(srui_message::Msg::Transaction(_))
    ));

    let event = Event::activate(1, "evt-half-close", session.current_revision(), button)
        .with_client_instance_id(CLIENT_ID);
    write
        .send(SruiMessage {
            msg: Some(srui_message::Msg::Event(event.to_wire())),
        })
        .await
        .expect("send event");
    write.close().await.expect("half-close client write stream");

    let ack_message = timeout(DEADLOCK, read.next())
        .await
        .expect("event acknowledgement timeout")
        .expect("server closed without the settled event acknowledgement")
        .expect("decode event acknowledgement");
    let ack = match ack_message.msg {
        Some(srui_message::Msg::ServerEventAck(ack)) => ack,
        other => panic!("expected ServerEventAck, got {other:?}"),
    };
    assert_eq!(ack.event_id, b"evt-half-close");
    assert_eq!(ack.status(), EventAckStatus::Processed);
    assert_eq!(ack.last_processed_event_seq, 1);
    assert_eq!(invocations.load(Ordering::SeqCst), 1);

    let server_result = timeout(DEADLOCK, server_task)
        .await
        .expect("server did not finish after inbound EOF")
        .expect("server task join");
    assert!(
        server_result.is_ok(),
        "clean inbound EOF must finish cleanly: {server_result:?}"
    );
}
