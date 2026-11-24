//! PX-031 at the wire: what a client of a real `srtop --actions` instance is
//! told about a confirmed action across a broken transport and across a
//! restart of the daemon (D1 §§17, 18.2, Appendix B; D2 T24).
//!
//! srtop adds no acknowledgement protocol of its own. A confirmation's
//! result is the state its handler committed, and the runtime's event result
//! cache settles the event with the revision that state reached:
//!
//! * **Transport lost after the effect, before the acknowledgement.** The
//!   client sends Confirm and never reads the answer; the effect happens
//!   (observed independently: another client sees the result, and on Linux
//!   an owned worker records the one SIGTERM); the transport drops; the
//!   client reconnects with `CLIENT_RESUME`, is answered `RESUME_OK` with a
//!   frontier that already covers the event, receives the missed commits and
//!   replays its pending event. The replay is `DUPLICATE`, accepted, at the
//!   revision of the original effect, commits nothing and sends nothing: the
//!   worker has still received exactly one SIGTERM, and the client shows the
//!   settled result.
//! * **The daemon restarts.** A pending intent (and, on Linux, a request
//!   being watched) lives only in the old process. The new instance answers
//!   `RESYNC_REQUIRED{REPLACED}`; its snapshot holds no confirmation and no
//!   watch; an old Confirm replayed anyway (by a client that ignored §18.2's
//!   rule to abandon its retry set) names no node and is rejected; nothing is
//!   sent to the worker.
//!
//! Portable tests use the fixture and the dry-run executor; the Linux test
//! uses the live `/proc` source, `--actions terminate` and an owned worker.

#[cfg(target_os = "linux")]
use srui_process_explorer::actions::TERMINATE_STATUS;
use srui_process_explorer::actions::{PanelIds, DRY_RUN_STATUS, OBSERVED_IDLE};
use srui_process_explorer::{
    ACTIONS_OBSERVED, ACTIONS_STATUS, ACTION_CANCEL, ACTION_PREPARE, MODEL, TABLE,
};
use srui_protocol::{
    decode_framed, encode_framed, srui_message::Msg, ClientHello, ClientResume, EventAckStatus,
    ServerEventAck, SessionContinuity, SruiMessage,
};
use srui_sdk::{ItemId, NodeId, PropertyRef, Value, COLUMNS, ENABLED, TEXT};
use srui_semantic_tree::{Event as DomainEvent, ResyncSnapshot, SemanticStore};
use srui_sessiond::CORE_VERSION;
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

const CLIENT: &[u8] = b"px031-wire-client";
const OBSERVER: &[u8] = b"px031-wire-observer";
const PROFILE: &str = "org.srui.standard-widgets/1";

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
            "/tmp/srtop-px031-{}-{nonce}-{sequence}",
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

    /// A normal stop of this test's own instance: it ends, and with it its
    /// incarnation and everything it held in memory. Bounded: killed if it
    /// has not exited within 30 s.
    fn stop(mut self) {
        assert!(Command::new("/bin/kill")
            .args(["-TERM", &self.0.id().to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success());
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                assert!(status.success(), "srtop exited with {status}");
                return;
            }
            assert!(Instant::now() < deadline, "srtop did not stop");
            thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// How a server answered a `CLIENT_RESUME`.
#[derive(Debug, PartialEq, Eq)]
enum Decision {
    /// The same incarnation: `last_processed` is its event frontier for this
    /// client.
    ResumeOk {
        last_processed: u64,
    },
    Replaced,
}

/// A client replica of one srtop session that keeps its continuity
/// checkpoint, as the generic client does (§18).
struct Client {
    stream: UnixStream,
    instance: &'static [u8],
    buffer: Vec<u8>,
    replica: SemanticStore,
    session_id: String,
    last_acked: u64,
    next_seq: u64,
}

impl Client {
    fn connect(socket: &Path) -> UnixStream {
        let stream = UnixStream::connect(socket).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        stream
    }

    fn hello(socket: &Path, instance: &'static [u8]) -> Self {
        let mut client = Self {
            stream: Self::connect(socket),
            instance,
            buffer: Vec::new(),
            replica: SemanticStore::new(),
            session_id: String::new(),
            last_acked: 0,
            next_seq: 1,
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
        client.session_id = welcome.session_id;
        client.take_snapshot(welcome.initial_revision);
        client
    }

    /// A new transport and `CLIENT_RESUME` from the replica's revision and
    /// the acknowledged frontier. A replacement's snapshot replaces the
    /// replica whole, and its sequence space starts again (§18).
    fn resume(&mut self, socket: &Path) -> Decision {
        self.stream = Self::connect(socket);
        self.buffer.clear();
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
                    assert_eq!(ok.session_id, self.session_id);
                    return Decision::ResumeOk {
                        last_processed: ok.last_processed_event_seq,
                    };
                }
                Some(Msg::ServerResyncRequired(resync)) => {
                    let continuity =
                        SessionContinuity::try_from(resync.continuity).expect("a known continuity");
                    assert_eq!(
                        continuity,
                        SessionContinuity::Replaced,
                        "this test resumes only within retention or after a restart"
                    );
                    assert_ne!(resync.session_id, self.session_id);
                    assert_eq!(resync.last_processed_event_seq, 0);
                    self.replica = SemanticStore::new();
                    self.last_acked = 0;
                    self.next_seq = 1;
                    self.session_id = resync.session_id.clone();
                    self.take_snapshot(resync.snapshot_revision);
                    return Decision::Replaced;
                }
                _ => {}
            }
        }
    }

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

    /// Loses the transport, as a dropped network does: nothing more of this
    /// connection is read, the answer to anything in flight included.
    fn lose_transport(&mut self) {
        let _ = self.stream.shutdown(Shutdown::Both);
    }

    fn send(&mut self, message: &SruiMessage) {
        self.stream
            .write_all(&encode_framed(message).unwrap())
            .unwrap();
    }

    /// Writes `event` and reads nothing.
    fn send_event(&mut self, event: &DomainEvent) {
        self.send(&SruiMessage {
            msg: Some(Msg::Event(
                event
                    .clone()
                    .with_client_instance_id(self.instance.to_vec())
                    .to_wire(),
            )),
        });
    }

    fn next(&mut self) -> SruiMessage {
        let message = self.next_raw();
        if let Some(Msg::Transaction(transaction)) = &message.msg {
            let (base, new) = (transaction.base_revision, transaction.new_revision);
            self.replica
                .apply_delivered_transaction(transaction.clone())
                .unwrap_or_else(|error| panic!("commit {base} -> {new} did not apply: {error}"));
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

    /// Sends `event` and returns its acknowledgement, after applying every
    /// commit up to the revision its effects reached.
    fn deliver(&mut self, event: &DomainEvent) -> ServerEventAck {
        self.send_event(event);
        let ack = loop {
            if let Some(Msg::ServerEventAck(ack)) = self.next().msg {
                break ack;
            }
        };
        if ack.status != EventAckStatus::Rejected as i32 {
            self.last_acked = self.last_acked.max(ack.last_processed_event_seq);
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.revision() < ack.revision_after_effect {
            assert!(Instant::now() < deadline, "the effects never arrived");
            self.next();
        }
        ack
    }

    fn click_event(&mut self, node: NodeId) -> DomainEvent {
        let seq = self.next_seq;
        self.next_seq += 1;
        DomainEvent::activate(seq, format!("px031-click-{seq}"), self.revision(), node)
    }

    fn click(&mut self, node: NodeId) -> ServerEventAck {
        let event = self.click_event(node);
        self.deliver(&event)
    }

    fn select(&mut self, pid: u64) {
        let item = self
            .item_of(pid)
            .unwrap_or_else(|| panic!("PID {pid} is shown"));
        let seq = self.next_seq;
        self.next_seq += 1;
        let event = DomainEvent::selection_changed(
            seq,
            format!("px031-select-{seq}"),
            self.revision(),
            TABLE,
            item,
        );
        assert_eq!(self.deliver(&event).status, processed());
    }

    /// Reads and applies messages until `done` holds for the replica.
    fn until(&mut self, what: &str, done: impl Fn(&Self) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !done(self) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            self.next();
        }
    }

    fn property(&self, node: NodeId, property: PropertyRef) -> Option<Value> {
        self.replica
            .get_node(node)
            .and_then(|node| node.get_property(property))
            .cloned()
    }

    fn text(&self, node: NodeId) -> String {
        match self.property(node, TEXT) {
            Some(Value::String(text)) => text,
            other => panic!("no text on {node:?}: {other:?}"),
        }
    }

    fn status(&self) -> String {
        self.text(ACTIONS_STATUS)
    }

    fn observed(&self) -> String {
        self.text(ACTIONS_OBSERVED)
    }

    fn enabled(&self, node: NodeId) -> bool {
        self.property(node, ENABLED) != Some(Value::Bool(false))
    }

    fn item_of(&self, pid: u64) -> Option<ItemId> {
        let Some(Value::List(headings)) = self.property(TABLE, COLUMNS) else {
            return None;
        };
        let at = headings
            .iter()
            .position(|heading| heading == &Value::String("PID".into()))?;
        let model = self.replica.get_model(MODEL)?;
        (0..model.item_count)
            .filter_map(|index| model.get_item_by_index(index))
            .find(|item| match &item.value {
                Value::List(cells) => match &cells[at] {
                    Value::String(text) => text == &pid.to_string(),
                    Value::UnsignedInt(number) => *number == pid,
                    _ => false,
                },
                _ => false,
            })
            .map(|item| item.item_id)
    }
}

fn processed() -> i32 {
    EventAckStatus::Processed as i32
}

fn duplicate() -> i32 {
    EventAckStatus::Duplicate as i32
}

fn rejected() -> i32 {
    EventAckStatus::Rejected as i32
}

/// The fixture's own process, prepared through the real handlers: returns
/// its panel.
fn prepare(client: &mut Client, pid: u64) -> PanelIds {
    client.select(pid);
    assert_eq!(client.click(ACTION_PREPARE).status, processed());
    assert!(
        client.status().starts_with("Prepared #"),
        "{}",
        client.status()
    );
    let slot = (0..64)
        .map(PanelIds::of)
        .find(|panel| client.replica.get_node(panel.row).is_some())
        .expect("a confirmation panel");
    assert!(client.enabled(slot.confirm));
    slot
}

/// Sends `confirm` and loses the transport once `effect` is seen by an
/// independent observer of the same instance, without ever reading the
/// answer; then resumes, replays `confirm` and returns its acknowledgement
/// with the revision the observer saw the effect committed at.
fn break_after_the_effect_and_replay(
    socket: &Path,
    client: &mut Client,
    confirm: &DomainEvent,
    effect: impl Fn(&str) -> bool,
) -> (ServerEventAck, u64) {
    let mut observer = Client::hello(socket, OBSERVER);
    let before = client.revision();
    client.send_event(confirm);
    observer.until("the confirmation's effect", |observer| {
        effect(&observer.status())
    });
    let effect_revision = observer.revision();
    // Past the handler: its result is settled once the handler returns.
    thread::sleep(Duration::from_millis(300));
    assert_eq!(client.revision(), before, "the client read nothing");
    client.lose_transport();
    match client.resume(socket) {
        Decision::ResumeOk { last_processed } => assert_eq!(
            last_processed, confirm.event_seq,
            "the surviving session settled the event the client never saw acknowledged"
        ),
        other => panic!("the session survived: {other:?}"),
    }
    let ack = client.deliver(confirm);
    (ack, effect_revision)
}

/// Portable: the transport breaks after a dry-run Confirm took effect and
/// before its acknowledgement reached the client. The resumed client
/// replays it and is answered from the result cache: `DUPLICATE`, accepted,
/// at the revision of the original effect, with nothing re-run.
#[test]
fn a_replay_after_a_broken_transport_is_answered_from_the_settled_result() {
    let directory = SocketDirectory::new();
    let socket = directory.socket();
    let _server = Server::start(
        &socket,
        &[
            "--fake-source",
            "--actions",
            "dry-run",
            "--refresh-interval-ms",
            "300000",
        ],
    );
    let mut client = Client::hello(&socket, CLIENT);
    let panel = prepare(&mut client, 4101);
    let confirm = client.click_event(panel.confirm);
    let (ack, effect_revision) =
        break_after_the_effect_and_replay(&socket, &mut client, &confirm, |status| {
            status.starts_with("Dry run #1 requested (terminate PID 4101 · worker)")
        });
    assert_eq!(ack.status, duplicate(), "{ack:?}");
    assert_eq!(ack.revision_after_effect, effect_revision, "{ack:?}");
    assert_eq!(ack.settled_event_seq, confirm.event_seq);
    assert!(ack.reject_reason.is_empty(), "{ack:?}");
    // The client shows the settled result, from the commits RESUME_OK
    // replayed, and nothing was committed by the replay.
    assert!(
        client
            .status()
            .starts_with("Dry run #1 requested (terminate PID 4101 · worker): nothing was sent"),
        "{}",
        client.status()
    );
    assert!(client.replica.get_node(panel.row).is_none());
    assert_eq!(client.revision(), effect_revision);
    // Still one intent: the next Prepare is #2.
    assert_eq!(client.click(ACTION_PREPARE).status, processed());
    assert!(
        client.status().starts_with("Prepared #2"),
        "{}",
        client.status()
    );
}

/// Portable: a restart abandons a pending intent. The new instance holds
/// no confirmation, says nothing is being watched, and an old Confirm
/// replayed against it anyway names no node and is rejected.
#[test]
fn a_restarted_daemon_abandons_the_old_intent_and_confirms_nothing_from_before() {
    let directory = SocketDirectory::new();
    let socket = directory.socket();
    let options = [
        "--fake-source",
        "--actions",
        "dry-run",
        "--refresh-interval-ms",
        "300000",
    ];
    let server = Server::start(&socket, &options);
    let mut client = Client::hello(&socket, CLIENT);
    let panel = prepare(&mut client, 4101);
    let old_confirm = client.click_event(panel.confirm);
    server.stop();
    let _restarted = Server::start(&socket, &options);
    assert_eq!(client.resume(&socket), Decision::Replaced);
    assert!(client.replica.get_node(panel.row).is_none(), "no old panel");
    assert_eq!(client.status(), DRY_RUN_STATUS);
    assert_eq!(client.observed(), OBSERVED_IDLE);
    assert!(!client.enabled(ACTION_CANCEL), "nothing is prepared");
    // Past the old revision, so the old Confirm is refused for naming no
    // node, not for a revision this instance has not reached. Another
    // client's selections advance it, outside this client's sequence space.
    let mut observer = Client::hello(&socket, OBSERVER);
    let mut toggle = [4102, 4101].into_iter().cycle();
    while observer.revision() <= old_confirm.observed_revision.get() {
        observer.select(toggle.next().unwrap());
    }
    let target = observer.revision();
    client.until("the observer's commits", |client| {
        client.revision() >= target
    });
    let ack = client.deliver(&old_confirm);
    assert_eq!(ack.status, rejected(), "{ack:?}");
    assert_eq!(
        ack.reject_reason,
        format!(
            "event target node {} does not exist in store",
            panel.confirm
        ),
    );
    assert_eq!(client.status(), DRY_RUN_STATUS);
}

/// Waits until `client`'s instance has passed `event`'s observed revision,
/// so a replay of it is judged by the node it names.
#[cfg(target_os = "linux")]
fn past(client: &mut Client, event: &DomainEvent) {
    let observed = event.observed_revision.get();
    client.until("the new instance past the old revision", |client| {
        client.revision() > observed
    });
}

/// Linux, live: the same two cases with `--actions terminate`, the live
/// `/proc` source and an owned worker that records every SIGTERM and keeps
/// running. The transport breaks after the SIGTERM and before the
/// acknowledgement: the replay is answered from the cache and the worker
/// has received exactly one SIGTERM. Then a second intent is prepared on the
/// same worker and srtop restarts while the first request is still watched:
/// the new instance holds neither, an old Confirm is rejected, and the
/// worker receives nothing more.
#[cfg(target_os = "linux")]
#[test]
fn live_replay_sends_one_sigterm_and_a_restart_retries_nothing() {
    let directory = SocketDirectory::new();
    let socket = directory.socket();
    let terms = directory.0.join("terms");
    let mut worker = Worker::counting(&terms);
    let pid = u64::from(worker.0.id());
    let options = [
        "--live-source",
        "--refresh-interval-ms",
        "200",
        "--actions",
        "terminate",
    ];
    let server = Server::start(&socket, &options);
    let mut client = Client::hello(&socket, CLIENT);
    client.until("the worker's row", |client| client.item_of(pid).is_some());
    let at = client.revision();
    client.until("two more commits", |client| client.revision() >= at + 2);
    let panel = prepare(&mut client, pid);
    let confirm = client.click_event(panel.confirm);
    let requested = format!("Requested #1 (terminate PID {pid} · sh): SIGTERM was sent");
    let (ack, effect_revision) =
        break_after_the_effect_and_replay(&socket, &mut client, &confirm, |status| {
            status.starts_with(&requested)
        });
    assert_eq!(ack.status, duplicate(), "{ack:?}");
    assert_eq!(ack.revision_after_effect, effect_revision, "{ack:?}");
    assert!(
        client.status().starts_with(&requested),
        "{}",
        client.status()
    );
    let observed = client.observed();
    assert!(
        observed.starts_with(&format!("Watching #1 (terminate PID {pid} · sh)")),
        "{observed}"
    );
    thread::sleep(Duration::from_millis(1000));
    assert_eq!(
        worker.terms(&terms),
        1,
        "exactly one SIGTERM across the replay"
    );
    assert!(
        worker.running(),
        "the worker handles SIGTERM and keeps running"
    );

    // A second intent, prepared and never confirmed; then a restart while
    // request #1 is still watched.
    let panel = prepare(&mut client, pid);
    let old_confirm = client.click_event(panel.confirm);
    server.stop();
    let _restarted = Server::start(&socket, &options);
    assert_eq!(client.resume(&socket), Decision::Replaced);
    assert!(client.replica.get_node(panel.row).is_none(), "no old panel");
    assert_eq!(client.status(), TERMINATE_STATUS);
    assert_eq!(client.observed(), OBSERVED_IDLE, "no watch survives");
    past(&mut client, &old_confirm);
    let ack = client.deliver(&old_confirm);
    assert_eq!(ack.status, rejected(), "{ack:?}");
    assert_eq!(
        ack.reject_reason,
        format!(
            "event target node {} does not exist in store",
            panel.confirm
        ),
    );
    // Several samples of the new instance later: nothing more was sent.
    let at = client.revision();
    client.until("five more commits", |client| client.revision() >= at + 5);
    assert_eq!(client.observed(), OBSERVED_IDLE);
    assert_eq!(
        worker.terms(&terms),
        1,
        "nothing retried against the new instance"
    );
    assert!(worker.running());
    println!(
        "PX-031 wire: worker PID {pid} received exactly one SIGTERM across a broken transport, a DUPLICATE replay at revision {effect_revision}, and a restart"
    );
}

/// One owned worker: a `sh` that records each SIGTERM in a file and keeps
/// running; it ends by itself after about 60 s. Killed and reaped on drop.
#[cfg(target_os = "linux")]
struct Worker(Child);

#[cfg(target_os = "linux")]
impl Worker {
    fn counting(terms: &Path) -> Self {
        let worker = Self(
            Command::new("sh")
                .arg("-c")
                .arg("trap 'echo term >> \"$0\"' TERM; i=0; while [ $i -lt 600 ]; do sleep 0.1; i=$((i+1)); done; exit 3")
                .arg(terms)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        thread::sleep(Duration::from_millis(200));
        assert_eq!(
            fs::read_to_string(format!("/proc/{}/comm", worker.0.id()))
                .unwrap_or_default()
                .trim(),
            "sh",
            "the owned worker is the sh this test started"
        );
        worker
    }

    fn running(&mut self) -> bool {
        self.0.try_wait().unwrap().is_none()
    }

    fn terms(&self, terms: &Path) -> usize {
        fs::read_to_string(terms)
            .unwrap_or_default()
            .lines()
            .count()
    }
}

#[cfg(target_os = "linux")]
impl Drop for Worker {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}
