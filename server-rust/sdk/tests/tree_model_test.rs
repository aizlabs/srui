//! SDK support for hierarchical `Tree` models (PX-018-G01; §7.6, §8, §22.5, §26, §32 items 2 and 9).
//!
//! - [`TreeModel`] flattens a forest into preorder items with `tree_depth` and `descendant_count`
//!   and refuses a forest deeper than `max_model_tree_depth`, at and past the bound;
//! - the items it produces are exactly what the store accepts;
//! - a `UiTransaction` that leaves a touched model malformed is refused at commit;
//! - `Tree::on_expansion_changed` delivers `(item, expanded)` and never a malformed report;
//! - `Tree` reads `columns` like `Table`.

use std::sync::{Arc, Mutex};

use srui_sdk::*;

const MODEL: ModelId = ModelId::new(10);

fn depth_and_count(item: &ModelItem) -> (u64, u64) {
    let read = |p| match item.get_property(p) {
        Some(Value::UnsignedInt(v)) => *v,
        other => panic!("{p:?} must be stamped as an unsigned integer, got {other:?}"),
    };
    (read(TREE_DEPTH), read(DESCENDANT_COUNT))
}

fn sample_forest() -> Vec<TreeNode> {
    vec![
        TreeNode::new(1u64, "src")
            .with_child(TreeNode::new(2u64, "lib").with_child(TreeNode::new(3u64, "mod.rs")))
            .with_child(TreeNode::new(4u64, "main.rs").with_property(LABEL, "entry")),
        TreeNode::new(5u64, "docs").with_children([TreeNode::new(6u64, "guide.md")]),
        TreeNode::new(7u64, "README.md"),
    ]
}

#[test]
fn flattens_a_forest_into_preorder_with_depth_and_descendant_counts() {
    let items = TreeModel::new(sample_forest())
        .into_items()
        .expect("valid forest");
    let ids: Vec<u64> = items.iter().map(|i| i.item_id.get()).collect();
    assert_eq!(ids, vec![1, 2, 3, 4, 5, 6, 7]);
    let shape: Vec<(u64, u64)> = items.iter().map(depth_and_count).collect();
    assert_eq!(
        shape,
        vec![(0, 3), (1, 1), (2, 0), (1, 0), (0, 1), (1, 0), (0, 0)]
    );
    // Other properties survive.
    assert_eq!(
        items[3].get_property(LABEL),
        Some(&Value::String("entry".into()))
    );
}

#[test]
fn an_empty_forest_is_empty() {
    assert!(TreeModel::new(Vec::new()).into_items().unwrap().is_empty());
}

#[test]
fn stale_shape_properties_on_input_items_are_overwritten() {
    let item = ModelItem::new(
        ItemId::new(1),
        Value::Null,
        [
            (TREE_DEPTH, Value::UnsignedInt(9)),
            (DESCENDANT_COUNT, Value::UnsignedInt(9)),
        ],
    );
    let items = TreeModel::new([TreeNode::from_item(item)])
        .into_items()
        .unwrap();
    assert_eq!(depth_and_count(&items[0]), (0, 0));
}

fn chain(levels: u64) -> TreeNode {
    let mut node = TreeNode::new(levels - 1, "leaf");
    for id in (0..levels - 1).rev() {
        node = TreeNode::new(id, "level").with_child(node);
    }
    node
}

#[test]
fn the_default_depth_limit_admits_256_levels_and_refuses_257() {
    assert_eq!(DEFAULT_MAX_MODEL_TREE_DEPTH, 256);
    let items = TreeModel::new([chain(256)])
        .into_items()
        .expect("256 levels");
    assert_eq!(depth_and_count(&items[255]), (255, 0));
    assert_eq!(depth_and_count(&items[0]), (0, 255));
    assert_eq!(
        TreeModel::new([chain(257)]).into_items(),
        Err(TreeModelError::DepthLimitExceeded {
            item_id: ItemId::new(256),
            tree_depth: 256,
            limit: 256,
        })
    );
}

#[test]
fn a_custom_limit_follows_the_store_limits() {
    let limits = StoreLimits::default().with_max_model_tree_depth(2);
    assert!(TreeModel::new([chain(2)])
        .with_limits(&limits)
        .into_items()
        .is_ok());
    assert!(matches!(
        TreeModel::new([chain(3)]).with_limits(&limits).into_items(),
        Err(TreeModelError::DepthLimitExceeded {
            tree_depth: 2,
            limit: 2,
            ..
        })
    ));
}

#[test]
fn an_adversarially_deep_forest_is_refused_without_recursion() {
    // 100k levels would overflow a recursive flattener; the iterative one stops at the limit.
    let deep = chain(100_000);
    assert!(matches!(
        TreeModel::new([deep]).into_items(),
        Err(TreeModelError::DepthLimitExceeded {
            tree_depth: 256,
            ..
        })
    ));
}

#[test]
fn duplicate_item_ids_are_refused() {
    let forest = vec![
        TreeNode::new(1u64, "a").with_child(TreeNode::new(2u64, "b")),
        TreeNode::new(2u64, "again"),
    ];
    assert_eq!(
        TreeModel::new(forest).into_items(),
        Err(TreeModelError::DuplicateItemId(ItemId::new(2)))
    );
}

#[test]
fn the_reset_operation_is_accepted_by_the_store_and_reconstructs_the_shape() {
    let session = Session::new("tree-model");
    session
        .transaction(|ui| {
            ui.apply_op(&Operation::create_model(MODEL, TypeRef::TREE, 0))?;
            let reset = TreeModel::new(sample_forest())
                .reset_op(MODEL)
                .expect("forest");
            ui.apply_op(&reset)?;
            Tree::builder(1u64).model_ref(MODEL).create(ui)?;
            Ok(())
        })
        .expect("TreeModel output is a well-formed hierarchical model");
    session.with_store(|store| {
        let model = store.get_model(MODEL).unwrap();
        assert_eq!(model.item_count(), 7);
        assert!(model.tree_shaped);
        assert_eq!(depth_and_count(model.get_item_by_index(0).unwrap()), (0, 3));
    });
}

#[test]
fn a_transaction_leaving_a_touched_model_malformed_is_refused_at_commit() {
    let session = Session::new("tree-commit");
    session
        .transaction(|ui| {
            ui.apply_op(&Operation::create_model(MODEL, TypeRef::TREE, 0))?;
            ui.apply_op(&TreeModel::new(sample_forest()).reset_op(MODEL).unwrap())?;
            Ok(())
        })
        .unwrap();
    let revision = session.current_revision();
    // Insert a child at the end of `src`'s span without growing `src`.
    let child = TreeModel::new([TreeNode::new(8u64, "new.rs")])
        .into_items()
        .unwrap()
        .into_iter()
        .map(|mut item| {
            item.set_property(TREE_DEPTH, Value::UnsignedInt(1));
            item
        });
    let err = session
        .transaction(|ui| {
            ui.apply_op(&Operation::model_insert(MODEL, 4, child))?;
            Ok(())
        })
        .unwrap_err();
    assert!(
        matches!(
            err,
            SdkError::Store(StoreError::InvalidModelTree {
                violation: ModelTreeViolation::OutsideParentSpan { parent_index: 0 },
                ..
            })
        ),
        "{err:?}"
    );
    assert_eq!(session.current_revision(), revision);
}

#[test]
fn the_sdk_commit_refuses_a_deeper_item_after_a_cached_leaf() {
    let leaf_then_deeper = [
        ModelItem::new(
            ItemId::new(1),
            Value::Null,
            [
                (TREE_DEPTH, Value::UnsignedInt(0)),
                (DESCENDANT_COUNT, Value::UnsignedInt(0)),
            ],
        ),
        ModelItem::new(
            ItemId::new(2),
            Value::Null,
            [
                (TREE_DEPTH, Value::UnsignedInt(1)),
                (DESCENDANT_COUNT, Value::UnsignedInt(0)),
            ],
        ),
    ];
    let session = Session::new("leaf-then-deeper");
    let err = session
        .transaction(|ui| {
            ui.apply_op(&Operation::create_model(MODEL, TypeRef::TREE, 0))?;
            ui.apply_op(&Operation::model_insert(MODEL, 0, leaf_then_deeper))?;
            Ok(())
        })
        .unwrap_err();
    assert!(
        matches!(
            err,
            SdkError::Store(StoreError::InvalidModelTree {
                index: 1,
                violation: ModelTreeViolation::OutsideParentSpan { parent_index: 0 },
                ..
            })
        ),
        "{err:?}"
    );
    assert_eq!(session.current_revision().get(), 0);
}

#[test]
fn on_expansion_changed_reports_item_and_state_and_ignores_malformed_reports() {
    let session = Session::new("tree-expansion");
    let tree = session
        .transaction(|ui| {
            ui.apply_op(&Operation::create_model(MODEL, TypeRef::TREE, 0))?;
            ui.apply_op(&TreeModel::new(sample_forest()).reset_op(MODEL).unwrap())?;
            Tree::builder(1u64).model_ref(MODEL).create(ui)
        })
        .unwrap();
    let seen: Arc<Mutex<Vec<ExpansionChange>>> = Arc::default();
    let sink = Arc::clone(&seen);
    tree.on_expansion_changed(&session, move |_, change| sink.lock().unwrap().push(change));

    let revision = session.current_revision();
    session
        .dispatch(Event::expansion_changed(
            1,
            "e1",
            revision,
            tree.id(),
            1u64,
            false,
        ))
        .unwrap();
    session
        .dispatch(Event::expansion_changed(
            2,
            "e2",
            revision,
            tree.id(),
            1u64,
            true,
        ))
        .unwrap();
    // Malformed: a bare boolean with no item, and an item list of two.
    session
        .dispatch(Event::new(
            None,
            3,
            "e3",
            revision,
            tree.id(),
            EXPANSION_CHANGED,
            [(VALUE, Value::Bool(true))],
        ))
        .unwrap();
    session
        .dispatch(Event::new(
            None,
            4,
            "e4",
            revision,
            tree.id(),
            EXPANSION_CHANGED,
            [
                (VALUE, Value::Bool(true)),
                (
                    ITEMS,
                    Value::List(vec![
                        Value::ItemId(ItemId::new(1)),
                        Value::ItemId(ItemId::new(5)),
                    ]),
                ),
            ],
        ))
        .unwrap();

    let seen = seen.lock().unwrap();
    assert_eq!(
        *seen,
        vec![
            ExpansionChange {
                node: tree.id(),
                item: ItemId::new(1),
                expanded: false,
                observed_revision: revision,
            },
            ExpansionChange {
                node: tree.id(),
                item: ItemId::new(1),
                expanded: true,
                observed_revision: revision,
            },
        ]
    );
}

#[test]
fn tree_reads_columns_like_table() {
    let mut store = SemanticStore::new();
    store.create_model(MODEL, TypeRef::TREE, 0).unwrap();
    let tree = Tree::builder(1u64)
        .model_ref(MODEL)
        .columns(vec![
            Value::String("Name".into()),
            Value::String("Size".into()),
        ])
        .create(&mut store)
        .unwrap();
    assert_eq!(tree.columns(&store).map(|c| c.len()), Some(2));
    tree.set_columns(&mut store, [Value::String("Name".into())])
        .unwrap();
    assert_eq!(tree.columns(&store).map(|c| c.len()), Some(1));
    tree.clear_columns(&mut store).unwrap();
    assert_eq!(tree.columns(&store), None);
}

// --- Replacing a populated model (PR #101 review: a smaller forest must not leave a stale tail) ---

/// A session whose model holds `forest`, committed through `reset_op` on an empty model.
fn session_holding(name: &str, forest: Vec<TreeNode>) -> Session {
    let session = Session::new(name);
    session
        .transaction(|ui| {
            ui.apply_op(&Operation::create_model(MODEL, TypeRef::TREE, 0))?;
            ui.apply_op(&TreeModel::new(forest).reset_op(MODEL).unwrap())?;
            Ok(())
        })
        .expect("populating an empty model");
    session
}

/// Replaces the model's whole preorder in one transaction through `TreeModel::replace_ops`.
fn replace(session: &Session, forest: Vec<TreeNode>) -> Result<(), SdkError> {
    session.transaction(|ui| {
        let current = ui.get_model(MODEL).map_or(0, |m| m.item_count());
        for op in TreeModel::new(forest).replace_ops(MODEL, current).unwrap() {
            ui.apply_op(&op)?;
        }
        Ok(())
    })
}

fn cached(session: &Session) -> (u64, Vec<(u64, ModelItem)>) {
    session.with_store(|store| {
        let model = store.get_model(MODEL).unwrap();
        let items = model
            .iter_cached_items()
            .map(|(&i, item)| (i, item.clone()))
            .collect();
        (model.item_count(), items)
    })
}

fn parent_and_child() -> Vec<TreeNode> {
    vec![TreeNode::new(1u64, "parent").with_child(TreeNode::new(2u64, "child"))]
}

#[test]
fn reset_op_alone_cannot_shrink_a_populated_tree() {
    // `MODEL_RESET_RANGE` rewrites only its own range: index 1 stays cached past the new count.
    let session = session_holding("reset-shrink", parent_and_child());
    let revision = session.current_revision();
    let err = session
        .transaction(|ui| {
            let reset = TreeModel::new([TreeNode::new(1u64, "parent")])
                .reset_op(MODEL)
                .unwrap();
            ui.apply_op(&reset)?;
            Ok(())
        })
        .unwrap_err();
    assert!(
        matches!(
            err,
            SdkError::Store(StoreError::InvalidModelTree {
                index: 1,
                violation: ModelTreeViolation::DescendantCountOutOfRange { item_count: 1, .. },
                ..
            })
        ),
        "{err:?}"
    );
    assert_eq!(session.current_revision(), revision);
}

#[test]
fn replacing_a_parent_and_child_with_the_parent_as_a_leaf_commits() {
    let session = session_holding("replace-leaf", parent_and_child());
    replace(&session, vec![TreeNode::new(1u64, "parent")])
        .expect("a smaller forest replaces the whole preorder");
    let (count, items) = cached(&session);
    assert_eq!(count, 1);
    assert_eq!(items.len(), 1, "no old item stays cached: {items:?}");
    assert_eq!(items[0].0, 0);
    assert_eq!(depth_and_count(&items[0].1), (0, 0));
}

#[test]
fn replace_ops_delete_the_old_tail_before_the_reset() {
    let ops = TreeModel::new([TreeNode::new(1u64, "a")])
        .replace_ops(MODEL, 7)
        .unwrap();
    assert_eq!(ops.len(), 2, "{ops:?}");
    assert_eq!(ops[0], Operation::model_delete_range(MODEL, 1, 6));
    assert!(matches!(
        &ops[1],
        Operation::ModelResetRange { start_index: 0, total_count: Some(1), items, .. } if items.len() == 1
    ));
    // Growing or same size: the reset alone.
    let grow = TreeModel::new(sample_forest())
        .replace_ops(MODEL, 2)
        .unwrap();
    assert!(matches!(
        grow.as_slice(),
        [Operation::ModelResetRange {
            total_count: Some(7),
            ..
        }]
    ));
    let same = TreeModel::new(sample_forest())
        .replace_ops(MODEL, 7)
        .unwrap();
    assert!(matches!(
        same.as_slice(),
        [Operation::ModelResetRange {
            total_count: Some(7),
            ..
        }]
    ));
    // Empty forest: delete everything, no reset; nothing at all for an empty model.
    let empty = TreeModel::new(Vec::new()).replace_ops(MODEL, 7).unwrap();
    assert_eq!(empty, vec![Operation::model_delete_range(MODEL, 0, 7)]);
    assert!(TreeModel::new(Vec::new())
        .replace_ops(MODEL, 0)
        .unwrap()
        .is_empty());
}

/// Smaller forests of every size below the sample's 7 items, with fresh ids, ids reused from the
/// old tail at new positions, and nested shapes.
fn smaller_forests() -> Vec<Vec<TreeNode>> {
    let mut forests = Vec::new();
    for n in 1..7u64 {
        // A chain whose ids come from the end of the old preorder (7, 6, ...).
        let mut chain = TreeNode::new(7 - (n - 1), "leaf");
        for id in (7 - (n - 1) + 1)..=7 {
            chain = TreeNode::new(id, "level").with_child(chain);
        }
        forests.push(vec![chain]);
        // Flat roots with fresh ids.
        forests.push((0..n).map(|i| TreeNode::new(100 + i, "root")).collect());
        // One root over n - 1 children, ids reversed from the old preorder.
        forests.push(vec![
            TreeNode::new(n, "top").with_children((1..n).rev().map(|id| TreeNode::new(id, "kid")))
        ]);
    }
    forests
}

#[test]
fn replacing_a_populated_forest_with_any_smaller_forest_matches_building_it_fresh() {
    for (case, forest) in smaller_forests().into_iter().enumerate() {
        let expected = cached(&session_holding("fresh", forest.clone()));
        let session = session_holding("replace-smaller", sample_forest());
        replace(&session, forest).unwrap_or_else(|err| panic!("case {case}: {err:?}"));
        let got = cached(&session);
        assert_eq!(got, expected, "case {case}");
        assert!(
            got.1.iter().all(|(i, _)| *i < got.0),
            "case {case}: {got:?}"
        );
    }
}

#[test]
fn replacing_a_small_forest_with_a_larger_one_matches_building_it_fresh() {
    let session = session_holding("replace-grow", parent_and_child());
    replace(&session, sample_forest()).unwrap();
    assert_eq!(
        cached(&session),
        cached(&session_holding("fresh", sample_forest()))
    );
}

#[test]
fn replacing_a_populated_forest_with_an_empty_one_empties_the_model() {
    let session = session_holding("replace-empty", sample_forest());
    replace(&session, Vec::new()).expect("an empty forest empties the model");
    assert_eq!(cached(&session), (0, Vec::new()));
    // Replacing the now empty model with an empty forest commits nothing to the model.
    replace(&session, Vec::new()).unwrap();
    assert_eq!(cached(&session), (0, Vec::new()));
    // And it can be populated again.
    replace(&session, parent_and_child()).unwrap();
    assert_eq!(cached(&session).0, 2);
}
