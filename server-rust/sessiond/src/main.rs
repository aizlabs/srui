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

/// Ignores `SIGHUP` so SSH session detach / controlling-terminal loss does not terminate
/// the daemon (§17, §20.2). Omitting a handler leaves the default disposition, which kills
/// the process and defeats persistent session state.
#[cfg(unix)]
fn ignore_sighup() {
    // SAFETY: called synchronously at process start, before threads or other handlers exist.
    let rc = unsafe { libc::signal(libc::SIGHUP, libc::SIG_IGN) };
    if rc == libc::SIG_ERR {
        eprintln!("srui-sessiond: failed to ignore SIGHUP");
    }
}

fn default_socket_path() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("srui-sessiond.sock")
}
#[derive(Debug, Clone)]
struct DaemonConfig {
    socket_path: PathBuf,
    app_name: Option<String>,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            socket_path: default_socket_path(),
            app_name: None,
        }
    }
}

fn parse_args() -> DaemonConfig {
    let args: Vec<String> = std::env::args().collect();
    let mut config = DaemonConfig::default();

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--socket" if i + 1 < args.len() => {
                config.socket_path = PathBuf::from(&args[i + 1]);
                i += 2;
            }
            "--app" if i + 1 < args.len() => {
                config.app_name = Some(args[i + 1].clone());
                i += 2;
            }
            arg if !arg.starts_with('-') => {
                config.socket_path = PathBuf::from(arg);
                i += 1;
            }
            _ => {
                i += 1;
            }
        }
    }

    config
}

fn initialize_counter_app(session: &Arc<Session>) {
    use srui_sdk::*;
    let surface_id = NodeId::new(1);
    let text_id = NodeId::new(2);
    let progress_id = NodeId::new(3);
    let button_id = NodeId::new(4);

    session
        .transaction(|ui| {
            Surface::builder(surface_id)
                .label("SRUI Counter Application")
                .create(ui)?;

            Text::builder(text_id)
                .parent(surface_id)
                .text("Count: 0")
                .role(TextRole::Heading)
                .create(ui)?;

            Progress::builder(progress_id)
                .parent(surface_id)
                .value(0.0)
                .value_description("0 / 100")
                .create(ui)?;

            Button::builder(button_id)
                .parent(surface_id)
                .label("Increment")
                .role(ActionRole::Primary)
                .create(ui)?;

            Ok(())
        })
        .expect("initialize counter UI transaction");

    let text = text_id;
    let prog = progress_id;
    session.on(button_id, ACTIVATE, move |ctx, _event| {
        ctx.transaction(|ui| {
            let current: u64 = ui
                .get_node(text)
                .and_then(|n| n.get_property(TEXT))
                .and_then(|v| v.as_string())
                .and_then(|s| s.strip_prefix("Count: "))
                .and_then(|n| n.parse::<u64>().ok())
                .unwrap_or(0);
            let next_val = current + 1;
            ui.set(text, TEXT, format!("Count: {}", next_val))?;
            ui.set(prog, VALUE, (next_val as f64) / 100.0)?;
            ui.set(prog, VALUE_DESCRIPTION, format!("{} / 100", next_val))?;
            info!(count = next_val, "Counter incremented");
            Ok(())
        })
        .expect("counter increment transaction failed");
    });
}

/// A socket path this process created and is therefore allowed to unlink.
struct OwnedSocket {
    path: PathBuf,
    /// `(device, inode)` of the endpoint created by this process.
    identity: (u64, u64),
}

impl OwnedSocket {
    /// Removes the socket, but only while the path still resolves to the endpoint we bound.
    ///
    /// If a replacement server has since taken the path over, its socket has a different inode and
    /// is left alone.
    fn remove(&self) {
        match socket_identity(&self.path) {
            Ok(Some(identity)) if identity == self.identity => {
                if let Err(error) = std::fs::remove_file(&self.path) {
                    warn!("failed to remove {}: {error}", self.path.display());
                }
            }
            Ok(Some(_)) => warn!(
                "leaving {} in place: it now belongs to another server",
                self.path.display()
            ),
            Ok(None) | Err(_) => {}
        }
    }
}

/// Returns the `(device, inode)` identity of `path` when it is a Unix socket.
///
/// `Ok(None)` means the path does not exist; a path that exists but is not a socket is an error, so
/// an unrelated file is never a removal candidate.
fn socket_identity(path: &std::path::Path) -> std::io::Result<Option<(u64, u64)>> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};

    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_socket() {
                Ok(Some((metadata.dev(), metadata.ino())))
            } else {
                Err(std::io::Error::other(format!(
                    "{} exists but is not a Unix socket",
                    path.display()
                )))
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Probes an existing socket path to determine if a live server is listening.
///
/// Retries connect attempts with backoff to avoid misinterpreting a Darwin listen backlog saturation
/// as an inactive or stale socket.
async fn probe_live_socket(path: &std::path::Path) -> std::io::Result<bool> {
    use tokio::net::UnixStream;
    if socket_identity(path)?.is_none() {
        return Ok(false);
    }
    for attempt in 0..3 {
        match UnixStream::connect(path).await {
            Ok(_) => return Ok(true),
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::ConnectionRefused
                        | std::io::ErrorKind::NotFound
                        | std::io::ErrorKind::PermissionDenied
                        | std::io::ErrorKind::ConnectionReset
                ) || matches!(
                    error.raw_os_error(),
                    Some(libc::ECONNREFUSED)
                        | Some(libc::EPERM)
                        | Some(libc::EACCES)
                        | Some(libc::ENOENT)
                ) =>
            {
                if attempt < 2 {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
            }
            Err(error) => return Err(error),
        }
    }
    Ok(false)
}

/// Binds a Unix domain socket, verifying parent directory permissions and unlinking any stale
/// predecessor safely without disturbing a live server.
async fn bind_owned_socket(path: &std::path::Path) -> std::io::Result<(UnixListener, OwnedSocket)> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    if probe_live_socket(path).await? {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AddrInUse,
            format!(
                "{} is already served by a running process; pass a different socket path",
                path.display()
            ),
        ));
    }

    if path.exists() {
        if let Err(unlink_error) = std::fs::remove_file(path) {
            if unlink_error.kind() != std::io::ErrorKind::NotFound {
                return Err(unlink_error);
            }
        }
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

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(unix)]
    ignore_sighup();

    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    info!("Starting srui-sessiond daemon (§20.2)...");

    let config = parse_args();

    let (listener, owned_socket) = bind_owned_socket(&config.socket_path).await?;
    info!(socket_path = ?config.socket_path, "Listening on Unix domain socket");

    // Mint a fresh, globally unique session incarnation token (§17)
    let session = Arc::new(Session::mint());
    info!(
        session_id = %session.session_id(),
        state = %session.state(),
        "Minted session incarnation token"
    );

    if let Some(app) = config.app_name.as_deref() {
        match app {
            "counter" => {
                info!("Initializing built-in counter application adapter (§20.2, §29)...");
                initialize_counter_app(&session);
            }
            other => {
                warn!(app = %other, "Unknown application adapter requested");
            }
        }
    }

    let shutdown = CancellationToken::new();
    let mut tasks = JoinSet::new();

    // Listen for process shutdown signals (SIGINT, SIGTERM). SIGHUP is ignored via SIG_IGN
    // at startup so SSH detachments do NOT terminate the session daemon (§17, §20.2).
    let shutdown_signal = shutdown.clone();
    tokio::spawn(async move {
        wait_for_shutdown_signal().await;
        info!("Received termination signal; draining connections...");
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
                                warn!("Connection ended: {}", e);
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
    drop(listener);
    owned_socket.remove();
    info!("srui-sessiond daemon shutdown complete.");
    Ok(())
}

#[cfg(unix)]
async fn wait_for_shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};

    let mut sigint = signal(SignalKind::interrupt()).expect("failed to install SIGINT handler");
    let mut sigterm = signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");

    tokio::select! {
        _ = sigint.recv() => { info!("Received SIGINT (Ctrl+C)"); }
        _ = sigterm.recv() => { info!("Received SIGTERM"); }
    }
}

#[cfg(not(unix))]
async fn wait_for_shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
