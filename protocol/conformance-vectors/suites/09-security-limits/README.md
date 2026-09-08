# Suite 9 — Security limits (§32 item 9)

**Status:** `active`  
**Spec sections:** §26, §20.1, §27, §32.9

Run this suite alone:

```bash
scripts/run-conformance --suite 9
```

## Fixtures

## Runners

**Rust**

```bash
cargo test -p srui-sessiond --test security_limits_test
cargo test -p srui-protocol --test framing_decoder_matrix_test
```

**Swift**

```bash
swift test --package-path client-macos --filter FramingDecoderMatrixTests
swift test --package-path client-macos --filter DecoderTests
swift test --package-path client-macos --filter SSHTransportPostureTests
swift test --package-path client-macos --filter TransactionRateLimiterTests
```
