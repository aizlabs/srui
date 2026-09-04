use srui_semantic_tree::{
    resolve_standard_node_type, resolve_standard_property, ItemId, ModelId, ModelItem, NodeId,
    Operation, PropertyRef, Revision, SemanticStore, StoreError, StoreLimits, Transaction,
    TxnError, Value, DEFAULT_MAX_MODEL_COUNT, DEFAULT_MAX_TRANSACTION_OPERATIONS,
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
        vec![
            (col_name, Value::from("nginx")),
            (col_cpu, Value::from(0.05)),
        ],
    );
    let item2 = ModelItem::new(
        ItemId::new(202),
        "postgres",
        vec![
            (col_name, Value::from("postgres")),
            (col_cpu, Value::from(0.35)),
        ],
    );
    let item3 = ModelItem::new(
        ItemId::new(303),
        "redis",
        vec![
            (col_name, Value::from("redis")),
            (col_cpu, Value::from(0.02)),
        ],
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
    assert_eq!(
        model.get_item_by_id(ItemId::new(101)).unwrap().value,
        Value::from("nginx")
    );
    assert_eq!(
        model.get_item_by_id(ItemId::new(202)).unwrap().value,
        Value::from("postgres")
    );
    assert_eq!(
        model.get_item_by_id(ItemId::new(303)).unwrap().value,
        Value::from("redis")
    );

    // 3. Insert a new item into the sparse collection using MODEL_INSERT (§13)
    let item_inserted = ModelItem::new(
        ItemId::new(150),
        "memcached",
        vec![
            (col_name, Value::from("memcached")),
            (col_cpu, Value::from(0.01)),
        ],
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
        vec![
            (col_name, Value::from("postgres-master")),
            (col_cpu, Value::from(0.50)),
        ],
    );
    store
        .model_update(model_id, None, vec![updated_postgres])
        .expect("update postgres");

    // Confirm only the addressed item changes; other items remain untouched
    let model = store.get_model(model_id).unwrap();
    assert_eq!(
        model.get_item_by_id(ItemId::new(202)).unwrap().value,
        Value::from("postgres-master")
    );
    assert_eq!(
        model
            .get_item_by_id(ItemId::new(202))
            .unwrap()
            .get_property(col_cpu),
        Some(&Value::from(0.50))
    );
    assert_eq!(
        model.get_item_by_id(ItemId::new(101)).unwrap().value,
        Value::from("nginx")
    );
    assert_eq!(
        model.get_item_by_id(ItemId::new(150)).unwrap().value,
        Value::from("memcached")
    );
    assert_eq!(
        model.get_item_by_id(ItemId::new(303)).unwrap().value,
        Value::from("redis")
    );
    assert_eq!(model.cached_item_count(), 4);

    // 5. Delete an item by item_id (delete nginx ItemId(101))
    store
        .model_delete(model_id, None, None, &[ItemId::new(101)])
        .expect("delete nginx");

    // Confirm only the addressed item was removed; others remain intact
    let model = store.get_model(model_id).unwrap();
    assert_eq!(model.cached_item_count(), 3);
    assert!(!model.contains_item(ItemId::new(101)));
    assert_eq!(
        model.get_item_by_id(ItemId::new(150)).unwrap().value,
        Value::from("memcached")
    );
    assert_eq!(
        model.get_item_by_id(ItemId::new(202)).unwrap().value,
        Value::from("postgres-master")
    );
    assert_eq!(
        model.get_item_by_id(ItemId::new(303)).unwrap().value,
        Value::from("redis")
    );
}

#[test]
fn test_model_reset_range_replaces_range_without_touching_count_or_other_ranges() {
    let mut store = SemanticStore::new();

    let list_type = resolve_standard_node_type("List").expect("List type");
    let model_id = ModelId::new(42);
    let total_count = 100_000u64;

    store
        .create_model(model_id, list_type, total_count)
        .unwrap();

    // Populate Range A at index 100..103
    let range_a = vec![
        ModelItem::with_value(ItemId::new(1), "A0"),
        ModelItem::with_value(ItemId::new(2), "A1"),
        ModelItem::with_value(ItemId::new(3), "A2"),
    ];
    store
        .model_reset_range(model_id, 100, range_a, None)
        .unwrap();

    // Populate Range B at index 500..502
    let range_b = vec![
        ModelItem::with_value(ItemId::new(10), "B0"),
        ModelItem::with_value(ItemId::new(11), "B1"),
    ];
    store
        .model_reset_range(model_id, 500, range_b, None)
        .unwrap();

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
    store
        .model_reset_range(model_id, 100, new_range_a, None)
        .unwrap();

    let model = store.get_model(model_id).unwrap();
    // 1. Total item_count must NOT change
    assert_eq!(model.item_count(), 100_000);

    // 2. Range A items are replaced
    assert!(!model.contains_item(ItemId::new(1)));
    assert!(!model.contains_item(ItemId::new(2)));
    assert!(!model.contains_item(ItemId::new(3)));
    assert_eq!(
        model.get_item_by_id(ItemId::new(1001)).unwrap().value,
        Value::from("X0")
    );
    assert_eq!(
        model.get_item_by_id(ItemId::new(1002)).unwrap().value,
        Value::from("X1")
    );
    assert_eq!(
        model.get_item_by_id(ItemId::new(1003)).unwrap().value,
        Value::from("X2")
    );

    // 3. Range B items are completely unchanged
    assert_eq!(
        model.get_item_by_id(ItemId::new(10)).unwrap().value,
        Value::from("B0")
    );
    assert_eq!(
        model.get_item_by_id(ItemId::new(11)).unwrap().value,
        Value::from("B1")
    );
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
    assert_eq!(
        store
            .get_model(ModelId::new(10))
            .unwrap()
            .cached_item_count(),
        0
    );

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

    let err = store
        .model_insert(model_id, 0, vec![invalid_item])
        .unwrap_err();
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

    let new_rev = store
        .apply_transaction_record(&txn)
        .expect("apply mixed txn");
    assert_eq!(new_rev, Revision::new(1));
    assert_eq!(store.revision(), Revision::new(1));
    assert_eq!(store.node_count(), 1);
    assert_eq!(store.model_count(), 1);

    let model = store.get_model(ModelId::new(1)).unwrap();
    assert_eq!(model.cached_item_count(), 2);
    assert_eq!(
        model.get_item_by_id(ItemId::new(10)).unwrap().value,
        Value::from("root_node")
    );
}

#[test]
fn test_reset_range_rejects_item_id_cached_outside_replaced_range() {
    let mut store = SemanticStore::new();
    let list_type = resolve_standard_node_type("List").unwrap();
    let model_id = ModelId::new(1);

    store.create_model(model_id, list_type, 1_000).unwrap();
    store
        .model_reset_range(
            model_id,
            500,
            vec![ModelItem::with_value(ItemId::new(42), "cached-at-500")],
            None,
        )
        .unwrap();

    let err = store
        .model_reset_range(
            model_id,
            100,
            vec![ModelItem::with_value(ItemId::new(42), "collision")],
            None,
        )
        .unwrap_err();

    assert_eq!(
        err,
        StoreError::DuplicateItemId {
            model_id,
            item_id: ItemId::new(42),
        }
    );
}

#[test]
fn test_model_delete_range_rejects_out_of_bounds() {
    let mut store = SemanticStore::new();
    let list_type = resolve_standard_node_type("List").unwrap();
    let model_id = ModelId::new(1);

    store.create_model(model_id, list_type, 10).unwrap();

    let err = store
        .model_delete(model_id, Some(5), Some(100), &[])
        .unwrap_err();
    assert_eq!(
        err,
        StoreError::ModelIndexOutOfBounds {
            index: 105,
            count: 10,
        }
    );
}

#[test]
fn test_model_delete_rejects_combined_identity_and_range() {
    let mut store = SemanticStore::new();
    let list_type = resolve_standard_node_type("List").unwrap();
    let model_id = ModelId::new(1);

    store.create_model(model_id, list_type, 100).unwrap();
    store
        .model_reset_range(
            model_id,
            0,
            vec![
                ModelItem::with_value(ItemId::new(1), "a"),
                ModelItem::with_value(ItemId::new(2), "b"),
            ],
            None,
        )
        .unwrap();

    let err = store
        .model_delete(
            model_id,
            Some(0),
            Some(2),
            &[ItemId::new(1), ItemId::new(2)],
        )
        .unwrap_err();

    assert!(matches!(err, StoreError::InvalidModelDelete(_)));
}

#[test]
fn test_model_items_per_operation_limit_enforced() {
    let limits = StoreLimits::default().with_max_items_per_model_operation(2);
    let mut store = SemanticStore::with_limits(limits);
    let list_type = resolve_standard_node_type("List").unwrap();
    let model_id = ModelId::new(1);

    store.create_model(model_id, list_type, 100).unwrap();

    let err = store
        .model_insert(
            model_id,
            0,
            vec![
                ModelItem::with_value(ItemId::new(1), "a"),
                ModelItem::with_value(ItemId::new(2), "b"),
                ModelItem::with_value(ItemId::new(3), "c"),
            ],
        )
        .unwrap_err();

    assert_eq!(
        err,
        StoreError::MaxItemsPerModelOperationExceeded {
            limit: 2,
            actual: 3
        }
    );
}

#[test]
fn test_max_model_count_limit_enforced() {
    let limits = StoreLimits::default().with_max_model_count(2);
    let mut store = SemanticStore::with_limits(limits);
    let list_type = resolve_standard_node_type("List").unwrap();

    store.create_model(ModelId::new(1), list_type, 100).unwrap();
    store.create_model(ModelId::new(2), list_type, 100).unwrap();

    let err = store
        .create_model(ModelId::new(3), list_type, 100)
        .unwrap_err();

    assert_eq!(
        err,
        StoreError::MaxModelCountExceeded {
            limit: 2,
            current: 2
        }
    );
    assert_eq!(store.model_count(), 2);
}

#[test]
fn test_max_cached_items_per_model_enforced() {
    let limits = StoreLimits::default().with_max_cached_items_per_model(3);
    let mut store = SemanticStore::with_limits(limits);
    let list_type = resolve_standard_node_type("List").unwrap();
    let model_id = ModelId::new(1);

    store.create_model(model_id, list_type, 10_000).unwrap();

    // Insert 2 items -> ok
    store
        .model_insert(
            model_id,
            0,
            vec![
                ModelItem::with_value(ItemId::new(1), "a"),
                ModelItem::with_value(ItemId::new(2), "b"),
            ],
        )
        .unwrap();

    // Insert 2 more items -> projected cached is 4, which exceeds limit 3
    let err = store
        .model_insert(
            model_id,
            2,
            vec![
                ModelItem::with_value(ItemId::new(3), "c"),
                ModelItem::with_value(ItemId::new(4), "d"),
            ],
        )
        .unwrap_err();

    assert_eq!(
        err,
        StoreError::MaxCachedItemsPerModelExceeded {
            limit: 3,
            current: 2,
            attempted: 4,
        }
    );

    // Reset range with 4 items -> exceeds limit 3
    let err = store
        .model_reset_range(
            model_id,
            0,
            vec![
                ModelItem::with_value(ItemId::new(10), "x0"),
                ModelItem::with_value(ItemId::new(11), "x1"),
                ModelItem::with_value(ItemId::new(12), "x2"),
                ModelItem::with_value(ItemId::new(13), "x3"),
            ],
            None,
        )
        .unwrap_err();

    assert_eq!(
        err,
        StoreError::MaxCachedItemsPerModelExceeded {
            limit: 3,
            current: 2,
            attempted: 4,
        }
    );
}

#[test]
fn test_model_reset_range_evicts_farthest_cached_items_to_stay_in_budget() {
    let limits = StoreLimits::default().with_max_cached_items_per_model(3);
    let mut store = SemanticStore::with_limits(limits);
    let list_type = resolve_standard_node_type("List").unwrap();
    let model_id = ModelId::new(1);
    store.create_model(model_id, list_type, 10_000).unwrap();
    store
        .model_reset_range(
            model_id,
            0,
            vec![
                ModelItem::with_value(ItemId::new(1), "a"),
                ModelItem::with_value(ItemId::new(2), "b"),
                ModelItem::with_value(ItemId::new(3), "c"),
            ],
            None,
        )
        .unwrap();

    store
        .model_reset_range(
            model_id,
            100,
            vec![
                ModelItem::with_value(ItemId::new(10), "far-0"),
                ModelItem::with_value(ItemId::new(11), "far-1"),
            ],
            None,
        )
        .unwrap();

    let model = store.get_model(model_id).unwrap();
    assert_eq!(model.cached_item_count(), 3);
    assert!(model.get_item_by_index(100).is_some());
    assert!(model.get_item_by_index(101).is_some());
    // Midpoint of the keep window is 101; index 0 is farthest of {0,1,2} and is dropped first,
    // then 1, leaving the closest previous row.
    assert!(model.get_item_by_index(0).is_none());
    assert!(model.get_item_by_index(1).is_none());
    assert!(model.get_item_by_index(2).is_some());
}

#[test]
fn test_evict_farthest_outside_count_zero_is_a_no_op() {
    let limits = StoreLimits::default().with_max_cached_items_per_model(3);
    let mut store = SemanticStore::with_limits(limits);
    let list_type = resolve_standard_node_type("List").unwrap();
    let model_id = ModelId::new(1);
    store.create_model(model_id, list_type, 10_000).unwrap();
    store
        .model_reset_range(
            model_id,
            0,
            vec![
                ModelItem::with_value(ItemId::new(1), "a"),
                ModelItem::with_value(ItemId::new(2), "b"),
            ],
            None,
        )
        .unwrap();
    store
        .get_model_mut(model_id)
        .unwrap()
        .evict_farthest_outside(0, 2, 0);
    let model = store.get_model(model_id).unwrap();
    assert_eq!(model.cached_item_count(), 2);
    assert!(model.get_item_by_index(0).is_some());
    assert!(model.get_item_by_index(1).is_some());
}

#[test]
fn test_model_delete_combined_identity_and_range_preserves_item_count() {
    let mut store = SemanticStore::new();
    let list_type = resolve_standard_node_type("List").unwrap();
    let model_id = ModelId::new(1);

    store.create_model(model_id, list_type, 100).unwrap();
    store
        .model_reset_range(
            model_id,
            5,
            vec![
                ModelItem::with_value(ItemId::new(50), "item-5"),
                ModelItem::with_value(ItemId::new(51), "item-6"),
            ],
            None,
        )
        .unwrap();

    // Attempt invalid combined delete
    let err = store
        .model_delete(model_id, Some(5), Some(2), &[ItemId::new(50)])
        .unwrap_err();

    assert!(matches!(err, StoreError::InvalidModelDelete(_)));

    // Verify item count and cache are completely untouched
    let model = store.get_model(model_id).unwrap();
    assert_eq!(model.item_count(), 100);
    assert_eq!(model.cached_item_count(), 2);
    assert!(model.contains_item(ItemId::new(50)));
    assert!(model.contains_item(ItemId::new(51)));
}

#[test]
fn test_model_delete_sparse_range_large_count() {
    let mut store = SemanticStore::new();
    let list_type = resolve_standard_node_type("List").unwrap();
    let model_id = ModelId::new(1);
    let total_count = 1_000_000u64;

    store
        .create_model(model_id, list_type, total_count)
        .unwrap();

    // Place sparse items at index 10, 50, 100, 200
    store
        .model_reset_range(
            model_id,
            10,
            vec![ModelItem::with_value(ItemId::new(10), "at-10")],
            None,
        )
        .unwrap();
    store
        .model_reset_range(
            model_id,
            50,
            vec![ModelItem::with_value(ItemId::new(50), "at-50")],
            None,
        )
        .unwrap();
    store
        .model_reset_range(
            model_id,
            100,
            vec![ModelItem::with_value(ItemId::new(100), "at-100")],
            None,
        )
        .unwrap();
    store
        .model_reset_range(
            model_id,
            200,
            vec![ModelItem::with_value(ItemId::new(200), "at-200")],
            None,
        )
        .unwrap();

    // Delete range [40, 140) (count = 100). This covers items at 50 and 100.
    store
        .model_delete(model_id, Some(40), Some(100), &[])
        .unwrap();

    let model = store.get_model(model_id).unwrap();
    assert_eq!(model.item_count(), 999_900);
    assert_eq!(model.cached_item_count(), 2);

    // Item 10 is untouched before range
    assert_eq!(model.index_of(ItemId::new(10)), Some(10));
    // Items 50 and 100 removed
    assert!(!model.contains_item(ItemId::new(50)));
    assert!(!model.contains_item(ItemId::new(100)));
    // Item 200 shifted down by 100 to index 100
    assert_eq!(model.index_of(ItemId::new(200)), Some(100));
}

#[test]
fn test_model_batch_limits_enforced_across_all_ops() {
    let limits = StoreLimits::default().with_max_items_per_model_operation(2);
    let mut store = SemanticStore::with_limits(limits);
    let list_type = resolve_standard_node_type("List").unwrap();
    let model_id = ModelId::new(1);

    store.create_model(model_id, list_type, 100).unwrap();

    // 1. model_update batch limit
    let update_err = store
        .model_update(
            model_id,
            Some(0),
            vec![
                ModelItem::with_value(ItemId::new(1), "a"),
                ModelItem::with_value(ItemId::new(2), "b"),
                ModelItem::with_value(ItemId::new(3), "c"),
            ],
        )
        .unwrap_err();
    assert_eq!(
        update_err,
        StoreError::MaxItemsPerModelOperationExceeded {
            limit: 2,
            actual: 3
        }
    );

    // 2. model_reset_range batch limit
    let reset_err = store
        .model_reset_range(
            model_id,
            0,
            vec![
                ModelItem::with_value(ItemId::new(1), "a"),
                ModelItem::with_value(ItemId::new(2), "b"),
                ModelItem::with_value(ItemId::new(3), "c"),
            ],
            None,
        )
        .unwrap_err();
    assert_eq!(
        reset_err,
        StoreError::MaxItemsPerModelOperationExceeded {
            limit: 2,
            actual: 3
        }
    );

    // 3. model_delete batch limit (item_ids)
    let delete_err = store
        .model_delete(
            model_id,
            None,
            None,
            &[ItemId::new(1), ItemId::new(2), ItemId::new(3)],
        )
        .unwrap_err();
    assert_eq!(
        delete_err,
        StoreError::MaxItemsPerModelOperationExceeded {
            limit: 2,
            actual: 3
        }
    );
}

#[test]
fn test_model_update_by_identity_and_by_index() {
    let mut store = SemanticStore::new();
    let model_id = ModelId::new(1);
    let list_type = resolve_standard_node_type("List").unwrap();

    store.create_model(model_id, list_type, 10).unwrap();

    store
        .model_insert(
            model_id,
            0,
            vec![
                ModelItem::with_value(ItemId::new(10), "original_10"),
                ModelItem::with_value(ItemId::new(20), "original_20"),
            ],
        )
        .unwrap();

    // 1. Update by identity (index: None)
    store
        .model_update(
            model_id,
            None,
            vec![ModelItem::with_value(ItemId::new(20), "updated_20_by_id")],
        )
        .unwrap();

    let model = store.get_model(model_id).unwrap();
    assert_eq!(
        model.get_item_by_index(1).unwrap().value,
        Value::from("updated_20_by_id")
    );
    assert_eq!(
        model.get_item_by_id(ItemId::new(20)).unwrap().value,
        Value::from("updated_20_by_id")
    );

    // 2. Update by index (index: Some(0))
    store
        .model_update(
            model_id,
            Some(0),
            vec![ModelItem::with_value(ItemId::new(10), "updated_10_by_idx")],
        )
        .unwrap();

    let model = store.get_model(model_id).unwrap();
    assert_eq!(
        model.get_item_by_index(0).unwrap().value,
        Value::from("updated_10_by_idx")
    );
    assert_eq!(
        model.get_item_by_id(ItemId::new(10)).unwrap().value,
        Value::from("updated_10_by_idx")
    );
}

#[test]
fn test_model_update_positional_conflict_reindexes_cleanly() {
    let mut store = SemanticStore::new();
    let model_id = ModelId::new(1);
    let list_type = resolve_standard_node_type("List").unwrap();

    store.create_model(model_id, list_type, 10).unwrap();

    store
        .model_insert(
            model_id,
            0,
            vec![
                ModelItem::with_value(ItemId::new(1), "item_at_0"),
                ModelItem::with_value(ItemId::new(2), "item_at_1"),
                ModelItem::with_value(ItemId::new(3), "item_at_2"),
            ],
        )
        .unwrap();

    // Positional update writes ItemId(3) to index 0 (previously at index 2, displacing ItemId(1))
    store
        .model_update(
            model_id,
            Some(0),
            vec![ModelItem::with_value(ItemId::new(3), "item_3_moved_to_0")],
        )
        .unwrap();

    let model = store.get_model(model_id).unwrap();
    // Index 0 has ItemId(3)
    assert_eq!(model.get_item_by_index(0).unwrap().item_id, ItemId::new(3));
    assert_eq!(
        model.get_item_by_index(0).unwrap().value,
        Value::from("item_3_moved_to_0")
    );
    assert_eq!(model.index_of(ItemId::new(3)), Some(0));

    // Old index 2 is now vacant
    assert_eq!(model.get_item_by_index(2), None);

    // Displaced ItemId(1) was removed from cache
    assert_eq!(model.get_item_by_id(ItemId::new(1)), None);
    assert_eq!(model.index_of(ItemId::new(1)), None);

    // Index 1 (ItemId(2)) remains intact
    assert_eq!(model.get_item_by_index(1).unwrap().item_id, ItemId::new(2));
}

#[test]
fn test_store_limits_granular_builder_methods() {
    let limits = StoreLimits::default()
        .with_max_tree_depth(12)
        .with_max_node_count(500)
        .with_max_string_length(4096)
        .with_max_value_depth(4)
        .with_max_list_elements(50)
        .with_max_record_properties(25)
        .with_max_transaction_operations(100)
        .with_max_model_count(50)
        .with_max_cached_items_per_model(2_000)
        .with_max_items_per_model_operation(500);

    assert_eq!(limits.max_tree_depth, 12);
    assert_eq!(limits.max_node_count, 500);
    assert_eq!(limits.max_string_length, 4096);
    assert_eq!(limits.max_value_depth, 4);
    assert_eq!(limits.max_list_elements, 50);
    assert_eq!(limits.max_record_properties, 25);
    assert_eq!(limits.max_transaction_operations, 100);
    assert_eq!(limits.max_model_count, 50);
    assert_eq!(limits.max_cached_items_per_model, 2000);
    assert_eq!(limits.max_items_per_model_operation, 500);

    // Test with_tree_and_value_limits
    let tree_val_limits = StoreLimits::with_tree_and_value_limits(10, 200, 512, 3, 20, 15);
    assert_eq!(tree_val_limits.max_tree_depth, 10);
    assert_eq!(tree_val_limits.max_node_count, 200);
    assert_eq!(tree_val_limits.max_string_length, 512);
    assert_eq!(tree_val_limits.max_value_depth, 3);
    assert_eq!(tree_val_limits.max_list_elements, 20);
    assert_eq!(tree_val_limits.max_record_properties, 15);
    assert_eq!(
        tree_val_limits.max_transaction_operations,
        DEFAULT_MAX_TRANSACTION_OPERATIONS
    );
    assert_eq!(tree_val_limits.max_model_count, DEFAULT_MAX_MODEL_COUNT);

    // Test true 10-parameter with_all_limits
    let all_limits = StoreLimits::with_all_limits(8, 100, 256, 2, 10, 5, 50, 25, 1_000, 100);
    assert_eq!(all_limits.max_tree_depth, 8);
    assert_eq!(all_limits.max_node_count, 100);
    assert_eq!(all_limits.max_string_length, 256);
    assert_eq!(all_limits.max_value_depth, 2);
    assert_eq!(all_limits.max_list_elements, 10);
    assert_eq!(all_limits.max_record_properties, 5);
    assert_eq!(all_limits.max_transaction_operations, 50);
    assert_eq!(all_limits.max_model_count, 25);
    assert_eq!(all_limits.max_cached_items_per_model, 1000);
    assert_eq!(all_limits.max_items_per_model_operation, 100);
}

#[test]
fn test_dangling_model_ref_rejected_across_all_property_mutation_methods() {
    let mut store = SemanticStore::new();
    let table_type = resolve_standard_node_type("Table").unwrap();
    let model_ref_prop = PropertyRef::MODEL_REF;
    let non_existent_model = ModelId::new(999);

    // 1. create_node with non-existent model_ref must fail
    let create_err = store
        .create_node(
            NodeId::new(1),
            table_type,
            None,
            None,
            vec![(model_ref_prop, Value::from(non_existent_model.get()))],
        )
        .unwrap_err();
    assert_eq!(create_err, StoreError::ModelNotFound(non_existent_model));
    assert!(!store.contains_node(NodeId::new(1)));

    // Create valid node with empty properties
    store
        .create_node(NodeId::new(1), table_type, None, None, vec![])
        .unwrap();

    // 2. set_property with non-existent model_ref must fail
    let set_err = store
        .set_property(
            NodeId::new(1),
            model_ref_prop,
            Value::from(non_existent_model.get()),
        )
        .unwrap_err();
    assert_eq!(set_err, StoreError::ModelNotFound(non_existent_model));

    // 3. batch_property_set with non-existent model_ref must fail
    let batch_err = store
        .batch_property_set(
            NodeId::new(1),
            vec![(model_ref_prop, Value::from(non_existent_model.get()))],
        )
        .unwrap_err();
    assert_eq!(batch_err, StoreError::ModelNotFound(non_existent_model));

    // 4. Create model and verify setting model_ref now succeeds
    store
        .create_model(non_existent_model, table_type, 100)
        .unwrap();
    store
        .set_property(
            NodeId::new(1),
            model_ref_prop,
            Value::from(non_existent_model.get()),
        )
        .unwrap();

    let node = store.get_node(NodeId::new(1)).unwrap();
    assert_eq!(node.model_ref(), Some(non_existent_model));
    assert!(store.get_model_for_node(NodeId::new(1)).is_some());
}
