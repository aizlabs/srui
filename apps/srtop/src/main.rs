//! Existing sessiond runtime/SSH bridge host for the PX-001 shell (§§12, 20, 27, 29).
use srui_process_explorer::{
    initialize, initialize_from_source, source::FakeProcessSource, update_title, FIXTURE_TITLE,
};
use srui_sessiond::{handle_connection, Session};
use srui_unix_security::{
    effective_uid, prepare_private_socket_parent, require_unprivileged_uid, validate_peer,
};
use std::{path::PathBuf, sync::Arc};
use tokio::{net::UnixListener, task::JoinSet};
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--socket")) {
        return Err("usage: srtop --socket PATH [--smoke-fixture] [--fake-source]".into());
    }
    let socket_path = PathBuf::from(args.next().ok_or("missing --socket PATH")?);
    let mut fixture = false;
    let mut fake_source = false;
    for value in args {
        match value.to_str() {
            Some("--smoke-fixture") if !fixture => fixture = true,
            Some("--fake-source") if !fake_source => fake_source = true,
            _ => return Err("unknown or repeated option".into()),
        }
    }
    let uid = effective_uid();
    require_unprivileged_uid(uid, "srtop")?;
    // Install both handlers before publishing the socket so a ready instance
    // always routes normal stop signals through owned-socket cleanup.
    let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let parent = prepare_private_socket_parent(&socket_path, uid)?;
    // Never replace another instance's socket, including a stale socket.
    let listener = UnixListener::from_std(parent.bind()?)?;
    let identity = parent
        .socket_identity()?
        .ok_or("socket vanished after bind")?;
    let session = Arc::new(Session::mint());
    if fake_source {
        initialize_from_source(&session, &mut FakeProcessSource)?;
    } else {
        initialize(&session)?;
    }
    let shutdown = CancellationToken::new();
    let mut connections = JoinSet::new();
    // Only the explicitly requested fixture installs this local test trigger.
    let mut title_signal = if fixture {
        Some(tokio::signal::unix::signal(
            tokio::signal::unix::SignalKind::user_defined1(),
        )?)
    } else {
        None
    };
    let result: Result<(), Box<dyn std::error::Error>> = loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, _) = match accepted {
                    Ok(value) => value,
                    Err(error) => break Err(error.into()),
                };
                if let Err(error) = validate_peer(&stream, uid) {
                    eprintln!("srtop: rejected peer: {error}");
                    continue;
                }
                let session = Arc::clone(&session);
                let token = shutdown.child_token();
                connections.spawn(async move {
                    if let Err(error) = handle_connection(stream, session, token).await {
                        eprintln!("srtop: connection ended: {error}");
                    }
                });
            }
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
            _ = async {
                match &mut title_signal {
                    Some(signal) => { signal.recv().await; }
                    None => std::future::pending::<()>().await,
                }
            } => {
                if let Err(error) = update_title(&session, FIXTURE_TITLE) {
                    break Err(error.into());
                }
            }
            _ = sigint.recv() => break Ok(()),
            _ = sigterm.recv() => break Ok(()),
        }
    };
    shutdown.cancel();
    while connections.join_next().await.is_some() {}
    if parent.socket_identity()? == Some(identity) {
        parent.remove_socket()?;
    }
    result
}
