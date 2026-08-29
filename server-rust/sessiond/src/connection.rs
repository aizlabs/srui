//! # Client Connection Handler
//!
//! Manages an attached client/bridge stream over Unix socket or SSH channel (§18, §20.2, §21).
//!
//! Conforms strictly to:
//! - [`async-cancel-safety`](rules/async-cancel-safety.md): uses [`SruiCodec`] with `tokio_util::codec::FramedRead`
//!   inside `tokio::select!` so mid-frame cancellations do not corrupt stream buffers.
//! - [`async-bounded-channel`](rules/async-bounded-channel.md): all transaction and event flows use bounded queues.
//! - [`async-cancellation-token`](rules/async-cancellation-token.md): uses [`CancellationToken`] for clean disconnection.

use std::sync::Arc;
use std::time::Duration;
use futures::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::broadcast;
use tokio_util::codec::{FramedRead, FramedWrite};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use srui_protocol::{
    srui_message, FramingError, SruiCodec, SruiMessage,
};
use crate::session::{ResumeOutcome, Session, SessionError};
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

    #[error("unexpected message during handshake: {0}")]
    UnexpectedMessage(&'static str),

    #[error("client-originated transaction rejected: server is authoritative (§12, §20.2)")]
    ClientTransactionRejected,
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

    match handshake_msg.msg {
        Some(srui_message::Msg::ClientHello(hello)) => {
            info!("Received ClientHello from client instance {:?}", hello.client_instance_id);
            let welcome = session.handle_hello(&hello)?;
            let welcome_envelope = SruiMessage {
                msg: Some(srui_message::Msg::ServerWelcome(welcome)),
            };
            framed_write.send(welcome_envelope).await?;
        }
        Some(srui_message::Msg::ClientResume(resume)) => {
            info!("Received ClientResume for session {} from revision {}", resume.session_id, resume.last_applied_revision);
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
        }
        _ => return Err(ConnectionError::UnexpectedMessage("expected ClientHello or ClientResume")),
    }

    // -------------------------------------------------------------------------
    // Phase 2: Multiplexed Event & Transaction Streaming (§18, §20)
    // -------------------------------------------------------------------------
    let mut tx_rx = session.subscribe_transactions();

    loop {
        tokio::select! {
            // Cancel-safe incoming message receiver (async-cancel-safety)
            incoming = framed_read.next() => {
                match incoming {
                    Some(Ok(msg)) => {
                        handle_incoming_message(msg, &session).await?;
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

async fn handle_incoming_message(msg: SruiMessage, session: &Session) -> Result<(), ConnectionError> {
    match msg.msg {
        Some(srui_message::Msg::Event(event)) => {
            debug!("Processing incoming event {:?}", event.event_id);
            let _ = session.process_event(&event)?;
        }
        Some(srui_message::Msg::Transaction(tx)) => {
            warn!(
                "Rejecting client-originated transaction rev {} -> {}; remote authority forbids client commits",
                tx.base_revision, tx.new_revision
            );
            return Err(ConnectionError::ClientTransactionRejected);
        }
        Some(other) => {
            debug!("Ignoring unhandled message during active stream: {:?}", other);
        }
        None => {}
    }
    Ok(())
}
