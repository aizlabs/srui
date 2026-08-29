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
            core_version: "0.4.0".to_string(),
            profiles: vec!["org.srui.standard-widgets/1".to_string()],
            limits: None,
            client_instance_id: vec![7, 7, 7],
            client_metadata: Default::default(),
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
    tokio::spawn(async move { bridge_streams(ssh_bridge_side, session_bridge_side, shutdown).await })
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
    ssh_write.write_all(&frame).await.expect("partial writes to bridge");

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
    assert!(eof.is_none(), "session read should observe EOF after ssh close");

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

/// The binary routes tracing to stderr so stdout stays a pure protocol stream (§19.1, §20.1).
#[test]
fn binary_stderr_contains_no_protocol_bytes() {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixListener;
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    let socket_path = std::path::PathBuf::from(format!("/tmp/srui-bridge-{}", std::process::id()));
    let _ = std::fs::remove_file(&socket_path);

    let listener = UnixListener::bind(&socket_path).expect("bind unix listener");
    let (accept_tx, accept_rx) = mpsc::channel();

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

    let mut session_stream = accept_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("bridge should connect to session socket");
    accept_handle.join().expect("accept thread join");

    let frame = encode_framed(&sample_hello()).expect("encode hello");
    {
        let stdin = child.stdin.as_mut().expect("child stdin");
        stdin.write_all(&frame).expect("write framed hello to bridge stdin");
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

    let stderr_text = String::from_utf8_lossy(&stderr_bytes);
    assert!(
        stderr_text.contains("srui-ssh-bridge"),
        "stderr should contain bridge diagnostics, got: {stderr_text}"
    );
    assert!(
        !stderr_bytes.windows(frame.len()).any(|window| window == frame),
        "protocol frame bytes must not appear on stderr"
    );

    let _ = std::fs::remove_file(&socket_path);
}
