//! A userspace RTT injector (PX-025; D1 §31.4).
//!
//! [`DelayRelay`] listens on a Unix socket and joins every client it accepts
//! to srtop's own socket (or to anything else that speaks the same stream).
//! Each direction is a delay line: a chunk read at instant *t* is written on
//! at *t* + RTT/2, in order, so a request and its answer together cross the
//! relay one RTT later than they would without it, and a server push
//! arrives RTT/2 later. Nothing waits for a chunk to be written before the
//! next one is read, so the delay never accumulates and never throttles.
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
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

/// The largest RTT a relay injects.
pub const MAX_RTT: Duration = Duration::from_secs(10);

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
    up_delay: Duration,
    down_delay: Duration,
    stopping: AtomicBool,
    up: AtomicU64,
    down: AtomicU64,
    accepted: AtomicU64,
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
            up_delay: one_way,
            down_delay: rtt - one_way,
            stopping: AtomicBool::new(false),
            up: AtomicU64::new(0),
            down: AtomicU64::new(0),
            accepted: AtomicU64::new(0),
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
        let joined = (|| -> io::Result<()> {
            pump(
                client.try_clone()?,
                server.try_clone()?,
                inner.up_delay,
                Arc::clone(&inner),
                Direction::Up,
                id,
            )?;
            pump(
                server,
                client,
                inner.down_delay,
                Arc::clone(&inner),
                Direction::Down,
                id,
            )
        })();
        if joined.is_err() {
            continue;
        }
    }
}

#[derive(Clone, Copy)]
enum Direction {
    Up,
    Down,
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
) -> io::Result<()> {
    let (line, queue) = mpsc::channel::<Option<(Instant, Vec<u8>)>>();
    let writer = {
        let from = from.try_clone()?;
        thread::Builder::new()
            .name("relay-write".into())
            .spawn(move || {
                while let Ok(Some((due, chunk))) = queue.recv() {
                    let now = Instant::now();
                    if due > now {
                        thread::sleep(due - now);
                    }
                    if to.write_all(&chunk).is_err() {
                        let _ = from.shutdown(Shutdown::Both);
                        let _ = to.shutdown(Shutdown::Both);
                        break;
                    }
                    let counter = match direction {
                        Direction::Up => &inner.up,
                        Direction::Down => &inner.down,
                    };
                    counter.fetch_add(chunk.len() as u64, Ordering::SeqCst);
                }
                // The reader saw end of file (or the far side failed): pass it on.
                let _ = to.shutdown(Shutdown::Write);
                // The server's side is done, so the connection is: forget it.
                if matches!(direction, Direction::Down) {
                    inner.live.lock().unwrap().retain(|entry| entry.0 != id);
                }
            })?
    };
    thread::Builder::new()
        .name("relay-read".into())
        .spawn(move || {
            let mut buffer = vec![0_u8; 64 * 1024];
            loop {
                match from.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => {
                        let due = Instant::now() + delay;
                        if line.send(Some((due, buffer[..read].to_vec()))).is_err() {
                            break;
                        }
                    }
                }
            }
            let _ = line.send(None);
            drop(writer);
        })?;
    Ok(())
}
