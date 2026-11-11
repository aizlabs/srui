# Agent ticket — SRUI Process Explorer

Edition 1.2 · 12 September 2026. Use the authoritative SRUI v0.6 design and the revision-specific baseline.

## PX-017-G01 — Server-hinted keep-in-view for collection items

**Phase:** B · **Scope size:** M · **Status:** planned · **Gate:** R1-prerequisite
**Dependencies:** PX-016
**Read:** D1 §§4 inv. 13, 6.3, 7.4, 8, 12.1, 22.5, 22.7, 26, 32; registry List (17), Table (18), Tree (19)

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

Add one standard property `keep_visible` on model-backed List, Table and Tree nodes: an ItemId the client keeps in view. It is presentation guidance from the server (§6.3) and never selection state: setting, changing or clearing it never selects, deselects or replaces a local selection, never posts a selection notification and never emits an event. Whenever a commit sets the property, or changes the row index at which the named item is shown, the client scrolls the minimum needed to make that row visible (the same one-shot reveal as a promotion, without the selection); a commit that leaves the item's row index unchanged never scrolls, so a local pan persists until the item moves. If the item is not cached the client waits and reveals it when it arrives through range data, requesting nothing new. A local selection, scroll or keyboard navigation never clears or overrides the property. Store rules in Rust and Swift: the value is an ItemId only, admitted only on model-backed List, Table and Tree, otherwise the transaction is refused. Authoritative settlement mirrors PX-011-G01: a transaction that removes the named item from the collection's model (delete, reset, positional update, model_ref change) clears the property in the same commit; an authoritative set naming an item the model does not hold is refused with ItemNotFound; a pre-built commit that would leave it dangling is refused. SDK set and clear on the three widgets. Registry, spec/registries.md, build.rs count asserts and generated Swift tables updated through codegen. A client without support keeps its viewport where the user put it, documented as the fallback. No application-specific client path.

### Out of scope

No change to `value` or promotion semantics, no Tree disclosure interaction beyond scrolling the row if it is visible (hierarchical disclosure is PX-018-G01's), no client-to-server viewport reporting, and no Process Explorer feature work (PX-017 adopts this separately).

### Verification / acceptance criteria

Rust and Swift tests plus conformance vectors: store admission and value-type refusals with rollback in both languages; settlement clears on every removal path and refuses an unknown item; positive and negative suite 1 vectors and the manifest count updated exactly. Native AppKit tests on a 10,000-row table: setting the property reveals the row without changing an existing local selection or emitting SELECTION_CHANGED; a commit that moves the item's row reveals it again; a commit that does not move it leaves a local pan untouched; 20 rapid local selections while the item keeps moving end on the last selection with no backward jump and no selection change caused by the property; an uncached item is revealed when it arrives; clearing the property never moves the viewport or the selection.

### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-017-G01.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

### Reference lookup

Read [BASELINE.md](../BASELINE.md) and [SOURCES.md](../SOURCES.md). Verify version-specific OS/API details at implementation time and record exact references in the completion note.
