//! Concurrent `Session::transaction()` commit ordering, journal integrity,
//! failure isolation, and broadcast visibility (§12.1, §18, §20.2).

use std::sync::atomic::{AtomicUsize, Ordering};
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

    broadcasts.sort_by_key(|tx| tx.new_revision);
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
