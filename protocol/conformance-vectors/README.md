# Conformance Vectors

Cross-language conformance test vectors for the SRUI protocol suite (§32).

## 1. Binary Wire Fixtures (Protobuf & Framing)

Fixed binary wire fixtures for cross-language Protobuf conformance checks (§16, §19, §32).

| File | Encodes |
|---|---|
| `golden_node_record.bin` | A `NodeRecord` (Button #42, §7.1 example) |
| `golden_transaction.bin` | A `Transaction` with three operations (§12.1 example) |
| `golden_framed_message.bin` | Length-prefixed framed `SruiMessage` containing a transaction |
| `golden_event_ack.bin` | Length-prefixed framed `SruiMessage` containing a `ServerEventAck` (§18.2) |
| `golden_client_model_range_request.bin` | Length-prefixed framed `SruiMessage` containing a `ClientModelRangeRequest` (§8, §22.7) |
| `golden_text_edit_event.bin` | Length-prefixed framed `SruiMessage` containing a valid whole-value `TEXT_EDIT` with positive `edit_seq` (§18.3, §22.6) |
| `malformed_overlong_varint.bin` | Truncated/overlong varint rejection check |
| `malformed_truncated_frame.bin` | Truncated length-prefixed frame rejection check |
| `malformed_text_edit_zero_edit_seq.bin` | Protobuf-valid `TEXT_EDIT` rejected during semantic conversion because `edit_seq == 0` |
| `malformed_activate_nonzero_edit_seq.bin` | Protobuf-valid non-text event rejected during semantic conversion because `edit_seq != 0` |
| `golden_terminal_data.bin` | Length-prefixed framed `SruiMessage` containing a `TerminalData` output frame (§21) |
| `golden_terminal_input.bin` | Length-prefixed framed `SruiMessage` containing a `TerminalInput` keystroke frame (§21) |
| `golden_terminal_resize.bin` | Length-prefixed framed `SruiMessage` containing a `TerminalResize` (TIOCSWINSZ) frame (§21) |
| `golden_terminal_resync_required.bin` | Length-prefixed framed `SruiMessage` containing a `TerminalResyncRequired` with `RETENTION_LOSS` (§21.2) |
| `malformed_terminal_input_empty.bin` | Protobuf-valid `TerminalInput` rejected at the wire boundary because it carries no payload (§21, §26) |
| `malformed_terminal_data_empty.bin` | Protobuf-valid `TerminalData` rejected by the client apply path because it carries no payload (§21, §26) |

See `expected.json` for canonical hex and field declarations.

## 2. The Twelve §32 Suites (`suites/`)

[`suites/manifest.json`](suites/manifest.json) is the machine-readable index of the twelve
conformance suites §32 requires. It is the single source for which suites exist, where their
fixtures live, how many there are, which commands run them, and which scenarios are known gaps.

| # | Directory | Suite (§32 item) | Status | Fixtures | Gap owner |
|---|---|---|---|---|---|
| 1 | [`01-core-state-machine/`](suites/01-core-state-machine/) | Core state-machine tests | `active` | 48 JSON vectors | — |
| 2 | [`02-widget-semantics/`](suites/02-widget-semantics/) | Widget semantic tests | `active` | generated from `registry.yaml` | none — tier is a deliberate scope boundary, not a defect |
| 3 | [`03-semantic-not-paint/`](suites/03-semantic-not-paint/) | Semantic-not-paint tests | `active` | 1 JSON vector | — |
| 4 | [`04-frame-independence/`](suites/04-frame-independence/) | Frame-independence tests | `active` | code-driven | — |
| 5 | [`05-semantic-input/`](suites/05-semantic-input/) | Semantic-input tests | `active` | generated from `registry.yaml` | Task 36 (VectorScene profile, optional) |
| 6 | [`06-local-text-interaction/`](suites/06-local-text-interaction/) | Local text-interaction tests | `active` | code-driven | — |
| 7 | [`07-extension-negotiation/`](suites/07-extension-negotiation/) | Extension-negotiation tests | `active` | code-driven | Task 31 merge (branch codex/task-31-coding-agent) |
| 8 | [`08-reconnect/`](suites/08-reconnect/) | Reconnect tests | `active` | code-driven | — |
| 9 | [`09-security-limits/`](suites/09-security-limits/) | Security limits | `active` | code-driven | — |
| 10 | [`10-renderer-semantics/`](suites/10-renderer-semantics/) | Renderer semantic tests | `active` | code-driven | — |
| 11 | [`11-semantic-inspection/`](suites/11-semantic-inspection/) | Semantic inspection tests | **`known_gap`** | code-driven | Task 35 (local semantic inspection and automation API) |
| 12 | [`12-toolkit-mapping/`](suites/12-toolkit-mapping/) | Toolkit mapping tests | `active` | generated from `registry.yaml` | — |

### Running the suites

```bash
scripts/run-conformance                       # all twelve, Rust + Swift (macOS)
scripts/run-conformance --suite 8             # one suite by id
scripts/run-conformance --suite reconnect     # one suite by slug
scripts/run-conformance --implementation rust # Linux-viable subset
scripts/run-conformance --list                # index only, runs nothing
```

Result semantics:

| Result | Meaning | Exit contribution |
|---|---|---|
| `PASS` | every declared runner for the suite succeeded | 0 |
| `FAIL` | a runner failed, or its binary was missing | 1 |
| `GAP` | a documented, manifest-declared gap with an owning task | 0 |
| `SKIP` | no runner for the selected implementation (e.g. suite 10 under `--implementation rust`) | 0 |

The runner also exits non-zero when the manifest is malformed, when a suite declares a vector
directory or generated fixture that does not exist, **or when a declared gap turns out to be
closed**. That last case matters: without it, suite 11 would stay labelled `GAP` forever after
Task 35 lands. Gap entries carry a negative `gap_probe`, and a match means the manifest must be
updated rather than the result quietly changing.

A full Rust+Swift run needs macOS, because `RendererAppKit` cannot build on Linux.

### Adding a fixture

1. Add the `.json` file under the suite's `vectors/` directory.
2. **Bump that suite's `vectors.count` in `suites/manifest.json` in the same commit.** The Rust
   loader (`server-rust/semantic-tree/tests/common/mod.rs`), the Swift loader
   (`client-macos/Tests/SemanticModelTests/ConformanceManifest.swift`) and
   `protocol/tests/test_conformance_manifest.py` all assert the count *exactly*, in both
   directions — a fixture that is not declared fails just as loudly as one that went missing.
   This is deliberate: a floor assertion would let a reorganization drop most of a corpus and
   still report green.
3. Run `scripts/run-conformance --suite <id>`.

### Generated fixtures

Suites 2, 5 and 12 consume fixtures generated from `protocol/registry.yaml` by
`protocol/generate_conformance_matrix.py` (wired into `./protocol/generate_proto.sh`, and gated
by `git diff --exit-code` in CI). Node tier, category and the `emits` matrix are registry-owned
and are never restated by hand, so promoting a widget from `should` to `required` cannot leave a
stale copy behind in a test fixture.

The AppKit mapping table is the one part that is *not* registry-derived, and deliberately so:
§22.4 makes native mappings informative, so they must not become protocol source of truth. It
lives in the generator and fails codegen if the registry gains a node type it does not cover.

### Known gaps

- **Suite 5** — the positive half of §32.5 (coordinates *accepted* for a subscribed custom scene)
  has no implementation: `POINTER_*` events are registered, but no `VectorScene` node type or
  subscription model exists. Owned by **Task 36**.
- **Suite 7** — capability negotiation and must-understand rejection are covered; the
  fallback-subtree half is **Task 31** work not yet merged to `origin/main`.
- **Suite 11** — `client-macos/Accessibility` is a placeholder; the whole suite awaits **Task 35**,
  whose acceptance criteria are recorded in the manifest and in
  [`suites/11-semantic-inspection/README.md`](suites/11-semantic-inspection/README.md).

