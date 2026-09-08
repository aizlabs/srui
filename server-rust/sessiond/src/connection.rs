//! # Client Connection Handler
//!
//! Manages an attached client/bridge stream over Unix socket or SSH channel (§18, §20.2, §21).
//!
//! Every settled client event receives a `SERVER EVENT_ACK` on the same connection (§18.2), which
//! is control-class traffic (§19.2). An overlapping in-flight replay remains unacknowledged until
//! a retry can read the settled result. Validation refusals are acknowledged as `REJECTED` rather
//! than closing the stream; only protocol violations are fatal.
//!
//! Post-handshake, reading and writing are concurrently driven futures over the already-split
//! socket halves: the read future consumes semantic events independently of resource output, and
//! the write future selects exactly one frame through
//! [`crate::outbound::LogicalChannelScheduler`]. Acks are enqueued one-at-a-time on a bounded control channel (`send().await`) rather than dropped.
//!
//! Conforms strictly to:
//! - [`async-cancel-safety`](rules/async-cancel-safety.md): uses [`SruiCodec`] with `tokio_util::codec::FramedRead`
//!   inside `tokio::select!` so mid-frame cancellations do not corrupt stream buffers.
//! - [`async-bounded-channel`](rules/async-bounded-channel.md): all transaction, control, and event flows use bounded queues.
//! - [`async-cancellation-token`](rules/async-cancellation-token.md): uses a connection-local [`CancellationToken`] for clean disconnection.
//! - [`async-no-lock-await`](rules/async-no-lock-await.md): no locks are held across `.await`.

mod writer;

use self::writer::write_loop;
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;
use tokio_util::codec::{FramedRead, FramedWrite};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::outbound::{
    server_envelope_matches_class, LogicalChannelClass, OutboundReceiver, OutboundRecvError,
};
use crate::session::terminal::{event_to_message, live_class_for_event};
use crate::session::{
    run_model_range_worker, EventOutcome, ModelRangeRequestInbox, ResumeOutcome, Session,
    SessionError,
};
use srui_protocol::{
    srui_message, EventAckStatus, FramingError, ServerEventAck, SruiCodec, SruiMessage,
    MAX_TERMINAL_INPUT_BYTES,
};
use srui_pty::TerminalSubscription;
use srui_semantic_tree::NodeId;
use thiserror::Error;

/// Handshake timeout in seconds (5 seconds, §18.1).
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// How long one outbound frame may stay unaccepted before the client is treated as unreachable.
///
/// The outbound queue bounds how much a slow client may buffer, but it cannot bound a client that
/// stops reading the socket entirely: TCP backpressure then parks the write itself, and neither
/// the queue nor the disconnect token ever fires because nothing else is trying to publish to it.
/// The deadline turns that indefinite park into a detach, after which the client resyncs (§20.2).
pub const WRITE_TIMEOUT: Duration = Duration::from_secs(30);

/// Bounded per-connection queue of control-class frames (SERVER EVENT_ACK, §18.2, §19.2).
///
/// send().await applies backpressure into the read future instead of dropping acknowledgements.
const CONTROL_CHANNEL_CAPACITY: usize = 64;

/// Errors occurring during connection lifecycle.
#[derive(Debug, Error)]
pub enum ConnectionError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("framing error: {0}")]
    Framing(#[from] FramingError),

    #[error("session error: {0}")]
    Session(#[from] SessionError),

    #[error("handshake timed out")]
    HandshakeTimeout,

    #[error("connection closed unexpectedly")]
    ConnectionClosed,

    #[error("unexpected message: {0}")]
    UnexpectedMessage(&'static str),

    #[error("client-originated transaction rejected: server is authoritative (§12, §20.2)")]
    ClientTransactionRejected,

    #[error("event client_instance_id does not match the connection handshake")]
    ClientInstanceMismatch,

    #[error("client did not accept an outbound frame within {0:?}; detaching for resync")]
    WriteTimeout(Duration),
}

/// Sends `envelope` unless shutdown or outbound overflow/close fires first.
///
/// Returns `Ok(true)` if the frame was written, `Ok(false)` if the connection should
/// unwind cleanly (shutdown or hub close). A lagged queue is a hard resync error.
///
/// `logical_class` documents the [`LogicalChannelClass`] of this write. Handshake writes
/// keep their original order; the active-session writer selects classes through the
/// scheduler. WELCOME/resume responses are control; snapshots and replayed transactions
/// are UI (§19.2).
///
/// # Cancellation
///
/// The write is polled first (`biased`), so a frame the socket can accept immediately is always
/// written whole even when a cancellation is already pending; only a send that would block yields
/// to the token, which is what keeps a stalled client from delaying `LaggedResyncRequired` (§20.2).
/// [`SinkExt::send`] is not cancel-safe, so a frame large enough to block mid-flush may still be
/// truncated on the wire when a token wins; the connection closes immediately afterwards, and the
/// peer resyncs (§18, §20.2).
async fn send_message<W>(
    framed_write: &mut FramedWrite<W, SruiCodec>,
    envelope: SruiMessage,
    logical_class: LogicalChannelClass,
    shutdown: &CancellationToken,
    outbound: &OutboundReceiver,
) -> Result<bool, ConnectionError>
where
    W: AsyncWrite + Unpin,
{
    send_message_with_read_state(
        framed_write,
        envelope,
        logical_class,
        shutdown,
        outbound,
        None,
    )
    .await
}

async fn send_message_with_read_state<W>(
    framed_write: &mut FramedWrite<W, SruiCodec>,
    envelope: SruiMessage,
    logical_class: LogicalChannelClass,
    shutdown: &CancellationToken,
    outbound: &OutboundReceiver,
    read_finished: Option<&CancellationToken>,
) -> Result<bool, ConnectionError>
where
    W: AsyncWrite + Unpin,
{
    debug_assert!(
        server_envelope_matches_class(&envelope, logical_class),
        "envelope class must match the annotated write class"
    );
    debug!(?logical_class, "sending outbound frame");

    // Keep the same send future across state changes. Once the read side finishes, aborting an
    // in-flight frame is not cancellation-safe and could also discard accepted control frames
    // behind it, so outbound close/lag stops preempting this bounded write.
    let send = tokio::time::timeout(WRITE_TIMEOUT, framed_write.send(envelope));
    tokio::pin!(send);
    let mut draining_after_read = read_finished.is_some_and(CancellationToken::is_cancelled);

    loop {
        tokio::select! {
            biased;
            res = &mut send => {
                return match res {
                    Ok(sent) => {
                        sent?;
                        Ok(true)
                    }
                    Err(_) => {
                        warn!(timeout = ?WRITE_TIMEOUT, "Client did not accept an outbound frame; detaching for resync");
                        Err(ConnectionError::WriteTimeout(WRITE_TIMEOUT))
                    }
                };
            }
            _ = shutdown.cancelled() => return Ok(false),
            _ = async {
                if let Some(read_finished) = read_finished {
                    read_finished.cancelled().await;
                }
            }, if !draining_after_read && read_finished.is_some() => {
                draining_after_read = true;
            }
            _ = outbound.disconnect_token().cancelled(), if !draining_after_read => {
                return match outbound.termination() {
                    Some(OutboundRecvError::Closed) => Ok(false),
                    Some(OutboundRecvError::Lagged(reason)) => {
                        warn!(%reason, "Client outbound queue overflowed during send; closing connection to force resync");
                        Err(ConnectionError::Session(SessionError::LaggedResyncRequired))
                    }
                    None => {
                        warn!("Client outbound subscriber disconnected during send; closing connection to force resync");
                        Err(ConnectionError::Session(SessionError::LaggedResyncRequired))
                    }
                };
            }
        }
    }
}

/// Clears the §20.2 overflow marker once the catch-up write has been delivered.
///
/// The subscription is created during handshake bootstrap, before the welcome/resync message and
/// the snapshot are written, so it can overflow *during* that write. Clearing unconditionally would
/// erase that fresh marker and downgrade the next resume to a journal replay, defeating the forced
/// resync the marker exists to guarantee (§20.2).
fn clear_stale_if_settled(
    session: &Session,
    client_instance_id: &[u8],
    outbound: &OutboundReceiver,
) {
    if outbound.termination().is_none() {
        session.clear_stale_client(client_instance_id);
    }
}

/// Handles an active client connection stream through handshake and event processing.
pub async fn handle_connection<S>(
    stream: S,
    session: Arc<Session>,
    shutdown: CancellationToken,
) -> Result<(), ConnectionError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    // Attach at transport connect (§17, App. B). The guard drops on every exit path—including
    // handshake failure, cancellation, and EOF—transitioning ATTACHED -> DETACHED.
    let _attachment = session
        .attach()
        .ok_or_else(|| ConnectionError::Session(SessionError::TerminalState(session.state())))?;

    let (read_half, write_half) = tokio::io::split(stream);
    let mut framed_read = FramedRead::new(read_half, SruiCodec::new());
    let mut framed_write = FramedWrite::new(write_half, SruiCodec::new());

    // -------------------------------------------------------------------------
    // Phase 1: Handshake Negotiation (§15, §18)
    // -------------------------------------------------------------------------
    let handshake_msg = tokio::select! {
        msg = tokio::time::timeout(HANDSHAKE_TIMEOUT, framed_read.next()) => {
            match msg {
                Ok(Some(Ok(m))) => m,
                Ok(Some(Err(e))) => return Err(ConnectionError::Framing(e)),
                Ok(None) => return Err(ConnectionError::ConnectionClosed),
                Err(_) => return Err(ConnectionError::HandshakeTimeout),
            }
        }
        _ = shutdown.cancelled() => {
            return Ok(());
        }
    };

    let (client_instance_id, tx_rx, terminal) = match handshake_msg.msg {
        Some(srui_message::Msg::ClientHello(hello)) => {
            info!(
                client_instance_id = ?hello.client_instance_id,
                "Received ClientHello"
            );
            let bootstrap = session.bootstrap_fresh_client(&hello)?;
            let welcome_envelope = SruiMessage {
                msg: Some(srui_message::Msg::ServerWelcome(bootstrap.welcome)),
            };
            if !send_message(
                &mut framed_write,
                welcome_envelope,
                LogicalChannelClass::Control,
                &shutdown,
                &bootstrap.transactions,
            )
            .await?
            {
                return Ok(());
            }
            if let Some(snapshot) = bootstrap.snapshot {
                let snapshot_envelope = SruiMessage {
                    msg: Some(srui_message::Msg::Transaction(snapshot)),
                };
                if !send_message(
                    &mut framed_write,
                    snapshot_envelope,
                    LogicalChannelClass::Ui,
                    &shutdown,
                    &bootstrap.transactions,
                )
                .await?
                {
                    return Ok(());
                }
            }
            clear_stale_if_settled(&session, &hello.client_instance_id, &bootstrap.transactions);
            (
                hello.client_instance_id,
                bootstrap.transactions,
                TerminalConnection {
                    negotiated: bootstrap.terminal_negotiated,
                    live: bootstrap.terminal.live,
                    catch_up: bootstrap.terminal.catch_up,
                },
            )
        }
        Some(srui_message::Msg::ClientResume(resume)) => {
            info!(
                session_id = %resume.session_id,
                last_applied_revision = resume.last_applied_revision,
                "Received ClientResume"
            );
            let bootstrap = session.bootstrap_resume(&resume)?;
            match bootstrap.outcome {
                ResumeOutcome::Replay {
                    welcome_msg,
                    replayed,
                } => {
                    let envelope = SruiMessage {
                        msg: Some(srui_message::Msg::ServerResumeOk(welcome_msg)),
                    };
                    if !send_message(
                        &mut framed_write,
                        envelope,
                        LogicalChannelClass::Control,
                        &shutdown,
                        &bootstrap.transactions,
                    )
                    .await?
                    {
                        return Ok(());
                    }
                    for tx in replayed {
                        let tx_env = SruiMessage {
                            msg: Some(srui_message::Msg::Transaction(tx)),
                        };
                        if !send_message(
                            &mut framed_write,
                            tx_env,
                            LogicalChannelClass::Ui,
                            &shutdown,
                            &bootstrap.transactions,
                        )
                        .await?
                        {
                            return Ok(());
                        }
                    }
                }
                ResumeOutcome::Resync {
                    resync_msg,
                    snapshot_transaction,
                } => {
                    let envelope = SruiMessage {
                        msg: Some(srui_message::Msg::ServerResyncRequired(resync_msg)),
                    };
                    if !send_message(
                        &mut framed_write,
                        envelope,
                        LogicalChannelClass::Control,
                        &shutdown,
                        &bootstrap.transactions,
                    )
                    .await?
                    {
                        return Ok(());
                    }
                    let snapshot_env = SruiMessage {
                        msg: Some(srui_message::Msg::Transaction(snapshot_transaction)),
                    };
                    if !send_message(
                        &mut framed_write,
                        snapshot_env,
                        LogicalChannelClass::Ui,
                        &shutdown,
                        &bootstrap.transactions,
                    )
                    .await?
                    {
                        return Ok(());
                    }
                    clear_stale_if_settled(
                        &session,
                        &resume.client_instance_id,
                        &bootstrap.transactions,
                    );
                }
            }
            (
                resume.client_instance_id,
                bootstrap.transactions,
                TerminalConnection {
                    negotiated: bootstrap.terminal_negotiated,
                    live: bootstrap.terminal.live,
                    catch_up: bootstrap.terminal.catch_up,
                },
            )
        }
        _ => {
            return Err(ConnectionError::UnexpectedMessage(
                "expected ClientHello or ClientResume",
            ));
        }
    };

    run_active_session(
        framed_read,
        framed_write,
        session,
        client_instance_id,
        tx_rx,
        terminal,
        shutdown,
    )
    .await
}

const TERMINAL_LANE_CAPACITY: usize = 64;

struct TerminalConnection {
    negotiated: bool,
    live: Vec<TerminalSubscription>,
    catch_up: Vec<(LogicalChannelClass, SruiMessage)>,
}

pub(super) struct TerminalLanes {
    high_rx: mpsc::Receiver<SruiMessage>,
    normal_rx: mpsc::Receiver<SruiMessage>,
    catch_up: Vec<(LogicalChannelClass, SruiMessage)>,
    catch_up_released: tokio::sync::watch::Sender<bool>,
}

pub(super) struct WriterCancel {
    shutdown: CancellationToken,
    session_cancel: CancellationToken,
    read_finished: CancellationToken,
}

fn spawn_terminal_live_pumps(
    live: Vec<TerminalSubscription>,
    catch_up: Vec<(LogicalChannelClass, SruiMessage)>,
    shutdown: CancellationToken,
    session_cancel: CancellationToken,
) -> (TerminalLanes, Vec<tokio::task::JoinHandle<()>>) {
    let (high_tx, high_rx) = mpsc::channel(TERMINAL_LANE_CAPACITY);
    let (normal_tx, normal_rx) = mpsc::channel(TERMINAL_LANE_CAPACITY);
    let (catch_up_released, released_rx) = tokio::sync::watch::channel(catch_up.is_empty());
    let mut tasks = Vec::new();

    // Live High must not race handshake replay: the Swift client treats an
    // ahead-of-cursor frame as a local reset and then drops older replay as
    // duplicates. Gate pumps until the writer has put every catch-up frame
    // on the wire through TerminalNormal / TerminalHigh scheduler slots.
    for mut subscription in live {
        let high_tx = high_tx.clone();
        let normal_tx = normal_tx.clone();
        let shutdown = shutdown.clone();
        let session_cancel = session_cancel.clone();
        let mut released = released_rx.clone();
        tasks.push(tokio::spawn(async move {
            // Do not drain the ring while the gate is closed. Staging live output here
            // would bypass both the bounded output ring and the bounded terminal lane, so a
            // continuously writing child could grow the server heap for as long as the socket
            // blocks. Leaving the bytes in the ring keeps the §21.2 retention bound; a cursor
            // that falls behind the retained window emits the existing SubscriberFallbehind
            // resync on the first drain after release.
            while !*released.borrow() {
                tokio::select! {
                    biased;
                    _ = session_cancel.cancelled() => return,
                    _ = shutdown.cancelled() => return,
                    changed = released.changed() => {
                        if changed.is_err() {
                            return;
                        }
                    }
                }
            }

            loop {
                tokio::select! {
                    biased;
                    _ = session_cancel.cancelled() => return,
                    _ = shutdown.cancelled() => return,
                    events = subscription.recv() => {
                        if events.is_empty() {
                            return;
                        }
                        for event in events {
                            let class = live_class_for_event(&event);
                            let msg = event_to_message(event);
                            match class {
                                LogicalChannelClass::TerminalHigh => {
                                    if high_tx.send(msg).await.is_err() {
                                        return;
                                    }
                                }
                                LogicalChannelClass::TerminalNormal => {
                                    if normal_tx.send(msg).await.is_err() {
                                        return;
                                    }
                                }
                                _ => {
                                    if high_tx.send(msg).await.is_err() {
                                        return;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }));
    }
    drop(high_tx);
    drop(normal_tx);
    drop(released_rx);
    (
        TerminalLanes {
            high_rx,
            normal_rx,
            catch_up,
            catch_up_released,
        },
        tasks,
    )
}

/// Concurrently drives inbound events and scheduled outbound writes (§18.2, §19.2, §20).
///
/// Inbound event handling never waits for the physical writer, so socket backpressure cannot stall
/// semantic input. Clean inbound EOF drops the control sender and lets the writer drain every
/// acknowledgement already accepted by the bounded channel before closing the connection.
async fn run_active_session<R, W>(
    framed_read: FramedRead<R, SruiCodec>,
    framed_write: FramedWrite<W, SruiCodec>,
    session: Arc<Session>,
    client_instance_id: Vec<u8>,
    outbound: OutboundReceiver,
    terminal: TerminalConnection,
    shutdown: CancellationToken,
) -> Result<(), ConnectionError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let session_cancel = CancellationToken::new();
    let read_finished = CancellationToken::new();
    let (control_tx, control_rx) = mpsc::channel(CONTROL_CHANNEL_CAPACITY);
    let range_inbox = ModelRangeRequestInbox::new();
    let range_worker = {
        let session = Arc::clone(&session);
        let inbox = range_inbox.clone();
        let shutdown = shutdown.clone();
        let session_cancel = session_cancel.clone();
        tokio::spawn(async move {
            run_model_range_worker(session, inbox, shutdown, session_cancel).await;
        })
    };

    let (terminal_lanes, terminal_pumps) = spawn_terminal_live_pumps(
        terminal.live,
        terminal.catch_up,
        shutdown.clone(),
        session_cancel.clone(),
    );
    let read = read_loop(
        framed_read,
        session,
        client_instance_id,
        control_tx,
        range_inbox.clone(),
        terminal.negotiated,
        WriterCancel {
            shutdown: shutdown.clone(),
            session_cancel: session_cancel.clone(),
            read_finished: read_finished.clone(),
        },
    );
    let write = write_loop(
        framed_write,
        outbound,
        control_rx,
        terminal_lanes,
        WriterCancel {
            shutdown,
            session_cancel: session_cancel.clone(),
            read_finished: read_finished.clone(),
        },
    );

    tokio::pin!(read);
    tokio::pin!(write);
    let result = tokio::select! {
        biased;
        result = &mut read => {
            // Stop selecting new low-priority work while the closed control sender is drained.
            read_finished.cancel();
            match result {
                // The completed reader has dropped control_tx. Drain queued acknowledgements
                // rather than cancelling the writer on a clean inbound half-close.
                Ok(()) => write.await,
                Err(error) => {
                    // The reader has dropped control_tx even on failure. Preserve its error as
                    // the connection result, but first give already accepted acknowledgements the
                    // same bounded drain opportunity as a clean inbound half-close.
                    if let Err(drain_error) = write.await {
                        warn!(
                            error = %drain_error,
                            "Connection writer failed while draining acknowledgements after read error"
                        );
                    }
                    Err(error)
                }
            }
        }
        result = &mut write => {
            session_cancel.cancel();
            result
        }
    };
    range_inbox.close();
    session_cancel.cancel();
    for task in terminal_pumps {
        let _ = task.await;
    }
    let _ = range_worker.await;
    result
}

async fn read_loop<R>(
    mut framed_read: FramedRead<R, SruiCodec>,
    session: Arc<Session>,
    client_instance_id: Vec<u8>,
    control_tx: mpsc::Sender<ServerEventAck>,
    range_inbox: ModelRangeRequestInbox,
    terminal_negotiated: bool,
    cancel: WriterCancel,
) -> Result<(), ConnectionError>
where
    R: AsyncRead + Unpin,
{
    loop {
        tokio::select! {
            biased;
            _ = cancel.session_cancel.cancelled() => return Ok(()),
            _ = cancel.shutdown.cancelled() => {
                debug!("Connection read loop terminating due to shutdown signal");
                return Ok(());
            }
            incoming = framed_read.next() => {
                match incoming {
                    Some(Ok(msg)) => {
                        if let Some(ack) =
                            handle_incoming_message(
                                msg,
                                &session,
                                &client_instance_id,
                                &range_inbox,
                                terminal_negotiated,
                            )
                            .await?
                        {
                            tokio::select! {
                                biased;
                                sent = control_tx.send(ack) => {
                                    if sent.is_err() {
                                        return Ok(());
                                    }
                                }
                                _ = cancel.session_cancel.cancelled() => return Ok(()),
                                _ = cancel.shutdown.cancelled() => return Ok(()),
                            }
                        }
                    }
                    Some(Err(e)) => {
                        error!(error = %e, "Framing error on client stream");
                        return Err(ConnectionError::Framing(e));
                    }
                    None => {
                        info!("Client disconnected normally");
                        return Ok(());
                    }
                }
            }
        }
    }
}

async fn handle_incoming_message(
    msg: SruiMessage,
    session: &Session,
    client_instance_id: &[u8],
    range_inbox: &ModelRangeRequestInbox,
    terminal_negotiated: bool,
) -> Result<Option<ServerEventAck>, ConnectionError> {
    match msg.msg {
        Some(srui_message::Msg::Event(event)) => {
            if event.client_instance_id.as_slice() != client_instance_id {
                warn!(
                    "Rejecting event {:?}: client_instance_id does not match handshake",
                    event.event_id
                );
                return Err(ConnectionError::ClientInstanceMismatch);
            }

            // §18.2: a settled re-delivery is answered from the result cache rather than re-run.
            // A replay observed while another connection is still dispatching stays non-terminal
            // and receives no ack, so the client keeps it in the retry set.
            let outcome = session.process_event(&event)?;
            match &outcome {
                EventOutcome::Processed { .. } => {
                    debug!(
                        event_seq = event.event_seq,
                        event_id_bytes = event.event_id.len(),
                        "Handled event"
                    );
                }
                EventOutcome::Pending { .. } => {
                    debug!(
                        event_seq = event.event_seq,
                        event_id_bytes = event.event_id.len(),
                        "Event is already in flight"
                    );
                }
                EventOutcome::Duplicate { .. } => {
                    debug!(
                        event_seq = event.event_seq,
                        event_id_bytes = event.event_id.len(),
                        "Ignored duplicate event"
                    );
                }
                EventOutcome::Rejected { error, .. } => {
                    // Never log the peer-controlled identifier itself. Oversized identifiers land
                    // here deliberately, and formatting them would turn rejection into log
                    // amplification.
                    warn!(
                        event_seq = event.event_seq,
                        event_id_bytes = event.event_id.len(),
                        error = %error,
                        "Rejecting event"
                    );
                }
            }
            Ok(build_event_ack(
                &event,
                &outcome,
                session.max_string_length(),
                session.session_id(),
            ))
        }
        Some(srui_message::Msg::ClientModelRangeRequest(request)) => {
            range_inbox.submit(request);
            Ok(None)
        }
        Some(srui_message::Msg::Transaction(tx)) => {
            warn!(
                "Rejecting client-originated transaction rev {} -> {}; remote authority forbids client commits",
                tx.base_revision, tx.new_revision
            );
            Err(ConnectionError::ClientTransactionRejected)
        }
        Some(srui_message::Msg::TerminalInput(input)) => {
            handle_terminal_input(session, terminal_negotiated, input)?;
            Ok(None)
        }
        Some(srui_message::Msg::TerminalResize(resize)) => {
            handle_terminal_resize(session, terminal_negotiated, resize)?;
            Ok(None)
        }
        Some(srui_message::Msg::ClientHello(_)) => Err(ConnectionError::UnexpectedMessage(
            "ClientHello is valid only during handshake",
        )),
        Some(srui_message::Msg::ClientResume(_)) => Err(ConnectionError::UnexpectedMessage(
            "ClientResume is valid only during handshake",
        )),
        Some(srui_message::Msg::TerminalData(_))
        | Some(srui_message::Msg::TerminalResyncRequired(_)) => Err(
            ConnectionError::UnexpectedMessage("server-only terminal message received from client"),
        ),
        Some(_) => Err(ConnectionError::UnexpectedMessage(
            "server-only or unsupported message during active session",
        )),
        // prost decodes any envelope whose oneof field number this build does not know to `None`,
        // so failing here would drop the connection of a client speaking a newer protocol. §4
        // inv. 13 requires unknown *required* semantics to fail closed; an unrecognized optional
        // envelope is ignored instead.
        None => {
            warn!("Ignoring empty or unrecognized active-session envelope");
            Ok(None)
        }
    }
}

fn handle_terminal_input(
    session: &Session,
    terminal_negotiated: bool,
    input: srui_protocol::TerminalInput,
) -> Result<(), ConnectionError> {
    if !terminal_negotiated {
        return Err(ConnectionError::UnexpectedMessage(
            "TerminalInput is legal only after org.srui.terminal/1 negotiation",
        ));
    }
    if input.data.is_empty() {
        return Err(ConnectionError::UnexpectedMessage(
            "empty TerminalInput is forbidden",
        ));
    }
    if input.data.len() > MAX_TERMINAL_INPUT_BYTES {
        return Err(ConnectionError::UnexpectedMessage(
            "TerminalInput exceeds MAX_TERMINAL_INPUT_BYTES",
        ));
    }
    let stream_id = NodeId::new(input.stream_id);
    match session.pty().input(stream_id, input.data) {
        Ok(()) => Ok(()),
        Err(srui_pty::PTYManagerError::UnknownStream(_)) => {
            Err(ConnectionError::UnexpectedMessage(
                "TerminalInput targeted an unknown or unnegotiated stream",
            ))
        }
        Err(srui_pty::PTYManagerError::Stream(srui_pty::TerminalStreamError::CommandQueueFull)) => {
            warn!("dropping TerminalInput because the PTY command queue is full");
            Ok(())
        }
        Err(srui_pty::PTYManagerError::Stream(srui_pty::TerminalStreamError::Closed)) => {
            tracing::debug!(
                "dropping TerminalInput because stream {} is closed",
                stream_id.get()
            );
            Ok(())
        }
        Err(error) => Err(ConnectionError::Session(SessionError::InvalidInput(
            error.to_string(),
        ))),
    }
}

fn handle_terminal_resize(
    session: &Session,
    terminal_negotiated: bool,
    resize: srui_protocol::TerminalResize,
) -> Result<(), ConnectionError> {
    if !terminal_negotiated {
        return Err(ConnectionError::UnexpectedMessage(
            "TerminalResize is legal only after org.srui.terminal/1 negotiation",
        ));
    }
    let stream_id = NodeId::new(resize.stream_id);
    match session.pty().resize(
        stream_id,
        resize.columns,
        resize.rows,
        resize.pixel_width,
        resize.pixel_height,
    ) {
        Ok(()) => Ok(()),
        Err(srui_pty::PTYManagerError::UnknownStream(_)) => {
            Err(ConnectionError::UnexpectedMessage(
                "TerminalResize targeted an unknown or unnegotiated stream",
            ))
        }
        Err(srui_pty::PTYManagerError::Stream(srui_pty::TerminalStreamError::CommandQueueFull)) => {
            warn!("dropping TerminalResize because the PTY command queue is full");
            Ok(())
        }
        Err(srui_pty::PTYManagerError::Stream(srui_pty::TerminalStreamError::Closed)) => {
            tracing::debug!(
                "dropping TerminalResize because stream {} is closed",
                stream_id.get()
            );
            Ok(())
        }
        Err(error) => Err(ConnectionError::Session(SessionError::InvalidInput(
            error.to_string(),
        ))),
    }
}

/// Builds the `SERVER EVENT_ACK` settling one client event (§18.2, App. B).
/// Returns `None` for an in-flight replay because it has no terminal outcome to acknowledge.
fn build_event_ack(
    event: &srui_protocol::Event,
    outcome: &EventOutcome,
    max_string_length: usize,
    session_id: String,
) -> Option<ServerEventAck> {
    let (status, revision_after_effect, last_processed_event_seq, reject_reason) = match outcome {
        EventOutcome::Processed {
            revision_after_effect,
            last_processed_event_seq,
        } => (
            EventAckStatus::Processed,
            *revision_after_effect,
            *last_processed_event_seq,
            String::new(),
        ),
        EventOutcome::Pending { .. } => return None,
        EventOutcome::Duplicate {
            accepted,
            revision_after_effect,
            last_processed_event_seq,
            reject_reason,
        } => (
            // §18.2: re-delivery returns the *prior* acknowledgement. A replay of an event that
            // was originally refused stays refused rather than silently reading as handled.
            if *accepted {
                EventAckStatus::Duplicate
            } else {
                EventAckStatus::Rejected
            },
            *revision_after_effect,
            *last_processed_event_seq,
            reject_reason.clone(),
        ),
        EventOutcome::Rejected {
            error,
            revision_after_effect,
            last_processed_event_seq,
        } => (
            EventAckStatus::Rejected,
            *revision_after_effect,
            *last_processed_event_seq,
            crate::session::bound_diagnostic_string(error.to_string(), max_string_length),
        ),
    };

    Some(ServerEventAck {
        client_instance_id: event.client_instance_id.clone(),
        event_id: crate::session::bounded_event_id_for_response(event),
        last_processed_event_seq,
        status: status as i32,
        revision_after_effect,
        reject_reason: crate::session::bound_diagnostic_string(reject_reason, max_string_length),
        session_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outbound::logical_class_for_server_envelope;
    use srui_sdk::{NodeId, Surface};
    use tokio::io::duplex;

    fn ack_envelope(seq: u64) -> SruiMessage {
        SruiMessage {
            msg: Some(srui_message::Msg::ServerEventAck(ServerEventAck {
                last_processed_event_seq: seq,
                ..Default::default()
            })),
        }
    }

    fn overflow_client_queue(session: &Session) {
        for i in 1..=3u64 {
            session
                .transaction(|ui| {
                    Surface::builder(NodeId::new(i)).create(ui)?;
                    Ok(())
                })
                .expect("commit structural transaction");
        }
    }

    /// A frame the socket can accept immediately must not be dropped or truncated just because a
    /// cancellation is already pending; only a send that would block yields to the token (§20.2).
    #[tokio::test]
    async fn test_send_message_completes_writable_frame_when_shutdown_pending() {
        const FRAMES: u64 = 20;

        let session = Session::new("send-under-shutdown");
        let outbound = session
            .subscribe_transactions(vec![1])
            .expect("subscribe outbound");
        let shutdown = CancellationToken::new();
        shutdown.cancel();

        let (client_io, server_io) = duplex(64 * 1024);
        let mut framed_write = FramedWrite::new(server_io, SruiCodec::new());
        let mut framed_read = FramedRead::new(client_io, SruiCodec::new());

        for seq in 0..FRAMES {
            let sent = send_message(
                &mut framed_write,
                ack_envelope(seq),
                LogicalChannelClass::Control,
                &shutdown,
                &outbound,
            )
            .await
            .expect("send must not fail on a writable sink");
            assert!(
                sent,
                "frame {seq} was abandoned even though the sink could accept it immediately"
            );
        }

        for seq in 0..FRAMES {
            let frame = framed_read
                .next()
                .await
                .expect("frame present")
                .expect("frame decodes cleanly");
            match frame.msg {
                Some(srui_message::Msg::ServerEventAck(ack)) => {
                    assert_eq!(ack.last_processed_event_seq, seq);
                }
                other => panic!("expected ServerEventAck, got {other:?}"),
            }
        }
    }

    /// The resume subscription exists before the resync snapshot is written, so it can overflow
    /// during that write. Clearing the marker unconditionally would erase that fresh overflow and
    /// downgrade the next resume to a journal replay (§20.2).
    #[test]
    fn test_stale_marker_survives_overflow_during_catch_up_write() {
        let session = Session::with_outbound_queue_capacity("stale-marker-race", 1);
        let client = vec![9u8];
        let outbound = session
            .subscribe_transactions(client.clone())
            .expect("subscribe outbound");

        overflow_client_queue(&session);
        assert!(session.outbound_hub.is_client_stale(&client));

        clear_stale_if_settled(&session, &client, &outbound);

        assert!(
            session.outbound_hub.is_client_stale(&client),
            "a queue that overflowed during the catch-up write must stay marked for resync"
        );
    }

    /// A catch-up write that completed on a healthy subscription must clear the marker, otherwise
    /// the client resyncs forever (§20.2).
    #[test]
    fn test_stale_marker_cleared_after_settled_catch_up_write() {
        let session = Session::with_outbound_queue_capacity("stale-marker-clear", 1);
        let client = vec![9u8];
        let stale = session
            .subscribe_transactions(client.clone())
            .expect("subscribe outbound");

        overflow_client_queue(&session);
        assert!(session.outbound_hub.is_client_stale(&client));
        drop(stale);

        let fresh = session
            .subscribe_transactions(client.clone())
            .expect("resubscribe outbound");
        clear_stale_if_settled(&session, &client, &fresh);

        assert!(
            !session.outbound_hub.is_client_stale(&client),
            "a settled catch-up write must clear the overflow marker"
        );
    }

    #[test]
    fn server_event_ack_is_control_class() {
        let ack = ack_envelope(3);
        assert_eq!(
            logical_class_for_server_envelope(&ack),
            Some(LogicalChannelClass::Control)
        );
        match ack.msg {
            Some(srui_message::Msg::ServerEventAck(_)) => {}
            other => panic!("fixture must stay an individual ServerEventAck, got {other:?}"),
        }
    }
}
