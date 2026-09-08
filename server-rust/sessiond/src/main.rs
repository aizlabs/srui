//! # `srui-sessiond` Daemon Binary
//!
//! Per-user persistent session daemon (§20.2).
//! Manages durable UI state across transient SSH bridge connections.

mod unix_security;

use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::UnixListener;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use srui_sessiond::{handle_connection, Session, SessionConfig};
use unix_security::{
    default_socket_path as private_default_socket_path, effective_uid,
    prepare_private_socket_parent, require_unprivileged_uid, validate_peer,
    validate_private_socket,
};
/// Ignores `SIGHUP` so SSH session detach / controlling-terminal loss does not terminate
/// the daemon (§17, §20.2). Omitting a handler leaves the default disposition, which kills
/// the process and defeats persistent session state.
#[cfg(unix)]
fn ignore_sighup() -> Result<(), std::io::Error> {
    // SAFETY: `signal(2)` mutates process-wide disposition. This is called synchronously at process
    // start, before any thread, task or other handler exists, so no concurrent observer can see the
    // intermediate state, and `SIG_IGN` installs no handler that could run unsafe code.
    let rc = unsafe { libc::signal(libc::SIGHUP, libc::SIG_IGN) };
    if rc == libc::SIG_ERR {
        // Failing silently would leave the daemon killable by the very disconnect this call exists
        // to survive (§17, §20.2).
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

/// How long a graceful shutdown waits for in-flight connections before aborting them (§20.4).
const SHUTDOWN_DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

fn default_socket_path() -> PathBuf {
    private_default_socket_path(effective_uid())
}
/// Parsed command line: socket path, optional built-in app adapter, and the §18.1 journal
/// retention window (maximum retained transaction count).
#[derive(Debug, Clone)]
struct DaemonConfig {
    socket_path: PathBuf,
    app_name: Option<String>,
    journal_capacity: usize,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            socket_path: default_socket_path(),
            app_name: None,
            journal_capacity: SessionConfig::default().journal_capacity,
        }
    }
}

/// Parses the daemon command line, rejecting anything it does not understand.
///
/// A silently swallowed flag would start the daemon on the default endpoint instead of the
/// requested one — particularly dangerous for a background SSH subsystem, where nobody reads the
/// startup log — so an unknown flag or a missing value is a hard error.
fn parse_args_from(args: &[String]) -> Result<DaemonConfig, String> {
    let mut config = DaemonConfig::default();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--socket" => {
                let value = args
                    .get(i + 1)
                    .filter(|value| !value.starts_with('-'))
                    .ok_or_else(|| "--socket requires a path argument".to_string())?;
                config.socket_path = PathBuf::from(value);
                i += 2;
            }
            "--app" => {
                let value = args
                    .get(i + 1)
                    .filter(|value| !value.starts_with('-'))
                    .ok_or_else(|| "--app requires an application name".to_string())?;
                config.app_name = Some(value.clone());
                i += 2;
            }
            "--journal-capacity" => {
                // A zero or unparsable retention window would silently degrade every reconnect
                // to a snapshot resync, so refuse to start instead of clamping (§18.1).
                // Only a following flag counts as a missing value: `-1` is a value this option
                // must reject by range, and reporting it as a missing argument would point the
                // operator at the wrong mistake.
                let value = args
                    .get(i + 1)
                    .filter(|value| !value.starts_with("--"))
                    .ok_or_else(|| "--journal-capacity requires a value".to_string())?;
                config.journal_capacity = value
                    .parse::<usize>()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or_else(|| {
                        format!("--journal-capacity must be a positive integer, got {value}")
                    })?;
                i += 2;
            }
            arg if !arg.starts_with('-') => {
                config.socket_path = PathBuf::from(arg);
                i += 1;
            }
            other => return Err(format!("unrecognized argument: {other}")),
        }
    }

    Ok(config)
}

fn parse_args() -> Result<DaemonConfig, String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    parse_args_from(&args)
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
    /// Exclusive advisory lock on the socket path, released when this value is dropped.
    _lock: std::fs::File,
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

/// The advisory lock path guarding `socket_path`.
fn socket_lock_path(socket_path: &std::path::Path) -> PathBuf {
    let mut name = socket_path.as_os_str().to_os_string();
    name.push(".lock");
    PathBuf::from(name)
}

/// Takes the exclusive advisory lock that marks this process as the owner of the socket path.
///
/// The lock, not the connect probe, is what decides ownership: it is held across the probe, the
/// unlink and the bind, so two daemons can never both conclude the endpoint was free and race to
/// replace each other's freshly bound socket. The kernel releases it when the descriptor closes,
/// including on `SIGKILL`, so a crashed daemon never leaves the path permanently claimed.
fn acquire_socket_lock(socket_path: &std::path::Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::io::AsRawFd;

    let lock_path = socket_lock_path(socket_path);
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&lock_path)?;

    // SAFETY: `file` owns a valid open descriptor for the duration of the call.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let error = std::io::Error::last_os_error();
        return if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
            Err(std::io::Error::new(
                std::io::ErrorKind::AddrInUse,
                format!(
                    "{} is already served by a running process; pass a different socket path",
                    socket_path.display()
                ),
            ))
        } else {
            Err(error)
        };
    }
    Ok(file)
}

/// What a connect probe was able to establish about an existing socket path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SocketLiveness {
    /// A server answered: the endpoint is in use.
    Live,
    /// The endpoint is definitively gone or definitively unserved.
    Absent,
    /// The probe failed in a way that does not prove anything either way.
    Ambiguous,
}

/// Classifies an existing socket path by attempting one connection.
///
/// Only two outcomes are treated as proof of absence: the path no longer exists, or connection is
/// refused. `ECONNREFUSED` is conclusive *here* precisely because [`acquire_socket_lock`] already
/// excluded every other daemon instance, so it cannot be the backlog-saturated peer of a sibling.
/// Anything else — `EPERM`, `EACCES`, `ECONNRESET` — can equally well come from a live foreign
/// listener, and a foreign endpoint must never be displaced.
async fn probe_socket_liveness(path: &std::path::Path) -> std::io::Result<SocketLiveness> {
    use tokio::net::UnixStream;
    if socket_identity(path)?.is_none() {
        return Ok(SocketLiveness::Absent);
    }
    match UnixStream::connect(path).await {
        Ok(_) => Ok(SocketLiveness::Live),
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound
                || error.raw_os_error() == Some(libc::ENOENT)
                || error.kind() == std::io::ErrorKind::ConnectionRefused
                || error.raw_os_error() == Some(libc::ECONNREFUSED) =>
        {
            Ok(SocketLiveness::Absent)
        }
        Err(_) => Ok(SocketLiveness::Ambiguous),
    }
}

/// Binds a Unix domain socket, verifying parent directory permissions and unlinking any stale
/// predecessor safely without disturbing a live server.
async fn bind_owned_socket(path: &std::path::Path) -> std::io::Result<(UnixListener, OwnedSocket)> {
    let uid = effective_uid();
    prepare_private_socket_parent(path, uid)?;

    let lock = acquire_socket_lock(path)?;

    match probe_socket_liveness(path).await? {
        SocketLiveness::Live => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AddrInUse,
                format!(
                    "{} is already served by a running process; pass a different socket path",
                    path.display()
                ),
            ));
        }
        SocketLiveness::Ambiguous => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AddrInUse,
                format!(
                    "{} could not be proven unused; refusing to unlink it. Pass a different socket path",
                    path.display()
                ),
            ));
        }
        SocketLiveness::Absent => {}
    }

    // The lock is held across the unlink and the bind, so no other daemon can slip in between.
    // `socket_identity` refuses a path that exists but is not a socket, so an unrelated file is
    // never a removal candidate.
    if socket_identity(path)?.is_some() {
        info!("removing stale socket {}", path.display());
        std::fs::remove_file(path)?;
    }

    // Create the endpoint as `0600`: a Unix socket honours the umask, and any connector can drive
    // the session, so the endpoint must not be world-connectable.
    //
    // SAFETY: `umask(2)` reads and replaces a process-wide value and cannot fail. This runs during
    // single-threaded startup, and the previous value is restored immediately after the bind.
    let previous_umask = unsafe { libc::umask(0o177) };
    let bind_result = UnixListener::bind(path);
    unsafe { libc::umask(previous_umask) };
    let listener = bind_result?;
    validate_private_socket(path, uid)?;

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
            _lock: lock,
        },
    ))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(unix)]
    ignore_sighup()?;

    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    info!("Starting srui-sessiond daemon (§20.2)...");

    let daemon_uid = effective_uid();
    require_unprivileged_uid(daemon_uid)?;

    let config = parse_args().map_err(|message| {
        error!("{message}");
        message
    })?;

    let (listener, owned_socket) = bind_owned_socket(&config.socket_path).await?;
    info!(socket_path = ?config.socket_path, "Listening on Unix domain socket");

    // Mint a fresh, globally unique session incarnation token (§17)
    let session = Arc::new(Session::mint_with_config(SessionConfig {
        journal_capacity: config.journal_capacity,
        ..SessionConfig::default()
    }));
    info!(
        session_id = %session.session_id(),
        state = %session.state(),
        journal_capacity = config.journal_capacity,
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
                        if let Err(error) = validate_peer(&stream, daemon_uid) {
                            warn!(
                                error = %error,
                                "Rejecting Unix socket peer outside the authenticated user boundary"
                            );
                            continue;
                        }
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

    // Await all connection tasks, but never let one unresponsive peer hold the daemon open: a
    // connection blocked writing to a client that stopped reading only unwinds once its own write
    // deadline elapses, and an unbounded join here would outlive any operator's patience (§20.4).
    let drained = tokio::time::timeout(SHUTDOWN_DRAIN_TIMEOUT, async {
        while let Some(res) = tasks.join_next().await {
            if let Err(e) = res {
                error!("Connection task panicked: {}", e);
            }
        }
    })
    .await;

    if drained.is_err() {
        warn!(
            timeout = ?SHUTDOWN_DRAIN_TIMEOUT,
            remaining = tasks.len(),
            "Connection drain timed out; aborting remaining connections"
        );
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
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

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_string()).collect()
    }

    #[test]
    fn recognized_options_are_parsed() {
        let config = parse_args_from(&args(&["--socket", "/tmp/a.sock", "--app", "counter"]))
            .expect("valid arguments");
        assert_eq!(config.socket_path, PathBuf::from("/tmp/a.sock"));
        assert_eq!(config.app_name.as_deref(), Some("counter"));
    }

    #[test]
    fn a_bare_path_still_selects_the_socket() {
        let config = parse_args_from(&args(&["/tmp/b.sock"])).expect("valid arguments");
        assert_eq!(config.socket_path, PathBuf::from("/tmp/b.sock"));
    }

    #[test]
    fn an_unknown_flag_is_rejected_instead_of_silently_ignored() {
        // Falling back to the default endpoint on a typo is what makes this dangerous for a
        // background SSH subsystem: nobody reads the startup log.
        let error = parse_args_from(&args(&["--sockets", "/tmp/c.sock"])).expect_err("rejected");
        assert!(error.contains("--sockets"), "{error}");
    }

    #[test]
    fn a_missing_option_value_is_rejected() {
        assert!(parse_args_from(&args(&["--socket"])).is_err());
        assert!(parse_args_from(&args(&["--socket", "--app"])).is_err());
        assert!(parse_args_from(&args(&["--app"])).is_err());
        assert!(parse_args_from(&args(&["--journal-capacity"])).is_err());
        assert_eq!(
            parse_args_from(&args(&["--journal-capacity", "--app"]))
                .expect_err("missing retention window"),
            "--journal-capacity requires a value"
        );
    }

    /// §18.1: the retention window is configurable, and a window that cannot retain anything is
    /// refused at startup rather than silently degrading every reconnect to a snapshot resync.
    #[test]
    fn journal_capacity_is_parsed_and_defaults_to_the_session_default() {
        let config = parse_args_from(&args(&["--journal-capacity", "64"])).expect("valid window");
        assert_eq!(config.journal_capacity, 64);

        let default = parse_args_from(&args(&[])).expect("valid arguments");
        assert_eq!(
            default.journal_capacity,
            SessionConfig::default().journal_capacity
        );
    }

    #[test]
    fn a_non_positive_or_unparsable_journal_capacity_is_rejected() {
        for value in ["0", "-1", "many"] {
            let error = parse_args_from(&args(&["--journal-capacity", value]))
                .expect_err("rejected retention window");
            // Asserted verbatim: a negative window used to be reported as a *missing* value,
            // which points the operator at the wrong mistake (§18.1).
            assert_eq!(
                error,
                format!("--journal-capacity must be a positive integer, got {value}")
            );
        }
    }
}
