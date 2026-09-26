//! Existing sessiond runtime/SSH bridge host for the PX-001 shell (§§12, 20, 27, 29).
use srui_process_explorer::{
    initialize,
    procfs::ProcFsSource,
    refresh::{poll, DEFAULT_REFRESH_INTERVAL, MAX_REFRESH_INTERVAL, MIN_REFRESH_INTERVAL},
    source::{FakeProcessSource, ProcessSource, ScriptedFakeSource},
    start_from_source, update_title, FIXTURE_TITLE,
};
use srui_sessiond::{handle_connection, Session};
use srui_unix_security::{
    effective_uid, prepare_private_socket_parent, require_unprivileged_uid, validate_peer,
};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::{net::UnixListener, task::JoinSet};
use tokio_util::sync::CancellationToken;

/// Which process source, if any, this instance collects from.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Collection {
    /// No collection at all: the empty shell fixture.
    None,
    Fake,
    Sequence,
    Live,
}

/// Publishes the first snapshot and starts the refresh loop for one source.
fn collect<S>(
    session: &Arc<Session>,
    mut source: S,
    interval: Duration,
    shutdown: &CancellationToken,
    connections: &mut JoinSet<()>,
) -> Result<(), Box<dyn std::error::Error>>
where
    S: ProcessSource + Send + 'static,
{
    let (view, _) = start_from_source(session, &mut source)?;
    let session = Arc::clone(session);
    let shutdown = shutdown.child_token();
    connections.spawn(poll(view, source, session, interval, shutdown));
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    if args.next().as_deref() != Some(std::ffi::OsStr::new("--socket")) {
        return Err("usage: srtop --socket PATH [--smoke-fixture] \
             [--fake-source | --fake-sequence | --live-source] [--refresh-interval-ms MS]"
            .into());
    }
    let socket_path = PathBuf::from(args.next().ok_or("missing --socket PATH")?);
    let mut fixture = false;
    let mut collection = Collection::None;
    let mut interval = None;
    let mut args = args.peekable();
    while let Some(value) = args.next() {
        let select = |current: &mut Collection, chosen| {
            if *current != Collection::None {
                return Err("choose one of --fake-source, --fake-sequence or --live-source");
            }
            *current = chosen;
            Ok(())
        };
        match value.to_str() {
            Some("--smoke-fixture") if !fixture => fixture = true,
            Some("--fake-source") => select(&mut collection, Collection::Fake)?,
            Some("--fake-sequence") => select(&mut collection, Collection::Sequence)?,
            Some("--live-source") => select(&mut collection, Collection::Live)?,
            Some("--refresh-interval-ms") if interval.is_none() => {
                let milliseconds: u64 = args
                    .next()
                    .ok_or("missing --refresh-interval-ms MS")?
                    .to_str()
                    .ok_or("--refresh-interval-ms MS must be a number")?
                    .parse()
                    .map_err(|_| "--refresh-interval-ms MS must be a number")?;
                let chosen = Duration::from_millis(milliseconds);
                if !(MIN_REFRESH_INTERVAL..=MAX_REFRESH_INTERVAL).contains(&chosen) {
                    return Err(format!(
                        "--refresh-interval-ms must be between {} and {}",
                        MIN_REFRESH_INTERVAL.as_millis(),
                        MAX_REFRESH_INTERVAL.as_millis()
                    )
                    .into());
                }
                interval = Some(chosen);
            }
            _ => return Err("unknown or repeated option".into()),
        }
    }
    let interval = interval.unwrap_or(DEFAULT_REFRESH_INTERVAL);
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
    let shutdown = CancellationToken::new();
    let mut connections = JoinSet::new();
    // Each collecting mode publishes its first snapshot and then refreshes the
    // same rows in place on every tick; the empty shell has nothing to poll.
    let initialized: Result<(), Box<dyn std::error::Error>> = match collection {
        Collection::Fake => collect(
            &session,
            FakeProcessSource,
            interval,
            &shutdown,
            &mut connections,
        ),
        Collection::Sequence => collect(
            &session,
            ScriptedFakeSource::default(),
            interval,
            &shutdown,
            &mut connections,
        ),
        // Read-only sampling of the host's process filesystem; no process
        // controls are installed.
        Collection::Live => collect(
            &session,
            ProcFsSource::live(),
            interval,
            &shutdown,
            &mut connections,
        ),
        Collection::None => initialize(&session).map_err(Into::into),
    };
    // The socket is already published. Sampling a real host can fail where the
    // constant fixture cannot, and leaving the socket behind would make the next
    // start refuse it as another instance's: this instance owns it and removes
    // it on the failure path too.
    if let Err(error) = initialized {
        shutdown.cancel();
        while connections.join_next().await.is_some() {}
        if parent.socket_identity()? == Some(identity) {
            parent.remove_socket()?;
        }
        return Err(error);
    }
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
