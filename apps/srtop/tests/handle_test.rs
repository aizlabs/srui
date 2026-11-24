//! PX-029: identity-checked, bounded-lifetime process handles, through a
//! deterministic fake of the kernel's PID file descriptors and `/proc`. The
//! fake numbers each process instance (an *incarnation*), binds a descriptor to
//! the incarnation that held the PID when it was opened, delivers a signal
//! only to a live incarnation, and counts every descriptor it opens and
//! closes, so exit and PID reuse can be injected at every point a real kernel
//! allows: before the handle is acquired, between its open and the identity
//! re-check, and after it was prepared.

use srui_process_explorer::handle::{
    AcquireError, HandleSignal, Pidfd, PidfdSystem, ProcessHandles, SignalOutcome, Unsupported,
};
use srui_process_explorer::on_demand::{MonotonicClock, SourceTime};
use srui_process_explorer::source::{
    BootId, CreationToken, HostId, MissingReason, Observed, PidNamespaceId, ProcessKey, SourceId,
    ThreadIdentity,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const BOOT: &str = "7d5b3f9e-boot";
const NAMESPACE: u64 = 4_026_531_836;
const LIVE_SOURCE: &str = "procfs:/proc";
const LIFETIME: Duration = Duration::from_secs(10);

const EPERM: i32 = 1;
const ENOENT: i32 = 2;
const ENODEV: i32 = 19;
const ENFILE: i32 = 23;
const EMFILE: i32 = 24;

#[derive(Debug, Default)]
struct ManualClock(AtomicU64);

impl ManualClock {
    fn advance(&self, by: Duration) {
        self.0.fetch_add(by.as_nanos() as u64, Ordering::SeqCst);
    }
}

impl MonotonicClock for ManualClock {
    fn now(&self) -> SourceTime {
        SourceTime(Duration::from_nanos(self.0.load(Ordering::SeqCst)))
    }
}

/// What the fake kernel does right after the next successful open, before
/// the handle's identity is re-checked.
#[derive(Debug, Clone, Copy)]
enum AfterOpen {
    /// The PID's process exits and is reaped.
    Exit(u32),
    /// The PID's process exits, is reaped, and a newcomer started at the
    /// given tick takes the PID.
    Reuse(u32, u64),
}

#[derive(Debug, Clone, Copy)]
struct Process {
    incarnation: u64,
    start_ticks: u64,
}

#[derive(Debug, Default)]
struct State {
    available: bool,
    boot: String,
    namespace: u64,
    next_incarnation: u64,
    /// The unreaped process holding each PID.
    by_pid: HashMap<u32, Process>,
    reaped: HashSet<u64>,
    delivered: Vec<(u64, HandleSignal)>,
    open_attempts: usize,
    open_descriptors: usize,
    closed: usize,
    open_errors: VecDeque<io::Error>,
    send_errors: VecDeque<io::Error>,
    stat_errors: VecDeque<io::Error>,
    garbled_stat: bool,
    after_open: Option<AfterOpen>,
}

#[derive(Debug)]
struct Kernel(Mutex<State>);

impl Kernel {
    fn new() -> Arc<Self> {
        Arc::new(Self(Mutex::new(State {
            available: true,
            boot: BOOT.into(),
            namespace: NAMESPACE,
            next_incarnation: 1,
            ..State::default()
        })))
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.0.lock().unwrap()
    }

    /// Starts a process under `pid`; its incarnation.
    fn start(&self, pid: u32, start_ticks: u64) -> u64 {
        start(&mut self.state(), pid, start_ticks)
    }

    /// The process holding `pid` exits and is reaped.
    fn reap(&self, pid: u32) {
        reap(&mut self.state(), pid);
    }

    /// `pid`'s process is reaped and a newcomer takes the PID; its incarnation.
    fn reuse(&self, pid: u32, start_ticks: u64) -> u64 {
        let mut state = self.state();
        reap(&mut state, pid);
        start(&mut state, pid, start_ticks)
    }

    fn delivered_to(&self, incarnation: u64) -> Vec<HandleSignal> {
        self.state()
            .delivered
            .iter()
            .filter(|(to, _)| *to == incarnation)
            .map(|(_, signal)| *signal)
            .collect()
    }

    fn deliveries(&self) -> usize {
        self.state().delivered.len()
    }

    fn open_descriptors(&self) -> usize {
        self.state().open_descriptors
    }

    fn open_attempts(&self) -> usize {
        self.state().open_attempts
    }
}

fn start(state: &mut State, pid: u32, start_ticks: u64) -> u64 {
    let incarnation = state.next_incarnation;
    state.next_incarnation += 1;
    let previous = state.by_pid.insert(
        pid,
        Process {
            incarnation,
            start_ticks,
        },
    );
    assert!(previous.is_none(), "a PID is held by one process at a time");
    incarnation
}

fn reap(state: &mut State, pid: u32) {
    let process = state.by_pid.remove(&pid).expect("a process holds the PID");
    state.reaped.insert(process.incarnation);
}

#[derive(Debug)]
struct FakeSystem(Arc<Kernel>);

#[derive(Debug)]
struct FakePidfd {
    kernel: Arc<Kernel>,
    incarnation: u64,
}

impl Drop for FakePidfd {
    fn drop(&mut self) {
        let mut state = self.kernel.state();
        state.open_descriptors -= 1;
        state.closed += 1;
    }
}

impl Pidfd for FakePidfd {
    fn send(&self, signal: HandleSignal) -> io::Result<()> {
        let mut state = self.kernel.state();
        if let Some(error) = state.send_errors.pop_front() {
            return Err(error);
        }
        if state.reaped.contains(&self.incarnation) {
            return Err(io::Error::from_raw_os_error(3));
        }
        state.delivered.push((self.incarnation, signal));
        Ok(())
    }
}

impl PidfdSystem for FakeSystem {
    fn available(&self) -> bool {
        self.0.state().available
    }

    fn open(&self, pid: u32) -> io::Result<Box<dyn Pidfd>> {
        let mut state = self.0.state();
        state.open_attempts += 1;
        if let Some(error) = state.open_errors.pop_front() {
            return Err(error);
        }
        let Some(process) = state.by_pid.get(&pid).copied() else {
            return Err(io::Error::from_raw_os_error(3));
        };
        state.open_descriptors += 1;
        match state.after_open.take() {
            Some(AfterOpen::Exit(pid)) => reap(&mut state, pid),
            Some(AfterOpen::Reuse(pid, ticks)) => {
                reap(&mut state, pid);
                start(&mut state, pid, ticks);
            }
            None => {}
        }
        Ok(Box::new(FakePidfd {
            kernel: Arc::clone(&self.0),
            incarnation: process.incarnation,
        }))
    }

    fn read_stat(&self, pid: u32) -> io::Result<Vec<u8>> {
        let mut state = self.0.state();
        if let Some(error) = state.stat_errors.pop_front() {
            return Err(error);
        }
        if std::mem::take(&mut state.garbled_stat) {
            return Ok(format!("{pid} (worker").into_bytes());
        }
        match state.by_pid.get(&pid) {
            Some(process) => Ok(format!(
                "{pid} (worker) S 1 {pid} {pid} 0 -1 4194560 0 0 0 0 3 1 0 0 20 0 1 0 {} 1000 200\n",
                process.start_ticks
            )
            .into_bytes()),
            None => Err(io::Error::from_raw_os_error(ENOENT)),
        }
    }

    fn boot_id(&self) -> io::Result<String> {
        Ok(self.0.state().boot.clone())
    }

    fn pid_namespace(&self) -> io::Result<u64> {
        Ok(self.0.state().namespace)
    }
}

fn key(pid: u32, start_ticks: u64) -> ProcessKey {
    ProcessKey {
        source: SourceId(LIVE_SOURCE.into()),
        host: Observed::Known(HostId("box".into())),
        boot: Observed::Known(BootId(BOOT.into())),
        pid_namespace: Observed::Known(PidNamespaceId(NAMESPACE)),
        pid: Observed::Known(pid),
        creation: CreationToken::LinuxBootTicks(start_ticks),
        thread: None,
    }
}

struct Harness {
    kernel: Arc<Kernel>,
    clock: Arc<ManualClock>,
    handles: ProcessHandles,
}

fn harness() -> Harness {
    let kernel = Kernel::new();
    let clock = Arc::new(ManualClock::default());
    let handles = ProcessHandles::new(
        Arc::new(FakeSystem(Arc::clone(&kernel))),
        Arc::clone(&clock) as Arc<dyn MonotonicClock>,
        SourceId(LIVE_SOURCE.into()),
    )
    .with_lifetime(LIFETIME);
    Harness {
        kernel,
        clock,
        handles,
    }
}

#[test]
fn a_handle_signals_exactly_the_sampled_instance_once_and_is_closed() {
    let mut h = harness();
    let worker = h.kernel.start(4101, 500);
    let id = h
        .handles
        .acquire(&key(4101, 500))
        .expect("the sampled instance");
    assert_eq!(h.handles.live_count(), 1);
    assert_eq!(h.kernel.open_descriptors(), 1);
    assert_eq!(h.handles.key_of(id), Some(&key(4101, 500)));
    assert_eq!(h.handles.expires_at(id), Some(LIFETIME));
    assert_eq!(
        h.handles.settle(id, HandleSignal::Terminate),
        SignalOutcome::Sent
    );
    assert_eq!(h.kernel.delivered_to(worker), vec![HandleSignal::Terminate]);
    assert_eq!(h.kernel.open_descriptors(), 0, "settlement closes it");
    assert!(!h.handles.holds(id));
    assert_eq!(
        h.handles.settle(id, HandleSignal::Terminate),
        SignalOutcome::NoHandle,
        "a handle settles one request"
    );
    assert_eq!(h.kernel.deliveries(), 1);
}

#[test]
fn an_exit_before_acquisition_holds_nothing() {
    let mut h = harness();
    h.kernel.start(4101, 500);
    h.kernel.reap(4101);
    assert_eq!(h.handles.acquire(&key(4101, 500)), Err(AcquireError::Ended));
    assert_eq!(h.kernel.open_descriptors(), 0);
    assert_eq!(h.handles.live_count(), 0);
}

#[test]
fn a_pid_reused_before_acquisition_is_refused_and_the_replacement_receives_nothing() {
    let mut h = harness();
    let sampled = h.kernel.start(4101, 500);
    let replacement = h.kernel.reuse(4101, 900);
    // The open reaches the replacement, the only process under the PID; the
    // re-read after it names the replacement's start time.
    assert_eq!(
        h.handles.acquire(&key(4101, 500)),
        Err(AcquireError::Replaced)
    );
    assert_eq!(h.kernel.open_attempts(), 1);
    assert_eq!(
        h.kernel.open_descriptors(),
        0,
        "the replacement's handle is closed"
    );
    assert_eq!(h.handles.live_count(), 0);
    // Neither a start time one tick off nor the same PID alone is the key.
    assert_eq!(
        h.handles.acquire(&key(4101, 899)),
        Err(AcquireError::Replaced)
    );
    assert_eq!(
        h.handles.acquire(&key(4101, 901)),
        Err(AcquireError::Replaced)
    );
    assert!(h.kernel.delivered_to(replacement).is_empty());
    assert!(h.kernel.delivered_to(sampled).is_empty());
    // The replacement's own key is a handle on the replacement.
    let id = h.handles.acquire(&key(4101, 900)).unwrap();
    assert_eq!(
        h.handles.settle(id, HandleSignal::Terminate),
        SignalOutcome::Sent
    );
    assert_eq!(
        h.kernel.delivered_to(replacement),
        vec![HandleSignal::Terminate]
    );
}

#[test]
fn an_exit_or_reuse_between_the_open_and_the_identity_check_is_refused() {
    let mut h = harness();
    let sampled = h.kernel.start(4101, 500);
    h.kernel.state().after_open = Some(AfterOpen::Reuse(4101, 900));
    assert_eq!(
        h.handles.acquire(&key(4101, 500)),
        Err(AcquireError::Replaced)
    );
    assert_eq!(h.kernel.open_descriptors(), 0);
    assert!(h.kernel.delivered_to(sampled).is_empty());

    let mut h = harness();
    h.kernel.start(4101, 500);
    h.kernel.state().after_open = Some(AfterOpen::Exit(4101));
    assert_eq!(h.handles.acquire(&key(4101, 500)), Err(AcquireError::Ended));
    assert_eq!(h.kernel.open_descriptors(), 0);
}

#[test]
fn an_exit_or_pid_reuse_after_preparation_never_reaches_the_replacement() {
    let mut h = harness();
    let sampled = h.kernel.start(4101, 500);
    let id = h.handles.acquire(&key(4101, 500)).unwrap();
    // Between preparation and confirmation the process exits, is reaped,
    // and a newcomer takes its PID.
    let replacement = h.kernel.reuse(4101, 900);
    assert_eq!(
        h.handles.settle(id, HandleSignal::Terminate),
        SignalOutcome::Ended
    );
    assert!(h.kernel.delivered_to(replacement).is_empty());
    assert!(h.kernel.delivered_to(sampled).is_empty());
    assert_eq!(h.kernel.open_descriptors(), 0);
    assert_eq!(h.kernel.open_attempts(), 1, "nothing is reopened by PID");

    // An exit with no newcomer is the same answer.
    let mut h = harness();
    h.kernel.start(4102, 700);
    let id = h.handles.acquire(&key(4102, 700)).unwrap();
    h.kernel.reap(4102);
    assert_eq!(
        h.handles.settle(id, HandleSignal::Terminate),
        SignalOutcome::Ended
    );
    assert_eq!(h.kernel.deliveries(), 0);
    assert_eq!(h.kernel.open_descriptors(), 0);
}

#[test]
fn a_key_that_cannot_name_a_live_process_here_opens_nothing() {
    let thread = ProcessKey {
        thread: Some(ThreadIdentity {
            tid: 4102,
            creation: CreationToken::LinuxBootTicks(510),
        }),
        ..key(4101, 500)
    };
    type Expected = fn(&AcquireError) -> bool;
    let cases: Vec<(&str, ProcessKey, Expected)> = vec![
        (
            "another source",
            ProcessKey {
                source: SourceId("procfs:/host/proc".into()),
                ..key(4101, 500)
            },
            |e| matches!(e, AcquireError::Unidentifiable(_)),
        ),
        ("a thread", thread, |e| {
            matches!(e, AcquireError::Unidentifiable(_))
        }),
        (
            "no PID",
            ProcessKey {
                pid: Observed::Missing(MissingReason::Denied),
                ..key(4101, 500)
            },
            |e| matches!(e, AcquireError::Unidentifiable(_)),
        ),
        (
            "no Linux start time",
            ProcessKey {
                creation: CreationToken::Opaque("fake-4101".into()),
                ..key(4101, 500)
            },
            |e| matches!(e, AcquireError::Unidentifiable(_)),
        ),
        (
            "no boot",
            ProcessKey {
                boot: Observed::Missing(MissingReason::Unavailable),
                ..key(4101, 500)
            },
            |e| matches!(e, AcquireError::Unidentifiable(_)),
        ),
        (
            "no PID namespace",
            ProcessKey {
                pid_namespace: Observed::Missing(MissingReason::Unavailable),
                ..key(4101, 500)
            },
            |e| matches!(e, AcquireError::Unidentifiable(_)),
        ),
        (
            "another boot",
            ProcessKey {
                boot: Observed::Known(BootId("an-earlier-boot".into())),
                ..key(4101, 500)
            },
            |e| *e == AcquireError::OtherBoot,
        ),
        (
            "another PID namespace",
            ProcessKey {
                pid_namespace: Observed::Known(PidNamespaceId(NAMESPACE + 1)),
                ..key(4101, 500)
            },
            |e| *e == AcquireError::OtherNamespace,
        ),
    ];
    for (name, key, expected) in cases {
        let mut h = harness();
        h.kernel.start(4101, 500);
        let result = h.handles.acquire(&key);
        assert!(result.as_ref().is_err_and(expected), "{name}: {result:?}");
        assert_eq!(h.kernel.open_attempts(), 0, "{name}: nothing is opened");
    }
}

#[test]
fn a_record_that_cannot_confirm_the_identity_after_the_open_fails_closed() {
    let mut h = harness();
    h.kernel.start(4101, 500);
    h.kernel
        .state()
        .stat_errors
        .push_back(io::Error::from_raw_os_error(EPERM));
    assert!(matches!(
        h.handles.acquire(&key(4101, 500)),
        Err(AcquireError::Unverified(_))
    ));
    assert_eq!(h.kernel.open_descriptors(), 0);
    h.kernel.state().garbled_stat = true;
    assert!(matches!(
        h.handles.acquire(&key(4101, 500)),
        Err(AcquireError::Unverified(_))
    ));
    assert_eq!(h.kernel.open_descriptors(), 0);
    assert_eq!(h.handles.live_count(), 0);
}

#[test]
fn permission_denial_at_the_signal_is_reported_and_closes_the_handle() {
    let mut h = harness();
    h.kernel.start(4101, 500);
    let id = h.handles.acquire(&key(4101, 500)).unwrap();
    h.kernel
        .state()
        .send_errors
        .push_back(io::Error::from_raw_os_error(EPERM));
    assert_eq!(
        h.handles.settle(id, HandleSignal::Terminate),
        SignalOutcome::Denied
    );
    assert_eq!(h.kernel.deliveries(), 0);
    assert_eq!(h.kernel.open_descriptors(), 0);
    assert_eq!(
        h.handles.settle(id, HandleSignal::Terminate),
        SignalOutcome::NoHandle,
        "a denied request is settled, not retried"
    );
}

#[test]
fn an_unsupported_api_is_refused_and_nothing_is_signalled() {
    // No PID file descriptors on this system at all.
    let mut h = harness();
    h.kernel.start(4101, 500);
    h.kernel.state().available = false;
    assert_eq!(
        h.handles.probe(),
        Err(AcquireError::Unsupported(Unsupported::Platform))
    );
    assert_eq!(
        h.handles.acquire(&key(4101, 500)),
        Err(AcquireError::Unsupported(Unsupported::Platform))
    );
    assert_eq!(h.kernel.open_attempts(), 0);

    // ENOSYS, a filter's EPERM, ENODEV from pidfd_open: each once, never retried.
    for (error, expected) in [
        (
            io::Error::from(io::ErrorKind::Unsupported),
            Unsupported::NoSystemCall,
        ),
        (io::Error::from_raw_os_error(EPERM), Unsupported::Refused),
        (
            io::Error::from_raw_os_error(ENODEV),
            Unsupported::NoAnonymousInodes,
        ),
    ] {
        let mut h = harness();
        h.kernel.start(4101, 500);
        h.kernel.state().open_errors.push_back(error);
        assert_eq!(
            h.handles.acquire(&key(4101, 500)),
            Err(AcquireError::Unsupported(expected))
        );
        assert_eq!(h.kernel.open_attempts(), 1, "{expected:?}");
        assert_eq!(h.kernel.open_descriptors(), 0);
        assert_eq!(h.kernel.deliveries(), 0, "{expected:?}: no fallback signal");
    }

    // The probe opens a handle on this process only, sends nothing, closes it.
    let h = harness();
    h.kernel.start(std::process::id(), 1);
    assert_eq!(h.handles.probe(), Ok(()));
    assert_eq!(h.kernel.open_descriptors(), 0);
    h.kernel
        .state()
        .open_errors
        .push_back(io::Error::from(io::ErrorKind::Unsupported));
    assert_eq!(
        h.handles.probe(),
        Err(AcquireError::Unsupported(Unsupported::NoSystemCall))
    );
    assert_eq!(h.kernel.deliveries(), 0);

    // ENOSYS from pidfd_send_signal: settled unsent, closed.
    let mut h = harness();
    h.kernel.start(4101, 500);
    let id = h.handles.acquire(&key(4101, 500)).unwrap();
    h.kernel
        .state()
        .send_errors
        .push_back(io::Error::from(io::ErrorKind::Unsupported));
    assert_eq!(
        h.handles.settle(id, HandleSignal::Terminate),
        SignalOutcome::Unsupported(Unsupported::NoSystemCall)
    );
    assert_eq!(h.kernel.deliveries(), 0);
    assert_eq!(h.kernel.open_descriptors(), 0);
    assert_eq!(h.kernel.open_attempts(), 1);
}

#[test]
fn descriptor_exhaustion_is_reported_once_and_never_retried() {
    for (errno, system_wide) in [(EMFILE, false), (ENFILE, true)] {
        let mut h = harness();
        h.kernel.start(4101, 500);
        h.kernel
            .state()
            .open_errors
            .push_back(io::Error::from_raw_os_error(errno));
        assert_eq!(
            h.handles.acquire(&key(4101, 500)),
            Err(AcquireError::DescriptorsExhausted { system_wide })
        );
        assert_eq!(h.kernel.open_attempts(), 1, "exhaustion is not retried");
        assert_eq!(h.handles.live_count(), 0);
        assert_eq!(h.kernel.open_descriptors(), 0);
        // Nothing is left stuck: once a descriptor is free, a handle opens.
        let id = h.handles.acquire(&key(4101, 500)).unwrap();
        assert_eq!(h.kernel.open_attempts(), 2);
        assert!(h.handles.cancel(id));
        assert_eq!(h.kernel.open_descriptors(), 0);
    }
}

#[test]
fn handles_close_on_cancel_expiry_settlement_and_drop_and_are_bounded() {
    let mut h = harness();
    h.handles = ProcessHandles::new(
        Arc::new(FakeSystem(Arc::clone(&h.kernel))),
        Arc::clone(&h.clock) as Arc<dyn MonotonicClock>,
        SourceId(LIVE_SOURCE.into()),
    )
    .with_lifetime(LIFETIME)
    .with_limit(2);
    for (pid, ticks) in [(4101, 500), (4102, 600), (4103, 700), (4104, 800)] {
        h.kernel.start(pid, ticks);
    }
    let a = h.handles.acquire(&key(4101, 500)).unwrap();
    let b = h.handles.acquire(&key(4102, 600)).unwrap();
    assert_eq!(h.kernel.open_descriptors(), 2);
    // Bounded: a third is refused before anything is opened.
    assert_eq!(
        h.handles.acquire(&key(4103, 700)),
        Err(AcquireError::LimitReached { limit: 2 })
    );
    assert_eq!(h.kernel.open_attempts(), 2);
    // Cancellation closes one.
    assert!(h.handles.cancel(a));
    assert!(!h.handles.cancel(a));
    assert_eq!(h.kernel.open_descriptors(), 1);
    assert_eq!(
        h.handles.settle(a, HandleSignal::Terminate),
        SignalOutcome::NoHandle
    );
    let c = h.handles.acquire(&key(4103, 700)).unwrap();
    assert_eq!(h.kernel.open_descriptors(), 2);
    // Expiry closes both at their lifetime, not a nanosecond before.
    h.clock.advance(LIFETIME - Duration::from_nanos(1));
    assert_eq!(h.handles.expire(), 0);
    assert_eq!(h.kernel.open_descriptors(), 2);
    h.clock.advance(Duration::from_nanos(1));
    assert_eq!(h.handles.expire(), 2);
    assert_eq!(h.kernel.open_descriptors(), 0);
    assert!(!h.handles.holds(b) && !h.handles.holds(c));
    assert_eq!(h.kernel.deliveries(), 0, "expiry sends nothing");

    // An expired handle is closed at settlement, unsent; one a nanosecond
    // younger is used.
    let d = h.handles.acquire(&key(4104, 800)).unwrap();
    h.clock.advance(LIFETIME);
    assert_eq!(
        h.handles.settle(d, HandleSignal::Terminate),
        SignalOutcome::Expired
    );
    assert_eq!(h.kernel.deliveries(), 0);
    assert_eq!(h.kernel.open_descriptors(), 0);
    let e = h.handles.acquire(&key(4104, 800)).unwrap();
    h.clock.advance(LIFETIME - Duration::from_nanos(1));
    assert_eq!(
        h.handles.settle(e, HandleSignal::Terminate),
        SignalOutcome::Sent
    );
    assert_eq!(h.kernel.open_descriptors(), 0);

    // An acquisition first closes what has expired, so expired handles never
    // hold the bound.
    let f = h.handles.acquire(&key(4101, 500)).unwrap();
    let g = h.handles.acquire(&key(4102, 600)).unwrap();
    h.clock.advance(LIFETIME);
    let i = h.handles.acquire(&key(4103, 700)).unwrap();
    assert!(!h.handles.holds(f) && !h.handles.holds(g) && h.handles.holds(i));
    assert_eq!(h.kernel.open_descriptors(), 1);

    // Dropping the table closes everything it holds.
    let _j = h.handles.acquire(&key(4104, 800)).unwrap();
    assert_eq!(h.kernel.open_descriptors(), 2);
    drop(h.handles);
    assert_eq!(h.kernel.open_descriptors(), 0);
    let state = h.kernel.state();
    assert_eq!(
        state.closed, state.open_attempts,
        "every descriptor opened was closed"
    );
}

/// Off Linux the live handles are unsupported and open nothing.
#[cfg(not(target_os = "linux"))]
#[test]
fn the_live_handles_are_unsupported_off_linux() {
    use srui_process_explorer::procfs::ProcFsSource;
    let mut handles = ProcessHandles::live();
    assert_eq!(
        handles.probe(),
        Err(AcquireError::Unsupported(Unsupported::Platform))
    );
    let live = ProcessKey {
        source: ProcFsSource::live().source_id().clone(),
        ..key(std::process::id(), 1)
    };
    assert_eq!(
        handles.acquire(&live),
        Err(AcquireError::Unsupported(Unsupported::Platform))
    );
    assert_eq!(handles.live_count(), 0);
}

/// The kernel's `ENOSYS` is the error kind the handles read as unsupported.
#[cfg(target_os = "linux")]
#[test]
fn enosys_is_read_as_unsupported() {
    assert_eq!(
        io::Error::from_raw_os_error(libc::ENOSYS).kind(),
        io::ErrorKind::Unsupported
    );
}
