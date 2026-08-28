//! # SRUI SSH Bridge Library
//!
//! Ephemeral forwarding bridge between SSH standard I/O and `srui-sessiond` Unix socket (§20.1).
//!
//! Conforms strictly to:
//! - [`async-cancel-safety`](rules/async-cancel-safety.md): bidirectional raw-byte forwarding.
//! - [`async-tokio-runtime`](rules/async-tokio-runtime.md): designed for `current_thread` single-task efficiency.
//! - [`async-cancellation-token`](rules/async-cancellation-token.md): cleanly detaches upon SSH session termination.

use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info};
use thiserror::Error;

/// Errors returned by the bridge forwarder.
#[derive(Debug, Error)]
pub enum BridgeError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// Bridges an SSH channel stream with a session daemon stream bi-directionally.
///
/// Forwards length-prefixed wire bytes verbatim without decode/re-encode so the bridge remains
/// a transparent transport proxy (§16, §20.1).
pub async fn bridge_streams<S1, S2>(
    stream1: S1,
    stream2: S2,
    shutdown: CancellationToken,
) -> Result<(), BridgeError>
where
    S1: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    S2: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let mut stream1 = stream1;
    let mut stream2 = stream2;

    info!("SSH bridge raw-byte forwarder active");

    tokio::select! {
        res = tokio::io::copy_bidirectional(&mut stream1, &mut stream2) => {
            res?;
        }
        _ = shutdown.cancelled() => {
            debug!("Bridge forwarder terminating on shutdown signal");
        }
    }

    Ok(())
}
