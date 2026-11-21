//! PX-025 acceptance for the demonstration worker: bounded, test-owned, and
//! gone after success, after failure and after its owner is interrupted
//! (SIGINT, SIGTERM, SIGKILL).
//!
//! Every process these tests signal is one they started and have not reaped
//! (a [`DemoWorker`] or the `srtop-demo workers` harness's [`Child`]), so its
//! PID cannot name anything else. Cleanup is checked by the run's unique owner
//! token ([`processes_with_owner`]), never by a process name.

use srtop_demo::{
    harness::{fresh_owner, processes_with_owner, wait_until_none_live, DemoWorker},
    worker::{
        exit, Mode, WorkerSpec, MAX_CHILDREN, MAX_CHILD_LIFETIME, MAX_CPU_THREADS, MAX_DURATION,
        MAX_MEMORY_MIB, MAX_SPAWN_INTERVAL, MAX_THREADS, MIN_SPAWN_INTERVAL, OWNER_ENV, OWNER_FLAG,
    },
};
use std::{
    io::{BufRead, BufReader},
    os::unix::process::ExitStatusExt,
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

fn worker_exe() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_srtop-demo-worker"))
}

fn spec(owner: &str, mode: Mode, duration_ms: u64) -> WorkerSpec {
    WorkerSpec {
        owner: owner.to_string(),
        mode,
        duration: Duration::from_millis(duration_ms),
        fail_after: None,
    }
}

fn short_lived(count: u32, lifetime_ms: u64, interval_ms: u64) -> Mode {
    Mode::ShortLived {
        count,
        lifetime: Duration::from_millis(lifetime_ms),
        interval: Duration::from_millis(interval_ms),
    }
}

fn args(spec: &WorkerSpec) -> Vec<String> {
    spec.to_args()
}

/// The workloads one test runs, all under one owner.
fn every_mode(owner: &str, duration_ms: u64) -> Vec<WorkerSpec> {
    vec![
        spec(owner, Mode::Cpu { threads: 1 }, duration_ms),
        spec(owner, Mode::Memory { mib: 16 }, duration_ms),
        spec(owner, Mode::Threads { count: 8 }, duration_ms),
        spec(owner, short_lived(20, 150, 20), duration_ms),
    ]
}

fn none_live(owner: &str, within: Duration) {
    let live = wait_until_none_live(owner, within).unwrap();
    assert!(
        live.is_empty(),
        "processes of {owner} are still running: {live:?}"
    );
}

#[test]
fn every_limit_is_accepted_at_its_bound_and_refused_just_past_it() {
    let owner = fresh_owner();
    let ms = |duration: Duration| duration.as_millis() as u64;
    let accepted = [
        spec(&owner, Mode::Cpu { threads: 1 }, ms(MAX_DURATION)),
        spec(
            &owner,
            Mode::Cpu {
                threads: MAX_CPU_THREADS,
            },
            10,
        ),
        spec(
            &owner,
            Mode::Memory {
                mib: MAX_MEMORY_MIB,
            },
            10,
        ),
        spec(&owner, Mode::Threads { count: MAX_THREADS }, 10),
        spec(&owner, short_lived(MAX_CHILDREN, 1, 10), 10),
        spec(
            &owner,
            short_lived(1, ms(MAX_CHILD_LIFETIME), ms(MIN_SPAWN_INTERVAL)),
            10,
        ),
        spec(&owner, short_lived(1, 1, ms(MAX_SPAWN_INTERVAL)), 10),
        spec(&owner, Mode::Child, ms(MAX_CHILD_LIFETIME)),
        WorkerSpec {
            fail_after: Some(Duration::from_millis(10)),
            ..spec(&owner, Mode::Cpu { threads: 1 }, 10)
        },
    ];
    for spec in &accepted {
        assert_eq!(spec.validate(), Ok(()), "{spec:?}");
        // And the argument vector round-trips through the worker's parser.
        assert_eq!(
            WorkerSpec::parse(&args(spec), Some(&owner)).as_ref(),
            Ok(spec),
            "{spec:?}"
        );
    }
    let refused = [
        spec(&owner, Mode::Cpu { threads: 1 }, ms(MAX_DURATION) + 1),
        spec(&owner, Mode::Cpu { threads: 1 }, 0),
        spec(
            &owner,
            Mode::Cpu {
                threads: MAX_CPU_THREADS + 1,
            },
            10,
        ),
        spec(&owner, Mode::Cpu { threads: 0 }, 10),
        spec(
            &owner,
            Mode::Memory {
                mib: MAX_MEMORY_MIB + 1,
            },
            10,
        ),
        spec(&owner, Mode::Memory { mib: 0 }, 10),
        spec(
            &owner,
            Mode::Threads {
                count: MAX_THREADS + 1,
            },
            10,
        ),
        spec(&owner, short_lived(MAX_CHILDREN + 1, 1, 10), 10),
        spec(&owner, short_lived(1, ms(MAX_CHILD_LIFETIME) + 1, 10), 10),
        spec(&owner, short_lived(1, 1, ms(MIN_SPAWN_INTERVAL) - 1), 10),
        spec(&owner, short_lived(1, 1, ms(MAX_SPAWN_INTERVAL) + 1), 10),
        spec(&owner, Mode::Child, ms(MAX_CHILD_LIFETIME) + 1),
        WorkerSpec {
            fail_after: Some(Duration::from_millis(11)),
            ..spec(&owner, Mode::Cpu { threads: 1 }, 10)
        },
    ];
    for spec in &refused {
        assert!(spec.validate().is_err(), "{spec:?} must be refused");
        assert!(
            WorkerSpec::parse(&args(spec), Some(&owner)).is_err(),
            "{spec:?}"
        );
        assert!(
            DemoWorker::spawn(worker_exe(), spec).is_err(),
            "{spec:?} is refused before anything starts"
        );
    }
    // The binary refuses the same values itself, past the bound and at none.
    for spec in &refused[..1] {
        let status = Command::new(worker_exe())
            .args(args(spec))
            .env(OWNER_ENV, &owner)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(exit::REFUSED));
    }
    none_live(&owner, Duration::from_secs(1));
}

/// A worker is opt-in: without the owner token in both its arguments and its
/// environment, or with a malformed one, it does nothing.
#[test]
fn a_worker_without_its_test_owner_marker_refuses_to_start() {
    let owner = fresh_owner();
    let good = args(&spec(&owner, Mode::Cpu { threads: 1 }, 30_000));
    let without_flag: Vec<String> = good[2..].to_vec();
    let cases: Vec<(Vec<String>, Option<String>)> = vec![
        (good.clone(), None),
        (good.clone(), Some(format!("{owner}x"))),
        (without_flag, Some(owner.clone())),
        (
            {
                let mut bad = good.clone();
                bad[1] = "production-db".into();
                bad
            },
            Some("production-db".into()),
        ),
        (
            {
                let mut bad = good.clone();
                bad[1] = "srtop-demo-UPPER".into();
                bad
            },
            Some("srtop-demo-UPPER".into()),
        ),
    ];
    for (arguments, env) in cases {
        let mut command = Command::new(worker_exe());
        command
            .args(&arguments)
            .env_remove(OWNER_ENV)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(env) = &env {
            command.env(OWNER_ENV, env);
        }
        let started = Instant::now();
        let status = command.status().unwrap();
        assert_eq!(
            status.code(),
            Some(exit::REFUSED),
            "{arguments:?} with {env:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "refused at once"
        );
    }
    none_live(&owner, Duration::from_secs(1));
}

/// Success: every mode is marked with its owner while it runs, runs until its
/// deadline and not past it, and leaves nothing behind — the short-lived
/// worker's children included.
#[test]
fn every_mode_is_test_owned_and_ends_by_itself_at_its_deadline() {
    let owner = fresh_owner();
    let duration = Duration::from_millis(1_500);
    let started = Instant::now();
    let mut workers: Vec<DemoWorker> = every_mode(&owner, duration.as_millis() as u64)
        .iter()
        .map(|spec| DemoWorker::spawn(worker_exe(), spec).unwrap())
        .collect();
    // Marked while running: each worker, and children of the short-lived one,
    // carry the token in their arguments (and, on Linux, their environment).
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let found = processes_with_owner(&owner).unwrap();
        let workers_found = workers
            .iter()
            .all(|worker| found.iter().any(|process| process.pid == worker.pid()));
        if workers_found && found.len() > workers.len() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "every worker and a short-lived child carry {OWNER_FLAG} {owner}: {found:?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    #[cfg(target_os = "linux")]
    for worker in &workers {
        let environ = std::fs::read(format!("/proc/{}/environ", worker.pid())).unwrap();
        let wanted = format!("{OWNER_ENV}={owner}");
        assert!(
            environ
                .split(|byte| *byte == 0)
                .any(|entry| entry == wanted.as_bytes()),
            "worker {} carries {wanted}",
            worker.pid()
        );
    }
    // At the bound, not before it: still running shortly before the deadline.
    let early = duration - Duration::from_millis(400);
    std::thread::sleep(early.saturating_sub(started.elapsed()));
    for worker in &mut workers {
        assert_eq!(
            worker.try_status().unwrap(),
            None,
            "worker {} ended before its deadline",
            worker.pid()
        );
    }
    // And not past it: each ends by itself, `COMPLETED`, within a second.
    for worker in &mut workers {
        let status = worker
            .wait_timeout((duration + Duration::from_secs(1)).saturating_sub(started.elapsed()))
            .unwrap()
            .unwrap_or_else(|| panic!("worker {} ran past its deadline", worker.pid()));
        assert_eq!(status.code(), Some(exit::COMPLETED));
    }
    let elapsed = started.elapsed();
    assert!(elapsed >= duration, "{elapsed:?}");
    none_live(&owner, Duration::from_secs(2));
    println!("PX-025 worker deadline evidence: duration {duration:?}, all ended after {elapsed:?}");
}

/// An orderly stop ends a worker and its children at once.
#[test]
fn stop_ends_a_worker_and_its_children_at_once() {
    let owner = fresh_owner();
    let mut worker = DemoWorker::spawn(
        worker_exe(),
        &spec(&owner, short_lived(40, 2_000, 10), 30_000),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while processes_with_owner(&owner).unwrap().len() < 4 {
        assert!(Instant::now() < deadline, "children start");
        std::thread::sleep(Duration::from_millis(10));
    }
    let started = Instant::now();
    let status = worker.stop(Duration::from_secs(5)).unwrap();
    assert_eq!(status.code(), Some(exit::STOPPED));
    none_live(&owner, Duration::from_secs(2));
    println!(
        "PX-025 worker stop evidence: stopped with its children in {:?}",
        started.elapsed()
    );
}

/// Failure, the worker's: a workload that fails ends, and its children end
/// with it.
#[test]
fn a_failing_worker_leaves_nothing_behind() {
    let owner = fresh_owner();
    let mut worker = DemoWorker::spawn(
        worker_exe(),
        &WorkerSpec {
            fail_after: Some(Duration::from_millis(400)),
            ..spec(&owner, short_lived(40, 2_000, 10), 30_000)
        },
    )
    .unwrap();
    let status = worker
        .wait_timeout(Duration::from_secs(5))
        .unwrap()
        .expect("the failing worker ends");
    assert_eq!(status.code(), Some(exit::FAILED));
    none_live(&owner, Duration::from_secs(2));
}

/// Failure, the harness's: a test that fails while its workers run unwinds
/// through their handles, which end them.
#[test]
fn a_failing_test_cleans_up_its_workers_as_it_unwinds() {
    let owner = fresh_owner();
    let pids = std::sync::Mutex::new(Vec::new());
    let outcome = std::panic::catch_unwind(|| {
        let workers: Vec<DemoWorker> = every_mode(&owner, 30_000)
            .iter()
            .map(|spec| DemoWorker::spawn(worker_exe(), spec).unwrap())
            .collect();
        pids.lock()
            .unwrap()
            .extend(workers.iter().map(DemoWorker::pid));
        let deadline = Instant::now() + Duration::from_secs(5);
        while processes_with_owner(&owner).unwrap().len() <= workers.len() {
            assert!(Instant::now() < deadline, "children start");
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("a failed assertion while the workers run");
    });
    assert!(outcome.is_err(), "the scenario failed");
    assert_eq!(pids.lock().unwrap().len(), 4);
    // 30 s deadlines: only cleanup can end them this soon.
    none_live(&owner, Duration::from_secs(2));
}

/// An owner that vanishes without running any code closes the lifeline, and
/// the worker ends by itself long before its deadline.
#[test]
fn a_worker_whose_owner_vanished_ends_at_once() {
    let owner = fresh_owner();
    let mut worker = DemoWorker::spawn(
        worker_exe(),
        &spec(&owner, Mode::Threads { count: 4 }, 30_000),
    )
    .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(worker.try_status().unwrap(), None);
    worker.cut_lifeline();
    let status = worker
        .wait_timeout(Duration::from_secs(2))
        .unwrap()
        .expect("an orphaned worker ends");
    assert_eq!(status.code(), Some(exit::ORPHANED));
    none_live(&owner, Duration::from_secs(1));
}

/// The `srtop-demo workers` harness, started with every mode and read until
/// it has named its owner token and every worker.
fn start_harness() -> (Child, String, Vec<(String, u32)>) {
    let mut harness = Command::new(env!("CARGO_BIN_EXE_srtop-demo"))
        .args([
            "workers",
            "--duration-ms",
            "30000",
            "--cpu-threads",
            "1",
            "--memory-mib",
            "16",
            "--threads",
            "8",
            "--short-lived",
            "64",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut lines = BufReader::new(harness.stdout.take().unwrap()).lines();
    let owner = lines
        .next()
        .unwrap()
        .unwrap()
        .strip_prefix("owner ")
        .expect("the harness names its owner first")
        .to_string();
    let workers = (0..4)
        .map(|_| {
            let line = lines.next().unwrap().unwrap();
            let mut words = line.split(' ');
            assert_eq!(words.next(), Some("worker"), "{line}");
            let mode = words.next().unwrap().to_string();
            (mode, words.next().unwrap().parse().unwrap())
        })
        .collect();
    // The rest of its output is not needed; dropping the reader closes it.
    (harness, owner, workers)
}

/// Interruption: whichever way the harness is stopped — SIGINT, SIGTERM or
/// SIGKILL, none of which runs its cleanup code — every worker and every
/// short-lived child ends at once, long before its 30 s deadline.
#[test]
fn interrupting_the_harness_ends_every_worker_and_child() {
    for signal in ["INT", "TERM", "KILL"] {
        let (mut harness, owner, workers) = start_harness();
        let modes: Vec<&str> = workers.iter().map(|(mode, _)| mode.as_str()).collect();
        assert_eq!(modes, ["cpu", "memory", "threads", "short-lived"]);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let found = processes_with_owner(&owner).unwrap();
            let all_workers = workers
                .iter()
                .all(|(_, pid)| found.iter().any(|process| process.pid == *pid));
            if all_workers && found.len() > workers.len() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "SIG{signal}: workers and children start: {found:?}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        // The harness is this test's own unreaped child, so its PID is still its.
        let status = Command::new("/bin/kill")
            .args([&format!("-{signal}"), &harness.id().to_string()])
            .status()
            .unwrap();
        assert!(status.success());
        let interrupted = Instant::now();
        let ended = harness.wait().unwrap();
        let expected = match signal {
            "INT" => 2,
            "TERM" => 15,
            _ => 9,
        };
        assert_eq!(
            ended.signal(),
            Some(expected),
            "SIG{signal} ended the harness"
        );
        none_live(&owner, Duration::from_secs(3));
        println!(
            "PX-025 interruption evidence: SIG{signal} -> {} workers and their children gone in {:?}",
            workers.len(),
            interrupted.elapsed()
        );
    }
}
