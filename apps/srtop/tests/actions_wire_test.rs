//! PX-028 at the wire: the action controls of a real `srtop --actions`
//! instance, driven by a minimal client replica over its private socket (D1
//! §§7.7, 18.2, 27; D2 T24).
//!
//! Every click is a framed `EVENT` the instance's connection loop admits,
//! validates and dispatches, and every answer is the `SERVER_EVENT_ACK` it
//! settles with:
//!
//! * `--actions dry-run`: Confirm is disabled, and a click on it rejected,
//!   until Prepare has prepared an intent for the selected process; Confirm
//!   then settles as processed with the dry run requested, a redelivery of
//!   the same event is a duplicate that changes nothing, and another
//!   account's process is refused at Prepare;
//! * `--actions read-only`: every control is disabled and a click rejected;
//! * without `--actions` no action control exists, so a click names no node;
//! * an unknown mode, and `--actions` without a process source, are refused at
//!   start.

use srui_process_explorer::actions::{DRY_RUN_STATUS, READ_ONLY_STATUS};
use srui_process_explorer::{
    ACTIONS_ROW, ACTIONS_STATUS, ACTION_CANCEL, ACTION_CONFIRM, ACTION_PREPARE, MODEL, TABLE,
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
    assert!(!client.enabled(ACTION_CONFIRM) && !client.enabled(ACTION_CANCEL));
    // A disabled Confirm is rejected at validation, before any handler.
    let ack = client.click(ACTION_CONFIRM);
    assert_eq!(ack.status, rejected(), "{ack:?}");
    assert!(ack.reject_reason.contains("disabled"), "{ack:?}");

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
    assert!(client.enabled(ACTION_CONFIRM) && client.enabled(ACTION_CANCEL));
    let confirm = client.click_event(ACTION_CONFIRM);
    let ack = client.deliver(&confirm);
    assert_eq!(ack.status, processed(), "{ack:?}");
    let status = client.status();
    assert!(
        status.starts_with("Dry run #1 requested (terminate PID 4101 · worker): nothing was sent"),
        "{status}"
    );
    assert!(!client.enabled(ACTION_CONFIRM), "a confirm is single-use");
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
    assert!(!client.enabled(ACTION_CONFIRM));
}

#[test]
fn a_read_only_instance_disables_and_rejects_every_action_control() {
    let directory = SocketDirectory::new();
    let socket = directory.socket();
    let _server = Server::start(&socket, &["--fake-source", "--actions", "read-only"]);
    let mut client = Client::hello(&socket);
    assert_eq!(client.status(), READ_ONLY_STATUS);
    client.select(4101);
    for button in [ACTION_PREPARE, ACTION_CONFIRM, ACTION_CANCEL] {
        assert!(!client.enabled(button), "{button:?}");
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
        ACTION_CONFIRM,
        ACTION_CANCEL,
        ACTIONS_STATUS,
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
    ] {
        let refused = SocketDirectory::new();
        let output = Command::new(env!("CARGO_BIN_EXE_srtop"))
            .arg("--socket")
            .arg(refused.socket())
            .args(options)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .unwrap();
        assert!(!output.status.success(), "{options:?} was accepted");
        assert!(
            !refused.socket().exists(),
            "{options:?} published a socket before refusing"
        );
    }
}
