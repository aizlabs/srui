# Suite 6 — Local text-interaction tests (§32 item 6)

**Status:** `active`  
**Spec sections:** §9, §18.3, §22.6, §32.6

Run this suite alone:

```bash
scripts/run-conformance --suite 6
```

## Fixtures

Code-driven suite: no shared fixtures. This suite asserts behaviour against the registry tables `build.rs` and `generate_swift_registry.py` already generate, rather than introducing a second copy of the registry.

Shortcuts (PX-016-G01): a bare printable key is refused by both stores (`shortcut_test`, suite 1 vector 57), no shortcut dispatches while the first responder has marked text, letters typed into a TextInput never trigger one (`ShortcutRoutingTests`), and Cmd-C in a TextInput copies through the generic shell's Edit menu without the Surface claiming it (`ApplicationMenuTests`).

## Runners

**Rust**

```bash
cargo test --manifest-path server-rust/Cargo.toml -p srui-sessiond --test text_edit_test
cargo test --manifest-path server-rust/Cargo.toml -p srui-semantic-tree --test shortcut_test
```

**Swift**

```bash
swift test --package-path client-macos --filter TextEditingSessionTests
swift test --package-path client-macos --filter TextEditingSessionDebounceTests
swift test --package-path client-macos --filter TextEditingIntegrationTests
swift test --package-path client-macos --filter NativeTextEditorAdapterTests
swift test --package-path client-macos --filter ShortcutRoutingTests
swift test --package-path client-macos --filter ApplicationMenuTests
```
