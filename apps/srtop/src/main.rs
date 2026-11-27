//! Existing sessiond runtime/SSH bridge host for the PX-001 shell (§§12, 20, 27, 29).
//!
//! `--journal-capacity N` sets the §18.1 retention window of this instance's
//! one session (PX-022): how many committed transactions a reconnecting client
//! can still be replayed (`RESUME_OK`) before it has to be resynchronized from
//! a snapshot (`RESYNC_REQUIRED{SAME_SESSION}`). The continuity decision is the
//! runtime's (`srui_sessiond`); srtop only configures the window. Every start
//! mints a new incarnation and restores nothing, so a client resuming an
//! earlier instance is answered `REPLACED` (§§17–18).
//!
//! `--actions read-only|dry-run|terminate` adds the action controls under the
//! inspector (PX-028, PX-030, §§7.7, 27). In `read-only` the server refuses
//! every action and the controls are disabled; in `dry-run` Prepare and
//! Confirm are authorized in full and handed to the dry-run executor, which
//! sends nothing; in `terminate` a confirmed intent is handed to the PID file
//! descriptor terminator, which sends SIGTERM to exactly the confirmed
//! process instance, and where this host has no PID file descriptors the
//! controls say so and stay disabled. Without the option no action control or
//! handler exists, and the package stays read-only. `--protect-pid PID`
//! (repeatable, at most 64, only with `--actions`) adds a PID no action may
//! target, besides PID 1 and the support chain the server derives itself.
//!
//! Saved views (PX-069) live in this user's settings file on the server:
//! with `--live-source` it is [`settings_file::default_path`]
//! (`$XDG_CONFIG_HOME/srtop/settings`, or the platform's configuration
//! directory), `--settings-path PATH` names another file, and
//! `--no-settings` keeps none. The fixture sources keep none unless
//! `--settings-path` is given, so no test run reads or writes the real
//! user's settings. Without a settings file the view switcher is not created.
use srui_process_explorer::{
    actions::{
        ActionGate, ActionMode, DryRunExecutor, ProcessActions, DEFAULT_PROTECTED_PIDS,
        MAX_CONFIGURED_PROTECTED_PIDS,
    },
    initialize, install_actions, install_column_handlers, install_filter_handlers,
    install_follow_handlers, install_inspector, install_owner_handlers, install_sampling_handlers,
    install_saved_views, install_search_handlers, install_sort_handlers, install_tag_handlers,
    install_thread_handlers, install_view_handlers,
    procfs::ProcFsSource,
    refresh::{poll, ProcessView, DEFAULT_REFRESH_INTERVAL},
    sampling::parse_interval,
    saved_views::SavedViews,
    settings_file::{self, SettingsStore},
    source::{
        FakeProcessSource, FollowSource, ProcessSource, RepeatedNamesSource, ScriptedFakeSource,
        SyntheticProcessSource, ThreadsSource, TreeSource,
    },
    support::ServerSupportChain,
    terminate::PidfdTerminator,
    update_title, FIXTURE_TITLE,
};
use srui_sessiond::{handle_connection, Session, SessionConfig};
use srui_unix_security::{
    effective_uid, prepare_private_socket_parent, require_unprivileged_uid, validate_peer,
};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{net::UnixListener, task::JoinSet};
use tokio_util::sync::CancellationToken;

/// Which process source, if any, this instance collects from.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Collection {
    /// No collection at all: the empty shell fixture.
    None,
    Fake,
    /// The PX-011 fixture: three processes named exactly `worker`.
    RepeatedNames,
    /// The PX-017 fixture: one process that changes CPU rank on every sample,
    /// then ends while a new process reuses its PID.
    Follow,
    /// The PX-018 fixture: a small process forest with a missing parent, a
    /// reused parent PID and a cycle, whose `make` ends and is reparented.
    Tree,
    /// The PX-019 fixture: user processes with threads, kernel threads, a
    /// process whose threads are denied, and a thread that ends and a TID
    /// that is reused.
    Threads,
    Sequence,
    Live,
    Scale(usize),
}

/// How this instance's action controls are set up, when it has them.
struct Actions {
    mode: ActionMode,
    /// The PIDs `--protect-pid` named.
    protected: Vec<u32>,
    /// The processes serving this session, with every connected transport
    /// registered as it connects (PX-030).
    support: Arc<ServerSupportChain>,
}

/// Where saved views are kept (PX-069).
#[derive(Clone, PartialEq, Eq)]
enum SettingsChoice {
    /// The default path for the live source; none for a fixture.
    Default,
    Path(PathBuf),
    Disabled,
}

/// Publishes the first snapshot and starts the refresh loop for one source.
fn collect<S>(
    session: &Arc<Session>,
    mut source: S,
    interval: Duration,
    settings: Option<&std::path::Path>,
    actions: Option<&Actions>,
    shutdown: &CancellationToken,
    connections: &mut JoinSet<()>,
) -> Result<(), Box<dyn std::error::Error>>
where
    S: ProcessSource + Send + 'static,
{
    // The sampling controls state the interval the loop polls at from the
    // first transaction (PX-015).
    let (view, snapshot) = ProcessView::start_with_interval(session, &mut source, interval)?;
    // The refresh loop and the sort buttons' handlers share the published view
    // (PX-009). The handlers are installed here, before the socket is served,
    // so a ready instance always answers a click.
    let view = Arc::new(Mutex::new(view));
    install_sort_handlers(session, &view);
    install_filter_handlers(session, &view);
    install_column_handlers(session, &view);
    install_search_handlers(session, &view);
    install_owner_handlers(session, &view);
    // The sampling controls record requests on the view and wake the one
    // refresh loop below; they start no timer of their own (PX-015).
    install_sampling_handlers(session, &view);
    // Follow selected process keeps the followed row in view through the
    // table's `keep_visible` hint (PX-017, PX-017-G01).
    install_follow_handlers(session, &view);
    // The view switch moves the same rows into the process tree, whose
    // disclosure stays the client's (PX-018).
    install_view_handlers(session, &view);
    // The thread controls choose which tasks the collection holds, and the
    // loop reads user threads only while they are shown (PX-019).
    install_thread_handlers(session, &view);
    // The tag buttons mark the selected process, or it and its sampled
    // descendants, by process instance; they touch no process (PX-020).
    install_tag_handlers(session, &view);
    // The inspector reads details through its own reader of the source's
    // records, on its own bounded worker (PX-013).
    install_inspector(session, &view, source.details());
    // The saved views apply the active view before the socket is served and
    // before any action control exists (PX-069). Their file is scoped to this
    // source's identity.
    if let Some(path) = settings {
        let saved = Arc::new(Mutex::new(SavedViews::open(
            SettingsStore::new(path),
            &snapshot.source.0,
        )));
        install_saved_views(session, &view, &saved)?;
    }
    // The action controls act on the inspector's resolved selection (PX-028),
    // through the dry-run executor, or through the one PID file descriptor
    // terminator of `--actions terminate` (PX-030).
    if let Some(actions) = actions {
        let executor: Arc<dyn ProcessActions> = match actions.mode {
            ActionMode::Terminate => Arc::new(PidfdTerminator::live()),
            ActionMode::ReadOnly | ActionMode::DryRun => Arc::new(DryRunExecutor::default()),
        };
        let gate = ActionGate::new(actions.mode, executor)
            .with_protected_pids(
                DEFAULT_PROTECTED_PIDS
                    .into_iter()
                    .chain(actions.protected.iter().copied()),
            )
            .with_support_chain(Arc::clone(&actions.support) as _);
        install_actions(session, &view, &Arc::new(Mutex::new(gate)))?;
    }
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
             [--fake-source | --fake-repeated-names | --fake-follow | --fake-tree | --fake-threads \
             | --fake-sequence \
             | --live-source | --procfs | --scale-source COUNT] \
             [--refresh-interval-ms MS | --scale-interval-ms MS] [--journal-capacity N] \
             [--settings-path PATH | --no-settings] \
             [--actions read-only|dry-run|terminate [--protect-pid PID]...]"
            .into());
    }
    let socket_path = PathBuf::from(args.next().ok_or("missing --socket PATH")?);
    let mut fixture = false;
    let mut collection = Collection::None;
    let mut interval = None;
    let mut journal_capacity = None;
    let mut actions = None;
    let mut settings = SettingsChoice::Default;
    let mut protected: Vec<u32> = Vec::new();
    let mut args = args.peekable();
    while let Some(value) = args.next() {
        let select = |current: &mut Collection, chosen| {
            if *current != Collection::None {
                return Err("choose one of --fake-source, --fake-repeated-names, --fake-follow, --fake-tree, --fake-threads, --fake-sequence, --live-source (or --procfs) or --scale-source");
            }
            *current = chosen;
            Ok(())
        };
        match value.to_str() {
            Some("--smoke-fixture") if !fixture => fixture = true,
            Some("--fake-source") => select(&mut collection, Collection::Fake)?,
            Some("--fake-repeated-names") => select(&mut collection, Collection::RepeatedNames)?,
            Some("--fake-follow") => select(&mut collection, Collection::Follow)?,
            Some("--fake-tree") => select(&mut collection, Collection::Tree)?,
            Some("--fake-threads") => select(&mut collection, Collection::Threads)?,
            Some("--fake-sequence") => select(&mut collection, Collection::Sequence)?,
            Some("--live-source") | Some("--procfs") => select(&mut collection, Collection::Live)?,
            Some("--scale-source") => {
                let count: usize = args
                    .next()
                    .ok_or("missing --scale-source COUNT")?
                    .to_str()
                    .ok_or("--scale-source COUNT must be a number")?
                    .parse()
                    .map_err(|_| "--scale-source COUNT must be a number")?;
                select(&mut collection, Collection::Scale(count))?;
            }
            Some("--refresh-interval-ms") | Some("--scale-interval-ms") if interval.is_none() => {
                // The same validator as the interval field (PX-015).
                let chosen = parse_interval(
                    args.next()
                        .ok_or("missing --refresh-interval-ms MS")?
                        .to_str()
                        .ok_or("--refresh-interval-ms MS must be a number")?,
                )
                .map_err(|error| format!("--refresh-interval-ms: {error}"))?;
                interval = Some(chosen);
            }
            Some("--journal-capacity") if journal_capacity.is_none() => {
                // As `srui-sessiond --journal-capacity`: a window that can
                // retain nothing would turn every reconnect into a snapshot
                // resync, so it is refused rather than clamped (§18.1).
                let capacity = args
                    .next()
                    .ok_or("missing --journal-capacity N")?
                    .to_str()
                    .and_then(|text| text.parse::<usize>().ok())
                    .filter(|capacity| *capacity > 0)
                    .ok_or("--journal-capacity N must be a positive integer")?;
                journal_capacity = Some(capacity);
            }
            Some("--settings-path") if settings == SettingsChoice::Default => {
                let path = PathBuf::from(args.next().ok_or("missing --settings-path PATH")?);
                if path.as_os_str().is_empty() || path.file_name().is_none() {
                    return Err("--settings-path PATH must name a file".into());
                }
                settings = SettingsChoice::Path(path);
            }
            Some("--no-settings") if settings == SettingsChoice::Default => {
                settings = SettingsChoice::Disabled;
            }
            Some("--actions") if actions.is_none() => {
                let mode = args
                    .next()
                    .ok_or("missing --actions read-only|dry-run|terminate")?
                    .to_str()
                    .and_then(ActionMode::parse)
                    .ok_or("--actions must be read-only, dry-run or terminate")?;
                actions = Some(mode);
            }
            Some("--protect-pid") => {
                // A positive PID; 0 names no process.
                let pid = args
                    .next()
                    .ok_or("missing --protect-pid PID")?
                    .to_str()
                    .and_then(|text| text.parse::<u32>().ok())
                    .filter(|pid| *pid > 0)
                    .ok_or("--protect-pid PID must be a positive integer")?;
                if protected.len() >= MAX_CONFIGURED_PROTECTED_PIDS {
                    return Err(format!(
                        "--protect-pid may be given at most {MAX_CONFIGURED_PROTECTED_PIDS} times"
                    )
                    .into());
                }
                protected.push(pid);
            }
            _ => return Err("unknown or repeated option".into()),
        }
    }
    let interval = interval.unwrap_or(DEFAULT_REFRESH_INTERVAL);
    if actions.is_some() && collection == Collection::None {
        return Err("--actions needs a process source".into());
    }
    if !protected.is_empty() && actions.is_none() {
        return Err("--protect-pid needs --actions".into());
    }
    if matches!(settings, SettingsChoice::Path(_)) && collection == Collection::None {
        return Err("--settings-path needs a process source".into());
    }
    // Only the live source uses this user's own settings file by default; a
    // fixture keeps none unless a path is named (PX-069).
    let settings: Option<PathBuf> = match settings {
        SettingsChoice::Path(path) => Some(path),
        SettingsChoice::Disabled => None,
        // With neither `XDG_CONFIG_HOME` nor `HOME` absolute there is no
        // per-user directory: saved views are off, and no view control exists.
        SettingsChoice::Default if collection == Collection::Live => settings_file::default_path(),
        SettingsChoice::Default => None,
    };
    let settings = settings.as_deref();
    // The chain serving this session is derived here, from this process and
    // the transports that connect to it (PX-030); nothing a client sends
    // names a member.
    let support = Arc::new(ServerSupportChain::new());
    let actions = actions.map(|mode| Actions {
        mode,
        protected,
        support: Arc::clone(&support),
    });
    let actions = actions.as_ref();
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
    let mut config = SessionConfig::default();
    if let Some(capacity) = journal_capacity {
        config.journal_capacity = capacity;
    }
    let session = Arc::new(Session::mint_with_config(config));
    let shutdown = CancellationToken::new();
    let mut connections = JoinSet::new();
    // Each collecting mode publishes its first snapshot and then refreshes the
    // same rows in place on every tick; the empty shell has nothing to poll.
    let initialized: Result<(), Box<dyn std::error::Error>> = match collection {
        Collection::Fake => collect(
            &session,
            FakeProcessSource,
            interval,
            settings,
            actions,
            &shutdown,
            &mut connections,
        ),
        Collection::RepeatedNames => collect(
            &session,
            RepeatedNamesSource,
            interval,
            settings,
            actions,
            &shutdown,
            &mut connections,
        ),
        Collection::Follow => collect(
            &session,
            FollowSource::default(),
            interval,
            settings,
            actions,
            &shutdown,
            &mut connections,
        ),
        Collection::Tree => collect(
            &session,
            TreeSource::default(),
            interval,
            settings,
            actions,
            &shutdown,
            &mut connections,
        ),
        Collection::Threads => collect(
            &session,
            ThreadsSource::default(),
            interval,
            settings,
            actions,
            &shutdown,
            &mut connections,
        ),
        Collection::Sequence => collect(
            &session,
            ScriptedFakeSource::default(),
            interval,
            settings,
            actions,
            &shutdown,
            &mut connections,
        ),
        // Read-only sampling of the host's process filesystem; no process
        // controls are installed.
        Collection::Live => collect(
            &session,
            ProcFsSource::live(),
            interval,
            settings,
            actions,
            &shutdown,
            &mut connections,
        ),
        Collection::Scale(count) => collect(
            &session,
            SyntheticProcessSource::new(count),
            interval,
            settings,
            actions,
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
                // The process at the other end of the socket (the SSH
                // bridge) and its ancestors serve this session while the
                // connection lasts, so no action may target them (PX-030).
                let peer = stream
                    .peer_cred()
                    .ok()
                    .and_then(|credentials| credentials.pid())
                    .and_then(|pid| u32::try_from(pid).ok())
                    .filter(|pid| *pid > 0);
                let transport = support.connected(peer);
                let session = Arc::clone(&session);
                let token = shutdown.child_token();
                connections.spawn(async move {
                    let _transport = transport;
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
