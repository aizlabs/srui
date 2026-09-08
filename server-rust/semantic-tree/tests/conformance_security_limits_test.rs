//! SRUI Security Limits Conformance Suite (§32 item 9).
//!
//! Implements: §26 (resource/complexity limits), §27 (no server-supplied executable code),
//! §4 inv. 10/18, §32.9.
//!
//! §32 item 9 asks for "oversized/deep/malformed payloads and forbidden executable/script payload
//! classes". Malformed *wire* payloads are covered by the framing/decoder matrices this suite also
//! runs; this file covers the two halves those cannot reach:
//!
//! 1. every §26 store limit class refuses at its boundary — depth, node count, string length,
//!    value depth, list elements, record properties, transaction operations, and the model limits;
//! 2. §27's payload-class rule: nothing the server sends is executable on the client.
//!
//! Suite 1's vectors exercise several of these too, but §32 item 9 must stand alone: running the
//! security suite by itself has to establish the limit posture without depending on the
//! state-machine suite being run as well.

mod common;

use srui_semantic_tree::*;

fn store_with(limits: StoreLimits) -> SemanticStore {
    SemanticStore::with_limits(limits)
}

fn root(store: &mut SemanticStore) -> NodeId {
    let id = NodeId::new(1);
    store
        .create_node(id, TypeRef::SURFACE, None, None, [])
        .expect("root surface");
    id
}

/// §26: a tree deeper than `max_tree_depth` is refused at the boundary, not truncated.
#[test]
fn test_max_tree_depth_refuses_at_the_boundary() {
    let mut store = store_with(StoreLimits {
        max_tree_depth: 4,
        ..StoreLimits::default()
    });
    let mut parent = root(&mut store);

    // Depth 1 is the root; fill up to the limit.
    for depth in 2..=4u64 {
        let id = NodeId::new(depth);
        store
            .create_node(id, TypeRef::ROW, Some(parent), None, [])
            .unwrap_or_else(|e| panic!("depth {depth} must be accepted: {e:?}"));
        parent = id;
    }

    let overflow = store.create_node(NodeId::new(99), TypeRef::ROW, Some(parent), None, []);
    assert!(
        matches!(overflow, Err(StoreError::MaxTreeDepthExceeded { .. })),
        "depth 5 must be refused with MaxTreeDepthExceeded, got {overflow:?}"
    );
}

/// §26: `max_node_count` is a hard ceiling on the whole tree.
#[test]
fn test_max_node_count_refuses_at_the_boundary() {
    let mut store = store_with(StoreLimits {
        max_node_count: 3,
        ..StoreLimits::default()
    });
    let root_id = root(&mut store);
    for id in 2..=3u64 {
        store
            .create_node(NodeId::new(id), TypeRef::TEXT, Some(root_id), None, [])
            .expect("within node budget");
    }

    let overflow = store.create_node(NodeId::new(4), TypeRef::TEXT, Some(root_id), None, []);
    assert!(
        matches!(overflow, Err(StoreError::MaxNodeCountExceeded { .. })),
        "the fourth node must be refused, got {overflow:?}"
    );
}

/// §26: an oversized string is refused rather than truncated — a silently shortened label is a
/// different semantic claim than the one the server made.
#[test]
fn test_max_string_length_refuses_oversized_values() {
    let mut store = store_with(StoreLimits {
        max_string_length: 16,
        ..StoreLimits::default()
    });
    let root_id = root(&mut store);

    let at_limit = store.set_property(root_id, PropertyRef::LABEL, Value::String("x".repeat(16)));
    assert!(at_limit.is_ok(), "a string at the limit must be accepted");

    let over = store.set_property(root_id, PropertyRef::LABEL, Value::String("x".repeat(17)));
    assert!(
        matches!(over, Err(StoreError::MaxStringLengthExceeded { .. })),
        "a string one byte over the limit must be refused, got {over:?}"
    );
}

/// §26: nested values are depth-bounded, so a deeply nested payload cannot drive unbounded
/// recursion in any decoder or renderer that walks them.
#[test]
fn test_max_value_depth_refuses_deep_nesting() {
    let mut store = store_with(StoreLimits {
        max_value_depth: 3,
        ..StoreLimits::default()
    });
    let root_id = root(&mut store);

    let mut nested = Value::SignedInt(1);
    for _ in 0..3 {
        nested = Value::List(vec![nested]);
    }
    let over = store.set_property(root_id, PropertyRef::VALUE, nested);
    assert!(
        matches!(over, Err(StoreError::MaxValueDepthExceeded { .. })),
        "a value nested past max_value_depth must be refused, got {over:?}"
    );
}

/// §26: list and record payloads are bounded by element and property count.
#[test]
fn test_collection_valued_properties_are_bounded() {
    let mut store = store_with(StoreLimits {
        max_list_elements: 4,
        max_record_properties: 2,
        ..StoreLimits::default()
    });
    let root_id = root(&mut store);

    let long_list = Value::List((0..5).map(Value::SignedInt).collect());
    assert!(
        matches!(
            store.set_property(root_id, PropertyRef::VALUE, long_list),
            Err(StoreError::MaxListLengthExceeded { .. })
        ),
        "a list past max_list_elements must be refused"
    );

    let wide_record = Value::Record(SmallRecord::new(
        TypeRef::standard(1),
        vec![
            Property::new(PropertyRef::LABEL, Value::SignedInt(1)),
            Property::new(PropertyRef::VALUE, Value::SignedInt(2)),
            Property::new(PropertyRef::TEXT, Value::SignedInt(3)),
        ],
    ));

    assert!(
        matches!(
            store.set_property(root_id, PropertyRef::VALUE, wide_record),
            Err(StoreError::MaxRecordPropertiesExceeded { .. })
        ),
        "a record past max_record_properties must be refused"
    );
}

/// §26: `max_model_count` is asserted on its own, so no other limit can mask it.
#[test]
fn test_max_model_count_is_bounded() {
    let mut store = store_with(StoreLimits {
        max_model_count: 1,
        ..StoreLimits::default()
    });

    store
        .create_model(ModelId::new(1), TypeRef::standard(1), 100)
        .expect("first model");
    assert!(
        matches!(
            store.create_model(ModelId::new(2), TypeRef::standard(1), 100),
            Err(StoreError::MaxModelCountExceeded { .. })
        ),
        "a second model must be refused when max_model_count is 1"
    );
}

/// §26: `max_items_per_model_operation` bounds a single mutation.
#[test]
fn test_max_items_per_model_operation_is_bounded() {
    let mut store = store_with(StoreLimits {
        max_items_per_model_operation: 2,
        ..StoreLimits::default()
    });
    store
        .create_model(ModelId::new(1), TypeRef::standard(1), 100)
        .expect("model");

    let items: Vec<ModelItem> = (0..3)
        .map(|i| ModelItem::new(ItemId::new(i), Value::SignedInt(i as i64), []))
        .collect();
    assert!(
        matches!(
            store.model_insert(ModelId::new(1), 0, items),
            Err(StoreError::MaxItemsPerModelOperationExceeded { .. })
        ),
        "an insert past max_items_per_model_operation must be refused"
    );
}

/// §26: `max_cached_items_per_model` bounds the *aggregate* cache, across many individually
/// legal operations. The per-operation limit is set high here deliberately, so it cannot be the
/// thing doing the refusing — otherwise this test would pass without the aggregate limit
/// existing at all.
#[test]
fn test_max_cached_items_per_model_bounds_the_aggregate_not_one_operation() {
    let mut store = store_with(StoreLimits {
        max_cached_items_per_model: 5,
        // Deliberately larger than the cache ceiling: every individual insert below is legal.
        max_items_per_model_operation: 100,
        ..StoreLimits::default()
    });
    store
        .create_model(ModelId::new(1), TypeRef::standard(1), 1_000)
        .expect("model");

    // Five items in two legal operations: still within the cache budget.
    for batch in 0..2u64 {
        let items: Vec<ModelItem> = (0..2)
            .map(|i| {
                let id = batch * 2 + i;
                ModelItem::new(ItemId::new(id), Value::SignedInt(id as i64), [])
            })
            .collect();
        store
            .model_insert(ModelId::new(1), batch * 2, items)
            .unwrap_or_else(|e| panic!("batch {batch} is within budget: {e:?}"));
    }

    // One more legal-sized operation crosses the aggregate ceiling.
    let overflow: Vec<ModelItem> = (10..14)
        .map(|i| ModelItem::new(ItemId::new(i), Value::SignedInt(i as i64), []))
        .collect();
    assert!(
        matches!(
            store.model_insert(ModelId::new(1), 4, overflow),
            Err(StoreError::MaxCachedItemsPerModelExceeded { .. })
        ),
        "the aggregate cached-item ceiling must refuse growth even when every single operation \
         is within max_items_per_model_operation"
    );
}

/// §26: the per-transaction operation ceiling is a pre-check, so an oversized transaction is
/// refused before any operation is applied.
#[test]
fn test_max_transaction_operations_is_a_precheck() {
    let mut store = store_with(StoreLimits {
        max_transaction_operations: 2,
        ..StoreLimits::default()
    });
    let before = store.revision();

    let ops: Vec<Operation> = (10..13u64)
        .map(|id| Operation::CreateNode {
            id: NodeId::new(id),
            node_type: TypeRef::SURFACE,
            parent_id: None,
            child_index: None,
            properties: vec![],
        })
        .collect();

    let outcome = store.apply_transaction(before, ops);
    assert!(
        matches!(outcome, Err(TxnError::MaxOperationsExceeded { .. })),
        "an oversized transaction must be refused as a pre-check, got {outcome:?}"
    );
    assert_eq!(
        store.revision(),
        before,
        "a refused transaction must not consume a revision"
    );
    assert_eq!(store.node_count(), 0, "no operation may have been applied");
}

/// §27 / §4 inv. 10 & 18: script-looking payloads survive the store as inert data.
///
/// Name scanning alone proves nothing, so this drives real script-shaped strings through the
/// store and reads them back: they must round-trip byte-for-byte as `Value::String`, never be
/// parsed, evaluated, or reclassified into anything with a dispatch path.
#[test]
fn test_script_shaped_payloads_round_trip_as_inert_data() {
    let payloads = [
        "<script>alert('x')</script>",
        "javascript:void(0)",
        "${jndi:ldap://example.invalid/a}",
        "'; DROP TABLE nodes; --",
        "$(rm -rf /)",
        "\u{1b}]0;title\u{7}",
        "data:text/html;base64,PHNjcmlwdD4=",
    ];

    let (mut store, target) = {
        let mut store = SemanticStore::new();
        store
            .create_node(NodeId::new(1), TypeRef::SURFACE, None, None, [])
            .expect("root");
        store
            .create_node(
                NodeId::new(2),
                TypeRef::TEXT,
                Some(NodeId::new(1)),
                None,
                [],
            )
            .expect("text node");
        (store, NodeId::new(2))
    };

    for payload in payloads {
        store
            .set_property(
                target,
                PropertyRef::TEXT,
                Value::String(payload.to_string()),
            )
            .unwrap_or_else(|e| panic!("payload must be storable as data: {e:?}"));

        let stored = store
            .get_node(target)
            .and_then(|n| n.get_property(PropertyRef::TEXT))
            .cloned();

        assert_eq!(
            stored,
            Some(Value::String(payload.to_string())),
            "a script-shaped payload must round-trip unchanged as inert text (§27)"
        );
        // It stays a string. Nothing promotes it to a reference, resource, or callable.
        assert!(
            matches!(stored, Some(Value::String(_))),
            "a script-shaped payload must never be reclassified out of Value::String"
        );
    }
}

/// §27: the value type system has no variant that could name executable content — no code,
/// script, callable, or URL-with-scheme variant a renderer could be induced to dispatch.
///
/// `Value` is exhaustively matched here, so adding such a variant fails to compile.
#[test]
fn test_value_type_system_has_no_executable_variant() {
    let inhabitants = [
        Value::Null,
        Value::Bool(true),
        Value::SignedInt(1),
        Value::UnsignedInt(1),
        Value::Float64(1.0),
        Value::String(String::new()),
        Value::List(vec![]),
    ];

    for value in inhabitants {
        match value {
            // Data variants only. A `Value::Script`/`Value::Code`/`Value::Callable` addition
            // breaks this match, which is the point (§27, §4 inv. 10).
            Value::Null
            | Value::Bool(_)
            | Value::SignedInt(_)
            | Value::UnsignedInt(_)
            | Value::Float64(_)
            | Value::String(_)
            | Value::NodeId(_)
            | Value::ItemId(_)
            | Value::ResourceHash(_)
            | Value::EnumToken(_)
            | Value::Size(_)
            | Value::Point(_)
            | Value::Range(_)
            | Value::Rect(_)
            | Value::EdgeInsets(_)
            | Value::List(_)
            | Value::Record(_) => {}
        }
    }
}

/// §27 / §4 inv. 10: no standard property or node type *names* an execution surface either.
///
/// Matched against whole snake_case segments, not raw substrings: "accessible_description"
/// legitimately contains "script".
#[test]
fn test_no_standard_property_can_carry_executable_payload() {
    let executable_concepts = [
        "script",
        "javascript",
        "js",
        "bytecode",
        "wasm",
        "shader",
        "plugin",
        "eval",
        "exec",
        "shell",
        "code",
        "program",
    ];

    for &(id, name) in STANDARD_PROPERTIES {
        let segments: Vec<String> = name.split('_').map(str::to_lowercase).collect();
        for concept in executable_concepts {
            assert!(
                !segments.iter().any(|segment| segment == concept),
                "standard property {id} ('{name}') could carry an executable payload class \
                 ('{concept}'); the client executes no server-supplied code (§27, §4 inv. 10)"
            );
        }
    }

    // Node type names are UpperCamelCase, so they are split on case boundaries and matched whole,
    // exactly as the snake_case properties above are split on '_'. A raw substring test would
    // report a future `Subscription` or `Description` as a scripting surface.
    for &(id, name) in STANDARD_NODE_TYPES {
        let mut words: Vec<String> = Vec::new();
        for ch in name.chars() {
            if ch.is_uppercase() || words.is_empty() {
                words.push(String::new());
            }
            words
                .last_mut()
                .expect("a word is always pushed before the first character")
                .push(ch.to_ascii_lowercase());
        }
        for concept in ["script", "webview", "browser", "plugin", "canvas"] {
            assert!(
                !words.iter().any(|word| word == concept),
                "standard node type {id} ('{name}') implies an execution surface ('{concept}'); \
                 the base client hosts no scripting environment (§27, §4 inv. 10)"
            );
        }
    }
}

/// §26: limits are per-store configuration, and the defaults are finite. An unbounded default
/// would make every other assertion in this suite vacuous in production.
#[test]
fn test_default_limits_are_finite() {
    let limits = StoreLimits::default();
    for (name, value) in [
        ("max_tree_depth", limits.max_tree_depth),
        ("max_node_count", limits.max_node_count),
        ("max_string_length", limits.max_string_length),
        ("max_value_depth", limits.max_value_depth),
        ("max_list_elements", limits.max_list_elements),
        ("max_record_properties", limits.max_record_properties),
        (
            "max_transaction_operations",
            limits.max_transaction_operations,
        ),
        ("max_model_count", limits.max_model_count),
        (
            "max_cached_items_per_model",
            limits.max_cached_items_per_model,
        ),
        (
            "max_items_per_model_operation",
            limits.max_items_per_model_operation,
        ),
    ] {
        assert!(value > 0, "{name} must be positive");
        assert!(
            value < usize::MAX,
            "{name} must be a real ceiling, not effectively unbounded (§26)"
        );
    }
}
