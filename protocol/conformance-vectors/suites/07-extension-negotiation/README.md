# Suite 7 — Extension-negotiation tests (§32 item 7)

**Status:** `active`  
**Spec sections:** §5, §17, §22.4, §32.7

Run this suite alone:

```bash
scripts/run-conformance --suite 7
```

## Fixtures

Code-driven suite: no shared fixtures. This suite asserts behaviour against the registry tables `build.rs` and `generate_swift_registry.py` already generate, rather than introducing a second copy of the registry.

## Runners

**Rust**

```bash
cargo test --manifest-path server-rust/Cargo.toml -p srui-semantic-tree --test capability_test
```

**Swift**

```bash
swift test --package-path client-macos --filter CapabilityTests
swift test --package-path client-macos --filter HandshakeNegotiationTests
swift test --package-path client-macos --filter ControlFactoryTests
swift test --package-path client-macos --filter CodingAgentFallbackSocketTests
```
