use bytes::BytesMut;
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};
use srui_protocol::{
    srui_message, ClientResume, SessionContinuity, SruiCodec, SruiMessage, TerminalResyncReason,
};
use srui_pty::{
    PTYManager, SubscribeSnapshot, TerminalEvent, TerminalSpec, MAX_TERMINAL_OUTPUT_FRAME_BYTES,
};
use srui_resources::CHUNK_PAYLOAD_SIZE;
use srui_sdk::{Button, Surface, ACTIVATE};
use srui_semantic_tree::{
    resolve_standard_node_type, resolve_standard_property, Event as DomainEvent, NodeId, Operation,
    Revision, Transaction, Value,
};
use srui_sessiond::{
    EventOutcome, LogicalChannelClass, OutboundItem, OutboundReceiver, ResumeOutcome, Session,
    SessionConfig,
};
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::thread;
use std::time::{Duration, Instant};
use tokio::time::timeout;
use tokio_util::codec::{Decoder, Encoder};

#[derive(Deserialize)]
struct Fixture {
    nodes: Vec<FixtureNode>,
}
#[derive(Deserialize)]
struct FixtureNode {
    id: u64,
    #[serde(rename = "type")]
    node_type: String,
    parent: Option<u64>,
    #[serde(default)]
    properties: BTreeMap<String, JsonValue>,
}

#[derive(Serialize)]
struct Output {
    artifacts: Artifacts,
    sections: Vec<Section>,
}

#[derive(Serialize)]
struct Artifacts {
    canonical_transaction_sha256: String,
    canonical_transaction_bytes: usize,
}

#[derive(Serialize)]
struct Section {
    id: &'static str,
    name: &'static str,
    metrics: Vec<Metric>,
    assertions: Vec<Assertion>,
    notes: Vec<String>,
}

#[derive(Serialize)]
struct Metric {
    name: String,
    value: f64,
    unit: &'static str,
    statistic: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    target: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_direction: Option<&'static str>,
}

#[derive(Serialize)]
struct Assertion {
    name: &'static str,
    passed: bool,
    detail: String,
}

fn metric(
    name: impl Into<String>,
    value: f64,
    unit: &'static str,
    statistic: &'static str,
) -> Metric {
    Metric {
        name: name.into(),
        value,
        unit,
        statistic,
        target: None,
        target_direction: None,
    }
}

fn percentile(mut values: Vec<f64>, fraction: f64) -> f64 {
    values.sort_by(f64::total_cmp);
    let index = ((values.len() - 1) as f64 * fraction).round() as usize;
    values[index.min(values.len() - 1)]
}

fn p50(values: Vec<f64>) -> f64 {
    percentile(values, 0.50)
}

fn json_to_value(value: &JsonValue) -> Result<Value, String> {
    match value {
        JsonValue::String(value) => Ok(Value::from(value.as_str())),
        JsonValue::Bool(value) => Ok(Value::from(*value)),
        JsonValue::Number(value) => value
            .as_f64()
            .map(Value::from)
            .ok_or_else(|| "fixture number is not representable as f64".to_string()),
        other => Err(format!("unsupported fixture value: {other}")),
    }
}

fn build_transaction(fixture: &Fixture) -> Result<Transaction, String> {
    let mut operations = Vec::with_capacity(fixture.nodes.len());
    for node in &fixture.nodes {
        let node_type =
            resolve_standard_node_type(&node.node_type).map_err(|error| error.to_string())?;
        let mut properties = Vec::with_capacity(node.properties.len());
        for (name, value) in &node.properties {
            properties.push((
                resolve_standard_property(name).map_err(|error| error.to_string())?,
                json_to_value(value)?,
            ));
        }
        operations.push(Operation::create_node(
            NodeId::new(node.id),
            node_type,
            node.parent.map(NodeId::new),
            None,
            properties,
        ));
    }
    Ok(Transaction::new(Revision::INITIAL, operations))
}

fn serialization(fixture: &Fixture, iterations: usize) -> Result<Section, String> {
    let mut generation_ms = Vec::with_capacity(iterations);
    let mut serialization_ms = Vec::with_capacity(iterations);
    let mut bytes = 0;
    for _ in 0..iterations {
        let start = Instant::now();
        let transaction = build_transaction(fixture)?;
        generation_ms.push(start.elapsed().as_secs_f64() * 1_000.0);

        let start = Instant::now();
        let encoded = transaction.to_wire_bytes();
        serialization_ms.push(start.elapsed().as_secs_f64() * 1_000.0);
        bytes = encoded.len();
        std::hint::black_box(encoded);
    }
    Ok(Section {
        id: "31.2",
        name: "Serialization",
        metrics: vec![
            metric(
                "abstract state generation",
                p50(generation_ms.clone()),
                "ms",
                "p50",
            ),
            metric(
                "abstract state generation",
                percentile(generation_ms.clone(), 0.95),
                "ms",
                "p95",
            ),
            metric(
                "abstract state generation",
                percentile(generation_ms, 0.99),
                "ms",
                "p99",
            ),
            metric(
                "protobuf serialization",
                p50(serialization_ms.clone()),
                "ms",
                "p50",
            ),
            metric(
                "protobuf serialization",
                percentile(serialization_ms.clone(), 0.95),
                "ms",
                "p95",
            ),
            metric(
                "protobuf serialization",
                percentile(serialization_ms, 0.99),
                "ms",
                "p99",
            ),
            metric(
                "serialized transaction size",
                bytes as f64,
                "bytes",
                "exact",
            ),
        ],
        assertions: vec![Assertion {
            name: "shared fixture produced SRUI protobuf",
            passed: bytes > 0,
            detail: format!("{} nodes encoded into {bytes} bytes", fixture.nodes.len()),
        }],
        notes: vec![
            "PTY spawn and renderer work are excluded from server serialization timing.".into(),
        ],
    })
}
fn push_timing_distributions(metrics: &mut Vec<Metric>, timings: BTreeMap<&'static str, Vec<f64>>) {
    for (name, values) in timings {
        metrics.push(metric(name, p50(values.clone()), "ms", "p50"));
        metrics.push(metric(name, percentile(values.clone(), 0.95), "ms", "p95"));
        metrics.push(metric(name, percentile(values, 0.99), "ms", "p99"));
    }
}

async fn next_resource(receiver: &mut OutboundReceiver) -> Result<OutboundItem, String> {
    timeout(
        Duration::from_secs(2),
        receiver.recv_class(LogicalChannelClass::Resource),
    )
    .await
    .map_err(|_| "timed out waiting for a production resource frame".to_string())?
    .map_err(|error| error.to_string())
}

fn empty_wire_transaction(base_revision: u64) -> srui_protocol::Transaction {
    srui_protocol::Transaction {
        base_revision,
        new_revision: base_revision + 1,
        priority: 0,
        operations: Vec::new(),
    }
}

fn resume_request(session: &Session, client: &[u8], revision: u64) -> ClientResume {
    ClientResume {
        session_id: session.session_id(),
        client_instance_id: client.to_vec(),
        last_applied_revision: revision,
        ..ClientResume::default()
    }
}

fn partial_transaction_frame_is_buffered(
    transaction: &srui_protocol::Transaction,
) -> Result<bool, String> {
    let mut codec = SruiCodec::new();
    let mut complete = BytesMut::new();
    codec
        .encode(
            SruiMessage {
                msg: Some(srui_message::Msg::Transaction(transaction.clone())),
            },
            &mut complete,
        )
        .map_err(|error| error.to_string())?;
    let split = complete.len() / 2;
    let mut partial = BytesMut::from(&complete[..split]);
    let decoded = codec
        .decode(&mut partial)
        .map_err(|error| error.to_string())?;
    Ok(decoded.is_none() && partial.len() == split)
}

async fn reconnect(iterations: usize) -> Result<Section, String> {
    let mut timings: BTreeMap<&'static str, Vec<f64>> = BTreeMap::new();
    let mut resource_replay_correct = true;
    let mut transaction_replay_correct = true;
    let mut event_boundary_correct = true;
    let mut duplicate_correct = true;
    let mut retention_correct = true;
    let resource_payload: Vec<u8> = (0..(CHUNK_PAYLOAD_SIZE * 2 + 37))
        .map(|index| (index % 251) as u8)
        .collect();

    for sample in 0..iterations {
        // Interrupt an actual production resource lane after its first chunk. A replacement
        // subscription is seeded from the session CAS and must restart at metadata/offset zero.
        let resource_session = Session::new(format!("benchmark-resource-{sample}"));
        let published = resource_session
            .publish_resource(&resource_payload)
            .map_err(|error| error.to_string())?;
        let client = format!("resource-client-{sample}").into_bytes();
        let mut interrupted = resource_session
            .subscribe_transactions(client.clone())
            .map_err(|error| error.to_string())?;
        let metadata = next_resource(&mut interrupted).await?;
        let first_chunk = next_resource(&mut interrupted).await?;
        let interrupted_at_real_boundary = matches!(
            &metadata,
            OutboundItem::ResourceMetadata(value)
                if value.resource_hash == published.hash.0.to_vec()
                    && value.encoded_length == resource_payload.len() as u64
        ) && matches!(
            &first_chunk,
            OutboundItem::ResourceChunk(value)
                if value.resource_hash == published.hash.0.to_vec()
                    && value.byte_offset == 0
                    && value.data.len() == CHUNK_PAYLOAD_SIZE
        );
        drop(interrupted);

        let start = Instant::now();
        let mut resumed = resource_session
            .subscribe_transactions(client)
            .map_err(|error| error.to_string())?;
        let mut resumed_metadata = false;
        let mut resumed_bytes = Vec::with_capacity(resource_payload.len());
        let mut next_offset = 0_u64;
        while resumed_bytes.len() < resource_payload.len() {
            match next_resource(&mut resumed).await? {
                OutboundItem::ResourceMetadata(value) => {
                    resumed_metadata = value.resource_hash == published.hash.0.to_vec()
                        && value.encoded_length == resource_payload.len() as u64;
                }
                OutboundItem::ResourceChunk(value) => {
                    if value.resource_hash != published.hash.0.to_vec()
                        || value.byte_offset != next_offset
                    {
                        resource_replay_correct = false;
                    }
                    next_offset = next_offset.saturating_add(value.data.len() as u64);
                    resumed_bytes.extend_from_slice(&value.data);
                }
                OutboundItem::Transaction(_) => {
                    resource_replay_correct = false;
                }
            }
        }
        timings
            .entry("mid-resource reconnect and exact replay")
            .or_default()
            .push(start.elapsed().as_secs_f64() * 1_000.0);
        resource_replay_correct &= interrupted_at_real_boundary
            && resumed_metadata
            && resumed_bytes == resource_payload
            && next_offset == resource_payload.len() as u64;

        // Feed half of a real length-delimited TRANSACTION through SruiCodec, detach before it
        // completes, then resume through Session::bootstrap_resume and recover the atomic commit.
        let transaction_session = Session::new(format!("benchmark-frame-{sample}"));
        let committed = transaction_session
            .commit_transaction(empty_wire_transaction(0))
            .map_err(|error| error.to_string())?;
        let attachment = transaction_session
            .attach()
            .ok_or_else(|| "transaction session refused attachment".to_string())?;
        let start = Instant::now();
        let partial_was_buffered = partial_transaction_frame_is_buffered(&committed)?;
        drop(attachment);
        let resumed_transaction = transaction_session
            .bootstrap_resume(&resume_request(
                &transaction_session,
                format!("frame-client-{sample}").as_bytes(),
                0,
            ))
            .map_err(|error| error.to_string())?;
        timings
            .entry("mid-transaction frame discard and atomic replay")
            .or_default()
            .push(start.elapsed().as_secs_f64() * 1_000.0);
        transaction_replay_correct &= partial_was_buffered
            && transaction_session.is_detached()
            && matches!(
                resumed_transaction.outcome,
                ResumeOutcome::Replay { replayed, .. }
                    if replayed == vec![committed]
            );

        // Use Session's production event admission, validation, handler dispatch, settlement,
        // and cached result path. Dropping the attachment models loss immediately before receipt;
        // replaying after Processed models loss after the side effect but before EVENT_ACK.
        let event_session = Session::new(format!("benchmark-event-{sample}"));
        event_session
            .transaction(|ui| {
                Surface::builder(NodeId::new(1)).create(ui)?;
                Button::builder(NodeId::new(2))
                    .parent(NodeId::new(1))
                    .label("Run")
                    .create(ui)?;
                Ok(())
            })
            .map_err(|error| error.to_string())?;
        let side_effects = Arc::new(AtomicUsize::new(0));
        let handler_side_effects = Arc::clone(&side_effects);
        event_session.on(NodeId::new(2), ACTIVATE, move |_, _| {
            handler_side_effects.fetch_add(1, Ordering::SeqCst);
        });
        let event = DomainEvent::activate(
            1,
            "benchmark-event",
            event_session.current_revision(),
            NodeId::new(2),
        )
        .with_client_instance_id(format!("event-client-{sample}").into_bytes())
        .to_wire();

        let pre_receipt_attachment = event_session
            .attach()
            .ok_or_else(|| "event session refused attachment".to_string())?;
        let start = Instant::now();
        drop(pre_receipt_attachment);
        timings
            .entry("disconnect immediately before event receipt")
            .or_default()
            .push(start.elapsed().as_secs_f64() * 1_000.0);
        event_boundary_correct &=
            event_session.is_detached() && side_effects.load(Ordering::SeqCst) == 0;

        let start = Instant::now();
        let first = event_session
            .process_event(&event)
            .map_err(|error| error.to_string())?;
        timings
            .entry("event receipt through settled side effect")
            .or_default()
            .push(start.elapsed().as_secs_f64() * 1_000.0);
        event_boundary_correct &= matches!(first, EventOutcome::Processed { .. })
            && side_effects.load(Ordering::SeqCst) == 1;

        let start = Instant::now();
        let replay = event_session
            .process_event(&event)
            .map_err(|error| error.to_string())?;
        timings
            .entry("lost ACK cached DUPLICATE response")
            .or_default()
            .push(start.elapsed().as_secs_f64() * 1_000.0);
        duplicate_correct &= matches!(
            replay,
            EventOutcome::Duplicate {
                accepted: true,
                revision_after_effect: 1,
                last_processed_event_seq: 1,
                ..
            }
        ) && side_effects.load(Ordering::SeqCst) == 1;

        // Exercise both resume decisions through Session::bootstrap_resume, not the journal type.
        let retention_session = Session::with_config(
            format!("benchmark-retention-{sample}"),
            SessionConfig {
                journal_capacity: 4,
                ..SessionConfig::default()
            },
        );
        for base in 0..8 {
            retention_session
                .commit_transaction(empty_wire_transaction(base))
                .map_err(|error| error.to_string())?;
        }

        let start = Instant::now();
        let within = retention_session
            .bootstrap_resume(&resume_request(
                &retention_session,
                format!("retained-client-{sample}").as_bytes(),
                6,
            ))
            .map_err(|error| error.to_string())?;
        timings
            .entry("resume within journal retention")
            .or_default()
            .push(start.elapsed().as_secs_f64() * 1_000.0);
        retention_correct &= matches!(
            within.outcome,
            ResumeOutcome::Replay { replayed, .. }
                if replayed.len() == 2
                    && replayed[0].base_revision == 6
                    && replayed[1].new_revision == 8
        );

        let start = Instant::now();
        let beyond = retention_session
            .bootstrap_resume(&resume_request(
                &retention_session,
                format!("expired-client-{sample}").as_bytes(),
                0,
            ))
            .map_err(|error| error.to_string())?;
        timings
            .entry("resume beyond journal retention")
            .or_default()
            .push(start.elapsed().as_secs_f64() * 1_000.0);
        retention_correct &= matches!(
            beyond.outcome,
            ResumeOutcome::Resync {
                resync_msg,
                snapshot_transaction,
            } if resync_msg.continuity == SessionContinuity::SameSession as i32
                && resync_msg.snapshot_revision == 8
                && snapshot_transaction.new_revision == 8
        );
    }

    let mut metrics = Vec::new();
    push_timing_distributions(&mut metrics, timings);
    Ok(Section {
        id: "31.5",
        name: "Reconnect",
        metrics,
        assertions: vec![
            Assertion {
                name: "mid-resource reconnect restarts production transfer at offset zero",
                passed: resource_replay_correct,
                detail: format!(
                    "{}-byte CAS object interrupted after one real chunk and reconstructed exactly",
                    resource_payload.len()
                ),
            },
            Assertion {
                name: "mid-transaction disconnect exposes no partial frame",
                passed: transaction_replay_correct,
                detail:
                    "SruiCodec retained no complete message; Session resume replayed one atomic commit"
                        .into(),
            },
            Assertion {
                name: "before/after event receipt boundaries preserve exactly-once dispatch",
                passed: event_boundary_correct,
                detail: "zero handlers before receipt; one settled handler dispatch after receipt"
                    .into(),
            },
            Assertion {
                name: "lost ACK replay is DUPLICATE without a second side effect",
                passed: duplicate_correct,
                detail:
                    "Session::process_event returned cached accepted revision 1; handler count stayed 1"
                        .into(),
            },
            Assertion {
                name: "journal retention boundary selects replay versus same-session resync",
                passed: retention_correct,
                detail: "revision 6 replayed 6→8; revision 0 produced SAME_SESSION snapshot at 8"
                    .into(),
            },
        ],
        notes: vec![
            "Superseded resume-attempt inertness is measured against the Task 23 client generation guard in the macOS driver."
                .into(),
        ],
    })
}
const TERMINAL_LINE: &[u8] = b"\x1b[32mbenchmark output\x1b[0m\r\n";
const TERMINAL_LINES: usize = 256;

fn terminal_script(lines: usize) -> String {
    format!(
        "stty raw -echo; i=0; while [ \"$i\" -lt {lines} ]; do \
         printf '\\033[32mbenchmark output\\033[0m\\r\\n'; i=$((i + 1)); done"
    )
}

fn pty_roundtrip(payload: &[u8], script: &str) -> Result<(f64, Vec<u8>, bool), String> {
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| error.to_string())?;
    let mut command = CommandBuilder::new("/bin/sh");
    command.arg("-c");
    command.arg(script);
    let start = Instant::now();
    let mut child = pair
        .slave
        .spawn_command(command)
        .map_err(|error| error.to_string())?;
    drop(pair.slave);
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|error| error.to_string())?;
    let mut received = Vec::with_capacity(payload.len());
    reader
        .read_to_end(&mut received)
        .map_err(|error| error.to_string())?;
    let status = child.wait().map_err(|error| error.to_string())?;
    let elapsed = start.elapsed().as_secs_f64() * 1_000.0;
    Ok((elapsed, received, status.success()))
}

fn terminal_spec(script: &str, ring_capacity: usize) -> TerminalSpec {
    TerminalSpec {
        executable: "/bin/sh".into(),
        args: vec!["-c".into(), script.into()],
        ring_capacity,
        ..TerminalSpec::default()
    }
}

fn wait_for_terminal_bytes(
    manager: &PTYManager,
    id: NodeId,
    expected_next_offset: u64,
) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if manager
            .offsets(id)
            .is_some_and(|(_, next)| next >= expected_next_offset)
        {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(1));
    }
    Err(format!(
        "timed out waiting for terminal {id:?} to reach offset {expected_next_offset}"
    ))
}

fn embedded_pty_roundtrip(
    payload: &[u8],
    script: &str,
) -> Result<(f64, Vec<u8>, usize, bool), String> {
    let manager = PTYManager::default();
    let id = NodeId::new(1);
    let start = Instant::now();
    manager
        .spawn(id, terminal_spec(script, payload.len() * 2))
        .map_err(|error| error.to_string())?;
    wait_for_terminal_bytes(&manager, id, payload.len() as u64)?;
    let outcome = manager
        .subscribe(id, 0)
        .map_err(|error| error.to_string())?;
    let mut received = Vec::with_capacity(payload.len());
    let mut expected_offset = 0_u64;
    let mut offsets_exact = true;
    let frames = match outcome.snapshot {
        SubscribeSnapshot::Replay { frames } => frames,
        SubscribeSnapshot::Resync { .. } => {
            manager.shutdown();
            return Ok((start.elapsed().as_secs_f64() * 1_000.0, received, 0, false));
        }
    };
    for frame in &frames {
        offsets_exact &= frame.stream_id == id.get()
            && frame.byte_offset == expected_offset
            && !frame.data.is_empty()
            && frame.data.len() <= MAX_TERMINAL_OUTPUT_FRAME_BYTES;
        expected_offset = expected_offset.saturating_add(frame.data.len() as u64);
        received.extend_from_slice(&frame.data);
    }
    let elapsed = start.elapsed().as_secs_f64() * 1_000.0;
    manager.shutdown();
    Ok((elapsed, received, frames.len(), offsets_exact))
}

fn terminal(iterations: usize) -> Result<Section, String> {
    let payload = TERMINAL_LINE.repeat(TERMINAL_LINES);
    let script = terminal_script(TERMINAL_LINES);
    let sample_count = iterations.min(20);
    let mut standalone_ms = Vec::with_capacity(sample_count);
    let mut embedded_ms = Vec::with_capacity(sample_count);
    let mut exact_payloads = true;
    let mut child_exit_success = true;
    let mut offsets_exact = true;
    let mut embedded_frame_count = 0;

    for _ in 0..sample_count {
        let (elapsed, received, exited_successfully) = pty_roundtrip(&payload, &script)?;
        standalone_ms.push(elapsed);
        exact_payloads &= received == payload;
        child_exit_success &= exited_successfully;

        let (elapsed, received, frame_count, sample_offsets_exact) =
            embedded_pty_roundtrip(&payload, &script)?;
        embedded_ms.push(elapsed);
        exact_payloads &= received == payload;
        offsets_exact &= sample_offsets_exact;
        embedded_frame_count = frame_count;
    }

    // Exhaust the ring through a real PTY stream, then reconnect through PTYManager::subscribe.
    // The public subscription maps the retention gap to one explicit RETENTION_LOSS event.
    let exhaustion_manager = PTYManager::default();
    let exhaustion_id = NodeId::new(2);
    exhaustion_manager
        .spawn(exhaustion_id, terminal_spec(&script, 1_024))
        .map_err(|error| error.to_string())?;
    wait_for_terminal_bytes(&exhaustion_manager, exhaustion_id, payload.len() as u64)?;
    let start = Instant::now();
    let exhausted = exhaustion_manager
        .subscribe(exhaustion_id, 0)
        .map_err(|error| error.to_string())?;
    let exhaustion_ms = start.elapsed().as_secs_f64() * 1_000.0;
    let exhaustion_detected = matches!(
        &exhausted.snapshot,
        SubscribeSnapshot::Resync {
            requested_offset: 0,
            retained_from_offset,
            resume_at_offset,
            reason: TerminalResyncReason::RetentionLoss,
        } if *retained_from_offset > 0 && *resume_at_offset == payload.len() as u64
    ) && matches!(
        exhausted.catch_up_events().as_slice(),
        [TerminalEvent::Resync(resync)]
            if resync.reason == TerminalResyncReason::RetentionLoss as i32
                && resync.requested_offset == 0
                && resync.retained_from_offset > 0
                && resync.resume_at_offset == payload.len() as u64
    );
    exhaustion_manager.shutdown();

    let mut metrics = Vec::new();
    let mut timings = BTreeMap::new();
    timings.insert("standalone PTY exact ANSI interaction", standalone_ms);
    timings.insert(
        "embedded SRUI PTY exact ANSI capture and framing",
        embedded_ms,
    );
    push_timing_distributions(&mut metrics, timings);
    metrics.push(metric(
        "terminal reconnect retention-loss decision",
        exhaustion_ms,
        "ms",
        "sample",
    ));
    metrics.push(metric(
        "terminal payload",
        payload.len() as f64,
        "bytes",
        "exact",
    ));
    metrics.push(metric(
        "embedded terminal frame count",
        embedded_frame_count as f64,
        "messages",
        "exact",
    ));

    Ok(Section {
        id: "31.6",
        name: "Terminal",
        metrics,
        assertions: vec![
            Assertion {
                name: "standalone and embedded PTYs emit the identical ANSI byte stream",
                passed: exact_payloads,
                detail: format!("both paths compared all {} payload bytes", payload.len()),
            },
            Assertion {
                name: "standalone terminal command exits successfully",
                passed: child_exit_success,
                detail: format!("{sample_count} child exit statuses checked"),
            },
            Assertion {
                name: "embedded terminal frames preserve exact offsets and bounds",
                passed: offsets_exact,
                detail: format!(
                    "{embedded_frame_count} ordered frames; each at most {MAX_TERMINAL_OUTPUT_FRAME_BYTES} bytes"
                ),
            },
            Assertion {
                name: "reconnect ring-buffer exhaustion maps to RETENTION_LOSS",
                passed: exhaustion_detected,
                detail:
                    "PTYManager::subscribe returned Resync and catch_up emitted TerminalResyncRequired"
                        .into(),
            },
        ],
        notes: vec![
            "Standalone and embedded samples both include production PTY spawn, identical shell execution, EOF, and exact ANSI output; embedded additionally captures and frames the bytes."
                .into(),
        ],
    })
}

fn argument(name: &str) -> Result<String, String> {
    let mut args = std::env::args();
    while let Some(arg) = args.next() {
        if arg == name {
            return args
                .next()
                .ok_or_else(|| format!("{name} requires a value"));
        }
    }
    Err(format!("missing {name}"))
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<(), String> {
    let fixture_path = PathBuf::from(argument("--fixture")?);
    let output_path = PathBuf::from(argument("--output")?);
    let profile = argument("--profile")?;
    let iterations = if profile == "full" { 500 } else { 25 };
    let fixture: Fixture = serde_json::from_slice(
        &fs::read(&fixture_path).map_err(|error| format!("{}: {error}", fixture_path.display()))?,
    )
    .map_err(|error| error.to_string())?;

    let canonical_transaction = build_transaction(&fixture)?;
    let canonical_bytes = canonical_transaction.to_wire_bytes();
    let canonical_digest = Sha256::digest(&canonical_bytes);
    let output = Output {
        artifacts: Artifacts {
            canonical_transaction_sha256: format!("{canonical_digest:x}"),
            canonical_transaction_bytes: canonical_bytes.len(),
        },
        sections: vec![
            serialization(&fixture, iterations)?,
            reconnect(iterations).await?,
            terminal(iterations)?,
        ],
    };
    let json = serde_json::to_vec_pretty(&output).map_err(|error| error.to_string())?;
    fs::write(Path::new(&output_path), json).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_srui_transaction_frame_is_not_decoded() {
        let transaction = empty_wire_transaction(0);
        assert!(partial_transaction_frame_is_buffered(&transaction).unwrap());
    }

    #[test]
    fn terminal_fixture_matches_cross_language_contract() {
        let payload = TERMINAL_LINE.repeat(TERMINAL_LINES);
        assert_eq!(payload.len(), 6_912);
        assert_eq!(
            &payload[..TERMINAL_LINE.len()],
            b"\x1b[32mbenchmark output\x1b[0m\r\n"
        );
    }
}
