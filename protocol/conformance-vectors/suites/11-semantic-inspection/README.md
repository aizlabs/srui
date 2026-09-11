# Suite 11 — Semantic inspection tests (§32 item 11)

**Status:** `active`
**Spec sections:** §4, §7.7, §22.9, §32.11

Run this suite alone:

```bash
scripts/run-conformance --suite 11
```

## Fixtures

Code-driven suite: no shared conformance-vector files. Inspection reads the client's semantic
snapshot, and available actions come from the generated registry tables rather than from AppKit
widget classes or a second hand-maintained oracle. The suite builds the Rust coding-agent demo
before the live fallback-socket runner so it also works from a clean checkout.

Programmatic text entry is intentionally not exposed by Task 35. `TEXT_EDIT` is not a generic value
change: it requires per-editor `edit_seq`, composition/coalescing, resume, and acknowledgement
handling owned by `TextEditingSession`. Text editors remain inspectable and advertise their
registry-owned `EVENT_TEXT_EDIT`; `setValue` is not an alias for text entry.

## Runners

**Swift**

```bash
scripts/check-accessibility-api-boundary.sh
swift test --package-path client-macos --filter SemanticInspectorTests
swift test --package-path client-macos --filter SemanticInspectionAutomationTests
cargo build --manifest-path examples/coding-agent-demo/Cargo.toml
swift test --package-path client-macos --filter CodingAgentFallbackSocketTests
```

These cover semantic identity and deterministic traversal, checked automation through the normal
event/authorization path, and an end-to-end coding-agent interaction without exposing AppKit
objects. The boundary runner enforces the toolkit-free package, source-import, and compiled
public-symbol surface before the suite can report `PASS`.

**Rust** — `N/A`: Semantic inspection and local automation are client-side APIs (§22.9); the
server has no inspection API surface.
