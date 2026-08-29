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

fn parse_socket_path() -> Result<PathBuf, String> {
    let args: Vec<String> = std::env::args().collect();
    if let Some(pos) = args.iter().position(|a| a == "--socket") {
        match args.get(pos + 1) {
            Some(path_str) if !path_str.starts_with('-') => Ok(PathBuf::from(path_str)),
            _ => Err("--socket requires a path argument".into()),
        }
    } else if let Some(first_arg) = args.get(1) {
        if !first_arg.starts_with('-') {
            Ok(PathBuf::from(first_arg))
        } else {
            Ok(default_socket_path())
        }
    } else {
        Ok(default_socket_path())
    }
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

    info!("Starting srui-ssh-bridge proxy (§19, §19.1, §20.1)...");

    let socket_path = parse_socket_path().map_err(|message| {
        error!("{message}");
        message
    })?;

    info!("Connecting to session daemon socket at {:?}", socket_path);
    let session_stream = UnixStream::connect(&socket_path).await.map_err(|e| {
        error!("Failed to connect to srui-sessiond socket: {}", e);
        e
    })?;

    let shutdown = CancellationToken::new();
    let shutdown_signal = shutdown.clone();
    tokio::spawn(async move {
        wait_for_shutdown_signal().await;
        shutdown_signal.cancel();
    });

    let stdio = tokio::io::join(tokio::io::stdin(), tokio::io::stdout());
    bridge_streams(stdio, session_stream, shutdown).await?;

    info!("srui-ssh-bridge exited cleanly.");
    Ok(())
}

#[cfg(unix)]
async fn wait_for_shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    use tracing::info;

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
