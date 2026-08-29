//! # Client Connection Handler
//!
//! Manages an attached client/bridge stream over Unix socket or SSH channel (§18, §20.2, §21).
//!
//! Every settled client event receives a `SERVER EVENT_ACK` on the same connection (§18.2), which
//! is control-class traffic (§19.2). An overlapping in-flight replay remains unacknowledged until
//! a retry can read the settled result. Validation refusals are acknowledged as `REJECTED` rather
//! than closing the stream; only protocol violations are fatal.
//!
//! Conforms strictly to:
//! - [`async-cancel-safety`](rules/async-cancel-safety.md): uses [`SruiCodec`] with `tokio_util::codec::FramedRead`
//!   inside `tokio::select!` so mid-frame cancellations do not corrupt stream buffers.
//! - [`async-bounded-channel`](rules/async-bounded-channel.md): all transaction and event flows use bounded queues.
//! - [`async-cancellation-token`](rules/async-cancellation-token.md): uses [`CancellationToken`] for clean disconnection.

use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::broadcast;
use tokio_util::codec::{FramedRead, FramedWrite};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::session::{EventOutcome, ResumeOutcome, Session, SessionError};
use srui_protocol::{
    srui_message, EventAckStatus, FramingError, ServerEventAck, SruiCodec, SruiMessage,
};
use thiserror::Error;

/// Handshake timeout in seconds (5 seconds, §18.1).
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

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

    let client_instance_id = match handshake_msg.msg {
        Some(srui_message::Msg::ClientHello(hello)) => {
            info!(
                "Received ClientHello from client instance {:?}",
                hello.client_instance_id
            );
            let welcome = session.handle_hello(&hello)?;
            let welcome_envelope = SruiMessage {
                msg: Some(srui_message::Msg::ServerWelcome(welcome)),
            };
            framed_write.send(welcome_envelope).await?;
            hello.client_instance_id
        }
        Some(srui_message::Msg::ClientResume(resume)) => {
            info!(
                "Received ClientResume for session {} from revision {}",
                resume.session_id, resume.last_applied_revision
            );
            match session.handle_resume(&resume)? {
                ResumeOutcome::Replay {
                    welcome_msg,
                    from_revision,
                } => {
                    let envelope = SruiMessage {
                        msg: Some(srui_message::Msg::ServerResumeOk(welcome_msg)),
                    };
                    framed_write.send(envelope).await?;
                    let replayed = session.collect_replayed_transactions(from_revision)?;
                    for tx in replayed {
                        let tx_env = SruiMessage {
                            msg: Some(srui_message::Msg::Transaction(tx)),
                        };
                        framed_write.send(tx_env).await?;
                    }
                }
                ResumeOutcome::Resync {
                    resync_msg,
                    snapshot_transaction,
                } => {
                    let envelope = SruiMessage {
                        msg: Some(srui_message::Msg::ServerResyncRequired(resync_msg)),
                    };
                    framed_write.send(envelope).await?;
                    let snapshot_env = SruiMessage {
                        msg: Some(srui_message::Msg::Transaction(snapshot_transaction)),
                    };
                    framed_write.send(snapshot_env).await?;
                }
            }
            resume.client_instance_id
        }
        _ => {
            return Err(ConnectionError::UnexpectedMessage(
                "expected ClientHello or ClientResume",
            ))
        }
    };

    // -------------------------------------------------------------------------
    // Phase 2: Multiplexed Event & Transaction Streaming (§18, §20)
    // -------------------------------------------------------------------------
    let mut tx_rx = session.subscribe_transactions()?;

    loop {
        tokio::select! {
            // Cancel-safe incoming message receiver (async-cancel-safety)
            incoming = framed_read.next() => {
                match incoming {
                    Some(Ok(msg)) => {
                        // Acks are control-class traffic (§19.2): emitted on the same connection,
                        // in order, never coalesced or dropped.
                        if let Some(response) =
                            handle_incoming_message(msg, &session, &client_instance_id).await?
                        {
                            framed_write.send(response).await?;
                        }
                    }
                    Some(Err(e)) => {
                        error!("Framing error on client stream: {}", e);
                        return Err(ConnectionError::Framing(e));
                    }
                    None => {
                        info!("Client disconnected normally");
                        break;
                    }
                }
            }

            // Outgoing broadcast transaction receiver (async-bounded-channel)
            broadcast_tx = tx_rx.recv() => {
                match broadcast_tx {
                    Ok(tx) => {
                        let envelope = SruiMessage {
                            msg: Some(srui_message::Msg::Transaction(tx)),
                        };
                        framed_write.send(envelope).await?;
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        warn!("Client lagged behind by {} transaction revisions; closing connection to force resync", skipped);
                        return Err(ConnectionError::Session(SessionError::LaggedResyncRequired));
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        break;
                    }
                }
            }

            // Clean shutdown signal (async-cancellation-token)
            _ = shutdown.cancelled() => {
                debug!("Connection loop terminating due to shutdown signal");
                break;
            }
        }
    }

    Ok(())
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
