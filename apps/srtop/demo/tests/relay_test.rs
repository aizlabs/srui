//! PX-025 acceptance for the RTT injector: RTT/2 in each direction, in order
//! and without accumulating, byte counts in both directions, a cut that ends
//! every connection while the relay keeps accepting, and the `srtop-demo relay`
//! command that the native client's tests put in front of srtop.

use srtop_demo::relay::{DelayRelay, Traffic, DEFAULT_IN_FLIGHT, MAX_IN_FLIGHT, MAX_RTT};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::Shutdown,
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        mpsc, Arc,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = PathBuf::from(format!(
            "/tmp/srtop-px025-relay-{}-{nanos}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// An echo server that reports when each one-byte message arrived.
fn echo_server(socket: &std::path::Path) -> mpsc::Receiver<(u8, Instant)> {
    let listener = UnixListener::bind(socket).unwrap();
    let (arrived, arrivals) = mpsc::channel();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let arrived = arrived.clone();
            thread::spawn(move || {
                let mut byte = [0_u8; 1];
                while let Ok(1) = stream.read(&mut byte) {
                    let _ = arrived.send((byte[0], Instant::now()));
                    if stream.write_all(&byte).is_err() {
                        return;
                    }
                }
            });
        }
    });
    arrivals
}

fn connect(socket: &std::path::Path) -> UnixStream {
    let stream = UnixStream::connect(socket).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stream
}

/// Each direction adds RTT/2, at every RTT D1 §31.4 names.
#[test]
fn each_direction_is_delayed_by_half_the_rtt() {
    let scratch = Scratch::new();
    let upstream = scratch.path("echo.sock");
    let arrivals = echo_server(&upstream);
    for rtt_ms in [0_u64, 100, 300, 600] {
        let rtt = Duration::from_millis(rtt_ms);
        let relay =
            DelayRelay::start(&scratch.path(&format!("r{rtt_ms}.sock")), &upstream, rtt).unwrap();
        assert_eq!(relay.one_way(), (rtt / 2, rtt / 2));
        let mut client = connect(relay.listen_path());
        let mut ups = Vec::new();
        let mut downs = Vec::new();
        for trial in 0..3_u8 {
            let sent = Instant::now();
            client.write_all(&[trial]).unwrap();
            let (byte, at_server) = arrivals.recv_timeout(Duration::from_secs(5)).unwrap();
            assert_eq!(byte, trial);
            let mut echo = [0_u8; 1];
            client.read_exact(&mut echo).unwrap();
            let back = Instant::now();
            assert_eq!(echo[0], trial);
            ups.push(at_server - sent);
            downs.push(back - at_server);
        }
        for (up, down) in ups.iter().zip(&downs) {
            // A chunk is never released before its delay is over.
            assert!(*up >= rtt / 2, "rtt {rtt_ms}: up {up:?}");
            assert!(*down >= rtt / 2, "rtt {rtt_ms}: down {down:?}");
            // Nor held much past it.
            assert!(
                *up < rtt / 2 + Duration::from_millis(60),
                "rtt {rtt_ms}: up {up:?}"
            );
            assert!(
                *down < rtt / 2 + Duration::from_millis(60),
                "rtt {rtt_ms}: down {down:?}"
            );
        }
        println!("PX-025 relay evidence: rtt {rtt_ms} ms: client->server {ups:?}, server->client {downs:?}");
    }
}

/// The delay is a delay line, not a queue: messages sent 10 ms apart each
/// arrive one RTT after they were sent, in order.
#[test]
fn the_delay_neither_accumulates_nor_reorders() {
    let scratch = Scratch::new();
    let upstream = scratch.path("echo.sock");
    let _arrivals = echo_server(&upstream);
    let rtt = Duration::from_millis(200);
    let relay = DelayRelay::start(&scratch.path("r.sock"), &upstream, rtt).unwrap();
    let client = connect(relay.listen_path());
    // Echoes are read and stamped as they arrive, while the sends go on, so
    // each is timed against its own send and nothing else.
    let reader = {
        let mut reader = client.try_clone().unwrap();
        thread::spawn(move || {
            (0..20)
                .map(|_| {
                    let mut byte = [0_u8; 1];
                    reader.read_exact(&mut byte).unwrap();
                    (byte[0], Instant::now())
                })
                .collect::<Vec<_>>()
        })
    };
    let sends: Vec<Instant> = {
        let mut writer = client;
        (0..20_u8)
            .map(|index| {
                let at = Instant::now();
                writer.write_all(&[index]).unwrap();
                thread::sleep(Duration::from_millis(10));
                at
            })
            .collect()
    };
    let echoed = reader.join().unwrap();
    // Not a queue: the sends span longer than one RTT, so a delay that
    // accumulated would push the later echoes well past it.
    assert!(
        *sends.last().unwrap() - sends[0] >= Duration::from_millis(190),
        "the sends overlap the delay"
    );
    for (index, (byte, at)) in echoed.iter().enumerate() {
        assert_eq!(usize::from(*byte), index, "in order");
        let round_trip = *at - sends[index];
        assert!(round_trip >= rtt, "message {index}: {round_trip:?}");
        assert!(
            round_trip < rtt + Duration::from_millis(80),
            "message {index} waited behind the others: {round_trip:?}"
        );
    }
}

/// The relay counts what it carries, each direction on its own.
#[test]
fn traffic_is_counted_in_both_directions() {
    let scratch = Scratch::new();
    let upstream = scratch.path("echo.sock");
    let _arrivals = echo_server(&upstream);
    let relay = DelayRelay::start(&scratch.path("r.sock"), &upstream, Duration::ZERO).unwrap();
    let mut client = connect(relay.listen_path());
    client.write_all(&[7; 5]).unwrap();
    let mut echo = [0_u8; 5];
    client.read_exact(&mut echo).unwrap();
    assert_eq!(relay.traffic(), Traffic { up: 5, down: 5 });
    assert_eq!(relay.accepted(), 1);
}

/// A cut ends every live connection, both ways, and the relay accepts the
/// reconnect.
#[test]
fn a_cut_ends_every_connection_and_the_relay_accepts_again() {
    let scratch = Scratch::new();
    let upstream = scratch.path("echo.sock");
    let _arrivals = echo_server(&upstream);
    let relay = DelayRelay::start(
        &scratch.path("r.sock"),
        &upstream,
        Duration::from_millis(100),
    )
    .unwrap();
    let mut first = connect(relay.listen_path());
    let mut second = connect(relay.listen_path());
    for client in [&mut first, &mut second] {
        client.write_all(&[1]).unwrap();
        let mut echo = [0_u8; 1];
        client.read_exact(&mut echo).unwrap();
    }
    assert_eq!(relay.cut(), 2);
    for client in [&mut first, &mut second] {
        let mut byte = [0_u8; 1];
        assert!(
            matches!(client.read(&mut byte), Ok(0) | Err(_)),
            "a cut connection ends"
        );
    }
    let mut again = connect(relay.listen_path());
    again.write_all(&[2]).unwrap();
    let mut echo = [0_u8; 1];
    again.read_exact(&mut echo).unwrap();
    assert_eq!(echo, [2]);
    assert_eq!(relay.accepted(), 3);
}

#[test]
fn an_rtt_is_accepted_at_the_bound_and_refused_past_it() {
    let scratch = Scratch::new();
    let upstream = scratch.path("echo.sock");
    let _arrivals = echo_server(&upstream);
    let at = DelayRelay::start(&scratch.path("a.sock"), &upstream, MAX_RTT).unwrap();
    assert_eq!(at.rtt(), MAX_RTT);
    // Owner-only, as the SSH bridge requires of the socket it is pointed at.
    assert_eq!(
        fs::metadata(scratch.path("a.sock"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    drop(at);
    assert!(
        !scratch.path("a.sock").exists(),
        "a stopped relay removes its socket"
    );
    assert!(DelayRelay::start(
        &scratch.path("b.sock"),
        &upstream,
        MAX_RTT + Duration::from_millis(1)
    )
    .is_err());
    assert!(!scratch.path("b.sock").exists());
}

/// An upstream that writes `first` bytes to its one client as soon as it
/// connects, then `then` more once `go` is set, counting what its writes
/// have handed to the kernel.
struct Pusher {
    written: Arc<AtomicUsize>,
    first_done: Arc<AtomicBool>,
    all_done: Arc<AtomicBool>,
    go: Arc<AtomicBool>,
    /// The server's writing thread has returned: done, or its write failed.
    ended: Arc<AtomicBool>,
}

impl Pusher {
    fn start(socket: &std::path::Path, first: usize, then: usize) -> Self {
        let listener = UnixListener::bind(socket).unwrap();
        let pusher = Pusher {
            written: Arc::new(AtomicUsize::new(0)),
            first_done: Arc::new(AtomicBool::new(false)),
            all_done: Arc::new(AtomicBool::new(false)),
            go: Arc::new(AtomicBool::new(false)),
            ended: Arc::new(AtomicBool::new(false)),
        };
        let ended = Arc::clone(&pusher.ended);
        let (written, first_done, all_done, go) = (
            Arc::clone(&pusher.written),
            Arc::clone(&pusher.first_done),
            Arc::clone(&pusher.all_done),
            Arc::clone(&pusher.go),
        );
        thread::spawn(move || {
            struct Ended(Arc<AtomicBool>);
            impl Drop for Ended {
                fn drop(&mut self) {
                    self.0.store(true, Ordering::SeqCst);
                }
            }
            let _ended = Ended(ended);
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut push = |mut left: usize| {
                let chunk = [0x5a_u8; 16 * 1024];
                while left > 0 {
                    let size = left.min(chunk.len());
                    match stream.write(&chunk[..size]) {
                        Ok(0) | Err(_) => return false,
                        Ok(sent) => {
                            written.fetch_add(sent, Ordering::SeqCst);
                            left -= sent;
                        }
                    }
                }
                true
            };
            if !push(first) {
                return;
            }
            first_done.store(true, Ordering::SeqCst);
            while !go.load(Ordering::SeqCst) {
                thread::sleep(Duration::from_millis(5));
            }
            if push(then) {
                all_done.store(true, Ordering::SeqCst);
            }
        });
        pusher
    }
}

fn wait_for(what: &str, timeout: Duration, done: impl Fn() -> bool) {
    let deadline = Instant::now() + timeout;
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        thread::sleep(Duration::from_millis(5));
    }
}

/// The delay line holds its budget exactly: with every chunk held for its
/// delay, `budget` bytes from the server all enter it and the server's write
/// completes; one byte more does not enter it.
#[test]
fn the_delay_line_holds_exactly_its_budget_and_not_a_byte_more() {
    let scratch = Scratch::new();
    let upstream = scratch.path("push.sock");
    let budget = 256 * 1024;
    let pusher = Pusher::start(&upstream, budget, 1);
    // 3 s each way: nothing is released while this test looks.
    let relay = DelayRelay::start_with_budget(
        &scratch.path("r.sock"),
        &upstream,
        Duration::from_secs(6),
        budget,
    )
    .unwrap();
    assert_eq!(relay.budget(), budget);
    // A client that never reads.
    let _client = connect(relay.listen_path());
    wait_for("the server's first write", Duration::from_secs(2), || {
        pusher.first_done.load(Ordering::SeqCst)
    });
    // At the bound: all of it is in the line.
    wait_for("the budget to fill", Duration::from_secs(2), || {
        relay.in_flight().down == budget as u64
    });
    assert_eq!(relay.peak_in_flight().down, budget as u64);
    // Just past it: the next byte stays with the sender's socket.
    pusher.go.store(true, Ordering::SeqCst);
    wait_for("the server's last byte", Duration::from_secs(2), || {
        pusher.all_done.load(Ordering::SeqCst)
    });
    thread::sleep(Duration::from_millis(300));
    assert_eq!(relay.in_flight().down, budget as u64);
    assert_eq!(
        relay.peak_in_flight().down,
        budget as u64,
        "the line took a byte past its budget"
    );
    assert_eq!(
        relay.traffic().down,
        0,
        "nothing is released before its delay"
    );
}

/// Backpressure reaches the server: with a client that never reads and no
/// delay, the relay holds at most its budget and the server's writes stop,
/// as they would on a socket to that client directly.
#[test]
fn a_client_that_never_reads_blocks_the_server_at_the_budget() {
    let scratch = Scratch::new();
    let upstream = scratch.path("push.sock");
    let budget = 256 * 1024;
    let offered = 64 << 20;
    let pusher = Pusher::start(&upstream, offered, 0);
    let relay =
        DelayRelay::start_with_budget(&scratch.path("r.sock"), &upstream, Duration::ZERO, budget)
            .unwrap();
    let _client = connect(relay.listen_path());
    thread::sleep(Duration::from_millis(1_500));
    let written = pusher.written.load(Ordering::SeqCst);
    assert!(
        !pusher.first_done.load(Ordering::SeqCst),
        "the server wrote all {offered} bytes to a client that never reads"
    );
    // What the server got rid of: the relay's budget plus the kernel's
    // buffers on both sockets.
    assert!(
        written <= budget + (8 << 20),
        "{written} bytes left the server"
    );
    assert!(relay.peak_in_flight().down <= budget as u64);
    assert!(relay.in_flight().down <= budget as u64);
    println!(
        "PX-025 backpressure evidence: budget {budget}, server wrote {written} of {offered} bytes in 1.5 s, relay holding {} (peak {}), delivered {}",
        relay.in_flight().down,
        relay.peak_in_flight().down,
        relay.traffic().down
    );
}

#[test]
fn an_in_flight_budget_is_accepted_at_its_bounds_and_refused_past_them() {
    let scratch = Scratch::new();
    let upstream = scratch.path("echo.sock");
    let _arrivals = echo_server(&upstream);
    for (name, budget, accepted) in [
        ("a.sock", 1, true),
        ("b.sock", MAX_IN_FLIGHT, true),
        ("c.sock", 0, false),
        ("d.sock", MAX_IN_FLIGHT + 1, false),
    ] {
        let started =
            DelayRelay::start_with_budget(&scratch.path(name), &upstream, Duration::ZERO, budget);
        assert_eq!(started.is_ok(), accepted, "budget {budget}");
    }
    // A one-byte budget still carries everything, a byte at a time.
    let relay =
        DelayRelay::start_with_budget(&scratch.path("e.sock"), &upstream, Duration::ZERO, 1)
            .unwrap();
    let mut client = connect(relay.listen_path());
    client.write_all(&[1, 2, 3, 4]).unwrap();
    let mut echo = [0_u8; 4];
    client.read_exact(&mut echo).unwrap();
    assert_eq!(echo, [1, 2, 3, 4]);
    assert_eq!(relay.peak_in_flight().up, 1);
    assert_eq!(
        DelayRelay::start(&scratch.path("f.sock"), &upstream, Duration::ZERO)
            .unwrap()
            .budget(),
        DEFAULT_IN_FLIGHT
    );
}

/// Stopping a relay cuts its live connections: with a client that never
/// reads and a server blocked writing to it, `stop()` ends the server's write
/// (an error) and every delay-line thread within RTT/2 + 100 ms.
#[test]
fn stop_cuts_a_blocked_connection_and_ends_its_threads() {
    let scratch = Scratch::new();
    let upstream = scratch.path("push.sock");
    let pusher = Pusher::start(&upstream, 64 << 20, 0);
    let rtt = Duration::from_millis(100);
    let mut relay =
        DelayRelay::start_with_budget(&scratch.path("r.sock"), &upstream, rtt, 256 * 1024).unwrap();
    let _client = connect(relay.listen_path());
    // Blocked: the line is full and the server has stopped getting rid of bytes.
    wait_for("the line to fill", Duration::from_secs(2), || {
        relay.in_flight().down > 0 && relay.pump_threads() == 4
    });
    let mut last = pusher.written.load(Ordering::SeqCst);
    loop {
        thread::sleep(Duration::from_millis(100));
        let now = pusher.written.load(Ordering::SeqCst);
        if now == last {
            break;
        }
        last = now;
    }
    assert!(
        !pusher.ended.load(Ordering::SeqCst),
        "the server is still blocked"
    );
    assert_eq!(relay.live_connections(), 1);
    let stopped = Instant::now();
    relay.stop();
    let bound = rtt / 2 + Duration::from_millis(100);
    wait_for("the server's write to end", bound, || {
        pusher.ended.load(Ordering::SeqCst)
    });
    assert!(
        !pusher.first_done.load(Ordering::SeqCst),
        "the server's write ended by an error, not by completing"
    );
    wait_for("every delay-line thread to end", bound, || {
        relay.pump_threads() == 0
    });
    assert_eq!(relay.live_connections(), 0);
    println!(
        "PX-025 stop evidence: blocked server and 4 delay-line threads ended {:?} after stop()",
        stopped.elapsed()
    );
}

/// A relay at `rtt` joined to an upstream this test drives directly: the
/// client's socket and the upstream's accepted socket.
fn joined(scratch: &Scratch, rtt: Duration) -> (DelayRelay, UnixStream, UnixStream) {
    let upstream = scratch.path("u.sock");
    let listener = UnixListener::bind(&upstream).unwrap();
    let relay = DelayRelay::start(&scratch.path("r.sock"), &upstream, rtt).unwrap();
    let client = connect(relay.listen_path());
    let (server, _) = listener.accept().unwrap();
    server
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    wait_for(
        "the four delay-line threads",
        Duration::from_secs(2),
        || relay.pump_threads() == 4,
    );
    (relay, client, server)
}

/// Which end shuts its write half first.
#[derive(Clone, Copy, Debug)]
enum Closer {
    Server,
    Client,
}

/// Reads until end of file or an error, on its own thread; reports when.
fn read_to_end_on_a_thread(mut stream: UnixStream) -> mpsc::Receiver<(usize, Instant)> {
    let (done, ended) = mpsc::channel();
    thread::spawn(move || {
        let mut total = 0;
        let mut buffer = [0_u8; 8192];
        loop {
            match stream.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => total += read,
            }
        }
        let _ = done.send((total, Instant::now()));
    });
    ended
}

/// A half-closed connection stays the relay's until both directions end:
/// after one end shuts its write half (and keeps reading), `stop()` or
/// `cut()` still reaches the other direction, so the half-closed end's read
/// ends and every delay-line thread is gone within RTT/2 + 100 ms.
#[test]
fn a_half_closed_connection_is_still_cut_and_stopped() {
    let rtt = Duration::from_millis(100);
    let bound = rtt / 2 + Duration::from_millis(100);
    for closer in [Closer::Server, Closer::Client] {
        for by_stop in [true, false] {
            let scratch = Scratch::new();
            let (mut relay, client, server) = joined(&scratch, rtt);
            let (closing, mut other) = match closer {
                Closer::Server => (server, client),
                Closer::Client => (client, server),
            };
            other
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            closing.shutdown(Shutdown::Write).unwrap();
            // The FIN is forwarded: the other end reads end of file.
            let mut byte = [0_u8; 1];
            assert_eq!(
                other.read(&mut byte).unwrap(),
                0,
                "{closer:?}: EOF forwarded"
            );
            // That direction is done; the other one still runs.
            wait_for("one direction to end", Duration::from_secs(2), || {
                relay.pump_threads() == 2
            });
            assert_eq!(
                relay.live_connections(),
                1,
                "{closer:?}: a half-closed connection is still the relay's"
            );
            let ended = read_to_end_on_a_thread(closing.try_clone().unwrap());
            let started = Instant::now();
            if by_stop {
                relay.stop();
            } else {
                assert_eq!(
                    relay.cut(),
                    1,
                    "{closer:?}: cut finds the half-closed connection"
                );
            }
            let (_, at) = ended.recv_timeout(bound).unwrap_or_else(|_| {
                panic!("{closer:?}, stop {by_stop}: the half-closed end's read did not end")
            });
            assert!(at - started < bound);
            wait_for("every delay-line thread to end", bound, || {
                relay.pump_threads() == 0
            });
            assert_eq!(relay.live_connections(), 0);
            drop(other);
        }
    }
}

/// Half-close is forwarded and nothing more: after one end sends its last
/// bytes and FIN, the other end receives every byte and then end of file,
/// and can still send to the half-closed end, which receives everything; when
/// the other end closes too, the connection ends by itself.
#[test]
fn half_close_is_forwarded_in_each_direction_without_ending_the_other() {
    let rtt = Duration::from_millis(100);
    for closer in [Closer::Server, Closer::Client] {
        let scratch = Scratch::new();
        let (relay, client, server) = joined(&scratch, rtt);
        let (mut closing, other) = match closer {
            Closer::Server => (server, client),
            Closer::Client => (client, server),
        };
        let first: Vec<u8> = (0..100_000_u32).map(|index| (index % 251) as u8).collect();
        let reply: Vec<u8> = (0..70_000_u32).map(|index| (index % 241) as u8).collect();
        closing.write_all(&first).unwrap();
        closing.shutdown(Shutdown::Write).unwrap();
        let mut other_reader = other.try_clone().unwrap();
        other_reader
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut received = Vec::new();
        other_reader.read_to_end(&mut received).unwrap();
        assert_eq!(received, first, "{closer:?}: every byte, then end of file");
        // The other direction still flows.
        let mut other_writer = other;
        other_writer.write_all(&reply).unwrap();
        let mut back = vec![0_u8; reply.len()];
        closing
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        closing.read_exact(&mut back).unwrap();
        assert_eq!(
            back, reply,
            "{closer:?}: the half-closed end still receives"
        );
        assert_eq!(relay.live_connections(), 1);
        // The other end closes too: both directions end, and so does the connection.
        other_writer.shutdown(Shutdown::Write).unwrap();
        let mut byte = [0_u8; 1];
        assert_eq!(closing.read(&mut byte).unwrap(), 0);
        wait_for(
            "the connection to end by itself",
            rtt + Duration::from_secs(1),
            || relay.pump_threads() == 0 && relay.live_connections() == 0,
        );
    }
}

/// A relay whose socket path was removed underneath it still stops, and its
/// owner is not left waiting.
#[test]
fn a_relay_stops_even_when_its_socket_path_is_gone() {
    let scratch = Scratch::new();
    let upstream = scratch.path("echo.sock");
    let _arrivals = echo_server(&upstream);
    let relay = DelayRelay::start(&scratch.path("r.sock"), &upstream, Duration::ZERO).unwrap();
    fs::remove_file(scratch.path("r.sock")).unwrap();
    let (stopped, done) = mpsc::channel();
    thread::spawn(move || {
        drop(relay);
        let _ = stopped.send(());
    });
    assert!(
        done.recv_timeout(Duration::from_secs(2)).is_ok(),
        "the relay did not stop"
    );
}

/// The command the native client's tests run: `ready`, `cut`, `traffic`, and
/// it stops (removing its socket) when its standard input closes.
#[test]
fn the_relay_command_is_driven_through_its_standard_input() {
    let scratch = Scratch::new();
    let upstream = scratch.path("echo.sock");
    let _arrivals = echo_server(&upstream);
    let listen = scratch.path("r.sock");
    let mut relay = Command::new(env!("CARGO_BIN_EXE_srtop-demo"))
        .arg("relay")
        .arg("--listen")
        .arg(&listen)
        .arg("--upstream")
        .arg(&upstream)
        .args(["--rtt-ms", "100"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut control = relay.stdin.take().unwrap();
    let mut replies = BufReader::new(relay.stdout.take().unwrap()).lines();
    assert_eq!(
        replies.next().unwrap().unwrap(),
        "ready rtt_ms=100 up_ms=50 down_ms=50"
    );
    let mut client = connect(&listen);
    client.write_all(&[9, 9]).unwrap();
    let mut echo = [0_u8; 2];
    client.read_exact(&mut echo).unwrap();
    writeln!(control, "traffic").unwrap();
    assert_eq!(
        replies.next().unwrap().unwrap(),
        "traffic up=2 down=2 accepted=1"
    );
    writeln!(control, "cut").unwrap();
    assert_eq!(replies.next().unwrap().unwrap(), "cut 1");
    let mut byte = [0_u8; 1];
    assert!(matches!(client.read(&mut byte), Ok(0) | Err(_)));
    drop(control);
    let status = relay.wait().unwrap();
    assert!(status.success());
    assert!(!listen.exists(), "the relay removed its socket");
}
