# Suite 8 — Reconnect tests (§32 item 8)

**Status:** `active`  
**Spec sections:** §18, §18.2, §18.3, §21, §32.8

Run this suite alone:

```bash
scripts/run-conformance --suite 8
```

## Fixtures

Code-driven suite: no shared fixtures. This suite asserts behaviour against the registry tables `build.rs` and `generate_swift_registry.py` already generate, rather than introducing a second copy of the registry.

## Runners

**Rust**

```bash
cargo test --manifest-path server-rust/Cargo.toml -p srui-sessiond --test conformance_reconnect_test
cargo test --manifest-path server-rust/Cargo.toml -p srui-sessiond --test text_edit_test
cargo test --manifest-path server-rust/Cargo.toml -p srui-sessiond --test session_resume_test
```

**Swift**

```bash
swift test --package-path client-macos --filter ReconnectConformanceTests
swift test --package-path client-macos --filter SessionResumeContinuityTests
swift test --package-path client-macos --filter EventOutboxTests
swift test --package-path client-macos --filter SessionControllerResyncTests
```
