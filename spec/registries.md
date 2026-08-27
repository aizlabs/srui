# SRUI Registries Specification

This specification defines the canonical, language-agnostic registry for **Namespace 0** (the standard registry) and outlines the ID-assignment rules, namespace architecture, and modification procedures for SRUI (Semantic Remote UI).

---

## 1. Overview and Purpose

The protocol registry ([`protocol/registry.yaml`](file:///Users/zozulya/github/srui/protocol/registry.yaml)) serves as the single source of truth for standard semantic primitives in SRUI:
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
     - `enums`: Each enum type scopes its own value IDs $1 \ldots K$
     - `events`: IDs $1 \ldots E$
     - `operations`: IDs $1 \ldots O$

---

## 4. Registry Categories

### 4.1 Node Types (§7.2, §7.3)
Enumerates standard semantic nodes across tiers:
- **Required Tier (§7.3)**: `Surface`, `Row`, `Column`, `Grid`, `Spacer`, `Separator`, `Scroll`, `Text`, `RichText`, `Button`, `Toggle`, `TextInput`, `TextArea`, `Progress`, `Image`, `List`, `Table`, `Tree`.
- **SHOULD Tier (§7.3)**: `Select`, `ChoiceGroup`, `Slider`, `NumberInput`, `Tabs`, `Split`.
- **Standard Containers & Deferred (§7.2, §7.3)**: `Dialog`, `Menu`, `Toolbar`.

> **Admission Rule for Standard Widgets (§7.2)**: A widget belongs in the Standard Profile only if its meaning and state machine are stable across multiple major desktop toolkit families (AppKit, WinUI, GTK, SWT), its behavior is specified without reference to a particular drawing API, and it supports accessibility mapping.

### 4.2 Common Properties (§7.4)
Divided into five structural categories:
1. **Identity & Accessibility**: `label`, `accessible_description`, `role`, `value_description`, `actions`.
2. **Common State**: `visibility`, `enabled`, `read_only`, `busy`, `selected`, `validation_state`.
3. **Content**: `text`, `value`, `placeholder`, `resource`, `items`, `model_ref`.
4. **Layout Intent**: `horizontal_alignment`, `vertical_alignment`, `grow`, `shrink`, `minimum_size`, `maximum_size`, `preferred_size`, `spacing_role`, `padding_role`.
5. **Control-Specific & Semantic Metadata**: `presentation_hint`, `action_key`, `columns`, `selection_mode`.

### 4.3 Standard Enums (§7.5, §7.2, §7.4)
- `TextRole`: `title`, `heading`, `body`, `caption`, `code`, `status`, `warning`, `error`.
- `ActionRole`: `normal`, `primary`, `destructive`, `quiet`.
- `InputRole`: `plain`, `search`, `secure`, `command`.
- `Importance`: `normal`, `emphasized`, `de_emphasized`.
- `TogglePresentationHint`: `automatic`, `checkbox`, `switch`.
- Supporting layout & state enums: `Visibility`, `SpacingRole`, `PaddingRole`, `HorizontalAlignment`, `VerticalAlignment`, `SelectionMode`, `ValidationState`.

### 4.4 Standard Events (§7.6, §7.7)
- **Semantic Events**: `ACTIVATE`, `VALUE_CHANGED`, `SELECTION_CHANGED`, `EXPANSION_CHANGED`, `TEXT_EDIT`, `VIEWPORT_CHANGED`.
- **Coordinate Pointer Events**: `POINTER_DOWN`, `POINTER_UP`, `POINTER_MOVE`, `POINTER_CANCEL`, `POINTER_SCROLL` (applicable only to custom scene nodes that explicitly subscribe to coordinate streams).

### 4.5 Core Mutation Operations (§13)
- **Required Core Operations**: `CREATE_NODE`, `DELETE_NODE`, `SET_PROPERTY`, `CLEAR_PROPERTY`, `COMMIT`.
- **Standard Model Operations**: `CREATE_MODEL`, `MODEL_INSERT`, `MODEL_DELETE`, `MODEL_UPDATE`, `MODEL_RESET_RANGE`.
- **Optional Optimization Operations**: `MOVE_NODE`, `REORDER_CHILDREN`, `BATCH_PROPERTY_SET`.

---

## 5. Safe Modification Procedure

When adding a new primitive to Namespace 0:

1. **Verify Admission Criteria**:
   - Ensure the candidate primitive satisfies the admission rules defined in §7.2.
   - If the primitive is specific to a single domain or experimental workflow, define it within an **Extension Profile** rather than Namespace 0.

2. **Assign Next Monotonic ID**:
   - Locate the target category in [`protocol/registry.yaml`](file:///Users/zozulya/github/srui/protocol/registry.yaml).
   - Find the current maximum ID ($ID_{\max}$) in that category.
   - Assign $ID_{\text{new}} = ID_{\max} + 1$. Do NOT insert between existing IDs.

3. **Complete Metadata**:
   - Add a descriptive `name`, appropriate `tier` or `category`, `value_type` (for properties), and human-readable `description`.

4. **Validate**:
   - Run the automated registry validation script:
     ```bash
     python3 protocol/validate_registry.py
     ```
   - Verify that:
     - No duplicate IDs exist.
     - No sequential gaps exist.
     - All required-tier primitives remain present.

5. **Update Code Generation & Specs**:
   - Update protocol documentation in `spec/` and downstream code generators in subsequent development tasks.
