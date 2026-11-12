# Agent ticket — SRUI Process Explorer

Edition 1.2 · 12 September 2026. Use the authoritative SRUI v0.6 design and the revision-specific baseline.

## PX-018-G02 — Stamp collection selection and disclosure events with the rendered revision

**Phase:** B · **Scope size:** M · **Status:** planned · **Gate:** R1-prerequisite
**Dependencies:** PX-018, PX-017-G02
**Read:** D1 §§4 inv. 13, 6.3, 7.6, 7.7, 12.1, 12.2, 18.2, 22.5, 22.7, 32; registry List (17), Table (18), Tree (19), events SELECTION_CHANGED (3), EXPANSION_CHANGED (4)

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

The macOS client stamps every collection SELECTION_CHANGED (the item form and the cleared form of PX-017-G02) and every Tree EXPANSION_CHANGED with the revision its renderer had applied when the user acted, not with the applier's published snapshot. Today `SessionController.enqueueNativeSemanticAction` (`client-macos/Session/SessionController.swift:1201-1206`) sets `observedRevision` from `semanticInspectionSourceSnapshot()` (`:883-895`), which reads `applier.currentSnapshot`. The applier publishes a commit several suspensions before the main-actor render that applies it, so an action taken in that gap claims a revision the user never saw. PX-018 records the consequence as a known limitation (its completion note, Codex review round 5 and verification V6-F1; the srtop README): a collapse made between a Go to PID promotion's delivery and its render carries the promotion's revision, srtop takes it for a collapse made after the promotion and clears the inspector, and the render then expands the parent and selects the match with selection events suppressed. Keep a rendered revision on the main actor that advances only once the renderer has applied a commit, the catch-up snapshot and a resume replay included, and stamp these events' `observed_revision` with it. It is never ahead of the store, so the §18.2 future-revision rejection is unaffected; `event_seq`, event IDs, the outbox, retention, acknowledgement and replay are unchanged. The stamp then says what the user saw (§12.2 commits are not frames; §22.5 local interaction state). srtop needs no new rule: `ProcessView::expansion_changed_at` already treats a collapse observed at or before the revision before the latest revealing promotion as undone by it; once the native test passes, retire the PX-018 limitation in srtop's README and in the PX-018 completion note by pointing to this ticket's evidence. No protocol, registry, proto or framing change is expected; if one proves strictly necessary, stop and record a blocker, because it needs the user's approval. No application-specific client path.

### Out of scope

No change to which events are sent or to their arguments, no change to promotion, keep_visible or disclosure semantics, no change to TEXT_EDIT (§18.3 keeps its own observed-revision rules) or to ACTIVATE and VALUE_CHANGED stamping, no srtop behaviour change beyond retiring the documented PX-018 limitation, no server-side heuristic that guesses which reports were rendered, no edit of the read-only design document, and no protocol, registry, proto or framing change without recorded user approval.

### Verification / acceptance criteria

Each new test fails with the behaviour reverted. Native SSH against srtop `--fake-tree` in `ProcessExplorerShellTests`: through `liveTransactionPublishedInterceptorForTesting`, after the store has applied a Go to PID commit and before the render, the outline collapses the match's parent (`OutlineCollectionAdapter.setExpanded`); the EXPANSION_CHANGED carries the pre-promotion revision, the render expands the parent and selects the match, and after a later round trip the inspector still describes the match, reproducing the PX-018 verification V6-F1 sequence that fails today. Session tests: a SELECTION_CHANGED (item and cleared) and an EXPANSION_CHANGED made in the published-to-rendered gap carry the last rendered revision, one made after the render carries the new revision, the catch-up snapshot and a same-session resume advance the rendered revision only once rendered, a stamped revision is never ahead of the applier's, and a retained event replays with its original revision. The PX-018 native tests (a collapse made before a Go to PID arrives keeps the inspector on the match; a fresh client inherits no collapse), the PX-017-G02 deselect test and the relevant SRUI conformance suites still pass. Compatibility is recorded in the completion note: a server that ignores `observed_revision` for these events behaves as before, an older client keeps today's stamping and the documented PX-018 window, and an older srtop with a new client loses nothing.

### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-018-G02.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

### Reference lookup

Read [BASELINE.md](../BASELINE.md) and [SOURCES.md](../SOURCES.md). Verify version-specific OS/API details at implementation time and record exact references in the completion note.
