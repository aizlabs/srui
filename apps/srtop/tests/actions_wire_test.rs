//! PX-028 at the wire: the action controls of a real `srtop --actions`
//! instance, driven by a minimal client replica over its private socket (D1
//! §§7.7, 18.2, 27; D2 T24).
//!
//! Every click is a framed `EVENT` the instance's connection loop admits,
//! validates and dispatches, and every answer is the `SERVER_EVENT_ACK` it
//! settles with:
//!
//! * `--actions dry-run`: there is no Confirm, and a click on its node names
//!   nothing, until Prepare has prepared an intent for the selected process
//!   and shown its confirmation panel (PX-030); that panel's Confirm then
//!   settles as processed with the dry run requested and the panel is
//!   deleted, a redelivery of the same event is a duplicate that changes
//!   nothing, and another account's process is refused at Prepare;
//! * `--actions terminate` (PX-030): "Terminate…", disabled with the reason
//!   where this host has no PID file descriptors (macOS); on Linux a fixture
//!   process is prepared and confirmed by name, and the terminator refuses to
//!   hold a key no live scan of this host produced, so nothing is sent;
//! * `--actions read-only`: every control is disabled and a click rejected;
//! * without `--actions` no action control exists, so a click names no node;
//! * an unknown mode, `--actions` without a process source, and a bad or
//!   orphaned `--protect-pid` are refused at start.

use srui_process_explorer::actions::{PanelIds, DRY_RUN_STATUS, READ_ONLY_STATUS, TERMINATE_LABEL};
use srui_process_explorer::{
    ACTIONS_ROW, ACTIONS_STATUS, ACTION_CANCEL, ACTION_PREPARE, MODEL, TABLE,
};
use srui_protocol::{
    decode_framed, encode_framed, srui_message::Msg, ClientHello, EventAckStatus, ServerEventAck,
    SruiMessage,
};
use srui_sdk::{ItemId, NodeId, PropertyRef, Value, COLUMNS, ENABLED, TEXT};
use srui_semantic_tree::{Event as DomainEvent, ResyncSnapshot, SemanticStore};
use srui_sessiond::CORE_VERSION;
use std::{
    fs,
    io::{Read, Write},
    os::unix::{fs::PermissionsExt, net::UnixStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const CLIENT: &[u8] = b"px028-wire-client";
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
            "/tmp/srtop-px028-{}-{nonce}-{sequence}",
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
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A client replica of one srtop session.
struct Client {
    stream: UnixStream,
    buffer: Vec<u8>,
    replica: SemanticStore,
    next_seq: u64,
}

impl Client {
    fn hello(socket: &Path) -> Self {
        let stream = UnixStream::connect(socket).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut client = Self {
            stream,
            buffer: Vec::new(),
            replica: SemanticStore::new(),
            next_seq: 1,
        };
        client.send(&SruiMessage {
            msg: Some(Msg::ClientHello(ClientHello {
                core_version: CORE_VERSION.to_string(),
                profiles: vec![PROFILE.to_string()],
                client_instance_id: CLIENT.to_vec(),
                ..ClientHello::default()
            })),
        });
        let welcome = loop {
            if let Some(Msg::ServerWelcome(welcome)) = client.next().msg {
                break welcome;
            }
        };
        let wire = loop {
            if let Some(Msg::Transaction(transaction)) = client.next_raw().msg {
                break transaction;
            }
        };
        assert_eq!(wire.new_revision, welcome.initial_revision);
        let snapshot = ResyncSnapshot::try_from(wire).expect("a valid snapshot");
        client
            .replica
            .replace_from_snapshot(&snapshot)
            .expect("the snapshot applies");
        client
    }

    fn revision(&self) -> u64 {
        self.replica.revision().get()
    }

    fn send(&mut self, message: &SruiMessage) {
        self.stream
            .write_all(&encode_framed(message).unwrap())
            .unwrap();
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
        self.send(&SruiMessage {
            msg: Some(Msg::Event(
                event
                    .clone()
                    .with_client_instance_id(CLIENT.to_vec())
                    .to_wire(),
            )),
        });
        let ack = loop {
            if let Some(Msg::ServerEventAck(ack)) = self.next().msg {
                break ack;
            }
        };
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
        DomainEvent::activate(seq, format!("px028-click-{seq}"), self.revision(), node)
    }

    fn click(&mut self, node: NodeId) -> ServerEventAck {
        let event = self.click_event(node);
        self.deliver(&event)
    }

    fn select(&mut self, pid: u64) {
        let item = self.item_of(pid);
        let seq = self.next_seq;
        self.next_seq += 1;
        let event = DomainEvent::selection_changed(
            seq,
            format!("px028-select-{seq}"),
            self.revision(),
            TABLE,
            item,
        );
        assert_eq!(self.deliver(&event).status, processed());
    }

    fn property(&self, node: NodeId, property: PropertyRef) -> Option<Value> {
        self.replica
            .get_node(node)
            .and_then(|node| node.get_property(property))
            .cloned()
    }

    /// Whether the table shows a row with PID `pid`.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    fn shows(&self, pid: u64) -> bool {
        let Some(Value::List(headings)) = self.property(TABLE, COLUMNS) else {
            return false;
        };
        let Some(at) = headings
            .iter()
            .position(|heading| heading == &Value::String("PID".into()))
        else {
            return false;
        };
        let Some(model) = self.replica.get_model(MODEL) else {
            return false;
        };
        (0..model.item_count)
            .filter_map(|index| model.get_item_by_index(index))
            .any(|item| match &item.value {
                Value::List(cells) => match &cells[at] {
                    Value::String(text) => text == &pid.to_string(),
                    Value::UnsignedInt(number) => *number == pid,
                    _ => false,
                },
                _ => false,
            })
    }

    fn status(&self) -> String {
        match self.property(ACTIONS_STATUS, TEXT) {
            Some(Value::String(text)) => text,
            other => panic!("no actions status: {other:?}"),
        }
    }

    fn enabled(&self, node: NodeId) -> bool {
        self.property(node, ENABLED) != Some(Value::Bool(false))
    }

    fn item_of(&self, pid: u64) -> ItemId {
        let Some(Value::List(headings)) = self.property(TABLE, COLUMNS) else {
            panic!("the table has headings")
        };
        let at = headings
            .iter()
            .position(|heading| heading == &Value::String("PID".into()))
            .expect("a PID column");
        let model = self.replica.get_model(MODEL).expect("the process model");
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
            .unwrap_or_else(|| panic!("PID {pid} is shown"))
            .item_id
    }
}

fn processed() -> i32 {
    EventAckStatus::Processed as i32
}

fn rejected() -> i32 {
    EventAckStatus::Rejected as i32
}

#[test]
fn a_dry_run_instance_settles_prepare_and_confirm_and_refuses_what_it_must() {
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
    let mut client = Client::hello(&socket);
    assert_eq!(client.status(), DRY_RUN_STATUS);
    assert!(client.enabled(ACTION_PREPARE));
    assert!(!client.enabled(ACTION_CANCEL));
    let panel = PanelIds::of(0);
    assert!(client.replica.get_node(panel.row).is_none(), "no panel yet");
    // A Confirm that does not exist is rejected at validation, before any
    // handler.
    let ack = client.click(panel.confirm);
    assert_eq!(ack.status, rejected(), "{ack:?}");

    // PID 4101 is the fixture account's own process.
    client.select(4101);
    assert_eq!(client.click(ACTION_PREPARE).status, processed());
    assert!(
        client
            .status()
            .starts_with("Prepared #1: terminate PID 4101 · worker (dry run)"),
        "{}",
        client.status()
    );
    assert!(client.enabled(panel.confirm) && client.enabled(ACTION_CANCEL));
    assert_eq!(
        client.property(panel.confirm, srui_sdk::LABEL),
        Some(Value::String(
            "Confirm dry run: terminate PID 4101 · worker".into()
        ))
    );
    let confirm = client.click_event(panel.confirm);
    let ack = client.deliver(&confirm);
    assert_eq!(ack.status, processed(), "{ack:?}");
    let status = client.status();
    assert!(
        status.starts_with("Dry run #1 requested (terminate PID 4101 · worker): nothing was sent"),
        "{status}"
    );
    assert!(
        client.replica.get_node(panel.row).is_none(),
        "a confirm is single-use: its panel is deleted"
    );
    // The same event again: the settled result, nothing re-run (§18.2).
    let revision = client.revision();
    let replay = client.deliver(&confirm);
    assert_eq!(
        replay.status,
        EventAckStatus::Duplicate as i32,
        "{replay:?}"
    );
    assert_eq!(replay.revision_after_effect, ack.revision_after_effect);
    assert_eq!(client.status(), status);
    assert_eq!(client.revision(), revision);

    // PID 4102 belongs to another account: refused at Prepare.
    client.select(4102);
    assert_eq!(client.click(ACTION_PREPARE).status, processed());
    assert!(
        client
            .status()
            .starts_with("Refused: the process is owned by UID 1001, not this account (UID 1000)"),
        "{}",
        client.status()
    );
    assert!(client.replica.get_node(PanelIds::of(1).row).is_none());
}

/// PX-030: `--actions terminate` over the wire. Off Linux there is no PID
/// file descriptor, so Terminate… is published disabled with the reason and a
/// click is rejected. On Linux the fixture's own process is prepared and its
/// panel names it; the Confirm reaches the terminator, which refuses to hold
/// a key this host's live scan did not produce, so nothing is sent; and a
/// redelivery of the same Confirm is a duplicate.
#[test]
fn a_terminate_instance_is_truthful_about_what_it_can_send() {
    let directory = SocketDirectory::new();
    let socket = directory.socket();
    let _server = Server::start(
        &socket,
        &[
            "--fake-source",
            "--actions",
            "terminate",
            "--refresh-interval-ms",
            "300000",
        ],
    );
    let mut client = Client::hello(&socket);
    assert_eq!(
        client.property(ACTION_PREPARE, srui_sdk::LABEL),
        Some(Value::String(TERMINATE_LABEL.into()))
    );
    client.select(4101);
    if !cfg!(target_os = "linux") {
        assert_eq!(
            client.status(),
            "Terminate unavailable: this system has no PID file descriptors (Linux 5.3 or later is needed); nothing can be sent to any process"
        );
        assert!(!client.enabled(ACTION_PREPARE));
        assert_eq!(client.click(ACTION_PREPARE).status, rejected());
        assert!(client.replica.get_node(PanelIds::of(0).row).is_none());
        return;
    }
    assert!(client.enabled(ACTION_PREPARE), "{}", client.status());
    assert_eq!(client.click(ACTION_PREPARE).status, processed());
    let panel = PanelIds::of(0);
    assert_eq!(
        client.property(panel.question, TEXT),
        Some(Value::String(
            "Terminate PID 4101 · worker (UID 1000)? Confirm sends it SIGTERM, which asks it to exit; it may handle or ignore it.".into()
        ))
    );
    let confirm = client.click_event(panel.confirm);
    let ack = client.deliver(&confirm);
    assert_eq!(ack.status, processed(), "{ack:?}");
    let status = client.status();
    assert!(
        status.starts_with("Refused #1 (terminate PID 4101 · worker): the process could not be held for signalling: the process cannot be identified here: it was not sampled from this host's /proc"),
        "{status}"
    );
    let replay = client.deliver(&confirm);
    assert_eq!(
        replay.status,
        EventAckStatus::Duplicate as i32,
        "{replay:?}"
    );
    assert_eq!(replay.revision_after_effect, ack.revision_after_effect);
}

/// PX-030: a PID configured with `--protect-pid` is refused at Prepare.
#[test]
fn a_configured_protected_pid_is_refused() {
    let directory = SocketDirectory::new();
    let socket = directory.socket();
    let _server = Server::start(
        &socket,
        &[
            "--fake-source",
            "--actions",
            "dry-run",
            "--protect-pid",
            "4101",
        ],
    );
    let mut client = Client::hello(&socket);
    client.select(4101);
    assert_eq!(client.click(ACTION_PREPARE).status, processed());
    assert!(
        client
            .status()
            .starts_with("Refused: PID 4101 is protected; nothing was requested"),
        "{}",
        client.status()
    );
    assert!(client.replica.get_node(PanelIds::of(0).row).is_none());
}

#[test]
fn a_read_only_instance_disables_and_rejects_every_action_control() {
    let directory = SocketDirectory::new();
    let socket = directory.socket();
    let _server = Server::start(&socket, &["--fake-source", "--actions", "read-only"]);
    let mut client = Client::hello(&socket);
    assert_eq!(client.status(), READ_ONLY_STATUS);
    client.select(4101);
    for button in [ACTION_PREPARE, ACTION_CANCEL, PanelIds::of(0).confirm] {
        assert!(
            !client.enabled(button) || client.replica.get_node(button).is_none(),
            "{button:?}"
        );
        let ack = client.click(button);
        assert_eq!(ack.status, rejected(), "{button:?}: {ack:?}");
    }
    assert_eq!(client.status(), READ_ONLY_STATUS);
}

#[test]
fn without_the_option_no_action_control_exists_and_bad_options_are_refused() {
    let directory = SocketDirectory::new();
    let socket = directory.socket();
    let server = Server::start(&socket, &["--fake-source"]);
    let mut client = Client::hello(&socket);
    for node in [
        ACTIONS_ROW,
        ACTION_PREPARE,
        ACTION_CANCEL,
        ACTIONS_STATUS,
        PanelIds::of(0).row,
        PanelIds::of(0).confirm,
    ] {
        assert!(client.replica.get_node(node).is_none(), "{node:?}");
    }
    let ack = client.click(ACTION_PREPARE);
    assert_eq!(ack.status, rejected(), "{ack:?}");
    drop(client);
    drop(server);
    for options in [
        &["--fake-source", "--actions", "enabled"][..],
        &["--fake-source", "--actions"][..],
        &["--actions", "dry-run"][..],
        &[
            "--fake-source",
            "--actions",
            "dry-run",
            "--actions",
            "dry-run",
        ][..],
        // PX-030: `--protect-pid` needs `--actions` and a positive PID.
        &["--fake-source", "--protect-pid", "4101"][..],
        &[
            "--fake-source",
            "--actions",
            "terminate",
            "--protect-pid",
            "0",
        ][..],
        &[
            "--fake-source",
            "--actions",
            "terminate",
            "--protect-pid",
            "x",
        ][..],
        &["--fake-source", "--actions", "terminate", "--protect-pid"][..],
    ] {
        let refused = SocketDirectory::new();
        let status = refused_at_start(&refused.socket(), options);
        assert!(!status.success(), "{options:?} was accepted");
        assert!(
            !refused.socket().exists(),
            "{options:?} published a socket before refusing"
        );
    }
}

/// Starts `srtop` with `options`, which it must refuse, and returns its exit
/// status (PX-030 verification round 1, F3). An instance that accepted them
/// would serve until killed, so this never waits unbounded: as soon as it
/// publishes its socket, or after a generous deadline (a newly linked
/// binary's first exec is assessed by macOS before `main` runs), that one
/// child is killed and reaped, and the test fails.
fn refused_at_start(socket: &Path, options: &[&str]) -> std::process::ExitStatus {
    let mut child = Command::new(env!("CARGO_BIN_EXE_srtop"))
        .arg("--socket")
        .arg(socket)
        .args(options)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        let accepted = socket.exists();
        if accepted || Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "{options:?} was not refused: {}",
                if accepted {
                    "srtop published its socket"
                } else {
                    "srtop was still running at the deadline"
                }
            );
        }
        thread::sleep(Duration::from_millis(10));
    }
}

/// The body of the transport helper (PX-030 verification round 1, F1): run
/// in a child process by
/// `a_connected_transport_is_refused_only_while_it_is_connected`, it holds a
/// client connection to the instance until told to drop it, then waits to be
/// told to exit. A no-op unless its parent set the environment; bounded by
/// its own deadline, so it cannot outlive a failed parent by long.
#[test]
fn transport_helper_body() {
    let (Ok(socket), Ok(directory)) = (
        std::env::var(TRANSPORT_HELPER_SOCKET),
        std::env::var(TRANSPORT_HELPER_DIR),
    ) else {
        return;
    };
    let directory = PathBuf::from(directory);
    let client = Client::hello(Path::new(&socket));
    fs::write(directory.join("connected"), b"").unwrap();
    let deadline = Instant::now() + Duration::from_secs(90);
    while !directory.join("release").exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
    drop(client);
    fs::write(directory.join("released"), b"").unwrap();
    while !directory.join("exit").exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(20));
    }
}

const TRANSPORT_HELPER_SOCKET: &str = "SRTOP_PX030_TRANSPORT_HELPER_SOCKET";
const TRANSPORT_HELPER_DIR: &str = "SRTOP_PX030_TRANSPORT_HELPER_DIR";

/// The transport helper, a child of this test (never an ancestor of srtop);
/// killed and reaped on drop.
#[cfg(target_os = "linux")]
struct Helper(Child);

#[cfg(target_os = "linux")]
impl Drop for Helper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[cfg(target_os = "linux")]
fn waits_for(path: &Path, deadline: Instant, what: &str) {
    while !path.exists() {
        assert!(Instant::now() < deadline, "{what}");
        thread::sleep(Duration::from_millis(20));
    }
}

/// PX-030 verification round 1 (F1): `main` registers every accepted
/// connection's transport with the support chain for as long as it lasts. A
/// helper process of this test, a sibling of srtop and not its ancestor,
/// connects to a live instance; while it is connected its PID is refused as a
/// client's transport, and once it has disconnected (still running) the same
/// process can be prepared. Linux only: it needs the live `/proc` source.
#[cfg(target_os = "linux")]
#[test]
fn a_connected_transport_is_refused_only_while_it_is_connected() {
    let directory = SocketDirectory::new();
    let socket = directory.socket();
    let _server = Server::start(
        &socket,
        &[
            "--live-source",
            "--refresh-interval-ms",
            "200",
            "--actions",
            "dry-run",
        ],
    );
    let mut client = Client::hello(&socket);
    let signals = directory.0.join("helper");
    fs::create_dir(&signals).unwrap();
    let helper = Helper(
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "transport_helper_body", "--test-threads", "1"])
            .env(TRANSPORT_HELPER_SOCKET, &socket)
            .env(TRANSPORT_HELPER_DIR, &signals)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let pid = u64::from(helper.0.id());
    let deadline = Instant::now() + Duration::from_secs(30);
    waits_for(
        &signals.join("connected"),
        deadline,
        "the helper never connected",
    );
    while !client.shows(pid) {
        assert!(Instant::now() < deadline, "the helper's row never appeared");
        client.next();
    }
    // Two more commits, so the row is the latest scan's confirmed instance.
    let at = client.revision();
    while client.revision() < at + 2 {
        client.next();
    }
    client.select(pid);
    assert_eq!(client.click(ACTION_PREPARE).status, processed());
    let status = client.status();
    assert!(
        status.starts_with(&format!(
            "Refused: PID {pid} is a connected client's transport (its SSH bridge), which serves this session"
        )),
        "a connected transport is refused: {status}"
    );
    assert!(client.replica.get_node(PanelIds::of(0).row).is_none());
    // The helper drops its connection and keeps running: it leaves the chain.
    fs::write(signals.join("release"), b"").unwrap();
    waits_for(
        &signals.join("released"),
        deadline,
        "the helper never released",
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        assert_eq!(client.click(ACTION_PREPARE).status, processed());
        let status = client.status();
        if status.starts_with("Prepared #") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "a disconnected transport leaves the chain: {status}"
        );
        thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(client.click(ACTION_CANCEL).status, processed());
    fs::write(signals.join("exit"), b"").unwrap();
    drop(helper);
    println!("PX-030 wire: helper PID {pid} refused as a connected transport, prepared once it disconnected");
}
