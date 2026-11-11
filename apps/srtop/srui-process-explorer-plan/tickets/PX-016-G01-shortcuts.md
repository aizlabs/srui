# Agent ticket — SRUI Process Explorer

Edition 1.2 · 12 September 2026. Use the authoritative SRUI v0.6 design and the revision-specific baseline.

## PX-016-G01 — Platform-neutral semantic keyboard shortcuts

**Phase:** B · **Scope size:** M · **Status:** planned · **Gate:** R1-prerequisite
**Dependencies:** PX-015
**Read:** D1 §§4 inv. 13, 7.2, 7.4, 7.6, 7.7, 22.5, 22.6, 22.8, 22.9, 26, 32; D2 T9, T16, T33, T35

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

Add one standard property `shortcut`: a string in a closed canonical grammar `[primary+][shift+][alt+]<key>`, where `<key>` is one ASCII letter or digit or one of a small closed set of named keys, bounded at 32 bytes; `primary` is Command on macOS and Control elsewhere. Unknown tokens, duplicate or misordered modifiers, and over-length values fail explicitly at decode and apply (§4 inv. 13). Semantics by node type: Button dispatches ACTIVATE through the same path as a click; Toggle changes its value through the normal VALUE_CHANGED path; TextInput, TextArea, List, Table, Tree and selectable RichText move local focus only and send no event (§22.5); any other node type is refused. A printable key requires `primary`, so a bare letter never competes with a text editor. The client never dispatches while the first responder has marked text (IME), or for a disabled, hidden or collapsed node, or when its window is not key; the server still refuses a forged event to a disabled node. A shortcut in the platform's reserved table is never bound and yields a renderer diagnostic (macOS: Cmd-Q, Cmd-W, Cmd-H, Opt-Cmd-H, Cmd-M, Cmd-Tab, Cmd-`, Cmd-comma, Cmd-C, Cmd-V, Cmd-X, Cmd-A, Cmd-Z, Shift-Cmd-Z, Cmd-Space, Ctrl-Cmd-F, Cmd-?). Duplicate shortcuts within one Surface are refused at apply. The platform-formatted shortcut is discoverable in the tooltip, accessibility help and the T35 inspection snapshot. SDK builder/set/clear on the admitted widgets. Registry, spec/registries.md, build.rs count asserts and generated Swift tables updated through codegen; DirtyClassifier treats it as an in-place update. The generic client shell gains a standard Edit menu (Undo, Redo, Cut, Copy, Paste, Select All) and keeps the Surface window's key-view loop recalculated after remounts. No application-specific client path.

### Out of scope

No Menu or Toolbar node types, user-customizable bindings, multi-key chords, global hotkeys, external IPC, local policy engine, or Process Explorer feature work (PX-016 adopts this separately).

### Verification / acceptance criteria

Rust and Swift tests plus conformance vectors: suite 2 (accepted on admitted types, refused on others), suite 5 (a shortcut ACTIVATE is byte-identical to a click; a disabled node gets no event and a forged one is refused with NodeDisabled; a focus shortcut emits no event), suite 6 (a bare printable key is refused; no dispatch with marked text; letters typed into a TextInput never trigger a shortcut), suite 9 (negative malformed vectors for over-length, unknown token and duplicate modifier on both Rust and Swift decode paths), suites 10 and 12 (maps to keyEquivalent and modifier mask; a reserved combination stays unbound with a diagnostic; SET and CLEAR update in place; duplicates refused) and suite 11 (shortcut in the inspection snapshot). Native AppKit tests with real NSEvents: a Command shortcut activates a Button; the same key does nothing for a disabled Button or during IME composition; Cmd-C in a TextInput copies through the new Edit menu and is not overridden; Tab order survives a remount.

### Handoff and completion

Apply the standing completion contract. Record evidence in `docs/process-explorer/completions/PX-016-G01.md`, update the individual feature ledger, and commit only this bounded change after required checks pass. A gate remains open if a required behavior or native test is missing.

### Reference lookup

Read [BASELINE.md](../BASELINE.md) and [SOURCES.md](../SOURCES.md). Verify version-specific OS/API details at implementation time and record exact references in the completion note.
