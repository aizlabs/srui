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
use srui_process_explorer::procfs::MAX_RECORDS;
use srui_process_explorer::refresh::ProcessView;
use srui_process_explorer::source::*;
use srui_process_explorer::{start_from_source, MODEL, STATUS};
use srui_sdk::{ItemId, Value, TEXT};
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
        capped: 0,
        completeness: Completeness::Complete,
    }
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
    let capped = || {
        let mut tick = scan(&[4101]);
        // Entries beyond the collector's own bound were never read: their PIDs
        // are unknown, so nothing absent can be attributed.
        tick.capped = 2;
        tick.completeness = Completeness::from_scan(
            SkippedRecords::none(),
            vec![EnumerationIssue {
                scope: IssueScope::Limit,
                reason: MissingReason::Unavailable,
                detail: "record limit 1 reached".into(),
            }],
        );
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
        ("capped", &capped),
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
