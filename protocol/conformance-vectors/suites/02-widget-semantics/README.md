# Suite 2 — Widget semantic tests (§32 item 2)

**Status:** `active`  
**Spec sections:** §7.2, §7.3, §7.6, §32.2

Run this suite alone:

```bash
scripts/run-conformance --suite 2
```

## Fixtures

Code-driven suite: no shared fixtures. This suite asserts behaviour against the registry tables `build.rs` and `generate_swift_registry.py` already generate, rather than introducing a second copy of the registry.

Shortcut admission (PX-016-G01): `shortcut_test` and `ShortcutStoreTests` accept the standard `shortcut` property on exactly Button, Toggle, TextInput, TextArea, List, Table, Tree and RichText and refuse it on every other registry node type (`shortcut_not_admitted`); `widgets_test` covers the SDK builder/set/clear on each admitted widget; suite 1 vectors 54–56 replay the same rules in both languages.

## Runners

**Rust**

```bash
cargo test --manifest-path server-rust/Cargo.toml -p srui-sdk --test widgets_test
cargo test --manifest-path server-rust/Cargo.toml -p srui-semantic-tree --test shortcut_test
```

**Swift**

```bash
swift test --package-path client-macos --filter WidgetSemanticsConformanceTests
swift test --package-path client-macos --filter ControlFactoryTests
swift test --package-path client-macos --filter ControlFactoryPropertyTests
swift test --package-path client-macos --filter ShortcutStoreTests
swift test --package-path client-macos --filter ShortcutRoutingTests
```

## Open gaps

This suite reports `GAP`, not `PASS`, while any of these is open. The runner exits non-zero if a probe shows one has been closed without the manifest being updated.

### Tree EXPANSION_CHANGED and Surface VIEWPORT_CHANGED are declared in §7.6 but the renderer has no interaction path that can originate them.

- **Why:** RendererAppKit/SemanticInteraction.swift models only activate, valueChanged, selectionChanged and textEdit. A required-tier widget can therefore never emit the disclosure or viewport events the registry declares for it.
- **Owner:** unowned — needs a SemanticInteraction case plus the AppKit outline/window wiring
- **Closure probe:** `client-macos/RendererAppKit/SemanticInteraction.swift` matching `case +(expansionChanged|viewportChanged)`
