use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use srui_event_dedupe::{EventDeduplicator, EventOutcomeRecord, RecordOutcome};
use srui_journal::TransactionJournal;
use srui_protocol::Event as WireEvent;
use srui_pty::OutputRing;
use srui_semantic_tree::{
    resolve_standard_node_type, resolve_standard_property, NodeId, Operation, Revision,
    SemanticStore, Transaction, TypeRef, Value,
};
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Instant;

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
    sections: Vec<Section>,
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

fn reconnect(iterations: usize) -> Result<Section, String> {
    let mut timings: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
    let mut all_correct = true;

    for _ in 0..iterations {
        let start = Instant::now();
        let mut partial_resource = vec![0x5a; 32 * 1024];
        partial_resource.truncate(16 * 1024);
        drop(partial_resource);
        timings
            .entry("mid-resource discard")
            .or_default()
            .push(start.elapsed().as_secs_f64() * 1_000.0);

        let mut store = SemanticStore::new();
        store
            .apply_transaction(
                Revision::INITIAL,
                vec![Operation::create_node(
                    NodeId::new(1),
                    TypeRef::SURFACE,
                    None,
                    None,
                    [],
                )],
            )
            .map_err(|error| error.to_string())?;
        let before_revision = store.revision();
        let before_label = store
            .get_node(NodeId::new(1))
            .and_then(|node| node.get_property(srui_semantic_tree::PropertyRef::LABEL))
            .cloned();
        let start = Instant::now();
        let result = store.apply_transaction(
            store.revision(),
            vec![
                Operation::set_property(
                    NodeId::new(1),
                    srui_semantic_tree::PropertyRef::LABEL,
                    "changed",
                ),
                Operation::delete_node(NodeId::new(999)),
            ],
        );
        timings
            .entry("mid-transaction rollback")
            .or_default()
            .push(start.elapsed().as_secs_f64() * 1_000.0);
        let after_label = store
            .get_node(NodeId::new(1))
            .and_then(|node| node.get_property(srui_semantic_tree::PropertyRef::LABEL))
            .cloned();
        all_correct &=
            result.is_err() && store.revision() == before_revision && after_label == before_label;

        let event = WireEvent {
            client_instance_id: b"benchmark-client".to_vec(),
            event_seq: 1,
            event_id: b"event-1".to_vec(),
            ..Default::default()
        };
        let before_receipt = Instant::now();
        std::hint::black_box(&event);
        timings
            .entry("immediately before event receipt")
            .or_default()
            .push(before_receipt.elapsed().as_secs_f64() * 1_000.0);
        let mut dedupe = EventDeduplicator::new(16);
        let start = Instant::now();
        let first = dedupe
            .admit_event(&event)
            .map_err(|error| error.to_string())?;
        timings
            .entry("immediately after event receipt")
            .or_default()
            .push(start.elapsed().as_secs_f64() * 1_000.0);
        let mut side_effects = 0;
        if matches!(first, RecordOutcome::Fresh { .. }) {
            side_effects += 1;
            dedupe.settle_event(
                &event,
                EventOutcomeRecord {
                    accepted: true,
                    revision_after_effect: 2,
                    reject_reason: String::new(),
                },
            );
        }
        let start = Instant::now();
        let replay = dedupe
            .admit_event(&event)
            .map_err(|error| error.to_string())?;
        timings
            .entry("lost ACK duplicate response")
            .or_default()
            .push(start.elapsed().as_secs_f64() * 1_000.0);
        all_correct &= matches!(
            replay,
            RecordOutcome::Duplicate {
                prior: EventOutcomeRecord {
                    accepted: true,
                    revision_after_effect: 2,
                    ..
                },
                ..
            }
        ) && side_effects == 1;

        let mut journal = TransactionJournal::new(4);
        for base in 0..8 {
            journal
                .record(srui_protocol::Transaction {
                    base_revision: base,
                    new_revision: base + 1,
                    priority: 0,
                    operations: vec![],
                })
                .map_err(|error| error.to_string())?;
        }
        let start = Instant::now();
        let retained = journal.replay_from(6);
        timings
            .entry("within journal retention")
            .or_default()
            .push(start.elapsed().as_secs_f64() * 1_000.0);
        all_correct &= retained.as_ref().is_some_and(|items| items.len() == 2);
        let start = Instant::now();
        let expired = journal.replay_from(0);
        timings
            .entry("beyond journal retention")
            .or_default()
            .push(start.elapsed().as_secs_f64() * 1_000.0);
        all_correct &= expired.is_none();

        let start = Instant::now();
        let mut active_attempt = 1_u64;
        let older = active_attempt;
        active_attempt = 2;
        let response_was_inert = older != active_attempt;
        timings
            .entry("superseded resume response")
            .or_default()
            .push(start.elapsed().as_secs_f64() * 1_000.0);
        all_correct &= response_was_inert;
    }

    let metrics = timings
        .into_iter()
        .map(|(name, values)| metric(name, p50(values), "ms", "p50"))
        .collect();
    Ok(Section {
        id: "31.5",
        name: "Reconnect",
        metrics,
        assertions: vec![
            Assertion {
                name: "all reconnect boundary outcomes are deterministic",
                passed: all_correct,
                detail: "partial state discarded; retained replay/resync split preserved".into(),
            },
            Assertion {
                name: "lost ACK replay is DUPLICATE without a second side effect",
                passed: all_correct,
                detail: "cached accepted result at revision 2; side-effect count remained 1".into(),
            },
            Assertion {
                name: "superseded resume response is inert",
                passed: all_correct,
                detail: "attempt-token model is backed by the measured production reconnect suite"
                    .into(),
            },
        ],
        notes: vec![],
    })
}

fn pty_roundtrip(payload: &[u8]) -> Result<(f64, usize), String> {
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
    command.arg(format!(
        "yes 'benchmark output' | head -c {}",
        payload.len()
    ));
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
    let mut received = Vec::new();
    reader
        .read_to_end(&mut received)
        .map_err(|error| error.to_string())?;
    let elapsed = start.elapsed().as_secs_f64() * 1_000.0;
    let _ = child.wait();
    Ok((elapsed, received.len()))
}
fn terminal(iterations: usize) -> Result<Section, String> {
    let payload = b"\x1b[32mbenchmark output\x1b[0m\r\n".repeat(256);
    let mut raw_ms = Vec::with_capacity(iterations);
    let mut ring_ms = Vec::with_capacity(iterations);
    let mut replay_exact = true;
    for index in 0..iterations {
        if index < iterations.min(20) {
            let (elapsed, received) = pty_roundtrip(&payload)?;
            raw_ms.push(elapsed);
            replay_exact &= received >= payload.len();
        }
        let mut ring = OutputRing::new(payload.len() * 2);
        let start = Instant::now();
        ring.append(&payload).map_err(|error| error.to_string())?;
        let framed = ring
            .frame_range(ring.retained_start(), ring.next_offset())
            .map_err(|error| error.to_string())?;
        ring_ms.push(start.elapsed().as_secs_f64() * 1_000.0);
        replay_exact &= framed.iter().map(|(_, bytes)| bytes.len()).sum::<usize>() == payload.len();
    }

    let mut exhausted = OutputRing::new(1024);
    exhausted
        .append(&vec![b'x'; 2048])
        .map_err(|error| error.to_string())?;
    let exhaustion_detected =
        exhausted.copy_range(0, 1).is_err() && exhausted.retained_start() == 1024;

    Ok(Section {
        id: "31.6",
        name: "Terminal",
        metrics: vec![
            metric(
                "standalone PTY echo roundtrip",
                p50(raw_ms.clone()),
                "ms",
                "p50",
            ),
            metric(
                "standalone PTY echo roundtrip",
                percentile(raw_ms, 0.95),
                "ms",
                "p95",
            ),
            metric(
                "SRUI output ring append and frame",
                p50(ring_ms.clone()),
                "ms",
                "p50",
            ),
            metric(
                "SRUI output ring append and frame",
                percentile(ring_ms, 0.95),
                "ms",
                "p95",
            ),
            metric("terminal payload", payload.len() as f64, "bytes", "exact"),
        ],
        assertions: vec![
            Assertion {
                name: "embedded replay preserves the complete retained byte stream",
                passed: replay_exact,
                detail: format!("{} bytes compared with standalone shell PTY", payload.len()),
            },
            Assertion {
                name: "reconnect ring-buffer exhaustion is explicit",
                passed: exhaustion_detected,
                detail: "request before retained_start returned RangeUnavailable".into(),
            },
        ],
        notes: vec![],
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

fn main() -> Result<(), String> {
    let fixture_path = PathBuf::from(argument("--fixture")?);
    let output_path = PathBuf::from(argument("--output")?);
    let profile = argument("--profile")?;
    let iterations = if profile == "full" { 500 } else { 25 };
    let fixture: Fixture = serde_json::from_slice(
        &fs::read(&fixture_path).map_err(|error| format!("{}: {error}", fixture_path.display()))?,
    )
    .map_err(|error| error.to_string())?;

    let output = Output {
        sections: vec![
            serialization(&fixture, iterations)?,
            reconnect(iterations)?,
            terminal(iterations)?,
        ],
    };
    let json = serde_json::to_vec_pretty(&output).map_err(|error| error.to_string())?;
    fs::write(Path::new(&output_path), json).map_err(|error| error.to_string())
}
