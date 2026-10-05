//! §32 suite 8 (reconnect): multi-envelope snapshot delivery vectors.
//!
//! Covers the PX-004-G01 multi-envelope snapshot extension documented in `protocol/README.md`.
//! Grounded in: §12.1 (an incomplete transaction is discarded), §18 (the single-envelope snapshot
//! form the extension splits), §26 (frame and operation bounds), §32.8.
//!
//! Each vector in `protocol/conformance-vectors/suites/08-reconnect/vectors` gives the client's
//! limits, the snapshot a continuity decision announced, and the envelopes that followed. The
//! replica stages them with `srui_protocol::SnapshotAssembler` and applies the reassembled snapshot
//! through the dedicated §18 entry point. `SnapshotFramingConformanceTests` in the Swift client
//! replays the same files, so both replicas are held to one oracle.

mod common;

use serde_json::Value as Json;
use srui_protocol::{
    operation, value, CreateModelOp, CreateNodeOp, ModelResetRangeOp, NodeRecord, Operation,
    Property, SnapshotAssembler, SnapshotAssemblyError, Transaction as WireTransaction,
};
use srui_semantic_tree::{
    ItemId, ModelId, NodeId, PropertyRef, ResyncSnapshot, Revision, SemanticStore, StoreLimits,
    TypeRef, Value,
};

const SUITE: u32 = 8;

fn u(json: &Json, key: &str) -> u64 {
    json[key]
        .as_u64()
        .unwrap_or_else(|| panic!("missing integer {key} in {json}"))
}

fn wire_string(text: &str) -> srui_protocol::Value {
    srui_protocol::Value {
        value: Some(value::Value::StringValue(text.to_string())),
    }
}

fn standard_type(local_id: u64) -> srui_protocol::TypeRef {
    srui_protocol::TypeRef {
        namespace_id: 0,
        local_id: local_id as u32,
    }
}

fn wire_operation(json: &Json) -> Operation {
    let op = if let Some(node) = json.get("create_node") {
        operation::Op::CreateNode(CreateNodeOp {
            node: Some(NodeRecord {
                node_id: u(node, "node_id"),
                r#type: Some(standard_type(u(node, "type"))),
                parent_id: u(node, "parent_id"),
                child_index: u(node, "child_index") as u32,
                properties: node["properties"]
                    .as_array()
                    .expect("properties")
                    .iter()
                    .map(|p| Property {
                        property: Some(srui_protocol::PropertyRef {
                            namespace_id: 0,
                            local_id: u(p, "property") as u32,
                        }),
                        value: Some(wire_string(p["string"].as_str().expect("string"))),
                    })
                    .collect(),
            }),
        })
    } else if let Some(model) = json.get("create_model") {
        operation::Op::CreateModel(CreateModelOp {
            model_id: u(model, "model_id"),
            model_type: Some(standard_type(u(model, "model_type"))),
            item_count: u(model, "item_count"),
        })
    } else if let Some(range) = json.get("model_reset_range") {
        operation::Op::ModelResetRange(ModelResetRangeOp {
            model_id: u(range, "model_id"),
            start_index: u(range, "start_index"),
            items: range["items"]
                .as_array()
                .expect("items")
                .iter()
                .map(|item| srui_protocol::ModelItem {
                    item_id: u(item, "item_id"),
                    value: Some(wire_string(item["string"].as_str().expect("string"))),
                    ..Default::default()
                })
                .collect(),
            total_count: 0,
        })
    } else {
        panic!("unknown vector operation {json}");
    };
    Operation { op: Some(op) }
}

fn wire_envelope(json: &Json) -> WireTransaction {
    WireTransaction {
        base_revision: u(json, "base_revision"),
        new_revision: u(json, "new_revision"),
        priority: 0,
        operations: json["operations"]
            .as_array()
            .expect("operations")
            .iter()
            .map(wire_operation)
            .collect(),
    }
}

fn error_code(error: &SnapshotAssemblyError) -> &'static str {
    match error {
        SnapshotAssemblyError::PartsExceedLimit { .. } => "parts_exceed_limit",
        SnapshotAssemblyError::NotASnapshotPart { .. } => "not_a_snapshot_part",
        SnapshotAssemblyError::EmptyPart { .. } => "empty_part",
        SnapshotAssemblyError::OperationLimitExceeded { .. } => "operation_limit_exceeded",
        SnapshotAssemblyError::AlreadyComplete => "already_complete",
    }
}

/// What a replica observed: the applied store, or why and where staging stopped.
enum Observed {
    Applied(Box<SemanticStore>),
    Rejected {
        code: &'static str,
        at_envelope: Option<usize>,
    },
    Incomplete {
        received: usize,
    },
}

fn replay(vector: &Json) -> Observed {
    let limits = &vector["client_limits"];
    let decision = &vector["decision"];
    let max_operations = u(limits, "max_transaction_operations") as usize;
    let mut assembler = match SnapshotAssembler::new(
        u(decision, "snapshot_revision"),
        u(decision, "snapshot_parts") as u32,
        u(limits, "max_snapshot_parts") as u32,
        max_operations,
    ) {
        Ok(assembler) => assembler,
        Err(error) => {
            return Observed::Rejected {
                code: error_code(&error),
                at_envelope: None,
            }
        }
    };

    let envelopes = vector["envelopes"].as_array().expect("envelopes");
    for (index, envelope) in envelopes.iter().enumerate() {
        match assembler.accept(wire_envelope(envelope)) {
            Ok(None) => {}
            Ok(Some(snapshot)) => {
                assert_eq!(
                    index + 1,
                    envelopes.len(),
                    "the snapshot completed before its last envelope"
                );
                let snapshot = ResyncSnapshot::try_from(snapshot).expect("snapshot decodes");
                let mut replica = SemanticStore::with_limits_and_revision(
                    StoreLimits {
                        max_transaction_operations: max_operations,
                        ..StoreLimits::default()
                    },
                    Revision::INITIAL,
                );
                replica
                    .replace_from_snapshot(&snapshot)
                    .expect("snapshot applies");
                return Observed::Applied(Box::new(replica));
            }
            Err(error) => {
                return Observed::Rejected {
                    code: error_code(&error),
                    at_envelope: Some(index),
                }
            }
        }
    }
    Observed::Incomplete {
        received: envelopes.len(),
    }
}

fn string_props(json: &Json) -> Vec<(PropertyRef, Value)> {
    let mut props: Vec<_> = json
        .as_array()
        .expect("properties")
        .iter()
        .map(|p| {
            (
                PropertyRef::new(0, u(p, "property") as u32),
                Value::String(p["string"].as_str().expect("string").to_string()),
            )
        })
        .collect();
    props.sort_by_key(|(p, _)| (p.namespace_id, p.local_id));
    props
}

fn assert_store(name: &str, store: &SemanticStore, expected: &Json) {
    assert_eq!(
        store.revision().get(),
        u(expected, "revision"),
        "{name}: revision"
    );
    let roots: Vec<u64> = store.root_ids().iter().map(|id| id.get()).collect();
    let want_roots: Vec<u64> = expected["root_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap())
        .collect();
    assert_eq!(roots, want_roots, "{name}: root order");

    let nodes = expected["nodes"].as_array().unwrap();
    assert_eq!(store.node_count(), nodes.len(), "{name}: node count");
    for want in nodes {
        let id = NodeId::new(u(want, "node_id"));
        let node = store
            .get_node(id)
            .unwrap_or_else(|| panic!("{name}: node {id:?}"));
        assert_eq!(
            node.node_type,
            TypeRef::new(0, u(want, "type") as u32),
            "{name}"
        );
        assert_eq!(
            node.parent_id.map_or(0, |p| p.get()),
            u(want, "parent_id"),
            "{name}: parent of {id:?}"
        );
        let children: Vec<u64> = node.ordered_children.iter().map(|c| c.get()).collect();
        let want_children: Vec<u64> = want["children"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap())
            .collect();
        assert_eq!(children, want_children, "{name}: children of {id:?}");
        let mut props: Vec<_> = node
            .properties
            .iter()
            .map(|(p, v)| (*p, v.clone()))
            .collect();
        props.sort_by_key(|(p, _)| (p.namespace_id, p.local_id));
        assert_eq!(
            props,
            string_props(&want["properties"]),
            "{name}: properties"
        );
    }

    let models = expected["models"].as_array().unwrap();
    assert_eq!(store.model_count(), models.len(), "{name}: model count");
    for want in models {
        let model = store
            .get_model(ModelId::new(u(want, "model_id")))
            .unwrap_or_else(|| panic!("{name}: model"));
        assert_eq!(
            model.model_type,
            TypeRef::new(0, u(want, "model_type") as u32)
        );
        assert_eq!(
            model.item_count,
            u(want, "item_count"),
            "{name}: item_count"
        );
        let items = want["items"].as_array().unwrap();
        assert_eq!(
            model.cached_item_count(),
            items.len(),
            "{name}: cached items"
        );
        for item in items {
            let got = model
                .get_item_by_index(u(item, "index"))
                .unwrap_or_else(|| panic!("{name}: item {item}"));
            assert_eq!(got.item_id, ItemId::new(u(item, "item_id")), "{name}");
            assert_eq!(
                got.value,
                Value::String(item["string"].as_str().unwrap().to_string()),
                "{name}"
            );
        }
    }
}

#[test]
fn suite_8_snapshot_framing_vectors() {
    let files = common::suite_vectors(SUITE);
    for path in &files {
        let vector: Json =
            serde_json::from_str(&std::fs::read_to_string(path).expect("read vector"))
                .expect("parse vector");
        let name = vector["name"].as_str().expect("name").to_string();
        let expected = &vector["expected_outcome"];
        match (
            expected["status"].as_str().expect("status"),
            replay(&vector),
        ) {
            ("applied", Observed::Applied(store)) => assert_store(&name, &store, expected),
            ("rejected", Observed::Rejected { code, at_envelope }) => {
                assert_eq!(code, expected["error"].as_str().unwrap(), "{name}: error");
                assert_eq!(
                    at_envelope.map(|i| i as u64),
                    expected["at_envelope"].as_u64(),
                    "{name}: rejected envelope"
                );
            }
            ("incomplete", Observed::Incomplete { received }) => {
                assert_eq!(received as u64, u(expected, "received_envelopes"), "{name}");
            }
            (status, Observed::Applied(_)) => panic!("{name}: expected {status}, snapshot applied"),
            (status, Observed::Rejected { code, at_envelope }) => {
                panic!("{name}: expected {status}, rejected {code} at {at_envelope:?}")
            }
            (status, Observed::Incomplete { received }) => {
                panic!("{name}: expected {status}, incomplete after {received}")
            }
        }
    }
}
