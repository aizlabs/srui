//! # SRUI SSH Bridge Library
//!
//! Ephemeral forwarding bridge between SSH standard I/O and `srui-sessiond` Unix socket (§20.1).
//!
//! Conforms strictly to:
//! - [`async-cancel-safety`](rules/async-cancel-safety.md): bidirectional raw-byte forwarding.
//! - [`async-tokio-runtime`](rules/async-tokio-runtime.md): designed for `current_thread` single-task efficiency.
//! - [`async-cancellation-token`](rules/async-cancellation-token.md): cleanly detaches upon SSH session termination.

use thiserror::Error;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info};

/// Errors returned by the bridge forwarder.
#[derive(Debug, Error)]
pub enum BridgeError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Bridges an SSH channel stream (`stream1`) with a session daemon stream (`stream2`)
/// bi-directionally.
///
/// Forwards length-prefixed wire bytes verbatim without decode/re-encode so the bridge remains
/// a transparent transport proxy (§16, §20.1).
///
/// The two directions end differently. When the SSH side reaches EOF, the daemon is told
/// (its write half is shut down) and whatever it still sends is forwarded until it closes too.
/// When the daemon side reaches EOF, the session's transport is over: the daemon has stopped or
/// dropped this connection and will accept nothing more, so the bridge returns at once rather
/// than wait for the SSH side to close. Its process then exits and the SSH channel closes, which
/// is how the client learns the connection is gone (§17 DETACHED, §18). Waiting for both
/// directions instead kept an idle client "connected" to a daemon that no longer existed: an
/// SSH channel's standard output is not closed by shutting down its write half (PX-022-G02).
pub async fn bridge_streams<S1, S2>(
    stream1: S1,
    stream2: S2,
    shutdown: CancellationToken,
) -> Result<(), BridgeError>
where
    S1: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    S2: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut ssh_read, mut ssh_write) = tokio::io::split(stream1);
    let (mut daemon_read, mut daemon_write) = tokio::io::split(stream2);

    info!("SSH bridge raw-byte forwarder active");

    let to_daemon = async {
        tokio::io::copy(&mut ssh_read, &mut daemon_write).await?;
        daemon_write.shutdown().await
    };
    let to_ssh = async {
        tokio::io::copy(&mut daemon_read, &mut ssh_write).await?;
        ssh_write.flush().await
    };
    tokio::pin!(to_daemon, to_ssh);

    tokio::select! {
        res = &mut to_ssh => {
            res?;
            debug!("Session daemon closed the connection; ending the bridge");
        }
        res = &mut to_daemon => {
            res?;
            tokio::select! {
                res = &mut to_ssh => { res?; }
                _ = shutdown.cancelled() => {
                    debug!("Bridge forwarder terminating on shutdown signal");
                }
            }
        }
        _ = shutdown.cancelled() => {
            debug!("Bridge forwarder terminating on shutdown signal");
        }
    }

    Ok(())
}
