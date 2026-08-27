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
     - `enums`: Each enum type scopes its own value IDs $1 \ldots K$
     - `events`: IDs $1 \ldots E$
     - `operations`: IDs $1 \ldots O$

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
The canonical registry defines 30 standard properties in five categories:
1. **Identity & Accessibility** (§7.4)
2. **Common State** (§7.4)
3. **Content** (§7.4)
4. **Layout Intent** (§7.4)
5. **Control-Specific & Semantic Metadata** (§8): `presentation_hint`, `action_key`, `columns`, `selection_mode`

Each property entry includes `value_type` metadata validated by the registry tool.

### 4.3 Standard Enums (§7.5, §7.2, §7.4, §8)
The canonical registry defines 12 standard enums with contiguous value IDs. These include appearance roles (`TextRole`, `ActionRole`, `InputRole`, `Importance`), toggle presentation hints, and supporting layout/state enums (`Visibility`, `SpacingRole`, `PaddingRole`, `HorizontalAlignment`, `VerticalAlignment`, `SelectionMode`, `ValidationState`).

### 4.4 Standard Events (§7.6, §7.7)
Semantic and coordinate pointer events are defined with `kind` metadata (`semantic` or `coordinate`).

### 4.5 Core Mutation Operations (§13)
Required core, model, and optimization operations are defined with `category` metadata (`required`, `model`, `optimization`).

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
