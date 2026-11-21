# Agent ticket — SRUI Process Explorer

Edition 1.2 · 12 September 2026. Use the authoritative SRUI v0.6 design and the revision-specific baseline.

## PX-026-G02 — Refuse incompatible clients with the existing handshake refusal

**Phase:** C · **Scope size:** S · **Status:** planned · **Gate:** R1-prerequisite
**Dependencies:** PX-026
**Read:** D1 §§15, 18, 19.2, 26, 32.7

### Standing execution contract

```text
You are implementing ONE bounded ticket for SRUI Process Explorer, an application built on the existing SRUI runtime. Read this entire ticket before changing code.

AUTHORITATIVE CONTEXT
- SRUI design v0.6 is authoritative and read-only. Read BASELINE.md and PX-000 evidence for the exact checkout: T0–T35 are audit scope, not a blanket completion claim. Reuse the real T21 example where verified. Check T36–T38 capabilities rather than assuming presence or absence.
- Read the ticket's cited design sections, current app code, prerequisite completion notes, and feature ledger. Reuse T21 where safe. Existing T21 process-changing demo code is not automatically suitable for production.
- Initial stack: Rust server/app, generic Swift/AppKit client, existing Protobuf and non-PTY SSH binding. Suggested module names in the plan are roles, not assertions about files in the repository.

WORKING RULES
- Before changing any repository file, verify the branch and worktree. Never edit, generate, format, stage, or commit files in a checkout on main. Use a dedicated task worktree on a new non-main branch based on origin/main. Main changes only through pull-request merges; preserve other worktrees and unrelated work.
- Follow task-index.json execution_order and depends_on, not numeric ticket order. Read BASELINE.md and EXECUTION_TIMELINE.md for scope; time estimates never waive acceptance gates.
- Implement only this ticket. Preserve user work and unrelated examples; no speculative framework rewrite, global formatting, silent dependency upgrade, or following-ticket implementation.
- Keep collection, filtering policy, process identity, authorization and action execution server-side. Standard UI uses the SDK and generic renderer. Never add a process-name/PID-specific client code path.
- Reuse atomic transactions, event settlement, range models, resource limits, and continuity-aware reconnect. No own frame stream/retry stack. MODEL_UPDATE is not eligible for the scalar SET_PROPERTY coalescing rule.
- A numeric PID, row index, or display name is not process-instance identity. Actions bind to immutable server-validated targets, not mutable selection. No shell interpolation. No root sessiond/bridge, no unreviewed privilege helper, and no remote-triggered local file/clipboard/URL access.
- Local typing/scroll feedback stays local; remote results may arrive later. T35 automation is in-process only unless a separately reviewed IPC ticket is being executed.
- Respect configured memory/time/message bounds. Unavailable, denied, warming-up, stale and failed values must not silently become zero. Use deterministic fake sources for correctness and only bounded disposable owned workers for live tests.
- A missing generic protocol/widget facility is a separate narrowly scoped prerequisite with conformance tests. Do not extend standard semantics ad hoc, weaken tests, modify the read-only design, or claim unsupported infrastructure already exists.
- If the task exceeds one coherent change, write suffixed prerequisite/continuation tickets with exact dependencies and acceptance tests before expanding it. Existing IDs are append-only. Review-gated branches require recorded external approval, not agent self-approval.

COMPLETION CONTRACT
Run the repository commands recorded by PX-000, plus the ticket's verification. Add deterministic positive and negative tests; add wire/UI integration tests where the feature crosses those boundaries. Verify relevant SRUI conformance still passes. Record actual commands, results, native/manual evidence, and any unavailable environment in docs/process-explorer/completions/<ticket-id>.md. Update the feature ledger and README where behavior changes. A skipped test or absent OS/hardware is not a pass. On a real blocker, produce a precise blocker report and preserve completed work; do not silently waive the gate. Commit the bounded change only after required checks pass, using a message beginning with the ticket ID. Report changed files, tests, limitations and remaining blockers; do not proceed to another ticket.
```

### Build

A generic `srui-sessiond` fix that PX-026-G01's "incompatible versions fail clearly" criterion needs. Today a HELLO or RESUME whose `core_version` has a `major.minor` this build does not serve (`SessionError::UnsupportedCoreVersion`), or whose offer does not satisfy the session's required profiles (`SessionError::Negotiation`), is closed without a frame. Both sides use the same `major.minor` rule and the server checks first, so real core-version skew always takes this path: the client reports only `transport ended: receive stream closed by peer` and logs `Reconnect and resume to recover (§18)`. §18 requires the server to fail closed before it subscribes the connection or emits replay, snapshot, resource or Terminal data. `handle_connection` now writes the existing `ServerHandshakeRefused { reason: HANDSHAKE_REFUSAL_REASON_UNSPECIFIED, detail }` through the refusal path PX-004-G01 added for undeliverable snapshots, flushes it within `WRITE_TIMEOUT`, and closes. The detail is deterministic, bounded to 256 bytes and names only versions and profiles. It echoes at most 32 characters of the client's `core_version`, with every character outside `[A-Za-z0-9.+_-]` replaced by `?` and an absent version shown as `(none)`. It lists at most 8 missing required profiles followed by `+N more`, and never echoes the client's own offer. Every other handshake failure keeps its close-only behaviour. The generic Swift client already fails as `handshakeRefused` without reconnecting; for `UNSPECIFIED` it now shows the detail without a `reason 0:` prefix. `protocol/README.md` and the `connection.rs` documentation state when the refusal is sent and how older clients behave.

### Out of scope

No change to `protocol/srui.proto`, `registry.yaml`, codegen, generated files or enum values, and no new refusal reason. No change to the negotiation rules, the core version, session continuity, resume, replay or retry, `ConnectionManager` or srtop. No edit of the read-only design document, and none of PX-026-G01's packaging work.

### Verification / acceptance criteria

Each new test fails with its fix reverted, and the mutants are recorded in the completion note: no refusal sent, refusal sent after a snapshot or replay frame, unbounded detail, refusal sent to a compatible client, and RESUME skipping the refusal. `connection_failure_test` asserts exactly one `ServerHandshakeRefused{UNSPECIFIED, detail}` and then close, with no frame before it. It covers HELLO and RESUME (same and replaced incarnation), for both the core-version and the profile case. It also asserts a bounded, sanitized detail for a hostile multi-megabyte `core_version` and a huge hostile profile offer, a bounded list for many or very long missing required profiles, and an unchanged WELCOME or RESUME_OK for a compatible client. `HandshakeNegotiationTests` shows the client reports `server refused the handshake: <detail>` and sends no further HELLO or RESUME. Against a live `srui-sessiond` over a Unix socket, both cases are refused on the wire and the native client shows the detail. `cargo test --all-targets`, `cargo clippy --all-targets -- -D warnings`, `cargo test -p srui-benchmark-driver`, srtop `cargo test --locked` on macOS and on Linux, `bash scripts/run-swift-tests.sh`, `scripts/run-conformance --suite 7` and `--suite 8` for both implementations, and `uv run pytest protocol/tests` pass.

### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-026-G02.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

### Reference lookup

Read [BASELINE.md](../BASELINE.md) and [SOURCES.md](../SOURCES.md). Verify version-specific OS/API details at implementation time and record exact references in the completion note.
