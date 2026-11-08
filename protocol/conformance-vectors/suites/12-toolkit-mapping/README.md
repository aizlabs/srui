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

## Runners

**Swift**

```bash
swift test --package-path client-macos --filter ControlFactoryTests
swift test --package-path client-macos --filter ShortcutRoutingTests
```

**Rust** — `N/A`: Native toolkit mapping is renderer-side and informative (§22.4); the server has no half.
