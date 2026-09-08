# Suite 9 — Security limits (§32 item 9)

**Status:** `active`  
**Spec sections:** §26, §20.1, §27, §32.9

Run this suite alone:

```bash
scripts/run-conformance --suite 9
```

## Fixtures

Code-driven suite: no shared vectors. The runners are listed in [`../manifest.json`](../manifest.json).

## Runners

**Rust**

```bash
cargo test --manifest-path server-rust/Cargo.toml -p srui-sessiond --test security_limits_test
cargo test --manifest-path server-rust/Cargo.toml -p srui-protocol --test framing_decoder_matrix_test
cargo test --manifest-path server-rust/Cargo.toml -p srui-semantic-tree --test conformance_security_limits_test
```

**Swift**

```bash
swift test --package-path client-macos --filter FramingDecoderMatrixTests
swift test --package-path client-macos --filter DecoderTests
swift test --package-path client-macos --filter SSHTransportPostureTests
swift test --package-path client-macos --filter TransactionRateLimiterTests
```
