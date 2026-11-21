//! PX-025 scenarios against a real srtop through the delay relay: remote
//! result latency at 0/100/300/600 ms RTT, a transport interruption, a
//! collector failure and, on Linux, srtop collecting the demo workers from
//! `/proc`.
//!
//! srtop is the binary `apps/srtop` builds (`cargo build --locked
//! --manifest-path apps/srtop/Cargo.toml`), or `$SRTOP_BIN`. A missing binary
//! fails these tests; nothing is skipped.

use srtop_demo::{
    scenario::{
        collector_failure, median, remote_results, transport_interruption, COLLECTOR_ERROR,
        FAILED_SCAN, RTTS_MS,
    },
    wire::Decision,
};
use srui_process_explorer::source::SEQUENCE_STATUS_TEXT;
use std::{
    fs,
    os::unix::{fs::PermissionsExt, net::UnixStream},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicU64, Ordering},
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
            "/tmp/srtop-px025-{}-{nanos}-{}",
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
        // Generous: macOS assesses a newly linked binary's first exec.
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
            std::thread::sleep(Duration::from_millis(10));
        }
        srtop
    }

    fn socket(&self) -> PathBuf {
        self.directory.join("s.sock")
    }

    fn relay_socket(&self, name: &str) -> PathBuf {
        self.directory.join(name)
    }
}

impl Drop for Srtop {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.directory);
    }
}

/// Server-dependent results — the committed filter and search status after a
/// `TEXT_EDIT` — arrive no sooner than one RTT and track it at every RTT.
#[test]
fn remote_results_are_delayed_by_the_injected_rtt() {
    let srtop = Srtop::start(&["--fake-repeated-names"]);
    // This run's 0 ms floor for each kind of result: its fastest result
    // without injected delay, at most 30 ms. Set by the first RTT, which is 0;
    // the filter and the search each have their own.
    assert_eq!(RTTS_MS[0], 0);
    let mut floors: [Option<Duration>; 2] = [None, None];
    // Every RTT is measured and every violation reported, so a regression
    // shows at each RTT it affects rather than at the first one only.
    let mut violations = Vec::new();
    for rtt_ms in RTTS_MS {
        let rtt = Duration::from_millis(rtt_ms);
        let report = remote_results(
            &srtop.socket(),
            &srtop.relay_socket(&format!("r{rtt_ms}.sock")),
            rtt,
            5,
        )
        .unwrap();
        for (index, (kind, samples)) in [("filter", &report.filter), ("search", &report.search)]
            .into_iter()
            .enumerate()
        {
            assert_eq!(samples.len(), 5);
            for sample in samples.iter() {
                if *sample < rtt {
                    violations.push(format!(
                        "rtt {rtt_ms}: a {kind} result arrived after {sample:?}, sooner than one RTT"
                    ));
                }
            }
            if median(samples) >= rtt + Duration::from_millis(400) {
                violations.push(format!("rtt {rtt_ms}: {kind} p50 {:?}", median(samples)));
            }
            // Not one round trip more: the fastest result is within 1.5 RTT
            // plus the 0 ms floor plus 5 ms, while a result that waited an
            // extra RTT takes at least 2 RTT. Only at a nonzero RTT: at 0 ms
            // the floor is this very measurement.
            let fastest = *samples.iter().min().unwrap();
            let floor =
                *floors[index].get_or_insert_with(|| fastest.min(Duration::from_millis(30)));
            let ceiling = rtt + rtt / 2 + floor + Duration::from_millis(5);
            if rtt_ms > 0 && fastest >= ceiling {
                violations.push(format!(
                    "rtt {rtt_ms}: the fastest {kind} result took {fastest:?}, past {ceiling:?}"
                ));
            }
        }
        // The handshake crosses the relay too.
        if report.attach < rtt {
            violations.push(format!("rtt {rtt_ms}: attach {:?}", report.attach));
        }
        assert!(report.traffic.up > 0 && report.traffic.down > 0);
        println!("PX-025 remote result evidence: {}", report.to_json());
    }
    assert!(violations.is_empty(), "{}", violations.join("\n"));
}

/// A cut transport is resumed through the same relay with the runtime's own
/// `CLIENT_RESUME`, at every RTT, and the replay catches the client up.
#[test]
fn a_transport_interruption_resumes_through_the_relay_at_every_rtt() {
    let srtop = Srtop::start(&["--fake-sequence", "--refresh-interval-ms", "100"]);
    for rtt_ms in RTTS_MS {
        let rtt = Duration::from_millis(rtt_ms);
        let report = transport_interruption(
            &srtop.socket(),
            &srtop.relay_socket(&format!("i{rtt_ms}.sock")),
            rtt,
            Duration::from_millis(300),
        )
        .unwrap();
        assert_eq!(report.cut, 1, "rtt {rtt_ms}: the one connection was cut");
        assert!(
            matches!(report.decision, Decision::ResumeOk { .. }),
            "rtt {rtt_ms}: within the default journal the same session replays: {:?}",
            report.decision
        );
        assert!(
            report.resume >= rtt,
            "rtt {rtt_ms}: resume {:?}",
            report.resume
        );
        assert!(report.revision_after > report.revision_at_cut);
        println!("PX-025 interruption evidence: {}", report.to_json());
    }
}

/// The scripted source's failed scan reaches a client behind 300 ms of RTT as
/// a failure: every row kept with its values, the status and freshness line
/// saying why, and recovery converging on the same rows.
#[test]
fn a_collector_failure_reaches_a_delayed_client_as_a_failure() {
    let srtop = Srtop::start(&["--fake-sequence", "--refresh-interval-ms", "100"]);
    let report = collector_failure(
        &srtop.socket(),
        &srtop.relay_socket("f.sock"),
        Duration::from_millis(300),
    )
    .unwrap();
    assert_eq!(report.rows_before.len(), 3, "{:?}", report.rows_before);
    assert_eq!(
        report.rows_during, report.rows_before,
        "a failed scan keeps every row and every value; nothing becomes zero"
    );
    assert!(
        report.status_during.contains(FAILED_SCAN)
            && report
                .status_during
                .contains("3 rows retained from an earlier scan"),
        "{}",
        report.status_during
    );
    assert!(
        report.freshness_during.starts_with(COLLECTOR_ERROR),
        "{}",
        report.freshness_during
    );
    assert_eq!(report.status_after, SEQUENCE_STATUS_TEXT);
    assert_eq!(
        report.rows_after, report.rows_before,
        "recovery moves no row"
    );
    println!("PX-025 collector failure evidence: {}", report.to_json());
}

/// The scenario command composes all of the above and prints one report.
#[test]
fn the_scenarios_command_prints_one_report() {
    let output = Command::new(env!("CARGO_BIN_EXE_srtop-demo"))
        .args(["scenarios", "--trials", "2", "--srtop"])
        .arg(srtop_binary())
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report = String::from_utf8(output.stdout).unwrap();
    for rtt in RTTS_MS {
        assert_eq!(
            report.matches(&format!("\"rtt_ms\": {rtt},")).count(),
            if rtt == 300 { 3 } else { 2 },
            "{report}"
        );
    }
    assert!(report.contains("\"decision\": \"ResumeOk"), "{report}");
    assert!(report.contains(COLLECTOR_ERROR), "{report}");
    println!("PX-025 scenario report: {report}");
}

/// On Linux, srtop's live `/proc` source shows the demo workers as they are:
/// the memory worker's resident size, the CPU worker's CPU and the processes
/// themselves, each a test-owned process and nothing else.
#[cfg(target_os = "linux")]
#[test]
fn srtop_collects_the_demo_workers_from_live_proc() {
    use srtop_demo::{
        harness::{fresh_owner, wait_until_none_live, DemoWorker},
        wire::Replica,
        worker::{Mode, WorkerSpec},
    };
    let exe = std::path::Path::new(env!("CARGO_BIN_EXE_srtop-demo-worker"));
    let owner = fresh_owner();
    let spec = |mode| WorkerSpec {
        owner: owner.clone(),
        mode,
        duration: Duration::from_secs(30),
        fail_after: None,
    };
    let mut memory = DemoWorker::spawn(exe, &spec(Mode::Memory { mib: 96 })).unwrap();
    let mut cpu = DemoWorker::spawn(exe, &spec(Mode::Cpu { threads: 1 })).unwrap();
    let srtop = Srtop::start(&["--live-source", "--refresh-interval-ms", "250"]);
    let mut replica = Replica::hello(&srtop.socket(), b"px025-live").unwrap();
    let headings = replica.headings();
    let column = |name: &str| {
        headings
            .iter()
            .position(|heading| heading.starts_with(name))
            .unwrap_or_else(|| panic!("no {name} column in {headings:?}"))
    };
    let (pid, resident, cpu_column) = (column("PID"), column("Resident"), column("CPU"));
    let row_of = |replica: &Replica, wanted: u32| {
        replica
            .rows()
            .into_iter()
            .find(|row| row[pid] == wanted.to_string())
    };
    let mib = |cell: &str| -> f64 {
        let (number, unit) = cell.split_once(' ').unwrap_or((cell, "B"));
        let number: f64 = number.parse().unwrap_or(0.0);
        number
            * match unit {
                "KiB" => 1.0 / 1024.0,
                "MiB" => 1.0,
                "GiB" => 1024.0,
                _ => 0.0,
            }
    };
    let percent = |cell: &str| -> f64 { cell.trim_end_matches('%').parse().unwrap_or(0.0) };
    let (memory_pid, cpu_pid) = (memory.pid(), cpu.pid());
    replica
        .until(
            "both workers with their figures",
            Duration::from_secs(20),
            |replica| {
                let memory_row = row_of(replica, memory_pid);
                let cpu_row = row_of(replica, cpu_pid);
                memory_row.is_some_and(|row| mib(&row[resident]) >= 96.0)
                    && cpu_row.is_some_and(|row| percent(&row[cpu_column]) >= 50.0)
            },
        )
        .unwrap();
    let memory_row = row_of(&replica, memory_pid).unwrap();
    let cpu_row = row_of(&replica, cpu_pid).unwrap();
    println!("PX-025 live Linux evidence: memory worker {memory_row:?}; cpu worker {cpu_row:?}");
    memory.stop(Duration::from_secs(2)).unwrap();
    cpu.stop(Duration::from_secs(2)).unwrap();
    assert!(wait_until_none_live(&owner, Duration::from_secs(2))
        .unwrap()
        .is_empty());
}
