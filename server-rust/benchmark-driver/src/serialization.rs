use crate::report::{metric, p50, percentile, Assertion, Section};
use serde::Deserialize;
use serde_json::Value as JsonValue;
use srui_semantic_tree::{
    resolve_standard_node_type, resolve_standard_property, NodeId, Operation, Revision,
    Transaction, Value,
};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

#[derive(Deserialize)]
pub(crate) struct Fixture {
    first_paint_node_count: usize,
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

fn json_to_value(value: &JsonValue) -> Result<Value, String> {
    match value {
        JsonValue::String(value) => Ok(Value::from(value.as_str())),
        JsonValue::Bool(value) => Ok(Value::from(*value)),
        JsonValue::Number(value) => value
            .as_f64()
            .map(Value::from)
            .ok_or_else(|| "fixture number is not representable as f64".to_string()),
        JsonValue::Array(values) => values
            .iter()
            .map(json_to_value)
            .collect::<Result<Vec<_>, _>>()
            .map(Value::List),
        other => Err(format!("unsupported fixture value: {other}")),
    }
}

fn build_operation(node: &FixtureNode) -> Result<Operation, String> {
    let node_type =
        resolve_standard_node_type(&node.node_type).map_err(|error| error.to_string())?;
    let mut properties = Vec::with_capacity(node.properties.len());
    for (name, value) in &node.properties {
        properties.push((
            resolve_standard_property(name).map_err(|error| error.to_string())?,
            json_to_value(value)?,
        ));
    }
    Ok(Operation::create_node(
        NodeId::new(node.id),
        node_type,
        node.parent.map(NodeId::new),
        None,
        properties,
    ))
}

pub(crate) fn build_transactions(fixture: &Fixture) -> Result<[Transaction; 2], String> {
    let first_count = fixture.first_paint_node_count;
    if first_count == 0 {
        return Err("first_paint_node_count must be greater than zero".to_string());
    }
    if first_count >= fixture.nodes.len() {
        return Err(format!(
            "first_paint_node_count ({first_count}) must be less than fixture node count ({})",
            fixture.nodes.len()
        ));
    }

    let mut seen_ids = BTreeSet::new();
    let mut first_operations = Vec::with_capacity(first_count);
    let mut remaining_operations = Vec::with_capacity(fixture.nodes.len() - first_count);
    for (index, node) in fixture.nodes.iter().enumerate() {
        if seen_ids.contains(&node.id) {
            return Err(format!("fixture contains duplicate node id {}", node.id));
        }
        if let Some(parent) = node.parent {
            if !seen_ids.contains(&parent) {
                return Err(format!(
                    "fixture node {} references parent {parent} before it is created",
                    node.id
                ));
            }
        }
        seen_ids.insert(node.id);

        let operation = build_operation(node)?;
        if index < first_count {
            first_operations.push(operation);
        } else {
            remaining_operations.push(operation);
        }
    }

    Ok([
        Transaction::new(Revision::INITIAL, first_operations),
        Transaction::new(Revision::new(1), remaining_operations),
    ])
}

pub(crate) fn encode_transactions(transactions: &[Transaction; 2]) -> [Vec<u8>; 2] {
    [
        transactions[0].to_wire_bytes(),
        transactions[1].to_wire_bytes(),
    ]
}

pub(crate) fn frame_encoded_transactions(encoded: &[Vec<u8>; 2]) -> Result<Vec<u8>, String> {
    let capacity = encoded.iter().try_fold(16_usize, |total, bytes| {
        total
            .checked_add(bytes.len())
            .ok_or_else(|| "canonical transaction artifact length overflow".to_string())
    })?;
    let mut artifact = Vec::with_capacity(capacity);
    for bytes in encoded {
        let length = u64::try_from(bytes.len())
            .map_err(|_| "transaction wire length does not fit in u64".to_string())?;
        artifact.extend_from_slice(&length.to_be_bytes());
        artifact.extend_from_slice(bytes);
    }
    Ok(artifact)
}

pub(crate) fn serialization(fixture: &Fixture, iterations: usize) -> Result<Section, String> {
    let mut generation_ms = Vec::with_capacity(iterations);
    let mut serialization_ms = Vec::with_capacity(iterations);
    let mut bytes = 0;
    for _ in 0..iterations {
        let start = Instant::now();
        let transactions = build_transactions(fixture)?;
        generation_ms.push(start.elapsed().as_secs_f64() * 1_000.0);

        let start = Instant::now();
        let encoded = encode_transactions(&transactions);
        serialization_ms.push(start.elapsed().as_secs_f64() * 1_000.0);

        let artifact = frame_encoded_transactions(&encoded)?;
        bytes = artifact.len();
        std::hint::black_box((encoded, artifact));
    }
    let generation_samples = generation_ms.len();
    let serialization_samples = serialization_ms.len();
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
            id: "fixture_protobuf_valid",
            name: "shared fixture produced SRUI protobuf",
            passed: bytes > 16,
            detail: format!(
                "{} nodes split {} + {} across revisions 0→1→2 and encoded into {bytes} framed bytes",
                fixture.nodes.len(),
                fixture.first_paint_node_count,
                fixture.nodes.len() - fixture.first_paint_node_count
            ),
        }],
        notes: vec![
            "Generation constructs the same progressive two-transaction abstract UI plan used by the renderer benchmark.".into(),
            "Serialization timing covers only the two production Transaction::to_wire_bytes calls. Deterministic u64 big-endian artifact framing/copying happens after the timer; PTY spawn and renderer work are excluded.".into(),
        ],
        sample_counts: BTreeMap::from([
            ("rust.generation", generation_samples),
            ("rust.serialization", serialization_samples),
        ]),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: u64, parent: Option<u64>, node_type: &str) -> FixtureNode {
        FixtureNode {
            id,
            node_type: node_type.to_string(),
            parent,
            properties: BTreeMap::new(),
        }
    }

    fn progressive_fixture(first_paint_node_count: usize) -> Fixture {
        Fixture {
            first_paint_node_count,
            nodes: vec![
                node(1, None, "Surface"),
                node(2, Some(1), "Column"),
                node(3, Some(2), "Text"),
            ],
        }
    }

    #[test]
    fn builds_two_revision_ordered_transactions_at_fixture_boundary() {
        let transactions = build_transactions(&progressive_fixture(2)).unwrap();

        assert_eq!(transactions[0].base_revision, Revision::INITIAL);
        assert_eq!(transactions[0].new_revision, Revision::new(1));
        assert_eq!(transactions[0].operations.len(), 2);
        assert_eq!(transactions[1].base_revision, Revision::new(1));
        assert_eq!(transactions[1].new_revision, Revision::new(2));
        assert_eq!(transactions[1].operations.len(), 1);
    }

    #[test]
    fn canonical_artifact_length_prefixes_both_wire_transactions() {
        let transactions = build_transactions(&progressive_fixture(2)).unwrap();
        let expected_first_wire = transactions[0].to_wire_bytes();
        let expected_second_wire = transactions[1].to_wire_bytes();
        let encoded = encode_transactions(&transactions);
        assert_eq!(encoded[0], expected_first_wire);
        assert_eq!(encoded[1], expected_second_wire);
        let artifact = frame_encoded_transactions(&encoded).unwrap();
        let first_wire = &encoded[0];
        let second_wire = &encoded[1];
        let first_length = u64::from_be_bytes(artifact[0..8].try_into().unwrap()) as usize;
        assert_eq!(first_length, first_wire.len());
        assert_eq!(&artifact[8..8 + first_length], first_wire.as_slice());

        let second_offset = 8 + first_length;
        let second_length = u64::from_be_bytes(
            artifact[second_offset..second_offset + 8]
                .try_into()
                .unwrap(),
        ) as usize;
        assert_eq!(second_length, second_wire.len());
        assert_eq!(
            &artifact[second_offset + 8..second_offset + 8 + second_length],
            second_wire.as_slice()
        );
        assert_eq!(artifact.len(), 16 + first_wire.len() + second_wire.len());
    }

    #[test]
    fn rejects_empty_or_complete_first_paint_prefix() {
        let zero = build_transactions(&progressive_fixture(0)).unwrap_err();
        assert!(zero.contains("greater than zero"));

        let complete = build_transactions(&progressive_fixture(3)).unwrap_err();
        assert!(complete.contains("must be less than fixture node count"));
    }

    #[test]
    fn rejects_parent_that_has_not_already_been_created() {
        let fixture = Fixture {
            first_paint_node_count: 1,
            nodes: vec![node(1, Some(2), "Surface"), node(2, None, "Column")],
        };

        let error = build_transactions(&fixture).unwrap_err();
        assert!(error.contains("references parent 2 before it is created"));
    }
}
