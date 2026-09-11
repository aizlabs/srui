use crate::report::{push_timing_distributions, Assertion, Section};
use bytes::BytesMut;
use futures::{SinkExt, StreamExt};
use srui_protocol::{
    srui_message, ClientHello, ClientResume, EventAckStatus, SessionContinuity, SruiCodec,
    SruiMessage,
};
use srui_resources::CHUNK_PAYLOAD_SIZE;
use srui_sdk::{Button, Surface, ACTIVATE, LABEL};
use srui_semantic_tree::{Event as DomainEvent, NodeId, Revision, SemanticStore};
use srui_sessiond::{
    handle_connection, ConnectionError, EventOutcome, LogicalChannelClass, OutboundItem,
    OutboundReceiver, ResumeOutcome, Session, SessionConfig, CORE_VERSION,
};
use std::collections::BTreeMap;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::time::timeout;
use tokio_util::codec::{Decoder, Encoder, FramedRead, FramedWrite};
use tokio_util::sync::CancellationToken;

async fn next_resource(receiver: &mut OutboundReceiver) -> Result<OutboundItem, String> {
    timeout(
        Duration::from_secs(2),
        receiver.recv_class(LogicalChannelClass::Resource),
    )
    .await
    .map_err(|_| "timed out waiting for a production resource frame".to_string())?
    .map_err(|error| error.to_string())
}

fn empty_wire_transaction(base_revision: u64) -> srui_protocol::Transaction {
    srui_protocol::Transaction {
        base_revision,
        new_revision: base_revision + 1,
        priority: 0,
        operations: Vec::new(),
    }
}

fn resume_request(session: &Session, client: &[u8], revision: u64) -> ClientResume {
    ClientResume {
        session_id: session.session_id(),
        client_instance_id: client.to_vec(),
        last_applied_revision: revision,
        ..ClientResume::default()
    }
}

fn partial_transaction_frame_is_buffered(
    transaction: &srui_protocol::Transaction,
) -> Result<bool, String> {
    let mut codec = SruiCodec::new();
    let mut complete = BytesMut::new();
    codec
        .encode(
            SruiMessage {
                msg: Some(srui_message::Msg::Transaction(transaction.clone())),
            },
            &mut complete,
        )
        .map_err(|error| error.to_string())?;
    let split = complete.len() / 2;
    let mut partial = BytesMut::from(&complete[..split]);
    let decoded = codec
        .decode(&mut partial)
        .map_err(|error| error.to_string())?;
    Ok(decoded.is_none() && partial.len() == split)
}

type WireClientRead = FramedRead<tokio::io::ReadHalf<DuplexStream>, SruiCodec>;
type WireClientWrite = FramedWrite<tokio::io::WriteHalf<DuplexStream>, SruiCodec>;
type WireServerTask = tokio::task::JoinHandle<Result<(), ConnectionError>>;

fn open_wire_connection(
    session: Arc<Session>,
    shutdown: CancellationToken,
) -> (WireClientWrite, WireClientRead, WireServerTask) {
    open_wire_connection_with_capacity(session, shutdown, 1024 * 1024)
}

fn open_wire_connection_with_capacity(
    session: Arc<Session>,
    shutdown: CancellationToken,
    capacity: usize,
) -> (WireClientWrite, WireClientRead, WireServerTask) {
    let (client_io, server_io) = duplex(capacity);
    let server_task =
        tokio::spawn(async move { handle_connection(server_io, session, shutdown).await });
    let (client_read, client_write) = tokio::io::split(client_io);
    (
        FramedWrite::new(client_write, SruiCodec::new()),
        FramedRead::new(client_read, SruiCodec::new()),
        server_task,
    )
}

async fn read_wire_message(read: &mut WireClientRead) -> Result<SruiMessage, String> {
    timeout(Duration::from_secs(2), read.next())
        .await
        .map_err(|_| "timed out waiting for a wire benchmark frame".to_string())?
        .ok_or_else(|| "wire benchmark connection closed before the expected frame".to_string())?
        .map_err(|error| error.to_string())
}

fn activate_message(
    client_instance_id: &[u8],
    event_id: &str,
    observed_revision: u64,
    node_id: NodeId,
) -> SruiMessage {
    SruiMessage {
        msg: Some(srui_message::Msg::Event(
            DomainEvent::activate(1, event_id, observed_revision, node_id)
                .with_client_instance_id(client_instance_id.to_vec())
                .to_wire(),
        )),
    }
}

async fn wire_pre_receipt_disconnect(sample: usize) -> Result<(f64, bool), String> {
    let session = Arc::new(Session::new(format!("benchmark-wire-pre-receipt-{sample}")));
    let button = NodeId::new(2);
    session
        .transaction(|ui| {
            Surface::builder(NodeId::new(1)).create(ui)?;
            Button::builder(button)
                .parent(NodeId::new(1))
                .label("Run")
                .create(ui)?;
            Ok(())
        })
        .map_err(|error| error.to_string())?;

    let side_effects = Arc::new(AtomicUsize::new(0));
    let handler_side_effects = Arc::clone(&side_effects);
    session.on(button, ACTIVATE, move |context, _| {
        handler_side_effects.fetch_add(1, Ordering::SeqCst);
        context
            .transaction(|ui| {
                ui.set(button, LABEL, "Clicked")?;
                Ok(())
            })
            .expect("pre-receipt benchmark handler transaction");
    });

    let client_instance_id = format!("wire-pre-receipt-client-{sample}").into_bytes();
    let event_id = "benchmark-pre-receipt";
    let shutdown = CancellationToken::new();
    let (mut first_write, mut first_read, first_task) =
        open_wire_connection(Arc::clone(&session), shutdown.clone());
    first_write
        .send(SruiMessage {
            msg: Some(srui_message::Msg::ClientHello(ClientHello {
                core_version: CORE_VERSION.to_string(),
                profiles: vec!["org.srui.standard-widgets/1".to_string()],
                client_instance_id: client_instance_id.clone(),
                ..ClientHello::default()
            })),
        })
        .await
        .map_err(|error| error.to_string())?;
    let welcome = read_wire_message(&mut first_read).await?;
    let snapshot = read_wire_message(&mut first_read).await?;
    let fresh_correct = matches!(
        welcome.msg,
        Some(srui_message::Msg::ServerWelcome(value))
            if value.session_id == session.session_id() && value.initial_revision == 1
    ) && matches!(
        snapshot.msg,
        Some(srui_message::Msg::Transaction(transaction))
            if transaction.base_revision == 0 && transaction.new_revision == 1
    );

    // Start immediately before the complete EVENT could be received. The first transport is
    // closed without sending any event bytes; the replacement must resume, admit one complete
    // event, and expose its resulting transaction before this measurement stops.
    let start = Instant::now();
    drop(first_write);
    drop(first_read);
    join_interrupted_connection(first_task, "pre-receipt EVENT").await?;
    let first_connection_inert = session.is_detached()
        && side_effects.load(Ordering::SeqCst) == 0
        && session.current_revision() == 1
        && session.with_store(|store| {
            Button::from_store(store, button).and_then(|value| value.label(store)) == Some("Run")
        });

    let (mut resumed_write, mut resumed_read, resumed_task) =
        open_wire_connection(Arc::clone(&session), shutdown.clone());
    resumed_write
        .send(SruiMessage {
            msg: Some(srui_message::Msg::ClientResume(ClientResume {
                session_id: session.session_id(),
                client_instance_id: client_instance_id.clone(),
                last_applied_revision: 1,
                last_acked_event_seq: 0,
                ..ClientResume::default()
            })),
        })
        .await
        .map_err(|error| error.to_string())?;
    let resume = read_wire_message(&mut resumed_read).await?;
    let resume_correct = matches!(
        resume.msg,
        Some(srui_message::Msg::ServerResumeOk(value))
            if value.session_id == session.session_id()
                && value.replay_from_revision == 1
                && value.last_processed_event_seq == 0
    );

    resumed_write
        .send(activate_message(&client_instance_id, event_id, 1, button))
        .await
        .map_err(|error| error.to_string())?;
    let processed_ack = read_wire_message(&mut resumed_read).await?;
    let handler_transaction = read_wire_message(&mut resumed_read).await?;
    let processed_once = matches!(
        processed_ack.msg,
        Some(srui_message::Msg::ServerEventAck(ack))
            if ack.status() == EventAckStatus::Processed
                && ack.client_instance_id == client_instance_id
                && ack.event_id == event_id.as_bytes()
                && ack.revision_after_effect == 2
                && ack.last_processed_event_seq == 1
                && ack.settled_event_seq == 1
    ) && matches!(
        handler_transaction.msg,
        Some(srui_message::Msg::Transaction(transaction))
            if transaction.base_revision == 1 && transaction.new_revision == 2
    ) && side_effects.load(Ordering::SeqCst) == 1
        && session.current_revision() == 2
        && session.with_store(|store| {
            Button::from_store(store, button).and_then(|value| value.label(store))
                == Some("Clicked")
        });
    let no_second_effect = timeout(Duration::from_millis(5), resumed_read.next())
        .await
        .is_err();
    let elapsed = start.elapsed().as_secs_f64() * 1_000.0;

    shutdown.cancel();
    let resumed_finished = timeout(Duration::from_secs(2), resumed_task)
        .await
        .map_err(|_| "resumed pre-receipt connection did not terminate".to_string())?
        .map_err(|error| error.to_string())?
        .is_ok();

    Ok((
        elapsed,
        fresh_correct
            && first_connection_inert
            && resume_correct
            && processed_once
            && no_second_effect
            && resumed_finished,
    ))
}

async fn wire_lost_ack_reconnect(sample: usize) -> Result<(f64, bool), String> {
    let session = Arc::new(Session::new(format!("benchmark-wire-event-{sample}")));
    let button = NodeId::new(2);
    session
        .transaction(|ui| {
            Surface::builder(NodeId::new(1)).create(ui)?;
            Button::builder(button)
                .parent(NodeId::new(1))
                .label("Run")
                .create(ui)?;
            Ok(())
        })
        .map_err(|error| error.to_string())?;

    let side_effects = Arc::new(AtomicUsize::new(0));
    let handler_side_effects = Arc::clone(&side_effects);
    session.on(button, ACTIVATE, move |context, _| {
        handler_side_effects.fetch_add(1, Ordering::SeqCst);
        context
            .transaction(|ui| {
                ui.set(button, LABEL, "Clicked")?;
                Ok(())
            })
            .expect("benchmark event handler transaction");
    });

    let client_instance_id = format!("wire-event-client-{sample}").into_bytes();
    let event_id = "benchmark-lost-ack";
    let shutdown = CancellationToken::new();
    let (mut first_write, mut first_read, first_task) =
        open_wire_connection(Arc::clone(&session), shutdown.clone());
    first_write
        .send(SruiMessage {
            msg: Some(srui_message::Msg::ClientHello(ClientHello {
                core_version: CORE_VERSION.to_string(),
                profiles: vec!["org.srui.standard-widgets/1".to_string()],
                client_instance_id: client_instance_id.clone(),
                ..ClientHello::default()
            })),
        })
        .await
        .map_err(|error| error.to_string())?;
    let fresh_welcome = read_wire_message(&mut first_read).await?;
    let fresh_snapshot = read_wire_message(&mut first_read).await?;
    let fresh_handshake_correct = matches!(
        fresh_welcome.msg,
        Some(srui_message::Msg::ServerWelcome(welcome))
            if welcome.session_id == session.session_id() && welcome.initial_revision == 1
    ) && matches!(
        fresh_snapshot.msg,
        Some(srui_message::Msg::Transaction(transaction))
            if transaction.base_revision == 0 && transaction.new_revision == 1
    );

    first_write
        .send(activate_message(&client_instance_id, event_id, 1, button))
        .await
        .map_err(|error| error.to_string())?;
    timeout(Duration::from_secs(2), async {
        while side_effects.load(Ordering::SeqCst) != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(|_| "timed out waiting for the first wire event side effect".to_string())?;

    // Drop both halves after handler completion without consuming SERVER EVENT_ACK.
    let start = Instant::now();
    drop(first_write);
    drop(first_read);
    let _first_connection_result = timeout(Duration::from_secs(2), first_task)
        .await
        .map_err(|_| "first wire connection did not terminate".to_string())?
        .map_err(|error| error.to_string())?;

    let (mut resumed_write, mut resumed_read, resumed_task) =
        open_wire_connection(Arc::clone(&session), shutdown.clone());
    resumed_write
        .send(SruiMessage {
            msg: Some(srui_message::Msg::ClientResume(ClientResume {
                session_id: session.session_id(),
                client_instance_id: client_instance_id.clone(),
                last_applied_revision: 1,
                last_acked_event_seq: 0,
                ..ClientResume::default()
            })),
        })
        .await
        .map_err(|error| error.to_string())?;
    let resume = read_wire_message(&mut resumed_read).await?;
    let replayed_transaction = read_wire_message(&mut resumed_read).await?;
    let resume_correct = matches!(
        resume.msg,
        Some(srui_message::Msg::ServerResumeOk(resume_ok))
            if resume_ok.session_id == session.session_id()
                && resume_ok.replay_from_revision == 1
                && resume_ok.last_processed_event_seq == 1
    ) && matches!(
        replayed_transaction.msg,
        Some(srui_message::Msg::Transaction(transaction))
            if transaction.base_revision == 1 && transaction.new_revision == 2
    );

    resumed_write
        .send(activate_message(&client_instance_id, event_id, 1, button))
        .await
        .map_err(|error| error.to_string())?;
    let duplicate_ack = read_wire_message(&mut resumed_read).await?;
    let elapsed = start.elapsed().as_secs_f64() * 1_000.0;
    let duplicate_correct = matches!(
        duplicate_ack.msg,
        Some(srui_message::Msg::ServerEventAck(ack))
            if ack.status() == EventAckStatus::Duplicate
                && ack.client_instance_id == client_instance_id
                && ack.event_id == event_id.as_bytes()
                && ack.revision_after_effect == 2
                && ack.last_processed_event_seq == 1
                && ack.settled_event_seq == 1
    );
    let no_duplicate_transaction = timeout(Duration::from_millis(5), resumed_read.next())
        .await
        .is_err();
    let state_correct = side_effects.load(Ordering::SeqCst) == 1
        && session.current_revision() == 2
        && session.with_store(|store| {
            Button::from_store(store, button).and_then(|value| value.label(store))
                == Some("Clicked")
        });

    shutdown.cancel();
    let resumed_finished = timeout(Duration::from_secs(2), resumed_task)
        .await
        .map_err(|_| "resumed wire connection did not terminate".to_string())?
        .map_err(|error| error.to_string())?
        .is_ok();

    Ok((
        elapsed,
        fresh_handshake_correct
            && resume_correct
            && duplicate_correct
            && no_duplicate_transaction
            && state_correct
            && resumed_finished,
    ))
}

async fn join_interrupted_connection(
    task: WireServerTask,
    boundary: &'static str,
) -> Result<(), String> {
    let _connection_result = timeout(Duration::from_secs(2), task)
        .await
        .map_err(|_| format!("{boundary} connection did not terminate"))?
        .map_err(|error| error.to_string())?;
    Ok(())
}

async fn wire_partial_transaction_reconnect(sample: usize) -> Result<(f64, bool), String> {
    let session = Arc::new(Session::new(format!("benchmark-wire-transaction-{sample}")));
    let client_instance_id = format!("wire-transaction-client-{sample}").into_bytes();
    let shutdown = CancellationToken::new();
    // Capacity one forces the server's frame write to remain in progress after the first byte.
    let (mut first_write, mut first_read, first_task) =
        open_wire_connection_with_capacity(Arc::clone(&session), shutdown.clone(), 1);
    first_write
        .send(SruiMessage {
            msg: Some(srui_message::Msg::ClientHello(ClientHello {
                core_version: CORE_VERSION.to_string(),
                profiles: vec!["org.srui.standard-widgets/1".to_string()],
                client_instance_id: client_instance_id.clone(),
                ..ClientHello::default()
            })),
        })
        .await
        .map_err(|error| error.to_string())?;
    let welcome = read_wire_message(&mut first_read).await?;
    let fresh_correct = matches!(
        welcome.msg,
        Some(srui_message::Msg::ServerWelcome(value))
            if value.session_id == session.session_id() && value.initial_revision == 0
    );

    session
        .transaction(|ui| {
            Surface::builder(NodeId::new(1)).create(ui)?;
            Ok(())
        })
        .map_err(|error| error.to_string())?;

    let start = Instant::now();
    let mut raw_read = first_read.into_inner();
    let mut first_frame_byte = [0_u8; 1];
    timeout(
        Duration::from_secs(2),
        raw_read.read_exact(&mut first_frame_byte),
    )
    .await
    .map_err(|_| "timed out reading the first transaction frame byte".to_string())?
    .map_err(|error| error.to_string())?;
    let mut partial_buffer = BytesMut::from(first_frame_byte.as_slice());
    let no_partial_decode = SruiCodec::new()
        .decode(&mut partial_buffer)
        .map_err(|error| error.to_string())?
        .is_none();
    let mut replica = SemanticStore::new();
    let replica_untouched = replica.revision() == Revision::INITIAL && replica.node_count() == 0;

    drop(first_write);
    drop(raw_read);
    join_interrupted_connection(first_task, "partial transaction").await?;
    let detached_after_partial = session.is_detached();

    let (mut resumed_write, mut resumed_read, resumed_task) =
        open_wire_connection(Arc::clone(&session), shutdown.clone());
    resumed_write
        .send(SruiMessage {
            msg: Some(srui_message::Msg::ClientResume(ClientResume {
                session_id: session.session_id(),
                client_instance_id: client_instance_id.clone(),
                last_applied_revision: 0,
                last_acked_event_seq: 0,
                ..ClientResume::default()
            })),
        })
        .await
        .map_err(|error| error.to_string())?;
    let resume = read_wire_message(&mut resumed_read).await?;
    let replay = read_wire_message(&mut resumed_read).await?;
    let resume_correct = matches!(
        resume.msg,
        Some(srui_message::Msg::ServerResumeOk(value))
            if value.session_id == session.session_id()
                && value.replay_from_revision == 0
                && value.last_processed_event_seq == 0
    );
    let replayed_transaction = match replay.msg {
        Some(srui_message::Msg::Transaction(transaction)) => Some(transaction),
        _ => None,
    };
    let applied_once = replayed_transaction.as_ref().is_some_and(|transaction| {
        transaction.base_revision == 0
            && transaction.new_revision == 1
            && replica.apply_wire_transaction(transaction.clone()).is_ok()
    });
    let no_second_transaction = timeout(Duration::from_millis(5), resumed_read.next())
        .await
        .is_err();
    let replica_correct = replica.revision() == Revision::new(1)
        && replica.node_count() == 1
        && replica.contains_node(NodeId::new(1));
    let elapsed = start.elapsed().as_secs_f64() * 1_000.0;

    shutdown.cancel();
    let resumed_finished = timeout(Duration::from_secs(2), resumed_task)
        .await
        .map_err(|_| "resumed transaction connection did not terminate".to_string())?
        .map_err(|error| error.to_string())?
        .is_ok();

    Ok((
        elapsed,
        fresh_correct
            && no_partial_decode
            && replica_untouched
            && detached_after_partial
            && resume_correct
            && applied_once
            && no_second_transaction
            && replica_correct
            && resumed_finished,
    ))
}

async fn wire_partial_event_reconnect(sample: usize) -> Result<(f64, bool), String> {
    let session = Arc::new(Session::new(format!(
        "benchmark-wire-partial-event-{sample}"
    )));
    let button = NodeId::new(2);
    session
        .transaction(|ui| {
            Surface::builder(NodeId::new(1)).create(ui)?;
            Button::builder(button)
                .parent(NodeId::new(1))
                .label("Run")
                .create(ui)?;
            Ok(())
        })
        .map_err(|error| error.to_string())?;

    let side_effects = Arc::new(AtomicUsize::new(0));
    let handler_side_effects = Arc::clone(&side_effects);
    session.on(button, ACTIVATE, move |context, _| {
        handler_side_effects.fetch_add(1, Ordering::SeqCst);
        context
            .transaction(|ui| {
                ui.set(button, LABEL, "Clicked")?;
                Ok(())
            })
            .expect("partial event benchmark handler transaction");
    });

    let client_instance_id = format!("wire-partial-event-client-{sample}").into_bytes();
    let event_id = "benchmark-partial-event";
    let shutdown = CancellationToken::new();
    let (mut first_write, mut first_read, first_task) =
        open_wire_connection_with_capacity(Arc::clone(&session), shutdown.clone(), 1);
    first_write
        .send(SruiMessage {
            msg: Some(srui_message::Msg::ClientHello(ClientHello {
                core_version: CORE_VERSION.to_string(),
                profiles: vec!["org.srui.standard-widgets/1".to_string()],
                client_instance_id: client_instance_id.clone(),
                ..ClientHello::default()
            })),
        })
        .await
        .map_err(|error| error.to_string())?;
    let welcome = read_wire_message(&mut first_read).await?;
    let snapshot = read_wire_message(&mut first_read).await?;
    let fresh_correct = matches!(
        welcome.msg,
        Some(srui_message::Msg::ServerWelcome(value))
            if value.session_id == session.session_id() && value.initial_revision == 1
    ) && matches!(
        snapshot.msg,
        Some(srui_message::Msg::Transaction(transaction))
            if transaction.base_revision == 0 && transaction.new_revision == 1
    );

    let full_event = activate_message(&client_instance_id, event_id, 1, button);
    let mut encoded_event = BytesMut::new();
    SruiCodec::new()
        .encode(full_event.clone(), &mut encoded_event)
        .map_err(|error| error.to_string())?;
    let split = encoded_event.len() / 2;
    let start = Instant::now();
    let mut raw_write = first_write.into_inner();
    timeout(
        Duration::from_secs(2),
        raw_write.write_all(&encoded_event[..split]),
    )
    .await
    .map_err(|_| "timed out sending the partial EVENT frame".to_string())?
    .map_err(|error| error.to_string())?;
    drop(raw_write);
    drop(first_read);
    join_interrupted_connection(first_task, "partial EVENT").await?;
    let partial_was_inert = session.is_detached()
        && side_effects.load(Ordering::SeqCst) == 0
        && session.current_revision() == 1;

    let (mut resumed_write, mut resumed_read, resumed_task) =
        open_wire_connection(Arc::clone(&session), shutdown.clone());
    resumed_write
        .send(SruiMessage {
            msg: Some(srui_message::Msg::ClientResume(ClientResume {
                session_id: session.session_id(),
                client_instance_id: client_instance_id.clone(),
                last_applied_revision: 1,
                last_acked_event_seq: 0,
                ..ClientResume::default()
            })),
        })
        .await
        .map_err(|error| error.to_string())?;
    let resume = read_wire_message(&mut resumed_read).await?;
    let resume_correct = matches!(
        resume.msg,
        Some(srui_message::Msg::ServerResumeOk(value))
            if value.session_id == session.session_id()
                && value.replay_from_revision == 1
                && value.last_processed_event_seq == 0
    );

    resumed_write
        .send(full_event)
        .await
        .map_err(|error| error.to_string())?;
    let processed_ack = read_wire_message(&mut resumed_read).await?;
    let handler_transaction = read_wire_message(&mut resumed_read).await?;
    let processed_once = matches!(
        processed_ack.msg,
        Some(srui_message::Msg::ServerEventAck(ack))
            if ack.status() == EventAckStatus::Processed
                && ack.client_instance_id == client_instance_id
                && ack.event_id == event_id.as_bytes()
                && ack.revision_after_effect == 2
                && ack.last_processed_event_seq == 1
                && ack.settled_event_seq == 1
    ) && matches!(
        handler_transaction.msg,
        Some(srui_message::Msg::Transaction(transaction))
            if transaction.base_revision == 1 && transaction.new_revision == 2
    ) && side_effects.load(Ordering::SeqCst) == 1
        && session.current_revision() == 2
        && session.with_store(|store| {
            Button::from_store(store, button).and_then(|value| value.label(store))
                == Some("Clicked")
        });
    let no_second_effect = timeout(Duration::from_millis(5), resumed_read.next())
        .await
        .is_err();
    let elapsed = start.elapsed().as_secs_f64() * 1_000.0;

    shutdown.cancel();
    let resumed_finished = timeout(Duration::from_secs(2), resumed_task)
        .await
        .map_err(|_| "resumed event connection did not terminate".to_string())?
        .map_err(|error| error.to_string())?
        .is_ok();

    Ok((
        elapsed,
        fresh_correct
            && partial_was_inert
            && resume_correct
            && processed_once
            && no_second_effect
            && resumed_finished,
    ))
}

pub(crate) async fn reconnect(iterations: usize) -> Result<Section, String> {
    let mut timings: BTreeMap<&'static str, Vec<f64>> = BTreeMap::new();
    let mut resource_replay_correct = true;
    let mut transaction_replay_correct = true;
    let mut wire_transaction_replay_correct = true;
    let mut event_boundary_correct = true;
    let mut wire_pre_receipt_correct = true;
    let mut wire_partial_event_correct = true;
    let mut duplicate_correct = true;
    let mut wire_duplicate_correct = true;
    let mut retention_correct = true;
    let resource_payload: Vec<u8> = (0..(CHUNK_PAYLOAD_SIZE * 2 + 37))
        .map(|index| (index % 251) as u8)
        .collect();

    for sample in 0..iterations {
        // Interrupt an actual production resource lane after its first chunk. A replacement
        // subscription is seeded from the session CAS and must restart at metadata/offset zero.
        let resource_session = Session::new(format!("benchmark-resource-{sample}"));
        let published = resource_session
            .publish_resource(&resource_payload)
            .map_err(|error| error.to_string())?;
        let client = format!("resource-client-{sample}").into_bytes();
        let mut interrupted = resource_session
            .subscribe_transactions(client.clone())
            .map_err(|error| error.to_string())?;
        let metadata = next_resource(&mut interrupted).await?;
        let first_chunk = next_resource(&mut interrupted).await?;
        let interrupted_at_real_boundary = matches!(
            &metadata,
            OutboundItem::ResourceMetadata(value)
                if value.resource_hash == published.hash.0.to_vec()
                    && value.encoded_length == resource_payload.len() as u64
        ) && matches!(
            &first_chunk,
            OutboundItem::ResourceChunk(value)
                if value.resource_hash == published.hash.0.to_vec()
                    && value.byte_offset == 0
                    && value.data.len() == CHUNK_PAYLOAD_SIZE
        );
        drop(interrupted);

        let start = Instant::now();
        let mut resumed = resource_session
            .subscribe_transactions(client)
            .map_err(|error| error.to_string())?;
        let mut resumed_metadata = false;
        let mut resumed_bytes = Vec::with_capacity(resource_payload.len());
        let mut next_offset = 0_u64;
        while resumed_bytes.len() < resource_payload.len() {
            match next_resource(&mut resumed).await? {
                OutboundItem::ResourceMetadata(value) => {
                    resumed_metadata = value.resource_hash == published.hash.0.to_vec()
                        && value.encoded_length == resource_payload.len() as u64;
                }
                OutboundItem::ResourceChunk(value) => {
                    if value.resource_hash != published.hash.0.to_vec()
                        || value.byte_offset != next_offset
                    {
                        resource_replay_correct = false;
                    }
                    next_offset = next_offset.saturating_add(value.data.len() as u64);
                    resumed_bytes.extend_from_slice(&value.data);
                }
                OutboundItem::Transaction(_) => {
                    resource_replay_correct = false;
                }
            }
        }
        timings
            .entry("mid-resource reconnect and exact replay")
            .or_default()
            .push(start.elapsed().as_secs_f64() * 1_000.0);
        resource_replay_correct &= interrupted_at_real_boundary
            && resumed_metadata
            && resumed_bytes == resource_payload
            && next_offset == resource_payload.len() as u64;

        // Feed half of a real length-delimited TRANSACTION through SruiCodec, detach before it
        // completes, then resume through Session::bootstrap_resume and recover the atomic commit.
        let transaction_session = Session::new(format!("benchmark-frame-{sample}"));
        let committed = transaction_session
            .commit_transaction(empty_wire_transaction(0))
            .map_err(|error| error.to_string())?;
        let attachment = transaction_session
            .attach()
            .ok_or_else(|| "transaction session refused attachment".to_string())?;
        let start = Instant::now();
        let partial_was_buffered = partial_transaction_frame_is_buffered(&committed)?;
        drop(attachment);
        let resumed_transaction = transaction_session
            .bootstrap_resume(&resume_request(
                &transaction_session,
                format!("frame-client-{sample}").as_bytes(),
                0,
            ))
            .map_err(|error| error.to_string())?;
        timings
            .entry("mid-transaction frame discard and atomic replay")
            .or_default()
            .push(start.elapsed().as_secs_f64() * 1_000.0);
        transaction_replay_correct &= partial_was_buffered
            && transaction_session.is_detached()
            && matches!(
                resumed_transaction.outcome,
                ResumeOutcome::Replay { replayed, .. }
                    if replayed == vec![committed]
            );

        // Keep the direct admission/result-cache timings as component measurements. The
        // pre-receipt reconnect itself is measured separately below over handle_connection.
        let event_session = Session::new(format!("benchmark-event-{sample}"));
        event_session
            .transaction(|ui| {
                Surface::builder(NodeId::new(1)).create(ui)?;
                Button::builder(NodeId::new(2))
                    .parent(NodeId::new(1))
                    .label("Run")
                    .create(ui)?;
                Ok(())
            })
            .map_err(|error| error.to_string())?;
        let side_effects = Arc::new(AtomicUsize::new(0));
        let handler_side_effects = Arc::clone(&side_effects);
        event_session.on(NodeId::new(2), ACTIVATE, move |_, _| {
            handler_side_effects.fetch_add(1, Ordering::SeqCst);
        });
        let event = DomainEvent::activate(
            1,
            "benchmark-event",
            event_session.current_revision(),
            NodeId::new(2),
        )
        .with_client_instance_id(format!("event-client-{sample}").into_bytes())
        .to_wire();

        let pre_receipt_attachment = event_session
            .attach()
            .ok_or_else(|| "event session refused attachment".to_string())?;
        drop(pre_receipt_attachment);
        event_boundary_correct &=
            event_session.is_detached() && side_effects.load(Ordering::SeqCst) == 0;

        let start = Instant::now();
        let first = event_session
            .process_event(&event)
            .map_err(|error| error.to_string())?;
        timings
            .entry("event receipt through settled side effect")
            .or_default()
            .push(start.elapsed().as_secs_f64() * 1_000.0);
        event_boundary_correct &= matches!(first, EventOutcome::Processed { .. })
            && side_effects.load(Ordering::SeqCst) == 1;

        let start = Instant::now();
        let replay = event_session
            .process_event(&event)
            .map_err(|error| error.to_string())?;
        timings
            .entry("in-process cached DUPLICATE response")
            .or_default()
            .push(start.elapsed().as_secs_f64() * 1_000.0);
        duplicate_correct &= matches!(
            replay,
            EventOutcome::Duplicate {
                accepted: true,
                revision_after_effect: 1,
                last_processed_event_seq: 1,
                ..
            }
        ) && side_effects.load(Ordering::SeqCst) == 1;

        // Exercise both resume decisions through Session::bootstrap_resume, not the journal type.
        let retention_session = Session::with_config(
            format!("benchmark-retention-{sample}"),
            SessionConfig {
                journal_capacity: 4,
                ..SessionConfig::default()
            },
        );
        for base in 0..8 {
            retention_session
                .commit_transaction(empty_wire_transaction(base))
                .map_err(|error| error.to_string())?;
        }

        let start = Instant::now();
        let within = retention_session
            .bootstrap_resume(&resume_request(
                &retention_session,
                format!("retained-client-{sample}").as_bytes(),
                6,
            ))
            .map_err(|error| error.to_string())?;
        timings
            .entry("resume within journal retention")
            .or_default()
            .push(start.elapsed().as_secs_f64() * 1_000.0);
        retention_correct &= matches!(
            within.outcome,
            ResumeOutcome::Replay { replayed, .. }
                if replayed.len() == 2
                    && replayed[0].base_revision == 6
                    && replayed[1].new_revision == 8
        );

        let start = Instant::now();
        let beyond = retention_session
            .bootstrap_resume(&resume_request(
                &retention_session,
                format!("expired-client-{sample}").as_bytes(),
                0,
            ))
            .map_err(|error| error.to_string())?;
        timings
            .entry("resume beyond journal retention")
            .or_default()
            .push(start.elapsed().as_secs_f64() * 1_000.0);
        retention_correct &= matches!(
            beyond.outcome,
            ResumeOutcome::Resync {
                resync_msg,
                snapshot_transaction,
            } if resync_msg.continuity == SessionContinuity::SameSession as i32
                && resync_msg.snapshot_revision == 8
                && snapshot_transaction.new_revision == 8
        );
    }

    let wire_sample_count = iterations.min(50);
    for sample in 0..wire_sample_count {
        let (elapsed, correct) = wire_pre_receipt_disconnect(sample).await?;
        timings
            .entry("disconnect immediately before event receipt")
            .or_default()
            .push(elapsed);
        wire_pre_receipt_correct &= correct;

        let (elapsed, correct) = wire_lost_ack_reconnect(sample).await?;
        timings
            .entry("lost ACK wire reconnect through DUPLICATE acknowledgement")
            .or_default()
            .push(elapsed);
        wire_duplicate_correct &= correct;

        let (elapsed, correct) = wire_partial_transaction_reconnect(sample).await?;
        timings
            .entry("mid-transaction wire disconnect and exact atomic replay")
            .or_default()
            .push(elapsed);
        wire_transaction_replay_correct &= correct;

        let (elapsed, correct) = wire_partial_event_reconnect(sample).await?;
        timings
            .entry("partial EVENT disconnect and one processed replay")
            .or_default()
            .push(elapsed);
        wire_partial_event_correct &= correct;
    }

    let mut sample_counts = BTreeMap::new();
    for (name, values) in &timings {
        let group = match *name {
            "mid-resource reconnect and exact replay" => "rust.mid_resource",
            "mid-transaction frame discard and atomic replay" => "rust.mid_transaction_codec",
            "event receipt through settled side effect" => "rust.event_side_effect",
            "in-process cached DUPLICATE response" => "rust.cached_duplicate",
            "resume within journal retention" => "rust.resume_within_retention",
            "resume beyond journal retention" => "rust.resume_beyond_retention",
            "disconnect immediately before event receipt" => "rust.pre_receipt_wire",
            "lost ACK wire reconnect through DUPLICATE acknowledgement" => "rust.lost_ack_wire",
            "mid-transaction wire disconnect and exact atomic replay" => {
                "rust.mid_transaction_wire"
            }
            "partial EVENT disconnect and one processed replay" => "rust.partial_event_wire",
            unexpected => {
                return Err(format!(
                    "reconnect timing group has no sample-count identity: {unexpected}"
                ));
            }
        };
        if sample_counts.insert(group, values.len()).is_some() {
            return Err(format!(
                "duplicate reconnect sample-count identity: {group}"
            ));
        }
    }

    let mut metrics = Vec::new();
    push_timing_distributions(&mut metrics, timings);
    Ok(Section {
        id: "31.5",
        name: "Reconnect",
        sample_counts,
        metrics,
        assertions: vec![
            Assertion {
                id: "mid_resource_exact_restart",
                name: "mid-resource reconnect restarts production transfer at offset zero",
                passed: resource_replay_correct,
                detail: format!(
                    "{}-byte CAS object interrupted after one real chunk and reconstructed exactly",
                    resource_payload.len()
                ),
            },
            Assertion {
                id: "mid_transaction_wire_atomic",
                name: "mid-transaction wire disconnect exposes no partial state",
                passed: transaction_replay_correct && wire_transaction_replay_correct,
                detail: format!(
                    "{wire_sample_count} capacity-one handle_connection streams decoded no partial frame, left the replica at revision 0, then replayed exactly one atomic revision"
                ),
            },
            Assertion {
                id: "partial_event_wire_once",
                name: "pre-receipt and partial EVENT disconnects are inert before one processed replay",
                passed: event_boundary_correct
                    && wire_pre_receipt_correct
                    && wire_partial_event_correct,
                detail: format!(
                    "{wire_sample_count} pre-receipt and capacity-one partial-frame handle_connection reconnects each dispatched zero events on the interrupted connection, then decoded one Processed acknowledgement and one resulting transaction"
                ),
            },
            Assertion {
                id: "lost_ack_wire_duplicate_once",
                name: "lost ACK wire replay is DUPLICATE without a second side effect",
                passed: duplicate_correct && wire_duplicate_correct,
                detail: format!(
                    "{wire_sample_count} duplex reconnects decoded ServerEventAck::Duplicate with cached revision 2; handler count and state stayed at one effect"
                ),
            },
            Assertion {
                id: "journal_retention_boundary",
                name: "journal retention boundary selects replay versus same-session resync",
                passed: retention_correct,
                detail: "revision 6 replayed 6→8; revision 0 produced SAME_SESSION snapshot at 8"
                    .into(),
            },
        ],
        notes: vec![
            "Superseded resume-attempt inertness is measured against the Task 23 client generation guard in the macOS driver."
                .into(),
        ],
    })
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_srui_transaction_frame_is_not_decoded() {
        let transaction = empty_wire_transaction(0);
        assert!(partial_transaction_frame_is_buffered(&transaction).unwrap());
    }

    #[tokio::test]
    async fn pre_receipt_wire_reconnect_dispatches_only_after_resume() {
        let (_, correct) = wire_pre_receipt_disconnect(0).await.unwrap();
        assert!(correct);
    }

    #[tokio::test]
    async fn partial_transaction_wire_reconnect_applies_exactly_once() {
        let (_, correct) = wire_partial_transaction_reconnect(0).await.unwrap();
        assert!(correct);
    }

    #[tokio::test]
    async fn partial_event_wire_reconnect_dispatches_only_complete_event() {
        let (_, correct) = wire_partial_event_reconnect(0).await.unwrap();
        assert!(correct);
    }

    #[tokio::test]
    async fn lost_ack_wire_reconnect_returns_duplicate_without_second_effect() {
        let (_, correct) = wire_lost_ack_reconnect(0).await.unwrap();
        assert!(correct);
    }
}
