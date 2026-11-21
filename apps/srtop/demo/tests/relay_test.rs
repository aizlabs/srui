//! PX-025 acceptance for the RTT injector: RTT/2 in each direction, in order
//! and without accumulating, byte counts in both directions, a cut that ends
//! every connection while the relay keeps accepting, and the `srtop-demo relay`
//! command that the native client's tests put in front of srtop.

use srtop_demo::relay::{DelayRelay, Traffic, MAX_RTT};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc,
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
    let mut reader = client.try_clone().unwrap();
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
    let mut echoed = Vec::new();
    while echoed.len() < sends.len() {
        let mut byte = [0_u8; 1];
        reader.read_exact(&mut byte).unwrap();
        echoed.push((byte[0], Instant::now()));
    }
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
