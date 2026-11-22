//! PX-027: what the R1 showcase measures on the wire, and how (D1 §§31–32; D2 T34).
//!
//! Two byte counts are kept apart for every run, never added together:
//!
//! * **Semantic payload**: the SRUI frames themselves. Over SSH they are the
//!   bytes of the `srui` subsystem channel, which is exactly what `ssh -s srui`
//!   reads and writes on its standard streams, and exactly what the bridge
//!   carries to srtop's socket. [`SruiProbe`] counts them twice, independently:
//!   as raw bytes read and as the sum of the decoded frames' extents. The two
//!   must agree, so a probe that counted a frame twice is refused.
//! * **Encrypted transport**: the bytes of the SSH connection's TCP stream.
//!   [`TcpCountRelay`] is a userspace TCP relay in front of the devbox's port:
//!   `ssh` connects to it and every byte of the encrypted stream crosses it and
//!   is counted. No root, no pcap, no packet filter. The SSH overhead of a run
//!   is the encrypted count minus the payload count, which can never be
//!   negative; [`Split::new`] refuses a split that says otherwise.
//!
//! [`TtyProbe`] measures htop the same way: the bytes of its pseudo-terminal
//! stream (what `ssh -tt` reads and writes) as its payload, and the same relay
//! for the encrypted stream.
//!
//! The relay can also add a fixed delay in each direction (RTT/2 each way),
//! so the RTT envelope is measured over real SSH, the same way PX-025's
//! [`crate::relay::DelayRelay`] adds RTT on the Unix socket behind the bridge.

use crate::{relay::Traffic, wire::frame_extent};
use srui_process_explorer::{MODEL, SAMPLING_STATUS};
use srui_protocol::{
    decode_framed, encode_framed, srui_message::Msg, ClientHello, SruiMessage,
    DEFAULT_MAX_FRAME_SIZE,
};
use srui_semantic_tree::{PropertyRef, ResyncSnapshot, SemanticStore, Value};
use srui_sessiond::{logical_class_for_server_envelope, LogicalChannelClass, CORE_VERSION};
use std::{
    io::{self, Read, Write},
    net::{Shutdown, SocketAddr, TcpListener, TcpStream},
    os::unix::net::UnixStream,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub type Result<T> = std::result::Result<T, String>;

/// The largest RTT the TCP relay injects: the same ceiling as PX-025's relay.
pub const MAX_TCP_RTT: Duration = crate::relay::MAX_RTT;
/// The largest single read the TCP relay makes.
const TCP_CHUNK: usize = 64 * 1024;
/// Chunks one delayed direction may hold before its reader stops reading:
/// 256 × 64 KiB = 16 MiB, eight times SSH's 2 MiB channel window.
const TCP_DELAY_CHUNKS: usize = 256;
/// How long a relay read waits before it looks at the stop flag again.
const TCP_READ_POLL: Duration = Duration::from_millis(25);
/// How long the relay's accept loop sleeps between polls.
const TCP_ACCEPT_POLL: Duration = Duration::from_millis(5);
/// How long the relay waits for the upstream connection.
const TCP_CONNECT_DEADLINE: Duration = Duration::from_secs(5);
/// The most events one probe records; a run past it is refused, not truncated.
pub const MAX_PROBE_EVENTS: usize = 200_000;

struct TcpInner {
    upstream: SocketAddr,
    one_way: (Duration, Duration),
    up: AtomicU64,
    down: AtomicU64,
    accepted: AtomicU64,
    stopping: AtomicBool,
    sockets: Mutex<Vec<TcpStream>>,
    pumps: Mutex<Vec<JoinHandle<()>>>,
}

/// A userspace TCP relay on `127.0.0.1` that joins every client it accepts to
/// `upstream`, counts every byte it carries in each direction, and optionally
/// delays each direction by RTT/2.
pub struct TcpCountRelay {
    local: SocketAddr,
    inner: Arc<TcpInner>,
    accept: Option<JoinHandle<()>>,
}

impl TcpCountRelay {
    /// Listens on an ephemeral loopback port. `rtt` is split RTT/2 up and the
    /// rest down, as [`crate::relay::DelayRelay`] splits it.
    pub fn start(upstream: SocketAddr, rtt: Duration) -> io::Result<Self> {
        if rtt > MAX_TCP_RTT {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("an RTT of {rtt:?} exceeds {MAX_TCP_RTT:?}"),
            ));
        }
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let local = listener.local_addr()?;
        let up = rtt / 2;
        let inner = Arc::new(TcpInner {
            upstream,
            one_way: (up, rtt - up),
            up: AtomicU64::new(0),
            down: AtomicU64::new(0),
            accepted: AtomicU64::new(0),
            stopping: AtomicBool::new(false),
            sockets: Mutex::new(Vec::new()),
            pumps: Mutex::new(Vec::new()),
        });
        let accepting = Arc::clone(&inner);
        let accept = thread::Builder::new()
            .name("tcp-count-accept".into())
            .spawn(move || tcp_accept_loop(listener, accepting))?;
        Ok(Self {
            local,
            inner,
            accept: Some(accept),
        })
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local
    }

    /// Every byte carried so far: `up` client to upstream, `down` back.
    pub fn traffic(&self) -> Traffic {
        Traffic {
            up: self.inner.up.load(Ordering::SeqCst),
            down: self.inner.down.load(Ordering::SeqCst),
        }
    }

    pub fn accepted(&self) -> u64 {
        self.inner.accepted.load(Ordering::SeqCst)
    }

    pub fn one_way(&self) -> (Duration, Duration) {
        self.inner.one_way
    }

    /// Stops accepting, shuts every live connection and joins every thread.
    pub fn stop(&mut self) {
        self.inner.stopping.store(true, Ordering::SeqCst);
        if let Some(accept) = self.accept.take() {
            let _ = accept.join();
        }
        for socket in lock(&self.inner.sockets).drain(..) {
            let _ = socket.shutdown(Shutdown::Both);
        }
        let pumps: Vec<_> = lock(&self.inner.pumps).drain(..).collect();
        for pump in pumps {
            let _ = pump.join();
        }
    }
}

impl Drop for TcpCountRelay {
    fn drop(&mut self) {
        self.stop();
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn tcp_accept_loop(listener: TcpListener, inner: Arc<TcpInner>) {
    // lc-debt: the accept loop polls every 5 ms so `stop` never waits on a
    // blocked accept; upgrade path: a self-pipe and poll(2), which needs FFI
    // this std-only package avoids.
    while !inner.stopping.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((client, _)) => {
                inner.accepted.fetch_add(1, Ordering::SeqCst);
                if let Err(error) = tcp_join(client, &inner) {
                    eprintln!("tcp-count-relay: {error}");
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(TCP_ACCEPT_POLL)
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => {
                eprintln!("tcp-count-relay: accept: {error}");
                thread::sleep(TCP_ACCEPT_POLL);
            }
        }
    }
}

fn tcp_join(client: TcpStream, inner: &Arc<TcpInner>) -> io::Result<()> {
    client.set_nonblocking(false)?;
    let server = match TcpStream::connect_timeout(&inner.upstream, TCP_CONNECT_DEADLINE) {
        Ok(server) => server,
        Err(error) => {
            let _ = client.shutdown(Shutdown::Both);
            return Err(error);
        }
    };
    for socket in [&client, &server] {
        socket.set_nodelay(true)?;
        socket.set_read_timeout(Some(TCP_READ_POLL))?;
    }
    {
        let mut sockets = lock(&inner.sockets);
        sockets.push(client.try_clone()?);
        sockets.push(server.try_clone()?);
    }
    let (up, down) = inner.one_way;
    let directions = [
        (client.try_clone()?, server.try_clone()?, up, true),
        (server, client, down, false),
    ];
    let mut pumps = lock(&inner.pumps);
    for (from, to, delay, upward) in directions {
        let inner = Arc::clone(inner);
        pumps.push(
            thread::Builder::new()
                .name("tcp-count-pump".into())
                .spawn(move || tcp_pump(from, to, delay, upward, inner))?,
        );
    }
    Ok(())
}

/// One direction: reads, and writes on at once or, with a delay, from a bounded
/// delay line. A byte is counted when it is written on to the far side, not
/// when it is read: at an injected RTT the count then moves with what the far
/// side has received, in step with the probe's payload window, instead of
/// leading it by RTT/2 (PX-027 round 2, F5). An end of file is passed on as a
/// half close, so `ssh` and sshd see each other's FIN.
fn tcp_pump(
    mut from: TcpStream,
    to: TcpStream,
    delay: Duration,
    upward: bool,
    inner: Arc<TcpInner>,
) {
    let count = move |inner: &TcpInner, bytes: usize| {
        let counter = if upward { &inner.up } else { &inner.down };
        counter.fetch_add(bytes as u64, Ordering::SeqCst);
    };
    let (line, writer) = if delay.is_zero() {
        (None, None)
    } else {
        let (send, receive) = mpsc::sync_channel::<(Instant, Vec<u8>)>(TCP_DELAY_CHUNKS);
        let mut out = match to.try_clone() {
            Ok(out) => out,
            Err(_) => return,
        };
        let counting = Arc::clone(&inner);
        let writer = thread::spawn(move || {
            for (due, chunk) in receive {
                let now = Instant::now();
                if due > now {
                    thread::sleep(due - now);
                }
                if out.write_all(&chunk).is_err() {
                    break;
                }
                count(&counting, chunk.len());
            }
            let _ = out.shutdown(Shutdown::Write);
        });
        (Some(send), Some(writer))
    };
    let mut direct = to;
    let mut chunk = vec![0_u8; TCP_CHUNK];
    loop {
        match from.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => {
                let sent = match &line {
                    Some(line) => line
                        .send((Instant::now() + delay, chunk[..read].to_vec()))
                        .is_ok(),
                    None => {
                        let written = direct.write_all(&chunk[..read]).is_ok();
                        if written {
                            count(&inner, read);
                        }
                        written
                    }
                };
                if !sent {
                    break;
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) =>
            {
                if inner.stopping.load(Ordering::SeqCst) {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    match (line, writer) {
        (Some(line), Some(writer)) => {
            drop(line);
            let _ = writer.join();
        }
        _ => {
            let _ = direct.shutdown(Shutdown::Write);
        }
    }
}

/// Payload and encrypted bytes of one direction over one window, kept apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Split {
    pub payload: u64,
    pub encrypted: u64,
}

impl Split {
    /// Refuses a split whose payload exceeds what the encrypted stream
    /// carried: a payload byte crosses the TCP stream inside SSH's framing, so
    /// it can never be more than the encrypted count, and a split that says
    /// so swapped or double counted something.
    pub fn new(payload: u64, encrypted: u64) -> Result<Self> {
        if payload > encrypted {
            return Err(format!(
                "payload {payload} B exceeds the encrypted stream's {encrypted} B: the split is swapped or double counted"
            ));
        }
        Ok(Self { payload, encrypted })
    }

    /// SSH's own bytes: framing, padding, MAC, key exchange, channel control.
    pub fn overhead(&self) -> u64 {
        self.encrypted - self.payload
    }
}

/// How a probe reaches the session: `ssh` (or any command) whose standard
/// streams are the subsystem channel, or srtop's Unix socket directly.
#[derive(Debug, Clone)]
pub enum Transport {
    Command(Vec<String>),
    Unix(PathBuf),
}

/// One message a probe received, when, its framed size and its class.
#[derive(Debug, Clone)]
pub struct ProbeEvent {
    pub at: Instant,
    pub bytes: u64,
    pub kind: &'static str,
    /// The `Ui` class: a `Transaction`, the one message that changes a replica
    /// (§12.1). The runtime classes it, not this probe
    /// (`logical_class_for_server_envelope`, as PX-008's release gate does).
    pub ui: bool,
    pub control: bool,
    pub revisions: Option<(u64, u64)>,
    pub operations: usize,
}

#[derive(Default)]
struct ProbeLog {
    raw: u64,
    framed: u64,
    events: Vec<ProbeEvent>,
    synced: Option<(Instant, u64)>,
    rows: u64,
    sampling_status: Option<String>,
    error: Option<String>,
    closed: bool,
}

fn kind(message: &SruiMessage) -> &'static str {
    match &message.msg {
        Some(Msg::ServerWelcome(_)) => "ServerWelcome",
        Some(Msg::ServerResumeOk(_)) => "ServerResumeOk",
        Some(Msg::ServerResyncRequired(_)) => "ServerResyncRequired",
        Some(Msg::ServerHandshakeRefused(_)) => "ServerHandshakeRefused",
        Some(Msg::ServerEventAck(_)) => "ServerEventAck",
        Some(Msg::Transaction(_)) => "Transaction",
        Some(Msg::ResourceMetadata(_)) => "ResourceMetadata",
        Some(Msg::ResourceChunk(_)) => "ResourceChunk",
        Some(Msg::TerminalData(_)) => "TerminalData",
        Some(Msg::TerminalResyncRequired(_)) => "TerminalResyncRequired",
        Some(_) => "client-originated message",
        None => "empty envelope",
    }
}

fn string_of(store: &SemanticStore, node: srui_semantic_tree::NodeId) -> Option<String> {
    match store
        .get_node(node)
        .and_then(|node| node.get_property(PropertyRef::TEXT))
    {
        Some(Value::String(text)) => Some(text.clone()),
        _ => None,
    }
}

fn read_session(mut reader: Box<dyn Read + Send>, log: Arc<Mutex<ProbeLog>>) {
    let mut store = SemanticStore::new();
    let mut buffer: Vec<u8> = Vec::new();
    let mut chunk = vec![0_u8; 64 * 1024];
    let mut welcomed = false;
    let fail = |log: &Mutex<ProbeLog>, error: String| {
        let mut log = lock(log);
        log.error.get_or_insert(error);
        log.closed = true;
    };
    loop {
        let read = match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) =>
            {
                if lock(&log).closed {
                    return;
                }
                continue;
            }
            Err(error) => return fail(&log, format!("read: {error}")),
        };
        lock(&log).raw += read as u64;
        buffer.extend_from_slice(&chunk[..read]);
        loop {
            let extent = match frame_extent(&buffer, DEFAULT_MAX_FRAME_SIZE) {
                Ok(Some(extent)) => extent,
                Ok(None) => break,
                Err(error) => return fail(&log, error),
            };
            let frame: Vec<u8> = buffer.drain(..extent).collect();
            let message = match decode_framed::<SruiMessage>(&frame) {
                Ok(message) => message,
                Err(error) => return fail(&log, format!("decode: {error}")),
            };
            let at = Instant::now();
            let class = logical_class_for_server_envelope(&message);
            let mut revisions = None;
            let mut operations = 0;
            if let Some(Msg::ServerWelcome(_)) = &message.msg {
                welcomed = true;
            }
            if let Some(Msg::ServerHandshakeRefused(refused)) = &message.msg {
                return fail(
                    &log,
                    format!("the server refused the handshake: {}", refused.detail),
                );
            }
            if let Some(Msg::Transaction(transaction)) = &message.msg {
                let (base, new) = (transaction.base_revision, transaction.new_revision);
                revisions = Some((base, new));
                operations = transaction.operations.len();
                let applied = if base == 0 {
                    ResyncSnapshot::try_from(transaction.clone())
                        .map_err(|error| error.to_string())
                        .and_then(|snapshot| {
                            store
                                .replace_from_snapshot(&snapshot)
                                .map(|_| ())
                                .map_err(|error| error.to_string())
                        })
                } else {
                    store
                        .apply_delivered_transaction(transaction.clone())
                        .map(|_| ())
                        .map_err(|error| error.to_string())
                };
                if let Err(error) = applied {
                    return fail(
                        &log,
                        format!("commit {base} -> {new} did not apply: {error}"),
                    );
                }
            }
            let mut log = lock(&log);
            log.framed += extent as u64;
            if log.events.len() >= MAX_PROBE_EVENTS {
                log.error = Some(format!("more than {MAX_PROBE_EVENTS} messages in one run"));
                log.closed = true;
                return;
            }
            if let Some((0, revision)) = revisions {
                if welcomed && log.synced.is_none() {
                    log.synced = Some((at, revision));
                }
            }
            if revisions.is_some() {
                log.rows = store.get_model(MODEL).map_or(0, |model| model.item_count());
                log.sampling_status = string_of(&store, SAMPLING_STATUS);
            }
            log.events.push(ProbeEvent {
                at,
                bytes: extent as u64,
                kind: kind(&message),
                ui: class == Some(LogicalChannelClass::Ui),
                control: class == Some(LogicalChannelClass::Control),
                revisions,
                operations,
            });
        }
    }
    let mut log = lock(&log);
    log.closed = true;
    if !buffer.is_empty() {
        log.error.get_or_insert(format!(
            "{} bytes of an unfinished frame at the end",
            buffer.len()
        ));
    }
}

/// The interval srtop itself published, from its sampling status
/// ("Live · sampling every 1000 ms"), in milliseconds.
pub fn published_interval_ms(status: &str) -> Option<u64> {
    let rest = &status[status.find("every ")? + "every ".len()..];
    rest.split(' ').next()?.parse().ok()
}

/// Counts of one observation window of a session probe.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Window {
    pub payload_up: u64,
    pub payload_down: u64,
    pub messages: u64,
    pub ui_transactions: u64,
    pub control_messages: u64,
    pub other_messages: u64,
}

/// What one SRUI session probe measured.
#[derive(Debug, Clone)]
pub struct SruiRun {
    /// Start to the first snapshot applied (welcome, snapshot).
    pub attach: Duration,
    pub sync_revision: u64,
    /// Every byte before the steady window: handshake, snapshot, warm-up.
    pub startup: Window,
    /// The steady window, `duration` long, after `warmup`.
    pub steady: Window,
    /// Ui-class transactions from the snapshot to the end of the steady window:
    /// the app mutations an unchanged source must not produce (PX-008).
    pub ui_after_sync: u64,
    /// Every message after synchronization, by kind and size, in order.
    pub after_sync: Vec<(&'static str, u64)>,
    pub encrypted_startup: Option<Traffic>,
    pub encrypted_steady: Option<Traffic>,
    pub rows: u64,
    pub published_interval_ms: Option<u64>,
    pub steady_duration: Duration,
    /// The canary, when one was asked for: the first transaction after it was
    /// fired, how long after, its revisions and operation count.
    pub canary: Option<Canary>,
    /// Raw bytes read and the sum of the decoded frames: equal, or refused.
    pub raw_down: u64,
    pub framed_down: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Canary {
    pub after: Duration,
    pub revisions: (u64, u64),
    pub operations: usize,
}

/// Plan of one session probe.
pub struct SruiPlan<'a> {
    pub transport: Transport,
    pub warmup: Duration,
    pub duration: Duration,
    /// Sampled at the start of the steady window and at its end.
    pub encrypted: Option<&'a dyn Fn() -> Traffic>,
    /// Fired once after the steady window; the probe then waits for the first
    /// transaction it causes.
    pub canary: Option<&'a dyn Fn() -> Result<()>>,
    pub stderr: Stdio,
}

const ATTACH_DEADLINE: Duration = Duration::from_secs(60);
const CANARY_DEADLINE: Duration = Duration::from_secs(15);
const CLOSE_DEADLINE: Duration = Duration::from_secs(10);

struct Session {
    child: Option<Child>,
    /// The Unix transport's socket, so `close` can half-close it.
    socket: Option<UnixStream>,
    writer: Box<dyn Write + Send>,
    reader: Option<JoinHandle<()>>,
    log: Arc<Mutex<ProbeLog>>,
    up: u64,
}

impl Session {
    fn open(transport: &Transport, stderr: Stdio) -> Result<Self> {
        let log = Arc::new(Mutex::new(ProbeLog::default()));
        let mut socket = None;
        let (child, reader, writer): (Option<Child>, Box<dyn Read + Send>, Box<dyn Write + Send>) =
            match transport {
                Transport::Command(argv) => {
                    let (program, args) = argv.split_first().ok_or("an empty command")?;
                    let mut child = Command::new(program)
                        .args(args)
                        .stdin(Stdio::piped())
                        .stdout(Stdio::piped())
                        .stderr(stderr)
                        .spawn()
                        .map_err(|error| format!("start {program}: {error}"))?;
                    let stdout = child.stdout.take().ok_or("no stdout")?;
                    let stdin = child.stdin.take().ok_or("no stdin")?;
                    (Some(child), Box::new(stdout), Box::new(stdin))
                }
                Transport::Unix(path) => {
                    let stream = UnixStream::connect(path)
                        .map_err(|error| format!("connect {}: {error}", path.display()))?;
                    stream
                        .set_read_timeout(Some(crate::relay::READ_POLL))
                        .map_err(|error| error.to_string())?;
                    let writer = stream.try_clone().map_err(|error| error.to_string())?;
                    socket = Some(stream.try_clone().map_err(|error| error.to_string())?);
                    (None, Box::new(stream), Box::new(writer))
                }
            };
        let reading = Arc::clone(&log);
        let reader = thread::spawn(move || read_session(reader, reading));
        Ok(Self {
            child,
            socket,
            writer,
            reader: Some(reader),
            log,
            up: 0,
        })
    }

    fn send(&mut self, message: &SruiMessage) -> Result<()> {
        let bytes = encode_framed(message).map_err(|error| error.to_string())?;
        self.writer
            .write_all(&bytes)
            .and_then(|()| self.writer.flush())
            .map_err(|error| format!("send: {error}"))?;
        self.up += bytes.len() as u64;
        Ok(())
    }

    fn wait_for(
        &self,
        what: &str,
        deadline: Duration,
        done: impl Fn(&ProbeLog) -> bool,
    ) -> Result<()> {
        let until = Instant::now() + deadline;
        loop {
            {
                let log = lock(&self.log);
                if done(&log) {
                    return Ok(());
                }
                if let Some(error) = &log.error {
                    return Err(format!("{what}: {error}"));
                }
                if log.closed {
                    return Err(format!("{what}: the session ended first"));
                }
            }
            if Instant::now() >= until {
                return Err(format!("timed out waiting for {what}"));
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    /// Ends the session: end of file on its input (ssh closes the channel and
    /// the bridge exits), then the child is waited for, killed only if it
    /// outlives the deadline. The child is this probe's own.
    fn close(mut self) -> Result<(u64, u64)> {
        drop(std::mem::replace(&mut self.writer, Box::new(io::sink())));
        if let Some(socket) = self.socket.take() {
            // The server sees the end of its input, ends the connection and
            // closes its side: the reader then reads to that end of file.
            let _ = socket.shutdown(std::net::Shutdown::Write);
        }
        if let Some(mut child) = self.child.take() {
            let until = Instant::now() + CLOSE_DEADLINE;
            loop {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) if Instant::now() < until => thread::sleep(Duration::from_millis(10)),
                    _ => {
                        let _ = child.kill();
                        let _ = child.wait();
                        break;
                    }
                }
            }
        }
        if let Some(reader) = self.reader.take() {
            let until = Instant::now() + CLOSE_DEADLINE;
            while !reader.is_finished() && Instant::now() < until {
                thread::sleep(Duration::from_millis(10));
            }
            // A reader of a socket the server never closed stops at its next timeout.
            lock(&self.log).closed = true;
            reader
                .join()
                .map_err(|_| "the reader panicked".to_string())?;
        }
        let log = lock(&self.log);
        Ok((log.raw, log.framed))
    }
}

fn window_of(events: &[ProbeEvent], from: Option<Instant>, to: Instant) -> Window {
    let mut window = Window::default();
    for event in events
        .iter()
        .filter(|event| from.is_none_or(|from| event.at >= from) && event.at < to)
    {
        window.messages += 1;
        window.payload_down += event.bytes;
        if event.ui {
            window.ui_transactions += 1;
        } else if event.control {
            window.control_messages += 1;
        } else {
            window.other_messages += 1;
        }
    }
    window
}

fn difference(after: Traffic, before: Traffic) -> Traffic {
    Traffic {
        up: after.up.saturating_sub(before.up),
        down: after.down.saturating_sub(before.down),
    }
}

/// A minimal replica session: `CLIENT_HELLO`, the welcome and snapshot, then
/// every commit applied as it arrives, for `warmup` and then `duration`.
pub struct SruiProbe;

impl SruiProbe {
    pub fn run(mut plan: SruiPlan<'_>, instance: &[u8]) -> Result<SruiRun> {
        let start = Instant::now();
        let stderr = std::mem::replace(&mut plan.stderr, Stdio::null());
        let mut session = Session::open(&plan.transport, stderr)?;
        let outcome = Self::observe(&mut session, &plan, instance, start);
        let closed = session.close();
        let mut run = outcome?;
        // The totals once nothing is in flight: the session has ended.
        let (raw, framed) = closed?;
        run.raw_down = raw;
        run.framed_down = framed;
        Ok(run)
    }

    fn observe(
        session: &mut Session,
        plan: &SruiPlan<'_>,
        instance: &[u8],
        start: Instant,
    ) -> Result<SruiRun> {
        let encrypted = |what: Option<&dyn Fn() -> Traffic>| what.map(|sample| sample());
        session.send(&SruiMessage {
            msg: Some(Msg::ClientHello(ClientHello {
                core_version: CORE_VERSION.to_string(),
                profiles: vec!["org.srui.standard-widgets/1".to_string()],
                client_instance_id: instance.to_vec(),
                ..ClientHello::default()
            })),
        })?;
        session.wait_for("the snapshot", ATTACH_DEADLINE, |log| log.synced.is_some())?;
        let (synced_at, sync_revision) = lock(&session.log).synced.expect("synchronized");
        thread::sleep(plan.warmup);
        let steady_from = Instant::now();
        let encrypted_from = encrypted(plan.encrypted);
        let up_from = session.up;
        thread::sleep(plan.duration);
        let steady_to = Instant::now();
        let encrypted_to = encrypted(plan.encrypted);
        let mut canary = None;
        if let Some(fire) = plan.canary {
            let fired = Instant::now();
            fire()?;
            session.wait_for("the canary's transaction", CANARY_DEADLINE, |log| {
                log.events.iter().any(|event| event.ui && event.at >= fired)
            })?;
            let log = lock(&session.log);
            let event = log
                .events
                .iter()
                .find(|event| event.ui && event.at >= fired)
                .expect("waited for");
            canary = Some(Canary {
                after: event.at - fired,
                revisions: event.revisions.unwrap_or_default(),
                operations: event.operations,
            });
        }
        let log = lock(&session.log);
        let mut startup = window_of(&log.events, None, steady_from);
        startup.payload_up = up_from;
        let mut steady = window_of(&log.events, Some(steady_from), steady_to);
        steady.payload_up = session.up - up_from;
        let after: Vec<&ProbeEvent> = log
            .events
            .iter()
            .filter(|event| event.at > synced_at && event.at < steady_to)
            .collect();
        let ui_after_sync = after.iter().filter(|event| event.ui).count() as u64;
        let status = log.sampling_status.clone();
        Ok(SruiRun {
            attach: synced_at - start,
            sync_revision,
            startup,
            steady,
            ui_after_sync,
            after_sync: after
                .iter()
                .map(|event| (event.kind, event.bytes))
                .collect(),
            encrypted_startup: encrypted_from,
            encrypted_steady: encrypted_from
                .zip(encrypted_to)
                .map(|(from, to)| difference(to, from)),
            rows: log.rows,
            published_interval_ms: status.as_deref().and_then(published_interval_ms),
            steady_duration: steady_to - steady_from,
            canary,
            raw_down: log.raw,
            framed_down: log.framed,
        })
    }
}

/// Refuses a run whose two independent payload counts disagree: the raw bytes
/// read and the sum of the decoded frames' extents (checked once the session
/// has closed, so no frame is in flight).
pub fn check_payload_counts(run: &SruiRun) -> Result<()> {
    if run.raw_down != run.framed_down {
        return Err(format!(
            "payload counted {} B raw but {} B in frames: a frame was double counted or lost",
            run.raw_down, run.framed_down
        ));
    }
    let windows = run.startup.payload_down + run.steady.payload_down;
    if windows > run.framed_down {
        return Err(format!(
            "the windows hold {windows} B of payload, more than the {} B received",
            run.framed_down
        ));
    }
    Ok(())
}

/// What one terminal (htop) probe measured.
#[derive(Debug, Clone)]
pub struct TtyRun {
    /// Start to the first byte of terminal output.
    pub attach: Duration,
    pub startup_down: u64,
    pub steady_down: u64,
    pub steady_up: u64,
    pub encrypted_startup: Option<Traffic>,
    pub encrypted_steady: Option<Traffic>,
    pub steady_duration: Duration,
    /// The first [`TTY_PREFIX`] bytes of the terminal stream: the session's own
    /// `stty size` answer comes first (PX-027 round 2, F8).
    pub prefix: Vec<u8>,
}

/// How much of the start of the terminal stream a [`TtyRun`] keeps.
pub const TTY_PREFIX: usize = 256;

/// The pseudo-terminal's size as the remote session itself reported it with
/// `stty size` ("ROWS COLUMNS" on the stream's first line): `(columns, lines)`.
pub fn tty_size_from_prefix(prefix: &[u8]) -> Option<(u16, u16)> {
    let text = std::str::from_utf8(&prefix[..prefix.len().min(TTY_PREFIX)]).ok()?;
    let first = text.split(['\r', '\n']).next()?;
    let mut words = first.split(' ');
    let lines = words.next()?.parse().ok()?;
    let columns = words.next()?.parse().ok()?;
    words.next().is_none().then_some((columns, lines))
}

/// htop (or any full-screen program) over `ssh -tt`: its terminal stream is
/// the payload. The first output byte stands where the SRUI probe's snapshot
/// does; the same warm-up and steady window follow, and `quit` is written at
/// the end (htop's `q`).
pub struct TtyProbe;

impl TtyProbe {
    pub fn run(
        argv: &[String],
        warmup: Duration,
        duration: Duration,
        quit: &[u8],
        encrypted: Option<&dyn Fn() -> Traffic>,
        stderr: Stdio,
    ) -> Result<TtyRun> {
        let (program, args) = argv.split_first().ok_or("an empty command")?;
        let start = Instant::now();
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(stderr)
            .spawn()
            .map_err(|error| format!("start {program}: {error}"))?;
        let mut stdout = child.stdout.take().ok_or("no stdout")?;
        let mut stdin = child.stdin.take().ok_or("no stdin")?;
        let arrivals: Arc<Mutex<Vec<(Instant, u64)>>> = Arc::new(Mutex::new(Vec::new()));
        let prefix: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
        let reading = Arc::clone(&arrivals);
        let keeping = Arc::clone(&prefix);
        let reader = thread::spawn(move || {
            let mut chunk = vec![0_u8; 64 * 1024];
            while let Ok(read) = stdout.read(&mut chunk) {
                if read == 0 {
                    break;
                }
                {
                    let mut prefix = lock(&keeping);
                    let room = TTY_PREFIX.saturating_sub(prefix.len()).min(read);
                    prefix.extend_from_slice(&chunk[..room]);
                }
                let mut arrivals = lock(&reading);
                if arrivals.len() < MAX_PROBE_EVENTS {
                    arrivals.push((Instant::now(), read as u64));
                }
            }
        });
        let first = {
            let until = Instant::now() + ATTACH_DEADLINE;
            loop {
                if let Some((at, _)) = lock(&arrivals).first() {
                    break Ok(*at);
                }
                if Instant::now() >= until || !matches!(child.try_wait(), Ok(None)) {
                    break Err("no terminal output".to_string());
                }
                thread::sleep(Duration::from_millis(5));
            }
        };
        let measured = first.map(|first| {
            thread::sleep(warmup);
            let steady_from = Instant::now();
            let encrypted_from = encrypted.map(|sample| sample());
            thread::sleep(duration);
            let steady_to = Instant::now();
            let encrypted_to = encrypted.map(|sample| sample());
            (first, steady_from, steady_to, encrypted_from, encrypted_to)
        });
        let _ = stdin.write_all(quit);
        let _ = stdin.flush();
        drop(stdin);
        let until = Instant::now() + CLOSE_DEADLINE;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < until => thread::sleep(Duration::from_millis(10)),
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break;
                }
            }
        }
        let _ = reader.join();
        let (first, steady_from, steady_to, encrypted_from, encrypted_to) = measured?;
        let kept = lock(&prefix).clone();
        let arrivals = lock(&arrivals);
        if arrivals.len() >= MAX_PROBE_EVENTS {
            return Err(format!("more than {MAX_PROBE_EVENTS} terminal reads"));
        }
        let sum = |from: Option<Instant>, to: Instant| {
            arrivals
                .iter()
                .filter(|(at, _)| from.is_none_or(|from| *at >= from) && *at < to)
                .map(|(_, bytes)| bytes)
                .sum::<u64>()
        };
        Ok(TtyRun {
            attach: first - start,
            startup_down: sum(None, steady_from),
            steady_down: sum(Some(steady_from), steady_to),
            steady_up: 0,
            encrypted_startup: encrypted_from,
            encrypted_steady: encrypted_from
                .zip(encrypted_to)
                .map(|(from, to)| difference(to, from)),
            steady_duration: steady_to - steady_from,
            prefix: kept,
        })
    }
}

/// min, median, p95 (nearest rank) and max of repeated runs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Distribution {
    pub n: usize,
    pub min: f64,
    pub median: f64,
    pub p95: f64,
    pub max: f64,
}

impl Distribution {
    pub fn of(values: &[f64]) -> Option<Self> {
        if values.is_empty() || values.iter().any(|value| !value.is_finite()) {
            return None;
        }
        let mut sorted = values.to_vec();
        sorted.sort_by(f64::total_cmp);
        let n = sorted.len();
        let median = if n % 2 == 1 {
            sorted[n / 2]
        } else {
            (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
        };
        // Nearest rank: the smallest value with at least 95% of the runs at or below it.
        let rank = ((0.95 * n as f64).ceil() as usize).clamp(1, n);
        Some(Self {
            n,
            min: sorted[0],
            median,
            p95: sorted[rank - 1],
            max: sorted[n - 1],
        })
    }

    /// `min / median / p95 / max`, to `digits` decimals.
    pub fn cells(&self, digits: usize) -> String {
        format!(
            "{:.d$} / {:.d$} / {:.d$} / {:.d$}",
            self.min,
            self.median,
            self.p95,
            self.max,
            d = digits
        )
    }
}
