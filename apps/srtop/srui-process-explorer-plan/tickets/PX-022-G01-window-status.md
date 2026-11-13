# Agent ticket — SRUI Process Explorer

Edition 1.2 · 12 September 2026. Use the authoritative SRUI v0.6 design and the revision-specific baseline.

## PX-022-G01 — Client-owned session-window connection status

**Phase:** C · **Scope size:** M · **Status:** planned · **Gate:** R1-prerequisite
**Dependencies:** PX-021
**Read:** D1 §§17–18.3, 22.3, 22.8; D2 T22–T24, T29, T38

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

The generic macOS client shows the transport state of a session on every window of that session, as client-owned chrome that no server can set or clear. Today the only trusted indicator is the row status in the separate Connections window (`ConnectionStatus.displayText`, `client-macos/ConnectionManager/ConnectionManager.swift`); the session window's title is server-owned (`ControlFactory.apply(property:value:to:store:)` writes a Surface `label` to `NSWindow.title`, `client-macos/RendererAppKit/ControlFactory.swift:403`), and `ConnectionManager` reaches the session windows only through `context.renderer.showWindows()`. Add a public `AppKitRenderer` API that sets one client-owned status line on every Surface window of the session as `NSWindow.subtitle`, which AppKit draws under the title and which no Surface property writes, so the status and the server title never overwrite each other. The renderer keeps the status and applies it to every Surface window it creates, so it survives a structural remount (which closes and rebuilds the windows) and reaches windows created later. It also sets each window's accessibility help to the same text, so VoiceOver reaches it whether or not the macOS release folds the subtitle into the window's accessibility title (§22.8). `ConnectionManager` drives it on the session context's renderer from the same transitions that set `ConnectionStatus` (D2 T38): connecting and disconnected say the window shows stale data from the last applied revision (`connecting — showing data from revision N (stale)`, `disconnected — showing data from revision N (stale)`; §17 DETACHED), resynchronizing says `resynchronizing — showing data from revision N (stale)` until the catch-up that follows the continuity decision is ready (§18), a REPLACED decision says `session replaced — rebuilding` (§18), and ready clears it. Only the current resume generation changes it: an outcome from a superseded attempt is dropped by the token check that already guards `ConnectionStatus` (§18, generation-bound resume attempts). The status is cleared before `ConnectionManager` drops a session context (a failed first connection, a removed entry, a draft that never connected at shutdown). No application-specific client path.

### Out of scope

No change to session continuity, resume, replay or retry (no new retry protocol), to the `ConnectionStatus` values or the Connections window, to the server-owned window title or any Surface property, or to T29 pending-text handling. No SDK attach/detach notice (not approved), no server, SDK, protocol, registry, proto or framing change, no srtop change (PX-022 consumes this API), no edit of the read-only design document, and no spoken announcement of status changes.

### Verification / acceptance criteria

Each new test fails with the behaviour reverted. `RendererAppKitTests`: setting, changing and clearing the status updates every Surface window of a two-Surface store; the server title stays intact when the status is set, and the status stays when a commit changes the Surface `label`; a status set before the first mount reaches the first windows, and a window created by a later structural commit and every window a remount rebuilds carry it; each window's accessibility help carries the status and clearing removes it. `ConnectionManagerTests` with fake attempts: every `ConnectionStatus` transition maps to its chrome on the context's renderer (connecting and disconnected name the last applied revision as stale, resynchronizing, REPLACED, ready clears), including on a mounted real `NSWindow`; REPLACED followed by ready clears it; a late failure from a superseded attempt cannot overwrite the newer attempt's status; a failed first connection clears the status before its context is dropped. Native evidence: a real `NSWindow` in the AppKit test host shows the status as its subtitle beside the unchanged server title, recorded in the completion note. `bash scripts/run-swift-tests.sh` passes twice, with `scripts/check-test-process-stdio.sh`, `scripts/check-test-main-runloop-nesting.sh` and `scripts/parse-changed-swift.sh`.

### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-022-G01.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

### Reference lookup

Read [BASELINE.md](../BASELINE.md) and [SOURCES.md](../SOURCES.md). Verify version-specific OS/API details at implementation time and record exact references in the completion note.
