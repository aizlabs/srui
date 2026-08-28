# SRUI Protocol Wire Schema

Protobuf wire encoding for the SRUI semantic protocol (Design v0.4 §6.5, §13, §15, §16, §18).

## Files

| File | Purpose |
|---|---|
| `srui.proto` | Authoritative Protobuf schema |
| `registry.yaml` | Canonical namespace-0 ID registry (Tasks 0–1) |
| `generate_proto.sh` | Swift code generation script |
| `conformance-vectors/` | Fixed binary golden fixtures for cross-language wire checks |

## Code generation

### Rust (`prost` + `build.rs`)

Rust types are generated at compile time by `server-rust/protocol/build.rs` using
[prost-build](https://github.com/tokio-rs/prost) with a vendored `protoc` binary
(`protoc-bin-vendored`). Generated code lands in `target/` and is **not** committed.

**Why prost + build.rs:** prost is the de-facto Protobuf crate in the Rust ecosystem,
integrates cleanly with Cargo rebuild tracking (`rerun-if-changed`), and keeps generated
code out of version control while guaranteeing every `cargo build` compiles against the
current `srui.proto`.

```bash
cd server-rust
cargo build -p srui-protocol   # triggers codegen
cargo test -p srui-protocol    # runs conformance tests
```

### Swift (committed script + checked-in output)

Swift types live in `client-macos/Protocol/srui.pb.swift`, produced by
`protocol/generate_proto.sh` with `protoc` and `protoc-gen-swift` (SwiftProtobuf).

**Why a script instead of a SwiftPM build plugin:** SwiftPM protobuf plugins add
checkout/build complexity and require every developer CI machine to resolve
`protoc-gen-swift` before `swift build` succeeds. Checking in the generated Swift
file plus a small regen script keeps `swift build` self-contained (only the
SwiftProtobuf runtime dependency) while still making regeneration explicit and
reviewable when `srui.proto` changes.

```bash
./protocol/generate_proto.sh          # regenerate srui.pb.swift
cd client-macos && swift test        # runs conformance tests
```

Regenerate Swift output whenever `srui.proto` changes and commit the diff alongside
the proto edit.

## Golden conformance fixtures

`conformance-vectors/golden_node_record.bin` and `golden_transaction.bin` are
**fixed, committed bytes** — tests decode them but never rewrite them.

Fixtures encode:

- **NodeRecord:** Button #42 (`label="Delete"`, `role=destructive`, `enabled=true`) — §7.1 example
- **Transaction:** revision 104→105 with CREATE_NODE, SET_PROPERTY, and BATCH_PROPERTY_SET ops — §12.1 example

The one-shot authoring utility `server-rust/protocol/src/bin/generate_fixtures.rs`
can reproduce these bytes for review when `SRUI_WRITE_FIXTURES=1` is set; CI and tests
always read the committed files.

## COMMIT semantics

Registry operation `COMMIT` (id 5) is a **logical** §13 operation for validation and
future journal typing. On the wire, atomic commit is expressed by the `Transaction`
envelope (`base_revision` → `new_revision`), not as an `Operation` oneof variant.

Both Rust (`server-rust/protocol/tests/conformance_test.rs`) and Swift
(`client-macos/Tests/SRUITests.swift`) decode the same bytes and assert identical
field values — the first cross-language wire-format conformance check.
