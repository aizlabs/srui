# Suite 3 — Semantic-not-paint tests (§32 item 3)

**Status:** `active`  
**Spec sections:** §4.7, §4.16, §4.17, §7.1, §10, §32.3

Run this suite alone:

```bash
scripts/run-conformance --suite 3
```

## Fixtures

`suites/03-semantic-not-paint/vectors/` — exactly **2** JSON vector(s), count-pinned by [`../manifest.json`](../manifest.json). Adding or removing one without updating the manifest fails the Rust loader, the Swift loader and `protocol/tests/test_conformance_manifest.py`.

Split (PX-070-G02): `12_split_panes_without_geometry.json` builds a stacked `Split` of host panels, a process table and a collapsed inspector from the axis, size hints, `grow` and `visibility` alone. No fixture field and no standard property names a pixel position, a frame or a divider position (`test_semantic_not_paint_architectural_invariants`, `testSplitCarriesNoDividerPositionOrPaneGeometry`): divider positions are local interaction state (§7.6, §22.5).

## Runners

**Rust**

```bash
cargo test --manifest-path server-rust/Cargo.toml -p srui-semantic-tree --test conformance_semantic_not_paint_test
```

**Swift**

```bash
swift test --package-path client-macos --filter SemanticNotPaintConformanceTests
```
