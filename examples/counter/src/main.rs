//! Runnable binary demonstrating the SRUI Counter Example (§20.2, §29).
//!
//! Modes:
//! - Default: in-process simulated clicks demo.
//! - Server: `--socket <path>` binds a Unix domain socket and hosts the counter application.
//! - Server: `--port <port>` binds a TCP loopback socket and hosts the counter application.

use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::{TcpListener, UnixListener};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use srui_example_counter::CounterApp;
use srui_sdk::*;
use srui_sessiond::{handle_connection, Session};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();

    if let Some(pos) = args.iter().position(|a| a == "--socket") {
        if let Some(socket_path_str) = args.get(pos + 1) {
            let socket_path = PathBuf::from(socket_path_str);
            run_unix_server(socket_path).await?;
            return Ok(());
        }
    }

    if let Some(pos) = args.iter().position(|a| a == "--port") {
        if let Some(port_str) = args.get(pos + 1) {
            let port: u16 = port_str.parse().expect("valid port number");
            run_tcp_server(port).await?;
            return Ok(());
        }
    }

    // Default in-process run
    println!("=== SRUI Counter Example ===");
    let app = CounterApp::new().expect("failed to initialize CounterApp");

    println!("Initial UI State (Revision {}):", app.current_revision());
    println!("  Text:     {}", app.get_text().unwrap_or_default());
    println!("  Progress: {:.2} ({})", app.get_progress().unwrap_or(0.0), app.get_progress_description().unwrap_or_default());

    // Dispatch 5 simulated button clicks
    for seq in 1..=5 {
        app.click(seq).expect("click dispatch failed");
        println!("After Click {} (Revision {}):", seq, app.current_revision());
        println!("  Text:     {}", app.get_text().unwrap_or_default());
        println!("  Progress: {:.2} ({})", app.get_progress().unwrap_or(0.0), app.get_progress_description().unwrap_or_default());
    }

    println!("=== Counter Example completed successfully! ===");
    Ok(())
}

fn initialize_counter_session(session: &Arc<Session>) -> (NodeId, NodeId, NodeId, NodeId) {
    let surface_id = NodeId::new(1);
    let text_id = NodeId::new(2);
    let progress_id = NodeId::new(3);
    let button_id = NodeId::new(4);

    // Initial transaction (Revision 0 -> 1)
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

    // Register ACTIVATE event handler on button (§7.6, §29)
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

    (surface_id, text_id, progress_id, button_id)
}

async fn run_unix_server(socket_path: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    info!("Starting SRUI Counter Unix Socket Server...");

    if socket_path.exists() {
        let _ = std::fs::remove_file(&socket_path);
    }
    if let Some(parent) = socket_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let listener = UnixListener::bind(&socket_path)?;
    info!("Listening on Unix domain socket: {:?}", socket_path);

    let session = Arc::new(Session::new("counter-socket-session"));
    let _ = initialize_counter_session(&session);

    let shutdown = CancellationToken::new();

    loop {
        tokio::select! {
            accept_result = listener.accept() => {
                match accept_result {
                    Ok((stream, _)) => {
                        let session_clone = session.clone();
                        let shutdown_child = shutdown.child_token();
                        tokio::spawn(async move {
                            if let Err(e) = handle_connection(stream, session_clone, shutdown_child).await {
                                warn!("Client connection ended: {}", e);
                            }
                        });
                    }
                    Err(e) => {
                        warn!("Accept error: {}", e);
                    }
                }
            }
            _ = tokio::signal::ctrl_c() => {
                info!("Received interrupt signal, stopping server...");
                break;
            }
        }
    }

    let _ = std::fs::remove_file(&socket_path);
    Ok(())
}

async fn run_tcp_server(port: u16) -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let addr = format!("127.0.0.1:{}", port);
    let listener = TcpListener::bind(&addr).await?;
    info!("Listening on TCP loopback: {}", addr);

    let session = Arc::new(Session::new("counter-tcp-session"));
    let _ = initialize_counter_session(&session);

    let shutdown = CancellationToken::new();

    loop {
        tokio::select! {
            accept_result = listener.accept() => {
                match accept_result {
                    Ok((stream, _)) => {
                        let session_clone = session.clone();
                        let shutdown_child = shutdown.child_token();
                        tokio::spawn(async move {
                            if let Err(e) = handle_connection(stream, session_clone, shutdown_child).await {
                                warn!("Client connection ended: {}", e);
                            }
                        });
                    }
                    Err(e) => {
                        warn!("Accept error: {}", e);
                    }
                }
            }
            _ = tokio::signal::ctrl_c() => {
                info!("Received interrupt signal, stopping server...");
                break;
            }
        }
    }

    Ok(())
}
