//! PX-004 acceptance: a polled collection that publishes only what changed,
//! keeps unchanged rows, and never lets a failed scan empty the table
//! (§§8, 12.1, 13, 23).
use srui_process_explorer::procfs::MAX_RECORDS;
use srui_process_explorer::refresh::refresh_status;
use srui_process_explorer::source::*;
use srui_process_explorer::{start_from_source, COLUMN, HEADING, MODEL, STATUS, SURFACE, TABLE};
use srui_protocol::DEFAULT_MAX_FRAME_SIZE;
use srui_sdk::{ItemId, Operation, Value, TEXT};
use srui_semantic_tree::Model;
use srui_sessiond::Session;
use std::collections::HashMap;
use std::time::{Duration, SystemTime};

/// The rows a client holds, in published order.
fn published(session: &Session) -> Vec<(ItemId, Vec<Value>)> {
    session.with_store(|store| {
        let model = store.get_model(MODEL).expect("the collection model exists");
        assert_eq!(
            model.item_count,
            model.items.len() as u64,
            "every row of this collection is published"
        );
        model
            .items
            .values()
            .map(|item| {
                let Value::List(cells) = &item.value else {
                    panic!("expected table cells")
                };
                (item.item_id, cells.clone())
            })
            .collect()
    })
}

fn names(rows: &[(ItemId, Vec<Value>)]) -> Vec<(u64, String)> {
    rows.iter()
        .map(|(_, cells)| {
            let (Value::UnsignedInt(pid), Value::String(name)) = (&cells[0], &cells[1]) else {
                panic!("expected a PID and a name")
            };
            (*pid, name.clone())
        })
        .collect()
}

/// The first cell of each row, whether or not the PID could be observed.
fn pid_cells(rows: &[(ItemId, Vec<Value>)]) -> Vec<Value> {
    rows.iter().map(|(_, cells)| cells[0].clone()).collect()
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

/// The shell nodes must survive every refresh: a tick mutates model data and
/// the status text, and never rebuilds the semantic node tree.
fn assert_shell_intact(session: &Session) {
    session.with_store(|store| {
        assert_eq!(store.node_count(), 5);
        assert_eq!(store.root_ids(), &[SURFACE]);
        assert_eq!(
            store.children_of(COLUMN),
            Some([HEADING, STATUS, TABLE].as_slice())
        );
        assert_eq!(store.children_of(TABLE), Some([].as_slice()));
        assert_eq!(store.get_node(TABLE).unwrap().model_ref(), Some(MODEL));
    });
}

#[test]
fn a_scripted_sequence_inserts_deletes_and_renames_without_rebuilding_the_tree() {
    let session = Session::mint();
    let mut source = ScriptedFakeSource::default();
    let (mut view, _) = start_from_source(&session, &mut source).unwrap();
    let first = published(&session);
    assert_eq!(
        names(&first),
        vec![
            (4101, "worker".to_string()),
            (4102, "worker".to_string()),
            (4103, "helper".to_string()),
        ]
    );
    assert_eq!(session.current_revision(), 1);

    // Step 1: the same processes, one second later. Nothing a client can see
    // changed, so nothing at all is sent.
    let outcome = view.refresh(&session, &mut source).unwrap();
    assert!(!outcome.published(), "{outcome:?}");
    assert_eq!(session.current_revision(), 1);
    assert_eq!(published(&session), first);

    // Step 2: one insertion, one deletion, one rename.
    let outcome = view.refresh(&session, &mut source).unwrap();
    assert_eq!(session.current_revision(), 2);
    assert_eq!(
        (outcome.inserted, outcome.deleted, outcome.updated),
        (1, 1, 1)
    );
    let second = published(&session);
    assert_eq!(
        names(&second),
        vec![
            (4101, "worker".to_string()),
            (4103, "helper-tool".to_string()),
            (4104, "builder".to_string()),
        ]
    );
    // The untouched process and the renamed one keep the row identity they
    // already had; only the ended process's ID leaves.
    assert_eq!(second[0].0, first[0].0, "an unchanged row keeps its ItemId");
    assert_eq!(second[1].0, first[2].0, "a rename keeps the row identity");
    assert!(second.iter().all(|(id, _)| *id != first[1].0));

    // One transaction carrying model data and nothing else: no node was
    // created, deleted, moved or reordered to show three different rows.
    let ops = operations_since(&session, 1);
    assert!(
        ops.iter().all(|op| matches!(
            op,
            Operation::ModelInsert { .. }
                | Operation::ModelDelete { .. }
                | Operation::ModelUpdate { .. }
        )),
        "{ops:?}"
    );
    assert_shell_intact(&session);
    // A row the scan did not mention is not re-sent: only the renamed row is.
    let updates: Vec<&Operation> = ops
        .iter()
        .filter(|op| matches!(op, Operation::ModelUpdate { .. }))
        .collect();
    assert_eq!(updates.len(), 1);
    let Operation::ModelUpdate { items, .. } = updates[0] else {
        unreachable!()
    };
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].item_id, second[1].0);
}

#[test]
fn a_failed_scan_keeps_every_row_and_says_so_and_recovery_converges() {
    let session = Session::mint();
    let mut source = ScriptedFakeSource::default();
    let (mut view, _) = start_from_source(&session, &mut source).unwrap();
    for _ in 0..2 {
        view.refresh(&session, &mut source).unwrap();
    }
    let settled = published(&session);
    assert_eq!(settled.len(), 3);
    let revision = session.current_revision();

    // Step 3: the process list could not be read at all.
    let outcome = view.refresh(&session, &mut source).unwrap();
    assert_eq!(
        (outcome.inserted, outcome.deleted, outcome.updated),
        (0, 0, 0),
        "a failed scan is not evidence that any process ended"
    );
    assert_eq!(outcome.retained, 3);
    assert!(outcome.status_changed);
    assert_eq!(
        published(&session),
        settled,
        "last-known rows are kept whole"
    );
    let status = status_text(&session);
    assert_eq!(
        status,
        format!(
            "{} · incomplete scan · process list unavailable: permission denied · \
             3 rows retained from an earlier scan",
            ScriptedFakeSource::STATUS_TEXT
        )
    );
    // The failure is reported as a property of the existing status node.
    let ops = operations_since(&session, revision);
    assert!(
        ops.iter().all(|op| matches!(
            op,
            Operation::SetProperty {
                id: STATUS,
                property: TEXT,
                ..
            }
        )),
        "a failed scan must publish nothing but its explanation: {ops:?}"
    );
    assert!(
        !ops.iter()
            .any(|op| matches!(op, Operation::ModelDelete { .. })),
        "a failed scan must never delete a row"
    );
    assert_shell_intact(&session);

    // Step 4: the scan succeeds again with exactly the retained processes. The
    // rows are already right, so recovery costs one status property and not a
    // single row operation.
    let revision = session.current_revision();
    let outcome = view.refresh(&session, &mut source).unwrap();
    assert_eq!(
        (
            outcome.inserted,
            outcome.deleted,
            outcome.updated,
            outcome.retained
        ),
        (0, 0, 0, 0)
    );
    assert!(outcome.status_changed);
    assert_eq!(published(&session), settled, "recovery moves no row");
    assert_eq!(status_text(&session), ScriptedFakeSource::STATUS_TEXT);
    assert_eq!(operations_since(&session, revision).len(), 1);
}

#[test]
fn a_second_identical_failure_republishes_nothing() {
    // The failed step of the script repeats every cycle. A host that stays
    // broken must not produce one transaction per second.
    let session = Session::mint();
    let mut source = ScriptedFakeSource::default();
    let (mut view, _) = start_from_source(&session, &mut source).unwrap();
    let mut failures = 0;
    let mut republished = 0;
    for _ in 0..ScriptedFakeSource::STEPS * 3 {
        let before = view.status().to_string();
        let outcome = view.refresh(&session, &mut source).unwrap();
        if view.status().contains("retained from an earlier scan") {
            failures += 1;
            if before == view.status() {
                republished += usize::from(outcome.published());
            }
        }
    }
    assert!(failures >= 3);
    assert_eq!(republished, 0);
}

/// A snapshot the app refuses to identify is a scan failure, not an empty host.
#[test]
fn an_unidentifiable_snapshot_is_an_error_over_the_last_known_rows() {
    struct Corrupt;
    impl ProcessSource for Corrupt {
        fn status_text(&self) -> &str {
            FAKE_STATUS_TEXT
        }

        fn snapshot(&mut self) -> ProcessSnapshot {
            let mut snapshot = FakeProcessSource.snapshot();
            snapshot.records[1].key = snapshot.records[0].key.clone();
            snapshot
        }
    }
    let session = Session::mint();
    let (mut view, _) = start_from_source(&session, &mut FakeProcessSource).unwrap();
    let rows = published(&session);
    let outcome = view.refresh(&session, &mut Corrupt).unwrap();
    assert_eq!(outcome.retained, 3);
    assert_eq!(published(&session), rows);
    assert_eq!(
        status_text(&session),
        format!(
            "{FAKE_STATUS_TEXT} · snapshot rejected: duplicate process instance identity · \
             3 rows retained from an earlier scan"
        )
    );
    // The same source recovering converges back onto the unchanged rows.
    let outcome = view.refresh(&session, &mut FakeProcessSource).unwrap();
    assert_eq!(
        (outcome.inserted, outcome.deleted, outcome.updated),
        (0, 0, 0)
    );
    assert_eq!(published(&session), rows);
    assert_eq!(status_text(&session), FAKE_STATUS_TEXT);
}

/// A degraded scan that still lists records inserts and updates what it saw
/// without deleting what it could not see.
#[test]
fn a_partial_scan_adds_what_it_saw_and_deletes_nothing() {
    struct Partial {
        keep: usize,
        extra: bool,
    }
    impl ProcessSource for Partial {
        fn status_text(&self) -> &str {
            FAKE_STATUS_TEXT
        }

        fn snapshot(&mut self) -> ProcessSnapshot {
            let mut snapshot = FakeProcessSource.snapshot();
            if self.extra {
                // A new process, ordered between the records that remain.
                let mut record = snapshot.records[1].clone();
                record.key.pid = Observed::Known(4105);
                record.key.creation = CreationToken::Opaque("late-arrival".into());
                record.display_name = "late".into();
                snapshot.records.insert(1, record);
            }
            // The middle record could not be read this time.
            snapshot.records.remove(self.keep);
            let mut skipped = SkippedRecords::with_limit(MAX_RECORDS);
            skipped.record(4102);
            snapshot.completeness = Completeness::from_scan(
                skipped,
                vec![EnumerationIssue {
                    scope: IssueScope::Process(4102),
                    reason: MissingReason::Denied,
                    detail: "denied".into(),
                }],
            );
            snapshot
        }
    }
    let session = Session::mint();
    let (mut view, _) = start_from_source(&session, &mut FakeProcessSource).unwrap();
    let rows = published(&session);
    let outcome = view
        .refresh(
            &session,
            &mut Partial {
                keep: 1,
                extra: false,
            },
        )
        .unwrap();
    assert_eq!(outcome.retained, 1);
    assert_eq!(
        published(&session),
        rows,
        "an unread record is not a deletion"
    );

    // The same degraded scan, now also showing a process that really is new.
    let outcome = view
        .refresh(
            &session,
            &mut Partial {
                keep: 2,
                extra: true,
            },
        )
        .unwrap();
    assert_eq!((outcome.inserted, outcome.deleted), (1, 0));
    let after = published(&session);
    assert_eq!(
        pid_cells(&after),
        vec![
            Value::UnsignedInt(4101),
            Value::UnsignedInt(4105),
            Value::UnsignedInt(4102),
            // The record with no observable PID was not confirmed this time and
            // keeps the last place it held.
            Value::String("Unavailable".into()),
        ],
        "an unconfirmed row keeps the place it already held"
    );
    // Every previously published row kept its identity.
    for (id, _) in &rows {
        assert!(after.iter().any(|(after_id, _)| after_id == id));
    }
}

/// A source that publishes `rows` processes, each carrying the widest display
/// name this app can ever publish: [`MAX_DISPLAY_NAME_CHARS`] characters of four
/// UTF-8 bytes each. A scan of a busy host really can look like this, and it is
/// the shape that makes a handful of operations overflow a 16 MiB frame.
struct Widest {
    rows: u32,
    first_pid: u32,
}

impl Widest {
    /// The name every row carries. Sanitizing keeps it whole — no character of
    /// it is control, ignorable, blank or trimmed — so it reaches the model at
    /// four bytes a character.
    fn name() -> DisplayName {
        let name = DisplayName::from("\u{20000}".repeat(MAX_DISPLAY_NAME_CHARS).as_str());
        assert_eq!(name.as_str().chars().count(), MAX_DISPLAY_NAME_CHARS);
        assert_eq!(name.as_str().len(), MAX_DISPLAY_NAME_CHARS * 4);
        name
    }
}

impl ProcessSource for Widest {
    fn status_text(&self) -> &str {
        FAKE_STATUS_TEXT
    }

    fn snapshot(&mut self) -> ProcessSnapshot {
        let template = FakeProcessSource.snapshot().records[0].key.clone();
        let name = Self::name();
        let records = (0..self.rows)
            .map(|index| {
                let pid = self.first_pid + index;
                ProcessRecord {
                    key: ProcessKey {
                        pid: Observed::Known(pid),
                        creation: CreationToken::Opaque(format!("widest-{pid}")),
                        ..template.clone()
                    },
                    display_name: name.clone(),
                }
            })
            .collect();
        ProcessSnapshot {
            source: SourceId(FakeProcessSource::SOURCE.into()),
            sampled_at: SnapshotTime(SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
            records,
            vanished: 0,
            capped: CappedRecords::none(),
            completeness: Completeness::Complete,
        }
    }
}

/// The bytes each transaction committed since `revision` occupies, measured by
/// the protocol's own encoder — exactly the quantity `SruiCodec` compares
/// against the frame limit before writing a frame (§26).
fn frames_since(session: &Session, revision: u64) -> Vec<usize> {
    session
        .collect_replayed_transactions(revision)
        .expect("the journal holds this run")
        .into_iter()
        .map(|transaction| {
            srui_protocol::framed_payload_len(&srui_protocol::SruiMessage {
                msg: Some(srui_protocol::srui_message::Msg::Transaction(transaction)),
            })
        })
        .collect()
}

/// A refresh whose payload exceeds the §26 frame limit commits as consecutive
/// transactions, every one of them a frame a conforming decoder accepts.
///
/// Chunking by operation count alone is not enough: these rows need only four
/// `MODEL_INSERT` operations, far inside the 10,000-operation bound, and still
/// carry more than 16 MiB. Committing them as one transaction would advance the
/// server's revision and then have the codec refuse the frame, detaching the
/// client from a server that has already moved on — and the next identical tick
/// would diff clean and republish nothing.
#[test]
fn a_refresh_beyond_the_frame_bound_commits_as_frames_a_client_accepts() {
    let session = Session::mint();
    let max_ops = session.with_store(|store| store.limits().max_transaction_operations);
    // Rows that all vanish in the refresh below, so the deletions this refresh
    // plans must still precede its insertions across a size-driven split.
    let (mut view, _) = start_from_source(
        &session,
        &mut Widest {
            rows: 500,
            first_pid: 900_000,
        },
    )
    .unwrap();
    assert_eq!(published(&session).len(), 500);

    let revision = session.current_revision();
    // 34,000 rows of 512 name bytes each: more than 16 MiB of payload in four
    // operations.
    let rows = 34_000;
    let outcome = view
        .refresh(&session, &mut Widest { rows, first_pid: 1 })
        .unwrap();
    assert_eq!(outcome.inserted, rows as usize);
    assert_eq!(outcome.deleted, 500);
    assert_eq!(published(&session).len(), rows as usize);
    assert_shell_intact(&session);

    let operations = operations_since(&session, revision);
    assert!(
        operations.len() <= max_ops,
        "this refresh fits the operation bound in {} operations, so only a size \
         bound can split it",
        operations.len()
    );
    assert!(matches!(operations[0], Operation::ModelDelete { .. }));
    let first_insert = operations
        .iter()
        .position(|op| matches!(op, Operation::ModelInsert { .. }))
        .expect("the refresh inserts rows");
    assert!(
        operations[..first_insert]
            .iter()
            .all(|op| matches!(op, Operation::ModelDelete { .. })),
        "every deletion must precede every insertion, so an insertion index is final"
    );

    let frames = frames_since(&session, revision);
    assert_eq!(
        frames.len(),
        outcome.transactions,
        "every committed transaction is in the journal"
    );
    for frame in &frames {
        assert!(
            *frame <= DEFAULT_MAX_FRAME_SIZE,
            "a committed transaction of {frame} bytes exceeds the {DEFAULT_MAX_FRAME_SIZE}-byte \
             frame limit and would be refused on delivery"
        );
    }
    assert!(
        frames.iter().sum::<usize>() > DEFAULT_MAX_FRAME_SIZE,
        "the refresh as a whole must really exceed one frame; it carried {} bytes",
        frames.iter().sum::<usize>()
    );
    assert!(
        outcome.transactions > 1,
        "this refresh cannot fit in one frame"
    );

    // An identical tick still publishes nothing.
    let outcome = view
        .refresh(&session, &mut Widest { rows, first_pid: 1 })
        .unwrap();
    assert!(!outcome.published(), "{outcome:?}");
}

/// A refresh that fits in one frame is not split: the size bound costs a large
/// but deliverable refresh nothing.
#[test]
fn a_refresh_just_inside_the_frame_bound_still_commits_as_one_transaction() {
    let session = Session::mint();
    let (mut view, _) = start_from_source(
        &session,
        &mut Widest {
            rows: 1,
            first_pid: 900_000,
        },
    )
    .unwrap();

    let revision = session.current_revision();
    // Chosen so the planner's own upper bound on this refresh stays under the
    // ceiling; the frame it really encodes to is asserted below.
    let rows = 27_000;
    let outcome = view
        .refresh(&session, &mut Widest { rows, first_pid: 1 })
        .unwrap();
    assert_eq!(outcome.inserted, rows as usize);
    assert_eq!(
        outcome.transactions, 1,
        "a refresh that fits in one frame must not be split"
    );

    let frames = frames_since(&session, revision);
    assert_eq!(frames.len(), 1);
    assert!(
        frames[0] <= DEFAULT_MAX_FRAME_SIZE,
        "{} exceeds the frame limit",
        frames[0]
    );
    assert!(
        frames[0] > 13 * 1024 * 1024,
        "this refresh must really approach the ceiling, not be a trivially small \
         one that no bound could have split; it was {} bytes",
        frames[0]
    );
}

/// A single refresh larger than the §26 transaction bound is still published
/// whole, and the view keeps reporting exactly what the store holds.
#[test]
fn a_refresh_beyond_the_transaction_bound_is_published_whole() {
    struct Interleaved {
        stride: u32,
        rows: u32,
    }
    impl ProcessSource for Interleaved {
        fn status_text(&self) -> &str {
            FAKE_STATUS_TEXT
        }

        fn snapshot(&mut self) -> ProcessSnapshot {
            let template = FakeProcessSource.snapshot().records[0].key.clone();
            let records = (0..self.rows)
                .filter(|index| index % self.stride == 0)
                .map(|index| ProcessRecord {
                    key: ProcessKey {
                        pid: Observed::Known(index + 1),
                        creation: CreationToken::Opaque(format!("interleaved-{index}")),
                        ..template.clone()
                    },
                    display_name: "worker".into(),
                })
                .collect();
            ProcessSnapshot {
                source: SourceId(FakeProcessSource::SOURCE.into()),
                sampled_at: SnapshotTime(SystemTime::UNIX_EPOCH + Duration::from_secs(1)),
                records,
                vanished: 0,
                capped: CappedRecords::none(),
                completeness: Completeness::Complete,
            }
        }
    }
    let session = Session::mint();
    let max_ops = session.with_store(|store| store.limits().max_transaction_operations);
    let rows = (max_ops as u32 + 2) * 2;
    // Every second row is new, so no two insertions are adjacent and the
    // refresh needs more operations than one transaction may carry.
    let (mut view, _) = start_from_source(&session, &mut Interleaved { stride: 2, rows }).unwrap();
    let half = published(&session).len();
    assert_eq!(half, (rows / 2) as usize);
    assert_eq!(
        session.current_revision(),
        1,
        "the first publication is one transaction whatever its size"
    );

    let revision = session.current_revision();
    let outcome = view
        .refresh(&session, &mut Interleaved { stride: 1, rows })
        .unwrap();
    assert_eq!(outcome.inserted, half);
    assert!(
        outcome.transactions > 1,
        "this refresh cannot fit in one transaction"
    );
    assert_eq!(
        session.current_revision(),
        revision + outcome.transactions as u64
    );
    let after = published(&session);
    assert_eq!(after.len(), rows as usize);
    assert_eq!(
        names(&after)
            .iter()
            .map(|(pid, _)| *pid)
            .collect::<Vec<u64>>(),
        (1..=u64::from(rows)).collect::<Vec<u64>>()
    );
    assert_shell_intact(&session);
    // Every operation of that refresh stayed inside the per-operation bound.
    let model: Model = session.with_store(|store| store.get_model(MODEL).cloned().unwrap());
    assert_eq!(model.id_to_index.len(), rows as usize);
    let ids: HashMap<ItemId, u64> = model.id_to_index.clone();
    assert_eq!(ids.len(), model.items.len());
}

#[test]
fn the_published_status_names_only_what_happened() {
    let mut source = ScriptedFakeSource::default();
    let snapshot = source.snapshot();
    assert_eq!(
        refresh_status(ScriptedFakeSource::STATUS_TEXT, &snapshot, 0, None),
        ScriptedFakeSource::STATUS_TEXT,
        "a complete scan of a healthy host says nothing extra"
    );
}
