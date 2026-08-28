//! # `srui-sessiond` Daemon Binary
//!
//! Per-user persistent session daemon (§20.2).
//! Manages durable UI state across transient SSH bridge connections.

use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::UnixListener;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use srui_sessiond::{handle_connection, Session};

fn default_socket_path() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("srui-sessiond.sock")
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    info!("Starting srui-sessiond daemon (§20.2)...");

    let socket_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(default_socket_path);

    if socket_path.exists() {
        let _ = std::fs::remove_file(&socket_path);
    }

    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let listener = UnixListener::bind(&socket_path)?;
    info!("Listening on Unix domain socket: {:?}", socket_path);

    let session = Arc::new(Session::new("default"));
    let shutdown = CancellationToken::new();
    let mut tasks = JoinSet::new();

    // Listen for shutdown signals (SIGINT, SIGTERM, SIGHUP on Unix)
    let shutdown_signal = shutdown.clone();
    tokio::spawn(async move {
        wait_for_shutdown_signal().await;
        info!("Received shutdown signal; draining connections...");
        shutdown_signal.cancel();
    });

    loop {
        tokio::select! {
            Some(res) = tasks.join_next(), if !tasks.is_empty() => {
                if let Err(e) = res {
                    error!("Connection task panicked: {}", e);
                }
            }
            accept_result = listener.accept() => {
                match accept_result {
                    Ok((stream, _peer_addr)) => {
                        let session_clone = session.clone();
                        let shutdown_child = shutdown.child_token();
                        tasks.spawn(async move {
                            if let Err(e) = handle_connection(stream, session_clone, shutdown_child).await {
                                warn!("Connection ended with error: {}", e);
                            }
                        });
                    }
                    Err(e) => {
                        error!("Failed to accept Unix socket connection: {}", e);
                    }
                }
            }
            _ = shutdown.cancelled() => {
                info!("Stopping listener; waiting for active connections to finish...");
                break;
            }
        }
    }

    // Await all connection tasks
    while let Some(res) = tasks.join_next().await {
        if let Err(e) = res {
            error!("Connection task panicked: {}", e);
        }
    }

    // Clean up socket file
    let _ = std::fs::remove_file(&socket_path);
    info!("srui-sessiond daemon shutdown complete.");
    Ok(())
}

#[cfg(unix)]
async fn wait_for_shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};

    let mut sigint = signal(SignalKind::interrupt()).expect("failed to install SIGINT handler");
    let mut sigterm = signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");
    let mut sighup = signal(SignalKind::hangup()).expect("failed to install SIGHUP handler");

    tokio::select! {
        _ = sigint.recv() => { info!("Received SIGINT (Ctrl+C)"); }
        _ = sigterm.recv() => { info!("Received SIGTERM"); }
        _ = sighup.recv() => { info!("Received SIGHUP (SSH session detach)"); }
    }
}

#[cfg(not(unix))]
async fn wait_for_shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
