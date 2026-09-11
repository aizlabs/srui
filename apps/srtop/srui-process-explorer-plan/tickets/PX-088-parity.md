# Agent ticket — SRUI Process Explorer

Edition 1.2 · 12 September 2026. Use the authoritative SRUI v0.6 design and the revision-specific baseline.

## PX-088 — Close the pinned Linux htop feature ledger

**Phase:** I · **Scope size:** S · **Status:** planned · **Gate:** R3
**Dependencies:** PX-087, PX-041, PX-059, PX-067, PX-075
**Read:** D1 §§31–32; D2 T33–T34; sources H1–H8

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

Re-enumerate htop 3.5.3 actions, available fields, meter registrations, display settings, startup options, and optional compile-time features for the named Linux build. Compare with the machine-readable ledger, not memory or screenshots. Account for entries present in source but omitted from the manual.
For every unresolved implementable gap, create a separately numbered small ticket (suffixes preserve existing IDs), implement it, and rerun the relevant tests before marking closure. Break a large gap into collector/UI/verification work. This gate is not permission to write all missing features in one unreviewable commit.
Distinguish functional equivalence, native presentation adaptation, permission/environment unavailable, not implemented, and verified. Missing code is never excused as absent hardware. Keep PCP, additional OS backends, privileged administration, and terminal-only deployment as separate explicitly tracked scopes.

### Out of scope

No unreviewed new protocol semantics, weakened tests to obtain parity, silent feature exclusions, or unrelated product features.

### Verification / acceptance criteria

Every upstream inventory entry has a mapping and evidence/status. Ordinary-user Linux features are verified on an identified reference system; optional hardware/build paths need fixtures plus named live evidence before being called verified. The release claim lists exclusions and cannot say full parity while any required feature is missing.

### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-088.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

### Reference lookup

Read [BASELINE.md](../BASELINE.md) and [SOURCES.md](../SOURCES.md). External reference IDs: H1–H8. Verify version-specific OS/API details at implementation time and record exact references in the completion note.
