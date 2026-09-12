---
name: process-explorer-implement-ticket
description: Implement exactly one SRUI Process Explorer plan ticket in its assigned worktree, including tests and completion evidence.
---

# Process Explorer Implement Ticket

Use this skill only when an orchestrator or user assigns one concrete ticket from the SRUI Process Explorer plan.

## Scoped verification policy

Follow the orchestrator's [scoped verification policy](../process-explorer-orchestrator/SKILL.md#scoped-verification-policy). Use its recorded profile, or select and record one from the ticket and affected components if none was supplied. App-only work needs app checks and the required native/wire/Linux evidence, not every unchanged infrastructure, example or benchmark suite. Documentation-only work needs relevant validators.

Treat historical baseline/setup command lists as a menu. Reuse applicable baseline evidence for unchanged components, recording the revision and why it still applies; it cannot replace tests of new behavior. Add checks only for newly affected code, an explicit acceptance/release requirement, or a concrete regression, and record the reason.

After the selected checks pass on the final relevant code, finish the completion record and handoff without optional extra audits or unrelated reruns. Report selected commands, results, reused evidence and blockers concisely. Existing configured delivery hooks remain separate and must not be bypassed.

## Required context

Read the assigned ticket completely, then read the relevant sections of:

- `apps/srtop/srui-process-explorer-plan/BASELINE.md`
- `apps/srtop/srui-process-explorer-plan/STANDING_AGENT_CONTRACT.md`
- `apps/srtop/srui-process-explorer-plan/task-index.json`
- the authoritative SRUI design cited by the ticket
- completion notes for verified dependencies

Inspect actual repository APIs and tests. Suggested module names in the plan are roles, not guaranteed paths.

## Worktree and scope

Before any write, verify `git status --short --branch`, the current branch, and `git worktree list`. The assigned worktree must be on a non-main task branch. Never edit, generate, format, stage, or commit in a checkout on `main`.

Implement one ticket only. Preserve unrelated changes and other worktrees. Do not silently change dependencies, acceptance criteria, protocol semantics, or the plan's feature claims. If a missing reusable capability is discovered, stop at the boundary and record a bounded prerequisite or blocker.

Keep process enumeration, metric interpretation, filtering, identity, authorization, and actions server-side. Reuse SRUI transactions, collection models, range requests, continuity, limits, and event settlement. Do not add process-specific client policy or a second transport/retry protocol.

## Implementation loop

1. Establish the ticket's acceptance criteria and scoped verification profile; update the profile if the actual diff affects additional components.
2. Inspect the smallest relevant code surface and existing tests.
3. Implement the bounded change with deterministic positive and negative tests.
4. Add or update the ticket completion note with exact commands, results, native/live evidence, and limitations.
5. Run the ticket-specific checks. Touch edited Rust/Swift files before trusting incremental results when the repository's mtime warning applies.
6. Review the diff for unrelated files, generated-file freshness, secrets, unsafe shell construction, and scope expansion.
7. Commit a bounded change with a message beginning with the ticket ID.

Report the commit SHA, changed files, commands and exit statuses, evidence paths, limitations, and any blocked acceptance item. Do not start the next ticket and do not declare independent verification.
