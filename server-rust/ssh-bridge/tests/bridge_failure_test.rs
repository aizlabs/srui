//! SSH bridge failure and edge-case tests (§20.1).
//!
//! Covers partial writes, fragmented frames, EOF in both directions, idle
//! cancellation, one-side close propagation, stderr isolation for the binary,
//! and backpressure ordering across a small duplex buffer.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use futures::{SinkExt, StreamExt};
use tokio::io::{duplex, AsyncWrite, AsyncWriteExt};
use tokio_util::codec::{FramedRead, FramedWrite};
use tokio_util::sync::CancellationToken;

use srui_protocol::{
    encode_framed, srui_message, ClientHello, SruiCodec, SruiMessage, Transaction,
};
use srui_ssh_bridge::bridge_streams;

fn sample_hello() -> SruiMessage {
    SruiMessage {
        msg: Some(srui_message::Msg::ClientHello(ClientHello {
            core_version: "0.5.0".to_string(),
            profiles: vec!["org.srui.standard-widgets/1".to_string()],
            limits: None,
            client_instance_id: vec![7, 7, 7],
            client_metadata: Default::default(),
            known_resource_hashes: vec![],
        })),
    }
}

fn sample_transaction(revision: u64) -> SruiMessage {
    SruiMessage {
        msg: Some(srui_message::Msg::Transaction(Transaction {
            base_revision: revision,
            new_revision: revision + 1,
            priority: 1,
            operations: vec![],
        })),
    }
}

async fn spawn_bridge(
    ssh_bridge_side: tokio::io::DuplexStream,
    session_bridge_side: tokio::io::DuplexStream,
    shutdown: CancellationToken,
) -> tokio::task::JoinHandle<Result<(), srui_ssh_bridge::BridgeError>> {
    tokio::spawn(
        async move { bridge_streams(ssh_bridge_side, session_bridge_side, shutdown).await },
    )
}

/// Limits each `write` call to at most `max_chunk` bytes so `copy_bidirectional`
/// must perform multiple partial writes.
struct LimitedWrite<W> {
    inner: W,
    max_chunk: usize,
}

impl<W> LimitedWrite<W> {
    fn new(inner: W, max_chunk: usize) -> Self {
        Self { inner, max_chunk }
    }
}

impl<W: AsyncWrite + Unpin> AsyncWrite for LimitedWrite<W> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        let this = self.get_mut();
        let chunk_len = buf.len().min(this.max_chunk);
        Pin::new(&mut this.inner).poll_write(cx, &buf[..chunk_len])
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

#[tokio::test]
async fn partial_writes_forward_all_bytes() {
    let (ssh_client, ssh_bridge_side) = duplex(1024);
    let (session_daemon, session_bridge_side) = duplex(1024);
    let shutdown = CancellationToken::new();

    let bridge_task = spawn_bridge(ssh_bridge_side, session_bridge_side, shutdown.clone()).await;

    let frame = encode_framed(&sample_hello()).expect("encode hello");
    let (mut _ssh_read, ssh_write) = tokio::io::split(ssh_client);
    let mut ssh_write = LimitedWrite::new(ssh_write, 1);
    ssh_write
        .write_all(&frame)
        .await
        .expect("partial writes to bridge");

    let (mut session_read, _session_write) = tokio::io::split(session_daemon);
    let mut received = vec![0u8; frame.len()];
    tokio::io::AsyncReadExt::read_exact(&mut session_read, &mut received)
        .await
        .expect("session read exact frame");
    assert_eq!(received, frame);

    shutdown.cancel();
    assert!(bridge_task.await.expect("bridge join").is_ok());
}

#[tokio::test]
async fn fragmented_frames_decode_after_bridge() {
    let (ssh_client, ssh_bridge_side) = duplex(1024);
    let (session_daemon, session_bridge_side) = duplex(1024);
    let shutdown = CancellationToken::new();

    let bridge_task = spawn_bridge(ssh_bridge_side, session_bridge_side, shutdown.clone()).await;

    let hello = sample_hello();
    let frame = encode_framed(&hello).expect("encode hello");

    let (mut _ssh_read, mut ssh_write) = tokio::io::split(ssh_client);
    for chunk in frame.chunks(3) {
        ssh_write.write_all(chunk).await.expect("write frame chunk");
    }

    let (session_read, _session_write) = tokio::io::split(session_daemon);
    let mut session_framed_read = FramedRead::new(session_read, SruiCodec::new());
    let received = session_framed_read
        .next()
        .await
        .expect("session received frame")
        .expect("decoded frame");
    assert_eq!(hello, received);

    shutdown.cancel();
    assert!(bridge_task.await.expect("bridge join").is_ok());
}

#[tokio::test]
async fn eof_on_ssh_side_terminates_bridge() {
    let (ssh_client, ssh_bridge_side) = duplex(1024);
    let (session_daemon, session_bridge_side) = duplex(1024);
    let shutdown = CancellationToken::new();

    let bridge_task = spawn_bridge(ssh_bridge_side, session_bridge_side, shutdown).await;

    drop(ssh_client);
    drop(session_daemon);

    let result = bridge_task.await.expect("bridge join");
    assert!(result.is_ok());
}

#[tokio::test]
async fn eof_on_session_side_terminates_bridge() {
    let (ssh_client, ssh_bridge_side) = duplex(1024);
    let (session_daemon, session_bridge_side) = duplex(1024);
    let shutdown = CancellationToken::new();

    let bridge_task = spawn_bridge(ssh_bridge_side, session_bridge_side, shutdown).await;

    drop(session_daemon);
    drop(ssh_client);

    let result = bridge_task.await.expect("bridge join");
    assert!(result.is_ok());
}

/// PX-022-G02: a daemon that stops or drops the connection ends the bridge even while the SSH side
/// stays open and idle, so the client's transport ends and it can say it is disconnected. Before,
/// the bridge waited for the SSH side to close as well, and an idle client stayed "connected" to
/// a daemon that no longer existed.
#[tokio::test]
async fn daemon_eof_ends_the_bridge_while_the_ssh_side_stays_open() {
    let (ssh_client, ssh_bridge_side) = duplex(1024);
    let (session_daemon, session_bridge_side) = duplex(1024);
    let shutdown = CancellationToken::new();

    let bridge_task = spawn_bridge(ssh_bridge_side, session_bridge_side, shutdown).await;

    // What the daemon sent before it went is still delivered.
    let transaction = sample_transaction(4);
    let (session_read, session_write) = tokio::io::split(session_daemon);
    let mut session_framed_write = FramedWrite::new(session_write, SruiCodec::new());
    session_framed_write
        .send(transaction.clone())
        .await
        .expect("send from the daemon");
    drop(session_framed_write);
    drop(session_read);

    let (ssh_read, _ssh_write) = tokio::io::split(ssh_client);
    let mut ssh_framed_read = FramedRead::new(ssh_read, SruiCodec::new());
    let received = ssh_framed_read
        .next()
        .await
        .expect("ssh received frame")
        .expect("decoded frame");
    assert_eq!(transaction, received);

    let result = tokio::time::timeout(std::time::Duration::from_secs(5), bridge_task)
        .await
        .expect("the bridge ends when the daemon side closes, with the ssh side still open");
    assert!(result.expect("bridge join").is_ok());
    assert!(
        ssh_framed_read.next().await.is_none(),
        "the ssh side sees EOF once the bridge has ended"
    );
}

#[tokio::test]
async fn cancellation_while_idle_exits_cleanly() {
    let (_ssh_client, ssh_bridge_side) = duplex(1024);
    let (_session_daemon, session_bridge_side) = duplex(1024);
    let shutdown = CancellationToken::new();

    let bridge_task = spawn_bridge(ssh_bridge_side, session_bridge_side, shutdown.clone()).await;

    shutdown.cancel();

    let result = bridge_task.await.expect("bridge join");
    assert!(result.is_ok());
}

#[tokio::test]
async fn one_side_close_propagates_eof_to_peer() {
    let (ssh_client, ssh_bridge_side) = duplex(1024);
    let (session_daemon, session_bridge_side) = duplex(1024);
    let shutdown = CancellationToken::new();

    let bridge_task = spawn_bridge(ssh_bridge_side, session_bridge_side, shutdown).await;

    let hello = sample_hello();
    let (mut _ssh_read, ssh_write) = tokio::io::split(ssh_client);
    let mut ssh_framed_write = FramedWrite::new(ssh_write, SruiCodec::new());
    ssh_framed_write
        .send(hello.clone())
        .await
        .expect("send hello from ssh");

    let (mut session_read, mut session_write) = tokio::io::split(session_daemon);
    let mut session_framed_read = FramedRead::new(&mut session_read, SruiCodec::new());
    let received = session_framed_read
        .next()
        .await
        .expect("session received frame")
        .expect("decoded frame");
    assert_eq!(hello, received);

    // Close SSH write half; session read should eventually observe EOF.
    ssh_framed_write.close().await.expect("close ssh write");
    drop(ssh_framed_write);

    let eof = session_framed_read.next().await;
    assert!(
        eof.is_none(),
        "session read should observe EOF after ssh close"
    );

    session_write.shutdown().await.expect("close session write");
    drop(session_read);
    drop(session_write);

    assert!(bridge_task.await.expect("bridge join").is_ok());
}

#[tokio::test]
async fn backpressure_preserves_message_order() {
    // Small buffer forces the forwarder to wait for downstream reads.
    let (ssh_client, ssh_bridge_side) = duplex(64);
    let (session_daemon, session_bridge_side) = duplex(64);
    let shutdown = CancellationToken::new();

    let bridge_task = spawn_bridge(ssh_bridge_side, session_bridge_side, shutdown.clone()).await;

    let messages = vec![
        sample_transaction(0),
        sample_transaction(1),
        sample_transaction(2),
    ];

    let (mut _ssh_read, ssh_write) = tokio::io::split(ssh_client);
    let mut ssh_framed_write = FramedWrite::new(ssh_write, SruiCodec::new());
    for msg in &messages {
        ssh_framed_write.send(msg.clone()).await.expect("send tx");
    }

    let (session_read, _session_write) = tokio::io::split(session_daemon);
    let mut session_framed_read = FramedRead::new(session_read, SruiCodec::new());

    for expected in &messages {
        let received = session_framed_read
            .next()
            .await
            .expect("session received frame")
            .expect("decoded frame");
        assert_eq!(expected, &received);
    }

    shutdown.cancel();
    assert!(bridge_task.await.expect("bridge join").is_ok());
}

/// Runs `srui-ssh-bridge` once, outside the readiness deadline of `binary_stderr_contains_no_protocol_bytes`.
///
/// macOS assesses a newly linked executable on its first exec, before `main`; the first exec of a
/// just-relinked binary took 12-19 s on a busy machine and the next 0.02 s. A relink of `srui-ssh-bridge`
/// followed at once by this test failed it 2 of 5 times on its 5 s deadline, with nothing wrong
/// in the binary. `--socket` with no path makes it refuse and exit before it connects or binds
/// anything, so this run does nothing else.
fn absorb_first_launch_assessment() {
    static ASSESSED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    ASSESSED.get_or_init(|| {
        let mut probe = std::process::Command::new(env!("CARGO_BIN_EXE_srui-ssh-bridge"))
            .arg("--socket")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn the srui-ssh-bridge warm-up");
        // Bounded so a launch that never completes still fails rather than hangs the run, and
        // killed then so it cannot outlive the test.
        let deadline = std::time::Instant::now() + LAUNCH_ASSESSMENT_BOUND;
        while probe.try_wait().expect("poll the srui-ssh-bridge warm-up").is_none() {
            if std::time::Instant::now() >= deadline {
                let _ = probe.kill();
                let _ = probe.wait();
                panic!("the first exec of srui-ssh-bridge did not finish within {LAUNCH_ASSESSMENT_BOUND:?}");
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    });
}

/// Far above the 12-19 s measured for a first exec on a busy machine.
const LAUNCH_ASSESSMENT_BOUND: std::time::Duration = std::time::Duration::from_secs(120);

/// PX-022-G02: the real binary exits when the daemon closes its connection, although the SSH side
/// keeps its standard input open and idle, and its standard output then reads EOF. That is how
/// an idle client learns a daemon stopped: an open bridge process holds the SSH channel open.
/// Skipped as root for the reason given at `binary_stderr_contains_no_protocol_bytes`.
#[test]
fn binary_exits_when_the_daemon_closes_while_stdin_stays_open() {
    if srui_unix_security::effective_uid() == 0 {
        eprintln!(
            "skipping binary_exits_when_the_daemon_closes_while_stdin_stays_open: \
             srui-ssh-bridge refuses effective UID 0 (§27)"
        );
        return;
    }

    use std::io::{Read, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant, SystemTime};

    let nonce = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let runtime_dir =
        std::path::Path::new("/tmp").join(format!("srui-d-{}-{nonce}", std::process::id()));
    std::fs::create_dir(&runtime_dir).expect("create private bridge test runtime directory");
    std::fs::set_permissions(&runtime_dir, std::fs::Permissions::from_mode(0o700))
        .expect("secure bridge test runtime directory");
    let socket_path = runtime_dir.join("s");
    let listener = UnixListener::bind(&socket_path).expect("bind unix listener");
    std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))
        .expect("secure bridge test socket");
    absorb_first_launch_assessment();

    let mut child = Command::new(env!("CARGO_BIN_EXE_srui-ssh-bridge"))
        .arg(&socket_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn srui-ssh-bridge");
    // Kept open for the whole test: the SSH side never closes.
    let stdin = child.stdin.take().expect("child stdin");

    listener
        .set_nonblocking(true)
        .expect("a non-blocking listener");
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut daemon = loop {
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = std::fs::remove_dir_all(&runtime_dir);
                    panic!("bridge should connect to session socket");
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("accept: {error}"),
        }
    };
    daemon
        .set_nonblocking(false)
        .expect("a blocking daemon stream");
    let frame = encode_framed(&sample_transaction(9)).expect("encode transaction");
    daemon.write_all(&frame).expect("the daemon sends a frame");
    // The daemon goes.
    drop(daemon);

    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll the bridge") {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_dir_all(&runtime_dir);
            panic!("the bridge outlived its daemon's connection with stdin still open");
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(
        status.success(),
        "a daemon that closes is a clean end: {status}"
    );
    let mut forwarded = Vec::new();
    child
        .stdout
        .take()
        .expect("child stdout")
        .read_to_end(&mut forwarded)
        .expect("stdout reads to EOF");
    assert_eq!(forwarded, frame, "what the daemon sent first was delivered");
    drop(stdin);

    let _ = std::fs::remove_file(&socket_path);
    std::fs::remove_dir(&runtime_dir).expect("remove bridge test runtime directory");
}

/// The binary routes tracing to stderr so stdout stays a pure protocol stream (§19.1, §20.1).
///
/// Spawns the real binary, which refuses effective UID 0 (§27). Containerized runners commonly
/// execute as root, where the child would exit before accepting and this test would block on
/// `accept(2)` until the harness timed out. Skip explicitly rather than hang.
#[test]
fn binary_stderr_contains_no_protocol_bytes() {
    if srui_unix_security::effective_uid() == 0 {
        eprintln!(
            "skipping binary_stderr_contains_no_protocol_bytes: srui-ssh-bridge refuses \
             effective UID 0 (§27); run as an unprivileged user to exercise it"
        );
        return;
    }

    use std::io::{Read, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, SystemTime};

    let nonce = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("system clock")
        .as_nanos();
    let runtime_dir =
        std::path::Path::new("/tmp").join(format!("srui-b-{}-{nonce}", std::process::id()));
    std::fs::create_dir(&runtime_dir).expect("create private bridge test runtime directory");
    std::fs::set_permissions(&runtime_dir, std::fs::Permissions::from_mode(0o700))
        .expect("secure bridge test runtime directory");
    let socket_path = runtime_dir.join("s");

    let listener = UnixListener::bind(&socket_path).expect("bind unix listener");
    std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))
        .expect("secure bridge test socket");
    let (accept_tx, accept_rx) = mpsc::channel();
    absorb_first_launch_assessment();

    let accept_handle = thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept bridge connection");
        accept_tx.send(stream).expect("send accepted stream");
    });

    let mut child = Command::new(env!("CARGO_BIN_EXE_srui-ssh-bridge"))
        .arg(&socket_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn srui-ssh-bridge");

    let mut session_stream = match accept_rx.recv_timeout(Duration::from_secs(5)) {
        Ok(stream) => stream,
        Err(error) => {
            // Leave nothing behind: a failed run used to orphan the bridge and its socket
            // directory.
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_dir_all(&runtime_dir);
            panic!("bridge should connect to session socket: {error:?}");
        }
    };
    accept_handle.join().expect("accept thread join");

    let frame = encode_framed(&sample_hello()).expect("encode hello");
    {
        let stdin = child.stdin.as_mut().expect("child stdin");
        stdin
            .write_all(&frame)
            .expect("write framed hello to bridge stdin");
    }

    let mut received = vec![0u8; frame.len()];
    session_stream
        .read_exact(&mut received)
        .expect("session socket receives forwarded frame");
    assert_eq!(received, frame);

    thread::sleep(Duration::from_millis(100));
    child.kill().expect("terminate bridge child");
    let mut stderr_bytes = Vec::new();
    child
        .stderr
        .take()
        .expect("child stderr")
        .read_to_end(&mut stderr_bytes)
        .expect("read stderr");
    let status = child.wait().expect("reap bridge child");
    assert!(
        !status.success(),
        "killed bridge child must not exit successfully"
    );

    let stderr_text = String::from_utf8_lossy(&stderr_bytes);
    assert!(
        stderr_text.contains("srui-ssh-bridge"),
        "stderr should contain bridge diagnostics, got: {stderr_text}"
    );
    assert!(
        !stderr_bytes
            .windows(frame.len())
            .any(|window| window == frame),
        "protocol frame bytes must not appear on stderr"
    );

    let _ = std::fs::remove_file(&socket_path);
    std::fs::remove_dir(&runtime_dir).expect("remove bridge test runtime directory");
}
