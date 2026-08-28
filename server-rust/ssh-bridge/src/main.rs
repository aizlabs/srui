//! # `srui-ssh-bridge` Binary
//!
//! Ephemeral proxy invoked by SSH subsystem to forward standard I/O to `srui-sessiond` (§20.1).

use std::path::PathBuf;
use tokio::net::UnixStream;
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

use srui_ssh_bridge::bridge_streams;

fn default_socket_path() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("srui-sessiond.sock")
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr) // Crucial: write diagnostics to stderr so stdout remains binary protocol stream (§19.1)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    info!("Starting srui-ssh-bridge proxy (§20.1)...");

    let socket_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(default_socket_path);

    info!("Connecting to session daemon socket at {:?}", socket_path);
    let session_stream = UnixStream::connect(&socket_path).await.map_err(|e| {
        error!("Failed to connect to srui-sessiond socket: {}", e);
        e
    })?;

    let shutdown = CancellationToken::new();
    let shutdown_signal = shutdown.clone();
    tokio::spawn(async move {
        let _ = tokio::signal::ctrl_c().await;
        shutdown_signal.cancel();
    });

    let stdio = tokio::io::join(tokio::io::stdin(), tokio::io::stdout());
    bridge_streams(stdio, session_stream, shutdown).await?;

    info!("srui-ssh-bridge exited cleanly.");
    Ok(())
}
