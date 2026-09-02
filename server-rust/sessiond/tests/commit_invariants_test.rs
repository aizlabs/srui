//! Store/journal agreement invariants for authoritative commits (§12.1, §18.1, §20.4).
//!
//! These are the properties the delivery-form split exists to protect. They are stated over
//! observable session behaviour rather than over any one code path, so a future refactor that
//! reintroduces a mutate-then-validate window fails here regardless of where it puts the window:
//!
//! 1. a refused commit changes nothing — store contents, store revision, journal contents;
//! 2. after every successful commit the store revision equals the journal's latest revision;
//! 3. every journal entry is single-step and contiguous with its predecessor;
//! 4. a coalesced delivery stream lands a replica in the same state as the raw commit stream;
//! 5. a structural transaction is never merged into a multi-revision delivery span;
//! 6. replay from every retained journal revision reconstructs the authoritative store.

use srui_protocol::{
    operation::Op, CreateNodeOp, NodeRecord, Operation as WireOp, PropertyRef as WirePropRef,
    SetPropertyOp, Transaction as WireTransaction, TypeRef as WireTypeRef, Value as WireValue,
};
use srui_sdk::{NodeId, Surface, Text, LABEL};
use srui_semantic_tree::{
    DeliveredTransaction, PropertyRef, SemanticStore, Transaction as DomainTxn, Value,
};
use srui_sessiond::{Session, SessionConfig};

const ROOT: u64 = 1;
const TEXT: u64 = 2;

/// State of a session that every refused commit must leave untouched.
#[derive(Debug, PartialEq)]
struct SessionFingerprint {
    revision: u64,
    node_count: usize,
    journal: Vec<WireTransaction>,
}

fn fingerprint(session: &Session) -> SessionFingerprint {
    SessionFingerprint {
        revision: session.current_revision(),
        node_count: session.node_count(),
        journal: session
            .collect_replayed_transactions(0)
            .expect("journal window covers revision 0"),
    }
}

/// Asserts the store and the journal agree, and that the journal is a contiguous single-step run
/// from revision 0 (§12.1, §18.1).
fn assert_store_and_journal_agree(session: &Session, context: &str) {
    let entries = session
        .collect_replayed_transactions(0)
        .expect("journal window covers revision 0");

    let mut expected_base = 0;
    for (idx, tx) in entries.iter().enumerate() {
        assert_eq!(
            tx.base_revision, expected_base,
            "{context}: journal entry #{idx} is not contiguous with its predecessor"
        );
        assert_eq!(
            tx.new_revision,
            tx.base_revision + 1,
            "{context}: journal entry #{idx} is not a single-step commit"
        );
        expected_base = tx.new_revision;
    }

    assert_eq!(
        session.current_revision(),
        expected_base,
        "{context}: store revision and journal head disagree"
    );
}

fn scalar_tx(base: u64, new_rev: u64, node_id: u64, value: &str) -> WireTransaction {
    WireTransaction {
        base_revision: base,
        new_revision: new_rev,
        priority: 0,
        operations: vec![WireOp {
            op: Some(Op::SetProperty(SetPropertyOp {
                node_id,
                property: Some(WirePropRef {
                    namespace_id: 0,
                    local_id: PropertyRef::LABEL.local_id,
                }),
                value: Some(WireValue {
                    value: Some(srui_protocol::value::Value::StringValue(value.to_string())),
                }),
            })),
        }],
    }
}

fn create_node_tx(base: u64, new_rev: u64, node_id: u64) -> WireTransaction {
    WireTransaction {
        base_revision: base,
        new_revision: new_rev,
        priority: 0,
        operations: vec![WireOp {
            op: Some(Op::CreateNode(CreateNodeOp {
                node: Some(NodeRecord {
                    node_id,
                    r#type: Some(WireTypeRef {
                        namespace_id: 0,
                        local_id: srui_semantic_tree::TypeRef::SURFACE.local_id,
                    }),
                    parent_id: 0,
                    child_index: 0,
                    properties: vec![],
                }),
            })),
        }],
    }
}

fn populated_session(name: &str) -> Session {
    let session = Session::new(name);
    session
        .transaction(|ui| {
            Surface::builder(NodeId::new(ROOT)).create(ui)?;
            Text::builder(NodeId::new(TEXT))
                .parent(NodeId::new(ROOT))
                .label("v0")
                .create(ui)?;
            Ok(())
        })
        .expect("initial tree");
    session
}

/// Every way a commit can fail must leave the session exactly as it was: the store cannot roll back
/// a committed revision, so a partial commit would strand the journal behind it permanently.
#[test]
fn test_refused_commit_leaves_store_and_journal_untouched() {
    let session = populated_session("refused-commit");
    let before = fingerprint(&session);

    // 1. Stale base revision
    let stale = scalar_tx(0, 1, TEXT, "stale");
    assert!(session.commit_transaction(stale).is_err());
    assert_eq!(fingerprint(&session), before, "stale base revision");

    // 2. A coalesced delivery span, which is not an authoritative commit at all (§12.1, §20.4)
    let span = scalar_tx(before.revision, before.revision + 4, TEXT, "span");
    assert!(session.commit_transaction(span).is_err());
    assert_eq!(fingerprint(&session), before, "multi-revision span");

    // 3. An operation that fails mid-transaction
    let missing_node = scalar_tx(before.revision, before.revision + 1, 9_999, "ghost");
    assert!(session.commit_transaction(missing_node).is_err());
    assert_eq!(fingerprint(&session), before, "failing operation");

    // 4. §26 operation limit, enforced as a pre-check before anything is staged
    let mut oversized = scalar_tx(before.revision, before.revision + 1, TEXT, "bulk");
    let op = oversized.operations[0].clone();
    oversized.operations = std::iter::repeat_n(op, 100_001).collect();
    assert!(session.commit_transaction(oversized).is_err());
    assert_eq!(fingerprint(&session), before, "§26 operation limit");

    assert_store_and_journal_agree(&session, "after four refused commits");

    // The session is still usable: a wedged session would fail here with NonContiguousRevision.
    session
        .transaction(|ui| {
            ui.set(NodeId::new(TEXT), LABEL, "after")?;
            Ok(())
        })
        .expect("session remains committable after refused commits");
    assert_store_and_journal_agree(&session, "after a successful commit");
}

/// Store and journal advance together across a long mixed run, and the journal stays a contiguous
/// single-step log (§12.1, §18.1).
#[test]
fn test_store_and_journal_agree_across_mixed_commit_sequence() {
    let session = populated_session("agreement");

    // Deterministic pseudo-random mix: a seeded LCG keeps the sequence reproducible without a
    // property-testing dependency, and drives both accepted and refused commits.
    let mut seed: u64 = 0x5EED_1234_ABCD_0001;
    let mut next = move || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (seed >> 33) as u32
    };

    for i in 0..200u32 {
        let rev = session.current_revision();
        match next() % 4 {
            0 => {
                // Structural commit through the SDK
                let id = 1000 + u64::from(i);
                session
                    .transaction(|ui| {
                        Surface::builder(NodeId::new(id)).create(ui)?;
                        Ok(())
                    })
                    .expect("structural commit");
            }
            1 => {
                // Scalar commit through the wire path
                let tx = scalar_tx(rev, rev + 1, TEXT, &format!("v{i}"));
                session.commit_transaction(tx).expect("scalar commit");
            }
            2 => {
                // Refused: a delivery span never advances authoritative state
                let tx = scalar_tx(rev, rev + 3, TEXT, &format!("span{i}"));
                assert!(session.commit_transaction(tx).is_err());
            }
            _ => {
                // Refused: stale base
                let tx = scalar_tx(rev.saturating_sub(1), rev, TEXT, &format!("stale{i}"));
                assert!(session.commit_transaction(tx).is_err());
            }
        }

        assert_store_and_journal_agree(&session, &format!("iteration {i}"));
    }
}

/// Replay from any retained revision reconstructs exactly the authoritative store (§18, §18.1).
#[test]
fn test_replay_from_every_retained_revision_reconstructs_the_store() {
    let session = populated_session("replay");
    for i in 0..25u32 {
        let rev = session.current_revision();
        if i % 5 == 0 {
            session
                .transaction(|ui| {
                    Surface::builder(NodeId::new(2000 + u64::from(i))).create(ui)?;
                    Ok(())
                })
                .expect("structural commit");
        } else {
            session
                .commit_transaction(scalar_tx(rev, rev + 1, TEXT, &format!("v{i}")))
                .expect("scalar commit");
        }
    }

    let final_revision = session.current_revision();
    let full: Vec<WireTransaction> = session
        .collect_replayed_transactions(0)
        .expect("full journal window");

    for from in 0..=final_revision {
        let replayed = session
            .collect_replayed_transactions(from)
            .unwrap_or_else(|e| panic!("replay from revision {from} unavailable: {e}"));

        // Rebuild the prefix [0, from) from the full log, then replay the suffix like a client.
        let mut replica = SemanticStore::new();
        for tx in full.iter().take(from as usize) {
            replica
                .apply_delivered_transaction(tx.clone())
                .expect("prefix applies");
        }
        for tx in replayed {
            replica
                .apply_delivered_transaction(tx)
                .expect("replayed suffix applies");
        }

        assert_eq!(
            replica.revision().get(),
            final_revision,
            "replay from revision {from} ended on the wrong revision"
        );
        session.with_store(|store| {
            assert_eq!(
                replica.node_count(),
                store.node_count(),
                "replay from revision {from} produced a different node count"
            );
            assert_eq!(
                replica
                    .get_node(srui_semantic_tree::NodeId::new(TEXT))
                    .and_then(|n| n.get_property(PropertyRef::LABEL)),
                store
                    .get_node(srui_semantic_tree::NodeId::new(TEXT))
                    .and_then(|n| n.get_property(PropertyRef::LABEL)),
                "replay from revision {from} produced a different label"
            );
        });
    }
}

/// A coalesced delivery stream must leave a replica exactly where the raw commit stream would, and
/// must never merge a structural transaction into a multi-revision span (§12.1, §20.4).
#[test]
fn test_coalesced_delivery_stream_matches_raw_commit_stream() {
    let session = Session::with_config(
        "coalescing-equivalence",
        SessionConfig {
            // Deep enough to hold the whole run, so coalescing — not overflow — is what compacts it.
            outbound_queue_capacity: 64,
            ..SessionConfig::default()
        },
    );
    // Subscribed from revision 0 so the delivery stream is the whole history: no catch-up snapshot
    // is involved, and the two replicas below start from the same empty store. Nothing is drained
    // until the run finishes, so the queue coalesces behind a slow client (§20.2).
    let mut outbound = session
        .subscribe_transactions(b"equivalence".to_vec())
        .expect("subscribe");

    session
        .transaction(|ui| {
            Surface::builder(NodeId::new(ROOT)).create(ui)?;
            Text::builder(NodeId::new(TEXT))
                .parent(NodeId::new(ROOT))
                .label("v0")
                .create(ui)?;
            Ok(())
        })
        .expect("initial tree");

    for i in 1..=40u32 {
        let rev = session.current_revision();
        if i % 10 == 0 {
            session
                .commit_transaction(create_node_tx(rev, rev + 1, 3000 + u64::from(i)))
                .expect("structural commit");
        } else {
            session
                .commit_transaction(scalar_tx(rev, rev + 1, TEXT, &format!("v{i}")))
                .expect("scalar commit");
        }
    }

    let mut delivered = Vec::new();
    while let Some(tx) = outbound.try_recv().expect("queue did not overflow") {
        delivered.push(tx);
    }
    assert!(
        delivered.len() < 40,
        "expected the stream to be compacted by coalescing, got {} frames",
        delivered.len()
    );

    // Every multi-revision frame must be a scalar-only delta: a structural transaction is a
    // barrier, never merged across (§20.4).
    let mut spans = 0;
    for tx in &delivered {
        let domain = DomainTxn::try_from(tx.clone()).expect("frame decodes");
        if domain.new_revision.get() > domain.base_revision.get() + 1 {
            spans += 1;
            assert!(
                matches!(
                    DeliveredTransaction::try_from(domain).expect("frame is a delivery form"),
                    DeliveredTransaction::Delta(_)
                ),
                "a multi-revision frame must be a coalesced scalar delta"
            );
        }
    }
    assert!(spans > 0, "expected at least one coalesced span in the run");

    // A replica fed the coalesced stream lands exactly where the raw journal stream lands.
    let mut coalesced_replica = SemanticStore::new();
    for tx in delivered {
        coalesced_replica
            .apply_delivered_transaction(tx)
            .expect("coalesced frame applies");
    }

    let mut raw_replica = SemanticStore::new();
    for tx in session
        .collect_replayed_transactions(0)
        .expect("full journal window")
    {
        raw_replica
            .apply_delivered_transaction(tx)
            .expect("raw commit applies");
    }

    assert_eq!(
        coalesced_replica.revision(),
        raw_replica.revision(),
        "coalesced delivery ended on a different revision than the raw stream"
    );
    assert_eq!(
        coalesced_replica.node_count(),
        raw_replica.node_count(),
        "coalesced delivery produced a different tree"
    );
    assert_eq!(
        coalesced_replica
            .get_node(srui_semantic_tree::NodeId::new(TEXT))
            .and_then(|n| n.get_property(PropertyRef::LABEL)),
        // Iteration 40 is structural, so v39 is the last label the run wrote.
        Some(&Value::String("v39".to_string())),
        "coalescing must retain the latest value of each property"
    );
    assert_eq!(
        raw_replica.revision(),
        srui_semantic_tree::Revision::new(session.current_revision()),
        "the raw stream must reconstruct the authoritative revision"
    );
}
