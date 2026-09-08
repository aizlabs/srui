# Suite 6 — Local text-interaction tests (§32 item 6)

**Status:** `active`  
**Spec sections:** §9, §18.3, §22.6, §32.6

Run this suite alone:

```bash
scripts/run-conformance --suite 6
```

## Fixtures

Code-driven suite: no shared vectors. The runners are listed in [`../manifest.json`](../manifest.json).

## Runners

**Rust**

```bash
cargo test --manifest-path server-rust/Cargo.toml -p srui-sessiond --test text_edit_test
```

**Swift**

```bash
swift test --package-path client-macos --filter TextEditingSessionTests
swift test --package-path client-macos --filter TextEditingSessionDebounceTests
swift test --package-path client-macos --filter TextEditingIntegrationTests
swift test --package-path client-macos --filter NativeTextEditorAdapterTests
```
