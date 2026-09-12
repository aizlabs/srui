//! Runnable SRUI UI gallery server (§19.1, §20.1, §20.2).
//!
//! Hosts the gallery on a private Unix domain socket. All diagnostics go to stderr so a bridged
//! stdout stays a pure binary protocol stream (§19.1).

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tokio::net::{UnixListener, UnixStream};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use srui_example_ui_gallery::GalleryApp;
use srui_sessiond::{handle_connection, Session};
use srui_unix_security::{
    default_named_socket_path, effective_uid, prepare_private_socket_parent,
    require_unprivileged_uid, validate_peer, PrivateSocketParent, SocketIdentity,
};

const DEFAULT_AUTOPLAY_INTERVAL: Duration = Duration::from_secs(4);
const USAGE: &str = "usage: ui-gallery [--socket PATH] [--autoplay] [--autoplay-interval SECONDS]";

struct Options {
    socket_path: PathBuf,
    autoplay_interval: Duration,
    autoplay_on_start: bool,
}

enum ParseOutcome {
    Run(Options),
    Help,
}

fn parse_options(args: &[String]) -> Result<ParseOutcome, String> {
    let mut socket_path = None;
    let mut autoplay_interval = DEFAULT_AUTOPLAY_INTERVAL;
    let mut autoplay_on_start = false;
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "-h" | "--help" => return Ok(ParseOutcome::Help),
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

    Ok(ParseOutcome::Run(Options {
        socket_path: socket_path
            .unwrap_or_else(|| default_named_socket_path(effective_uid(), "srui-ui-gallery.sock")),
        autoplay_interval,
        autoplay_on_start,
    }))
}

type FileIdentity = (u64, u64);

fn metadata_identity(metadata: &std::fs::Metadata) -> FileIdentity {
    use std::os::unix::fs::MetadataExt;
    (metadata.dev(), metadata.ino())
}

struct OwnedLock {
    path: PathBuf,
    identity: FileIdentity,
    _file: std::fs::File,
}

impl Drop for OwnedLock {
    fn drop(&mut self) {
        match std::fs::symlink_metadata(&self.path) {
            Ok(metadata) if metadata_identity(&metadata) == self.identity => {
                if let Err(error) = std::fs::remove_file(&self.path) {
                    warn!("failed to remove {}: {error}", self.path.display());
                }
            }
            Ok(_) => warn!(
                "leaving {} in place: the lock path now belongs to another process",
                self.path.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => warn!("failed to inspect {}: {error}", self.path.display()),
        }
    }
}

struct OwnedSocket {
    path: PathBuf,
    identity: SocketIdentity,
    parent: PrivateSocketParent,
    _lock: OwnedLock,
}

impl Drop for OwnedSocket {
    fn drop(&mut self) {
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
            Ok(None) => {}
            Err(error) => warn!("failed to inspect {}: {error}", self.path.display()),
        }
    }
}

fn socket_lock_path(socket_path: &Path) -> PathBuf {
    let mut name = socket_path.as_os_str().to_os_string();
    name.push(".lock");
    PathBuf::from(name)
}

fn acquire_socket_lock(socket_path: &Path) -> std::io::Result<OwnedLock> {
    use std::os::unix::fs::OpenOptionsExt;

    let path = socket_lock_path(socket_path);
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&path)?;

    if let Err(error) = fs2::FileExt::try_lock_exclusive(&file) {
        return if error.kind() == std::io::ErrorKind::WouldBlock {
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

    let identity = metadata_identity(&file.metadata()?);
    Ok(OwnedLock {
        path,
        identity,
        _file: file,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SocketLiveness {
    Live,
    Absent,
    Ambiguous,
}

async fn probe_socket_liveness(
    path: &Path,
    parent: &PrivateSocketParent,
) -> std::io::Result<SocketLiveness> {
    if parent.socket_identity()?.is_none() {
        return Ok(SocketLiveness::Absent);
    }

    match UnixStream::connect(path).await {
        Ok(_) => Ok(SocketLiveness::Live),
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound
                || error.kind() == std::io::ErrorKind::ConnectionRefused =>
        {
            Ok(SocketLiveness::Absent)
        }
        Err(_) => Ok(SocketLiveness::Ambiguous),
    }
}

async fn serve_gallery_connection(
    stream: UnixStream,
    session: Arc<Session>,
    app: Arc<GalleryApp>,
    shutdown: CancellationToken,
) {
    let mut connection = Box::pin(handle_connection(stream, session, shutdown));

    let result = tokio::select! {
        biased;
        result = &mut connection => result,
        _ = tokio::task::yield_now() => {
            if let Err(error) = app.refresh_connection_telemetry() {
                warn!("failed to refresh telemetry after client attach: {error}");
            }
            connection.await
        }
    };

    if let Err(error) = app.refresh_connection_telemetry() {
        warn!("failed to refresh telemetry after client detach: {error}");
    }
    if let Err(error) = result {
        warn!("client connection ended: {error}");
    }
}

enum ServerActivity {
    Accepted(std::io::Result<(UnixStream, tokio::net::unix::SocketAddr)>),
    TaskFinished(Result<(), tokio::task::JoinError>),
}

async fn next_server_activity(listener: &UnixListener, tasks: &mut JoinSet<()>) -> ServerActivity {
    tokio::select! {
        Some(result) = tasks.join_next(), if !tasks.is_empty() => {
            ServerActivity::TaskFinished(result)
        }
        accept_result = listener.accept() => ServerActivity::Accepted(accept_result),
    }
}

fn require_gallery_uid(uid: u32) -> std::io::Result<()> {
    require_unprivileged_uid(uid, "ui-gallery")
}

async fn bind_owned_socket(path: &Path, uid: u32) -> std::io::Result<(UnixListener, OwnedSocket)> {
    let parent = prepare_private_socket_parent(path, uid)?;
    let lock = acquire_socket_lock(path)?;

    match probe_socket_liveness(path, &parent).await? {
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

    if parent.socket_identity()?.is_some() {
        info!("removing stale socket at {}", path.display());
        parent.remove_socket()?;
    }

    let listener = UnixListener::from_std(parent.bind()?)?;
    let identity = parent.socket_identity()?.ok_or_else(|| {
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
            parent,
            _lock: lock,
        },
    ))
}

async fn run_autoplay(
    app: Arc<GalleryApp>,
    interval_duration: Duration,
    shutdown: CancellationToken,
) {
    let mut autoplay = app.autoplay_updates();

    loop {
        while !*autoplay.borrow_and_update() {
            tokio::select! {
                changed = autoplay.changed() => {
                    if changed.is_err() {
                        return;
                    }
                }
                _ = shutdown.cancelled() => return,
            }
        }

        let dwell = tokio::time::sleep(interval_duration);
        tokio::pin!(dwell);
        tokio::select! {
            _ = &mut dwell => {
                if *autoplay.borrow() {
                    if let Err(error) = app.next_scene() {
                        warn!("autoplay transaction failed: {error}");
                    }
                }
            }
            changed = autoplay.changed() => {
                if changed.is_err() {
                    return;
                }
            }
            _ = shutdown.cancelled() => return,
        }
    }
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
    let options = match parse_options(&args) {
        Ok(ParseOutcome::Run(options)) => options,
        Ok(ParseOutcome::Help) => {
            println!("{USAGE}");
            return Ok(());
        }
        Err(message) => {
            eprintln!("{message}");
            eprintln!("{USAGE}");
            std::process::exit(2);
        }
    };

    let uid = effective_uid();
    require_gallery_uid(uid)?;
    let (listener, owned_socket) = bind_owned_socket(&options.socket_path, uid).await?;
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
    tasks.spawn(run_autoplay(
        app.clone(),
        options.autoplay_interval,
        shutdown.clone(),
    ));

    loop {
        tokio::select! {
            activity = next_server_activity(&listener, &mut tasks) => {
                match activity {
                    ServerActivity::Accepted(Ok((stream, _peer))) => {
                        if let Err(error) = validate_peer(&stream, uid) {
                            warn!(
                                error = %error,
                                "rejecting Unix socket peer outside the authenticated user boundary"
                            );
                            continue;
                        }
                        let child = shutdown.child_token();
                        tasks.spawn(serve_gallery_connection(
                            stream,
                            session.clone(),
                            app.clone(),
                            child,
                        ));
                    }
                    ServerActivity::Accepted(Err(error)) => error!("accept failed: {error}"),
                    ServerActivity::TaskFinished(Err(error)) => {
                        warn!("task ended abnormally: {error}");
                    }
                    ServerActivity::TaskFinished(Ok(())) => {}
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
    drop(owned_socket);
    info!("ui gallery shutdown complete");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temporary_socket_path(label: &str) -> (PathBuf, PathBuf) {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is after the Unix epoch")
            .as_nanos();
        let directory = PathBuf::from("/tmp").join(format!(
            "srui-gallery-{}-{label}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir(&directory).expect("temporary socket directory is created");
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
            .expect("temporary directory becomes private");
        let path = directory.join("gallery.sock");
        (directory, path)
    }

    #[test]
    fn short_and_long_help_flags_request_successful_help() {
        for flag in ["-h", "--help"] {
            let parsed = parse_options(&[flag.to_string()]).expect("help parses");
            assert!(matches!(parsed, ParseOutcome::Help));
        }
    }

    #[test]
    fn root_uid_is_refused_before_socket_binding() {
        let error = require_gallery_uid(0).expect_err("root must be refused");
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
        assert!(error
            .to_string()
            .contains("ui-gallery refuses to run as root"));
    }

    #[tokio::test]
    async fn completed_tasks_are_reaped_while_accept_is_idle() {
        let (directory, path) = temporary_socket_path("task-reaping");
        let (listener, owned) = bind_owned_socket(&path, effective_uid())
            .await
            .expect("socket binds");
        let mut tasks = JoinSet::new();
        tasks.spawn(async {});

        let activity = tokio::time::timeout(
            Duration::from_secs(1),
            next_server_activity(&listener, &mut tasks),
        )
        .await
        .expect("completed task is reaped without an incoming connection");
        assert!(matches!(activity, ServerActivity::TaskFinished(Ok(()))));
        assert!(
            tasks.is_empty(),
            "completed task is removed from the JoinSet"
        );

        drop(listener);
        drop(owned);
        std::fs::remove_dir(directory).expect("temporary socket directory is removed");
    }

    #[tokio::test]
    async fn bound_socket_is_private_validates_peers_and_cleans_owned_paths() {
        let (directory, path) = temporary_socket_path("security");
        let lock_path = socket_lock_path(&path);
        let uid = effective_uid();

        let (listener, owned) = bind_owned_socket(&path, uid).await.expect("socket binds");
        let mode = std::fs::symlink_metadata(&path)
            .expect("bound socket has metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, srui_unix_security::PRIVATE_SOCKET_MODE);

        let client = UnixStream::connect(&path)
            .await
            .expect("same-user client connects");
        let (server, _) = listener.accept().await.expect("server accepts peer");
        validate_peer(&server, uid).expect("same-user peer is authenticated");
        drop(client);
        drop(server);
        drop(listener);
        drop(owned);

        assert!(!path.exists(), "owned socket is removed on orderly drop");
        assert!(!lock_path.exists(), "owned lock is removed on orderly drop");
        std::fs::remove_dir(directory).expect("temporary socket directory is removed");
    }

    #[tokio::test]
    async fn shutdown_does_not_remove_a_replacement_lock_file() {
        let (directory, path) = temporary_socket_path("replacement-lock");
        let lock_path = socket_lock_path(&path);
        let (listener, owned) = bind_owned_socket(&path, effective_uid())
            .await
            .expect("socket binds");

        std::fs::remove_file(&lock_path).expect("test unlinks the owned lock name");
        std::fs::write(&lock_path, b"replacement").expect("replacement lock is created");

        drop(listener);
        drop(owned);
        assert_eq!(
            std::fs::read(&lock_path).expect("replacement lock survives"),
            b"replacement"
        );

        std::fs::remove_file(lock_path).expect("replacement lock is removed");
        std::fs::remove_dir(directory).expect("temporary socket directory is removed");
    }

    #[tokio::test(start_paused = true)]
    async fn autoplay_waits_a_full_dwell_after_enable_and_reenable() {
        let app = GalleryApp::start(Arc::new(Session::mint())).expect("gallery starts");
        let shutdown = CancellationToken::new();
        let task = tokio::spawn(run_autoplay(
            app.clone(),
            Duration::from_secs(4),
            shutdown.clone(),
        ));

        app.set_autoplay(true).expect("autoplay enables");
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(3_999)).await;
        tokio::task::yield_now().await;
        assert_eq!(app.scene(), srui_example_ui_gallery::Scene::Baseline);
        tokio::time::advance(Duration::from_millis(1)).await;
        tokio::task::yield_now().await;
        assert_eq!(app.scene(), srui_example_ui_gallery::Scene::Content);

        app.set_autoplay(false).expect("autoplay disables");
        tokio::time::advance(Duration::from_secs(8)).await;
        tokio::task::yield_now().await;
        assert_eq!(app.scene(), srui_example_ui_gallery::Scene::Content);

        app.set_autoplay(true).expect("autoplay re-enables");
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(3_999)).await;
        tokio::task::yield_now().await;
        assert_eq!(app.scene(), srui_example_ui_gallery::Scene::Content);
        tokio::time::advance(Duration::from_millis(1)).await;
        tokio::task::yield_now().await;
        assert_eq!(app.scene(), srui_example_ui_gallery::Scene::State);

        shutdown.cancel();
        task.await.expect("autoplay task exits cleanly");
    }

    fn attached_clients_text(session: &Session) -> String {
        let node = session
            .get_node(srui_example_ui_gallery::ids::CONN_CLIENTS)
            .expect("connection telemetry node exists");
        srui_sdk::Text::text_of(&node)
            .expect("connection telemetry is text")
            .to_string()
    }

    #[tokio::test]
    async fn connection_lifecycle_publishes_attach_and_detach_counts() {
        let session = Arc::new(Session::mint());
        let app = GalleryApp::start(session.clone()).expect("gallery starts");
        let (client, server) = UnixStream::pair().expect("socket pair opens");
        let shutdown = CancellationToken::new();

        let connection = tokio::spawn(serve_gallery_connection(
            server,
            session.clone(),
            app,
            shutdown,
        ));

        tokio::time::timeout(Duration::from_secs(1), async {
            while attached_clients_text(&session) != "Attached clients: 1" {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("attach telemetry is published");

        drop(client);
        tokio::time::timeout(Duration::from_secs(1), connection)
            .await
            .expect("connection exits after peer disconnects")
            .expect("connection task does not panic");

        assert_eq!(
            attached_clients_text(&session),
            "Attached clients: 0",
            "the transaction after AttachmentGuard drops must publish the detached count"
        );
    }
}
