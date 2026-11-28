//! Store and decode rules for the standard `Split` container (PX-070-G02).
//!
//! Implements: §4 inv. 13 (an unknown `split_axis` value and `split_axis` on any other node type
//! fail explicitly), §7.3 (Split, SHOULD tier), §12.1 (a refused operation discards its
//! transaction) and §26 (at most `MAX_SPLIT_PANES` panes; nesting counts toward
//! `max_tree_depth`). Suites 1, 2 and 9 of §32 run this file; the cross-language parity vectors
//! are suite 1 vectors 95–103 and the malformed vectors `malformed_split_*.bin`.

use srui_protocol::framing::decode_framed;
use srui_semantic_tree::*;
use std::path::Path;

const SURFACE: NodeId = NodeId::new(1);
const SPLIT: NodeId = NodeId::new(2);

fn axis(token: EnumToken) -> (PropertyRef, Value) {
    (PropertyRef::SPLIT_AXIS, Value::EnumToken(token))
}

fn split_store(properties: Vec<(PropertyRef, Value)>) -> SemanticStore {
    let mut store = SemanticStore::new();
    store
        .create_node(SURFACE, TypeRef::SURFACE, None, None, [])
        .expect("surface");
    store
        .create_node(SPLIT, TypeRef::SPLIT, Some(SURFACE), None, properties)
        .expect("split");
    store
}

fn add_panes(store: &mut SemanticStore, parent: NodeId, first_id: u64, count: u64) {
    for id in first_id..first_id + count {
        store
            .create_node(
                NodeId::new(id),
                TypeRef::TEXT,
                Some(parent),
                None,
                [(PropertyRef::TEXT, Value::String(format!("pane {id}")))],
            )
            .expect("pane within the limit");
    }
}

fn code(result: Result<impl std::fmt::Debug, StoreError>) -> &'static str {
    result
        .expect_err("operation must be refused")
        .conformance_code()
}

#[test]
fn a_split_accepts_one_through_eight_panes_on_either_axis() {
    for axis_value in [
        None,
        Some(EnumToken::SPLIT_AXIS_SIDE_BY_SIDE),
        Some(EnumToken::SPLIT_AXIS_STACKED),
    ] {
        for panes in 1..=MAX_SPLIT_PANES as u64 {
            let mut store = split_store(axis_value.map(axis).into_iter().collect());
            add_panes(&mut store, SPLIT, 10, panes);
            assert_eq!(
                store.children_of(SPLIT).unwrap().len() as u64,
                panes,
                "{axis_value:?}"
            );
        }
    }
    assert_eq!(MAX_SPLIT_PANES, 8);
}

#[test]
fn the_axis_defaults_to_side_by_side_and_parses_only_splitaxis_values() {
    assert_eq!(
        effective_split_axis(None),
        Some(StandardSplitAxis::SideBySide)
    );
    assert_eq!(
        effective_split_axis(Some(&Value::EnumToken(EnumToken::SPLIT_AXIS_STACKED))),
        Some(StandardSplitAxis::Stacked)
    );
    assert_eq!(
        parse_split_axis(&Value::EnumToken(EnumToken::new(13, 3))),
        Err(SplitAxisError::UnknownValue {
            enum_id: 13,
            value_id: 3
        })
    );
    assert_eq!(
        parse_split_axis(&Value::String("stacked".into())),
        Err(SplitAxisError::NotAnEnumValue)
    );
}

#[test]
fn a_ninth_pane_is_refused_on_create_and_rolls_back_the_transaction() {
    let mut store = split_store(vec![]);
    add_panes(&mut store, SPLIT, 10, 8);
    let base = store.revision();
    let err = store
        .apply_transaction(
            base,
            vec![
                Operation::set_property(SPLIT, PropertyRef::LABEL, "Dashboard"),
                Operation::create_node(NodeId::new(30), TypeRef::TEXT, Some(SURFACE), None, []),
                Operation::create_node(NodeId::new(31), TypeRef::TEXT, Some(SPLIT), None, []),
            ],
        )
        .expect_err("a ninth pane is refused");
    match &err {
        TxnError::OpFailed { op_index, source } => {
            assert_eq!(*op_index, 2);
            assert_eq!(
                *source,
                StoreError::MaxSplitPanesExceeded {
                    split_id: SPLIT,
                    limit: MAX_SPLIT_PANES
                }
            );
            assert_eq!(source.conformance_code(), "max_split_panes_exceeded");
        }
        other => panic!("expected OpFailed, got {other:?}"),
    }
    // Nothing of the transaction is observable (§12.1).
    assert_eq!(store.revision(), base);
    assert!(!store.contains_node(NodeId::new(30)));
    assert!(!store.is_id_used(NodeId::new(31)));
    assert!(!store
        .get_node(SPLIT)
        .unwrap()
        .has_property(PropertyRef::LABEL));
    assert_eq!(store.children_of(SPLIT).unwrap().len(), 8);

    // An explicit child_index does not get round the limit either.
    assert_eq!(
        code(store.create_node(NodeId::new(32), TypeRef::TEXT, Some(SPLIT), Some(0), [])),
        "max_split_panes_exceeded"
    );
}

#[test]
fn the_limit_applies_to_each_split_and_frees_up_after_a_delete() {
    let mut store = split_store(vec![]);
    add_panes(&mut store, SPLIT, 10, 8);
    // A nested Split is one pane of its parent and holds its own eight panes.
    store.delete_node(NodeId::new(17)).unwrap();
    store
        .create_node(NodeId::new(40), TypeRef::SPLIT, Some(SPLIT), None, [])
        .expect("the deleted pane's slot is free again");
    add_panes(&mut store, NodeId::new(40), 41, 8);
    assert_eq!(
        code(store.create_node(
            NodeId::new(49),
            TypeRef::TEXT,
            Some(NodeId::new(40)),
            None,
            []
        )),
        "max_split_panes_exceeded"
    );
    // Other containers are unaffected by the Split limit.
    add_panes(&mut store, SURFACE, 60, 12);
}

#[test]
fn moving_a_ninth_pane_in_is_refused_but_moving_within_the_split_is_not() {
    let mut store = split_store(vec![]);
    add_panes(&mut store, SPLIT, 10, 8);
    store
        .create_node(NodeId::new(30), TypeRef::TEXT, Some(SURFACE), None, [])
        .unwrap();
    assert_eq!(
        code(store.move_node(NodeId::new(30), Some(SPLIT), None)),
        "max_split_panes_exceeded"
    );
    assert_eq!(store.parent_of(NodeId::new(30)), Some(Some(SURFACE)));
    // A pane moving inside its own full Split keeps the count at eight.
    store
        .move_node(NodeId::new(17), Some(SPLIT), Some(0))
        .expect("reordering within the split adds no pane");
    assert_eq!(store.children_of(SPLIT).unwrap()[0], NodeId::new(17));
    store
        .reorder_children(SPLIT, &(10..18).rev().map(NodeId::new).collect::<Vec<_>>())
        .expect("REORDER_CHILDREN adds no pane");
    // Moving a pane out frees a slot for the moved node.
    store
        .move_node(NodeId::new(10), Some(SURFACE), None)
        .unwrap();
    store
        .move_node(NodeId::new(30), Some(SPLIT), None)
        .expect("seven panes admit an eighth");
}

#[test]
fn split_axis_is_admitted_on_split_and_refused_on_every_other_node_type() {
    for &(id, name) in STANDARD_NODE_TYPES {
        let node_type = TypeRef::standard(id);
        let stacked = axis(EnumToken::SPLIT_AXIS_STACKED);
        let node = NodeId::new(5);

        // CREATE_NODE
        let mut store = split_store(vec![]);
        let created = store.create_node(node, node_type, Some(SURFACE), None, [stacked.clone()]);
        // SET_PROPERTY and BATCH_PROPERTY_SET on an existing node of the type
        let mut set_store = split_store(vec![]);
        set_store
            .create_node(node, node_type, Some(SURFACE), None, [])
            .unwrap();
        let set = set_store.set_property(node, PropertyRef::SPLIT_AXIS, stacked.1.clone());
        let batch = set_store.batch_property_set(node, [stacked.clone()]);

        if node_type == TypeRef::SPLIT {
            created.unwrap_or_else(|e| panic!("{name}: {e}"));
            set.unwrap_or_else(|e| panic!("{name}: {e}"));
            batch.unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(
                set_store
                    .get_node(node)
                    .unwrap()
                    .get_property(PropertyRef::SPLIT_AXIS),
                Some(&stacked.1)
            );
        } else {
            for (path, result) in [
                ("create", created.map(|_| ())),
                ("set", set.map(|_| ())),
                ("batch", batch),
            ] {
                let err = result.expect_err(&format!("{name} {path} must refuse split_axis"));
                assert_eq!(
                    err,
                    StoreError::SplitAxisNotAdmitted {
                        node_id: node,
                        node_type
                    },
                    "{name} {path}"
                );
                assert_eq!(err.conformance_code(), "split_axis_not_admitted");
            }
            assert!(!store.contains_node(node), "{name}: no partial create");
            assert!(!set_store
                .get_node(node)
                .unwrap()
                .has_property(PropertyRef::SPLIT_AXIS));
        }
    }
}

#[test]
fn a_value_outside_splitaxis_is_refused_at_apply() {
    for bad in [
        Value::EnumToken(EnumToken::new(13, 0)),
        Value::EnumToken(EnumToken::new(13, 3)),
        Value::EnumToken(EnumToken::VISIBILITY_HIDDEN),
        Value::String("stacked".into()),
        Value::UnsignedInt(2),
        Value::Null,
    ] {
        let mut store = split_store(vec![]);
        assert_eq!(
            code(store.set_property(SPLIT, PropertyRef::SPLIT_AXIS, bad.clone())),
            "invalid_split_axis",
            "{bad:?}"
        );
        assert_eq!(
            code(store.batch_property_set(SPLIT, [(PropertyRef::SPLIT_AXIS, bad.clone())])),
            "invalid_split_axis"
        );
        assert_eq!(
            code(store.create_node(
                NodeId::new(9),
                TypeRef::SPLIT,
                Some(SURFACE),
                None,
                [(PropertyRef::SPLIT_AXIS, bad.clone())]
            )),
            "invalid_split_axis"
        );
        // The value is checked before admission, as for shortcut: a Button gets the same code.
        store
            .create_node(NodeId::new(10), TypeRef::BUTTON, Some(SURFACE), None, [])
            .unwrap();
        assert_eq!(
            code(store.set_property(NodeId::new(10), PropertyRef::SPLIT_AXIS, bad)),
            "invalid_split_axis"
        );
    }
    // Clearing it is always allowed and restores the side_by_side default.
    let mut store = split_store(vec![axis(EnumToken::SPLIT_AXIS_STACKED)]);
    store
        .clear_property(SPLIT, PropertyRef::SPLIT_AXIS)
        .expect("clear");
    assert_eq!(
        effective_split_axis(
            store
                .get_node(SPLIT)
                .unwrap()
                .get_property(PropertyRef::SPLIT_AXIS)
        ),
        Some(StandardSplitAxis::SideBySide)
    );
}

#[test]
fn nested_splits_count_toward_the_tree_depth_limit() {
    let limits = StoreLimits::default().with_max_tree_depth(4);
    let mut store = SemanticStore::with_limits(limits);
    store
        .create_node(SURFACE, TypeRef::SURFACE, None, None, [])
        .unwrap();
    store
        .create_node(NodeId::new(2), TypeRef::SPLIT, Some(SURFACE), None, [])
        .unwrap();
    store
        .create_node(
            NodeId::new(3),
            TypeRef::SPLIT,
            Some(NodeId::new(2)),
            None,
            [],
        )
        .unwrap();
    store
        .create_node(
            NodeId::new(4),
            TypeRef::TEXT,
            Some(NodeId::new(3)),
            None,
            [],
        )
        .expect("depth 4 is the limit");
    assert_eq!(
        code(store.create_node(
            NodeId::new(5),
            TypeRef::SPLIT,
            Some(NodeId::new(4)),
            None,
            []
        )),
        "max_tree_depth_exceeded"
    );
}

fn wire_set_split_axis(value: Value) -> srui_protocol::Operation {
    Operation::set_property(SPLIT, PropertyRef::SPLIT_AXIS, value).to_wire()
}

#[test]
fn every_decode_path_refuses_a_value_outside_splitaxis() {
    for bad in [
        Value::EnumToken(EnumToken::new(13, 3)),
        Value::EnumToken(EnumToken::new(6, 1)),
        Value::String("stacked".into()),
    ] {
        // SET_PROPERTY
        let err = Operation::try_from(wire_set_split_axis(bad.clone())).expect_err("refused");
        assert!(
            matches!(err, TxnError::WireError(ref m) if m.starts_with("invalid split_axis: ")),
            "{err:?}"
        );
        // BATCH_PROPERTY_SET, CREATE_NODE and NodeRecord share Property::try_from
        let property =
            srui_protocol::Property::from(Property::new(PropertyRef::SPLIT_AXIS, bad.clone()));
        assert!(matches!(
            Property::try_from(property),
            Err(ValueConversionError::InvalidSplitAxis(_))
        ));
        let batch = Operation::batch_property_set(SPLIT, [(PropertyRef::SPLIT_AXIS, bad.clone())]);
        assert!(Operation::try_from(batch.to_wire()).is_err());
        let create = Operation::create_node(
            SPLIT,
            TypeRef::SPLIT,
            Some(SURFACE),
            None,
            [(PropertyRef::SPLIT_AXIS, bad.clone())],
        );
        assert!(Operation::try_from(create.to_wire()).is_err());
        let record = NodeRecord::new(
            SPLIT,
            TypeRef::SPLIT,
            Some(SURFACE),
            None,
            [Property::new(PropertyRef::SPLIT_AXIS, bad)],
        );
        assert!(NodeRecord::from_wire(record.to_wire()).is_err());
    }
    // Both canonical values decode unchanged.
    for good in [
        EnumToken::SPLIT_AXIS_SIDE_BY_SIDE,
        EnumToken::SPLIT_AXIS_STACKED,
    ] {
        let decoded = Operation::try_from(wire_set_split_axis(Value::EnumToken(good))).unwrap();
        assert_eq!(
            decoded,
            Operation::set_property(SPLIT, PropertyRef::SPLIT_AXIS, Value::EnumToken(good))
        );
    }
}

fn malformed_transaction(file: &str) -> srui_protocol::Transaction {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../protocol/conformance-vectors");
    let bytes = std::fs::read(root.join(file)).unwrap_or_else(|e| panic!("{file}: {e}"));
    let message: srui_protocol::SruiMessage = decode_framed(&bytes).expect("protobuf-valid");
    let Some(srui_protocol::srui_message::Msg::Transaction(wire)) = message.msg else {
        panic!("{file} must hold a transaction");
    };
    wire
}

/// The committed malformed vectors (suite 9): an unknown axis value is refused when the framed
/// transaction is decoded; a ninth pane decodes (a stateless decoder cannot know the parent's
/// type) and is refused when the decoded transaction is applied, with nothing committed.
#[test]
fn the_malformed_split_vectors_are_refused() {
    let unknown = malformed_transaction("malformed_split_axis_unknown_value.bin");
    let err = Transaction::try_from(unknown).expect_err("unknown axis value");
    assert_eq!(
        err.to_string(),
        "wire transaction error: invalid split_axis: split_axis must be SplitAxis side_by_side (13:1) or stacked (13:2), got enum 13 value 3"
    );

    let ninth = malformed_transaction("malformed_split_ninth_pane.bin");
    let txn = Transaction::try_from(ninth.clone()).expect("structurally valid: decodes");
    assert_eq!(txn.operations.len(), 11);
    let mut store = SemanticStore::new();
    let err = store
        .apply_wire_transaction(ninth)
        .expect_err("the ninth pane is refused at apply");
    assert_eq!(err.conformance_code(), Some("max_split_panes_exceeded"));
    assert!(
        matches!(err, TxnError::OpFailed { op_index: 10, .. }),
        "{err:?}"
    );
    assert_eq!(
        err.to_string(),
        "operation at index 10 failed: split NodeId(2) already holds 8 panes, the most a Split admits (§26)"
    );
    assert!(store.is_empty());
    assert_eq!(store.revision(), Revision(0));
}
