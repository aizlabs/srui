# Suite 8 — Reconnect tests (§32 item 8)

**Status:** `active`  
**Spec sections:** §18, §18.2, §18.3, §21, §32.8

Run this suite alone:

```bash
scripts/run-conformance --suite 8
```

## Fixtures

Code-driven suite: no shared vectors. The runners are listed in [`../manifest.json`](../manifest.json).

## Runners

**Rust**

```bash
cargo test -p srui-sessiond --test conformance_reconnect_test
```

**Swift**

```bash
swift test --package-path client-macos --filter ReconnectConformanceTests
```
