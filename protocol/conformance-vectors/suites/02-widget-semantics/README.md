# Suite 2 — Widget semantic tests (§32 item 2)

**Status:** `active`  
**Spec sections:** §7.2, §7.3, §7.6, §32.2

Run this suite alone:

```bash
scripts/run-conformance --suite 2
```

## Fixtures

`widgets.generated.json` — **generated** from `protocol/registry.yaml` by `protocol/generate_conformance_matrix.py`. Do not edit by hand; run `./protocol/generate_proto.sh` and commit the result. CI fails on any diff.

## Runners

**Rust**

```bash
cargo test --manifest-path server-rust/Cargo.toml -p srui-semantic-tree --test conformance_widget_semantics_test
```

**Swift**

```bash
swift test --package-path client-macos --filter WidgetSemanticsConformanceTests
```

## Documented gaps

The runner reports this suite as `GAP` rather than `PASS` while any of these remain open, and exits non-zero if a probe shows one has been closed without the manifest being updated.

### Tree EXPANSION_CHANGED and Surface VIEWPORT_CHANGED are declared in §7.6 but the renderer has no interaction path that can originate them.

- **Why:** RendererAppKit/SemanticInteraction.swift models only activate, valueChanged, selectionChanged and textEdit. A required-tier widget can therefore never emit the disclosure or viewport events the registry declares for it.
- **Owner:** unowned — needs a SemanticInteraction case plus the AppKit outline/window wiring
- **Closure probe:** `client-macos/RendererAppKit/SemanticInteraction.swift` matching `case +(expansionChanged|viewportChanged)`
