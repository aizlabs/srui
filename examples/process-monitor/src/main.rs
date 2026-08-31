//! Runnable SRUI remote process monitor server (§19.1, §20.1, §20.2).
//!
//! Hosts the semantic process-monitor application on a Unix domain socket, which
//! `srui-ssh-bridge` forwards to over an SSH subsystem. All diagnostics go to stderr so a bridged
//! stdout remains a pure binary protocol stream (§19.1).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::net::UnixListener;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use srui_example_process_monitor::{
    effective_uid, measure_transaction, Monitor, SignalTerminator, SysinfoProcessSource,
};
use srui_sessiond::{handle_connection, Session};

const POLL_INTERVAL: Duration = Duration::from_secs(1);

struct Options {
    socket_path: PathBuf,
    wire_stats: bool,
}

fn default_socket_path() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("srui-process-monitor.sock")
}

fn parse_options(args: &[String]) -> Result<Options, String> {
    let mut socket_path = None;
    let mut wire_stats = false;

    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--socket" => {
                let value = args
                    .get(index + 1)
                    .filter(|value| !value.starts_with('-'))
                    .ok_or_else(|| "--socket requires a path argument".to_string())?;
                socket_path = Some(PathBuf::from(value));
                index += 2;
            }
            "--wire-stats" => {
                wire_stats = true;
                index += 1;
            }
            other => {
                return Err(format!("unrecognized argument: {other}"));
            }
        }
    }

    Ok(Options {
        socket_path: socket_path.unwrap_or_else(default_socket_path),
        wire_stats,
    })
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
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

/// Binds `path`, refusing to displace a socket another server is still listening on.
///
/// An existing socket is probed by connecting to it: a successful connection means a live server
/// owns the endpoint and this process must not start. Only a socket that refuses connections is
/// treated as stale and unlinked.
async fn bind_owned_socket(path: &Path) -> std::io::Result<(UnixListener, OwnedSocket)> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    if socket_identity(path)?.is_some() {
        match tokio::net::UnixStream::connect(path).await {
            Ok(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AddrInUse,
                    format!(
                        "{} is already served by a running process; pass a different --socket",
                        path.display()
                    ),
                ));
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::ConnectionRefused
                        | std::io::ErrorKind::NotFound
                        | std::io::ErrorKind::PermissionDenied
                        | std::io::ErrorKind::ConnectionReset
                ) || matches!(
                    error.raw_os_error(),
                    Some(libc::ECONNREFUSED) | Some(libc::EPERM) | Some(libc::EACCES) | Some(libc::ENOENT)
                ) =>
            {
                info!("removing stale socket {}", path.display());
                std::fs::remove_file(path)?;
            }
            Err(error) => return Err(error),
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
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let options = parse_options(&args).map_err(|message| {
        error!("{message}");
        message
    })?;

    // Ignore SIGHUP so detached process-monitor daemons survive SSH bridge disconnects (§17, §20.2).
    unsafe {
        let _ = nix::sys::signal::signal(
            nix::sys::signal::Signal::SIGHUP,
            nix::sys::signal::SigHandler::SigIgn,
        );
    }

    let session = Arc::new(Session::mint());

    let shutdown = CancellationToken::new();
    let mut tasks = JoinSet::new();

    // Subscribe before the initial transaction so the very first line reports the full snapshot
    // every later tick is compared against.
    if options.wire_stats {
        let mut receiver = session.subscribe_transactions()?;
        let stats_shutdown = shutdown.clone();
        tasks.spawn(async move {
            loop {
                tokio::select! {
                    received = receiver.recv() => match received {
                        Ok(transaction) => info!(target: "srui::wire_stats", "{}", measure_transaction(&transaction)),
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                            warn!("wire-stats observer lagged by {skipped} transactions");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    },
                    _ = stats_shutdown.cancelled() => break,
                }
            }
        });
    }

    // Bind before sampling so a duplicate instance fails fast and never disturbs the live server.
    let (listener, owned_socket) = bind_owned_socket(&options.socket_path).await?;
    info!(
        "listening on Unix domain socket: {}",
        options.socket_path.display()
    );

    let uid = effective_uid();
    let monitor = {
        let session = session.clone();
        tokio::task::spawn_blocking(move || {
            // sysinfo derives CPU utilization from the delta between two refreshes: warm up
            // deliberately so the first published sample is meaningful rather than a zeroed
            // placeholder.
            let source = SysinfoProcessSource::new();
            std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
            Monitor::start(session, Box::new(source), Box::new(SignalTerminator), uid)
        })
        .await??
    };
    info!(
        "process monitor initialized with {} visible processes (uid {uid})",
        monitor.with_state(|state| state.visible().len())
    );

    // Polling task (§12.2: a commit is a state-consistency boundary, not a frame).
    let poll_monitor = monitor.clone();
    let poll_shutdown = shutdown.clone();
    tasks.spawn(async move {
        let mut interval = tokio::time::interval(POLL_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let monitor = poll_monitor.clone();
                    // sysinfo refreshes are blocking work: keep them off the async runtime.
                    match tokio::task::spawn_blocking(move || monitor.tick()).await {
                        Ok(Ok(_)) => {}
                        Ok(Err(error)) => warn!("polling transaction failed: {error}"),
                        Err(error) => warn!("polling task failed: {error}"),
                    }
                }
                _ = poll_shutdown.cancelled() => break,
            }
        }
    });

    loop {
        tokio::select! {
            accept_result = listener.accept() => {
                match accept_result {
                    Ok((stream, _peer)) => {
                        let session = session.clone();
                        let child = shutdown.child_token();
                        tasks.spawn(async move {
                            if let Err(error) = handle_connection(stream, session, child).await {
                                warn!("client connection ended: {error}");
                            }
                        });
                    }
                    Err(error) => error!("accept failed: {error}"),
                }
            }
            _ = tokio::signal::ctrl_c() => {
                info!("interrupt received; stopping process monitor");
                break;
            }
        }
    }

    shutdown.cancel();
    drop(listener);
    while let Some(result) = tasks.join_next().await {
        if let Err(error) = result {
            warn!("task ended abnormally: {error}");
        }
    }
    owned_socket.remove();
    info!("process monitor shutdown complete");
    Ok(())
}
