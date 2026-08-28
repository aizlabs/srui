//! # SRUI SSH Bridge Library
//!
//! Ephemeral forwarding bridge between SSH standard I/O and `srui-sessiond` Unix socket (§20.1).
//!
//! Conforms strictly to:
//! - [`async-cancel-safety`](rules/async-cancel-safety.md): cancel-safe bi-directional streaming using [`SruiCodec`].
//! - [`async-tokio-runtime`](rules/async-tokio-runtime.md): designed for `current_thread` single-task efficiency.
//! - [`async-cancellation-token`](rules/async-cancellation-token.md): cleanly detaches upon SSH session termination.

use futures::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::codec::{FramedRead, FramedWrite};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info};

use srui_protocol::{FramingError, SruiCodec};
use thiserror::Error;

/// Errors returned by the bridge forwarder.
#[derive(Debug, Error)]
pub enum BridgeError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("framing error: {0}")]
    Framing(#[from] FramingError),

    #[error("SSH stream closed")]
    SshStreamClosed,

    #[error("session daemon stream closed")]
    SessionStreamClosed,
}

/// Bridges an SSH channel stream with a session daemon stream bi-directionally.
pub async fn bridge_streams<S1, S2>(
    ssh_stream: S1,
    session_stream: S2,
    shutdown: CancellationToken,
) -> Result<(), BridgeError>
where
    S1: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    S2: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (ssh_read, ssh_write) = tokio::io::split(ssh_stream);
    let (session_read, session_write) = tokio::io::split(session_stream);

    let mut ssh_framed_read = FramedRead::new(ssh_read, SruiCodec::new());
    let mut ssh_framed_write = FramedWrite::new(ssh_write, SruiCodec::new());

    let mut session_framed_read = FramedRead::new(session_read, SruiCodec::new());
    let mut session_framed_write = FramedWrite::new(session_write, SruiCodec::new());

    info!("SSH bridge forwarder active");

    loop {
        tokio::select! {
            // Forward from SSH client -> sessiond (cancel-safe)
            from_ssh = ssh_framed_read.next() => {
                match from_ssh {
                    Some(Ok(msg)) => {
                        session_framed_write.send(msg).await?;
                    }
                    Some(Err(e)) => {
                        error!("Framing error from SSH client: {}", e);
                        return Err(BridgeError::Framing(e));
                    }
                    None => {
                        info!("SSH client disconnected; exiting bridge (§20.1)");
                        break;
                    }
                }
            }

            // Forward from sessiond -> SSH client (cancel-safe)
            from_session = session_framed_read.next() => {
                match from_session {
                    Some(Ok(msg)) => {
                        ssh_framed_write.send(msg).await?;
                    }
                    Some(Err(e)) => {
                        error!("Framing error from session daemon: {}", e);
                        return Err(BridgeError::Framing(e));
                    }
                    None => {
                        info!("Session daemon connection closed; exiting bridge");
                        break;
                    }
                }
            }

            // Clean shutdown signal (e.g. SIGINT / SIGTERM)
            _ = shutdown.cancelled() => {
                debug!("Bridge forwarder terminating on shutdown signal");
                break;
            }
        }
    }

    Ok(())
}
