//! PX-027: the showcase's measuring instruments, checked against independent
//! counts before they are trusted on the devbox.
//!
//! * The TCP relay counts exactly the bytes each side wrote, and its delay
//!   adds one RTT to a request and its answer.
//! * The session probe's payload count equals PX-025's delay relay's count of
//!   the same stream, byte for byte, and equals its own sum of frames.
//! * The idle check passes an unchanged fake source and refuses one that
//!   commits, and its canary is the next transaction.
//! * The comparison refuses mismatched intervals, windows, terminals or
//!   workloads, and a payload/encrypted split that is swapped or double
//!   counted.
//!
//! srtop is `$SRTOP_BIN` or the binary `apps/srtop` builds; a missing binary
//! fails these tests, nothing is skipped.

use srtop_demo::{
    relay::{DelayRelay, Traffic},
    showcase::{
        check_idle, check_matched, check_workload_fits, compare, htop_delay_from_args, htop_json,
        htop_record, idle_json, json_string, rtt, srui_json, srui_record, Devbox, HtopRecord,
        IdleReport, Settings, SruiRecord, Workload, WorkloadWatch,
    },
    traffic::{
        check_payload_counts, published_interval_ms, tty_size_from_prefix, Distribution, Split,
        SruiPlan, SruiProbe, SruiRun, TcpCountRelay, Transport, TtyProbe, TtyRun, Window,
    },
};
use std::{
    fs,
    io::{Read, Write},
    net::{Shutdown, TcpListener},
    os::unix::{fs::PermissionsExt, net::UnixStream},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

fn srtop_binary() -> PathBuf {
    let path = std::env::var_os("SRTOP_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/debug/srtop"));
    assert!(
        path.is_file(),
        "{} is missing: build srtop first (cargo build --locked --manifest-path apps/srtop/Cargo.toml) or set SRTOP_BIN",
        path.display()
    );
    path
}

/// One srtop in a private directory; killed, reaped and removed on drop.
struct Srtop {
    child: Child,
    directory: PathBuf,
}

impl Srtop {
    fn start(options: &[&str]) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = PathBuf::from(format!(
            "/tmp/srtop-px027-{}-{nanos}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&directory).unwrap();
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700)).unwrap();
        let socket = directory.join("s.sock");
        let child = Command::new(srtop_binary())
            .arg("--socket")
            .arg(&socket)
            .args(options)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut srtop = Self { child, directory };
        let deadline = Instant::now() + Duration::from_secs(120);
        while UnixStream::connect(&socket).is_err() {
            assert!(
                srtop.child.try_wait().unwrap().is_none(),
                "srtop exited before it was ready"
            );
            assert!(
                Instant::now() < deadline,
                "srtop's socket did not become ready"
            );
            thread::sleep(Duration::from_millis(10));
        }
        srtop
    }

    fn socket(&self) -> PathBuf {
        self.directory.join("s.sock")
    }

    fn path(&self, name: &str) -> PathBuf {
        self.directory.join(name)
    }

    /// SIGUSR1 to this test's own srtop, after `ps` confirms the PID is srtop.
    fn signal_usr1(&self) -> Result<(), String> {
        let pid = self.child.id().to_string();
        let comm = Command::new("ps")
            .args(["-p", &pid, "-o", "comm="])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .map_err(|error| error.to_string())?;
        let comm = String::from_utf8_lossy(&comm.stdout);
        if !comm.trim().ends_with("srtop") {
            return Err(format!("PID {pid} is {comm:?}, not srtop"));
        }
        let status = Command::new("kill")
            .args(["-USR1", &pid])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|error| error.to_string())?;
        status
            .success()
            .then_some(())
            .ok_or_else(|| format!("kill: {status}"))
    }
}

impl Drop for Srtop {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.directory);
    }
}

/// A TCP server that reads `expect` bytes, answers `answer` bytes and closes.
fn answering_server(
    expect: usize,
    answer: usize,
) -> (std::net::SocketAddr, thread::JoinHandle<usize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut got = vec![0_u8; expect];
        stream.read_exact(&mut got).unwrap();
        stream.write_all(&vec![7_u8; answer]).unwrap();
        stream.shutdown(Shutdown::Write).unwrap();
        let mut rest = Vec::new();
        stream.read_to_end(&mut rest).unwrap();
        expect + rest.len()
    });
    (address, server)
}

#[test]
fn the_tcp_relay_counts_every_byte_each_way_and_delays_by_the_rtt() {
    for rtt in [Duration::ZERO, Duration::from_millis(200)] {
        let (upstream, server) = answering_server(100_000, 250_000);
        let mut relay = TcpCountRelay::start(upstream, rtt).unwrap();
        let mut client = std::net::TcpStream::connect(relay.local_addr()).unwrap();
        let start = Instant::now();
        client.write_all(&vec![1_u8; 100_000]).unwrap();
        let mut first = [0_u8; 1];
        client.read_exact(&mut first).unwrap();
        let answered = start.elapsed();
        let mut rest = Vec::new();
        client.shutdown(Shutdown::Write).unwrap();
        client.read_to_end(&mut rest).unwrap();
        assert_eq!(
            rest.len() + 1,
            250_000,
            "rtt {rtt:?}: every answer byte arrived"
        );
        assert_eq!(server.join().unwrap(), 100_000);
        // The relay counts as it reads; both directions are complete here.
        let deadline = Instant::now() + Duration::from_secs(5);
        while relay.traffic()
            != (Traffic {
                up: 100_000,
                down: 250_000,
            })
            && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            relay.traffic(),
            Traffic {
                up: 100_000,
                down: 250_000
            },
            "rtt {rtt:?}"
        );
        assert_eq!(relay.accepted(), 1);
        assert!(answered >= rtt, "rtt {rtt:?}: answered after {answered:?}");
        if !rtt.is_zero() {
            assert!(
                answered < rtt * 3,
                "rtt {rtt:?}: answered after {answered:?}"
            );
        }
        relay.stop();
    }
}

#[test]
fn a_split_never_has_more_payload_than_encrypted_bytes() {
    let split = Split::new(700, 1000).unwrap();
    assert_eq!(split.overhead(), 300);
    assert_eq!(Split::new(0, 0).unwrap().overhead(), 0);
    let swapped = Split::new(1000, 700).unwrap_err();
    assert!(swapped.contains("swapped or double counted"), "{swapped}");
}

#[test]
fn distributions_are_min_median_nearest_rank_p95_and_max() {
    let dist = Distribution::of(&[5.0, 1.0, 3.0, 2.0, 4.0]).unwrap();
    assert_eq!(
        (dist.n, dist.min, dist.median, dist.p95, dist.max),
        (5, 1.0, 3.0, 5.0, 5.0)
    );
    let dist = Distribution::of(&(1..=20).map(f64::from).collect::<Vec<_>>()).unwrap();
    assert_eq!((dist.median, dist.p95, dist.max), (10.5, 19.0, 20.0));
    assert!(Distribution::of(&[]).is_none());
    assert!(Distribution::of(&[1.0, f64::NAN]).is_none());
    assert_eq!(
        Distribution::of(&[2.0]).unwrap().cells(1),
        "2.0 / 2.0 / 2.0 / 2.0"
    );
}

#[test]
fn the_intervals_each_side_reports_are_read_from_their_own_words() {
    assert_eq!(
        published_interval_ms("Live · sampling every 1000 ms"),
        Some(1000)
    );
    assert_eq!(
        published_interval_ms("Resuming · sampling now, then every 250 ms"),
        Some(250)
    );
    assert_eq!(published_interval_ms("Paused"), None);
    assert_eq!(htop_delay_from_args("htop -d 10"), Some(10));
    assert_eq!(htop_delay_from_args("/usr/bin/htop -d20"), Some(20));
    assert_eq!(htop_delay_from_args("htop"), None);
    assert_eq!(htop_delay_from_args("sh -c htop -d 10"), None);
}

/// The probe's payload count and PX-025's relay's count of the same stream,
/// through the relay at 0 ms, agree to the byte in both directions, and the
/// probe's raw count equals its sum of frames.
#[test]
fn the_probes_payload_count_equals_the_delay_relays_count_of_the_same_stream() {
    // The synthetic source churns on every tick for as long as it runs (the
    // scripted sequence ends after a few steps), so the window always holds
    // commits whenever the probe attaches.
    let srtop = Srtop::start(&["--scale-source", "50", "--scale-interval-ms", "100"]);
    let mut relay =
        DelayRelay::start(&srtop.path("r.sock"), &srtop.socket(), Duration::ZERO).unwrap();
    let run = SruiProbe::run(
        SruiPlan {
            transport: Transport::Unix(srtop.path("r.sock")),
            warmup: Duration::from_millis(200),
            duration: Duration::from_millis(1200),
            encrypted: None,
            canary: None,
            held: None,
            stderr: Stdio::null(),
        },
        b"px027-differential",
    )
    .unwrap();
    check_payload_counts(&run).unwrap();
    assert!(
        run.steady.ui_transactions >= 3,
        "the synthetic source commits every tick: {:?}",
        run.steady
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while relay.live_connections() > 0 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    let traffic = relay.traffic();
    assert_eq!(
        traffic.down, run.raw_down,
        "every byte srtop sent, counted once by each"
    );
    assert_eq!(traffic.up, run.startup.payload_up + run.steady.payload_up);
    assert!(run.startup.payload_down + run.steady.payload_down <= run.framed_down);
    assert_eq!(run.published_interval_ms, Some(100));
    assert_eq!(run.rows, 50);
    relay.stop();
    println!(
        "PX-027 payload differential: relay down {} B = probe raw {} B = probe framed {} B; up {} B",
        traffic.down, run.raw_down, run.framed_down, traffic.up
    );
}

/// A double-counted frame or a lost one is refused.
#[test]
fn a_probe_whose_two_payload_counts_disagree_is_refused() {
    let srtop = Srtop::start(&["--fake-source"]);
    let mut run = SruiProbe::run(
        SruiPlan {
            transport: Transport::Unix(srtop.socket()),
            warmup: Duration::ZERO,
            duration: Duration::from_millis(100),
            encrypted: None,
            canary: None,
            held: None,
            stderr: Stdio::null(),
        },
        b"px027-counts",
    )
    .unwrap();
    check_payload_counts(&run).unwrap();
    run.raw_down *= 2;
    let error = check_payload_counts(&run).unwrap_err();
    assert!(error.contains("double counted or lost"), "{error}");
}

fn idle_report(srtop: &Srtop, window: Duration) -> IdleReport {
    let canary = || srtop.signal_usr1();
    let run = SruiProbe::run(
        SruiPlan {
            transport: Transport::Unix(srtop.socket()),
            warmup: Duration::ZERO,
            duration: window,
            encrypted: None,
            canary: Some(&canary),
            held: None,
            stderr: Stdio::null(),
        },
        b"px027-idle",
    )
    .unwrap();
    check_payload_counts(&run).unwrap();
    let canary = run.canary.clone().expect("the canary was seen");
    IdleReport {
        window: run.steady_duration,
        sync_revision: run.sync_revision,
        ui_after_sync: run.ui_after_sync,
        after_sync: run.after_sync.clone(),
        canary_after: canary.after,
        canary_revisions: canary.revisions,
        canary_operations: canary.operations,
        payload_down_window: run.steady.payload_down,
        encrypted_window: Traffic::default(),
    }
}

/// PX-008's rule over srtop's own binary and socket: an unchanged fake source,
/// at the shortest interval the binary accepts, commits no app transaction
/// after the snapshot; the canary is the next one. The same check refuses a
/// source that changes.
#[test]
fn the_idle_check_passes_an_unchanged_source_and_refuses_a_changing_one() {
    let frozen = Srtop::start(&[
        "--fake-source",
        "--smoke-fixture",
        "--refresh-interval-ms",
        "50",
    ]);
    let report = idle_report(&frozen, Duration::from_millis(2500));
    check_idle(&report).unwrap();
    assert_eq!(report.ui_after_sync, 0);
    assert!(report.window >= Duration::from_millis(2500));
    println!(
        "PX-027 idle evidence (local): window {:?}, revision {}, app transactions 0, messages {:?}, canary {:?} after {:?}",
        report.window, report.sync_revision, report.after_sync, report.canary_revisions, report.canary_after
    );

    let changing = Srtop::start(&[
        "--fake-sequence",
        "--smoke-fixture",
        "--refresh-interval-ms",
        "100",
    ]);
    let report = idle_report(&changing, Duration::from_millis(1000));
    let error = check_idle(&report).unwrap_err();
    assert!(
        error.contains("app transactions after the snapshot"),
        "{error}"
    );
}

fn settings() -> Settings {
    Settings {
        interval_ms: 1000,
        warmup: Duration::from_secs(3),
        duration: Duration::from_secs(15),
        columns: 120,
        lines: 40,
        runs: 1,
    }
}

fn srui(workload: &str, interval: Option<u64>, secs: f64, processes: u64) -> SruiRecord {
    SruiRecord {
        workload: workload.into(),
        run: 0,
        rtt_ms: 0,
        processes,
        rows: processes,
        interval_ms: interval,
        attach_ms: 50.0,
        startup_payload: 1000,
        startup_encrypted: 5000,
        steady_secs: secs,
        down: Split::new(100, 200).unwrap(),
        up: Split::new(0, 50).unwrap(),
        ui_transactions: 15,
        workload_end_ms: 0.0,
    }
}

fn htop(
    workload: &str,
    delay: Option<u64>,
    secs: f64,
    processes: u64,
    size: (u16, u16),
) -> HtopRecord {
    HtopRecord {
        workload: workload.into(),
        run: 0,
        processes,
        delay_ds: delay,
        columns: size.0,
        lines: size.1,
        attach_ms: 60.0,
        startup_payload: 2000,
        startup_encrypted: 6000,
        steady_secs: secs,
        down: Split::new(300, 500).unwrap(),
        up: Split::new(0, 50).unwrap(),
        workload_end_ms: 0.0,
    }
}

#[test]
fn a_comparison_is_refused_unless_both_sides_ran_matched() {
    let s = settings();
    check_matched(
        &s,
        &[srui("busy", Some(1000), 15.0, 210)],
        &[htop("busy", Some(10), 15.0, 212, (120, 40))],
    )
    .unwrap();
    let cases = [
        (
            srui("busy", Some(1000), 15.0, 210),
            htop("busy", Some(20), 15.0, 210, (120, 40)),
            "mismatched intervals",
        ),
        (
            srui("busy", Some(500), 15.0, 210),
            htop("busy", Some(10), 15.0, 210, (120, 40)),
            "published an interval",
        ),
        (
            srui("busy", None, 15.0, 210),
            htop("busy", Some(10), 15.0, 210, (120, 40)),
            "published an interval",
        ),
        (
            srui("busy", Some(1000), 15.0, 210),
            htop("busy", None, 15.0, 210, (120, 40)),
            "mismatched intervals",
        ),
        (
            srui("busy", Some(1000), 15.0, 210),
            htop("busy", Some(10), 10.0, 210, (120, 40)),
            "windows of",
        ),
        (
            srui("busy", Some(1000), 15.0, 210),
            htop("busy", Some(10), 15.0, 20, (120, 40)),
            "processes",
        ),
        (
            srui("busy", Some(1000), 15.0, 210),
            htop("busy", Some(10), 15.0, 210, (80, 24)),
            "terminal",
        ),
        (
            srui("busy", Some(1000), 15.0, 210),
            htop("idle", Some(10), 15.0, 210, (120, 40)),
            "against 0 htop runs",
        ),
    ];
    for (ours, theirs, expected) in cases {
        let error = check_matched(&s, &[ours], &[theirs]).unwrap_err();
        assert!(error.contains(expected), "expected {expected:?} in {error}");
    }
    let mut odd = settings();
    odd.interval_ms = 1050;
    assert!(check_matched(&odd, &[], &[])
        .unwrap_err()
        .contains("no exact match"));
}

fn probe_run(payload_down: u64, encrypted_down: u64) -> SruiRun {
    let window = |payload_down, payload_up| Window {
        payload_up,
        payload_down,
        messages: 1,
        ui_transactions: 1,
        control_messages: 0,
        other_messages: 0,
    };
    SruiRun {
        attach: Duration::from_millis(40),
        sync_revision: 3,
        startup: window(4000, 60),
        steady: window(payload_down, 0),
        ui_after_sync: 1,
        after_sync: vec![("Transaction", payload_down)],
        encrypted_startup: Some(Traffic {
            up: 3000,
            down: 6000,
        }),
        encrypted_steady: Some(Traffic {
            up: 90,
            down: encrypted_down,
        }),
        rows: 9,
        published_interval_ms: Some(1000),
        steady_duration: Duration::from_secs(15),
        canary: None,
        raw_down: 4000 + payload_down,
        framed_down: 4000 + payload_down,
    }
}

/// The record keeps the probe's payload as payload and the relay's count as
/// encrypted: the overhead is their difference, in each window.
#[test]
fn a_record_keeps_payload_and_encrypted_bytes_apart() {
    let record = srui_record("busy", 0, 0, 9, &probe_run(1000, 1600)).unwrap();
    assert_eq!(
        (
            record.down.payload,
            record.down.encrypted,
            record.down.overhead()
        ),
        (1000, 1600, 600)
    );
    assert_eq!((record.up.payload, record.up.encrypted), (0, 90));
    assert_eq!(
        (record.startup_payload, record.startup_encrypted),
        (4060, 9000)
    );
    // A run whose encrypted stream carried less than its payload is refused.
    assert!(srui_record("busy", 0, 0, 9, &probe_run(1700, 1600)).is_err());
    let mut lost = probe_run(1000, 1600);
    lost.framed_down -= 1;
    assert!(srui_record("busy", 0, 0, 9, &lost).is_err());
}

/// htop is started with the `-d` the configured interval implies, and the
/// comparison holds the argument it was started with against srtop's own.
#[test]
fn htop_is_started_at_the_interval_srtop_publishes() {
    let devbox = Devbox {
        docker: "docker".into(),
        container: "c".into(),
        // Not 2222, the old fixed alias: the alias must follow the box's port (round 3, F10).
        port: 2242,
        key: PathBuf::from("/k"),
        known_hosts: PathBuf::from("/h"),
        logs: PathBuf::from("/l"),
    };
    let s = settings();
    let argv = devbox.htop_argv(
        "127.0.0.1:4000".parse().unwrap(),
        s.htop_delay_ds().unwrap(),
        120,
        40,
    );
    let remote = argv.last().unwrap();
    let delay = htop_delay_from_args(&remote[remote.find("htop ").unwrap()..]);
    assert!(argv.iter().any(|arg| arg == "-tt"));
    // The session reports its own size before htop starts (round 3, F8r).
    assert!(
        remote.starts_with("stty cols 120 rows 40 && stty size && exec env "),
        "{remote}"
    );
    check_matched(
        &s,
        &[srui("busy", Some(1000), 15.0, 210)],
        &[htop("busy", delay, 15.0, 210, (120, 40))],
    )
    .unwrap();
    let srui_argv = devbox.srui_argv("127.0.0.1:4000".parse().unwrap());
    assert_eq!(
        &srui_argv[srui_argv.len() - 3..],
        ["-s", "srui@127.0.0.1", "srui"]
    );
    for argv in [&argv, &srui_argv] {
        assert!(
            argv.windows(2).any(|pair| pair == ["-F", "/dev/null"]),
            "never the user's ssh config"
        );
        assert!(argv.iter().any(|arg| arg == "StrictHostKeyChecking=yes"));
        assert!(argv
            .iter()
            .any(|arg| arg == "Ciphers=chacha20-poly1305@openssh.com"));
        // The box's own known_hosts name, from its port.
        assert!(argv
            .iter()
            .any(|arg| arg == "HostKeyAlias=[127.0.0.1]:2242"));
    }
}

#[test]
fn the_tty_probe_counts_the_terminal_stream_by_window() {
    let argv: Vec<String> = ["sh", "-c", "printf abc; sleep 0.6; printf defg; exec cat"]
        .map(String::from)
        .to_vec();
    let run = TtyProbe::run(
        &argv,
        Duration::ZERO,
        Duration::from_millis(1500),
        b"q",
        None,
        None,
        Stdio::null(),
    )
    .unwrap();
    assert_eq!(run.startup_down + run.steady_down, 7);
    assert_eq!(run.steady_down, 4, "{run:?}");
    assert!(run.steady_duration >= Duration::from_millis(1500));
    // `cat` echoes the quit key after the window; the stream's start is kept.
    assert!(run.prefix.starts_with(b"abcdefg"), "{:?}", run.prefix);
}

/// PX-027 round 2 (F5): at an injected RTT the relay counts a byte when it is
/// delivered to the far side, not when it is read, so the encrypted window
/// moves with what the far side has received instead of leading it by RTT/2.
#[test]
fn the_tcp_relay_counts_a_byte_when_it_is_delivered_not_when_it_is_read() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let upstream = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut got = vec![0_u8; 1000];
        stream.read_exact(&mut got).unwrap();
        let at = Instant::now();
        let mut rest = Vec::new();
        stream.read_to_end(&mut rest).unwrap();
        at
    });
    let rtt = Duration::from_millis(800);
    let mut relay = TcpCountRelay::start(upstream, rtt).unwrap();
    let mut client = std::net::TcpStream::connect(relay.local_addr()).unwrap();
    let sent = Instant::now();
    client.write_all(&[3_u8; 1000]).unwrap();
    thread::sleep(Duration::from_millis(150));
    assert_eq!(
        relay.traffic().up,
        0,
        "150 ms after the write, 400 ms before its delivery, nothing has reached the far side"
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    while relay.traffic().up < 1000 && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(5));
    }
    let counted = Instant::now();
    assert_eq!(relay.traffic().up, 1000);
    assert!(
        counted >= sent + rtt / 2,
        "counted after {:?}, before its RTT/2 delivery",
        counted - sent
    );
    client.shutdown(Shutdown::Write).unwrap();
    let delivered = server.join().unwrap();
    assert!(delivered >= sent + rtt / 2);
    relay.stop();
}

/// PX-027 round 2 (F6): the canary must be the very next transaction after the
/// synchronized revision, with its two operations; anything else is refused.
#[test]
fn the_idle_check_refuses_a_canary_that_is_not_the_next_transaction() {
    let report = |revisions: (u64, u64), operations: usize| IdleReport {
        window: Duration::from_secs(10),
        sync_revision: 7,
        ui_after_sync: 0,
        after_sync: Vec::new(),
        canary_after: Duration::from_millis(20),
        canary_revisions: revisions,
        canary_operations: operations,
        payload_down_window: 0,
        encrypted_window: Traffic::default(),
    };
    check_idle(&report((7, 8), 2)).unwrap();
    for (revisions, operations) in [
        ((8, 9), 2),
        ((6, 7), 2),
        ((7, 9), 2),
        ((7, 8), 3),
        ((7, 8), 1),
    ] {
        let error = check_idle(&report(revisions, operations)).unwrap_err();
        assert!(
            error.contains("not the next transaction"),
            "{revisions:?} {operations}: {error}"
        );
    }
}

/// PX-027 round 2 (F7): the terminal probe drains as fast as the program
/// writes. 600 KB written in one burst inside the window all arrive inside it,
/// so the probe never trails htop or holds back what it already sent.
#[test]
fn the_tty_probe_keeps_up_with_a_fast_writer() {
    let argv: Vec<String> = [
        "sh",
        "-c",
        "printf go; sleep 0.3; head -c 600000 /dev/zero; exec cat",
    ]
    .map(String::from)
    .to_vec();
    let run = TtyProbe::run(
        &argv,
        Duration::ZERO,
        Duration::from_millis(1500),
        b"q",
        None,
        None,
        Stdio::null(),
    )
    .unwrap();
    assert_eq!(run.startup_down, 2, "{run:?}");
    assert_eq!(
        run.steady_down, 600_000,
        "every byte written in the window was drained in it"
    );
}

/// PX-027 round 2 (F8): the terminal size in a record is the one the remote
/// session reported with `stty size`, never the size that was asked for.
#[test]
fn an_htop_record_carries_the_terminal_size_the_session_reported() {
    assert_eq!(
        tty_size_from_prefix(b"40 120\r\n\x1b[?1049h"),
        Some((120, 40))
    );
    assert_eq!(tty_size_from_prefix(b"\x1b[?1049h"), None);
    assert_eq!(tty_size_from_prefix(b"40 120 7\r\n"), None);
    let run = |prefix: &[u8]| TtyRun {
        attach: Duration::from_millis(50),
        startup_down: 9000,
        steady_down: 300,
        steady_up: 0,
        encrypted_startup: Some(Traffic {
            up: 4000,
            down: 11000,
        }),
        encrypted_steady: Some(Traffic { up: 0, down: 500 }),
        steady_duration: Duration::from_secs(15),
        prefix: prefix.to_vec(),
    };
    let s = settings();
    let ours = [srui("busy", Some(1000), 15.0, 210)];
    let reported = htop_record("busy", 0, 210, Some(10), &run(b"40 120\r\n")).unwrap();
    assert_eq!((reported.columns, reported.lines), (120, 40));
    check_matched(&s, &ours, &[reported]).unwrap();
    for prefix in [&b"24 80\r\n"[..], b"\x1b[H"] {
        let record = htop_record("busy", 0, 210, Some(10), &run(prefix)).unwrap();
        let error = check_matched(&s, &ours, &[record]).unwrap_err();
        assert!(error.contains("terminal"), "{error}");
    }
}

// PX-027, Codex review: a session, program or connection that does not last the
// whole warm-up and steady window is refused, naming the phase and how far into
// it; the steady window reported is the one observed, never the one requested.

fn srui_plan(transport: Transport, held: Option<&dyn Fn() -> Result<(), String>>) -> SruiPlan<'_> {
    SruiPlan {
        transport,
        warmup: Duration::from_millis(1000),
        duration: Duration::from_millis(1000),
        encrypted: None,
        canary: None,
        held,
        stderr: Stdio::null(),
    }
}

#[test]
fn a_session_that_ends_in_its_warmup_or_steady_window_is_refused() {
    let srtop = Srtop::start(&["--fake-source"]);
    for (cut_after, phase) in [
        (400, "during the warm-up"),
        (1500, "during the steady window"),
    ] {
        let socket = srtop.path(&format!("cut-{cut_after}.sock"));
        let relay = std::sync::Arc::new(std::sync::Mutex::new(
            DelayRelay::start(&socket, &srtop.socket(), Duration::ZERO).unwrap(),
        ));
        let cutter = {
            let relay = std::sync::Arc::clone(&relay);
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(cut_after));
                relay.lock().unwrap().cut()
            })
        };
        let started = Instant::now();
        let error =
            SruiProbe::run(srui_plan(Transport::Unix(socket), None), b"px027-cut").unwrap_err();
        assert_eq!(cutter.join().unwrap(), 1, "the one connection was cut");
        assert!(
            error.contains("the session ended") && error.contains(phase),
            "{error}"
        );
        assert!(
            error.contains("the window was not observed whole"),
            "{error}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "refused promptly"
        );
        relay.lock().unwrap().stop();
    }
    // Held open, the same plan passes, with a window at least as long as requested.
    let run = SruiProbe::run(
        srui_plan(Transport::Unix(srtop.socket()), None),
        b"px027-held",
    )
    .unwrap();
    assert!(run.steady_duration >= Duration::from_millis(1000));
}

#[test]
fn a_terminal_program_that_exits_in_its_warmup_or_steady_window_is_refused() {
    for (script, phase) in [
        ("printf go; sleep 0.4", "during the warm-up"),
        ("printf go; sleep 1.5", "during the steady window"),
    ] {
        let argv: Vec<String> = ["sh", "-c", script].map(String::from).to_vec();
        let started = Instant::now();
        let error = TtyProbe::run(
            &argv,
            Duration::from_millis(1000),
            Duration::from_millis(1000),
            b"q",
            None,
            None,
            Stdio::null(),
        )
        .unwrap_err();
        assert!(
            error.contains("exited") || error.contains("stream ended"),
            "{error}"
        );
        assert!(error.contains(phase), "{script}: {error}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "refused promptly"
        );
    }
}

/// An upstream that closes its end of the TCP relay's connection after
/// `close_after`, with the relay in front of it and a client connected.
fn closing_upstream(close_after: Duration) -> (TcpCountRelay, std::net::TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let upstream = listener.local_addr().unwrap();
    thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        thread::sleep(close_after);
        drop(stream);
    });
    let relay = TcpCountRelay::start(upstream, Duration::ZERO).unwrap();
    let client = std::net::TcpStream::connect(relay.local_addr()).unwrap();
    (relay, client)
}

#[test]
fn a_relay_connection_that_closes_in_the_window_fails_the_run() {
    let argv: Vec<String> = ["sh", "-c", "printf go; exec cat"]
        .map(String::from)
        .to_vec();
    for (close_after, phase) in [
        (400, "during the warm-up"),
        (1500, "during the steady window"),
    ] {
        let (mut relay, _client) = closing_upstream(Duration::from_millis(close_after));
        relay.check_open().unwrap();
        let held = || relay.check_open();
        let started = Instant::now();
        let error = TtyProbe::run(
            &argv,
            Duration::from_millis(1000),
            Duration::from_millis(1000),
            b"q",
            None,
            Some(&held),
            Stdio::null(),
        )
        .unwrap_err();
        assert!(
            error.contains("counting relay closed") && error.contains(phase),
            "{error}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "refused promptly"
        );
        relay.stop();
    }
    // A relay whose connection stays open holds the same run.
    let (mut relay, _client) = closing_upstream(Duration::from_secs(30));
    let held = || relay.check_open();
    TtyProbe::run(
        &argv,
        Duration::from_millis(300),
        Duration::from_millis(300),
        b"q",
        None,
        Some(&held),
        Stdio::null(),
    )
    .unwrap();
    relay.stop();
}

/// The window reported is the one observed: here a slow encrypted sample at
/// the start of the window makes it longer than requested, and the record says so.
#[test]
fn the_steady_window_reported_is_the_one_observed() {
    let slow = || {
        thread::sleep(Duration::from_millis(300));
        Traffic::default()
    };
    let srtop = Srtop::start(&["--fake-source"]);
    let mut plan = srui_plan(Transport::Unix(srtop.socket()), None);
    plan.warmup = Duration::ZERO;
    plan.duration = Duration::from_millis(500);
    plan.encrypted = Some(&slow);
    let run = SruiProbe::run(plan, b"px027-observed").unwrap();
    assert!(
        run.steady_duration >= Duration::from_millis(800),
        "{:?}",
        run.steady_duration
    );
    let argv: Vec<String> = ["sh", "-c", "printf go; exec cat"]
        .map(String::from)
        .to_vec();
    let run = TtyProbe::run(
        &argv,
        Duration::ZERO,
        Duration::from_millis(500),
        b"q",
        Some(&slow),
        None,
        Stdio::null(),
    )
    .unwrap();
    assert!(
        run.steady_duration >= Duration::from_millis(800),
        "{:?}",
        run.steady_duration
    );
}

// PX-027, Codex review (second pass): a workload must outlive its window, and
// every string written into JSON is escaped.

#[test]
fn a_workload_that_would_outlive_its_workers_is_refused_before_anything_starts() {
    let mut long = settings();
    long.duration = Duration::from_millis(60_000);
    let error = check_workload_fits(&[Workload::Busy], &[long.lifetime()]).unwrap_err();
    assert!(
        error.contains("72.5 s") && error.contains("exceeds the workers' 60 s limit"),
        "{error}"
    );
    check_workload_fits(&[Workload::Idle], &[long.lifetime()]).unwrap();
    check_workload_fits(&[Workload::Churn], &[settings().lifetime()]).unwrap();
    // Refused before any process starts: a docker that does not exist is never reached.
    let devbox = Devbox {
        docker: "/nonexistent/px027-docker".into(),
        container: "c".into(),
        port: 2242,
        key: PathBuf::from("/k"),
        known_hosts: PathBuf::from("/h"),
        logs: PathBuf::from("/l"),
    };
    for error in [
        compare(&devbox, &[Workload::Idle, Workload::Churn], &long).unwrap_err(),
        rtt(&devbox, Workload::Busy, &[0, 600], &long).unwrap_err(),
    ] {
        assert!(
            error.contains("exceeds the workers'") && !error.contains("docker"),
            "{error}"
        );
    }
}

fn box_processes(workers: &[&str], sleepers: usize) -> Vec<(String, String)> {
    let mut processes: Vec<(String, String)> = workers
        .iter()
        .map(|mode| {
            (
                "srtop-demo-work".to_string(),
                format!("/usr/local/bin/srtop-demo-worker --srtop-test-owner srtop-demo-1 --mode {mode} --duration-ms 27500"),
            )
        })
        .collect();
    processes.extend((0..sleepers).map(|_| ("sleep".to_string(), "sleep 27".to_string())));
    processes.push(("sshd".into(), "sshd: srui [priv]".into()));
    processes
}

#[test]
fn a_workload_is_whole_only_while_every_worker_and_sleeper_lives() {
    Workload::Busy
        .check_alive(&box_processes(&["cpu", "memory", "threads"], 200))
        .unwrap();
    Workload::Churn
        .check_alive(&box_processes(
            &["cpu", "memory", "threads", "short-lived", "child", "child"],
            200,
        ))
        .unwrap();
    Workload::Idle.check_alive(&[]).unwrap();
    for (workload, processes, missing) in [
        (
            Workload::Busy,
            box_processes(&["cpu", "threads"], 200),
            "memory worker 0/1",
        ),
        (
            Workload::Churn,
            box_processes(&["cpu", "memory", "threads"], 200),
            "short-lived worker 0/1",
        ),
        (
            Workload::Busy,
            box_processes(&["cpu", "memory", "threads"], 199),
            "sleepers 199/200",
        ),
    ] {
        let error = workload.check_alive(&processes).unwrap_err();
        assert!(error.contains(missing), "{error}");
    }
}

/// A real PX-025 worker killed in the middle of the steady window fails the run
/// through the probe's `held` hook, naming the phase.
#[test]
fn a_workload_worker_killed_mid_window_fails_the_run() {
    use srtop_demo::{
        harness::{fresh_owner, live_processes_with_owner, wait_until_none_live, DemoWorker},
        worker::{Mode, WorkerSpec},
    };
    let owner = fresh_owner();
    let spec = WorkerSpec {
        owner: owner.clone(),
        mode: Mode::Cpu { threads: 1 },
        duration: Duration::from_secs(20),
        fail_after: None,
    };
    let worker = std::sync::Arc::new(std::sync::Mutex::new(
        DemoWorker::spawn(
            std::path::Path::new(env!("CARGO_BIN_EXE_srtop-demo-worker")),
            &spec,
        )
        .unwrap(),
    ));
    let deadline = Instant::now() + Duration::from_secs(5);
    while live_processes_with_owner(&owner).unwrap().is_empty() {
        assert!(Instant::now() < deadline, "the worker started");
        thread::sleep(Duration::from_millis(10));
    }
    let watching = owner.clone();
    let mut watch =
        WorkloadWatch::start(
            Duration::from_millis(50),
            move || match live_processes_with_owner(&watching)
                .map_err(|error| error.to_string())?
                .len()
            {
                1 => Ok(()),
                alive => Err(format!("cpu worker {alive}/1")),
            },
        );
    let killer = {
        let worker = std::sync::Arc::clone(&worker);
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(1000));
            worker.lock().unwrap().kill().unwrap();
        })
    };
    let held = || watch.held();
    let argv: Vec<String> = ["sh", "-c", "printf go; exec cat"]
        .map(String::from)
        .to_vec();
    let started = Instant::now();
    let error = TtyProbe::run(
        &argv,
        Duration::from_millis(300),
        Duration::from_millis(2000),
        b"q",
        None,
        Some(&held),
        Stdio::null(),
    )
    .unwrap_err();
    killer.join().unwrap();
    watch.stop();
    assert!(
        error.contains("the workload did not last the window")
            && error.contains("during the steady window"),
        "{error}"
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(wait_until_none_live(&owner, Duration::from_secs(5))
        .unwrap()
        .is_empty());
}

/// Labels round-trip through a real JSON parser unchanged, whatever they hold.
#[test]
fn every_string_written_into_json_is_escaped() {
    assert_eq!(json_string("a\"b\\c"), r#""a\"b\\c""#);
    assert_eq!(
        json_string("\n\t\r\u{8}\u{c}\u{1}\u{1f}"),
        r#""\n\t\r\b\f\u0001\u001f""#
    );
    assert_eq!(json_string("é ✓"), "\"é ✓\"");
    let label = "x\"y\\z\nnew\tline\u{1}é ✓";
    let mut record = srui("busy", Some(1000), 15.0, 210);
    record.workload = label.to_string();
    let srui_text = srui_json(&record);
    let mut theirs = htop("busy", Some(10), 15.0, 210, (120, 40));
    theirs.workload = label.to_string();
    let htop_text = htop_json(&theirs);
    let idle_text = idle_json(&IdleReport {
        window: Duration::from_secs(10),
        sync_revision: 1,
        ui_after_sync: 0,
        after_sync: vec![("Transaction", 12)],
        canary_after: Duration::from_millis(5),
        canary_revisions: (1, 2),
        canary_operations: 2,
        payload_down_window: 0,
        encrypted_window: Traffic::default(),
    });
    for (text, field) in [(&srui_text, "workload"), (&htop_text, "workload")] {
        assert_eq!(python_json_field(text, field), label.as_bytes(), "{text}");
    }
    assert_eq!(
        python_json_field(&idle_text, "messages_after_sync"),
        b"[\"Transaction (12 B)\"]"
    );
}

/// Parses `text` with Python's `json` and prints `field` (a string as itself, anything else
/// as compact JSON), as UTF-8 bytes.
fn python_json_field(text: &str, field: &str) -> Vec<u8> {
    let mut child = Command::new("python3")
        .args([
            "-I",
            "-c",
            "import json,sys\nvalue=json.loads(sys.stdin.buffer.read().decode('utf-8'))[sys.argv[1]]\nsys.stdout.buffer.write((value if isinstance(value,str) else json.dumps(value,separators=(',',':'))).encode('utf-8'))",
            field,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("python3 parses the JSON");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(text.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "python3 refused {text}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}
