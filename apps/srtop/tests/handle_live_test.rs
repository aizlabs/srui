//! PX-029 live: real PID file descriptors (`pidfd_open`, `pidfd_send_signal`)
//! on this host's `/proc`, against disposable workers this test spawns and
//! reaps itself. No other process is ever signalled: every handle is acquired
//! from the key a live scan published for a worker's own PID, and the only
//! process a handle is opened on without being one of them is this test
//! process itself, which is never signalled.
//!
//! Everything runs in one test, in sequence, so this binary opens no
//! descriptor the count of `/proc/self/fd` does not account for.
#![cfg(target_os = "linux")]

use srui_process_explorer::handle::{AcquireError, HandleSignal, ProcessHandles, SignalOutcome};
use srui_process_explorer::procfs::ProcFsSource;
use srui_process_explorer::source::{CreationToken, Observed, ProcessKey, ProcessSource};
use std::io::Write as _;
use std::os::unix::process::ExitStatusExt;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

const SIGTERM: i32 = 15;
const EXHAUSTION_BODY: &str = "descriptor_exhaustion_body";
const EXHAUSTION_ENV: &str = "SRTOP_PX029_EXHAUSTION_BODY";
const EXHAUSTION_DONE: &str = "PX-029 descriptor exhaustion body ran";

/// A worker that sleeps until signalled.
fn sleeper() -> Child {
    Command::new("sleep")
        .arg("60")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("sleep starts")
}

/// A worker that exits on its own once its input is closed.
fn reader() -> Child {
    Command::new("cat")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("cat starts")
}

/// Closes a reader's input and reaps it.
fn finish(mut child: Child) {
    drop(child.stdin.take());
    let status = child.wait().expect("the worker is reaped");
    assert!(status.success(), "the worker exited on its own: {status:?}");
}

/// The key a live scan publishes for `pid`.
fn live_key(pid: u32) -> ProcessKey {
    let snapshot = ProcFsSource::live().snapshot();
    snapshot
        .records
        .into_iter()
        .map(|record| record.key)
        .find(|key| key.pid == Observed::Known(pid) && key.thread.is_none())
        .unwrap_or_else(|| panic!("the live scan lists PID {pid}"))
}

fn open_descriptors() -> usize {
    std::fs::read_dir("/proc/self/fd")
        .expect("/proc/self/fd is readable")
        .count()
}

fn expect_terminated(mut child: Child) {
    let status = child.wait().expect("the worker is reaped");
    assert_eq!(status.signal(), Some(SIGTERM), "{status:?}");
}

#[test]
fn live_pidfd_handles_on_owned_workers() {
    let mut handles = ProcessHandles::live();
    assert_eq!(
        handles.probe(),
        Ok(()),
        "pidfd_open is available on this kernel"
    );
    let before = open_descriptors();

    // The sampled instance is held and terminated through its handle.
    let worker = sleeper();
    let key = live_key(worker.id());
    let id = handles.acquire(&key).expect("a handle on the worker");
    assert_eq!(
        open_descriptors(),
        before + 1,
        "the handle is one descriptor"
    );
    assert_eq!(
        handles.settle(id, HandleSignal::Terminate),
        SignalOutcome::Sent
    );
    assert_eq!(open_descriptors(), before, "settlement closes it");
    expect_terminated(worker);

    // Exit after preparation: the handle's process is gone, and the signal
    // reaches nothing, whoever holds the PID by then.
    let worker = reader();
    let key = live_key(worker.id());
    let id = handles.acquire(&key).unwrap();
    finish(worker);
    assert_eq!(
        handles.settle(id, HandleSignal::Terminate),
        SignalOutcome::Ended
    );

    // Exit before acquisition: nothing is held.
    let worker = reader();
    let key = live_key(worker.id());
    finish(worker);
    let refused = handles.acquire(&key);
    assert!(
        matches!(refused, Err(AcquireError::Ended | AcquireError::Replaced)),
        "{refused:?}"
    );
    assert_eq!(handles.live_count(), 0);

    // A key whose start time is not the process's — what a scan of an
    // earlier process under the same PID would carry — is refused after the
    // real open, and the process under the PID receives nothing.
    let mut worker = sleeper();
    let key = live_key(worker.id());
    let CreationToken::LinuxBootTicks(ticks) = key.creation else {
        panic!("a live key carries a Linux start time");
    };
    let earlier = ProcessKey {
        creation: CreationToken::LinuxBootTicks(ticks.wrapping_sub(1)),
        ..key.clone()
    };
    assert_eq!(handles.acquire(&earlier), Err(AcquireError::Replaced));
    assert_eq!(open_descriptors(), before, "the refused handle is closed");
    std::thread::sleep(Duration::from_millis(50));
    assert!(
        worker.try_wait().unwrap().is_none(),
        "the worker received nothing"
    );
    let id = handles.acquire(&key).unwrap();
    assert_eq!(
        handles.settle(id, HandleSignal::Terminate),
        SignalOutcome::Sent
    );
    expect_terminated(worker);

    // Cancellation and expiry close real descriptors.
    let mut short = ProcessHandles::live().with_lifetime(Duration::from_millis(100));
    let first = sleeper();
    let second = sleeper();
    let (first_key, second_key) = (live_key(first.id()), live_key(second.id()));
    let a = short.acquire(&first_key).unwrap();
    let b = short.acquire(&second_key).unwrap();
    assert_eq!(open_descriptors(), before + 2);
    assert!(short.cancel(a));
    assert_eq!(open_descriptors(), before + 1);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(short.expire(), 1);
    assert!(!short.holds(b));
    assert_eq!(open_descriptors(), before);
    for (worker, key) in [(first, first_key), (second, second_key)] {
        let id = handles.acquire(&key).unwrap();
        assert_eq!(
            handles.settle(id, HandleSignal::Terminate),
            SignalOutcome::Sent
        );
        expect_terminated(worker);
    }
    drop(short);
    assert_eq!(handles.live_count(), 0);
    assert_eq!(open_descriptors(), before, "no descriptor leaked");

    // Descriptor exhaustion, in a child process whose own limit is lowered.
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", EXHAUSTION_BODY, "--ignored", "--nocapture"])
        .args(["--test-threads", "1"])
        .env(EXHAUSTION_ENV, std::process::id().to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("the exhaustion body runs");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success() && stdout.contains(EXHAUSTION_DONE),
        "{}\n{stdout}\n{}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    println!(
        "PX-029 live: kernel {}; handles opened, confirmed, signalled, refused and closed on owned workers; descriptors {before} before and after",
        std::fs::read_to_string("/proc/sys/kernel/osrelease")
            .unwrap_or_default()
            .trim()
    );
}

/// The body of the exhaustion check: with this process's descriptor limit
/// lowered and every descriptor under it taken, an acquisition is refused as
/// exhausted once, and succeeds again once one is free. The only process a
/// handle is opened on is this one, and nothing is ever sent. Ignored, and a
/// no-op unless its parent test started it.
#[test]
#[ignore = "the body of live_pidfd_handles_on_owned_workers' exhaustion check, run in a child process"]
fn descriptor_exhaustion_body() {
    let parent = std::env::var(EXHAUSTION_ENV).unwrap_or_default();
    if parent.parse::<u32>().ok() != Some(std::os::unix::process::parent_id()) {
        return;
    }
    let key = live_key(std::process::id());
    let mut handles = ProcessHandles::live();
    let id = handles.acquire(&key).expect("a handle on this process");
    assert!(handles.cancel(id));

    let highest = std::fs::read_dir("/proc/self/fd")
        .unwrap()
        .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse::<u64>().ok())
        .max()
        .unwrap();
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `limit` is a valid `rlimit` for the kernel to fill.
    assert_eq!(
        unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) },
        0
    );
    let lowered = libc::rlimit {
        rlim_cur: (highest + 8).min(limit.rlim_max),
        rlim_max: limit.rlim_max,
    };
    // SAFETY: `lowered` is a valid `rlimit`; only this process's soft limit
    // changes.
    assert_eq!(unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &lowered) }, 0);
    let mut filler = Vec::new();
    let exhausted = loop {
        match std::fs::File::open("/dev/null") {
            Ok(file) if filler.len() < 64 => filler.push(file),
            Ok(_) => panic!("64 descriptors under a lowered limit still opened"),
            Err(error) => break error,
        }
    };
    assert_eq!(exhausted.raw_os_error(), Some(libc::EMFILE));
    assert_eq!(
        handles.acquire(&key),
        Err(AcquireError::DescriptorsExhausted { system_wide: false })
    );
    assert_eq!(handles.live_count(), 0);
    drop(filler);
    let id = handles
        .acquire(&key)
        .expect("a handle once descriptors are free");
    assert!(handles.cancel(id));
    let mut out = std::io::stdout();
    writeln!(out, "{EXHAUSTION_DONE}").unwrap();
}
