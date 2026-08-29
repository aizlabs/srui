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
use srui_sdk::ServerCapabilities;
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
        .join("srui-sessiond.sock")
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

/// Removes a stale socket path, refusing to unlink anything that is not a Unix socket.
fn clear_stale_socket(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::FileTypeExt;

    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_socket() => std::fs::remove_file(path),
        Ok(_) => Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("{} exists and is not a Unix socket", path.display()),
        )),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
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
    let options = parse_options(&args).map_err(|message| {
        error!("{message}");
        message
    })?;

    let session = Arc::new(Session::with_capabilities(
        "process-monitor",
        ServerCapabilities::standard_widgets(),
    ));

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

    let uid = effective_uid();
    let monitor = {
        let session = session.clone();
        tokio::task::spawn_blocking(move || {
            // sysinfo derives CPU utilization from the delta between two refreshes: warm up
            // deliberately so the first published sample is meaningful rather than a zeroed
            // placeholder.
            let source = SysinfoProcessSource::new();
            std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL);
            Monitor::start(
                session,
                Box::new(source),
                Box::new(SignalTerminator),
                Some(uid),
            )
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

    clear_stale_socket(&options.socket_path)?;
    if let Some(parent) = options.socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let listener = UnixListener::bind(&options.socket_path)?;
    info!(
        "listening on Unix domain socket: {}",
        options.socket_path.display()
    );

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
    let _ = clear_stale_socket(&options.socket_path);
    info!("process monitor shutdown complete");
    Ok(())
}
