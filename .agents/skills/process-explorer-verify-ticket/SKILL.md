---
name: process-explorer-verify-ticket
description: Independently verify one completed SRUI Process Explorer ticket from a clean context and commit, without changing product code.
---

# Process Explorer Verify Ticket

Use this skill after an implementer reports a completed ticket.

## Independence

Start from the candidate implementation commit in a fresh worktree or clean checkout. Do not reuse the implementer's conversation, unstaged state, build assumptions, or conclusions. Read only the assigned ticket, the authoritative SRUI design sections it cites, the revision-specific baseline, verified dependency evidence, and the candidate diff.

Before any write, confirm the verifier worktree is non-main. Product source, tests, generated code, and plan specifications are read-only for this skill. Write only an external or explicitly designated verification report; never patch failures during verification.

Before testing, use the orchestrator's [worktree setup and recovery](../process-explorer-orchestrator/references/worktree-setup.md) for this fresh checkout. Installing locked development dependencies, producing ignored build/index artifacts, and pinning a child tool session to this workspace are environment preparation; product files remain read-only. If the assigned role mandates graph evidence, confirm a query resolves candidate-local symbols before the main verification run. A missing index or wrong workspace binding needs diagnosis, not an immediate claim that the required tool is unavailable.

## Review procedure

Check, in order:

1. Scope: changed files and behavior stay inside the ticket; no hidden follow-on work or weakened acceptance criteria.
2. Architecture: server authority, semantic transactions, collection identity, range/query generations, limits, reconnect, and client isolation follow the plan.
3. Safety: process targets use instance identity; confirmations bind immutable intent; no shell interpolation, unsafe privilege path, secret persistence, or local-resource escape.
4. Evidence: required deterministic tests, negative tests, wire/native tests, Linux/macOS/hardware/live evidence, completion note, and generated-file checks are present.
5. Reproducibility: run the narrow ticket profile from the clean worktree. Run broader release checks only when required by the ticket or gate.
6. Freshness: touch edited Rust/Swift sources if necessary, then distinguish a real result from stale incremental artifacts.
7. Claims: unavailable, denied, warming-up, stale, and failed states remain distinct; evidence supports every verified claim and release scope.

Classify the result as:

- `pass`: all acceptance criteria and required evidence are satisfied.
- `fail`: the implementation can be corrected within the ticket; list exact findings and commands.
- `blocked`: required OS, device, toolchain, reviewer, or external evidence is unavailable; do not convert absence into success.

Tie every failure to an explicit acceptance criterion or mandatory reviewer requirement. An incomplete optional screenshot is a warning unless it leaves required visual behavior unverified; passing object-identity assertions alone do not prove pixels, and missing pixels in a capture alone do not prove a renderer defect. Report what each source of evidence actually establishes.

## Result format

Write a machine-readable report and a concise human summary:

```json
{
  "ticket": "PX-008",
  "status": "pass|fail|blocked",
  "commit": "<candidate-sha>",
  "commands": [{"command": "...", "status": 0}],
  "evidence": ["docs/process-explorer/completions/PX-008.md"],
  "findings": [],
  "limitations": []
}
```

The report must state the clean worktree/base commit, exact commands, exit statuses, skipped checks with reasons, and whether any acceptance criterion remains unverified. A passing unit test cannot substitute for required native, Linux, hardware, or live-process evidence.

Never commit product changes from the verifier and never merge or push to `main`.
