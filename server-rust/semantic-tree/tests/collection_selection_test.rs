//! Store rules for a server-promoted collection selection (§6.3, §7.4, §8, §12.1).
//!
//! A `List`, `Table` or `Tree` holds its promoted selection in the standard `value` property.
//! The store enforces, identically on every replica, that the value is an `ItemId` and that it
//! is held only while `selection_mode` is `single` or `multiple`.

use srui_semantic_tree::{
    EnumToken, ItemId, ModelId, ModelItem, NodeId, Operation, PropertyRef, SemanticStore,
    StoreError, TypeRef, Value,
};

const TABLE: NodeId = NodeId::new(2);
const MODEL: ModelId = ModelId::new(7);

fn single() -> Value {
    Value::EnumToken(EnumToken::SELECTION_MODE_SINGLE)
}

fn none() -> Value {
    Value::EnumToken(EnumToken::SELECTION_MODE_NONE)
}

fn store_with_table(node_type: TypeRef, mode: Option<Value>) -> SemanticStore {
    let mut store = SemanticStore::new();
    store.create_model(MODEL, TypeRef::TABLE, 0).expect("model");
    store
        .model_insert(
            MODEL,
            0,
            (1..=3u64)
                .map(|i| ModelItem::with_value(ItemId::new(i), Value::String(format!("row {i}"))))
                .collect(),
        )
        .expect("rows");
    let mut props = vec![(PropertyRef::MODEL_REF, Value::UnsignedInt(MODEL.get()))];
    if let Some(mode) = mode {
        props.push((PropertyRef::SELECTION_MODE, mode));
    }
    store
        .create_node(TABLE, node_type, None, None, props)
        .expect("collection");
    store
}

fn assert_invalid(result: Result<impl std::fmt::Debug, StoreError>) {
    match result {
        Err(err @ StoreError::InvalidCollectionSelection { .. }) => {
            assert_eq!(err.conformance_code(), "invalid_collection_selection");
        }
        other => panic!("expected InvalidCollectionSelection, got {other:?}"),
    }
}

#[test]
fn item_id_value_is_the_promoted_selection_of_every_collection_type() {
    for node_type in [TypeRef::LIST, TypeRef::TABLE, TypeRef::TREE] {
        let mut store = store_with_table(node_type, Some(single()));
        store
            .set_property(TABLE, PropertyRef::VALUE, Value::ItemId(ItemId::new(2)))
            .expect("promotion");
        let node = store.get_node(TABLE).unwrap();
        assert_eq!(node.promoted_selection(), Some(ItemId::new(2)));
    }
}

#[test]
fn promotion_of_an_uncached_item_is_not_a_store_error() {
    // A sparse replica need not cache the promoted item; only the authoritative SDK refuses an
    // unknown item (§8).
    let mut store = store_with_table(TypeRef::TABLE, Some(single()));
    store
        .set_property(TABLE, PropertyRef::VALUE, Value::ItemId(ItemId::new(999)))
        .expect("store keeps cache-independent rules only");
    assert_eq!(
        store.get_node(TABLE).unwrap().promoted_selection(),
        Some(ItemId::new(999))
    );
}

#[test]
fn row_index_or_other_value_is_refused() {
    for value in [
        Value::UnsignedInt(2),
        Value::SignedInt(2),
        Value::String("2".into()),
        Value::List(vec![Value::ItemId(ItemId::new(2))]),
    ] {
        let mut store = store_with_table(TypeRef::TABLE, Some(single()));
        assert_invalid(store.set_property(TABLE, PropertyRef::VALUE, value));
        assert_eq!(store.get_node(TABLE).unwrap().promoted_selection(), None);
    }
}

#[test]
fn selection_mode_none_or_absent_refuses_promotion() {
    for mode in [Some(none()), None] {
        let mut store = store_with_table(TypeRef::TABLE, mode);
        assert_invalid(store.set_property(
            TABLE,
            PropertyRef::VALUE,
            Value::ItemId(ItemId::new(1)),
        ));
        assert!(!store
            .get_node(TABLE)
            .unwrap()
            .has_property(PropertyRef::VALUE));
    }
}

#[test]
fn disabling_selection_while_promoted_is_refused() {
    let mut store = store_with_table(TypeRef::TABLE, Some(single()));
    store
        .set_property(TABLE, PropertyRef::VALUE, Value::ItemId(ItemId::new(1)))
        .expect("promotion");
    assert_invalid(store.set_property(TABLE, PropertyRef::SELECTION_MODE, none()));
    assert_invalid(store.clear_property(TABLE, PropertyRef::SELECTION_MODE));
    assert_eq!(
        store
            .get_node(TABLE)
            .unwrap()
            .get_property(PropertyRef::SELECTION_MODE),
        Some(&single())
    );

    // Withdrawing the promotion first makes the same change legal.
    store
        .clear_property(TABLE, PropertyRef::VALUE)
        .expect("withdraw");
    store
        .set_property(TABLE, PropertyRef::SELECTION_MODE, none())
        .expect("disable selection");
}

#[test]
fn create_node_and_batch_use_the_resulting_property_set() {
    let mut store = SemanticStore::new();
    store.create_model(MODEL, TypeRef::TABLE, 0).expect("model");
    assert_invalid(store.create_node(
        TABLE,
        TypeRef::TABLE,
        None,
        None,
        [
            (PropertyRef::MODEL_REF, Value::UnsignedInt(MODEL.get())),
            (PropertyRef::VALUE, Value::ItemId(ItemId::new(1))),
        ],
    ));
    assert!(!store.contains_node(TABLE));
    store
        .create_node(
            TABLE,
            TypeRef::TABLE,
            None,
            None,
            [
                (PropertyRef::VALUE, Value::ItemId(ItemId::new(1))),
                (PropertyRef::SELECTION_MODE, single()),
            ],
        )
        .expect("order inside one CREATE_NODE does not matter");

    assert_invalid(store.batch_property_set(TABLE, [(PropertyRef::SELECTION_MODE, none())]));
    store
        .batch_property_set(
            TABLE,
            [
                (PropertyRef::SELECTION_MODE, none()),
                (PropertyRef::SELECTION_MODE, single()),
                (PropertyRef::VALUE, Value::ItemId(ItemId::new(3))),
            ],
        )
        .expect("last assignment wins");
    assert_eq!(
        store.get_node(TABLE).unwrap().promoted_selection(),
        Some(ItemId::new(3))
    );
}

#[test]
fn failed_promotion_rolls_back_the_whole_transaction() {
    let mut store = store_with_table(TypeRef::TABLE, Some(none()));
    let base = store.revision();
    let result = store.apply_transaction(
        base,
        vec![
            Operation::set_property(TABLE, PropertyRef::LABEL, "Processes"),
            Operation::set_property(TABLE, PropertyRef::VALUE, Value::ItemId(ItemId::new(1))),
        ],
    );
    assert!(result.is_err());
    assert_eq!(store.revision(), base);
    assert!(!store
        .get_node(TABLE)
        .unwrap()
        .has_property(PropertyRef::LABEL));
}

#[test]
fn value_on_non_collection_nodes_is_unaffected() {
    let mut store = SemanticStore::new();
    store
        .create_node(
            NodeId::new(1),
            TypeRef::TEXT_INPUT,
            None,
            None,
            [(PropertyRef::VALUE, Value::String("abc".into()))],
        )
        .expect("text input value");
    store
        .set_property(
            NodeId::new(1),
            PropertyRef::VALUE,
            Value::ItemId(ItemId::new(1)),
        )
        .expect("no collection rule outside List/Table/Tree");
    assert_eq!(
        store.get_node(NodeId::new(1)).unwrap().promoted_selection(),
        None
    );
}
