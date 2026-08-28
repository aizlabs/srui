//! Integration tests for SemanticStore and mutation operations (§6.2, §6.3, §13, §26).

use srui_semantic_tree::{
    NodeId, Property, PropertyRef, SemanticStore, SmallRecord, StoreError, StoreLimits, TypeRef,
    Value,
};

#[test]
fn test_tree_construction_and_hierarchy() {
    let mut store = SemanticStore::new();

    // 1. Create root Surface (#1)
    let root_id = NodeId::new(1);
    store
        .create_node(
            root_id,
            TypeRef::SURFACE,
            None,
            None,
            [(PropertyRef::LABEL, Value::from("App Window"))],
        )
        .expect("create root surface");

    // 2. Create Column (#2) under root (#1)
    let col_id = NodeId::new(2);
    store
        .create_node(
            col_id,
            TypeRef::COLUMN,
            Some(root_id),
            None,
            [(PropertyRef::SPACING_ROLE, Value::from(2u32))],
        )
        .expect("create column");

    // 3. Create Text (#3) and Button (#4) under Column (#2)
    let text_id = NodeId::new(3);
    store
        .create_node(
            text_id,
            TypeRef::TEXT,
            Some(col_id),
            None,
            [(PropertyRef::TEXT, Value::from("Hello SRUI"))],
        )
        .expect("create text");

    let button_id = NodeId::new(4);
    store
        .create_node(
            button_id,
            TypeRef::BUTTON,
            Some(col_id),
            None,
            [
                (PropertyRef::LABEL, Value::from("Click Me")),
                (PropertyRef::ENABLED, Value::from(true)),
            ],
        )
        .expect("create button");

    // Assert shape
    assert_eq!(store.node_count(), 4);
    assert_eq!(store.root_ids(), vec![root_id]);
    assert_eq!(store.children_of(root_id), Some(&[col_id][..]));
    assert_eq!(store.children_of(col_id), Some(&[text_id, button_id][..]));
    assert_eq!(store.children_of(text_id), Some(&[][..]));
    assert_eq!(store.children_of(button_id), Some(&[][..]));

    assert_eq!(store.node_depth(root_id), Some(1));
    assert_eq!(store.node_depth(col_id), Some(2));
    assert_eq!(store.node_depth(text_id), Some(3));
    assert_eq!(store.node_depth(button_id), Some(3));
    assert_eq!(store.subtree_depth(root_id), 3);

    // Verify property access
    let text_node = store.get_node(text_id).expect("get text node");
    assert_eq!(
        text_node.get_property(PropertyRef::TEXT),
        Some(&Value::from("Hello SRUI"))
    );

    let btn_node = store.get_node(button_id).expect("get button node");
    assert_eq!(
        btn_node.get_property(PropertyRef::LABEL),
        Some(&Value::from("Click Me"))
    );
    assert_eq!(
        btn_node.get_property(PropertyRef::ENABLED),
        Some(&Value::from(true))
    );
}

#[test]
fn test_property_mutations_set_clear_batch() {
    let mut store = SemanticStore::new();
    let root_id = NodeId::new(1);
    store
        .create_node(root_id, TypeRef::SURFACE, None, None, [])
        .expect("create root");

    // Set property
    let prev = store
        .set_property(root_id, PropertyRef::LABEL, Value::from("Initial Title"))
        .expect("set property");
    assert_eq!(prev, None);
    assert_eq!(
        store.get_node(root_id).unwrap().get_property(PropertyRef::LABEL),
        Some(&Value::from("Initial Title"))
    );

    // Update property
    let prev = store
        .set_property(root_id, PropertyRef::LABEL, Value::from("Updated Title"))
        .expect("update property");
    assert_eq!(prev, Some(Value::from("Initial Title")));
    assert_eq!(
        store.get_node(root_id).unwrap().get_property(PropertyRef::LABEL),
        Some(&Value::from("Updated Title"))
    );

    // Batch set properties
    store
        .batch_property_set(
            root_id,
            [
                (PropertyRef::ENABLED, Value::from(false)),
                (PropertyRef::BUSY, Value::from(true)),
            ],
        )
        .expect("batch set");
    let node = store.get_node(root_id).unwrap();
    assert_eq!(node.get_property(PropertyRef::ENABLED), Some(&Value::from(false)));
    assert_eq!(node.get_property(PropertyRef::BUSY), Some(&Value::from(true)));

    // Clear property
    let removed = store
        .clear_property(root_id, PropertyRef::BUSY)
        .expect("clear property");
    assert_eq!(removed, Some(Value::from(true)));
    assert_eq!(store.get_node(root_id).unwrap().get_property(PropertyRef::BUSY), None);
}

#[test]
fn test_reorder_children() {
    let mut store = SemanticStore::new();
    let root_id = NodeId::new(1);
    let col_id = NodeId::new(2);
    let c1 = NodeId::new(10);
    let c2 = NodeId::new(20);
    let c3 = NodeId::new(30);

    store.create_node(root_id, TypeRef::SURFACE, None, None, []).unwrap();
    store.create_node(col_id, TypeRef::COLUMN, Some(root_id), None, []).unwrap();
    store.create_node(c1, TypeRef::TEXT, Some(col_id), None, []).unwrap();
    store.create_node(c2, TypeRef::TEXT, Some(col_id), None, []).unwrap();
    store.create_node(c3, TypeRef::TEXT, Some(col_id), None, []).unwrap();

    assert_eq!(store.children_of(col_id), Some(&[c1, c2, c3][..]));

    // Reorder children to [c3, c1, c2]
    store
        .reorder_children(col_id, &[c3, c1, c2])
        .expect("valid reorder");
    assert_eq!(store.children_of(col_id), Some(&[c3, c1, c2][..]));

    // Reject mismatch length
    let err = store
        .reorder_children(col_id, &[c3, c1])
        .expect_err("mismatched length");
    assert!(matches!(err, StoreError::InvalidChildrenReorder { .. }));

    // Reject foreign child ID
    let foreign = NodeId::new(999);
    let err = store
        .reorder_children(col_id, &[c3, c1, foreign])
        .expect_err("foreign child");
    assert!(matches!(err, StoreError::InvalidChildrenReorder { .. }));

    // Reject duplicates
    let err = store
        .reorder_children(col_id, &[c3, c1, c1])
        .expect_err("duplicate child in list");
    assert!(matches!(err, StoreError::InvalidChildrenReorder { .. }));

    // State remains unaffected by rejected reorders
    assert_eq!(store.children_of(col_id), Some(&[c3, c1, c2][..]));
}

#[test]
fn test_move_node() {
    let mut store = SemanticStore::new();
    let root_id = NodeId::new(1);
    let col1_id = NodeId::new(2);
    let col2_id = NodeId::new(3);
    let item_id = NodeId::new(4);
    let item_child_id = NodeId::new(5);

    store.create_node(root_id, TypeRef::SURFACE, None, None, []).unwrap();
    store.create_node(col1_id, TypeRef::COLUMN, Some(root_id), None, []).unwrap();
    store.create_node(col2_id, TypeRef::COLUMN, Some(root_id), None, []).unwrap();
    store.create_node(item_id, TypeRef::ROW, Some(col1_id), None, []).unwrap();
    store.create_node(item_child_id, TypeRef::TEXT, Some(item_id), None, []).unwrap();

    assert_eq!(store.children_of(col1_id), Some(&[item_id][..]));
    assert_eq!(store.children_of(col2_id), Some(&[][..]));
    assert_eq!(store.node_depth(item_child_id), Some(4));

    // Move item_id from col1 to col2
    store
        .move_node(item_id, Some(col2_id), None)
        .expect("move item to col2");

    assert_eq!(store.children_of(col1_id), Some(&[][..]));
    assert_eq!(store.children_of(col2_id), Some(&[item_id][..]));
    assert_eq!(store.parent_of(item_id), Some(Some(col2_id)));
    assert_eq!(store.node_depth(item_child_id), Some(4));

    // Move item_id to root (parent = None)
    store
        .move_node(item_id, None, None)
        .expect("move item to root");
    assert_eq!(store.children_of(col2_id), Some(&[][..]));
    assert_eq!(store.parent_of(item_id), Some(None));
    assert_eq!(store.node_depth(item_id), Some(1));
    assert_eq!(store.node_depth(item_child_id), Some(2));
}

#[test]
fn test_move_node_same_parent_append_index() {
    let mut store = SemanticStore::new();
    let root_id = NodeId::new(1);
    let col_id = NodeId::new(2);
    let c1 = NodeId::new(3);
    let c2 = NodeId::new(4);
    let c3 = NodeId::new(5);

    store.create_node(root_id, TypeRef::SURFACE, None, None, []).unwrap();
    store.create_node(col_id, TypeRef::COLUMN, Some(root_id), None, []).unwrap();
    store.create_node(c1, TypeRef::TEXT, Some(col_id), Some(0), []).unwrap();
    store.create_node(c2, TypeRef::TEXT, Some(col_id), Some(1), []).unwrap();
    store.create_node(c3, TypeRef::TEXT, Some(col_id), Some(2), []).unwrap();

    assert_eq!(store.children_of(col_id), Some(&[c1, c2, c3][..]));

    let append_index = store.children_of(col_id).unwrap().len();
    store
        .move_node(c2, Some(col_id), Some(append_index))
        .expect("same-parent append index should be valid");

    assert_eq!(store.children_of(col_id), Some(&[c1, c3, c2][..]));
}

#[test]
fn test_move_node_cycle_prevention() {
    let mut store = SemanticStore::new();
    let root_id = NodeId::new(1);
    let parent_id = NodeId::new(2);
    let child_id = NodeId::new(3);
    let grandchild_id = NodeId::new(4);

    store.create_node(root_id, TypeRef::SURFACE, None, None, []).unwrap();
    store.create_node(parent_id, TypeRef::COLUMN, Some(root_id), None, []).unwrap();
    store.create_node(child_id, TypeRef::ROW, Some(parent_id), None, []).unwrap();
    store.create_node(grandchild_id, TypeRef::TEXT, Some(child_id), None, []).unwrap();

    // Moving parent under itself -> Cycle
    let err = store
        .move_node(parent_id, Some(parent_id), None)
        .expect_err("move under self");
    assert_eq!(
        err,
        StoreError::CycleDetected {
            node_id: parent_id,
            target_parent: parent_id
        }
    );

    // Moving parent under its grandchild -> Cycle
    let err = store
        .move_node(parent_id, Some(grandchild_id), None)
        .expect_err("move under grandchild");
    assert_eq!(
        err,
        StoreError::CycleDetected {
            node_id: parent_id,
            target_parent: grandchild_id
        }
    );

    // Tree structure remains unmodified
    assert_eq!(store.parent_of(parent_id), Some(Some(root_id)));
    assert_eq!(store.parent_of(grandchild_id), Some(Some(child_id)));
}

#[test]
fn test_delete_node_recursive_subtree_cleanup() {
    let mut store = SemanticStore::new();
    let root_id = NodeId::new(1);
    let col_id = NodeId::new(2);
    let c1 = NodeId::new(3);
    let c2 = NodeId::new(4);
    let gc1 = NodeId::new(5);

    store.create_node(root_id, TypeRef::SURFACE, None, None, []).unwrap();
    store.create_node(col_id, TypeRef::COLUMN, Some(root_id), None, []).unwrap();
    store.create_node(c1, TypeRef::ROW, Some(col_id), None, []).unwrap();
    store.create_node(gc1, TypeRef::TEXT, Some(c1), None, []).unwrap();
    store.create_node(c2, TypeRef::BUTTON, Some(col_id), None, []).unwrap();

    assert_eq!(store.node_count(), 5);

    // Delete subtree rooted at c1 (#3)
    let deleted = store.delete_node(c1).expect("delete c1");
    assert_eq!(deleted.len(), 2);
    assert!(deleted.contains(&c1));
    assert!(deleted.contains(&gc1));

    // Active nodes count is reduced
    assert_eq!(store.node_count(), 3);
    assert!(!store.contains_node(c1));
    assert!(!store.contains_node(gc1));
    assert!(store.contains_node(c2));
    assert_eq!(store.children_of(col_id), Some(&[c2][..]));
}

#[test]
fn test_node_id_cannot_be_reused_even_after_deletion() {
    let mut store = SemanticStore::new();
    let root_id = NodeId::new(1);
    let item_id = NodeId::new(42);

    store.create_node(root_id, TypeRef::SURFACE, None, None, []).unwrap();
    store
        .create_node(item_id, TypeRef::BUTTON, Some(root_id), None, [])
        .unwrap();

    // 1. Attempting duplicate CREATE_NODE while active is rejected (§6.2)
    let err = store
        .create_node(item_id, TypeRef::BUTTON, Some(root_id), None, [])
        .expect_err("reusing active id must fail");
    assert_eq!(err, StoreError::NodeIdAlreadyUsed(item_id));

    // 2. Delete the node
    store.delete_node(item_id).expect("delete item");
    assert!(!store.contains_node(item_id));
    assert!(store.is_id_used(item_id));

    // 3. Attempting to create a node with the deleted ID is STILL rejected (§6.2)
    let err = store
        .create_node(item_id, TypeRef::TEXT, Some(root_id), None, [])
        .expect_err("reusing deleted id must fail per §6.2");
    assert_eq!(err, StoreError::NodeIdAlreadyUsed(item_id));
}

#[test]
fn test_create_under_nonexistent_parent_rejected() {
    let mut store = SemanticStore::new();
    let fake_parent = NodeId::new(999);
    let child_id = NodeId::new(1);

    let err = store
        .create_node(child_id, TypeRef::BUTTON, Some(fake_parent), None, [])
        .expect_err("nonexistent parent rejected");

    assert_eq!(err, StoreError::ParentNotFound(fake_parent));
    assert!(store.is_empty());
    // Since creation failed during validation, the id is not marked used
    assert!(!store.is_id_used(child_id));
}

#[test]
fn test_max_tree_depth_enforced_and_store_unchanged() {
    // Limit max tree depth to 3
    let limits = StoreLimits::new(3, 1000, 1024);
    let mut store = SemanticStore::with_limits(limits);

    let n1 = NodeId::new(1); // depth 1
    let n2 = NodeId::new(2); // depth 2
    let n3 = NodeId::new(3); // depth 3
    let n4 = NodeId::new(4); // depth 4 -> should fail

    store.create_node(n1, TypeRef::SURFACE, None, None, []).unwrap();
    store.create_node(n2, TypeRef::COLUMN, Some(n1), None, []).unwrap();
    store.create_node(n3, TypeRef::ROW, Some(n2), None, []).unwrap();

    assert_eq!(store.node_depth(n3), Some(3));
    assert_eq!(store.node_count(), 3);

    // Creating n4 under n3 would produce depth 4 > 3
    let err = store
        .create_node(n4, TypeRef::TEXT, Some(n3), None, [])
        .expect_err("depth limit exceeded");

    assert_eq!(
        err,
        StoreError::MaxTreeDepthExceeded {
            limit: 3,
            actual: 4
        }
    );

    // Store is left completely unchanged
    assert_eq!(store.node_count(), 3);
    assert!(!store.contains_node(n4));
    assert!(!store.is_id_used(n4));
    assert_eq!(store.children_of(n3), Some(&[][..]));

    // Moving a subtree that would exceed max depth is also rejected
    let other_root = NodeId::new(10); // depth 1
    let other_child = NodeId::new(11); // depth 2
    store
        .create_node(other_root, TypeRef::SURFACE, None, None, [])
        .unwrap();
    store
        .create_node(other_child, TypeRef::BUTTON, Some(other_root), None, [])
        .unwrap();

    // moving other_root (height 2) under n3 (depth 3) would result in depth 3 + 2 = 5 > 3
    let err = store
        .move_node(other_root, Some(n3), None)
        .expect_err("move exceeding depth limit rejected");
    assert!(matches!(err, StoreError::MaxTreeDepthExceeded { .. }));
    assert_eq!(store.parent_of(other_root), Some(None));
}

#[test]
fn test_max_node_count_enforced_and_store_unchanged() {
    // Limit max node count to 2
    let limits = StoreLimits::new(64, 2, 1024);
    let mut store = SemanticStore::with_limits(limits);

    let n1 = NodeId::new(1);
    let n2 = NodeId::new(2);
    let n3 = NodeId::new(3);

    store.create_node(n1, TypeRef::SURFACE, None, None, []).unwrap();
    store.create_node(n2, TypeRef::BUTTON, Some(n1), None, []).unwrap();

    assert_eq!(store.node_count(), 2);

    // Adding 3rd node exceeds limit of 2
    let err = store
        .create_node(n3, TypeRef::TEXT, Some(n1), None, [])
        .expect_err("node count limit exceeded");

    assert_eq!(
        err,
        StoreError::MaxNodeCountExceeded {
            limit: 2,
            current: 2
        }
    );

    // Store is left completely unchanged
    assert_eq!(store.node_count(), 2);
    assert!(!store.contains_node(n3));
    assert!(!store.is_id_used(n3));
    assert_eq!(store.children_of(n1), Some(&[n2][..]));
}

#[test]
fn test_max_string_length_enforced_and_store_unchanged() {
    // Limit string length to 10 bytes
    let limits = StoreLimits::new(64, 1000, 10);
    let mut store = SemanticStore::with_limits(limits);

    let n1 = NodeId::new(1);

    // Valid string (<= 10 bytes)
    store
        .create_node(
            n1,
            TypeRef::SURFACE,
            None,
            None,
            [(PropertyRef::LABEL, Value::from("Short"))],
        )
        .expect("valid short string");

    // Exceeding on set_property
    let err = store
        .set_property(
            n1,
            PropertyRef::LABEL,
            Value::from("This is way too long string!"),
        )
        .expect_err("string limit exceeded");

    assert!(matches!(
        err,
        StoreError::MaxStringLengthExceeded {
            limit: 10,
            actual: 28
        }
    ));

    // Property in store is unchanged
    assert_eq!(
        store.get_node(n1).unwrap().get_property(PropertyRef::LABEL),
        Some(&Value::from("Short"))
    );

    // Exceeding on create_node
    let n2 = NodeId::new(2);
    let err = store
        .create_node(
            n2,
            TypeRef::TEXT,
            Some(n1),
            None,
            [(PropertyRef::TEXT, Value::from("Oversized string payload"))],
        )
        .expect_err("oversized create string");

    assert!(matches!(err, StoreError::MaxStringLengthExceeded { .. }));
    assert!(!store.contains_node(n2));
    assert!(!store.is_id_used(n2));
}

#[test]
fn test_apply_protobuf_wire_operations() {
    let mut store = SemanticStore::new();

    // 1. CreateNodeOp
    let create_op = srui_protocol::Operation {
        op: Some(srui_protocol::operation::Op::CreateNode(
            srui_protocol::CreateNodeOp {
                node: Some(srui_protocol::NodeRecord {
                    node_id: 100,
                    r#type: Some(TypeRef::SURFACE.into()),
                    parent_id: 0,
                    child_index: 0,
                    properties: vec![srui_protocol::Property {
                        property: Some(PropertyRef::LABEL.into()),
                        value: Some(Value::from("Wire Surface").into()),
                    }],
                }),
            },
        )),
    };
    store.apply_operation(&create_op).expect("apply create_op");
    assert_eq!(store.node_count(), 1);

    // 2. SetPropertyOp
    let set_op = srui_protocol::Operation {
        op: Some(srui_protocol::operation::Op::SetProperty(
            srui_protocol::SetPropertyOp {
                node_id: 100,
                property: Some(PropertyRef::ENABLED.into()),
                value: Some(Value::from(true).into()),
            },
        )),
    };
    store.apply_operation(&set_op).expect("apply set_op");
    assert_eq!(
        store.get_node(NodeId::new(100)).unwrap().get_property(PropertyRef::ENABLED),
        Some(&Value::from(true))
    );

    // 3. ClearPropertyOp
    let clear_op = srui_protocol::Operation {
        op: Some(srui_protocol::operation::Op::ClearProperty(
            srui_protocol::ClearPropertyOp {
                node_id: 100,
                property: Some(PropertyRef::ENABLED.into()),
            },
        )),
    };
    store.apply_operation(&clear_op).expect("apply clear_op");
    assert_eq!(
        store.get_node(NodeId::new(100)).unwrap().get_property(PropertyRef::ENABLED),
        None
    );

    // 4. DeleteNodeOp
    let del_op = srui_protocol::Operation {
        op: Some(srui_protocol::operation::Op::DeleteNode(
            srui_protocol::DeleteNodeOp { node_id: 100 },
        )),
    };
    store.apply_operation(&del_op).expect("apply del_op");
    assert!(store.is_empty());
    assert!(store.is_id_used(NodeId::new(100)));

    // 5. Create children with exact index vs append sentinel (u32::MAX)
    let root_op = srui_protocol::Operation {
        op: Some(srui_protocol::operation::Op::CreateNode(
            srui_protocol::CreateNodeOp {
                node: Some(srui_protocol::NodeRecord {
                    node_id: 1,
                    r#type: Some(TypeRef::SURFACE.into()),
                    parent_id: 0,
                    child_index: 0,
                    properties: vec![],
                }),
            },
        )),
    };
    store.apply_operation(&root_op).expect("create root");

    // First child at index 0
    let child1 = srui_protocol::Operation {
        op: Some(srui_protocol::operation::Op::CreateNode(
            srui_protocol::CreateNodeOp {
                node: Some(srui_protocol::NodeRecord {
                    node_id: 2,
                    r#type: Some(TypeRef::BUTTON.into()),
                    parent_id: 1,
                    child_index: 0,
                    properties: vec![],
                }),
            },
        )),
    };
    store.apply_operation(&child1).expect("create child1 at 0");

    // Second child appended using u32::MAX sentinel
    let child2 = srui_protocol::Operation {
        op: Some(srui_protocol::operation::Op::CreateNode(
            srui_protocol::CreateNodeOp {
                node: Some(srui_protocol::NodeRecord {
                    node_id: 3,
                    r#type: Some(TypeRef::TEXT.into()),
                    parent_id: 1,
                    child_index: u32::MAX,
                    properties: vec![],
                }),
            },
        )),
    };
    store.apply_operation(&child2).expect("create child2 with append sentinel");
    assert_eq!(
        store.children_of(NodeId::new(1)),
        Some(&[NodeId::new(2), NodeId::new(3)][..])
    );

    // Third child inserted at index 1 (between 2 and 3)
    let child3 = srui_protocol::Operation {
        op: Some(srui_protocol::operation::Op::CreateNode(
            srui_protocol::CreateNodeOp {
                node: Some(srui_protocol::NodeRecord {
                    node_id: 4,
                    r#type: Some(TypeRef::TOGGLE.into()),
                    parent_id: 1,
                    child_index: 1,
                    properties: vec![],
                }),
            },
        )),
    };
    store.apply_operation(&child3).expect("create child3 at 1");
    assert_eq!(
        store.children_of(NodeId::new(1)),
        Some(&[NodeId::new(2), NodeId::new(4), NodeId::new(3)][..])
    );

    // Move child 2 to end using append sentinel
    let move_op = srui_protocol::Operation {
        op: Some(srui_protocol::operation::Op::MoveNode(
            srui_protocol::MoveNodeOp {
                node_id: 2,
                new_parent_id: 1,
                new_child_index: u32::MAX,
            },
        )),
    };
    store.apply_operation(&move_op).expect("move child 2 to end");
    assert_eq!(
        store.children_of(NodeId::new(1)),
        Some(&[NodeId::new(4), NodeId::new(3), NodeId::new(2)][..])
    );
}

#[test]
fn test_wire_create_root_nodes_preserve_explicit_child_index() {
    let mut store = SemanticStore::new();

    // Second root inserted at index 0 (prepend)
    let root_b = srui_protocol::Operation {
        op: Some(srui_protocol::operation::Op::CreateNode(
            srui_protocol::CreateNodeOp {
                node: Some(srui_protocol::NodeRecord {
                    node_id: 2,
                    r#type: Some(TypeRef::SURFACE.into()),
                    parent_id: 0,
                    child_index: 0,
                    properties: vec![],
                }),
            },
        )),
    };
    store.apply_operation(&root_b).expect("create root B at index 0");

    // First root appended at index 1
    let root_a = srui_protocol::Operation {
        op: Some(srui_protocol::operation::Op::CreateNode(
            srui_protocol::CreateNodeOp {
                node: Some(srui_protocol::NodeRecord {
                    node_id: 1,
                    r#type: Some(TypeRef::SURFACE.into()),
                    parent_id: 0,
                    child_index: 1,
                    properties: vec![],
                }),
            },
        )),
    };
    store.apply_operation(&root_a).expect("create root A at index 1");

    assert_eq!(
        store.root_ids(),
        &[NodeId::new(2), NodeId::new(1)],
        "wire root child_index must be preserved for multi-root ordering"
    );
}

#[test]
fn test_nested_value_depth_limit_enforced() {
    let limits = StoreLimits::with_tree_and_value_limits(64, 1000, 1024, 3, 100, 100);
    let mut store = SemanticStore::with_limits(limits);

    // Depth 1: List containing scalar
    let val_depth_1 = Value::List(vec![Value::from(42i64)]);
    store
        .create_node(
            NodeId::new(1),
            TypeRef::SURFACE,
            None,
            None,
            [(PropertyRef::VALUE, val_depth_1)],
        )
        .expect("depth 1 list ok");

    // Depth 3: List -> List -> List -> scalar (depth 4 when inspecting inner)
    let nested_val = Value::List(vec![Value::List(vec![Value::List(vec![Value::List(vec![
        Value::from(1i64),
    ])])])]);

    let err = store
        .set_property(NodeId::new(1), PropertyRef::VALUE, nested_val)
        .expect_err("nested value depth limit must fail");

    assert!(matches!(
        err,
        StoreError::MaxValueDepthExceeded { limit: 3, actual: 4 }
    ));
}

#[test]
fn test_max_list_elements_limit_enforced() {
    let limits = StoreLimits::with_tree_and_value_limits(64, 1000, 1024, 10, 3, 100);
    let mut store = SemanticStore::with_limits(limits);

    let list_ok = Value::List(vec![Value::from(1i64), Value::from(2i64), Value::from(3i64)]);
    store
        .create_node(
            NodeId::new(1),
            TypeRef::SURFACE,
            None,
            None,
            [(PropertyRef::ITEMS, list_ok)],
        )
        .expect("3 items list ok");

    let list_too_long = Value::List(vec![
        Value::from(1i64),
        Value::from(2i64),
        Value::from(3i64),
        Value::from(4i64),
    ]);
    let err = store
        .set_property(NodeId::new(1), PropertyRef::ITEMS, list_too_long)
        .expect_err("exceeding max list elements must fail");

    assert_eq!(
        err,
        StoreError::MaxListLengthExceeded { limit: 3, actual: 4 }
    );
}

#[test]
fn test_max_record_properties_limit_enforced() {
    let limits = StoreLimits::with_tree_and_value_limits(64, 1000, 1024, 10, 100, 2);
    let mut store = SemanticStore::with_limits(limits);

    let record_too_many_props = Value::Record(SmallRecord::new(
        TypeRef::standard(1),
        vec![
            Property::new(PropertyRef::standard(1), Value::from("A")),
            Property::new(PropertyRef::standard(2), Value::from("B")),
            Property::new(PropertyRef::standard(3), Value::from("C")),
        ],
    ));

    let err = store
        .create_node(
            NodeId::new(1),
            TypeRef::SURFACE,
            None,
            None,
            [(PropertyRef::VALUE, record_too_many_props)],
        )
        .expect_err("exceeding record properties limit must fail");

    assert_eq!(
        err,
        StoreError::MaxRecordPropertiesExceeded { limit: 2, actual: 3 }
    );
}
