//! A userspace RTT injector (PX-025; D1 §31.4).
//!
//! [`DelayRelay`] listens on a Unix socket and joins every client it accepts
//! to srtop's own socket (or to anything else that speaks the same stream).
//! Each direction is a delay line: a chunk read at instant *t* is written on
//! at *t* + RTT/2, in order, so a request and its answer together cross the
//! relay one RTT later than they would without it, and a server push
//! arrives RTT/2 later. Nothing waits for a chunk to be written before the
//! next one is read, so the delay never accumulates.
//!
//! Each direction holds at most [`DEFAULT_IN_FLIGHT`] bytes (or the budget
//! given to [`DelayRelay::start_with_budget`]) read but not yet written on.
//! When that budget is full its reader stops reading, so the sender's socket
//! fills and the sender blocks, exactly as it would on a socket to the far
//! side directly: a client that stops reading still pushes back on srtop.
//!
//! It needs no privilege: no `tc`, `dnctl`, `pfctl` or root. In the SSH path
//! it stands between the SSH bridge and srtop (the bridge's subsystem socket is
//! the relay's), so every byte the client and srtop exchange crosses it.
//!
//! [`DelayRelay::cut`] shuts every live connection down at once, the way a
//! lost network does, and the relay keeps accepting, so a client can
//! reconnect through it: that is the transport interruption scenario.

use std::{
    io::{self, Read, Write},
    net::Shutdown,
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc, Arc, Condvar, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

/// The largest RTT a relay injects.
pub const MAX_RTT: Duration = Duration::from_secs(10);

/// The bytes one direction of one connection may hold in its delay line by
/// default. It bounds what a stalled reader can make the relay buffer, while
/// leaving the scenarios unthrottled: a delay line must hold what its sender
/// sends during RTT/2 (the bandwidth-delay product), and 4 MiB sustains
/// about 13 MiB/s at 600 ms RTT (300 ms one way) and 0.8 MiB/s at the
/// 10 s maximum, against srtop's largest burst here of about 20 KiB (a
/// 60-row snapshot). It is twice SSH's 2 MiB default channel window, the
/// next hop's own in-flight bound.
pub const DEFAULT_IN_FLIGHT: usize = 4 << 20;

/// The largest in-flight budget a relay accepts.
pub const MAX_IN_FLIGHT: usize = 64 << 20;

/// The largest single read: also the largest chunk a delay line holds.
const READ_CHUNK: usize = 64 * 1024;

/// Byte counts of everything a relay carried, both directions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Traffic {
    /// Client to server.
    pub up: u64,
    /// Server to client.
    pub down: u64,
}

struct Inner {
    upstream: PathBuf,
    /// The in-flight budget of each direction of each connection.
    budget: usize,
    /// Bytes read but not yet written on, over every connection, each way.
    in_flight_up: AtomicU64,
    in_flight_down: AtomicU64,
    /// The most any one direction of one connection held at once, each way.
    peak_up: AtomicU64,
    peak_down: AtomicU64,
    up_delay: Duration,
    down_delay: Duration,
    stopping: AtomicBool,
    up: AtomicU64,
    down: AtomicU64,
    accepted: AtomicU64,
    /// Delay-line threads (a reader and a writer per direction) still running.
    pumps: AtomicU64,
    /// Both sockets of every connection not yet cut or closed, by number.
    live: Mutex<Vec<(u64, UnixStream, UnixStream)>>,
}

/// One running relay; stopped and its socket removed on drop.
pub struct DelayRelay {
    inner: Arc<Inner>,
    listen: PathBuf,
    rtt: Duration,
    accept: Option<thread::JoinHandle<()>>,
}

impl DelayRelay {
    /// Listens on `listen` (which must not exist) and joins each client to
    /// `upstream`, adding `rtt / 2` in each direction.
    pub fn start(listen: &Path, upstream: &Path, rtt: Duration) -> io::Result<Self> {
        Self::start_with_budget(listen, upstream, rtt, DEFAULT_IN_FLIGHT)
    }

    /// As [`Self::start`], holding at most `budget` bytes (1 to
    /// [`MAX_IN_FLIGHT`]) in each direction of each connection.
    pub fn start_with_budget(
        listen: &Path,
        upstream: &Path,
        rtt: Duration,
        budget: usize,
    ) -> io::Result<Self> {
        if budget == 0 || budget > MAX_IN_FLIGHT {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("an in-flight budget of 1..={MAX_IN_FLIGHT} bytes, not {budget}"),
            ));
        }
        if rtt > MAX_RTT {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("an RTT of at most {MAX_RTT:?} is injected, not {rtt:?}"),
            ));
        }
        let listener = UnixListener::bind(listen)?;
        // Owner-only, like srtop's own socket: the SSH bridge refuses one that
        // group or others could reach.
        if let Err(error) = std::fs::set_permissions(listen, std::fs::Permissions::from_mode(0o600))
        {
            let _ = std::fs::remove_file(listen);
            return Err(error);
        }
        let one_way = rtt / 2;
        let inner = Arc::new(Inner {
            upstream: upstream.to_path_buf(),
            budget,
            in_flight_up: AtomicU64::new(0),
            in_flight_down: AtomicU64::new(0),
            peak_up: AtomicU64::new(0),
            peak_down: AtomicU64::new(0),
            up_delay: one_way,
            down_delay: rtt - one_way,
            stopping: AtomicBool::new(false),
            up: AtomicU64::new(0),
            down: AtomicU64::new(0),
            accepted: AtomicU64::new(0),
            pumps: AtomicU64::new(0),
            live: Mutex::new(Vec::new()),
        });
        let accept = {
            let inner = Arc::clone(&inner);
            thread::Builder::new()
                .name("relay-accept".into())
                .spawn(move || accept_loop(listener, inner))?
        };
        Ok(Self {
            inner,
            listen: listen.to_path_buf(),
            rtt,
            accept: Some(accept),
        })
    }

    pub fn rtt(&self) -> Duration {
        self.rtt
    }

    /// The delay each direction adds: client to server, server to client.
    pub fn one_way(&self) -> (Duration, Duration) {
        (self.inner.up_delay, self.inner.down_delay)
    }

    pub fn listen_path(&self) -> &Path {
        &self.listen
    }

    /// Everything carried so far.
    pub fn traffic(&self) -> Traffic {
        Traffic {
            up: self.inner.up.load(Ordering::SeqCst),
            down: self.inner.down.load(Ordering::SeqCst),
        }
    }

    /// Bytes read but not yet written on, each way, over every connection.
    pub fn in_flight(&self) -> Traffic {
        Traffic {
            up: self.inner.in_flight_up.load(Ordering::SeqCst),
            down: self.inner.in_flight_down.load(Ordering::SeqCst),
        }
    }

    /// The most one direction of one connection has held at once.
    pub fn peak_in_flight(&self) -> Traffic {
        Traffic {
            up: self.inner.peak_up.load(Ordering::SeqCst),
            down: self.inner.peak_down.load(Ordering::SeqCst),
        }
    }

    /// The in-flight budget of each direction of each connection.
    pub fn budget(&self) -> usize {
        self.inner.budget
    }

    /// Connections not yet cut or closed.
    pub fn live_connections(&self) -> usize {
        self.inner.live.lock().unwrap().len()
    }

    /// Delay-line threads still running: four per live connection.
    pub fn pump_threads(&self) -> u64 {
        self.inner.pumps.load(Ordering::SeqCst)
    }

    /// How many connections were accepted.
    pub fn accepted(&self) -> u64 {
        self.inner.accepted.load(Ordering::SeqCst)
    }

    /// Cuts every live connection, both sides, as a lost network does.
    /// Bytes still in a delay line are lost with it. Returns how many were
    /// cut; the relay keeps accepting.
    pub fn cut(&self) -> usize {
        let live = std::mem::take(&mut *self.inner.live.lock().unwrap());
        for (_, client, server) in &live {
            let _ = client.shutdown(Shutdown::Both);
            let _ = server.shutdown(Shutdown::Both);
        }
        live.len()
    }

    /// Stops accepting, cuts every connection and removes the socket.
    pub fn stop(&mut self) {
        if self.inner.stopping.swap(true, Ordering::SeqCst) {
            return;
        }
        // The accept loop polls `stopping`, so this returns even when the
        // socket's path has been removed underneath the relay.
        if let Some(accept) = self.accept.take() {
            let _ = accept.join();
        }
        self.cut();
        let _ = std::fs::remove_file(&self.listen);
    }
}

impl Drop for DelayRelay {
    fn drop(&mut self) {
        self.stop();
    }
}

/// How often an idle accept loop looks at `stopping`.
// lc-debt: an idle relay wakes every 2 ms (and a new client waits up to 2 ms to be accepted) so
// that stop() never depends on waking a blocked accept; upgrade path: poll(2) on the listener
// and a self-pipe, which needs FFI this std-only package avoids.
const ACCEPT_POLL: Duration = Duration::from_millis(2);

fn accept_loop(listener: UnixListener, inner: Arc<Inner>) {
    // Non-blocking, so a stop never depends on waking a blocked accept.
    if listener.set_nonblocking(true).is_err() {
        return;
    }
    loop {
        if inner.stopping.load(Ordering::SeqCst) {
            return;
        }
        let client = match listener.accept() {
            Ok((client, _)) => client,
            // Nothing waiting (or a transient failure): look again shortly.
            Err(_) => {
                thread::sleep(ACCEPT_POLL);
                continue;
            }
        };
        // An accepted socket may inherit the listener's non-blocking mode.
        if client.set_nonblocking(false).is_err() {
            continue;
        }
        let Ok(server) = UnixStream::connect(&inner.upstream) else {
            let _ = client.shutdown(Shutdown::Both);
            continue;
        };
        let id = inner.accepted.fetch_add(1, Ordering::SeqCst);
        let (Ok(client_copy), Ok(server_copy)) = (client.try_clone(), server.try_clone()) else {
            continue;
        };
        inner
            .live
            .lock()
            .unwrap()
            .push((id, client_copy, server_copy));
        // The connection stays registered, so `cut` and `stop` can reach both
        // of its sockets, until both directions have ended: a half-closed
        // connection still carries the other direction.
        let directions = Arc::new(AtomicUsize::new(2));
        let (Ok(client_up), Ok(server_up)) = (client.try_clone(), server.try_clone()) else {
            abandon(&inner, id, &client, &server);
            continue;
        };
        let joined = pump(
            client_up,
            server_up,
            inner.up_delay,
            Arc::clone(&inner),
            Direction::Up,
            id,
            Arc::clone(&directions),
        )
        .and_then(|()| {
            pump(
                server.try_clone()?,
                client.try_clone()?,
                inner.down_delay,
                Arc::clone(&inner),
                Direction::Down,
                id,
                directions,
            )
        });
        if joined.is_err() {
            // A thread could not be started: end what was started, which
            // sees its sockets shut, and forget the connection now, since a
            // direction that never started will never end it.
            abandon(&inner, id, &client, &server);
        }
    }
}

/// Shuts both sockets of connection `id` and forgets it.
fn abandon(inner: &Inner, id: u64, client: &UnixStream, server: &UnixStream) {
    let _ = client.shutdown(Shutdown::Both);
    let _ = server.shutdown(Shutdown::Both);
    inner.live.lock().unwrap().retain(|entry| entry.0 != id);
}

#[derive(Clone, Copy)]
enum Direction {
    Up,
    Down,
}

/// One direction's in-flight byte budget: the reader admits a chunk only
/// while it fits, the writer releases it once written, and a writer that has
/// ended closes it so a waiting reader gives up.
struct Budget {
    state: Mutex<(usize, bool)>,
    room: Condvar,
    limit: usize,
}

impl Budget {
    fn new(limit: usize) -> Self {
        Self {
            state: Mutex::new((0, false)),
            room: Condvar::new(),
            limit,
        }
    }

    /// Waits until `bytes` more fit; returns the bytes then held, or `None`
    /// once the writer has ended.
    fn admit(&self, bytes: usize) -> Option<usize> {
        let mut state = self.state.lock().unwrap();
        while !state.1 && state.0 + bytes > self.limit {
            state = self.room.wait(state).unwrap();
        }
        if state.1 {
            return None;
        }
        state.0 += bytes;
        Some(state.0)
    }

    fn release(&self, bytes: usize) {
        let mut state = self.state.lock().unwrap();
        state.0 -= bytes;
        self.room.notify_all();
    }

    fn close(&self) {
        let mut state = self.state.lock().unwrap();
        state.1 = true;
        self.room.notify_all();
    }
}

/// Counts one delay-line thread for as long as it runs; dropped when the
/// thread's closure ends (or is dropped unspawned).
struct PumpThread(Arc<Inner>);

impl PumpThread {
    fn start(inner: &Arc<Inner>) -> Self {
        inner.pumps.fetch_add(1, Ordering::SeqCst);
        Self(Arc::clone(inner))
    }
}

impl Drop for PumpThread {
    fn drop(&mut self) {
        self.0.pumps.fetch_sub(1, Ordering::SeqCst);
    }
}

/// One direction's delay line: a reader that timestamps each chunk and a
/// writer that releases it `delay` after it was read.
fn pump(
    mut from: UnixStream,
    mut to: UnixStream,
    delay: Duration,
    inner: Arc<Inner>,
    direction: Direction,
    id: u64,
    // Directions of this connection still running; the last to end forgets it.
    directions: Arc<AtomicUsize>,
) -> io::Result<()> {
    // The channel itself is unbounded; what it may hold is bounded by `budget`.
    let (line, queue) = mpsc::channel::<Option<(Instant, Vec<u8>)>>();
    let budget = Arc::new(Budget::new(inner.budget));
    let writer = {
        let running = PumpThread::start(&inner);
        let from = from.try_clone()?;
        let budget = Arc::clone(&budget);
        let inner = Arc::clone(&inner);
        thread::Builder::new()
            .name("relay-write".into())
            .spawn(move || {
                let _running = running;
                while let Ok(Some((due, chunk))) = queue.recv() {
                    let now = Instant::now();
                    if due > now {
                        thread::sleep(due - now);
                    }
                    let written = to.write_all(&chunk);
                    budget.release(chunk.len());
                    let (counter, in_flight) = match direction {
                        Direction::Up => (&inner.up, &inner.in_flight_up),
                        Direction::Down => (&inner.down, &inner.in_flight_down),
                    };
                    in_flight.fetch_sub(chunk.len() as u64, Ordering::SeqCst);
                    if written.is_err() {
                        let _ = from.shutdown(Shutdown::Both);
                        let _ = to.shutdown(Shutdown::Both);
                        break;
                    }
                    counter.fetch_add(chunk.len() as u64, Ordering::SeqCst);
                }
                // A reader waiting for room must not wait for a writer that ended.
                budget.close();
                // What the line still held is dropped with it.
                while let Ok(Some((_, chunk))) = queue.try_recv() {
                    let in_flight = match direction {
                        Direction::Up => &inner.in_flight_up,
                        Direction::Down => &inner.in_flight_down,
                    };
                    in_flight.fetch_sub(chunk.len() as u64, Ordering::SeqCst);
                }
                // The reader saw end of file (or the far side failed): pass it
                // on. Only this direction's write half: the other direction of
                // a half-closed connection goes on.
                let _ = to.shutdown(Shutdown::Write);
                // Both directions are done, so the connection is: forget it.
                if directions.fetch_sub(1, Ordering::SeqCst) == 1 {
                    inner.live.lock().unwrap().retain(|entry| entry.0 != id);
                }
            })?
    };
    thread::Builder::new().name("relay-read".into()).spawn({
        let running = PumpThread::start(&inner);
        move || {
            let _running = running;
            let mut buffer = vec![0_u8; READ_CHUNK.min(inner.budget)];
            let (in_flight, peak) = match direction {
                Direction::Up => (&inner.in_flight_up, &inner.peak_up),
                Direction::Down => (&inner.in_flight_down, &inner.peak_down),
            };
            loop {
                match from.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => {
                        // Stop reading while the line is full: the sender's
                        // socket then fills and the sender blocks.
                        let Some(held) = budget.admit(read) else {
                            break;
                        };
                        peak.fetch_max(held as u64, Ordering::SeqCst);
                        in_flight.fetch_add(read as u64, Ordering::SeqCst);
                        // Timed from when the chunk entered the line.
                        let due = Instant::now() + delay;
                        if line.send(Some((due, buffer[..read].to_vec()))).is_err() {
                            in_flight.fetch_sub(read as u64, Ordering::SeqCst);
                            break;
                        }
                    }
                }
            }
            let _ = line.send(None);
            drop(writer);
        }
    })?;
    Ok(())
}
