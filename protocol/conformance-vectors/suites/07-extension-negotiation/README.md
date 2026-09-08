# Suite 7 — Extension-negotiation tests (§32 item 7)

**Status:** `active`  
**Spec sections:** §5, §17, §22.4, §32.7

Run this suite alone:

```bash
scripts/run-conformance --suite 7
```

## Fixtures

Code-driven suite: no shared vectors. The runners are listed in [`../manifest.json`](../manifest.json).

## Runners

**Rust**

```bash
cargo test --manifest-path server-rust/Cargo.toml -p srui-semantic-tree --test capability_test
```

**Swift**

```bash
swift test --package-path client-macos --filter CapabilityTests
swift test --package-path client-macos --filter HandshakeNegotiationTests
```

## Documented gaps

The runner reports this suite as `GAP` rather than `PASS` while any of these remain open, and exits non-zero if a probe shows one has been closed without the manifest being updated.

### Extension fallback subtrees — an un-negotiated extension node degrading to a declared Standard Widget Profile fallback subtree rather than failing.

- **Why:** Capability negotiation and must-understand rejection are implemented and covered; the fallback-subtree half of §32.7 is Task 31 work that has not been merged to origin/main.
- **Owner:** Task 31 merge (branch codex/task-31-coding-agent)
- **Closure probe:** `server-rust/semantic-tree/src/capability.rs` matching `fallback_subtree|must_understand|FallbackSubtree`
