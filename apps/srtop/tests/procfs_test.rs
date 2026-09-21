//! PX-003 acceptance: one real Linux snapshot, process-instance identity, and
//! degraded scans that never masquerade as an authoritative empty result
//! (K1; §§8, 12, 22, 29).
//!
//! Fixture roots make every branch deterministic on any platform; the live
//! `/proc` cases are compiled for Linux, where they must run for real. This
//! suite must run unprivileged, like the app itself: the denied-record case
//! relies on mode 0 being unreadable.
use srui_process_explorer::procfs::{
    parse_pid, parse_stat, ProcFsSource, LIVE_STATUS_TEXT, MAX_FILE_BYTES,
};
use srui_process_explorer::published_status;
use srui_process_explorer::source::{
    Completeness, CreationToken, DisplayName, IssueScope, MissingReason, Observed, ProcessSource,
    SourceId, FAKE_STATUS_TEXT, MAX_RECORDED_ISSUES,
};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

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
        self
    }

    fn process(&self, pid: u32, comm: &[u8], ticks: u64) -> &Self {
        self.raw(pid, &stat_line(pid, comm, ticks))
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

    /// Shapes the root like a real procfs mount whose `self` symlink names this
    /// process as *that mount* numbers it, with its own `ns/pid` link beneath.
    fn self_numbered(&self, named: u32, namespace: &str) -> &Self {
        let _ = fs::remove_dir_all(self.0.join("self"));
        let _ = fs::remove_file(self.0.join("self"));
        fs::create_dir_all(self.0.join(format!("{named}/ns"))).unwrap();
        std::os::unix::fs::symlink(namespace, self.0.join(format!("{named}/ns/pid"))).unwrap();
        std::os::unix::fs::symlink(named.to_string(), self.0.join("self")).unwrap();
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

/// `/proc/<pid>/stat`: `pid (comm) state ...` with start time as field 22 (K1).
fn stat_line(pid: u32, comm: &[u8], ticks: u64) -> Vec<u8> {
    let mut line = format!("{pid} (").into_bytes();
    line.extend_from_slice(comm);
    line.extend_from_slice(b") S");
    for filler in 1..=18 {
        line.extend_from_slice(format!(" {filler}").as_bytes());
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
    assert!(scoped.status_text().contains("fixture"));
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

    // The same mount read by the process it numbers: the namespace is known.
    let own = ProcFixture::new();
    own.identity("fixture-host", "boot-a", "pid:[4026531836]")
        .process(1, b"systemd", 7)
        .self_numbered(std::process::id(), "pid:[4026531999]");
    let snapshot = own.source().snapshot();
    assert_eq!(
        snapshot.records[0].key.pid_namespace,
        Observed::Known(srui_process_explorer::source::PidNamespaceId(4_026_531_999))
    );
    assert!(snapshot
        .completeness
        .issues()
        .iter()
        .all(|issue| issue.scope != IssueScope::PidNamespace));
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
    assert_eq!(snapshot.capped, 3);
    assert_eq!(snapshot.completeness.skipped(), 0);
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
        parse_stat(808, &stat_line(808, hostile, 77)).map(|(_, ticks)| ticks),
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
    let mut padded = stat_line(900, b"padded", 31);
    padded.pop();
    padded.extend(std::iter::repeat_n(b' ', MAX_FILE_BYTES as usize));
    padded.extend_from_slice(b"\n");
    fixture.raw(900, &padded);
    let snapshot = fixture.source().snapshot();
    assert_eq!(snapshot.records.len(), 1);
    assert_eq!(
        snapshot.records[0].key.creation,
        CreationToken::LinuxBootTicks(31)
    );
}

#[cfg(target_os = "linux")]
mod live {
    use super::*;
    use srui_process_explorer::{initialize_from_source, published_status, MODEL};
    use srui_sdk::Value;
    use srui_sessiond::Session;
    use std::process::{Child, Command, Stdio};
    use std::time::{Duration, Instant};

    /// A bounded, disposable worker owned by this test.
    struct Worker(Child);

    impl Worker {
        fn start() -> Self {
            let child = Command::new("/bin/sleep")
                .arg("47")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("the test owns this worker");
            let worker = Self(child);
            let deadline = Instant::now() + Duration::from_secs(10);
            while !PathBuf::from(format!("/proc/{}/stat", worker.pid())).exists() {
                assert!(Instant::now() < deadline, "worker never appeared in /proc");
                std::thread::sleep(Duration::from_millis(10));
            }
            worker
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
        assert_eq!(record.display_name.as_str(), "sleep");
        let CreationToken::LinuxBootTicks(ticks) = record.key.creation else {
            panic!("a live Linux record must carry a boot-ticks creation token")
        };
        assert!(ticks > 0);
        let raw = std::fs::read(format!("/proc/{pid}/stat")).unwrap();
        assert_eq!(parse_stat(pid, &raw).map(|(_, ticks)| ticks), Some(ticks));
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
            assert_eq!(cells[1], Value::String("sleep".into()));
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
}
