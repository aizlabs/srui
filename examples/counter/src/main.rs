//! Runnable binary demonstrating the SRUI Counter Example (§20.2, §29).
//!
//! Modes:
//! - Default: in-process simulated clicks demo.
//! - Server: `--socket <path>` binds a Unix domain socket and hosts the counter application.
//! - Server: `--port <port>` binds a TCP loopback socket and hosts the counter application.
//! - Opt-in: `--image-fixture` publishes a deterministic 1×1 PNG and mounts an Image node
//!   referencing its `ResourceHash` for cross-language resource integration tests (§14).
//! - Opt-in: `--terminal-fixture` adds a required Terminal extension node and spawns a
//!   trusted PTY (`/bin/sh -i` by default). There is no automatic tmux redraw after the
//!   output ring is lost.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{TcpListener, UnixListener};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use srui_example_counter::CounterApp;
use srui_sdk::*;
use srui_semantic_tree::{ItemId, ModelId, ModelItem, Operation, TypeRef, Value};
use srui_sessiond::{handle_connection, ModelRangeProvider, Session, TerminalSpec};
use srui_unix_security::{effective_uid, prepare_private_socket_parent, secure_bound_socket};

/// Deterministic valid 1×1 RGB PNG (69 bytes); shared with Swift ResourceCacheTests.
fn fixture_png() -> Vec<u8> {
    vec![
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
        0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x60,
        0x60, 0x60, 0x00, 0x00, 0x00, 0x04, 0x00, 0x01, 0xF6, 0x17, 0x38, 0x55, 0x00, 0x00, 0x00,
        0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ]
}

fn parse_server_capabilities(args: &[String]) -> ServerCapabilities {
    let mut server_caps = ServerCapabilities::standard_widgets();
    if let Some(pos) = args.iter().position(|a| a == "--require-profile") {
        if let Some(profile_str) = args.get(pos + 1) {
            let profile = Profile::parse(profile_str).expect("valid profile syntax");
            server_caps.required.insert(profile);
        }
    }
    server_caps
}

fn wants_image_fixture(args: &[String]) -> bool {
    args.iter().any(|a| a == "--image-fixture")
}

fn wants_large_collection_fixture(args: &[String]) -> bool {
    args.iter().any(|a| a == "--large-collection-fixture")
}

fn wants_terminal_fixture(args: &[String]) -> bool {
    args.iter().any(|a| a == "--terminal-fixture")
}

fn terminal_ring_capacity(args: &[String]) -> usize {
    args.iter()
        .position(|a| a == "--terminal-ring-bytes")
        .and_then(|pos| args.get(pos + 1))
        .and_then(|value| value.parse().ok())
        .unwrap_or(1024 * 1024)
}

fn terminal_command(args: &[String]) -> TerminalSpec {
    let mut spec = TerminalSpec::interactive_shell();
    spec.ring_capacity = terminal_ring_capacity(args);
    if let Some(pos) = args.iter().position(|a| a == "--terminal-command") {
        if let Some(command) = args.get(pos + 1) {
            spec.executable = command.into();
            spec.args = args
                .iter()
                .skip(pos + 2)
                .take_while(|arg| !arg.starts_with("--"))
                .cloned()
                .collect();
        }
    }
    spec
}

fn collection_provider_delay() -> Duration {
    std::env::var("SRUI_COLLECTION_PROVIDER_DELAY_MS")
        .ok()
        .and_then(|value| value.parse().ok())
        .map(Duration::from_millis)
        .unwrap_or(Duration::ZERO)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let capabilities = parse_server_capabilities(&args);
    let image_fixture = wants_image_fixture(&args);
    let large_collection = wants_large_collection_fixture(&args);
    let terminal_fixture = wants_terminal_fixture(&args);
    let terminal_spec = terminal_command(&args);

    if let Some(pos) = args.iter().position(|a| a == "--socket") {
        if let Some(socket_path_str) = args.get(pos + 1) {
            let socket_path = PathBuf::from(socket_path_str);
            run_unix_server(
                socket_path,
                capabilities,
                image_fixture,
                large_collection,
                terminal_fixture,
                terminal_spec,
            )
            .await?;
            return Ok(());
        }
    }

    if let Some(pos) = args.iter().position(|a| a == "--port") {
        if let Some(port_str) = args.get(pos + 1) {
            let port: u16 = port_str.parse().expect("valid port number");
            run_tcp_server(
                port,
                capabilities,
                image_fixture,
                large_collection,
                terminal_fixture,
                terminal_spec,
            )
            .await?;
            return Ok(());
        }
    }

    // Default in-process run
    println!("=== SRUI Counter Example ===");
    let app = CounterApp::new().expect("failed to initialize CounterApp");

    println!("Initial UI State (Revision {}):", app.current_revision());
    println!("  Text:     {}", app.get_text().unwrap_or_default());
    println!(
        "  Progress: {:.2} ({})",
        app.get_progress().unwrap_or(0.0),
        app.get_progress_description().unwrap_or_default()
    );

    // Dispatch 5 simulated button clicks
    for seq in 1..=5 {
        app.click(seq).expect("click dispatch failed");
        println!("After Click {} (Revision {}):", seq, app.current_revision());
        println!("  Text:     {}", app.get_text().unwrap_or_default());
        println!(
            "  Progress: {:.2} ({})",
            app.get_progress().unwrap_or(0.0),
            app.get_progress_description().unwrap_or_default()
        );
    }

    println!("=== Counter Example completed successfully! ===");
    Ok(())
}

fn initialize_counter_session(
    session: &Arc<Session>,
    image_fixture: bool,
) -> (NodeId, NodeId, NodeId, NodeId) {
    let surface_id = NodeId::new(1);
    let text_id = NodeId::new(2);
    let progress_id = NodeId::new(3);
    let button_id = NodeId::new(4);
    let image_id = NodeId::new(5);

    let image_hash = if image_fixture {
        Some(
            session
                .publish_resource(fixture_png())
                .expect("publish image fixture")
                .hash,
        )
    } else {
        None
    };

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

            if let Some(hash) = image_hash {
                Image::builder(image_id)
                    .parent(surface_id)
                    .label("Fixture Image")
                    .resource(hash)
                    .create(ui)?;
            }

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

fn initialize_terminal_fixture(session: &Arc<Session>, spec: TerminalSpec) {
    let terminal_id = NodeId::new(30);
    session
        .create_terminal_node(terminal_id, NodeId::new(1), spec.clone())
        .expect("initialize terminal fixture");
    info!(
        "Terminal fixture ready: node={} command={} ring_bytes={} (v1 has no redraw backend / tmux integration)",
        terminal_id.get(),
        spec.executable.display(),
        spec.ring_capacity
    );
}

async fn initialize_large_collection_fixture(session: &Arc<Session>) {
    let table_id = NodeId::new(10);
    let model_id = ModelId::new(1);
    const ITEM_COUNT: u64 = 500_000;
    const PRELOAD: u64 = 64;

    session
        .transaction(|ui| {
            ui.apply_op(&Operation::create_model(
                model_id,
                TypeRef::TABLE,
                ITEM_COUNT,
            ))?;
            Table::builder(table_id)
                .parent(NodeId::new(1))
                .label("Large Collection")
                .model_ref(model_id)
                .columns(["Index", "Label"])
                .create(ui)?;
            Ok(())
        })
        .expect("initialize large collection");

    let delay = collection_provider_delay();
    let provider: ModelRangeProvider = Arc::new(move |query| {
        Box::pin(async move {
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            Ok((0..query.count)
                .map(|offset| {
                    let index = query.start_index + offset;
                    ModelItem::with_value(
                        ItemId::new(index + 1),
                        Value::List(vec![
                            Value::String(index.to_string()),
                            Value::String(format!("Row {index}")),
                        ]),
                    )
                })
                .collect())
        })
    });
    session.register_model_range_provider(model_id, provider);
    session
        .push_visible_model_range(table_id, model_id, 0, PRELOAD)
        .await
        .expect("proactive visible range");
    info!(
        "Large collection fixture ready: model={} items={} preloaded={}",
        model_id.get(),
        ITEM_COUNT,
        PRELOAD
    );
}

async fn run_unix_server(
    socket_path: PathBuf,
    capabilities: ServerCapabilities,
    image_fixture: bool,
    large_collection: bool,
    terminal_fixture: bool,
    terminal_spec: TerminalSpec,
) -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    info!("Starting SRUI Counter Unix Socket Server...");

    let uid = effective_uid();
    prepare_private_socket_parent(&socket_path, uid)?;
    if socket_path.exists() {
        let _ = std::fs::remove_file(&socket_path);
    }

    let listener = UnixListener::bind(&socket_path)?;
    secure_bound_socket(&socket_path, uid)?;
    info!("Listening on Unix domain socket: {:?}", socket_path);

    let session = Arc::new(Session::with_capabilities(
        "counter-socket-session",
        capabilities,
    ));
    let _ = initialize_counter_session(&session, image_fixture);
    if large_collection {
        initialize_large_collection_fixture(&session).await;
    }
    if terminal_fixture {
        initialize_terminal_fixture(&session, terminal_spec);
    }

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

async fn run_tcp_server(
    port: u16,
    capabilities: ServerCapabilities,
    image_fixture: bool,
    large_collection: bool,
    terminal_fixture: bool,
    terminal_spec: TerminalSpec,
) -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let addr = format!("127.0.0.1:{}", port);
    let listener = TcpListener::bind(&addr).await?;
    info!("Listening on TCP loopback: {}", addr);

    let session = Arc::new(Session::with_capabilities(
        "counter-tcp-session",
        capabilities,
    ));
    let _ = initialize_counter_session(&session, image_fixture);
    if large_collection {
        initialize_large_collection_fixture(&session).await;
    }
    if terminal_fixture {
        initialize_terminal_fixture(&session, terminal_spec);
    }

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
