# Suite 10 — Renderer semantic tests (§32 item 10)

**Status:** `active`  
**Spec sections:** §22, §23, §24, §32.10

Run this suite alone:

```bash
scripts/run-conformance --suite 10
```

## Fixtures

Code-driven suite: no shared fixtures. This suite asserts behaviour against the registry tables `build.rs` and `generate_swift_registry.py` already generate, rather than introducing a second copy of the registry.

Shortcuts (PX-016-G01): `ShortcutRoutingTests` asserts the native binding of every mounted shortcut, SET and CLEAR as in-place updates (same `NSButton`, tooltip and accessibility help rewritten), a reserved combination left unbound with a `ShortcutDiagnostic`, a duplicate refused by the store before the renderer sees it, and the Surface window's Tab order after a remount.

Hierarchical Tree (PX-018-G01): `HierarchicalTreeTests` asserts native outline levels from cached `tree_depth` with one reused cell per visible row, local collapse/expand keyed by `ItemId`, placeholders and asynchronous flat-index range requests for uncached visible rows after a collapsed span, promotion reveal through collapsed ancestors only, collapsed-set preservation across model updates and remounts, a 255-deep tree at the default limit (256 refused by the store, previous rows kept), and Tree columns with a visible header.

## Runners

**Swift**

```bash
swift test --package-path client-macos --filter AppKitRendererTests
swift test --package-path client-macos --filter ControlFactoryTests
swift test --package-path client-macos --filter ControlFactoryPropertyTests
swift test --package-path client-macos --filter CollectionAdaptersTests
swift test --package-path client-macos --filter NativeTextEditorAdapterTests
swift test --package-path client-macos --filter ShortcutRoutingTests
swift test --package-path client-macos --filter HierarchicalTreeTests
```

**Rust** — `N/A`: Renderer semantics are AppKit-only; RendererAppKit cannot build on Linux.
