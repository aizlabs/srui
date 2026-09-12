---
name: process-explorer-orchestrator
description: Drive the SRUI Process Explorer ticket backlog one dependency-ready ticket at a time, using dedicated worktrees and an independent verification context.
---

# Process Explorer Orchestrator

Use this skill when the user asks to automate, resume, or supervise implementation of the SRUI Process Explorer plan.

## Source of truth

Read these files from the candidate base revision before scheduling work:

- `apps/srtop/srui-process-explorer-plan/task-index.json`
- `apps/srtop/srui-process-explorer-plan/IMPLEMENTATION_PLAN.md`
- `apps/srtop/srui-process-explorer-plan/BASELINE.md`
- `apps/srtop/srui-process-explorer-plan/STANDING_AGENT_CONTRACT.md`

Use `task-index.json.execution_order` and each ticket's `depends_on`; ticket numbers alone do not define scheduling. Treat the plan as specification and keep execution state outside every repository checkout by default. Require an absolute external run directory (for example, `${CODEX_HOME}/process-explorer-run/<run-id>/` or `/tmp/process-explorer-run/<run-id>/`); never resolve a relative path from the launcher checkout. If external storage is unavailable, create a dedicated non-main orchestration worktree and record its absolute path.

## Scheduling loop

For each ticket:

1. Confirm the candidate base commit, branch, and worktree. Never write in a checkout on `main`. Create a new `codex/<ticket-id>-<short-name>` worktree from the intended base.
2. Mark the ticket `implementing` in external run state with the commit, branch, worktree, and timestamp.
3. Start one implementer context with only the ticket, required design sections, baseline, dependency completion notes, and worktree path.
4. Wait for its bounded result. It must commit only its ticket and report changed files, commands, evidence, and blockers.
5. Start a fresh verifier context from the implementation commit. Do not pass the implementer's conclusions as trusted evidence.
6. Accept the ticket only when the verifier returns `pass`; record the verifier commit, commands, evidence, and report path.
7. On `fail`, preserve the implementation commit and send only the verifier findings back for a bounded correction. Re-verify from a fresh context. On `blocked`, record the missing environment or prerequisite and stop dependent scheduling.
8. Schedule the next ready ticket only after dependencies are verified. Do not implement follow-on tickets opportunistically.

Keep state transitions explicit: `planned → ready → implementing → implementation_complete → verifying → verified`, with terminal `blocked`, `failed_verification`, and `cancelled`.

## State and evidence

The run state must be resumable and separate from the plan:

```text
.codex/process-explorer-run/
  state.json
  PX-008/
    implementation-result.json
    verification-result.json
```

Each result records the ticket ID, base and implementation commit, branch/worktree, exact commands, exit status, evidence paths, limitations, and timestamp. Never mark a ticket verified solely from a passing fake test when the ticket requires Linux, macOS, hardware, native UI, or live SSH evidence.

## Verification profiles

Select checks from the ticket and repository guidance:

- planning/documentation: plan validator, link/fence checks, `git diff --check`
- Rust: targeted Cargo tests, formatting, Clippy, and relevant conformance
- Swift: mtime refresh after edits, syntax/package checks, and native tests where available
- protocol: generation, registry validation, and conformance vectors
- live process work: deterministic fixtures plus bounded disposable owned workers
- release gates: clean install, reconnect, compatibility, privacy, accessibility, and measured evidence

Do not run a repository-wide hook merely because a ticket changes documentation. Use the narrow profile first; run broader checks when the ticket or release gate requires them. A hook failure is evidence to classify, not permission to weaken acceptance criteria.

## External actions

Creating branches, commits, and worktrees is part of this workflow. Pushing or opening a pull request requires the user's authorization for the current run; merging to `main` is never performed by this skill. Do not send messages or deploy artifacts unless separately authorized.

## Stop conditions

Stop and report when the dependency graph is invalid, a required clean verification context cannot be created, a required environment is unavailable, a verifier finds an unresolved safety issue, or a ticket exceeds its declared scope. Preserve all completed work and state.
