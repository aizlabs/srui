//! PX-004 review follow-up: an incomplete scan withholds deletion only for the
//! absences it could not account for (§§8, 12.1, 22; PR #64 review finding).
//!
//! The reported defect: one persistently denied `/proc/<pid>/stat` marks every
//! scan incomplete, so a single global "not authoritative" flag suppressed every
//! deletion forever. On a shared host that is the steady state, and ordinary
//! churn then accumulates a permanently stale row per exit until the model's
//! cached-item limit refuses the next refresh.
use srui_process_explorer::refresh::ProcessView;
use srui_process_explorer::source::*;
use srui_process_explorer::{start_from_source, MODEL, STATUS};
use srui_sdk::{ItemId, Value, TEXT};
use srui_sessiond::Session;
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
/// not be read: one `IssueScope::Process` issue each, and a skipped count that
/// matches, so every absence is accounted for.
fn denied(pids: &[u32]) -> Completeness {
    Completeness::from_scan(
        pids.len(),
        pids.iter()
            .map(|pid| EnumerationIssue {
                scope: IssueScope::Process(*pid),
                reason: MissingReason::Denied,
                detail: format!("{pid}/stat: Permission denied (os error 13)"),
            })
            .collect(),
    )
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

/// The negative case. Where the uncertainty really is global — the root could
/// not be listed, an entry of it could not be examined, the scan stopped at its
/// own record bound, or the bounded issue list dropped some of what it skipped —
/// nothing may be deleted, because the uncertain identities cannot be named.
#[test]
fn a_scan_that_cannot_enumerate_its_uncertainty_still_retains_every_row() {
    let unlistable_root = || {
        let mut tick = scan(&[]);
        tick.completeness = Completeness::from_scan(
            0,
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
        tick.completeness = Completeness::from_scan(
            1,
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
            0,
            vec![EnumerationIssue {
                scope: IssueScope::Limit,
                reason: MissingReason::Unavailable,
                detail: "record limit 1 reached".into(),
            }],
        );
        tick
    };
    let truncated_issues = || {
        let mut tick = scan(&[4101]);
        // PX-003 bounds retained explanations while `skipped` counts every
        // record: more records were skipped than this list can name, so the
        // uncertain set is not enumerable even though every issue here names a
        // PID — and none of them is 4102 or 4103.
        let issues: Vec<EnumerationIssue> = (0..MAX_RECORDED_ISSUES)
            .map(|index| EnumerationIssue {
                scope: IssueScope::Process(9000 + index as u32),
                reason: MissingReason::Denied,
                detail: "denied".into(),
            })
            .collect();
        tick.completeness = Completeness::from_scan(MAX_RECORDED_ISSUES + 1, issues);
        tick
    };

    for (name, build) in [
        ("root", &unlistable_root as &dyn Fn() -> ProcessSnapshot),
        ("entry", &unreadable_entry),
        ("capped", &capped),
        ("truncated", &truncated_issues),
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

    // The truncation check is what makes the last case global: the same scan
    // with a skipped count its issue list fully accounts for names exactly which
    // records are uncertain, and 4102 and 4103 are not among them.
    let mut accounted = truncated_issues();
    let Completeness::Incomplete { issues, .. } = accounted.completeness.clone() else {
        panic!("the fixture is degraded")
    };
    accounted.completeness = Completeness::from_scan(issues.len(), issues);
    assert_eq!(
        accounted.retention().uncertain_pids().map(BTreeSet::len),
        Some(MAX_RECORDED_ISSUES),
        "an enumerable uncertain set is bounded by the recorded issues"
    );
    let (session, mut view) = started(&[4101, 4102, 4103]);
    let outcome = view.apply(&session, STATUS_TEXT, &accounted).unwrap();
    assert_eq!((outcome.deleted, outcome.retained), (2, 0));
    assert_eq!(pids(&published(&session)), vec![4101]);
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
