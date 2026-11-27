//! PX-027: high-churn delayed range requests make bounded progress, not merely
//! avoid wrong-row data (D1 §§8, 12.1, 22.7, 31.4).
//!
//! A replica behind PX-025's delay relay asks for a window of the process
//! table with the standard `CLIENT_MODEL_RANGE_REQUEST`, while the table churns
//! (srtop's synthetic source replacing 2% of its rows every tick, or, on
//! Linux, the live `/proc` source watching PX-025's short-lived-process
//! workers), and asks again before the last answer could have come back, so
//! requests overlap. srtop publishes every row it holds and installs no range
//! provider, so the session answers such a request from the rows it already
//! pushed (`AlreadyCached`, nothing sent); what the request needs is that the
//! window's rows keep arriving. Each request converges when:
//!
//! 1. **progress**: the delayed replica has applied a revision committed after
//!    the request reached srtop — newer than every revision an undelayed
//!    replica of the same session had applied by then (the relay's one-way
//!    delay is exact, so that instant is known);
//! 2. **the requested rows**: every index of the window, clamped to the
//!    table's size at that revision, is held by the delayed replica;
//! 3. **no wrong row**: those rows are exactly the undelayed replica's rows at
//!    the same revision.
//!
//! Every request must converge within [`bound`], which is RTT plus two
//! refresh intervals plus 750 ms. A request still open past it fails the
//! test, whether it was starved (nothing newer ever arrived) or superseded
//! (newer requests or newer samples kept replacing what it waited for).

use srtop_demo::{relay::DelayRelay, wire::Replica};
use srui_process_explorer::{MODEL, TABLE};
use srui_protocol::{srui_message::Msg, ClientModelRangeRequest, SruiMessage};
use srui_semantic_tree::Value;
use std::{
    collections::VecDeque,
    fs,
    os::unix::{fs::PermissionsExt, net::UnixStream},
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
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
            "/tmp/srtop-px027r-{}-{nanos}-{}",
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
}

impl Drop for Srtop {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.directory);
    }
}

type Row = (u64, Value);

/// The table's rows by index: `None` for an index the replica does not hold.
fn rows(replica: &Replica) -> Vec<Option<Row>> {
    let Some(model) = replica.store.get_model(MODEL) else {
        return Vec::new();
    };
    (0..model.item_count())
        .map(|index| {
            model
                .get_item_by_index(index)
                .map(|item| (item.item_id.get(), item.value.clone()))
        })
        .collect()
}

/// The undelayed replica's history: when each revision arrived, and the rows at it.
#[derive(Default)]
struct History {
    arrivals: VecDeque<(Instant, u64)>,
    rows: VecDeque<(u64, Vec<Option<Row>>)>,
    error: Option<String>,
}

/// Revisions kept: at a 100 ms interval, well over the longest bound.
const HISTORY: usize = 512;

impl History {
    fn record(&mut self, at: Instant, revision: u64, rows: Vec<Option<Row>>) {
        self.arrivals.push_back((at, revision));
        self.rows.push_back((revision, rows));
        while self.arrivals.len() > HISTORY {
            self.arrivals.pop_front();
        }
        while self.rows.len() > HISTORY {
            self.rows.pop_front();
        }
    }

    /// The newest revision the undelayed replica had applied at `at`.
    fn revision_at(&self, at: Instant) -> Option<u64> {
        self.arrivals
            .iter()
            .take_while(|(arrived, _)| *arrived <= at)
            .map(|(_, revision)| *revision)
            .last()
    }

    fn rows_at(&self, revision: u64) -> Option<&Vec<Option<Row>>> {
        self.rows
            .iter()
            .find(|(at, _)| *at == revision)
            .map(|(_, rows)| rows)
    }
}

struct Request {
    sent: Instant,
    start: u64,
    count: u64,
    /// The newest revision srtop had certainly committed before the request arrived.
    floor: Option<u64>,
    converged: Option<(Duration, u64)>,
}

/// RTT + two refresh intervals + 750 ms: the request's own round trip, one
/// tick that may have just been taken without it and the one after, and room
/// for a scan and the scheduler.
fn bound(rtt: Duration, interval: Duration) -> Duration {
    rtt + interval * 2 + Duration::from_millis(750)
}

/// What one RTT run measured.
struct Outcome {
    latencies: Vec<Duration>,
    open: Vec<String>,
    rows_seen: (u64, u64),
    revisions: (u64, u64),
}

const REQUESTS: usize = 12;
const SPACING: Duration = Duration::from_millis(250);
const WINDOW: u64 = 16;

fn run(srtop: &Srtop, rtt: Duration, interval: Duration, label: &str) -> Outcome {
    let relay_socket = srtop.path(&format!("{label}-{}.sock", rtt.as_millis()));
    let mut relay = DelayRelay::start(&relay_socket, &srtop.socket(), rtt).unwrap();
    let one_way_up = relay.one_way().0;
    let history = Arc::new(Mutex::new(History::default()));
    let stop = Arc::new(AtomicBool::new(false));
    let mut direct = Replica::hello(
        &srtop.socket(),
        format!("px027-direct-{label}-{rtt:?}").as_bytes(),
    )
    .unwrap();
    {
        let mut held = history.lock().unwrap();
        held.record(Instant::now(), direct.revision(), rows(&direct));
    }
    let watcher = {
        let (history, stop) = (Arc::clone(&history), Arc::clone(&stop));
        thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                match direct.next_message() {
                    Ok(SruiMessage {
                        msg: Some(Msg::Transaction(_)),
                    }) => {
                        let at = Instant::now();
                        let snapshot = rows(&direct);
                        history
                            .lock()
                            .unwrap()
                            .record(at, direct.revision(), snapshot);
                    }
                    Ok(_) => {}
                    Err(error) => {
                        if !stop.load(Ordering::SeqCst) {
                            history.lock().unwrap().error = Some(error);
                        }
                        return;
                    }
                }
            }
        })
    };

    let mut delayed = Replica::hello(
        &relay_socket,
        format!("px027-delayed-{label}-{rtt:?}").as_bytes(),
    )
    .unwrap();
    let limit = bound(rtt, interval);
    let mut requests: Vec<Request> = Vec::new();
    let mut next_send = Instant::now();
    let mut rows_seen = (u64::MAX, 0);
    let first_revision = delayed.revision();
    let give_up = Instant::now() + SPACING * REQUESTS as u32 + limit + Duration::from_secs(5);
    loop {
        let now = Instant::now();
        if requests.len() < REQUESTS && now >= next_send {
            let size = delayed
                .store
                .get_model(MODEL)
                .map_or(0, |model| model.item_count());
            let count = WINDOW.min(size.max(1));
            // Head, middle and tail windows in turn: the tail moves most under churn.
            let start = match requests.len() % 3 {
                0 => 0,
                1 => size.saturating_sub(count) / 2,
                _ => size.saturating_sub(count),
            };
            delayed
                .send(&SruiMessage {
                    msg: Some(Msg::ClientModelRangeRequest(ClientModelRangeRequest {
                        node_id: TABLE.get(),
                        model_id: MODEL.get(),
                        start_index: start,
                        count,
                        observed_revision: delayed.revision(),
                    })),
                })
                .unwrap();
            requests.push(Request {
                sent: now,
                start,
                count,
                floor: None,
                converged: None,
            });
            // Overlapping: the next request goes before this one could be answered.
            next_send = now + SPACING;
        }
        if requests.len() == REQUESTS && requests.iter().all(|request| request.converged.is_some())
        {
            break;
        }
        if now >= give_up {
            break;
        }
        // A session that stops committing leaves this read waiting until the
        // replica's own read deadline: every open request is then reported.
        let message = match delayed.next_message() {
            Ok(message) => message,
            Err(error) => {
                println!("PX-027 range progress: {label} rtt {rtt:?}: the delayed replica stopped: {error}");
                break;
            }
        };
        if !matches!(message.msg, Some(Msg::Transaction(_))) {
            continue;
        }
        let arrived = Instant::now();
        let revision = delayed.revision();
        let held = rows(&delayed);
        rows_seen = (
            rows_seen.0.min(held.len() as u64),
            rows_seen.1.max(held.len() as u64),
        );
        let history = history.lock().unwrap();
        if let Some(error) = &history.error {
            panic!("the undelayed replica failed: {error}");
        }
        for request in requests
            .iter_mut()
            .filter(|request| request.converged.is_none())
        {
            // When the request reached srtop: the relay releases it RTT/2 after it was sent.
            let reached = request.sent + one_way_up;
            if request.floor.is_none() && arrived >= reached {
                request.floor = history.revision_at(reached);
            }
            let Some(floor) = request.floor else { continue };
            if revision <= floor {
                continue;
            }
            let end = (request.start + request.count).min(held.len() as u64);
            let window = request.start.min(end)..end;
            let complete = window.clone().all(|index| held[index as usize].is_some());
            let truth = history.rows_at(revision).unwrap_or_else(|| {
                panic!("the undelayed replica never applied revision {revision}")
            });
            let same = truth.len() == held.len()
                && window
                    .clone()
                    .all(|index| truth[index as usize] == held[index as usize]);
            assert!(
                same,
                "{label} rtt {rtt:?}: wrong rows for [{}, {}) at revision {revision}",
                request.start,
                request.start + request.count
            );
            if complete {
                request.converged = Some((arrived - request.sent, revision));
            }
        }
    }
    stop.store(true, Ordering::SeqCst);
    relay.stop();
    // The undelayed replica stops at its next commit, a tick away.
    watcher.join().unwrap();
    let open = requests
        .iter()
        .enumerate()
        .filter_map(|(index, request)| match request.converged {
            Some((latency, _)) if latency <= limit => None,
            Some((latency, revision)) => Some(format!(
                "request {index} [{}, {}) converged at revision {revision} only after {latency:?}, past the {limit:?} bound",
                request.start,
                request.start + request.count
            )),
            None => Some(format!(
                "request {index} [{}, {}) sent at revision floor {:?} never converged: starved or superseded",
                request.start,
                request.start + request.count,
                request.floor
            )),
        })
        .collect();
    Outcome {
        latencies: requests
            .iter()
            .filter_map(|request| request.converged.map(|(latency, _)| latency))
            .collect(),
        open,
        rows_seen,
        revisions: (first_revision, delayed.revision()),
    }
}

fn assert_bounded_progress(srtop: &Srtop, interval: Duration, label: &str, rtts_ms: &[u64]) {
    let mut failures = Vec::new();
    for rtt_ms in rtts_ms {
        let rtt = Duration::from_millis(*rtt_ms);
        let outcome = run(srtop, rtt, interval, label);
        let mut latencies = outcome.latencies.clone();
        latencies.sort();
        let max = latencies.last().copied().unwrap_or_default();
        println!(
            "PX-027 range progress evidence: {label} rtt={rtt_ms}ms interval={}ms requests={REQUESTS} window={WINDOW} converged={} bound={:?} min={:?} median={:?} max={max:?} rows={}..{} revisions {}->{}",
            interval.as_millis(),
            latencies.len(),
            bound(rtt, interval),
            latencies.first().copied().unwrap_or_default(),
            latencies.get(latencies.len() / 2).copied().unwrap_or_default(),
            outcome.rows_seen.0,
            outcome.rows_seen.1,
            outcome.revisions.0,
            outcome.revisions.1,
        );
        for latency in &outcome.latencies {
            // Never sooner than one RTT: the relay delays the commits too.
            if *latency < rtt {
                failures.push(format!(
                    "{label} rtt {rtt_ms}: converged after {latency:?}, sooner than one RTT"
                ));
            }
        }
        failures.extend(
            outcome
                .open
                .into_iter()
                .map(|open| format!("{label} rtt {rtt_ms}: {open}")),
        );
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The synthetic source: 2,000 rows, 2% of them replaced on every 100 ms
/// tick, so the table's rows churn on every commit. Runs everywhere.
#[test]
fn delayed_range_requests_converge_under_synthetic_churn() {
    let interval = Duration::from_millis(100);
    let srtop = Srtop::start(&["--scale-source", "2000", "--scale-interval-ms", "100"]);
    assert_bounded_progress(&srtop, interval, "synthetic", &[100, 300, 600]);
}

/// Live `/proc` on Linux, churned by PX-025's bounded short-lived-process
/// workers: two at a time, each starting a child every 20 ms that lives
/// 150 ms, replaced as they finish, for the whole run.
#[cfg(target_os = "linux")]
#[test]
fn delayed_range_requests_converge_under_live_short_lived_process_churn() {
    use srtop_demo::{
        harness::{fresh_owner, wait_until_none_live, DemoWorker},
        worker::{Mode, WorkerSpec},
    };
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_srtop-demo-worker"));
    let owner = fresh_owner();
    let stop = Arc::new(AtomicBool::new(false));
    let started = Arc::new(AtomicU64::new(0));
    let churn = {
        let (owner, stop, started) = (owner.clone(), Arc::clone(&stop), Arc::clone(&started));
        thread::spawn(move || {
            let spec = WorkerSpec {
                owner,
                mode: Mode::ShortLived {
                    count: 64,
                    lifetime: Duration::from_millis(150),
                    interval: Duration::from_millis(20),
                },
                // Its 64 children take 1.28 s; it ends soon after, and is replaced.
                duration: Duration::from_millis(1500),
                fail_after: None,
            };
            let mut running: Vec<DemoWorker> = Vec::new();
            while !stop.load(Ordering::SeqCst) {
                running.retain_mut(|worker| matches!(worker.try_status(), Ok(None)));
                while running.len() < 2 {
                    running.push(DemoWorker::spawn(&exe, &spec).expect("a bounded worker starts"));
                    started.fetch_add(1, Ordering::SeqCst);
                    thread::sleep(Duration::from_millis(400));
                }
                thread::sleep(Duration::from_millis(50));
            }
            for mut worker in running {
                let _ = worker.stop(Duration::from_secs(2));
            }
        })
    };
    thread::sleep(Duration::from_millis(800));
    let interval = Duration::from_millis(100);
    let srtop = Srtop::start(&[
        "--live-source",
        "--no-settings",
        "--refresh-interval-ms",
        "100",
    ]);
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        assert_bounded_progress(&srtop, interval, "live", &[100, 300, 600])
    }));
    stop.store(true, Ordering::SeqCst);
    churn.join().unwrap();
    let left = wait_until_none_live(&owner, Duration::from_secs(5)).unwrap();
    println!(
        "PX-027 live churn workers: {} bounded short-lived workers started; left running afterwards: {}",
        started.load(Ordering::SeqCst),
        left.len()
    );
    assert!(left.is_empty(), "workers outlived the run: {left:?}");
    if let Err(panic) = outcome {
        std::panic::resume_unwind(panic);
    }
}
