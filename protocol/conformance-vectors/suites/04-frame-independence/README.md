# Suite 4 — Frame-independence tests (§32 item 4)

**Status:** `active`  
**Spec sections:** §4.16, §12.2, §23, §32.4

Run this suite alone:

```bash
scripts/run-conformance --suite 4
```

## Fixtures

## Runners

**Rust**

```bash
cargo test -p srui-sessiond --test conformance_frame_independence_test
```

**Swift**

```bash
swift test --package-path client-macos --filter FrameIndependenceConformanceTests
```
