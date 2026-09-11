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
fixtures live, how many there are, which commands run them, which implementations they apply to,
and which scenarios are open gaps.

| # | Directory | Suite (§32 item) | Current | Fixtures | Gap owner |
|---|---|---|---|---|---|
| 1 | [`01-core-state-machine/`](suites/01-core-state-machine/) | Core state-machine tests | `PASS` | 48 JSON vector(s) | — |
| 2 | [`02-widget-semantics/`](suites/02-widget-semantics/) | Widget semantic tests | `GAP` | generated from `registry.yaml` | unowned |
| 3 | [`03-semantic-not-paint/`](suites/03-semantic-not-paint/) | Semantic-not-paint tests | `PASS` | 1 JSON vector(s) | — |
| 4 | [`04-frame-independence/`](suites/04-frame-independence/) | Frame-independence tests | `PASS` | code-driven | — |
| 5 | [`05-semantic-input/`](suites/05-semantic-input/) | Semantic-input tests | `GAP` | generated from `registry.yaml` | Task 36 (VectorScene profile) |
| 6 | [`06-local-text-interaction/`](suites/06-local-text-interaction/) | Local text-interaction tests | `PASS` | code-driven | — |
| 7 | [`07-extension-negotiation/`](suites/07-extension-negotiation/) | Extension-negotiation tests | `GAP` | code-driven | Task 31 merge (branch codex/task-31-coding-agent) |
| 8 | [`08-reconnect/`](suites/08-reconnect/) | Reconnect tests | `PASS` | code-driven | — |
| 9 | [`09-security-limits/`](suites/09-security-limits/) | Security limits | `PASS` | code-driven | — |
| 10 | [`10-renderer-semantics/`](suites/10-renderer-semantics/) | Renderer semantic tests | `PASS` | code-driven | — |
| 11 | [`11-semantic-inspection/`](suites/11-semantic-inspection/) | Semantic inspection tests | `PASS` | code-driven | — |
| 12 | [`12-toolkit-mapping/`](suites/12-toolkit-mapping/) | Toolkit mapping tests | `PASS` | generated from `registry.yaml` | — |

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
| `PASS` | every declared runner succeeded **and** the suite has no open gaps | 0 |
| `GAP` | runners pass, but the suite documents §32 scenarios this base cannot exercise | 0 |
| `FAIL` | a runner failed, or a selected implementation has no runner and no declared reason | 1 |
| `SKIP` | the suite declares the selected implementation not applicable, with a reason | 0 |

A suite that documents an open gap reports `GAP`, never `PASS`. Reporting `PASS` alongside a
missing required scenario is exactly the overstatement the pass-or-known-gap criterion exists to
prevent.

The runner exits non-zero when the manifest is malformed, when a declared vector directory or
generated fixture is missing, when a selected implementation has no runner and no
`not_applicable` reason, **or when a declared gap turns out to be closed**. That last case
matters: without it a suite would stay labelled `GAP` forever after the owning task lands. Every
gap carries a negative `gap_probe`, and a match means the manifest must be updated rather than
the result quietly changing.

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

Fixture replay is shared: `server-rust/semantic-tree/tests/common/fixture_replay.rs` and
`StateMachineConformanceTests.replayVectorFile` are used by suites 1 and 3 alike, so a vector
relocated between suites is still *executed* rather than merely parsed.

### Registry-derived suites

Suites 2, 5 and 12 have **no fixture files**. They read the registry through the tables that
`./protocol/generate_proto.sh` already generates — `standardNodeTypesTable` (id, name and `tier`)
and `STANDARD_EVENTS` in `client-macos/SemanticModel/RegistryTables.swift` and
`server-rust/semantic-tree`'s build output — and drive real code against them.

An earlier revision generated a JSON fixture per suite from `registry.yaml` instead. That only
proved the generator agreed with itself: it could not fail when the *implementation* diverged from
the registry. Node tier, category and the `emits` matrix stay registry-owned, but they are
consumed from the existing generated tables rather than copied into a second oracle, so there is
no third artefact to keep fresh.

`emits` transcribes the §7.6 *Standard events* table exactly. A node absent from that table emits
nothing, and no capability may be added to the registry without adding it to the design document
first.

The AppKit mapping is the one part that is *not* registry-derived, and deliberately so: §22.4
makes native mappings informative, so they must not become protocol source of truth. Suite 12
asserts it directly against `ControlFactory`, which is the mapping's only real definition.

### Open gaps

Each gap below makes its suite report `GAP`. Full text, closure probes and acceptance criteria
are in the manifest and in each suite's README.

- **Suite 2** — §7.6 assigns `EXPANSION_CHANGED` to `Tree` and `VIEWPORT_CHANGED` to `Surface`,
  both required-tier, but `RendererAppKit/SemanticInteraction.swift` has no case that can
  originate either, so the renderer cannot emit them.
- **Suite 5** — server event validation accepts `POINTER_*` against ordinary widgets: the §32.5
  rule that coordinates are refused outside a subscribed scene is **unenforced**, not merely
  untested. The positive subscribed-scene half has no implementation either. Owned by **Task 36**.
- **Suite 7** — capability negotiation and must-understand rejection are covered; the
  fallback-subtree half is **Task 31** work not yet merged to `origin/main`.

