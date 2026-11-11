# Suite 12 — Toolkit mapping tests (§32 item 12)

**Status:** `active`  
**Spec sections:** §22.4, §32.12

Run this suite alone:

```bash
scripts/run-conformance --suite 12
```

## Fixtures

Code-driven suite: no shared fixtures. This suite asserts behaviour against the registry tables `build.rs` and `generate_swift_registry.py` already generate, rather than introducing a second copy of the registry.

Shortcuts (PX-016-G01): `ShortcutRoutingTests` maps every grammar class to `keyEquivalent`, modifier mask, key code and macOS display text (`⌥⇧⌘R`, `⌘↩`, `F6`), and checks every expressible macOS reserved combination stays unbound.

Hierarchical Tree (PX-018-G01): a model-backed `Tree` maps to `NSOutlineView` with a flat data source over the preorder; `tree_depth` becomes the indentation level of the outline column cell, a cached item with descendants gets a disclosure button, and `columns` maps to `NSTableColumn`s exactly as for `Table`, with the first declared column as the outline column and a visible header. The informative positive model fixtures are suite 1 vectors 70–75 (nested tree; insert, delete and reset inside a subtree).

## Runners

**Swift**

```bash
swift test --package-path client-macos --filter ControlFactoryTests
swift test --package-path client-macos --filter ShortcutRoutingTests
swift test --package-path client-macos --filter HierarchicalTreeTests
```

**Rust** — `N/A`: Native toolkit mapping is renderer-side and informative (§22.4); the server has no half.
