# Suite 3 — Semantic-not-paint tests (§32 item 3)

**Status:** `active`  
**Spec sections:** §4.7, §4.16, §4.17, §7.1, §10, §32.3

Run this suite alone:

```bash
scripts/run-conformance --suite 3
```

## Fixtures

`suites/03-semantic-not-paint/vectors/` — exactly **1** JSON vector(s), count-pinned by [`../manifest.json`](../manifest.json). Adding or removing one without updating the manifest fails the Rust loader, the Swift loader and `protocol/tests/test_conformance_manifest.py`.

## Runners

**Rust**

```bash
cargo test --manifest-path server-rust/Cargo.toml -p srui-semantic-tree --test conformance_semantic_not_paint_test
```

**Swift**

```bash
swift test --package-path client-macos --filter SemanticNotPaintConformanceTests
```
