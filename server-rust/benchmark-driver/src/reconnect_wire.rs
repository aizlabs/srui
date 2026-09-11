use bytes::BytesMut;
use futures::{SinkExt, StreamExt};
use srui_protocol::{
    srui_message, ClientHello, ClientResume, EventAckStatus, SruiCodec, SruiMessage,
};
use srui_sdk::{Button, Surface, ACTIVATE, LABEL};
use srui_semantic_tree::{Event as DomainEvent, NodeId, Revision, SemanticStore};
use srui_sessiond::{handle_connection, ConnectionError, Session, CORE_VERSION};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::{Duration, Instant};
use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt, DuplexStream};
use tokio::time::timeout;
use tokio_util::codec::{Decoder, Encoder, FramedRead, FramedWrite};
use tokio_util::sync::CancellationToken;

const WIRE_TIMEOUT: Duration = Duration::from_secs(2);

type WireClientRead = FramedRead<tokio::io::ReadHalf<DuplexStream>, SruiCodec>;
type WireClientWrite = FramedWrite<tokio::io::WriteHalf<DuplexStream>, SruiCodec>;
type WireServerTask = tokio::task::JoinHandle<Result<(), ConnectionError>>;

struct WireConnection {
    write: Option<WireClientWrite>,
    read: Option<WireClientRead>,
    server_task: WireServerTask,
    shutdown: CancellationToken,
}

impl WireConnection {
    fn open(session: Arc<Session>) -> Self {
        Self::open_with_capacity(session, 1024 * 1024)
    }

    fn open_with_capacity(session: Arc<Session>, capacity: usize) -> Self {
        let shutdown = CancellationToken::new();
        let (client_io, server_io) = duplex(capacity);
        let server_shutdown = shutdown.clone();
        let server_task =
            tokio::spawn(
                async move { handle_connection(server_io, session, server_shutdown).await },
            );
        let (client_read, client_write) = tokio::io::split(client_io);
        Self {
            write: Some(FramedWrite::new(client_write, SruiCodec::new())),
            read: Some(FramedRead::new(client_read, SruiCodec::new())),
            server_task,
            shutdown,
        }
    }

    async fn send(&mut self, message: SruiMessage) -> Result<(), String> {
        self.write
            .as_mut()
            .ok_or_else(|| "wire benchmark writer is already closed".to_string())?
            .send(message)
            .await
            .map_err(|error| error.to_string())
    }

    async fn hello(&mut self, client_instance_id: &[u8]) -> Result<(), String> {
        self.send(SruiMessage {
            msg: Some(srui_message::Msg::ClientHello(ClientHello {
                core_version: CORE_VERSION.to_string(),
                profiles: vec!["org.srui.standard-widgets/1".to_string()],
                client_instance_id: client_instance_id.to_vec(),
                ..ClientHello::default()
            })),
        })
        .await
    }

    async fn resume(&mut self, request: ClientResume) -> Result<(), String> {
        self.send(SruiMessage {
            msg: Some(srui_message::Msg::ClientResume(request)),
        })
        .await
    }

    async fn read(&mut self) -> Result<SruiMessage, String> {
        let read = self
            .read
            .as_mut()
            .ok_or_else(|| "wire benchmark reader is already closed".to_string())?;
        timeout(WIRE_TIMEOUT, read.next())
            .await
            .map_err(|_| "timed out waiting for a wire benchmark frame".to_string())?
            .ok_or_else(|| {
                "wire benchmark connection closed before the expected frame".to_string()
            })?
            .map_err(|error| error.to_string())
    }

    fn take_read(&mut self) -> Result<WireClientRead, String> {
        self.read
            .take()
            .ok_or_else(|| "wire benchmark reader is already closed".to_string())
    }

    fn take_write(&mut self) -> Result<WireClientWrite, String> {
        self.write
            .take()
            .ok_or_else(|| "wire benchmark writer is already closed".to_string())
    }

    async fn close(mut self, boundary: &'static str) -> Result<(), String> {
        drop(self.write.take());
        drop(self.read.take());

        let mut server_task = self.server_task;
        let joined = match timeout(WIRE_TIMEOUT, &mut server_task).await {
            Ok(joined) => joined,
            Err(_) => {
                self.shutdown.cancel();
                timeout(WIRE_TIMEOUT, server_task).await.map_err(|_| {
                    format!("{boundary} connection did not terminate after shutdown")
                })?
            }
        };
        let _connection_result = joined.map_err(|error| error.to_string())?;
        Ok(())
    }
}

struct EventFixture {
    session: Arc<Session>,
    button: NodeId,
    side_effects: Arc<AtomicUsize>,
    client_instance_id: Vec<u8>,
    event_id: &'static str,
}

impl EventFixture {
    fn new(prefix: &str, sample: usize, event_id: &'static str) -> Result<Self, String> {
        let session = Arc::new(Session::new(format!("{prefix}-{sample}")));
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
                .expect("wire benchmark handler transaction");
        });

        Ok(Self {
            session,
            button,
            side_effects,
            client_instance_id: format!("{prefix}-client-{sample}").into_bytes(),
            event_id,
        })
    }

    fn event(&self) -> SruiMessage {
        SruiMessage {
            msg: Some(srui_message::Msg::Event(
                DomainEvent::activate(1, self.event_id, 1, self.button)
                    .with_client_instance_id(self.client_instance_id.clone())
                    .to_wire(),
            )),
        }
    }

    fn state_is(&self, effect_count: usize, revision: u64, label: &str) -> bool {
        self.side_effects.load(Ordering::SeqCst) == effect_count
            && self.session.current_revision() == revision
            && self.session.with_store(|store| {
                Button::from_store(store, self.button).and_then(|value| value.label(store))
                    == Some(label)
            })
    }
}

async fn event_fresh_handshake(
    connection: &mut WireConnection,
    fixture: &EventFixture,
) -> Result<bool, String> {
    connection.hello(&fixture.client_instance_id).await?;
    let welcome = connection.read().await?;
    let snapshot = connection.read().await?;
    Ok(matches!(
        welcome.msg,
        Some(srui_message::Msg::ServerWelcome(value))
            if value.session_id == fixture.session.session_id() && value.initial_revision == 1
    ) && matches!(
        snapshot.msg,
        Some(srui_message::Msg::Transaction(transaction))
            if transaction.base_revision == 0 && transaction.new_revision == 1
    ))
}

fn resume_request(fixture: &EventFixture, last_applied_revision: u64) -> ClientResume {
    ClientResume {
        session_id: fixture.session.session_id(),
        client_instance_id: fixture.client_instance_id.clone(),
        last_applied_revision,
        last_acked_event_seq: 0,
        ..ClientResume::default()
    }
}

fn processed_ack_is_exact(message: &SruiMessage, fixture: &EventFixture) -> bool {
    matches!(
        &message.msg,
        Some(srui_message::Msg::ServerEventAck(ack))
            if ack.status() == EventAckStatus::Processed
                && ack.client_instance_id == fixture.client_instance_id
                && ack.event_id == fixture.event_id.as_bytes()
                && ack.revision_after_effect == 2
                && ack.last_processed_event_seq == 1
                && ack.settled_event_seq == 1
    )
}

fn effect_transaction_is_exact(message: &SruiMessage) -> bool {
    matches!(
        &message.msg,
        Some(srui_message::Msg::Transaction(transaction))
            if transaction.base_revision == 1 && transaction.new_revision == 2
    )
}

pub(crate) async fn wire_pre_receipt_disconnect(sample: usize) -> Result<(f64, bool), String> {
    let fixture = EventFixture::new(
        "benchmark-wire-pre-receipt",
        sample,
        "benchmark-pre-receipt",
    )?;
    let mut first = WireConnection::open(Arc::clone(&fixture.session));
    let fresh_correct = event_fresh_handshake(&mut first, &fixture).await?;

    let start = Instant::now();
    first.close("pre-receipt EVENT").await?;
    let first_connection_inert = fixture.session.is_detached() && fixture.state_is(0, 1, "Run");

    let mut resumed = WireConnection::open(Arc::clone(&fixture.session));
    resumed.resume(resume_request(&fixture, 1)).await?;
    let resume = resumed.read().await?;
    let resume_correct = matches!(
        resume.msg,
        Some(srui_message::Msg::ServerResumeOk(value))
            if value.session_id == fixture.session.session_id()
                && value.replay_from_revision == 1
                && value.last_processed_event_seq == 0
    );

    resumed.send(fixture.event()).await?;
    let processed_ack = resumed.read().await?;
    let handler_transaction = resumed.read().await?;
    resumed.close("resumed pre-receipt EVENT").await?;
    let processed_once = processed_ack_is_exact(&processed_ack, &fixture)
        && effect_transaction_is_exact(&handler_transaction)
        && fixture.session.is_detached()
        && fixture.state_is(1, 2, "Clicked");
    let elapsed = start.elapsed().as_secs_f64() * 1_000.0;

    Ok((
        elapsed,
        fresh_correct && first_connection_inert && resume_correct && processed_once,
    ))
}

pub(crate) async fn wire_lost_ack_reconnect(sample: usize) -> Result<(f64, bool), String> {
    let fixture = EventFixture::new("benchmark-wire-event", sample, "benchmark-lost-ack")?;
    let mut first = WireConnection::open(Arc::clone(&fixture.session));
    let fresh_correct = event_fresh_handshake(&mut first, &fixture).await?;
    first.send(fixture.event()).await?;
    timeout(WIRE_TIMEOUT, async {
        while fixture.side_effects.load(Ordering::SeqCst) != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(|_| "timed out waiting for the first wire event side effect".to_string())?;

    let start = Instant::now();
    first.close("lost-ack EVENT").await?;
    let state_after_first = fixture.session.is_detached() && fixture.state_is(1, 2, "Clicked");

    let mut resumed = WireConnection::open(Arc::clone(&fixture.session));
    resumed.resume(resume_request(&fixture, 1)).await?;
    let resume = resumed.read().await?;
    let replayed_transaction = resumed.read().await?;
    let resume_correct = matches!(
        resume.msg,
        Some(srui_message::Msg::ServerResumeOk(value))
            if value.session_id == fixture.session.session_id()
                && value.replay_from_revision == 1
                && value.last_processed_event_seq == 1
    ) && effect_transaction_is_exact(&replayed_transaction);

    resumed.send(fixture.event()).await?;
    let duplicate_ack = resumed.read().await?;
    resumed.close("resumed lost-ack EVENT").await?;
    let elapsed = start.elapsed().as_secs_f64() * 1_000.0;
    let duplicate_correct = matches!(
        duplicate_ack.msg,
        Some(srui_message::Msg::ServerEventAck(ack))
            if ack.status() == EventAckStatus::Duplicate
                && ack.client_instance_id == fixture.client_instance_id
                && ack.event_id == fixture.event_id.as_bytes()
                && ack.revision_after_effect == 2
                && ack.last_processed_event_seq == 1
                && ack.settled_event_seq == 1
    );
    let final_state_exact = fixture.session.is_detached() && fixture.state_is(1, 2, "Clicked");

    Ok((
        elapsed,
        fresh_correct
            && state_after_first
            && resume_correct
            && duplicate_correct
            && final_state_exact,
    ))
}

pub(crate) async fn wire_partial_transaction_reconnect(
    sample: usize,
) -> Result<(f64, bool), String> {
    let session = Arc::new(Session::new(format!("benchmark-wire-transaction-{sample}")));
    let client_instance_id = format!("wire-transaction-client-{sample}").into_bytes();
    let mut first = WireConnection::open_with_capacity(Arc::clone(&session), 1);
    first.hello(&client_instance_id).await?;
    let welcome = first.read().await?;
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
    let mut raw_read = first.take_read()?.into_inner();
    let mut first_frame_byte = [0_u8; 1];
    timeout(WIRE_TIMEOUT, raw_read.read_exact(&mut first_frame_byte))
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

    drop(raw_read);
    first.close("partial transaction").await?;
    let detached_after_partial = session.is_detached();

    let mut resumed = WireConnection::open(Arc::clone(&session));
    resumed
        .resume(ClientResume {
            session_id: session.session_id(),
            client_instance_id,
            last_applied_revision: 0,
            last_acked_event_seq: 0,
            ..ClientResume::default()
        })
        .await?;
    let resume = resumed.read().await?;
    let replay = resumed.read().await?;
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
    resumed.close("resumed partial transaction").await?;
    let replica_correct = replica.revision() == Revision::new(1)
        && replica.node_count() == 1
        && replica.contains_node(NodeId::new(1));
    let elapsed = start.elapsed().as_secs_f64() * 1_000.0;

    Ok((
        elapsed,
        fresh_correct
            && no_partial_decode
            && replica_untouched
            && detached_after_partial
            && resume_correct
            && applied_once
            && replica_correct
            && session.is_detached()
            && session.current_revision() == 1,
    ))
}

pub(crate) async fn wire_partial_event_reconnect(sample: usize) -> Result<(f64, bool), String> {
    let fixture = EventFixture::new(
        "benchmark-wire-partial-event",
        sample,
        "benchmark-partial-event",
    )?;
    let mut first = WireConnection::open_with_capacity(Arc::clone(&fixture.session), 1);
    let fresh_correct = event_fresh_handshake(&mut first, &fixture).await?;

    let full_event = fixture.event();
    let mut encoded_event = BytesMut::new();
    SruiCodec::new()
        .encode(full_event.clone(), &mut encoded_event)
        .map_err(|error| error.to_string())?;
    let split = encoded_event.len() / 2;
    let start = Instant::now();
    let mut raw_write = first.take_write()?.into_inner();
    timeout(WIRE_TIMEOUT, raw_write.write_all(&encoded_event[..split]))
        .await
        .map_err(|_| "timed out sending the partial EVENT frame".to_string())?
        .map_err(|error| error.to_string())?;
    drop(raw_write);
    first.close("partial EVENT").await?;
    let partial_was_inert = fixture.session.is_detached() && fixture.state_is(0, 1, "Run");

    let mut resumed = WireConnection::open(Arc::clone(&fixture.session));
    resumed.resume(resume_request(&fixture, 1)).await?;
    let resume = resumed.read().await?;
    let resume_correct = matches!(
        resume.msg,
        Some(srui_message::Msg::ServerResumeOk(value))
            if value.session_id == fixture.session.session_id()
                && value.replay_from_revision == 1
                && value.last_processed_event_seq == 0
    );

    resumed.send(full_event).await?;
    let processed_ack = resumed.read().await?;
    let handler_transaction = resumed.read().await?;
    resumed.close("resumed partial EVENT").await?;
    let processed_once = processed_ack_is_exact(&processed_ack, &fixture)
        && effect_transaction_is_exact(&handler_transaction)
        && fixture.session.is_detached()
        && fixture.state_is(1, 2, "Clicked");
    let elapsed = start.elapsed().as_secs_f64() * 1_000.0;

    Ok((
        elapsed,
        fresh_correct && partial_was_inert && resume_correct && processed_once,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

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
