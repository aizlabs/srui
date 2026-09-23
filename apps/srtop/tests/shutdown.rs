//! Real owned-process regression for normal stop/restart cycles.
use std::{
    fs,
    os::unix::{fs::PermissionsExt, net::UnixStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
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

struct Server(Child);

impl Server {
    fn start(socket: &Path) -> Self {
        Self::start_with(socket, &[])
    }

    fn start_with(socket: &Path, options: &[&str]) -> Self {
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
    for interval in ["0", "1", "600000", "not-a-number"] {
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

#[test]
fn sigint_cleans_socket_and_allows_restart() {
    assert_restart_after("-INT");
}
