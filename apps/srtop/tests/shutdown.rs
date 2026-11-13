//! Real owned-process regression for normal stop/restart cycles.
use srui_process_explorer::{INTERVAL_APPLY, INTERVAL_INPUT, SAMPLING_PAUSE};
use srui_protocol::{
    decode_framed, encode_framed, srui_message::Msg, ClientHello, EventAckStatus, SruiMessage,
};
use srui_semantic_tree::{EditSeq, Event as DomainEvent};
use srui_sessiond::CORE_VERSION;
use std::{
    fs,
    io::{Read, Write},
    os::unix::{fs::PermissionsExt, net::UnixStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        OnceLock,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct SocketDirectory(PathBuf);

impl SocketDirectory {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        // Keep AF_UNIX paths short even when macOS TMPDIR is deeply nested.
        // The clock can return the same timestamp to parallel test threads.
        static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);
        let sequence = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let path = PathBuf::from(format!(
            "/tmp/srtop-{}-{nonce}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }
}

impl Drop for SocketDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Runs the binary once, outside the readiness deadline below.
///
/// macOS assesses a newly linked executable on its first exec, before `main`, and `cargo test`
/// and `cargo build` alternately replace `target/debug/srtop` (observed by its mtime). On a
/// busy machine that first exec took 12-19 s and the next 0.02 s, so the 10 s readiness deadline
/// was spent on the platform's assessment, not on srtop: this suite failed 4 of 6 tests right
/// after `apps/srtop/test.sh`'s Swift run, and passed alone. With no arguments srtop only prints
/// its usage, so this run does nothing else.
fn absorb_first_launch_assessment() {
    static ASSESSED: OnceLock<()> = OnceLock::new();
    ASSESSED.get_or_init(|| {
        let mut probe = Command::new(env!("CARGO_BIN_EXE_srtop"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        // Bounded so a launch that never completes still fails rather than hangs the run, and
        // killed then so it cannot outlive the test.
        let deadline = Instant::now() + LAUNCH_ASSESSMENT_BOUND;
        while probe.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                let _ = probe.kill();
                let _ = probe.wait();
                panic!("the first exec of srtop did not finish within {LAUNCH_ASSESSMENT_BOUND:?}");
            }
            thread::sleep(Duration::from_millis(50));
        }
    });
}

/// Far above the 12-19 s measured for a first exec on a busy machine.
const LAUNCH_ASSESSMENT_BOUND: Duration = Duration::from_secs(120);

struct Server(Child);

impl Server {
    fn start(socket: &Path) -> Self {
        Self::start_with(socket, &[])
    }

    fn start_with(socket: &Path, options: &[&str]) -> Self {
        absorb_first_launch_assessment();
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
        let deadline = Instant::now() + Duration::from_secs(10);
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

    fn stop(&mut self, signal: &str) {
        // Only signal the live child owned by this guard, using fixed argv.
        assert!(Command::new("/bin/kill")
            .args([signal, &self.0.id().to_string()])
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

fn assert_restart_after(signal: &str) {
    let directory = SocketDirectory::new();
    let socket = directory.0.join("s.sock");
    // The second launch must bind the same path without manual unlinking.
    for _ in 0..2 {
        let mut server = Server::start(&socket);
        server.stop(signal);
        assert!(!socket.exists(), "shutdown left its socket behind");
    }
}

#[test]
fn sigterm_cleans_socket_and_allows_restart() {
    assert_restart_after("-TERM");
}

/// A collecting instance keeps polling in the background; a normal stop must
/// still shut it down and release the socket it owns.
#[test]
fn a_polling_instance_stops_and_releases_its_socket() {
    let directory = SocketDirectory::new();
    let socket = directory.0.join("s.sock");
    let mut server =
        Server::start_with(&socket, &["--fake-sequence", "--refresh-interval-ms", "50"]);
    // Long enough for several ticks, including the script's failed scan.
    thread::sleep(Duration::from_millis(400));
    server.stop("-TERM");
    assert!(!socket.exists(), "shutdown left its socket behind");
}

/// The sampling interval is configuration, and an unusable value is refused
/// rather than quietly clamped into something else.
#[test]
fn an_out_of_range_refresh_interval_is_refused_without_publishing_a_socket() {
    let directory = SocketDirectory::new();
    let socket = directory.0.join("s.sock");
    for interval in ["0", "1", "600000", "not-a-number", "+50", "1.5", "50ms", ""] {
        let exit = Command::new(env!("CARGO_BIN_EXE_srtop"))
            .arg("--socket")
            .arg(&socket)
            .args(["--live-source", "--refresh-interval-ms", interval])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(!exit.success(), "interval {interval} must be refused");
        assert!(!socket.exists(), "a refused start must publish no socket");
    }
}

/// Two sources are a contradiction, not a preference order.
#[test]
fn two_sources_are_refused() {
    let directory = SocketDirectory::new();
    let socket = directory.0.join("s.sock");
    for pair in [
        ["--fake-source", "--live-source"],
        ["--fake-source", "--fake-sequence"],
        ["--fake-sequence", "--fake-sequence"],
    ] {
        let exit = Command::new(env!("CARGO_BIN_EXE_srtop"))
            .arg("--socket")
            .arg(&socket)
            .args(pair)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert!(!exit.success(), "{pair:?} must be refused");
        assert!(!socket.exists());
    }
}

/// A blocking client of a running instance, for driving its sampling
/// controls the way a real client does (PX-015).
struct Client {
    stream: UnixStream,
    buffer: Vec<u8>,
    seq: u64,
}

const CLIENT_INSTANCE: &[u8] = b"px015-shutdown-client";

impl Client {
    /// Connects, says hello and waits for the catch-up snapshot.
    fn attach(socket: &Path) -> Self {
        let stream = UnixStream::connect(socket).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut client = Self {
            stream,
            buffer: Vec::new(),
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

    /// Sends `event` and waits until the server settles it as processed.
    fn settle(&mut self, event: srui_protocol::Event) {
        self.send(&SruiMessage {
            msg: Some(Msg::Event(event)),
        });
        loop {
            if let Some(Msg::ServerEventAck(ack)) = self.next().msg {
                assert_eq!(
                    ack.status,
                    EventAckStatus::Processed as i32,
                    "not processed: {ack:?}"
                );
                return;
            }
        }
    }

    fn click(&mut self, node: srui_sdk::NodeId) {
        self.seq += 1;
        let event = DomainEvent::activate(self.seq, format!("px015-click-{}", self.seq), 0, node)
            .with_client_instance_id(CLIENT_INSTANCE.to_vec())
            .to_wire();
        self.settle(event);
    }

    fn type_into(&mut self, node: srui_sdk::NodeId, text: &str) {
        self.seq += 1;
        let event = DomainEvent::text_edit(
            self.seq,
            format!("px015-edit-{}", self.seq),
            0u64,
            node.get(),
            text,
            EditSeq::new(self.seq).unwrap(),
        )
        .with_client_instance_id(CLIENT_INSTANCE.to_vec())
        .to_wire();
        self.settle(event);
    }
}

/// PX-015: every sampling state a real client can put a collecting instance
/// in — paused, resumed, retimed, and paused again with the client gone —
/// still stops on a normal signal and releases the socket. The one ticker
/// belongs to the session's refresh loop, not to a connection: a client that
/// disconnects ends only its own connection task, and the loop, its ticker
/// and its wake end with the instance.
#[test]
fn a_paused_or_retimed_instance_stops_and_releases_its_socket() {
    for signal in ["-TERM", "-INT"] {
        let directory = SocketDirectory::new();
        let socket = directory.0.join("s.sock");
        let mut server =
            Server::start_with(&socket, &["--fake-sequence", "--refresh-interval-ms", "50"]);
        let mut client = Client::attach(&socket);
        client.click(SAMPLING_PAUSE);
        thread::sleep(Duration::from_millis(200));
        client.click(SAMPLING_PAUSE);
        client.type_into(INTERVAL_INPUT, "75");
        client.click(INTERVAL_APPLY);
        client.type_into(INTERVAL_INPUT, "60");
        client.click(INTERVAL_APPLY);
        thread::sleep(Duration::from_millis(200));
        client.click(SAMPLING_PAUSE);
        // The client goes away while the view is paused.
        drop(client);
        thread::sleep(Duration::from_millis(100));
        // A new client attaches to the same paused session.
        let reattached = Client::attach(&socket);
        drop(reattached);
        server.stop(signal);
        assert!(!socket.exists(), "shutdown left its socket behind");
    }
}

#[test]
fn sigint_cleans_socket_and_allows_restart() {
    assert_restart_after("-INT");
}
