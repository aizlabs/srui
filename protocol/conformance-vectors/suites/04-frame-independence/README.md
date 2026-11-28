# Suite 4 — Frame-independence tests (§32 item 4)

**Status:** `active`  
**Spec sections:** §4.16, §12.2, §23, §32.4

Run this suite alone:

```bash
scripts/run-conformance --suite 4
```

## Fixtures

Code-driven suite: no shared fixtures. This suite asserts behaviour against the registry tables `build.rs` and `generate_swift_registry.py` already generate, rather than introducing a second copy of the registry.

Split (PX-070-G02): moving a `Split` divider is local and changes no protocol traffic. `SplitFrameIndependenceTests` connects the real `SessionController` and `AppKitRenderer` to an in-process server over a `PipeTransport`, captures every frame the client sends after mounting a two-pane `Split` and a `Button`, and runs twice: once leaving the divider alone, once moving it through `NSSplitView` and its accessibility splitter. Neither run sends a frame for the divider, and after one `Button` press (the positive control) the two streams are byte-identical once each event's random `event_id` and `client_instance_id` are blanked. `SplitRenderingTests` adds that a content commit keeps the divider where the user put it, without remounting.

## Runners

**Rust**

```bash
cargo test --manifest-path server-rust/Cargo.toml -p srui-sessiond --test conformance_frame_independence_test
```

**Swift**

```bash
swift test --package-path client-macos --filter FrameIndependenceConformanceTests
swift test --package-path client-macos --filter SplitFrameIndependenceTests
swift test --package-path client-macos --filter SplitRenderingTests
```
