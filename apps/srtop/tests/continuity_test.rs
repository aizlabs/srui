//! PX-022 acceptance at the wire: what a reconnecting client of a real
//! `srtop` instance is told, and what its replica then holds (D1 §§17–18.2;
//! D2 T22–T24).
//!
//! srtop adds no continuity logic of its own. It hosts the runtime's session,
//! whose journal (`--journal-capacity`, §18.1) and incarnation token decide
//! every outcome; these tests only check that the explorer's state follows
//! those decisions:
//!
//! * Within retention the same incarnation answers `RESUME_OK` and replays
//!   exactly the commits the client missed, in order: the selected process,
//!   its tag and every row keep their item identities, and nothing is
//!   re-sent as a snapshot.
//! * Outside retention the same incarnation answers
//!   `RESYNC_REQUIRED{SAME_SESSION}` with this client's event frontier, and
//!   the snapshot carries the same identities, the same inspector target and
//!   the same tags: per-client and server-side explorer state belongs to the
//!   incarnation, not to the connection.
//! * After a real restart the new instance answers
//!   `RESYNC_REQUIRED{REPLACED}` under a new token with frontier 0, and its
//!   snapshot inherits no selection, tag or last-seen record of the old one.
//! * The window is exact: with `--journal-capacity N`, and with the runtime
//!   default when the flag is absent, exactly N missed commits replay and
//!   N + 1 resynchronize the same session.
//! * A retention window that cannot retain anything is refused at start.
//!
//! The client here is a minimal replica over the instance's private socket;
//! the generic client's handling of the same decisions over SSH, with its
//! window chrome, is `ProcessExplorerShellTests` (PX-022).

use srui_process_explorer::summary::FRESHNESS_TEXT;
use srui_process_explorer::tags::TAGGED_CELL;
use srui_process_explorer::{INSPECTOR_STATUS, MODEL, TABLE, TAGS_STATUS, TAG_TOGGLE};
use srui_protocol::{
    decode_framed, encode_framed, srui_message::Msg, ClientHello, ClientResume, EventAckStatus,
    SessionContinuity, SruiMessage,
};
use srui_sdk::{ItemId, NodeId, PropertyRef, Value, COLUMNS, TEXT};
use srui_semantic_tree::{Event as DomainEvent, ResyncSnapshot, SemanticStore};
use srui_sessiond::{SessionConfig, CORE_VERSION};
use std::{
    fs,
    io::{Read, Write},
    net::Shutdown,
    os::unix::{fs::PermissionsExt, net::UnixStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const CLIENT: &[u8] = b"px022-continuity-client";
const PROFILE: &str = "org.srui.standard-widgets/1";
/// The scripted fixture (`--fake-sequence`) publishes on most ticks, so a
/// detached client misses commits at this interval within a few ticks.
const INTERVAL_MS: &str = "50";
/// The script's one process present on every step.
const STEADY_PID: u64 = 4101;

struct SocketDirectory(PathBuf);

impl SocketDirectory {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
        // Short: AF_UNIX paths are bounded and macOS TMPDIR is deep.
        let path = PathBuf::from(format!(
            "/tmp/srtop-px022-{}-{nonce}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }

    fn socket(&self) -> PathBuf {
        self.0.join("s.sock")
    }
}

impl Drop for SocketDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// One owned `srtop` process; killed and reaped on drop.
struct Server(Child);

impl Server {
    fn start(socket: &Path, options: &[&str]) -> Self {
        let mut server = Self(
            Command::new(env!("CARGO_BIN_EXE_srtop"))
                .arg("--socket")
                .arg(socket)
                .args(options)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        // Generous: a newly linked binary's first exec is assessed by macOS
        // before `main` runs (see `shutdown.rs`).
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            assert!(
                server.0.try_wait().unwrap().is_none(),
                "srtop exited before it was ready"
            );
            if UnixStream::connect(socket).is_ok() {
                return server;
            }
            assert!(Instant::now() < deadline, "socket did not become ready");
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// A normal stop: the instance ends, and with it its incarnation.
    fn stop(mut self) {
        assert!(Command::new("/bin/kill")
            .args(["-TERM", &self.0.id().to_string()])
            .status()
            .unwrap()
            .success());
        let status = self.0.wait().unwrap();
        assert!(status.success(), "srtop exited with {status}");
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// How a server answered a `CLIENT_RESUME` (§18).
#[derive(Debug, PartialEq, Eq)]
enum Decision {
    ResumeOk { replay_from: u64 },
    Resync { continuity: SessionContinuity },
}

/// A client replica of one srtop session: it applies every commit it is sent
/// exactly as the generic client does, and remembers the continuity
/// checkpoint a warm reconnect resumes from (§18).
struct Client {
    stream: UnixStream,
    instance: &'static [u8],
    buffer: Vec<u8>,
    replica: SemanticStore,
    session_id: String,
    last_acked: u64,
    next_seq: u64,
    /// Commits applied since the last attach, in order: `(base, new)`.
    applied: Vec<(u64, u64)>,
}

impl Client {
    fn connect(socket: &Path) -> UnixStream {
        let stream = UnixStream::connect(socket).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        stream
    }

    /// A fresh client: `CLIENT_HELLO`, then the welcome and its snapshot.
    fn hello(socket: &Path) -> Self {
        Self::hello_as(socket, CLIENT)
    }

    fn hello_as(socket: &Path, instance: &'static [u8]) -> Self {
        let mut client = Self {
            stream: Self::connect(socket),
            instance,
            buffer: Vec::new(),
            replica: SemanticStore::new(),
            session_id: String::new(),
            last_acked: 0,
            next_seq: 1,
            applied: Vec::new(),
        };
        client.send(&SruiMessage {
            msg: Some(Msg::ClientHello(ClientHello {
                core_version: CORE_VERSION.to_string(),
                profiles: vec![PROFILE.to_string()],
                client_instance_id: instance.to_vec(),
                ..ClientHello::default()
            })),
        });
        let welcome = loop {
            if let Some(Msg::ServerWelcome(welcome)) = client.next().msg {
                break welcome;
            }
        };
        assert!(
            welcome.snapshot_parts <= 1,
            "a client that offers no limit gets one envelope"
        );
        client.session_id = welcome.session_id;
        client.take_snapshot(welcome.initial_revision);
        client
    }

    /// The warm reconnect of this client: a new transport, then
    /// `CLIENT_RESUME` from the replica's revision and the acknowledged
    /// frontier. Returns the decision; a resync's snapshot is applied, and a
    /// `REPLACED` one replaces the replica of the old incarnation whole.
    fn resume(&mut self, socket: &Path) -> Decision {
        self.stream = Self::connect(socket);
        self.buffer.clear();
        self.applied.clear();
        self.send(&SruiMessage {
            msg: Some(Msg::ClientResume(ClientResume {
                session_id: self.session_id.clone(),
                client_instance_id: self.instance.to_vec(),
                last_applied_revision: self.revision(),
                last_acked_event_seq: self.last_acked,
                core_version: CORE_VERSION.to_string(),
                profiles: vec![PROFILE.to_string()],
                ..ClientResume::default()
            })),
        });
        loop {
            match self.next().msg {
                Some(Msg::ServerResumeOk(ok)) => {
                    assert_eq!(
                        ok.session_id, self.session_id,
                        "RESUME_OK names the requested incarnation"
                    );
                    assert_eq!(ok.last_processed_event_seq, self.last_acked);
                    return Decision::ResumeOk {
                        replay_from: ok.replay_from_revision,
                    };
                }
                Some(Msg::ServerResyncRequired(resync)) => {
                    let continuity =
                        SessionContinuity::try_from(resync.continuity).expect("a known continuity");
                    match continuity {
                        SessionContinuity::SameSession => {
                            assert_eq!(resync.session_id, self.session_id);
                            assert_eq!(resync.last_processed_event_seq, self.last_acked);
                        }
                        SessionContinuity::Replaced => {
                            assert_ne!(
                                resync.session_id, self.session_id,
                                "a replacement is a new incarnation"
                            );
                            assert_eq!(resync.last_processed_event_seq, 0);
                            // The old incarnation's replica and frontier are
                            // abandoned, never merged (§18).
                            self.replica = SemanticStore::new();
                            self.last_acked = 0;
                            self.next_seq = 1;
                            self.session_id = resync.session_id.clone();
                        }
                        SessionContinuity::Unspecified => {
                            panic!("an unspecified continuity is never sent")
                        }
                    }
                    assert!(resync.snapshot_parts <= 1);
                    self.take_snapshot(resync.snapshot_revision);
                    return Decision::Resync { continuity };
                }
                _ => {}
            }
        }
    }

    /// Reads the one snapshot envelope at `revision` and replaces the
    /// replica from it (§18, "Snapshot delivery form").
    fn take_snapshot(&mut self, revision: u64) {
        let wire = loop {
            if let Some(Msg::Transaction(transaction)) = self.next_raw().msg {
                break transaction;
            }
        };
        assert_eq!(wire.base_revision, 0, "a snapshot has base revision 0");
        assert_eq!(wire.new_revision, revision);
        let snapshot = ResyncSnapshot::try_from(wire).expect("a valid snapshot");
        self.replica
            .replace_from_snapshot(&snapshot)
            .expect("the snapshot applies");
    }

    fn revision(&self) -> u64 {
        self.replica.revision().get()
    }

    /// Loses the transport, as a dropped network does: the session is not
    /// told anything and lives on (§17).
    fn lose_transport(&mut self) {
        let _ = self.stream.shutdown(Shutdown::Both);
    }

    fn send(&mut self, message: &SruiMessage) {
        self.stream
            .write_all(&encode_framed(message).unwrap())
            .unwrap();
    }

    /// The next message, with every commit applied to the replica as it
    /// arrives (§12.1). A commit that does not apply fails the test.
    fn next(&mut self) -> SruiMessage {
        let message = self.next_raw();
        if let Some(Msg::Transaction(transaction)) = &message.msg {
            let base = transaction.base_revision;
            let new = transaction.new_revision;
            assert_ne!(base, 0, "no snapshot outside a continuity decision");
            self.replica
                .apply_delivered_transaction(transaction.clone())
                .unwrap_or_else(|error| panic!("commit {base} -> {new} did not apply: {error}"));
            self.applied.push((base, new));
        }
        message
    }

    fn next_raw(&mut self) -> SruiMessage {
        loop {
            let mut value = 0_usize;
            let mut prefix = None;
            for (index, byte) in self.buffer.iter().take(10).enumerate() {
                value |= usize::from(byte & 0x7f) << (7 * index);
                if byte & 0x80 == 0 {
                    prefix = Some(index + 1);
                    break;
                }
            }
            if let Some(prefix) = prefix {
                if self.buffer.len() >= prefix + value {
                    let frame: Vec<u8> = self.buffer.drain(..prefix + value).collect();
                    return decode_framed(&frame).unwrap();
                }
            }
            let mut chunk = [0_u8; 64 * 1024];
            let read = self.stream.read(&mut chunk).expect("the server answers");
            assert!(read > 0, "the server closed the connection");
            self.buffer.extend_from_slice(&chunk[..read]);
        }
    }

    /// Reads and applies messages until `done` holds for the replica.
    fn until(&mut self, what: &str, done: impl Fn(&Self) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done(self) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            self.next();
        }
    }

    /// Sends one event and waits for its terminal acknowledgement; a
    /// processed event advances the acknowledged frontier. Returns the
    /// revision the event's effects reached (`revision_after_effect`); the
    /// commits up to it may still be on their way.
    fn settle(&mut self, event: DomainEvent) -> u64 {
        self.next_seq += 1;
        self.send(&SruiMessage {
            msg: Some(Msg::Event(
                event
                    .with_client_instance_id(self.instance.to_vec())
                    .to_wire(),
            )),
        });
        loop {
            if let Some(Msg::ServerEventAck(ack)) = self.next().msg {
                assert_eq!(
                    ack.status,
                    EventAckStatus::Processed as i32,
                    "not processed: {ack:?}"
                );
                self.last_acked = ack.last_processed_event_seq;
                return ack.revision_after_effect;
            }
        }
    }

    fn select(&mut self, item: ItemId) -> u64 {
        let seq = self.next_seq;
        let observed = self.revision();
        self.settle(DomainEvent::selection_changed(
            seq,
            format!("px022-select-{seq}"),
            observed,
            TABLE,
            item,
        ))
    }

    fn click(&mut self, node: NodeId) -> u64 {
        let seq = self.next_seq;
        let observed = self.revision();
        self.settle(DomainEvent::activate(
            seq,
            format!("px022-click-{seq}"),
            observed,
            node,
        ))
    }

    fn text(&self, node: NodeId) -> String {
        match self
            .replica
            .get_node(node)
            .and_then(|node| node.get_property(TEXT))
        {
            Some(Value::String(text)) => text.clone(),
            other => panic!("no text on {node:?}: {other:?}"),
        }
    }

    fn property(&self, node: NodeId, property: PropertyRef) -> Option<Value> {
        self.replica
            .get_node(node)
            .and_then(|node| node.get_property(property))
            .cloned()
    }

    /// The table's rows as the replica holds them: item, PID and tag cell.
    fn rows(&self) -> Vec<(ItemId, u64, String)> {
        let Some(Value::List(headings)) = self.property(TABLE, COLUMNS) else {
            panic!("the table has headings")
        };
        let at = |wanted: &str| {
            headings
                .iter()
                .position(|heading| heading == &Value::String(wanted.into()))
                .unwrap_or_else(|| panic!("no {wanted} column"))
        };
        let (pid, tag) = (at("PID"), at("Tag"));
        let model = self.replica.get_model(MODEL).expect("the process model");
        (0..model.item_count)
            .filter_map(|index| model.get_item_by_index(index))
            .map(|item| {
                let Value::List(cells) = &item.value else {
                    panic!("a row is a list of cells")
                };
                let number = match &cells[pid] {
                    Value::String(text) => text.parse().unwrap_or(0),
                    Value::UnsignedInt(number) => *number,
                    _ => 0,
                };
                let tag = match &cells[tag] {
                    Value::String(text) => text.clone(),
                    other => format!("{other:?}"),
                };
                (item.item_id, number, tag)
            })
            .collect()
    }

    fn item_of(&self, pid: u64) -> Option<ItemId> {
        self.rows()
            .into_iter()
            .find(|row| row.1 == pid)
            .map(|row| row.0)
    }

    fn tag_of(&self, pid: u64) -> Option<String> {
        self.rows()
            .into_iter()
            .find(|row| row.1 == pid)
            .map(|row| row.2)
    }
}

/// What the explorer shows about the steady process once it is selected and
/// tagged: its item, the inspector line and the tags status.
fn select_and_tag_steady_process(client: &mut Client) -> (ItemId, String) {
    let target = format!("Selected process: PID {STEADY_PID} · worker");
    client.until("the steady process", |client| {
        client.item_of(STEADY_PID).is_some()
    });
    let item = client.item_of(STEADY_PID).unwrap();
    client.select(item);
    client.until("the inspector on the steady process", |client| {
        client.text(INSPECTOR_STATUS).starts_with(&target)
    });
    client.click(TAG_TOGGLE);
    client.until("the steady process tagged", |client| {
        client.tag_of(STEADY_PID).as_deref() == Some(TAGGED_CELL)
            && client.text(TAGS_STATUS).starts_with("Tags: 1 tagged")
    });
    (item, target)
}

/// Lets a detached session commit at least `commits` more transactions,
/// observed by a second, independent client of the same instance.
fn let_the_session_advance(socket: &Path, from: u64, commits: u64) {
    let mut observer = Client::hello_as(socket, b"px022-observer");
    observer.until("the session to advance", |observer| {
        observer.revision() >= from + commits
    });
}

/// Within retention the same incarnation replays exactly the missed commits,
/// and the explorer's per-client and server-side state is all still there.
#[test]
fn a_reconnect_within_retention_replays_the_missed_commits_and_keeps_every_identity() {
    let directory = SocketDirectory::new();
    let socket = directory.socket();
    let _server = Server::start(
        &socket,
        &["--fake-sequence", "--refresh-interval-ms", INTERVAL_MS],
    );
    let mut client = Client::hello(&socket);
    let (item, inspector) = select_and_tag_steady_process(&mut client);
    let rows = client.rows();
    // Steps 1 and 3 of the script publish only a freshness or status line,
    // so no row may move for them either.
    let freshness = client.text(FRESHNESS_TEXT);
    let detached_at = client.revision();
    // The transport goes; the session lives on and keeps sampling (§17).
    client.lose_transport();
    let_the_session_advance(&socket, detached_at, 3);

    let decision = client.resume(&socket);
    // The runtime names the revision the replay applies to, the client's own
    // (`srui_sessiond` handshake); the first replayed commit is based on it.
    assert_eq!(
        decision,
        Decision::ResumeOk {
            replay_from: detached_at
        }
    );
    client.until("the replay", |client| client.revision() >= detached_at + 3);
    // Exactly the missed commits, contiguous and in order, and no snapshot.
    let mut expected = detached_at;
    for (base, new) in &client.applied {
        assert_eq!(
            *base, expected,
            "replay is contiguous: {:?}",
            client.applied
        );
        expected = *new;
    }
    assert_eq!(client.item_of(STEADY_PID), Some(item), "identity kept");
    for (item, pid, _) in client.rows() {
        if let Some(before) = rows.iter().find(|row| row.1 == pid) {
            assert_eq!(before.0, item, "PID {pid} kept its item identity");
        }
    }
    assert!(
        client.text(INSPECTOR_STATUS).starts_with(&inspector),
        "the inspector stays on the selected process: {}",
        client.text(INSPECTOR_STATUS)
    );
    assert_eq!(client.tag_of(STEADY_PID).as_deref(), Some(TAGGED_CELL));
    assert!(client.text(TAGS_STATUS).starts_with("Tags: 1 tagged"));
    // The replay carried newer samples: the freshness line moved on.
    assert_ne!(client.text(FRESHNESS_TEXT), freshness);
}

/// Outside retention the same incarnation resynchronizes from a snapshot,
/// which carries the same identities and the same explorer state.
#[test]
fn a_reconnect_outside_retention_resynchronizes_the_same_session_with_the_same_identities() {
    let directory = SocketDirectory::new();
    let socket = directory.socket();
    let _server = Server::start(
        &socket,
        &[
            "--fake-sequence",
            "--refresh-interval-ms",
            INTERVAL_MS,
            "--journal-capacity",
            "1",
        ],
    );
    let mut client = Client::hello(&socket);
    let (item, inspector) = select_and_tag_steady_process(&mut client);
    let detached_at = client.revision();
    client.lose_transport();
    // Two commits past a one-entry journal: revision `detached_at + 1` is no
    // longer retained.
    let_the_session_advance(&socket, detached_at, 3);

    let decision = client.resume(&socket);
    assert_eq!(
        decision,
        Decision::Resync {
            continuity: SessionContinuity::SameSession
        }
    );
    assert!(client.revision() >= detached_at + 3);
    assert_eq!(client.item_of(STEADY_PID), Some(item), "identity kept");
    assert!(
        client.text(INSPECTOR_STATUS).starts_with(&inspector),
        "the inspector stays on the selected process: {}",
        client.text(INSPECTOR_STATUS)
    );
    assert_eq!(client.tag_of(STEADY_PID).as_deref(), Some(TAGGED_CELL));
    // The resynchronized client is live again: the next commit applies.
    let resynced_at = client.revision();
    client.until("a commit after the snapshot", |client| {
        client.revision() > resynced_at
    });
}

/// Slower than any test: after its first sample the instance publishes
/// nothing on its own, so the only commits are the ones a test causes.
const QUIET_INTERVAL_MS: &str = "300000";

/// Detaches a client from an instance started with `capacity` (the runtime
/// default when `None`), lets exactly `missed` commits happen, one tag toggle
/// each by a second client, and resumes. A replay must be exactly those
/// commits from the client's own revision; a resync's snapshot must be at
/// exactly the revision they reached. Returns `None` for a replay, else the
/// resync's continuity.
fn continuity_after_exactly(capacity: Option<&str>, missed: u64) -> Option<SessionContinuity> {
    let directory = SocketDirectory::new();
    let socket = directory.socket();
    let mut options = vec![
        "--fake-sequence",
        "--refresh-interval-ms",
        QUIET_INTERVAL_MS,
    ];
    if let Some(capacity) = capacity {
        options.extend(["--journal-capacity", capacity]);
    }
    let _server = Server::start(&socket, &options);
    let mut client = Client::hello(&socket);
    let mut observer = Client::hello_as(&socket, b"px022-bound-observer");
    observer.until("the steady process", |observer| {
        observer.item_of(STEADY_PID).is_some()
    });
    let item = observer.item_of(STEADY_PID).unwrap();
    // The sampler is quiet, so every commit is an event's effect. A selection
    // commits its inspector line and then, once its details are read off the
    // event path, the line without "loading details".
    observer.select(item);
    let inspected = format!("Selected process: PID {STEADY_PID} · worker");
    observer.until("the selected process's details", |observer| {
        observer.text(INSPECTOR_STATUS) == inspected
    });
    let detached_at = observer.revision();
    client.until("the observer's revision", |client| {
        client.revision() >= detached_at
    });
    assert_eq!(client.revision(), detached_at, "nothing commits on its own");
    client.lose_transport();
    let mut reached = detached_at;
    for _ in 0..missed {
        let after = observer.click(TAG_TOGGLE);
        assert_eq!(after, reached + 1, "one toggle, one commit");
        reached = after;
        observer.until("the toggle's commit", |observer| {
            observer.revision() >= after
        });
    }
    assert_eq!(reached, detached_at + missed);
    assert_eq!(observer.revision(), reached, "nothing commits on its own");

    let continuity = match client.resume(&socket) {
        Decision::ResumeOk { replay_from } => {
            assert_eq!(
                replay_from, detached_at,
                "replay from the client's revision"
            );
            client.until("the replay", |client| client.revision() >= reached);
            let expected: Vec<(u64, u64)> = (detached_at..reached)
                .map(|base| (base, base + 1))
                .collect();
            assert_eq!(
                client.applied, expected,
                "exactly the missed commits, contiguous and in order"
            );
            None
        }
        Decision::Resync { continuity } => {
            assert_eq!(client.revision(), reached, "the snapshot is current");
            Some(continuity)
        }
    };
    assert_eq!(client.item_of(STEADY_PID), Some(item), "identity kept");
    let tagged = client.tag_of(STEADY_PID).as_deref() == Some(TAGGED_CELL);
    assert_eq!(tagged, missed % 2 == 1, "every toggle reached the replica");
    continuity
}

const REPLAYED: Option<SessionContinuity> = None;
const SAME_SESSION: Option<SessionContinuity> = Some(SessionContinuity::SameSession);

/// `--journal-capacity N` retains exactly N commits (§18.1): N missed commits
/// replay, N + 1 resynchronize the same session. Pinned at and past the bound
/// for N = 1 and for N = 3, so a window off by one either way fails.
#[test]
fn a_journal_capacity_replays_exactly_that_many_missed_commits_and_no_more() {
    for (capacity, missed, want) in [
        ("1", 1, REPLAYED),
        ("1", 2, SAME_SESSION),
        ("3", 3, REPLAYED),
        ("3", 4, SAME_SESSION),
    ] {
        assert_eq!(
            continuity_after_exactly(Some(capacity), missed),
            want,
            "--journal-capacity {capacity} after exactly {missed} missed commits"
        );
    }
}

/// Without `--journal-capacity` srtop keeps the runtime's default window
/// (`SessionConfig::default()`), exactly: that many missed commits replay,
/// one more resynchronizes the same session.
#[test]
fn the_default_journal_capacity_is_the_runtime_default_exactly() {
    let default = u64::try_from(SessionConfig::default().journal_capacity).unwrap();
    for (missed, want) in [(default, REPLAYED), (default + 1, SAME_SESSION)] {
        assert_eq!(
            continuity_after_exactly(None, missed),
            want,
            "default journal after exactly {missed} missed commits"
        );
    }
}

/// After a real restart the requested incarnation is gone: the new instance
/// replaces it, and nothing of the old one's selection, tags or last-seen
/// records survives.
#[test]
fn a_reconnect_after_a_restart_is_replaced_and_inherits_no_selection_or_tag() {
    let directory = SocketDirectory::new();
    let socket = directory.socket();
    let options = ["--fake-sequence", "--refresh-interval-ms", INTERVAL_MS];
    let server = Server::start(&socket, &options);
    let mut client = Client::hello(&socket);
    let old_session = client.session_id.clone();
    select_and_tag_steady_process(&mut client);
    server.stop();
    assert!(!socket.exists(), "the stopped instance released its socket");
    let _restarted = Server::start(&socket, &options);

    let decision = client.resume(&socket);
    assert_eq!(
        decision,
        Decision::Resync {
            continuity: SessionContinuity::Replaced
        }
    );
    assert_ne!(client.session_id, old_session);
    assert_eq!(
        client.text(INSPECTOR_STATUS),
        srui_process_explorer::inspector::NO_SELECTION_STATUS
    );
    assert!(
        client.text(TAGS_STATUS).starts_with("Tags: none"),
        "no live or historical tag: {}",
        client.text(TAGS_STATUS)
    );
    assert!(client
        .rows()
        .iter()
        .all(|(_, _, tag)| tag.as_str() != TAGGED_CELL));
}

/// The same three decisions over a live Linux process table (`--live-source`,
/// reading `/proc`): the instance's own process keeps its item identity and
/// the inspector stays on it through a replay and through a same-session
/// snapshot, and a restarted instance, a new process itself, inherits no
/// selection of the old one.
#[cfg(target_os = "linux")]
#[test]
fn live_procfs_identities_follow_every_continuity_decision() {
    let directory = SocketDirectory::new();
    let socket = directory.socket();
    // The default window replays; a one-commit window resynchronizes.
    for capacity in [None, Some("1")] {
        let mut options = vec![
            "--live-source",
            "--no-settings",
            "--refresh-interval-ms",
            "100",
        ];
        if let Some(capacity) = capacity {
            options.extend(["--journal-capacity", capacity]);
        }
        let server = Server::start(&socket, &options);
        let own = u64::from(server.0.id());
        let mut client = Client::hello(&socket);
        client.until("srtop's own row", |client| client.item_of(own).is_some());
        let item = client.item_of(own).unwrap();
        client.select(item);
        let target = format!("Selected process: PID {own} · srtop");
        client.until("the inspector on srtop", |client| {
            client.text(INSPECTOR_STATUS).starts_with(&target)
        });
        let detached_at = client.revision();
        client.lose_transport();
        let_the_session_advance(&socket, detached_at, 3);
        let expected = match capacity {
            None => Decision::ResumeOk {
                replay_from: detached_at,
            },
            Some(_) => Decision::Resync {
                continuity: SessionContinuity::SameSession,
            },
        };
        let decision = client.resume(&socket);
        assert_eq!(decision, expected);
        client.until("a newer sample", |client| client.revision() > detached_at);
        println!(
            "PX-022 live Linux evidence: journal={capacity:?} srtop pid={own} item={item:?} \
             detached at revision {detached_at} -> {decision:?} at revision {}; inspector={:?}",
            client.revision(),
            client.text(INSPECTOR_STATUS)
        );
        assert_eq!(
            client.item_of(own),
            Some(item),
            "srtop's row kept its identity"
        );
        assert!(
            client.text(INSPECTOR_STATUS).starts_with(&target),
            "{}",
            client.text(INSPECTOR_STATUS)
        );

        // The instance stops; a new one, a new process, takes the socket.
        server.stop();
        let restarted = Server::start(&socket, &options);
        assert_eq!(
            client.resume(&socket),
            Decision::Resync {
                continuity: SessionContinuity::Replaced
            }
        );
        assert_eq!(
            client.text(INSPECTOR_STATUS),
            srui_process_explorer::inspector::NO_SELECTION_STATUS
        );
        let replacement = u64::from(restarted.0.id());
        client.until("the new instance's own row", |client| {
            client.item_of(replacement).is_some()
        });
        assert_eq!(
            client.item_of(own),
            None,
            "the stopped instance is not listed"
        );
        println!(
            "PX-022 live Linux evidence: restarted as pid={replacement} -> REPLACED, session {}; inspector={:?}",
            client.session_id,
            client.text(INSPECTOR_STATUS)
        );
        restarted.stop();
    }
}

/// A window that retains nothing would make every reconnect a snapshot
/// resync, and an unparsable one is no window at all: both are refused
/// before any socket is published (§18.1).
#[test]
fn an_unusable_journal_capacity_is_refused_without_publishing_a_socket() {
    let directory = SocketDirectory::new();
    let socket = directory.socket();
    for options in [
        vec!["--journal-capacity", "0"],
        vec!["--journal-capacity", "-1"],
        vec!["--journal-capacity", "many"],
        vec!["--journal-capacity", ""],
        vec!["--journal-capacity"],
        vec!["--journal-capacity", "4", "--journal-capacity", "8"],
    ] {
        let exit = Command::new(env!("CARGO_BIN_EXE_srtop"))
            .arg("--socket")
            .arg(&socket)
            .args(&options)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(!exit.success(), "{options:?} must be refused");
        assert!(!socket.exists(), "{options:?} published a socket");
    }
}
