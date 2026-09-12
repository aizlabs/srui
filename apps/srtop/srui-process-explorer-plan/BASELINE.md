# Process Explorer baseline

Recorded 12 September 2026 for PX-000 at implementation revision 270a6a5651f53511bd873119e123159d73d3891d. This is a revision-specific source audit and evidence record; it is not a claim that PX-000, T0–T35, or the product release gates are complete.

## Checkout and environment

- Candidate base: origin/codex/process-explorer-plan.
- Checkout: 270a6a5651f53511bd873119e123159d73d3891d (codex/px-000-implementation worktree).
- git status --short --branch was clean before this ticket's edits.
- The main checkout and submitted planning worktree were not modified.
- Host: macOS 26.4.1 (25E253).
- Toolchains: rustc 1.98.0 (88d9e12ae, 2026-08-18); cargo 1.98.0 (797e8a9bc, 2026-08-05).
- Native macOS client checks are available in principle; no Linux host or remote SSH test host was available to this run.

## Product location and entry point

The product is planned under apps/srtop/. The existing runnable process-monitor implementation is the reusable checkpoint at examples/process-monitor/:

- Binary entry point: examples/process-monitor/src/main.rs, binary process-monitor.
- Library entry point: examples/process-monitor/src/lib.rs.
- Server monitor orchestration: src/monitor.rs.
- Process domain and identity values: src/domain.rs.
- Sampling adapter: src/source.rs.
- Termination and PID identity checks: src/terminator.rs.
- Incremental model diffing: src/diff.rs.
- State, filtering, selection ownership, and denylist: src/state.rs.
- Semantic tree construction: src/ui.rs.
- Wire accounting: src/wire_stats.rs.
- Deterministic integration fixtures: src/testing.rs and tests/.

PX-000 does not create a second monitor or alter the example. Future product tickets must decide which generic seams to extract under apps/srtop/.

## Locked dependencies

The example's Cargo.toml declares the following direct dependencies; versions below are resolved from examples/process-monitor/Cargo.lock:

| Dependency | Resolved version | Role |
| --- | --- | --- |
| sysinfo | 0.39.6 | process and host sampling |
| tokio | 1.53.1 | async listener and polling tasks |
| tokio-util | 0.7.19 | cancellation support |
| tracing | 0.1.44 | diagnostics |
| tracing-subscriber | 0.3.23 | diagnostic configuration |
| nix | 0.31.3 | Unix signal/user APIs |
| libc | 0.2.189 | platform syscall bindings |
| srui-sdk, srui-sessiond, srui-protocol, srui-semantic-tree, srui-unix-security | 0.1.0 path packages | existing SRUI runtime |

The lockfile also contains nix 0.25.1 transitively; PX-000 does not upgrade or change dependencies.

## Reusable capabilities mapped to design

| Existing capability | Evidence | Applicable design |
| --- | --- | --- |
| Atomic semantic transactions and committed revisions | server-rust/sessiond, server-rust/semantic-tree | D1 §§4, 12.1 |
| Semantic widget builders and model-backed Table | server-rust/sdk, examples/process-monitor/src/ui.rs | D1 §§7.3, 8, 29 |
| Incremental model insert/update/delete | examples/process-monitor/src/diff.rs, src/state.rs | D1 §§8, 12.1 |
| Stable process-instance key and stale identity checks | ProcessKey(pid,start_time), src/monitor.rs, src/terminator.rs | D1 §4 identity invariant; D2 T21 |
| Event routing and per-client selection ownership | src/monitor.rs, src/state.rs, tests/kill_security_test.rs | D1 §§7.6, 7.7, 18 |
| Socket ownership/peer validation and session bootstrap | examples/process-monitor/src/main.rs, server-rust/sessiond, server-rust/unix-security | D1 §§12.1, 18, 29 |
| Deterministic fake-source test seam | examples/process-monitor/src/testing.rs and integration tests | D2 T21 verification support |

## Named gaps and acceptance limits

- The current example is the real T21-style checkpoint, but it is not a production Process Explorer and does not close all later PX tickets.
- ProcessKey currently uses the source's start_time value; its suitability across platforms and PID namespaces requires the PX-003 identity audit before production actions.
- Unavailable/denied metric states, richer typed fields, sorting, filtering/search UX, range progress, tree navigation, dialogs, and reconnect behavior need their named PX tickets and evidence.
- The generic Swift/AppKit renderer and SSH path require native end-to-end evidence; unit and fake-source tests cannot substitute for that evidence.
- Linux collector, Linux action, native macOS GUI/SSH, full Rust/Swift/conformance, and scale/stress checks were not available to this audit and remain open.
- The example intentionally lacks virtualization/range fetching, coalescing/backpressure, terminal embedding, confirmation/undo, and the full btop-inspired dashboard; these belong to later tickets.

## Executed baseline check

~~~
cargo test --locked --manifest-path examples/process-monitor/Cargo.toml
~~~

Result: exit code 0; 65 tests passed on this macOS checkout. This confirms the existing example and deterministic fixtures compile and pass at the audited revision. It does not certify live OS process collection, native rendering, SSH, Linux behavior, or release readiness.

## Reference commands not run

The following remain useful follow-up checks and were not claimed as passing here:

~~~
cargo build --locked --manifest-path examples/process-monitor/Cargo.toml
cargo test --manifest-path server-rust/Cargo.toml --workspace --all-targets
bash scripts/run-swift-tests.sh
uv run pytest protocol/tests
~~~
