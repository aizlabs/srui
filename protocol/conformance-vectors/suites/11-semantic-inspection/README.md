# Suite 11 — Semantic inspection tests (§32 item 11)

**Status:** `known_gap`  
**Spec sections:** §24, §25, §32.11

Run this suite alone:

```bash
scripts/run-conformance --suite 11
```

## Fixtures

Code-driven suite: no shared vectors. The runners are listed in [`../manifest.json`](../manifest.json).

## Runners

**Rust** — not applicable: Semantic inspection is a client-side API (§24, §25); there is no server half.

**Swift** — not applicable: No implementation exists yet — the whole suite is the Task 35 gap.

## Documented gaps

The runner reports this suite as `GAP` rather than `PASS` while any of these remain open, and exits non-zero if a probe shows one has been closed without the manifest being updated.

### Inspection exposes semantic identity rather than AppKit object identity; automation actions route through the normal event/authorization path.

- **Why:** client-macos/Accessibility is a 4-line placeholder; no inspection or automation API exists on this base.
- **Owner:** Task 35 (local semantic inspection and automation API)
- **Closure probe:** `client-macos/Accessibility/Accessibility.swift` matching `public (struct|final class|protocol|func) +(Semantic)?Inspect`
- **Acceptance:** Inspection results are keyed by semantic NodeId, never by NSView identity.
- **Acceptance:** Each inspected node exposes role, label, value, hierarchy position, and available actions.
- **Acceptance:** Automation actions are dispatched through EventOutbox, not by invoking AppKit targets directly.
- **Acceptance:** Actions on a disabled node are refused with the same authorization path as user input.
- **Acceptance:** No AppKit type appears in the public inspection API surface.
