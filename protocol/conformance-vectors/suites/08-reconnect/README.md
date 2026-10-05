# Suite 8 — Reconnect tests (§32 item 8)

**Status:** `active`  
**Spec sections:** §12.1, §18, §18.2, §18.3, §21, §26, §32.8

Run this suite alone:

```bash
scripts/run-conformance --suite 8
```

## Fixtures

`vectors/` holds the shared multi-envelope snapshot delivery vectors (count pinned in
`manifest.json`). A snapshot is still one `0 -> snapshot_revision` transaction applied atomically;
when its single envelope would exceed the §26 frame limit, the continuity decision
(`SERVER WELCOME` / `SERVER RESYNC_REQUIRED`) announces `snapshot_parts`, the client bounds it with
`ClientLimits.max_snapshot_parts`, and the replica stages the envelopes and applies them only after
the last one. Each vector gives:

- `client_limits` — the client's `max_snapshot_parts` (0 = legacy single envelope) and
  `max_transaction_operations`;
- `decision` — the announced `snapshot_revision` and `snapshot_parts` (0 = one envelope);
- `envelopes` — the `Transaction` envelopes that followed, with `create_node`, `create_model` and
  `model_reset_range` operations in standard-namespace numeric IDs and string values;
- `expected_outcome` — `applied` with the exact reconstructed store, `rejected` with the error code
  (`parts_exceed_limit`, `not_a_snapshot_part`, `empty_part`, `operation_limit_exceeded`) and the
  envelope index (`null` when refused at the decision), or `incomplete` when the connection ends
  first. Nothing reaches the replica unless the outcome is `applied`.

The Rust and Swift replicas replay the same files. Wire encodings of the new fields are pinned by
the `golden_snapshot_parts_*` and `golden_handshake_refused` vectors in `expected.json`. The rest of
the suite is code-driven.

## Runners

**Rust**

```bash
cargo test --manifest-path server-rust/Cargo.toml -p srui-sessiond --test conformance_reconnect_test
cargo test --manifest-path server-rust/Cargo.toml -p srui-sessiond --test session_resume_test
cargo test --manifest-path server-rust/Cargo.toml -p srui-sessiond --test session_resync_test
cargo test --manifest-path server-rust/Cargo.toml -p srui-sessiond --test text_edit_test
cargo test --manifest-path server-rust/Cargo.toml -p srui-sessiond --test event_delivery_test
cargo test --manifest-path server-rust/Cargo.toml -p srui-semantic-tree --test conformance_snapshot_framing_test
cargo test --manifest-path server-rust/Cargo.toml -p srui-sessiond --test snapshot_framing_test
```

**Swift**

```bash
swift test --package-path client-macos --filter ReconnectConformanceTests
swift test --package-path client-macos --filter SessionResumeContinuityTests
swift test --package-path client-macos --filter EventOutboxTests
swift test --package-path client-macos --filter SessionControllerResyncTests
swift test --package-path client-macos --filter SnapshotFramingConformanceTests
```
