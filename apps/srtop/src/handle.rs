//! Identity-checked, bounded-lifetime process handles for signaling (D1 §27
//! server security controls, §6.2 identity that is never reused; D2 T32; K1
//! `/proc/<pid>/stat`, K2 `pidfd_send_signal(2)`, K3 `pidfd_open(2)`; PX-029).
//!
//! A PID is not a process instance: the kernel hands a freed PID to the next
//! process it starts. Signaling by PID (`kill(2)`) therefore has a window
//! between deciding on a target and the signal in which the target can exit
//! and a newcomer take its number. This module closes that window for signals
//! and nothing else:
//!
//! * **A handle is a PID file descriptor.** [`ProcessHandles::acquire`] opens
//!   one with `pidfd_open(pid, 0)` (Linux 5.3, K3). The descriptor refers to
//!   the *process* the PID named at that call, not to the number: once that
//!   process has been reaped, `pidfd_send_signal` on it fails with `ESRCH`
//!   (K2), whoever holds the number by then.
//! * **The handle is checked against the sampled [`ProcessKey`] after it is
//!   open.** The key must come from the live `/proc` source, name a process
//!   (not a thread) with a known PID, boot and PID namespace, and carry a
//!   Linux start time ([`CreationToken::LinuxBootTicks`]). The boot and the
//!   reader's own PID namespace (the one `pidfd_open` interprets the PID in)
//!   must be the key's. Then, *after* the descriptor is open,
//!   `/proc/<pid>/stat` is read again and its start time (field 22, K1) must
//!   equal the key's. Why that proves the descriptor is the key's process: the
//!   sampled process held the PID at the scan, before the open; if the open had
//!   reached a replacement, the sampled process had already been reaped, and a
//!   process never gets its PID back, so the record read *after* the open
//!   would name the replacement's start time (or none). A match after the open
//!   therefore means no replacement was in between. A mismatch, an exit, or a
//!   record that cannot be read or parsed closes the descriptor and refuses
//!   the handle; nothing is guessed. The one residue is a replacement started
//!   in the same clock tick (10 ms at `USER_HZ` 100) as the sampled process,
//!   under the same PID, which the start time cannot tell apart.
//! * **Bounded lifetime and count.** A handle lives in one [`ProcessHandles`]
//!   table, at most [`DEFAULT_MAX_LIVE_HANDLES`] at once, each for at most
//!   [`DEFAULT_HANDLE_LIFETIME`] on a suspend-aware monotonic clock. It is
//!   closed on cancellation ([`ProcessHandles::cancel`]), on expiry
//!   ([`ProcessHandles::expire`], and before any acquisition or signal), and
//!   on settlement ([`ProcessHandles::settle`], which is single-use: the
//!   descriptor is closed after its one signal, whatever the result), and when
//!   the table is dropped. Descriptors are owned (`OwnedFd`), so every path
//!   out of this module closes them.
//! * **No fallback.** Where `pidfd_open` or `pidfd_send_signal` is missing or
//!   refused — not Linux, `ENOSYS` (a kernel before 5.3, or a filter that
//!   answers `ENOSYS`), `EPERM`/`EACCES` from `pidfd_open` (which has no
//!   permission check of its own, so a seccomp filter or security module
//!   refused it), `ENODEV` — the answer is [`Unsupported`], and there is no
//!   PID-addressed path to fall back to: this module names no `kill`, and
//!   PX-008's audit refuses one anywhere in production code. Descriptor
//!   exhaustion (`EMFILE`, `ENFILE`) is reported once and never retried.
//!
//! **Scope.** A held PID file descriptor makes *signals* race-free and
//! nothing else. Every other PID-addressed operation — `setpriority`,
//! `sched_setaffinity`, `ioprio_set`, `prlimit`, reading or writing
//! `/proc/<pid>/...` by path — still names a number, and holding a descriptor
//! does not make that number stable. This module offers no such operation and
//! no PID-based path: each later action family needs its own reviewed
//! targeting strategy (PX-036..PX-040).
//!
//! **Not wired.** No production action path reaches this module yet: the only
//! executor is still [`crate::actions::DryRunExecutor`], and PX-008's audit
//! refuses any production reference to this module outside it. PX-030 wires
//! graceful termination through it, and must use **exactly one** table:
//! a [`HandleId`] is minted per table and [`ProcessHandles::settle`] takes no
//! key, so an ID from one table could settle another table's handle, and
//! PX-030 must also check [`ProcessHandles::key_of`] against the intent's
//! target key before settling.
//!
//! **Minimum supported path.** Linux 5.3 or later on the architectures the
//! `libc` crate (0.2.189, the version already locked by the runtime's
//! `srui-unix-security`) defines `SYS_pidfd_open` and `SYS_pidfd_send_signal`
//! for. Both calls are made through `syscall(2)`, so no C library wrapper is
//! needed (glibc only added `pidfd_open` and `pidfd_send_signal` wrappers in
//! 2.36). `pidfd_send_signal` (5.1) is older than `pidfd_open` (5.3), so a
//! kernel that opens a handle can signal through it. No flag is used: neither
//! `PIDFD_NONBLOCK` (5.10) nor `PIDFD_THREAD` (6.9).

use crate::on_demand::{MonotonicClock, SuspendAwareClock};
use crate::procfs::{ended, parse_namespace, parse_stat, read_bounded, DEFAULT_PROC_ROOT};
use crate::source::{BootId, CreationToken, Observed, PidNamespaceId, ProcessKey, SourceId};
use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// How long a handle may be held before it is closed unused: the same 30 s an
/// intent can be confirmed for (PX-028's `DEFAULT_INTENT_LIFETIME`).
pub const DEFAULT_HANDLE_LIFETIME: Duration = crate::actions::DEFAULT_INTENT_LIFETIME;
/// The most handles one table holds at once.
pub const DEFAULT_MAX_LIVE_HANDLES: usize = 16;
/// The first Linux release with `pidfd_open(2)` (K3).
pub const MIN_KERNEL_PIDFD_OPEN: &str = "5.3";
/// The first Linux release with `pidfd_send_signal(2)` (K2).
pub const MIN_KERNEL_PIDFD_SEND_SIGNAL: &str = "5.1";

/// `ESRCH`, `ENODEV`, `ENFILE` and `EMFILE`, matched by number because
/// `io::ErrorKind` has no stable variant for them. They are the same on every
/// Linux architecture (`errno-base.h`) and on macOS, where only the fakes in
/// the tests produce them.
const ESRCH: i32 = 3;
const ENODEV: i32 = 19;
const ENFILE: i32 = 23;
const EMFILE: i32 = 24;

/// The signals a handle can send: a closed set, so no caller can name an
/// arbitrary signal number. PX-030 sends [`HandleSignal::Terminate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HandleSignal {
    /// `SIGTERM`: a request to terminate, which the process may handle.
    Terminate,
}

impl HandleSignal {
    /// The signal number sent for this signal: `SIGTERM` (15) for
    /// [`HandleSignal::Terminate`]. The one mapping; PX-008's audit pins it
    /// token for token, and it is the only `libc::SIG*` this module names.
    pub fn number(self) -> i32 {
        match self {
            Self::Terminate => libc::SIGTERM,
        }
    }
}

/// Why no PID file descriptor path exists here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Unsupported {
    /// Not Linux: there is no PID file descriptor at all.
    Platform,
    /// `ENOSYS`: the kernel predates the call, or a filter answers `ENOSYS`.
    NoSystemCall,
    /// `EPERM` or `EACCES` from `pidfd_open`, which has no permission check
    /// of its own: a seccomp filter or a security module refused the call.
    Refused,
    /// `ENODEV`: the kernel has no anonymous inode filesystem.
    NoAnonymousInodes,
}

impl Unsupported {
    pub fn describe(self) -> &'static str {
        match self {
            Self::Platform => "this system has no PID file descriptors (Linux 5.3 or later is needed)",
            Self::NoSystemCall => {
                "the kernel does not provide pidfd_open or pidfd_send_signal (Linux 5.3 or later is needed), or a system-call filter hides them"
            }
            Self::Refused => "a system-call filter or security module refused pidfd_open",
            Self::NoAnonymousInodes => "the kernel has no anonymous inode filesystem for PID file descriptors",
        }
    }
}

/// Why [`ProcessHandles::acquire`] holds no handle. Every one is final for
/// this attempt: nothing is retried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AcquireError {
    /// No PID file descriptor path here.
    Unsupported(Unsupported),
    /// The key cannot be bound to a live process here, for the reason given:
    /// another source, a thread, no PID, no Linux start time, no boot or PID
    /// namespace.
    Unidentifiable(&'static str),
    /// The machine has booted since the key was sampled.
    OtherBoot,
    /// The key's PID namespace is not the one this reader opens handles in.
    OtherNamespace,
    /// The process exited before a handle could be taken.
    Ended,
    /// Another process holds the PID now.
    Replaced,
    /// The identity could not be confirmed after the open (an unreadable or
    /// unparsable record), so the handle was closed rather than trusted.
    Unverified(String),
    /// `EMFILE` (this process) or `ENFILE` (the whole system).
    DescriptorsExhausted { system_wide: bool },
    /// This table already holds its bound of live handles.
    LimitReached { limit: usize },
    /// Any other failure, as the system reported it.
    Failed(String),
}

impl AcquireError {
    pub fn describe(&self) -> String {
        match self {
            Self::Unsupported(why) => why.describe().into(),
            Self::Unidentifiable(why) => format!("the process cannot be identified here: {why}"),
            Self::OtherBoot => "the machine has restarted since the process was sampled".into(),
            Self::OtherNamespace => {
                "the process was sampled in another PID namespace than this server's".into()
            }
            Self::Ended => "the process exited before it could be held".into(),
            Self::Replaced => "another process now holds its PID".into(),
            Self::Unverified(why) => {
                format!("the process could not be confirmed after it was opened: {why}")
            }
            Self::DescriptorsExhausted { system_wide: false } => {
                "this server has no file descriptor left for a process handle".into()
            }
            Self::DescriptorsExhausted { system_wide: true } => {
                "the system has no file descriptor left for a process handle".into()
            }
            Self::LimitReached { limit } => {
                format!("this server already holds its {limit} process handles")
            }
            Self::Failed(why) => format!("the process could not be held: {why}"),
        }
    }
}

/// What one [`ProcessHandles::settle`] did. Only [`SignalOutcome::Sent`]
/// means the kernel accepted a signal, and then only for the handle's own
/// process; whether the process then ended is for an observation to say
/// (PX-031).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignalOutcome {
    /// The kernel accepted the signal for the handle's process.
    Sent,
    /// No handle with that ID is held: it was cancelled, expired, already
    /// settled, or never acquired. Nothing was sent.
    NoHandle,
    /// The handle expired before the signal; it was closed and nothing sent.
    Expired,
    /// The handle's process has been reaped (`ESRCH`); nothing was sent, and
    /// nothing else can have received it.
    Ended,
    /// `EPERM`: the account may not signal the process, or a system-call
    /// filter refused the call. Nothing was sent.
    Denied,
    /// `pidfd_send_signal` is missing. Nothing was sent.
    Unsupported(Unsupported),
    /// Any other failure, as the system reported it. Nothing was sent.
    Failed(String),
}

/// An open PID file descriptor. Dropping it closes it.
pub trait Pidfd: Send + fmt::Debug {
    /// `pidfd_send_signal(fd, signal, NULL, 0)`.
    fn send(&self, signal: HandleSignal) -> io::Result<()>;
}

/// The system calls and reads a handle needs: the seam tests replace.
pub trait PidfdSystem: Send + Sync + fmt::Debug {
    /// Whether this system has PID file descriptors at all.
    fn available(&self) -> bool {
        true
    }
    /// `pidfd_open(pid, 0)`, in the caller's PID namespace.
    fn open(&self, pid: u32) -> io::Result<Box<dyn Pidfd>>;
    /// `/proc/<pid>/stat`, read by path.
    fn read_stat(&self, pid: u32) -> io::Result<Vec<u8>>;
    /// `/proc/sys/kernel/random/boot_id`, trimmed.
    fn boot_id(&self) -> io::Result<String>;
    /// The inode of this process's own active PID namespace
    /// (`/proc/self/ns/pid`), the namespace `pidfd_open` reads a PID in.
    fn pid_namespace(&self) -> io::Result<u64>;
}

/// The host's own PID file descriptors and `/proc`. On every system but
/// Linux it is [`Unsupported::Platform`] and makes no call at all.
#[derive(Debug, Clone, Copy, Default)]
pub struct LivePidfds;

impl PidfdSystem for LivePidfds {
    fn available(&self) -> bool {
        cfg!(target_os = "linux")
    }

    fn open(&self, pid: u32) -> io::Result<Box<dyn Pidfd>> {
        #[cfg(target_os = "linux")]
        {
            linux::open(pid)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = pid;
            Err(io::ErrorKind::Unsupported.into())
        }
    }

    fn read_stat(&self, pid: u32) -> io::Result<Vec<u8>> {
        read_bounded(&Path::new(DEFAULT_PROC_ROOT).join(format!("{pid}/stat")))
    }

    fn boot_id(&self) -> io::Result<String> {
        let bytes = read_bounded(&Path::new(DEFAULT_PROC_ROOT).join("sys/kernel/random/boot_id"))?;
        Ok(String::from_utf8_lossy(&bytes).trim().to_string())
    }

    fn pid_namespace(&self) -> io::Result<u64> {
        let link = std::fs::read_link(Path::new(DEFAULT_PROC_ROOT).join("self/ns/pid"))?;
        parse_namespace(&link.to_string_lossy())
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "not a pid:[inode] link"))
    }
}

/// The two system calls, and nothing else. PX-008's audit pins each `unsafe`
/// block here token for token and refuses any other `libc` item.
#[cfg(target_os = "linux")]
mod linux {
    use super::{HandleSignal, Pidfd};
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

    #[derive(Debug)]
    struct LinuxPidfd(OwnedFd);

    pub(super) fn open(pid: u32) -> io::Result<Box<dyn Pidfd>> {
        let pid = libc::pid_t::try_from(pid)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "PID out of range"))?;
        // SAFETY: `pidfd_open` takes a PID and flags by value and touches no
        // memory of this process; it returns a new descriptor or -1.
        let result = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0 as libc::c_uint) };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        // A descriptor number always fits a C `int`: the kernel returns one.
        let fd = result as RawFd;
        // SAFETY: `fd` was just returned by the kernel, is open, and is owned
        // by nothing else, so `OwnedFd` may close it exactly once.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        Ok(Box::new(LinuxPidfd(fd)))
    }

    impl Pidfd for LinuxPidfd {
        fn send(&self, signal: HandleSignal) -> io::Result<()> {
            let number = signal.number();
            let fd = self.0.as_raw_fd();
            // SAFETY: the descriptor is open for as long as `self` is; the
            // `siginfo_t` argument is NULL, which the kernel accepts (K2), so
            // no memory of this process is read or written.
            let result = unsafe {
                libc::syscall(
                    libc::SYS_pidfd_send_signal,
                    fd,
                    number,
                    std::ptr::null::<u8>(),
                    0 as libc::c_uint,
                )
            };
            if result < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        }
    }
}

/// One handle's identity within its table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct HandleId(u64);

impl HandleId {
    pub fn get(self) -> u64 {
        self.0
    }
}

/// One held handle: the descriptor, the key it was confirmed against and when
/// it expires.
#[derive(Debug)]
struct Held {
    pidfd: Box<dyn Pidfd>,
    key: ProcessKey,
    expires_at: Duration,
}

/// The server's process handles: at most `limit`, each for at most
/// `lifetime`. Dropping the table closes every one.
pub struct ProcessHandles {
    system: Arc<dyn PidfdSystem>,
    clock: Arc<dyn MonotonicClock>,
    source: SourceId,
    lifetime: Duration,
    limit: usize,
    next: u64,
    held: BTreeMap<HandleId, Held>,
    /// This process's boot ID and active PID namespace, read once and kept:
    /// neither can change while the process runs (a `setns` into a PID
    /// namespace changes only where its children are created). Kept so that
    /// the open is the first descriptor an acquisition needs.
    host: Option<(String, u64)>,
}

impl fmt::Debug for ProcessHandles {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProcessHandles")
            .field("system", &self.system)
            .field("source", &self.source)
            .field("lifetime", &self.lifetime)
            .field("limit", &self.limit)
            .field("held", &self.held.len())
            .finish_non_exhaustive()
    }
}

impl ProcessHandles {
    /// Handles on the host's own processes, for keys from the live `/proc`
    /// source, on a suspend-aware clock.
    pub fn live() -> Self {
        Self::new(
            Arc::new(LivePidfds),
            Arc::new(SuspendAwareClock::new()),
            crate::procfs::live_source_id(),
        )
    }

    /// Handles through `system`, timed by `clock`, for keys from `source`.
    pub fn new(
        system: Arc<dyn PidfdSystem>,
        clock: Arc<dyn MonotonicClock>,
        source: SourceId,
    ) -> Self {
        Self {
            system,
            clock,
            source,
            lifetime: DEFAULT_HANDLE_LIFETIME,
            limit: DEFAULT_MAX_LIVE_HANDLES,
            next: 1,
            held: BTreeMap::new(),
            host: None,
        }
    }

    /// This table with handles that expire `lifetime` after acquisition.
    pub fn with_lifetime(mut self, lifetime: Duration) -> Self {
        self.lifetime = lifetime;
        self
    }

    /// This table holding at most `limit` handles at once.
    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }

    /// Whether handles can be opened here at all: a handle on this process
    /// itself is opened and closed at once, and nothing is sent. `Ok` means
    /// the path is available; [`AcquireError::Unsupported`] means the
    /// actions that need it must stay disabled; any other error is transient.
    pub fn probe(&self) -> Result<(), AcquireError> {
        if !self.system.available() {
            return Err(AcquireError::Unsupported(Unsupported::Platform));
        }
        self.system
            .open(std::process::id())
            .map(drop)
            .map_err(open_error)
    }

    /// How many handles are open now.
    pub fn live_count(&self) -> usize {
        self.held.len()
    }

    /// Whether `id` is held.
    pub fn holds(&self, id: HandleId) -> bool {
        self.held.contains_key(&id)
    }

    /// The key `id` was confirmed against.
    pub fn key_of(&self, id: HandleId) -> Option<&ProcessKey> {
        self.held.get(&id).map(|held| &held.key)
    }

    /// The first instant, on this table's clock, at which `id` is no longer
    /// usable.
    pub fn expires_at(&self, id: HandleId) -> Option<Duration> {
        self.held.get(&id).map(|held| held.expires_at)
    }

    fn now(&self) -> Duration {
        self.clock.now().0
    }

    /// Opens a handle on the process `key` names and confirms, after the
    /// open, that it is that process instance (see the module notes).
    pub fn acquire(&mut self, key: &ProcessKey) -> Result<HandleId, AcquireError> {
        self.expire();
        if !self.system.available() {
            return Err(AcquireError::Unsupported(Unsupported::Platform));
        }
        if self.held.len() >= self.limit {
            return Err(AcquireError::LimitReached { limit: self.limit });
        }
        let (pid, start_ticks) = self.identifiable(key)?;
        self.same_boot_and_namespace(key)?;
        let pidfd = self.system.open(pid).map_err(open_error)?;
        // The descriptor is open: from here every refusal drops it, closing it.
        let bytes = match self.system.read_stat(pid) {
            Ok(bytes) => bytes,
            Err(error) if ended(&error) => return Err(AcquireError::Ended),
            Err(error) => return Err(read_error("stat", error)),
        };
        let Some(stat) = parse_stat(pid, &bytes) else {
            return Err(AcquireError::Unverified("stat does not parse".into()));
        };
        if stat.start_ticks != start_ticks {
            return Err(AcquireError::Replaced);
        }
        let id = HandleId(self.next);
        self.next += 1;
        let expires_at = self.now().saturating_add(self.lifetime);
        self.held.insert(
            id,
            Held {
                pidfd,
                key: key.clone(),
                expires_at,
            },
        );
        Ok(id)
    }

    /// The PID and start time of a key this table can bind.
    fn identifiable(&self, key: &ProcessKey) -> Result<(u32, u64), AcquireError> {
        if key.source != self.source {
            return Err(AcquireError::Unidentifiable(
                "it was not sampled from this host's /proc",
            ));
        }
        if key.is_thread() {
            return Err(AcquireError::Unidentifiable(
                "it is a thread, not a process",
            ));
        }
        let Observed::Known(pid) = key.pid else {
            return Err(AcquireError::Unidentifiable("its PID is not known"));
        };
        let CreationToken::LinuxBootTicks(start_ticks) = key.creation else {
            return Err(AcquireError::Unidentifiable("it has no Linux start time"));
        };
        Ok((pid, start_ticks))
    }

    /// Whether the key was sampled in this boot and in the PID namespace
    /// `pidfd_open` will read its PID in.
    fn same_boot_and_namespace(&mut self, key: &ProcessKey) -> Result<(), AcquireError> {
        let Observed::Known(BootId(boot)) = &key.boot else {
            return Err(AcquireError::Unidentifiable("its boot is not known"));
        };
        let Observed::Known(PidNamespaceId(namespace)) = key.pid_namespace else {
            return Err(AcquireError::Unidentifiable(
                "its PID namespace is not known",
            ));
        };
        let (current, own) = match &self.host {
            Some(host) => host.clone(),
            None => {
                let current = self
                    .system
                    .boot_id()
                    .map_err(|error| read_error("boot_id", error))?;
                let own = self
                    .system
                    .pid_namespace()
                    .map_err(|error| read_error("self/ns/pid", error))?;
                self.host.insert((current, own)).clone()
            }
        };
        if &current != boot {
            return Err(AcquireError::OtherBoot);
        }
        if own != namespace {
            return Err(AcquireError::OtherNamespace);
        }
        Ok(())
    }

    /// Closes `id` unused. `false` when it was not held.
    pub fn cancel(&mut self, id: HandleId) -> bool {
        self.held.remove(&id).is_some()
    }

    /// Closes every handle whose lifetime is over; how many.
    pub fn expire(&mut self) -> usize {
        let now = self.now();
        let before = self.held.len();
        self.held.retain(|_, held| now < held.expires_at);
        before - self.held.len()
    }

    /// Sends `signal` through `id` once and closes it, whatever the result:
    /// a handle settles exactly one request. Nothing is sent through a handle
    /// that has expired, and nothing by PID.
    pub fn settle(&mut self, id: HandleId, signal: HandleSignal) -> SignalOutcome {
        let Some(held) = self.held.remove(&id) else {
            return SignalOutcome::NoHandle;
        };
        if self.now() >= held.expires_at {
            return SignalOutcome::Expired;
        }
        match held.pidfd.send(signal) {
            Ok(()) => SignalOutcome::Sent,
            Err(error) if error.raw_os_error() == Some(ESRCH) => SignalOutcome::Ended,
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied => SignalOutcome::Denied,
            Err(error) if error.kind() == io::ErrorKind::Unsupported => {
                SignalOutcome::Unsupported(Unsupported::NoSystemCall)
            }
            Err(error) => SignalOutcome::Failed(error.to_string()),
        }
    }
}

/// What a failed identity read means: descriptor exhaustion (`EMFILE`,
/// `ENFILE` — the read itself needed a descriptor, perhaps the last one the
/// open just took) is reported as exhaustion, never retried; anything else
/// means the identity could not be confirmed.
fn read_error(what: &str, error: io::Error) -> AcquireError {
    match error.raw_os_error() {
        Some(EMFILE) => AcquireError::DescriptorsExhausted { system_wide: false },
        Some(ENFILE) => AcquireError::DescriptorsExhausted { system_wide: true },
        _ => AcquireError::Unverified(format!("{what}: {error}")),
    }
}

/// What a failed `pidfd_open` means.
fn open_error(error: io::Error) -> AcquireError {
    match error.raw_os_error() {
        Some(ESRCH) => return AcquireError::Ended,
        Some(EMFILE) => return AcquireError::DescriptorsExhausted { system_wide: false },
        Some(ENFILE) => return AcquireError::DescriptorsExhausted { system_wide: true },
        Some(ENODEV) => return AcquireError::Unsupported(Unsupported::NoAnonymousInodes),
        _ => {}
    }
    match error.kind() {
        io::ErrorKind::Unsupported => AcquireError::Unsupported(Unsupported::NoSystemCall),
        io::ErrorKind::PermissionDenied => AcquireError::Unsupported(Unsupported::Refused),
        _ => AcquireError::Failed(error.to_string()),
    }
}
