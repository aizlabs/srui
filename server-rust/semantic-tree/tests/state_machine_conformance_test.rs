//! SRUI Core State-Machine Conformance Test Runner (§32 items 1 & 3, §4).
//!
//! Loads and executes declarative conformance vectors from `protocol/conformance-vectors/state-machine/*.json`,
//! validating atomic transactions, identity invariants, safety limits, model operations, rollback behavior,
//! and the "semantic-not-paint" architectural invariant.

use serde::Deserialize;
use serde_json::Value as JsonValue;
use srui_semantic_tree::*;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

// ==============================================================================
// Fixture Data Models (JSON Deserialization)
// ==============================================================================

#[derive(Debug, Deserialize)]
struct Fixture {
    #[allow(dead_code)]
    name: String,
    #[allow(dead_code)]
    description: String,
    #[allow(dead_code)]
    spec_sections: Vec<String>,
    #[serde(default)]
    initial_limits: Option<FixtureLimits>,
    #[serde(default)]
    initial_revision: Option<u64>,
    #[serde(default)]
    setup_transactions: Vec<FixtureTransaction>,
    transaction: FixtureTransaction,
    expected_outcome: FixtureExpectedOutcome,
}

#[derive(Debug, Deserialize, Default)]
struct FixtureLimits {
    max_tree_depth: Option<usize>,
    max_node_count: Option<usize>,
    max_transaction_operations: Option<usize>,
    max_string_length: Option<usize>,
    max_value_depth: Option<usize>,
    max_list_length: Option<usize>,
    max_list_elements: Option<usize>,
    max_record_properties: Option<usize>,
    max_model_count: Option<usize>,
    max_cached_items_per_model: Option<usize>,
    max_items_per_model_operation: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct FixtureTransaction {
    base_revision: u64,
    new_revision: u64,
    operations: Vec<FixtureOperation>,
}

#[derive(Debug, Deserialize)]
struct FixtureOperation {
    #[serde(rename = "type")]
    op_type: String,
    // CREATE_NODE / DELETE_NODE / SET_PROPERTY / CLEAR_PROPERTY / BATCH_PROPERTY_SET / MOVE_NODE
    node_id: Option<u64>,
    node_type: Option<JsonValue>,
    parent_id: Option<Option<u64>>,
    child_index: Option<Option<usize>>,
    #[serde(default)]
    properties: Option<JsonValue>,
    property: Option<JsonValue>,
    value: Option<JsonValue>,
    new_parent_id: Option<Option<u64>>,
    new_child_index: Option<Option<usize>>,
    new_order: Option<Vec<u64>>,
    // CREATE_MODEL / MODEL_INSERT / MODEL_DELETE / MODEL_UPDATE / MODEL_RESET_RANGE
    model_id: Option<u64>,
    model_type: Option<JsonValue>,
    item_count: Option<u64>,
    index: Option<Option<u64>>,
    count: Option<Option<u64>>,
    #[serde(default)]
    item_ids: Option<Vec<u64>>,
    #[serde(default)]
    items: Option<Vec<FixtureModelItem>>,
    start_index: Option<u64>,
    total_count: Option<Option<u64>>,
}

#[derive(Debug, Deserialize)]
struct FixtureModelItem {
    item_id: u64,
    value: JsonValue,
    #[serde(default)]
    properties: HashMap<String, JsonValue>,
}

#[derive(Debug, Deserialize)]
struct FixtureExpectedOutcome {
    status: String,
    committed_revision: Option<u64>,
    store_state: Option<FixtureStoreState>,
    error_code: Option<String>,
    failed_op_index: Option<usize>,
    expected_store_revision: Option<u64>,
    #[serde(default)]
    rollback_verified: bool,
}

#[derive(Debug, Deserialize)]
struct FixtureStoreState {
    node_count: usize,
    roots: Vec<u64>,
    #[serde(default)]
    nodes: HashMap<String, FixtureNode>,
    #[serde(default)]
    model_count: usize,
    #[serde(default)]
    models: HashMap<String, FixtureModel>,
}

#[derive(Debug, Deserialize)]
struct FixtureNode {
    #[allow(dead_code)]
    node_id: u64,
    node_type: JsonValue,
    parent_id: Option<u64>,
    #[serde(default)]
    ordered_children: Vec<u64>,
    #[serde(default)]
    properties: HashMap<String, JsonValue>,
}

#[derive(Debug, Deserialize)]
struct FixtureModel {
    #[allow(dead_code)]
    model_id: u64,
    #[allow(dead_code)]
    model_type: JsonValue,
    item_count: u64,
    cached_item_count: usize,
    #[serde(default)]
    cached_ranges: Vec<FixtureRange>,
    #[serde(default)]
    items: HashMap<String, FixtureModelItem>,
}

#[derive(Debug, Deserialize)]
struct FixtureRange {
    start: u64,
    length: u64,
}

// ==============================================================================
// Helper Converters: JSON -> SRUI Core Types
// ==============================================================================

fn resolve_type_ref(val: &JsonValue) -> TypeRef {
    match val {
        JsonValue::String(s) => resolve_standard_node_type(s)
            .unwrap_or_else(|e| panic!("Unknown node type {}: {}", s, e)),
        JsonValue::Object(map) => {
            let ns = map.get("namespace_id").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            let local = map.get("local_id").and_then(|v| v.as_u64()).unwrap() as u32;
            TypeRef::new(ns, local)
        }
        JsonValue::Number(n) => TypeRef::standard(n.as_u64().unwrap() as u32),
        other => panic!("Invalid type_ref JSON: {:?}", other),
    }
}

fn resolve_property_ref(val: &JsonValue) -> PropertyRef {
    match val {
        JsonValue::String(s) => resolve_standard_property(s)
            .unwrap_or_else(|e| panic!("Unknown property {}: {}", s, e)),
        JsonValue::Object(map) => {
            let ns = map.get("namespace_id").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            let local = map.get("local_id").and_then(|v| v.as_u64()).unwrap() as u32;
            PropertyRef::new(ns, local)
        }
        JsonValue::Number(n) => PropertyRef::standard(n.as_u64().unwrap() as u32),
        other => panic!("Invalid property_ref JSON: {:?}", other),
    }
}

fn resolve_enum_token(enum_name: &str, val_name: &str) -> EnumToken {
    let enum_id = lookup_standard_enum(enum_name)
        .or_else(|| lookup_standard_enum(&format!("Enum{}", enum_name)))
        .unwrap_or_else(|| panic!("Unknown standard enum: {}", enum_name));

    // Lookup value ID for the specific enum
    let val_id = match (enum_name, val_name) {
        // ActionRole
        ("ActionRole", "normal") => 1,
        ("ActionRole", "primary") => 2,
        ("ActionRole", "destructive") => 3,
        ("ActionRole", "quiet") => 4,
        // TextRole
        ("TextRole", "title") => 1,
        ("TextRole", "heading") => 2,
        ("TextRole", "body") => 3,
        ("TextRole", "caption") => 4,
        ("TextRole", "code") => 5,
        ("TextRole", "status") => 6,
        ("TextRole", "warning") => 7,
        ("TextRole", "error") => 8,
        // InputRole
        ("InputRole", "plain") => 1,
        ("InputRole", "search") => 2,
        ("InputRole", "secure") => 3,
        ("InputRole", "command") => 4,
        // Importance
        ("Importance", "normal") => 1,
        ("Importance", "emphasized") => 2,
        ("Importance", "de_emphasized") => 3,
        // TogglePresentationHint
        ("TogglePresentationHint", "automatic") => 1,
        ("TogglePresentationHint", "checkbox") => 2,
        ("TogglePresentationHint", "switch") => 3,
        // Visibility
        ("Visibility", "visible") => 1,
        ("Visibility", "hidden") => 2,
        ("Visibility", "collapsed") => 3,
        // SpacingRole
        ("SpacingRole", "none") => 1,
        ("SpacingRole", "tight") => 2,
        ("SpacingRole", "normal") => 3,
        ("SpacingRole", "relaxed") => 4,
        // PaddingRole
        ("PaddingRole", "none") => 1,
        ("PaddingRole", "tight") => 2,
        ("PaddingRole", "normal") => 3,
        ("PaddingRole", "relaxed") => 4,
        // HorizontalAlignment
        ("HorizontalAlignment", "leading") => 1,
        ("HorizontalAlignment", "center") => 2,
        ("HorizontalAlignment", "trailing") => 3,
        ("HorizontalAlignment", "fill") => 4,
        // VerticalAlignment
        ("VerticalAlignment", "top") => 1,
        ("VerticalAlignment", "center") => 2,
        ("VerticalAlignment", "bottom") => 3,
        ("VerticalAlignment", "fill") => 4,
        // SelectionMode
        ("SelectionMode", "none") => 1,
        ("SelectionMode", "single") => 2,
        ("SelectionMode", "multiple") => 3,
        // ValidationState
        ("ValidationState", "valid") => 1,
        ("ValidationState", "warning") => 2,
        ("ValidationState", "error") => 3,
        _ => panic!("Unknown enum value pair: ({}, {})", enum_name, val_name),
    };

    EnumToken::new(enum_id, val_id)
}

fn convert_value(val: &JsonValue) -> Value {
    match val {
        JsonValue::Null => Value::Null,
        JsonValue::Bool(b) => Value::Bool(*b),
        JsonValue::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::SignedInt(i)
            } else if let Some(u) = n.as_u64() {
                Value::UnsignedInt(u)
            } else if let Some(f) = n.as_f64() {
                Value::Float64(f)
            } else {
                panic!("Invalid number: {:?}", n)
            }
        }
        JsonValue::String(s) => Value::String(s.clone()),
        JsonValue::Array(arr) => {
            let list: Vec<Value> = arr.iter().map(convert_value).collect();
            Value::List(list)
        }
        JsonValue::Object(map) => {
            if let (Some(enum_val), Some(val_val)) = (map.get("enum"), map.get("value")) {
                let enum_name = enum_val.as_str().expect("enum name string");
                let val_name = val_val.as_str().expect("value name string");
                Value::EnumToken(resolve_enum_token(enum_name, val_name))
            } else if let (Some(enum_id), Some(val_id)) = (map.get("enum_id"), map.get("value_id")) {
                Value::EnumToken(EnumToken::new(
                    enum_id.as_u64().unwrap() as u32,
                    val_id.as_u64().unwrap() as u32,
                ))
            } else if let Some(node_id) = map.get("node_id") {
                Value::NodeId(NodeId::new(node_id.as_u64().unwrap()))
            } else if let Some(item_id) = map.get("item_id") {
                Value::ItemId(ItemId::new(item_id.as_u64().unwrap()))
            } else if let Some(hash_str) = map.get("resource_hash") {
                let hash = ResourceHash::from_hex(hash_str.as_str().unwrap())
                    .expect("valid resource hash hex");
                Value::ResourceHash(hash)
            } else if let (Some(w), Some(h)) = (map.get("width"), map.get("height")) {
                Value::Size(Size::new(w.as_f64().unwrap(), h.as_f64().unwrap()))
            } else if let (Some(x), Some(y), Some(w), Some(h)) =
                (map.get("x"), map.get("y"), map.get("width"), map.get("height"))
            {
                Value::Rect(Rect::new(
                    x.as_f64().unwrap(),
                    y.as_f64().unwrap(),
                    w.as_f64().unwrap(),
                    h.as_f64().unwrap(),
                ))
            } else if let (Some(x), Some(y)) = (map.get("x"), map.get("y")) {
                Value::Point(Point::new(x.as_f64().unwrap(), y.as_f64().unwrap()))
            } else if let (Some(s), Some(l)) = (map.get("start"), map.get("length")) {
                Value::Range(Range::new(s.as_u64().unwrap(), l.as_u64().unwrap()))
            } else if let (Some(t), Some(lead), Some(b), Some(tr)) =
                (map.get("top"), map.get("leading"), map.get("bottom"), map.get("trailing"))
            {
                Value::EdgeInsets(EdgeInsets::new(
                    t.as_f64().unwrap(),
                    lead.as_f64().unwrap(),
                    b.as_f64().unwrap(),
                    tr.as_f64().unwrap(),
                ))
            } else if let (Some(rec_type), Some(props)) = (map.get("record_type"), map.get("properties")) {
                let type_ref = resolve_type_ref(rec_type);
                let prop_map = props.as_object().expect("record properties object");
                let mut record_props = Vec::new();
                for (k, v) in prop_map {
                    let p_ref = resolve_property_ref(&JsonValue::String(k.clone()));
                    record_props.push(Property::new(p_ref, convert_value(v)));
                }
                Value::Record(SmallRecord::new(type_ref, record_props))
            } else {
                panic!("Unrecognized structured value object in fixture: {:?}", map)
            }
        }
    }
}

fn convert_properties(json_props: &Option<JsonValue>) -> Vec<(PropertyRef, Value)> {
    let mut props = Vec::new();
    if let Some(val) = json_props {
        match val {
            JsonValue::Object(map) => {
                for (k, v) in map {
                    let prop_ref = resolve_property_ref(&JsonValue::String(k.clone()));
                    let prop_val = convert_value(v);
                    props.push((prop_ref, prop_val));
                }
            }
            JsonValue::Array(arr) => {
                for item in arr {
                    let prop_obj = item.as_object().expect("property item object");
                    let prop_ref = resolve_property_ref(prop_obj.get("property").unwrap());
                    let prop_val = convert_value(prop_obj.get("value").unwrap());
                    props.push((prop_ref, prop_val));
                }
            }
            _ => {}
        }
    }
    props
}

fn convert_model_item(item: &FixtureModelItem) -> ModelItem {
    let item_id = ItemId::new(item.item_id);
    let value = convert_value(&item.value);
    let mut properties = HashMap::new();
    for (k, v) in &item.properties {
        let p_ref = resolve_property_ref(&JsonValue::String(k.clone()));
        properties.insert(p_ref, convert_value(v));
    }
    ModelItem {
        item_id,
        value,
        properties,
    }
}

fn convert_operation(op: &FixtureOperation) -> Operation {
    match op.op_type.as_str() {
        "CREATE_NODE" => {
            let id = NodeId::new(op.node_id.expect("node_id for CREATE_NODE"));
            let node_type = resolve_type_ref(op.node_type.as_ref().expect("node_type for CREATE_NODE"));
            let parent_id = op.parent_id.flatten().map(NodeId::new);
            let child_index = op.child_index.flatten();
            let properties = convert_properties(&op.properties);
            Operation::create_node(id, node_type, parent_id, child_index, properties)
        }
        "DELETE_NODE" => {
            let id = NodeId::new(op.node_id.expect("node_id for DELETE_NODE"));
            Operation::delete_node(id)
        }
        "SET_PROPERTY" => {
            let id = NodeId::new(op.node_id.expect("node_id for SET_PROPERTY"));
            let property = resolve_property_ref(op.property.as_ref().expect("property for SET_PROPERTY"));
            let value = convert_value(op.value.as_ref().expect("value for SET_PROPERTY"));
            Operation::set_property(id, property, value)
        }
        "CLEAR_PROPERTY" => {
            let id = NodeId::new(op.node_id.expect("node_id for CLEAR_PROPERTY"));
            let property = resolve_property_ref(op.property.as_ref().expect("property for CLEAR_PROPERTY"));
            Operation::clear_property(id, property)
        }
        "BATCH_PROPERTY_SET" => {
            let id = NodeId::new(op.node_id.expect("node_id for BATCH_PROPERTY_SET"));
            let properties = convert_properties(&op.properties);
            Operation::batch_property_set(id, properties)
        }
        "MOVE_NODE" => {
            let id = NodeId::new(op.node_id.expect("node_id for MOVE_NODE"));
            let new_parent_id = op.new_parent_id.flatten().map(NodeId::new);
            let new_child_index = op.new_child_index.flatten();
            Operation::move_node(id, new_parent_id, new_child_index)
        }
        "REORDER_CHILDREN" => {
            let parent_id = NodeId::new(op.node_id.or(op.parent_id.flatten()).expect("parent_id for REORDER_CHILDREN"));
            let new_order = op
                .new_order
                .as_ref()
                .expect("new_order for REORDER_CHILDREN")
                .iter()
                .map(|id| NodeId::new(*id))
                .collect::<Vec<_>>();
            Operation::reorder_children(parent_id, new_order)
        }
        "CREATE_MODEL" => {
            let id = ModelId::new(op.model_id.expect("model_id for CREATE_MODEL"));
            let model_type = resolve_type_ref(op.model_type.as_ref().expect("model_type for CREATE_MODEL"));
            let item_count = op.item_count.expect("item_count for CREATE_MODEL");
            Operation::create_model(id, model_type, item_count)
        }
        "MODEL_INSERT" => {
            let id = ModelId::new(op.model_id.expect("model_id for MODEL_INSERT"));
            let index = op.index.flatten().expect("index for MODEL_INSERT");
            let items: Vec<ModelItem> = op
                .items
                .as_ref()
                .expect("items for MODEL_INSERT")
                .iter()
                .map(convert_model_item)
                .collect();
            Operation::model_insert(id, index, items)
        }
        "MODEL_DELETE" => {
            let id = ModelId::new(op.model_id.expect("model_id for MODEL_DELETE"));
            let index = op.index.flatten();
            let count = op.count.flatten();
            let item_ids: Vec<ItemId> = op
                .item_ids
                .as_ref()
                .map(|v| v.iter().map(|id| ItemId::new(*id)).collect())
                .unwrap_or_default();
            Operation::model_delete(id, index, count, item_ids)
        }
        "MODEL_UPDATE" => {
            let id = ModelId::new(op.model_id.expect("model_id for MODEL_UPDATE"));
            let index = op.index.flatten();
            let items: Vec<ModelItem> = op
                .items
                .as_ref()
                .expect("items for MODEL_UPDATE")
                .iter()
                .map(convert_model_item)
                .collect();
            Operation::model_update(id, index, items)
        }
        "MODEL_RESET_RANGE" => {
            let id = ModelId::new(op.model_id.expect("model_id for MODEL_RESET_RANGE"));
            let start_index = op.start_index.expect("start_index for MODEL_RESET_RANGE");
            let total_count = op.total_count.flatten();
            let items: Vec<ModelItem> = op
                .items
                .as_ref()
                .expect("items for MODEL_RESET_RANGE")
                .iter()
                .map(convert_model_item)
                .collect();
            Operation::model_reset_range(id, start_index, items, total_count)
        }
        other => panic!("Unsupported operation type in fixture: {}", other),
    }
}

fn build_store_limits(fl: &Option<FixtureLimits>) -> StoreLimits {
    let mut limits = StoreLimits::default();
    if let Some(l) = fl {
        if let Some(v) = l.max_tree_depth {
            limits.max_tree_depth = v;
        }
        if let Some(v) = l.max_node_count {
            limits.max_node_count = v;
        }
        if let Some(v) = l.max_transaction_operations {
            limits.max_transaction_operations = v;
        }
        if let Some(v) = l.max_string_length {
            limits.max_string_length = v;
        }
        if let Some(v) = l.max_value_depth {
            limits.max_value_depth = v;
        }
        if let Some(v) = l.max_list_length.or(l.max_list_elements) {
            limits.max_list_elements = v;
        }
        if let Some(v) = l.max_record_properties {
            limits.max_record_properties = v;
        }
        if let Some(v) = l.max_model_count {
            limits.max_model_count = v;
        }
        if let Some(v) = l.max_cached_items_per_model {
            limits.max_cached_items_per_model = v;
        }
        if let Some(v) = l.max_items_per_model_operation {
            limits.max_items_per_model_operation = v;
        }
    }
    limits
}

fn get_fixture_files() -> Vec<PathBuf> {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let dir = Path::new(manifest_dir).join("../../protocol/conformance-vectors/state-machine");
    let mut files = Vec::new();
    for entry in fs::read_dir(&dir).unwrap_or_else(|e| panic!("Failed to read dir {:?}: {}", dir, e)) {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("json") {
            files.push(path);
        }
    }
    files.sort();
    assert!(
        files.len() >= 14,
        "Expected at least 14 conformance fixtures in {:?}, found {}",
        dir,
        files.len()
    );
    files
}

// ==============================================================================
// State Snapshot for Rollback Verification
// ==============================================================================

#[derive(Clone, Debug, PartialEq)]
struct StoreSnapshot {
    revision: Revision,
    node_count: usize,
    roots: Vec<NodeId>,
    nodes: BTreeMap<NodeId, (TypeRef, Option<NodeId>, Vec<NodeId>, HashMap<PropertyRef, Value>)>,
    model_count: usize,
    models: BTreeMap<ModelId, (TypeRef, u64, usize)>,
}

fn take_snapshot(store: &SemanticStore) -> StoreSnapshot {
    let mut nodes = BTreeMap::new();
    for &root in store.root_ids() {
        collect_nodes_snapshot(store, root, &mut nodes);
    }

    let mut models = BTreeMap::new();
    // Snapshot active models
    for m_id in 1..=1000u64 {
        if let Some(model) = store.get_model(ModelId::new(m_id)) {
            models.insert(
                ModelId::new(m_id),
                (model.model_type, model.item_count, model.cached_item_count()),
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
    nodes: &mut BTreeMap<NodeId, (TypeRef, Option<NodeId>, Vec<NodeId>, HashMap<PropertyRef, Value>)>,
) {
    if let Some(n) = store.get_node(id) {
        nodes.insert(
            id,
            (
                n.node_type,
                n.parent_id,
                n.ordered_children.clone(),
                n.properties.clone(),
            ),
        );
        for &child in &n.ordered_children {
            collect_nodes_snapshot(store, child, nodes);
        }
    }
}

// ==============================================================================
// State-Machine Fixture Test Replayer
// ==============================================================================

fn replay_fixture(path: &Path) {
    let text = fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("Failed to read fixture {:?}: {}", path, e));
    let fixture: Fixture = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("Failed to parse JSON in {:?}: {}", path, e));

    let limits = build_store_limits(&fixture.initial_limits);
    let initial_rev = Revision::new(fixture.initial_revision.unwrap_or(0));
    let mut store = SemanticStore::with_limits_and_revision(limits, initial_rev);

    // 1. Replay setup transactions
    for (idx, setup_tx) in fixture.setup_transactions.iter().enumerate() {
        let ops: Vec<Operation> = setup_tx.operations.iter().map(convert_operation).collect();
        let res = store.apply_transaction(setup_tx.base_revision, ops);
        assert!(
            res.is_ok(),
            "Setup transaction #{} in {:?} failed unexpectedly: {:?}",
            idx,
            path.file_name().unwrap(),
            res.err()
        );
        assert_eq!(
            res.unwrap().get(),
            setup_tx.new_revision,
            "Setup transaction #{} revision mismatch in {:?}",
            idx,
            path.file_name().unwrap()
        );
    }

    // 2. Capture pre-transaction snapshot for rollback validation
    let pre_snapshot = take_snapshot(&store);

    // 3. Replay target transaction under test
    let target_tx = &fixture.transaction;
    let target_ops: Vec<Operation> = target_tx.operations.iter().map(convert_operation).collect();
    let result = store.apply_transaction(target_tx.base_revision, target_ops);

    // 4. Assert outcome
    match fixture.expected_outcome.status.as_str() {
        "success" => {
            assert!(
                result.is_ok(),
                "Transaction in {:?} was expected to succeed, but failed with: {:?}",
                path.file_name().unwrap(),
                result.err()
            );
            let committed_rev = result.unwrap();
            let exp_rev = fixture.expected_outcome.committed_revision.expect("committed_revision");
            assert_eq!(
                committed_rev.get(),
                exp_rev,
                "Committed revision mismatch in {:?}",
                path.file_name().unwrap()
            );
            assert_eq!(
                store.revision().get(),
                exp_rev,
                "Store revision mismatch in {:?}",
                path.file_name().unwrap()
            );

            // Validate store state assertions
            if let Some(state) = &fixture.expected_outcome.store_state {
                assert_eq!(
                    store.node_count(),
                    state.node_count,
                    "Store node_count mismatch in {:?}",
                    path.file_name().unwrap()
                );
                let exp_roots: Vec<NodeId> = state.roots.iter().map(|id| NodeId::new(*id)).collect();
                assert_eq!(
                    store.root_ids(),
                    exp_roots.as_slice(),
                    "Store root IDs mismatch in {:?}",
                    path.file_name().unwrap()
                );

                for (id_str, exp_node) in &state.nodes {
                    let node_id = NodeId::new(id_str.parse::<u64>().expect("numeric node_id key"));
                    let node = store
                        .get_node(node_id)
                        .unwrap_or_else(|| panic!("Expected node {} missing in {:?}", node_id, path));

                    let exp_type = resolve_type_ref(&exp_node.node_type);
                    assert_eq!(
                        node.node_type, exp_type,
                        "NodeType mismatch for node {} in {:?}",
                        node_id, path
                    );
                    assert_eq!(
                        node.parent_id,
                        exp_node.parent_id.map(NodeId::new),
                        "Parent ID mismatch for node {} in {:?}",
                        node_id,
                        path
                    );
                    let exp_children: Vec<NodeId> =
                        exp_node.ordered_children.iter().map(|c| NodeId::new(*c)).collect();
                    assert_eq!(
                        node.ordered_children, exp_children,
                        "Ordered children mismatch for node {} in {:?}",
                        node_id, path
                    );

                    for (k, v) in &exp_node.properties {
                        let prop_ref = resolve_property_ref(&JsonValue::String(k.clone()));
                        let exp_val = convert_value(v);
                        let actual_val = node.properties.get(&prop_ref).unwrap_or_else(|| {
                            panic!("Property {} missing on node {} in {:?}", k, node_id, path)
                        });
                        assert_eq!(
                            actual_val, &exp_val,
                            "Property {} value mismatch on node {} in {:?}",
                            k, node_id, path
                        );
                    }
                }

                // Assert collection model state if defined
                assert_eq!(
                    store.model_count(),
                    state.model_count,
                    "Store model_count mismatch in {:?}",
                    path.file_name().unwrap()
                );
                for (id_str, exp_model) in &state.models {
                    let model_id = ModelId::new(id_str.parse::<u64>().expect("numeric model_id key"));
                    let model = store
                        .get_model(model_id)
                        .unwrap_or_else(|| panic!("Expected model {} missing in {:?}", model_id, path));

                    assert_eq!(
                        model.item_count, exp_model.item_count,
                        "Model {} item_count mismatch in {:?}",
                        model_id, path
                    );
                    assert_eq!(
                        model.cached_item_count(),
                        exp_model.cached_item_count,
                        "Model {} cached_item_count mismatch in {:?}",
                        model_id,
                        path
                    );

                    let exp_ranges: Vec<Range> = exp_model
                        .cached_ranges
                        .iter()
                        .map(|r| Range::new(r.start, r.length))
                        .collect();
                    assert_eq!(
                        model.cached_ranges(),
                        exp_ranges,
                        "Model {} cached_ranges mismatch in {:?}",
                        model_id,
                        path
                    );

                    for (idx_str, exp_item) in &exp_model.items {
                        let idx = idx_str.parse::<u64>().expect("numeric item index key");
                        let item = model
                            .get_item_by_index(idx)
                            .unwrap_or_else(|| panic!("Item at index {} missing in model {} in {:?}", idx, model_id, path));

                        assert_eq!(
                            item.item_id.get(),
                            exp_item.item_id,
                            "Item ID mismatch at index {} in model {} in {:?}",
                            idx,
                            model_id,
                            path
                        );
                        let exp_val = convert_value(&exp_item.value);
                        assert_eq!(
                            item.value, exp_val,
                            "Item value mismatch at index {} in model {} in {:?}",
                            idx, model_id, path
                        );
                    }
                }
            }
        }
        "rejected" => {
            assert!(
                result.is_err(),
                "Transaction in {:?} was expected to fail, but succeeded with revision: {:?}",
                path.file_name().unwrap(),
                result.ok()
            );
            let err = result.unwrap_err();
            let exp_err_code = fixture
                .expected_outcome
                .error_code
                .as_ref()
                .expect("error_code for rejected outcome");

            match exp_err_code.as_str() {
                "stale_base_revision" => {
                    assert!(
                        matches!(err, TxnError::StaleBaseRevision { .. }),
                        "Expected StaleBaseRevision, got {:?}",
                        err
                    );
                }
                "invalid_new_revision" => {
                    assert!(
                        matches!(err, TxnError::InvalidNewRevision { .. }),
                        "Expected InvalidNewRevision, got {:?}",
                        err
                    );
                }
                "max_operations_exceeded" => {
                    assert!(
                        matches!(err, TxnError::MaxOperationsExceeded { .. }),
                        "Expected MaxOperationsExceeded, got {:?}",
                        err
                    );
                }
                "node_id_already_used" => {
                    assert!(
                        matches!(err, TxnError::OpFailed { source: StoreError::NodeIdAlreadyUsed(..), .. }),
                        "Expected NodeIdAlreadyUsed, got {:?}",
                        err
                    );
                }
                "node_not_found" => {
                    assert!(
                        matches!(err, TxnError::OpFailed { source: StoreError::NodeNotFound(..), .. }),
                        "Expected NodeNotFound, got {:?}",
                        err
                    );
                }
                "parent_not_found" => {
                    assert!(
                        matches!(err, TxnError::OpFailed { source: StoreError::ParentNotFound(..), .. }),
                        "Expected ParentNotFound, got {:?}",
                        err
                    );
                }
                "max_node_count_exceeded" => {
                    assert!(
                        matches!(err, TxnError::OpFailed { source: StoreError::MaxNodeCountExceeded { .. }, .. }),
                        "Expected MaxNodeCountExceeded, got {:?}",
                        err
                    );
                }
                "max_tree_depth_exceeded" => {
                    assert!(
                        matches!(err, TxnError::OpFailed { source: StoreError::MaxTreeDepthExceeded { .. }, .. }),
                        "Expected MaxTreeDepthExceeded, got {:?}",
                        err
                    );
                }
                "max_string_length_exceeded" => {
                    assert!(
                        matches!(err, TxnError::OpFailed { source: StoreError::MaxStringLengthExceeded { .. }, .. }),
                        "Expected MaxStringLengthExceeded, got {:?}",
                        err
                    );
                }
                "max_model_count_exceeded" => {
                    assert!(
                        matches!(err, TxnError::OpFailed { source: StoreError::MaxModelCountExceeded { .. }, .. }),
                        "Expected MaxModelCountExceeded, got {:?}",
                        err
                    );
                }
                "max_cached_items_per_model_exceeded" => {
                    assert!(
                        matches!(err, TxnError::OpFailed { source: StoreError::MaxCachedItemsPerModelExceeded { .. }, .. }),
                        "Expected MaxCachedItemsPerModelExceeded, got {:?}",
                        err
                    );
                }
                "max_items_per_model_operation_exceeded" => {
                    assert!(
                        matches!(err, TxnError::OpFailed { source: StoreError::MaxItemsPerModelOperationExceeded { .. }, .. }),
                        "Expected MaxItemsPerModelOperationExceeded, got {:?}",
                        err
                    );
                }
                "model_id_already_used" => {
                    assert!(
                        matches!(err, TxnError::OpFailed { source: StoreError::ModelIdAlreadyUsed(..), .. }),
                        "Expected ModelIdAlreadyUsed, got {:?}",
                        err
                    );
                }
                "model_not_found" => {
                    assert!(
                        matches!(err, TxnError::OpFailed { source: StoreError::ModelNotFound(..), .. }),
                        "Expected ModelNotFound, got {:?}",
                        err
                    );
                }
                "item_not_found" => {
                    assert!(
                        matches!(err, TxnError::OpFailed { source: StoreError::ItemNotFound(..), .. }),
                        "Expected ItemNotFound, got {:?}",
                        err
                    );
                }
                "cycle_detected" => {
                    assert!(
                        matches!(err, TxnError::OpFailed { source: StoreError::CycleDetected { .. }, .. }),
                        "Expected CycleDetected, got {:?}",
                        err
                    );
                }
                "child_index_out_of_bounds" => {
                    assert!(
                        matches!(err, TxnError::OpFailed { source: StoreError::ChildIndexOutOfBounds { .. }, .. }),
                        "Expected ChildIndexOutOfBounds, got {:?}",
                        err
                    );
                }
                "invalid_children_reorder" => {
                    assert!(
                        matches!(err, TxnError::OpFailed { source: StoreError::InvalidChildrenReorder { .. }, .. }),
                        "Expected InvalidChildrenReorder, got {:?}",
                        err
                    );
                }
                "invalid_model_delete" => {
                    assert!(
                        matches!(err, TxnError::OpFailed { source: StoreError::InvalidModelDelete(..), .. }),
                        "Expected InvalidModelDelete, got {:?}",
                        err
                    );
                }
                other => panic!("Unrecognized expected error_code: {}", other),
            }

            if let Some(exp_op_idx) = fixture.expected_outcome.failed_op_index {
                match err {
                    TxnError::OpFailed { op_index, .. } => {
                        assert_eq!(
                            op_index, exp_op_idx,
                            "Failed operation index mismatch in {:?}",
                            path.file_name().unwrap()
                        );
                    }
                    other => panic!("Expected TxnError::OpFailed with op_index, got {:?}", other),
                }
            }

            if fixture.expected_outcome.rollback_verified {
                let exp_store_rev = fixture
                    .expected_outcome
                    .expected_store_revision
                    .unwrap_or(fixture.initial_revision.unwrap_or(0));
                assert_eq!(
                    store.revision().get(),
                    exp_store_rev,
                    "Store revision corrupted after transaction rollback in {:?}",
                    path.file_name().unwrap()
                );

                let post_snapshot = take_snapshot(&store);
                assert_eq!(
                    pre_snapshot, post_snapshot,
                    "Store state was mutated despite transaction rollback in {:?}",
                    path.file_name().unwrap()
                );
            }
        }
        other => panic!("Unknown outcome status: {}", other),
    }
}

// ==============================================================================
// Integration Test Cases
// ==============================================================================

#[test]
fn test_all_state_machine_conformance_fixtures() {
    let fixture_paths = get_fixture_files();
    println!(
        "Replaying {} state-machine conformance fixtures...",
        fixture_paths.len()
    );

    for path in &fixture_paths {
        println!("  -> Replaying fixture: {:?}", path.file_name().unwrap());
        replay_fixture(path);
    }
}

#[test]
fn test_semantic_not_paint_architectural_invariants() {
    // §32 item 3 & §4 Invariants:
    // Assert that the Protocol Core and Standard Widget Profile contain zero notions of:
    // 1. Frame cadence (no start_frame, end_frame, frame_id, vsync)
    // 2. Paint / drawing commands (no draw_rect, fill_path, set_pixel, draw_text)
    // 3. Mandatory absolute pixel geometry (no pixel_x, pixel_y, width_px, height_px)

    // 1. Verify Standard Node Types
    for &(id, name) in STANDARD_NODE_TYPES {
        assert!(
            !name.to_lowercase().contains("frame"),
            "Node type {} ({}) must not contain frame concepts (§4.16)",
            id,
            name
        );
        assert!(
            !name.to_lowercase().contains("paint"),
            "Node type {} ({}) must not contain paint concepts (§4.7)",
            id,
            name
        );
        assert!(
            !name.to_lowercase().contains("raster"),
            "Node type {} ({}) must not contain raster concepts (§4.7)",
            id,
            name
        );
    }

    // 2. Verify Standard Properties
    for &(id, name) in STANDARD_PROPERTIES {
        assert!(
            !name.contains("pixel_x") && !name.contains("pixel_y") && !name.contains("px"),
            "Standard property {} ({}) must not require absolute pixel coordinates (§4.17)",
            id,
            name
        );
        assert!(
            !name.contains("color_hex") && !name.contains("background_color") && !name.contains("brush"),
            "Standard property {} ({}) must not prescribe direct painting brushes (§4.7)",
            id,
            name
        );
        assert!(
            !name.contains("font_family") && !name.contains("font_size"),
            "Standard property {} ({}) must not dictate renderer fonts in v0.1 (§3.2, §4.7)",
            id,
            name
        );
    }

    // 3. Verify Standard Operations
    for &(id, name) in STANDARD_OPERATIONS {
        let name_lower = name.to_lowercase();
        assert!(
            !name_lower.contains("frame"),
            "Operation {} ({}) must not introduce display frame cadence (§4.16, §12.2)",
            id,
            name
        );
        assert!(
            !name_lower.contains("paint") && !name_lower.contains("draw") && !name_lower.contains("render"),
            "Operation {} ({}) must not be a paint or drawing command (§4.7, §7.1)",
            id,
            name
        );
    }

    // 4. Verify Standard Events
    for &(id, name) in STANDARD_EVENTS {
        assert!(
            !name.to_lowercase().contains("mouse_move") && !name.to_lowercase().contains("raw_pointer"),
            "Standard event {} ({}) must route semantic intent rather than raw pointer streams (§7.6, §7.7)",
            id,
            name
        );
    }
}
