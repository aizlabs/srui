# Suite 5 — Semantic-input tests (§32 item 5)

**Status:** `active`  
**Spec sections:** §7.6, §7.7, §18.3, §32.5

Run this suite alone:

```bash
scripts/run-conformance --suite 5
```

## Fixtures

Code-driven suite: no shared fixtures. This suite asserts behaviour against the registry tables `build.rs` and `generate_swift_registry.py` already generate, rather than introducing a second copy of the registry.

## Runners

**Rust**

```bash
cargo test --manifest-path server-rust/Cargo.toml -p srui-semantic-tree --test conformance_semantic_input_test
```

**Swift**

```bash
swift test --package-path client-macos --filter SemanticInputConformanceTests
swift test --package-path client-macos --filter WidgetSemanticsConformanceTests
```

## Open gaps

This suite reports `GAP`, not `PASS`, while any of these is open. The runner exits non-zero if a probe shows one has been closed without the manifest being updated.

### Event validation performs no event-type/node-type compatibility check: POINTER_* is accepted against an ordinary Button, and ACTIVATE is accepted against non-interactive Text/Progress/Image/Separator nodes.

- **Why:** Event::validate (server-rust/semantic-tree/src/event.rs) checks observed revision, node existence and enabled state, but never compares the event type against the target node type. §32.5's rule that coordinates are refused outside a subscribed scene, and §7.6's per-node event sets, are therefore unenforced. Both are pinned by #[should_panic] tests in conformance_semantic_input_test.rs rather than papered over.
- **Owner:** Task 36 (VectorScene) for the coordinate half; the event-type/node-type check is unowned
- **Closure probe:** `server-rust/semantic-tree/src/event.rs` matching `CoordinateEvent|coordinate event .*(subscrib|scene)|NodeNotSubscribed`
