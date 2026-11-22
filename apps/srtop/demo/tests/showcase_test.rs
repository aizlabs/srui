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
        check_idle, check_matched, htop_delay_from_args, srui_record, Devbox, HtopRecord,
        IdleReport, Settings, SruiRecord,
    },
    traffic::{
        check_payload_counts, published_interval_ms, Distribution, Split, SruiPlan, SruiProbe,
        SruiRun, TcpCountRelay, Transport, TtyProbe, Window,
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
        port: 2222,
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
    assert!(remote.starts_with("stty cols 120 rows 40 && "), "{remote}");
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
    }
}

#[test]
fn the_tty_probe_counts_the_terminal_stream_by_window() {
    let argv: Vec<String> = [
        "sh",
        "-c",
        "printf abc; sleep 0.6; printf defg; exec cat >/dev/null",
    ]
    .map(String::from)
    .to_vec();
    let run = TtyProbe::run(
        &argv,
        Duration::ZERO,
        Duration::from_millis(1500),
        b"q",
        None,
        Stdio::null(),
    )
    .unwrap();
    assert_eq!(run.startup_down + run.steady_down, 7);
    assert_eq!(run.steady_down, 4, "{run:?}");
    assert!(run.steady_duration >= Duration::from_millis(1500));
}
