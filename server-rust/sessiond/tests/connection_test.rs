use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::io::duplex;
use tokio_util::codec::{FramedRead, FramedWrite};
use tokio_util::sync::CancellationToken;

use srui_protocol::{srui_message, ClientHello, ClientResume, SruiCodec, SruiMessage, Transaction};
use srui_sessiond::{handle_connection, Session};

fn seed_fat_session(session: &Session) -> u64 {
    use srui_sdk::{List, Surface};
    use srui_semantic_tree::{ModelId, ModelItem, Operation, TypeRef, Value};

    let model_id = ModelId::new(10);
    session
        .transaction(|ui| {
            ui.apply_op(&Operation::create_model(model_id, TypeRef::LIST, 10))?;
            ui.apply_op(&Operation::model_reset_range(
                model_id,
                0,
                [
                    ModelItem::with_value(
                        srui_semantic_tree::ItemId::new(1),
                        Value::String("item-1".into()),
                    ),
                    ModelItem::with_value(
                        srui_semantic_tree::ItemId::new(2),
                        Value::String("item-2".into()),
                    ),
                ],
                None,
            ))?;
            Surface::builder(1)
                .label("Initial Window with Long Title to Exceed Duplex Buffer Size")
                .create(ui)?;
            List::builder(2)
                .parent(1)
                .model_ref(model_id)
                .label("List of Items")
                .create(ui)?;
            Ok(())
        })
        .expect("seed session state");
    session.current_revision()
}

#[tokio::test]
async fn test_sessiond_connection_handshake_and_transaction_broadcast() {
    let session = Arc::new(Session::new("test-session"));
    let shutdown = CancellationToken::new();

    let (client_io, server_io) = duplex(1024 * 1024);

    // Spawn server connection handler
    let session_clone = session.clone();
    let shutdown_clone = shutdown.clone();
    let server_task =
        tokio::spawn(
            async move { handle_connection(server_io, session_clone, shutdown_clone).await },
        );

    let (client_read, client_write) = tokio::io::split(client_io);
    let mut client_framed_read = FramedRead::new(client_read, SruiCodec::new());
    let mut client_framed_write = FramedWrite::new(client_write, SruiCodec::new());

    // 1. Send ClientHello
    let hello = SruiMessage {
        msg: Some(srui_message::Msg::ClientHello(ClientHello {
            core_version: "0.4.0".to_string(),
            profiles: vec!["org.srui.standard-widgets/1".to_string()],
            limits: None,
            client_instance_id: vec![1, 2, 3, 4],
            client_metadata: Default::default(),
            known_resource_hashes: vec![],
        })),
    };

    client_framed_write.send(hello).await.expect("send hello");

    // 2. Receive ServerWelcome
    let welcome_msg = client_framed_read
        .next()
        .await
        .expect("receive welcome")
        .expect("decode welcome");

    match welcome_msg.msg {
        Some(srui_message::Msg::ServerWelcome(w)) => {
            assert_eq!(w.session_id, "test-session");
            assert_eq!(w.initial_revision, 0);
        }
        other => panic!("expected ServerWelcome, got {:?}", other),
    }

    // 3. Server commits a transaction -> client should receive it via broadcast
    let tx = Transaction {
        base_revision: 0,
        new_revision: 1,
        priority: 1,
        operations: vec![],
    };
    session.commit_transaction(tx).expect("commit transaction");

    let received_tx = client_framed_read
        .next()
        .await
        .expect("receive broadcast tx")
        .expect("decode broadcast tx");

    match received_tx.msg {
        Some(srui_message::Msg::Transaction(t)) => {
            assert_eq!(t.base_revision, 0);
            assert_eq!(t.new_revision, 1);
        }
        other => panic!("expected Transaction, got {:?}", other),
    }

    // 4. Shutdown connection
    shutdown.cancel();
    let res = server_task.await.expect("server task completed");
    assert!(res.is_ok());
}

#[tokio::test]
async fn test_sessiond_connection_resume_replay() {
    let session = Arc::new(Session::new("test-session-resume"));

    // Advance session revisions in journal (0 -> 1 -> 2)
    session
        .commit_transaction(Transaction {
            base_revision: 0,
            new_revision: 1,
            priority: 1,
            operations: vec![],
        })
        .unwrap();

    session
        .commit_transaction(Transaction {
            base_revision: 1,
            new_revision: 2,
            priority: 1,
            operations: vec![],
        })
        .unwrap();

    let shutdown = CancellationToken::new();
    let (client_io, server_io) = duplex(1024 * 1024);

    let session_clone = session.clone();
    let shutdown_clone = shutdown.clone();
    let server_task =
        tokio::spawn(
            async move { handle_connection(server_io, session_clone, shutdown_clone).await },
        );

    let (client_read, client_write) = tokio::io::split(client_io);
    let mut client_framed_read = FramedRead::new(client_read, SruiCodec::new());
    let mut client_framed_write = FramedWrite::new(client_write, SruiCodec::new());

    // Send ClientResume requesting replay from revision 0
    let resume = SruiMessage {
        msg: Some(srui_message::Msg::ClientResume(ClientResume {
            session_id: "test-session-resume".to_string(),
            client_instance_id: vec![99],
            last_applied_revision: 0,
            last_acked_event_seq: 0,
            terminal_stream_offsets: Default::default(),
            limits: None,
            known_resource_hashes: vec![],
        })),
    };

    client_framed_write.send(resume).await.expect("send resume");

    // Expect ServerResumeOk
    let resume_ok_msg = client_framed_read
        .next()
        .await
        .expect("receive resume ok")
        .expect("decode resume ok");

    match resume_ok_msg.msg {
        Some(srui_message::Msg::ServerResumeOk(ok)) => {
            assert_eq!(ok.replay_from_revision, 0);
        }
        other => panic!("expected ServerResumeOk, got {:?}", other),
    }

    // Expect replayed Transaction 0 -> 1
    let tx1 = client_framed_read.next().await.unwrap().unwrap();
    match tx1.msg {
        Some(srui_message::Msg::Transaction(t)) => {
            assert_eq!(t.base_revision, 0);
            assert_eq!(t.new_revision, 1);
        }
        other => panic!("expected tx 0->1, got {:?}", other),
    }

    // Expect replayed Transaction 1 -> 2
    let tx2 = client_framed_read.next().await.unwrap().unwrap();
    match tx2.msg {
        Some(srui_message::Msg::Transaction(t)) => {
            assert_eq!(t.base_revision, 1);
            assert_eq!(t.new_revision, 2);
        }
        other => panic!("expected tx 1->2, got {:?}", other),
    }

    shutdown.cancel();
    let res = server_task.await.expect("server task completed");
    assert!(res.is_ok());
}

#[tokio::test]
async fn test_connection_fresh_hello_backpressured_snapshot_and_concurrent_commit() {
    let session = Arc::new(Session::new("test-session-backpressure"));
    let initial_revision = seed_fat_session(&session);
    assert!(initial_revision > 0);

    let shutdown = CancellationToken::new();
    // Deliberately tiny duplex buffer (64 bytes) to guarantee snapshot write backpressure
    let (client_io, server_io) = duplex(64);

    let session_clone = session.clone();
    let shutdown_clone = shutdown.clone();
    let server_task =
        tokio::spawn(
            async move { handle_connection(server_io, session_clone, shutdown_clone).await },
        );

    let (client_read, client_write) = tokio::io::split(client_io);
    let mut client_framed_read = FramedRead::new(client_read, SruiCodec::new());
    let mut client_framed_write = FramedWrite::new(client_write, SruiCodec::new());

    // 1. Send ClientHello
    let hello = SruiMessage {
        msg: Some(srui_message::Msg::ClientHello(ClientHello {
            core_version: "0.4.0".to_string(),
            profiles: vec!["org.srui.standard-widgets/1".to_string()],
            limits: None,
            client_instance_id: vec![42],
            client_metadata: Default::default(),
            known_resource_hashes: vec![],
        })),
    };
    client_framed_write.send(hello).await.expect("send hello");

    // 2. Receive ServerWelcome
    let welcome_msg = client_framed_read
        .next()
        .await
        .expect("receive welcome")
        .expect("decode welcome");

    match welcome_msg.msg {
        Some(srui_message::Msg::ServerWelcome(w)) => {
            assert_eq!(w.session_id, "test-session-backpressure");
            assert_eq!(w.initial_revision, initial_revision);
        }
        other => panic!("expected ServerWelcome, got {:?}", other),
    }

    // 3. While snapshot write is backpressured in the 64-byte duplex buffer, commit a new transaction
    let next_tx = Transaction {
        base_revision: initial_revision,
        new_revision: initial_revision + 1,
        priority: 1,
        operations: vec![],
    };
    session
        .commit_transaction(next_tx)
        .expect("commit next transaction");

    // 4. Receive snapshot transaction
    let snapshot_msg = client_framed_read
        .next()
        .await
        .expect("receive snapshot")
        .expect("decode snapshot");

    match snapshot_msg.msg {
        Some(srui_message::Msg::Transaction(s)) => {
            assert_eq!(s.base_revision, 0);
            assert_eq!(s.new_revision, initial_revision);
            assert!(!s.operations.is_empty());
        }
        other => panic!("expected snapshot Transaction, got {:?}", other),
    }

    // 5. Receive subsequent broadcast transaction
    let broadcast_msg = client_framed_read
        .next()
        .await
        .expect("receive broadcast tx")
        .expect("decode broadcast tx");

    match broadcast_msg.msg {
        Some(srui_message::Msg::Transaction(t)) => {
            assert_eq!(t.base_revision, initial_revision);
            assert_eq!(t.new_revision, initial_revision + 1);
        }
        other => panic!("expected Transaction, got {:?}", other),
    }

    shutdown.cancel();
    let res = server_task.await.expect("server task completed");
    assert!(res.is_ok());
}

#[tokio::test]
async fn test_connection_resume_resync_backpressured_snapshot_and_concurrent_commit() {
    let session = Arc::new(Session::new("test-session-resume-backpressure"));
    let initial_revision = seed_fat_session(&session);
    assert!(initial_revision > 0);

    let shutdown = CancellationToken::new();
    let (client_io, server_io) = duplex(64);

    let session_clone = session.clone();
    let shutdown_clone = shutdown.clone();
    let server_task =
        tokio::spawn(
            async move { handle_connection(server_io, session_clone, shutdown_clone).await },
        );

    let (client_read, client_write) = tokio::io::split(client_io);
    let mut client_framed_read = FramedRead::new(client_read, SruiCodec::new());
    let mut client_framed_write = FramedWrite::new(client_write, SruiCodec::new());

    let resume = SruiMessage {
        msg: Some(srui_message::Msg::ClientResume(ClientResume {
            session_id: "replaced-incarnation".to_string(),
            client_instance_id: vec![42],
            last_applied_revision: 0,
            last_acked_event_seq: 0,
            terminal_stream_offsets: Default::default(),
            limits: None,
            known_resource_hashes: vec![],
        })),
    };
    client_framed_write.send(resume).await.expect("send resume");

    let resync_msg = client_framed_read
        .next()
        .await
        .expect("receive resync")
        .expect("decode resync");

    match resync_msg.msg {
        Some(srui_message::Msg::ServerResyncRequired(r)) => {
            assert_eq!(r.session_id, "test-session-resume-backpressure");
            assert_eq!(r.snapshot_revision, initial_revision);
        }
        other => panic!("expected ServerResyncRequired, got {:?}", other),
    }

    let next_tx = Transaction {
        base_revision: initial_revision,
        new_revision: initial_revision + 1,
        priority: 1,
        operations: vec![],
    };
    session
        .commit_transaction(next_tx)
        .expect("commit next transaction");

    let snapshot_msg = client_framed_read
        .next()
        .await
        .expect("receive snapshot")
        .expect("decode snapshot");

    match snapshot_msg.msg {
        Some(srui_message::Msg::Transaction(s)) => {
            assert_eq!(s.base_revision, 0);
            assert_eq!(s.new_revision, initial_revision);
            assert!(!s.operations.is_empty());
        }
        other => panic!("expected snapshot Transaction, got {:?}", other),
    }

    let broadcast_msg = client_framed_read
        .next()
        .await
        .expect("receive broadcast tx")
        .expect("decode broadcast tx");

    match broadcast_msg.msg {
        Some(srui_message::Msg::Transaction(t)) => {
            assert_eq!(t.base_revision, initial_revision);
            assert_eq!(t.new_revision, initial_revision + 1);
        }
        other => panic!("expected Transaction, got {:?}", other),
    }

    shutdown.cancel();
    let res = server_task.await.expect("server task completed");
    assert!(res.is_ok());
}
