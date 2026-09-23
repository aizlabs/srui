//! PX-004 review follow-up: an incomplete scan withholds deletion only for the
//! absences it could not account for (§§8, 12.1, 22; PR #64 review findings).
//!
//! Round 1's defect: one persistently denied `/proc/<pid>/stat` marks every scan
//! incomplete, so a single global "not authoritative" flag suppressed every
//! deletion forever. On a shared host that is the steady state, and ordinary
//! churn then accumulates a permanently stale row per exit until the model's
//! cached-item limit refuses the next refresh.
//!
//! Round 2's defect: the uncertain identities were read back out of the bounded
//! `EnumerationIssue` list, so a host with more than `MAX_RECORDED_ISSUES`
//! unreadable records — equally routine — fell back to global retention on every
//! scan, reinstating that same growth at a higher threshold. The skipped PIDs
//! are now the scan's own knowledge, bounded by its record bound, and the issue
//! list only explains.
//!
//! Round 3's defect: a scan that reached its own record bound was treated as
//! unable to name anything, so a host with more readable PIDs than `MAX_RECORDS`
//! returned global retention on *every* tick and the collection grew from the
//! record bound until the model's item limit refused the next refresh. Reading
//! each further record's detail is what the bound stops; listing it is not, and
//! a listing entry is a PID, so those entries are named too and only a ledger
//! asked to hold more than `MAX_UNCERTAIN_PIDS` identities is unenumerable.
//!
//! Round 5's defect: a scan that could not read one *global* identity file —
//! `sys/kernel/hostname`, `sys/kernel/random/boot_id`, `1/ns/pid` — skips no PID,
//! so retention keeps nothing, while every record's `ProcessKey` loses that
//! component and becomes a new key. The whole table was therefore deleted and
//! reinserted under fresh item IDs on the degraded tick, and again on the tick
//! the file came back. The session now remembers the last value each global
//! component was observed to hold and keys a scan that could not read one with
//! it, while a component that comes back *different* is treated as what it is: a
//! different source, whose records take new identities.
use srui_process_explorer::procfs::{MAX_RECORDS, MAX_UNCERTAIN_PIDS};
use srui_process_explorer::refresh::ProcessView;
use srui_process_explorer::source::*;
use srui_process_explorer::{start_from_source, MODEL, STATUS};
use srui_sdk::{ItemId, Operation, Value, TEXT};
use srui_sessiond::Session;
use std::cell::Cell;
use std::collections::BTreeSet;
use std::time::{Duration, SystemTime};

const SOURCE: &str = "retention-fixture-v1";
const STATUS_TEXT: &str = "Read-only · Retention fixture";

fn key(pid: u32) -> ProcessKey {
    ProcessKey {
        source: SourceId(SOURCE.into()),
        host: Observed::Known(HostId("fixture-host".into())),
        boot: Observed::Known(BootId("fixture-boot".into())),
        pid_namespace: Observed::Known(PidNamespaceId(4_026_531_836)),
        pid: Observed::Known(pid),
        creation: CreationToken::Opaque(format!("instance-{pid}")),
    }
}

fn record(pid: u32) -> ProcessRecord {
    ProcessRecord {
        key: key(pid),
        display_name: format!("worker-{pid}").as_str().into(),
    }
}

/// A complete scan listing exactly `pids`, which callers then degrade.
fn scan(pids: &[u32]) -> ProcessSnapshot {
    ProcessSnapshot {
        source: SourceId(SOURCE.into()),
        sampled_at: SnapshotTime(SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000)),
        records: pids.iter().copied().map(record).collect(),
        vanished: 0,
        capped: CappedRecords::none(),
        completeness: Completeness::Complete,
    }
}

/// The ledger a scan builds for the entries it listed past its record bound:
/// every one of them named, up to `limit` of them, exactly as the collector
/// records them while it walks the rest of the listing.
fn capped_beyond(pids: &[u32], limit: usize) -> CappedRecords {
    let mut capped = CappedRecords::with_limit(limit);
    for pid in pids {
        capped.record(*pid);
    }
    capped
}

/// The completeness of a scan that stopped at its own record bound: one
/// explanation for the bound, and not one unreadable record.
fn at_the_record_bound(limit: usize) -> Completeness {
    Completeness::from_scan(
        SkippedRecords::none(),
        vec![EnumerationIssue {
            scope: IssueScope::Limit,
            reason: MissingReason::Unavailable,
            detail: format!("record limit {limit} reached"),
        }],
    )
}

/// The completeness a real procfs scan records when exactly these records could
/// not be read: each PID recorded as the scan skips it, and one explanation each
/// for as many as `MAX_RECORDED_ISSUES` retains. `built` counts the explanations
/// actually constructed, so a caller can check that the bound still costs
/// nothing past it (PX-003).
fn denied_within(pids: &[u32], limit: usize, built: &Cell<usize>) -> Completeness {
    let mut skipped = SkippedRecords::with_limit(limit);
    let mut issues = Vec::new();
    for pid in pids {
        skipped.record(*pid);
        record_issue(&mut issues, || {
            built.set(built.get() + 1);
            EnumerationIssue {
                scope: IssueScope::Process(*pid),
                reason: MissingReason::Denied,
                detail: format!("{pid}/stat: Permission denied (os error 13)"),
            }
        });
    }
    Completeness::from_scan(skipped, issues)
}

/// The same, for a scan whose record bound is the collector's own.
fn denied(pids: &[u32]) -> Completeness {
    denied_within(pids, MAX_RECORDS, &Cell::new(0))
}

/// A source that answers every sample with the same prepared snapshot; the
/// refresh ticks themselves go through [`ProcessView::apply`].
struct Once(ProcessSnapshot);

impl ProcessSource for Once {
    fn status_text(&self) -> &str {
        STATUS_TEXT
    }

    fn snapshot(&mut self) -> ProcessSnapshot {
        self.0.clone()
    }
}

fn started(pids: &[u32]) -> (Session, ProcessView) {
    let session = Session::mint();
    let (view, _) = start_from_source(&session, &mut Once(scan(pids))).unwrap();
    (session, view)
}

/// The rows a client holds: item identity and PID, in published order.
fn published(session: &Session) -> Vec<(ItemId, u64)> {
    session.with_store(|store| {
        let model = store.get_model(MODEL).expect("the collection model exists");
        assert_eq!(model.item_count, model.items.len() as u64);
        model
            .items
            .values()
            .map(|item| {
                let Value::List(cells) = &item.value else {
                    panic!("expected table cells")
                };
                let Value::UnsignedInt(pid) = &cells[0] else {
                    panic!("expected an observed PID")
                };
                (item.item_id, *pid)
            })
            .collect()
    })
}

fn pids(rows: &[(ItemId, u64)]) -> Vec<u64> {
    rows.iter().map(|(_, pid)| *pid).collect()
}

fn status_text(session: &Session) -> String {
    session.with_store(|store| {
        let Some(Value::String(text)) = store.get_node(STATUS).unwrap().get_property(TEXT).cloned()
        else {
            panic!("the status node must carry text")
        };
        text
    })
}

/// The positive case. A scan that named the one record it could not read has
/// accounted for every other absence, so an unrelated process that ended is
/// deleted on the very next tick while the unread record's row stays.
#[test]
fn a_named_skip_keeps_only_its_own_row_while_an_unrelated_exit_is_deleted() {
    let (session, mut view) = started(&[4101, 4102, 4103]);
    let before = published(&session);
    assert_eq!(pids(&before), vec![4101, 4102, 4103]);

    // 4102 is denied and says so; 4103 exited between the two scans, which is
    // ordinary churn and never degrades completeness.
    let mut tick = scan(&[4101]);
    tick.vanished = 1;
    tick.completeness = denied(&[4102]);
    let outcome = view.apply(&session, STATUS_TEXT, &tick).unwrap();

    assert_eq!(
        (outcome.inserted, outcome.deleted, outcome.retained),
        (0, 1, 1),
        "the ended process is deleted and only the unread record is retained"
    );
    let after = published(&session);
    assert_eq!(pids(&after), vec![4101, 4102]);
    assert_eq!(
        after[1].0, before[1].0,
        "the row this scan could not read keeps the identity it already had"
    );
    assert_eq!(
        status_text(&session),
        format!(
            "{STATUS_TEXT} · incomplete scan · 1 process listed · 1 unreadable · \
             1 row retained from an earlier scan"
        )
    );

    // The denied record recovering deletes nothing and moves nothing.
    let outcome = view
        .apply(&session, STATUS_TEXT, &scan(&[4101, 4102]))
        .unwrap();
    assert_eq!(
        (outcome.inserted, outcome.deleted, outcome.retained),
        (0, 0, 0)
    );
    assert_eq!(published(&session), after, "recovery moves no row");
    assert_eq!(status_text(&session), STATUS_TEXT);
}

/// The negative case. Where the uncertainty really is global — the records were
/// never read, so their identities are unknown — nothing may be deleted: the
/// root could not be listed, an entry of it could not be examined, the scan
/// stopped at its own record bound, or it skipped more records than that same
/// bound lets it name.
#[test]
fn a_scan_that_cannot_enumerate_its_uncertainty_still_retains_every_row() {
    let unlistable_root = || {
        let mut tick = scan(&[]);
        tick.completeness = Completeness::from_scan(
            SkippedRecords::unenumerable(),
            vec![EnumerationIssue {
                scope: IssueScope::Root,
                reason: MissingReason::Denied,
                detail: "/proc: Permission denied (os error 13)".into(),
            }],
        );
        tick
    };
    let unreadable_entry = || {
        let mut tick = scan(&[4101]);
        // The directory entry itself could not be read, so the record it names
        // has no known PID.
        let mut skipped = SkippedRecords::with_limit(MAX_RECORDS);
        skipped.unnamed();
        tick.completeness = Completeness::from_scan(
            skipped,
            vec![EnumerationIssue {
                scope: IssueScope::Entry,
                reason: MissingReason::Unavailable,
                detail: "directory entry: Input/output error (os error 5)".into(),
            }],
        );
        tick
    };
    let capped_past_the_ledger_bound = || {
        let mut tick = scan(&[4101]);
        // More entries were listed past the record bound than the ledger naming
        // them may hold, so the scan says so instead of growing a set per scan —
        // and none of the PIDs it did name is 4102 or 4103.
        let beyond: Vec<u32> = (9000..9010).collect();
        tick.capped = capped_beyond(&beyond, 2);
        tick.completeness = at_the_record_bound(1);
        tick
    };
    let past_the_record_bound = || {
        let mut tick = scan(&[4101]);
        // More records were skipped than this scan's own record bound lets it
        // name, so the uncertain set stops being enumerable rather than growing
        // — and none of the PIDs it did name is 4102 or 4103.
        let denied: Vec<u32> = (9000..9010).collect();
        tick.completeness = denied_within(&denied, 2, &Cell::new(0));
        tick
    };

    for (name, build) in [
        ("root", &unlistable_root as &dyn Fn() -> ProcessSnapshot),
        ("entry", &unreadable_entry),
        (
            "capped past the ledger bound",
            &capped_past_the_ledger_bound,
        ),
        ("past the record bound", &past_the_record_bound),
    ] {
        let tick = build();
        let (session, mut view) = started(&[4101, 4102, 4103]);
        let before = published(&session);
        let outcome = view.apply(&session, STATUS_TEXT, &tick).unwrap();
        assert_eq!(outcome.deleted, 0, "{name}: nothing may be deleted");
        assert_eq!(
            published(&session),
            before,
            "{name}: every last-known row is kept, in place, with its identity"
        );
        assert!(
            status_text(&session).contains("retained from an earlier scan"),
            "{name}: {}",
            status_text(&session)
        );
        assert!(
            tick.retention().is_global(),
            "{name}: this scan cannot name which absences are uncertain"
        );
    }

    // None of those states is reachable merely by skipping many records. The
    // same scan, under the collector's real record bound, names every one of the
    // records it skipped even though `MAX_RECORDED_ISSUES` dropped most of the
    // explanations — and 4102 and 4103 are not among them, so their rows go.
    let mut accounted = scan(&[4101]);
    let unreadable: Vec<u32> = (9000..9000 + MAX_RECORDED_ISSUES as u32 + 8).collect();
    accounted.completeness = denied(&unreadable);
    assert_eq!(
        accounted.completeness.issues().len(),
        MAX_RECORDED_ISSUES,
        "the explanations are truncated"
    );
    assert!(
        !accounted.retention().is_global(),
        "many skips are not uncertainty: every one of them was named"
    );
    assert_eq!(
        accounted.retention().uncertain_pids().map(BTreeSet::len),
        Some(unreadable.len()),
        "the uncertain set is the scan's own knowledge, not its retained explanations"
    );
    let (session, mut view) = started(&[4101, 4102, 4103]);
    let outcome = view.apply(&session, STATUS_TEXT, &accounted).unwrap();
    assert_eq!((outcome.deleted, outcome.retained), (2, 0));
    assert_eq!(pids(&published(&session)), vec![4101]);

    // Nor is any of them reachable merely by being capped. Under the collector's
    // real ledger bound the scan names every entry it listed past its record
    // bound — a `/proc` entry is a PID, whatever the bound stopped it reading —
    // and 4102 and 4103 are not among them, so their rows go.
    let mut capped = scan(&[4101]);
    let beyond: Vec<u32> = (9000..9000 + MAX_RECORDED_ISSUES as u32 + 8).collect();
    capped.capped = capped_beyond(&beyond, MAX_UNCERTAIN_PIDS);
    capped.completeness = at_the_record_bound(1);
    assert_eq!(
        capped.completeness.issues().len(),
        1,
        "the record bound is explained once, not per entry"
    );
    assert!(
        !capped.retention().is_global(),
        "a host larger than the record bound is not an unknowable host"
    );
    assert_eq!(
        capped.retention().uncertain_pids().map(BTreeSet::len),
        Some(beyond.len()),
        "the uncertain set is every entry the listing named beyond the bound"
    );
    let (session, mut view) = started(&[4101, 4102, 4103]);
    let outcome = view.apply(&session, STATUS_TEXT, &capped).unwrap();
    assert_eq!((outcome.deleted, outcome.retained), (2, 0));
    assert_eq!(pids(&published(&session)), vec![4101]);
}

/// The round-3 regression, over a long run. Every tick of a host with more
/// readable PIDs than one scan publishes is capped, so this is the steady state,
/// not an incident: the published collection must stay the size of the host's
/// own listing — the records this scan confirmed plus one row per entry it named
/// beyond its bound — however long the churn runs.
#[test]
fn churn_on_a_scan_capped_on_every_tick_holds_the_published_rows_at_the_listing() {
    // This collector publishes two records per scan and lists two more it never
    // reads, so the published collection is bounded at four rows: two confirmed,
    // two named beyond the bound.
    const RECORD_BOUND: usize = 2;
    let beyond = [4201u32, 4202];
    const BOUND: usize = RECORD_BOUND + 2;

    let (session, mut view) = started(&[4101, 4201, 4202, 5000]);
    let capped_rows: Vec<(ItemId, u64)> = published(&session)[1..=beyond.len()].to_vec();
    assert_eq!(pids(&capped_rows), vec![4201, 4202]);

    for tick in 1..=250u32 {
        // Every tick: the same stable process, the same two entries past the
        // record bound, and one short-lived process replacing the previous one.
        let ephemeral = 5000 + tick;
        let mut scanned = scan(&[4101, ephemeral]);
        assert_eq!(scanned.records.len(), RECORD_BOUND);
        scanned.vanished = 1;
        scanned.capped = capped_beyond(&beyond, MAX_UNCERTAIN_PIDS);
        scanned.completeness = at_the_record_bound(RECORD_BOUND);
        view.apply(&session, STATUS_TEXT, &scanned).unwrap();

        let rows = published(&session);
        assert_eq!(
            rows.len(),
            BOUND,
            "tick {tick}: two confirmed rows and two rows named beyond the bound"
        );
        assert!(
            !scanned.retention().is_global(),
            "tick {tick}: a capped scan still accounts for every other absence"
        );
        assert_eq!(view.row_count(), rows.len());
        assert_eq!(
            pids(&rows).into_iter().collect::<BTreeSet<u64>>(),
            BTreeSet::from([4101, 4201, 4202, u64::from(ephemeral)]),
            "tick {tick}: every process that ended has left the collection"
        );
        for row in &capped_rows {
            assert!(
                rows.contains(row),
                "tick {tick}: each entry past the bound keeps its row and its identity"
            );
        }
    }
    // The status says what happened in counts and fixed wording: two records
    // listed, two entries never read, two rows carried over. Nothing here claims
    // a read failure, and nothing here is a process's own text.
    assert_eq!(
        status_text(&session),
        format!(
            "{STATUS_TEXT} · incomplete scan · 2 processes listed · 2 beyond the record limit · \
             2 rows retained from an earlier scan"
        )
    );
    assert!(
        !status_text(&session).contains("unreadable"),
        "{}",
        status_text(&session)
    );
}

/// The round-2 regression, directly. A scan that skips more records than the
/// bounded issue list can explain still accounts for every other absence: the
/// unrelated process that ended is deleted on that same tick, and all forty
/// unreadable rows keep their places and their item IDs.
#[test]
fn more_skips_than_explanations_still_deletes_an_unrelated_exit() {
    let unreadable: Vec<u32> = (4200..4240).collect();
    assert!(unreadable.len() > MAX_RECORDED_ISSUES);
    let mut listed = vec![4101];
    listed.extend(unreadable.iter().copied());
    // The process that ends between the two scans, published last.
    listed.push(4102);
    let (session, mut view) = started(&listed);
    let before = published(&session);
    assert_eq!(before.len(), unreadable.len() + 2);

    let built = Cell::new(0);
    let mut tick = scan(&[4101]);
    tick.vanished = 1;
    tick.completeness = denied_within(&unreadable, MAX_RECORDS, &built);
    assert_eq!(
        built.get(),
        MAX_RECORDED_ISSUES,
        "an explanation past the bound is never built"
    );
    assert_eq!(
        tick.completeness.issues().len(),
        MAX_RECORDED_ISSUES,
        "the issue list is truncated for this scan"
    );
    assert_eq!(tick.completeness.skipped(), unreadable.len());

    let outcome = view.apply(&session, STATUS_TEXT, &tick).unwrap();
    assert_eq!(
        (outcome.inserted, outcome.deleted, outcome.retained),
        (0, 1, unreadable.len()),
        "the ended process is deleted and only the unread records are retained"
    );
    let after = published(&session);
    let mut expected: Vec<u64> = vec![4101];
    expected.extend(unreadable.iter().map(|pid| u64::from(*pid)));
    assert_eq!(pids(&after), expected);
    assert_eq!(
        after[1..],
        before[1..before.len() - 1],
        "every unread row keeps the identity and the place it already had"
    );
    assert_eq!(
        status_text(&session),
        format!(
            "{STATUS_TEXT} · incomplete scan · 1 process listed · 40 unreadable · \
             40 rows retained from an earlier scan"
        )
    );
}

/// The same regression over a long run: more permanently denied records than the
/// issue list can explain must still leave the published collection the size of
/// the host, not the size of its history.
#[test]
fn churn_under_many_permanently_denied_records_does_not_grow_the_published_rows() {
    let unreadable: Vec<u32> = (4200..4240).collect();
    assert!(unreadable.len() > MAX_RECORDED_ISSUES);
    let mut listed = vec![4101];
    listed.extend(unreadable.iter().copied());
    listed.push(5000);
    let (session, mut view) = started(&listed);
    let denied_rows: Vec<(ItemId, u64)> = published(&session)[1..=unreadable.len()].to_vec();
    let expected_rows = unreadable.len() + 2;

    for tick in 1..=120u32 {
        let ephemeral = 5000 + tick;
        let mut scanned = scan(&[4101, ephemeral]);
        scanned.vanished = 1;
        scanned.completeness = denied(&unreadable);
        view.apply(&session, STATUS_TEXT, &scanned).unwrap();

        let rows = published(&session);
        assert_eq!(
            rows.len(),
            expected_rows,
            "tick {tick}: one stable row, forty retained rows, one live row"
        );
        assert_eq!(view.row_count(), rows.len());
        let mut expected: BTreeSet<u64> = unreadable.iter().map(|pid| u64::from(*pid)).collect();
        expected.insert(4101);
        expected.insert(u64::from(ephemeral));
        assert_eq!(
            pids(&rows).into_iter().collect::<BTreeSet<u64>>(),
            expected,
            "tick {tick}: every process that ended has left the collection"
        );
        for row in &denied_rows {
            assert!(
                rows.contains(row),
                "tick {tick}: each denied record keeps its row and its identity"
            );
        }
    }
    assert_eq!(
        status_text(&session),
        format!(
            "{STATUS_TEXT} · incomplete scan · 2 processes listed · 40 unreadable · \
             40 rows retained from an earlier scan"
        )
    );
}

/// The regression. A record that stays denied forever must not make every later
/// exit unobservable: the published collection stays the size of the host, not
/// the size of its history.
#[test]
fn churn_under_a_permanently_denied_record_does_not_grow_the_published_rows() {
    let (session, mut view) = started(&[4101, 4102, 5000]);
    let denied_row = published(&session)[1];
    assert_eq!(denied_row.1, 4102);

    for tick in 1..=250u32 {
        // Every tick: the same stable process, the same permanently denied
        // record, and one short-lived process that replaces the previous one.
        let ephemeral = 5000 + tick;
        let mut scanned = scan(&[4101, ephemeral]);
        scanned.vanished = 1;
        scanned.completeness = denied(&[4102]);
        view.apply(&session, STATUS_TEXT, &scanned).unwrap();

        let rows = published(&session);
        assert_eq!(
            rows.len(),
            3,
            "tick {tick}: one stable row, one retained row, one live row"
        );
        assert_eq!(view.row_count(), rows.len());
        assert_eq!(
            pids(&rows).into_iter().collect::<BTreeSet<u64>>(),
            BTreeSet::from([4101, 4102, u64::from(ephemeral)]),
            "tick {tick}: every process that ended has left the collection"
        );
        assert!(
            rows.contains(&denied_row),
            "tick {tick}: the denied record keeps its row and its identity"
        );
    }
    assert_eq!(
        status_text(&session),
        format!(
            "{STATUS_TEXT} · incomplete scan · 2 processes listed · 1 unreadable · \
             1 row retained from an earlier scan"
        )
    );
}

/// One of the three global identity components a scan stamps every record with.
/// They are facts about the source, not about a record: one hostname per UTS
/// namespace, one boot ID per running kernel, one namespace per procfs mount.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Component {
    Host,
    Boot,
    PidNamespace,
}

impl Component {
    /// The scope a collector records when it cannot read this component.
    fn scope(self) -> IssueScope {
        match self {
            Self::Host => IssueScope::HostIdentity,
            Self::Boot => IssueScope::BootIdentity,
            Self::PidNamespace => IssueScope::PidNamespace,
        }
    }

    /// The identity file this component is read from, for the issue detail.
    fn file(self) -> &'static str {
        match self {
            Self::Host => "sys/kernel/hostname",
            Self::Boot => "sys/kernel/random/boot_id",
            Self::PidNamespace => "1/ns/pid",
        }
    }

    /// This scan could not read the component at all.
    fn lost(self, key: &mut ProcessKey) {
        let lost = MissingReason::Unavailable;
        match self {
            Self::Host => key.host = Observed::Missing(lost),
            Self::Boot => key.boot = Observed::Missing(lost),
            Self::PidNamespace => key.pid_namespace = Observed::Missing(lost),
        }
    }

    /// This scan read the component and it is a different value: a different
    /// host, a different boot, or a different PID namespace.
    fn changed(self, key: &mut ProcessKey) {
        match self {
            Self::Host => key.host = Observed::Known(HostId("another-host".into())),
            Self::Boot => key.boot = Observed::Known(BootId("fixture-boot-0002".into())),
            Self::PidNamespace => {
                key.pid_namespace = Observed::Known(PidNamespaceId(4_026_532_999))
            }
        }
    }
}

const COMPONENTS: [Component; 3] = [Component::Host, Component::Boot, Component::PidNamespace];

/// A scan that read every record it listed and could not read one global
/// identity file. Nothing is skipped and no PID is uncertain — that is the whole
/// point: an identity file degrades every record equally and hides none — so
/// this snapshot's retention is empty and every row it fails to confirm would be
/// deleted.
fn identity_lost(pids: &[u32], component: Component) -> ProcessSnapshot {
    let mut tick = scan(pids);
    for record in &mut tick.records {
        component.lost(&mut record.key);
    }
    tick.completeness = Completeness::from_scan(
        SkippedRecords::none(),
        vec![EnumerationIssue {
            scope: component.scope(),
            reason: MissingReason::Unavailable,
            detail: format!(
                "{}: No such file or directory (os error 2)",
                component.file()
            ),
        }],
    );
    tick
}

/// A scan that read every record and every identity file, with one component
/// answering a different value than the session has seen before.
fn identity_changed(pids: &[u32], component: Component) -> ProcessSnapshot {
    let mut tick = scan(pids);
    for record in &mut tick.records {
        component.changed(&mut record.key);
    }
    tick
}

/// Operations committed since `revision`, decoded from the wire form a client
/// actually receives, flattened in commit order.
fn operations_since(session: &Session, revision: u64) -> Vec<Operation> {
    session
        .collect_replayed_transactions(revision)
        .expect("the journal holds this run")
        .into_iter()
        .flat_map(|transaction| transaction.operations)
        .map(|op| Operation::try_from(op).expect("a committed operation decodes"))
        .collect()
}

/// The round-5 regression. A global identity file that cannot be read for one
/// tick and is readable again on the next must not re-key a single record: no
/// row is deleted, none is inserted, and every row keeps the item ID the client
/// already holds, on the degraded tick and on the recovering one alike.
#[test]
fn a_global_identity_lost_for_one_tick_and_recovered_churns_no_row() {
    for component in COMPONENTS {
        let (session, mut view) = started(&[4101, 4102, 4103]);
        let before = published(&session);
        let start = session.current_revision();

        let degraded = view
            .apply(
                &session,
                STATUS_TEXT,
                &identity_lost(&[4101, 4102, 4103], component),
            )
            .unwrap();
        assert_eq!(
            (
                degraded.inserted,
                degraded.deleted,
                degraded.updated,
                degraded.retained
            ),
            (0, 0, 0, 0),
            "{component:?}: a lost identity file re-keys nothing, so there is nothing \
             to delete, insert or even retain"
        );
        assert_eq!(
            published(&session),
            before,
            "{component:?}: every row keeps its identity, its place and its cells"
        );
        // The status is still built from counts and fixed wording, and it does
        // say what happened: the degradation is published even though the keys
        // held steady.
        assert_eq!(
            status_text(&session),
            format!(
                "{STATUS_TEXT} · incomplete scan · 3 processes listed · host identity incomplete"
            ),
            "{component:?}"
        );

        let recovered = view
            .apply(&session, STATUS_TEXT, &scan(&[4101, 4102, 4103]))
            .unwrap();
        assert_eq!(
            (
                recovered.inserted,
                recovered.deleted,
                recovered.updated,
                recovered.retained
            ),
            (0, 0, 0, 0),
            "{component:?}: recovery is not a change either"
        );
        assert_eq!(
            published(&session),
            before,
            "{component:?}: recovery moves no row"
        );
        assert_eq!(status_text(&session), STATUS_TEXT, "{component:?}");

        // On the wire, across both ticks: the status text and nothing else.
        let ops = operations_since(&session, start);
        assert!(
            ops.iter()
                .all(|op| matches!(op, Operation::SetProperty { .. })),
            "{component:?}: {ops:?}"
        );
    }
}

/// The honest exception. A component that comes back as a *different* value is
/// not a degradation and is never smoothed over: a different host, boot or PID
/// namespace is a different source, its records are different process instances,
/// and they visibly take new identities. The new value is then what a later
/// degraded scan anchors to.
#[test]
fn a_different_global_identity_is_a_different_source_and_takes_new_identities() {
    for component in COMPONENTS {
        let (session, mut view) = started(&[4101, 4102, 4103]);
        let before = published(&session);
        // Degrade first, so the session has a remembered value that *could* have
        // been reused, and prove it is not reused when the answer disagrees.
        view.apply(
            &session,
            STATUS_TEXT,
            &identity_lost(&[4101, 4102, 4103], component),
        )
        .unwrap();

        let outcome = view
            .apply(
                &session,
                STATUS_TEXT,
                &identity_changed(&[4101, 4102, 4103], component),
            )
            .unwrap();
        assert_eq!(
            (outcome.inserted, outcome.deleted, outcome.retained),
            (3, 3, 0),
            "{component:?}: the rows of the source that left are deleted and the new \
             source's rows are inserted"
        );
        let after = published(&session);
        assert_eq!(
            pids(&after),
            pids(&before),
            "{component:?}: the same PID numbers, which is exactly why they must not \
             be taken for the same instances"
        );
        assert!(
            after
                .iter()
                .all(|(id, _)| before.iter().all(|(held, _)| held != id)),
            "{component:?}: not one row is silently equal to the one it replaced"
        );
        assert_eq!(status_text(&session), STATUS_TEXT, "{component:?}");

        // And the session now anchors to the source that is actually there: a
        // later scan that loses the same file keeps the *new* identities.
        let outcome = view
            .apply(
                &session,
                STATUS_TEXT,
                &identity_lost(&[4101, 4102, 4103], component),
            )
            .unwrap();
        assert_eq!(
            (outcome.inserted, outcome.deleted, outcome.retained),
            (0, 0, 0),
            "{component:?}: the remembered identity is the one last observed"
        );
        assert_eq!(published(&session), after, "{component:?}");
    }
}
