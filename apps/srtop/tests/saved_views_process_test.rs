//! PX-069 through the real binary: `--settings-path` and `--no-settings`, and
//! a restart of the `srtop` process recovering its saved views from the file
//! another run wrote. A fixture source keeps no settings unless a path is
//! named, so no test run reads or writes the real user's settings: here the
//! child's `HOME` and `XDG_CONFIG_HOME` point at a temporary directory
//! holding a settings file, and the fixture neither reads nor touches it.
use srui_process_explorer::{
    SAVED_VIEWS_ROW, SAVED_VIEW_CREATE, SAVED_VIEW_NAME_INPUT, SAVED_VIEW_SLOTS, SORT_CPU,
};
use srui_protocol::{
    decode_framed, encode_framed, srui_message::Msg, ClientHello, EventAckStatus, SruiMessage,
};
use srui_sdk::{NodeId, PropertyRef, Value};
use srui_semantic_tree::{EditSeq, Event as DomainEvent, ResyncSnapshot, SemanticStore};
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

/// A short private directory under `/tmp` (AF_UNIX paths stay short),
/// removed when dropped.
struct Directory(PathBuf);

impl Directory {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = PathBuf::from(format!(
            "/tmp/px069-{}-{nonce}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Server(Child);

impl Server {
    /// Starts srtop on `socket` with `options`, its `HOME` and
    /// `XDG_CONFIG_HOME` pointed at `home`.
    fn start(socket: &Path, home: &Path, options: &[&str]) -> Self {
        let mut server = Self(
            Command::new(env!("CARGO_BIN_EXE_srtop"))
                .arg("--socket")
                .arg(socket)
                .args(options)
                .env("HOME", home)
                .env("XDG_CONFIG_HOME", home.join("config"))
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        // Generous: a first exec of a fresh binary is assessed on macOS.
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            assert!(
                server.0.try_wait().unwrap().is_none(),
                "server exited before ready"
            );
            if UnixStream::connect(socket).is_ok() {
                return server;
            }
            assert!(Instant::now() < deadline, "socket did not become ready");
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn stop(&mut self) {
        // Only the live child this guard owns, with a fixed argument vector.
        assert!(Command::new("/bin/kill")
            .args(["-TERM", &self.0.id().to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success());
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                assert!(status.success(), "shutdown exited with {status}");
                return;
            }
            assert!(Instant::now() < deadline, "server did not shut down");
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

/// A blocking client holding a replica of everything committed.
struct Client {
    stream: UnixStream,
    buffer: Vec<u8>,
    replica: SemanticStore,
    seq: u64,
}

const CLIENT_INSTANCE: &[u8] = b"px069-process-client";

impl Client {
    fn attach(socket: &Path) -> Self {
        let stream = UnixStream::connect(socket).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut client = Self {
            stream,
            buffer: Vec::new(),
            replica: SemanticStore::new(),
            seq: 0,
        };
        client.send(&SruiMessage {
            msg: Some(Msg::ClientHello(ClientHello {
                core_version: CORE_VERSION.to_string(),
                profiles: vec!["org.srui.standard-widgets/1".to_string()],
                client_instance_id: CLIENT_INSTANCE.to_vec(),
                ..ClientHello::default()
            })),
        });
        loop {
            if let Some(Msg::Transaction(transaction)) = client.next().msg {
                if transaction.base_revision == 0 {
                    let snapshot = ResyncSnapshot::try_from(transaction).unwrap();
                    client.replica.replace_from_snapshot(&snapshot).unwrap();
                    return client;
                }
            }
        }
    }

    fn send(&mut self, message: &SruiMessage) {
        self.stream
            .write_all(&encode_framed(message).unwrap())
            .unwrap();
    }

    fn next(&mut self) -> SruiMessage {
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

    /// Sends `event`, applies every transaction until it is settled as
    /// processed.
    fn settle(&mut self, event: srui_protocol::Event) {
        self.send(&SruiMessage {
            msg: Some(Msg::Event(event)),
        });
        // The acknowledgement names the revision that holds the effect; the
        // transactions up to it may arrive on either side of it.
        let mut settled = None;
        loop {
            if settled.is_some_and(|revision| self.replica.revision().get() >= revision) {
                return;
            }
            match self.next().msg {
                Some(Msg::Transaction(transaction))
                    if transaction.base_revision == self.replica.revision().get() =>
                {
                    self.replica
                        .apply_delivered_transaction(transaction)
                        .unwrap();
                }
                Some(Msg::ServerEventAck(ack)) => {
                    assert_eq!(
                        ack.status,
                        EventAckStatus::Processed as i32,
                        "not processed: {ack:?}"
                    );
                    settled = Some(ack.revision_after_effect);
                }
                _ => {}
            }
        }
    }

    fn click(&mut self, node: NodeId) {
        self.seq += 1;
        let event = DomainEvent::activate(
            self.seq,
            format!("px069-click-{}", self.seq),
            self.replica.revision().get(),
            node,
        )
        .with_client_instance_id(CLIENT_INSTANCE.to_vec())
        .to_wire();
        self.settle(event);
    }

    fn type_into(&mut self, node: NodeId, text: &str) {
        self.seq += 1;
        let event = DomainEvent::text_edit(
            self.seq,
            format!("px069-edit-{}", self.seq),
            self.replica.revision().get(),
            node.get(),
            text,
            EditSeq::new(self.seq).unwrap(),
        )
        .with_client_instance_id(CLIENT_INSTANCE.to_vec())
        .to_wire();
        self.settle(event);
    }

    fn label(&self, node: NodeId) -> Option<String> {
        match self
            .replica
            .get_node(node)?
            .get_property(PropertyRef::LABEL)?
        {
            Value::String(text) => Some(text.clone()),
            _ => None,
        }
    }
}

fn refused(options: &[&str]) {
    let directory = Directory::new();
    let socket = directory.0.join("s.sock");
    let exit = Command::new(env!("CARGO_BIN_EXE_srtop"))
        .arg("--socket")
        .arg(&socket)
        .args(options)
        .env("HOME", &directory.0)
        .env("XDG_CONFIG_HOME", directory.0.join("config"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(!exit.success(), "{options:?} must be refused");
    assert!(!socket.exists(), "a refused start must publish no socket");
    assert!(
        !directory.0.join("config").exists(),
        "a refused start wrote settings"
    );
}

#[test]
fn contradictory_or_incomplete_settings_options_are_refused() {
    refused(&["--settings-path"]);
    refused(&["--settings-path", ""]);
    refused(&["--settings-path", "/tmp/px069-x/settings"]);
    refused(&[
        "--fake-source",
        "--settings-path",
        "/tmp/a",
        "--no-settings",
    ]);
    refused(&[
        "--fake-source",
        "--no-settings",
        "--settings-path",
        "/tmp/a",
    ]);
    refused(&[
        "--fake-source",
        "--settings-path",
        "/tmp/a",
        "--settings-path",
        "/tmp/b",
    ]);
    refused(&["--fake-source", "--no-settings", "--no-settings"]);
}

#[test]
fn a_restarted_process_recovers_the_views_another_run_saved() {
    let directory = Directory::new();
    let socket = directory.0.join("s.sock");
    let path = directory.0.join("views").join("settings");
    let path_text = path.to_str().unwrap();
    let options = ["--fake-source", "--settings-path", path_text];

    let mut server = Server::start(&socket, &directory.0, &options);
    let mut client = Client::attach(&socket);
    assert_eq!(
        client.label(SAVED_VIEW_SLOTS[0]).as_deref(),
        Some("View: Default (active)")
    );
    assert!(!path.exists(), "starting wrote the settings file");
    client.click(SORT_CPU);
    let sorted = client.label(SORT_CPU).unwrap();
    client.type_into(SAVED_VIEW_NAME_INPUT, "By CPU");
    client.click(SAVED_VIEW_CREATE);
    assert_eq!(
        client.label(SAVED_VIEW_SLOTS[1]).as_deref(),
        Some("View: By CPU (active)")
    );
    assert!(path.exists());
    drop(client);
    server.stop();

    let mut server = Server::start(&socket, &directory.0, &options);
    let client = Client::attach(&socket);
    assert_eq!(
        client.label(SAVED_VIEW_SLOTS[1]).as_deref(),
        Some("View: By CPU (active)")
    );
    assert_eq!(
        client.label(SORT_CPU),
        Some(sorted),
        "the restart applied the active view's sort"
    );
    drop(client);
    server.stop();
    assert!(
        !directory.0.join("config").exists(),
        "the default settings directory was used"
    );
}

#[test]
fn a_fixture_without_a_settings_path_neither_reads_nor_writes_the_users_settings() {
    let directory = Directory::new();
    let socket = directory.0.join("s.sock");
    // A settings file at the default path for this child's environment,
    // written by a newer srtop: were it read, the views row would say so.
    let default = directory.0.join("config").join("srtop").join("settings");
    fs::create_dir_all(default.parent().unwrap()).unwrap();
    fs::write(&default, b"srtop-settings 99\n").unwrap();
    for options in [
        &["--fake-source"][..],
        &["--fake-source", "--no-settings"][..],
    ] {
        let mut server = Server::start(&socket, &directory.0, options);
        let client = Client::attach(&socket);
        assert!(
            client.replica.get_node(SAVED_VIEWS_ROW).is_none(),
            "{options:?} created the saved views"
        );
        drop(client);
        server.stop();
    }
    assert_eq!(fs::read(&default).unwrap(), b"srtop-settings 99\n");
    let names: Vec<_> = fs::read_dir(default.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names, ["settings"]);
}
