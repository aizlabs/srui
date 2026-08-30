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

use srui_sessiond::{handle_connection, Session, SessionConfig};

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
/// Parsed command line: socket path, optional built-in app adapter, and the §18.1 journal
/// retention window (maximum retained transaction count).
struct Args {
    socket_path: PathBuf,
    app_name: Option<String>,
    journal_capacity: usize,
}

fn parse_args() -> Result<Args, String> {
    let args: Vec<String> = std::env::args().collect();
    let mut socket_path = None;
    let mut app_name = None;
    let mut journal_capacity = SessionConfig::default().journal_capacity;

    let mut i = 1;
    while i < args.len() {
        if args[i] == "--socket" && i + 1 < args.len() {
            socket_path = Some(PathBuf::from(&args[i + 1]));
            i += 2;
        } else if args[i] == "--app" && i + 1 < args.len() {
            app_name = Some(args[i + 1].clone());
            i += 2;
        } else if args[i] == "--journal-capacity" {
            // A zero or unparsable retention window would silently degrade every reconnect to a
            // snapshot resync, so refuse to start instead of clamping (§18.1).
            let raw = args
                .get(i + 1)
                .ok_or_else(|| "--journal-capacity requires a value".to_string())?;
            journal_capacity = raw
                .parse::<usize>()
                .ok()
                .filter(|n| *n > 0)
                .ok_or_else(|| {
                    format!("--journal-capacity must be a positive integer, got {raw}")
                })?;
            i += 2;
        } else if !args[i].starts_with('-') && socket_path.is_none() {
            socket_path = Some(PathBuf::from(&args[i]));
            i += 1;
        } else {
            i += 1;
        }
    }

    Ok(Args {
        socket_path: socket_path.unwrap_or_else(default_socket_path),
        app_name,
        journal_capacity,
    })
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
            info!("Counter incremented to {}", next_val);
            Ok(())
        })
        .expect("counter increment transaction failed");
    });
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

    let Args {
        socket_path,
        app_name,
        journal_capacity,
    } = parse_args()?;

    if socket_path.exists() {
        let _ = std::fs::remove_file(&socket_path);
    }

    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let listener = UnixListener::bind(&socket_path)?;
    info!("Listening on Unix domain socket: {:?}", socket_path);

    // Mint a fresh, globally unique session incarnation token (§17)
    let session = Arc::new(Session::mint_with_config(SessionConfig {
        journal_capacity,
        ..SessionConfig::default()
    }));
    info!(
        journal_capacity,
        "Minted session incarnation token {} (initial state: {:?}, journal retention: {} transactions)",
        session.session_id(),
        session.state(),
        journal_capacity
    );

    if let Some(app) = app_name.as_deref() {
        match app {
            "counter" => {
                info!("Initializing built-in counter application adapter (§20.2, §29)...");
                initialize_counter_app(&session);
            }
            other => {
                warn!("Unknown application adapter: {}", other);
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
    let _ = std::fs::remove_file(&socket_path);
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
