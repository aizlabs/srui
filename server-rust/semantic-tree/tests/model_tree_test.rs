//! Hierarchical collection models: preorder shape and the `max_model_tree_depth` limit
//! (PX-018-G01; §4 inv. 13, §8, §12.1, §26, §32 items 1 and 9).
//!
//! A `Tree` model's index space is the depth-first preorder of the forest, described by the
//! standard item properties `tree_depth` and `descendant_count`. These tests pin:
//!
//! - the shared case table `protocol/conformance-vectors/model_tree_shape_cases.json`, which the
//!   Swift replica and a Python reference assert row for row (differential parity);
//! - that the check runs after the last operation of a transaction touching the model, so a
//!   subtree edit (insert plus ancestor update) is judged as a whole, on every apply path;
//! - the limit at and past its bound, with the default and a custom value;
//! - all-or-nothing rollback.

mod common;

use std::collections::BTreeMap;

use serde::Deserialize;
use srui_semantic_tree::{
    AuthoritativeCommit, ItemId, Model, ModelId, ModelItem, ModelTreeViolation, Operation,
    PropertyRef, Revision, SemanticStore, StoreError, StoreLimits, Transaction, TxnError, TypeRef,
    Value, DEFAULT_MAX_MODEL_TREE_DEPTH,
};

const MODEL: ModelId = ModelId::new(10);

fn tree_item(id: u64, depth: u64, descendants: u64) -> ModelItem {
    ModelItem::new(
        ItemId::new(id),
        Value::String(format!("item {id}")),
        [
            (PropertyRef::TREE_DEPTH, Value::UnsignedInt(depth)),
            (
                PropertyRef::DESCENDANT_COUNT,
                Value::UnsignedInt(descendants),
            ),
        ],
    )
}

/// A, B, C, D, E, F in preorder: A{B{C}, D}, E{F}.
fn nested() -> Vec<ModelItem> {
    vec![
        tree_item(1, 0, 3),
        tree_item(2, 1, 1),
        tree_item(3, 2, 0),
        tree_item(4, 1, 0),
        tree_item(5, 0, 1),
        tree_item(6, 1, 0),
    ]
}

fn store_with_nested(limits: StoreLimits) -> SemanticStore {
    let mut store = SemanticStore::with_limits_and_revision(limits, Revision::INITIAL);
    store
        .apply_transaction(
            Revision::INITIAL,
            vec![
                Operation::create_model(MODEL, TypeRef::TREE, 0),
                Operation::model_insert(MODEL, 0, nested()),
            ],
        )
        .expect("nested tree is well-formed");
    store
}

fn code(err: &TxnError) -> (&'static str, Option<usize>) {
    match err {
        TxnError::OpFailed { op_index, source } => (source.conformance_code(), Some(*op_index)),
        other => (other.conformance_code().unwrap_or("unknown"), None),
    }
}

// -----------------------------------------------------------------------------
// Shared case table (Rust / Swift / Python parity)
// -----------------------------------------------------------------------------

#[derive(Deserialize)]
struct CaseTable {
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    name: String,
    item_count: u64,
    max_model_tree_depth: usize,
    items: Vec<(u64, serde_json::Value, serde_json::Value)>,
    expected: Option<Expected>,
}

#[derive(Deserialize, PartialEq, Debug)]
struct Expected {
    code: String,
    index: u64,
}

fn json_shape(raw: &serde_json::Value) -> Option<Value> {
    match raw {
        serde_json::Value::Null => None,
        serde_json::Value::Number(n) => Some(match n.as_u64() {
            Some(u) => Value::UnsignedInt(u),
            None => Value::SignedInt(n.as_i64().expect("integer case value")),
        }),
        serde_json::Value::String(s) => Some(Value::String(s.clone())),
        other => panic!("unsupported case value {other:?}"),
    }
}

fn model_from_case(case: &Case) -> Model {
    let mut model = Model::new(MODEL, TypeRef::TREE, case.item_count);
    // Contiguous runs go in through the ordinary reset path, so the shape flag is set exactly as
    // a replica sets it.
    let mut runs: BTreeMap<u64, Vec<ModelItem>> = BTreeMap::new();
    let mut run_start = None;
    let mut last = None;
    for (index, depth, count) in &case.items {
        let mut props = Vec::new();
        if let Some(v) = json_shape(depth) {
            props.push((PropertyRef::TREE_DEPTH, v));
        }
        if let Some(v) = json_shape(count) {
            props.push((PropertyRef::DESCENDANT_COUNT, v));
        }
        let item = ModelItem::new(ItemId::new(1000 + index), Value::Null, props);
        let start = match (run_start, last) {
            (Some(start), Some(prev)) if prev + 1 == *index => start,
            _ => *index,
        };
        run_start = Some(start);
        last = Some(*index);
        runs.entry(start).or_default().push(item);
    }
    for (start, items) in runs {
        model
            .reset_range(start, items, None)
            .unwrap_or_else(|e| panic!("{}: case rows must fit item_count: {e}", case.name));
    }
    model
}

fn load_cases(path: &std::path::Path) -> CaseTable {
    serde_json::from_str(&std::fs::read_to_string(path).expect("case table")).expect("json")
}

#[test]
fn shared_case_table_matches_row_for_row() {
    let table = load_cases(&common::vectors_root().join("model_tree_shape_cases.json"));
    assert_eq!(
        table.cases.len(),
        323,
        "pinned with protocol/tests and the Swift loader"
    );
    assert_cases(&table);
}

/// Differential fuzz hook: `SRUI_MODEL_TREE_FUZZ_CASES=<file>` replays a larger generated table
/// in the same format (expected verdicts from the Python reference) against this store; the Swift
/// `ModelTreeStoreTests` honours the same variable. Unset, it does nothing.
#[test]
fn optional_fuzz_table_matches_row_for_row() {
    if let Some(path) = std::env::var_os("SRUI_MODEL_TREE_FUZZ_CASES") {
        let table = load_cases(std::path::Path::new(&path));
        assert_cases(&table);
        println!("fuzz table: {} cases agreed", table.cases.len());
    }
}

/// Exhaustive small-forest sweep hook: `SRUI_MODEL_TREE_SWEEP_OUT=<file>` enumerates every cache
/// of at most five items with depths 0..=3 and every descendant count, full and sparse, under
/// limits 3 and 4, and writes one verdict letter per cache (`.` accepted, `L`, `C`, `J`, `S`) in
/// the order `protocol/tests/test_model_tree_shape_cases.py::enumerate_sweep` defines. The
/// Swift `ModelTreeStoreTests` writes the same file format; the Python true-forest oracle
/// classifies every verdict. Unset, it does nothing.
#[test]
fn optional_exhaustive_sweep_writes_verdicts() {
    let Some(path) = std::env::var_os("SRUI_MODEL_TREE_SWEEP_OUT") else {
        return;
    };
    const DEPTHS: [u64; 4] = [0, 1, 2, 3];
    let mut out = Vec::new();
    for max_depth in [3usize, 4] {
        for n in 1u64..=5 {
            let base = DEPTHS.len() as u64 * n;
            for mask in 1u32..(1 << n) {
                let cached: Vec<u64> = (0..n).filter(|i| mask >> i & 1 == 1).collect();
                for t in 0..base.pow(cached.len() as u32) {
                    let mut model = Model::new(MODEL, TypeRef::TREE, n);
                    let mut x = t;
                    for &index in &cached {
                        let digit = x % base;
                        x /= base;
                        let item = tree_item(1000 + index, DEPTHS[(digit / n) as usize], digit % n);
                        model.id_to_index.insert(item.item_id, index);
                        model.items.insert(index, item);
                    }
                    model.tree_shaped = true;
                    out.push(match model.validate_tree_shape(max_depth) {
                        Ok(()) => b'.',
                        Err(StoreError::MaxModelTreeDepthExceeded { .. }) => b'L',
                        Err(StoreError::InvalidModelTree { violation, .. }) => match violation {
                            ModelTreeViolation::DescendantCountOutOfRange { .. } => b'C',
                            ModelTreeViolation::DepthJump { .. } => b'J',
                            ModelTreeViolation::OutsideParentSpan { .. } => b'S',
                            ModelTreeViolation::InvalidProperty { .. } => b'P',
                        },
                        Err(other) => panic!("unexpected {other}"),
                    });
                }
            }
        }
    }
    std::fs::write(&path, &out).expect("write sweep verdicts");
    println!("sweep: {} caches written", out.len());
}

fn reset_code(items: Vec<ModelItem>, count: u64) -> Option<(&'static str, Option<usize>)> {
    let mut store = SemanticStore::new();
    store
        .apply_transaction(
            Revision::INITIAL,
            vec![
                Operation::create_model(MODEL, TypeRef::TREE, count),
                Operation::model_reset_range(MODEL, 0, items, None),
            ],
        )
        .err()
        .map(|e| code(&e))
}

/// A deeper item directly after a cached leaf would be that leaf's child outside its empty span:
/// refused on the SDK and store paths (round 1, F1).
#[test]
fn a_deeper_item_after_a_cached_leaf_is_refused() {
    assert_eq!(
        reset_code(vec![tree_item(1, 0, 0), tree_item(2, 1, 0)], 2),
        Some(("model_tree_outside_parent_span", Some(1)))
    );
    assert_eq!(
        reset_code(
            vec![tree_item(1, 0, 1), tree_item(2, 1, 0), tree_item(3, 2, 0)],
            3
        ),
        Some(("model_tree_outside_parent_span", Some(1)))
    );
    assert_eq!(
        reset_code(
            vec![tree_item(1, 0, 1), tree_item(2, 1, 0), tree_item(3, 0, 0)],
            3
        ),
        None
    );
    // `validate_model_tree`, which the SDK settles every commit through, refuses it too.
    let mut store = SemanticStore::new();
    store.create_model(MODEL, TypeRef::TREE, 2).unwrap();
    store
        .model_reset_range(MODEL, 0, vec![tree_item(1, 0, 0), tree_item(2, 1, 0)], None)
        .unwrap();
    assert_eq!(
        store
            .validate_model_tree(MODEL)
            .unwrap_err()
            .conformance_code(),
        "model_tree_outside_parent_span"
    );
}

/// Sparse caches no forest can produce are refused even across uncached gaps (round 1, F1).
#[test]
fn sparse_caches_are_judged_against_every_possible_forest() {
    // Depth 3 two indices after a root: the uncached index between can be at most depth 1.
    let mut model = Model::new(MODEL, TypeRef::TREE, 4);
    model
        .reset_range(0, vec![tree_item(1, 0, 3)], None)
        .unwrap();
    model
        .reset_range(2, vec![tree_item(3, 3, 0)], None)
        .unwrap();
    assert_eq!(
        model
            .validate_tree_shape(256)
            .unwrap_err()
            .conformance_code(),
        "model_tree_depth_jump"
    );
    // An item at the deepest level cannot have descendants.
    let mut model = Model::new(MODEL, TypeRef::TREE, 4);
    model
        .reset_range(0, vec![tree_item(1, 0, 3)], None)
        .unwrap();
    model
        .reset_range(2, vec![tree_item(3, 2, 1)], None)
        .unwrap();
    assert_eq!(
        model.validate_tree_shape(3).unwrap_err().conformance_code(),
        "max_model_tree_depth_exceeded"
    );
}

/// A flat model that gains the shape properties only through MODEL_UPDATE is checked from that
/// transaction on (round 1, F3): `update_items` marks the model shaped.
#[test]
fn a_flat_model_gaining_shape_through_model_update_is_checked() {
    let mut store = SemanticStore::new();
    let flat: Vec<ModelItem> = (0..3)
        .map(|i| ModelItem::with_value(ItemId::new(i), Value::Null))
        .collect();
    store
        .apply_transaction(
            Revision::INITIAL,
            vec![
                Operation::create_model(MODEL, TypeRef::TREE, 0),
                Operation::model_insert(MODEL, 0, flat),
            ],
        )
        .unwrap();
    assert!(!store.get_model(MODEL).unwrap().tree_shaped);
    let bad = ModelItem::new(
        ItemId::new(1),
        Value::Null,
        [(PropertyRef::TREE_DEPTH, Value::UnsignedInt(2))],
    );
    let err = store
        .apply_transaction(
            store.revision(),
            vec![Operation::model_update(MODEL, None, [bad])],
        )
        .unwrap_err();
    assert_eq!(code(&err), ("model_tree_depth_jump", Some(0)));
    assert!(!store.get_model(MODEL).unwrap().tree_shaped, "rolled back");
    let shaped = [
        ModelItem::new(
            ItemId::new(0),
            Value::Null,
            [(PropertyRef::DESCENDANT_COUNT, Value::UnsignedInt(2))],
        ),
        ModelItem::new(
            ItemId::new(1),
            Value::Null,
            [(PropertyRef::TREE_DEPTH, Value::UnsignedInt(1))],
        ),
        ModelItem::new(
            ItemId::new(2),
            Value::Null,
            [(PropertyRef::TREE_DEPTH, Value::UnsignedInt(1))],
        ),
    ];
    store
        .apply_transaction(
            store.revision(),
            vec![Operation::model_update(MODEL, None, shaped)],
        )
        .expect("a valid forest through MODEL_UPDATE");
    assert!(store.get_model(MODEL).unwrap().tree_shaped);
}

fn assert_cases(table: &CaseTable) {
    for case in &table.cases {
        let model = model_from_case(case);
        let actual = match model.validate_tree_shape(case.max_model_tree_depth) {
            Ok(()) => None,
            Err(StoreError::MaxModelTreeDepthExceeded { index, .. }) => Some(Expected {
                code: "max_model_tree_depth_exceeded".into(),
                index,
            }),
            Err(StoreError::InvalidModelTree {
                index, violation, ..
            }) => Some(Expected {
                code: violation.conformance_code().into(),
                index,
            }),
            Err(other) => panic!("{}: unexpected error {other}", case.name),
        };
        assert_eq!(actual, case.expected, "case {}", case.name);
    }
}

// -----------------------------------------------------------------------------
// Transaction semantics
// -----------------------------------------------------------------------------

#[test]
fn a_flat_model_without_tree_properties_is_never_shape_checked() {
    let mut store = SemanticStore::new();
    let flat: Vec<ModelItem> = (0..5)
        .map(|i| ModelItem::with_value(ItemId::new(i), Value::Null))
        .collect();
    store
        .apply_transaction(
            Revision::INITIAL,
            vec![
                Operation::create_model(MODEL, TypeRef::TREE, 0),
                Operation::model_insert(MODEL, 0, flat),
            ],
        )
        .unwrap();
    assert!(!store.get_model(MODEL).unwrap().tree_shaped);
}

#[test]
fn subtree_insert_with_ancestor_update_commits_as_one_edit() {
    let mut store = store_with_nested(StoreLimits::default());
    let base = store.revision();
    store
        .apply_transaction(
            base,
            vec![
                Operation::model_insert(MODEL, 4, [tree_item(7, 1, 0)]),
                Operation::model_update(MODEL, None, [tree_item(1, 0, 4)]),
            ],
        )
        .expect("insert plus ancestor update is well-formed");
    let model = store.get_model(MODEL).unwrap();
    assert_eq!(model.item_count(), 7);
    assert_eq!(model.index_of(ItemId::new(7)), Some(4));
}

#[test]
fn the_failure_is_reported_at_the_models_last_operation_and_rolls_back() {
    let mut store = store_with_nested(StoreLimits::default());
    let before = store.get_model(MODEL).unwrap().clone();
    let base = store.revision();
    let err = store
        .apply_transaction(
            base,
            vec![
                Operation::model_insert(MODEL, 4, [tree_item(7, 1, 0)]),
                Operation::model_update(MODEL, None, [tree_item(1, 0, 3)]),
            ],
        )
        .unwrap_err();
    assert_eq!(code(&err), ("model_tree_outside_parent_span", Some(1)));
    assert_eq!(store.revision(), base);
    assert_eq!(store.get_model(MODEL).unwrap(), &before);
}

#[test]
fn an_unrelated_later_operation_does_not_move_the_checkpoint() {
    let mut store = store_with_nested(StoreLimits::default());
    let base = store.revision();
    let err = store
        .apply_transaction(
            base,
            vec![
                Operation::model_insert(MODEL, 4, [tree_item(7, 1, 0)]),
                Operation::create_model(ModelId::new(11), TypeRef::LIST, 0),
            ],
        )
        .unwrap_err();
    assert_eq!(code(&err), ("model_tree_outside_parent_span", Some(0)));
}

#[test]
fn every_apply_path_enforces_the_shape() {
    let bad = vec![
        Operation::create_model(MODEL, TypeRef::TREE, 0),
        Operation::model_insert(MODEL, 0, [tree_item(1, 0, 1), tree_item(2, 2, 0)]),
    ];
    let expect = ("model_tree_depth_jump", Some(1));

    let mut store = SemanticStore::new();
    assert_eq!(
        code(
            &store
                .apply_transaction(Revision::INITIAL, bad.clone())
                .unwrap_err()
        ),
        expect
    );

    let record = Transaction::with_revisions(Revision::INITIAL, Revision::new(1), bad.clone(), 0);
    let mut store = SemanticStore::new();
    assert_eq!(
        code(&store.apply_transaction_record(&record).unwrap_err()),
        expect
    );

    let mut store = SemanticStore::new();
    let wire: srui_protocol::Transaction = (&record).into();
    assert_eq!(
        code(&store.apply_wire_transaction(wire.clone()).unwrap_err()),
        expect
    );

    let mut store = SemanticStore::new();
    assert_eq!(
        code(&store.apply_delivered_transaction(wire).unwrap_err()),
        expect
    );

    let store = SemanticStore::new();
    let commit = AuthoritativeCommit::new(Revision::INITIAL, bad.clone());
    assert_eq!(code(&store.prepare_commit(&commit).unwrap_err()), expect);

    let snapshot_record =
        Transaction::with_revisions(Revision::INITIAL, Revision::new(5), bad.clone(), 0);
    let snapshot = srui_semantic_tree::ResyncSnapshot::try_from(snapshot_record).expect("shape");
    let mut store = SemanticStore::new();
    assert_eq!(
        code(&store.replace_from_snapshot(&snapshot).unwrap_err()),
        expect
    );

    // A lone wire operation is its own last touch.
    let mut store = SemanticStore::new();
    store
        .apply_operation(&(&bad[0]).into())
        .expect("create model");
    let err = store.apply_operation(&(&bad[1]).into()).unwrap_err();
    assert_eq!(err.conformance_code(), "model_tree_depth_jump");
}

// -----------------------------------------------------------------------------
// §26 limit at and past its bound
// -----------------------------------------------------------------------------

fn chain(levels: u64) -> Vec<ModelItem> {
    (0..levels)
        .map(|d| tree_item(100 + d, d, levels - 1 - d))
        .collect()
}

fn apply_chain(limits: StoreLimits, levels: u64) -> Result<Revision, TxnError> {
    let mut store = SemanticStore::with_limits_and_revision(limits, Revision::INITIAL);
    store.apply_transaction(
        Revision::INITIAL,
        vec![
            Operation::create_model(MODEL, TypeRef::TREE, 0),
            Operation::model_insert(MODEL, 0, chain(levels)),
        ],
    )
}

#[test]
fn default_limit_admits_256_levels_and_refuses_257() {
    assert_eq!(DEFAULT_MAX_MODEL_TREE_DEPTH, 256);
    assert_eq!(StoreLimits::default().max_model_tree_depth, 256);
    apply_chain(StoreLimits::default(), 256).expect("tree_depth 255 is the deepest level");
    let err = apply_chain(StoreLimits::default(), 257).unwrap_err();
    match err {
        // Depth 255 is the deepest level, so the item there cannot hold the 257th level.
        TxnError::OpFailed {
            op_index: 1,
            source:
                StoreError::MaxModelTreeDepthExceeded {
                    index: 255,
                    limit: 256,
                    tree_depth: 255,
                    ..
                },
        } => {}
        other => panic!("expected the depth limit at index 255, got {other:?}"),
    }
}

#[test]
fn custom_limit_is_enforced_at_and_past_its_bound() {
    let limits = StoreLimits::default().with_max_model_tree_depth(3);
    apply_chain(limits.clone(), 3).expect("three levels fit a limit of three");
    assert_eq!(
        code(&apply_chain(limits, 4).unwrap_err()),
        ("max_model_tree_depth_exceeded", Some(1))
    );
}

#[test]
fn the_depth_check_precedes_structural_checks_for_the_same_item() {
    // Index 0 at depth 300 is both too deep and not a root; the limit is reported first.
    let mut store = SemanticStore::new();
    let err = store
        .apply_transaction(
            Revision::INITIAL,
            vec![
                Operation::create_model(MODEL, TypeRef::TREE, 0),
                Operation::model_insert(MODEL, 0, [tree_item(1, 300, 0)]),
            ],
        )
        .unwrap_err();
    assert_eq!(code(&err), ("max_model_tree_depth_exceeded", Some(1)));
}

#[test]
fn violations_name_the_offending_item_and_rule() {
    let mut store = SemanticStore::new();
    let err = store
        .apply_transaction(
            Revision::INITIAL,
            vec![
                Operation::create_model(MODEL, TypeRef::TREE, 0),
                Operation::model_insert(
                    MODEL,
                    0,
                    [ModelItem::new(
                        ItemId::new(1),
                        Value::Null,
                        [(PropertyRef::TREE_DEPTH, Value::String("0".into()))],
                    )],
                ),
            ],
        )
        .unwrap_err();
    match err {
        TxnError::OpFailed {
            source:
                StoreError::InvalidModelTree {
                    index: 0,
                    violation: ModelTreeViolation::InvalidProperty { property },
                    ..
                },
            ..
        } => assert_eq!(property, "tree_depth"),
        other => panic!("{other:?}"),
    }
}
