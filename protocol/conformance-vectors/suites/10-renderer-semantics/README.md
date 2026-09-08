# Suite 10 — Renderer semantic tests (§32 item 10)

**Status:** `active`  
**Spec sections:** §22, §23, §24, §32.10

Run this suite alone:

```bash
scripts/run-conformance --suite 10
```

## Fixtures

Code-driven suite: no shared vectors. The runners are listed in [`../manifest.json`](../manifest.json).

## Runners

**Swift**

```bash
swift test --package-path client-macos --filter AppKitRendererTests
swift test --package-path client-macos --filter ControlFactoryTests
swift test --package-path client-macos --filter ControlFactoryPropertyTests
swift test --package-path client-macos --filter CollectionAdaptersTests
swift test --package-path client-macos --filter NativeTextEditorAdapterTests
```

**Rust** — not applicable: Renderer semantics are AppKit-only; RendererAppKit cannot build on Linux.
