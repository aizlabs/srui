//! One-shot Linux `/proc` enumeration with process-instance identity
//! (K1; design §§6.3, 8, 12, 22; PX-003). Read-only: this module opens no process
//! for control, sends no signal and never passes a process name to a shell.
//!
//! The scan root is injected, so every branch — denied records, vanished records,
//! hostile names, missing identity files — is exercised by deterministic fixtures
//! on any platform. Only [`ProcFsSource::live`] reads the real `/proc`, which
//! exists on Linux; on other systems it honestly reports an incomplete scan
//! instead of an authoritative empty result.
//!
//! One scan's memory is bounded by [`MAX_RECORDS`] published records, by
//! [`crate::source::MAX_RECORDED_ISSUES`] retained explanations, and by
//! [`MAX_UNCERTAIN_PIDS`] named identities in each of its two ledgers — the
//! records it read and could not use, and the entries it listed but never read
//! (PX-003, PX-004). A scan that would have to name more than a ledger holds
//! reports itself unenumerable rather than growing a set per scan.
//!
//! CPU usage (PX-006) is the one thing a single scan cannot measure: it is a
//! difference between two samples of the same process instance. The source is
//! therefore a persistent collector — the refresh loop keeps one instance for
//! its whole life — and it retains one counter baseline per record it last
//! published, keyed by PID *and* creation token, so a reused PID is a new
//! instance and never inherits another process's counters. The interval is
//! measured on an injected [`MonotonicClock`], never on the wall clock that
//! stamps [`SnapshotTime`], so a wall-clock jump cannot stretch or shrink it.
//!
//! Resident memory (PX-005, PX-005-G01) is read from a second file of the same
//! record, `statm`, after the `stat` that identifies it: `stat`'s own `rss` is
//! approximate on every kernel since Linux 6.2, while `statm`'s is exact from
//! 6.16. Where the reader's own procfs can name an open directory
//! (`/proc/self/fd/<n>`, on Linux), both files are read through one handle on
//! the record's directory, so a PID reused between the two reads cannot lend its
//! memory to the instance `stat` identified (see `RecordAccess`).
use crate::source::{
    record_issue, BootId, CappedRecords, Completeness, CpuInterval, CpuUsage, CreationToken,
    DisplayName, EnumerationIssue, HostId, IssueScope, MissingReason, Observed, PidNamespaceId,
    ProcessKey, ProcessRecord, ProcessSnapshot, ProcessSource, SkippedRecords, SnapshotTime,
    SourceId,
};
use srui_semantic_tree::DEFAULT_MAX_CACHED_ITEMS_PER_MODEL;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::fs::File;
use std::io::{self, Read};
use std::os::unix::fs::MetadataExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

/// The real Linux process filesystem.
pub const DEFAULT_PROC_ROOT: &str = "/proc";
/// Source-owned status label: this data is a live read of a real process
/// filesystem and is never described as a fixture.
pub const LIVE_STATUS_TEXT: &str = "Read-only · Live process snapshot";
/// Default bound on records published from one scan; entries beyond it are
/// counted as capped, never as unreadable.
pub const MAX_RECORDS: usize = 65_536;
/// Bound on the identities one scan may name in each of its two ledgers: the
/// records it read and could not use ([`SkippedRecords`]), and the entries it
/// listed but never read ([`CappedRecords`]).
///
/// This is what keeps *scoped* retention bounded, not just one scan's memory. A
/// refresh publishes at most one row per PID the scan either confirmed or left
/// uncertain (PX-004), so as long as both ledgers can name what they left out,
/// the collection holds at most `MAX_RECORDS + 2 * MAX_UNCERTAIN_PIDS` rows —
/// exactly the model's own §26 item bound.
///
/// A ledger asked to name more says so, and that scan retains every absent row,
/// which is why this is a ceiling on naming rather than a budget the collection
/// may spend. What bounds the collection in *that* case — an overflowed ledger
/// on a host with more readable entries than one scan may name, where every tick
/// is unenumerable — is not here but at
/// [`crate::refresh::MAX_PUBLISHED_ROWS`], which no scan, ledger or retention
/// decision can lift.
pub const MAX_UNCERTAIN_PIDS: usize = (DEFAULT_MAX_CACHED_ITEMS_PER_MODEL - MAX_RECORDS) / 2;
const _: () = assert!(MAX_RECORDS + 2 * MAX_UNCERTAIN_PIDS <= DEFAULT_MAX_CACHED_ITEMS_PER_MODEL);
/// Bound on every single file this scan reads.
pub const MAX_FILE_BYTES: u64 = 64 * 1024;
/// `/proc/<pid>/stat` field 22 (start time) is the 20th field after `comm`.
const STARTTIME_FIELD_AFTER_COMM: usize = 19;
/// `/proc/<pid>/statm` is seven unsigned page counts — `size resident shared
/// text lib data dt` — separated by single spaces and ended by a newline, and
/// `0 0 0 0 0 0 0` for a task with no address space (`proc_pid_statm` in
/// fs/proc/array.c; K1).
const STATM_FIELDS: usize = 7;
/// `resident`, the second of them: the pages of the three resident counters
/// (file, anonymous, shared memory) `task_statm` sums (fs/proc/task_mmu.c).
const STATM_RESIDENT: usize = 1;
/// `ESRCH`, matched by number because `io::ErrorKind` has no stable variant for
/// it. It is 3 on every Linux architecture (`errno-base.h`).
const ESRCH: i32 = 3;
/// `AT_PAGESZ`: the auxiliary-vector entry through which the kernel tells a
/// process its page size (K1, `getauxval(3)`). It is read from the scanned
/// mount's own `self/auxv`, the same interface `sysconf(_SC_PAGESIZE)` answers
/// from, so this scan needs no new dependency and a fixture tree can state its
/// own page size as deterministically as it states its own hostname.
const AT_PAGESZ: u64 = 6;
/// `AT_NULL`: terminates the auxiliary vector. Nothing after it is defined.
const AT_NULL: u64 = 0;
/// Largest page size this scan will believe. Real base page sizes are 4 KiB to
/// 64 KiB; a gibibyte is far above every one of them and still refuses an
/// implausible value that would turn a page count into a nonsense byte count.
const MAX_PAGE_SIZE: u64 = 1 << 30;
/// Smallest page size this scan will believe.
const MIN_PAGE_SIZE: u64 = 512;
/// `/proc/<pid>/stat` field 14 (`utime`, user-mode clock ticks) and field 15
/// (`stime`, kernel-mode clock ticks) are the 12th and 13th fields after `comm`
/// (K1, `proc_pid_stat(5)`). Neither is a ptrace-gated field, so an unprivileged
/// scan reads real values rather than the zero the kernel substitutes for those.
const UTIME_FIELD_AFTER_COMM: usize = 11;
const STIME_FIELD_AFTER_COMM: usize = 12;
/// `AT_CLKTCK`: the auxiliary-vector entry carrying the frequency `times(2)`
/// counts at — the clock ticks `utime` and `stime` are measured in, and what
/// glibc's `sysconf(_SC_CLK_TCK)` answers from (`getauxval(3)`). Read from the
/// scanned mount's own `self/auxv` exactly as [`AT_PAGESZ`] is.
const AT_CLKTCK: u64 = 17;
/// Largest tick rate this scan will believe. `USER_HZ` is 100 on nearly every
/// Linux architecture and 1024 on a few; a megahertz is far above both and still
/// refuses a value that would turn tick counts into nonsense.
const MAX_CLOCK_TICKS_PER_SECOND: u64 = 1_000_000;

/// A monotonic clock, as elapsed time since an arbitrary fixed origin.
///
/// The CPU interval is measured on this and nothing else. It is injected so a
/// test can state every interval exactly — including the zero and backwards
/// ones a real monotonic clock never produces — and so no wall-clock reading
/// can reach the divisor.
pub trait MonotonicClock: Send + Sync + std::fmt::Debug {
    fn now(&self) -> Duration;
}

/// [`Instant`]: `clock_gettime(CLOCK_MONOTONIC)` on Linux. Like `utime` and
/// `stime`, it does not advance while the host is suspended, so a suspend does
/// not dilute the interval with time no process could have been scheduled in.
#[derive(Debug)]
pub struct SystemMonotonicClock {
    origin: Instant,
}

impl Default for SystemMonotonicClock {
    fn default() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl MonotonicClock for SystemMonotonicClock {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }
}

/// One process instance's CPU counter at one sample, kept until the next scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuBaseline {
    /// `utime + stime`, in the kernel's clock ticks.
    pub ticks: u64,
    /// When it was read, on the source's [`MonotonicClock`].
    pub at: Duration,
}

/// The CPU usage between `previous` and `current` samples of the same process
/// instance, and the baseline the next scan should subtract from.
///
/// Every case that cannot be a real measurement is reported as such, never as a
/// zero or a spike:
///
/// * an unknown tick rate makes every interval unmeasurable, so the reason the
///   rate is missing is published from the first sample on;
/// * no previous sample of this instance is [`CpuUsage::WarmingUp`];
/// * an interval that did not advance is unavailable, and the *older* baseline
///   is kept so the next scan measures across a real interval;
/// * a clock that went backwards, or a counter that went backwards (a reset), is
///   unavailable, and the current sample becomes the new baseline.
pub fn sample_cpu(
    previous: Option<&CpuBaseline>,
    current: CpuBaseline,
    ticks_per_second: &Observed<u64>,
) -> (CpuUsage, CpuBaseline) {
    let (usage, keep) = interval_since(previous, current);
    // The baseline is kept the same way whatever the rate, because the counter
    // itself was read; but without a rate no interval is a percentage, and the
    // reason the rate is missing is what every scan publishes.
    match (usage, ticks_per_second) {
        (_, Observed::Missing(reason)) => (CpuUsage::Missing(*reason), keep),
        (Ok((ticks, elapsed)), Observed::Known(rate)) => (
            CpuUsage::Measured(CpuInterval {
                ticks,
                ticks_per_second: *rate,
                elapsed,
            }),
            keep,
        ),
        (Err(usage), Observed::Known(_)) => (usage, keep),
    }
}

/// The counter delta and monotonic interval between two samples, or the state
/// to publish instead, with the baseline the next scan should use.
fn interval_since(
    previous: Option<&CpuBaseline>,
    current: CpuBaseline,
) -> (Result<(u64, Duration), CpuUsage>, CpuBaseline) {
    let unavailable = Err(CpuUsage::Missing(MissingReason::Unavailable));
    let Some(previous) = previous else {
        return (Err(CpuUsage::WarmingUp), current);
    };
    let elapsed = match current.at.checked_sub(previous.at) {
        None => return (unavailable, current),
        Some(Duration::ZERO) => return (unavailable, *previous),
        Some(elapsed) => elapsed,
    };
    match current.ticks.checked_sub(previous.ticks) {
        Some(ticks) => (Ok((ticks, elapsed)), current),
        None => (unavailable, current),
    }
}

/// Remembers a global identity component the CPU baselines were sampled under,
/// and reports whether this scan observed a *different* one. A component this
/// scan could not read is not a change — the processes did not move — but a
/// different boot or PID namespace numbers different process instances, whose
/// counters must never be subtracted from these.
fn identity_changed<T: Clone + PartialEq>(
    remembered: &mut Option<T>,
    observed: &Observed<T>,
) -> bool {
    let Observed::Known(value) = observed else {
        return false;
    };
    let changed = remembered.as_ref().is_some_and(|known| known != value);
    *remembered = Some(value.clone());
    changed
}

/// What a scanned root is, as far as an unprivileged scan can prove.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MountKind {
    /// Not a procfs mount: no `self` symlink. A fixture tree, whose own identity
    /// files are the only thing that can answer for it.
    Tree,
    /// The procfs this process runs under, so the kernel interfaces read through
    /// it describe the processes it lists.
    Own,
    /// A procfs mount this scan cannot prove is its own — or a root it cannot
    /// even classify. Identity read through it belongs to the reader, not to the
    /// records, and is reported unavailable rather than guessed.
    Unproven,
}

/// How one scan reaches the files of each record it reads (PX-005-G01).
///
/// A record is read twice — `stat` for its identity, name and CPU counters,
/// then `statm` for its resident memory — and a PID is only a number: by path,
/// the second read reaches whichever process holds that number *now*. Linux
/// hands a number out again only once the process that held it has been reaped
/// and its cyclic PID allocator has come back round to that number, so by path a
/// misattribution needs both to happen between two back-to-back reads; a pinned
/// directory rules it out instead of relying on that. procfs binds
/// an open `/proc/<pid>` directory to the process instance it was opened on (its
/// `struct pid`), so once that instance is reaped every lookup through the
/// handle fails with `ESRCH` (`proc_pid_permission`, fs/proc/base.c) and never
/// reaches a later process under the same number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecordAccess {
    /// Each record's directory is opened once, and its files are read through
    /// the reader's own `/proc/self/fd/<n>` link to that handle: `std` alone,
    /// with no `openat`.
    Pinned,
    /// Each file is opened by its path under the scanned root. Used where the
    /// reader has no procfs that names an open directory: every system but
    /// Linux, and a Linux reader without its own `/proc`.
    ByName,
}

/// One record's directory, as one scan reaches it.
struct RecordDir {
    /// Where this record's files are read from: the pinned handle's link, or the
    /// record's own path under the root.
    base: PathBuf,
    /// The handle `base` names, held until the record has been read and never
    /// read itself. Dropping it closes the directory.
    _pin: Option<File>,
}

impl RecordDir {
    fn open(path: PathBuf, access: RecordAccess) -> io::Result<Self> {
        match access {
            RecordAccess::ByName => Ok(Self {
                base: path,
                _pin: None,
            }),
            RecordAccess::Pinned => {
                let pin = File::open(&path)?;
                Ok(Self {
                    base: fd_link(&pin),
                    _pin: Some(pin),
                })
            }
        }
    }

    fn read(&self, file: &str) -> io::Result<Vec<u8>> {
        read_bounded(&self.base.join(file))
    }
}

/// One-shot reader over a `/proc`-shaped directory tree.
#[derive(Debug, Clone)]
pub struct ProcFsSource {
    root: PathBuf,
    source: SourceId,
    status: String,
    record_limit: usize,
    /// Bound on the identities either ledger of one scan may name.
    uncertain_limit: usize,
    /// Whether the root names one fixed tree for the life of this source. A
    /// relative root that could not be anchored does not, and is never scanned.
    anchored: bool,
    /// The clock CPU intervals are measured on.
    clock: Arc<dyn MonotonicClock>,
    /// One CPU baseline per record the previous scan published, keyed by PID and
    /// creation token. Rebuilt on every scan from that scan's records, so it is
    /// bounded by the record bound and forgets every process that ended.
    cpu_baselines: HashMap<(u32, u64), CpuBaseline>,
    /// The boot the baselines were sampled under, as last observed.
    cpu_boot: Option<BootId>,
    /// The PID namespace the baselines were sampled under, as last observed.
    cpu_namespace: Option<PidNamespaceId>,
}

impl ProcFsSource {
    /// Reads the host's real process filesystem. Only this constructor may
    /// describe its data as live.
    pub fn live() -> Self {
        Self::rooted(
            PathBuf::from(DEFAULT_PROC_ROOT),
            LIVE_STATUS_TEXT.to_string(),
        )
    }

    /// Reads a `/proc`-shaped tree. The root is part of the source identity, so
    /// records from different roots can never alias.
    ///
    /// The status describes what was read and from where, and claims neither
    /// liveness nor synthesis. A path cannot tell the two apart: `/host/proc` is
    /// a bind mount of a real process filesystem, and calling its rows a fixture
    /// would label live host processes as synthetic — the same defect as the
    /// reverse, in the other direction.
    pub fn with_root(root: impl Into<PathBuf>) -> Self {
        // A relative root is anchored here, not at scan time. The scan happens
        // later and the process may have changed directory in between, which
        // would silently point the same source — same `SourceId`, same status
        // line — at a different tree, and make records from two trees compare
        // as one. Anchoring is textual on purpose: it does not resolve symlinks
        // or require the root to exist, because a missing root must still scan
        // and report itself unreadable rather than fail construction.
        let given = root.into();
        let Some(root) = anchor_root(&given, std::env::current_dir().ok().as_deref()) else {
            // The working directory is gone, so a relative root names no fixed
            // tree: it would follow the next `chdir` while keeping one identity.
            // The source is constructed — callers get a source, not a panic — but
            // it scans nothing and says why, rather than emitting records whose
            // identity it cannot stand behind.
            return Self {
                root: given.clone(),
                source: SourceId(format!("procfs-unanchored:{}", given.display())),
                status: format!(
                    "Read-only · Process filesystem snapshot unavailable: {} cannot be anchored to a working directory",
                    given.display()
                ),
                record_limit: MAX_RECORDS,
                uncertain_limit: MAX_UNCERTAIN_PIDS,
                anchored: false,
                clock: Arc::new(SystemMonotonicClock::default()),
                cpu_baselines: HashMap::new(),
                cpu_boot: None,
                cpu_namespace: None,
            };
        };
        let status = format!(
            "Read-only · Process filesystem snapshot: {}",
            root.display()
        );
        Self::rooted(root, status)
    }

    fn rooted(root: PathBuf, status: String) -> Self {
        let source = source_id_for(&root);
        Self {
            root,
            source,
            status,
            record_limit: MAX_RECORDS,
            uncertain_limit: MAX_UNCERTAIN_PIDS,
            anchored: true,
            clock: Arc::new(SystemMonotonicClock::default()),
            cpu_baselines: HashMap::new(),
            cpu_boot: None,
            cpu_namespace: None,
        }
    }

    /// Measures CPU intervals on `clock` instead of the system's monotonic
    /// clock. Baselines already taken were measured on the old clock and are
    /// discarded, so no interval spans two clocks.
    pub fn with_clock(mut self, clock: Arc<dyn MonotonicClock>) -> Self {
        self.clock = clock;
        self.cpu_baselines.clear();
        self
    }

    /// How many CPU baselines this source is holding for the next scan.
    pub fn cpu_baselines(&self) -> usize {
        self.cpu_baselines.len()
    }

    /// Bounds how many records one scan publishes. Entries beyond the bound are
    /// reported as capped rather than unreadable, and the scan is not complete.
    pub fn with_record_limit(mut self, limit: usize) -> Self {
        self.record_limit = limit;
        self
    }

    /// Bounds how many identities one scan may name in each of its two ledgers.
    /// A scan that would have to name more reports itself unenumerable, which
    /// keeps every absent row; see [`MAX_UNCERTAIN_PIDS`] for why the default is
    /// what it is.
    pub fn with_uncertain_limit(mut self, limit: usize) -> Self {
        self.uncertain_limit = limit;
        self
    }

    pub fn source_id(&self) -> &SourceId {
        &self.source
    }

    /// The page size this scan converts resident page counts with, read from the
    /// scanned mount's own `self/auxv` ([`AT_PAGESZ`]).
    ///
    /// Read through the scanned root rather than from the real `/proc`, for the
    /// same reason the identity files are: a source is whatever its root says it
    /// is. A fixture tree states its own page size and is therefore deterministic
    /// on any host, and a scan of a tree that states none reports the metric
    /// unread instead of attributing this machine's page size to records that
    /// were never measured on it.
    fn page_size(&self) -> Observed<u64> {
        self.auxv_entry(parse_page_size)
    }

    /// The clock-tick rate `utime` and `stime` are counted in, read from the
    /// scanned mount's own `self/auxv` ([`AT_CLKTCK`]) for the same reasons as
    /// [`Self::page_size`]: a fixture states its own, and a mount that states
    /// none publishes no CPU value instead of borrowing this machine's rate.
    fn clock_ticks(&self) -> Observed<u64> {
        self.auxv_entry(parse_clock_ticks)
    }

    fn auxv_entry(&self, parse: fn(&[u8]) -> Option<u64>) -> Observed<u64> {
        match read_bounded(&self.root.join("self/auxv")) {
            Ok(bytes) => match parse(&bytes) {
                Some(value) => Observed::Known(value),
                None => Observed::Missing(MissingReason::Unavailable),
            },
            Err(error) => Observed::Missing(reason_for(&error)),
        }
    }

    fn identity<T>(
        &self,
        relative: &str,
        scope: IssueScope,
        issues: &mut Vec<EnumerationIssue>,
        wrap: impl FnOnce(String) -> T,
    ) -> Observed<T> {
        match read_bounded(&self.root.join(relative)) {
            Ok(bytes) => {
                let text = String::from_utf8_lossy(&bytes).trim().to_string();
                if text.is_empty() {
                    record_issue(issues, || EnumerationIssue {
                        scope,
                        reason: MissingReason::Unavailable,
                        detail: format!("{relative} is empty"),
                    });
                    Observed::Missing(MissingReason::Unavailable)
                } else {
                    Observed::Known(wrap(text))
                }
            }
            Err(error) => {
                let reason = reason_for(&error);
                record_issue(issues, || EnumerationIssue {
                    scope,
                    reason,
                    detail: format!("{relative}: {error}"),
                });
                Observed::Missing(reason)
            }
        }
    }

    /// The PID namespace the scanned records are numbered in.
    ///
    /// `self/ns/pid` always names the *reader's* active namespace, which is the
    /// right answer only when the mount numbers PIDs the way this process is
    /// numbered. A host `/proc` bind-mounted into a container breaks exactly
    /// that: the numeric directories are scoped to the mount's namespace while
    /// the link resolves to the container's, so every key would be stamped with
    /// a namespace its PIDs do not belong to.
    ///
    /// The mount's own namespace init answers it directly when this scan may
    /// read it: `<root>/1` is the task *that mount* numbers 1, so its `ns/pid`
    /// link names the namespace the enumerated PIDs are numbered in, whatever
    /// namespace the reader is in. Reading another task's `ns/` link needs
    /// ptrace access, so an unprivileged scan of a host `/proc` normally cannot,
    /// and falls back to the reader's own link — but only with both proofs that
    /// it describes these records:
    ///
    /// * `<root>/self` names *this* process's own PID. A nested PID namespace
    ///   that inherited an outer `/proc` (`unshare --pid --fork` with no
    ///   remount) fails here: the mount still numbers this process by its outer
    ///   PID while `self/ns/pid` names the inner namespace.
    /// * `<root>/self` and `/proc/self` are the same device and inode, so the
    ///   mount is the procfs this process runs under. A procfs superblock
    ///   belongs to exactly one PID namespace; a bind-mounted host `/proc`
    ///   fails here even when the two numberings happen to agree.
    ///
    /// Failing either, the namespace is reported unavailable rather than
    /// guessed: an unknown identity component fails explicitly instead of
    /// degrading silently. A fixture tree has no `self` symlink and is not a
    /// procfs mount, so its own identity files answer for it.
    /// What the scanned root is, as far as this scan can prove. Every kernel
    /// interface reached *through* a procfs mount — `self/ns/pid`,
    /// `sys/kernel/hostname` — answers for the reader's namespaces, not the
    /// mount's, so the answer is only this mount's identity when the mount is
    /// the procfs this process runs under.
    fn mount_kind(&self) -> MountKind {
        let scanned_self = self.root.join("self");
        // A real procfs mount always has `self` as a symlink; a tree that does
        // not is not a procfs mount and answers from its own identity files.
        // `Path::is_symlink` would fold "cannot tell" into that second case, so
        // the file type is read explicitly and an error counts as unproven.
        let Ok(kind) = std::fs::symlink_metadata(&scanned_self).map(|data| data.file_type()) else {
            return MountKind::Unproven;
        };
        if !kind.is_symlink() {
            return MountKind::Tree;
        }
        let own_self = Path::new(DEFAULT_PROC_ROOT).join("self");
        let numbers_this_process = std::fs::read_link(&scanned_self)
            .ok()
            .and_then(|target| parse_pid(target.as_os_str().as_encoded_bytes()))
            == Some(std::process::id());
        let own_procfs = match (
            std::fs::metadata(&scanned_self),
            std::fs::metadata(&own_self),
        ) {
            (Ok(scanned), Ok(own)) => scanned.dev() == own.dev() && scanned.ino() == own.ino(),
            _ => false,
        };
        if readers_namespace_describes(numbers_this_process, own_procfs) {
            MountKind::Own
        } else {
            MountKind::Unproven
        }
    }

    fn pid_namespace(
        &self,
        mount: MountKind,
        issues: &mut Vec<EnumerationIssue>,
    ) -> Observed<PidNamespaceId> {
        if let Some(inode) = std::fs::read_link(self.root.join("1/ns/pid"))
            .ok()
            .and_then(|target| parse_namespace(&target.to_string_lossy()))
        {
            return Observed::Known(PidNamespaceId(inode));
        }
        if mount == MountKind::Unproven {
            record_issue(issues, || EnumerationIssue {
                scope: IssueScope::PidNamespace,
                reason: MissingReason::Unavailable,
                detail: format!(
                    "{} numbers its records in a PID namespace this scan cannot prove is its own",
                    self.root.display()
                ),
            });
            return Observed::Missing(MissingReason::Unavailable);
        }
        let path = self.root.join("self/ns/pid");
        match std::fs::read_link(&path) {
            Ok(target) => match parse_namespace(&target.to_string_lossy()) {
                Some(inode) => Observed::Known(PidNamespaceId(inode)),
                None => {
                    record_issue(issues, || EnumerationIssue {
                        scope: IssueScope::PidNamespace,
                        reason: MissingReason::Unavailable,
                        detail: "self/ns/pid is not a pid:[inode] link".into(),
                    });
                    Observed::Missing(MissingReason::Unavailable)
                }
            },
            Err(error) => {
                let reason = reason_for(&error);
                record_issue(issues, || EnumerationIssue {
                    scope: IssueScope::PidNamespace,
                    reason,
                    detail: format!("self/ns/pid: {error}"),
                });
                Observed::Missing(reason)
            }
        }
    }
}

impl ProcessSource for ProcFsSource {
    fn status_text(&self) -> &str {
        &self.status
    }

    fn snapshot(&mut self) -> ProcessSnapshot {
        let sampled_at = SnapshotTime(SystemTime::now());
        let mut issues = Vec::new();
        if !self.anchored {
            // No fixed tree to scan: an empty list here is explicitly not an
            // authoritative "no processes", and no record is emitted under an
            // identity that could name a different tree a moment later.
            issues.push(EnumerationIssue {
                scope: IssueScope::Root,
                reason: MissingReason::Unavailable,
                detail: format!(
                    "{} is relative and could not be anchored to a working directory",
                    self.root.display()
                ),
            });
            return ProcessSnapshot {
                source: self.source.clone(),
                sampled_at,
                records: Vec::new(),
                vanished: 0,
                capped: CappedRecords::none(),
                // Nothing was enumerated at all: no absence can be attributed.
                completeness: Completeness::from_scan(SkippedRecords::unenumerable(), issues),
            };
        }
        // The unpublished records are named as the scan leaves them out, so the
        // uncertain set never depends on how many explanations were retained.
        // A record this scan reads and cannot use is bounded by the scan's own
        // record bound as well as by the ceiling: it could not have skipped more
        // records than it would have published. An entry beyond the record bound
        // is bounded by the ceiling alone, because by construction there are as
        // many of those as the host has PIDs past that bound.
        let mut skipped = SkippedRecords::with_limit(self.record_limit.min(self.uncertain_limit));
        let mut vanished = 0usize;
        let mut capped = CappedRecords::with_limit(self.uncertain_limit);
        let mount = self.mount_kind();
        // `sys/kernel/hostname` is a sysctl: the kernel answers it from the
        // *reader's* UTS namespace, whatever mount it is read through. Two
        // containers scanning one bind-mounted host `/proc` would otherwise
        // stamp the same processes with two different host identities, and one
        // of them with a hostname belonging to no process in the list.
        let host = match host_label(mount, uts_namespace_tag()) {
            HostLabel::Plain => self.identity(
                "sys/kernel/hostname",
                IssueScope::HostIdentity,
                &mut issues,
                HostId,
            ),
            HostLabel::Qualified(tag) => self.identity(
                "sys/kernel/hostname",
                IssueScope::HostIdentity,
                &mut issues,
                |text| HostId(format!("{text}@{tag}")),
            ),
            HostLabel::Unidentifiable(why) => {
                record_issue(&mut issues, || EnumerationIssue {
                    scope: IssueScope::HostIdentity,
                    reason: MissingReason::Unavailable,
                    detail: format!("{}: {why}", self.root.display()),
                });
                Observed::Missing(MissingReason::Unavailable)
            }
        };
        // The boot identity is not gated the same way: `random/boot_id` is one
        // value per running kernel, not per namespace, so every procfs mount on
        // this machine answers it identically.
        let boot = self.identity(
            "sys/kernel/random/boot_id",
            IssueScope::BootIdentity,
            &mut issues,
            BootId,
        );
        let pid_namespace = self.pid_namespace(mount, &mut issues);
        // Read once per scan, not once per record: the page size is a property of
        // the kernel behind the scanned mount, and every record's page count is
        // in those pages. A mount that cannot state it publishes every resident
        // value as unread (see [`resident_bytes`]) — visibly, in every row —
        // rather than degrading `Completeness`, which is about whether the record
        // *list* is authoritative, not about one field of a record.
        let page_size = self.page_size();
        // Likewise the tick rate the CPU counters are in.
        let ticks_per_second = self.clock_ticks();
        // Whether each record's two reads go through one pinned directory is a
        // property of this reader, decided once per scan (see [`RecordAccess`]).
        let access = record_access(&self.root);
        // Baselines taken under another boot or PID namespace count other
        // process instances; none of them may be subtracted from this scan's.
        // Both components are checked, so neither short-circuits the other's
        // remembered value.
        if identity_changed(&mut self.cpu_boot, &boot)
            | identity_changed(&mut self.cpu_namespace, &pid_namespace)
        {
            self.cpu_baselines.clear();
        }
        // The CPU interval is measured per record, not per scan: each record's
        // instant is taken on the monotonic clock the moment its own `stat`
        // read returns (below). A scan reads records one after another, and a
        // slow read — a stalled file, a host with tens of thousands of entries
        // — would otherwise divide one process's read-to-read counter delta by
        // a scan-start-to-scan-start interval it was never measured over.
        let mut baselines = HashMap::with_capacity(self.cpu_baselines.len());
        let mut records = Vec::new();
        let entries = match std::fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(error) => {
                // The whole scan failed: an empty list here is explicitly not
                // an authoritative "no processes" answer, and the records it
                // hides were never read, so none of them can be named. It read
                // no counter either, so the baselines are left exactly as they
                // were: the next good scan measures across this one.
                skipped.mark_unenumerable();
                record_issue(&mut issues, || EnumerationIssue {
                    scope: IssueScope::Root,
                    reason: reason_for(&error),
                    detail: format!("{}: {error}", self.root.display()),
                });
                return ProcessSnapshot {
                    source: self.source.clone(),
                    sampled_at,
                    records,
                    vanished,
                    capped,
                    completeness: Completeness::from_scan(skipped, issues),
                };
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    // The entry itself could not be examined, so this record has
                    // no PID to name and its absence cannot be attributed.
                    skipped.unnamed();
                    record_issue(&mut issues, || EnumerationIssue {
                        scope: IssueScope::Entry,
                        reason: reason_for(&error),
                        detail: format!("directory entry: {error}"),
                    });
                    continue;
                }
            };
            let name = entry.file_name();
            // Non-numeric entries are kernel state, not processes; skipping them
            // is not a degradation.
            let Some(pid) = parse_pid(name.as_encoded_bytes()) else {
                continue;
            };
            if records.len() >= self.record_limit {
                // This entry was never read: it is omitted by the collector's own
                // bound, not because anything denied or hid it. Counting it as
                // skipped would publish a read failure that never happened.
                //
                // The bound stopped the read, not the listing, and the listing
                // already named this record: its PID is recorded so its absence
                // from the published rows stays attributable, and a host larger
                // than the record bound does not make every scan keep every
                // absent row (PX-004).
                capped.record(pid);
                if capped.count() == 1 {
                    let limit = self.record_limit;
                    record_issue(&mut issues, || EnumerationIssue {
                        scope: IssueScope::Limit,
                        reason: MissingReason::Unavailable,
                        detail: format!("record limit {limit} reached"),
                    });
                }
                continue;
            }
            // Both of this record's reads go through one directory, pinned where
            // this reader can pin it (see [`RecordAccess`]).
            let directory = match RecordDir::open(self.root.join(&name), access) {
                Ok(directory) => directory,
                // Gone before its directory could be pinned: the same churn as a
                // `stat` that is already gone, below.
                Err(error) if ended(&error) => {
                    vanished += 1;
                    continue;
                }
                Err(error) => {
                    skipped.record(pid);
                    record_issue(&mut issues, || EnumerationIssue {
                        scope: IssueScope::Process(pid),
                        reason: reason_for(&error),
                        detail: format!("{pid}: {error}"),
                    });
                    continue;
                }
            };
            match directory.read("stat") {
                // The process exited between listing the root and reading it.
                // Nothing was inaccessible: the record no longer exists at sample
                // time, which is ordinary churn on any busy host, so it is
                // counted apart and does not degrade the scan.
                Err(error) if ended(&error) => {
                    vanished += 1;
                }
                // One unreadable record is skipped with a reason; it never fails
                // the snapshot (PX-003).
                Err(error) => {
                    skipped.record(pid);
                    record_issue(&mut issues, || EnumerationIssue {
                        scope: IssueScope::Process(pid),
                        reason: reason_for(&error),
                        detail: format!("{pid}/stat: {error}"),
                    });
                }
                // The instant is read before parsing, immediately after the
                // counters were read, so it brackets exactly this record's read.
                Ok(bytes) => match (self.clock.now(), parse_stat(pid, &bytes)) {
                    (_, None) => {
                        skipped.record(pid);
                        record_issue(&mut issues, || EnumerationIssue {
                            scope: IssueScope::Process(pid),
                            reason: MissingReason::Unavailable,
                            detail: format!("{pid}/stat is not parsable for this PID"),
                        });
                    }
                    (read_at, Some(stat)) => {
                        // Resident memory is read second, from the same
                        // directory: `statm`'s `resident`, which the kernel sums
                        // exactly from Linux 6.16, where `stat` field 24 stays
                        // approximate on every kernel since 6.2 (PX-005-G01).
                        let resident_pages = match directory.read("statm") {
                            // The process ended between its own two reads, so it
                            // no longer exists at sample time — exactly like one
                            // whose `stat` was already gone. It is counted as
                            // vanished, never published with half a sample, and
                            // no counter baseline is kept for it.
                            Err(error) if ended(&error) => {
                                vanished += 1;
                                continue;
                            }
                            // Refused, or failed for another reason: this one
                            // field is unread, with that reason, and the record
                            // is still published (PX-005).
                            Err(error) => Observed::Missing(reason_for(&error)),
                            Ok(bytes) => match parse_statm_resident(&bytes) {
                                Some(pages) => Observed::Known(pages),
                                None => Observed::Missing(MissingReason::Unavailable),
                            },
                        };
                        // The instance is the PID *and* its creation token: a
                        // reused PID is a different key, so it warms up instead
                        // of inheriting the counters of the process it replaced.
                        let instance = (pid, stat.start_ticks);
                        let cpu = match stat.cpu_ticks {
                            Some(ticks) => {
                                let (usage, keep) = sample_cpu(
                                    self.cpu_baselines.get(&instance),
                                    CpuBaseline { ticks, at: read_at },
                                    &ticks_per_second,
                                );
                                baselines.insert(instance, keep);
                                usage
                            }
                            // No counter, no baseline: the next readable sample
                            // of this instance warms up again.
                            None => CpuUsage::Missing(MissingReason::Unavailable),
                        };
                        records.push(ProcessRecord {
                            key: ProcessKey {
                                source: self.source.clone(),
                                host: host.clone(),
                                boot: boot.clone(),
                                pid_namespace: pid_namespace.clone(),
                                pid: Observed::Known(pid),
                                creation: CreationToken::LinuxBootTicks(stat.start_ticks),
                            },
                            display_name: stat.display_name,
                            resident: resident_bytes(resident_pages, &page_size),
                            cpu,
                        });
                    }
                },
            }
        }
        // Directory order is not meaningful; publish a stable ascending order.
        records.sort_by_key(|record| match record.key.pid {
            Observed::Known(pid) => pid,
            Observed::Missing(_) => u32::MAX,
        });
        // Only what this scan published is kept for the next one, so the map is
        // bounded by the record bound and a process that ended is forgotten.
        self.cpu_baselines = baselines;
        ProcessSnapshot {
            source: self.source.clone(),
            sampled_at,
            records,
            vanished,
            capped,
            completeness: Completeness::from_scan(skipped, issues),
        }
    }
}

/// Whether `self/ns/pid` — always the *reader's* active namespace — describes
/// the records of the scanned mount. Both proofs are required, and each rules
/// out a different real configuration: a mount that does not number this process
/// by its own PID is numbering its records elsewhere (a nested namespace that
/// inherited an outer `/proc`), and a mount that is not the procfs this process
/// runs under belongs to another namespace even when the two numberings happen
/// to agree (a bind-mounted host `/proc`).
fn readers_namespace_describes(numbers_this_process: bool, is_own_procfs: bool) -> bool {
    numbers_this_process && is_own_procfs
}

/// How a hostname read under a given mount may be published.
#[derive(Debug, Clone, PartialEq, Eq)]
enum HostLabel {
    /// Not read through a procfs mount at all: the tree's own file answers for
    /// it, with nothing to qualify.
    Plain,
    /// Published as `<hostname>@uts:[inode]`.
    Qualified(String),
    /// Not an identity, with the reason to publish.
    Unidentifiable(&'static str),
}

/// Decides that, and is deliberately total.
///
/// A hostname is a property of the UTS namespace it was read in, never of the
/// listed processes, so it may only be published when that namespace can be
/// named. `"<hostname>@uts:unknown"` would be worse than withholding it: two
/// readers in *different* namespaces that happen to share a hostname would
/// receive byte-identical known host components, and their keys could alias —
/// the failure the qualifier exists to prevent, reintroduced by its own
/// fallback.
fn host_label(mount: MountKind, uts: Option<String>) -> HostLabel {
    match (mount, uts) {
        (MountKind::Tree, _) => HostLabel::Plain,
        (MountKind::Unproven, _) => HostLabel::Unidentifiable(
            "a procfs mount this scan cannot prove is its own, so its hostname would be the reader's",
        ),
        (MountKind::Own, Some(tag)) => HostLabel::Qualified(tag),
        (MountKind::Own, None) => HostLabel::Unidentifiable(
            "the reader's UTS namespace could not be named, so its hostname identifies nothing",
        ),
    }
}

/// The reader's UTS namespace, as a tag to qualify a hostname with.
///
/// `sys/kernel/hostname` is answered from the reading process's UTS namespace
/// whatever mount it is read through, so the name alone is not an identity of
/// the listed processes — only of the namespace it was read in. Naming that
/// namespace makes the value self-describing: two readers that disagree about
/// the hostname now disagree visibly, instead of both claiming to have named
/// the same machine.
fn uts_namespace_tag() -> Option<String> {
    let link = std::fs::read_link(Path::new(DEFAULT_PROC_ROOT).join("self/ns/uts")).ok()?;
    let inode = parse_namespace_of_kind(&link.to_string_lossy(), "uts")?;
    Some(format!("uts:[{inode}]"))
}

/// Resolves a root to the one tree this source will scan for its whole life.
///
/// An absolute root already names it. A relative root is joined to the working
/// directory *once*, because the scan happens later and a `chdir` in between
/// would otherwise point the same source — same identity, same status line — at
/// a different tree. With no working directory to join to (it was deleted),
/// there is no such tree: the answer is `None`, not the movable original.
///
/// The join is textual on purpose. `canonicalize` would require the root to
/// exist, when a missing root must still scan and report itself unreadable, and
/// it would resolve symlinks, collapsing two deliberately distinct roots into
/// one identity — the opposite of what this field is for.
fn anchor_root(root: &Path, working_directory: Option<&Path>) -> Option<PathBuf> {
    if root.is_absolute() {
        return Some(root.to_path_buf());
    }
    working_directory.map(|working| working.join(root))
}

/// The source identity of a root, lossless in that root's bytes.
///
/// `Path::display` replaces invalid UTF-8 with U+FFFD, so two roots differing
/// only in those bytes would share one `SourceId`, and their records could then
/// compare equal on every component — precisely the cross-root aliasing
/// [`ProcFsSource::with_root`] promises cannot happen. A path that is valid
/// UTF-8 keeps the readable form; anything else is hex-encoded under a distinct
/// prefix, so no byte sequence can reach the same identity by two routes.
fn source_id_for(root: &Path) -> SourceId {
    match root.as_os_str().to_str() {
        Some(text) => SourceId(format!("procfs:{text}")),
        None => {
            let bytes = root.as_os_str().as_encoded_bytes();
            let mut id = String::with_capacity("procfs-bytes:".len() + bytes.len() * 2);
            id.push_str("procfs-bytes:");
            for byte in bytes {
                let _ = write!(id, "{byte:02x}");
            }
            SourceId(id)
        }
    }
}

fn reason_for(error: &io::Error) -> MissingReason {
    match error.kind() {
        io::ErrorKind::PermissionDenied => MissingReason::Denied,
        _ => MissingReason::Unavailable,
    }
}

fn read_bounded(path: &Path) -> io::Result<Vec<u8>> {
    // `/proc` files report size 0, so read through a hard byte bound instead.
    // Reading one byte past the bound is what separates "exactly this long" from
    // "cut here": a stat line cut mid-field still parses — first `(`, last `)`,
    // twenty whitespace fields — and would mint a *wrong* creation token, a
    // stable but false process identity, from the digits that happened to fit.
    // An oversized file is therefore an error, skipped with a reason, never a
    // silently shortened record.
    let mut bytes = Vec::new();
    File::open(path)?
        .take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("exceeds the {MAX_FILE_BYTES} byte read bound"),
        ));
    }
    Ok(bytes)
}

/// Whether a read failed because the process it was reading no longer exists:
/// by path its PID's directory is gone (`ENOENT`), and through a pinned
/// directory the directory is still held but the instance it was opened on has
/// been reaped (`ESRCH`, see [`RecordAccess`]). Either access can meet either
/// error, and both are an exit during the scan, never an unreadable record.
fn ended(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::NotFound || error.raw_os_error() == Some(ESRCH)
}

/// Whether this reader can pin record directories under `root`: it opens the
/// root itself and checks that its own `/proc/self/fd/<n>` link reaches the very
/// directory it opened. Decided per scan, so a reader without that link reads by
/// name instead of taking every record's missing link for a process that ended.
fn record_access(root: &Path) -> RecordAccess {
    let Ok(directory) = File::open(root) else {
        return RecordAccess::ByName;
    };
    match (std::fs::metadata(fd_link(&directory)), directory.metadata()) {
        (Ok(linked), Ok(opened))
            if linked.dev() == opened.dev() && linked.ino() == opened.ino() =>
        {
            RecordAccess::Pinned
        }
        _ => RecordAccess::ByName,
    }
}

/// The reader's own name for an open handle, through its own procfs.
fn fd_link(handle: &File) -> PathBuf {
    Path::new(DEFAULT_PROC_ROOT).join(format!("self/fd/{}", handle.as_raw_fd()))
}

/// Accepts only a plain positive decimal PID directory name.
pub fn parse_pid(name: &[u8]) -> Option<u32> {
    if name.is_empty() || name.len() > 10 || !name.iter().all(u8::is_ascii_digit) {
        return None;
    }
    match std::str::from_utf8(name).ok()?.parse::<u32>().ok()? {
        0 => None,
        pid => Some(pid),
    }
}

fn parse_namespace(link: &str) -> Option<u64> {
    parse_namespace_of_kind(link, "pid")
}

fn parse_namespace_of_kind(link: &str, kind: &str) -> Option<u64> {
    let inner = link
        .strip_prefix(kind)?
        .strip_prefix(":[")?
        .strip_suffix(']')?;
    inner.parse().ok()
}

/// What one `/proc/<pid>/stat` line says about a process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatFields {
    pub display_name: DisplayName,
    /// Field 22: start time in kernel clock ticks since boot. Mandatory — it is
    /// this record's creation token, and a record without one is not identified.
    pub start_ticks: u64,
    /// Fields 14 and 15 summed: user plus kernel CPU time in clock ticks, when
    /// both are plain counts and their sum fits. Optional where the creation
    /// token is not, because a metric is not an identity: an unreadable counter
    /// is an unread metric of an identified process, not a missing process, and
    /// it is reported unread rather than as zero.
    pub cpu_ticks: Option<u64>,
}

/// Parses `comm`, the creation token and the CPU counters out of one
/// `/proc/<pid>/stat` line.
///
/// `comm` is raw bytes wrapped in parentheses and may itself contain spaces,
/// parentheses, control characters and invalid UTF-8 (K1). Fields are therefore
/// located from the first `(` and the **last** `)`, never by splitting on
/// whitespace, so a hostile name cannot shift the field indices. The leading PID
/// field must also match the directory this line came from.
///
/// Field 24 (`rss`) is not read: the kernel fills it from `get_mm_rss`, an
/// approximate count on every kernel since 6.2, and resident memory comes from
/// `statm` instead (PX-005-G01).
pub fn parse_stat(pid: u32, bytes: &[u8]) -> Option<StatFields> {
    let open = bytes.iter().position(|byte| *byte == b'(')?;
    let close = bytes.iter().rposition(|byte| *byte == b')')?;
    if close < open {
        return None;
    }
    if parse_pid(trim_ascii(&bytes[..open])) != Some(pid) {
        return None;
    }
    let fields: Vec<&[u8]> = bytes[close + 1..]
        .split(u8::is_ascii_whitespace)
        .filter(|field| !field.is_empty())
        .take(STARTTIME_FIELD_AFTER_COMM + 1)
        .collect();
    // Every field read here is an unsigned count: a negative or non-numeric
    // field is unread rather than reinterpreted.
    let count = |index: usize| {
        fields
            .get(index)
            .and_then(|field| std::str::from_utf8(field).ok())
            .and_then(|field| field.parse::<u64>().ok())
    };
    let start_ticks = count(STARTTIME_FIELD_AFTER_COMM)?;
    let cpu_ticks = count(UTIME_FIELD_AFTER_COMM)
        .zip(count(STIME_FIELD_AFTER_COMM))
        .and_then(|(user, system)| user.checked_add(system));
    Some(StatFields {
        display_name: DisplayName::sanitize(&bytes[open + 1..close]),
        start_ticks,
        cpu_ticks,
    })
}

/// The `resident` page count of one `/proc/<pid>/statm` line, if the line is
/// exactly what the kernel writes: [`STATM_FIELDS`] plain decimal counts, each
/// fitting a `u64`, separated by single spaces and ended by one newline.
///
/// Anything else is refused as a whole rather than read in part — a field too
/// many or too few, a sign, a non-digit, a count past `u64`, a doubled or
/// leading space, a missing newline — because a count taken from a line that is
/// not the kernel's is not a count of anything. A refused line is an unread
/// metric, never a zero.
fn parse_statm_resident(bytes: &[u8]) -> Option<u64> {
    let line = bytes.strip_suffix(b"\n")?;
    let mut fields = line.split(|byte| *byte == b' ');
    let mut counts = [0u64; STATM_FIELDS];
    for count in &mut counts {
        *count = parse_count(fields.next()?)?;
    }
    if fields.next().is_some() {
        return None;
    }
    Some(counts[STATM_RESIDENT])
}

/// A plain unsigned decimal: digits only, at least one, fitting a `u64`.
fn parse_count(field: &[u8]) -> Option<u64> {
    if field.is_empty() || !field.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(field).ok()?.parse().ok()
}

/// The value of `wanted` in an auxiliary vector, if the vector carries one.
///
/// The vector is pairs of native-endian `unsigned long` words, terminated by
/// [`AT_NULL`] (K1). The word size is this target's pointer width, so a 32-bit
/// build reads the 32-bit vector a 32-bit kernel writes. A trailing partial word
/// ends the walk: half a word is not a value.
fn auxv_value(bytes: &[u8], wanted: u64) -> Option<u64> {
    let word = std::mem::size_of::<usize>();
    for pair in bytes.chunks_exact(word * 2) {
        let read = |slice: &[u8]| {
            let mut native = [0u8; std::mem::size_of::<usize>()];
            native.copy_from_slice(slice);
            usize::from_ne_bytes(native) as u64
        };
        let key = read(&pair[..word]);
        if key == AT_NULL {
            return None;
        }
        if key == wanted {
            return Some(read(&pair[word..]));
        }
    }
    None
}

/// The page size an auxiliary vector reports, if it reports a believable one.
///
/// A value that is not a power of two, or is outside [`MIN_PAGE_SIZE`] ..=
/// [`MAX_PAGE_SIZE`], is refused rather than used. Multiplying a page count by a
/// wrong page size would publish a confident, wrong byte count for every process
/// on the host, which is worse than reporting the metric unread.
fn parse_page_size(bytes: &[u8]) -> Option<u64> {
    auxv_value(bytes, AT_PAGESZ)
        .filter(|value| value.is_power_of_two() && (MIN_PAGE_SIZE..=MAX_PAGE_SIZE).contains(value))
}

/// The clock-tick rate an auxiliary vector reports, if it reports a believable
/// one: at least one tick per second and at most
/// [`MAX_CLOCK_TICKS_PER_SECOND`]. Zero would divide by nothing, and a wrong
/// rate would publish a confident, wrong percentage for every process.
fn parse_clock_ticks(bytes: &[u8]) -> Option<u64> {
    auxv_value(bytes, AT_CLKTCK).filter(|rate| (1..=MAX_CLOCK_TICKS_PER_SECOND).contains(rate))
}

/// A record's resident memory in bytes, or why this scan could not state it.
///
/// Both inputs are needed and neither is guessed: a page count with no page size
/// is not a byte count, and a page size with no count describes nothing. The
/// record's own reason is published before the mount's. The multiplication is
/// checked, so a kernel reporting an absurd count reports the metric unread
/// rather than a wrapped one.
fn resident_bytes(pages: Observed<u64>, page_size: &Observed<u64>) -> Observed<u64> {
    match (pages, page_size) {
        (Observed::Known(pages), Observed::Known(size)) => match pages.checked_mul(*size) {
            Some(bytes) => Observed::Known(bytes),
            None => Observed::Missing(MissingReason::Unavailable),
        },
        (Observed::Missing(reason), _) => Observed::Missing(reason),
        // The page size is a property of the scanned mount's kernel, so its
        // reason applies to every record equally.
        (_, Observed::Missing(reason)) => Observed::Missing(*reason),
    }
}

fn trim_ascii(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|byte| !byte.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map_or(start, |index| index + 1);
    &bytes[start..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    fn at(seconds: u64, ticks: u64) -> CpuBaseline {
        CpuBaseline {
            ticks,
            at: Duration::from_secs(seconds),
        }
    }

    const HZ: Observed<u64> = Observed::Known(100);

    #[test]
    fn a_first_sample_warms_up_and_becomes_the_baseline() {
        let (usage, keep) = sample_cpu(None, at(5, 900), &HZ);
        assert_eq!(usage, CpuUsage::WarmingUp);
        assert_eq!(keep, at(5, 900));
        // With no tick rate there will never be an interval to publish, so the
        // reason is published from the first sample on rather than a warm-up
        // that never ends.
        for reason in [MissingReason::Denied, MissingReason::Unavailable] {
            let (usage, keep) = sample_cpu(None, at(5, 900), &Observed::Missing(reason));
            assert_eq!(usage, CpuUsage::Missing(reason));
            assert_eq!(keep, at(5, 900), "the counter was still read");
        }
    }

    #[test]
    fn an_interval_is_the_counter_delta_over_the_monotonic_delta() {
        let (usage, keep) = sample_cpu(Some(&at(10, 1_000)), at(12, 1_100), &HZ);
        assert_eq!(
            usage,
            CpuUsage::Measured(CpuInterval {
                ticks: 100,
                ticks_per_second: 100,
                elapsed: Duration::from_secs(2),
            })
        );
        assert_eq!(keep, at(12, 1_100));
        assert_eq!(crate::metric::cpu_cell(&usage), "50.0%");
    }

    #[test]
    fn no_interval_is_published_where_none_was_measured() {
        let unavailable = CpuUsage::Missing(MissingReason::Unavailable);
        // The clock did not advance: no divisor. The older baseline is kept, so
        // the next scan measures across a real interval instead of warming up.
        let (usage, keep) = sample_cpu(Some(&at(10, 1_000)), at(10, 1_050), &HZ);
        assert_eq!((usage, keep), (unavailable, at(10, 1_000)));
        // The clock went backwards, which a real monotonic clock never does:
        // the old baseline belongs to a timeline that no longer applies.
        let (usage, keep) = sample_cpu(Some(&at(10, 1_000)), at(9, 1_050), &HZ);
        assert_eq!((usage, keep), (unavailable, at(9, 1_050)));
        // The counter went backwards: a reset, never a huge unsigned delta.
        let (usage, keep) = sample_cpu(Some(&at(10, 1_000)), at(11, 10), &HZ);
        assert_eq!((usage, keep), (unavailable, at(11, 10)));
        // A tick rate lost after the baseline was taken: the reason, not a value.
        let (usage, keep) = sample_cpu(
            Some(&at(10, 1_000)),
            at(11, 1_010),
            &Observed::Missing(MissingReason::Denied),
        );
        assert_eq!(
            (usage, keep),
            (CpuUsage::Missing(MissingReason::Denied), at(11, 1_010))
        );
    }

    #[test]
    fn only_a_different_observed_boot_or_namespace_invalidates_baselines() {
        let mut remembered = None;
        assert!(!identity_changed(
            &mut remembered,
            &Observed::Known(BootId("a".into()))
        ));
        assert!(!identity_changed(
            &mut remembered,
            &Observed::Known(BootId("a".into()))
        ));
        // An unreadable boot ID is not a reboot.
        assert!(!identity_changed(
            &mut remembered,
            &Observed::Missing(MissingReason::Unavailable)
        ));
        assert_eq!(remembered, Some(BootId("a".into())));
        assert!(identity_changed(
            &mut remembered,
            &Observed::Known(BootId("b".into()))
        ));
        assert_eq!(remembered, Some(BootId("b".into())));
    }

    #[test]
    fn the_tick_rate_is_read_from_the_auxiliary_vector_and_refused_when_implausible() {
        let vector = |entries: &[(u64, u64)]| {
            let mut bytes = Vec::new();
            for (key, value) in entries {
                bytes.extend_from_slice(&(*key as usize).to_ne_bytes());
                bytes.extend_from_slice(&(*value as usize).to_ne_bytes());
            }
            bytes
        };
        assert_eq!(
            parse_clock_ticks(&vector(&[
                (AT_PAGESZ, 4096),
                (AT_CLKTCK, 100),
                (AT_NULL, 0)
            ])),
            Some(100)
        );
        assert_eq!(
            parse_clock_ticks(&vector(&[(AT_CLKTCK, 1024), (AT_NULL, 0)])),
            Some(1024)
        );
        for refused in [
            vector(&[(AT_CLKTCK, 0), (AT_NULL, 0)]),
            vector(&[(AT_CLKTCK, MAX_CLOCK_TICKS_PER_SECOND + 1), (AT_NULL, 0)]),
            vector(&[(AT_NULL, 0), (AT_CLKTCK, 100)]),
            vector(&[(AT_PAGESZ, 4096), (AT_NULL, 0)]),
            Vec::new(),
        ] {
            assert_eq!(parse_clock_ticks(&refused), None);
        }
        // The page size is still found beside it.
        assert_eq!(
            parse_page_size(&vector(&[
                (AT_CLKTCK, 100),
                (AT_PAGESZ, 4096),
                (AT_NULL, 0)
            ])),
            Some(4096)
        );
    }

    #[test]
    fn cpu_counters_are_fields_14_and_15_and_an_unreadable_one_is_unread() {
        let line = |utime: &str, stime: &str| {
            let mut fields: Vec<String> = (4..=21).map(|field| field.to_string()).collect();
            fields[14 - 4] = utime.into();
            fields[15 - 4] = stime.into();
            format!("7 (a) b) S {} 900 4096 3 0\n", fields.join(" "))
        };
        let parsed = parse_stat(7, line("30", "12").as_bytes()).unwrap();
        assert_eq!(parsed.cpu_ticks, Some(42));
        assert_eq!(parsed.start_ticks, 900);
        for (utime, stime) in [("-1", "2"), ("x", "2"), ("2", "-5")] {
            let parsed = parse_stat(7, line(utime, stime).as_bytes()).unwrap();
            assert_eq!(parsed.cpu_ticks, None, "{utime} {stime}");
            // The identity is untouched by it.
            assert_eq!(parsed.start_ticks, 900);
        }
        let overflowing = format!("{}", u64::MAX);
        let parsed = parse_stat(7, line(&overflowing, "1").as_bytes()).unwrap();
        assert_eq!(parsed.cpu_ticks, None, "an overflowing sum is not wrapped");
    }

    #[test]
    fn statm_resident_is_the_second_of_exactly_seven_plain_counts() {
        assert_eq!(parse_statm_resident(b"557 256 252 8 0 88 0\n"), Some(256));
        // What the kernel writes for a task with no address space: a kernel
        // thread, a zombie. A known zero, as `stat` field 24 reported it.
        assert_eq!(parse_statm_resident(b"0 0 0 0 0 0 0\n"), Some(0));
        let widest = format!("1 {} 0 0 0 0 0\n", u64::MAX);
        assert_eq!(parse_statm_resident(widest.as_bytes()), Some(u64::MAX));
        let refused: [&[u8]; 17] = [
            b"",
            b"\n",
            b"557 256 252 8 0 88\n",
            b"557 256 252 8 0 88 0 0\n",
            b"557 256 252 8 0 88 0",
            b"557 -256 252 8 0 88 0\n",
            b"557 +256 252 8 0 88 0\n",
            b"557 many 252 8 0 88 0\n",
            b"557 18446744073709551616 252 8 0 88 0\n",
            b"557  256 252 8 0 88 0\n",
            b" 557 256 252 8 0 88 0\n",
            b"557 256 252 8 0 88 0 \n",
            b"557\t256 252 8 0 88 0\n",
            b"557 256 252 8 0 88 0\r\n",
            b"557 256 252 8 0 88 0\n\n",
            // Strict about every field, not only the one it publishes: a line
            // with any field that is not a count is not the kernel's line.
            b"557 256 252 8 x 88 0\n",
            b"557 256 252 8 0 88 -0\n",
        ];
        for line in refused {
            assert_eq!(
                parse_statm_resident(line),
                None,
                "{:?}",
                String::from_utf8_lossy(line)
            );
        }
    }

    #[test]
    fn only_a_process_that_is_gone_counts_as_ended() {
        assert!(ended(&io::Error::from(io::ErrorKind::NotFound)));
        assert!(ended(&io::Error::from_raw_os_error(ESRCH)));
        // Refused, failed or oversized reads are not exits: the record exists.
        assert!(!ended(&io::Error::from(io::ErrorKind::PermissionDenied)));
        assert!(!ended(&io::Error::from_raw_os_error(5)));
        assert!(!ended(&io::Error::new(
            io::ErrorKind::InvalidData,
            "exceeds the read bound"
        )));
    }

    #[test]
    fn this_reader_pins_record_directories_wherever_its_procfs_can_name_them() {
        let nonce = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "srtop-record-access-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir(&root).unwrap();
        let access = record_access(&root);
        std::fs::remove_dir(&root).unwrap();
        // Linux names an open directory through `/proc/self/fd/<n>`; no other
        // system this builds on does, and there every record is read by name.
        let expected = if cfg!(target_os = "linux") {
            RecordAccess::Pinned
        } else {
            RecordAccess::ByName
        };
        assert_eq!(access, expected);
        // A root that cannot be opened is read by name, and its scan reports the
        // root unreadable as it always has, rather than every record vanished.
        assert_eq!(record_access(&root), RecordAccess::ByName);
    }

    /// On a real kernel, a pinned directory is what makes a record's two reads
    /// one instance's: once the process it was opened on has been reaped, a read
    /// through it fails with `ESRCH` — it never reaches a newcomer under the same
    /// PID — and that failure is an exit, not an unreadable record. The worker is
    /// bounded and owned by this test, killed and reaped before the second read.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_pinned_directory_of_a_reaped_process_reads_as_ended() {
        use std::process::{Command, Stdio};
        let mut worker = Command::new("/bin/sleep")
            .arg("60")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("the test owns this worker");
        let root = Path::new(DEFAULT_PROC_ROOT);
        let access = record_access(root);
        let opened = RecordDir::open(root.join(worker.id().to_string()), access);
        let alive = match &opened {
            Ok(directory) => directory.read("statm").map(|_| ()),
            Err(error) => Err(io::Error::new(error.kind(), error.to_string())),
        };
        let _ = worker.kill();
        let _ = worker.wait();
        let directory = opened.expect("the worker existed when its directory was opened");
        assert!(
            alive.is_ok(),
            "a live worker's statm is readable: {alive:?}"
        );
        let error = directory
            .read("statm")
            .expect_err("a reaped process has no statm");
        assert!(ended(&error), "{error:?}");
        if access == RecordAccess::Pinned {
            assert_eq!(error.raw_os_error(), Some(ESRCH), "{error:?}");
        }
    }

    #[test]
    fn an_unnameable_namespace_withholds_the_hostname_rather_than_labelling_it_unknown() {
        let tag = "uts:[4026531838]".to_string();
        assert_eq!(
            host_label(MountKind::Own, Some(tag.clone())),
            HostLabel::Qualified(tag)
        );
        assert_eq!(host_label(MountKind::Tree, None), HostLabel::Plain);
        assert_eq!(
            host_label(MountKind::Tree, Some("uts:[1]".into())),
            HostLabel::Plain,
            "a fixture tree's own file is not read through any namespace"
        );
        // The case this test exists for: an unnameable namespace is not a
        // hostname with an "unknown" suffix. Two readers in different UTS
        // namespaces that share a hostname would otherwise receive identical
        // *known* host components and their keys could alias.
        assert!(matches!(
            host_label(MountKind::Own, None),
            HostLabel::Unidentifiable(_)
        ));
        assert!(matches!(
            host_label(MountKind::Unproven, Some("uts:[1]".into())),
            HostLabel::Unidentifiable(_)
        ));
        assert!(matches!(
            host_label(MountKind::Unproven, None),
            HostLabel::Unidentifiable(_)
        ));
    }

    #[test]
    fn a_hostname_names_the_namespace_it_was_read_in() {
        assert_eq!(
            parse_namespace_of_kind("uts:[4026531838]", "uts"),
            Some(4_026_531_838)
        );
        assert_eq!(
            parse_namespace_of_kind("pid:[4026531836]", "pid"),
            Some(4_026_531_836)
        );
        // A namespace link of another kind is never mistaken for this one: a
        // hostname qualified with a PID-namespace inode would compare equal
        // across two UTS namespaces that share a PID namespace, which is the
        // case this tag exists to separate.
        assert_eq!(parse_namespace_of_kind("pid:[4026531836]", "uts"), None);
        assert_eq!(parse_namespace_of_kind("uts:[4026531838]", "pid"), None);
        assert_eq!(parse_namespace_of_kind("uts:[]", "uts"), None);
        assert_eq!(parse_namespace_of_kind("uts:4026531838", "uts"), None);
        // On a host with a readable `/proc/self/ns/uts` the tag is well formed;
        // elsewhere it is absent and the hostname says so rather than implying
        // a namespace it cannot name.
        if let Some(tag) = uts_namespace_tag() {
            assert!(tag.starts_with("uts:[") && tag.ends_with(']'), "{tag}");
            assert!(parse_namespace_of_kind(&tag, "uts").is_some(), "{tag}");
        }
    }

    #[test]
    fn a_relative_root_with_no_working_directory_is_never_scanned() {
        // `current_dir` fails when the working directory has been deleted. The
        // relative root then names no fixed tree, so keeping it would recreate
        // exactly the aliasing anchoring exists to prevent.
        assert_eq!(anchor_root(Path::new("proc"), None), None);
        assert_eq!(
            anchor_root(Path::new("proc"), Some(Path::new("/var/empty"))),
            Some(PathBuf::from("/var/empty/proc"))
        );
        assert_eq!(
            anchor_root(Path::new(DEFAULT_PROC_ROOT), None),
            Some(PathBuf::from(DEFAULT_PROC_ROOT)),
            "an absolute root needs no working directory"
        );

        let mut unanchored = ProcFsSource {
            root: PathBuf::from("proc"),
            source: SourceId("procfs-unanchored:proc".into()),
            status: "unanchored".into(),
            anchored: false,
            ..ProcFsSource::rooted(PathBuf::from("proc"), String::new())
        };
        let snapshot = unanchored.snapshot();
        assert!(snapshot.records.is_empty());
        assert!(
            !snapshot.completeness.is_complete(),
            "an unscannable source is never an authoritative empty result"
        );
        assert_eq!(
            snapshot
                .completeness
                .issues()
                .iter()
                .map(|issue| issue.scope)
                .collect::<Vec<_>>(),
            vec![IssueScope::Root]
        );
        // Its identity cannot collide with an anchored source's.
        assert_ne!(unanchored.source_id(), &source_id_for(Path::new("proc")));
    }

    #[test]
    fn a_relative_root_is_anchored_so_a_later_chdir_cannot_move_the_scan() {
        let source = ProcFsSource::with_root("srtop-relative-root");
        let anchored = std::env::current_dir()
            .expect("a test process has a working directory")
            .join("srtop-relative-root");
        assert_eq!(
            source.source_id(),
            &SourceId(format!("procfs:{}", anchored.display())),
            "the identity names the tree that will actually be scanned"
        );
        assert!(source
            .status_text()
            .contains(&anchored.display().to_string()));
        // An absolute root is untouched.
        assert_eq!(
            ProcFsSource::with_root(DEFAULT_PROC_ROOT).source_id(),
            &SourceId("procfs:/proc".into())
        );
    }

    #[test]
    fn roots_differing_only_in_invalid_utf8_never_share_a_source_identity() {
        let first = Path::new(OsStr::from_bytes(b"/tmp/procfs-\xff"));
        let second = Path::new(OsStr::from_bytes(b"/tmp/procfs-\xfe"));
        // Both display as the same text, which is exactly why the identity
        // cannot be built from `Path::display`.
        assert_eq!(first.display().to_string(), second.display().to_string());
        assert_ne!(source_id_for(first), source_id_for(second));
        // A readable root keeps its readable identity.
        assert_eq!(
            source_id_for(Path::new(DEFAULT_PROC_ROOT)),
            SourceId("procfs:/proc".into())
        );
        // The two encodings live in separate namespaces, so a path whose text
        // looks like an encoded one cannot collide with it.
        assert!(source_id_for(first).0.starts_with("procfs-bytes:"));
        assert_ne!(
            source_id_for(first),
            source_id_for(Path::new(&source_id_for(first).0))
        );
    }

    #[test]
    fn the_readers_namespace_describes_records_only_under_both_proofs() {
        // The mount this process runs under and is numbered by.
        assert!(readers_namespace_describes(true, true));
        // A nested PID namespace that inherited an outer `/proc`: the same
        // mount, so the same superblock, but it still numbers this process by
        // its outer PID while the link names the inner namespace.
        assert!(!readers_namespace_describes(false, true));
        // A bind-mounted host `/proc` whose numbering happens to agree: PIDs
        // coincide across namespaces, so agreement proves nothing on its own.
        assert!(!readers_namespace_describes(true, false));
        assert!(!readers_namespace_describes(false, false));
    }
}
