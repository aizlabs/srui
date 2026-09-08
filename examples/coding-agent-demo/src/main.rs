//! Runnable Task 31 coding-agent example over a real Unix transport (§11.1, §20.2, §30).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use srui_example_coding_agent::{CodingAgentApp, APPROVE_ID};
use srui_sessiond::handle_connection;
use tokio::net::UnixListener;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// A socket path this process owns, guarded against competing demo instances.
struct OwnedSocket {
    path: PathBuf,
    identity: (u64, u64),
    _lock: std::fs::File,
}

impl Drop for OwnedSocket {
    fn drop(&mut self) {
        match socket_identity(&self.path) {
            Ok(Some(identity)) if identity == self.identity => {
                if let Err(error) = std::fs::remove_file(&self.path) {
                    warn!(path = %self.path.display(), error = %error, "failed to remove socket");
                }
            }
            Ok(Some(_)) => warn!(
                path = %self.path.display(),
                "leaving socket in place because it now belongs to another server"
            ),
            Ok(None) | Err(_) => {}
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

fn socket_lock_path(socket_path: &Path) -> PathBuf {
    let mut name = socket_path.as_os_str().to_os_string();
    name.push(".lock");
    PathBuf::from(name)
}

fn acquire_socket_lock(socket_path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::OpenOptionsExt;

    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(socket_lock_path(socket_path))?;

    // SAFETY: file owns a valid descriptor for the duration of this non-blocking flock call.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SocketLiveness {
    Live,
    Absent,
    Ambiguous,
}

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

async fn bind_owned_socket(path: &Path) -> std::io::Result<(UnixListener, OwnedSocket)> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

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
                    "{} could not be proven unused; refusing to unlink it",
                    path.display()
                ),
            ));
        }
        SocketLiveness::Absent => {}
    }

    if socket_identity(path)?.is_some() {
        info!(path = %path.display(), "removing stale socket");
        std::fs::remove_file(path)?;
    }

    // The endpoint carries action events, so make it owner-only from the instant it is bound.
    //
    // SAFETY: umask reads and replaces a process-wide value and cannot fail. Server startup calls
    // this once before accepting connections, and restores the previous value immediately.
    let previous_umask = unsafe { libc::umask(0o177) };
    let bind_result = UnixListener::bind(path);
    // SAFETY: previous_umask is exactly the value returned by the preceding umask call.
    unsafe { libc::umask(previous_umask) };
    let listener = bind_result?;

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

async fn run_server(path: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    let app = CodingAgentApp::new()?;
    let session = app.session_arc();
    let (listener, _owned_socket) = bind_owned_socket(&path).await?;
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
                        warn!(error = %error, "client connection ended");
                    }
                });
            }
            completed = connections.join_next(), if !connections.is_empty() => {
                if let Some(Err(error)) = completed {
                    warn!(error = %error, "client connection task failed");
                }
            }
            _ = tokio::signal::ctrl_c() => break,
        }
    }

    shutdown.cancel();
    while let Some(result) = connections.join_next().await {
        if let Err(error) = result {
            warn!(error = %error, "client connection task failed");
        }
    }
    session.pty().shutdown();
    Ok(())
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

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[tokio::test]
    async fn binding_recovers_stale_socket_enforces_single_owner_and_private_mode() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("coding-agent.sock");
        let stale = std::os::unix::net::UnixListener::bind(&path).expect("bind stale socket");
        drop(stale);

        let (listener, owned_socket) = bind_owned_socket(&path)
            .await
            .expect("replace stale socket");
        let mode = std::fs::metadata(&path)
            .expect("socket metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);

        let competing_bind = bind_owned_socket(&path).await;
        assert!(matches!(
            competing_bind,
            Err(error) if error.kind() == std::io::ErrorKind::AddrInUse
        ));

        drop(listener);
        drop(owned_socket);
        assert!(!path.exists(), "owned socket must be removed on drop");
    }
}
