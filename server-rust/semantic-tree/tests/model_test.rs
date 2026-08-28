use srui_semantic_tree::{
    resolve_standard_node_type, resolve_standard_property, ItemId, ModelId, ModelItem, NodeId,
    Operation, PropertyRef, Revision, SemanticStore, StoreError, StoreLimits, Transaction, TxnError,
    Value,
};

#[test]
fn test_create_sparse_model_and_item_mutations_by_identity() {
    let mut store = SemanticStore::new();

    let table_type = resolve_standard_node_type("Table").expect("Table type");
    let model_id = ModelId::new(7);
    let large_count = 500_000u64;

    // 1. Create a model with large item_count (500,000) and only a few cached items (§8)
    store
        .create_model(model_id, table_type, large_count)
        .expect("create model");

    assert_eq!(store.model_count(), 1);
    let model = store.get_model(model_id).expect("model exists");
    assert_eq!(model.item_count(), large_count);
    assert_eq!(model.cached_item_count(), 0);
    assert!(model.cached_ranges().is_empty());

    // 2. Insert initial items at specific sparse positions
    let col_name = PropertyRef::standard(1);
    let col_cpu = PropertyRef::standard(2);

    let item1 = ModelItem::new(
        ItemId::new(101),
        "nginx",
        vec![(col_name, Value::from("nginx")), (col_cpu, Value::from(0.05))],
    );
    let item2 = ModelItem::new(
        ItemId::new(202),
        "postgres",
        vec![(col_name, Value::from("postgres")), (col_cpu, Value::from(0.35))],
    );
    let item3 = ModelItem::new(
        ItemId::new(303),
        "redis",
        vec![(col_name, Value::from("redis")), (col_cpu, Value::from(0.02))],
    );

    // Reset range at index 10 for item1 and item2
    store
        .model_reset_range(model_id, 10, vec![item1.clone(), item2.clone()], None)
        .expect("reset range at 10");

    // Reset range at index 1000 for item3
    store
        .model_reset_range(model_id, 1000, vec![item3.clone()], None)
        .expect("reset range at 1000");

    let model = store.get_model(model_id).unwrap();
    assert_eq!(model.item_count(), large_count);
    assert_eq!(model.cached_item_count(), 3);
    assert_eq!(model.cached_ranges().len(), 2);
    assert_eq!(model.get_item_by_id(ItemId::new(101)).unwrap().value, Value::from("nginx"));
    assert_eq!(model.get_item_by_id(ItemId::new(202)).unwrap().value, Value::from("postgres"));
    assert_eq!(model.get_item_by_id(ItemId::new(303)).unwrap().value, Value::from("redis"));

    // 3. Insert a new item into the sparse collection using MODEL_INSERT (§13)
    let item_inserted = ModelItem::new(
        ItemId::new(150),
        "memcached",
        vec![(col_name, Value::from("memcached")), (col_cpu, Value::from(0.01))],
    );
    store
        .model_insert(model_id, 11, vec![item_inserted])
        .expect("insert memcached at index 11");

    // Verify insertion: item count incremented, item1 at index 10 untouched, item2 shifted to 12, item3 shifted to 1001
    let model = store.get_model(model_id).unwrap();
    assert_eq!(model.item_count(), large_count + 1);
    assert_eq!(model.cached_item_count(), 4);
    assert_eq!(model.index_of(ItemId::new(101)), Some(10));
    assert_eq!(model.index_of(ItemId::new(150)), Some(11));
    assert_eq!(model.index_of(ItemId::new(202)), Some(12));
    assert_eq!(model.index_of(ItemId::new(303)), Some(1001));

    // 4. Update an addressed item by item_id (update postgres -> postgres-master)
    let updated_postgres = ModelItem::new(
        ItemId::new(202),
        "postgres-master",
        vec![(col_name, Value::from("postgres-master")), (col_cpu, Value::from(0.50))],
    );
    store
        .model_update(model_id, None, vec![updated_postgres])
        .expect("update postgres");

    // Confirm only the addressed item changes; other items remain untouched
    let model = store.get_model(model_id).unwrap();
    assert_eq!(model.get_item_by_id(ItemId::new(202)).unwrap().value, Value::from("postgres-master"));
    assert_eq!(model.get_item_by_id(ItemId::new(202)).unwrap().get_property(col_cpu), Some(&Value::from(0.50)));
    assert_eq!(model.get_item_by_id(ItemId::new(101)).unwrap().value, Value::from("nginx"));
    assert_eq!(model.get_item_by_id(ItemId::new(150)).unwrap().value, Value::from("memcached"));
    assert_eq!(model.get_item_by_id(ItemId::new(303)).unwrap().value, Value::from("redis"));
    assert_eq!(model.cached_item_count(), 4);

    // 5. Delete an item by item_id (delete nginx ItemId(101))
    store
        .model_delete(model_id, None, None, vec![ItemId::new(101)])
        .expect("delete nginx");

    // Confirm only the addressed item was removed; others remain intact
    let model = store.get_model(model_id).unwrap();
    assert_eq!(model.cached_item_count(), 3);
    assert!(!model.contains_item(ItemId::new(101)));
    assert_eq!(model.get_item_by_id(ItemId::new(150)).unwrap().value, Value::from("memcached"));
    assert_eq!(model.get_item_by_id(ItemId::new(202)).unwrap().value, Value::from("postgres-master"));
    assert_eq!(model.get_item_by_id(ItemId::new(303)).unwrap().value, Value::from("redis"));
}

#[test]
fn test_model_reset_range_replaces_range_without_touching_count_or_other_ranges() {
    let mut store = SemanticStore::new();

    let list_type = resolve_standard_node_type("List").expect("List type");
    let model_id = ModelId::new(42);
    let total_count = 100_000u64;

    store.create_model(model_id, list_type, total_count).unwrap();

    // Populate Range A at index 100..103
    let range_a = vec![
        ModelItem::with_value(ItemId::new(1), "A0"),
        ModelItem::with_value(ItemId::new(2), "A1"),
        ModelItem::with_value(ItemId::new(3), "A2"),
    ];
    store.model_reset_range(model_id, 100, range_a, None).unwrap();

    // Populate Range B at index 500..502
    let range_b = vec![
        ModelItem::with_value(ItemId::new(10), "B0"),
        ModelItem::with_value(ItemId::new(11), "B1"),
    ];
    store.model_reset_range(model_id, 500, range_b, None).unwrap();

    let model = store.get_model(model_id).unwrap();
    assert_eq!(model.item_count(), 100_000);
    assert_eq!(model.cached_item_count(), 5);
    assert_eq!(model.cached_ranges().len(), 2);

    // Reset Range A with new items (X0, X1, X2) without touching total_count or Range B
    let new_range_a = vec![
        ModelItem::with_value(ItemId::new(1001), "X0"),
        ModelItem::with_value(ItemId::new(1002), "X1"),
        ModelItem::with_value(ItemId::new(1003), "X2"),
    ];
    store.model_reset_range(model_id, 100, new_range_a, None).unwrap();

    let model = store.get_model(model_id).unwrap();
    // 1. Total item_count must NOT change
    assert_eq!(model.item_count(), 100_000);

    // 2. Range A items are replaced
    assert!(!model.contains_item(ItemId::new(1)));
    assert!(!model.contains_item(ItemId::new(2)));
    assert!(!model.contains_item(ItemId::new(3)));
    assert_eq!(model.get_item_by_id(ItemId::new(1001)).unwrap().value, Value::from("X0"));
    assert_eq!(model.get_item_by_id(ItemId::new(1002)).unwrap().value, Value::from("X1"));
    assert_eq!(model.get_item_by_id(ItemId::new(1003)).unwrap().value, Value::from("X2"));

    // 3. Range B items are completely unchanged
    assert_eq!(model.get_item_by_id(ItemId::new(10)).unwrap().value, Value::from("B0"));
    assert_eq!(model.get_item_by_id(ItemId::new(11)).unwrap().value, Value::from("B1"));
    assert_eq!(model.index_of(ItemId::new(10)), Some(500));
    assert_eq!(model.index_of(ItemId::new(11)), Some(501));
    assert_eq!(model.cached_item_count(), 5);
}

#[test]
fn test_transaction_atomicity_spans_node_ops_and_model_ops() {
    let mut store = SemanticStore::new();

    let surface_type = resolve_standard_node_type("Surface").unwrap();
    let table_type = resolve_standard_node_type("Table").unwrap();

    // Initialize baseline revision 1 with a surface and a model
    let rev1 = store
        .apply_transaction(
            Revision::INITIAL,
            vec![
                Operation::create_node(NodeId::new(1), surface_type, None, None, vec![]),
                Operation::create_model(ModelId::new(10), table_type, 1_000),
            ],
        )
        .expect("initial setup transaction");
    assert_eq!(rev1, Revision::new(1));
    assert_eq!(store.node_count(), 1);
    assert_eq!(store.model_count(), 1);
    assert_eq!(store.get_model(ModelId::new(10)).unwrap().cached_item_count(), 0);

    // Build a transaction combining:
    // 1. A valid MODEL_INSERT on model 10
    // 2. An invalid CREATE_NODE op (referencing non-existent parent NodeId(999))
    let invalid_txn_ops = vec![
        Operation::model_insert(
            ModelId::new(10),
            0,
            vec![
                ModelItem::with_value(ItemId::new(50), "item-50"),
                ModelItem::with_value(ItemId::new(51), "item-51"),
            ],
        ),
        // Invalid op: parent 999 does not exist!
        Operation::create_node(
            NodeId::new(2),
            surface_type,
            Some(NodeId::new(999)),
            None,
            vec![],
        ),
    ];

    let result = store.apply_transaction(rev1, invalid_txn_ops);
    assert!(result.is_err());
    match result.unwrap_err() {
        TxnError::OpFailed { op_index, source } => {
            assert_eq!(op_index, 1);
            assert_eq!(source, StoreError::ParentNotFound(NodeId::new(999)));
        }
        other => panic!("expected OpFailed error, got: {:?}", other),
    }

    // Verify complete atomic rollback:
    // - Revision remains at rev1 (1)
    // - Node graph remains unchanged (1 node)
    // - Model 10 has 0 cached items (model_insert rolled back cleanly!)
    assert_eq!(store.revision(), rev1);
    assert_eq!(store.node_count(), 1);
    assert_eq!(store.model_count(), 1);
    let model = store.get_model(ModelId::new(10)).unwrap();
    assert_eq!(model.cached_item_count(), 0);
    assert_eq!(model.item_count(), 1_000);
    assert!(!model.contains_item(ItemId::new(50)));
    assert!(!model.contains_item(ItemId::new(51)));
}

#[test]
fn test_node_referencing_model_via_model_ref_property() {
    let mut store = SemanticStore::new();

    let table_type = resolve_standard_node_type("Table").unwrap();
    let model_ref_prop = resolve_standard_property("model_ref").unwrap();
    let model_id = ModelId::new(77);

    // Create table node referencing Model #77 (§8)
    store
        .create_model(model_id, table_type, 50_000)
        .expect("create model");
    store
        .create_node(
            NodeId::new(40),
            table_type,
            None,
            None,
            vec![(model_ref_prop, Value::from(77u64))],
        )
        .expect("create table node");

    let table_node = store.get_node(NodeId::new(40)).unwrap();
    assert_eq!(table_node.model_ref(), Some(model_id));

    let referenced_model = store.get_model_for_node(NodeId::new(40)).unwrap();
    assert_eq!(referenced_model.id, model_id);
    assert_eq!(referenced_model.item_count(), 50_000);
}

#[test]
fn test_model_id_never_reused_in_session() {
    let mut store = SemanticStore::new();
    let list_type = resolve_standard_node_type("List").unwrap();
    let model_id = ModelId::new(5);

    store.create_model(model_id, list_type, 100).unwrap();
    assert!(store.contains_model(model_id));

    // Delete model
    store.delete_model(model_id).unwrap();
    assert!(!store.contains_model(model_id));
    assert!(store.is_model_id_used(model_id));

    // Attempting to recreate using same ModelId must fail (§6.2, §8)
    let err = store.create_model(model_id, list_type, 200).unwrap_err();
    assert_eq!(err, StoreError::ModelIdAlreadyUsed(model_id));
}

#[test]
fn test_model_operations_wire_protobuf_roundtrip() {
    let list_type = resolve_standard_node_type("List").unwrap();
    let model_id = ModelId::new(99);

    let create_op = Operation::create_model(model_id, list_type, 10_000);
    let wire_create: srui_protocol::Operation = create_op.clone().into();
    let roundtrip_create = Operation::try_from(wire_create).unwrap();
    assert_eq!(create_op, roundtrip_create);

    let insert_op = Operation::model_insert(
        model_id,
        5,
        vec![
            ModelItem::with_value(ItemId::new(1), "item1"),
            ModelItem::with_value(ItemId::new(2), "item2"),
        ],
    );
    let wire_insert: srui_protocol::Operation = insert_op.clone().into();
    let roundtrip_insert = Operation::try_from(wire_insert).unwrap();
    assert_eq!(insert_op, roundtrip_insert);

    let delete_op = Operation::model_delete_items(model_id, vec![ItemId::new(1), ItemId::new(2)]);
    let wire_delete: srui_protocol::Operation = delete_op.clone().into();
    let roundtrip_delete = Operation::try_from(wire_delete).unwrap();
    assert_eq!(delete_op, roundtrip_delete);

    let update_op = Operation::model_update(
        model_id,
        Some(5),
        vec![ModelItem::with_value(ItemId::new(1), "item1-updated")],
    );
    let wire_update: srui_protocol::Operation = update_op.clone().into();
    let roundtrip_update = Operation::try_from(wire_update).unwrap();
    assert_eq!(update_op, roundtrip_update);

    let reset_op = Operation::model_reset_range(
        model_id,
        0,
        vec![ModelItem::with_value(ItemId::new(10), "r0")],
        Some(10_000),
    );
    let wire_reset: srui_protocol::Operation = reset_op.clone().into();
    let roundtrip_reset = Operation::try_from(wire_reset).unwrap();
    assert_eq!(reset_op, roundtrip_reset);
}

#[test]
fn test_model_limits_enforcement() {
    let limits = StoreLimits::default();
    let max_str_len = limits.max_string_length;
    let mut store = SemanticStore::with_limits(limits);

    let list_type = resolve_standard_node_type("List").unwrap();
    let model_id = ModelId::new(1);
    store.create_model(model_id, list_type, 100).unwrap();

    // Create an item with an oversized string value
    let oversized_str = "x".repeat(max_str_len + 1);
    let invalid_item = ModelItem::with_value(ItemId::new(1), oversized_str);

    let err = store.model_insert(model_id, 0, vec![invalid_item]).unwrap_err();
    assert_eq!(
        err,
        StoreError::MaxStringLengthExceeded {
            limit: max_str_len,
            actual: max_str_len + 1,
        }
    );
}

#[test]
fn test_successful_mixed_transaction_commits_atomically() {
    let mut store = SemanticStore::new();

    let surface_type = resolve_standard_node_type("Surface").unwrap();
    let tree_type = resolve_standard_node_type("Tree").unwrap();
    let model_ref_prop = resolve_standard_property("model_ref").unwrap();

    let txn = Transaction::new(
        Revision::INITIAL,
        vec![
            Operation::create_model(ModelId::new(1), tree_type, 500),
            Operation::model_reset_range(
                ModelId::new(1),
                0,
                vec![
                    ModelItem::with_value(ItemId::new(10), "root_node"),
                    ModelItem::with_value(ItemId::new(11), "child_node"),
                ],
                None,
            ),
            Operation::create_node(
                NodeId::new(1),
                surface_type,
                None,
                None,
                vec![(model_ref_prop, Value::from(1u64))],
            ),
        ],
    );

    let new_rev = store.apply_transaction_record(&txn).expect("apply mixed txn");
    assert_eq!(new_rev, Revision::new(1));
    assert_eq!(store.revision(), Revision::new(1));
    assert_eq!(store.node_count(), 1);
    assert_eq!(store.model_count(), 1);

    let model = store.get_model(ModelId::new(1)).unwrap();
    assert_eq!(model.cached_item_count(), 2);
    assert_eq!(model.get_item_by_id(ItemId::new(10)).unwrap().value, Value::from("root_node"));
}
