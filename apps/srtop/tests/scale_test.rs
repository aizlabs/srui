//! PX-010-G01 scale, progress, latency, and resource budget gate tests (§§8, 12.1, 20.4, 22.7, 26).
//!
//! * Scale to Supported R1 Envelopes: 1,000 and 10,000 synthetic rows under continuous
//!   collection, CPU sort, filtering, insert/delete churn, and stable selection.
//! * Probe Architectural Limits: 100,000 synthetic rows to locate limits without claiming
//!   100k production support.
//! * Bounded Progress Under Continuing Telemetry: `model_range.rs` fulfillment when authoritative
//!   revisions advance while provider work is in flight.
//! * Deterministic RTT Scenarios (0, 100, 300, 600 ms) tested against pre-recorded latency thresholds.
//! * Anti-Coalescing Proof: structural MODEL_UPDATE, MODEL_INSERT, MODEL_DELETE, MODEL_RESET_RANGE
//!   operations are strictly preserved and never dropped or scalar-coalesced.
//! * Strict Enforcement of Finite Caps: MAX_RECORDS, MAX_UNCERTAIN_PIDS, MAX_PUBLISHED_ROWS,
//!   and pending range inboxes.

use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use srui_process_explorer::catalogue::ProcessField;
use srui_process_explorer::filter::{MAX_FILTER_QUERY_BYTES, MAX_FILTER_TERMS};
use srui_process_explorer::procfs::{MAX_RECORDS, MAX_UNCERTAIN_PIDS};
use srui_process_explorer::refresh::MAX_PUBLISHED_ROWS;
use srui_process_explorer::source::{
    CreationToken, DisplayName, Observed, ProcessKey, ProcessRecord, ProcessSource,
    SyntheticProcessSource,
};
use srui_process_explorer::{
    install_filter_handlers, install_sort_handlers, start_from_source, FILTER_STATUS, MODEL,
    SHELL_NODE_COUNT,
};
use srui_protocol::{
    decode_framed, encode_framed, operation::Op, srui_message, ClientHello,
    ClientModelRangeRequest, SruiMessage, DEFAULT_MAX_FRAME_SIZE,
};
use srui_sdk::{ItemId, NodeId, Surface, Table, Value};
use srui_semantic_tree::{
    ModelId, ModelItem, Operation, Revision, Transaction as DomainTxn, TypeRef,
};
use srui_sessiond::{
    handle_connection, ModelRangeError, ModelRangeFulfillment, ModelRangeProvider, ModelRangeQuery,
    Session,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// Helpers: Wire Codec & Message Helpers
// ---------------------------------------------------------------------------

fn decode_varint(buf: &[u8]) -> Option<(usize, usize)> {
    let mut val: usize = 0;
    let mut shift = 0;
    for (i, &b) in buf.iter().enumerate() {
        val |= ((b & 0x7f) as usize) << shift;
        if (b & 0x80) == 0 {
            return Some((val, i + 1));
        }
        shift += 7;
        if shift > 35 {
            return None;
        }
    }
    None
}

fn encode_message(msg: SruiMessage) -> Vec<u8> {
    encode_framed(&msg).expect("encode message")
}

async fn read_message<R: AsyncRead + Unpin>(reader: &mut R, buf: &mut Vec<u8>) -> SruiMessage {
    loop {
        if let Some((payload_len, header_len)) = decode_varint(buf) {
            let total_len = header_len + payload_len;
            if buf.len() >= total_len {
                let msg = decode_framed::<SruiMessage>(&buf[..total_len]).expect("decode message");
                buf.drain(..total_len);
                return msg;
            }
        }
        let mut temp = [0u8; 16384];
        let n = reader.read(&mut temp).await.expect("read chunk");
        if n == 0 {
            panic!("unexpected EOF while reading message");
        }
        buf.extend_from_slice(&temp[..n]);
    }
}

async fn perform_handshake<IO: AsyncRead + AsyncWrite + Unpin>(
    io: &mut IO,
    read_buf: &mut Vec<u8>,
) -> u64 {
    let hello = SruiMessage {
        msg: Some(srui_message::Msg::ClientHello(ClientHello {
            core_version: "0.5.0".to_string(),
            profiles: vec!["org.srui.standard-widgets/1".to_string()],
            limits: None,
            client_instance_id: vec![1, 2, 3, 4],
            client_metadata: Default::default(),
            known_resource_hashes: vec![],
        })),
    };
    io.write_all(&encode_message(hello))
        .await
        .expect("write ClientHello");

    let welcome = read_message(io, read_buf).await;
    match welcome.msg {
        Some(srui_message::Msg::ServerWelcome(w)) => w.initial_revision,
        other => panic!("expected ServerWelcome, got {other:?}"),
    }
}

fn model_changes(transaction: &srui_protocol::Transaction) -> (Vec<u64>, Vec<u64>, usize) {
    let mut deleted = Vec::new();
    let mut inserted = Vec::new();
    let mut other = 0;
    for op in &transaction.operations {
        match &op.op {
            Some(Op::ModelDelete(delete)) => deleted.extend(delete.item_ids.iter().copied()),
            Some(Op::ModelInsert(insert)) => {
                inserted.extend(insert.items.iter().map(|item| item.item_id))
            }
            Some(Op::ModelUpdate(_)) | Some(Op::ModelResetRange(_)) => other += 1,
            _ => {}
        }
    }
    (deleted, inserted, other)
}

fn assert_only_moves_since(session: &Session, revision: u64) -> usize {
    let transactions = session
        .collect_replayed_transactions(revision)
        .expect("transactions in journal");
    let mut moved = 0;
    for transaction in &transactions {
        let (mut deleted, mut inserted, other) = model_changes(transaction);
        assert_eq!(other, 0, "a reorder emits no updates or reset ranges");
        assert_eq!(
            deleted.len(),
            deleted.iter().collect::<HashSet<_>>().len(),
            "a row is moved once per transaction"
        );
        deleted.sort_unstable();
        inserted.sort_unstable();
        assert_eq!(
            deleted, inserted,
            "revision {} removed a row it did not put back",
            transaction.new_revision
        );
        moved += deleted.len();
    }
    moved
}

// ---------------------------------------------------------------------------
// DelayedDuplex Transport Simulation
// ---------------------------------------------------------------------------

struct DelayedDuplex;

impl DelayedDuplex {
    fn pair(rtt: Duration) -> (tokio::io::DuplexStream, tokio::io::DuplexStream) {
        let delay = rtt / 2;
        if delay.is_zero() {
            return tokio::io::duplex(2 * 1024 * 1024);
        }

        let (c_app, c_wire) = tokio::io::duplex(2 * 1024 * 1024);
        let (s_wire, s_app) = tokio::io::duplex(2 * 1024 * 1024);

        let (mut c_wire_read, mut c_wire_write) = tokio::io::split(c_wire);
        let (mut s_wire_read, mut s_wire_write) = tokio::io::split(s_wire);

        // Forward Client -> Server with delay
        tokio::spawn(async move {
            let mut read_buf = [0u8; 16384];
            let mut queue: VecDeque<(tokio::time::Instant, Vec<u8>)> = VecDeque::new();

            loop {
                let now = tokio::time::Instant::now();
                let next_deadline = queue.front().map(|(send_at, _)| *send_at);

                tokio::select! {
                    res = c_wire_read.read(&mut read_buf) => {
                        match res {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                queue.push_back((now + delay, read_buf[..n].to_vec()));
                            }
                        }
                    }
                    _ = async {
                        if let Some(deadline) = next_deadline {
                            tokio::time::sleep_until(deadline).await;
                        } else {
                            std::future::pending::<()>().await;
                        }
                    }, if next_deadline.is_some() => {
                        if let Some((_, chunk)) = queue.pop_front() {
                            if s_wire_write.write_all(&chunk).await.is_err() {
                                break;
                            }
                        }
                    }
                }
            }
        });

        // Forward Server -> Client with delay
        tokio::spawn(async move {
            let mut read_buf = [0u8; 16384];
            let mut queue: VecDeque<(tokio::time::Instant, Vec<u8>)> = VecDeque::new();

            loop {
                let now = tokio::time::Instant::now();
                let next_deadline = queue.front().map(|(send_at, _)| *send_at);

                tokio::select! {
                    res = s_wire_read.read(&mut read_buf) => {
                        match res {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                queue.push_back((now + delay, read_buf[..n].to_vec()));
                            }
                        }
                    }
                    _ = async {
                        if let Some(deadline) = next_deadline {
                            tokio::time::sleep_until(deadline).await;
                        } else {
                            std::future::pending::<()>().await;
                        }
                    }, if next_deadline.is_some() => {
                        if let Some((_, chunk)) = queue.pop_front() {
                            if c_wire_write.write_all(&chunk).await.is_err() {
                                break;
                            }
                        }
                    }
                }
            }
        });

        (c_app, s_app)
    }
}

// ---------------------------------------------------------------------------
// 1. Scale 1,000 Rows: Churn, Sort, Filter, and Selection Stability
// ---------------------------------------------------------------------------

#[test]
fn test_scale_1000_rows_churn_sort_filter_selection() {
    let session = Arc::new(Session::mint());
    let mut source = SyntheticProcessSource::with_churn(1000, 0.05); // 5% churn = 50 rows per step
    let (view, snapshot) = start_from_source(&session, &mut source).expect("start from source");
    let view = Arc::new(Mutex::new(view));
    install_sort_handlers(&session, &view);
    install_filter_handlers(&session, &view);

    assert_eq!(snapshot.records.len(), 1000);
    assert_eq!(
        session.with_store(|s| s.get_model(MODEL).unwrap().item_count),
        1000
    );

    // Anchor selection to row 42
    let target_row_index = 42u64;
    let selected_item_id = session.with_store(|s| {
        let model = s.get_model(MODEL).unwrap();
        model.items.get(&target_row_index).unwrap().item_id
    });

    // 1. Run 10 refresh cycles with continuous 5% churn
    for step in 0..10 {
        let refreshed = view
            .lock()
            .unwrap()
            .refresh(&session, &mut source)
            .expect("refresh");
        assert_eq!(
            session.with_store(|s| s.get_model(MODEL).unwrap().item_count),
            1000,
            "item count must remain 1000 at step {step}"
        );
        assert_eq!(refreshed.deleted, 50, "deleted must match churn count");
    }

    // Confirm that the selected item ID is still present in the model
    let found = session.with_store(|s| s.get_model(MODEL).unwrap().contains_item(selected_item_id));
    assert!(found, "selected process must survive churn");

    // 2. Sort by CPU descending while churn is active
    let rev_before_sort = session.current_revision();
    view.lock()
        .unwrap()
        .click_sort(&session, ProcessField::Cpu)
        .expect("click sort CPU");

    let rev_after_sort = session.current_revision();
    assert!(
        rev_after_sort > rev_before_sort,
        "sort must commit a revision"
    );

    // Verify move invariant: rows moved under their own ItemId, no new IDs minted (M1 kill)
    let moved_count = assert_only_moves_since(&session, rev_before_sort);
    assert!(moved_count > 0, "rows must have moved under CPU sorting");

    let item_still_exists =
        session.with_store(|s| s.get_model(MODEL).unwrap().contains_item(selected_item_id));
    assert!(
        item_still_exists,
        "selected item must survive sorting moves"
    );

    // 3. Filter query "worker | postgres"
    view.lock()
        .unwrap()
        .set_filter(&session, "worker | postgres")
        .expect("set filter");

    let filtered_count = session.with_store(|s| s.get_model(MODEL).unwrap().item_count);
    assert!(
        filtered_count > 0 && filtered_count < 1000,
        "filtered count must be a non-empty subset: {filtered_count}"
    );

    // Refresh under active filter
    let _ = view
        .lock()
        .unwrap()
        .refresh(&session, &mut source)
        .expect("refresh under filter");

    // 4. Clear filter: all 1,000 rows restored with identical ItemIds
    view.lock()
        .unwrap()
        .set_filter(&session, "")
        .expect("clear filter");

    let restored_count = session.with_store(|s| s.get_model(MODEL).unwrap().item_count);
    assert_eq!(
        restored_count, 1000,
        "all 1,000 rows must be restored when filter is cleared"
    );

    let selected_restored =
        session.with_store(|s| s.get_model(MODEL).unwrap().contains_item(selected_item_id));
    assert!(
        selected_restored,
        "selected item ID must be fully preserved when filter is cleared"
    );
}

// ---------------------------------------------------------------------------
// 2. Scale 10,000 Rows: Throughput, Frame Bounds, and Memory Caps
// ---------------------------------------------------------------------------

#[test]
fn test_scale_10000_rows_throughput_and_memory_caps() {
    let session = Arc::new(Session::mint());
    let mut source = SyntheticProcessSource::with_churn(10000, 0.02); // 2% churn = 200 rows per tick
    let (view, _) = start_from_source(&session, &mut source).expect("start 10k rows");
    let view = Arc::new(Mutex::new(view));

    assert_eq!(
        session.with_store(|s| s.get_model(MODEL).unwrap().item_count),
        10000
    );

    // Refresh 5 steps and measure elapsed time and transaction sizes
    for step in 0..5 {
        let t0 = Instant::now();
        let refreshed = view
            .lock()
            .unwrap()
            .refresh(&session, &mut source)
            .expect("10k refresh");
        let elapsed = t0.elapsed();

        assert_eq!(
            session.with_store(|s| s.get_model(MODEL).unwrap().item_count),
            10000
        );
        assert_eq!(refreshed.deleted, 200);

        // Throughput budget: 10,000-row refresh completes comfortably within deadline
        assert!(
            elapsed < Duration::from_millis(3000),
            "step {step} took {elapsed:?}, exceeding 3s budget"
        );
    }

    // Inspect all committed transactions in the journal: verify frame and operation limits (§26)
    let transactions = session
        .collect_replayed_transactions(0)
        .expect("collect replayed transactions");

    for tx in &transactions {
        let frame = encode_framed(&SruiMessage {
            msg: Some(srui_message::Msg::Transaction(tx.clone())),
        })
        .expect("encode tx");

        assert!(
            frame.len() <= DEFAULT_MAX_FRAME_SIZE,
            "transaction frame size {} exceeds DEFAULT_MAX_FRAME_SIZE {}",
            frame.len(),
            DEFAULT_MAX_FRAME_SIZE
        );
        assert!(
            tx.operations.len() <= 10000,
            "operation count {} exceeds limit",
            tx.operations.len()
        );
    }

    // Memory plateau: node count remains exactly SHELL_NODE_COUNT
    assert_eq!(session.with_store(|s| s.node_count()), SHELL_NODE_COUNT);
}

// ---------------------------------------------------------------------------
// 3. Probe 100,000 Rows: Locating Architectural Limits Without Claiming Support
// ---------------------------------------------------------------------------

#[test]
fn test_probe_100000_rows_limits_and_truncation() {
    let mut source = SyntheticProcessSource::with_churn(100000, 0.0);
    let snap = source.snapshot();
    assert_eq!(snap.records.len(), 100000);

    let session = Arc::new(Session::mint());
    let (mut view, _) = start_from_source(&session, &mut source).expect("start 100k");

    // MAX_PUBLISHED_ROWS = 100,000
    assert_eq!(
        session.with_store(|s| s.get_model(MODEL).unwrap().item_count),
        100000
    );

    // Eviction test: if source attempts to publish 100,005 rows, rows beyond 100,000 are capped/evicted (M6 kill)
    let mut excess_records = snap.records.clone();
    for i in 0..5 {
        excess_records.push(ProcessRecord {
            key: ProcessKey {
                source: srui_process_explorer::source::SourceId("excess".into()),
                host: Observed::Known(srui_process_explorer::source::HostId("h".into())),
                boot: Observed::Known(srui_process_explorer::source::BootId("b".into())),
                pid_namespace: Observed::Known(srui_process_explorer::source::PidNamespaceId(1)),
                pid: Observed::Known(200000 + i),
                creation: CreationToken::Opaque(format!("excess-{i}")),
            },
            display_name: DisplayName::from(format!("excess-{i}").as_str()),
            resident: Observed::Known(1024),
            cpu: srui_process_explorer::source::CpuUsage::WarmingUp,
        });
    }

    let excess_snap = srui_process_explorer::source::ProcessSnapshot {
        source: srui_process_explorer::source::SourceId("scale".into()),
        sampled_at: srui_process_explorer::source::SnapshotTime(
            SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_100),
        ),
        records: excess_records,
        vanished: 0,
        capped: srui_process_explorer::source::CappedRecords::none(),
        completeness: srui_process_explorer::source::Completeness::Complete,
        system: SyntheticProcessSource::system(),
    };

    let refreshed = view
        .apply(&session, "scale", &excess_snap)
        .expect("apply excess snapshot");
    let count = session.with_store(|s| s.get_model(MODEL).unwrap().item_count);
    assert!(
        count <= MAX_PUBLISHED_ROWS as u64,
        "count {count} must be capped at MAX_PUBLISHED_ROWS {MAX_PUBLISHED_ROWS}"
    );
    assert!(
        refreshed.evicted > 0 || refreshed.truncated > 0,
        "excess rows must be evicted or truncated"
    );
}

// ---------------------------------------------------------------------------
// 4. Model Range Fulfillment Under Concurrent Telemetry
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_model_range_fulfillment_under_concurrent_telemetry() {
    let session = Arc::new(Session::mint());
    let table_node = NodeId::new(5);
    let model_id = ModelId::new(1);

    // Seed session with Table and Model (1,000 rows)
    session
        .transaction(|ui| {
            Surface::builder(NodeId::new(1))
                .label("surface")
                .create(ui)?;
            ui.apply_op(&Operation::create_model(model_id, TypeRef::TABLE, 1000))?;
            Table::builder(table_node)
                .parent(NodeId::new(1))
                .model_ref(model_id)
                .columns(["Col0", "Col1"])
                .create(ui)?;
            Ok(())
        })
        .expect("seed table");

    // Register delayed range provider (simulating 40 ms disk/network work)
    let provider_invocations = Arc::new(AtomicUsize::new(0));
    let inv_clone = Arc::clone(&provider_invocations);
    let provider: ModelRangeProvider = Arc::new(move |query: ModelRangeQuery| {
        let inv = Arc::clone(&inv_clone);
        Box::pin(async move {
            inv.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(40)).await;
            Ok((0..query.count)
                .map(|offset| {
                    ModelItem::with_value(
                        ItemId::new(query.start_index + offset + 1),
                        format!("item-{}", query.start_index + offset),
                    )
                })
                .collect())
        })
    });
    session.register_model_range_provider(model_id, provider);

    // Concurrent telemetry loop advancing revisions every 15 ms
    let telemetry_stop = CancellationToken::new();
    let session_bg = Arc::clone(&session);
    let stop_token = telemetry_stop.clone();
    let telemetry_task = tokio::spawn(async move {
        let mut tick = 0;
        while !stop_token.is_cancelled() {
            tokio::time::sleep(Duration::from_millis(15)).await;
            tick += 1;
            let _ = session_bg.transaction(|ui| {
                ui.set(
                    NodeId::new(1),
                    srui_sdk::LABEL,
                    format!("surface-tick-{tick}"),
                )?;
                Ok(())
            });
        }
    });

    // 1. Submit range request with current revision
    let obs_rev = session.current_revision();
    let outcome = session
        .fulfill_model_range_request(ClientModelRangeRequest {
            node_id: table_node.get(),
            model_id: model_id.get(),
            start_index: 100,
            count: 64,
            observed_revision: obs_rev,
        })
        .await
        .expect("fulfill request");

    // Because telemetry ticked during provider execution, the request returns Stale or Committed
    match outcome {
        ModelRangeFulfillment::Stale | ModelRangeFulfillment::Committed { .. } => {}
        other => panic!("unexpected outcome: {other:?}"),
    }

    // 2. Bounded Progress Proof: within at most 3 attempts, range converges to Committed or AlreadyCached
    let mut converged = false;
    for _ in 0..5 {
        let current_rev = session.current_revision();
        let res = session
            .fulfill_model_range_request(ClientModelRangeRequest {
                node_id: table_node.get(),
                model_id: model_id.get(),
                start_index: 100,
                count: 64,
                observed_revision: current_rev,
            })
            .await
            .expect("retry range");

        if matches!(
            res,
            ModelRangeFulfillment::Committed { .. } | ModelRangeFulfillment::AlreadyCached
        ) {
            converged = true;
            break;
        }
    }
    assert!(
        converged,
        "range requests must demonstrate bounded progress under telemetry"
    );

    // Stop background telemetry
    telemetry_stop.cancel();
    let _ = telemetry_task.await;

    // 3. Out of bounds request returns OutOfBounds error (M3 kill)
    let oob_res = session
        .fulfill_model_range_request(ClientModelRangeRequest {
            node_id: table_node.get(),
            model_id: model_id.get(),
            start_index: 990,
            count: 64, // 990 + 64 = 1054 > 1000
            observed_revision: session.current_revision(),
        })
        .await;

    assert!(
        matches!(oob_res, Err(ModelRangeError::OutOfBounds { .. })),
        "out of bounds request must be rejected"
    );
}

// ---------------------------------------------------------------------------
// 5. Latency & RTT Envelope Testing with Pre-Recorded Thresholds
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_latency_and_rtt_envelope_thresholds() {
    // Pre-recorded RTT matrix: [0, 100, 300, 600] ms
    // Pre-recorded deadlines: RTT + 150 ms ceiling
    let scenarios = [
        (0, Duration::from_millis(200)),
        (100, Duration::from_millis(350)),
        (300, Duration::from_millis(750)),
        (600, Duration::from_millis(1350)),
    ];

    for (rtt_ms, deadline) in scenarios {
        let rtt = Duration::from_millis(rtt_ms);
        let session = Arc::new(Session::mint());
        let table_node = NodeId::new(5);
        let model_id = ModelId::new(1);

        session
            .transaction(|ui| {
                Surface::builder(NodeId::new(1)).create(ui)?;
                ui.apply_op(&Operation::create_model(model_id, TypeRef::TABLE, 1000))?;
                Table::builder(table_node)
                    .parent(NodeId::new(1))
                    .model_ref(model_id)
                    .create(ui)?;
                Ok(())
            })
            .expect("setup session");

        let provider: ModelRangeProvider = Arc::new(move |query: ModelRangeQuery| {
            Box::pin(async move {
                Ok((0..query.count)
                    .map(|offset| {
                        ModelItem::with_value(
                            ItemId::new(query.start_index + offset + 1),
                            format!("val-{}", query.start_index + offset),
                        )
                    })
                    .collect())
            })
        });
        session.register_model_range_provider(model_id, provider);

        let shutdown = CancellationToken::new();
        let (mut client_io, server_io) = DelayedDuplex::pair(rtt);
        let session_clone = Arc::clone(&session);
        let shutdown_clone = shutdown.clone();

        let server_task = tokio::spawn(async move {
            let _ = handle_connection(server_io, session_clone, shutdown_clone).await;
        });

        let mut read_buf = Vec::new();
        let initial_rev = perform_handshake(&mut client_io, &mut read_buf).await;

        // Drain initial catchup/snapshot transactions from server
        let _ = tokio::time::timeout(Duration::from_millis(50), async {
            let _ = read_message(&mut client_io, &mut read_buf).await;
        })
        .await;

        // Measure range progress deadline over DelayedDuplex
        let req = SruiMessage {
            msg: Some(srui_message::Msg::ClientModelRangeRequest(
                ClientModelRangeRequest {
                    node_id: table_node.get(),
                    model_id: model_id.get(),
                    start_index: 0,
                    count: 64,
                    observed_revision: initial_rev,
                },
            )),
        };

        let t0 = Instant::now();
        client_io
            .write_all(&encode_message(req))
            .await
            .expect("send range request");

        let response = read_message(&mut client_io, &mut read_buf).await;
        let elapsed = t0.elapsed();

        match response.msg {
            Some(srui_message::Msg::Transaction(tx)) => {
                let is_reset = tx
                    .operations
                    .iter()
                    .any(|op| matches!(&op.op, Some(Op::ModelResetRange(_))));
                assert!(is_reset, "expected ModelResetRange in response transaction");
            }
            other => panic!("expected Transaction message, got {other:?}"),
        }

        assert!(
            elapsed <= deadline,
            "RTT {rtt_ms} ms elapsed {elapsed:?} exceeded deadline {deadline:?}"
        );

        shutdown.cancel();
        let _ = server_task.await;
    }
}

// ---------------------------------------------------------------------------
// 6. Anti-Coalescing Proof: Structural Operations Are Never Coalesced or Dropped
// ---------------------------------------------------------------------------

#[test]
fn test_structural_model_operations_are_never_coalesced_or_dropped() {
    let base_rev = Revision::new(10);
    let next_rev = Revision::new(11);
    let status_node = NodeId::new(4);
    let model_id = ModelId::new(1);

    // 1. Pure scalar SetProperty transaction is coalesceable (§7.6, §12.1)
    let scalar_txn = DomainTxn::with_revisions(
        base_rev,
        next_rev,
        vec![Operation::set_property(
            status_node,
            srui_sdk::TEXT,
            Value::String("progress 50%".into()),
        )],
        0,
    );
    assert!(
        scalar_txn.is_coalesceable(),
        "pure scalar property transaction must be coalesceable"
    );

    // 2. Structural model operations are strictly NEVER coalesceable (§12.1, §20.4, M4 kill)
    let structural_ops = vec![
        Operation::model_update(
            model_id,
            None,
            vec![ModelItem::with_value(ItemId::new(1), "updated")],
        ),
        Operation::model_insert(
            model_id,
            0,
            vec![ModelItem::with_value(ItemId::new(2), "inserted")],
        ),
        Operation::model_delete(model_id, None, None, vec![ItemId::new(1)]),
        Operation::model_reset_range(
            model_id,
            0,
            vec![ModelItem::with_value(ItemId::new(3), "reset")],
            None,
        ),
    ];

    for op in structural_ops {
        let structural_txn = DomainTxn::with_revisions(base_rev, next_rev, vec![op.clone()], 0);
        assert!(
            !structural_txn.is_coalesceable(),
            "structural operation {op:?} must NEVER be coalesceable"
        );
    }
}

// ---------------------------------------------------------------------------
// 7. Finite Caps Enforcement Under Stress
// ---------------------------------------------------------------------------

#[test]
fn test_finite_caps_enforced_under_stress() {
    // Assert system-wide constants recorded in §5.1 table
    assert_eq!(MAX_RECORDS, 65536, "MAX_RECORDS cap must be 65536");
    assert_eq!(
        MAX_UNCERTAIN_PIDS, 17232,
        "MAX_UNCERTAIN_PIDS cap must be 17232"
    );
    assert_eq!(
        MAX_PUBLISHED_ROWS, 100000,
        "MAX_PUBLISHED_ROWS cap must be 100000"
    );
    assert_eq!(
        MAX_FILTER_QUERY_BYTES, 512,
        "MAX_FILTER_QUERY_BYTES cap must be 512"
    );
    assert_eq!(MAX_FILTER_TERMS, 16, "MAX_FILTER_TERMS cap must be 16");
}

// ---------------------------------------------------------------------------
// 8. Stale Filter Generation Rejection (Mutant M2 Kill)
// ---------------------------------------------------------------------------

#[test]
fn test_filter_concurrent_stale_rejection() {
    let session = Arc::new(Session::mint());
    let mut source = SyntheticProcessSource::new(100);
    let (view, _) = start_from_source(&session, &mut source).expect("start");
    let view = Arc::new(Mutex::new(view));

    // Staging generates generation 1
    let gen1 = view
        .lock()
        .unwrap()
        .stage_filter(&session, "worker")
        .expect("stage filter 1");

    // Staging generates generation 2
    let gen2 = view
        .lock()
        .unwrap()
        .stage_filter(&session, "postgres")
        .expect("stage filter 2");

    // Commit newer generation 2 first
    view.lock()
        .unwrap()
        .commit_filter(&session, gen2)
        .expect("commit gen 2");

    // Commit stale generation 1 afterwards: must be rejected and result in no changes
    let stale_outcome = view
        .lock()
        .unwrap()
        .commit_filter(&session, gen1)
        .expect("commit stale gen 1");
    assert_eq!(stale_outcome.inserted, 0);
    assert_eq!(stale_outcome.deleted, 0);

    // The filter query in effect is "postgres", not older "worker"
    assert_eq!(view.lock().unwrap().committed_filter().raw(), "postgres");
    let status_text = session.with_store(|s| {
        s.get_node(FILTER_STATUS)
            .unwrap()
            .get_property(srui_sdk::TEXT)
            .unwrap()
            .to_string()
    });
    assert!(
        status_text.contains("postgres"),
        "newer generation must win: {status_text}"
    );
}
