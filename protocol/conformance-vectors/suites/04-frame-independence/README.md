# Suite 4 — Frame-independence tests (§32 item 4)

**Status:** `active`  
**Spec sections:** §4.16, §12.2, §23, §32.4

Run this suite alone:

```bash
scripts/run-conformance --suite 4
```

## Fixtures

Code-driven suite: no shared fixtures. This suite asserts behaviour against the registry tables `build.rs` and `generate_swift_registry.py` already generate, rather than introducing a second copy of the registry.

## Runners

**Rust**

```bash
cargo test --manifest-path server-rust/Cargo.toml -p srui-sessiond --test conformance_frame_independence_test
```

**Swift**

```bash
swift test --package-path client-macos --filter FrameIndependenceConformanceTests
```
