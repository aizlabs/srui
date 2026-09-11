use futures::{SinkExt, StreamExt};
use tokio::io::duplex;
use tokio_util::codec::{FramedRead, FramedWrite};
use tokio_util::sync::CancellationToken;

use srui_protocol::{srui_message, ClientHello, SruiCodec, SruiMessage, Transaction};
use srui_ssh_bridge::bridge_streams;

#[tokio::test]
async fn test_ssh_bridge_forwarding() {
    let (ssh_client, ssh_bridge_side) = duplex(1024 * 1024);
    let (session_daemon, session_bridge_side) = duplex(1024 * 1024);

    let shutdown = CancellationToken::new();
    let shutdown_clone = shutdown.clone();

    // Spawn bridge
    let bridge_task = tokio::spawn(async move {
        bridge_streams(ssh_bridge_side, session_bridge_side, shutdown_clone).await
    });

    let (ssh_read, ssh_write) = tokio::io::split(ssh_client);
    let mut ssh_framed_write = FramedWrite::new(ssh_write, SruiCodec::new());
    let mut ssh_framed_read = FramedRead::new(ssh_read, SruiCodec::new());

    let (session_read, session_write) = tokio::io::split(session_daemon);
    let mut session_framed_read = FramedRead::new(session_read, SruiCodec::new());
    let mut session_framed_write = FramedWrite::new(session_write, SruiCodec::new());

    // 1. SSH client sends ClientHello -> sessiond receives it
    let hello = SruiMessage {
        msg: Some(srui_message::Msg::ClientHello(ClientHello {
            core_version: "0.5.0".to_string(),
            profiles: vec!["org.srui.standard-widgets/1".to_string()],
            limits: None,
            client_instance_id: vec![7, 7, 7],
            client_metadata: Default::default(),
            known_resource_hashes: vec![],
        })),
    };
    ssh_framed_write
        .send(hello.clone())
        .await
        .expect("send hello from ssh");

    let received_by_session = session_framed_read
        .next()
        .await
        .expect("session received frame")
        .expect("decoded frame");
    assert_eq!(hello, received_by_session);

    // 2. Sessiond sends Transaction -> SSH client receives it
    let tx = SruiMessage {
        msg: Some(srui_message::Msg::Transaction(Transaction {
            base_revision: 0,
            new_revision: 1,
            priority: 1,
            operations: vec![],
        })),
    };
    session_framed_write
        .send(tx.clone())
        .await
        .expect("send tx from sessiond");

    let received_by_ssh = ssh_framed_read
        .next()
        .await
        .expect("ssh received frame")
        .expect("decoded frame");
    assert_eq!(tx, received_by_ssh);

    // 3. Clean detachment on shutdown
    shutdown.cancel();
    let res = bridge_task.await.expect("bridge task completed");
    assert!(res.is_ok());
}
