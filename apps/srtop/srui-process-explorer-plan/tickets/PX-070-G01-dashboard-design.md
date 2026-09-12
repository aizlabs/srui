# Agent ticket — SRUI Process Explorer

Edition 1.2 · 12 September 2026. Use the authoritative SRUI v0.6 design and the revision-specific baseline.

## PX-070-G01 — Specify the btop-inspired native dashboard and evaluation scope

**Phase:** G · **Scope size:** M · **Status:** planned · **Gate:** Dashboard-design
**Dependencies:** PX-031, PX-070
**Read:** D1 §§6–8, 12, 18, 22, 25–26, 31–32; PX-070–PX-073, PX-090; sources BT1

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

Pin a btop++ reference tag/commit and document selected workflows: at-a-glance host activity, process discovery, and sampled-trend inspection. Keep htop 3.5.3 as the process-management baseline. This is not full btop parity or permission to copy upstream code/assets.
Specify CPU aggregate, memory/swap, disk, and network panels beside a searchable process table. Include units, sample intervals, independent timestamps, warming-up/stale/denied states, and compact/expanded layouts. Selecting a process opens the existing inspector and its explicitly sampled history.
Define bounded panel configuration, local resizing, keyboard focus, accessibility alternatives, host/account labels, and disconnected/paused presentation. Detailed per-CPU, temperature, and GPU panels arrive later with verified collectors and do not block the core dashboard.
Define objective tasks and pass/fail criteria before implementation: locate a busy process, inspect a trend, inspect disk/network activity, recover after disconnect. Compare matched sampling, fields, workloads, and environments; do not infer superiority from screenshots.

### Out of scope

No btop parity commitment, copied code/assets, process-specific renderer logic, or R1 scope expansion.

### Verification / acceptance criteria

Review generic widget/profile support. Every required panel has a collector/renderer owner, unit/freshness policy, finite budget, and accessible fallback. Record a reference version and evaluation protocol. Missing generic capabilities become prerequisite tickets. R1/safe termination do not depend on this dashboard.

### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-070-G01.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

### Reference lookup

Read [BASELINE.md](../BASELINE.md) and [SOURCES.md](../SOURCES.md). External reference IDs: BT1. Verify version-specific OS/API details at implementation time and record exact references in the completion note.
