//! Runnable Task 31 coding-agent example over a real Unix transport (§11.1, §20.2, §30).

use std::path::{Path, PathBuf};

use srui_example_coding_agent::{CodingAgentApp, APPROVE_ID};
use srui_sessiond::handle_connection;
use tokio::net::UnixListener;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

struct OwnedSocket {
    path: PathBuf,
    identity: (u64, u64),
}

impl Drop for OwnedSocket {
    fn drop(&mut self) {
        if socket_identity(&self.path).ok().flatten() == Some(self.identity) {
            if let Err(error) = std::fs::remove_file(&self.path) {
                warn!("failed to remove {}: {error}", self.path.display());
            }
        }
    }
}

fn socket_identity(path: &Path) -> std::io::Result<Option<(u64, u64)>> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};

    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_socket() => {
            Ok(Some((metadata.dev(), metadata.ino())))
        }
        Ok(_) => Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("{} exists and is not a Unix socket", path.display()),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn bind_owned_socket(path: &Path) -> std::io::Result<(UnixListener, OwnedSocket)> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if socket_identity(path)?.is_some() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AddrInUse,
            format!(
                "{} already exists; pass a different --socket",
                path.display()
            ),
        ));
    }
    let listener = UnixListener::bind(path)?;
    let identity = socket_identity(path)?.ok_or_else(|| {
        std::io::Error::other(format!(
            "{} vanished immediately after bind",
            path.display()
        ))
    })?;
    Ok((
        listener,
        OwnedSocket {
            path: path.to_path_buf(),
            identity,
        },
    ))
}

async fn run_server(path: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    let app = CodingAgentApp::new()?;
    let session = app.session_arc();
    let (listener, _owned_socket) = bind_owned_socket(&path)?;
    let shutdown = CancellationToken::new();
    let mut connections = JoinSet::new();
    info!(revision = app.current_revision(), socket = %path.display(), "coding-agent demo ready");

    loop {
        tokio::select! {
            result = listener.accept() => {
                let (stream, _) = result?;
                let connection_session = Arc::clone(&session);
                let connection_shutdown = shutdown.child_token();
                connections.spawn(async move {
                    if let Err(error) = handle_connection(
                        stream,
                        connection_session,
                        connection_shutdown,
                    ).await {
                        warn!("client connection ended: {error}");
                    }
                });
            }
            _ = tokio::signal::ctrl_c() => break,
        }
    }

    shutdown.cancel();
    while connections.join_next().await.is_some() {}
    session.pty().shutdown();
    Ok(())
}

use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [flag, path] if flag == "--socket" => run_server(PathBuf::from(path)).await,
        [] => {
            let app = CodingAgentApp::new()?;
            println!("Initial revision: {}", app.current_revision());
            println!("Conversation: {}", app.conversation());
            app.activate(APPROVE_ID, 1)?;
            println!("After approval: revision {}", app.current_revision());
            app.edit_prompt(2, 1, "Please add tests for expired tokens")?;
            println!("After prompt edit: revision {}", app.current_revision());
            println!("Prompt: {}", app.prompt());
            app.session().pty().shutdown();
            Ok(())
        }
        _ => Err("usage: coding-agent-demo [--socket <path>]".into()),
    }
}
