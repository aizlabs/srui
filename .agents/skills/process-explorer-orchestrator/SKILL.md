---
name: process-explorer-orchestrator
description: Drive the SRUI Process Explorer ticket backlog one dependency-ready ticket at a time, using dedicated worktrees and an independent verification context.
---

# Process Explorer Orchestrator

Use this skill when the user asks to automate, resume, or supervise implementation of the SRUI Process Explorer plan.

## Scoped verification policy

Select checks from the ticket's acceptance criteria and affected components before implementation. Do not rerun unrelated repository suites merely because they exist. Record the affected components, exact selected commands, reasons, and reusable baseline evidence in external run state; pass this profile to both agents.

- **App-only changes:** run the app's tests, formatting/linting, and native, wire, Linux or live-process checks required by the ticket or changed behavior. Add conformance for the semantics affected. Using an unchanged SDK/runtime/renderer does not by itself require its entire test suite, unrelated examples, or benchmark packages.
- **Shared infrastructure changes:** add focused tests for the changed SDK, runtime, protocol, renderer or dependency and its affected consumers. Broaden to full suites when the affected surface or a concrete regression warrants it.
- **Documentation/skills:** use the relevant document, skill, schema, link/fence and whitespace validators; do not build product packages.
- **Release gates:** run the broader checks explicitly required by that gate. A scoped profile never substitutes fake tests for required native, Linux, hardware or live evidence.

Treat baseline command lists and setup-reference examples as a check-selection menu, not an instruction to rerun every historical command on every ticket. Reuse baseline evidence only after confirming its relevant source, dependency pins, test configuration and environment remain applicable; record its revision and results. Reused evidence does not verify new behavior.

The implementer runs the selected checks; the fresh verifier independently confirms their scope against the diff and reruns the ticket acceptance checks on the candidate. Neither agent should expand into unrelated suites by default. Record a concrete reason for additions. Once selected checks pass, repeat them only for relevant changes or unresolved failures; after environment repair, rerun the affected check. Checks excluded as unrelated are not missing acceptance evidence.

Repository-wide validation belongs in CI and explicit release/shared-change checks, subject to existing repository delivery requirements. This policy does not change or bypass configured Git hooks. Keep hook failures separate from ticket acceptance, and avoid an extra manual run of a hook that delivery will invoke.

## Source of truth

Read these files from the candidate base revision before scheduling work:

- `apps/srtop/srui-process-explorer-plan/task-index.json`
- `apps/srtop/srui-process-explorer-plan/IMPLEMENTATION_PLAN.md`
- `apps/srtop/srui-process-explorer-plan/BASELINE.md`
- `apps/srtop/srui-process-explorer-plan/STANDING_AGENT_CONTRACT.md`

Use `task-index.json.execution_order` and each ticket's `depends_on`; ticket numbers alone do not define scheduling. Treat the plan as specification and keep execution state outside every repository checkout by default. Require an absolute external run directory (for example, `${CODEX_HOME}/process-explorer-run/<run-id>/` or `/tmp/process-explorer-run/<run-id>/`); never resolve a relative path from the launcher checkout. If external storage is unavailable, create a dedicated non-main orchestration worktree and record its absolute path.

## Worktree preflight

Before dispatching either agent, read [Worktree setup and recovery](references/worktree-setup.md). Initialize the declared development dependencies in each new worktree, check the actual test interpreter, and establish required native/SSH and reviewer-tool access early. A successful check in another checkout does not establish this worktree's environment.

Choose an agent context whose requirements fit the ticket. A specialized reviewer can impose additional mandatory tooling checks; discover and preflight those before dispatch. Do not invent graph requirements when none apply, or silently discard a mandatory requirement after selecting that role.

## Scheduling loop

For each ticket:

1. Confirm the candidate base commit, branch, and worktree. Never write in a checkout on `main`. Create a new `codex/<ticket-id>-<short-name>` worktree from the intended base.
2. Mark the ticket `implementing` in external run state with the commit, branch, worktree, and timestamp.
3. Start one implementer context with only the ticket, required design sections, baseline, dependency completion notes, scoped verification profile, and worktree path.
4. Wait for its bounded result. It must report changed files, commands, evidence, and blockers and normally commit only its ticket. If the user requires the final task commit after verification, preserve an immutable candidate snapshot for the clean verifier, then confirm the final commit has the same tree. Do not pass unstaged implementation state to the verifier.
5. Start a fresh verifier context from the candidate commit with the ticket and scoped verification profile. Do not pass the implementer's conclusions as trusted evidence.
6. Accept the ticket only when the verifier returns `pass`; record the verifier commit, commands, evidence, and report path.
7. Honor an explicit stop-on-fail/blocked instruction when an agent returns that terminal result. Otherwise, on `fail`, preserve the candidate and send only the verifier findings back for a bounded correction; re-verify from a fresh context. On `blocked`, record the missing prerequisite and stop dependent scheduling. Ordinary implementation debugging and bounded setup recovery happen before a terminal verdict, unless the user explicitly requests stopping on the first unsuccessful command.
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

Record implementation, independent verification, and delivery separately. A pre-push environment failure leaves an existing verification pass intact: record `delivery: blocked` with the exact failed command, log, and recovery attempt. Preserve prior reports when resuming. If only environment/index configuration changes and the candidate tree is unchanged, rerun the affected check; do not repeat unrelated passing tests without a new reason. Product changes require fresh verification.

## Verification profiles

Select checks from the ticket and repository guidance:

- planning/documentation: plan validator, link/fence checks, `git diff --check`
- Rust: targeted Cargo tests, formatting, Clippy, and relevant conformance
- Swift: mtime refresh after edits, syntax/package checks, and native tests where available
- protocol: generation, registry validation, and conformance vectors
- live process work: deterministic fixtures plus bounded disposable owned workers
- release gates: clean install, reconnect, compatibility, privacy, accessibility, and measured evidence

Do not run a repository-wide hook merely because a ticket changes documentation. Use the narrow profile first; run broader checks when the ticket or release gate requires them. A hook failure is evidence to classify, not permission to weaken acceptance criteria. Prepare required delivery dependencies before pushing, and let configured hooks run. Diagnose and repair bounded local setup problems using the recovery reference; never bypass a required hook or change dependency pins merely to get a green result.

## External actions

Creating branches, commits, and worktrees is part of this workflow. For an ordinary implementation or supervision request, the delivery step after a passing verification is to push the task branch and open or update its pull request; the initiating implementation request authorizes that delivery. An explicit local-only or canary request suppresses push and pull-request creation. If the request does not establish either mode, mark delivery `pending_authorization` and ask before pushing. Merging to `main` is never performed by this skill. Do not send messages or deploy artifacts unless separately authorized.

## Stop conditions

Stop and report when the dependency graph is invalid, a required clean verification context cannot be created, a required environment is unavailable, a verifier finds an unresolved safety issue, or a ticket exceeds its declared scope. Preserve all completed work and state.
