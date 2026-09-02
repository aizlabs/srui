//! Integration tests for SemanticStore transactions and atomic revisions (§12, §12.1, §12.2, §26).

use srui_semantic_tree::{
    CoalescedScalarDelta, DeliveredTransaction, NodeId, Operation, PropertyRef, Revision,
    SemanticStore, StoreError, StoreLimits, Transaction, TxnError, TypeRef, Value,
};

#[test]
fn test_valid_transaction_commits_and_advances_revision_by_one() {
    let mut store = SemanticStore::new();
    assert_eq!(store.revision(), Revision::INITIAL);
    assert_eq!(store.node_count(), 0);

    // 1. Transaction 1 (base = 0 -> new = 1): Create root Surface and Column layout
    let root_id = NodeId::new(1);
    let col_id = NodeId::new(2);
    let btn_id = NodeId::new(3);

    let ops1 = vec![
        Operation::create_node(
            root_id,
            TypeRef::SURFACE,
            None,
            None,
            [(PropertyRef::LABEL, Value::from("Main Window"))],
        ),
        Operation::create_node(
            col_id,
            TypeRef::COLUMN,
            Some(root_id),
            None,
            [(PropertyRef::SPACING_ROLE, Value::from(1u32))],
        ),
        Operation::create_node(
            btn_id,
            TypeRef::BUTTON,
            Some(col_id),
            None,
            [
                (PropertyRef::LABEL, Value::from("Submit")),
                (PropertyRef::ENABLED, Value::from(true)),
            ],
        ),
    ];

    let new_rev1 = store
        .apply_transaction(Revision::INITIAL, ops1)
        .expect("transaction 1 should commit cleanly");

    assert_eq!(new_rev1, Revision::new(1));
    assert_eq!(store.revision(), Revision::new(1));
    assert_eq!(store.node_count(), 3);
    assert_eq!(store.root_ids(), &[root_id]);
    assert_eq!(store.children_of(root_id), Some(&[col_id][..]));
    assert_eq!(store.children_of(col_id), Some(&[btn_id][..]));

    let btn_node = store.get_node(btn_id).expect("button node exists");
    assert_eq!(
        btn_node.get_property(PropertyRef::LABEL),
        Some(&Value::from("Submit"))
    );
    assert_eq!(
        btn_node.get_property(PropertyRef::ENABLED),
        Some(&Value::from(true))
    );

    // 2. Transaction 2 (base = 1 -> new = 2): Mutate properties and add a Text node
    let text_id = NodeId::new(4);
    let ops2 = vec![
        Operation::set_property(btn_id, PropertyRef::ENABLED, false),
        Operation::create_node(
            text_id,
            TypeRef::TEXT,
            Some(col_id),
            Some(0), // insert at beginning of column
            [(PropertyRef::TEXT, Value::from("Status: Processing"))],
        ),
    ];

    let new_rev2 = store
        .apply_transaction(Revision::new(1), ops2)
        .expect("transaction 2 should commit cleanly");

    assert_eq!(new_rev2, Revision::new(2));
    assert_eq!(store.revision(), Revision::new(2));
    assert_eq!(store.node_count(), 4);
    assert_eq!(store.children_of(col_id), Some(&[text_id, btn_id][..]));

    let updated_btn = store.get_node(btn_id).unwrap();
    assert_eq!(
        updated_btn.get_property(PropertyRef::ENABLED),
        Some(&Value::from(false))
    );
}

#[test]
fn test_transaction_with_invalid_last_op_aborts_with_zero_side_effects() {
    let mut store = SemanticStore::new();

    // Setup initial valid state at Revision 1
    let root_id = NodeId::new(1);
    store
        .apply_transaction(
            Revision::INITIAL,
            vec![Operation::create_node(
                root_id,
                TypeRef::SURFACE,
                None,
                None,
                [(PropertyRef::LABEL, Value::from("Initial Title"))],
            )],
        )
        .expect("initial setup txn");

    assert_eq!(store.revision(), Revision::new(1));
    assert_eq!(store.node_count(), 1);

    // Transaction with 3 ops where op 1 and op 2 are valid, but op 3 fails on nonexistent node
    let col_id = NodeId::new(2);
    let nonexistent = NodeId::new(999);

    let failing_ops = vec![
        Operation::create_node(col_id, TypeRef::COLUMN, Some(root_id), None, []),
        Operation::set_property(root_id, PropertyRef::LABEL, "Modified Title"),
        Operation::set_property(nonexistent, PropertyRef::TEXT, "Fail here"),
    ];

    let err = store
        .apply_transaction(Revision::new(1), failing_ops)
        .expect_err("transaction with invalid last op must fail");

    assert_eq!(
        err,
        TxnError::OpFailed {
            op_index: 2,
            source: StoreError::NodeNotFound(nonexistent),
        }
    );

    // Verify ZERO visible side effects and unadvanced revision (§12.1)
    assert_eq!(store.revision(), Revision::new(1));
    assert_eq!(store.node_count(), 1);
    assert!(!store.contains_node(col_id));
    assert!(!store.is_id_used(col_id)); // NodeId(2) must not be consumed by aborted txn
    assert_eq!(store.children_of(root_id), Some(&[][..]));

    let root_node = store.get_node(root_id).unwrap();
    assert_eq!(
        root_node.get_property(PropertyRef::LABEL),
        Some(&Value::from("Initial Title"))
    );

    // Verify that a subsequent valid transaction using NodeId(2) succeeds completely
    let recovery_ops = vec![Operation::create_node(
        col_id,
        TypeRef::COLUMN,
        Some(root_id),
        None,
        [],
    )];
    let new_rev = store
        .apply_transaction(Revision::new(1), recovery_ops)
        .expect("subsequent valid transaction must succeed");

    assert_eq!(new_rev, Revision::new(2));
    assert_eq!(store.revision(), Revision::new(2));
    assert_eq!(store.node_count(), 2);
    assert!(store.contains_node(col_id));
}

#[test]
fn test_transaction_with_stale_or_wrong_base_revision_rejected() {
    let mut store = SemanticStore::new();

    // Commit transaction 1
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
        .expect("txn 1");

    assert_eq!(store.revision(), Revision::new(1));

    // 1. Submit with stale base_revision (0 when current is 1)
    let stale_err = store
        .apply_transaction(
            Revision::new(0),
            vec![Operation::create_node(
                NodeId::new(2),
                TypeRef::BUTTON,
                Some(NodeId::new(1)),
                None,
                [],
            )],
        )
        .expect_err("stale base revision must be rejected");

    assert_eq!(
        stale_err,
        TxnError::StaleBaseRevision {
            expected: Revision::new(1),
            actual: Revision::new(0),
        }
    );

    // 2. Submit with future / skipped base_revision (5 when current is 1)
    let future_err = store
        .apply_transaction(
            Revision::new(5),
            vec![Operation::create_node(
                NodeId::new(2),
                TypeRef::BUTTON,
                Some(NodeId::new(1)),
                None,
                [],
            )],
        )
        .expect_err("future base revision must be rejected");

    assert_eq!(
        future_err,
        TxnError::StaleBaseRevision {
            expected: Revision::new(1),
            actual: Revision::new(5),
        }
    );

    // Store state and revision remain unchanged
    assert_eq!(store.revision(), Revision::new(1));
    assert_eq!(store.node_count(), 1);
    assert!(!store.contains_node(NodeId::new(2)));
}

#[test]
fn test_max_operations_limit_enforced_as_precheck() {
    // Configure store with max_transaction_operations = 3
    let limits = StoreLimits::default().with_max_transaction_operations(3);
    let mut store = SemanticStore::with_limits(limits);

    // Submit transaction with 4 operations (exceeds limit of 3)
    let oversized_ops = vec![
        Operation::create_node(NodeId::new(1), TypeRef::SURFACE, None, None, []),
        Operation::create_node(
            NodeId::new(2),
            TypeRef::COLUMN,
            Some(NodeId::new(1)),
            None,
            [],
        ),
        Operation::create_node(
            NodeId::new(3),
            TypeRef::BUTTON,
            Some(NodeId::new(2)),
            None,
            [],
        ),
        Operation::create_node(
            NodeId::new(4),
            TypeRef::TEXT,
            Some(NodeId::new(2)),
            None,
            [],
        ),
    ];

    let err = store
        .apply_transaction(Revision::INITIAL, oversized_ops)
        .expect_err("oversized transaction must be rejected by precheck");

    assert_eq!(
        err,
        TxnError::MaxOperationsExceeded {
            limit: 3,
            actual: 4,
        }
    );

    // Pre-check prevents any execution: store is completely untouched
    assert_eq!(store.revision(), Revision::INITIAL);
    assert!(store.is_empty());
    assert!(!store.is_id_used(NodeId::new(1)));
    assert!(!store.is_id_used(NodeId::new(2)));
}

#[test]
fn test_transaction_record_with_invalid_new_revision_rejected() {
    let mut store = SemanticStore::new();

    let root_id = NodeId::new(1);
    let ops = vec![Operation::create_node(
        root_id,
        TypeRef::SURFACE,
        None,
        None,
        [],
    )];

    // Transaction specifies new_revision = 5 instead of expected 1
    let invalid_txn = Transaction::with_revisions(Revision::new(0), Revision::new(5), ops, 0);

    let err = store
        .apply_transaction_record(&invalid_txn)
        .expect_err("transaction with non-increment new_revision must fail");

    assert_eq!(
        err,
        TxnError::InvalidNewRevision {
            expected: Revision::new(1),
            actual: Revision::new(5),
        }
    );

    assert_eq!(store.revision(), Revision::INITIAL);
    assert!(store.is_empty());
}

/// A coalesced scalar span is a *delivery* form: a replica applies it and advances several
/// revisions at once, while the authoritative entry points refuse it (§12.1, §20.4).
#[test]
fn test_coalesced_scalar_delta_applies_to_replica_but_not_authoritative_path() {
    let mut store = SemanticStore::new();
    let root_id = NodeId::new(1);

    // Rev 0 -> 1: Create node
    store
        .apply_transaction(
            Revision::INITIAL,
            vec![Operation::create_node(
                root_id,
                TypeRef::SURFACE,
                None,
                None,
                [(PropertyRef::LABEL, Value::from("v0"))],
            )],
        )
        .expect("initial setup");
    assert_eq!(store.revision(), Revision::new(1));

    // Rev 1 -> 5: coalesced run of scalar updates (§12.1 delivery forms, §20.4)
    let span_txn = Transaction::with_revisions(
        Revision::new(1),
        Revision::new(5),
        vec![Operation::SetProperty {
            id: root_id,
            property: PropertyRef::LABEL,
            value: Value::from("v5"),
        }],
        0,
    );

    let err = store
        .apply_transaction_record(&span_txn)
        .expect_err("the authoritative path must advance exactly one revision");
    assert_eq!(
        err,
        TxnError::InvalidNewRevision {
            expected: Revision::new(2),
            actual: Revision::new(5),
        }
    );
    assert_eq!(
        store.revision(),
        Revision::new(1),
        "a refused commit must leave the store on its committed revision"
    );

    let delta = CoalescedScalarDelta::try_from(span_txn).expect("scalar span is a valid delta");
    let committed = store
        .apply_coalesced_delta(&delta)
        .expect("a replica must accept a coalesced scalar delta");
    assert_eq!(committed, Revision::new(5));
    assert_eq!(store.revision(), Revision::new(5));
    assert_eq!(
        store
            .get_node(root_id)
            .unwrap()
            .get_property(PropertyRef::LABEL),
        Some(&Value::from("v5"))
    );
}

/// A structural operation is a coalescing barrier: it may never ride inside a multi-revision
/// delivery span, on either the authoritative or the replica path (§20.4).
#[test]
fn test_structural_span_is_refused_as_a_delta() {
    let structural_span = Transaction::with_revisions(
        Revision::new(1),
        Revision::new(5),
        vec![Operation::create_node(
            NodeId::new(2),
            TypeRef::SURFACE,
            None,
            None,
            [],
        )],
        0,
    );

    let err = CoalescedScalarDelta::try_from(structural_span)
        .expect_err("a structural operation must not be coalesced across revisions");
    assert_eq!(
        err,
        TxnError::InvalidNewRevision {
            expected: Revision::new(2),
            actual: Revision::new(5),
        }
    );
}

/// The delivered entry point classifies by shape and refuses anything that is neither form.
#[test]
fn test_delivered_transaction_classifies_commit_and_delta() {
    let root_id = NodeId::new(1);

    let commit = Transaction::new(
        Revision::INITIAL,
        vec![Operation::create_node(
            root_id,
            TypeRef::SURFACE,
            None,
            None,
            [(PropertyRef::LABEL, Value::from("v0"))],
        )],
    );
    assert!(matches!(
        DeliveredTransaction::try_from(commit).expect("single-step frame"),
        DeliveredTransaction::Commit(_)
    ));

    let delta = Transaction::with_revisions(
        Revision::new(1),
        Revision::new(5),
        vec![Operation::SetProperty {
            id: root_id,
            property: PropertyRef::LABEL,
            value: Value::from("v5"),
        }],
        0,
    );
    assert!(matches!(
        DeliveredTransaction::try_from(delta).expect("scalar span frame"),
        DeliveredTransaction::Delta(_)
    ));

    let backwards = Transaction::with_revisions(Revision::new(5), Revision::new(2), vec![], 0);
    assert!(
        DeliveredTransaction::try_from(backwards).is_err(),
        "a span that is neither form must be rejected rather than guessed at"
    );
}

/// Performing the merge moved to the outbound queue that owns the policy
/// (`srui_sessiond::outbound::coalesce`, where its semantics and §26 limits are tested). The model
/// keeps only the predicate saying which transactions are eligible (§12.1, §20.4).
#[test]
fn test_is_coalesceable_admits_only_non_empty_scalar_property_runs() {
    let node_id = NodeId::new(10);

    let scalar = Transaction::with_revisions(
        Revision::new(1),
        Revision::new(2),
        vec![Operation::SetProperty {
            id: node_id,
            property: PropertyRef::LABEL,
            value: Value::from("first"),
        }],
        0,
    );
    assert!(scalar.is_coalesceable());

    let structural = Transaction::with_revisions(
        Revision::new(1),
        Revision::new(2),
        vec![Operation::DeleteNode { id: node_id }],
        0,
    );
    assert!(!structural.is_coalesceable());

    let mixed = Transaction::with_revisions(
        Revision::new(1),
        Revision::new(2),
        vec![
            Operation::SetProperty {
                id: node_id,
                property: PropertyRef::LABEL,
                value: Value::from("first"),
            },
            Operation::DeleteNode { id: node_id },
        ],
        0,
    );
    assert!(
        !mixed.is_coalesceable(),
        "one structural operation disqualifies the whole transaction"
    );

    let empty = Transaction::with_revisions(Revision::new(1), Revision::new(2), vec![], 0);
    assert!(
        !empty.is_coalesceable(),
        "an empty transaction carries no scalar state to collapse"
    );

    let value_change = Transaction::with_revisions(
        Revision::new(1),
        Revision::new(2),
        vec![Operation::SetProperty {
            id: node_id,
            property: PropertyRef::VALUE,
            value: Value::from(42.0),
        }],
        0,
    );
    assert!(value_change.is_coalesceable());
}

#[test]
fn test_wire_transaction_conversion_and_application() {
    let mut store = SemanticStore::new();

    let root_id = NodeId::new(10);
    let child_id = NodeId::new(20);

    let wire_txn = srui_protocol::Transaction {
        base_revision: 0,
        new_revision: 1,
        priority: 1,
        operations: vec![
            srui_protocol::Operation {
                op: Some(srui_protocol::operation::Op::CreateNode(
                    srui_protocol::CreateNodeOp {
                        node: Some(srui_protocol::NodeRecord {
                            node_id: root_id.get(),
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
            },
            srui_protocol::Operation {
                op: Some(srui_protocol::operation::Op::CreateNode(
                    srui_protocol::CreateNodeOp {
                        node: Some(srui_protocol::NodeRecord {
                            node_id: child_id.get(),
                            r#type: Some(TypeRef::BUTTON.into()),
                            parent_id: root_id.get(),
                            child_index: u32::MAX,
                            properties: vec![srui_protocol::Property {
                                property: Some(PropertyRef::LABEL.into()),
                                value: Some(Value::from("Wire Button").into()),
                            }],
                        }),
                    },
                )),
            },
        ],
    };

    let committed_rev = store
        .apply_wire_transaction(wire_txn)
        .expect("apply wire transaction");

    assert_eq!(committed_rev, Revision::new(1));
    assert_eq!(store.revision(), Revision::new(1));
    assert_eq!(store.node_count(), 2);
    assert_eq!(store.children_of(root_id), Some(&[child_id][..]));
}

#[test]
fn test_intermediate_failure_rolls_back_entire_transaction() {
    let mut store = SemanticStore::new();

    // Create root (#1) and parent (#2)
    store
        .apply_transaction(
            Revision::INITIAL,
            vec![
                Operation::create_node(NodeId::new(1), TypeRef::SURFACE, None, None, []),
                Operation::create_node(
                    NodeId::new(2),
                    TypeRef::COLUMN,
                    Some(NodeId::new(1)),
                    None,
                    [],
                ),
                Operation::create_node(
                    NodeId::new(3),
                    TypeRef::ROW,
                    Some(NodeId::new(2)),
                    None,
                    [],
                ),
            ],
        )
        .expect("setup tree");

    assert_eq!(store.revision(), Revision::new(1));
    assert_eq!(store.node_count(), 3);

    // Try a transaction with 4 ops:
    // Op 0: create #4 under #3 (valid)
    // Op 1: set_property on #1 (valid)
    // Op 2: move #2 under #3 -> CycleDetected error!
    // Op 3: create #5 (valid)
    let cycle_ops = vec![
        Operation::create_node(
            NodeId::new(4),
            TypeRef::TEXT,
            Some(NodeId::new(3)),
            None,
            [],
        ),
        Operation::set_property(NodeId::new(1), PropertyRef::LABEL, "Will Rollback"),
        Operation::move_node(NodeId::new(2), Some(NodeId::new(3)), None),
        Operation::create_node(
            NodeId::new(5),
            TypeRef::BUTTON,
            Some(NodeId::new(3)),
            None,
            [],
        ),
    ];

    let err = store
        .apply_transaction(Revision::new(1), cycle_ops)
        .expect_err("cycle detection in transaction must abort");

    assert_eq!(
        err,
        TxnError::OpFailed {
            op_index: 2,
            source: StoreError::CycleDetected {
                node_id: NodeId::new(2),
                target_parent: NodeId::new(3),
            },
        }
    );

    // Assert complete rollback
    assert_eq!(store.revision(), Revision::new(1));
    assert_eq!(store.node_count(), 3);
    assert!(!store.contains_node(NodeId::new(4)));
    assert!(!store.contains_node(NodeId::new(5)));
    assert!(!store.is_id_used(NodeId::new(4)));
    assert!(!store.is_id_used(NodeId::new(5)));
    assert_eq!(store.parent_of(NodeId::new(2)), Some(Some(NodeId::new(1))));
    assert_eq!(
        store
            .get_node(NodeId::new(1))
            .unwrap()
            .get_property(PropertyRef::LABEL),
        None
    );
}

#[test]
fn test_empty_transaction_advances_revision_by_one() {
    let mut store = SemanticStore::new();
    assert_eq!(store.revision(), Revision::INITIAL);

    let rev1 = store
        .apply_transaction(Revision::INITIAL, vec![])
        .expect("empty transaction should commit");
    assert_eq!(rev1, Revision::new(1));
    assert_eq!(store.revision(), Revision::new(1));

    let rev2 = store
        .apply_transaction(Revision::new(1), vec![])
        .expect("second empty transaction should commit");
    assert_eq!(rev2, Revision::new(2));
    assert_eq!(store.revision(), Revision::new(2));
}

#[test]
fn test_monotonic_sequential_revisions() {
    let mut store = SemanticStore::new();

    // Apply 10 sequential transactions
    for i in 0..10 {
        let base = Revision::new(i);
        let node_id = NodeId::new(i + 1);
        let op = if i == 0 {
            Operation::create_node(node_id, TypeRef::SURFACE, None, None, [])
        } else {
            Operation::create_node(node_id, TypeRef::TEXT, Some(NodeId::new(1)), None, [])
        };

        let new_rev = store
            .apply_transaction(base, vec![op])
            .expect("sequential commit");
        assert_eq!(new_rev, Revision::new(i + 1));
        assert_eq!(store.revision(), Revision::new(i + 1));
    }

    assert_eq!(store.revision(), Revision::new(10));
    assert_eq!(store.node_count(), 10);
}
