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
The canonical registry defines 31 standard properties in five categories:
1. **Identity & Accessibility** (§7.4)
2. **Common State** (§7.4)
3. **Content** (§7.4)
4. **Layout Intent** (§7.4)
5. **Control-Specific & Semantic Metadata** (§7.7, §8): `presentation_hint`, `action_key`, `columns`, `selection_mode`, `shortcut`

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
- **Failure (§4 inv. 13):** a non-string, empty, over-length, unknown-token (including upper case), duplicate-modifier, misordered-modifier, modifier-only or bare-key value is refused with `invalid_shortcut` on every decode path (node records, `SET_PROPERTY`, `BATCH_PROPERTY_SET`; vectors `malformed_shortcut_*.bin`) and again at apply.
- **Admission by node type:** `Button` — the client dispatches `ACTIVATE` through the same target/action path as a click; `Toggle` — it flips the value and sends the ordinary `VALUE_CHANGED`; `TextInput`, `TextArea`, `List`, `Table`, `Tree`, `RichText` — it moves local focus only and sends no event (§22.5). Any other node type is refused with `shortcut_not_admitted`.
- **Uniqueness:** within one `Surface` (the nearest `Surface` ancestor-or-self, or the root when there is none; a nested `Surface` is its own scope) no two nodes hold the same shortcut. `CREATE_NODE`, `SET_PROPERTY`, `BATCH_PROPERTY_SET` and a `MOVE_NODE` that changes scope are refused with `duplicate_shortcut`. Checks run per operation on every replica, so exchanging two shortcuts clears one first.
- **Client dispatch:** never while the first responder has marked text (IME), never for a disabled, hidden or collapsed node (or one under a hidden or collapsed ancestor), and never when the node's window is not key. The server still validates the resulting event like any other and refuses a forged event to a disabled node with `NodeDisabled`.
- **Reserved combinations:** a shortcut in the platform's reserved table is never bound and yields a renderer diagnostic. macOS: Cmd-Q, Cmd-W, Cmd-H, Opt-Cmd-H, Cmd-M, Cmd-Tab, Cmd-`, Cmd-comma, Cmd-C, Cmd-V, Cmd-X, Cmd-A, Cmd-Z, Shift-Cmd-Z, Cmd-Space, Ctrl-Cmd-F, Cmd-? (entries the grammar cannot express, such as Cmd-comma, are listed for completeness).
- **Discoverability (§22.8, §22.9):** the platform-formatted shortcut (macOS `⇧⌘R`) appears in the control's tooltip, its accessibility help and the semantic inspection snapshot.
- **Fallback (§4 inv. 13 optional semantics):** a client without support ignores the property; the node stays reachable by pointer, Tab and accessibility, and the application's behavior is unchanged.

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
