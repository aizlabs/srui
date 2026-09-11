# SRUI Implementation Plan — Sequential Task Tickets (T0–T38)

This is a task backlog for implementing the SRUI design (`SRUI_Semantic_Remote_UI_Design_v0.6.md`, v0.6) as a series of small, independently verifiable jobs for coding agents (Claude Code, Codex, etc.).

**One small note on the design doc, not a change:** its final line reads "End of SRUI design draft v0.2" — a leftover footer from an earlier revision. Cosmetic only; nothing in this plan depends on it, so it's left untouched rather than edited per your instruction to avoid touching the doc unless an error would cause real harm.

## How to use this document

Each task below is a **self-contained prompt** in a fenced block. To execute task N:

1. Start a fresh coding-agent session in the SRUI repository.
2. Attach/paste `SRUI_Semantic_Remote_UI_Design_v0.6.md`.
3. Copy the entire fenced block for that task and paste it as your instruction.
4. Let the agent inspect the repo, implement, and run the verification steps.
5. Only once verification passes, commit, and move to the next task's block.

Tasks are ordered so each one only needs what previous tasks already built, plus the design doc. Don't skip ahead — several later tasks (transport, reconnect, security) assume earlier invariants are already enforced in code, not just on paper.

Stack, fixed by the design doc itself: **Rust** for the server (`sessiond`, SDK), **Swift + AppKit** for the macOS reference client, **Protocol Buffers** as the reference wire encoding.

## Phase map

| Phase | Tasks | Theme |
|---|---|---|
| A | T0–T2 | Repo scaffold, ID registries, protobuf schema |
| B | T3–T8 | Rust Protocol Core (values, store, transactions, models, events, first conformance harness) |
| C | T9–T11 | Rust Standard Widget Profile + server SDK + wire encode |
| D | T12–T16 | Swift Protocol Core + fixture-driven AppKit renderer (no network yet) |
| E | T17–T18 | Local loopback transport, first end-to-end demo |
| F | T19–T25 | Real SSH transport, capability negotiation, a first live demo app (system/process monitor, T21 — the first checkpoint runnable over a real connection), session/connection split, reconnect journal, event dedupe, backpressure |
| G | T26–T27 | Resource CAS, priority scheduler |
| H | T28–T30 | Collections/virtualization, local text editing, terminal profile |
| I | T31–T35 | Coding-agent example, security hardening, conformance suite, benchmarks, local inspection API |
| J | T36–T37 | Deferred/optional: VectorScene, second-language client |
| K | T38 | Client connection manager UI (host entry, saved sessions, reconnect) |

Every task's paste block begins with the same standing preamble — repeated deliberately so each block works alone in a cold agent session.

---

## Phase A — Foundations

### Task 0 — Repository scaffold — Effort: Low

```
You are implementing one bounded task in a larger project called SRUI (Semantic Remote UI), a
protocol that replicates application UI *meaning/state* to a native local renderer instead of
remotely painting pixels. The authoritative design document is attached:
`SRUI_Semantic_Remote_UI_Design_v0.6.md`. Treat it as read-only and authoritative — do not edit
it. This is Task 0 of a sequential implementation plan (Tasks 0–38). This is the first task, so
the repository is currently empty or near-empty.

Read: design doc §28 (repository layout).

Build:
- Create the repository layout from §28: spec/, protocol/ (with conformance-vectors/ dir),
  client-macos/ (with the subdirectories listed: TransportSSH, Protocol, SemanticModel, Session,
  RendererAppKit, Collections, Text, Terminal, Resources, Accessibility, Tests), server-rust/
  (with ssh-bridge, sessiond, semantic-tree, journal, event-dedupe, resources, pty, sdk,
  examples), sdk/second-language/ (empty placeholder + README saying "not yet implemented, see
  Task 36"), examples/ (counter, process-monitor, coding-agent-demo — empty placeholders),
  benchmarks/ (parse-render, mutation, reconnect, network, terminal — empty placeholders).
- server-rust/: a Cargo workspace with empty-but-building library crates for semantic-tree,
  journal, event-dedupe, resources, pty, sdk, and a binary crate each for ssh-bridge and
  sessiond. They can just be stub `lib.rs`/`main.rs` files for now.
- client-macos/: a Swift Package (or Xcode project, your choice, but prefer SwiftPM for CI-ability)
  with empty-but-building targets matching the directory list above (SemanticModel, Protocol,
  Session, RendererAppKit, Collections, Text, Terminal, Resources, Accessibility as library
  targets; a Tests target).
- Root README.md: one paragraph pointing at the design doc and stating the project is being
  built as a sequence of tracked tasks.
- .gitignore appropriate for Rust + Swift.
- A CI config placeholder (GitHub Actions or similar) that just runs `cargo build` and
  `swift build` — no logic to test yet.

Out of scope: do not implement any protocol logic, types, or schemas yet. Every crate/target
should be an empty shell that compiles.

Verification (must pass before you stop):
- `cargo build` succeeds from server-rust/.
- `swift build` succeeds from client-macos/.
- Directory tree matches §28 exactly (report any deliberate deviations and why).
- Commit the result.
```

### Task 1 — Type/property/event ID registries — Effort: Low

```
You are implementing one bounded task in a larger project called SRUI (Semantic Remote UI). The
authoritative design document is attached: `SRUI_Semantic_Remote_UI_Design_v0.6.md`. Treat it as
read-only and authoritative — do not edit it. This is Task 1 of a sequential implementation plan
(Tasks 0–38). Task 0 already created the repository scaffold from §28 — inspect it before
starting; do not redo it.

Read: §6.4 (type/property references, namespaces), §7.2–7.6 (standard widget vocabulary,
properties, roles, events), §13 (core mutation operation names).

Build, under protocol/:
- `registry.yaml`: the canonical, language-agnostic registry for namespace 0 (the standard
  registry). Enumerate, with permanently assigned numeric IDs starting at a sane base and never
  to be reused once assigned:
  - all standard node types from the §7.3 required tier AND the SHOULD tier (Select,
    ChoiceGroup, Slider, NumberInput, Tabs, Split) AND Menu/Toolbar, plus the layout containers
    from §7.2 (Surface, Dialog, Row, Column, Grid, Spacer, Separator, Scroll) — the full §7.2
    table, not just the required tier (later tasks decide implementation order, but IDs must
    exist now so they never shift).
  - all common properties from §7.4 (identity/accessibility, common state, content, layout
    intent).
  - all standard enums from §7.5 (TextRole, ActionRole, InputRole, Importance) and the
    Toggle `presentation_hint` enum from §7.2.
  - all standard event types from §7.6 (ACTIVATE, VALUE_CHANGED, SELECTION_CHANGED,
    EXPANSION_CHANGED, TEXT_EDIT, VIEWPORT_CHANGED, POINTER_DOWN and siblings).
  - all core mutation operation names from §13.
- `spec/registries.md`: short doc explaining the ID-assignment rule (append-only, monotonic,
  never reuse or renumber, namespace 0 reserved for this registry, extensions get
  session-negotiated namespace numbers per §6.4) and how to add a new entry safely.
- A small validation script (language of your choice, e.g. Python or a Rust binary under
  protocol/) that checks registry.yaml for: no duplicate IDs within a category, no gaps that
  look like accidental deletions, and that every §7.3 required-tier node type is present.

Out of scope: do not generate Rust/Swift code from this registry yet (that's Task 2/3). Do not
implement extension namespace negotiation logic yet (Task 20).

Verification:
- Validation script runs clean against registry.yaml.
- Manually cross-check (and note in your summary) that every node type in the §7.2 table and
  every property in §7.4 appears exactly once.
- Commit the result.
```

### Task 2 — Protobuf wire schema + cross-language golden fixture — Effort: Medium

```
You are implementing one bounded task in a larger project called SRUI (Semantic Remote UI). The
authoritative design document is attached: `SRUI_Semantic_Remote_UI_Design_v0.6.md`. Treat it as
read-only and authoritative — do not edit it. This is Task 2 of a sequential implementation plan
(Tasks 0–38). Tasks 0–1 already created the repo scaffold and the ID registry (protocol/registry.yaml)
— inspect them before starting.

Read: §6.5 (value types), §15 (capability negotiation / HELLO-WELCOME), §16 (reference wire
encoding, including the illustrative proto in that section), §18 (reconnect messages:
CLIENT RESUME / SERVER RESUME_OK / SERVER RESYNC_REQUIRED).

Build, under protocol/:
- `srui.proto`, expanding §16's illustrative schema into something complete enough for later
  tasks to use as-is: TypeRef, PropertyRef, Value (oneof per §6.5, including size/range/point-like
  tuples and small typed records), NodeRecord, Operation (CREATE_NODE, DELETE_NODE, SET_PROPERTY,
  CLEAR_PROPERTY, MOVE_NODE, REORDER_CHILDREN, BATCH_PROPERTY_SET, CREATE_MODEL, MODEL_INSERT,
  MODEL_DELETE, MODEL_UPDATE, MODEL_RESET_RANGE per §13), Transaction, Event, and the handshake
  messages (ClientHello, ServerWelcome, ClientResume, ServerResumeOk, ServerResyncRequired) per
  §15 and §18. Use registry.yaml's numeric IDs for standard TypeRef/PropertyRef local_id values
  where you hardcode enums; namespace_id 0 is the standard registry.
- Set up code generation for both Rust (prost or protobuf crate — your choice) and Swift
  (SwiftProtobuf), wired into each build (build.rs for Rust, a SwiftPM plugin or a committed
  generation script for Swift — document which you chose and why).
- A golden fixture: a small, fixed, checked-in binary file (protocol/conformance-vectors/
  golden_node_record.bin or similar) produced by hand-writing one NodeRecord and one Transaction
  with a couple of operations, encoded once and committed as bytes (not regenerated per test run).

Out of scope: no application logic yet, no store, no networking. This task is purely schema +
codegen + one golden byte fixture.

Verification:
- `protoc`/your chosen toolchain compiles srui.proto without error for both language targets.
- A minimal Rust test decodes the golden fixture and asserts the expected field values.
- A minimal Swift test decodes the *same* golden fixture bytes and asserts the same field values
  (this is the first cross-language conformance check — both languages must agree on the wire
  format).
- Commit the result, including the golden fixture bytes.
```

---

## Phase B — Rust Protocol Core

### Task 3 — Rust: Value and identity types — Effort: Low

```
You are implementing one bounded task in a larger project called SRUI (Semantic Remote UI). The
authoritative design document is attached: `SRUI_Semantic_Remote_UI_Design_v0.6.md`. Treat it as
read-only and authoritative — do not edit it. This is Task 3 of a sequential implementation plan
(Tasks 0–38). Tasks 0–2 built the repo scaffold, the ID registry, and the protobuf schema +
codegen — inspect them before starting.

Read: §6.2 (node identity), §6.4 (type/property references, namespaces), §6.5 (value types).

Build, in server-rust/semantic-tree/:
- `src/value.rs`: a `Value` enum matching §6.5 exactly (null, bool, signed int, unsigned int,
  float64, string, node_id, item_id, resource_hash, enum token, size/range/point-like tuples,
  list of scalars, small typed record). No blobs — large binary content is a Resource, not a
  Value (per §6.5's closing line).
- `src/ids.rs`: newtypes for `NodeId`, `ItemId`, `ResourceHash` (SHA-256-shaped), plus
  `TypeRef { namespace_id: u32, local_id: u32 }` and `PropertyRef { namespace_id: u32, local_id: u32 }`
  per §6.4. Include a small lookup helper that resolves a standard (namespace 0) TypeRef/PropertyRef
  against the values checked into protocol/registry.yaml (load it at build time or embed a
  generated constant table — your choice, document it).

Out of scope: no SemanticStore, no transactions, no mutation operations yet. This is types only.

Verification:
- `cargo test` in semantic-tree: unit tests constructing and comparing each Value variant; a
  test that TypeRef/PropertyRef round-trip through equality/hashing correctly; a test that
  resolving a known registry.yaml entry (e.g. the `Button` node type or the `label` property)
  returns the expected numeric ID, and that an unknown name fails clearly rather than panicking.
- Commit the result.
```

### Task 4 — Rust: SemanticStore mutation primitives — Effort: Medium

```
You are implementing one bounded task in a larger project called SRUI (Semantic Remote UI). The
authoritative design document is attached: `SRUI_Semantic_Remote_UI_Design_v0.6.md`. Treat it as
read-only and authoritative — do not edit it. This is Task 4 of a sequential implementation plan
(Tasks 0–38). Task 3 built the Value/TypeRef/PropertyRef/NodeId types in server-rust/semantic-tree
— inspect it before starting; build directly on those types, do not redefine them.

Read: §6.2 (node identity rules), §6.3 (state ownership table — this store is the *authoritative*
side), §13 (CREATE_NODE, DELETE_NODE, SET_PROPERTY, CLEAR_PROPERTY, MOVE_NODE, REORDER_CHILDREN),
§26 (mandatory limits: max tree depth, max node count, max string length — enforce these here as
configurable constants, even if the actual numbers are placeholders for now).

Build, in server-rust/semantic-tree/src/store.rs:
- A mutable `SemanticStore` holding a node graph: node_id -> {type, parent_id, ordered
  children, properties map}.
- Direct (non-transactional) application of each mutation op from §13's "required" list plus
  MOVE_NODE and REORDER_CHILDREN. This layer is the low-level primitive that Task 5's
  transaction wrapper will call — it does not itself need to be atomic across multiple ops yet.
- Enforce invariants: node_id must not already exist on CREATE_NODE and must never be reused
  later in the store's lifetime (§6.2) even after DELETE_NODE; parent must exist (or be null only
  for a designated root) before a node can be attached under it; configurable max tree depth and
  max node count from §26, returning a typed error rather than panicking when exceeded.

Out of scope: no transaction/revision wrapper (Task 5), no model/collection data (Task 6), no
events, no networking, no Standard Widget typed API — this is the generic node-graph primitive
only.

Verification (`cargo test`):
- create/delete/reorder/move a handful of nodes and assert the resulting tree shape;
- creating a node with an already-used (including previously deleted) node_id is rejected;
- creating a node under a nonexistent parent is rejected;
- exceeding the configured max depth or max node count is rejected with a clear error, and the
  store is left unchanged by the rejected operation;
- Commit the result.
```

### Task 5 — Rust: Transactions and revisions — Effort: Medium

```
You are implementing one bounded task in a larger project called SRUI (Semantic Remote UI). The
authoritative design document is attached: `SRUI_Semantic_Remote_UI_Design_v0.6.md`. Treat it as
read-only and authoritative — do not edit it. This is Task 5 of a sequential implementation plan
(Tasks 0–38). Task 4 built the raw SemanticStore mutation primitives (create/delete/set/move/
reorder) in server-rust/semantic-tree/src/store.rs — inspect it before starting; wrap it, don't
replace it.

Read: §12 (persistent object graph, no full-document resends), §12.1 (revisions/transactions,
all-or-nothing, base/new revision), §12.2 (commits are not frames — this is a state-consistency
boundary, not a render cue; nothing in your implementation should imply pacing or frame
semantics), §26 (max transaction operations limit).

Build, in server-rust/semantic-tree/src/transaction.rs:
- `SemanticStore::apply_transaction(base_revision, ops: Vec<Operation>) -> Result<Revision, TxnError>`
  where `Operation` wraps Task 4's mutation primitives. Semantics required by §12.1:
  - reject if `base_revision` doesn't match the store's current committed revision;
  - apply all ops; if any op fails, the entire transaction is discarded and the store is left
    exactly as it was before the call (no partial application ever observable);
  - on full success, atomically advance to `new_revision = base_revision + 1` (or the caller-
    supplied new_revision, validated to be exactly one more — decide and document which);
  - enforce the §26 max-operations-per-transaction limit as a pre-check before applying anything.
- Revision is monotonically increasing and never decreases or repeats within a session.

Out of scope: no model/collection ops yet (Task 6), no events, no journal/replay (that's Task
22 — this task's revision counter is just in-memory, not yet persisted for reconnect), no
networking.

Verification (`cargo test`):
- a valid transaction with several ops commits and advances the revision by exactly 1;
- a transaction where the *last* op is invalid (e.g. references a nonexistent node) results in
  zero visible change to the store and the revision does not advance;
- a transaction submitted with a stale/wrong base_revision is rejected without side effects;
- a transaction exceeding the max-operations limit is rejected before any op is applied;
- Commit the result.
```

### Task 6 — Rust: Collection/model data — Effort: Medium

```
You are implementing one bounded task in a larger project called SRUI (Semantic Remote UI). The
authoritative design document is attached: `SRUI_Semantic_Remote_UI_Design_v0.6.md`. Treat it as
read-only and authoritative — do not edit it. This is Task 6 of a sequential implementation plan
(Tasks 0–38). Task 5 added atomic transactions/revisions on top of Task 4's store — inspect both
before starting.

Read: §8 (collections and model data — stable item_id, insert/delete/update by identity, cached
ranges, item_count vs. cached ranges distinction), §13 (CREATE_MODEL, MODEL_INSERT, MODEL_DELETE,
MODEL_UPDATE, MODEL_RESET_RANGE).

Build, in server-rust/semantic-tree/src/model.rs:
- A `Model` type: `item_count` (may be far larger than what's actually cached), a sparse
  representation of cached item ranges keyed by stable `ItemId`, and the four model operations
  from §13. A `List`/`Table`/`Tree` node references a model by id (per §8's example) rather than
  having one child node per row.
- Wire `CREATE_MODEL`/`MODEL_INSERT`/`MODEL_DELETE`/`MODEL_UPDATE`/`MODEL_RESET_RANGE` into
  Task 5's transaction op set so they participate in the same atomic commit/rollback semantics.

Out of scope: no networking, no actual async range-fetch protocol yet (that's part of Task 27
end-to-end; here you're just building the correct in-memory data structure and its transactional
mutation ops), no renderer/virtualization.

Verification (`cargo test`):
- create a model with a large item_count but only a few cached items; insert/update/delete by
  item_id and confirm only the addressed item changes;
- MODEL_RESET_RANGE replaces a range's cached items without touching item_count or other ranges;
- a transaction combining a MODEL_INSERT with an invalid node-graph op elsewhere fails and rolls
  back the model change too (atomicity spans both node ops and model ops in one transaction);
- Commit the result.
```

### Task 7 — Rust: Events and capability records — Effort: Low

```
You are implementing one bounded task in a larger project called SRUI (Semantic Remote UI). The
authoritative design document is attached: `SRUI_Semantic_Remote_UI_Design_v0.6.md`. Treat it as
read-only and authoritative — do not edit it. This is Task 7 of a sequential implementation plan
(Tasks 0–38). Tasks 3–6 built the value/id types, store, transactions, and model data in
server-rust/semantic-tree — inspect them before starting.

Read: §6.1 (core object list: Event, Capability), §7.6 and §7.7 (standard events, semantic input
routing, the Event shape with event_seq/event_id/observed_revision/node_id/type), §15
(capability negotiation shape — HELLO/WELCOME structure, required vs. optional profile lists).

Build, in server-rust/semantic-tree/:
- `src/event.rs`: an `Event` struct matching §7.7's shape (event_seq, event_id, observed_revision,
  node_id, event_type as a TypeRef-like reference, arguments as a small property map). No I/O —
  this is just the data shape plus basic validation helpers (e.g. "does this event's node_id
  currently exist in the store").
- `src/capability.rs`: a `CapabilitySet` type (profile identifier + version, e.g.
  `org.srui.standard-widgets/1`) with matching logic: given a client's offered profiles and a
  server's required/optional profiles, compute the negotiated set and detect an unsatisfiable
  required profile (§4 invariant 13: unknown required semantics must fail explicitly).

Out of scope: no wire encoding/decoding of these (Task 11 does Rust-side wire encoding), no
actual handshake over a transport (Task 20), no event deduplication yet (Task 23) — this task is
in-memory data shapes and pure negotiation-matching logic only.

Verification (`cargo test`):
- construct events and validate node_id existence checks against a populated store;
- capability negotiation: matching sets succeed; a required profile missing from the client's
  offered list is reported as a hard failure, not silently ignored; an optional profile missing
  is fine and simply absent from the negotiated set;
- Commit the result.
```

### Task 8 — Rust: first conformance harness (Core state-machine + semantic-not-paint) — Effort: Medium

```
You are implementing one bounded task in a larger project called SRUI (Semantic Remote UI). The
authoritative design document is attached: `SRUI_Semantic_Remote_UI_Design_v0.6.md`. Treat it as
read-only and authoritative — do not edit it. This is Task 8 of a sequential implementation plan
(Tasks 0–38). Tasks 3–7 built the full in-memory Rust Core (values, store, transactions, models,
events, capabilities) — inspect them before starting.

Read: §32 items 1 and 3 only ("Core state-machine tests" and "Semantic-not-paint tests"), and
re-skim §4 (architectural invariants) since the state-machine tests should encode those
invariants as data, not just as scattered unit tests.

Build:
- Under protocol/conformance-vectors/, author a set of human-readable fixture files (JSON or a
  simple text format — your choice, but document the format in a short README in that
  directory) describing: a sequence of operations plus expected outcome (commit with resulting
  state, or rejection with reason). Cover at least: valid multi-op transaction, reused node_id,
  orphan parent, mid-transaction failure/rollback, stale base_revision, exceeding node/depth/
  op-count limits, valid model operations, invalid model operation referencing unknown item_id.
- Also include fixtures whose *point* is "semantic-not-paint": assert that nothing in the
  fixture format or the store's public API has any notion of frames, paint commands, or
  mandatory absolute pixel geometry for standard widgets (this can be a written assertion/test
  plus a short note in the fixture README rather than a runtime check, since it's really a
  schema-shape property).
- A Rust test runner (in server-rust, a new integration-test file is fine) that loads every
  fixture and replays it against the Task 3–7 store, asserting the documented outcome.

Out of scope: don't build a Swift-side runner yet (Task 14 starts reusing these fixtures from
Swift). Don't implement the other 10 conformance suites from §32 yet (those land incrementally
through later tasks and get consolidated in Task 32).

Verification:
- `cargo test` runs the fixture-driven suite and all fixtures pass against the current
  implementation;
- the fixture format and directory are documented clearly enough that a different language's
  test runner (Swift, later) could consume the same files without re-deriving the scenarios;
- Commit the result.
```

---

## Phase C — Rust Standard Widgets + server SDK

### Task 9 — Rust: Standard Widget Profile (required tier) — Effort: Medium

```
You are implementing one bounded task in a larger project called SRUI (Semantic Remote UI). The
authoritative design document is attached: `SRUI_Semantic_Remote_UI_Design_v0.6.md`. Treat it as
read-only and authoritative — do not edit it. This is Task 9 of a sequential implementation plan
(Tasks 0–38). Tasks 3–8 built the generic Rust Core (store/transactions/models/events) — inspect
it before starting; this task adds a typed layer on top, it does not touch the generic store's
internals.

Read: §7.1 (semantic control, local appearance rule), §7.2 (portable control vocabulary — table),
§7.3 (required v0.1 implementation tier — this task implements exactly this list, not the SHOULD
tier), §7.4 (common properties by category), §7.5 (appearance roles), the checkbox/switch
sub-section of §7.2 ("Checkbox and switch are one semantic state machine").

Build, as a new crate or module server-rust/sdk/src/widgets.rs (your choice where it lives, but
it must sit on top of Task 4–7's generic store/transaction API, not duplicate it):
- Typed builder/accessor helpers for exactly the §7.3 required tier: Surface, Row, Column, Grid,
  Spacer, Separator, Text, RichText, Button, Toggle, TextInput, TextArea, Progress, Image,
  Scroll, List, Table, Tree. Each should expose the relevant §7.4 properties as typed
  getters/setters (e.g. `Button::set_label`, `Button::set_role`, `Toggle::set_value`,
  `Toggle::set_presentation_hint`) that internally call the generic `SET_PROPERTY`/`CREATE_NODE`
  machinery with the correct TypeRef/PropertyRef from protocol/registry.yaml.
- Implement Toggle with a single `value: bool` plus `presentation_hint` enum
  (checkbox|switch|automatic) exactly as specified — do not create separate Checkbox/Switch
  node types at the store level.
- Appearance roles (TextRole, ActionRole, InputRole, Importance) as typed enums settable on the
  relevant nodes.

Out of scope: do not implement Select, ChoiceGroup, Slider, NumberInput, Tabs, Split, Menu, or
Toolbar yet (SHOULD tier / deferred — add them in a later pass once the required tier is proven,
or note them as a fast-follow if you have spare capacity, but the required tier is what gets
verified here). No rendering, no networking, no server SDK ergonomics yet (Task 10).

Verification (`cargo test`):
- for each required-tier node type, construct one via the typed API, set its documented
  properties, and read them back through the *generic* store API to confirm the typed layer is
  a thin, correct wrapper (no divergent state);
- a Toggle round-trips both presentation_hint values and confirm no separate "Checkbox" node
  type was introduced anywhere in the registry or code;
- Commit the result.
```

### Task 10 — Rust: reference server SDK (in-process, no transport) — Effort: Medium

```
You are implementing one bounded task in a larger project called SRUI (Semantic Remote UI). The
authoritative design document is attached: `SRUI_Semantic_Remote_UI_Design_v0.6.md`. Treat it as
read-only and authoritative — do not edit it. This is Task 10 of a sequential implementation plan
(Tasks 0–38). Task 9 added the typed Standard Widget layer over the Task 3–8 Rust Core — inspect
it before starting.

Read: §29 (reference server SDK contract, including the `session.transaction(|ui| {...})` and
`session.on(node, EVENT, handler)` example).

Build, in server-rust/sdk/:
- An ergonomic `Session` API matching §29's shape: `session.transaction(|ui| { ... })` that
  internally opens a Task 5 transaction, lets the closure call Task 9's typed widget mutators,
  and commits (or rolls back on error) when the closure returns — the caller never sees
  base/new revision bookkeeping directly.
- `session.on(node_ref, EventType, handler)` registering an in-process handler invoked when a
  matching Task 7 `Event` is delivered to the session (delivery here is a direct in-process call
  — there is no transport yet, so "delivered" just means your own test code calls a
  `session.dispatch(event)` method).
- In examples/counter/: a minimal example program using only this SDK — a Surface containing a
  Text (or Progress) showing a counter value and a Button; clicking (via `dispatch`) an
  ACTIVATE event increments the counter through `session.transaction`.

Out of scope: no sockets, no SSH, no journal, no resources — everything here is in-process
function calls. This task proves the SDK ergonomics work, not the transport.

Verification (`cargo test` plus running examples/counter as a test, not a manual demo):
- a test drives the counter example by calling `session.dispatch(ACTIVATE on the button)`
  several times and asserts the resulting committed Text/Progress value matches expectations
  after each dispatch;
- a test confirms a panicking/erroring handler doesn't leave the store in a partially-committed
  state (the surrounding transaction still obeys Task 5's atomicity guarantee);
- Commit the result.
```

### Task 11 — Rust: wire encode/decode for Core + Standard Widgets — Effort: Medium

```
You are implementing one bounded task in a larger project called SRUI (Semantic Remote UI). The
authoritative design document is attached: `SRUI_Semantic_Remote_UI_Design_v0.6.md`. Treat it as
read-only and authoritative — do not edit it. This is Task 11 of a sequential implementation plan
(Tasks 0–38). Task 2 generated Rust protobuf bindings from protocol/srui.proto; Tasks 3–10 built
the in-memory Core, widgets, and SDK. Inspect all of it before starting — this task is the bridge
between them.

Read: §16 (reference wire encoding) again, focused this time on how NodeRecord/Transaction/Event/
Value map to your Task 3–7 in-memory types.

Build, in server-rust (a new module, e.g. semantic-tree/src/wire.rs or a dedicated crate):
- Functions converting: a committed Transaction (Task 5's op list + revisions) <-> the protobuf
  `Transaction`/`Operation` messages from Task 2; a `Value` (Task 3) <-> protobuf `Value`; an
  `Event` (Task 7) <-> protobuf `Event`. Round-trip in both directions.

Out of scope: no actual byte stream I/O over a socket yet (Task 17). This is pure in-memory
serialize/deserialize.

Verification (`cargo test`):
- drive the Task 10 counter example through a few transactions, serialize each committed
  transaction to protobuf bytes, deserialize it back, and confirm replaying the deserialized
  ops against a fresh store produces the same resulting state as the original;
- decode the Task 2 golden fixture bytes through this new path and confirm it produces the
  expected in-memory NodeRecord/Transaction structure (tie this back explicitly to Task 2's
  golden fixture rather than inventing a new one);
- Commit the result.
```

---

## Phase D — Swift Protocol Core + fixture-driven renderer

### Task 12 — Swift: Value and identity types — Effort: Low

```
You are implementing one bounded task in a larger project called SRUI (Semantic Remote UI). The
authoritative design document is attached: `SRUI_Semantic_Remote_UI_Design_v0.6.md`. Treat it as
read-only and authoritative — do not edit it. This is Task 12 of a sequential implementation plan
(Tasks 0–38). Task 0 created the client-macos SwiftPM scaffold with a SemanticModel target among
others — inspect it before starting. This task mirrors Task 3 (Rust Value/id types) in Swift; the
two are meant to be semantically equivalent, not the same code.

Read: §1 ("This separation is a normative requirement... a future Windows, GTK, or SWT client
must be able to implement SRUI without depending on any macOS concept" — this is why SemanticModel
must never import AppKit), §6.2, §6.4, §6.5 (same sections as Task 3).

Build, in client-macos/SemanticModel/:
- `Value.swift`: an enum/indirect-enum matching §6.5's value set.
- `Ids.swift`: `NodeId`, `ItemId`, `ResourceHash`, `TypeRef`, `PropertyRef` matching §6.4,
  plus a resolver against protocol/registry.yaml (embed a generated Swift constant table via a
  build-time script, or load the YAML at runtime — document your choice; it should be easy to
  keep in sync with the Rust side's Task 3 resolver since both read the same registry.yaml).

Out of scope: no store, no transactions, no rendering, no networking. Absolutely no `import
AppKit`, `import Cocoa`, or any AppKit type anywhere in the SemanticModel target — that's a hard
rule for the whole project.

Verification:
- `swift test` for a new SemanticModel test target covering the same cases as Task 3's Rust
  tests (construct/compare each Value variant, TypeRef/PropertyRef equality, known/unknown
  registry lookups);
- a scripted check (e.g. `grep -r "import AppKit" client-macos/SemanticModel` or a build-setting
  equivalent) confirms zero AppKit imports in this target, and wire this check into CI so a
  later task can't accidentally violate it;
- Commit the result.
```

### Task 13 — Swift: client-side SemanticStore (non-authoritative replica) — Effort: Medium

```
You are implementing one bounded task in a larger project called SRUI (Semantic Remote UI). The
authoritative design document is attached: `SRUI_Semantic_Remote_UI_Design_v0.6.md`. Treat it as
read-only and authoritative — do not edit it. This is Task 13 of a sequential implementation plan
(Tasks 0–38). Task 12 built the Swift Value/Ids types in client-macos/SemanticModel — inspect it
before starting.

Read: §6.2 (node identity), §6.3 (state ownership — this store is the *non-authoritative
replica* side of that table, unlike Task 4's Rust store which was authoritative), §2 architectural
invariant list in §4, specifically invariants 2, 3, and 11 (client replica exists only to render/
interact/inspect; presentation-only state is local and separate; a client can render the first
valid committed subtree before the whole UI arrives).

Build, in client-macos/SemanticModel/SemanticStore.swift:
- A `SemanticStore` mirroring Task 4's Rust store shape (node graph: id -> type/parent/children/
  properties), but explicitly documented as a *replica* — nothing here computes authoritative
  outcomes, it only applies operations it's told to apply.
- Same low-level mutation primitives as Task 4 (create/delete/set/clear/move/reorder), same
  invariant enforcement (no node_id reuse, parent must exist, depth/count limits) so a malformed
  or malicious mutation stream can't corrupt the client either.
- Expose the store as read-only to any external consumer except through a narrow "apply" entry
  point (this anticipates Task 22's TransactionApplier wrapping it, but don't build that yet —
  just make sure nothing outside this file can mutate nodes directly).

Out of scope: no transaction/revision wrapper yet (Task 14), no networking/decoding (Task 15),
no rendering (Task 16).

Verification (`swift test`):
- mirror Task 4's test cases (create/delete/move/reorder; reused id rejected; orphan parent
  rejected; depth/count limits enforced);
- a test that applies a list of mutation ops with a deliberately invalid op injected partway
  through, asserting the store is left in exactly its pre-call state (no operation before the
  failure point is left "half visible" outside an atomic wrapper) — note that true transaction
  atomicity is Task 14's job, but this task's primitives must not corrupt state even when misused
  directly;
- Commit the result.
```

### Task 14 — Swift: transactions, revisions, and model data — Effort: Medium

```
You are implementing one bounded task in a larger project called SRUI (Semantic Remote UI). The
authoritative design document is attached: `SRUI_Semantic_Remote_UI_Design_v0.6.md`. Treat it as
read-only and authoritative — do not edit it. This is Task 14 of a sequential implementation plan
(Tasks 0–38). Task 13 built the Swift SemanticStore replica primitives — inspect it before
starting. This task mirrors Rust Tasks 5 and 6 on the client side.

Read: §12.1 (transactions/revisions — same atomicity rules as the server side), §12.2 (commits
are not frames — make sure nothing in your API design implies frame pacing), §8 (collections/
model data), §13 (model operations), §18 ("last_applied_revision" — just track this counter for
now; full reconnect logic is Task 22).

Build, in client-macos/SemanticModel/:
- `TransactionApplier.swift`: the only way to mutate the SemanticStore from outside this module.
  `apply(baseRevision:operations:) -> Result<Revision, TxnError>` with the same all-or-nothing
  semantics as Task 5, tracking `lastAppliedRevision`.
- `Model.swift`: mirrors Task 6's Rust Model type and its four operations, wired into the same
  transactional apply path.
- Bring over the Task 8 conformance fixtures (protocol/conformance-vectors/) and write a Swift
  test runner that replays them against this Swift implementation, exactly as Task 8's Rust
  runner does. This is the first real cross-language conformance proof — both language
  implementations must agree on every fixture's expected outcome.

Out of scope: no wire decode yet (raw bytes) — fixtures here are applied as already-parsed
operations, matching how Task 8's fixtures work. No rendering, no networking, no events.

Verification (`swift test`):
- mirror Task 5/6's Rust test cases;
- run the full Task 8 fixture suite through this Swift runner; every fixture must produce the
  same pass/fail verdict as it does in Rust (call out and resolve any mismatch — a mismatch here
  means either the fixture or one implementation is wrong, and you must fix rather than special-
  case it away);
- Commit the result.
```

### Task 15 — Swift: wire decode with validation limits — Effort: Medium

```
You are implementing one bounded task in a larger project called SRUI (Semantic Remote UI). The
authoritative design document is attached: `SRUI_Semantic_Remote_UI_Design_v0.6.md`. Treat it as
read-only and authoritative — do not edit it. This is Task 15 of a sequential implementation plan
(Tasks 0–38). Task 2 generated SwiftProtobuf bindings; Tasks 12–14 built the Swift Core store/
transactions/model. Inspect all of it before starting.

Read: §16 (wire encoding) again from the client's perspective, §22.2 (threading model — network
IO and decoding happen off the main thread/actor; you don't need real networking yet, but design
the decode path so it's easy to run off-main later), §26 (mandatory client-side limits: max frame
size, max transaction operations, max tree depth, max node count, max string length, max model/
item count per message — enforce all of these at decode time, failing closed on violation).

Build, in client-macos/Protocol/:
- Decode functions: protobuf bytes -> Task 5-shaped Transaction/Operation list, protobuf Event
  bytes -> Task 7-shaped Event (if you haven't ported an Event type to Swift yet, add a minimal
  one here matching the Rust shape), feeding Task 14's TransactionApplier.
- Enforce the §26 limits during decode, before anything touches the store: oversized frames,
  too many ops in one transaction, strings over the length cap, etc. must be rejected with a
  clear typed error and must not partially apply.

Out of scope: no actual socket/transport (Task 18), no threading/actor isolation yet (that's
part of Task 18's real integration) — but structure the code so adding it later doesn't require
a rewrite.

Verification (`swift test`):
- decode the Task 2 golden fixture bytes and confirm the reconstructed Transaction matches what
  Task 11's Rust-side round-trip test expects for the same bytes (cross-language agreement on
  the actual wire format, not just on fixtures expressed as pre-parsed ops);
- feed deliberately oversized/malformed fixtures (add a few new ones under protocol/
  conformance-vectors/malformed/ if none exist yet) and confirm clean rejection with no store
  mutation and no crash;
- Commit the result.
```

### Task 16 — Swift: AppKit renderer for the required tier, driven by fixtures — Effort: High

```
You are implementing one bounded task in a larger project called SRUI (Semantic Remote UI). The
authoritative design document is attached: `SRUI_Semantic_Remote_UI_Design_v0.6.md`. Treat it as
read-only and authoritative — do not edit it. This is Task 16 of a sequential implementation plan
(Tasks 0–38). Tasks 12–15 built the Swift Core store/transactions/decode — inspect them before
starting. This is the first task that touches AppKit; it is the ONLY task so far allowed to
import AppKit, and only inside the RendererAppKit target, never in SemanticModel/Protocol.

Read: §22 (macOS rendering architecture overview), §22.3 (RenderHandle), §22.4 (AppKit mapping
table — implement mappings for exactly the §7.3 required tier: Surface, Row/Column/Grid, Spacer/
Separator, Text/RichText, Button, Toggle, TextInput/TextArea, Progress, Image, Scroll, List/
Table, Tree), §22.5 (local interaction state — hover/pressed/focus/etc. are local AppKit
behavior, not driven by the protocol), §23 (dirty-change classification and performance targets
— implement at least a basic version of the classification, full optimization can come later).

Build, in client-macos/RendererAppKit/:
- `RenderRegistry`: node_id -> RenderHandle (AppKit view + minimal layout/accessibility
  metadata), per §22.3. The SemanticModel/Protocol targets must remain untouched by this task
  except as a dependency you read from.
- `ControlFactory` + `LayoutRenderer` mapping each required-tier node type to the AppKit
  control(s) from §22.4's table (NSStackView-based layout is fine per §22.4's note that the
  mapping may change later without a protocol change).
- A tiny SwiftPM executable (or a test-host app target) that loads a canned sequence of Task 14-
  shaped transactions (hand-written fixtures, not from a live server) and displays the resulting
  UI in a real NSWindow, for manual visual verification.
- Basic dirty-node classification per §23 (at minimum: a scalar SET_PROPERTY on an existing
  control updates just that control's relevant AppKit property, it does not rebuild the tree).

Out of scope: no networking, no live server, no SHOULD-tier widgets (Select, ChoiceGroup,
Slider, NumberInput, Tabs, Split), no Menu/Toolbar, no accessibility bridge beyond whatever
AppKit gives you for free by using real controls, no text editing semantics beyond what a plain
NSTextField gives you out of the box (real edit_seq semantics are Task 28).

Verification:
- manual run of the demo executable: confirm each required-tier node type renders as something
  reasonable and a scalar property change (fed as a second fixture transaction) updates the
  already-rendered control in place rather than rebuilding the window;
- automated `swift test` for RenderRegistry's node_id -> handle bookkeeping and for the dirty-
  classification logic on a scalar SET_PROPERTY vs. a structural CREATE/DELETE;
- Commit the result.
```

---

## Phase E — Local end-to-end loop

### Task 17 — Rust: local sessiond + Unix-socket bridge — Effort: Medium

```
You are implementing one bounded task in a larger project called SRUI (Semantic Remote UI). The
authoritative design document is attached: `SRUI_Semantic_Remote_UI_Design_v0.6.md`. Treat it as
read-only and authoritative — do not edit it. This is Task 17 of a sequential implementation plan
(Tasks 0–38). Tasks 3–11 built the full in-memory Rust Core/widgets/SDK/wire-encode — inspect
them before starting.

Read: §20 (reference server architecture), §20.1–§20.2 (ssh-bridge and sessiond
responsibilities — note §20.2's explicit allowance: "For a very early demo, the bridge and
session daemon may be one process. Reconnect conformance is not achieved until state ownership
is moved out of the transient SSH process." — that's exactly this task's scope; real separation
comes in Task 21), §16 (length-prefixed envelope framing for the wire encoding).

Build:
- server-rust/sessiond: host a Task 10-style `Session` behind a local Unix domain socket
  (or TCP loopback if that's easier to test on your platform — document the choice), framing
  messages using the length-prefixed envelope approach implied by §16, and using Task 11's
  encode/decode for Transaction/Event bytes on the wire.
- server-rust/ssh-bridge: for this task, a minimal stand-in that just proxies raw bytes between
  stdin/stdout and the sessiond socket — no actual SSH yet (that's Task 19). Its only job here is
  to exist as a separate process so later tasks can slot real SSH in without restructuring.

Out of scope: no real SSH, no reconnect/journal, no event dedup, no resources — a single client
connects once and stays connected for the test's duration.

Verification (an integration test in server-rust, not a manual demo):
- spawn sessiond, connect to its socket directly from the test (bypassing the stdin/stdout
  bridge, to keep the test simple), drive the Task 10 counter example through a few ACTIVATE
  events sent as framed protobuf bytes, and assert the test harness receives the expected framed
  Transaction bytes back, matching what Task 11's in-memory encode would produce for the same
  sequence of actions;
- Commit the result.
```

### Task 18 — Swift + Rust: first real end-to-end demo (local loopback) — Effort: Medium

```
You are implementing one bounded task in a larger project called SRUI (Semantic Remote UI). The
authoritative design document is attached: `SRUI_Semantic_Remote_UI_Design_v0.6.md`. Treat it as
read-only and authoritative — do not edit it. This is Task 18 of a sequential implementation plan
(Tasks 0–38). Task 16 built the fixture-driven Swift AppKit renderer; Task 17 built the Rust
local sessiond + socket bridge. Inspect both before starting — this task wires them together for
the first time.

Read: §22 (macOS rendering architecture diagram — SSHTransport/FrameDecoder/SessionController/
SemanticStore/TransactionApplier/RendererAppKit/EventOutbox), §22.2 (threading — decode off-main,
apply serially, AppKit mutation on MainActor), §7.7 (semantic input routing / ACTIVATE event
shape for the EventOutbox side).

Build, in client-macos/:
- A transport adapter (this task's version connects over the same local socket Task 17's
  sessiond exposes — not real SSH yet, that's Task 19) that reads framed bytes, hands them to
  Task 15's decoder, applies them via Task 14's TransactionApplier, and triggers Task 16's
  renderer to update.
- `EventOutbox`: sends an `ACTIVATE` event (with event_seq/event_id per §7.7) when the demo's
  Button is clicked in the real AppKit window.
- Wire the threading split from §22.2: socket IO + decode off the main thread, store apply
  serialized, AppKit mutations dispatched to the main thread/MainActor, and the renderer never
  observes a half-committed transaction (this should already hold from Task 14's atomicity, but
  verify it end-to-end here under real concurrency).

Out of scope: no real SSH (Task 19), no capability negotiation handshake yet (Task 20 — for now
assume both sides just agree on the required profile out of band), no reconnect.

Verification:
- run Task 17's counter server and this Swift client together locally; click the Button in the
  real AppKit window and confirm the server-side counter increments and the resulting
  SET_PROPERTY change is reflected live in the same window, with no shell or PTY involved
  anywhere in the path;
- an automated integration test (can be a script that launches both processes and asserts on
  log/state output, since a full UI-automation test is heavier than this task needs) covering
  at least 3 consecutive click-and-observe cycles;
- Commit the result.
```

### Task 19 — Real SSH transport binding — Effort: High

```
You are implementing one bounded task in a larger project called SRUI (Semantic Remote UI). The
authoritative design document is attached: `SRUI_Semantic_Remote_UI_Design_v0.6.md`. Treat it as
read-only and authoritative — do not edit it. This is Task 19 of a sequential implementation plan
(Tasks 0–38). Task 18 got a full local-loopback demo working over a plain socket — inspect it
before starting. This task replaces the loopback transport with real SSH without touching
anything above the transport layer (decoder, store, renderer, EventOutbox all stay as-is).

Read: §19 (SSH transport binding) and §19.1 (recommended SSH posture) in full — this is the
normative checklist for this task.

Build:
- Server side: wire server-rust/ssh-bridge to be launched as an OpenSSH subsystem (add the
  `sshd_config` snippet as documentation/example, e.g. `Subsystem srui /path/to/srui-ssh-bridge`)
  instead of being invoked directly; it still proxies to sessiond exactly as in Task 17, just now
  reached through sshd rather than a bare pipe.
- Client side: replace client-macos's Task 18 loopback transport with one that establishes a
  real SSH connection (spawning the system `ssh` binary requesting the `srui` subsystem with no
  PTY, or a vetted SSH library — document and justify your choice) and follows every rule in
  §19.1: no PTY for this channel, normal host-key/`known_hosts` verification, no X11 forwarding,
  no agent forwarding by default, no ad hoc port forwards, a fixed subsystem/executable rather
  than a shell-interpolated command string, stderr handled separately from the binary protocol
  where the SSH library allows it, fail closed on host-key changes.

Out of scope: no capability negotiation handshake yet (Task 20), no reconnect journal (Task 22).

Verification:
- set up a local `sshd` (or reuse an existing dev one) with the srui subsystem configured; run
  the Task 18 counter demo end-to-end through `ssh -s srui` (or your library's equivalent) instead
  of the loopback socket, and confirm identical behavior to Task 18's demo;
- write a test or a documented manual check that inspects the actual ssh invocation/config used
  by the client and confirms none of §19.1's disallowed features (PTY, X11 forwarding, agent
  forwarding, ad hoc port forwards, shell-interpolated commands) are present;
- a deliberate bad-host-key test confirms the client fails closed rather than silently
  proceeding;
- Commit the result.
```

### Task 20 — Capability negotiation handshake — Effort: Low

```
You are implementing one bounded task in a larger project called SRUI (Semantic Remote UI). The
authoritative design document is attached: `SRUI_Semantic_Remote_UI_Design_v0.6.md`. Treat it as
read-only and authoritative — do not edit it. This is Task 20 of a sequential implementation plan
(Tasks 0–38). Task 19 made the transport real SSH; Task 7 already built the Rust-side
CapabilitySet negotiation logic (in-process only, no wire I/O). Inspect both before starting.

Read: §15 (capability negotiation) in full, §4 invariant 13 (unknown required semantics fail
explicitly; optional semantics are negotiated or have documented fallbacks).

Build:
- Wire §15's HELLO/WELCOME handshake onto the real transport from Task 19, using the
  ClientHello/ServerWelcome messages from Task 2's proto schema and Task 7's Rust-side
  negotiation logic (port equivalent matching logic to Swift if it doesn't exist yet). The
  handshake must complete, successfully or not, before any Transaction/Event traffic is
  permitted on the connection.
- Concrete failure path: if the server's required profile list isn't satisfied by the client's
  offered profiles (or vice versa, if you want symmetry), the connection is cleanly closed with
  a clear error surfaced to whichever side is easiest to test against — no partial/undefined
  protocol traffic should be attempted afterward.

Out of scope: no extension-profile fallback subtree logic yet (that's exercised in Task 30), no
reconnect/session-resume handshake yet (Task 22, though it shares this task's message framing
patterns).

Verification:
- end-to-end test: successful handshake with matching required profile (`org.srui.standard-
  widgets/1`), followed by the Task 18/19 counter demo working exactly as before;
- a mismatched-required-profile test (temporarily configure the server to require a profile the
  test client doesn't offer) confirms the connection fails cleanly at handshake time rather than
  failing confusingly later or being silently accepted;
- Commit the result.
```

### Task 21 — Checkpoint demo: live system/process monitor — Effort: Medium

```
You are implementing one bounded task in a larger project called SRUI (Semantic Remote UI). The
authoritative design document is attached: `SRUI_Semantic_Remote_UI_Design_v0.6.md`. Treat it as
read-only and authoritative — do not edit it. This is Task 21 of a sequential implementation plan
(Tasks 0–38). Unlike most tasks in this plan, this one is a demo/example checkpoint rather than
new protocol infrastructure: everything it needs — the required-tier Standard Widget Profile
(Task 9), the server SDK (Task 10), the fixture-driven AppKit renderer (Task 16), real SSH
transport (Task 19), and capability negotiation (Task 20) — already exists by this point.
This task assembles them into a real, runnable, genuinely useful demo application, filling in
the `examples/process-monitor/` placeholder reserved back in Task 0. It deliberately does NOT
depend on Tasks 22–25 (reconnect journal, event dedup, backpressure), which haven't been built
yet at this point in the sequence — a dropped connection here just means reconnecting via a
plain fresh HELLO/WELCOME handshake and a full resnapshot, which is expected and fine; do not
add any reconnect logic in this task, that is still Task 23's job. Inspect the current
server-rust/sdk, client-macos/RendererAppKit, and the real-SSH-plus-handshake path (Tasks
19–20) before starting.

Read: §7.2/§7.3 (the Progress, Table, Button, Toggle nodes this demo uses — all required tier,
nothing new), §7.6/§7.7 (semantic events — Table SELECTION_CHANGED, Button ACTIVATE, and the
`action_key`-is-data-not-code rule, which this task must actually honor, not just cite), §8
(collections/model data — the process list is a Model, not one child node per process), §12.2
(commits are not frames — the whole point of this demo is to make that principle visible: watch
live numbers update via tiny mutations, not a repainted screen), and re-read §22.9's own
"Services" / "Restart" illustration and the CPU/requests example in §5.2 — this task is a direct,
real implementation of exactly the kind of example the design doc itself uses to explain SRUI.

Build, in examples/process-monitor/ (a new Rust binary crate using the Task 10 SDK):
- Node tree: Surface > Column > [ Row(Text(role=heading, "System Monitor"),
  Progress(id=cpu_progress), Progress(id=mem_progress)), Toggle(id=show_all, label="Show all
  processes", presentation_hint=switch), Table(id=process_table, model=<process model>,
  columns=[pid, name, cpu%, mem], selection_mode=single), Row(Button(id=kill_button,
  label="Kill Selected", role=destructive)) ].
- Use a system-info crate (e.g. `sysinfo`) to poll CPU%, memory%, and the process list roughly
  once per second. On each tick, diff the new snapshot against the previous one inside one
  `session.transaction` call and emit only the operations for what actually changed (SET_PROPERTY
  on the two Progress nodes; MODEL_INSERT/MODEL_UPDATE/MODEL_DELETE on the process model for
  processes that started, changed, or exited) — do not resend the whole tree or the whole model
  every tick.
- `session.on(process_table, SELECTION_CHANGED, ...)`: record which process item_id is currently
  selected (server-side state, not client-trusted).
- `session.on(kill_button, ACTIVATE, ...)`: look up the pid behind the currently-selected item_id
  (server-side, from the same enumeration that populated the model) and terminate that specific
  process directly via the OS process API (e.g. sending SIGTERM to that numeric pid). No shell
  string is ever constructed from client input or crosses the wire — this is a live instance of
  the §7.7 rule that `action_key`/activation is data, not an executable command.
- Add one safety guardrail: refuse to act on a small denylist (at minimum pid 1, and the
  `sessiond`/example process's own pid) so a careless demo run can't take down the host or
  itself; treat an attempt to select/kill one of these as a no-op with a clear log message, not a
  crash.
- Toggle `show_all` changes what the server includes in the model (e.g. filtering to
  user-owned processes by default); this is server-side filtering, not something the client
  invents locally.

Out of scope:
- No virtualization or async range-fetching (Task 28's territory) — process counts are small
  enough that sending the currently-visible set each tick is fine for this demo; note this
  simplification rather than building range-fetch machinery early.
- No coalescing/backpressure (Task 25's territory) — a ~1 Hz tick rate doesn't need it; don't
  build that logic here.
- No terminal embedding (Task 30's territory) — this demo is pure Standard Widget Profile, no
  PTY involved.
- No confirmation dialog/undo for the Kill action beyond the denylist guardrail above — this is a
  demo, not a production process manager.

Verification:
- run the real macOS client against this server over real SSH (Tasks 19–20) and confirm: the CPU
  and memory Progress bars visibly update in near real time; spawn and then kill a throwaway
  process on the remote host during the test and confirm the Table reflects both the appearance
  and disappearance within a tick or two; select a row and press "Kill Selected" and confirm the
  real remote process is terminated and the Table updates accordingly;
- a wire-traffic sanity check: measure bytes sent per tick during a quiet steady state (no
  process churn) and confirm it's a small, constant handful of messages — not a resend of the
  whole tree or the whole model — this measurement is the actual point of the task, not optional
  polish;
- confirm the denylist guardrail: attempting to select/kill pid 1 or the example/sessiond
  process's own pid is refused rather than executed;
- Commit the result.
```

### Task 22 — Session/connection split: real attach/detach — Effort: Medium

```
You are implementing one bounded task in a larger project called SRUI (Semantic Remote UI). The
authoritative design document is attached: `SRUI_Semantic_Remote_UI_Design_v0.6.md` (Tasks 0–21
were originally written against an earlier v0.4 draft with identical technical content; all task
prompts now reference v0.6 for consistency). Treat it as read-only and authoritative — do not edit it. This is
Task 22 of a sequential implementation plan (Tasks 0–38). Tasks 17–20 got a real SSH connection
with capability negotiation running end to end, but sessiond has only ever served one connection
for the lifetime of a test. Inspect the current server-rust/sessiond before starting.

Read: §17 (session versus connection — this is the section this task implements, including the
v0.6 paragraph on `session_id` as an opaque incarnation token and the restart-reuse rule), §20.2's
closing note again (this task is exactly "moving state ownership out of the transient SSH
process," which §20.2 flags as the point where reconnect conformance actually begins).

Build:
- Make sessiond genuinely persistent across multiple sequential SSH connections: a session has
  a `session_id` and survives an ssh-bridge process exiting (transport lost) while sessiond
  keeps running and keeps its Task 4–11 state (store, revision, running application/example)
  intact. Implement the ATTACHED/DETACHED states from §17 (TERMINATING/EXPIRED can be stubbed
  for now if their triggering policy isn't built yet — note what's stubbed).
  A network failure moves ATTACHED -> DETACHED; it must not terminate the application.
- Treat `session_id` as an opaque, globally unique incarnation token, not a reusable name: this
  task does not build durable cross-process-restart persistence (that remains out of scope, same
  as before), so if sessiond itself exits and a new sessiond process starts, it MUST mint fresh
  session_ids and MUST NOT reuse any ID from a previous run. Do not implement partial state
  recovery across a real restart — per §17, reusing an ID after restoring only *some* of the old
  incarnation's state (semantic tree, journal, event result cache, per-client event frontiers)
  would falsely authorize stale-event replay, which is worse than just minting a new incarnation.
  This precondition is what lets Task 23 tell a live reconnect apart from a replaced incarnation.

Out of scope: no transaction journal/replay yet (Task 23 — for now, a newly attached connection
after a detach can just get a fresh full snapshot, which is allowed per §18 as the fallback
path), no event dedup yet (Task 24), no actual durable state restoration across a sessiond
process restart (only the ID-uniqueness guarantee above is required here).

Verification (integration test):
- start sessiond, attach connection A (real SSH per Task 19-20), run the counter example through
  a few ACTIVATEs, forcibly kill only the ssh-bridge/connection A (not sessiond), start a fresh
  connection B, and confirm the session_id is the same and the counter's current value (i.e. the
  committed semantic state) survived — even though, at this point, connection B may have to
  receive it as a full snapshot rather than a replay;
- restart sessiond itself (simulating a crash, not just a dropped connection) and confirm the new
  process never issues a session_id that collides with one from the previous run, even across
  many restarts in a loop;
- Commit the result.
```

### Task 23 — Transaction journal and reconnect replay (continuity-aware) — Effort: High

```
You are implementing one bounded task in a larger project called SRUI (Semantic Remote UI). The
authoritative design document is attached: `SRUI_Semantic_Remote_UI_Design_v0.6.md`. Treat it as
read-only and authoritative — do not edit it. This is Task 23 of a sequential implementation plan
(Tasks 0–38). Task 22 made sessiond survive a lost connection with state intact and guaranteed
`session_id` is never reused across a sessiond restart. This task implements the full v0.6 §18
resume/resync protocol, which is substantially more detailed than a plain "replay or snapshot"
dichotomy: the server must explicitly tell the client whether it is resuming the *same*
incarnation or being handed a *replacement*, and the client's replay behavior differs sharply
between those two outcomes. Inspect Task 22's session/connection code before starting.

Read: §18 (reconnect and resynchronization) in full — this section was substantially rewritten in
v0.6, so read it fresh even if you implemented an earlier draft of it before; pay particular
attention to: the client MUST NOT replay pending events or generate new ones until it has a
machine-readable continuity decision; the `SERVER RESUME_OK` / `SERVER RESYNC_REQUIRED` message
shapes, including the new `continuity` field (`SAME_SESSION` vs `REPLACED`) and
`last_processed_event_seq`; the exact abandon-vs-replay rules for each outcome; and the
generation-bound resume-attempt supersession rule at the end of §18. Also read §18.1 (journal, if
you haven't already) and the incarnation-restoration paragraph in Appendix B.

Build:
- Extend `protocol/srui.proto` (from Task 2) with a `Continuity` enum (`SAME_SESSION`,
  `REPLACED`) and add `continuity`, an echoed `session_id`, and `last_processed_event_seq` fields
  to the resume-response messages (`ServerResumeOk` / `ServerResyncRequired` from Task 2's
  schema, or a new shared message if that's cleaner — document your choice). Regenerate Rust and
  Swift bindings. This is an incremental extension of the existing schema, not a redo of Task 2.
- A bounded journal of committed transactions in sessiond (server-rust/journal), with a
  configurable retention policy (pick at least one of §18.1's listed policies — e.g. max
  transaction count or max age — and document which).
- Server-side continuity decision on `CLIENT RESUME`: if the requested `session_id` names a
  still-alive incarnation (per Task 22's ID-uniqueness guarantee, an unknown ID can only mean an
  expired/replaced one), respond `SAME_SESSION` — either `RESUME_OK` if the journal still covers
  the gap, or `RESYNC_REQUIRED{continuity=SAME_SESSION, snapshot_revision}` if it doesn't. If the
  ID is unknown, expired, or otherwise not the exact requested incarnation, respond
  `RESYNC_REQUIRED{continuity=REPLACED, session_id=<newly issued id>}`. `RESUME_OK.session_id`
  MUST exactly equal the requested ID.
- Client-side (upgrading Task 18's reconnect stub): on sending `CLIENT RESUME`, retain all
  pending events/edits but do not replay or generate new ones until a response arrives. On
  `RESUME_OK`: apply the reported event frontier, then replay remaining pending events with their
  original `event_id`/`event_seq`, serialized strictly before any newly generated event; apply
  replayed UI transactions in order via Task 14's TransactionApplier. On
  `RESYNC_REQUIRED{SAME_SESSION}`: apply the frontier, may still replay remaining pending events,
  discard the current semantic replica, and apply the snapshot; keep new user-generated events
  disabled until the snapshot commits. On `RESYNC_REQUIRED{REPLACED}`: abandon every unresolved
  event and pending text edit outright — do not replay any of them — reset the event outbox to
  the reported frontier, discard the replica, and apply the fresh snapshot. Treat a missing or
  unrecognized `continuity` value as a hard protocol failure (fail closed per §4 invariant 13),
  never as an implicit "assume same session."
- Generation-bound resume attempts: give each client-initiated resume attempt a local, strictly
  increasing generation number. Starting a new attempt immediately supersedes any earlier attempt
  still in flight. A response that arrives for a superseded attempt (e.g. from a connection the
  client itself has already abandoned in favor of a newer one) MUST be discarded outright — it
  must not replay events, rebind the event outbox, or unblock new event allocation. This matters
  most when a user reconnects twice in quick succession (exactly the case Task 38's connection UI
  will exercise).

Out of scope: the detailed per-event ack/settlement mechanics (`SERVER EVENT_ACK`, IN_FLIGHT vs
SETTLED, sequence windows) are Task 24's job — this task only needs to carry
`last_processed_event_seq` through the resume messages and get the abandon/replay decision right
at the coarse RESUME_OK/SAME_SESSION/REPLACED level; it does not yet implement the fine-grained
ack protocol those decisions ultimately rely on. No pending-text-edit reconciliation yet (Task 29
territory, §18.3). No actual durable state restoration across a sessiond crash (Task 22 already
scoped that out — a sessiond restart always yields `REPLACED` here, by construction).

Verification (integration tests, covering §31.5's boundary list at least partially — full
coverage of all its cases is fine to defer to Task 34's benchmark/reconnect pass, but implement
these core cases now):
- reconnect within journal retention, same incarnation: confirm `RESUME_OK` and that the client
  ends up with exactly the transactions it missed, applied in order, without a snapshot;
- reconnect after journal retention has been exceeded but the same incarnation is still alive:
  confirm `RESYNC_REQUIRED{continuity=SAME_SESSION}`, that the client still correctly replays any
  outstanding pending events after applying the frontier, and then applies the snapshot;
- reconnect against a sessiond that was fully restarted (using Task 22's guarantee that it never
  reuses old IDs): confirm `RESYNC_REQUIRED{continuity=REPLACED}` and that the client abandons all
  pending events/edits without replaying any of them, rather than treating snapshot "distance" as
  a heuristic;
- race test: fire two overlapping resume attempts back to back (simulating a user reconnecting
  twice quickly) and confirm only the newer attempt's response is honored — the older response,
  even if it arrives later, must not replay events, rebind the outbox, or enable new event
  allocation;
- confirm an unknown or omitted `continuity` value is treated as a hard failure, not silently
  accepted as either outcome;
- confirm §12.1's rule holds through all of this: a transaction that was only partially committed
  when the connection dropped is never replayed as partial — either it fully committed before the
  drop (and is replayed whole) or it didn't commit at all;
- Commit the result.
```

### Task 24 — Event settlement protocol: sequencing, acknowledgement, and the dedup cache — Effort: High

```
You are implementing one bounded task in a larger project called SRUI (Semantic Remote UI). The
authoritative design document is attached: `SRUI_Semantic_Remote_UI_Design_v0.6.md`. Treat it as
read-only and authoritative — do not edit it. This is Task 24 of a sequential implementation plan
(Tasks 0–38). Task 23 built continuity-aware transaction replay for UI *state*; this task handles
*side-effect events* (like a Button ACTIVATE) — and v0.6 specifies this in far more depth than a
simple "remember which event_ids we've seen" cache. It is now a TCP-like reliable-delivery layer:
every event is sequenced, explicitly acknowledged, and tracked through an IN_FLIGHT → SETTLED
lifecycle with bounded send/receive windows. If you previously implemented a simpler dedup-only
version of this task against an earlier design draft, treat this as a substantial rework rather
than a small patch — read the new §18.2 in full before touching code. Inspect Task 23's reconnect
flow, Task 7's Event type, and the EventOutbox built in Task 18 (this task upgrades that
component in place) before starting.

Read: §7.7's closing paragraph (event_seq is positive, contiguous per `client_instance_id`,
retained until terminal settlement, bounded by a negotiated send window), §16's new
`EventAckStatus` enum and `ServerEventAck` message, §18.2 (event deduplication — this section was
rewritten and is now the normative spec for this task; read it slowly, it's dense), §19.2 (
`EVENT_ACK` is explicitly control-class traffic, never coalesced or dropped), §26 ("maximum
pending unacknowledged events" now bounds this task's client-side retry set, and reaching it must
be reported, not silently dropped), and Appendix B's rewritten event-state-machine section (
`IN_FLIGHT`/`SETTLED`/`settled_out_of_order`, and the reference window sizes: 4,096 slots per
client instance server-side, 256 slots client-side).

Build, in server-rust/event-dedupe/ (already scaffolded in Task 0):
- Extend `protocol/srui.proto` (from Task 2) with the `EventAckStatus` enum
  (`UNSPECIFIED`/`PROCESSED`/`DUPLICATE`/`REJECTED`) and the `ServerEventAck` message
  (`client_instance_id`, `event_id`, `last_processed_event_seq`, `status`,
  `revision_after_effect`, `reject_reason`, `session_id`) exactly as specified in §16. Regenerate
  Rust and Swift bindings — this is an incremental schema extension, not a redo of Task 2.
- Server-side state machine per Appendix B: `(client_instance_id, event_id) -> IN_FLIGHT{event_seq}`
  on admission, transitioning to `SETTLED{event_seq, status, result, revision_after_effect,
  reject_reason}` once the handler (Task 10's `session.on(...)`) finishes. Track, per
  `client_instance_id`, a `last_contiguous_processed_seq` that only advances when the *next*
  sequence in order settles (not merely the highest one seen — this is the TCP-cumulative-ack
  behavior described in §18.2: if sequence 2 settles while 1 is still pending, the reported
  frontier stays at 0 until 1 settles, then jumps straight to 2), plus a bounded
  `settled_out_of_order` set for sequences that settled ahead of the frontier.
- Admission rules: reject a new `event_seq` that isn't strictly greater than
  `last_processed_event_seq` or that falls outside the negotiated receive window (reference size
  4,096 slots/client, FIFO-evictable only at or below the contiguous frontier — never evict an
  `IN_FLIGHT` or out-of-order-settled entry to make room); reject an `EVENT` whose
  `client_instance_id` doesn't match the identity bound at `HELLO`/`RESUME` time, before any
  dedupe lookup or sequence bookkeeping; reject an event with no stable, non-empty `event_id` as a
  protocol error before allocating any per-client dedupe state.
- Emit exactly one `SERVER EVENT_ACK` for every event that settles — whether newly admitted or a
  replay — on the connection that carried that specific delivery. Route it as control-class
  traffic in the scheduler (Task 27) so it is never coalesced or dropped behind lower-priority
  traffic. A replay of an already-`SETTLED` event returns the cached status/result (`DUPLICATE`
  with the original `revision_after_effect`, or `REJECTED` again with the original
  non-empty `reject_reason`) without re-running the handler. A delivery that arrives while the
  same `(client_instance_id, event_id)` is still `IN_FLIGHT` gets no terminal ack at all — the
  client keeps it pending and may only retry after the first execution settles.
- Client (`EventOutbox`, upgrading the version built in Task 18): retain every outbound
  side-effect event until it is terminally settled — never discard or silently replace an
  unacknowledged sequence. Enforce a client-side send window (reference size 256 slots) and apply
  backpressure — stop admitting *new* user-generated events — rather than evicting a pending one
  when the window fills. On receiving an ack, remove that specific `event_id` from the retry set
  even if an earlier sequence is still outstanding, but do not advance the client's own notion of
  `last_acked_event_seq` across that gap. Ignore any ack whose `session_id` or
  `client_instance_id` doesn't match the currently active outbox — this is the same
  stale/superseded-connection guard Task 23's generation-bound resume relies on.
- Coordinate with, rather than duplicate, Task 23: a replay during resume MUST reuse the exact
  original `event_id` and `event_seq` this task allocated, and Task 23's abandon/replay decisions
  ultimately bottom out in this task's IN_FLIGHT/SETTLED bookkeeping.

Out of scope: pending-text-edit reconciliation (Task 29, §18.3) is a separate concern. Outbound
(server → client) UI-transaction backpressure/coalescing is Task 25's job and a different
direction entirely — don't conflate the two mechanisms even though both involve "backpressure."

Verification:
- exactly-once effect test (§18.2's canonical scenario, now with explicit acks): client sends
  ACTIVATE, server performs the effect but the simulated ack never reaches the client before the
  connection drops, client reconnects via Task 23's resume flow and replays the identical
  `event_id`/`event_seq`; assert the handler ran exactly once and the replay is answered
  `DUPLICATE` with the original result;
- overlapping-delivery test (no reconnect involved): deliver the same event twice back-to-back
  before the first settles; confirm the second delivery gets no terminal ack until the first
  settles, and the handler still runs exactly once;
- out-of-order settlement test: force two events to settle out of order (e.g. make the earlier
  one's handler artificially slow) and assert the exact frontier behavior from §18.2 — an ack for
  the later sequence reports frontier unchanged while the earlier one is still pending, then the
  frontier jumps forward once the earlier one settles;
- window exhaustion test: fill the client's send window and confirm new user-generated events are
  held locally (backpressure) rather than an in-flight event being evicted or a sequence being
  skipped to make room;
- rejection-replay test: an event that is `REJECTED` once (e.g. targeting a disabled node) is
  replayed and confirm it is answered `REJECTED` again with the identical `reject_reason`, not
  silently retried against current state;
- identity-binding test: an `EVENT` whose `client_instance_id` doesn't match the bound identity is
  rejected before any dedupe lookup;
- stale-ack test: an ack carrying a `session_id`/`client_instance_id` that doesn't match the
  client's active outbox is ignored rather than incorrectly settling an entry in it;
- a control test with two genuinely different `event_id`s for the same action type confirms both
  are applied (dedup keys on `event_id`, never on action/node type);
- Commit the result.
```

### Task 25 — Backpressure and scalar coalescing (server → client UI traffic) — Effort: Medium

```
You are implementing one bounded task in a larger project called SRUI (Semantic Remote UI). The
authoritative design document is attached: `SRUI_Semantic_Remote_UI_Design_v0.6.md`. Treat it as
read-only and authoritative — do not edit it. This is Task 25 of a sequential implementation plan
(Tasks 0–38). Tasks 17–24 built a working reconnectable transport with event safety — inspect the
current sessiond send path before starting. Note the direction this task governs: it is about
*outbound* server → client UI-transaction traffic (committed transactions the server pushes).
Task 24 separately implemented an *inbound* client → server backpressure mechanism (bounded event
sequence windows, §18.2) for side-effect events. The two are related in spirit but are distinct
mechanisms guarding distinct directions — don't merge them or assume one subsumes the other.

Read: §20.4 (backpressure) in full.

Build:
- Bounded outbound queues per connection in sessiond.
- When a client falls behind: coalesce rapid scalar `SET_PROPERTY` updates to the same
  node+property (§20.4's example: progress 0.50 -> 0.51 -> 0.52 collapses to the latest value
  only) — but never silently drop a structural transaction (CREATE_NODE/DELETE_NODE/model
  structural changes), and never let unbounded memory growth occur regardless of how slow the
  client is (§20.4's explicit requirement).
- An escape valve: an excessively stale client may be detached and forced through Task 23's
  resync path rather than accumulating unbounded backlog.

Out of scope: no priority scheduling across different message classes yet (that's Task 27 —
this task is specifically about coalescing/bounding a single client's backlog, not about
control/UI/terminal/resource channel prioritization). No inbound event-sequence windowing — that
is Task 24's mechanism, already built, in the opposite direction.

Verification (a load-style integration test):
- fire e.g. 1,000 rapid scalar property updates from the server SDK against an artificially
  throttled/slow test client transport; assert the client eventually converges to the correct
  final value while having received materially fewer than 1,000 discrete update messages;
- interleave a structural transaction (e.g. adding a row) among the rapid scalar updates and
  confirm it is never dropped, appearing in the client's final state even under heavy coalescing
  of the scalar updates around it;
- confirm server-side memory/queue size stays bounded (assert against your configured bound)
  even when the test client is held artificially unresponsive for an extended period;
- Commit the result.
```

**Follow-up improvements (post-implementation errata):**

> Task 25 as implemented coalesces scalar updates on the server side, but the resulting
> envelope must use v0.6 §12.1's **coalesced scalar-delta delivery form** (`base=N, new=M`
> where `M > N+1`, containing only scalar `SET_PROPERTY` operations). Task 14's client-side
> `TransactionApplier` enforces a strict `base + 1` rule and will reject these multi-revision
> spans. The following items must be addressed:
>
> 1. **Server: emit the §12.1 envelope** — when coalescing collapses revisions N through M,
>    the outbound frame must carry `base=N, new=M` with only the latest value per
>    `(node, property)` pair. Structural transactions remain barriers that end a coalescing
>    run and are delivered individually as normal `base=K, new=K+1` commits.
> 2. **Client: upgrade the replica applier** — `TransactionApplier.applyDelivered()` (or
>    equivalent) must accept `new > base + 1` **if and only if** every operation in the
>    envelope is a scalar `SET_PROPERTY` and `base == currentRevision`. Multi-revision frames
>    containing structural operations, or applied to an authoritative store, must still be
>    rejected.
> 3. **Test: add coalesced-span conformance fixtures** — e.g.
>    `48_coalesced_scalar_delta_delivered.json` (valid span accepted by replica) and
>    `49_coalesced_scalar_delta_rejected_as_commit.json` (same span rejected on authoritative
>    path). The existing 1,000-update convergence test must exercise the upgraded applier
>    end-to-end.

---

## Phase G — Resources and scheduling

### Task 26 — Resource model (content-addressed store + image delivery) — Effort: Medium

```
You are implementing one bounded task in a larger project called SRUI (Semantic Remote UI). The
authoritative design document is attached: `SRUI_Semantic_Remote_UI_Design_v0.6.md`. Treat it as
read-only and authoritative — do not edit it. This is Task 26 of a sequential implementation plan
(Tasks 0–38). Tasks 9/16 already have an `Image` node type on both sides that currently has no
real content delivery — inspect them, along with the current transport/backpressure code from
Tasks 17–25, before starting.

Read: §14 (resource model) in full, §19.2's mention of resource chunk sizing (16–32 KiB) and
the "resource" logical channel being low priority relative to control/input/UI.

Build:
- server-rust/resources/: a content-addressed (SHA-256) resource store; publish/lookup API used
  by the SDK (`publish_resource(bytes)` per §29); chunked delivery (16–32 KiB chunks per §19.2)
  interleaved with other traffic rather than sent as one blocking multi-megabyte frame.
- client-macos/Resources/: a `ResourceCache` that verifies each resource's hash before
  committing it to cache, assembles chunks, and can persist across sessions (in-memory is fine
  for this task if cross-session persistence isn't wired up yet — note if you deferred that
  part).
- Wire `Image` nodes on both sides to reference a resource hash and resolve to actual decoded
  image content in the Task 16 AppKit renderer.
- Enforce the §26 limits relevant here: maximum resource encoded size, maximum decoded image
  dimensions/pixels.

Out of scope: no general priority scheduler across all channel classes yet (Task 27 generalizes
this task's "don't block UI traffic" behavior into the full documented scheme). No server-
supplied fonts (explicitly out of v0.1 per §14).

Verification (integration test):
- publish an image resource from the server SDK, confirm the client receives it, verifies its
  hash, and the Task 16 renderer displays it correctly in an `Image` node;
- corrupt a chunk in transit (in a test harness) and confirm the client rejects the resource
  rather than silently displaying corrupted content;
- while a large resource transfer is in flight, concurrently send a high-priority
  `SET_PROPERTY`, and measure that its delivery latency stays within a small, documented bound
  rather than waiting behind the resource transfer;
- Commit the result.
```

### Task 27 — Logical channel priority scheduler — Effort: Medium

```
You are implementing one bounded task in a larger project called SRUI (Semantic Remote UI). The
authoritative design document is attached: `SRUI_Semantic_Remote_UI_Design_v0.6.md`. Treat it as
read-only and authoritative — do not edit it. This is Task 27 of a sequential implementation plan
(Tasks 0–38). Task 26 handled resource-vs-UI interleaving ad hoc; this task generalizes it into
the documented scheme. Inspect the current transport send path on both sides before starting.

Read: §19.2 (logical channel scheduler) in full — the priority table (control/input highest, UI
high, terminal high/normal, resource low); note the control row now explicitly names
`SERVER EVENT_ACK` (§18.2, built in Task 24) alongside HELLO/WELCOME/errors/resume.

Build:
- Implement the full §19.2 priority scheme in the transport layer on both client and server:
  control messages (HELLO/WELCOME/errors/resume/`SERVER EVENT_ACK`) and input (semantic user
  events) at highest priority, committed UI transactions at high priority, terminal bytes at
  high/normal (terminal itself isn't built until Task 30, but reserve its priority slot now),
  resource chunks at low priority. Generalize Task 26's ad hoc interleaving to use this scheme
  rather than a special case. In particular, confirm `SERVER EVENT_ACK` traffic from Task 24
  actually rides the control class end to end — it must never be coalesced or dropped behind
  lower-priority traffic, per §18.2's explicit requirement.
- Document that this is designed so a future QUIC binding could map these classes to independent
  streams without changing Core semantics (§19.2's closing note) — you don't need to implement
  QUIC, just avoid designing yourself into a corner.

Out of scope: no QUIC transport (that's explicitly future work per the doc, not part of this
plan).

Verification (a scheduler-focused test harness):
- saturate the resource channel with a large transfer while simultaneously sending control,
  input, and UI-class traffic; assert each higher-priority class meets a documented latency
  bound regardless of the resource channel's load;
- specifically confirm `SERVER EVENT_ACK` messages are never delayed behind a saturated resource
  channel and are never coalesced/merged with anything else;
- confirm no class can starve another indefinitely (e.g. a pathological flood of one class still
  leaves room for the others within your documented bounds);
- Commit the result.
```

---

## Phase H — Rich interaction

### Task 28 — Collections/virtualization end to end — Effort: High

```
You are implementing one bounded task in a larger project called SRUI (Semantic Remote UI). The
authoritative design document is attached: `SRUI_Semantic_Remote_UI_Design_v0.6.md`. Treat it as
read-only and authoritative — do not edit it. This is Task 28 of a sequential implementation plan
(Tasks 0–38). Task 6 built the Rust Model type in-memory; Task 16's renderer never had real
List/Table/Tree data to virtualize since everything so far has been small fixtures. Inspect both,
plus the now-real transport from Tasks 17–27, before starting.

Read: §8 (collections and model data) again with focus on "asynchronous range requests for data
not yet replicated," "viewport hints... asynchronous and never required for layout correctness,"
and "server-pushed high-priority visible ranges"; §22.7 (collections on the AppKit side —
NSTableView/NSOutlineView fed from the replicated model, view-reuse, loading representation for
uncached ranges).

Build:
- Wire Task 6's Model operations through the real transport (Tasks 17–27) end to end.
- Client: NSTableView/NSOutlineView adapters (client-macos/Collections/) that read from the
  replicated Model, reuse views per AppKit's normal behavior, show a local loading placeholder
  for rows not yet cached, and send asynchronous range-fetch requests for scrolled-to but
  uncached ranges — scrolling itself must remain instantaneous/local, not blocked on the
  network.
- Server: respond to range requests and support pushing high-priority visible ranges
  proactively when it has reason to (e.g. right after a Table is first created).

Out of scope: no local sorting/filtering unless you have time to spare (§8 allows it "only when
explicitly allowed" — treat it as optional/deferred, note if skipped).

Verification:
- build a demo model with a very large item_count (e.g. 500,000) but only a small cached window;
  confirm exactly one native NSTableView/NSOutlineView exists (not one control per row) via a
  runtime assertion or view-count check;
- scroll to an uncached range and confirm: (a) a loading placeholder appears immediately with no
  network wait, (b) an async range request goes out, (c) the real data appears once it arrives,
  without ever blocking the scroll gesture itself;
- Commit the result.
```

### Task 29 — Local-first text editing — Effort: High

```
You are implementing one bounded task in a larger project called SRUI (Semantic Remote UI). The
authoritative design document is attached: `SRUI_Semantic_Remote_UI_Design_v0.6.md`. Treat it as
read-only and authoritative — do not edit it. This is Task 29 of a sequential implementation plan
(Tasks 0–38). TextInput/TextArea have existed as basic AppKit controls since Task 16 but without
real edit semantics. Inspect the current transport (Tasks 17–27) before starting.

Read: §18.3 (pending text edits) and §22.6 (text editing) in full — this is one of the design
doc's central optimizations, read it carefully.

Build:
- Client (client-macos/Text/): ordinary typing, caret movement, selection, marked-text/IME
  composition, clipboard paste/copy, and visible glyph rendering happen entirely via the native
  NSTextField/NSTextView with zero network dependency for visible feedback. IME marked/
  composition text stays local until the platform considers an edit ready to synchronize — do
  not attempt to reimplement IME behavior remotely.
- The client sends semantic edit results (whole-value or compact deltas — your choice, document
  it) tagged with a monotonically increasing `edit_seq`, batching/coalescing when safe rather
  than sending every keystroke.
- Server: applies edits with monotonically increasing edit_seq, may accept-and-publish, may
  normalize/validate and publish a corrected value, or may reject and publish validation state
  (wire whatever `validation_state` property already exists from Task 9's §7.4 properties).
- Reconnect interaction per §18.3: pending edits may be replayed only when the same session
  resumes (integrate with Task 23's resume flow); after a full resync, unresolved local edits
  are not silently merged unless a higher-level profile defines reconciliation (none does yet —
  so the correct v1 behavior is to discard unmerged local edits on forced resync and note this
  explicitly rather than inventing merge logic).

Out of scope: no spellcheck-over-the-wire concepts (spellchecking is local/AppKit's own
business, not a protocol concern) — the doc only asks that it be "locally appropriate," it's not
something you build.

Verification:
- an artificial-network-delay test (inject e.g. 300–600ms one-way delay into the test transport)
  confirms visible typing, caret movement, selection, and IME composition feedback timing is
  unaffected by the delay — i.e., these interactions never wait on a round trip;
- a server-side rejection/correction test: type a value the server-side validation rejects,
  confirm the server publishes a corrected value and the client's displayed text converges to it
  rather than the client's raw (rejected) input sticking around silently;
- a reconnect-with-pending-edit test exercising the §18.3 replay-only-on-resume rule;
- Commit the result.
```

### Task 30 — Terminal compatibility profile — Effort: High

```
You are implementing one bounded task in a larger project called SRUI (Semantic Remote UI). The
authoritative design document is attached: `SRUI_Semantic_Remote_UI_Design_v0.6.md`. Treat it as
read-only and authoritative — do not edit it. This is Task 30 of a sequential implementation plan
(Tasks 0–38). This is the first task touching the Terminal extension profile; server-rust/pty and
client-macos/Terminal were scaffolded empty in Task 0. Inspect the current transport/session
architecture (Tasks 17–27) before starting, since Terminal rides on top of it as an extension
node, not a special-cased channel.

Read: §21 (terminal compatibility profile) and §21.1–§21.2 in full.

Build:
- Server (server-rust/pty/ + wiring into sessiond): a PTYManager spawning a real PTY, streaming
  its output as `TERMINAL_DATA` and accepting `TERMINAL_INPUT`/`TERMINAL_RESIZE` (mapped to
  TIOCSWINSZ) as described in §21's diagram. The Terminal node is registered as an
  org.srui.terminal/1 extension node per §11, an opaque compatibility island — PTY bytes are
  never interpreted as standard semantic widgets.
- Client (client-macos/Terminal/): a VT/ANSI parser producing a retained TerminalGrid (cells,
  cursor, scrollback), and a TerminalRenderer displaying it inside an AppKit view. Keyboard input
  in that view becomes TERMINAL_INPUT.
- Reconnect per §21.2: each terminal stream has a monotonically increasing byte offset; the
  server keeps a bounded output ring; on resume, replay the needed range if retained, or mark
  `TERMINAL_RESYNC_REQUIRED` if not — this must not affect or be affected by the semantic
  session's own Task 23 reconnect logic (a terminal needing a redraw doesn't force the whole
  semantic session to resync).
- Optional but recommended per §21.2's note: back the PTY with `tmux` (or similar) so
  reattachment causes a natural redraw; if you skip this, note it as a known gap for later.

Out of scope: no attempt to reconstruct standard semantic widgets from parsed terminal content
(explicitly called out as generally unreliable in §21.2) — the parser retains structural
terminal info (grid/cursor/scrollback/hyperlinks/shell-integration regions if you have time) but
does not try to "understand" the application inside the terminal.

Verification:
- run an actual interactive shell through the Terminal node end to end (type commands, see
  output, resize the window and confirm TIOCSWINSZ takes effect);
- reconnect within the output ring's retention: confirm seamless replay with no visible gap;
- reconnect beyond retention: confirm `TERMINAL_RESYNC_REQUIRED` is signaled and the client
  handles it (redraw/restart strategy — even a simple "clear and show a resync notice" is
  acceptable for v1, just don't leave stale content displayed as if it were current);
- confirm a concurrent semantic UI region (e.g. the Task 10 counter's Button/Progress, embedded
  alongside the Terminal node in the same window) keeps working normally throughout, proving the
  Terminal island doesn't leak into or block standard widget traffic;
- Commit the result.
```

### Task 31 — Coding-agent example composition (with extension fallback) — Effort: Medium

```
You are implementing one bounded task in a larger project called SRUI (Semantic Remote UI). The
authoritative design document is attached: `SRUI_Semantic_Remote_UI_Design_v0.6.md`. Treat it as
read-only and authoritative — do not edit it. This is Task 31 of a sequential implementation plan
(Tasks 0–38). By now (Tasks 9, 16, 28, 29, 30) you have widgets, collections, text editing, and
terminal all working over a real reconnectable transport. Inspect the current examples/
directory and each of those subsystems before starting.

Read: §30 (example coding-agent UI) in full, §11.1 (extension fallback rule — a server must not
send an unsupported extension as required content; it sends a fallback subtree that a base
client renders instead).

Build, in examples/coding-agent-demo/:
- Assemble the §30 tree using real components from prior tasks: Surface > Column > Row(heading +
  Progress) > Row(Tree(files), Column(RichText(conversation), Terminal, Row(Approve/Reject
  buttons))) > TextArea(prompt). (Note: §30 shows a Split pane, but Split is a SHOULD-tier widget
  deferred in Tasks 9 and 16; a horizontal Row achieves the same two-panel layout using only
  required-tier components.)
- Add one placeholder extension node (e.g. representing a future `Diff` or `ApprovalRequest`
  from the coding profile mentioned in §30, which is explicitly NOT being implemented in this
  plan) that always carries a Standard Widget fallback subtree per §11.1, so the fallback path
  gets real exercise even though the real coding extension profile doesn't exist yet.

Out of scope: do not implement an actual coding/diff extension profile — that's explicitly
future work per the design doc (§30's "A future coding profile can add..."). This task only
proves the fallback mechanism works using a stand-in extension node.

Verification:
- run the full example end to end (real transport, real widgets, real terminal, real text
  editing) and confirm it behaves like a coherent small application, not just disconnected
  demos;
- an automated test confirms a client that does NOT declare support for the placeholder
  extension's capability renders the Standard Widget fallback correctly instead of erroring or
  hanging (this is the actual verification target of this task);
- Commit the result.
```

### Task 32 — Security hardening pass — Effort: Medium

```
You are implementing one bounded task in a larger project called SRUI (Semantic Remote UI). The
authoritative design document is attached: `SRUI_Semantic_Remote_UI_Design_v0.6.md`. Treat it as
read-only and authoritative — do not edit it. This is Task 32 of a sequential implementation plan
(Tasks 0–38). Individual limits have been enforced piecemeal since Task 4 (tree depth/node
count), Task 5 (max ops), Task 15 (decode-time limits), Task 24 (max pending unacknowledged
events, via the event send/receive windows), Task 26 (resource size/dimensions). Inspect all of
it before starting — this task is a consolidation and gap-closing pass, not a from-scratch build.

Read: §26 (client attack-surface controls) and §27 (server security controls) in full, plus the
security equivalence table at the end of Appendix B as a sanity check of what guarantees should
hold. Note
§26's new clarification that "maximum pending unacknowledged events" bounds the client's Task 24
retry set, and that reaching it must be reported, not silently and invisibly dropped.

Build:
- Audit every limit listed in §26's "mandatory limits" block (max frame size, max transaction
  operations, max tree depth, max node count, max string length, max model/item count per
  message, max resource encoded size, max decoded image dimensions/pixels, max update rate, max
  pending unacknowledged events, max terminal escape payload lengths) and confirm each is
  actually enforced somewhere in the code with a test proving it fails closed. Fill any gaps you
  find — do not assume a prior task covered something without checking. In particular, confirm
  that ordinary send-window exhaustion in Task 24 behaves as backpressure (new events are held,
  nothing pending is touched) rather than eviction, and separately confirm that IF an
  implementation is ever forced to actually give up on a still-unacknowledged event (a bug
  scenario, not the intended path), that is a loud, reported error/log condition, never a silent
  drop — §26 is explicit that discarding an event before it's known to be processed must be
  visible.
- Confirm resource hashes are verified before cache commit (Task 26); decompression is bounded
  if any compression is in use; malformed tree operations fail the transaction rather than
  corrupt state (Task 5's atomicity should already guarantee this — verify with a fuzz-style
  test); unknown required semantics fail closed (Task 20); terminal OSC clipboard mutation is
  disabled by default (Task 30 — add this control now if it wasn't already); arbitrary
  URL-opening/clipboard/filesystem/notification/camera/microphone access is gated behind
  capabilities that don't exist yet in this plan and therefore must simply not be reachable at
  all right now (confirm there's no accidental back door).
- §27 server-side: confirm sessiond runs as the authenticated user (not root); the ssh-bridge
  only reaches that user's own socket; application launch uses an argument vector / registered
  app ID, never shell interpolation; semantic action identifiers (like `action_key` from §7.7)
  are never interpreted as shell commands anywhere in the SDK or examples; stale node/client-
  instance IDs are rejected.

Out of scope: no new features — if you find a genuine missing feature (not a missing limit-
check) needed for security, flag it rather than building it, since it may belong in an earlier
task's scope.

Verification:
- for each item above, either point to an existing passing test from a prior task or add a new
  one — produce a checklist in your summary mapping each §26/§27 bullet to the test that covers
  it;
- a fuzz-style test suite feeding oversized/deep/malformed/adversarial payloads at both the
  server's decode path and the client's decode path (Task 15), asserting clean rejection with no
  crash and no state corruption in either;
- Commit the result.
```

### Task 33 — Conformance suite consolidation — Effort: Medium

```
You are implementing one bounded task in a larger project called SRUI (Semantic Remote UI). The
authoritative design document is attached: `SRUI_Semantic_Remote_UI_Design_v0.6.md`. Treat it as
read-only and authoritative — do not edit it. This is Task 33 of a sequential implementation plan
(Tasks 0–38). Individual conformance-relevant tests have accumulated across many prior tasks
(starting with Task 8's Core state-machine/semantic-not-paint fixtures). Inspect
protocol/conformance-vectors/ and the test suites across both server-rust and client-macos before
starting.

Read: §32 (conformance suites) in full — all 12 items. Suite 8 (reconnect) now needs to cover
substantially more ground than "replay vs. snapshot," given Tasks 23–24's continuity/settlement
protocol — re-read §18 and §18.2 alongside §32 item 8.

Build:
- Organize protocol/conformance-vectors/ and the corresponding test runners (Rust and Swift) so
  all 12 suites from §32 are clearly identifiable, not scattered ad hoc across unrelated test
  files: (1) Core state-machine, (2) widget semantic tests per node type, (3) semantic-not-paint,
  (4) frame-independence, (5) semantic-input (coordinate-free events for standard controls;
  coordinates only for subscribed scene nodes), (6) local text-interaction under injected
  latency, (7) extension-negotiation fallback, (8) reconnect — replay/snapshot,
  partial-transaction discard, pending-edit reconciliation, event settlement/dedupe, **and now
  explicitly**: `SAME_SESSION` vs `REPLACED` continuity outcomes (Task 23), generation-bound
  resume-attempt supersession (Task 23), and the IN_FLIGHT overlapping-delivery rule plus
  out-of-order frontier advancement (Task 24) — (9) security limits, (10) renderer semantic tests
  (accessibility roles/actions/enabled-disabled/selection/text editing), (11) semantic
  inspection tests, (12) toolkit mapping tests.
- Fill genuine gaps: from the work so far, (4) frame-independence and (5) semantic-input-as-a-
  named-suite are the most likely to exist only implicitly rather than as an explicit,
  independently-runnable suite — write them explicitly if missing. (8) is the other likely gap:
  Tasks 23–24 almost certainly tested continuity and settlement behavior inline as integration
  tests rather than as a named, independently-runnable conformance suite — consolidate those
  scenarios here rather than leaving them scattered. (11) semantic inspection tests depend on
  Task 35, which comes after this task in the plan; if Task 35 hasn't run yet, add a stub suite
  here that Task 35 will fill in, rather than skipping it silently.
- A single `run-conformance` script (or Make/cargo-xtask target) that runs all 12 suites against
  both the Rust and Swift implementations and prints a clear pass/fail table.

Out of scope: don't invent new protocol behavior to pass a suite — if a suite reveals an actual
gap in implemented behavior (not just missing test coverage), flag it rather than papering over
it with a weakened test.

Verification:
- `run-conformance` (or equivalent) executes cleanly and reports all 12 suites, with every suite
  either fully passing or explicitly marked as a known, documented gap (not silently absent);
- Commit the result.
```

### Task 34 — Benchmarks — Effort: Medium

```
You are implementing one bounded task in a larger project called SRUI (Semantic Remote UI). The
authoritative design document is attached: `SRUI_Semantic_Remote_UI_Design_v0.6.md`. Treat it as
read-only and authoritative — do not edit it. This is Task 34 of a sequential implementation plan
(Tasks 0–38). The benchmarks/ directory was scaffolded empty in Task 0; by now the full system
(Tasks 1–33) exists to benchmark. Inspect the current codebase before starting.

Read: §31 (benchmark methodology) in full, all six subsections, and §23 (macOS renderer
performance strategy — the concrete numeric targets to compare against).

Build, filling in benchmarks/parse-render, mutation, reconnect, network, terminal (and add a
fifth/sixth directory if you want to split serialization out separately, matching §31.2):
- §31.1 local renderer benchmark: warm-renderer first/complete paint, CPU, allocations, peak
  memory for a representative fixture.
- §31.2 serialization benchmark: server-side generation/serialization cost from the same
  abstract UI state used in 31.1.
- §31.3 steady-state mutation / frame-independence benchmark: 1, 100, and 1,000 value updates;
  measure bytes and decode-to-visible latency; repeat under different configured render
  cadences (60/120/144/240Hz or an uncapped synthetic renderer) and confirm wire bytes/message
  count stay materially unchanged across cadences while local repaint count may differ.
- §31.4 network/local-interaction benchmark: inject 0/100/300/600ms RTT; confirm local
  interactions (text entry, caret, selection, IME, scrolling, hover/pressed, menu opening) show
  no added latency from the injected RTT.
- §31.5 reconnect benchmark: the full boundary list (mid-resource, mid-transaction, immediately
  before/after event receipt, within/beyond journal retention) — Task 23/24 covered some of
  these already; this task turns them into a repeatable, measured benchmark rather than one-off
  tests, and fills any boundary cases not yet covered. Per §31.5's own note, the "after side
  effect but before ack" boundary specifically requires verifying that the replayed event is
  answered `DUPLICATE` from Task 24's result cache rather than re-running the action — measure
  and assert this explicitly, don't just eyeball it. Also add the boundary case v0.6 introduces:
  a resume attempt superseded by a newer one mid-flight (Task 23) — measure that the superseded
  attempt's eventual response is fully inert.
- §31.6 terminal benchmark: compare embedded Terminal interaction to a standalone terminal;
  explicitly test reconnect ring-buffer exhaustion.

Out of scope: fixing every performance shortfall found — this task measures and reports against
the §23 targets; if a target is missed by more than 2x, flag it clearly as a follow-up rather
than trying to fix it inside this task (mixing "measure" and "optimize" tasks makes both harder
to verify).

Verification:
- running the benchmark suite produces a report (numbers, not just pass/fail) for every
  subsection above;
- the report explicitly calls out any §23 target missed by more than 2x;
- Commit the result (report + code, not just the numbers pasted somewhere transient).
```

**Task 34 implementation note (2026-09-10).** The delivered §31.1 report uses explicitly named
signed net-live default-zone endpoint deltas; it does not claim cumulative allocation-call or
requested-byte counts. A real non-compacting `malloc_history -allEvents` pre-workload export
expanded to 1,902,439,272 bytes and was rejected without producing a metric. The bounded
benchmark-only Darwin interposition follow-up, detailed rationale, and acceptance criteria are
tracked in [issue #48](https://github.com/aizlabs/srui/issues/48). This deferral does not weaken
§31.1's allocation requirement.

### Task 35 — Local semantic inspection and automation API — Effort: Medium

```
You are implementing one bounded task in a larger project called SRUI (Semantic Remote UI). The
authoritative design document is attached: `SRUI_Semantic_Remote_UI_Design_v0.6.md`. Treat it as
read-only and authoritative — do not edit it. This is Task 35 of a sequential implementation plan
(Tasks 0–38). This is the last core-scope task before the deferred/optional ones. Inspect the
current client-macos/RendererAppKit and SemanticModel before starting.

Read: §22.9 (local semantic inspection and automation) in full, and re-read §4 invariant 18 plus
§7.7's action_key discussion — the key distinction this task must preserve: server-supplied data
is never executable, but *trusted local automation* may inspect the tree and invoke actions
through the same event/authorization path a real user click would use.

Build, in client-macos/Accessibility/ (or a new dedicated module if that fits better — document
your choice):
- A read-only semantic-tree inspection surface exposing node identity, role, label, value,
  hierarchy, state, and advertised actions — built from the existing SemanticStore, never
  exposing NSView pointers, AppKit class names, or any other renderer-internal identity as part
  of its public interface.
- A `find(role:label:)`-style helper returning a handle that supports `.activate()` and similar
  semantic actions, which internally routes through the exact same EventOutbox/event path
  (Task 18/24) a real user interaction would use — it must not have a private shortcut that
  mutates the local replica directly or bypasses server authorization.
- Per §22.9's closing line, leave any actual XPC/IPC exposure of this surface disabled/stubbed —
  this task builds the in-process API and proves it works in-process; exposing it to other local
  processes is explicitly future work requiring its own permission model.

Out of scope: no XPC service, no cross-process automation surface, no voice/accessibility-tool
integration beyond whatever falls out naturally from using real AppKit accessibility (already
inherited since Task 16 used real controls).

Verification:
- an in-process automation test that finds a button by role+label (e.g. the coding-agent demo's
  "Approve" button from Task 31) and calls `.activate()`, and confirms: (a) the resulting wire
  event and server-side effect are indistinguishable from a real mouse click on the same button,
  (b) the button's `enabled=false` state (if set) correctly blocks the automated activation just
  as it would block a real click;
- a static/API-surface check confirming no AppKit type or NSView reference is reachable through
  the public inspection API's types;
- Commit the result.
```

---

## Phase J — Deferred / optional

These two are explicitly not required for the MVP per the design doc itself (§33 item 14 for
VectorScene, §33 item 13 / §28's `sdk/second-language/` for the second implementation). Do them
only if you want the fuller system; otherwise the plan is "done" at Task 35 for a working,
reconnectable, secure, terminal-capable, benchmarked semantic remote UI system with a real macOS
renderer.

### Task 36 — VectorScene profile (optional, only after a real need appears) — Effort: High

```
You are implementing one bounded task in a larger project called SRUI (Semantic Remote UI). The
authoritative design document is attached: `SRUI_Semantic_Remote_UI_Design_v0.6.md`. Treat it as
read-only and authoritative — do not edit it. This is Task 36, an OPTIONAL task in a sequential
implementation plan (Tasks 0–38). The design doc itself (§33 item 14) says VectorScene should
only be added "after real applications demonstrate a need" — confirm with whoever is directing
this work that such a need has actually appeared before starting; do not build this speculatively
just because it's next in the numbered list.

Read: §11.2 (VectorScene) in full, §7.7's coordinate-input rule (POINTER_DOWN is only valid for
scenes that explicitly subscribe to pointer events), §12.2 (still no frame concept — VectorScene
is a retained scene graph with incremental mutations, not a per-frame drawing stream).

Build:
- The primitives from §11.2 (Path, Rect, Ellipse, TextRun, Image, Transform, Clip, Gradient,
  Group) as an extension profile (org.srui.vector-scene/1) following the same
  namespace/capability negotiation pattern as the standard profile (Tasks 6-7/20), with its own
  retained store and incremental mutation ops (reuse Task 5's transaction machinery rather than
  inventing a parallel one).
- Client renderer for the scene graph, plus POINTER_DOWN-style coordinate event routing scoped
  only to nodes that explicitly subscribe (per §7.7), in the scene's own documented logical
  coordinate system, not global screen pixels.
- If an SDK-level `Canvas` convenience API is desired, it must lower to these retained objects —
  it is explicitly not allowed to become a second immediate-mode wire primitive (§11.2's closing
  paragraph).

Out of scope: do not let VectorScene's existence weaken or bypass the semantic-first rule for
Standard Widget Profile nodes elsewhere in the codebase.

Verification:
- a demo scene (something genuinely requiring exact geometry/arbitrary drawing) renders
  correctly and updates incrementally under mutation, not via full-scene resends;
- pointer events only arrive for scenes that opted in, in the correct logical coordinate space;
- Commit the result.
```

### Task 37 — Minimal second-language client (optional, pre-1.0 hardening) — Effort: High

```
You are implementing one bounded task in a larger project called SRUI (Semantic Remote UI). The
authoritative design document is attached: `SRUI_Semantic_Remote_UI_Design_v0.6.md`. Treat it as
read-only and authoritative — do not edit it. This is Task 37, an OPTIONAL task in a sequential
implementation plan (Tasks 0–38), matching §33 item 13 ("Implement a minimal second client or
headless conformance client in another language before protocol 1.0") and the sdk/second-
language/ placeholder from Task 0.

Read: §1 (layer separation is a normative requirement — "a future Windows, GTK, or SWT client
must be able to implement SRUI without depending on any macOS concept"; this task is the actual
test of that claim), §32 conformance suites (this client's real job is to run those suites, not
to render a pretty UI).

Build, in sdk/second-language/:
- A minimal, headless (no real GUI needed) client in a language other than Swift (Rust, Go,
  Python, TypeScript — pick based on what's easiest to get networking + protobuf working in
  quickly) that can: complete the Task 20 capability handshake, connect over the Task 19 SSH
  transport, decode transactions into a simple in-memory tree (doesn't need a full typed
  Standard Widget layer, just enough structure to run Task 33's conformance fixtures against),
  and run the protocol/headless subset of the Task 33 conformance suite (suites 1–5, 7–9)
  against a live server. Renderer-specific suites — 6 (local text interaction), 10 (renderer
  semantics), 11 (semantic inspection/automation), and 12 (toolkit mappings) — are excluded
  because they require a GUI renderer or interactive text-editing stack that this client
  explicitly does not implement.

Out of scope: no rendering, no text editing, no terminal support, no resource caching beyond
what's needed to not crash on receiving a resource reference — this client's entire purpose is
to prove the protocol layer (not the renderer layer) is genuinely toolkit/platform-neutral.

Verification:
- this client passes the protocol/headless conformance suites (1–5, 7–9) when run against the
  same live server as the Swift client, with zero macOS-specific assumptions anywhere in its code
  (spot-check: it should build and run on a non-macOS machine, or at minimum have no
  import/dependency on anything Apple-specific). Renderer-specific suites (6, 10, 11, 12) are
  reserved for GUI clients and are not required here;
- Commit the result.
```

---

## Phase K — Client connection UX

### Task 38 — Connection manager UI (host entry, saved sessions, reconnect) — Effort: Medium

```
You are implementing one bounded task in a larger project called SRUI (Semantic Remote UI). The
authoritative design document is attached: `SRUI_Semantic_Remote_UI_Design_v0.6.md`. Treat it as
read-only and authoritative — do not edit it. This is Task 38 of a sequential implementation plan
(Tasks 0–38). It depends on Task 20 (real SSH transport + capability negotiation) at minimum, and
is far more useful once Task 23 (continuity-aware reconnect) also exists — check which of those
are already implemented before starting, and if Task 23 isn't done yet, build the saved-connection
list and resume wiring so it's ready to use RESUME once Task 23 lands, rather than skipping it.
Every prior task's demos (T18, T19, T31) connect to one hardcoded dev host as a fixed test
harness — this task is the first to build real end-user chrome around that machinery. Inspect the
current client-macos app target, SSHTransport (Task 19), and the resume/continuity API (Task 23,
if present) and the EventOutbox (Task 24, if present) before starting.

Read: §17 (session states — ATTACHED/DETACHED/TERMINATING/EXPIRED, so the UI can show a
meaningful status rather than a raw boolean — plus the incarnation-token paragraph, since a saved
entry's `session_id` may simply no longer exist by the time the user reconnects), §18 (reconnect
— the complete process-local continuity checkpoint; the cold-relaunch rule requiring a fresh
`client_instance_id` and revision 0 when no durable checkpoint exists; resume-time core/profile and
extension-namespace re-advertisement; the continuity decision the server returns; and the
generation-bound resume-attempt rule, which this UI can trigger directly if the user clicks Connect
twice), §19.1 (recommended SSH posture — host-key verification behavior
must stay visible to the user, not silently bypassed), §6.3's state ownership table (this task's
saved-connection list is purely local "presentation state," owned by the client, never
synchronized to the server — the server has no concept of it).

Build, as a new small app-level module in client-macos/ (e.g. `ConnectionManager/`) sitting above
Session/TransportSSH, not inside SemanticModel/Protocol:
- A "Connect" window/sheet: host, optional port, remote user, and a Connect action that invokes
  the Task 19 SSHTransport. Rely entirely on the user's existing `ssh`/agent/`known_hosts`
  infrastructure for credentials (§19.1 — do not build your own private-key handling or a
  credential store).
- A local, client-only saved-connections list (e.g. a JSON/plist file under Application
  Support): for each entry, at least a human label, host, user, and — once a session has been
  established — its `session_id` and last-known revision. The revision is presentation metadata,
  not cold-resume authority. While the application remains running, retain the entry's actual
  semantic replica, `client_instance_id`, event frontier/outbox, pending text state, terminal
  offsets, resource continuity, negotiated capabilities, and extension/Terminal namespace and type
  mappings in memory so a warm reconnect can resume from those exact values. After a cold
  relaunch without a durable continuity checkpoint, attempt `CLIENT RESUME`
  for the saved `session_id` from revision 0 with a fresh `client_instance_id` and empty event,
  text, and terminal continuity state; never advertise the persisted last-known revision. Every
  resume also re-advertises the current client's `core_version` and supported `profiles`. Before
  replay or snapshot traffic enters the data plane, validate the server-authoritative
  `required_profiles`, `optional_profiles`, and `extension_namespaces` carried by
  `SERVER_RESUME_OK` or `SERVER_RESYNC_REQUIRED`, then install the negotiated capability result
  and session-assigned namespace/type mappings. This list is never sent to the server and has no
  protocol meaning; it is exactly the kind of local presentation state §6.3 says the client owns
  unilaterally.
- A session list window showing saved entries with a status derived from the last known
  transport/session state (e.g. "connected," "disconnected — will resume," "unknown"), letting
  the user pick one to (re)connect or remove. Removing an entry only forgets it locally — it has
  no effect on the actual remote session, which persists server-side per §17 regardless of any
  one client's bookkeeping.
- Respect the continuity decision from Task 23 rather than assuming resume always succeeds: if a
  reconnect comes back `RESYNC_REQUIRED{continuity=REPLACED}` (the saved entry's incarnation is
  gone — expired, or the remote sessiond restarted), show the user that this session was lost/
  replaced rather than silently presenting it as a seamless resume; update the saved entry with
  the newly issued `session_id` so future reconnects target the replacement incarnation instead
  of repeatedly hitting a dead one.
- If the user triggers Connect again while a resume attempt for the same saved entry is still in
  flight (e.g. an impatient double-click), let Task 23's generation-bound supersession do its
  job rather than adding a second competing code path here — this UI's only obligation is to not
  block a newer attempt on an older one, and to reflect whichever attempt is currently newest.
- Surface host-key verification failures/changes as a clear, blocking dialog rather than
  connecting anyway — this must fail closed exactly like §19.1 requires of the transport itself.

Out of scope: no keychain-integrated secret storage beyond what the user's own ssh-agent/
known_hosts already provide; no simultaneous-multi-session window management beyond whatever
falls out naturally (one window per active connection is fine); no syncing the saved-connection
list across machines; no new protocol message kinds or unrelated server-side behavior. The one
allowed prerequisite is the minimal additive, wire-compatible resume-negotiation extension to the
existing messages: `CLIENT_RESUME.core_version`/`profiles`, and
`SERVER_RESUME_OK`/`SERVER_RESYNC_REQUIRED.required_profiles`, `optional_profiles`, and
`extension_namespaces`, with fail-closed validation before subscription or data-plane traffic. All
other work remains a UI layer over existing transport/session APIs. A literal revision-N cold
resume is also out of scope: it requires one atomic durable checkpoint of the semantic replica,
outbox, pending event and text state, terminal offsets, resource-continuity state, negotiated
capabilities, and extension/Terminal namespace and type mappings, and MUST NOT be approximated
from saved-list presentation metadata.

Verification:
- connect to a fresh host/user with no prior saved session: a new session is established and an
  entry is added to the saved list afterward with its session_id recorded;
- disconnect and reconnect without quitting: confirm `CLIENT RESUME` uses the same `session_id`
  and `client_instance_id`, the actual committed replica revision and event frontier, retained
  terminal offsets, negotiated capabilities, and extension/Terminal namespace and type mappings
  from the process-local continuity state. Exercise an extension-bearing or Terminal session and
  confirm a recreated controller does not fall back to fresh negotiation;
- quit and relaunch the client, then reconnect via the saved entry without a durable checkpoint:
  confirm (via a log/test hook) that it attempts Task 23's `CLIENT RESUME` with the remembered
  `session_id`, a fresh `client_instance_id`, revision 0, event frontier 0, empty terminal/text
  continuity state, and the current `core_version`/supported `profiles` — never the saved last-known
  revision. Exercise a Terminal session and both journal replay and same-session snapshot resync:
  confirm each resume response re-advertises the authoritative profile sets and Terminal namespace,
  the client validates and installs that mapping before replay/snapshot/Terminal data, and the UI is
  rebuilt to the session's actual current state;
- restart the remote sessiond (or otherwise force a `REPLACED` continuity outcome) and then
  reconnect via a saved Terminal entry: confirm `SERVER_RESYNC_REQUIRED{continuity=REPLACED}`
  carries the replacement's required/optional profiles and extension namespaces, the client
  validates and installs the replacement Terminal mapping before its snapshot/data plane, the UI
  clearly communicates that the session was replaced rather than presenting it as a normal resume,
  and the saved entry is updated to the new `session_id`;
- double-click Connect on the same saved entry in quick succession: confirm only the newer resume
  attempt's outcome is reflected in the UI and no duplicate side effects or duplicated windows
  result from the superseded attempt;
- point a saved or new entry at a host presenting a changed/unknown host key: confirm the app
  blocks the connection with a clear dialog instead of connecting;
- delete a saved entry, then confirm (e.g. by checking sessiond directly, or reconnecting fresh)
  that the underlying remote session was unaffected — deletion is purely local bookkeeping;
- Commit the result.
```
