//! Store rules for the server-hinted keep-in-view item, the standard `keep_visible` property
//! (§4 inv. 13, §6.3, §8, §12.1).
//!
//! Every replica enforces the same cache-independent rules: the value is an `ItemId`, held only
//! by a `List`, `Table` or `Tree` that has a `model_ref`. Item presence is not a store rule.

use srui_semantic_tree::{
    EnumToken, ItemId, ModelId, ModelItem, NodeId, Operation, PropertyRef, SemanticStore,
    StoreError, TypeRef, Value,
};

const TABLE: NodeId = NodeId::new(2);
const MODEL: ModelId = ModelId::new(7);

fn item(id: u64) -> Value {
    Value::ItemId(ItemId::new(id))
}

fn store_with(node_type: TypeRef, model_backed: bool) -> SemanticStore {
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
    let props = if model_backed {
        vec![(PropertyRef::MODEL_REF, Value::UnsignedInt(MODEL.get()))]
    } else {
        vec![]
    };
    store
        .create_node(TABLE, node_type, None, None, props)
        .expect("node");
    store
}

fn assert_invalid(result: Result<impl std::fmt::Debug, StoreError>) {
    match result {
        Err(err @ StoreError::InvalidKeepVisible { .. }) => {
            assert_eq!(err.conformance_code(), "invalid_keep_visible");
        }
        other => panic!("expected InvalidKeepVisible, got {other:?}"),
    }
}

fn held(store: &SemanticStore) -> Option<ItemId> {
    store.get_node(TABLE).unwrap().keep_visible()
}

#[test]
fn item_id_is_admitted_on_every_model_backed_collection_type() {
    for node_type in [TypeRef::LIST, TypeRef::TABLE, TypeRef::TREE] {
        let mut store = store_with(node_type, true);
        store
            .set_property(TABLE, PropertyRef::KEEP_VISIBLE, item(2))
            .expect("keep_visible");
        assert_eq!(held(&store), Some(ItemId::new(2)));
        // Never selection state: neither `value` nor `selection_mode` is involved.
        let node = store.get_node(TABLE).unwrap();
        assert_eq!(node.promoted_selection(), None);
        assert!(!node.has_property(PropertyRef::SELECTION_MODE));
    }
}

#[test]
fn an_uncached_item_is_not_a_store_error() {
    let mut store = store_with(TypeRef::TABLE, true);
    store
        .set_property(TABLE, PropertyRef::KEEP_VISIBLE, item(999))
        .expect("store keeps cache-independent rules only");
    assert_eq!(held(&store), Some(ItemId::new(999)));
}

#[test]
fn row_index_or_other_value_is_refused() {
    for value in [
        Value::UnsignedInt(2),
        Value::SignedInt(2),
        Value::String("2".into()),
        Value::Bool(true),
        Value::Null,
        Value::List(vec![item(2)]),
    ] {
        let mut store = store_with(TypeRef::TABLE, true);
        assert_invalid(store.set_property(TABLE, PropertyRef::KEEP_VISIBLE, value));
        assert!(!store
            .get_node(TABLE)
            .unwrap()
            .has_property(PropertyRef::KEEP_VISIBLE));
    }
}

#[test]
fn non_collection_or_model_less_nodes_refuse_it() {
    for node_type in [
        TypeRef::BUTTON,
        TypeRef::TEXT,
        TypeRef::SELECT,
        TypeRef::SURFACE,
    ] {
        let mut store = store_with(node_type, false);
        assert_invalid(store.set_property(TABLE, PropertyRef::KEEP_VISIBLE, item(1)));
    }
    // A collection without model_ref (an inline `items` collection) is not model-backed.
    for node_type in [TypeRef::LIST, TypeRef::TABLE, TypeRef::TREE] {
        let mut store = store_with(node_type, false);
        assert_invalid(store.set_property(TABLE, PropertyRef::KEEP_VISIBLE, item(1)));
        assert_eq!(held(&store), None);
    }
}

#[test]
fn clearing_model_ref_while_held_is_refused() {
    let mut store = store_with(TypeRef::TABLE, true);
    store
        .set_property(TABLE, PropertyRef::KEEP_VISIBLE, item(1))
        .unwrap();
    assert_invalid(store.clear_property(TABLE, PropertyRef::MODEL_REF));
    assert!(store
        .get_node(TABLE)
        .unwrap()
        .has_property(PropertyRef::MODEL_REF));

    // Clearing the hint first makes the same change legal.
    store
        .clear_property(TABLE, PropertyRef::KEEP_VISIBLE)
        .unwrap();
    store
        .clear_property(TABLE, PropertyRef::MODEL_REF)
        .expect("model_ref clear after the hint is gone");
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
        [(PropertyRef::KEEP_VISIBLE, item(1))],
    ));
    assert!(!store.contains_node(TABLE));
    assert_invalid(store.create_node(
        TABLE,
        TypeRef::BUTTON,
        None,
        None,
        [
            (PropertyRef::MODEL_REF, Value::UnsignedInt(MODEL.get())),
            (PropertyRef::KEEP_VISIBLE, item(1)),
        ],
    ));
    store
        .create_node(
            TABLE,
            TypeRef::TABLE,
            None,
            None,
            [
                (PropertyRef::KEEP_VISIBLE, item(1)),
                (PropertyRef::MODEL_REF, Value::UnsignedInt(MODEL.get())),
            ],
        )
        .expect("order inside one CREATE_NODE does not matter");

    assert_invalid(
        store.batch_property_set(TABLE, [(PropertyRef::KEEP_VISIBLE, Value::UnsignedInt(3))]),
    );
    store
        .batch_property_set(
            TABLE,
            [
                (PropertyRef::KEEP_VISIBLE, Value::UnsignedInt(3)),
                (PropertyRef::KEEP_VISIBLE, item(3)),
            ],
        )
        .expect("last assignment wins");
    assert_eq!(held(&store), Some(ItemId::new(3)));
}

#[test]
fn a_refused_hint_rolls_back_the_whole_transaction() {
    let mut store = store_with(TypeRef::TABLE, true);
    let base = store.revision();
    let result = store.apply_transaction(
        base,
        vec![
            Operation::set_property(TABLE, PropertyRef::LABEL, "Processes"),
            Operation::set_property(TABLE, PropertyRef::KEEP_VISIBLE, Value::UnsignedInt(1)),
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
fn the_hint_is_independent_of_selection_rules() {
    // selection_mode none still admits keep_visible; and keep_visible never satisfies or
    // breaks the promoted-selection rule.
    let mut store = store_with(TypeRef::TABLE, true);
    store
        .set_property(
            TABLE,
            PropertyRef::SELECTION_MODE,
            Value::EnumToken(EnumToken::SELECTION_MODE_NONE),
        )
        .unwrap();
    store
        .set_property(TABLE, PropertyRef::KEEP_VISIBLE, item(2))
        .expect("admitted with selection_mode none");
    assert!(matches!(
        store.set_property(TABLE, PropertyRef::VALUE, item(2)),
        Err(StoreError::InvalidCollectionSelection { .. })
    ));
    assert_eq!(held(&store), Some(ItemId::new(2)));
}
