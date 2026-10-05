# Agent ticket — SRUI Process Explorer

Edition 1.2 · 12 September 2026. Use the authoritative SRUI v0.6 design and the revision-specific baseline.

## PX-005-G01 — Read resident memory from statm, exact from Linux 6.16

**Phase:** A · **Scope size:** S · **Status:** planned · **Gate:** none
**Dependencies:** PX-005
**Read:** D1 §§6–8, 12, 22, 29; D2 T9–T11, T16, T21; sources K1

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

PX-005 publishes resident memory from `/proc/<pid>/stat` field 24, which `do_task_stat` fills from `get_mm_rss`. Since Linux 6.2 the mm rss counters are percpu_counters and `get_mm_counter` reads them with `percpu_counter_read_positive`, which leaves out every per-CPU delta below the batch of `max(32, 2 × online CPUs)` pages: each counter may be off by up to that batch per CPU, and a just-started or small process can read 0 while it holds resident pages. Linux 6.16 ("mm: fix the inaccurate memory statistics issue for users") switched `task_statm` and `task_mem` — `/proc/<pid>/statm` and `/proc/<pid>/status` — to the exact `get_mm_counter_sum`; `/proc/<pid>/stat` still reads the approximate count through current mainline.
Read resident memory from `/proc/<pid>/statm` field 2 (`resident`) times the scanned mount's page size: exact from 6.16, and on older kernels the same counter as today, so never worse, for one extra bounded read per process per scan. Do not branch on kernel version. Keep PX-005's Known, Unavailable and Denied states and the mount's own page size; read through `read_bounded`, parse the field count and every integer strictly, check the multiplication, and treat a failed read as unread, never zero. A task with no address space (a kernel thread, a zombie) keeps the state PX-005 gives it. A `statm` that fails after `stat` succeeded — vanished, denied or malformed — is handled consistently with the existing record and field states, never by a silent fallback to field 24.
Evaluate reading both files through one pinned `/proc/<pid>` directory with `std` alone, so a PID reused between the two reads cannot be attributed; measure the cost of the choice and document why.

### Out of scope

No change to PX-006 CPU behaviour, no protocol, runtime or client change, no kernel-version branch, no further memory field, and no change to the live tests' worker helper.

### Verification / acceptance criteria

Deterministic fixtures prove resident memory comes from `statm`, not `stat`: their `stat` and `statm` counts differ, so reverting to field 24 fails a test. Fixtures also cover a malformed, short or overflowing `statm`, one that vanished between the two reads, a denied one, and tasks with no address space; the existing live tests stay green, and the key behaviours are mutation-checked. Scan time is measured before and after on Linux with a few thousand bounded, disposable, test-owned sleeping workers, per scan and per process, noting the `MAX_RECORDS` bound. The README and completion note state that the value is exact from 6.16, approximate on 6.2–6.15 with its bound and a possible 0 B for a fresh process, the extra read's cost, and exact kernel source references (file, function, tag). Where the PX-005 note or ledger claims exact bytes, the correction is recorded in this ticket's note and ledger without rewriting PX-005's history.

### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-005-G01.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

### Reference lookup

Read [BASELINE.md](../BASELINE.md) and [SOURCES.md](../SOURCES.md). External reference IDs: K1. Verify version-specific OS/API details at implementation time and record exact references in the completion note.
