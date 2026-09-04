# SRUI UI Gallery

One scrollable surface containing **every node type the AppKit renderer can build today**, a real
image delivered over the chunked resource path, a guided sequence of server-driven mutations, and
two live telemetry panels.

It is the visual counterpart to the protocol test suite: if a semantic feature works, it is
visible here. Where a feature does *not* work yet, the gallery says so on screen rather than
pretending (§4 inv. 13).

---

## Run it

Two terminals.

**Server** — from the repository root:

```bash
cargo run --manifest-path examples/ui-gallery/Cargo.toml -- \
  --socket /tmp/srui-ui-gallery.sock
```

**Client** — from the repository root:

```bash
swift run --package-path client-macos RendererDemoApp \
  --socket /tmp/srui-ui-gallery.sock
```

Server flags:

| Flag | Default | Meaning |
| --- | --- | --- |
| `--socket PATH` | `$XDG_RUNTIME_DIR/srui-ui-gallery.sock`, else `$TMPDIR/…` | Unix socket to bind |
| `--autoplay` | off | Start the scene tour immediately |
| `--autoplay-interval SECONDS` | `4` | Autoplay cadence |

All diagnostics go to stderr, so the binary is safe to bridge over an SSH subsystem where stdout
is the protocol stream (§19.1, §20.1). Only one server may own a socket path at a time; ownership
is enforced with `flock(2)`, not a connect probe.

### What you should see

- The Blue Marble photograph, decoded locally from bytes that arrived as 16 KiB resource chunks.
- Seven sections: hero, typography, controls, layout, collections, protocol inspector, connection.
- `Next Scene` / `Previous Scene` / `Reset` / `Auto Play` driving mutations **in place** — the
  window is never remounted, the scroll position never jumps.
- Every button press, toggle, and row selection appearing in the protocol inspector within the
  same revision as the change it caused.

---

## Support matrix

### Rendered by the gallery (18 node types, §7.3 required tier)

| Category | Node types | Where |
| --- | --- | --- |
| Layout | `Surface`, `Scroll`, `Column`, `Row`, `Grid`, `Spacer`, `Separator` | frame + layout section |
| Content | `Text`, `RichText`, `Image` | hero + typography sections |
| Controls | `Button`, `Toggle`, `TextInput`, `TextArea`, `Progress` | controls section |
| Collections | `List`, `Table`, `Tree` | collections + inspector sections |

`tests/gallery_test.rs::initial_graph_contains_every_supported_node_type` asserts this list is
exactly what the initial graph instantiates — no more, no less.

### Deliberately absent

`Dialog`, `Select`, `ChoiceGroup`, `Slider`, `NumberInput`, `Tabs`, `Split`, `Menu`, `Toolbar`.

These exist in `protocol/registry.yaml` but `ControlFactory` throws `unsupportedNodeType` for
them. Creating one would put a node in the authoritative tree that no client can render — exactly
the silent degradation §4 inv. 13 forbids. `unsupported_node_types_are_not_advertised` fails the
build if one ever appears.

### Honest limitations, labelled in the UI

| Path | Status | Why |
| --- | --- | --- |
| Text editing (`TextInput`, `TextArea`) | **native-local only** | The renderer does not emit `TEXT_EDIT`. Keystrokes never reach the server and are discarded by the next server-driven `SET_PROPERTY`. |
| Tree interaction | **presentation-only** | The outline is built from a flat inline `items` list. There is no hierarchical model type and no `EXPANSION_CHANGED` event, so expanding a row changes nothing on the server. |
| Round-trip latency | **not measured** | The server never sees the client's clock, and `EVENT_ACK` is emitted below the `Session` API. The connection panel reports server-side handling time only. |

---

## Scenes

| # | Scene | Demonstrates |
| --- | --- | --- |
| 1 | Baseline | the gallery as first published |
| 2 | Content | `SET_PROPERTY` on text, labels, roles, progress value and description |
| 3 | State | `enabled`, `read_only`, `visibility`, `busy`, `validation_state` |
| 4 | Image resource | `CLEAR_PROPERTY` on `resource`, placeholder fallback; advancing restores it |
| 5 | Collection models | `MODEL_INSERT`, `MODEL_UPDATE`, `MODEL_DELETE` across the list and the table |
| 6 | Structure | `CREATE_NODE`, `MOVE_NODE`, `REORDER_CHILDREN`, `DELETE_NODE` |
| 7 | Layout | spacing, padding, alignment, and size hints |

**Every scene is reversible.** A scene declares `apply` and an exactly matching `revert`; a
transition is `revert(current)` then `apply(next)` inside one transaction. The graph after any
route through the tour depends only on the scene currently applied, never on how it was reached
(`reaching_a_scene_by_any_route_produces_the_same_graph`).

**Reset is a revert, not a rebuild.** Node ids, model ids, and item ids survive, so the client
keeps every view it already has (`a_full_scene_cycle_and_reset_restore_the_baseline`).

One exception, by design: the structure scene allocates a **fresh** node id each time it runs.
`DELETE_NODE` retires an id permanently for the session incarnation, and the store rejects a
`CREATE_NODE` that resurrects one (§6.2). Ids come from the transient block at 900 and upward.

### Image restoration costs no transfer

Scene 4 clears the `resource` property; advancing to scene 5 sets the same hash back. The client
already holds those bytes in its content-addressed cache, so the restore is a 32-byte hash
reference, not a re-send (§14, §19.2).

---

## Protocol inspector

A `Table` over a bounded model showing the semantic traffic in both directions:

- **S→C** — every `Operation` this server commits.
- **C→S** — every `Event` that reached a handler, with its `event_seq` and `observed_revision`.

### Why it does not loop

Appending a log row is itself a mutation, so "log every transaction" would log its own log
forever. The inspector is instead **self-describing inside a single transaction**: it runs after
the caller has staged its operations but before the commit, reads `UiTransaction::operations()`,
and appends the rows describing them to the same transaction. The row and the change it describes
reach the client atomically at one revision (§12.1), and no second transaction exists to recurse
on.

The inspector's own `MODEL_INSERT`/`MODEL_DELETE` and the telemetry `SET_PROPERTY` operations are
deliberately not traced — they are bookkeeping about the traffic, not the traffic itself
(`inspector_does_not_describe_its_own_bookkeeping`).

### What it cannot show

Framing, the handshake, `EVENT_ACK`, and `RESOURCE_*` chunk frames. Those are produced by
`handle_connection` below the `Session` API and are never visible to an application. They are
absent rather than faked.

---

## Connection panel

Everything measured above the `Session` API, which is the highest layer an application can see.

| Metric | How it is obtained | Exact? |
| --- | --- | --- |
| Throughput | each committed operation list is re-encoded into the same `SruiMessage{Transaction}` envelope and varint length-delimited frame `handle_connection` writes | yes, before SSH encryption (§26) |
| Size histogram | five buckets over that framed size, drawn as `Progress` bars | yes |
| Server-side latency | handler entry → operations staged; min / p50 / p95 / max over a bounded window | yes, server work only |
| Client revision lag | `current_revision - event.observed_revision` | yes |
| Session facts | `attached_count`, `current_revision`, `journal_latest_revision`, `outbound_queue_capacity`, `retained_client_state_bytes` | yes |
| Resource transfer | published bytes and `ceil(bytes / 16 KiB)` chunks | yes |

Throughput reports transactions committed strictly **before** the one being rendered: a
transaction's framed size is only knowable once its operation list is final, which is after the
panel has been written. Latency does include the event that caused the current transaction.

All session accessors are read *before* the transaction opens. `Session::transaction` holds the
same inner mutex those accessors need, so capturing first is a deadlock-avoidance requirement,
not an optimisation.

---

## Gallery image

`assets/gallery.png` is *The Earth seen from Apollo 17* ("The Blue Marble"), NASA AS17-148-22727,
public domain, downscaled to 383×384 PNG. Full provenance, licence, derivation commands, SHA-256,
and the reason the classic "Lenna" image is **not** used: [`assets/NOTICE.md`](assets/NOTICE.md).

The bytes are embedded with `include_bytes!` so the binary and the tests always agree, and a
working-directory change cannot silently turn the hero image into a placeholder.

---

## Adding a widget

The crate is structured so a new widget is three edits and a test:

1. **`src/ids.rs`** — take the next free id in the owning section's 100-wide block, or the next
   free block for a whole new section.
2. **`src/ui.rs`** — extend the section's `build_*` function. Declare a `BASELINE_*` constant for
   any property a scene will mutate, and set that property explicitly at build time: a revert can
   only restore a value the baseline actually set.
3. **`src/scenes.rs`** — add a `Scene` variant plus its `name`, `summary`, `apply`, and `revert`
   arms. `apply` and `revert` must be exact inverses.
4. **`tests/gallery_test.rs`** — add the type to the coverage assertion if it is a new node type,
   and an operation-kind expectation if it is a new scene.

Nothing else changes. The inspector and the connection panel pick up new traffic automatically.

---

## Test and lint

```bash
# The repository has seen stale-mtime rebuild skips; touch first if results look impossible.
find . -name '*.rs' -not -path './server-rust/target/*' -exec touch {} +

cargo fmt --manifest-path examples/ui-gallery/Cargo.toml -- --check
cargo clippy --manifest-path examples/ui-gallery/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path examples/ui-gallery/Cargo.toml
```

### What the tests prove

| Test | Claim |
| --- | --- |
| `initial_graph_contains_every_supported_node_type` | all 18 renderable node types appear, and only those |
| `unsupported_node_types_are_not_advertised` | no unrenderable registry type is ever created |
| `every_section_and_collection_is_present` | all seven sections, both models, all eight text roles |
| `gallery_image_is_published_and_referenced_by_the_image_node` | the asset bytes are published and the node holds the resulting `ResourceHash` |
| `button_activation_commits_exactly_one_status_transaction` | one accepted event → exactly one transaction |
| `toggle_value_changed_echoes_the_authoritative_value` | the server echoes state rather than trusting the client's view |
| `collection_selection_is_resolved_against_authoritative_state` | an item the server does not hold never becomes a selection |
| `each_scene_exercises_its_intended_operation_kinds` | every scene emits the scalar / model / structural operations it claims |
| `image_scene_clears_and_restores_the_cached_resource` | restore re-references the same hash; the resource stays retained |
| `model_scene_mutates_and_restores_both_collections` | exact item-id order after insert/update/delete and after revert |
| `structure_scene_creates_moves_and_deletes_one_transient_node` | create → move → reorder → delete |
| `transient_node_ids_are_never_reused` | `DELETE_NODE` retires an id permanently |
| `a_full_scene_cycle_and_reset_restore_the_baseline` | a full cycle and an explicit reset restore the baseline graph and node identity |
| `reaching_a_scene_by_any_route_produces_the_same_graph` | revert-then-apply makes the tour path-independent |
| `inspector_records_both_directions_and_stays_bounded` | C→S and S→C rows, committed with the change, bounded at 40 rows |
| `inspector_does_not_describe_its_own_bookkeeping` | no self-referential logging |
| `connection_statistics_are_published_as_semantic_state` | real framed byte counts, exact chunk count, latency percentiles |
