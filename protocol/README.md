# SRUI Protocol Wire Schema

Protobuf wire encoding and code generation for the SRUI semantic protocol (Design Doc v0.6 §6.5, §13, §15, §16, §18).

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

---

## Native text editing (§18.3, §22.6)

`TEXT_EDIT` travels on the existing `Event` transport. Immediate glyph, caret, selection, IME,
clipboard, and spellcheck feedback stay in the platform text system; SRUI does not add a
spellcheck protocol.

**Whole-value encoding.** Each committed local edit carries the current string in
`arguments[TEXT]`. There is no delta encoding in v0.1.

**`edit_seq`.** A positive `Event.edit_seq` is required on `TEXT_EDIT` and MUST be zero on every
other event type. The sequence is monotonic per `(session_id, client_instance_id, node_id)`. The
client increments it for every committed local edit; coalescing may skip values. Global
`event_seq` is allocated only when a coalesced edit enters the outbox, so `event_seq` stays
contiguous even when `edit_seq` has gaps. `edit_seq == 0` is absent on the wire.

**Resume.** `ClientResume.pending_text_edits` lists every assigned, unacknowledged `TEXT_EDIT`
(`event_id`, `event_seq`, `node_id`, `edit_seq`). `SERVER RESUME_OK` ignores that list: the client
replays assigned events byte-for-byte, then promotes the newest coalesced unsent draft.

**Forced same-session resync.** Before capturing the snapshot, the server settles each declared
text-event identity in the event-deduplication window (so removing them cannot open a global
`event_seq` gap), records discard watermarks, and echoes the exact refs in
`ServerResyncRequired.discarded_text_edits`. The client verifies the echo, selectively removes
those text events, marks their global sequences settled, discards unsent drafts, replays only
ordinary pending events, then applies the snapshot. A missing or mismatched echo fails closed.
Replacement resync (`SESSION_CONTINUITY_REPLACED`) abandons every old event and text-edit
sequence; both sequence spaces restart with the new incarnation.

---

## Terminal compatibility profile (§21, §21.2)

`org.srui.terminal/1` is a negotiated extension profile, not a Namespace 0 widget.
Local type ID `1` inside the session-assigned namespace is `Terminal`. The stream
ID equals that node's `NodeId`. Clients MUST read the numeric namespace from
`ServerWelcome.extension_namespaces` and MUST NOT assume it is `1`.

| Envelope | Direction | Notes |
|---|---|---|
| `TerminalData` | Server → client | `byte_offset` is the absolute offset of `data[0]`. The resumable next offset is `byte_offset + data.size()` with checked `uint64` arithmetic. Empty frames are forbidden. |
| `TerminalInput` | Client → server | Raw PTY bytes. Bounded by 65536 bytes. |
| `TerminalResize` | Client → server | `columns`/`rows` in `[1, 512]`; pixel dimensions in `[0, 16384]`. |
| `TerminalResyncRequired` | Server → client | `resume_at_offset` is where subsequent live data begins. Reasons distinguish retention loss, an offset ahead of the server, and a connected subscriber falling behind. |

`ClientResume.terminal_stream_offsets` is independent of `pending_text_edits` and
is capped at 256 entries. `TerminalResyncRequired` must not enter the semantic
`ServerResyncRequired` path. Wrong-direction terminal messages are protocol
errors. Terminal envelopes are legal only after successful terminal-profile
negotiation.

A session that emits a Terminal node marks `org.srui.terminal/1` required because
v1 has no semantic fallback. Direct PTYs survive SRUI network detachment, but
there is no automatic `tmux` redraw after the replay ring is lost.

---

## Event Settlement (§18.2)

`ServerEventAck.session_id` is **required and non-empty** on every acknowledgement. It names the
incarnation that settled the event and is what lets a client refuse an ack minted by an expired
incarnation or by a connection that is still draining. An ack that omits it can never retire an
intent, so the event would stay pending forever — replayed on every retry, answered `DUPLICATE`,
never settled — until the contiguous send window is exhausted. A client that receives one fails the
session explicitly rather than degrading silently (§4 inv. 13).
