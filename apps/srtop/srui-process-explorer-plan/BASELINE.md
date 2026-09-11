# Process Explorer baseline

Recorded 12 September 2026. This is a partial source audit plus one executed test command, not completion of PX-000 or certification of T0–T35.

## PR preparation update

The initial observations and example test result below refer to `3504b775`. Before preparing this PR, the main checkout had advanced to `f020b601` and matched the fetched origin/main. A dedicated worktree and branch `codex/process-explorer-plan` were created from that newer base. No source audit or example-test result below is silently transferred to the newer revision. PX-000 must refresh implementation evidence there. Main had no tracked changes to restore; the earlier placeholder wording is already corrected upstream.

## Initial checkout and environment

- Checkout: `3504b77550e5d4d87f457fc35a176f9d8eaf04a3`, branch `main`.
- `git status --short --branch` reported 50 commits behind the locally recorded `origin/main`; no fetch, pull, checkout, or merge was performed.
- The planning directory under `apps/` was untracked before this update; unrelated untracked directories/files were preserved.
- macOS 26.4.1 (25E253); rustc 1.98.0 (88d9e12ae 2026-08-18); cargo 1.98.0 (797e8a9bc 2026-08-05).
- PX-000 must record the chosen implementation revision and refresh this baseline if it differs. Do not automatically update the user's branch.

## Existing implementation and reuse decision

Preserve `examples/process-monitor/`. Build the product under `apps/srtop/`, with package/workspace layout determined at PX-000. Reuse the existing SDK/session/transport interfaces and only extract shared example code when a bounded change justifies it.

| Evidence | What exists | Product follow-up |
|---|---|---|
| `examples/process-monitor/src/main.rs` | Runnable Unix-socket server using Session, handle_connection, Monitor, SysinfoProcessSource, and SignalTerminator; one-second polling; shared Unix-security helpers | Reuse integration; product launches read-only |
| `examples/process-monitor/Cargo.toml` | Rust package srui-example-process-monitor; sysinfo 0.39 manifest requirement; SDK/session/protocol/semantic-tree/unix-security path dependencies | Use lockfiles for exact versions; no unsolicited upgrades |
| `examples/process-monitor/src/domain.rs` | Four columns: PID, Name, CPU %, Memory MiB; stable semantic ItemIds and typed row values | Keep useful data boundaries; improve availability/identity semantics |
| `examples/process-monitor/README.md` and app tests | Incremental model-backed rows, filtering, wire accounting, session/bootstrap integration, and documented SSH launch | Audit source/test coverage per acceptance slice; no duplicate transport |
| `server-rust/sessiond/src/session/model_range.rs` | Asynchronous range providers; observed revisions; stale-result outcome when authoritative revision changes during provider work | PX-010-G01 must prove progress under continuing updates and delayed requests |

The README describes an actual implementation, not a placeholder. Its example measurements are historical documentation, not new performance results from this review.

## Known gaps and early decisions

- `ProcessKey.start_time` is documented as whole seconds since the Unix epoch. The product needs source/boot/namespace context and a suitable native creation token. PID plus rounded seconds is insufficient for the new action invariant.
- The example has an unconfirmed Kill Selected workflow. Do not inherit it into R0/R1; immutable confirmation, authorization, handle identity, and observed outcomes belong to PX-028–PX-031.
- `quantize` turns nonfinite values into zero. Product metrics must preserve unavailable/warming-up/error states instead of treating them as measured zero.
- The example publishes the whole visible set and uses PID/start-time ordering. Typed live sorting, query generation, range progress, and finite retention/queue limits need an early end-to-end proof.
- PX-000 must inspect generic sort/header events, tree navigation, confirmation UI, local selection behavior, connection UX, and per-client query ownership. Existing type declarations alone are not native feature evidence.
- A held pidfd is not a reviewed strategy for every PID-based scheduling syscall. PX-035-G01 records supported/blocked decisions independently of shipping safe termination.
- Start privacy, accessibility, compatible-version behavior, and install/update/uninstall checks before R1; later broad audits extend these checks.
- No conclusion was reached about every T0–T35 milestone, optional T36–T38 capability, or every upstream htop feature.

## Executed verification

Command executed from the repository root:

```bash
cargo test --locked --manifest-path examples/process-monitor/Cargo.toml
```

Result: exit code 0 on the macOS environment above. This compiled and ran the example's local test suite, including wire and action-policy tests. The test output included successful terminator validation and wire/bootstrap checks. Full session log was written to `/tmp/srui-process-explorer-baseline-tests.log` (temporary, not a durable release artifact).

Not run: Linux live collector/action checks, macOS native GUI/SSH walkthrough, full Rust workspace/Swift/conformance suites, or stress benchmarks. The app test success does not prove those behaviors. PX-000 and each release ticket must record durable completion evidence.

## Reference commands for implementation

Use repository guidance and recheck at the implementation revision:

```bash
cargo build --locked --manifest-path examples/process-monitor/Cargo.toml
cargo test --locked --manifest-path examples/process-monitor/Cargo.toml
cargo test --manifest-path server-rust/Cargo.toml --workspace --all-targets
bash scripts/run-swift-tests.sh
uv run pytest protocol/tests
```

Inspect `scripts/run-conformance` for the required suite/implementation invocations. Native verification needs a Mac client and an accessible Linux host; fake fixtures alone do not close that gate. These reference commands, except the example test above, were not executed in this baseline update.
