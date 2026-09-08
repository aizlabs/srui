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
cargo test -p srui-semantic-tree --test conformance_semantic_input_test
```

**Swift**

```bash
swift test --package-path client-macos --filter SemanticInputConformanceTests
```

## Documented gaps

- **The positive half of §32.5 — a coordinate event ACCEPTED for an explicitly subscribed custom scene node — cannot be exercised.**
  - *Why:* POINTER_* events are registered in namespace 0, but no VectorScene node type, subscription model, or server-side 'coordinate event only for a subscribed scene' validation exists on this base.
  - *Owner:* Task 36 (VectorScene profile, optional)
