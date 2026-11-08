//! Store and decode rules for the standard `shortcut` property (PX-016-G01).
//!
//! Implements: §4 inv. 13 (malformed values fail explicitly), §7.4 (admission and uniqueness
//! within one Surface), §12.1 (a refused operation discards its transaction), §22.5 (focus-only
//! targets) and §26 (bounded grammar on every decode path). Suites 2, 5, 6 and 9 of §32 run this
//! file; the cross-language parity vectors are suite 1 vectors 54–66.

use srui_protocol::framing::decode_framed;
use srui_semantic_tree::*;
use std::path::Path;

const SURFACE: NodeId = NodeId::new(1);

fn surface_store() -> SemanticStore {
    let mut store = SemanticStore::new();
    store
        .create_node(SURFACE, TypeRef::SURFACE, None, None, [])
        .expect("surface");
    store
}

fn shortcut(value: &str) -> (PropertyRef, Value) {
    (PropertyRef::SHORTCUT, Value::String(value.to_string()))
}

fn code(result: Result<impl std::fmt::Debug, StoreError>) -> &'static str {
    result
        .expect_err("operation must be refused")
        .conformance_code()
}

#[test]
fn every_admitted_type_accepts_a_shortcut_and_every_other_type_refuses_it() {
    let admitted = [
        (TypeRef::BUTTON, ShortcutBehavior::Activate),
        (TypeRef::TOGGLE, ShortcutBehavior::Toggle),
        (TypeRef::TEXT_INPUT, ShortcutBehavior::Focus),
        (TypeRef::TEXT_AREA, ShortcutBehavior::Focus),
        (TypeRef::LIST, ShortcutBehavior::Focus),
        (TypeRef::TABLE, ShortcutBehavior::Focus),
        (TypeRef::TREE, ShortcutBehavior::Focus),
        (TypeRef::RICH_TEXT, ShortcutBehavior::Focus),
    ];
    let mut store = surface_store();
    for (index, &(node_type, behavior)) in admitted.iter().enumerate() {
        assert_eq!(shortcut_behavior(node_type), Some(behavior));
        let id = NodeId::new(10 + index as u64);
        let value = format!("primary+{index}");
        store
            .create_node(id, node_type, Some(SURFACE), None, [shortcut(&value)])
            .unwrap_or_else(|e| panic!("{node_type} must admit a shortcut: {e}"));
    }

    for &(id, name) in STANDARD_NODE_TYPES {
        let node_type = TypeRef::standard(id);
        if admitted.iter().any(|(t, _)| *t == node_type) {
            continue;
        }
        assert_eq!(shortcut_behavior(node_type), None, "{name}");
        let node_id = NodeId::new(100 + u64::from(id));
        let mut store = surface_store();
        let created = store.create_node(
            node_id,
            node_type,
            Some(SURFACE),
            None,
            [shortcut("primary+r")],
        );
        assert_eq!(code(created), "shortcut_not_admitted", "{name}");
        // Neither may a later SET_PROPERTY or BATCH_PROPERTY_SET add one.
        store
            .create_node(node_id, node_type, Some(SURFACE), None, [])
            .unwrap_or_else(|e| panic!("{name} without a shortcut: {e}"));
        let (p, v) = shortcut("primary+r");
        assert_eq!(
            code(store.set_property(node_id, p, v.clone())),
            "shortcut_not_admitted"
        );
        assert_eq!(
            code(store.batch_property_set(node_id, [(p, v)])),
            "shortcut_not_admitted"
        );
    }
}

#[test]
fn malformed_values_are_refused_at_apply_on_every_setting_path() {
    let mut store = surface_store();
    let button = NodeId::new(2);
    store
        .create_node(button, TypeRef::BUTTON, Some(SURFACE), None, [])
        .unwrap();
    for bad in [
        Value::String("r".into()),
        Value::String("cmd+r".into()),
        Value::String("primary+primary+r".into()),
        Value::String("shift+primary+r".into()),
        Value::String("primary+shift+alt+page_down+xxxxx".into()),
        Value::String(String::new()),
        Value::Bool(true),
        Value::Null,
    ] {
        assert_eq!(
            code(store.set_property(button, PropertyRef::SHORTCUT, bad.clone())),
            "invalid_shortcut",
            "{bad:?}"
        );
        assert_eq!(
            code(store.batch_property_set(button, [(PropertyRef::SHORTCUT, bad.clone())])),
            "invalid_shortcut"
        );
        assert_eq!(
            code(store.create_node(
                NodeId::new(3),
                TypeRef::BUTTON,
                Some(SURFACE),
                None,
                [(PropertyRef::SHORTCUT, bad)]
            )),
            "invalid_shortcut"
        );
    }
    assert!(store
        .get_node(button)
        .unwrap()
        .get_property(PropertyRef::SHORTCUT)
        .is_none());
    assert!(!store.contains_node(NodeId::new(3)));
}

#[test]
fn a_bare_printable_key_never_binds_but_a_bare_function_key_does() {
    let mut store = surface_store();
    let field = NodeId::new(2);
    store
        .create_node(field, TypeRef::TEXT_INPUT, Some(SURFACE), None, [])
        .unwrap();
    for bare in [
        "a", "z", "0", "space", "tab", "return", "delete", "up", "shift+a", "alt+a",
    ] {
        let (p, _) = shortcut(bare);
        assert_eq!(
            code(store.set_property(field, p, Value::String(bare.into()))),
            "invalid_shortcut",
            "{bare}"
        );
    }
    store
        .set_property(field, PropertyRef::SHORTCUT, Value::String("f6".into()))
        .expect("a bare function key is admitted");
}

#[test]
fn shortcuts_are_unique_within_one_surface() {
    let mut store = surface_store();
    let (a, b) = (NodeId::new(2), NodeId::new(3));
    store
        .create_node(
            a,
            TypeRef::BUTTON,
            Some(SURFACE),
            None,
            [shortcut("primary+r")],
        )
        .unwrap();
    let column = NodeId::new(4);
    store
        .create_node(column, TypeRef::COLUMN, Some(SURFACE), None, [])
        .unwrap();
    // A descendant at any depth shares the Surface's scope.
    assert_eq!(
        code(store.create_node(
            b,
            TypeRef::TOGGLE,
            Some(column),
            None,
            [shortcut("primary+r")]
        )),
        "duplicate_shortcut"
    );
    store
        .create_node(
            b,
            TypeRef::TOGGLE,
            Some(column),
            None,
            [shortcut("primary+t")],
        )
        .unwrap();
    let (p, v) = shortcut("primary+r");
    assert_eq!(
        code(store.set_property(b, p, v.clone())),
        "duplicate_shortcut"
    );
    assert_eq!(
        code(store.batch_property_set(b, [(p, v.clone())])),
        "duplicate_shortcut"
    );
    // Re-setting a node's own shortcut is not a conflict.
    store
        .set_property(a, p, v.clone())
        .expect("idempotent re-set");
    // Clearing frees the shortcut for another node.
    store.clear_property(a, p).unwrap();
    store
        .set_property(b, p, v)
        .expect("freed by CLEAR_PROPERTY");
}

#[test]
fn surfaces_scope_uniqueness_and_moves_are_checked_against_the_destination() {
    let mut store = surface_store();
    let second = NodeId::new(10);
    store
        .create_node(second, TypeRef::SURFACE, None, None, [])
        .unwrap();
    let a = NodeId::new(2);
    store
        .create_node(
            a,
            TypeRef::BUTTON,
            Some(SURFACE),
            None,
            [shortcut("primary+r")],
        )
        .unwrap();
    let column = NodeId::new(11);
    store
        .create_node(column, TypeRef::COLUMN, Some(second), None, [])
        .unwrap();
    let b = NodeId::new(12);
    store
        .create_node(
            b,
            TypeRef::BUTTON,
            Some(column),
            None,
            [shortcut("primary+r")],
        )
        .expect("another Surface is another scope");

    // A nested Surface is its own scope too.
    let nested = NodeId::new(20);
    store
        .create_node(nested, TypeRef::SURFACE, Some(SURFACE), None, [])
        .unwrap();
    let c = NodeId::new(21);
    store
        .create_node(
            c,
            TypeRef::BUTTON,
            Some(nested),
            None,
            [shortcut("primary+r")],
        )
        .expect("a nested Surface does not share its parent's scope");

    // Moving the subtree carrying primary+r into the first Surface conflicts with node 2.
    let moved = store.move_node(column, Some(SURFACE), None);
    assert!(
        matches!(
            moved,
            Err(StoreError::DuplicateShortcut { node_id, existing, .. }) if node_id == b && existing == a
        ),
        "{moved:?}"
    );
    assert_eq!(
        store.parent_of(column),
        Some(Some(second)),
        "refused move left no trace"
    );

    // Within the same Surface, and to a new root, a move never conflicts.
    let other_column = NodeId::new(13);
    store
        .create_node(other_column, TypeRef::COLUMN, Some(second), None, [])
        .unwrap();
    store.move_node(column, Some(other_column), None).unwrap();
    store.move_node(column, None, None).unwrap();
    // A whole Surface moves with its own scope intact.
    store.move_node(nested, Some(second), None).unwrap();
    // Once node 2 releases primary+r the subtree may join the first Surface.
    store.clear_property(a, PropertyRef::SHORTCUT).unwrap();
    store.move_node(column, Some(SURFACE), None).unwrap();
}

#[test]
fn a_refused_shortcut_discards_the_whole_transaction() {
    let mut store = surface_store();
    let button = NodeId::new(2);
    store
        .create_node(button, TypeRef::BUTTON, Some(SURFACE), None, [])
        .unwrap();
    let base = store.revision();
    let err = store
        .apply_transaction(
            base,
            vec![
                Operation::set_property(button, PropertyRef::LABEL, "Refresh"),
                Operation::set_property(button, PropertyRef::SHORTCUT, "r"),
            ],
        )
        .expect_err("bare key refused");
    assert_eq!(store.revision(), base);
    assert_eq!(err.conformance_code(), Some("invalid_shortcut"));
    assert!(matches!(err, TxnError::OpFailed { op_index: 1, .. }));
    assert!(store
        .get_node(button)
        .unwrap()
        .get_property(PropertyRef::LABEL)
        .is_none());
}

fn wire_set_shortcut(value: &str) -> srui_protocol::Operation {
    Operation::set_property(NodeId::new(2), PropertyRef::SHORTCUT, value).to_wire()
}

#[test]
fn every_decode_path_refuses_a_malformed_shortcut() {
    for bad in [
        "cmd+r",
        "primary+primary+r",
        "primary+shift+alt+page_down+xxxxx",
        "r",
    ] {
        // SET_PROPERTY
        let err = Operation::try_from(wire_set_shortcut(bad)).expect_err(bad);
        assert!(
            matches!(err, TxnError::WireError(ref m) if m.starts_with("invalid shortcut: ")),
            "{err:?}"
        );
        // BATCH_PROPERTY_SET, CREATE_NODE and NodeRecord share Property::try_from
        let property = srui_protocol::Property::from(Property::new(
            PropertyRef::SHORTCUT,
            Value::String(bad.into()),
        ));
        assert!(matches!(
            Property::try_from(property.clone()),
            Err(ValueConversionError::InvalidShortcut(_))
        ));
        let batch = Operation::batch_property_set(
            NodeId::new(2),
            [(PropertyRef::SHORTCUT, Value::String(bad.into()))],
        );
        assert!(Operation::try_from(batch.to_wire()).is_err());
        let record = NodeRecord::new(
            NodeId::new(2),
            TypeRef::BUTTON,
            Some(SURFACE),
            None,
            [Property::new(
                PropertyRef::SHORTCUT,
                Value::String(bad.into()),
            )],
        );
        assert!(NodeRecord::from_wire(record.to_wire()).is_err());
    }
    // A non-string value is refused at decode as well.
    let wrong_type = Operation::set_property(NodeId::new(2), PropertyRef::SHORTCUT, true);
    assert!(Operation::try_from(wrong_type.to_wire()).is_err());
    // A canonical value decodes unchanged.
    let decoded = Operation::try_from(wire_set_shortcut("primary+shift+r")).unwrap();
    assert_eq!(
        decoded,
        Operation::set_property(NodeId::new(2), PropertyRef::SHORTCUT, "primary+shift+r")
    );
}

/// The committed malformed vectors (suite 9) are refused when a framed transaction is decoded.
#[test]
fn the_malformed_shortcut_vectors_are_refused_on_decode() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../protocol/conformance-vectors");
    for (file, expected) in [
        (
            "malformed_shortcut_over_length.bin",
            "wire transaction error: invalid shortcut: a shortcut is at most 32 bytes, got 33",
        ),
        (
            "malformed_shortcut_unknown_token.bin",
            "wire transaction error: invalid shortcut: unknown shortcut token \"cmd\"",
        ),
        (
            "malformed_shortcut_duplicate_modifier.bin",
            "wire transaction error: invalid shortcut: modifier \"primary\" appears more than once",
        ),
    ] {
        let bytes = std::fs::read(root.join(file)).unwrap_or_else(|e| panic!("{file}: {e}"));
        let message: srui_protocol::SruiMessage = decode_framed(&bytes).expect("protobuf-valid");
        let Some(srui_protocol::srui_message::Msg::Transaction(wire)) = message.msg else {
            panic!("{file} must hold a transaction");
        };
        let err = Transaction::try_from(wire).expect_err(file);
        assert_eq!(err.to_string(), expected, "{file}");
    }
}
