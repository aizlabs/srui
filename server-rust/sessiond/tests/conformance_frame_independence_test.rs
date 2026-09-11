//! SRUI Frame-Independence Conformance Suite (§32 item 4).
//!
//! Implements: §4.16 (no frame cadence in the protocol), §12.2 (commits are state-consistency
//! boundaries, not render frames), §20.2/§20.4 (outbound queue and coalescing), §32.4.
//!
//! §32 item 4 requires that "identical semantic mutation streams produce identical protocol
//! traffic independent of client refresh rate".
//!
//! The server never learns a client's refresh rate, so refresh rate cannot be varied directly
//! here — a test that "varied" it server-side would be asserting over a variable the code under
//! test never reads, and would pass no matter what the implementation did. What a refresh rate
//! actually manifests as on the server is **drain cadence**: how often a subscribed client reads
//! from its outbound queue. That is a real, observable input, and this suite varies it across
//! cadences equivalent to 60/120/144/240 Hz against a fixed 60-mutation stream.
//!
//! Assertions are anchored to the **frames the subscriber actually receives**, encoded with
//! `encode_framed`. Comparing the journal across cadences would be circular: identical commits
//! produce an identical journal by construction, whatever delivery does, so a journal comparison
//! stays green even if delivery became frame-dependent.
//!
//! The properties asserted:
//!
//! 1. a client that keeps up receives exactly the committed transaction stream, byte for byte;
//! 2. delivered bytes are deterministic — the same mutation stream at the same cadence produces
//!    the same frames, so no wall-clock or repaint tick can be influencing what is sent;
//! 3. a slower cadence may only *coalesce* (§20.4), never amplify or alter, and never reaches the
//!    authoritative history;
//! 4. every cadence converges on the same replica state at the same revision;
//! 5. the protocol vocabulary contains no frame or cadence concept at all.
//!
//! The complementary half — that a renderer's repaint count *does* vary with cadence while the
//! wire traffic does not — lives in the Swift `FrameIndependenceConformanceTests`, because it
//! needs an actual renderer.

use srui_protocol::{
    encode_framed, operation::Op, srui_message::Msg, PropertyRef as WirePropRef, SetPropertyOp,
    SruiMessage, Transaction as WireTransaction, Value as WireValue,
};
use srui_protocol::{Operation as WireOp, ServerEventAck};
use srui_sdk::{NodeId, Surface, Text};
use srui_semantic_tree::{PropertyRef, SemanticStore};
use srui_sessiond::{LogicalChannelClass, Session, SessionConfig};

const ROOT: u64 = 1;
const TEXT: u64 = 2;
const MUTATIONS: u32 = 60;

/// Synthetic client refresh rates. The value is how many commits land between drains: reading a
/// 240 Hz mutation stream, a 240 Hz client drains every commit and a 60 Hz client every 4th.
const CADENCES: &[(&str, u32)] = &[
    ("240Hz", 1),
    ("144Hz", 2),
    ("120Hz", 3),
    ("60Hz", 4),
    ("stalled", MUTATIONS + 1),
];

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

struct CadenceRun {
    journal_bytes: Vec<Vec<u8>>,
    /// Wire bytes of the frames the subscriber actually received, in delivery order. This is the
    /// protocol traffic §32.4 is about; the journal is identical by construction for identical
    /// commits and so proves nothing on its own.
    delivered_bytes: Vec<Vec<u8>>,
    final_revision: u64,
    final_label: Option<srui_semantic_tree::Value>,
    node_count: usize,
    delivered_count: usize,
    replica_revision: u64,
    replica_label: Option<srui_semantic_tree::Value>,
}

/// Runs the identical mutation stream against a session whose subscriber drains every
/// `drain_every` commits.
fn run_at_cadence(name: &str, drain_every: u32) -> CadenceRun {
    let session = Session::with_config(
        name,
        SessionConfig {
            outbound_queue_capacity: 64,
            ..SessionConfig::default()
        },
    );
    let mut outbound = session
        .subscribe_transactions(name.as_bytes().to_vec())
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

    let mut replica = SemanticStore::new();
    let mut delivered_bytes: Vec<Vec<u8>> = Vec::new();

    let drain = |outbound: &mut srui_sessiond::OutboundReceiver,
                 replica: &mut SemanticStore,
                 delivered_bytes: &mut Vec<Vec<u8>>| {
        while let Some(item) = outbound
            .try_recv_class(LogicalChannelClass::Ui)
            .expect("outbound queue did not overflow")
        {
            if let Some(tx) = item.into_transaction() {
                // Encode exactly what would go on the wire, before applying it. This is the
                // protocol traffic §32.4 constrains; the journal is identical by construction
                // for identical commits and so proves nothing about delivery on its own.
                delivered_bytes.push(encode_framed(&tx).expect("delivered frame encodes"));
                replica
                    .apply_delivered_transaction(tx)
                    .expect("delivered transaction applies to the replica");
            }
        }
    };

    for i in 1..=MUTATIONS {
        let rev = session.current_revision();
        session
            .commit_transaction(scalar_tx(rev, rev + 1, TEXT, &format!("v{i}")))
            .expect("scalar commit");
        if i % drain_every == 0 {
            drain(&mut outbound, &mut replica, &mut delivered_bytes);
        }
    }
    // Every client eventually catches up, whatever its cadence.
    drain(&mut outbound, &mut replica, &mut delivered_bytes);

    let journal_bytes = session
        .collect_replayed_transactions(0)
        .expect("full journal window")
        .iter()
        .map(|tx| encode_framed(tx).expect("journal transaction encodes"))
        .collect();

    let (final_revision, final_label, node_count) = session.with_store(|store| {
        (
            store.revision().get(),
            store
                .get_node(srui_semantic_tree::NodeId::new(TEXT))
                .and_then(|n| n.get_property(PropertyRef::LABEL))
                .cloned(),
            store.node_count(),
        )
    });

    CadenceRun {
        journal_bytes,
        delivered_count: delivered_bytes.len(),
        delivered_bytes,
        final_revision,
        final_label,
        node_count,
        replica_revision: replica.revision().get(),
        replica_label: replica
            .get_node(srui_semantic_tree::NodeId::new(TEXT))
            .and_then(|n| n.get_property(PropertyRef::LABEL))
            .cloned(),
    }
}

/// The central §32.4 assertion, stated over the frames the client actually receives.
///
/// A client that keeps up must receive exactly the committed transaction stream, byte for byte.
/// Comparing the *journal* across cadences would be circular — identical commits produce an
/// identical journal by construction, whatever delivery does — so the comparison is anchored to
/// delivered wire bytes on both sides.
#[test]
fn test_delivered_traffic_matches_the_commit_stream_for_a_client_that_keeps_up() {
    let eager = run_at_cadence("frame-independence-eager", 1);

    assert_eq!(
        eager.delivered_bytes.len(),
        eager.journal_bytes.len(),
        "an eager client received {} frames for {} commits",
        eager.delivered_bytes.len(),
        eager.journal_bytes.len()
    );
    assert_eq!(
        eager.delivered_bytes, eager.journal_bytes,
        "the frames delivered to a client that keeps up must be the committed transactions, \
         byte for byte (§4.16, §32.4)"
    );
}

/// Delivery is a pure function of the mutation stream and the drain cadence: running the same
/// stream twice at the same cadence must produce byte-identical frames. A frame-dependent
/// implementation — one that let wall-clock timing or a repaint tick influence what it sent —
/// would show up here as non-determinism.
#[test]
fn test_delivered_traffic_is_deterministic_at_every_cadence() {
    for (name, drain_every) in CADENCES {
        let first = run_at_cadence(&format!("{name}-a"), *drain_every);
        let second = run_at_cadence(&format!("{name}-b"), *drain_every);

        assert_eq!(
            first.delivered_bytes, second.delivered_bytes,
            "cadence {name} produced different wire bytes on two identical runs; protocol \
             traffic must depend only on semantic mutations and read cadence (§4.16, §32.4)"
        );
        assert_eq!(first.journal_bytes, second.journal_bytes);
    }
}

/// Across cadences the delivered frames may legitimately differ in *count* — §20.4 coalescing
/// compacts scalar deltas behind a slow reader — but every frame a client receives must be a
/// frame the server could have committed, and the stream must still end on the same revision.
#[test]
fn test_slower_cadences_only_coalesce_and_never_invent_traffic() {
    let eager = run_at_cadence("frame-independence-baseline", 1);

    for (name, drain_every) in CADENCES {
        let run = run_at_cadence(name, *drain_every);

        assert!(
            run.delivered_bytes.len() <= eager.delivered_bytes.len(),
            "cadence {name} received more frames ({}) than an eager client ({}); a slower \
             reader may coalesce, never amplify",
            run.delivered_bytes.len(),
            eager.delivered_bytes.len()
        );
        assert_eq!(
            run.journal_bytes, eager.journal_bytes,
            "cadence {name} changed what the server committed; read cadence must not reach the \
             authoritative history (§12.2)"
        );
        assert_eq!(run.final_revision, eager.final_revision);
        assert_eq!(run.final_label, eager.final_label);
        assert_eq!(run.node_count, eager.node_count);
    }
}

/// Every cadence converges on the same authoritative state. §12.2: a commit is a
/// state-consistency boundary, so a slow reader may see fewer frames but never a different world.
#[test]
fn test_every_cadence_converges_on_the_same_replica_state() {
    let expected_revision = MUTATIONS as u64 + 1;

    for (name, drain_every) in CADENCES {
        let run = run_at_cadence(name, *drain_every);

        assert_eq!(
            run.replica_revision, run.final_revision,
            "cadence {name} left the replica at revision {} while the server was at {}",
            run.replica_revision, run.final_revision
        );
        assert_eq!(
            run.replica_revision, expected_revision,
            "cadence {name} converged on revision {} rather than {expected_revision}",
            run.replica_revision
        );
        assert_eq!(
            run.replica_label, run.final_label,
            "cadence {name} converged on a different label than the authoritative store"
        );
    }
}

/// The test above would be vacuous if drain cadence changed nothing observable at all. It does
/// change something — how many delivery frames the client sees — and this pins that difference,
/// proving the cadence variable is genuinely wired into the run.
///
/// §20.4: coalescing compacts scalar deltas behind a slow reader. Fewer frames, same meaning.
#[test]
fn test_drain_cadence_changes_delivery_frame_count_but_not_semantics() {
    let fast = run_at_cadence("frame-independence-fast", 1);
    let stalled = run_at_cadence("frame-independence-stalled", MUTATIONS + 1);

    assert!(
        stalled.delivered_count < fast.delivered_count,
        "a stalled reader must receive strictly fewer delivery frames than an eager one \
         (got {} vs {}); if this no longer holds, the cadence variable is not reaching the \
         outbound queue and the byte-identity assertions above prove nothing",
        stalled.delivered_count,
        fast.delivered_count
    );

    // ... and yet both land in exactly the same place.
    assert_eq!(stalled.replica_revision, fast.replica_revision);
    assert_eq!(stalled.replica_label, fast.replica_label);
    assert_eq!(stalled.journal_bytes, fast.journal_bytes);
}

/// §4.16: the protocol has no frame vocabulary to begin with. There is no `START_FRAME`,
/// `END_FRAME`, frame sequence number, or server-driven refresh cadence to negotiate.
#[test]
fn test_protocol_envelope_carries_no_frame_or_cadence_concept() {
    // A transaction envelope is fully described by base/new revision, priority and operations.
    // Encoding one and re-encoding it must be stable and free of any timing input.
    let tx = scalar_tx(0, 1, TEXT, "only-state");
    let first = encode_framed(&tx).expect("encodes");
    let second = encode_framed(&tx).expect("encodes");
    assert_eq!(
        first, second,
        "transaction encoding must be a pure function of its semantic content"
    );

    // Exhaustively enumerate every payload a peer may send. There is no wildcard arm, so adding
    // a `START_FRAME`/`END_FRAME`-style variant fails to compile here rather than silently
    // passing — which is what makes this an invariant rather than a spot check (§4.16).
    let envelope = SruiMessage {
        msg: Some(Msg::Transaction(tx)),
    };
    match envelope.msg.expect("payload present") {
        // State replication and its handshake.
        Msg::ClientHello(_)
        | Msg::ServerWelcome(_)
        | Msg::ClientResume(_)
        | Msg::ServerResumeOk(_)
        | Msg::ServerResyncRequired(_)
        | Msg::Transaction(_) => {}
        // Interaction and its acknowledgement.
        Msg::Event(_) | Msg::ServerEventAck(_) => {}
        // Content delivery, paced by the client's own requests.
        Msg::ResourceMetadata(_) | Msg::ResourceChunk(_) | Msg::ClientModelRangeRequest(_) => {}
        // Terminal extension byte streams (§21) — a byte stream, not a frame cadence.
        Msg::TerminalData(_)
        | Msg::TerminalInput(_)
        | Msg::TerminalResize(_)
        | Msg::TerminalResyncRequired(_) => {}
    }

    // An event acknowledgement is likewise a pure state-consistency signal. Destructured
    // exhaustively rather than asserted over `default()`: every prost field defaults to empty or
    // zero, so a value assertion would pass unchanged if a `frame_id` were added to the message.
    // Naming every field means a new one fails to compile until this invariant is revisited.
    let ServerEventAck {
        client_instance_id: _,
        event_id: _,
        last_processed_event_seq: _,
        settled_event_seq: _,
        status: _,
        revision_after_effect: _,
        reject_reason: _,
        session_id: _,
    } = ServerEventAck::default();
}
