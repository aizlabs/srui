# SRUI Core State-Machine Conformance Vectors

Cross-language conformance test vectors validating the **Protocol Core State Machine** (§32 item 1) and **Semantic-Not-Paint Architectural Invariants** (§32 item 3, §4).

---

## 1. Overview & Purpose

These fixtures encode the formal behavioral invariants of the SRUI distributed state store into declarative, human-readable JSON files. They are designed to be executed against any conforming SRUI client or server implementation (e.g. Rust Core in Task 8, Swift Core in Task 14) without language-specific scaffolding or re-deriving test scenarios.

### Scope of Coverage
- **Atomic Transactions & Revisions (§12.1, §12.2)**: All-or-nothing execution advancing monotonically from `base_revision` to `new_revision = base_revision + 1`.
- **Stable Identity & Lifetime Scoping (§6.2)**: `NodeId` and `ModelId` must never be reused within the same session, even after deletion.
- **Referential Integrity (§6.2, §13)**: Parent-child hierarchy validation, orphan rejection, and late-binding model reference validation.
- **Rollback on Failure (§12.1)**: Speculative execution where any failing operation discards the entire transaction with zero side effects.
- **Safety Limits (§26)**: Enforcement of tree depth, node count, transaction operation count, string length, and collection model bounds.
- **Collection Models (§8, §13)**: Large-scale virtualized data mutation (`CREATE_MODEL`, `MODEL_INSERT`, `MODEL_UPDATE`, `MODEL_RESET_RANGE`, `MODEL_DELETE`).
- **Semantic-Not-Paint (§4.7, §4.16, §4.17, §32.3)**: Standard Widget Profile contains only semantic intent and state, strictly forbidding display frame cadence, paint instructions, and mandatory absolute pixel geometry.

---

## 2. Fixture Format Specification

Each test vector is a single JSON file structured as follows:

```json
{
  "name": "valid_multi_op_transaction",
  "description": "Human-readable summary of the test scenario",
  "spec_sections": ["§4.5", "§6.2", "§12.1", "§13"],
  "initial_limits": {
    "max_tree_depth": 64,
    "max_node_count": 100000,
    "max_transaction_operations": 1000,
    "max_string_length": 1048576,
    "max_value_depth": 16,
    "max_list_length": 10000,
    "max_record_properties": 256,
    "max_model_count": 10000,
    "max_cached_items_per_model": 100000,
    "max_items_per_model_operation": 5000
  },
  "initial_revision": 0,
  "setup_transactions": [
    {
      "base_revision": 0,
      "new_revision": 1,
      "operations": [ ... ]
    }
  ],
  "transaction": {
    "base_revision": 1,
    "new_revision": 2,
    "operations": [ ... ]
  },
  "expected_outcome": {
    "status": "success",
    "committed_revision": 2,
    "store_state": {
      "node_count": 3,
      "roots": [1],
      "nodes": {
        "1": {
          "node_id": 1,
          "node_type": "Surface",
          "parent_id": null,
          "ordered_children": [2],
          "properties": {
            "label": "Main Window"
          }
        }
      },
      "model_count": 0,
      "models": {}
    }
  }
}
```

For rejected transactions:

```json
{
  "expected_outcome": {
    "status": "rejected",
    "error_code": "node_id_already_used",
    "failed_op_index": 0,
    "expected_store_revision": 1,
    "rollback_verified": true
  }
}
```

---

## 3. Operations Reference

All operations conform to §13 standard operations in namespace 0:

| Op Type | Parameters | Description |
|---|---|---|
| `CREATE_NODE` | `node_id`, `node_type`, `parent_id`, `child_index`, `properties` | Creates a new identified node with properties and parent. |
| `DELETE_NODE` | `node_id` | Deletes a node and its entire subtree recursively. |
| `SET_PROPERTY` | `node_id`, `property`, `value` | Sets or updates a property on an active node. |
| `CLEAR_PROPERTY` | `node_id`, `property` | Removes a property from an active node. |
| `BATCH_PROPERTY_SET` | `node_id`, `properties` | Sets multiple properties atomically on a node. |
| `MOVE_NODE` | `node_id`, `new_parent_id`, `new_child_index` | Reparents or reindexes a node in the tree. |
| `REORDER_CHILDREN` | `parent_id`, `new_order` | Permutes the child list of a parent node. |
| `CREATE_MODEL` | `model_id`, `model_type`, `item_count` | Creates an authoritative collection model. |
| `MODEL_INSERT` | `model_id`, `index`, `items` | Inserts items at an index position, shifting existing items. |
| `MODEL_DELETE` | `model_id`, `index`, `count`, `item_ids` | Deletes items by item IDs or by contiguous index range. |
| `MODEL_UPDATE` | `model_id`, `index`, `items` | Updates item values/properties by identity or index. |
| `MODEL_RESET_RANGE` | `model_id`, `start_index`, `items`, `total_count` | Replaces a contiguous cached item range. |

---

## 4. Value Encodings

Properties are encoded as JSON primitives or typed objects:
- **Primitives**: `null`, `true`/`false`, integers (signed/unsigned), floats, strings.
- **Node/Item References**: `{ "node_id": 42 }`, `{ "item_id": 100 }`.
- **Enums**: `{ "enum": "ActionRole", "value": "destructive" }` or `{ "enum_id": 2, "value_id": 3 }`.
- **Semantic Geometry Hints**:
  - `Size`: `{ "width": 120.0, "height": 24.0 }`
  - `Point`: `{ "x": 10.0, "y": 20.0 }`
  - `Rect`: `{ "x": 0.0, "y": 0.0, "width": 100.0, "height": 50.0 }`
  - `Range`: `{ "start": 0, "length": 50 }`
  - `EdgeInsets`: `{ "top": 4.0, "leading": 8.0, "bottom": 4.0, "trailing": 8.0 }`
- **Lists**: Array of values `[ ... ]`.
- **Small Records**: `{ "record_type": "TypeName", "properties": { ... } }`.

---

## 5. Standard Error Codes for Rejections

| Error Code | Store / Txn Error | Specification |
|---|---|---|
| `stale_base_revision` | `TxnError::StaleBaseRevision` | §12.1 |
| `invalid_new_revision` | `TxnError::InvalidNewRevision` | §12.1 |
| `max_operations_exceeded` | `TxnError::MaxOperationsExceeded` | §26 |
| `node_id_already_used` | `StoreError::NodeIdAlreadyUsed` | §6.2 |
| `node_not_found` | `StoreError::NodeNotFound` | §13 |
| `parent_not_found` | `StoreError::ParentNotFound` | §13 |
| `max_node_count_exceeded` | `StoreError::MaxNodeCountExceeded` | §26 |
| `max_tree_depth_exceeded` | `StoreError::MaxTreeDepthExceeded` | §26 |
| `max_string_length_exceeded` | `StoreError::MaxStringLengthExceeded` | §26 |
| `max_model_count_exceeded` | `StoreError::MaxModelCountExceeded` | §26 |
| `max_cached_items_per_model_exceeded` | `StoreError::MaxCachedItemsPerModelExceeded` | §26 |
| `max_items_per_model_operation_exceeded` | `StoreError::MaxItemsPerModelOperationExceeded` | §26 |
| `model_id_already_used` | `StoreError::ModelIdAlreadyUsed` | §6.2, §8 |
| `model_not_found` | `StoreError::ModelNotFound` | §8, §13 |
| `item_not_found` | `StoreError::ItemNotFound` | §8, §13 |
| `model_index_out_of_bounds` | `StoreError::ModelIndexOutOfBounds` | §8, §13 |
| `duplicate_item_id` | `StoreError::DuplicateItemId` | §8, §13 |
| `invalid_model_delete` | `StoreError::InvalidModelDelete` | §8, §13 |
| `cycle_detected` | `StoreError::CycleDetected` | §13 |
| `child_index_out_of_bounds` | `StoreError::ChildIndexOutOfBounds` | §13 |
| `invalid_children_reorder` | `StoreError::InvalidChildrenReorder` | §13 |

---

## 6. The "Semantic-Not-Paint" Invariant (§4.7, §4.16, §4.17, §32.3)

A core tenet of SRUI is that the Protocol Core and Standard Widget Profile synchronize **meaning and state**, never display paint commands or frame render cues:

1. **No Frame Cadence (§4.16, §12.2)**:
   - Wire transactions and store commits define *state-consistency boundaries*, not display frames.
   - The protocol contains no `START_FRAME`, `END_FRAME`, frame sequence numbers, or server-driven refresh cadences (60Hz, 120Hz). The local renderer independently drives display presentation.

2. **No Paint Commands (§4.7, §7.1)**:
   - Standard widgets describe role, content, and state (e.g. `Button` with `label="Delete"`, `role=destructive`, `enabled=true`).
   - The protocol strictly forbids remote rasterization commands (`draw_rect`, `fill_path`, `set_pixel`, `draw_text_at_coordinate`). Exact drawing is isolated exclusively to retained `VectorScene` extensions.

3. **No Mandatory Pixel Geometry (§4.17, §10)**:
   - Layout is computed locally from semantic relationships and intent (`Row`, `Column`, `Grid`, `horizontal_alignment`, `importance`, `spacing_role`).
   - Standard controls do not require server-specified absolute coordinates (`x_px`, `y_px`, `width_px`, `height_px`).

See fixture `11_semantic_not_paint_widget_properties.json` for a concrete realization of this invariant.
