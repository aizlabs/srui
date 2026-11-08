# SRUI Registries Specification

This specification defines the canonical, language-agnostic registry for **Namespace 0** (the standard registry) and outlines the ID-assignment rules, namespace architecture, and modification procedures for SRUI (Semantic Remote UI).

---

## 1. Overview and Purpose

The protocol registry ([`protocol/registry.yaml`](../protocol/registry.yaml)) serves as the single source of truth for standard semantic primitives in SRUI:
- **Node Types**: Standard layout containers, content controls, and collection views.
- **Common Properties**: Typed semantic properties covering identity, accessibility, common state, content, layout intent, and control-specific descriptors.
- **Standard Enums**: Type-safe enumerations communicating presentation intent, roles, alignment, and sizing without specifying raw pixel rendering.
- **Standard Events**: Semantic input events and scene coordinate pointer events reported from client to server.
- **Core Mutation Operations**: Atomic tree and model mutation operations executed within transactions.

Using compact, permanently assigned numeric identifiers on the wire ensures:
1. **Steady-state wire efficiency**: Scalar mutations and semantic events are encoded in tiny payloads without repeating long string identifiers.
2. **Deterministic session replay**: Identifiers remain stable across network reconnects and journal replays.
3. **Cross-language interoperability**: Server implementations (Rust, etc.) and client renderers (macOS Swift, GTK, Windows, etc.) share identical numeric semantics.

---

## 2. Namespace Model (§6.4)

SRUI partitions all type and property references into namespaces. Wire references are encoded as a pair:

$$\text{Reference} = (\text{namespace\_id}, \text{local\_id})$$

### 2.1 Namespace 0: The Standard Registry
- **Namespace `0`** is permanently reserved for the canonical SRUI standard registry defined in `protocol/registry.yaml`.
- All standard node types, common properties, enums, events, and core operations reside in Namespace 0.
- Standard semantics are universal across all compliant SRUI clients and servers.

### 2.2 Extension Namespaces
- Extensions use globally unique, canonical reverse-domain names, for example:
  - `org.srui.terminal/1`
  - `org.srui.vector-scene/1`
  - `org.example.coding/2`
- During capability negotiation in the connection handshake (§15), the client and server negotiate active extensions and map each extension to a compact session-local namespace integer ($\ge 1$).
- **Rule of Non-Interference**: An extension MUST NOT redefine, shadow, or alter the meaning of any standard node type or property in Namespace 0.

---

## 3. ID Assignment Rules & Invariants

To guarantee long-term protocol stability and backwards compatibility, the following rules are strictly enforced for Namespace 0:

1. **Monotonic Sequential Allocation**:
   - Numeric IDs within each category start at base ID `1` and increment contiguously ($1, 2, 3, \ldots, N$).
   - Every newly added entry must be assigned the next sequential integer:
     $$\text{ID}_{\text{new}} = \text{ID}_{\max} + 1$$

2. **Append-Only & Permanent Immutability**:
   - Once an ID is assigned in `registry.yaml`, it is **permanently immutable**.
   - IDs must **never be reused, shifted, or renumbered**, even if a node type, property, or enum value is deprecated or obsoleted in a future protocol version.

3. **No Accidental Gaps**:
   - Gaps in the sequential ID space indicate accidental deletions or indexing errors. The registry validation tool verifies that all ID sequences are strictly contiguous.

4. **Category-Scoped Numbering**:
   - Numeric IDs are scoped independently within each registry category:
     - `node_types`: IDs $1 \ldots N$
     - `properties`: IDs $1 \ldots M$
     - `enums`: Each enum **type** has a monotonic type id (`enums[].id`); value IDs are scoped per type ($1 \ldots K$)
     - `events`: IDs $1 \ldots E$
     - `operations`: IDs $1 \ldots O$
   - `EnumValue.enum_id` on the wire references `enums[].id`; `EnumValue.value_id` references `enums[].values[].id`.

---

## 4. Registry Categories

The authoritative symbol tables live in [`protocol/registry.yaml`](../protocol/registry.yaml). The summaries below are descriptive; when they differ from the registry file, the registry file wins. Automated validation is enforced by `protocol/validate_registry.py`.

### 4.1 Node Types (§7.2, §7.3)
The canonical registry defines 27 node types with `tier` metadata (`required`, `should`, `standard`, `deferred`) and `category` metadata (`container`, `layout`, `text`, `control`, `content`, `collection`, `shell`).

Design-doc tier groupings for reference:
- **Required Tier (§7.3)**: 18 node types including `Surface`, `Row`, `Column`, `Grid`, `Text`, `Button`, `List`, `Table`, and `Tree`.
- **SHOULD Tier (§7.3)**: 6 node types including `Select`, `ChoiceGroup`, `Slider`, `Tabs`, and `Split`.
- **Standard Containers & Deferred (§7.2, §7.3)**: `Dialog`, `Menu`, and `Toolbar`.

> **Admission Rule for Standard Widgets (§7.2)**: A widget belongs in the Standard Profile only if its meaning and state machine are stable across multiple major desktop toolkit families (AppKit, WinUI, GTK, SWT), its behavior is specified without reference to a particular drawing API, and it supports accessibility mapping.

### 4.2 Common Properties (§7.4, §8)
The canonical registry defines 34 standard properties in five categories:
1. **Identity & Accessibility** (§7.4)
2. **Common State** (§7.4)
3. **Content** (§7.4)
4. **Layout Intent** (§7.4)
5. **Control-Specific & Semantic Metadata** (§7.7, §8): `presentation_hint`, `action_key`, `columns`, `selection_mode`, `shortcut`, `keep_visible`, and the model item properties `tree_depth`, `descendant_count`

Each property entry includes `value_type` metadata validated by the registry tool.

**Collection `value` — server-promoted selection (§6.3, §7.6, §8, §22.5).** On a `List`, `Table` or `Tree`, property 13 `value` is the selection the server has explicitly promoted from local presentation state to semantic state:

- **Value:** an `ItemId` (`Value.item_id_value`) only, never a row index. No `value` means no promotion. Every replica refuses any other value type with `invalid_collection_selection`.
- **Validity:** held only while `selection_mode` is `single` or `multiple`; setting `selection_mode` to `none`, or clearing it, while a promotion is held is refused. A replica does not check that the item is cached, because sparse caches may omit it. An authoritative session refuses a promotion naming an `ItemId` the collection's model does not hold (`item_not_found`), and clears in the same commit an earlier promotion whose item that commit removed.
- **"Absent" means absent from the authoritative model's cache.** A far `MODEL_RESET_RANGE` that evicts the promoted item past `max_cached_items_per_model` therefore clears the promotion. An application promoting an item it has not cached must send the item in the same transaction (for example as a `MODEL_RESET_RANGE`); otherwise the SDK returns `item_not_found`.
- **Pre-built commits:** a commit submitted through sessiond `commit_transaction` cannot gain the clearing operation, so one whose result would leave a dangling promotion is refused with `item_not_found` rather than cleared.
- **Rendering:** a committed operation setting `value` selects the item's row, replacing local selection, and reveals it once; other commits never move the selection or scroll. Withdrawing the promotion (clearing `value`) keeps any selection the user made after the promotion; only an untouched promoted row is deselected, without a `SELECTION_CHANGED`.
- **Fallback (§4 inv. 13 optional semantics):** a client without support ignores the collection `value` and keeps local-only selection, as §6.3 and §22.5 describe; `SELECTION_CHANGED` is still sent for user selections.

**`shortcut` — platform-neutral keyboard shortcut (property 31; §4 inv. 13, §7.4, §7.6, §7.7, §22.5, §22.8, §22.9, §26).** A shortcut is local input routing to an existing semantic outcome, never a new wire event:

- **Value:** a string in the closed canonical grammar `[primary+][shift+][alt+]<key>`, at most 32 bytes (`MAX_SHORTCUT_BYTES`). `<key>` is one lowercase ASCII letter `a`–`z` or digit `0`–`9`, or one of `return`, `escape`, `tab`, `space`, `delete`, `up`, `down`, `left`, `right`, `home`, `end`, `page_up`, `page_down`, `f1`–`f12`. Modifiers appear at most once and in that order, so each combination has exactly one spelling. `primary` is Command on macOS and Control elsewhere.
- **Bare keys:** every key except `f1`–`f12` requires `primary`, so a bare letter, digit or editing key never competes with a text editor, a collection or the key-view loop.
- **Parsing:** tokens are split on the `+` byte of the UTF-8 encoding and compared byte for byte (never by Unicode canonical equivalence). A rejected token is rendered in messages as printable ASCII, with `"` and `\` backslash-escaped and every other scalar as `\u{hex}`; `protocol/conformance-vectors/shortcut_grammar_cases.json` is the shared Rust/Swift parity table.
- **Failure (§4 inv. 13):** a non-string, empty, over-length, unknown-token (including upper case), duplicate-modifier, misordered-modifier, modifier-only or bare-key value is refused with `invalid_shortcut` on every decode path (node records, `SET_PROPERTY`, `BATCH_PROPERTY_SET`; vectors `malformed_shortcut_*.bin`) and again at apply.
- **Admission by node type:** `Button` — the client dispatches `ACTIVATE` through the same target/action path as a click; `Toggle` — it flips the value and sends the ordinary `VALUE_CHANGED`; `TextInput`, `TextArea`, `List`, `Table`, `Tree`, `RichText` — it moves local focus only and sends no event (§22.5). Any other node type is refused with `shortcut_not_admitted`.
- **Uniqueness:** within one window-owning `Surface` — the root of the node's tree, everything under it included — no two nodes hold the same shortcut. A nested `Surface` is rendered inline in the enclosing window, so it shares that scope; two root `Surface`s are separate scopes. `CREATE_NODE`, `SET_PROPERTY`, `BATCH_PROPERTY_SET` and a `MOVE_NODE` that changes scope are refused with `duplicate_shortcut`. Checks run per operation on every replica, so exchanging two shortcuts clears one first.
- **Client dispatch:** never while the first responder has marked text (IME), never for a disabled, hidden or collapsed node (or one under a hidden or collapsed ancestor), and never when the node's window is not key. The server still validates the resulting event like any other and refuses a forged event to a disabled node with `NodeDisabled`.
- **Reserved combinations:** a shortcut in the platform's reserved table is never bound and yields a renderer diagnostic. macOS: Cmd-Q, Cmd-W, Cmd-H, Opt-Cmd-H, Cmd-M, Cmd-Tab, Cmd-`, Cmd-comma, Cmd-C, Cmd-V, Cmd-X, Cmd-A, Cmd-Z, Shift-Cmd-Z, Cmd-Space, Ctrl-Cmd-F, Cmd-? (entries the grammar cannot express, such as Cmd-comma, are listed for completeness).
- **Discoverability (§22.8, §22.9):** the platform-formatted shortcut (macOS `⇧⌘R`) appears in the control's tooltip, its accessibility help and the semantic inspection snapshot.
- **Fallback (§4 inv. 13 optional semantics):** a client without support ignores the property; the node stays reachable by pointer, Tab and accessibility, and the application's behavior is unchanged.

**Hierarchical models — `tree_depth` and `descendant_count` (properties 32 and 33; §4 inv. 13, §6.3, §7.6, §8, §12.1, §22.5, §22.7, §26).** A `Tree` model's index space is the depth-first preorder of the whole forest. Both are **model item** properties (`uint64`; absent means 0):

- **Shape:** `tree_depth` is 0 for a root; `descendant_count` is the subtree size excluding the item, so item *i*'s descendants are exactly indices *i*+1 … *i*+`descendant_count`.
- **No new wire messages:** range requests, `MODEL_INSERT`/`MODEL_DELETE`/`MODEL_UPDATE`/`MODEL_RESET_RANGE`, selection promotion and server-pushed ranges keep flat index semantics. A subtree edit is an ordinary flat edit plus a `MODEL_UPDATE` of the affected ancestors' `descendant_count`; hierarchical models should delete by index range, because a sparse replica cannot shift its cached indices for an identity delete of an item it does not cache.
- **Validation (all-or-nothing, §12.1):** checked on every replica after the **last operation of a transaction that touches the model**, so the whole edit is judged at once and a failure is reported at that operation's index. The check is exact for sparse caches: a cache is accepted iff some valid forest of `item_count` items (roots at depth 0, every child one level below its parent, every depth below the limit) agrees with every cached item. Items are visited in index order and the first violation wins: (1) both values are non-negative integers (`invalid_model_tree_property`); (2) `tree_depth` < `max_model_tree_depth`, and an item with descendants has `tree_depth + 1 < max_model_tree_depth` (`max_model_tree_depth_exceeded`); (3) `index + descendant_count < item_count` (`model_tree_descendant_count_out_of_range`); (4) the depth is reachable through the preceding indices — index 0 is a root, depth grows by at most one level per index, and the position right after any cached item's span is no deeper than that item, uncached gaps included (`model_tree_depth_jump`); (5) a cached item directly after a cached item's span — a leaf's span is empty — is no deeper than that item, and an item inside the nearest enclosing cached span is deeper than its owner and ends inside it (`model_tree_outside_parent_span`). `protocol/conformance-vectors/model_tree_shape_cases.json` is the shared Rust/Swift/Python parity table, checked against a true-forest oracle; suite 1 vectors 70–88 replay the transaction semantics.
- **§26 limit `max_model_tree_depth`:** counts levels, default 256 (`DEFAULT_MAX_MODEL_TREE_DEPTH` / `defaultMaxModelTreeDepth`), so the deepest admitted `tree_depth` is 255. It bounds model data; `max_tree_depth` still bounds the semantic node tree. A model that never held either property is never shape-checked.
- **Local disclosure (§22.5):** items start expanded. Collapsing an item hides indices *i*+1 … *i*+`descendant_count`; the collapsed set is keyed by `ItemId`, only cached items can be collapsed, and entries whose items leave the cache are dropped. Uncached rows that remain visible show placeholders and are requested asynchronously by their flat indices (§22.7). A revealing promotion inside collapsed subtrees expands exactly the collapsed ancestors whose hidden intervals contain it, and reports each with `EXPANSION_CHANGED` (`value` = `true`), so the server learns the client's disclosure state.
- **`EXPANSION_CHANGED` (§7.6):** a user collapse or expand is reported without awaiting a reply, with `value` = the new state (`true` expanded) and `items` = a list of exactly one `ItemId` (never a row index). The server never drives disclosure; the SDK delivers it through `Tree::on_expansion_changed`, and `Event::expansion_change` reads it.
- **Columns:** `Tree` reads `columns` exactly as `Table` does, with the outline column first and a visible header.
- **SDK:** `TreeModel` flattens a server-side forest into preorder items, stamps both properties and refuses a forest deeper than the limit (`TreeModelError::DepthLimitExceeded`) before anything reaches a transaction.
- **Fallback (§4 inv. 13 optional semantics):** a client without support renders the preorder flat, with correct identity and selection.

**`keep_visible` — server-hinted keep-in-view item (§4 inv. 13, §6.3, §8, §12.1, §22.5, §22.7).** On a model-backed `List`, `Table` or `Tree`, `keep_visible` names one item the client keeps in view. It is presentation guidance from the server, never selection state:

- **Value:** an `ItemId` (`Value.item_id_value`) only, never a row index. Absent means no hint.
- **Validity (every replica, `invalid_keep_visible`):** held only by a `List`, `Table` or `Tree` that has a `model_ref`, evaluated on the resulting property set of `CREATE_NODE`, `SET_PROPERTY`, `CLEAR_PROPERTY` and `BATCH_PROPERTY_SET`; any other node type, any other value type, a collection without `model_ref`, and clearing `model_ref` while the hint is held are refused and the whole transaction is discarded. Item presence is not a replica rule, because sparse caches may omit the item. It is independent of `value` and `selection_mode`.
- **Authoritative settlement (same paths as the promoted selection):** a set naming an `ItemId` the collection's model does not hold is refused with `item_not_found`; a commit that removes the named item from the model (delete, range reset, positional update, `model_ref` change) clears the hint in the same commit; a pre-built sessiond `commit_transaction` that would leave it dangling is refused with `item_not_found`. Settling a hint never touches `value`, and settling a promotion never touches `keep_visible`.
- **Rendering:** setting, changing or clearing it never selects, deselects or replaces a local selection, posts no selection notification and emits no event. When a commit sets it (even to the same item) or changes the row index at which the item is shown, the client scrolls the minimum needed to make that row visible; a commit that leaves the row index unchanged never scrolls, so a local pan persists until the item moves. An uncached item (or one without a displayed row) is not scrolled to and nothing is requested for it; it is revealed when it arrives through range data. A newly mounted collection view has no local viewport to keep, so it starts with the hinted row revealed. Local selection, scrolling and keyboard navigation never clear or override the hint.
- **Fallback (§4 inv. 13 optional semantics):** a client without support ignores it and keeps its viewport where the user put it.

### 4.3 Standard Enums (§7.5, §7.2, §7.4, §8)
The canonical registry defines 12 standard enum types with monotonic type IDs ($1 \ldots 12$) and contiguous value IDs within each enum.
On the wire, enum values are represented by `EnumValue(enum_id, value_id)` where `enum_id` references the `StandardEnum` type ID (e.g. `ActionRole = 2`) and `value_id` references the value within that enum (e.g. `destructive = 3`).

Standard enum types:
1. `TextRole` (ID 1)
2. `ActionRole` (ID 2)
3. `InputRole` (ID 3)
4. `Importance` (ID 4)
5. `TogglePresentationHint` (ID 5)
6. `Visibility` (ID 6)
7. `SpacingRole` (ID 7)
8. `PaddingRole` (ID 8)
9. `HorizontalAlignment` (ID 9)
10. `VerticalAlignment` (ID 10)
11. `SelectionMode` (ID 11)
12. `ValidationState` (ID 12)

### 4.4 Standard Events (§7.6, §7.7)
Semantic and coordinate pointer events are defined with `kind` metadata (`semantic` or `coordinate`) with monotonic IDs ($1 \ldots 11$).
`EXPANSION_CHANGED` (ID 4) carries `value` (bool, the new disclosure state) and `items` (a list of exactly one `ItemId`); see the hierarchical models entry in §4.2.

### 4.5 Core Mutation Operations (§13)
Required core, model, and optimization operations are defined with `category` metadata (`required`, `model`, `optimization`) with monotonic IDs ($1 \ldots 13$).
On the wire, `Operation` oneof field tags match the operation registry IDs $1 \ldots 13$ 1:1, including `CommitOp` (tag 5).

---

## 5. Safe Modification Procedure

When adding a new primitive to Namespace 0:

1. **Verify Admission Criteria**:
   - Ensure the candidate primitive satisfies the admission rules defined in §7.2.
   - If the primitive is specific to a single domain or experimental workflow, define it within an **Extension Profile** rather than Namespace 0.

2. **Assign Next Monotonic ID**:
   - Locate the target category in [`protocol/registry.yaml`](../protocol/registry.yaml).
   - Find the current maximum ID ($ID_{\max}$) in that category.
   - Assign $ID_{\text{new}} = ID_{\max} + 1$. Do NOT insert between existing IDs.

3. **Complete Metadata**:
   - Add a descriptive `name`, appropriate `tier` or `category`, `value_type` (for properties), and human-readable `description`.

4. **Validate**:
   - Install Python tooling once with `uv sync --extra dev`.
   - Run the automated registry validation script:
     ```bash
     uv run python protocol/validate_registry.py
     ```
   - For CI/automation, use machine-readable output:
     ```bash
     uv run python protocol/validate_registry.py --json
     ```
   - Run the validator test suite:
     ```bash
     uv run pytest protocol/tests
     ```
   - Verify that:
     - No duplicate IDs exist.
     - No sequential gaps exist.
     - All required-tier primitives remain present.
     - Metadata fields (`tier`, `category`, `value_type`, `kind`) are valid.
     - `protocol/validate/conformance.py` remains in sync with `protocol/registry.yaml` (enforced by tests).

5. **Update Code Generation & Specs**:
   - Update normative prose in `spec/` when behavior changes.
   - Do not duplicate symbol tables into `spec/registries.md`; link to `protocol/registry.yaml` instead.
   - Update downstream code generators in subsequent development tasks.
