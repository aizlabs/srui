# Suite 5 — Semantic-input tests (§32 item 5)

**Status:** `active`  
**Spec sections:** §7.6, §7.7, §18.3, §32.5

Run this suite alone:

```bash
scripts/run-conformance --suite 5
```

## Fixtures

`events.generated.json` — **generated** from `protocol/registry.yaml` by `protocol/generate_conformance_matrix.py`. Do not edit by hand; run `./protocol/generate_proto.sh` and commit the result. CI fails on any diff.

## Runners

**Rust**

```bash
cargo test --manifest-path server-rust/Cargo.toml -p srui-semantic-tree --test conformance_semantic_input_test
```

**Swift**

```bash
swift test --package-path client-macos --filter SemanticInputConformanceTests
```

## Documented gaps

The runner reports this suite as `GAP` rather than `PASS` while any of these remain open, and exits non-zero if a probe shows one has been closed without the manifest being updated.

### Server event validation accepts POINTER_* events targeting ordinary Standard Widget nodes; the §32.5 rule that coordinates are refused outside a subscribed scene is unenforced.

- **Why:** Event::validate (server-rust/semantic-tree/src/event.rs) checks observed revision, node existence, enabled/read-only state and TEXT_EDIT edit_seq, but never the event kind against the target node type. A POINTER_DOWN aimed at a Button validates successfully.
- **Owner:** Task 36 (VectorScene profile) — the rule needs the subscription model to state what coordinates are legal for
- **Closure probe:** `server-rust/semantic-tree/src/event.rs` matching `CoordinateEvent|coordinate event .*(subscrib|scene)|NodeNotSubscribed`

### The positive half of §32.5 — a coordinate event ACCEPTED for an explicitly subscribed custom scene node — cannot be exercised.

- **Why:** POINTER_* events are registered in namespace 0, but no VectorScene node type or subscription model exists on this base.
- **Owner:** Task 36 (VectorScene profile, optional)
- **Closure probe:** `protocol/registry.yaml` matching `name: VectorScene`
