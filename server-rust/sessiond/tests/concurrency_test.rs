//! Concurrent `Session::transaction()` commit ordering, journal integrity,
//! failure isolation, and broadcast visibility (§12.1, §18, §20.2).

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::thread;

use srui_protocol::{operation, Transaction};
use srui_sdk::{NodeId, Surface};
use srui_semantic_tree::StoreError;
use srui_sessiond::{Session, SessionError};

const CONCURRENT_WORKERS: usize = 32;

fn first_created_node_id(tx: &Transaction) -> Option<u64> {
    tx.operations.iter().find_map(|op| {
        if let Some(operation::Op::CreateNode(create)) = op.op.as_ref() {
            create.node.as_ref().map(|n| n.node_id)
        } else {
            None
        }
    })
}

fn commit_surface(session: &Session, node_id: u64, label: &str) -> Result<(), SessionError> {
    session
        .transaction(|ui| {
            Surface::builder(node_id).label(label).create(ui)?;
            Ok(())
        })
        .map(|_| ())
}

#[test]
fn test_concurrent_transactions_produce_contiguous_revisions() {
    let session = Arc::new(Session::new("concurrent-contiguous"));
    let barrier = Arc::new(Barrier::new(CONCURRENT_WORKERS));

    let handles: Vec<_> = (0..CONCURRENT_WORKERS)
        .map(|worker| {
            let session = Arc::clone(&session);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                let node_id = u64::try_from(worker + 1).expect("worker id fits in u64");
                let label = format!("worker-{worker}");
                commit_surface(&session, node_id, &label).expect("transaction must succeed");
                worker
            })
        })
        .collect();

    for handle in handles {
        handle.join().expect("worker thread must not panic");
    }

    assert_eq!(
        session.current_revision(),
        u64::try_from(CONCURRENT_WORKERS).expect("worker count fits in u64"),
        "each successful transaction advances revision by exactly one"
    );

    let journal = session
        .collect_replayed_transactions(0)
        .expect("journal replay available");
    assert_eq!(journal.len(), CONCURRENT_WORKERS);

    for (idx, tx) in journal.iter().enumerate() {
        let expected_base = u64::try_from(idx).expect("index fits in u64");
        assert_eq!(tx.base_revision, expected_base);
        assert_eq!(tx.new_revision, expected_base + 1);
    }

    for worker in 0..CONCURRENT_WORKERS {
        let node_id = NodeId::new(u64::try_from(worker + 1).expect("worker id fits in u64"));
        assert!(
            session.contains_node(node_id),
            "committed node {node_id:?} must exist in store"
        );
    }
}

#[test]
fn test_journal_order_matches_commit_order() {
    let session = Arc::new(Session::new("concurrent-journal-order"));
    let barrier = Arc::new(Barrier::new(CONCURRENT_WORKERS));
    let next_node_id = Arc::new(AtomicUsize::new(1));

    let handles: Vec<_> = (0..CONCURRENT_WORKERS)
        .map(|worker| {
            let session = Arc::clone(&session);
            let barrier = Arc::clone(&barrier);
            let next_node_id = Arc::clone(&next_node_id);
            thread::spawn(move || {
                barrier.wait();
                let node_id = next_node_id.fetch_add(1, Ordering::SeqCst) as u64 + 100;
                let label = format!("journal-worker-{worker}");
                commit_surface(&session, node_id, &label).expect("transaction must succeed");
            })
        })
        .collect();

    for handle in handles {
        handle.join().expect("worker thread must not panic");
    }

    let journal = session
        .collect_replayed_transactions(0)
        .expect("journal replay available");
    assert_eq!(journal.len(), CONCURRENT_WORKERS);

    let mut seen_node_ids = Vec::with_capacity(CONCURRENT_WORKERS);
    for (idx, tx) in journal.iter().enumerate() {
        let expected_base = u64::try_from(idx).expect("index fits in u64");
        assert_eq!(tx.base_revision, expected_base);
        assert_eq!(tx.new_revision, expected_base + 1);

        let node_id = first_created_node_id(tx).expect("journal entry records create op");
        assert!(
            seen_node_ids.iter().all(|seen| *seen != node_id),
            "each committed transaction must appear exactly once in the journal"
        );
        seen_node_ids.push(node_id);
        assert!(
            session.contains_node(NodeId::new(node_id)),
            "journal entry for node {node_id} must reflect committed store state"
        );
    }
}

#[test]
fn test_failed_and_panicking_transactions_do_not_clobber_committed_state() {
    let session = Arc::new(Session::new("concurrent-failure-isolation"));

    commit_surface(&session, 1, "baseline").expect("baseline transaction");
    let baseline_revision = session.current_revision();
    assert_eq!(baseline_revision, 1);

    enum WorkerKind {
        Success(u64),
        StoreError,
        Panic,
    }

    let workers = [
        WorkerKind::Success(10),
        WorkerKind::StoreError,
        WorkerKind::Panic,
        WorkerKind::Success(11),
        WorkerKind::StoreError,
        WorkerKind::Success(12),
        WorkerKind::Panic,
    ];

    let barrier = Arc::new(Barrier::new(workers.len()));
    let mut handles = Vec::new();

    for kind in workers {
        let session = Arc::clone(&session);
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            barrier.wait();
            match kind {
                WorkerKind::Success(node_id) => {
                    let label = format!("success-{node_id}");
                    commit_surface(&session, node_id, &label)
                }
                WorkerKind::StoreError => session
                    .transaction(|ui| -> Result<(), StoreError> {
                        Surface::builder(999).label("will-not-commit").create(ui)?;
                        Err(StoreError::OperationError(
                            "simulated concurrent failure".into(),
                        ))
                    })
                    .map(|_| ()),
                WorkerKind::Panic => session
                    .transaction(|_ui| -> Result<(), StoreError> {
                        panic!("simulated concurrent panic");
                    })
                    .map(|_| ()),
            }
        }));
    }

    let mut success_count = 0usize;
    let mut store_error_count = 0usize;
    let mut panic_count = 0usize;

    for handle in handles {
        match handle.join().expect("worker thread must not panic") {
            Ok(()) => success_count += 1,
            Err(SessionError::Store(_)) => store_error_count += 1,
            Err(SessionError::Panicked(_)) => panic_count += 1,
            Err(other) => panic!("unexpected session error: {other:?}"),
        }
    }

    assert_eq!(success_count, 3);
    assert_eq!(store_error_count, 2);
    assert_eq!(panic_count, 2);

    assert_eq!(session.current_revision(), baseline_revision + 3);
    assert!(session.contains_node(NodeId::new(1)));
    session.with_store(|store| {
        let baseline = Surface::from_store(store, NodeId::new(1)).expect("baseline surface");
        assert_eq!(baseline.label(store), Some("baseline"));
    });
    assert!(session.contains_node(NodeId::new(10)));
    assert!(session.contains_node(NodeId::new(11)));
    assert!(session.contains_node(NodeId::new(12)));
    assert!(!session.contains_node(NodeId::new(999)));

    let journal = session
        .collect_replayed_transactions(0)
        .expect("journal replay available");
    assert_eq!(journal.len(), 4, "baseline plus three successful commits");
    assert_eq!(journal[0].new_revision, 1);
    assert_eq!(journal[3].new_revision, 4);
}

#[tokio::test]
async fn test_broadcast_reflects_committed_state() {
    let session = Arc::new(Session::new("concurrent-broadcast"));
    let mut broadcast_rx = session
        .subscribe_transactions(vec![1, 2, 3])
        .expect("broadcast open");
    let barrier = Arc::new(Barrier::new(CONCURRENT_WORKERS));

    let commit_task = tokio::task::spawn_blocking({
        let session = Arc::clone(&session);
        let barrier = Arc::clone(&barrier);
        move || {
            let handles: Vec<_> = (0..CONCURRENT_WORKERS)
                .map(|worker| {
                    let session = Arc::clone(&session);
                    let barrier = Arc::clone(&barrier);
                    thread::spawn(move || {
                        barrier.wait();
                        let node_id = u64::try_from(worker + 1).expect("worker id fits in u64");
                        let label = format!("broadcast-worker-{worker}");
                        commit_surface(&session, node_id, &label)
                            .expect("transaction must succeed");
                    })
                })
                .collect();

            for handle in handles {
                handle.join().expect("worker thread must not panic");
            }
        }
    });

    let mut broadcasts = Vec::with_capacity(CONCURRENT_WORKERS);
    for _ in 0..CONCURRENT_WORKERS {
        let tx = broadcast_rx
            .recv()
            .await
            .expect("committed transaction must be broadcast");
        broadcasts.push(tx);
    }

    commit_task.await.expect("commit task must complete");

    // Deliberately NOT sorted: sorting by `new_revision` here would discard the only evidence
    // that publication order matches commit order, which is the property §12.1 requires and the
    // only one a replica can consume.
    assert_eq!(broadcasts.len(), CONCURRENT_WORKERS);
    assert_eq!(
        session.current_revision(),
        u64::try_from(CONCURRENT_WORKERS).expect("worker count fits in u64")
    );

    for (idx, tx) in broadcasts.iter().enumerate() {
        let expected_base = u64::try_from(idx).expect("index fits in u64");
        assert_eq!(tx.base_revision, expected_base);
        assert_eq!(tx.new_revision, expected_base + 1);

        let node_id = first_created_node_id(tx).expect("broadcast carries create op");
        assert!(
            session.contains_node(NodeId::new(node_id)),
            "store must contain node {node_id} before broadcast consumer observes revision {}",
            tx.new_revision
        );
    }
}

/// §12.1: the server publishes monotonically increasing committed revisions, so the delivered
/// stream must be ordered exactly like the journal.
///
/// Publishing outside the mutex that orders commits admits the schedule
/// `commit(N) | commit(N+1) | publish(N+1) | publish(N)`. A replica holding revision `N-1` that
/// receives `N+1` rejects it as `stale_base_revision {expected: N-1, actual: N}` with
/// `actual > expected` — not the benign replay-overlap case — so the client reports
/// `replicaDiverged` and tears the session down even though nothing actually failed.
#[test]
fn test_broadcast_delivery_order_matches_commit_order_under_concurrency() {
    const ROUNDS: usize = 64;

    for round in 0..ROUNDS {
        let session = Arc::new(Session::new(format!("delivery-order-{round}")));
        let mut broadcast_rx = session
            .subscribe_transactions(vec![1, 2, 3])
            .expect("broadcast open");
        let barrier = Arc::new(Barrier::new(CONCURRENT_WORKERS));

        let handles: Vec<_> = (0..CONCURRENT_WORKERS)
            .map(|worker| {
                let session = Arc::clone(&session);
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    barrier.wait();
                    let node_id = u64::try_from(worker + 1).expect("worker id fits in u64");
                    commit_surface(&session, node_id, "ordered").expect("transaction");
                })
            })
            .collect();

        for handle in handles {
            handle.join().expect("worker thread must not panic");
        }

        // Replays the delivered stream through a replica applying the same acceptance rule as the
        // Swift client: a transaction applies only when its base equals the committed revision.
        //
        // The delivery count is bounded, not fixed: an undrained subscriber lets the hub coalesce
        // scalar sets into its unsent tail (§20.4), so one delivery may stand for a run of commits.
        // A coalesced span is still contiguous, so the acceptance rule below is unchanged, and
        // merging can only ever reduce the count — never invent a delivery.
        let mut replica_revision = 0u64;
        let mut delivered: Vec<u64> = Vec::with_capacity(CONCURRENT_WORKERS);
        while let Ok(Some(tx)) = broadcast_rx.try_recv() {
            assert_eq!(
                tx.base_revision, replica_revision,
                "round {round}: replica at revision {replica_revision} cannot apply a transaction \
                 based on {}; delivered revisions so far: {delivered:?}",
                tx.base_revision
            );
            replica_revision = tx.new_revision;
            delivered.push(tx.new_revision);
        }

        assert!(
            !delivered.is_empty(),
            "round {round}: committed transactions must reach an attached subscriber"
        );
        assert!(
            delivered.len() <= CONCURRENT_WORKERS,
            "round {round}: coalescing may merge deliveries, never multiply them; got {}",
            delivered.len()
        );
        assert_eq!(replica_revision, session.current_revision());
        assert_eq!(
            session.current_revision(),
            session.journal_latest_revision()
        );
    }
}

/// §12.1: a committed revision must never become externally visible before it has been published.
///
/// Publication inside the commit critical section means any observer that reads revision `N`
/// through `current_revision()` — which takes the same mutex — is guaranteed that `publish(N)`
/// already happened. Publishing after the mutex is released opens a window in which the store
/// reports `N` while the subscriber has seen only `N - 1`, and that window is exactly what lets
/// two concurrent committers deliver their transactions out of order.
///
/// Measured through the delivered stream rather than queue depth: the outbound hub coalesces
/// scalar sets into an unsent tail (§20.4), so depth is no longer a count of published commits,
/// but the tail's `new_revision` still tracks the highest revision published.
#[test]
fn test_committed_revision_is_never_visible_before_it_is_published() {
    const COMMITS: u64 = 512;

    // Capacity above the commit count: an overflow would mark the subscriber stale and stop the
    // drain, which reads as a publication lag rather than the ordering property under test (§20.2).
    let session = Arc::new(Session::with_outbound_queue_capacity(
        "publish-before-visible",
        (COMMITS + 1) as usize,
    ));
    let mut rx = session
        .subscribe_transactions(vec![9, 9, 9])
        .expect("broadcast open");
    let stop = Arc::new(AtomicBool::new(false));

    let watcher = thread::spawn({
        let session = Arc::clone(&session);
        let stop = Arc::clone(&stop);
        move || {
            let mut published = 0u64;
            while !stop.load(Ordering::Relaxed) {
                // Drained first, so `published` can only lag the store, never lead it: a sample
                // where the store is behind the stream would be the reader's own staleness, not a
                // violation. The failing direction is the store running ahead of publication.
                while let Ok(Some(tx)) = rx.try_recv() {
                    published = tx.new_revision;
                }
                let revision = session.current_revision();
                while let Ok(Some(tx)) = rx.try_recv() {
                    published = tx.new_revision;
                }
                if published < revision {
                    return Some((revision, published));
                }
            }
            None
        }
    });

    for id in 1..=COMMITS {
        commit_surface(&session, id, "publish-order").expect("transaction must succeed");
    }
    stop.store(true, Ordering::Relaxed);

    let leak = watcher.join().expect("watcher thread must not panic");
    assert!(
        leak.is_none(),
        "store reported revision {} while the delivered stream had only reached {}: a subscriber \
         attaching here would miss a committed revision, and two concurrent committers racing in \
         this window deliver out of order (§12.1)",
        leak.unwrap().0,
        leak.unwrap().1
    );
}

/// §12.1: a successful commit advances store and journal together; a refused one advances
/// neither. The journal permit is what makes the append after the irreversible store commit
/// infallible instead of merely unlikely.
#[test]
fn test_store_and_journal_agree_on_the_committed_revision() {
    let session = Session::new("store-journal-agreement");
    assert_eq!(
        session.current_revision(),
        session.journal_latest_revision()
    );

    commit_surface(&session, 1, "first").expect("first commit");
    assert_eq!(session.current_revision(), 1);
    assert_eq!(session.journal_latest_revision(), 1);

    let refused = session.transaction(|ui| -> Result<(), StoreError> {
        Surface::builder(2).label("never").create(ui)?;
        Err(StoreError::OperationError("refused".into()))
    });
    assert!(matches!(refused, Err(SessionError::Store(_))));
    assert_eq!(session.current_revision(), 1);
    assert_eq!(session.journal_latest_revision(), 1);

    let panicked = session.transaction(|_ui| -> Result<(), StoreError> {
        panic!("simulated panic");
    });
    assert!(matches!(panicked, Err(SessionError::Panicked(_))));
    assert_eq!(session.current_revision(), 1);
    assert_eq!(session.journal_latest_revision(), 1);
}
