# Agent ticket — SRUI Process Explorer

Edition 1.2 · 12 September 2026. Use the authoritative SRUI v0.6 design and the revision-specific baseline.

## PX-017-G02 — Report a user-cleared collection selection

**Phase:** B · **Scope size:** M · **Status:** planned · **Gate:** R1-prerequisite
**Dependencies:** PX-017
**Read:** D1 §§4 inv. 13, 6.3, 7.6, 7.7, 8, 18.2, 22.5, 27, 32; registry List (17), Table (18), Tree (19), event SELECTION_CHANGED (3)

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

When a user action leaves a List, Table or Tree with no selected item, the client sends exactly one SELECTION_CHANGED with no `value` argument, meaning "nothing is selected now". Servers treat it as clearing that client's selection, never as malformed; a `value` that is present but not an ItemId stays malformed. A deselection the server causes (a promotion withdrawal, a commit that removes the selected row, a promotion waiting for an uncached item, a restore on remount) still sends nothing. Multiple selection stays one event per selected item, and a partial deselection in multiple mode stays unreported. Specify it in spec/registries.md (collection `value` entry and the standard-events section) without editing the read-only design document; no registry ID, proto or framing change. Rust `Event::selection_cleared` and `Event::selection_change` returning `SelectionChange::{Item, Cleared}`, re-exported by the SDK; Swift `Event.selectionCleared`. The client carries it through `SemanticInteraction.selectionCleared`, `SessionController` native dispatch, `EventOutbox.sendSelectionCleared` (retained and replayed like any event) and the in-process automation action `SemanticAction.selectionCleared`. `TableCollectionAdapter` and `OutlineCollectionAdapter` report it from the selection delegate when the selection becomes empty, and a Tree reports it when a user collapse hides every selected row; server-driven changes stay suppressed and PX-011-G01 withdrawal is unchanged. The ui-gallery and process-monitor examples clear their recorded selection instead of rejecting or ignoring it, so process-monitor's Kill Selected can no longer reach a deselected process. srtop says the selection was cleared: the inspector clears, follow stops with that reason, and turning follow on afterwards is refused. No application-specific client path.

### Out of scope

No new registry entry or wire field, no per-item deselection events in multiple mode, no change to promotion or keep_visible semantics, no Tabs/Select/ChoiceGroup behaviour (they have no empty selection), no edit of the read-only design document, and no tree-mode srtop work (PX-018).

### Verification / acceptance criteria

Each new test fails with the behaviour reverted. Rust: the cleared constructor and reader, including malformed values and other event types, and a wire roundtrip of the argumentless event. Native AppKit adapter tests: a user deselect sends exactly one cleared event; a commit removing the selected row, a promotion, its withdrawal, a pending promotion and an empty restore send none; a user deselect after a promotion is reported and the later withdrawal stays a no-op; multiple selection is unchanged and only a full clear is reported; a Tree collapse that hides the selection is reported once and one that keeps it shown is not. Session and outbox: the cleared event is encoded with no arguments, sent in order through the native path, retained, and replayed unchanged on same-session resume; the automation action is refused for selection_mode none. Native SSH against srtop: `deselectAll(nil)` through the table delegate clears the inspector, stops follow with its reason and clears the hint, and turning follow on afterwards is refused, including select, deselect, then follow on. Example and srtop unit tests cover the cleared event, and process-monitor's Kill Selected refuses after a clear.

### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-017-G02.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

### Reference lookup

Read [BASELINE.md](../BASELINE.md) and [SOURCES.md](../SOURCES.md). Verify version-specific OS/API details at implementation time and record exact references in the completion note.
