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
    measure_transaction, Monitor, SignalTerminator, SysinfoProcessSource,
};
use srui_sessiond::{handle_connection, Session};
use srui_unix_security::{
    default_named_socket_path, effective_uid, prepare_private_socket_parent,
    require_unprivileged_uid, validate_peer, PrivateSocketParent, SocketIdentity,
};

const POLL_INTERVAL: Duration = Duration::from_secs(1);

struct Options {
    socket_path: PathBuf,
    wire_stats: bool,
}

fn default_socket_path() -> PathBuf {
    default_named_socket_path(effective_uid(), "srui-process-monitor.sock")
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
    /// Descriptor-relative identity of the endpoint created by this process.
    identity: SocketIdentity,
    /// Retains the validated parent inode for identity checks and cleanup.
    parent: PrivateSocketParent,
    /// Exclusive advisory lock on the socket path, released when this value is dropped.
    _lock: std::fs::File,
}

impl OwnedSocket {
    /// Removes the socket, but only while the path still resolves to the endpoint we bound.
    ///
    /// If a replacement server has since taken the path over, its socket has a different inode and
    /// is left alone.
    fn remove(&self) {
        match self.parent.socket_identity() {
            Ok(Some(identity)) if identity == self.identity => {
                if let Err(error) = self.parent.remove_socket() {
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

/// The advisory lock path guarding `socket_path`.
fn socket_lock_path(socket_path: &Path) -> PathBuf {
    let mut name = socket_path.as_os_str().to_os_string();
    name.push(".lock");
    PathBuf::from(name)
}

/// Takes the exclusive advisory lock that marks this process as the owner of the socket path.
///
/// `flock(2)` is the authority on ownership, not a connect probe: the kernel releases it when the
/// descriptor closes, including on `SIGKILL`, so a crashed server never leaves the endpoint
/// permanently claimed, and holding it across the unlink-then-bind sequence closes the window in
/// which two instances could both conclude the existing socket was stale.
fn acquire_socket_lock(socket_path: &Path) -> std::io::Result<std::fs::File> {
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
                    "{} is already served by a running process; pass a different --socket",
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
/// excluded every other instance of this program, so it cannot be the backlog-saturated peer of a
/// sibling daemon. Anything else — `EPERM`, `EACCES`, `ECONNRESET` — can equally well come from a
/// live foreign listener, and a foreign endpoint must never be displaced.
async fn probe_socket_liveness(path: &Path) -> std::io::Result<SocketLiveness> {
    if socket_identity(path)?.is_none() {
        return Ok(SocketLiveness::Absent);
    }
    match tokio::net::UnixStream::connect(path).await {
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

/// Binds `path`, refusing to displace a socket another server is still listening on.
async fn bind_owned_socket(path: &Path, uid: u32) -> std::io::Result<(UnixListener, OwnedSocket)> {
    let socket_parent = prepare_private_socket_parent(path, uid)?;
    let lock = acquire_socket_lock(path)?;

    match probe_socket_liveness(path).await? {
        SocketLiveness::Live => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AddrInUse,
                format!(
                    "{} is already served by a running process; pass a different --socket",
                    path.display()
                ),
            ));
        }
        SocketLiveness::Ambiguous => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AddrInUse,
                format!(
                    "{} could not be proven unused; refusing to unlink it. Pass a different --socket",
                    path.display()
                ),
            ));
        }
        SocketLiveness::Absent => {}
    }

    // The lock is held across the unlink and the bind, so no other instance can slip in between.
    // `socket_identity` refuses a path that exists but is not a socket, so an unrelated file is
    // never a removal candidate.
    if socket_parent.socket_identity()?.is_some() {
        info!("removing stale socket {}", path.display());
        socket_parent.remove_socket()?;
    }

    let listener = UnixListener::from_std(socket_parent.bind()?)?;
    let identity = socket_parent.socket_identity()?.ok_or_else(|| {
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
            parent: socket_parent,
            _lock: lock,
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
    // Failing to install it silently would leave the daemon killable by the very disconnect this
    // call exists to survive, so the error is propagated instead of dropped.
    //
    // SAFETY: `signal(2)` mutates process-wide disposition. This runs during single-threaded
    // startup, before any task is spawned and before the socket exists, so no other thread can
    // observe or race the intermediate disposition, and `SigIgn` installs no handler that could
    // run non-async-signal-safe code.
    unsafe {
        nix::sys::signal::signal(
            nix::sys::signal::Signal::SIGHUP,
            nix::sys::signal::SigHandler::SigIgn,
        )
        .map_err(|errno| format!("failed to ignore SIGHUP: {errno}"))?;
    }

    let uid = effective_uid();
    require_unprivileged_uid(uid, "process-monitor")?;

    let session = Arc::new(Session::mint());

    let shutdown = CancellationToken::new();
    let mut tasks = JoinSet::new();

    // Subscribe before the initial transaction so the very first line reports the full snapshot
    // every later tick is compared against.
    if options.wire_stats {
        let mut receiver =
            session.subscribe_transactions(b"process-monitor-wire-stats".to_vec())?;
        let stats_shutdown = shutdown.clone();
        tasks.spawn(async move {
            loop {
                tokio::select! {
                    received = receiver.recv_class(srui_sessiond::LogicalChannelClass::Ui) => match received {
                        Ok(srui_sessiond::OutboundItem::Transaction(transaction)) => {
                            match measure_transaction(&transaction) {
                                Ok(stats) => info!(target: "srui::wire_stats", "{stats}"),
                                Err(error) => warn!(target: "srui::wire_stats", "{error}"),
                            }
                        }
                        Ok(srui_sessiond::OutboundItem::ResourceMetadata(_))
                        | Ok(srui_sessiond::OutboundItem::ResourceChunk(_)) => {
                            // Resource frames are low-priority delivery; wire-stats here track
                            // semantic transactions only.
                        }
                        Err(srui_sessiond::OutboundRecvError::Lagged(reason)) => {
                            warn!("wire-stats observer lagged: {reason}; closing");
                            break;
                        }
                        Err(srui_sessiond::OutboundRecvError::Closed) => break,
                    },
                    _ = stats_shutdown.cancelled() => break,
                }
            }
        });
    }

    // Bind before sampling so a duplicate instance fails fast and never disturbs the live server.
    let (listener, owned_socket) = bind_owned_socket(&options.socket_path, uid).await?;
    info!(
        "listening on Unix domain socket: {}",
        options.socket_path.display()
    );

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
                        if let Err(error) = validate_peer(&stream, uid) {
                            warn!(
                                error = %error,
                                "rejecting Unix socket peer outside the authenticated user boundary"
                            );
                            continue;
                        }
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
