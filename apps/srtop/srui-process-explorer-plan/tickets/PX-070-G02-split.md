# Agent ticket — SRUI Process Explorer

Edition 1.2 · 12 September 2026. Use the authoritative SRUI v0.6 design and the revision-specific baseline.

## PX-070-G02 — Render the standard Split container with locally resized panes

**Phase:** G · **Scope size:** M · **Status:** planned · **Gate:** none
**Dependencies:** PX-070-G01
**Read:** D1 §§4 inv. 13, 7.2, 7.3, 7.6, 10, 22.4, 22.5, 22.8, 26, 32; registry Split (25); `docs/process-explorer/design/dashboard.md`

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

`Split` (registry node type 25, SHOULD tier in §7.3, `NSSplitView` in §22.4, a core layout container in §10) is in `protocol/registry.yaml` but nothing builds or renders it: the SDK has no Split builder (`server-rust/sdk/src/widgets/` builds Surface, Row, Column, Grid and Scroll, not Split), and `ControlFactory.makeHandle` (`client-macos/RendererAppKit/ControlFactory.swift`) has no `.split` case and throws `ControlFactoryError.unsupportedNodeType`. PX-070-G01's dashboard specification (`docs/process-explorer/design/dashboard.md`) needs it for a user-resizable divider between the host panels and the process table. Add it as a generic standard container. Each child is one pane, in child order. D1 names no pane axis and no existing property expresses one (Row and Column express their axis by node type), so add one registry-owned property `split_axis` with a new enum `SplitAxis` (`side_by_side`, `stacked`); absent means `side_by_side`; it is admitted only on Split and refused on every other node type at apply in Rust and Swift (§4 inv. 13). Pane sizing reuses the existing layout hints on each child (`minimum_size`, `maximum_size`, `preferred_size`, `grow`) as hints, never pixel mandates (§10); a child whose `visibility` is `collapsed` collapses its pane, and `hidden` keeps its space. At most 8 panes per Split: a ninth child fails the transaction explicitly, and nesting counts toward the existing tree-depth limit (§26). Divider positions are local interaction state (§7.6 `Split`: resize locally; §22.5): the user drags a divider or adjusts it through accessibility, the client sends no geometry and this ticket emits no `VALUE_CHANGED` (Split's optional event stays unimplemented and is documented as such). A commit that keeps a Split's panes keeps the user's divider positions; adding or removing a pane keeps the remaining panes' proportions; a remount restores the proportions when the pane set is unchanged. SDK builder with the axis and panes. Registry, `spec/registries.md`, the `server-rust/semantic-tree/build.rs` count asserts and the committed generated Swift tables updated through `./protocol/generate_proto.sh`; `DirtyClassifier` classifies `split_axis` as structure-affecting for that Split only and pane hints as layout. No per-node-type capability negotiation is added: a client older than this ticket still refuses Split explicitly, which the completion note records as a compatibility limit. No application-specific client path.

### Out of scope

No Tabs, Menu, Toolbar or Overlay. No divider-position event, server-set divider position or persisted geometry. No per-node-type capability negotiation. No srtop change (PX-073-G01 adopts Split). No edit of the read-only design document.

### Verification / acceptance criteria

Each new test fails with its behaviour reverted, and the mutants are recorded in the completion note. Rust and Swift tests plus conformance vectors with the manifest counts updated exactly: suites 1 and 2 (a Split with 1 to 8 panes is accepted; a ninth child is refused with the whole transaction rolled back; `split_axis` is accepted on Split and refused on every other node type; an unknown `SplitAxis` value is refused), suite 3 (Split fixtures carry no pixel geometry and no divider position), suite 4 (dragging a divider changes no protocol traffic: byte-identical streams with and without local resizing), suite 9 (negative malformed vectors for an unknown axis value and an over-limit pane count on both the Rust and Swift decode paths), and suites 10 and 12 (`side_by_side` maps to a vertical-divider `NSSplitView` and `stacked` to a horizontal one; a collapsed child collapses its pane; the accessibility tree exposes a split group whose splitters carry a value and an increment/decrement action). Native AppKit tests on a real `NSWindow`: setting a divider position emits no event and no outgoing frame; a commit updating a pane's `Text` keeps the divider where the user put it; inserting a third pane keeps the first divider's proportion; a remount with the same panes restores the proportions; a pane's `minimum_size` is honoured while dragging. `./protocol/generate_proto.sh` leaves no diff, `uv run python protocol/validate_registry.py`, `cargo test --manifest-path server-rust/Cargo.toml`, `cargo clippy` for each changed crate with `-D warnings`, `scripts/run-conformance`, `bash scripts/run-swift-tests.sh`, `scripts/check-test-process-stdio.sh` and `scripts/parse-changed-swift.sh` pass.

### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-070-G02.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

### Reference lookup

Read [BASELINE.md](../BASELINE.md) and [SOURCES.md](../SOURCES.md). Verify version-specific OS/API details at implementation time and record exact references in the completion note.
