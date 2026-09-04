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
//! the write future selects exactly one frame through [`LogicalChannelScheduler`]. Acks are
//! enqueued one-at-a-time on a bounded control channel (`send().await`) rather than dropped.
//!
//! Conforms strictly to:
//! - [`async-cancel-safety`](rules/async-cancel-safety.md): uses [`SruiCodec`] with `tokio_util::codec::FramedRead`
//!   inside `tokio::select!` so mid-frame cancellations do not corrupt stream buffers.
//! - [`async-bounded-channel`](rules/async-bounded-channel.md): all transaction, control, and event flows use bounded queues.
//! - [`async-cancellation-token`](rules/async-cancellation-token.md): uses a connection-local [`CancellationToken`] for clean disconnection.
//! - [`async-no-lock-await`](rules/async-no-lock-await.md): no locks are held across `.await`.

use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TryRecvError;
use tokio_util::codec::{FramedRead, FramedWrite};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::outbound::{
    logical_class_for_server_envelope, LogicalChannelClass, LogicalChannelScheduler, OutboundItem,
    OutboundReceiver, OutboundRecvError,
};
use crate::session::{EventOutcome, ResumeOutcome, Session, SessionError};
use srui_protocol::{
    srui_message, EventAckStatus, FramingError, ServerEventAck, SruiCodec, SruiMessage,
};
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
    debug_assert_eq!(
        logical_class_for_server_envelope(&envelope),
        logical_class,
        "envelope class must match the annotated write class"
    );
    debug!(?logical_class, "sending outbound frame");
    tokio::select! {
        biased;
        // Bounded: a client that stops reading its socket parks this write in the kernel, where
        // neither the shutdown token nor the outbound queue can observe it (§20.2).
        res = tokio::time::timeout(WRITE_TIMEOUT, framed_write.send(envelope)) => {
            match res {
                Ok(sent) => {
                    sent?;
                    Ok(true)
                }
                Err(_) => {
                    warn!(timeout = ?WRITE_TIMEOUT, "Client did not accept an outbound frame; detaching for resync");
                    Err(ConnectionError::WriteTimeout(WRITE_TIMEOUT))
                }
            }
        }
        _ = shutdown.cancelled() => Ok(false),
        _ = outbound.disconnect_token().cancelled() => match outbound.termination() {
            Some(OutboundRecvError::Closed) => Ok(false),
            Some(OutboundRecvError::Lagged(reason)) => {
                warn!(%reason, "Client outbound queue overflowed during send; closing connection to force resync");
                Err(ConnectionError::Session(SessionError::LaggedResyncRequired))
            }
            None => {
                warn!("Client outbound subscriber disconnected during send; closing connection to force resync");
                Err(ConnectionError::Session(SessionError::LaggedResyncRequired))
            }
        },
    }
}

fn outbound_item_to_message(item: OutboundItem) -> SruiMessage {
    match item {
        OutboundItem::Transaction(tx) => SruiMessage {
            msg: Some(srui_message::Msg::Transaction(tx)),
        },
        OutboundItem::ResourceMetadata(meta) => SruiMessage {
            msg: Some(srui_message::Msg::ResourceMetadata(meta)),
        },
        OutboundItem::ResourceChunk(chunk) => SruiMessage {
            msg: Some(srui_message::Msg::ResourceChunk(chunk)),
        },
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

    let (client_instance_id, tx_rx) = match handshake_msg.msg {
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
            (hello.client_instance_id, bootstrap.transactions)
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
            (resume.client_instance_id, bootstrap.transactions)
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
        shutdown,
    )
    .await
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
    shutdown: CancellationToken,
) -> Result<(), ConnectionError>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let session_cancel = CancellationToken::new();
    let (control_tx, control_rx) = mpsc::channel(CONTROL_CHANNEL_CAPACITY);

    let read = read_loop(
        framed_read,
        session,
        client_instance_id,
        control_tx,
        shutdown.clone(),
        session_cancel.clone(),
    );
    let write = write_loop(
        framed_write,
        outbound,
        control_rx,
        shutdown,
        session_cancel.clone(),
    );

    tokio::pin!(read);
    tokio::pin!(write);
    tokio::select! {
        biased;
        result = &mut read => {
            match result {
                // The completed reader has dropped control_tx. Drain queued acknowledgements
                // rather than cancelling the writer on a clean inbound half-close.
                Ok(()) => write.await,
                Err(error) => {
                    session_cancel.cancel();
                    Err(error)
                }
            }
        }
        result = &mut write => {
            session_cancel.cancel();
            result
        }
    }
}

async fn read_loop<R>(
    mut framed_read: FramedRead<R, SruiCodec>,
    session: Arc<Session>,
    client_instance_id: Vec<u8>,
    control_tx: mpsc::Sender<SruiMessage>,
    shutdown: CancellationToken,
    session_cancel: CancellationToken,
) -> Result<(), ConnectionError>
where
    R: AsyncRead + Unpin,
{
    loop {
        tokio::select! {
            biased;
            _ = session_cancel.cancelled() => return Ok(()),
            _ = shutdown.cancelled() => {
                debug!("Connection read loop terminating due to shutdown signal");
                return Ok(());
            }
            incoming = framed_read.next() => {
                match incoming {
                    Some(Ok(msg)) => {
                        if let Some(response) =
                            handle_incoming_message(msg, &session, &client_instance_id).await?
                        {
                            debug_assert_eq!(
                                logical_class_for_server_envelope(&response),
                                LogicalChannelClass::Control,
                                "SERVER EVENT_ACK must remain control-class"
                            );
                            tokio::select! {
                                biased;
                                sent = control_tx.send(response) => {
                                    if sent.is_err() {
                                        return Ok(());
                                    }
                                }
                                _ = session_cancel.cancelled() => return Ok(()),
                                _ = shutdown.cancelled() => return Ok(()),
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

async fn write_loop<W>(
    mut framed_write: FramedWrite<W, SruiCodec>,
    mut outbound: OutboundReceiver,
    mut control_rx: mpsc::Receiver<SruiMessage>,
    shutdown: CancellationToken,
    session_cancel: CancellationToken,
) -> Result<(), ConnectionError>
where
    W: AsyncWrite + Unpin,
{
    let mut scheduler = LogicalChannelScheduler::new();
    let mut pending_control: Option<SruiMessage> = None;
    let mut control_closed = false;
    let mut outbound_idle = false;

    loop {
        if session_cancel.is_cancelled() || shutdown.is_cancelled() {
            return Ok(());
        }
        match outbound.termination() {
            Some(OutboundRecvError::Lagged(reason)) => {
                warn!(%reason, "Client outbound queue overflowed; closing connection to force resync");
                return Err(ConnectionError::Session(SessionError::LaggedResyncRequired));
            }
            Some(OutboundRecvError::Closed) | None => {}
        }

        if pending_control.is_none() && !control_closed {
            match control_rx.try_recv() {
                Ok(msg) => pending_control = Some(msg),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => control_closed = true,
            }
        }

        // A closed control channel means the cleanly completed reader cannot produce more
        // acknowledgements. Once its bounded queue is empty, finish without streaming unrelated
        // low-priority output forever to a client that has ended its input side.
        if control_closed && pending_control.is_none() {
            return Ok(());
        }

        let ready = |class: LogicalChannelClass| match class {
            LogicalChannelClass::Control => pending_control.is_some(),
            LogicalChannelClass::Input
            | LogicalChannelClass::TerminalHigh
            | LogicalChannelClass::TerminalNormal => false,
            LogicalChannelClass::Ui => outbound.class_ready(LogicalChannelClass::Ui),
            LogicalChannelClass::Resource => outbound.class_ready(LogicalChannelClass::Resource),
        };

        if let Some(class) = scheduler.select_next(ready) {
            let envelope = match class {
                LogicalChannelClass::Control => pending_control
                    .take()
                    .expect("control selected only when a frame is pending"),
                LogicalChannelClass::Ui | LogicalChannelClass::Resource => {
                    match outbound.pop_class(class) {
                        Ok(Some(item)) => outbound_item_to_message(item),
                        Ok(None) => continue,
                        Err(OutboundRecvError::Lagged(reason)) => {
                            warn!(%reason, "Client outbound queue overflowed; closing connection to force resync");
                            return Err(ConnectionError::Session(
                                SessionError::LaggedResyncRequired,
                            ));
                        }
                        Err(OutboundRecvError::Closed) => {
                            outbound_idle = true;
                            continue;
                        }
                    }
                }
                LogicalChannelClass::Input
                | LogicalChannelClass::TerminalHigh
                | LogicalChannelClass::TerminalNormal => continue,
            };
            debug_assert_eq!(logical_class_for_server_envelope(&envelope), class);
            if !send_message(&mut framed_write, envelope, class, &shutdown, &outbound).await? {
                return Ok(());
            }
            if class == LogicalChannelClass::Resource {
                tokio::task::yield_now().await;
            }
            continue;
        }

        if pending_control.is_none()
            && (outbound_idle || outbound.is_closed())
            && !outbound.class_ready(LogicalChannelClass::Ui)
            && !outbound.class_ready(LogicalChannelClass::Resource)
        {
            // Hub close ends the writer even while the read future is still attached; the
            // connection-local token then cancels the reader (§20.2).
            return Ok(());
        }

        let disconnect = outbound.disconnect_token().clone();
        tokio::select! {
            biased;
            _ = session_cancel.cancelled() => return Ok(()),
            _ = shutdown.cancelled() => return Ok(()),
            _ = disconnect.cancelled(), if !outbound_idle => match outbound.termination() {
                Some(OutboundRecvError::Lagged(reason)) => {
                    warn!(%reason, "Client outbound queue overflowed; closing connection to force resync");
                    return Err(ConnectionError::Session(SessionError::LaggedResyncRequired));
                }
                Some(OutboundRecvError::Closed) => {
                    outbound_idle = !outbound.class_ready(LogicalChannelClass::Ui)
                        && !outbound.class_ready(LogicalChannelClass::Resource);
                }
                None => {
                    warn!("Client outbound subscriber disconnected during wait; closing connection to force resync");
                    return Err(ConnectionError::Session(SessionError::LaggedResyncRequired));
                }
            },
            msg = control_rx.recv(), if pending_control.is_none() && !control_closed => {
                match msg {
                    Some(message) => pending_control = Some(message),
                    None => control_closed = true,
                }
            }
            result = outbound.wait_for_work(), if !outbound_idle => {
                match result {
                    Ok(()) => tokio::task::yield_now().await,
                    Err(OutboundRecvError::Lagged(reason)) => {
                        warn!(%reason, "Client outbound queue overflowed; closing connection to force resync");
                        return Err(ConnectionError::Session(SessionError::LaggedResyncRequired));
                    }
                    Err(OutboundRecvError::Closed) => {
                        outbound_idle = true;
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
) -> Result<Option<SruiMessage>, ConnectionError> {
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
                    debug!("Handled event {:?}", event.event_id);
                }
                EventOutcome::Pending { .. } => {
                    debug!(
                        "Event {:?} (seq {}) is already in flight",
                        event.event_id, event.event_seq
                    );
                }
                EventOutcome::Duplicate { .. } => {
                    debug!(
                        "Ignored duplicate event {:?} (seq {})",
                        event.event_id, event.event_seq
                    );
                }
                EventOutcome::Rejected { error, .. } => {
                    // Not connection-fatal: a rejected event that closed the stream would be
                    // replayed on the next resume and close it again, forever (§18, §18.2).
                    warn!(
                        "Rejecting event {:?} (seq {}): {}",
                        event.event_id, event.event_seq, error
                    );
                }
            }
            Ok(build_event_ack(
                &event,
                &outcome,
                session.max_string_length(),
                session.session_id(),
            )
            .map(|ack| SruiMessage {
                msg: Some(srui_message::Msg::ServerEventAck(ack)),
            }))
        }
        Some(srui_message::Msg::Transaction(tx)) => {
            warn!(
                "Rejecting client-originated transaction rev {} -> {}; remote authority forbids client commits",
                tx.base_revision, tx.new_revision
            );
            Err(ConnectionError::ClientTransactionRejected)
        }
        Some(srui_message::Msg::ClientHello(_)) => Err(ConnectionError::UnexpectedMessage(
            "ClientHello is valid only during handshake",
        )),
        Some(srui_message::Msg::ClientResume(_)) => Err(ConnectionError::UnexpectedMessage(
            "ClientResume is valid only during handshake",
        )),
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
        event_id: event.event_id.clone(),
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
            LogicalChannelClass::Control
        );
        match ack.msg {
            Some(srui_message::Msg::ServerEventAck(_)) => {}
            other => panic!("fixture must stay an individual ServerEventAck, got {other:?}"),
        }
    }
}
