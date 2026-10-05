//! Catch-up and resync snapshots larger than one frame (§12.1, §18, §26, §32.8).
//!
//! A store whose collection model encodes past `DEFAULT_MAX_FRAME_SIZE` must still reach a fresh
//! client and a resyncing client: the decision message announces `snapshot_parts`, every envelope
//! frames within the limit, and the replica reassembles and applies them as ONE snapshot that
//! reconstructs exactly the exported store. A snapshot that still cannot be delivered is refused at
//! handshake with `ServerHandshakeRefused` on the wire and a typed error on the server.

use std::sync::Arc;

use futures::{SinkExt, StreamExt};
use tokio::io::{duplex, DuplexStream, ReadHalf, WriteHalf};
use tokio::task::JoinHandle;
use tokio_util::codec::{FramedRead, FramedWrite};
use tokio_util::sync::CancellationToken;

use srui_protocol::{
    framed_payload_len, srui_message, ClientHello, ClientLimits, ClientResume,
    HandshakeRefusalReason, SessionContinuity, SnapshotAssembler, SnapshotFramingError, SruiCodec,
    SruiMessage, Transaction as WireTransaction, DEFAULT_MAX_FRAME_SIZE,
    DEFAULT_MAX_SNAPSHOT_PARTS,
};
use srui_sdk::*;
use srui_semantic_tree::{
    ItemId, ModelId, ModelItem, NodeId, Operation, ResyncSnapshot, Revision, SemanticStore,
    StoreLimits, TypeRef, Value, DEFAULT_MAX_TRANSACTION_OPERATIONS,
};
use srui_sessiond::{handle_connection, ConnectionError, Session, SessionConfig, SessionError};

/// Rows of the PX-004 round-6 shape: 34,000 wide rows exported 18,259,393 bytes against the
/// 16,777,216-byte frame limit. These rows are a little wider so the model alone clears the limit.
const ROWS: u64 = 34_000;
const ROW_BYTES: usize = 560;
const MODEL: ModelId = ModelId::new(10);

type ClientRead = FramedRead<ReadHalf<DuplexStream>, SruiCodec>;
type ClientWrite = FramedWrite<WriteHalf<DuplexStream>, SruiCodec>;

fn row(index: u64) -> String {
    let prefix = format!("row {index:06} ");
    let mut text = prefix.clone();
    while text.len() < ROW_BYTES {
        text.push_str(&prefix);
    }
    text.truncate(ROW_BYTES);
    text
}

/// Seeds a Surface, a List bound to a model of [`ROWS`] wide rows, and returns the revision.
///
/// Every seeding transaction respects §26 on its own; only the resulting store outgrows a frame.
fn seed_oversized_model(session: &Session) -> u64 {
    session
        .transaction(|ui| {
            ui.apply_op(&Operation::create_model(MODEL, TypeRef::LIST, ROWS))?;
            Surface::builder(1).label("Processes").create(ui)?;
            List::builder(2)
                .parent(1)
                .model_ref(MODEL)
                .label("rows")
                .create(ui)?;
            Ok(())
        })
        .expect("seed tree");
    let chunk = 8_500u64;
    let mut start = 0u64;
    while start < ROWS {
        let end = (start + chunk).min(ROWS);
        session
            .transaction(|ui| {
                ui.apply_op(&Operation::model_reset_range(
                    MODEL,
                    start,
                    (start..end).map(|i| {
                        ModelItem::with_value(ItemId::new(1_000_000 + i), Value::String(row(i)))
                    }),
                    None,
                ))
            })
            .expect("seed rows");
        start = end;
    }
    session.current_revision()
}

fn limits(max_snapshot_parts: u32, max_frame_size: u32) -> ClientLimits {
    ClientLimits {
        max_frame_size,
        max_snapshot_parts,
        ..ClientLimits::default()
    }
}

fn hello(limits: Option<ClientLimits>) -> SruiMessage {
    SruiMessage {
        msg: Some(srui_message::Msg::ClientHello(ClientHello {
            core_version: srui_sessiond::CORE_VERSION.to_string(),
            profiles: vec!["org.srui.standard-widgets/1".to_string()],
            limits,
            client_instance_id: vec![7, 7],
            client_metadata: Default::default(),
            known_resource_hashes: vec![],
        })),
    }
}

fn resume(session_id: &str, last_applied: u64, limits: Option<ClientLimits>) -> SruiMessage {
    SruiMessage {
        msg: Some(srui_message::Msg::ClientResume(ClientResume {
            core_version: srui_sessiond::CORE_VERSION.to_string(),
            profiles: vec!["org.srui.standard-widgets/1".to_string()],
            session_id: session_id.to_string(),
            client_instance_id: vec![8, 8],
            last_applied_revision: last_applied,
            last_acked_event_seq: 0,
            terminal_stream_offsets: Default::default(),
            limits,
            known_resource_hashes: vec![],
            pending_text_edits: vec![],
        })),
    }
}

async fn connect(
    session: &Arc<Session>,
    first: SruiMessage,
) -> (
    ClientRead,
    ClientWrite,
    CancellationToken,
    JoinHandle<Result<(), ConnectionError>>,
) {
    let (client_io, server_io) = duplex(1 << 20);
    let shutdown = CancellationToken::new();
    let server = tokio::spawn(handle_connection(
        server_io,
        Arc::clone(session),
        shutdown.clone(),
    ));
    let (read, write) = tokio::io::split(client_io);
    let read = FramedRead::new(read, SruiCodec::new());
    let mut write = FramedWrite::new(write, SruiCodec::new());
    write.send(first).await.expect("send handshake");
    (read, write, shutdown, server)
}

/// Reads one frame, asserting it was within the §26 frame limit on the wire.
async fn next_frame(read: &mut ClientRead) -> SruiMessage {
    let message = read
        .next()
        .await
        .expect("frame before EOF")
        .expect("frame within the codec limit");
    assert!(
        framed_payload_len(&message) <= DEFAULT_MAX_FRAME_SIZE,
        "frame of {} bytes exceeds the {DEFAULT_MAX_FRAME_SIZE}-byte limit",
        framed_payload_len(&message)
    );
    message
}

/// Stages the announced envelopes the way a replica does and returns the applied replica.
///
/// Asserts that nothing is produced before the last envelope and that every envelope is a
/// `0 -> snapshot_revision` transaction within the frame limit.
async fn receive_snapshot(
    read: &mut ClientRead,
    snapshot_revision: u64,
    announced_parts: u32,
) -> SemanticStore {
    let mut assembler = SnapshotAssembler::new(
        snapshot_revision,
        announced_parts,
        DEFAULT_MAX_SNAPSHOT_PARTS,
        DEFAULT_MAX_TRANSACTION_OPERATIONS,
    )
    .expect("announced parts within the advertised bound");
    let mut snapshot = None;
    for part in 0..assembler.expected_parts() {
        let frame = next_frame(read).await;
        let Some(srui_message::Msg::Transaction(tx)) = frame.msg else {
            panic!("snapshot envelope {part} is not a transaction: {frame:?}");
        };
        assert!(
            snapshot.is_none(),
            "snapshot assembled before its last envelope"
        );
        snapshot = assembler.accept(tx).expect("envelope accepted");
    }
    let snapshot = snapshot.expect("snapshot complete after the announced envelopes");
    let snapshot = ResyncSnapshot::try_from(snapshot).expect("assembled snapshot decodes");
    let mut replica =
        SemanticStore::with_limits_and_revision(StoreLimits::default(), Revision::INITIAL);
    replica
        .replace_from_snapshot(&snapshot)
        .expect("assembled snapshot applies");
    replica
}

fn assert_replica_matches(session: &Session, replica: &SemanticStore) {
    session.with_store(|expected| {
        assert_eq!(expected.revision(), replica.revision());
        assert_eq!(expected.root_ids(), replica.root_ids());
        assert_eq!(expected.node_count(), replica.node_count());
        for node_id in [NodeId::new(1), NodeId::new(2)] {
            let want = expected.get_node(node_id).expect("server node");
            let got = replica.get_node(node_id).expect("replica node");
            assert_eq!(want.node_type, got.node_type);
            assert_eq!(want.parent_id, got.parent_id);
            assert_eq!(want.ordered_children, got.ordered_children);
            assert_eq!(want.properties, got.properties);
        }
        assert_eq!(expected.model_count(), replica.model_count());
        let want = expected.get_model(MODEL).expect("server model");
        let got = replica.get_model(MODEL).expect("replica model");
        assert_eq!(want.model_type, got.model_type);
        assert_eq!(want.item_count, got.item_count);
        assert_eq!(want.cached_item_count(), got.cached_item_count());
        for (index, item) in want.iter_cached_items() {
            assert_eq!(got.get_item_by_index(*index), Some(item), "row {index}");
        }
    });
}

fn exported_single_envelope_bytes(session: &Session) -> usize {
    // What the pre-PX-004-G01 single envelope would have framed to: the model rows alone.
    session.with_store(|store| {
        let model = store.get_model(MODEL).expect("model");
        model
            .iter_cached_items()
            .map(|(_, item)| framed_payload_len(&srui_protocol::ModelItem::from(item)))
            .sum()
    })
}

#[tokio::test]
async fn fresh_client_receives_a_snapshot_larger_than_one_frame() {
    let session = Arc::new(Session::new("oversized-fresh"));
    let revision = seed_oversized_model(&session);
    assert!(
        exported_single_envelope_bytes(&session) > DEFAULT_MAX_FRAME_SIZE,
        "fixture must outgrow one frame"
    );

    let (mut read, _write, shutdown, server) =
        connect(&session, hello(Some(limits(DEFAULT_MAX_SNAPSHOT_PARTS, 0)))).await;
    let Some(srui_message::Msg::ServerWelcome(welcome)) = next_frame(&mut read).await.msg else {
        panic!("expected SERVER WELCOME");
    };
    assert_eq!(welcome.initial_revision, revision);
    assert!(
        welcome.snapshot_parts >= 2,
        "a snapshot past one frame must be announced as split, got {}",
        welcome.snapshot_parts
    );

    let replica = receive_snapshot(&mut read, revision, welcome.snapshot_parts).await;
    assert_replica_matches(&session, &replica);

    // The connection is live after the snapshot: the next commit streams on the same revision line.
    session
        .transaction(|ui| Text::builder(3).parent(1).text("after").create(ui))
        .expect("post-snapshot commit");
    let Some(srui_message::Msg::Transaction(live)) = next_frame(&mut read).await.msg else {
        panic!("expected the live transaction");
    };
    assert_eq!(
        (live.base_revision, live.new_revision),
        (revision, revision + 1)
    );

    shutdown.cancel();
    server.await.expect("join").expect("clean shutdown");
}

#[tokio::test]
async fn resync_delivers_a_snapshot_larger_than_one_frame() {
    let session = Arc::new(Session::with_config(
        "oversized-resync",
        SessionConfig {
            journal_capacity: 1,
            ..SessionConfig::default()
        },
    ));
    let revision = seed_oversized_model(&session);
    assert!(exported_single_envelope_bytes(&session) > DEFAULT_MAX_FRAME_SIZE);

    // Revision 1 has left the one-entry journal, so a same-session resume must resync.
    let (mut read, _write, shutdown, server) = connect(
        &session,
        resume(
            "oversized-resync",
            1,
            Some(limits(DEFAULT_MAX_SNAPSHOT_PARTS, 0)),
        ),
    )
    .await;
    let Some(srui_message::Msg::ServerResyncRequired(resync)) = next_frame(&mut read).await.msg
    else {
        panic!("expected SERVER RESYNC_REQUIRED");
    };
    assert_eq!(
        SessionContinuity::try_from(resync.continuity),
        Ok(SessionContinuity::SameSession)
    );
    assert_eq!(resync.snapshot_revision, revision);
    assert!(resync.snapshot_parts >= 2);

    let replica = receive_snapshot(&mut read, revision, resync.snapshot_parts).await;
    assert_replica_matches(&session, &replica);

    shutdown.cancel();
    server.await.expect("join").expect("clean shutdown");
}

/// A snapshot that fits one frame keeps the legacy single-envelope form byte for byte.
#[tokio::test]
async fn a_snapshot_within_one_frame_is_not_announced_as_split() {
    let session = Arc::new(Session::new("small-fresh"));
    session
        .transaction(|ui| Surface::builder(1).label("small").create(ui))
        .expect("seed");
    let (mut read, _write, shutdown, server) =
        connect(&session, hello(Some(limits(DEFAULT_MAX_SNAPSHOT_PARTS, 0)))).await;
    let Some(srui_message::Msg::ServerWelcome(welcome)) = next_frame(&mut read).await.msg else {
        panic!("expected SERVER WELCOME");
    };
    assert_eq!(welcome.snapshot_parts, 0);
    let replica = receive_snapshot(&mut read, 1, welcome.snapshot_parts).await;
    assert_eq!(replica.root_ids(), &[NodeId::new(1)]);
    shutdown.cancel();
    server.await.expect("join").expect("clean shutdown");
}

/// Reads the refusal, then EOF, and returns the server's own error.
async fn expect_refusal(
    mut read: ClientRead,
    server: JoinHandle<Result<(), ConnectionError>>,
) -> SessionError {
    let frame = next_frame(&mut read).await;
    let Some(srui_message::Msg::ServerHandshakeRefused(refusal)) = frame.msg else {
        panic!("expected SERVER HANDSHAKE_REFUSED, got {frame:?}");
    };
    assert_eq!(
        HandshakeRefusalReason::try_from(refusal.reason),
        Ok(HandshakeRefusalReason::SnapshotUndeliverable)
    );
    assert!(
        refusal.detail.contains("snapshot"),
        "refusal must say why: {:?}",
        refusal.detail
    );
    assert!(
        read.next().await.is_none(),
        "the server closes after refusing; no snapshot or subscription follows"
    );
    match server.await.expect("join") {
        Err(ConnectionError::Session(error)) => error,
        other => panic!("expected the handshake to fail with a session error, got {other:?}"),
    }
}

/// Size bound: a legacy client (no `max_snapshot_parts`) cannot stage a split snapshot, so the
/// server refuses loudly instead of writing a frame the codec would reject.
#[tokio::test]
async fn legacy_client_is_refused_a_snapshot_it_cannot_receive() {
    let session = Arc::new(Session::new("oversized-legacy"));
    seed_oversized_model(&session);

    let (read, _write, _shutdown, server) = connect(&session, hello(None)).await;
    match expect_refusal(read, server).await {
        SessionError::SnapshotUndeliverable(SnapshotFramingError::TooManyParts {
            required_parts,
            max_parts: 1,
            snapshot_bytes,
            max_frame_size,
        }) => {
            assert!(required_parts >= 2);
            assert!(snapshot_bytes > DEFAULT_MAX_FRAME_SIZE);
            assert_eq!(max_frame_size, DEFAULT_MAX_FRAME_SIZE);
        }
        other => panic!("expected TooManyParts, got {other:?}"),
    }
    // Deterministic: a reconnect is refused the same way rather than looping silently.
    let (read, _write, _shutdown, server) = connect(&session, hello(None)).await;
    assert!(matches!(
        expect_refusal(read, server).await,
        SessionError::SnapshotUndeliverable(SnapshotFramingError::TooManyParts { .. })
    ));
}

/// Size bound on the resync path, with a client that advertises a smaller frame and few parts.
#[tokio::test]
async fn resync_is_refused_when_the_split_exceeds_the_advertised_parts() {
    let session = Arc::new(Session::with_config(
        "oversized-resync-refused",
        SessionConfig {
            journal_capacity: 1,
            ..SessionConfig::default()
        },
    ));
    seed_oversized_model(&session);

    let (read, _write, _shutdown, server) = connect(
        &session,
        resume("oversized-resync-refused", 1, Some(limits(4, 2 << 20))),
    )
    .await;
    match expect_refusal(read, server).await {
        SessionError::SnapshotUndeliverable(SnapshotFramingError::TooManyParts {
            max_parts: 4,
            max_frame_size,
            required_parts,
            ..
        }) => {
            assert_eq!(max_frame_size, 2 << 20);
            assert!(required_parts > 4);
        }
        other => panic!("expected TooManyParts, got {other:?}"),
    }
}

/// Size bound on one operation: a client frame smaller than an exported range operation cannot
/// carry it in any split, so the handshake is refused naming the operation.
#[tokio::test]
async fn an_operation_larger_than_the_client_frame_is_refused() {
    let session = Arc::new(Session::new("oversized-operation"));
    seed_oversized_model(&session);

    let (read, _write, _shutdown, server) = connect(
        &session,
        hello(Some(limits(DEFAULT_MAX_SNAPSHOT_PARTS, 1 << 20))),
    )
    .await;
    match expect_refusal(read, server).await {
        SessionError::SnapshotUndeliverable(SnapshotFramingError::OperationExceedsFrame {
            framed_bytes,
            max_frame_size,
            ..
        }) => {
            assert_eq!(max_frame_size, 1 << 20);
            assert!(framed_bytes > max_frame_size);
        }
        other => panic!("expected OperationExceedsFrame, got {other:?}"),
    }
}

/// Operation bound: splitting lifts only the byte bound. The envelopes are still one transaction,
/// so a store needing more than `max_transaction_operations` is refused on the wire, too.
#[tokio::test]
async fn operation_bound_is_refused_on_the_wire() {
    let session = Arc::new(Session::new("oversized-operations"));
    session
        .transaction(|ui| Surface::builder(1).label("root").create(ui))
        .expect("root");
    let target = DEFAULT_MAX_TRANSACTION_OPERATIONS as u64 + 1;
    let mut next_id = 2u64;
    while next_id <= target {
        let end = (next_id + 999).min(target);
        session
            .transaction(|ui| {
                for id in next_id..=end {
                    Text::builder(NodeId::new(id))
                        .parent(NodeId::new(1))
                        .text("x")
                        .create(ui)?;
                }
                Ok(())
            })
            .expect("chunked create");
        next_id = end + 1;
    }

    let (read, _write, _shutdown, server) =
        connect(&session, hello(Some(limits(DEFAULT_MAX_SNAPSHOT_PARTS, 0)))).await;
    match expect_refusal(read, server).await {
        SessionError::SnapshotUnrepresentable { limit, actual } => {
            assert_eq!(limit, DEFAULT_MAX_TRANSACTION_OPERATIONS);
            assert!(actual > limit);
        }
        other => panic!("expected SnapshotUnrepresentable, got {other:?}"),
    }
}

/// A split never crosses into the live stream: a non-snapshot transaction cannot be staged.
#[test]
fn replica_rejects_a_live_transaction_inside_a_split_snapshot() {
    let mut assembler = SnapshotAssembler::new(5, 2, DEFAULT_MAX_SNAPSHOT_PARTS, 10).unwrap();
    let live = WireTransaction {
        base_revision: 5,
        new_revision: 6,
        priority: 0,
        operations: vec![],
    };
    assert!(assembler.accept(live).is_err());
}
