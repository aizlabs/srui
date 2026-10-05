//! PX-003 acceptance: one real Linux snapshot, process-instance identity, and
//! degraded scans that never masquerade as an authoritative empty result
//! (K1; §§8, 12, 22, 29).
//!
//! Fixture roots make every branch deterministic on any platform; the live
//! `/proc` cases are compiled for Linux, where they must run for real. This
//! suite must run unprivileged, like the app itself: the denied-record case
//! relies on mode 0 being unreadable.
use srui_process_explorer::procfs::{
    parse_pid, parse_stat, MonotonicClock, ProcFsSource, LIVE_STATUS_TEXT, MAX_FILE_BYTES,
};
use srui_process_explorer::published_status;
use srui_process_explorer::source::{
    Completeness, CpuInterval, CpuUsage, CreationToken, DisplayName, IssueScope, MissingReason,
    Observed, ProcessSource, Retention, SourceId, FAKE_STATUS_TEXT, MAX_RECORDED_ISSUES,
};
use std::collections::BTreeSet;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

struct ProcFixture(PathBuf);

impl ProcFixture {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = PathBuf::from(format!(
            "/tmp/srtop-procfs-{}-{nonce}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path)
    }

    /// Identity files, plus the page size the scan converts resident pages with.
    /// A fixture states its own page size for the same reason it states its own
    /// hostname: a canned tree is not this machine, and a test that borrowed this
    /// machine's page size would publish different byte counts on a 4 KiB host
    /// and a 16 KiB one.
    fn identity(&self, hostname: &str, boot_id: &str, namespace: &str) -> &Self {
        fs::create_dir_all(self.0.join("sys/kernel/random")).unwrap();
        fs::write(self.0.join("sys/kernel/hostname"), format!("{hostname}\n")).unwrap();
        fs::write(
            self.0.join("sys/kernel/random/boot_id"),
            format!("{boot_id}\n"),
        )
        .unwrap();
        fs::create_dir_all(self.0.join("self/ns")).unwrap();
        std::os::unix::fs::symlink(namespace, self.0.join("self/ns/pid")).unwrap();
        self.page_size(FIXTURE_PAGE_SIZE)
    }

    /// The page size this tree reports through `AT_PAGESZ` in its own
    /// `self/auxv`, exactly as a kernel reports one.
    fn page_size(&self, bytes: u64) -> &Self {
        self.auxv(&auxv(&[(AT_PAGESZ, bytes), (AT_NULL, 0)]))
    }

    /// A verbatim auxiliary vector, for the cases a well-formed one cannot cover.
    fn auxv(&self, bytes: &[u8]) -> &Self {
        let path = self.0.join("self");
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("auxv"), bytes).unwrap();
        self
    }

    fn process(&self, pid: u32, comm: &[u8], ticks: u64) -> &Self {
        self.raw(pid, &stat_line(pid, comm, ticks))
    }

    /// A record whose `stat` reports `resident_pages` in field 24.
    fn resident(&self, pid: u32, comm: &[u8], ticks: u64, resident_pages: u64) -> &Self {
        self.raw(
            pid,
            &stat_line_with_resident(pid, comm, ticks, &resident_pages.to_string()),
        )
    }

    /// A record whose `stat` reports `field` where its resident page count
    /// belongs — a count no scan can read as a number of pages.
    fn unreadable_resident(&self, pid: u32, comm: &[u8], ticks: u64, field: &str) -> &Self {
        self.raw(pid, &stat_line_with_resident(pid, comm, ticks, field))
    }

    /// A record whose `stat` reports `utime` and `stime` verbatim as fields 14
    /// and 15 (PX-006).
    fn cpu(&self, pid: u32, comm: &[u8], ticks: u64, utime: &str, stime: &str) -> &Self {
        self.raw(pid, &stat_line_with_cpu(pid, comm, ticks, utime, stime))
    }

    /// The page size and the clock-tick rate this tree reports, through
    /// `AT_PAGESZ` and `AT_CLKTCK` in its own `self/auxv`.
    fn clock_ticks(&self, hz: u64) -> &Self {
        self.auxv(&auxv(&[
            (AT_PAGESZ, FIXTURE_PAGE_SIZE),
            (AT_CLKTCK, hz),
            (AT_NULL, 0),
        ]))
    }

    fn raw(&self, pid: u32, stat: &[u8]) -> &Self {
        let directory = self.0.join(pid.to_string());
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("stat"), stat).unwrap();
        self
    }

    /// A record that exists but cannot be read by this unprivileged scan.
    fn denied(&self, pid: u32) -> &Self {
        self.process(pid, b"secret", 500);
        let stat = self.0.join(pid.to_string()).join("stat");
        fs::set_permissions(&stat, fs::Permissions::from_mode(0o000)).unwrap();
        self
    }

    /// The namespace init of the scanned mount: the task it numbers 1, with the
    /// `ns/pid` link that names the namespace its records are numbered in.
    fn namespace_init(&self, namespace: &str) -> &Self {
        fs::create_dir_all(self.0.join("1/ns")).unwrap();
        std::os::unix::fs::symlink(namespace, self.0.join("1/ns/pid")).unwrap();
        self
    }

    /// Shapes the root like a real procfs mount whose `self` symlink names this
    /// process as *that mount* numbers it, with its own `ns/pid` link beneath.
    fn self_numbered(&self, named: u32, namespace: &str) -> &Self {
        let _ = fs::remove_dir_all(self.0.join("self"));
        let _ = fs::remove_file(self.0.join("self"));
        fs::create_dir_all(self.0.join(format!("{named}/ns"))).unwrap();
        std::os::unix::fs::symlink(namespace, self.0.join(format!("{named}/ns/pid"))).unwrap();
        std::os::unix::fs::symlink(named.to_string(), self.0.join("self")).unwrap();
        // `self` now resolves elsewhere, so the page size this tree reported
        // through the directory it replaced is restored behind the new link.
        fs::write(
            self.0.join(format!("{named}/auxv")),
            auxv(&[(AT_PAGESZ, FIXTURE_PAGE_SIZE), (AT_NULL, 0)]),
        )
        .unwrap();
        self
    }

    /// A record that disappeared between listing the directory and reading it.
    fn vanished(&self, pid: u32) -> &Self {
        fs::create_dir_all(self.0.join(pid.to_string())).unwrap();
        self
    }

    fn noise(&self, name: &str) -> &Self {
        fs::create_dir_all(self.0.join(name)).unwrap();
        self
    }

    fn source(&self) -> ProcFsSource {
        ProcFsSource::with_root(&self.0)
    }
}

impl Drop for ProcFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Page size every fixture tree reports unless a test states another one. Chosen
/// to differ from this machine's: a byte count built from the host's page size
/// would be a different number on a 4 KiB host and a 16 KiB one.
const FIXTURE_PAGE_SIZE: u64 = 8192;
/// `AT_PAGESZ` and `AT_NULL` as the kernel writes them into an auxiliary vector
/// (K1, `getauxval(3)`). Transcribed here rather than imported, so the collector
/// and this suite cannot drift onto the same wrong constant.
const AT_PAGESZ: u64 = 6;
const AT_NULL: u64 = 0;
/// `AT_CLKTCK`, likewise transcribed from `getauxval(3)` (PX-006).
const AT_CLKTCK: u64 = 17;

/// `entries` as an auxiliary vector: pairs of native-endian pointer-width words,
/// exactly the layout `/proc/<pid>/auxv` carries.
fn auxv(entries: &[(u64, u64)]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for (key, value) in entries {
        for word in [key, value] {
            bytes.extend_from_slice(&(*word as usize).to_ne_bytes());
        }
    }
    bytes
}

/// `/proc/<pid>/stat`: `pid (comm) state ...` with start time as field 22 and a
/// resident page count of zero as field 24 (K1).
fn stat_line(pid: u32, comm: &[u8], ticks: u64) -> Vec<u8> {
    stat_line_with_resident(pid, comm, ticks, "0")
}

/// The same line with `resident` written verbatim as field 24, so a test can
/// supply a count, a nonsense field, or nothing at all.
fn stat_line_with_resident(pid: u32, comm: &[u8], ticks: u64, resident: &str) -> Vec<u8> {
    let mut line = format!("{pid} (").into_bytes();
    line.extend_from_slice(comm);
    line.extend_from_slice(b") S");
    for filler in 1..=18 {
        line.extend_from_slice(format!(" {filler}").as_bytes());
    }
    // Field 22 is the start time, 23 the virtual size, 24 the resident pages.
    line.extend_from_slice(format!(" {ticks} 4096 {resident} 18446744073709551615\n").as_bytes());
    line
}

/// The same line with `utime` and `stime` written verbatim as fields 14 and 15
/// (K1, `proc_pid_stat(5)`), and a resident count of zero.
fn stat_line_with_cpu(pid: u32, comm: &[u8], ticks: u64, utime: &str, stime: &str) -> Vec<u8> {
    let mut line = format!("{pid} (").into_bytes();
    line.extend_from_slice(comm);
    line.extend_from_slice(b") S");
    // Fields 4 through 21; field 14 is the 11th of them and field 15 the 12th.
    for (field, filler) in (4..=21).zip(1..=18) {
        let value = match field {
            14 => utime.to_string(),
            15 => stime.to_string(),
            _ => filler.to_string(),
        };
        line.extend_from_slice(format!(" {value}").as_bytes());
    }
    line.extend_from_slice(format!(" {ticks} 4096 0 18446744073709551615\n").as_bytes());
    line
}

#[test]
fn fixture_scan_reports_identity_order_and_an_authoritative_complete_result() {
    let fixture = ProcFixture::new();
    fixture
        .identity(
            "fixture-host",
            "11111111-2222-3333-4444-555555555555",
            "pid:[4026531836]",
        )
        .process(1, b"systemd", 7)
        .process(931, b"sleep", 88_812)
        .noise("cpuinfo")
        .noise("self");
    let snapshot = fixture.source().snapshot();
    assert_eq!(
        snapshot.source,
        SourceId(format!("procfs:{}", fixture.0.display()))
    );
    assert_eq!(snapshot.completeness, Completeness::Complete);
    assert!(snapshot.completeness.is_complete());
    let pids: Vec<_> = snapshot.records.iter().map(|r| r.key.pid.clone()).collect();
    assert_eq!(pids, vec![Observed::Known(1), Observed::Known(931)]);
    let worker = &snapshot.records[1];
    assert_eq!(worker.display_name.as_str(), "sleep");
    assert_eq!(worker.key.creation, CreationToken::LinuxBootTicks(88_812));
    assert_eq!(
        worker.key.pid_namespace,
        Observed::Known(srui_process_explorer::source::PidNamespaceId(4_026_531_836))
    );
    assert!(matches!(worker.key.boot, Observed::Known(_)));
    assert!(matches!(worker.key.host, Observed::Known(_)));
    assert_eq!(worker.key.source, snapshot.source);
    // Every component but the creation token matches PID 1's record.
    assert_ne!(worker.key, snapshot.records[0].key);
    assert_eq!(worker.key.boot, snapshot.records[0].key.boot);
}

#[test]
fn a_process_filesystem_source_never_describes_itself_as_a_fixture_source() {
    let fixture = ProcFixture::new();
    assert_eq!(ProcFsSource::live().status_text(), LIVE_STATUS_TEXT);
    assert_ne!(ProcFsSource::live().status_text(), FAKE_STATUS_TEXT);
    let scoped = fixture.source();
    assert_ne!(scoped.status_text(), LIVE_STATUS_TEXT);
    assert_ne!(scoped.status_text(), FAKE_STATUS_TEXT);
    assert!(scoped
        .status_text()
        .contains(&fixture.0.display().to_string()));
    // A scoped root states what it read and from where. It claims neither
    // liveness nor synthesis, because the path cannot tell them apart: a bind
    // mount of the host's real `/proc` is live data, and labeling those rows a
    // fixture would describe live processes as synthetic.
    for source in [scoped, ProcFsSource::with_root("/host/proc")] {
        let status = source.status_text().to_lowercase();
        assert!(!status.contains("fixture"), "{status}");
        assert!(!status.contains("fake"), "{status}");
        assert!(!status.contains("live"), "{status}");
    }
}

#[test]
fn an_empty_but_readable_root_is_an_authoritative_empty_result() {
    let fixture = ProcFixture::new();
    fixture.identity("fixture-host", "boot-a", "pid:[4026531836]");
    let snapshot = fixture.source().snapshot();
    assert!(snapshot.records.is_empty());
    assert_eq!(snapshot.completeness, Completeness::Complete);
}

#[test]
fn an_unreadable_root_is_incomplete_and_never_an_empty_result() {
    let mut source = ProcFsSource::with_root("/tmp/srtop-procfs-absent-root");
    let snapshot = source.snapshot();
    assert!(snapshot.records.is_empty());
    assert!(!snapshot.completeness.is_complete());
    assert!(snapshot
        .completeness
        .issues()
        .iter()
        .any(|issue| issue.scope == IssueScope::Root));
    // Nothing was reached, so the published line must not claim that every
    // record was readable beside an empty table.
    // An absent root also has no identity files, and both facts are published.
    let published = published_status(LIVE_STATUS_TEXT, &snapshot);
    assert_eq!(
        published,
        format!(
            "{LIVE_STATUS_TEXT} · incomplete scan · process list unavailable: unavailable · \
             host identity incomplete"
        )
    );
    assert!(!published.contains("unreadable"), "{published}");
}

#[test]
fn one_inaccessible_record_is_skipped_with_a_reason_without_failing_the_scan() {
    let fixture = ProcFixture::new();
    fixture
        .identity("fixture-host", "boot-a", "pid:[4026531836]")
        .process(1, b"systemd", 7)
        .denied(4242)
        .raw(4444, b"garbage without fields\n")
        .raw(4545, &stat_line(9999, b"mismatched", 12));
    let snapshot = fixture.source().snapshot();
    assert_eq!(snapshot.records.len(), 1, "the readable record survives");
    assert_eq!(snapshot.records[0].display_name.as_str(), "systemd");
    assert_eq!(snapshot.completeness.skipped(), 3);
    let issues = snapshot.completeness.issues();
    assert_eq!(issues.len(), 3);
    assert_eq!(
        issues
            .iter()
            .find(|issue| issue.scope == IssueScope::Process(4242))
            .map(|issue| issue.reason),
        Some(MissingReason::Denied),
        "a denied record keeps its reason and never becomes zero or empty"
    );
    for pid in [4444, 4545] {
        assert_eq!(
            issues
                .iter()
                .find(|issue| issue.scope == IssueScope::Process(pid))
                .map(|issue| issue.reason),
            Some(MissingReason::Unavailable)
        );
    }
}

#[test]
fn a_process_that_exits_during_the_scan_is_not_a_degradation() {
    let fixture = ProcFixture::new();
    fixture
        .identity("fixture-host", "boot-a", "pid:[4026531836]")
        .process(1, b"systemd", 7)
        .vanished(4343)
        .vanished(4344);
    let snapshot = fixture.source().snapshot();
    // Ordinary churn: the record no longer existed when the scan reached it.
    // Every readable process is listed, so the answer is still authoritative and
    // the shell is not labeled "incomplete scan" on every busy-host sample.
    assert_eq!(snapshot.records.len(), 1);
    assert_eq!(snapshot.vanished, 2);
    assert_eq!(snapshot.completeness, Completeness::Complete);
    assert_eq!(snapshot.completeness.skipped(), 0);
    assert!(snapshot.completeness.issues().is_empty());
    assert_eq!(
        published_status(LIVE_STATUS_TEXT, &snapshot),
        LIVE_STATUS_TEXT
    );
    // A record that exists but cannot be read is still a degradation, and stays
    // distinguishable from one that exited.
    fixture.denied(4242);
    let degraded = fixture.source().snapshot();
    assert_eq!(degraded.vanished, 2);
    assert_eq!(degraded.completeness.skipped(), 1);
    assert!(!degraded.completeness.is_complete());
}

#[test]
fn a_wholly_unreadable_root_keeps_issue_memory_bounded_while_counting_every_record() {
    let fixture = ProcFixture::new();
    fixture.identity("fixture-host", "boot-a", "pid:[4026531836]");
    let denied = MAX_RECORDED_ISSUES + 200;
    for pid in 0..denied as u32 {
        fixture.denied(pid + 1);
    }
    let snapshot = fixture.source().snapshot();
    assert!(snapshot.records.is_empty());
    assert!(
        !snapshot.completeness.is_complete(),
        "a host whose records are all unreadable is never an authoritative empty result"
    );
    assert_eq!(
        snapshot.completeness.skipped(),
        denied,
        "the count of what could not be read is never bounded"
    );
    assert_eq!(
        snapshot.completeness.issues().len(),
        MAX_RECORDED_ISSUES,
        "retained explanations stay at the bound"
    );
    // What the scan skipped is its own knowledge, not a reading of the
    // explanations it kept: every one of those records is still named, so an
    // absence this scan did not name is still a real exit.
    let skipped = snapshot
        .completeness
        .skipped_records()
        .expect("a degraded scan reports what it skipped");
    assert_eq!(skipped.pids().len(), denied);
    assert!(skipped.is_enumerable());
    let Retention::Skipped(uncertain) = snapshot.retention() else {
        panic!("a scan that named every record it skipped can enumerate them")
    };
    assert_eq!(uncertain.len(), denied);
    assert!(
        !uncertain.contains(&(denied as u32 + 1)),
        "and no other PID"
    );
}

#[test]
fn a_scan_that_skips_more_records_than_its_bound_can_name_says_so_instead_of_growing() {
    let fixture = ProcFixture::new();
    fixture.identity("fixture-host", "boot-a", "pid:[4026531836]");
    let bound = 2;
    for pid in 1..=10u32 {
        fixture.denied(pid);
    }
    let snapshot = fixture.source().with_record_limit(bound).snapshot();
    assert!(snapshot.records.is_empty());
    assert_eq!(
        snapshot.completeness.skipped(),
        10,
        "the count is the truth"
    );
    let skipped = snapshot
        .completeness
        .skipped_records()
        .expect("a degraded scan reports what it skipped");
    assert_eq!(
        skipped.pids().len(),
        bound,
        "the skipped identities never exceed the scan's own record bound"
    );
    assert!(!skipped.is_enumerable());
    // Unable to name everything it skipped, the scan keeps every absent row.
    assert_eq!(snapshot.retention(), Retention::Unenumerable);
}

/// The capped ledger is bounded like the skipped one: a scan that listed more
/// entries past its record bound than it may name says so, rather than growing a
/// set per scan, and that answer keeps every absent row (PX-004 round 3).
#[test]
fn more_capped_entries_than_the_ledger_can_name_says_so_instead_of_growing() {
    let fixture = ProcFixture::new();
    fixture.identity("fixture-host", "boot-a", "pid:[4026531836]");
    for pid in 1..=5u32 {
        fixture.process(pid, b"worker", 100 + u64::from(pid));
    }
    let snapshot = fixture
        .source()
        .with_record_limit(2)
        .with_uncertain_limit(1)
        .snapshot();
    assert_eq!(snapshot.records.len(), 2);
    assert_eq!(snapshot.capped.count(), 3, "the count is the truth");
    assert_eq!(
        snapshot.capped.pids().len(),
        1,
        "the named entries never exceed the ledger's own bound"
    );
    assert!(!snapshot.capped.is_enumerable());
    // Unable to name everything it left out, the scan keeps every absent row.
    assert_eq!(snapshot.retention(), Retention::Unenumerable);
    // The status still counts what happened, and still claims no read failure.
    let published = published_status(LIVE_STATUS_TEXT, &snapshot);
    assert_eq!(
        published,
        format!(
            "{LIVE_STATUS_TEXT} · incomplete scan · 2 processes listed · 3 beyond the record limit"
        )
    );
    assert!(!published.contains("unreadable"), "{published}");
}

#[test]
fn a_mount_numbering_pids_in_another_namespace_never_stamps_this_one() {
    let foreign = ProcFixture::new();
    foreign
        .identity("fixture-host", "boot-a", "pid:[4026531836]")
        .process(1, b"systemd", 7)
        // A host `/proc` seen from a container: the mount names this process by
        // a number that is not its PID here, and its `ns/pid` link is the
        // reader's own namespace, not the one those PIDs are numbered in.
        .self_numbered(u32::MAX - 1, "pid:[4026531836]");
    let snapshot = foreign.source().snapshot();
    assert_eq!(
        snapshot.records[0].key.pid_namespace,
        Observed::Missing(MissingReason::Unavailable),
        "a namespace that cannot be proven is never guessed from the caller's"
    );
    assert!(snapshot
        .completeness
        .issues()
        .iter()
        .any(|issue| issue.scope == IssueScope::PidNamespace));
    assert!(!snapshot.completeness.is_complete());

    // The host identity is withheld for the same reason. `sys/kernel/hostname`
    // is a sysctl the kernel answers from the *reader's* UTS namespace, so a
    // mount that cannot be proven this process's own would otherwise stamp its
    // records with a hostname belonging to none of them.
    assert_eq!(
        snapshot.records[0].key.host,
        Observed::Missing(MissingReason::Unavailable),
        "the reader's hostname is never published as a foreign mount's identity"
    );
    assert!(snapshot
        .completeness
        .issues()
        .iter()
        .any(|issue| issue.scope == IssueScope::HostIdentity));
    // The boot identity is one value per running kernel, not per namespace, so
    // it is still read.
    assert!(matches!(snapshot.records[0].key.boot, Observed::Known(_)));

    // A foreign mount that happens to number this process with the same value
    // it has here is still foreign: PIDs coincide across namespaces, so numeric
    // equality proves nothing and must not unlock the reader's namespace.
    let coincidence = ProcFixture::new();
    coincidence
        .identity("fixture-host", "boot-a", "pid:[4026531836]")
        .process(1, b"systemd", 7)
        .self_numbered(std::process::id(), "pid:[4026531836]");
    let snapshot = coincidence.source().snapshot();
    assert_eq!(
        snapshot.records[0].key.pid_namespace,
        Observed::Missing(MissingReason::Unavailable)
    );

    // When the mount's own namespace init is readable it answers directly, and
    // outranks the reader's link: these records are numbered in the mount's
    // namespace, not in whichever one this process happens to be in.
    let readable_init = ProcFixture::new();
    readable_init
        .identity("fixture-host", "boot-a", "pid:[4026531836]")
        .process(1, b"systemd", 7)
        .namespace_init("pid:[4026599999]")
        .self_numbered(u32::MAX - 1, "pid:[4026531836]");
    let snapshot = readable_init.source().snapshot();
    assert_eq!(
        snapshot.records[0].key.pid_namespace,
        Observed::Known(srui_process_explorer::source::PidNamespaceId(4_026_599_999))
    );
    assert!(snapshot
        .completeness
        .issues()
        .iter()
        .all(|issue| issue.scope != IssueScope::PidNamespace));

    // A fixture tree is not a procfs mount — it has no `self` symlink — so its
    // own identity files still answer for it, and the live `/proc` case, where
    // the mount *is* this process's procfs, is covered by
    // `live_snapshot_locates_the_test_owned_sleeping_worker`.
    let tree = ProcFixture::new();
    tree.identity("fixture-host", "boot-a", "pid:[4026531999]")
        .process(1, b"systemd", 7);
    let snapshot = tree.source().snapshot();
    assert_eq!(
        snapshot.records[0].key.pid_namespace,
        Observed::Known(srui_process_explorer::source::PidNamespaceId(4_026_531_999))
    );
    assert!(matches!(snapshot.records[0].key.host, Observed::Known(_)));
    assert_eq!(snapshot.completeness, Completeness::Complete);
}

#[test]
fn records_beyond_the_collector_limit_are_never_reported_as_unreadable() {
    let fixture = ProcFixture::new();
    fixture.identity("fixture-host", "boot-a", "pid:[4026531836]");
    for pid in 1..=5u32 {
        fixture.process(pid, b"worker", 100 + u64::from(pid));
    }
    let snapshot = fixture.source().with_record_limit(2).snapshot();
    assert_eq!(snapshot.records.len(), 2);
    // The omitted entries were never opened: nothing denied or hid them.
    assert_eq!(snapshot.capped.count(), 3);
    assert_eq!(snapshot.completeness.skipped(), 0);
    // The listing that produced them still named them, so their absences stay
    // attributable and this scan deletes the rows of processes that really ended
    // (PX-004 round 3).
    assert!(snapshot.capped.is_enumerable());
    let Retention::Skipped(uncertain) = snapshot.retention() else {
        panic!("a capped scan enumerates the entries it listed and never read")
    };
    assert_eq!(uncertain.len(), 3);
    // Directory order decides which two of the five were read; between them the
    // published records and the named entries account for every listed PID, and
    // for no other.
    let published_pids: BTreeSet<u32> = snapshot
        .records
        .iter()
        .map(|record| match record.key.pid {
            Observed::Known(pid) => pid,
            Observed::Missing(_) => panic!("a fixture record has an observable PID"),
        })
        .collect();
    assert!(published_pids.is_disjoint(&uncertain));
    assert_eq!(
        published_pids
            .union(&uncertain)
            .copied()
            .collect::<Vec<_>>(),
        (1..=5u32).collect::<Vec<_>>()
    );
    let issues = snapshot.completeness.issues();
    assert_eq!(
        issues.len(),
        1,
        "the bound is explained once, not per entry"
    );
    assert_eq!(issues[0].scope, IssueScope::Limit);
    // The list is still not authoritative, but it is not a read failure either.
    assert!(!snapshot.completeness.is_complete());
    let published = published_status(LIVE_STATUS_TEXT, &snapshot);
    assert_eq!(
        published,
        format!(
            "{LIVE_STATUS_TEXT} · incomplete scan · 2 processes listed · 3 beyond the record limit"
        )
    );
    assert!(!published.contains("unreadable"), "{published}");
}

#[test]
fn missing_identity_files_degrade_explicitly_instead_of_aliasing_silently() {
    let fixture = ProcFixture::new();
    fixture.process(1, b"systemd", 7);
    let snapshot = fixture.source().snapshot();
    assert_eq!(snapshot.records.len(), 1);
    assert_eq!(snapshot.completeness.skipped(), 0);
    assert!(!snapshot.completeness.is_complete());
    let scopes: Vec<_> = snapshot
        .completeness
        .issues()
        .iter()
        .map(|issue| issue.scope)
        .collect();
    for scope in [
        IssueScope::HostIdentity,
        IssueScope::BootIdentity,
        IssueScope::PidNamespace,
    ] {
        assert!(
            scopes.contains(&scope),
            "missing {scope:?} must be reported"
        );
    }
    let key = &snapshot.records[0].key;
    assert_eq!(key.boot, Observed::Missing(MissingReason::Unavailable));
    assert_eq!(key.host, Observed::Missing(MissingReason::Unavailable));
    // Every process that exists was listed: the degradation is the identity, and
    // saying "0 unreadable" would contradict that.
    let published = published_status(LIVE_STATUS_TEXT, &snapshot);
    assert_eq!(
        published,
        format!(
            "{LIVE_STATUS_TEXT} · incomplete scan · 1 process listed · host identity incomplete"
        )
    );
    assert!(!published.contains("unreadable"), "{published}");
}

#[test]
fn the_same_pid_with_a_different_creation_token_is_a_different_instance() {
    let first = ProcFixture::new();
    let second = ProcFixture::new();
    for fixture in [&first, &second] {
        fixture.identity("same-host", "same-boot", "pid:[4026531836]");
    }
    first.process(700, b"worker", 10_000);
    // The PID was reused after the first instance exited: same number, same
    // name, same host and boot, later creation token.
    second.process(700, b"worker", 10_500);
    let before = first.source().snapshot();
    let after = second.source().snapshot();
    let mut reused = after.records[0].key.clone();
    reused.source = before.records[0].key.source.clone();
    assert_eq!(reused.pid, before.records[0].key.pid);
    assert_eq!(
        after.records[0].display_name,
        before.records[0].display_name
    );
    assert_ne!(reused, before.records[0].key);
    assert_eq!(
        before.records[0].key.creation,
        CreationToken::LinuxBootTicks(10_000)
    );
    assert_eq!(reused.creation, CreationToken::LinuxBootTicks(10_500));
}

#[test]
fn hostile_names_cannot_shift_fields_or_reach_the_ui_as_live_content() {
    let hostile: &[u8] = b"ev\x07il ) (na\xffme) \x1b[31m";
    let fixture = ProcFixture::new();
    fixture
        .identity("fixture-host", "boot-a", "pid:[4026531836]")
        .process(808, hostile, 4_242_424)
        .process(809, b"$(reboot); rm -rf /", 9)
        .process(810, b"  spaced name  ", 10)
        .process(811, b"", 11)
        // Invisible and line-breaking characters outside Cc: a name that would
        // otherwise impersonate another row in the table.
        .process(812, "a\u{2028}sshd".as_bytes(), 12)
        .process(813, "a\u{3164}sshd".as_bytes(), 13);
    let snapshot = fixture.source().snapshot();
    assert_eq!(snapshot.completeness, Completeness::Complete);
    assert_eq!(snapshot.records.len(), 6);
    // Parsing is unaffected: the creation token still comes from field 22.
    assert_eq!(
        snapshot.records[0].key.creation,
        CreationToken::LinuxBootTicks(4_242_424)
    );
    let names: Vec<&str> = snapshot
        .records
        .iter()
        .map(|record| record.display_name.as_str())
        .collect();
    assert_eq!(
        names,
        vec![
            "ev\u{fffd}il ) (na\u{fffd}me) \u{fffd}[31m",
            "$(reboot); rm -rf /",
            "spaced name",
            "(unnamed)",
            "a\u{fffd}sshd",
            "a\u{fffd}sshd",
        ]
    );
    for name in names {
        assert!(!name.chars().any(DisplayName::is_unsafe), "{name:?}");
    }
    // Direct parser checks, independent of the directory scan.
    assert_eq!(
        parse_stat(808, &stat_line(808, hostile, 77)).map(|stat| stat.start_ticks),
        Some(77)
    );
    assert_eq!(parse_stat(808, &stat_line(809, b"x", 77)), None);
    assert_eq!(
        parse_stat(1, b"1 (x) S"),
        None,
        "a truncated line is not a process"
    );
    assert_eq!(parse_stat(1, b"1 )x( S"), None);
    assert_eq!(parse_pid(b"12"), Some(12));
    for rejected in [b"".as_slice(), b"0", b"1a", b" 1", b"-1", b"99999999999"] {
        assert_eq!(parse_pid(rejected), None, "{rejected:?} is not a PID");
    }
}

#[test]
fn oversized_records_are_bounded_rather_than_read_without_limit() {
    let fixture = ProcFixture::new();
    fixture.identity("fixture-host", "boot-a", "pid:[4026531836]");
    // Exactly at the bound: read whole and parsed.
    let mut snug = stat_line(900, b"padded", 31);
    snug.pop();
    snug.extend(std::iter::repeat_n(
        b' ',
        MAX_FILE_BYTES as usize - snug.len() - 1,
    ));
    snug.push(b'\n');
    assert_eq!(snug.len() as u64, MAX_FILE_BYTES);
    fixture.raw(900, &snug);

    // Past the bound, with the creation token straddling it. Cutting at the
    // bound leaves a line that still parses — first `(`, last `)`, twenty fields
    // — whose twentieth field is `9070`, the digits of `907081358` that happened
    // to fit. That is a stable but false process identity, so the record must be
    // skipped with a reason instead.
    let mut straddling = b"901 (padded) S".to_vec();
    for filler in 1..=18 {
        straddling.extend_from_slice(format!(" {filler}").as_bytes());
    }
    straddling.extend(std::iter::repeat_n(
        b' ',
        MAX_FILE_BYTES as usize - straddling.len() - 4,
    ));
    straddling.extend_from_slice(b"907081358 4096 0 18446744073709551615\n");
    assert!(straddling.len() as u64 > MAX_FILE_BYTES);
    assert_eq!(
        parse_stat(901, &straddling[..MAX_FILE_BYTES as usize]).map(|stat| stat.start_ticks),
        Some(9070),
        "a silent cut would mint this wrong creation token"
    );
    fixture.raw(901, &straddling);

    let snapshot = fixture.source().snapshot();
    assert_eq!(snapshot.records.len(), 1);
    assert_eq!(snapshot.records[0].key.pid, Observed::Known(900));
    assert_eq!(
        snapshot.records[0].key.creation,
        CreationToken::LinuxBootTicks(31)
    );
    assert_eq!(snapshot.completeness.skipped(), 1);
    assert_eq!(
        snapshot
            .completeness
            .issues()
            .iter()
            .find(|issue| issue.scope == IssueScope::Process(901))
            .map(|issue| issue.reason),
        Some(MissingReason::Unavailable),
        "an oversized record is skipped with a reason, never silently shortened"
    );
}

/// The resident cell of every published row, in published order.
fn resident_cells(session: &srui_sessiond::Session) -> Vec<String> {
    session.with_store(|store| {
        store
            .get_model(srui_process_explorer::MODEL)
            .expect("the shell publishes one collection")
            .items
            .values()
            .map(|item| {
                let srui_sdk::Value::List(cells) = &item.value else {
                    panic!("expected table cells, got {:?}", item.value)
                };
                let srui_sdk::Value::String(text) = &cells[2] else {
                    panic!("a metric cell is published as text, got {:?}", cells[2])
                };
                text.clone()
            })
            .collect()
    })
}

/// The resident value of every record a scan published, in scan order.
fn resident_of(snapshot: &srui_process_explorer::source::ProcessSnapshot) -> Vec<Observed<u64>> {
    snapshot
        .records
        .iter()
        .map(|record| record.resident.clone())
        .collect()
}

/// PX-005: a resident page count becomes an exact byte count through the page
/// size the scanned mount itself reports, and reaches the row as IEC text.
#[test]
fn resident_memory_is_published_from_pages_and_the_mounts_own_page_size() {
    let fixture = ProcFixture::new();
    fixture
        .identity(
            "fixture-host",
            "11111111-2222-3333-4444-555555555555",
            "pid:[4026531836]",
        )
        .resident(1, b"systemd", 7, 1)
        .resident(2, b"kernel-thread", 9, 0)
        .resident(3, b"builder", 11, 1_500_000);
    let snapshot = fixture.source().snapshot();
    assert_eq!(snapshot.completeness, Completeness::Complete);
    assert_eq!(
        resident_of(&snapshot),
        vec![
            Observed::Known(FIXTURE_PAGE_SIZE),
            // A process with no resident pages is a known zero, not a gap.
            Observed::Known(0),
            Observed::Known(1_500_000 * FIXTURE_PAGE_SIZE),
        ]
    );

    // The published text names the multiple it is in, and the byte count behind
    // it is the exact product — 12,288,000,000 bytes is 11.4 GiB, truncated
    // rather than rounded up to 11.5.
    let session = srui_sessiond::Session::mint();
    srui_process_explorer::initialize_from_source(&session, &mut fixture.source()).unwrap();
    assert_eq!(
        resident_cells(&session),
        vec!["8.0 KiB".to_string(), "0 B".into(), "11.4 GiB".into()]
    );
    assert_eq!(1_500_000 * FIXTURE_PAGE_SIZE, 12_288_000_000);

    // The same page counts under a different page size are different byte
    // counts: the unit is the mount's, never this machine's.
    let narrower = ProcFixture::new();
    narrower
        .identity("fixture-host", "boot-a", "pid:[4026531836]")
        .page_size(4096)
        .resident(1, b"systemd", 7, 1)
        .resident(3, b"builder", 11, 1_500_000);
    assert_eq!(
        resident_of(&narrower.source().snapshot()),
        vec![Observed::Known(4096), Observed::Known(6_144_000_000)]
    );
    let session = srui_sessiond::Session::mint();
    srui_process_explorer::initialize_from_source(&session, &mut narrower.source()).unwrap();
    assert_eq!(
        resident_cells(&session),
        vec!["4.0 KiB".to_string(), "5.7 GiB".into()]
    );
}

/// A metric is not an identity: a record whose resident field cannot be read is
/// still published, with that one field reported unread.
#[test]
fn a_resident_field_a_scan_cannot_read_is_unavailable_rather_than_zero() {
    // A line that ends before field 24 exists at all.
    let mut truncated = b"12 (short) S".to_vec();
    for filler in 1..=18 {
        truncated.extend_from_slice(format!(" {filler}").as_bytes());
    }
    truncated.extend_from_slice(b" 55\n");

    let fixture = ProcFixture::new();
    fixture
        .identity("fixture-host", "boot-a", "pid:[4026531836]")
        // `rss` is a signed long in the kernel's own format string, so a
        // negative value is possible on the wire and is not a page count.
        .unreadable_resident(10, b"negative", 5, "-1")
        .unreadable_resident(11, b"words", 5, "many")
        .raw(12, &truncated);
    let snapshot = fixture.source().snapshot();
    assert_eq!(
        resident_of(&snapshot),
        vec![
            Observed::Missing(MissingReason::Unavailable),
            Observed::Missing(MissingReason::Unavailable),
            Observed::Missing(MissingReason::Unavailable),
        ]
    );
    // The records themselves are intact: names, creation tokens and a complete
    // scan. An unreadable metric is not an unreadable record.
    assert_eq!(snapshot.completeness, Completeness::Complete);
    assert_eq!(snapshot.records.len(), 3);
    assert_eq!(
        snapshot
            .records
            .iter()
            .map(|record| record.key.creation.clone())
            .collect::<Vec<_>>(),
        vec![
            CreationToken::LinuxBootTicks(5),
            CreationToken::LinuxBootTicks(5),
            CreationToken::LinuxBootTicks(55),
        ]
    );
    let session = srui_sessiond::Session::mint();
    srui_process_explorer::initialize_from_source(&session, &mut fixture.source()).unwrap();
    assert_eq!(resident_cells(&session), vec!["Unavailable"; 3]);
    // And the status still describes an authoritative, complete scan: one field
    // of one record is not a degraded record list.
    assert_eq!(
        published_status(fixture.source().status_text(), &snapshot),
        fixture.source().status_text()
    );
}

/// A mount that cannot state its page size cannot state a byte count either, and
/// says so in every row rather than publishing zeros or this machine's page size.
#[test]
fn a_mount_that_cannot_state_its_page_size_publishes_no_resident_value() {
    let cases: Vec<(&str, Vec<u8>)> = vec![
        // A vector that never mentions AT_PAGESZ.
        ("no page-size entry", auxv(&[(31, 4096), (AT_NULL, 0)])),
        // Terminated first: nothing after AT_NULL is defined, so a page size
        // written past the terminator is not an answer.
        (
            "after the terminator",
            auxv(&[(AT_NULL, 0), (AT_PAGESZ, 4096)]),
        ),
        (
            "not a power of two",
            auxv(&[(AT_PAGESZ, 5000), (AT_NULL, 0)]),
        ),
        ("zero", auxv(&[(AT_PAGESZ, 0), (AT_NULL, 0)])),
        ("too small", auxv(&[(AT_PAGESZ, 256), (AT_NULL, 0)])),
        (
            "implausibly large",
            auxv(&[(AT_PAGESZ, 1 << 31), (AT_NULL, 0)]),
        ),
        // Half a word is not a value.
        ("a partial word", vec![0xff, 0x01, 0x02]),
        ("empty", Vec::new()),
    ];
    for (label, bytes) in cases {
        let fixture = ProcFixture::new();
        fixture
            .identity("fixture-host", "boot-a", "pid:[4026531836]")
            .auxv(&bytes)
            .resident(1, b"systemd", 7, 10);
        let snapshot = fixture.source().snapshot();
        assert_eq!(
            resident_of(&snapshot),
            vec![Observed::Missing(MissingReason::Unavailable)],
            "{label}"
        );
        // The record list is still authoritative: the page size says nothing
        // about which processes exist.
        assert_eq!(snapshot.completeness, Completeness::Complete, "{label}");
        let session = srui_sessiond::Session::mint();
        srui_process_explorer::initialize_from_source(&session, &mut fixture.source()).unwrap();
        assert_eq!(resident_cells(&session), vec!["Unavailable"], "{label}");
    }

    // No auxiliary vector at all: the file is simply absent.
    let absent = ProcFixture::new();
    absent
        .identity("fixture-host", "boot-a", "pid:[4026531836]")
        .resident(1, b"systemd", 7, 10);
    fs::remove_file(absent.0.join("self/auxv")).unwrap();
    assert_eq!(
        resident_of(&absent.source().snapshot()),
        vec![Observed::Missing(MissingReason::Unavailable)]
    );

    // Present but unreadable: the reason a value is missing is reported, not
    // flattened into "unavailable".
    let denied = ProcFixture::new();
    denied
        .identity("fixture-host", "boot-a", "pid:[4026531836]")
        .resident(1, b"systemd", 7, 10);
    let path = denied.0.join("self/auxv");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
    let snapshot = denied.source().snapshot();
    assert_eq!(
        resident_of(&snapshot),
        vec![Observed::Missing(MissingReason::Denied)]
    );
    assert_eq!(snapshot.completeness, Completeness::Complete);
    let session = srui_sessiond::Session::mint();
    srui_process_explorer::initialize_from_source(&session, &mut denied.source()).unwrap();
    assert_eq!(resident_cells(&session), vec!["Denied"]);
}

/// A page count that cannot be converted is reported unread, never wrapped into
/// a small confident number.
#[test]
fn a_page_count_too_large_to_convert_is_unavailable_rather_than_wrapped() {
    assert!(u64::MAX.checked_mul(FIXTURE_PAGE_SIZE).is_none());
    let fixture = ProcFixture::new();
    fixture
        .identity("fixture-host", "boot-a", "pid:[4026531836]")
        .resident(1, b"absurd", 7, u64::MAX)
        // One page past what the conversion can represent.
        .resident(2, b"absurd", 8, u64::MAX / FIXTURE_PAGE_SIZE + 1)
        // The largest count that still converts.
        .resident(3, b"largest", 9, u64::MAX / FIXTURE_PAGE_SIZE);
    let snapshot = fixture.source().snapshot();
    assert_eq!(
        resident_of(&snapshot),
        vec![
            Observed::Missing(MissingReason::Unavailable),
            Observed::Missing(MissingReason::Unavailable),
            Observed::Known((u64::MAX / FIXTURE_PAGE_SIZE) * FIXTURE_PAGE_SIZE),
        ]
    );
    assert_eq!(snapshot.completeness, Completeness::Complete);
}

/// A monotonic clock this suite sets by hand (PX-006).
///
/// The collector measures every CPU interval on its injected clock and nothing
/// else, so each interval below is exactly the one a test states — including
/// the zero and backwards intervals a real monotonic clock never produces —
/// however long the scan really took on the wall clock.
#[derive(Debug, Default)]
struct ManualClock(Mutex<Duration>);

impl ManualClock {
    fn set(&self, seconds: u64) {
        *self.0.lock().unwrap() = Duration::from_secs(seconds);
    }
}

impl MonotonicClock for ManualClock {
    fn now(&self) -> Duration {
        *self.0.lock().unwrap()
    }
}

/// A persistent collector over `fixture`, measuring on a clock the test owns.
fn clocked(fixture: &ProcFixture) -> (ProcFsSource, Arc<ManualClock>) {
    let clock = Arc::new(ManualClock::default());
    let source = fixture.source().with_clock(clock.clone());
    (source, clock)
}

/// The CPU usage of every record a scan published, in scan order.
fn cpu_of(snapshot: &srui_process_explorer::source::ProcessSnapshot) -> Vec<CpuUsage> {
    snapshot.records.iter().map(|record| record.cpu).collect()
}

/// The CPU cell of every published row, in model order.
fn cpu_cells(session: &srui_sessiond::Session) -> Vec<String> {
    session.with_store(|store| {
        store
            .get_model(srui_process_explorer::MODEL)
            .expect("the shell publishes one collection")
            .items
            .values()
            .map(|item| {
                let srui_sdk::Value::List(cells) = &item.value else {
                    panic!("expected table cells, got {:?}", item.value)
                };
                let srui_sdk::Value::String(text) = &cells[3] else {
                    panic!("a metric cell is published as text, got {:?}", cells[3])
                };
                text.clone()
            })
            .collect()
    })
}

fn measured(ticks: u64, seconds: u64) -> CpuUsage {
    CpuUsage::Measured(CpuInterval {
        ticks,
        ticks_per_second: 100,
        elapsed: Duration::from_secs(seconds),
    })
}

/// PX-006: deterministic counters give the expected single-CPU and multicore
/// percentages, over the injected monotonic interval and never the wall clock.
#[test]
fn cpu_usage_is_counter_ticks_over_the_injected_monotonic_interval() {
    let fixture = ProcFixture::new();
    fixture
        .identity("fixture-host", "boot-a", "pid:[4026531836]")
        .clock_ticks(100)
        .cpu(10, b"single", 500, "100", "50")
        .cpu(20, b"threads", 600, "1000", "0")
        .cpu(30, b"idle", 700, "5", "5");
    let (mut source, clock) = clocked(&fixture);
    clock.set(1_000);
    let first = source.snapshot();
    // Nothing to subtract yet: every first sample is visibly warming up, and
    // none of them is a zero.
    assert_eq!(cpu_of(&first), vec![CpuUsage::WarmingUp; 3]);
    assert_eq!(first.completeness, Completeness::Complete);

    // One CPU half used (user and system time both count), four and a half
    // CPUs fully used by one multithreaded process, and a process that ran not
    // at all — across two seconds of the injected monotonic clock.
    fixture
        .cpu(10, b"single", 500, "170", "80")
        .cpu(20, b"threads", 600, "1900", "0");
    clock.set(1_002);
    let second = source.snapshot();
    assert_eq!(
        cpu_of(&second),
        vec![measured(100, 2), measured(900, 2), measured(0, 2)]
    );
    let cells: Vec<String> = second
        .records
        .iter()
        .map(|record| srui_process_explorer::metric::cpu_cell(&record.cpu))
        .collect();
    assert_eq!(cells, vec!["50.0%", "450.0%", "0.0%"]);
    // The wall clock moved by far less than the two seconds divided by: the
    // sample time is a wall-clock stamp and never reaches the divisor, so a
    // wall-clock jump of any size cannot stretch or shrink an interval.
    let wall = second
        .sampled_at
        .0
        .duration_since(first.sampled_at.0)
        .unwrap_or_default();
    assert!(wall < Duration::from_secs(1), "{wall:?}");
}

/// A monotonic clock that advances one second on every reading: a stand-in for
/// a scan whose records are read one after another, each read taking time.
#[derive(Debug, Default)]
struct SteppingClock(Mutex<u64>);

impl MonotonicClock for SteppingClock {
    fn now(&self) -> Duration {
        let mut seconds = self.0.lock().unwrap();
        *seconds += 1;
        Duration::from_secs(*seconds)
    }
}

/// PX-006 review round 1 (B1): each record's interval is its *own* read-to-read
/// interval, taken when its `stat` read returns — never one scan-start instant
/// stamped on every record. With one second passing per read, two records read
/// in each of two scans are each measured across the reads that separate their
/// own samples. Directory order is not fixed, so the assertion is on the sum:
/// read-to-read intervals total four seconds whatever the order (2 + 2, or 3 +
/// 1), while a per-scan instant would divide both by one second and publish
/// 200% for each single-threaded process.
#[test]
fn each_record_is_measured_over_its_own_read_to_read_interval() {
    let fixture = ProcFixture::new();
    fixture
        .identity("fixture-host", "boot-a", "pid:[4026531836]")
        .clock_ticks(100)
        .cpu(10, b"a", 500, "0", "0")
        .cpu(20, b"b", 600, "0", "0");
    let clock = Arc::new(SteppingClock::default());
    let mut source = fixture.source().with_clock(clock.clone());
    assert_eq!(cpu_of(&source.snapshot()), vec![CpuUsage::WarmingUp; 2]);
    fixture
        .cpu(10, b"a", 500, "200", "0")
        .cpu(20, b"b", 600, "200", "0");
    let second = source.snapshot();
    let elapsed: Vec<Duration> = cpu_of(&second)
        .into_iter()
        .map(|usage| match usage {
            CpuUsage::Measured(interval) => {
                assert_eq!(interval.ticks, 200);
                interval.elapsed
            }
            other => panic!("a second sample is measured: {other:?}"),
        })
        .collect();
    assert_eq!(
        elapsed.iter().sum::<Duration>(),
        Duration::from_secs(4),
        "{elapsed:?}: each record must be measured read-to-read"
    );
    assert!(elapsed
        .iter()
        .all(|interval| *interval >= Duration::from_secs(1)));
    // The clock is read once per record read, and never for the scan as a whole.
    assert_eq!(*clock.0.lock().unwrap(), 4);
}

/// PX-006 review round 1 (W2): start ticks are numbered per PID namespace as
/// well as per boot. A namespace that comes back different discards every
/// baseline; one the scan merely could not read discards nothing.
#[test]
fn a_different_pid_namespace_discards_every_baseline_and_an_unreadable_one_does_not() {
    let fixture = ProcFixture::new();
    fixture
        .identity("fixture-host", "boot-a", "pid:[4026531836]")
        .clock_ticks(100)
        .cpu(10, b"worker", 500, "100", "0");
    let link = fixture.0.join("self/ns/pid");
    let (mut source, clock) = clocked(&fixture);
    clock.set(1);
    source.snapshot();
    fs::remove_file(&link).unwrap();
    fixture.cpu(10, b"worker", 500, "150", "0");
    clock.set(2);
    assert_eq!(cpu_of(&source.snapshot()), vec![measured(50, 1)]);
    std::os::unix::fs::symlink("pid:[4026531999]", &link).unwrap();
    fixture.cpu(10, b"worker", 500, "200", "0");
    clock.set(3);
    assert_eq!(cpu_of(&source.snapshot()), vec![CpuUsage::WarmingUp]);
}

/// PX-006: a reused PID with a new creation token is another process instance.
/// It warms up rather than inheriting — and subtracting — the counters of the
/// process it replaced, which would be a reset or a spike.
#[test]
fn a_replacement_under_a_reused_pid_warms_up_instead_of_inheriting_counters() {
    let fixture = ProcFixture::new();
    fixture
        .identity("fixture-host", "boot-a", "pid:[4026531836]")
        .clock_ticks(100)
        .cpu(10, b"worker", 500, "5000", "0");
    let (mut source, clock) = clocked(&fixture);
    clock.set(10);
    source.snapshot();
    // Same PID, same name, a later start time, and far fewer ticks.
    fixture.cpu(10, b"worker", 900, "7", "0");
    clock.set(11);
    let replaced = source.snapshot();
    assert_eq!(cpu_of(&replaced), vec![CpuUsage::WarmingUp]);
    assert_eq!(
        replaced.records[0].key.creation,
        CreationToken::LinuxBootTicks(900)
    );
    // And the new instance is then measured against its own first sample.
    fixture.cpu(10, b"worker", 900, "57", "0");
    clock.set(13);
    assert_eq!(cpu_of(&source.snapshot()), vec![measured(50, 2)]);
}

/// PX-006: a counter that went backwards for the same instance is a reset, not
/// a measurement — never a wrapped unsigned delta — and the next interval is
/// measured from the reset value.
#[test]
fn a_counter_that_goes_backwards_is_unavailable_and_then_measured_again() {
    let fixture = ProcFixture::new();
    fixture
        .identity("fixture-host", "boot-a", "pid:[4026531836]")
        .clock_ticks(100)
        .cpu(10, b"worker", 500, "300", "0");
    let (mut source, clock) = clocked(&fixture);
    clock.set(10);
    source.snapshot();
    fixture.cpu(10, b"worker", 500, "20", "0");
    clock.set(11);
    assert_eq!(
        cpu_of(&source.snapshot()),
        vec![CpuUsage::Missing(MissingReason::Unavailable)]
    );
    fixture.cpu(10, b"worker", 500, "120", "0");
    clock.set(12);
    assert_eq!(cpu_of(&source.snapshot()), vec![measured(100, 1)]);
}

/// PX-006: an interval that did not advance has no divisor, and one that ran
/// backwards has no meaning. Neither is published as a value or a spike.
#[test]
fn an_interval_that_does_not_advance_is_unavailable_and_never_a_spike() {
    let fixture = ProcFixture::new();
    fixture
        .identity("fixture-host", "boot-a", "pid:[4026531836]")
        .clock_ticks(100)
        .cpu(10, b"worker", 500, "0", "0");
    let (mut source, clock) = clocked(&fixture);
    clock.set(5);
    source.snapshot();
    // A hundred ticks in zero time would be an infinite percentage.
    fixture.cpu(10, b"worker", 500, "100", "0");
    let unavailable = vec![CpuUsage::Missing(MissingReason::Unavailable)];
    assert_eq!(cpu_of(&source.snapshot()), unavailable);
    // The older baseline was kept, so the next interval spans both scans.
    fixture.cpu(10, b"worker", 500, "200", "0");
    clock.set(6);
    assert_eq!(cpu_of(&source.snapshot()), vec![measured(200, 1)]);
    // A clock reading earlier than the baseline: unavailable, then rebased.
    fixture.cpu(10, b"worker", 500, "300", "0");
    clock.set(4);
    assert_eq!(cpu_of(&source.snapshot()), unavailable);
    fixture.cpu(10, b"worker", 500, "350", "0");
    clock.set(5);
    assert_eq!(cpu_of(&source.snapshot()), vec![measured(50, 1)]);
}

/// PX-006: a mount that cannot state its tick rate publishes no CPU value on any
/// scan — not a warm-up that never ends, and never a zero — and the reason the
/// rate is missing is the one published.
#[test]
fn a_mount_that_cannot_state_its_tick_rate_publishes_no_cpu_value() {
    let fixture = ProcFixture::new();
    // `identity` states a page size and no tick rate.
    fixture
        .identity("fixture-host", "boot-a", "pid:[4026531836]")
        .cpu(10, b"worker", 500, "0", "0");
    let (mut source, clock) = clocked(&fixture);
    for (second, ticks) in [(1, "0"), (2, "100")] {
        fixture.cpu(10, b"worker", 500, ticks, "0");
        clock.set(second);
        let snapshot = source.snapshot();
        assert_eq!(
            cpu_of(&snapshot),
            vec![CpuUsage::Missing(MissingReason::Unavailable)]
        );
        assert_eq!(snapshot.completeness, Completeness::Complete);
    }
    for (second, implausible) in [(3, 0), (4, 1_000_001)] {
        fixture.clock_ticks(implausible);
        clock.set(second);
        assert_eq!(
            cpu_of(&source.snapshot()),
            vec![CpuUsage::Missing(MissingReason::Unavailable)],
            "{implausible} ticks per second"
        );
    }
    // Present but unreadable: the reason is reported, not flattened.
    fixture.clock_ticks(100);
    fs::set_permissions(
        fixture.0.join("self/auxv"),
        fs::Permissions::from_mode(0o000),
    )
    .unwrap();
    clock.set(5);
    assert_eq!(
        cpu_of(&source.snapshot()),
        vec![CpuUsage::Missing(MissingReason::Denied)]
    );
    let session = srui_sessiond::Session::mint();
    srui_process_explorer::initialize_from_source(&session, &mut source).unwrap();
    assert_eq!(cpu_cells(&session), vec!["Denied"]);
}

/// PX-006: an unparsable counter is an unread metric of a published record; the
/// record list stays authoritative and the next readable sample warms up.
#[test]
fn an_unreadable_cpu_counter_is_unavailable_and_the_record_is_still_published() {
    let fixture = ProcFixture::new();
    fixture
        .identity("fixture-host", "boot-a", "pid:[4026531836]")
        .clock_ticks(100)
        .cpu(10, b"worker", 500, "100", "0");
    let (mut source, clock) = clocked(&fixture);
    clock.set(1);
    source.snapshot();
    for (second, utime, stime) in [(2, "-4", "0"), (3, "100", "x")] {
        fixture.cpu(10, b"worker", 500, utime, stime);
        clock.set(second);
        let snapshot = source.snapshot();
        assert_eq!(
            cpu_of(&snapshot),
            vec![CpuUsage::Missing(MissingReason::Unavailable)]
        );
        assert_eq!(snapshot.completeness, Completeness::Complete);
        assert_eq!(snapshot.records[0].display_name.as_str(), "worker");
    }
    fixture.cpu(10, b"worker", 500, "150", "0");
    clock.set(4);
    assert_eq!(cpu_of(&source.snapshot()), vec![CpuUsage::WarmingUp]);
}

/// PX-006: start ticks are only comparable within one boot. A boot ID that
/// comes back different discards every baseline; one the scan merely could not
/// read is not a reboot and discards nothing.
#[test]
fn a_different_boot_discards_every_baseline_and_an_unreadable_one_does_not() {
    let fixture = ProcFixture::new();
    fixture
        .identity("fixture-host", "boot-a", "pid:[4026531836]")
        .clock_ticks(100)
        .cpu(10, b"worker", 500, "100", "0");
    let boot = fixture.0.join("sys/kernel/random/boot_id");
    let (mut source, clock) = clocked(&fixture);
    clock.set(1);
    source.snapshot();
    fs::remove_file(&boot).unwrap();
    fixture.cpu(10, b"worker", 500, "150", "0");
    clock.set(2);
    assert_eq!(cpu_of(&source.snapshot()), vec![measured(50, 1)]);
    fs::write(&boot, "boot-b\n").unwrap();
    fixture.cpu(10, b"worker", 500, "200", "0");
    clock.set(3);
    assert_eq!(cpu_of(&source.snapshot()), vec![CpuUsage::WarmingUp]);
}

/// PX-006: the collector keeps one baseline per record it last published, so a
/// process that ended is forgotten; a scan that could not list the root read no
/// counter and keeps every baseline for the next good scan.
#[test]
fn cpu_baselines_are_bounded_by_the_last_published_records() {
    let fixture = ProcFixture::new();
    fixture
        .identity("fixture-host", "boot-a", "pid:[4026531836]")
        .clock_ticks(100)
        .cpu(10, b"a", 500, "0", "0")
        .cpu(20, b"b", 600, "0", "0")
        .cpu(30, b"c", 700, "0", "0");
    let (mut source, clock) = clocked(&fixture);
    clock.set(1);
    source.snapshot();
    assert_eq!(source.cpu_baselines(), 3);
    fs::remove_dir_all(fixture.0.join("20")).unwrap();
    fs::remove_dir_all(fixture.0.join("30")).unwrap();
    clock.set(2);
    source.snapshot();
    assert_eq!(source.cpu_baselines(), 1);

    // Searchable but not listable: identity files still open by path, the
    // listing itself fails.
    fs::set_permissions(&fixture.0, fs::Permissions::from_mode(0o300)).unwrap();
    clock.set(3);
    let failed = source.snapshot();
    fs::set_permissions(&fixture.0, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(failed.records.is_empty());
    assert!(!failed.completeness.is_complete());
    assert_eq!(
        source.cpu_baselines(),
        1,
        "a failed listing read no counter"
    );
    fixture.cpu(10, b"a", 500, "300", "0");
    clock.set(5);
    // Measured from the last scan that read the counter, across the failed one.
    assert_eq!(cpu_of(&source.snapshot()), vec![measured(300, 3)]);
}

/// PX-006 through the publication path: the first publication is visibly
/// warming up, the next refresh publishes the measured value in place, and an
/// idle process that stays idle publishes nothing further.
#[test]
fn the_first_publication_is_warming_up_and_a_refresh_publishes_the_measured_value() {
    let fixture = ProcFixture::new();
    fixture
        .identity("fixture-host", "boot-a", "pid:[4026531836]")
        .clock_ticks(100)
        .cpu(10, b"worker", 500, "100", "0");
    let (mut source, clock) = clocked(&fixture);
    clock.set(100);
    let session = srui_sessiond::Session::mint();
    let (mut view, _) = srui_process_explorer::start_from_source(&session, &mut source).unwrap();
    assert_eq!(cpu_cells(&session), vec!["Warming up"]);

    fixture.cpu(10, b"worker", 500, "125", "25");
    clock.set(101);
    let outcome = view.refresh(&session, &mut source).unwrap();
    assert_eq!((outcome.inserted, outcome.deleted), (0, 0));
    assert_eq!(outcome.updated, 1, "{outcome:?}");
    assert_eq!(cpu_cells(&session), vec!["50.0%"]);

    clock.set(102);
    let outcome = view.refresh(&session, &mut source).unwrap();
    assert_eq!(outcome.updated, 1, "{outcome:?}");
    assert_eq!(cpu_cells(&session), vec!["0.0%"]);
    let revision = session.current_revision();
    clock.set(103);
    let outcome = view.refresh(&session, &mut source).unwrap();
    assert_eq!(outcome.updated, 0, "{outcome:?}");
    assert_eq!(session.current_revision(), revision, "nothing changed");
}

#[cfg(target_os = "linux")]
mod live {
    use super::*;
    use srui_process_explorer::refresh::DEFAULT_REFRESH_INTERVAL;
    use srui_process_explorer::{
        initialize_from_source, published_status, start_from_source, MODEL,
    };
    use srui_sdk::{ItemId, Value};
    use srui_sessiond::Session;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::process::{Child, Command, Stdio};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    /// Set in a worker's environment to `<pid>:<activity>`: the PID of the test
    /// process that spawned it, and what it does once ready ([`WORKER_SLEEPS`] or
    /// [`WORKER_BURNS`]). [`test_owned_worker`] is a no-op unless that process is
    /// its parent, so an exported value cannot turn the harness into a worker.
    const WORKER_ENV: &str = "SRTOP_LIVE_WORKER";
    /// A worker that sleeps once it is ready.
    const WORKER_SLEEPS: &str = "sleep";
    /// A worker that keeps one CPU busy once it is ready.
    const WORKER_BURNS: &str = "burn";
    /// The libtest name of [`test_owned_worker`], the one test a worker runs.
    const WORKER_TEST: &str = "live::test_owned_worker";
    /// The name a worker gives itself, which a scan reads back from
    /// `/proc/<pid>/stat`; the kernel keeps at most 15 bytes.
    const WORKER_NAME: &str = "srtop-worker";
    /// What a worker prints, followed by its PID, once it is ready to be scanned.
    const WORKER_READY: &str = "srtop live worker ready: pid=";
    /// The least memory a worker writes and holds; see [`worker_resident_bytes`].
    const WORKER_RESIDENT_MIN: usize = 8 * 1024 * 1024;
    /// Page size assumed when this process cannot read its own auxiliary
    /// vector: the smallest Linux uses. Guessing too small shows up as the zero
    /// resident size the live test asserts against; guessing 64 KiB on a 4 KiB
    /// host writes sixteen times what the counter needs, which a memory-limited
    /// host answers by killing the worker before it reports ready.
    const FALLBACK_PAGE_SIZE: usize = 4096;
    /// CPUs assumed online when the kernel's list cannot be read: many, so a
    /// worker errs toward writing more pages rather than fewer.
    const FALLBACK_ONLINE_CPUS: usize = 256;
    /// How long a worker lives if nothing ends it sooner.
    const WORKER_LIFETIME: Duration = Duration::from_secs(60);
    /// How long a test waits for its worker to report ready.
    const WORKER_READY_TIMEOUT: Duration = Duration::from_secs(30);

    /// A bounded, disposable worker process owned by this test: this test
    /// binary, re-executed to run [`test_owned_worker`] alone.
    ///
    /// [`Worker::spawn`] returns once the worker says it is ready, because a
    /// `/proc/<pid>` entry proves nothing: it exists as soon as the child does,
    /// and `spawn` can return while the child is still inside `execve` with the
    /// spawning thread's name, or has just begun running with too few pages for
    /// the kernel's approximate count to show any.
    struct Worker(Child);

    impl Worker {
        /// A worker that sleeps once it is ready.
        fn start() -> Self {
            Self::spawn(WORKER_SLEEPS)
        }

        /// A worker that, once ready, keeps one CPU busy until it ends: one
        /// thread spins while libtest's main thread only waits for it, so every
        /// tick it is scheduled for is charged to this one process (PX-006). It
        /// is bounded like any worker, and its setup is done before it reports
        /// ready, so none of it falls inside a test's sampling interval.
        fn burn() -> Self {
            Self::spawn(WORKER_BURNS)
        }

        fn spawn(activity: &str) -> Self {
            let program = std::env::current_exe().expect("a test binary knows its own path");
            let mut child = Command::new(program)
                .args([
                    "--exact",
                    WORKER_TEST,
                    "--ignored",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env(WORKER_ENV, format!("{}:{activity}", std::process::id()))
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .expect("the test owns this worker");
            // Both pipes are this test's own, read on other threads so the wait
            // below has a deadline. stderr carries libtest's own errors and the
            // worker's panics.
            let (sender, lines) = mpsc::channel();
            forward_lines(
                child.stdout.take().expect("stdout is piped"),
                "",
                sender.clone(),
            );
            forward_lines(
                child.stderr.take().expect("stderr is piped"),
                "stderr: ",
                sender,
            );
            let mut worker = Self(child);
            // libtest may already have begun the line with the test's name.
            let ready = format!("{WORKER_READY}{}", worker.pid());
            let deadline = Instant::now() + WORKER_READY_TIMEOUT;
            let mut output = Vec::new();
            let cause = loop {
                match lines.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                    Ok(line) if line.ends_with(&ready) => return worker,
                    Ok(line) => output.push(line),
                    Err(cause) => break cause,
                }
            };
            let _ = worker.0.kill();
            let status = worker.0.wait();
            // Both pipes end once the worker is gone: keep what it printed last.
            let drained = Instant::now() + Duration::from_secs(5);
            while let Ok(line) =
                lines.recv_timeout(drained.saturating_duration_since(Instant::now()))
            {
                output.push(line);
            }
            panic!(
                "worker {} never reported ready ({cause:?}, waited up to \
                 {WORKER_READY_TIMEOUT:?}); status {status:?}; output {output:?}",
                worker.pid()
            );
        }

        fn pid(&self) -> u32 {
            self.0.id()
        }
    }

    impl Drop for Worker {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    /// Sends each line `pipe` yields, after `prefix`, until the pipe ends. It
    /// keeps draining after nobody listens, so a worker never blocks on output.
    fn forward_lines(
        pipe: impl Read + Send + 'static,
        prefix: &'static str,
        sender: mpsc::Sender<String>,
    ) {
        std::thread::spawn(move || {
            for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                let _ = sender.send(format!("{prefix}{line}"));
            }
        });
    }

    /// The body of a [`Worker`]. Ignored, and a no-op unless [`WORKER_ENV`]
    /// names this process's parent, so `--ignored` and `--include-ignored` runs
    /// stay harmless even with the variable exported.
    ///
    /// A worker names itself and writes the memory it holds, and only then
    /// reports ready. It sleeps, or spins, until it is killed, and ends by
    /// itself after [`WORKER_LIFETIME`] or once that parent is gone, so an
    /// orphan does not outlive a crashed test for long.
    #[test]
    #[ignore = "the body of a test-owned worker process, which Worker::spawn runs"]
    fn test_owned_worker() {
        let request = std::env::var(WORKER_ENV).unwrap_or_default();
        let Some((parent, activity)) = request.split_once(':') else {
            return;
        };
        let parent = parent.parse::<u32>().ok();
        // Checked against the parent as it is now, so a worker whose test died
        // before this point ends at once instead of adopting its new parent.
        let spawned_by_test = || parent == Some(std::os::unix::process::parent_id());
        if !spawned_by_test() {
            return;
        }
        // Settled before the setup, so an unknown request fails before ready.
        let burns = match activity {
            WORKER_SLEEPS => false,
            WORKER_BURNS => true,
            other => panic!("unknown worker activity {other:?}"),
        };
        // Renames the thread-group leader, whichever thread writes it: the name
        // `/proc/<pid>/stat` reports for the process.
        std::fs::write("/proc/self/comm", WORKER_NAME).expect("a process may rename itself");
        // Written, not merely allocated, so every page is resident.
        let resident = std::hint::black_box(vec![1u8; worker_resident_bytes()]);
        let mut stdout = std::io::stdout().lock();
        writeln!(stdout, "{WORKER_READY}{}", std::process::id()).expect("the test reads this");
        stdout.flush().expect("the test reads this");
        drop(stdout);
        let started = Instant::now();
        while started.elapsed() < WORKER_LIFETIME && spawned_by_test() {
            if burns {
                // Only this thread runs: libtest's main thread is blocked waiting
                // for it, so the process uses one CPU and no more.
                let slice = Instant::now();
                let mut spins = 0_u64;
                while slice.elapsed() < Duration::from_millis(100) {
                    spins = std::hint::black_box(spins.wrapping_add(1));
                }
            } else {
                std::thread::sleep(Duration::from_millis(100));
            }
        }
        drop(resident);
    }

    /// How much memory a worker writes and holds, so that the `rss` in
    /// `/proc/<pid>/stat` cannot read as zero. That field leaves out per-CPU
    /// counter deltas smaller than the kernel's batch, `max(32, 2 * online
    /// CPUs)` pages, so one thread writing four batches of its own pages has at
    /// least three counted. Never less than [`WORKER_RESIDENT_MIN`], and not
    /// capped: fewer than four batches brings back the zero this guards against.
    fn worker_resident_bytes() -> usize {
        let pages = 4 * (2 * online_cpus().unwrap_or(FALLBACK_ONLINE_CPUS)).max(32);
        (pages * own_auxv(AT_PAGESZ).unwrap_or(FALLBACK_PAGE_SIZE)).max(WORKER_RESIDENT_MIN)
    }

    /// The value of `wanted` in this process's own auxiliary vector: pairs of
    /// native-endian words ending at `AT_NULL`, the layout `procfs` reads from a
    /// mount's `self/auxv`.
    fn own_auxv(wanted: u64) -> Option<usize> {
        let bytes = std::fs::read("/proc/self/auxv").ok()?;
        let word = std::mem::size_of::<usize>();
        let read = |slice: &[u8]| usize::from_ne_bytes(slice.try_into().expect("one word"));
        bytes
            .chunks_exact(2 * word)
            .map(|pair| (read(&pair[..word]) as u64, read(&pair[word..])))
            .take_while(|&(key, _)| key != AT_NULL)
            .find(|&(key, _)| key == wanted)
            .map(|(_, value)| value)
    }

    /// How many CPUs the kernel has online, the count it sizes that batch by,
    /// from the ranges in `/sys/devices/system/cpu/online` (`0-3,8-11`). Not
    /// `available_parallelism`, which a cgroup can hold below that count.
    fn online_cpus() -> Option<usize> {
        let list = std::fs::read_to_string("/sys/devices/system/cpu/online").ok()?;
        list.trim()
            .split(',')
            .map(|range| {
                let (first, last) = range.split_once('-').unwrap_or((range, range));
                let (first, last) = (first.parse::<usize>().ok()?, last.parse::<usize>().ok()?);
                last.checked_sub(first).map(|span| span + 1)
            })
            .sum()
    }

    #[test]
    fn live_snapshot_locates_the_test_owned_sleeping_worker() {
        let mut worker = Worker::start();
        let pid = worker.pid();
        let snapshot = ProcFsSource::live().snapshot();
        assert_eq!(snapshot.source, SourceId("procfs:/proc".into()));
        assert!(
            snapshot.records.len() > 1,
            "a live host runs more than one process"
        );
        let record = snapshot
            .records
            .iter()
            .find(|record| record.key.pid == Observed::Known(pid))
            .expect("the owned worker must appear in a live snapshot");
        assert_eq!(record.display_name.as_str(), WORKER_NAME);
        // A sleeping worker holds a real, readable amount of memory: a live
        // record's metric is a value, not an unread field (PX-005).
        let Observed::Known(resident) = record.resident else {
            panic!(
                "a live record's resident memory must be readable: {:?}",
                record.resident
            )
        };
        assert!(resident > 0, "a running process holds resident pages");
        let CreationToken::LinuxBootTicks(ticks) = record.key.creation else {
            panic!("a live Linux record must carry a boot-ticks creation token")
        };
        assert!(ticks > 0);
        let raw = std::fs::read(format!("/proc/{pid}/stat")).unwrap();
        assert_eq!(
            parse_stat(pid, &raw).map(|stat| stat.start_ticks),
            Some(ticks)
        );
        assert!(matches!(record.key.boot, Observed::Known(_)));
        assert!(matches!(record.key.host, Observed::Known(_)));
        assert!(matches!(record.key.pid_namespace, Observed::Known(_)));
        // Any other visible record proves the same thing as PID 1 here, and PID
        // 1 is legitimately invisible to an unprivileged scan under `hidepid=2`
        // or in a restricted container — where the collector is working exactly
        // as intended.
        let other = snapshot
            .records
            .iter()
            .find(|candidate| candidate.key.pid != Observed::Known(pid))
            .expect("a live host shows more than this test's own worker");
        assert_ne!(record.key, other.key);
        assert_eq!(record.key.boot, other.key.boot);
        println!(
            "PX-003 live evidence: source={:?} host={:?} boot={:?} ns={:?} worker_pid={pid} \
             creation_ticks={ticks} records={} completeness_skipped={} status={:?}",
            snapshot.source,
            record.key.host,
            record.key.boot,
            record.key.pid_namespace,
            snapshot.records.len(),
            snapshot.completeness.skipped(),
            published_status(LIVE_STATUS_TEXT, &snapshot),
        );

        // The worker is bounded and disposable: once this test reaps it, the
        // next live snapshot no longer lists that process instance. The absent
        // thing is the full identity, not the PID: the kernel may hand the same
        // number to a new process before the second scan, and that record is a
        // different instance the collector is right to list.
        let reaped = record.key.clone();
        worker.0.kill().unwrap();
        worker.0.wait().unwrap();
        let after = ProcFsSource::live().snapshot();
        assert!(
            after.records.iter().all(|listed| listed.key != reaped),
            "the reaped worker's process identity must be gone from a later snapshot"
        );
    }

    #[test]
    fn a_degraded_live_scan_explains_itself_and_still_publishes_rows() {
        let worker = Worker::start();
        let session = Session::mint();
        let snapshot = initialize_from_source(&session, &mut ProcFsSource::live()).unwrap();
        let published = published_status(LIVE_STATUS_TEXT, &snapshot);
        // Unreadable per-process records are normal on a shared host; they are
        // always explained and never silently reduce the scan to "empty".
        if snapshot.completeness.is_complete() {
            assert_eq!(published, LIVE_STATUS_TEXT);
        } else {
            assert!(!snapshot.completeness.issues().is_empty());
            assert!(!snapshot.records.is_empty());
            assert!(published.contains("incomplete scan"), "{published}");
        }
        assert_ne!(published, FAKE_STATUS_TEXT);
        session.with_store(|store| {
            assert_eq!(
                store
                    .get_node(srui_process_explorer::STATUS)
                    .unwrap()
                    .get_property(srui_sdk::TEXT),
                Some(&Value::String(published.clone())),
                "live data must be labeled by the live source"
            );
        });
        assert_eq!(session.current_revision(), 1);
        session.with_store(|store| {
            assert_eq!(store.node_count(), 5, "live rows create no view nodes");
            let model = store.get_model(MODEL).unwrap();
            assert_eq!(
                model.item_count,
                u64::try_from(snapshot.records.len()).unwrap()
            );
            let worker_row = model
                .items
                .values()
                .find(|item| {
                    matches!(&item.value, Value::List(cells)
                        if cells[0] == Value::UnsignedInt(u64::from(worker.pid())))
                })
                .expect("the owned worker must reach the semantic model");
            let Value::List(cells) = &worker_row.value else {
                panic!("expected table cells")
            };
            assert_eq!(cells[1], Value::String(WORKER_NAME.into()));
            // Item IDs are session-allocated and opaque: the allocator hands out
            // 1..=count in projection order, whatever the PID values are.
            // Asserting that set keeps this meaningful in a PID namespace with
            // contiguous low PIDs, where `item_id != pid` would hold only by
            // coincidence.
            let mut ids: Vec<u64> = model
                .items
                .values()
                .map(|item| item.item_id.get())
                .collect();
            ids.sort_unstable();
            assert_eq!(ids, (1..=ids.len() as u64).collect::<Vec<u64>>());
            assert!(ids.contains(&worker_row.item_id.get()));
            for item in model.items.values() {
                let Value::List(cells) = &item.value else {
                    panic!("expected table cells")
                };
                let Value::String(name) = &cells[1] else {
                    panic!("a live process name must be a plain string")
                };
                assert!(
                    !name.chars().any(DisplayName::is_unsafe),
                    "live name {name:?} reached the UI with invisible characters"
                );
            }
        });
    }

    /// PX-004 acceptance on a real host: a process this test creates, and then
    /// ends, reaches the published collection within two sampling intervals —
    /// through the refresh path, not a fresh publication.
    #[test]
    fn a_test_owned_worker_appears_and_exits_within_two_sampling_intervals() {
        let interval = DEFAULT_REFRESH_INTERVAL;
        let session = Session::mint();
        let mut source = ProcFsSource::live();
        let (mut view, _) = start_from_source(&session, &mut source).unwrap();
        let before = session.current_revision();
        assert_eq!(before, 1);

        let mut worker = Worker::start();
        let pid = worker.pid();
        let started = Instant::now();
        let mut appeared = None;
        for tick in 1..=2 {
            std::thread::sleep(interval);
            view.refresh(&session, &mut source).unwrap();
            if let Some(row) = row_for(&session, pid) {
                appeared = Some((tick, started.elapsed(), row));
                break;
            }
        }
        let (appeared_tick, appeared_after, row) =
            appeared.expect("an owned worker must reach the collection within two intervals");
        let Value::List(cells) = &row.1 else {
            panic!("expected table cells")
        };
        assert_eq!(cells[0], Value::UnsignedInt(u64::from(pid)));
        assert_eq!(cells[1], Value::String(WORKER_NAME.into()));
        // The shell was built once and is refreshed in place.
        session.with_store(|store| assert_eq!(store.node_count(), 5));
        assert!(session.current_revision() > before);

        // A refresh that sees the same worker leaves its row identity alone.
        view.refresh(&session, &mut source).unwrap();
        assert_eq!(
            row_for(&session, pid).map(|(id, _)| id),
            Some(row.0),
            "a process that did not change must keep its row"
        );

        worker.0.kill().unwrap();
        worker.0.wait().unwrap();
        let ended = Instant::now();
        let mut gone = None;
        for tick in 1..=2 {
            std::thread::sleep(interval);
            view.refresh(&session, &mut source).unwrap();
            if row_for(&session, pid).is_none() {
                gone = Some((tick, ended.elapsed()));
                break;
            }
        }
        let (gone_tick, gone_after) =
            gone.expect("an ended worker must leave the collection within two intervals");
        session.with_store(|store| assert_eq!(store.node_count(), 5));
        println!(
            "PX-004 live evidence: interval={interval:?} worker_pid={pid} item_id={} \
             appeared_tick={appeared_tick} appeared_after={appeared_after:?} \
             exited_tick={gone_tick} exited_after={gone_after:?} rows={} revision={} status={:?}",
            row.0.get(),
            view.row_count(),
            session.current_revision(),
            view.status(),
        );
    }

    /// PX-005 acceptance on a real host: a bounded allocation this test owns
    /// moves the resident value the collector publishes for this very process,
    /// and the number it publishes agrees with the kernel's own `VmRSS` for the
    /// same process — which the kernel reports in kibibytes, so the page-size
    /// conversion is checked against a unit this app did not choose.
    ///
    /// Tolerances rather than equality, because live accounting is not a
    /// contract: the two reads happen at different instants, every other test
    /// thread in this process shares its address space, and a host under memory
    /// pressure may reclaim pages between them. Both bounds are one-sided in the
    /// direction that cannot pass by accident — the growth must be *at least*
    /// what was touched, less the stated slack, and the two independent reports
    /// must agree *within* it.
    #[test]
    fn a_test_owned_allocation_moves_the_published_resident_memory() {
        /// Bytes this test allocates and touches, then frees.
        const TOUCHED: u64 = 64 * 1024 * 1024;
        /// Slack allowed between what was touched and the growth observed.
        const GROWTH_SLACK: u64 = 16 * 1024 * 1024;
        /// Slack allowed between the collector's byte count and the kernel's own
        /// `VmRSS` for the same process, read moments apart.
        const AGREEMENT_SLACK: u64 = 32 * 1024 * 1024;
        /// Smallest page size any Linux host uses, so writing at this stride
        /// touches every page whatever the real page size is.
        const STRIDE: usize = 4096;

        let own = std::process::id();
        let session = Session::mint();
        let mut source = ProcFsSource::live();
        let (mut view, first) = start_from_source(&session, &mut source).unwrap();
        let before = resident_bytes_of(&first, own);
        assert_eq!(
            published_resident(&session, own),
            srui_process_explorer::metric::format_iec_bytes(before),
            "the first publication already carries this process's own resident memory"
        );

        // Touched, not merely allocated: an untouched mapping is not resident,
        // and one byte per page is what forces each page to exist.
        let mut block = vec![0u8; TOUCHED as usize];
        let mut offset = 0;
        while offset < block.len() {
            block[offset] = 1;
            offset += STRIDE;
        }
        std::hint::black_box(&block);

        let snapshot = source.snapshot();
        let kernel = vm_rss_bytes();
        let after = resident_bytes_of(&snapshot, own);
        let outcome = view.apply(&session, LIVE_STATUS_TEXT, &snapshot).unwrap();
        assert!(
            after >= before + TOUCHED - GROWTH_SLACK,
            "resident memory went from {before} to {after} bytes, which is less than the \
             {TOUCHED} bytes this test touched, less {GROWTH_SLACK} bytes of slack"
        );
        assert!(
            after.abs_diff(kernel) <= AGREEMENT_SLACK,
            "the collector published {after} bytes where the kernel's own VmRSS says {kernel}"
        );
        // The moved value reached the published row, through a refresh of the
        // shell that was already built.
        assert!(outcome.updated >= 1, "{outcome:?}");
        assert_eq!(
            published_resident(&session, own),
            srui_process_explorer::metric::format_iec_bytes(after)
        );
        session.with_store(|store| assert_eq!(store.node_count(), 5));
        println!(
            "PX-005 live evidence: pid={own} touched={TOUCHED} before={before} after={after} \
             growth={} kernel_vm_rss={kernel} published={:?} records={} updated={}",
            after - before,
            published_resident(&session, own),
            snapshot.records.len(),
            outcome.updated,
        );
        drop(block);
    }

    /// PX-006 acceptance on a real host: the CPU share published for a bounded
    /// worker this test owns, one runnable for the whole interval and one asleep,
    /// is what the kernel's own counters say. Both are first published as warming
    /// up.
    ///
    /// Checked against ground truth this test reads itself, not against a fixed
    /// band, because the scheduler is not a contract: a loaded host may give the
    /// burner any share of a CPU. Each worker's `utime + stime` and an `Instant`
    /// are read just before and just after each scan, so the counter srtop read
    /// inside a scan, and the moment it read it, lie between them. The share it
    /// publishes therefore lies between the smallest tick delta over the longest
    /// interval and the largest delta over the shortest, less the tenth of a
    /// point it truncates, whatever share the burner was given.
    #[test]
    fn a_test_owned_busy_worker_reads_about_one_cpu_and_a_sleeping_one_about_none() {
        const INTERVAL: Duration = Duration::from_secs(2);
        /// srtop publishes tenths of a point, truncated.
        const TRUNCATION: f64 = 0.1;
        /// Room for floating-point rounding in the bounds, far below a tenth.
        const ROUNDING: f64 = 1e-6;

        let burner = Worker::burn();
        let sleeper = Worker::start();
        let workers = [("burner", burner.pid()), ("sleeper", sleeper.pid())];
        let pids = workers.map(|(_, pid)| pid);
        let mut source = ProcFsSource::live();
        let session = Session::mint();
        let a = Counters::before(&pids);
        let (mut view, first) = start_from_source(&session, &mut source).unwrap();
        let b = Counters::after(&pids);
        assert_eq!(cpu_for(&first, burner.pid()), CpuUsage::WarmingUp);
        assert_eq!(cpu_for(&first, sleeper.pid()), CpuUsage::WarmingUp);
        assert_eq!(published_cpu(&session, burner.pid()), "Warming up");

        std::thread::sleep(INTERVAL);
        let c = Counters::before(&pids);
        let second = source.snapshot();
        let d = Counters::after(&pids);
        // The rate srtop reads too: this process's own `AT_CLKTCK`.
        let ticks_per_second = own_auxv(AT_CLKTCK).expect("a Linux process has AT_CLKTCK");
        let (shortest, longest) = (c.at - b.at, d.at - a.at);
        let share = |ticks: u64, over: Duration| {
            ticks as f64 / ticks_per_second as f64 / over.as_secs_f64() * 100.0
        };
        let mut evidence = Vec::new();
        let mut used = Vec::new();
        for (index, (role, pid)) in workers.into_iter().enumerate() {
            let CpuUsage::Measured(interval) = cpu_for(&second, pid) else {
                panic!("a second live sample of the {role} must be measured")
            };
            let published = srui_process_explorer::metric::cpu_tenths_of_percent(&interval)
                .expect("a live interval is publishable") as f64
                / 10.0;
            let fewest = c.ticks[index].saturating_sub(b.ticks[index]);
            let most = d.ticks[index].saturating_sub(a.ticks[index]);
            let (lower, upper) = (share(fewest, longest), share(most, shortest));
            assert!(
                lower - TRUNCATION - ROUNDING <= published && published <= upper + ROUNDING,
                "the {role} was published at {published:.1}%, outside [{lower:.3}%, \
                 {upper:.3}%]: {fewest}..{most} ticks at {ticks_per_second}/s over \
                 {shortest:?}..{longest:?}"
            );
            assert_eq!(
                interval.ticks_per_second, ticks_per_second as u64,
                "srtop and this test read different clock tick rates"
            );
            evidence.push(format!(
                "{role}_pid={pid} {role}={published:.1}% in [{lower:.3}%, {upper:.3}%] \
                 {role}_ticks={fewest}..{most}"
            ));
            used.push(most);
        }
        // The one check that does not depend on the scheduler at all: the burner
        // used more CPU than the sleeper did.
        assert!(
            used[0] > used[1],
            "the burner used {} ticks and the sleeper {}",
            used[0],
            used[1]
        );
        let outcome = view.apply(&session, LIVE_STATUS_TEXT, &second).unwrap();
        assert!(outcome.updated >= 2, "{outcome:?}");
        assert_eq!(
            published_cpu(&session, burner.pid()),
            srui_process_explorer::metric::cpu_cell(&cpu_for(&second, burner.pid()))
        );
        session.with_store(|store| assert_eq!(store.node_count(), 5));
        println!(
            "PX-006 live evidence: clk_tck={ticks_per_second} {} interval={shortest:?}..{longest:?} \
             burner_cell={:?} sleeper_cell={:?} records={} updated={}",
            evidence.join(" "),
            published_cpu(&session, burner.pid()),
            published_cpu(&session, sleeper.pid()),
            second.records.len(),
            outcome.updated,
        );
    }

    /// Each worker's CPU counter, with the moment around it: the instant is read
    /// before the counters in [`Counters::before`] and after them in
    /// [`Counters::after`], so a pair of them brackets everything in between.
    struct Counters {
        at: Instant,
        ticks: Vec<u64>,
    }

    impl Counters {
        fn before(pids: &[u32]) -> Self {
            let at = Instant::now();
            Self {
                at,
                ticks: pids.iter().map(|&pid| cpu_ticks(pid)).collect(),
            }
        }

        fn after(pids: &[u32]) -> Self {
            let ticks = pids.iter().map(|&pid| cpu_ticks(pid)).collect();
            Self {
                at: Instant::now(),
                ticks,
            }
        }
    }

    /// `utime + stime` of `pid`, fields 14 and 15 of `/proc/<pid>/stat`, read
    /// here rather than through the collector this checks.
    fn cpu_ticks(pid: u32) -> u64 {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .expect("a test's own worker has a stat file");
        let after_name = &stat[stat.rfind(')').expect("a stat line names its command") + 1..];
        // Field 3 is the first after the name, so 14 and 15 are the 12th and 13th.
        let fields: Vec<&str> = after_name.split_whitespace().collect();
        fields[11].parse::<u64>().expect("utime is a count")
            + fields[12].parse::<u64>().expect("stime is a count")
    }

    /// The CPU usage `snapshot` observed for `pid`.
    fn cpu_for(snapshot: &srui_process_explorer::source::ProcessSnapshot, pid: u32) -> CpuUsage {
        snapshot
            .records
            .iter()
            .find(|record| record.key.pid == Observed::Known(pid))
            .expect("a live scan lists the test's own worker")
            .cpu
    }

    /// The CPU cell a client would show for `pid`.
    fn published_cpu(session: &Session, pid: u32) -> String {
        let (_, value) = row_for(session, pid).expect("the row of this process is published");
        let Value::List(cells) = value else {
            panic!("expected table cells")
        };
        let Value::String(text) = &cells[3] else {
            panic!("a metric cell is published as text, got {:?}", cells[3])
        };
        text.clone()
    }

    /// The resident bytes `snapshot` observed for `pid`, which a live scan of
    /// this process's own `/proc` entry must be able to read.
    fn resident_bytes_of(
        snapshot: &srui_process_explorer::source::ProcessSnapshot,
        pid: u32,
    ) -> u64 {
        let record = snapshot
            .records
            .iter()
            .find(|record| record.key.pid == Observed::Known(pid))
            .expect("a live scan lists the scanning process itself");
        match record.resident {
            Observed::Known(bytes) => bytes,
            ref unread => {
                panic!("this process's own resident memory must be readable: {unread:?}")
            }
        }
    }

    /// The resident cell a client would show for `pid`.
    fn published_resident(session: &Session, pid: u32) -> String {
        let (_, value) = row_for(session, pid).expect("the row of this process is published");
        let Value::List(cells) = value else {
            panic!("expected table cells")
        };
        let Value::String(text) = &cells[2] else {
            panic!("a metric cell is published as text, got {:?}", cells[2])
        };
        text.clone()
    }

    /// `VmRSS` from this process's `/proc/self/status`, in bytes. The kernel
    /// prints it in kibibytes, so it is an independent witness to the page-size
    /// conversion this collector performs.
    fn vm_rss_bytes() -> u64 {
        let status =
            std::fs::read_to_string("/proc/self/status").expect("a Linux host has a status file");
        let line = status
            .lines()
            .find(|line| line.starts_with("VmRSS:"))
            .expect("a process with an address space reports VmRSS");
        let mut fields = line.split_whitespace();
        let kibibytes: u64 = fields
            .nth(1)
            .expect("VmRSS carries a value")
            .parse()
            .expect("VmRSS is a decimal count");
        assert_eq!(fields.next(), Some("kB"), "{line}");
        kibibytes * 1024
    }

    /// The published row of `pid`, if the collection currently holds one.
    fn row_for(session: &Session, pid: u32) -> Option<(ItemId, Value)> {
        session.with_store(|store| {
            store
                .get_model(MODEL)
                .unwrap()
                .items
                .values()
                .find(|item| {
                    matches!(&item.value, Value::List(cells)
                        if cells[0] == Value::UnsignedInt(u64::from(pid)))
                })
                .map(|item| (item.item_id, item.value.clone()))
        })
    }
}
