# Agent ticket — SRUI Process Explorer

Edition 1.2 · 12 September 2026. Use the authoritative SRUI v0.6 design and the revision-specific baseline.

## PX-018-G01 — Hierarchical model-backed Tree with local disclosure

**Phase:** B · **Scope size:** L · **Status:** planned · **Gate:** R1-prerequisite
**Dependencies:** PX-017
**Read:** D1 §§4 inv. 13, 6.3, 7.6, 8, 12.1, 22.5, 22.7, 22.8, 26, 32; registry Tree (19)

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

A Tree model's index space is the depth-first preorder of the whole forest. Two new standard item properties carry the shape: `tree_depth` (uint, 0 for a root) and `descendant_count` (uint, subtree size), added through registry.yaml, srui.proto, codegen and the build.rs count asserts, with committed Swift tables regenerated. No new wire messages: range requests, MODEL_INSERT/DELETE/UPDATE/RESET_RANGE, selection promotion and server-pushed ranges keep flat index semantics. Disclosure is local (§22.5): items start expanded; collapsing an item hides indices index+1 through index+descendant_count; the collapsed set is keyed by ItemId and only cached items can be collapsed; row-to-index mapping uses a sorted hidden-interval list so uncached gaps still show placeholders and fire asynchronous range requests for visible indices; indentation uses native outline levels from cached tree_depth, with no native control per item. Promotion reveal expands only the collapsed ancestors whose hidden intervals contain the promoted index. The client emits EXPANSION_CHANGED (§7.6) through a new semantic interaction without awaiting a reply, closing the Tree half of the suite-2 gap; the SDK gains Tree::on_expansion_changed. Tree reads `columns`, reusing the Table column and cell projection with the outline column first and a visible header. The SDK gains a TreeModel builder that converts a server-side forest into preorder items with depth and descendant counts and enforces the depth limit. A new §26 limit `max_model_tree_depth` (default 256) is enforced in Rust and Swift StoreLimits; apply-time checks are all-or-nothing and explicit: tree_depth within the limit, index+descendant_count < item_count, and where both neighbours are cached depth(i+1) <= depth(i)+1 and every child lies inside its parent's subtree span. A client without support renders the preorder flat with correct identity and selection, documented as the fallback. No application-specific client path.

### Out of scope

No per-parent child index spaces or new range messages, server-driven expansion state, initially-collapsed items, drag and drop or reordering, Surface VIEWPORT_CHANGED, or Process Explorer feature work (PX-018 adopts this separately).

### Verification / acceptance criteria

Rust and Swift tests plus conformance vectors: suite 9 negative vectors for depth over the limit, descendant_count past item_count, a depth jump over one, and a child outside its parent's span, rejected identically by Rust and Swift decode/apply; suites 1 and 12 positive vectors for a nested tree and for insert, delete and reset inside a subtree; suite 2 for the EXPANSION_CHANGED path with the manifest gap closed. Native AppKit tests: local collapse and expand send no range request and emit EXPANSION_CHANGED; a collapsed subtree with uncached ranges still loads visible rows asynchronously; promotion reveals through collapsed ancestors; a model update preserves the collapsed set by ItemId and drops entries whose items left the cache; a deep tree at and past the limit; Tree columns render with a header. SDK TreeModel tests for preorder, depth, descendant counts and limit enforcement.

### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-018-G01.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

### Reference lookup

Read [BASELINE.md](../BASELINE.md) and [SOURCES.md](../SOURCES.md). Verify version-specific OS/API details at implementation time and record exact references in the completion note.
