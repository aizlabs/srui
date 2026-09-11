# Suite 10 — Renderer semantic tests (§32 item 10)

**Status:** `active`  
**Spec sections:** §22, §23, §24, §32.10

Run this suite alone:

```bash
scripts/run-conformance --suite 10
```

## Fixtures

Code-driven suite: no shared fixtures. This suite asserts behaviour against the registry tables `build.rs` and `generate_swift_registry.py` already generate, rather than introducing a second copy of the registry.

## Runners

**Swift**

```bash
swift test --package-path client-macos --filter AppKitRendererTests
swift test --package-path client-macos --filter ControlFactoryTests
swift test --package-path client-macos --filter ControlFactoryPropertyTests
swift test --package-path client-macos --filter CollectionAdaptersTests
swift test --package-path client-macos --filter NativeTextEditorAdapterTests
```

**Rust** — `N/A`: Renderer semantics are AppKit-only; RendererAppKit cannot build on Linux.
