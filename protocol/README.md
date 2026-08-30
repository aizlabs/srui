# SRUI Protocol Wire Schema

Protobuf wire encoding and code generation for the SRUI semantic protocol (Design Doc v0.4 §6.5, §13, §15, §16, §18).

---

## Files & Layout

| File / Directory | Purpose |
|---|---|
| [`srui.proto`](srui.proto) | Authoritative Protobuf wire schema |
| [`registry.yaml`](registry.yaml) | Canonical namespace-0 ID registry (monotonically assigned IDs) |
| [`generate_proto.sh`](generate_proto.sh) | Portable cross-platform code generation script |
| [`conformance-vectors/expected.json`](conformance-vectors/expected.json) | Canonical JSON specification for golden binary fixtures and expected field values |
| [`conformance-vectors/*.bin`](conformance-vectors/) | Fixed binary golden fixtures for cross-language wire checks |

---

## Dual Codegen Architecture & Unified Developer Workflow

### What to run when `srui.proto` or `registry.yaml` changes:

Run the single top-level generation and validation script:

```bash
# 1. Regenerate Swift code
./protocol/generate_proto.sh

# 2. Run full triple-oracle validation and test suite
uv run python protocol/validate_registry.py
cargo test --manifest-path server-rust/Cargo.toml
swift test --package-path client-macos
```

### Why this dual strategy was chosen:

- **Rust (`prost` + `build.rs`)**:
  Rust types are generated automatically at build time in `server-rust/protocol/build.rs` using `prost-build` with `protoc-bin-vendored`.
  *Rationale*: Self-contained, zero external build dependencies, integrated with `cargo:rerun-if-changed`, keeps generated code out of git.

- **Swift (`SwiftProtobuf` + committed `srui.pb.swift`)**:
  Swift types live in `client-macos/Protocol/srui.pb.swift` and are compiled via `SwiftProtobuf`.
  *Rationale*: Keeps `swift build` and `swift test` self-contained without requiring host-level `protoc` or SwiftPM plugin sandboxing issues in diverse IDE and CI environments.

- **CI Freshness Guard**:
  CI automatically verifies that committed `srui.pb.swift` is fresh and in sync with `srui.proto` via `./protocol/generate_proto.sh && git diff --exit-code client-macos/Protocol/srui.pb.swift`.

---

## Canonical Conformance Vectors (`expected.json`)

To prevent assertion drift across languages, [`protocol/conformance-vectors/expected.json`](conformance-vectors/expected.json) is the single source of truth for:
- Binary file names, SHA-256 hashes, exact hex bytes, and byte lengths.
- Expected decoded structure and field values.

Tests in **Rust** (`server-rust/protocol/tests/conformance_test.rs`), **Swift** (`client-macos/Tests/SRUITests.swift`), and **Python** (`protocol/tests/test_validate_registry.py`) all read `expected.json` and assert:
1. **Decode Conformance**: Decoded messages match `expected.json` field-for-field.
2. **Encode Conformance**: Messages constructed from scratch in Rust and Swift serialize to bit-for-bit identical binary bytes matching `expected.json["hex"]`.
3. **Roundtrip Re-encode**: Decoded messages re-encode to the exact golden fixture bytes.

---

## Standard Registry & Operation Wire Tags

- **Standard Enums**: `StandardEnum` type IDs $1 \ldots 12$ match `registry.yaml` enums $1 \ldots 12$. On the wire, `EnumValue` carries `(enum_id, value_id)`.
- **Operations & Commit**: `Operation` oneof field tags $1 \ldots 13$ map 1:1 with numeric operation IDs in `StandardOperation` and `registry.yaml`, including `CommitOp commit = 5`.
- **Transactions**: The `Transaction` envelope (`base_revision` → `new_revision`) defines atomic commit boundaries.

---

## Resume Responses and Session Continuity (§18)

`CLIENT RESUME` is answered by exactly one of two messages, and the client may not replay pending
events or allocate new ones until one of them arrives.

| Response | Meaning | Client obligation |
|---|---|---|
| `ServerResumeOk{session_id, replay_from_revision, last_processed_event_seq}` | The exact requested incarnation survived and its journal still covers the gap. | Apply the frontier, then replay remaining pending events with their original `event_id`/`event_seq` before any new event. |
| `ServerResyncRequired{session_id, snapshot_revision, reason, continuity, last_processed_event_seq}` | A full snapshot is required. | Depends on `continuity`. |

`SessionContinuity` is carried **only on `ServerResyncRequired`**: `SERVER RESUME_OK` is
`SAME_SESSION` by construction, and its `session_id` MUST exactly equal the requested one, so a
separate field there would be redundant state a peer could contradict. Both responses report
`last_processed_event_seq`, the server's contiguous settled frontier for the bound client.

- `SESSION_CONTINUITY_SAME_SESSION` — the incarnation survived but the journal no longer covers
  the gap; `session_id` still equals the requested one. The client applies the frontier, may
  replay remaining pending events, discards its replica, and applies the snapshot.
- `SESSION_CONTINUITY_REPLACED` — the requested incarnation is gone; `session_id` is the
  replacement token. The client abandons every unresolved event and pending text edit without
  replaying any of them, resets its outbox to the reported frontier, and applies the snapshot.
- `SESSION_CONTINUITY_UNSPECIFIED` (or any unrecognized value) is a required-semantics failure
  (§4 inv. 13): the client fails the session rather than assuming either outcome.
