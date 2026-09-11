# Suite 11 — Semantic inspection tests (§32 item 11)

**Status:** `active`
**Spec sections:** §4, §7.7, §22.9, §32.11

Run this suite alone:

```bash
scripts/run-conformance --suite 11
```

## Fixtures

Code-driven suite: no shared fixtures. Inspection reads the client's semantic snapshot, and
available actions come from the generated registry tables rather than from AppKit widget classes
or a second hand-maintained oracle.

## Runners

**Swift**

```bash
scripts/check-accessibility-api-boundary.sh
swift test --package-path client-macos --filter SemanticInspectorTests
swift test --package-path client-macos --filter SemanticInspectionAutomationTests
swift test --package-path client-macos --filter CodingAgentFallbackSocketTests
```

These cover semantic identity and deterministic traversal, checked automation through the normal
event/authorization path, and an end-to-end coding-agent interaction without exposing AppKit
objects. The boundary runner enforces the toolkit-free package, source-import, and compiled
public-symbol surface before the suite can report `PASS`.

**Rust** — `N/A`: Semantic inspection and local automation are client-side APIs (§22.9); the
server has no inspection API surface.
