//! Deterministic seeded store trace tests (§6.2, §12.1, §13, §26, §32 item 1).
//!
//! Generates a pseudo-random operation sequence from a fixed seed, applies each transaction to
//! `SemanticStore`, and compares the live store against an independent **reference model** built
//! by replaying only successfully committed transactions on a fresh store.
//!
//! On rejection, proves the live store is bit-for-bit unchanged (rollback invariant §12.1).
//! If a mismatch is detected and `SRUI_WRITE_TRACE_FIXTURE=1` is set, persists a minimal failing
//! trace as a state-machine conformance vector for cross-language replay.

use srui_semantic_tree::{
    ItemId, ModelId, ModelItem, NodeId, Operation, PropertyRef, Range, Revision, SemanticStore,
    StoreLimits, Transaction, TypeRef, Value,
};
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::PathBuf;

// ==============================================================================
// Shared trace parameters (must match StoreTraceTests.swift)
// ==============================================================================

const TRACE_SEEDS: &[u64] = &[0x5EED_0001, 0x5EED_0002, 0x5EED_CAFE];
const STEPS_PER_SEED: usize = 120;

fn trace_limits() -> StoreLimits {
    StoreLimits {
        max_tree_depth: 8,
        max_node_count: 48,
        max_string_length: 256,
        max_value_depth: 8,
        max_list_elements: 16,
        max_record_properties: 16,
        max_transaction_operations: 6,
        max_model_count: 4,
        max_cached_items_per_model: 32,
        max_items_per_model_operation: 8,
    }
}

// ==============================================================================
// Portable deterministic PRNG (LCG; identical constants in Swift)
// ==============================================================================

struct LcgRng {
    state: u64,
}

impl LcgRng {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next(&mut self) -> u64 {
        self.state = self
            .state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        self.state
    }

    fn gen_usize(&mut self, upper_exclusive: usize) -> usize {
        if upper_exclusive == 0 {
            return 0;
        }
        (self.next() as usize) % upper_exclusive
    }

    fn gen_bool(&mut self, numer: u64, denom: u64) -> bool {
        self.next() % denom < numer
    }
}

// ==============================================================================
// Store snapshot (observable committed state)
// ==============================================================================

type NodeSnapshot = (
    TypeRef,
    Option<NodeId>,
    Vec<NodeId>,
    BTreeMap<PropertyRef, Value>,
);

#[derive(Clone, Debug, PartialEq)]
struct ModelSnapshot {
    model_type: TypeRef,
    item_count: u64,
    cached_item_count: usize,
    cached_ranges: Vec<Range>,
    items_by_index: BTreeMap<u64, (ItemId, Value)>,
}

#[derive(Clone, Debug, PartialEq)]
struct StoreSnapshot {
    revision: Revision,
    node_count: usize,
    roots: Vec<NodeId>,
    nodes: BTreeMap<NodeId, NodeSnapshot>,
    model_count: usize,
    models: BTreeMap<ModelId, ModelSnapshot>,
}

fn take_snapshot(store: &SemanticStore) -> StoreSnapshot {
    let mut nodes = BTreeMap::new();
    for &root in store.root_ids() {
        collect_nodes_snapshot(store, root, &mut nodes);
    }

    let mut models = BTreeMap::new();
    for model_id in store.model_ids() {
        if let Some(model) = store.get_model(model_id) {
            let items_by_index: BTreeMap<u64, (ItemId, Value)> = model
                .items
                .iter()
                .map(|(idx, item)| (*idx, (item.item_id, item.value.clone())))
                .collect();
            models.insert(
                model_id,
                ModelSnapshot {
                    model_type: model.model_type,
                    item_count: model.item_count,
                    cached_item_count: model.cached_item_count(),
                    cached_ranges: model.cached_ranges().to_vec(),
                    items_by_index,
                },
            );
        }
    }

    StoreSnapshot {
        revision: store.revision(),
        node_count: store.node_count(),
        roots: store.root_ids().to_vec(),
        nodes,
        model_count: store.model_count(),
        models,
    }
}

fn collect_nodes_snapshot(
    store: &SemanticStore,
    id: NodeId,
    nodes: &mut BTreeMap<NodeId, NodeSnapshot>,
) {
    if let Some(n) = store.get_node(id) {
        nodes.insert(
            id,
            (
                n.node_type,
                n.parent_id,
                n.ordered_children.clone(),
                n.properties.iter().map(|(k, v)| (*k, v.clone())).collect(),
            ),
        );
        for &child in &n.ordered_children {
            collect_nodes_snapshot(store, child, nodes);
        }
    }
}

/// Reference model: replay only committed transactions on a fresh store (§12.1).
fn reference_snapshot(committed: &[Transaction], limits: &StoreLimits) -> StoreSnapshot {
    let mut store = SemanticStore::with_limits(limits.clone());
    for txn in committed {
        store
            .apply_transaction_record(txn)
            .unwrap_or_else(|e| panic!("committed transaction must replay cleanly: {e:?}"));
    }
    take_snapshot(&store)
}

// ==============================================================================
// Trace generator
// ==============================================================================

struct TraceGen {
    next_node_id: u64,
    next_model_id: u64,
    next_item_id: u64,
    node_ids: Vec<NodeId>,
    model_ids: Vec<ModelId>,
}

impl TraceGen {
    fn new() -> Self {
        Self {
            next_node_id: 1,
            next_model_id: 100,
            next_item_id: 1_000,
            node_ids: Vec::new(),
            model_ids: Vec::new(),
        }
    }

    fn alloc_node_id(&mut self) -> NodeId {
        let id = NodeId::new(self.next_node_id);
        self.next_node_id += 1;
        id
    }

    fn alloc_model_id(&mut self) -> ModelId {
        let id = ModelId::new(self.next_model_id);
        self.next_model_id += 1;
        id
    }

    fn alloc_item_id(&mut self) -> ItemId {
        let id = ItemId::new(self.next_item_id);
        self.next_item_id += 1;
        id
    }

    fn leaf_nodes(&self, store: &SemanticStore) -> Vec<NodeId> {
        self.node_ids
            .iter()
            .copied()
            .filter(|id| {
                store
                    .get_node(*id)
                    .map(|n| n.ordered_children.is_empty())
                    .unwrap_or(false)
            })
            .collect()
    }

    fn pick_node(&self, rng: &mut LcgRng) -> Option<NodeId> {
        if self.node_ids.is_empty() {
            None
        } else {
            Some(self.node_ids[rng.gen_usize(self.node_ids.len())])
        }
    }

    fn pick_model(&self, rng: &mut LcgRng) -> Option<ModelId> {
        if self.model_ids.is_empty() {
            None
        } else {
            Some(self.model_ids[rng.gen_usize(self.model_ids.len())])
        }
    }

    fn node_types() -> &'static [TypeRef] {
        &[
            TypeRef::SURFACE,
            TypeRef::COLUMN,
            TypeRef::ROW,
            TypeRef::TEXT,
            TypeRef::BUTTON,
        ]
    }

    fn property_refs() -> &'static [PropertyRef] {
        &[
            PropertyRef::LABEL,
            PropertyRef::TEXT,
            PropertyRef::ENABLED,
            PropertyRef::BUSY,
        ]
    }

    fn string_value(rng: &mut LcgRng, tag: &str) -> Value {
        let n = rng.next() % 10_000;
        Value::from(format!("{tag}-{n}"))
    }

    fn generate_transaction(
        &mut self,
        rng: &mut LcgRng,
        store: &SemanticStore,
        step: usize,
    ) -> Transaction {
        let base = store.revision();
        let stale = step > 0 && rng.gen_bool(1, 20);
        let base_revision = if stale {
            Revision::new(base.get().saturating_sub(1))
        } else {
            base
        };

        let op_count = 1 + rng.gen_usize(3);
        let mut ops = Vec::with_capacity(op_count);

        for _ in 0..op_count {
            if self.node_ids.is_empty() {
                ops.push(self.gen_create_root(rng));
                continue;
            }

            let choice = rng.gen_usize(10);
            let op = match choice {
                0 | 1 => self.gen_create_node(rng, store),
                2 | 3 => self
                    .gen_set_property(rng)
                    .unwrap_or_else(|| self.gen_create_node(rng, store)),
                4 => self
                    .gen_clear_property(rng, store)
                    .unwrap_or_else(|| self.gen_create_node(rng, store)),
                5 => self
                    .gen_delete_leaf(rng, store)
                    .unwrap_or_else(|| self.gen_create_node(rng, store)),
                6 => self
                    .gen_move_node(rng, store)
                    .unwrap_or_else(|| self.gen_create_node(rng, store)),
                7 => self
                    .gen_batch_property_set(rng)
                    .unwrap_or_else(|| self.gen_create_node(rng, store)),
                8 => self.gen_create_model(rng),
                _ => self.gen_model_insert(rng, store),
            };
            ops.push(op);
        }

        if ops.is_empty() {
            ops.push(self.gen_create_root(rng));
        }

        Transaction::with_revisions(base_revision, base_revision.next(), ops, 0)
    }

    fn gen_create_root(&mut self, rng: &mut LcgRng) -> Operation {
        let id = self.alloc_node_id();
        self.node_ids.push(id);
        Operation::create_node(
            id,
            TypeRef::SURFACE,
            None,
            None,
            [(PropertyRef::LABEL, Self::string_value(rng, "root"))],
        )
    }

    fn gen_create_node(&mut self, rng: &mut LcgRng, store: &SemanticStore) -> Operation {
        let id = self.alloc_node_id();
        let parent = self.pick_node(rng);
        let node_type = Self::node_types()[rng.gen_usize(Self::node_types().len())];
        let child_index = if rng.gen_bool(1, 3) {
            parent.and_then(|p| {
                store
                    .children_of(p)
                    .map(|kids| rng.gen_usize(kids.len() + 1))
            })
        } else {
            None
        };
        self.node_ids.push(id);
        Operation::create_node(
            id,
            node_type,
            parent,
            child_index,
            [(PropertyRef::LABEL, Self::string_value(rng, "node"))],
        )
    }

    fn gen_set_property(&mut self, rng: &mut LcgRng) -> Option<Operation> {
        let id = self.pick_node(rng)?;
        let prop = Self::property_refs()[rng.gen_usize(Self::property_refs().len())];
        let value = match prop {
            PropertyRef::ENABLED | PropertyRef::BUSY => Value::from(rng.gen_bool(1, 2)),
            _ => Self::string_value(rng, "prop"),
        };
        Some(Operation::set_property(id, prop, value))
    }

    fn gen_clear_property(&mut self, rng: &mut LcgRng, store: &SemanticStore) -> Option<Operation> {
        let id = self.pick_node(rng)?;
        let node = store.get_node(id)?;
        if node.properties.is_empty() {
            return self.gen_set_property(rng);
        }
        let props: Vec<PropertyRef> = node.properties.keys().copied().collect();
        let prop = props[rng.gen_usize(props.len())];
        Some(Operation::clear_property(id, prop))
    }

    fn gen_delete_leaf(&mut self, rng: &mut LcgRng, store: &SemanticStore) -> Option<Operation> {
        let leaves = self.leaf_nodes(store);
        if leaves.len() <= 1 {
            return self.gen_set_property(rng);
        }
        let id = leaves[rng.gen_usize(leaves.len())];
        self.node_ids.retain(|&n| n != id);
        Some(Operation::delete_node(id))
    }

    fn gen_move_node(&mut self, rng: &mut LcgRng, store: &SemanticStore) -> Option<Operation> {
        if self.node_ids.len() < 2 {
            return self.gen_set_property(rng);
        }
        let id = self.pick_node(rng)?;
        if store.root_ids().len() == 1 && store.root_ids()[0] == id {
            return self.gen_set_property(rng);
        }
        let new_parent = self.pick_node(rng)?;
        if new_parent == id {
            return self.gen_set_property(rng);
        }
        let child_index = if rng.gen_bool(1, 2) {
            store
                .children_of(new_parent)
                .map(|kids| rng.gen_usize(kids.len() + 1))
        } else {
            None
        };
        Some(Operation::move_node(id, Some(new_parent), child_index))
    }

    fn gen_batch_property_set(&mut self, rng: &mut LcgRng) -> Option<Operation> {
        let id = self.pick_node(rng)?;
        let props = vec![
            (PropertyRef::LABEL, Self::string_value(rng, "batch")),
            (PropertyRef::ENABLED, Value::from(rng.gen_bool(1, 2))),
        ];
        Some(Operation::batch_property_set(id, props))
    }

    fn gen_create_model(&mut self, rng: &mut LcgRng) -> Operation {
        let id = self.alloc_model_id();
        self.model_ids.push(id);
        let item_count = 50 + (rng.next() % 200);
        Operation::create_model(id, TypeRef::LIST, item_count)
    }

    fn gen_model_insert(&mut self, rng: &mut LcgRng, store: &SemanticStore) -> Operation {
        if let Some(model_id) = self.pick_model(rng) {
            if let Some(model) = store.get_model(model_id) {
                let index = rng.next() % (model.item_count + 1);
                let item =
                    ModelItem::new(self.alloc_item_id(), Self::string_value(rng, "item"), []);
                return Operation::model_insert(model_id, index, vec![item]);
            }
        }
        self.gen_create_model(rng)
    }
}

// ==============================================================================
// Trace runner
// ==============================================================================

struct TraceMismatch {
    seed: u64,
    step: usize,
    committed: Vec<Transaction>,
    failing_txn: Transaction,
    live_snapshot: StoreSnapshot,
    reference_snapshot: StoreSnapshot,
    pre_snapshot: StoreSnapshot,
    rejected: bool,
}

fn run_seeded_trace(seed: u64) -> Result<(), Box<TraceMismatch>> {
    let limits = trace_limits();
    let mut store = SemanticStore::with_limits(limits.clone());
    let mut gen = TraceGen::new();
    let mut rng = LcgRng::new(seed);
    let mut committed: Vec<Transaction> = Vec::new();

    for step in 0..STEPS_PER_SEED {
        let txn = gen.generate_transaction(&mut rng, &store, step);
        let pre_snapshot = take_snapshot(&store);
        let reference_before = reference_snapshot(&committed, &limits);

        assert_eq!(
            pre_snapshot, reference_before,
            "seed {seed} step {step}: live store diverged from reference before applying txn"
        );

        let result = store.apply_transaction_record(&txn);

        match result {
            Ok(new_rev) => {
                assert_eq!(new_rev, txn.new_revision);
                committed.push(txn.clone());
                let live = take_snapshot(&store);
                let reference = reference_snapshot(&committed, &limits);
                if live != reference {
                    return Err(Box::new(TraceMismatch {
                        seed,
                        step,
                        committed: committed[..committed.len() - 1].to_vec(),
                        failing_txn: txn,
                        live_snapshot: live,
                        reference_snapshot: reference,
                        pre_snapshot,
                        rejected: false,
                    }));
                }
            }
            Err(_) => {
                let post_snapshot = take_snapshot(&store);
                if post_snapshot != pre_snapshot {
                    return Err(Box::new(TraceMismatch {
                        seed,
                        step,
                        committed: committed.clone(),
                        failing_txn: txn,
                        live_snapshot: post_snapshot,
                        reference_snapshot: reference_before,
                        pre_snapshot,
                        rejected: true,
                    }));
                }
                let reference = reference_snapshot(&committed, &limits);
                if post_snapshot != reference {
                    return Err(Box::new(TraceMismatch {
                        seed,
                        step,
                        committed: committed.clone(),
                        failing_txn: txn,
                        live_snapshot: post_snapshot,
                        reference_snapshot: reference,
                        pre_snapshot,
                        rejected: true,
                    }));
                }
            }
        }
    }

    Ok(())
}

fn persist_failing_trace(mismatch: &TraceMismatch) {
    if env::var("SRUI_WRITE_TRACE_FIXTURE").ok().as_deref() != Some("1") {
        return;
    }

    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let out_dir =
        PathBuf::from(manifest_dir).join("../../protocol/conformance-vectors/state-machine");
    let file_name = format!(
        "99_trace_seed_{:x}_step_{}.json",
        mismatch.seed, mismatch.step
    );
    let path = out_dir.join(&file_name);

    let setup: Vec<serde_json::Value> =
        mismatch.committed.iter().map(transaction_to_json).collect();

    let fixture = serde_json::json!({
        "name": format!("trace_seed_{:x}_step_{}", mismatch.seed, mismatch.step),
        "description": "Auto-generated minimal failing store trace (T35)",
        "spec_sections": ["§6.2", "§12.1", "§13", "§26"],
        "initial_limits": limits_to_json(&trace_limits()),
        "setup_transactions": setup,
        "transaction": transaction_to_json(&mismatch.failing_txn),
        "expected_outcome": {
            "status": if mismatch.rejected { "rejected" } else { "success" },
            "rollback_verified": mismatch.rejected,
            "expected_store_revision": mismatch.pre_snapshot.revision.get(),
        }
    });

    fs::write(&path, serde_json::to_string_pretty(&fixture).unwrap())
        .unwrap_or_else(|e| panic!("failed to write trace fixture {:?}: {}", path, e));
    eprintln!("Wrote failing trace fixture to {:?}", path);
}

fn limits_to_json(limits: &StoreLimits) -> serde_json::Value {
    serde_json::json!({
        "max_tree_depth": limits.max_tree_depth,
        "max_node_count": limits.max_node_count,
        "max_transaction_operations": limits.max_transaction_operations,
        "max_string_length": limits.max_string_length,
        "max_value_depth": limits.max_value_depth,
        "max_list_elements": limits.max_list_elements,
        "max_record_properties": limits.max_record_properties,
        "max_model_count": limits.max_model_count,
        "max_cached_items_per_model": limits.max_cached_items_per_model,
        "max_items_per_model_operation": limits.max_items_per_model_operation,
    })
}

fn transaction_to_json(txn: &Transaction) -> serde_json::Value {
    let ops: Vec<serde_json::Value> = txn.operations.iter().map(operation_to_json).collect();
    serde_json::json!({
        "base_revision": txn.base_revision.get(),
        "new_revision": txn.new_revision.get(),
        "operations": ops,
    })
}

fn operation_to_json(op: &Operation) -> serde_json::Value {
    match op {
        Operation::CreateNode {
            id,
            node_type,
            parent_id,
            child_index,
            properties,
        } => {
            let props: serde_json::Map<String, serde_json::Value> = properties
                .iter()
                .map(|(p, v)| (property_name(*p), value_to_json(v)))
                .collect();
            serde_json::json!({
                "type": "CREATE_NODE",
                "node_id": id.get(),
                "node_type": type_name(*node_type),
                "parent_id": parent_id.map(|p| serde_json::json!(p.get())),
                "child_index": child_index,
                "properties": props,
            })
        }
        Operation::DeleteNode { id } => serde_json::json!({
            "type": "DELETE_NODE",
            "node_id": id.get(),
        }),
        Operation::SetProperty {
            id,
            property,
            value,
        } => serde_json::json!({
            "type": "SET_PROPERTY",
            "node_id": id.get(),
            "property": property_name(*property),
            "value": value_to_json(value),
        }),
        Operation::ClearProperty { id, property } => serde_json::json!({
            "type": "CLEAR_PROPERTY",
            "node_id": id.get(),
            "property": property_name(*property),
        }),
        Operation::MoveNode {
            id,
            new_parent_id,
            new_child_index,
        } => serde_json::json!({
            "type": "MOVE_NODE",
            "node_id": id.get(),
            "new_parent_id": new_parent_id.map(|p| serde_json::json!(p.get())),
            "new_child_index": new_child_index,
        }),
        Operation::ReorderChildren {
            parent_id,
            new_order,
        } => serde_json::json!({
            "type": "REORDER_CHILDREN",
            "parent_id": parent_id.get(),
            "new_order": new_order.iter().map(|id| id.get()).collect::<Vec<_>>(),
        }),
        Operation::BatchPropertySet { id, properties } => {
            let props: serde_json::Map<String, serde_json::Value> = properties
                .iter()
                .map(|(p, v)| (property_name(*p), value_to_json(v)))
                .collect();
            serde_json::json!({
                "type": "BATCH_PROPERTY_SET",
                "node_id": id.get(),
                "properties": props,
            })
        }
        Operation::CreateModel {
            id,
            model_type,
            item_count,
        } => serde_json::json!({
            "type": "CREATE_MODEL",
            "model_id": id.get(),
            "model_type": type_name(*model_type),
            "item_count": item_count,
        }),
        Operation::ModelInsert { id, index, items } => serde_json::json!({
            "type": "MODEL_INSERT",
            "model_id": id.get(),
            "index": index,
            "items": items.iter().map(model_item_to_json).collect::<Vec<_>>(),
        }),
        Operation::ModelDelete {
            id,
            index,
            count,
            item_ids,
        } => serde_json::json!({
            "type": "MODEL_DELETE",
            "model_id": id.get(),
            "index": index,
            "count": count,
            "item_ids": item_ids.iter().map(|i| i.get()).collect::<Vec<_>>(),
        }),
        Operation::ModelUpdate { id, index, items } => serde_json::json!({
            "type": "MODEL_UPDATE",
            "model_id": id.get(),
            "index": index,
            "items": items.iter().map(model_item_to_json).collect::<Vec<_>>(),
        }),
        Operation::ModelResetRange {
            id,
            start_index,
            items,
            total_count,
        } => serde_json::json!({
            "type": "MODEL_RESET_RANGE",
            "model_id": id.get(),
            "start_index": start_index,
            "items": items.iter().map(model_item_to_json).collect::<Vec<_>>(),
            "total_count": total_count,
        }),
    }
}

fn model_item_to_json(item: &ModelItem) -> serde_json::Value {
    serde_json::json!({
        "item_id": item.item_id.get(),
        "value": value_to_json(&item.value),
    })
}

fn value_to_json(v: &Value) -> serde_json::Value {
    match v {
        Value::Null => serde_json::Value::Null,
        Value::Bool(b) => serde_json::json!(b),
        Value::SignedInt(i) => serde_json::json!(i),
        Value::UnsignedInt(u) => serde_json::json!(u),
        Value::Float64(f) => serde_json::json!(f),
        Value::String(s) => serde_json::json!(s),
        other => serde_json::json!(format!("{other:?}")),
    }
}

fn property_name(p: PropertyRef) -> String {
    p.standard_name().unwrap_or("unknown").to_string()
}

fn type_name(t: TypeRef) -> String {
    t.standard_name().unwrap_or("unknown").to_string()
}

// ==============================================================================
// Tests
// ==============================================================================

#[test]
fn test_deterministic_store_trace_seeded_sequences() {
    for &seed in TRACE_SEEDS {
        if let Err(mismatch) = run_seeded_trace(seed) {
            persist_failing_trace(&mismatch);
            panic!(
                "store trace mismatch: seed={:#x} step={} rejected={} live_rev={} ref_rev={}",
                mismatch.seed,
                mismatch.step,
                mismatch.rejected,
                mismatch.live_snapshot.revision.get(),
                mismatch.reference_snapshot.revision.get(),
            );
        }
    }
}

#[test]
fn test_trace_reference_model_matches_replay_on_empty_commit_log() {
    let limits = trace_limits();
    let store = SemanticStore::with_limits(limits.clone());
    assert_eq!(take_snapshot(&store), reference_snapshot(&[], &limits));
}
