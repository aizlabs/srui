//! Runnable SRUI UI gallery server (§19.1, §20.1, §20.2).
//!
//! Hosts the gallery on a Unix domain socket, which `srui-ssh-bridge` forwards to over an SSH
//! subsystem. All diagnostics go to stderr so a bridged stdout stays a pure binary protocol
//! stream (§19.1).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use tokio::net::UnixListener;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use srui_example_ui_gallery::GalleryApp;
use srui_sessiond::{handle_connection, Session};

/// Autoplay cadence. A commit is a state-consistency boundary, not a frame: the renderer paces
/// drawing independently of this interval (§12.2).
const DEFAULT_AUTOPLAY_INTERVAL: Duration = Duration::from_secs(4);

struct Options {
    socket_path: PathBuf,
    autoplay_interval: Duration,
    autoplay_on_start: bool,
}

fn default_socket_path() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("srui-ui-gallery.sock")
}

fn parse_options(args: &[String]) -> Result<Options, String> {
    let mut socket_path = None;
    let mut autoplay_interval = DEFAULT_AUTOPLAY_INTERVAL;
    let mut autoplay_on_start = false;

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
            "--autoplay-interval" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "--autoplay-interval requires a seconds argument".to_string())?;
                let seconds: u64 = value.parse().map_err(|_| {
                    format!("--autoplay-interval expects whole seconds, got {value:?}")
                })?;
                if seconds == 0 {
                    return Err("--autoplay-interval must be at least 1 second".to_string());
                }
                autoplay_interval = Duration::from_secs(seconds);
                index += 2;
            }
            "--autoplay" => {
                autoplay_on_start = true;
                index += 1;
            }
            other => return Err(format!("unrecognized argument: {other}")),
        }
    }

    Ok(Options {
        socket_path: socket_path.unwrap_or_else(default_socket_path),
        autoplay_interval,
        autoplay_on_start,
    })
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
/// `Ok(None)` means the path does not exist; a path that exists but is not a socket is an error,
/// so an unrelated file is never a removal candidate.
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

fn socket_lock_path(socket_path: &Path) -> PathBuf {
    let mut name = socket_path.as_os_str().to_os_string();
    name.push(".lock");
    PathBuf::from(name)
}

/// Takes the exclusive advisory lock marking this process as owner of the socket path.
///
/// `flock(2)` is the authority on ownership, not a connect probe: the kernel releases it when the
/// descriptor closes, including on `SIGKILL`, so a crashed server never leaves the endpoint
/// permanently claimed, and holding it across unlink-then-bind closes the window in which two
/// instances could both conclude the existing socket was stale.
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

static UMASK_LOCK: Mutex<()> = Mutex::new(());

/// Restores the process umask even when binding returns early or unwinds.
struct UmaskGuard {
    previous: libc::mode_t,
    _lock: MutexGuard<'static, ()>,
}

impl UmaskGuard {
    fn private_socket() -> Self {
        let lock = UMASK_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // SAFETY: `umask` accepts every mode value, has no pointer arguments, and the previous
        // process mask is retained by this guard until it is restored in Drop.
        let previous = unsafe { libc::umask(0o177) };
        Self {
            previous,
            _lock: lock,
        }
    }
}

impl Drop for UmaskGuard {
    fn drop(&mut self) {
        // SAFETY: restoring the mode returned by `umask` is always valid. The guard's mutex
        // serializes every bind performed through this module until restoration is complete.
        unsafe {
            libc::umask(self.previous);
        }
    }
}

async fn bind_owned_socket(path: &Path) -> std::io::Result<(UnixListener, OwnedSocket)> {
    let lock = acquire_socket_lock(path)?;

    // The lock is held, so any socket still at this path belongs to a process that is gone.
    if socket_identity(path)?.is_some() {
        info!("removing stale socket at {}", path.display());
        std::fs::remove_file(path)?;
    }

    // A Unix socket honors the process umask at creation. Keep the endpoint owner-only so
    // another local account cannot inspect the tree or inject authoritative UI events.
    let listener = {
        let _umask = UmaskGuard::private_socket();
        UnixListener::bind(path)?
    };
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
    // stderr only: a bridged stdout is the binary protocol stream (§19.1, §20.1).
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let options = match parse_options(&args) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("{message}");
            eprintln!(
                "usage: ui-gallery [--socket PATH] [--autoplay] [--autoplay-interval SECONDS]"
            );
            std::process::exit(2);
        }
    };

    let (listener, owned_socket) = bind_owned_socket(&options.socket_path).await?;
    info!("ui gallery listening on {}", options.socket_path.display());

    let session = Arc::new(Session::mint());
    let app = GalleryApp::start(session.clone())?;
    info!(
        "gallery published: {} nodes, image {}",
        session.node_count(),
        app.image()
            .map(|hash| hash.to_hex())
            .unwrap_or_else(|| "unavailable".to_string())
    );

    if options.autoplay_on_start {
        app.set_autoplay(true)?;
    }

    let shutdown = CancellationToken::new();
    let mut tasks = JoinSet::new();

    let autoplay_app = app.clone();
    let autoplay_shutdown = shutdown.clone();
    let interval_duration = options.autoplay_interval;
    tasks.spawn(async move {
        let mut interval = tokio::time::interval(interval_duration);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if autoplay_app.autoplay() {
                        if let Err(error) = autoplay_app.next_scene() {
                            warn!("autoplay transaction failed: {error}");
                        }
                    }
                }
                _ = autoplay_shutdown.cancelled() => break,
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
                info!("interrupt received; stopping ui gallery");
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
    info!("ui gallery shutdown complete");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct RestoreUmask(libc::mode_t);

    impl Drop for RestoreUmask {
        fn drop(&mut self) {
            // SAFETY: this is the mode returned by the successful `umask` call below.
            unsafe {
                libc::umask(self.0);
            }
        }
    }

    #[tokio::test]
    async fn bound_socket_is_owner_only_and_original_umask_is_restored() {
        // Arrange a permissive conventional mask so this test would observe 0755 without the
        // restrictive bind guard. Restore the process-wide setting even if an assertion unwinds.
        // SAFETY: `umask` accepts every mode value and returns the previous process mask.
        let previous = unsafe { libc::umask(0o022) };
        let _restore = RestoreUmask(previous);

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is after the Unix epoch")
            .as_nanos();
        // Unix socket paths are short (104 bytes on macOS); /tmp keeps the regression portable
        // even when the test runner's TMPDIR is a deeply nested sandbox path.
        let directory =
            PathBuf::from("/tmp").join(format!("srui-gallery-{}-{unique}", std::process::id()));
        std::fs::create_dir(&directory).expect("temporary socket directory is created");
        let path = directory.join("gallery.sock");

        let (listener, owned) = bind_owned_socket(&path).await.expect("socket binds");
        let mode = std::fs::symlink_metadata(&path)
            .expect("bound socket has metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            mode, 0o600,
            "the socket must be accessible only by its owner"
        );

        // Observe the current mask while leaving the test's sentinel mask in place.
        // SAFETY: the sentinel is a valid mode and RestoreUmask retains the original mode.
        let observed = unsafe { libc::umask(0o022) };
        assert_eq!(observed, 0o022, "binding must restore the caller's umask");

        drop(listener);
        owned.remove();
        std::fs::remove_file(socket_lock_path(&path)).expect("lock file is removed");
        std::fs::remove_dir(directory).expect("temporary socket directory is removed");
    }
}
