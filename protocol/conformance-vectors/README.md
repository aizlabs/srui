# Conformance Vectors

Fixed binary wire fixtures for cross-language Protobuf conformance checks.

| File | Encodes |
|---|---|
| `golden_node_record.bin` | A `NodeRecord` (Button #42, §7.1 example) |
| `golden_transaction.bin` | A `Transaction` with three operations (§12.1 example) |

These bytes are committed artifacts — tests decode them but never regenerate them.
See `protocol/README.md` for codegen and fixture authoring details.
