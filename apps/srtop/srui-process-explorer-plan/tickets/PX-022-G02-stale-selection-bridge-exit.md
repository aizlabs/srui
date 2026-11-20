# Agent ticket — SRUI Process Explorer

Edition 1.2 · 12 September 2026. Use the authoritative SRUI v0.6 design and the revision-specific baseline.

## PX-022-G02 — Freeze user selection in stale session windows and end the SSH bridge with its daemon

**Phase:** C · **Scope size:** S · **Status:** planned · **Gate:** R1-prerequisite
**Dependencies:** PX-022-G01
**Read:** D1 §§17–18.3, 20.1, 22.5, 22.7; D2 T17, T19, T22–T24, T28

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

Two generic fixes that PX-022's native reconnect evidence showed are missing, each a separate defect in shared code. (1) Renderer: while a session window carries the client-owned status of PX-022-G01 (`LayoutRenderer.sessionWindowStatus` is set), a List, Table or Tree takes no user-initiated selection change. Today a click or arrow key in a detached window moves the native highlight, its `SELECTION_CHANGED` is dropped because no session can carry it (§18: no new semantic events before a continuity decision), and the renderer's local selection memory keeps the unreported row through a `RESUME_OK` replay and through a same-session snapshot remount, so the highlight then disagrees with every server-side view of the selection, such as an inspector and an action that targets the process it describes (§22.5: the selection highlight is local and only semantic outcomes reach the server, so a local selection the server was never told of is a divergence, not a preference). `TableCollectionAdapter` and `OutlineCollectionAdapter` answer `selectionShouldChange(in:)` from a gate that `ControlFactory` holds and `LayoutRenderer.setSessionWindowStatus(_:)` sets: closed while a status is shown, open when it is cleared. Server promotions (`selectRowIndexes`, §22.7), scrolling and tree disclosure are unaffected, and every adapter a remount builds reads the same gate. (2) `srui-ssh-bridge`: the bridge ends as soon as its session daemon closes the connection, even while the SSH side stays open and idle (§20.1: the bridge attaches and detaches a connection and owns no state, so a connection the daemon ended is over). Today `bridge_streams` uses `copy_bidirectional`, which returns only when both directions reach EOF, and daemon EOF only shuts down the write half of tokio's stdout, which does not close the SSH channel; and after `bridge_streams` returns, `main` returns into a tokio runtime shutdown that waits for the blocking thread reading stdin. An idle client of a stopped daemon therefore stays `connected` with frozen data and no stale status (§17: the client must learn its transport is gone), and the Connections window offers Open rather than Connect. Daemon EOF ends the bridge after everything it sent is flushed, and the process exits; the SSH-to-daemon direction keeps its half-close behaviour. No application-specific code.

### Out of scope

No change to the text or semantics of `setSessionWindowStatus`, to `ConnectionManager`, `SessionController`, the outbox, event dispatch, text editing or tree disclosure. No change to session continuity, resume, replay or retry (no new retry protocol). No SDK attach/detach notice (not approved), no protocol, registry, proto, framing, SDK or `srui-sessiond` change, no srtop change (PX-022 consumes both fixes), and no edit of the read-only design document.

### Verification / acceptance criteria

Each new test fails with its fix reverted, and the mutants are recorded in the completion note. `RendererAppKitTests` on a real `NSTableView` and a real `NSOutlineView`: with a status set, the user's Down Arrow leaves the selection where it is and reports nothing, and clearing the status lets the next arrow select and report; a server promotion still selects in a stale window; the gate survives a remount, and an empty status clears it. `srui-ssh-bridge` tests: the library bridge returns when the daemon side closes while the SSH side stays open, after delivering what the daemon sent, and the SSH side then reads EOF; the real binary exits successfully when the daemon closes while its stdin stays open, and its stdout reads to EOF; every existing bridge test still passes. `cargo fmt -p srui-ssh-bridge --check`, `cargo clippy -p srui-ssh-bridge --all-targets -- -D warnings`, the bridge tests on macOS and, as an unprivileged user, on Linux, `bash scripts/run-swift-tests.sh`, `scripts/check-test-process-stdio.sh`, `scripts/check-test-main-runloop-nesting.sh` and `scripts/parse-changed-swift.sh` pass.

### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-022-G02.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

### Reference lookup

Read [BASELINE.md](../BASELINE.md) and [SOURCES.md](../SOURCES.md). Verify version-specific OS/API details at implementation time and record exact references in the completion note.
