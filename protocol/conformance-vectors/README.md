# Conformance Vectors

Cross-language conformance test vectors for the SRUI protocol suite (§32).

## 1. Binary Wire Fixtures (Protobuf & Framing)

Fixed binary wire fixtures for cross-language Protobuf conformance checks (§16, §19, §32).

| File | Encodes |
|---|---|
| `golden_node_record.bin` | A `NodeRecord` (Button #42, §7.1 example) |
| `golden_transaction.bin` | A `Transaction` with three operations (§12.1 example) |
| `golden_framed_message.bin` | Length-prefixed framed `SruiMessage` containing a transaction |
| `golden_event_ack.bin` | Length-prefixed framed `SruiMessage` containing a `ServerEventAck` (§18.2) |
| `golden_client_model_range_request.bin` | Length-prefixed framed `SruiMessage` containing a `ClientModelRangeRequest` (§8, §22.7) |
| `golden_text_edit_event.bin` | Length-prefixed framed `SruiMessage` containing a valid whole-value `TEXT_EDIT` with positive `edit_seq` (§18.3, §22.6) |
| `malformed_overlong_varint.bin` | Truncated/overlong varint rejection check |
| `malformed_truncated_frame.bin` | Truncated length-prefixed frame rejection check |
| `malformed_text_edit_zero_edit_seq.bin` | Protobuf-valid `TEXT_EDIT` rejected during semantic conversion because `edit_seq == 0` |
| `malformed_activate_nonzero_edit_seq.bin` | Protobuf-valid non-text event rejected during semantic conversion because `edit_seq != 0` |

See `expected.json` for canonical hex and field declarations.

## 2. Core State-Machine Fixtures (`state-machine/`)

Human-readable declarative JSON test vectors validating the **Protocol Core State Machine** (§32 item 1) and **Semantic-Not-Paint Invariants** (§32 item 3, §4).

See [`state-machine/README.md`](state-machine/README.md) for format specifications, error codes, and schema rules.

| Fixture | Scenario / Architectural Invariant |
|---|---|
| `01_valid_multi_op_transaction.json` | Valid atomic multi-op transaction (rev 0 -> 1) |
| `02_reused_node_id_rejected.json` | Reused session-scoped `NodeId` rejection (§6.2) |
| `03_orphan_parent_rejected.json` | Non-existent parent reference rejection (§13) |
| `04_mid_transaction_rollback.json` | Intermediate op failure and complete transaction rollback (§12.1) |
| `05_stale_base_revision_rejected.json` | Stale / mismatched `base_revision` rejection (§12.1) |
| `06_exceed_max_node_count_rejected.json` | `max_node_count` limit enforcement (§26) |
| `07_exceed_max_tree_depth_rejected.json` | `max_tree_depth` limit enforcement (§26) |
| `08_exceed_max_operations_rejected.json` | `max_transaction_operations` pre-check limit (§26) |
| `09_valid_model_operations.json` | Collection model operations (`CREATE_MODEL`, `INSERT`, `UPDATE`, `RESET`, `DELETE`) (§8, §13) |
| `10_invalid_model_item_not_found_rejected.json` | Model update referencing non-existent `item_id` rejection (§8, §13) |
| `11_semantic_not_paint_widget_properties.json` | Semantic intent vs paint commands / frame cadence (§4.7, §4.16, §4.17, §32.3) |
| `12_reused_deleted_node_id_rejected.json` | `NodeId` cannot be reused even after deletion (§6.2) |
| `13_sequential_transactions.json` | Monotonic sequential revisions (0 -> 1 -> 2 -> 3) (§12.1) |
| `14_move_node_cycle_prevention.json` | Moving ancestor under descendant cycle prevention (§13) |
| `15_invalid_new_revision_rejected.json` | `new_revision != base_revision + 1` rejection (§12.1) |
