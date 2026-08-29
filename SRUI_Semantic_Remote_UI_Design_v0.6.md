# SRUI — Semantic Remote UI Protocol

**Design and reference implementation**  
**Draft v0.6 — 29 August 2026**
**Normative core:** platform-neutral semantic protocol  
**Reference client:** macOS, Swift + AppKit  
**Reference server:** Rust, Unix-like hosts  
**Initial secure transport:** SSH subsystem, no PTY

> **Core idea:** replicate the *meaning and authoritative state* of a remote application's UI. Render that state locally at native speed. Network traffic should be proportional to application-state changes, not visual complexity or display refresh rate.

---

## 1. Status and architectural review

This revision is based on the earlier SRUI v0.1 draft and the RemoteUI research by Daniel Thommes and collaborators. The v0.1 architecture had the right central ideas—replicated semantic state, stable node identities, atomic mutations, native rendering, terminal compatibility, SSH transport, and reconnect—but it mixed several concerns too closely in the same specification.

v0.6 preserves the v0.5 layer boundaries and defines TCP-style selective event settlement, a contiguous processed frontier, and bounded receive/send windows:

1. **Application model** — remote business/domain state.
2. **Semantic UI model** — platform-neutral UI state exposed by an application or toolkit adapter.
3. **Protocol Core** — identity, revisions, transactions, events, resources, sessions, capabilities, and errors. It knows nothing about AppKit, GTK, WinUI, SWT, SSH, or Protobuf.
4. **Standard Widget Profile** — portable semantics for the common controls shared by modern desktop UI toolkits.
5. **Extension Profiles** — Terminal, VectorScene, Coding, MediaSurface, and future domain-specific semantics.
6. **Wire Encoding** — one compact representation of the protocol; Protobuf is the reference encoding, not the semantic model.
7. **Transport Binding** — SSH is the first secure binding; a future QUIC/TLS binding does not change UI semantics.
8. **Client Runtime** — replicated state, resources, presentation state, event queue, and reconnect state.
9. **Renderer Profile** — macOS/AppKit is one renderer implementation, not part of the protocol definition.
10. **Server Runtime** — persistent semantic sessions, application adapters, transaction journal, resource store, PTY manager, and connection attachment.

This separation is a normative requirement. A future Windows, GTK, or SWT client must be able to implement SRUI without depending on any macOS concept.

### 1.1 v0.6 clarifications

v0.6 makes the following principles normative:

- **synchronize meaning/state, do not remotely render ordinary GUI**: the remote host publishes semantic state and the local client renders it using the local platform;
- the remote host is authoritative while the client retains a non-authoritative semantic replica and local presentation state;
- the UI is a persistent object graph updated by atomic mutations, not a sequence of complete documents or display frames;
- transaction `COMMIT` boundaries are state-consistency boundaries, **not render-frame boundaries**;
- standard input is semantic (`ACTIVATE`, `VALUE_CHANGED`, `SELECTION_CHANGED`) rather than coordinate-based;
- ordinary layout transmits intent and relationships rather than server-computed pixel geometry;
- text editing, IME composition, caret movement, selection, clipboard integration, and ordinary editing feedback are local;
- common widgets are semantic first, with exact drawing isolated to retained `VectorScene` extensions;
- the base client is a thin state-replication and rendering adapter, not a downloaded application runtime;
- the retained semantic tree is a useful local interface for rendering, accessibility, inspection, testing, and policy-gated automation;
- reconnect handshakes bind client identity and explicitly distinguish the same session incarnation from an authoritative replacement; selective event acknowledgements and a contiguous processed frontier preserve retry safety only within the confirmed incarnation.
---

## 2. Historical basis

SRUI modernizes the durable ideas from the RemoteUI project, particularly the 2012 IEEE paper and Daniel Thommes' 2016 PhD work:

- abstract UI descriptions rather than framebuffer transmission;
- replicated UI trees with stable object identity;
- incremental manipulation of the replicated tree;
- prioritized transfer and resource caching;
- native client-side rendering;
- local handling of interactions that do not require application semantics;
- asynchronous events back to the authoritative remote application.

The important modernization is that SRUI treats these ideas as a **general desktop protocol**, not an Android/mobile remoting mechanism, and gives reconnectability, extensibility, modern accessibility, terminal compatibility, and explicit security boundaries first-class status.

SRUI does **not** copy the 2016 thesis protocol or implementation. It reproduces the architecture at a cleaner abstraction boundary.

---

## 3. Goals

### 3.1 Primary goals

- **Semantic replication, not remote painting.** Do not remotely render the ordinary GUI. Synchronize the meaning and committed state of `Button`, `Table`, `TextInput`, `Progress`, `Tree`, and similar objects, then render them locally.
- **Native-speed local interaction.** Hover, pressed appearance, caret movement, selection, scrolling, inertial scrolling, focus rings, native menu behavior, and ordinary animation occur locally.
- **Remote authority.** Business state and committed semantic UI state are authoritative on the remote host.
- **No downloaded application runtime.** The client executes no server-provided JavaScript, bytecode, native plug-ins, shaders, or arbitrary scripts.
- **Toolkit neutrality.** The semantic model is the lowest useful common denominator across AppKit, Windows UI toolkits, GTK, SWT, and comparable systems.
- **Renderer freedom.** A platform renderer decides how a semantic control looks, lays it out using local metrics, and implements platform-native interaction. Ordinary server state does not prescribe pixels.
- **Steady-state efficiency and frame independence.** After initial synchronization, a scalar state change should normally require only a small mutation message. The wire protocol is independent of display refresh rate.
- **Connection independence.** A semantic application session may survive the loss of the SSH/TCP connection and resume on a new authenticated connection.
- **Terminal compatibility.** Existing shell/TUI applications remain usable through a Terminal extension node.
- **Progressive adoption.** A product may mix semantic-native regions with terminal compatibility regions.
- **Accessibility by construction.** Role, label, value, state, hierarchy, and actions are protocol data.
- **Extensibility.** New widget profiles and domain-specific nodes can be negotiated without redefining the core protocol.

### 3.2 Non-goals for the first implementation

- pixel-perfect reproduction of a remote desktop;
- arbitrary CSS or a browser-compatible layout engine;
- client-side application code downloaded from the server;
- full browser navigation semantics;
- unrestricted local clipboard/filesystem/device access;
- arbitrary fonts supplied by the server;
- full video/3D remoting;
- automatic semantic recovery from arbitrary terminal output;
- immediate support for every exotic widget in every toolkit.

---

## 4. Architectural invariants

1. The remote host owns authoritative application/domain state and the authoritative committed semantic UI revision.
2. The client retains a non-authoritative replica of committed semantic UI state plus local presentation state. The replica exists only to render, interact, inspect, cache, and resume efficiently; it never becomes application truth.
3. Presentation-only state (hover, pressed visuals, caret, IME composition, scroll momentum, focus rings, local animation, window chrome) is local by default and MUST NOT require server round trips.
4. Rendering and layout MUST NOT require synchronous network queries.
5. UI transactions are atomic at explicit commit boundaries.
6. Node identity is stable for the lifetime of a semantic session.
7. Ordinary semantic controls are represented by meaning and state, not by paint instructions.
8. Exact drawing is confined to explicit escape-hatch profiles such as `VectorScene`.
9. Pixels/video are confined to explicit media/surface profiles.
10. The base client executes no server-supplied executable code.
11. A client can render the first valid committed subtree before the complete UI has arrived.
12. Idle semantic UI generates no application traffic other than keepalive/session-management traffic.
13. Unknown required semantics fail explicitly; optional semantics are negotiated or have documented fallbacks.
14. A transport connection is not the same object as an SRUI application session.
15. Events that can cause application side effects must be deduplicatable across reconnect.
16. For ordinary semantic UI, the wire protocol has no concept of display frames. Revisions and commits describe consistent application/UI state; the local renderer may display that state at 60 Hz, 120 Hz, 240 Hz, or any other cadence.
17. Ordinary widget geometry is computed locally from semantic layout intent. Server-specified exact coordinates are reserved for explicit exact-geometry profiles such as `VectorScene`.
18. Server-provided semantic data MUST NOT contain executable client application logic. Trusted local automation may inspect the semantic tree and invoke actions only through the same policy and event path as user interaction.

---

# Part I — Platform-neutral protocol

## 5. Normative layers

```text
REMOTE HOST                                             LOCAL HOST

Application / toolkit
        |
        | domain state
        v
+----------------------+                         +----------------------+
| Semantic UI adapter  |                         | Client runtime       |
+----------+-----------+                         | replicated model     |
           |                                     +----------+-----------+
           | semantic operations                            |
           v                                                v
+----------------------+      secure transport   +----------------------+
| SRUI Protocol Core   | ======================> | SRUI Protocol Core   |
| + Widget Profiles    | <====================== | + Widget Profiles    |
+----------+-----------+          events         +----------+-----------+
           |                                                |
        encoding                                         renderer API
           |                                                |
           v                                                v
+----------------------+                         +----------------------+
| Transport binding    |                         | Renderer             |
| SSH initially        |                         | AppKit initially     |
+----------------------+                         +----------------------+
```

### 5.1 What each layer is allowed to know

| Layer | May know | Must not require |
|---|---|---|
| Application adapter | application data, toolkit objects | client toolkit classes |
| Semantic model | semantic node types, properties, actions | AppKit/WinUI/GTK/SWT class names |
| Protocol Core | IDs, revisions, transactions, events, sessions, resources | widget appearance or transport |
| Widget Profile | portable control semantics | platform drawing APIs |
| Encoding | message structure and scalar encodings | transport security or renderer |
| Transport | bytes and connection lifecycle | widget types |
| Client runtime | semantic model + local state | remote application internals |
| Renderer | local framework and semantic nodes | remote process implementation |
| Server runtime | session persistence and application integration | client painting implementation |

This table is a conformance rule, not merely organizational advice.

---

### 5.2 Semantic-state boundary versus display remoting

SRUI's primary optimization is the abstraction boundary:

```text
remote application
    |
    | authoritative semantic state + mutations
    v
SRUI wire protocol
    |
    v
local semantic replica
    |
    v
local platform renderer
    |
    v
pixels
```

For ordinary UI, the remote side does **not** send paint commands, compositor surfaces, screenshots, or a stream of frames. The network representation is one abstraction level above drawing-oriented systems such as classical X11 and above surface/pixel-oriented remoting paths.

The normal SRUI cost model is therefore proportional to **semantic state change**, not to visual complexity or monitor refresh rate.

Examples:

```text
SET node=71 property=value value=0.72
SET node=88 property=text  value="91,821"
MODEL_INSERT model=4 item=927 value="connection opened"
COMMIT revision=18271
```

The client may render revision `18271` immediately, coalesce it with later revisions for display efficiency, or present it on the next local display refresh. None of those choices alter protocol semantics.

`VectorScene` and `MediaSurface` are explicit escape hatches for workloads that genuinely require exact geometry or pixels/video. Their existence does not weaken the semantic-first rule for ordinary controls.


## 6. Protocol Core

The **Protocol Core** contains no widget names. It defines the distributed-state machinery used by any profile.

### 6.1 Core objects

- **Session** — logical application/UI incarnation, optionally durable.
- **Connection** — transient transport attachment to a session.
- **Node** — identified semantic object with type, parent/children, and typed properties.
- **Model** — optional non-tree data source used by collections.
- **Resource** — content-addressed immutable binary object.
- **Transaction** — atomic change from revision `N` to `N+1`.
- **Event** — client-originated semantic user action.
- **Capability** — negotiated feature/profile identifier.
- **Extension namespace** — collision-free feature-specific type/property registry.

### 6.2 Node identity

Each semantic node has:

```text
node_id: uint64
node_type: TypeRef
parent_id: uint64 | null
ordered_children: [node_id]
properties: map<PropertyRef, Value>
```

Rules:

- `node_id` is session-scoped.
- A node ID MUST NOT be reused during the same session.
- A reconnect does not change node IDs if the session resumes.
- A full resync may preserve IDs but is not required to unless the server declares identity continuity.

Stable IDs make event routing, replay, accessibility identity, automation, presentation-state restoration, and reconnect deterministic.

### 6.3 State ownership: authoritative state versus local replica

SRUI is a replicated-state protocol, not a remote-paint protocol. The remote host remains authoritative, while the client keeps exactly the non-authoritative state needed to render and interact efficiently.

| State class | Owner / authority | Client behavior |
|---|---|---|
| Application/domain state | remote application | not replicated unless represented semantically |
| Committed semantic UI state | remote session | retained as a read-only/non-authoritative replica |
| Collection/model data needed by current UI | remote session | cached/virtualized replica, possibly partial |
| Presentation state | local client | owned locally; not synchronized unless explicitly promoted to semantic state |
| Render objects | local renderer | derived from semantic replica and disposable/rebuildable |
| Resource cache | content-addressed | local cache; never authoritative application state |

The client therefore **is not stateless**, but it also does not execute application policy. A local optimistic edit or toggle is provisional until the remote application publishes the corresponding authoritative semantic state.

A renderer may discard/rebuild local render objects at any time without changing protocol state. A client may also evict non-visible collection ranges or cached resources under policy, provided it can request/recover them asynchronously.

### 6.4 Type and property references

The standard namespace is namespace `0` and contains permanently assigned numeric IDs.

Extensions use canonical names such as:

```text
org.srui.terminal/1
org.srui.vector-scene/1
org.example.coding/2
```

During capability negotiation, both sides assign an extension a compact session-local namespace number. Wire references are then encoded as:

```text
(namespace_id, local_id)
```

This avoids transmitting long extension strings in steady state while preventing registry collisions.

An extension may not change the meaning of a standard node or standard property.

### 6.5 Values

The Core supports a deliberately small typed value set:

```text
null
bool
signed integer
unsigned integer
float64
string
node_id
item_id
resource_hash
enum token
size / range / point-like semantic tuples
list of scalar values
small typed record
```

Large blobs are resources, not properties.

---

## 7. Standard Widget Profile 1

The Standard Widget Profile defines portable user-interface semantics. It is independent of any renderer framework.

The profile deliberately standardizes **behavior and meaning**, not visual appearance.

### 7.1 Design rule: semantic control, local appearance

For example, the server sends:

```text
Button #42
  label = "Delete"
  role = destructive
  enabled = true
```

It does not send:

```text
background = #ff3b30
cornerRadius = 7
font = San Francisco 13
x = 812
width = 93
```

A macOS renderer, Windows renderer, GTK renderer, and SWT renderer may all draw different-looking controls while preserving the same user-visible meaning and action semantics.

### 7.2 Portable control vocabulary

The following set reflects the controls that recur across modern desktop toolkits. The **semantic standard is broader than the v0.1 implementation tier** so the protocol does not need redesign when another renderer is added.

| Semantic node | Meaning | AppKit example | Windows example | GTK 4 example | SWT example |
|---|---|---|---|---|---|
| `Surface` | top-level content/window surface | `NSWindow` content | Window | `GtkWindow` | `Shell` |
| `Dialog` | transient modal/modeless surface | panel/sheet/window | ContentDialog/dialog | `GtkWindow`/dialog API | `Shell` |
| `Row` / `Column` | ordered layout | `NSStackView` | StackPanel | `GtkBox` | Composite + Row/GridLayout |
| `Grid` | row/column layout | Auto Layout/custom | Grid | `GtkGrid` | GridLayout |
| `Spacer` | flexible empty layout item | view/layout guide | spacer/grid sizing | expanding widget | layout data |
| `Separator` | semantic visual grouping | separator view | separator/border | `GtkSeparator` | separator control/style |
| `Text` | non-editable text | label `NSTextField` | TextBlock/static | `GtkLabel` | `Label` |
| `RichText` | selectable structured text | `NSTextView` | RichTextBlock/RichEdit | text view/buffer | StyledText |
| `Button` | momentary action | `NSButton` | Button | `GtkButton` | `Button(PUSH)` |
| `Toggle` | boolean state | `NSSwitch` / checkbox | ToggleSwitch/CheckBox | `GtkSwitch`/`GtkCheckButton` | `Button(CHECK/TOGGLE)` |
| `ChoiceGroup` | mutually exclusive visible choices | radio buttons/segments | RadioButtons | grouped check buttons | `Button(RADIO)` |
| `Select` | choose from compact set | `NSPopUpButton` / `NSComboBox` | ComboBox | `GtkDropDown` | `Combo` |
| `Slider` | choose numeric value from range | `NSSlider` | Slider | `GtkScale` | `Scale` |
| `NumberInput` | editable numeric value with constraints/step | field + `NSStepper` | NumberBox/spinner | `GtkSpinButton` | `Spinner` |
| `TextInput` | single-line text editing | `NSTextField` | TextBox | `GtkEntry` | `Text` |
| `TextArea` | multi-line text editing | `NSTextView` | multiline TextBox | text view | multi-line `Text`/StyledText |
| `Progress` | determinate/indeterminate progress | `NSProgressIndicator` | ProgressBar/Ring | `GtkProgressBar` | `ProgressBar` |
| `Image` | raster/vector image resource | `NSImageView` | Image | `GtkPicture` | Label/custom image control |
| `Scroll` | scrollable viewport | `NSScrollView` | ScrollViewer | `GtkScrolledWindow` | `ScrolledComposite`/Scrollable |
| `List` | virtualized one-dimensional collection | `NSTableView` | ListView | `GtkListView` | `List`/`Table` |
| `Table` | row/column collection | `NSTableView` | list/grid control | `GtkColumnView` | `Table` |
| `Tree` | hierarchical collection | `NSOutlineView` | TreeView | tree-list model + list view | `Tree` |
| `Tabs` | mutually exclusive pages | `NSTabView`/controller | TabView/tab control | notebook/tab UI | `TabFolder` |
| `Split` | user-resizable pane division | `NSSplitView` | split/grid splitter | `GtkPaned` | `Sash`/SashForm |
| `Menu` | command collection | `NSMenu` | MenuFlyout/menu | menu model/popover | `Menu` |
| `Toolbar` | primary command strip | `NSToolbar` | CommandBar/toolbar | header/action bar | `ToolBar` |

The class names above are **informative mappings**, not protocol requirements. A renderer may use a custom lightweight implementation if it preserves the node's semantics, accessibility, input behavior, and state.

#### Admission rule for standard widgets

A semantic control belongs in the Standard Widget Profile only when all of the following are true:

1. its user-facing meaning and state machine are stable across multiple major toolkit families (at minimum desktop-native plus one independent cross-platform toolkit family);
2. its behavior can be specified without reference to a particular renderer class or drawing API;
3. it has a reasonable accessibility role/action mapping across platforms;
4. a renderer can adapt its appearance without changing application meaning; and
5. clients lacking a preferred presentation can still render a behaviorally correct fallback.

If a concept fails these tests, it belongs in an extension profile or is represented as a standard semantic primitive plus an advisory presentation hint. This rule is intended to keep the standard profile small while allowing it to grow deliberately.

Names in application SDKs do not have to match wire node names one-for-one. For example, an SDK may expose a general `Input(...)` convenience API that lowers to `TextInput`, `NumberInput`, `Select`, or another standard semantic control.

#### Checkbox and switch are one semantic state machine

A checkbox and a switch both represent a user-editable boolean value. SRUI therefore standardizes one `Toggle` semantic node rather than two wire-level state machines. A toolkit-facing SDK MAY expose convenience constructors such as `Checkbox(...)` and `Switch(...)`; they lower to:

```text
Toggle {
  value = true | false
  presentation_hint = checkbox | switch | automatic
}
```

The hint is advisory. The renderer may choose the locally appropriate control while preserving boolean semantics and accessibility. This keeps the protocol at the common semantic denominator instead of encoding platform fashion as protocol identity.

### 7.3 Required v0.1 implementation tier

The first macOS client and reference server MUST implement:

```text
Surface
Row / Column / Grid
Spacer / Separator
Text / RichText
Button
Toggle
TextInput / TextArea
Progress
Image
Scroll
List / Table
Tree
```

It SHOULD implement:

```text
Select
ChoiceGroup
Slider
NumberInput
Tabs
Split
```

`Menu` and `Toolbar` may be deferred until the application shell requires them.

### 7.4 Common semantic properties

Properties are split into four categories.

#### Identity/accessibility

```text
label
accessible_description
role
value_description
actions
```

#### Common state

```text
visibility = visible | hidden | collapsed
enabled
read_only
busy
selected
validation_state
```

#### Content

```text
text
value
placeholder
resource
items/model_ref
```

#### Layout intent

```text
horizontal_alignment
vertical_alignment
grow
shrink
minimum_size
maximum_size
preferred_size
spacing_role
padding_role
```

`spacing_role` and `padding_role` prefer semantic values such as `none`, `tight`, `normal`, `relaxed`. A renderer maps them to platform metrics. Explicit logical sizes are allowed when necessary but are hints for ordinary semantic widgets, not pixel-perfect mandates.

### 7.5 Standard appearance roles

Portable roles communicate intent without specifying paint:

```text
TextRole:
  title | heading | body | caption | code | status | warning | error

ActionRole:
  normal | primary | destructive | quiet

InputRole:
  plain | search | secure | command

Importance:
  normal | emphasized | de_emphasized
```

A renderer may decline an appearance hint if the local platform has no meaningful equivalent, while preserving accessibility and behavior.

### 7.6 Standard events

| Node | Local behavior | Event to server |
|---|---|---|
| `Button` | press/hover/focus animation | `ACTIVATE(node_id)` |
| `Toggle` | visual toggle may be optimistic | `VALUE_CHANGED(node_id, bool, edit_seq)` |
| `ChoiceGroup` | local selection indication | `SELECTION_CHANGED(node_id, item_id, edit_seq)` |
| `Select` | local popup/dropdown interaction | `SELECTION_CHANGED(...)` |
| `Slider` | dragging local | throttled/final `VALUE_CHANGED(...)` |
| `NumberInput` | local editing/stepper feedback | `VALUE_CHANGED(...)` |
| `TextInput` | IME, caret, selection, typing local | `TEXT_EDIT(...)` or full value |
| `TextArea` | local editing/scrolling | `TEXT_EDIT(...)` |
| `List/Table/Tree` | local selection highlight | `SELECTION_CHANGED(...)` |
| `Tree` | local disclosure animation | `EXPANSION_CHANGED(...)` |
| `Tabs` | local tab animation | `SELECTION_CHANGED(...)` |
| `Split` | resize locally | optional `VALUE_CHANGED` / layout event |
| `Surface` | local window resize | async `VIEWPORT_CHANGED` if subscribed |

The event is semantic whenever possible. Raw pointer coordinates are reserved for custom-rendering profiles.

### 7.7 Semantic input routing

For ordinary controls, the client reports **what happened to which semantic object**, not where the pointer happened to be. A button activation is therefore represented conceptually as:

```text
Event {
  event_seq = 712
  event_id = <stable retry-safe id>
  observed_revision = 104
  node_id = 183
  type = ACTIVATE
}
```

It is deliberately **not**:

```text
mouse_down x=483 y=292
mouse_up   x=483 y=292
```

The server resolves `node_id=183` and `ACTIVATE` against the currently committed node and its registered server-side handler. The client never receives or executes a shell command such as `restart nginx`.

An application-facing SDK may expose an opaque semantic action key for debugging, automation, or adapter convenience:

```text
Button {
  id = 183
  label = "Restart"
  enabled = true
  action_key = "restart"
}
```

`action_key` is **data, not executable code**. It is meaningful only inside the remote application's authorization domain. The wire event remains an activation of a node/action capability, and the server validates that the action is still enabled and permitted at the event's observed revision.

For custom scene nodes, coordinate input is allowed only when the scene explicitly subscribes to pointer events:

```text
POINTER_DOWN {
  scene_node_id = 48
  pointer_id = 1
  x = 381.3
  y = 220.7
  button = primary
  modifiers = [...]
}
```

Coordinates are expressed in the scene's documented logical coordinate system, not global screen pixels. Pointer streams are rate-limited/coalescible where semantics permit.

For each `client_instance_id`, side-effect events use positive, contiguous `event_seq` values starting at
1. Once allocated, a sequence MUST be retained until terminal settlement; a client MUST apply
backpressure rather than discard an unacknowledged sequence or allocate beyond its negotiated send
window. A replay preserves both `event_id` and `event_seq`.

---

## 8. Collections and model data

Large collection data SHOULD NOT be represented as thousands of child view nodes.

A `List`, `Table`, or `Tree` references a **collection model** containing stable item IDs.

```text
Table #40
  model = 7
  columns = [name, cpu, memory]
  selection_mode = multiple

Model #7
  item_count = 500000
  items[visible cached ranges] = ...
```

The protocol supports:

- stable `item_id` values;
- insert/delete/update operations by item identity;
- local sorting/filtering only when explicitly allowed;
- asynchronous range requests for data not yet replicated;
- viewport hints that are asynchronous and never required for layout correctness;
- server-pushed high-priority visible ranges;
- renderer virtualization.

A million-row table therefore maps to one table control plus a data model, not one million native controls.

---

## 9. Rich text

`RichText` is structured content, not HTML.

The v0.1 representation is a sequence/tree of runs with semantic annotations:

```text
Paragraph
  Text("Build ")
  Span(role=code, "42f7ab")
  Text(" failed")
```

Initial inline semantics:

```text
plain
emphasis
strong
code
link (capability-gated URL action)
warning/error annotation
```

No arbitrary CSS is allowed. The renderer controls typography and line layout.

A future `CodeDocument` extension can provide syntax tokens, diagnostics, fold regions, and semantic selections without forcing those concepts into the base RichText model.

---

## 10. Layout model

The layout model is intentionally much smaller than CSS.

Core containers:

```text
Row
Column
Grid
Overlay (optional)
Scroll
Split
```

Core sizing concepts:

```text
fit content
fill available
minimum / preferred / maximum
relative grow/shrink
semantic spacing/padding
alignment
```

The server expresses **relationships and intent**. The client calculates actual pixel geometry using local font metrics, control metrics, accessibility text size, display scale, and platform conventions.

For ordinary semantic controls, this is preferred:

```text
Button {
  parent = footer
  horizontal_alignment = trailing
  importance = primary
}
```

and this is intentionally avoided:

```text
Button {
  x = 817
  y = 426
  width = 113
  height = 32
}
```

Explicit logical size hints are permitted when application meaning truly depends on size, but they do not turn the Standard Widget Profile into a coordinate-based drawing protocol.

Ordinary layout MUST NOT depend on the server asking the client synchronously for measured pixel sizes.

For workloads requiring exact coordinates or arbitrary vector drawing, use `VectorScene`; do not contaminate standard widget semantics with pixel positioning.

---

## 11. Extension profiles

Extensions are first-class rather than afterthoughts.

Examples:

```text
org.srui.terminal/1
org.srui.vector-scene/1
org.srui.media-surface/1
org.srui.coding/1
```

An extension defines:

- its node/property/event IDs within its namespace;
- required Core version;
- capability string and version;
- whether it can have a standard fallback subtree;
- local security permissions it may request;
- serialization constraints;
- conformance tests.

### 11.1 Fallback rule

A server MUST NOT send an unsupported extension as required content.

It may instead send:

```text
ExtensionNode(type=org.example.diff/1)
  fallback:
    Column
      Text(...)
      RichText(...)
```

A capable client renders the extension. A base client renders the fallback.

### 11.2 VectorScene

`VectorScene` is the escape hatch for arbitrary retained drawing. It is intentionally not part of the base widget layer.

Expected primitives:

```text
Path
Rect
Ellipse
TextRun
Image
Transform
Clip
Gradient
Group
```

It is a retained scene graph with incremental mutations, not an SVG/XML document parser and not a scripting environment.

A toolkit SDK MAY expose a convenience API named `Canvas`, but `Canvas` is **not** a separate immediate-mode wire primitive in the base protocol. The SDK must lower it to retained `VectorScene` objects and mutations. This avoids accidentally recreating frame-by-frame remote drawing, while still supporting arbitrary custom graphics.

---

## 12. Persistent object graph and mutation stream

SRUI does not continuously send complete UI documents. The normal steady-state object is a **persistent semantic object graph plus an ordered mutation stream**, conceptually closer to database replication than document navigation.

Initial synchronization or resynchronization may use a snapshot, but after that the server publishes atomic mutations:

```text
BEGIN_TXN base=0 new=1
  CREATE_NODE ...
  CREATE_NODE ...
  CREATE_NODE ...
COMMIT_TXN revision=1

BEGIN_TXN base=1 new=2
  SET_PROPERTY ...
  MODEL_UPDATE ...
COMMIT_TXN revision=2

BEGIN_TXN base=2 new=3
  DELETE_NODE ...
  CREATE_NODE ...
  SET_PROPERTY ...
COMMIT_TXN revision=3
```

The client retains the committed replica between transactions. Unchanged state is not retransmitted. The local renderer consumes dirty semantic nodes/models and updates only affected native render objects.

A full snapshot is a recovery/bootstrap mechanism, **not the normal rendering unit**. A server SHOULD NOT resend an entire tree merely because one scalar property changed.

### 12.1 Revisions and transactions

The server publishes monotonically increasing committed revisions.

```text
BEGIN_TXN base=104 new=105
  SET_PROPERTY node=4 value=0.71
  SET_PROPERTY node=3 text="Running tests"
  CREATE_NODE node=19 type=Text parent=2 index=3
  SET_PROPERTY node=19 text="27 tests passed"
COMMIT_TXN revision=105
```

Rules:

- A transaction is applied atomically to the semantic store.
- The client MUST NOT expose a half-applied transaction to the renderer.
- If a connection drops mid-transaction, the incomplete transaction is discarded.
- The reconnect request references the last **committed** revision only.
- A transaction may carry a priority class.

### 12.2 Commits are not frames

A transaction commit establishes an atomic semantic state revision:

```text
BEGIN base_revision=18270

SET cpu.value = 0.71
SET requests.value = 91821
MODEL_INSERT log item=927 value="connection opened"

COMMIT revision=18271
```

`COMMIT revision=18271` means:

> all listed mutations form one consistent semantic state that may now become visible.

It does **not** mean:

> draw frame 18271 now.

The local renderer owns display scheduling. It may render the committed state on a 60 Hz, 120 Hz, 144 Hz, 240 Hz, variable-refresh, or headless accessibility-only client. A fast sequence of committed revisions may be coalesced for painting when doing so does not violate observable semantic/event ordering.

The semantic protocol therefore MUST NOT introduce `START_FRAME`, `END_FRAME`, frame acknowledgements, or server-driven repaint cadence for Standard Widget Profile nodes.

Profiles whose domain is intrinsically time-based media may define media timestamps, but those are not UI transaction frames.


### 12.3 Progressive first paint

A full application snapshot need not be one giant transaction.

The server may commit independent valid subtrees:

```text
revision 1: Surface + main layout + prompt
revision 2: toolbar/actions
revision 3: initial table rows
revision 4: background images/resources
```

This allows rendering as soon as the first useful subtree is complete.

---

## 13. Core mutation operations

Required:

```text
CREATE_NODE
DELETE_NODE
SET_PROPERTY
CLEAR_PROPERTY
COMMIT
```

Standard model operations:

```text
CREATE_MODEL
MODEL_INSERT
MODEL_DELETE
MODEL_UPDATE
MODEL_RESET_RANGE
```

Optional optimization operations:

```text
MOVE_NODE
REORDER_CHILDREN
BATCH_PROPERTY_SET
```

The semantic result of optimized operations must be expressible using required operations.

---

## 14. Resource model

Binary resources are immutable and content-addressed by SHA-256.

```text
Image #31
  resource = sha256:9d4e...a10f
```

The client cache may persist across sessions.

Protocol properties include:

```text
hash
media_type
encoded_length
optional decoded dimensions
priority
```

Initial allowed media formats are platform-decodable raster images with explicit size limits. Arbitrary server-supplied fonts are not part of v0.1.

Resource messages are chunked so that a large image cannot block latency-sensitive UI/control traffic behind a multi-megabyte frame.

---

## 15. Capability negotiation

Capabilities are semantic feature identifiers, not renderer brands.

Example:

```text
CLIENT HELLO
  core_version = 1.0
  profiles = [
    org.srui.standard-widgets/1,
    org.srui.terminal/1,
    org.srui.richtext/1
  ]
  limits = {...}

SERVER WELCOME
  core_version = 1.0
  required_profiles = [org.srui.standard-widgets/1]
  optional_profiles = [org.srui.terminal/1]
  session = ...
```

A client is never required to announce `macos-appkit`; renderer identity is diagnostic only.

---

## 16. Reference wire encoding

The semantic specification is independent of the encoding.

The **required reference encoding for v0.1** uses Protocol Buffers with length-prefixed envelopes because implementations exist in Swift, Rust, C++, Java, Go, and other likely adapter languages.

Crucially, widgets are encoded through generic node/property records rather than an AppKit-shaped Protobuf hierarchy.

Illustrative schema:

```proto
message TypeRef {
  uint32 namespace_id = 1; // 0 = standard registry
  uint32 local_id = 2;
}

message PropertyRef {
  uint32 namespace_id = 1;
  uint32 local_id = 2;
}

message Value {
  oneof value {
    bool bool_value = 1;
    sint64 int_value = 2;
    uint64 uint_value = 3;
    double float_value = 4;
    string string_value = 5;
    uint64 node_id = 6;
    bytes hash256 = 7;
    EnumValue enum_value = 8;
    SmallRecord record_value = 9;
  }
}

message NodeRecord {
  uint64 node_id = 1;
  TypeRef type = 2;
  uint64 parent_id = 3;
  uint32 child_index = 4;
  repeated Property properties = 5;
}

message Transaction {
  uint64 base_revision = 1;
  uint64 new_revision = 2;
  repeated Operation operations = 3;
}

message Event {
  bytes client_instance_id = 1;
  uint64 event_seq = 2;
  bytes event_id = 3; // globally unique within session lifetime
  uint64 observed_revision = 4;
  uint64 node_id = 5;
  TypeRef event_type = 6;
  repeated Property arguments = 7;
}

enum EventAckStatus {
  EVENT_ACK_STATUS_UNSPECIFIED = 0;
  EVENT_ACK_STATUS_PROCESSED = 1;
  EVENT_ACK_STATUS_DUPLICATE = 2;
  EVENT_ACK_STATUS_REJECTED = 3;
}

message ServerEventAck {
  bytes client_instance_id = 1;
  bytes event_id = 2;
  uint64 last_processed_event_seq = 3;
  EventAckStatus status = 4;
  uint64 revision_after_effect = 5;
  string reject_reason = 6;
  string session_id = 7;
}
```

Tiny UI messages are not compressed. Large snapshots/resources may negotiate zstd, but compression must not delay high-priority control traffic.

---

# Part II — Sessions, transport, and server runtime

## 17. Session versus connection

A central v0.2 requirement is that **SRUI session lifetime is independent of SSH connection lifetime**.

```text
Semantic session
  session_id = 9f...
  revision = 1842
  application process = alive
  resource store = alive
  optional PTYs = alive

       ^ attach / detach
       |
SSH connection A ---- broken
SSH connection B ---- authenticated later, resumes same session
```

Session states:

```text
ATTACHED
DETACHED
TERMINATING
EXPIRED
```

A network failure moves a session from `ATTACHED` to `DETACHED`; it does not terminate the application unless policy explicitly says so.

`session_id` is an opaque, globally unique **incarnation token**, not a reusable application name.
A server process restart MUST mint a new token unless it atomically restores the authoritative
semantic state, transaction journal, event result cache, and per-client contiguous event frontiers
of the old incarnation. Reusing an ID after restoring only some of that state falsely authorizes
stale-event replay.

The server retains detached sessions for a configurable TTL or indefinitely when the user requests a persistent session.

---

## 18. Reconnect and resynchronization

The client stores:

```text
session_id
client_instance_id
last_applied_revision
last_acked_event_seq
per-terminal received stream offsets
```

In steady state each client event is settled by an acknowledgement:

```text
CLIENT EVENT
  client_instance_id = c17
  event_seq = 593
  event_id = e123

SERVER EVENT_ACK
  session_id = abc
  client_instance_id = c17
  event_id = e123
  last_processed_event_seq = 593
  status = PROCESSED
  revision_after_effect = 1843
```

On a new authenticated transport the client first asks to resume a specific incarnation:

```text
CLIENT RESUME
  session_id = abc
  client_instance_id = c17
  last_applied_revision = 1842
  last_acked_event_seq = 593
```

The client MUST retain pending events but MUST NOT replay them or generate new semantic events until
the server provides a machine-readable continuity decision. UI-state similarity is not a valid
substitute for this decision.

If the exact incarnation survived and its journal still covers the gap:

```text
SERVER RESUME_OK
  session_id = abc
  replay_from_revision = 1843
  last_processed_event_seq = 593
```

`SERVER RESUME_OK.session_id` MUST exactly equal the requested ID. The client first applies the
reported contiguous event frontier, then replays the remaining pending events with their original
`event_id` and `event_seq`. The complete replay batch is serialized before newly generated events.

If the same incarnation survived but its transaction journal no longer covers the gap:

```text
SERVER RESYNC_REQUIRED
  session_id = abc
  continuity = SAME_SESSION
  snapshot_revision = 2210
  last_processed_event_seq = 593
```

The client applies the event frontier, may replay remaining old events, discards its semantic
replica, and applies the consistent snapshot. New user events remain disabled until that snapshot
commits.

If the requested incarnation expired, crashed without durable restoration, or was otherwise
replaced:

```text
SERVER RESYNC_REQUIRED
  session_id = def
  continuity = REPLACED
  snapshot_revision = 17
  last_processed_event_seq = 0
```

The client MUST abandon every unresolved event and text edit belonging to the expired incarnation.
It resets its event outbox to the replacement session's reported frontier, discards the old semantic
replica, and applies the fresh authoritative snapshot. It MUST NOT replay an old event merely
because the server reports a lower frontier. This deliberately chooses possible loss of an
unacknowledged user intent across application/session failure over applying stale intent twice or
against unrelated state.

The server, not the client, decides continuity. The client MUST NOT infer replacement from a
snapshot looking “far away” from its prior state. An unknown or omitted continuity value is a
required-semantics failure and does not authorize replay. Resume attempts are generation-bound per
outbox: starting a newer attempt supersedes every older attempt, and a delayed response from a
superseded connection MUST NOT replay events, rebind the outbox, or enable new event allocation.

Local presentation state such as window geometry and scroll position may be restored after resync
only when the server declares compatible identity continuity.

### 18.1 Transaction journal

The server keeps a bounded journal of committed transactions.

Retention policy may be based on:

- transactions acknowledged by all attached clients;
- maximum memory/bytes;
- maximum revision age;
- time.

When a reconnect falls outside retained history, snapshot resync is mandatory.

### 18.2 Event deduplication

This is required for correctness around interrupted connections.

Consider:

```text
user clicks Delete
client sends ACTIVATE event e123
server performs delete
connection dies before ACK reaches client
```

The client must not send a second semantically independent delete after reconnect.

Every application-side-effect event therefore contains a stable `event_id`. The server keeps a bounded deduplication/result cache. Re-delivery of the same event returns the prior acknowledgement/result and does not re-run the action.

This gives retry-safe behavior across ambiguous disconnects.

**Acknowledgement is normative.** The dedupe cache and the acknowledgement are two different
mechanisms and neither replaces the other: the cache keyed on `(client_instance_id, event_id)`
provides *idempotency*, so a replay is safe; the ack provides *settlement*, so the client knows it
may stop replaying. Without the ack the retry set only ever grows and a bounded result cache eventually rolls.
The cache and frontier MUST remain valid for the lifetime of the session incarnation. If a server
restart cannot restore them together with authoritative state, that incarnation has expired and
the server returns `continuity = REPLACED`; the client then abandons its old retry set rather than
re-running it against the replacement session.

Rules:

- Pending events may be replayed only after `RESUME_OK` or a `SAME_SESSION` resync for the
  exact requested session incarnation. A `REPLACED` resync abandons the old retry set.
- `CLIENT HELLO` or `CLIENT RESUME` binds `client_instance_id` to the connection. An active
  `EVENT.client_instance_id` that differs from the bound identity MUST be rejected before any
  dedupe lookup or sequence update.
- Before a transport send can suspend, the client MUST retain the side-effect event in its pending
  set. All event writes, including a complete reconnect replay batch, MUST be serialized so
  increasing `event_seq` values reach the transport in allocation order.
- The sender and receiver maintain bounded sequence windows. A new sequence MUST be greater than
  `last_processed_event_seq` and within the negotiated receive window. Reusing a sequence for a
  different `event_id`, changing the sequence of a replay, or sending beyond the receive window is
  a protocol error. Window exhaustion applies backpressure; it MUST NOT evict an `IN_FLIGHT` event.
- A newly admitted event is `IN_FLIGHT` until validation and handler dispatch finish. An overlapping
  delivery of the same `(client_instance_id, event_id)` MUST NOT receive `PROCESSED`,
  `DUPLICATE`, or `REJECTED`; it stays pending at the client and may retry after the first
  execution settles.
- Every newly admitted event that settles, and every replay whose result is already settled, MUST
  be answered with exactly one `SERVER EVENT_ACK` on the connection that carried that delivery.
  Acks are control-class traffic (§19.2) and are never coalesced or dropped behind lower-priority
  traffic.
- Each terminal ack is a selective acknowledgement for its `event_id`. The client removes that
  event from its retry set even when an earlier sequence remains pending, but it MUST NOT advance
  `last_acked_event_seq` across the gap.
- `last_processed_event_seq` is the highest **contiguous** settled sequence, analogous to a TCP
  cumulative ACK; it is never merely the largest sequence observed. If sequence 2 settles while
  sequence 1 is pending, an ack for sequence 2 reports frontier 0. When sequence 1 later settles,
  the frontier advances directly to 2.
- A server that settles events out of order tracks the bounded set beyond the contiguous frontier.
  A client likewise tracks selectively acknowledged sequences beyond `last_acked_event_seq`.
  Either side advances its frontier only while the next sequence is known settled.
- The ack carries the session incarnation, bound `client_instance_id`, settled `event_id`,
  contiguous `last_processed_event_seq`, status, and `revision_after_effect` recorded in the
  result cache (Appendix B). A client MUST ignore an ack whose session or `client_instance_id`
  does not match its active outbox; this prevents a draining old connection from settling events
  allocated after a replacement session reset.
- `PROCESSED` — newly admitted, validated, and dispatched.
- `DUPLICATE` — a replay of an already-settled `event_id`; the ack returns the prior result and
  the action is not re-run. A replay of an event that was originally refused is answered
  `REJECTED` again with the original non-empty `reject_reason`.
- `REJECTED` — refused by event validation (unknown node, disabled node, future
  `observed_revision`). This is a terminal rejection of that event, not a protocol violation.
- An event without a stable non-empty `event_id` is a protocol error and MUST be rejected before allocating persistent per-client dedupe state.
- Acks are optional to consume: an unknown optional status still settles its `event_id`, but never
  authorizes the client to cross a sequence gap.
### 18.3 Pending text edits

Text editing can remain locally responsive while events are awaiting acknowledgement.

For v0.1:

- each editor has an `edit_seq`;
- the server applies monotonically increasing edits;
- the server can overwrite with an authoritative property update;
- pending edits may be replayed only when the same semantic session resumes;
- after full resync, unresolved local edits are not silently merged unless the application profile defines reconciliation.

---

## 19. SSH transport binding

SRUI does not invent authentication or cryptography in v0.1.

The preferred binding is an SSH **subsystem** or a fixed non-PTY exec endpoint. An SSH subsystem is cleaner because it avoids shell parsing and startup output.

Conceptually:

```text
ssh connection
  |
  +-- session channel, no PTY
        |
        +-- request subsystem "srui"
              |
              +-- binary SRUI bridge
```

SSH provides:

- server host-key verification;
- user authentication;
- confidentiality/integrity;
- encrypted channel transport;
- execution under the authenticated remote account.

The SRUI binary stream itself is transported only after the SSH security context exists.

### 19.1 Recommended SSH posture

The reference client SHOULD:

- request no PTY for the SRUI protocol channel;
- use ordinary OpenSSH host-key verification/`known_hosts` behavior;
- not enable X11 forwarding;
- not enable agent forwarding by default;
- not create arbitrary local/remote port forwards as a side effect of SRUI;
- use a fixed subsystem or fixed executable, not a shell-interpolated command;
- treat remote stderr separately from the binary protocol if the SSH library exposes it;
- fail closed on host-key changes according to normal SSH policy.

The client should rely on the user's existing `ssh`/agent/keychain infrastructure rather than implementing private-key handling itself.

### 19.2 Logical channel scheduler

Within the SRUI stream:

| Logical class | Priority | Examples |
|---|---:|---|
| control | highest | HELLO, WELCOME, errors, resume, EVENT_ACK (§18.2) |
| input | highest | semantic user events |
| UI | high | committed transactions |
| terminal | high/normal | interactive PTY bytes |
| resource | low | images, attachments |

Large resource chunks are bounded (for example 16–32 KiB) and interleaved with high-priority traffic.

A future transport may map these classes to independent QUIC streams without changing Core semantics.

---

## 20. Reference server architecture

A reconnectable implementation cannot make the SSH child process itself the owner of application state.

The reference server therefore separates the transient transport bridge from the persistent user session runtime.

```text
                    REMOTE UNIX HOST

sshd
 |
 | authenticated user, subsystem "srui"
 v
+-------------------------+
| srui-ssh-bridge         |  ephemeral; lifetime = SSH attachment
+------------+------------+
             |
             | private Unix-domain socket / authenticated same-user IPC
             v
+-------------------------+
| srui-sessiond           |  per-user persistent runtime
|                         |
| SessionManager          |
| SemanticStore           |
| TransactionJournal      |
| EventDeduplicator       |
| ResourceCAS             |
| Scheduler               |
| PTYManager              |
| ApplicationAdapters     |
+-------------+-----------+
              |
       application / agent
```

### 20.1 `srui-ssh-bridge`

Responsibilities:

- read/write framed SRUI messages over the SSH channel;
- obtain the authenticated remote-user context by virtue of being launched by `sshd`;
- connect only to that user's session daemon;
- attach/detach a connection from a session;
- enforce outer message-size and rate limits before forwarding;
- exit when the SSH channel closes.

It does **not** own semantic state.

### 20.2 `srui-sessiond`

A per-user process (or on-demand daemon) owns durable sessions.

Responsibilities:

- launch or attach applications without a shell-injection boundary;
- own semantic session IDs;
- store the authoritative semantic tree or receive authoritative transactions from the application adapter;
- validate transactions before commit;
- maintain revisions and replay journal;
- deduplicate client events;
- retain resources and PTYs while detached;
- apply backpressure and memory limits;
- snapshot semantic state;
- expire sessions according to policy.

The daemon runs with the authenticated user's ordinary OS privileges, not root privileges.

The session daemon is the protocol authority for a session revision. An application adapter may compute semantic state, but only successfully committed/validated transactions advance the session's authoritative revision. A disconnected client cannot independently commit semantic state.

Connection interruption is therefore treated as a transport failure, not a semantic rollback:

```text
SSH/TCP connection lost
        ↓
srui-ssh-bridge exits
        ↓
srui-sessiond keeps application + committed semantic state + journal
        ↓
new authenticated SSH connection
        ↓
resume from last committed revision, or send snapshot
```

For a very early demo, the bridge and session daemon may be one process. **Reconnect conformance is not achieved until state ownership is moved out of the transient SSH process.**

### 20.3 Application adapter API

The server-side application integration should expose an API resembling:

```text
create_node(type, parent, properties)
set(node, property, value)
delete(node)
transaction { ... }
create_model(...)
update_model(...)
on_event(node, type, handler)
publish_resource(bytes)
```

The adapter may be:

- a dedicated SRUI-native application SDK;
- a Qt/GTK/SWT bridge that captures widget semantics before painting;
- an agent/tool application that already has structured internal state;
- a wrapper around a legacy PTY inside a Terminal node.

The application API must not expose AppKit-specific state.

### 20.4 Backpressure

Every queue is bounded.

If a client is slower than the application:

- scalar property updates may be coalesced when semantics allow (`progress=0.50`, `0.51`, `0.52` → latest value);
- committed structural transactions may not be silently dropped;
- resources may be paused;
- an excessively stale client may be detached and forced to resync;
- memory growth is bounded independently of remote application speed.

### 20.5 Server behavior while detached

Policy is application-selectable:

```text
continue        application keeps running
pause-input     application runs but no client interaction
pause-app       application receives detach notification
terminate       explicitly connection-bound session
```

The default for coding/operations sessions is `continue`.

---

## 21. Terminal compatibility profile

Terminal compatibility is an extension profile, not part of the Standard Widget Profile.

```text
legacy app -> PTY slave -> PTY master -> SRUI server
                                         |
                                         | TERMINAL_DATA
                                         v
                                   local TerminalView
                                   VT parser -> retained grid

keyboard -> TERMINAL_INPUT -> PTY master
resize   -> TERMINAL_RESIZE -> TIOCSWINSZ
```

### 21.1 Semantics

- The Terminal node is an opaque compatibility island.
- PTY bytes are not interpreted as standard semantic widgets.
- The local client owns the VT emulator state and screen grid.
- Semantic-native UI may surround an embedded terminal.

### 21.2 Terminal reconnect

Each terminal stream has a monotonically increasing byte offset. The server keeps a bounded output ring.

On resume:

- if the required output range is retained, replay it;
- if not, mark `TERMINAL_RESYNC_REQUIRED`.

A semantic SRUI session can therefore resume perfectly even if one terminal compatibility island needs a redraw/restart strategy.

For durable full-screen terminal sessions in v0.1, a server MAY back the PTY with `tmux` or another multiplexer so reattachment causes the application/multiplexer to redraw. A later `TerminalCheckpoint` extension may standardize transferable terminal-state checkpoints.

This limitation is explicit: unlike the semantic tree, an unparsed raw PTY stream does not inherently contain a current-state snapshot.

---

# Part III — macOS reference client

## 22. macOS rendering architecture

The macOS reference implementation uses **Swift + AppKit**.

AppKit is a rendering profile, not the protocol model.

```text
SRUIClient
├── SSHTransport
├── FrameDecoder
├── SessionController
├── SemanticStore             // platform-neutral objects
├── TransactionApplier        // off-main validation + atomic commit
├── ResourceCache
├── EventOutbox               // event IDs, ACK/retry
├── RendererAppKit
│   ├── RenderRegistry        // node_id -> RenderHandle
│   ├── ControlFactory
│   ├── LayoutRenderer
│   ├── CollectionAdapters
│   ├── TextAdapters
│   ├── AccessibilityBridge
│   └── TerminalRenderer
└── PresentationStateStore
```

### 22.1 Thin-client rule

The macOS reference client is intentionally **not** a browser engine and not a second application runtime.

Its SRUI-specific responsibilities are limited to:

```text
SSH/transport adapter
        |
protocol decoder / validator
        |
transaction + revision manager
        |
non-authoritative SemanticStore
        |
AppKit renderer adapter
        |
event encoder
```

with supporting:

```text
resource cache
collection/model cache
accessibility + semantic inspection bridge
Terminal extension parser
optional VectorScene renderer
```

The reference implementation SHOULD reuse the operating system for the expensive/general rendering machinery:

```text
AppKit          -> controls, windowing, standard layout behavior
Core Text       -> text shaping where AppKit does not already provide it
Core Animation  -> compositing / display scheduling
ImageIO/AppKit  -> approved image decoding paths
NSAccessibility -> native accessibility integration
```

The project SHOULD NOT implement a bespoke CSS engine, DOM, JavaScript VM, application scripting runtime, general browser navigation model, text shaper, or GPU compositor merely to render Standard Widget Profile controls.

A custom layout or rendering path is justified only by measured performance or by an extension such as `VectorScene`.

This distinction is normative:

```text
allowed:
  remote semantic data -> fixed local renderer behavior

not allowed in the base client:
  remote executable code -> local application behavior
```


### 22.2 Threading

- Network IO and Protobuf decoding occur off the main thread/actor.
- Transactions are validated and applied to the semantic store serially.
- At `COMMIT`, a compact render delta is produced.
- AppKit view mutations occur on `MainActor`.
- The renderer never observes a half-committed semantic transaction.

### 22.3 Render handles

A `RenderHandle` is an AppKit-specific object associated with a semantic node ID.

It may own:

- an `NSView`/`NSControl`;
- a lightweight layout object;
- a model adapter rather than one view per logical item;
- accessibility metadata;
- presentation-only state.

The semantic store itself contains no `NSView` references.

### 22.4 AppKit mapping

| SRUI node | Reference AppKit strategy |
|---|---|
| `Surface` | `NSWindow` + root content view |
| `Row` / `Column` | `NSStackView` initially; custom lightweight layout if profiling justifies it |
| `Grid` | Auto Layout/custom grid layout adapter |
| `Text` | non-editable `NSTextField` label |
| `RichText` | non-editable/selectable `NSTextView` |
| `Button` | `NSButton` |
| `Toggle` | `NSSwitch` or checkbox-style `NSButton` depending semantic preference |
| `ChoiceGroup` | radio-style controls or `NSSegmentedControl` when locally appropriate |
| `Select` | `NSPopUpButton` / `NSComboBox` |
| `Slider` | `NSSlider` |
| `NumberInput` | `NSTextField` + `NSStepper` |
| `TextInput` | `NSTextField` |
| `TextArea` | `NSTextView` in `NSScrollView` |
| `Progress` | `NSProgressIndicator` |
| `Image` | `NSImageView` |
| `Scroll` | `NSScrollView` |
| `List` / `Table` | `NSTableView` + local model adapter |
| `Tree` | `NSOutlineView` + local model adapter |
| `Tabs` | `NSTabView`/controller or local equivalent |
| `Split` | `NSSplitView` |
| `Menu` | `NSMenu` |
| `Toolbar` | `NSToolbar` or native-equivalent command strip |

The mapping is allowed to change without a protocol version change.

### 22.5 Local interaction state

The renderer handles locally:

```text
hover
pressed appearance
focus rings
caret blinking
text selection
marked-text / IME composition
scrolling and momentum
menu opening
selection highlight
disclosure animation
window movement/resize
system appearance changes
ordinary value animations
```

Only semantic outcomes are sent to the server.

### 22.6 Text editing

Text input is a primary example of why semantic remoting is preferable to terminal or coordinate remoting.

Once the client knows:

```text
TextInput #39
  value = "server"
  editable = true
```

ordinary editing is performed by the local AppKit text system:

```text
keyboard / dictation / IME
        |
        v
local NSTextField / NSTextView
        |
        +-- caret movement
        +-- selection
        +-- marked text / IME composition
        +-- clipboard paste/copy
        +-- spell checking where locally appropriate
        +-- immediate glyph rendering
```

These interactions MUST NOT require a network round trip before visible feedback.

The client sends semantic edit results to the server using whole-value edits or compact text deltas with a monotonic `edit_seq`. Implementations SHOULD batch/coalesce edits when safe rather than send every low-level key event.

The server remains authoritative for committed application value. It may:

- accept the edit and publish the corresponding semantic revision;
- normalize/validate the value and publish a corrected value;
- reject an edit and publish validation state;
- change editability or other semantic constraints.

IME marked/composition text is presentation/editing state and SHOULD remain local until the platform regards an edit as suitable for synchronization. SRUI MUST NOT attempt to reimplement platform IME behavior remotely.

Clipboard integration uses the normal local OS text control. Access to clipboard contents by the remote application is a separate local capability and is **not** implied by the existence of a `TextInput`.

Reconnect handling for pending edits is defined in §18.3.

### 22.7 Collections

`NSTableView` and `NSOutlineView` are fed from the replicated collection model. The renderer never creates a native control for each remote item.

Visible rows/cells are reused according to AppKit's normal view-reuse behavior.

For uncached ranges, the renderer displays a local loading representation and sends an asynchronous range request; scrolling itself remains local.

### 22.8 Accessibility

The semantic tree maps to native accessibility automatically where standard AppKit controls are used.

Custom views must expose equivalent `NSAccessibility` roles, labels, values, hierarchy, and actions.

Accessibility is not reconstructed from pixels or text after rendering.

### 22.9 Local semantic inspection and automation

The retained semantic replica is valuable independently of pixels. The client maintains a read-only semantic-tree API internally with stable node identities, roles, labels, values, hierarchy, state, and advertised actions.

Conceptually, local consumers may observe:

```text
Window
├── Heading "Services"
├── Table
│   ├── Row nginx Running
│   └── Row postgres Stopped
└── Button "Restart"
```

instead of screen-scraping coordinates.

A future XPC interface MAY expose controlled inspection/action APIs for:

- accessibility tools and screen readers;
- automated testing;
- voice interaction;
- local agent tooling;
- user-authored local automation.

For example, a trusted local automation client should be able to express the equivalent of:

```text
find(role=button, label="Restart").activate()
```

rather than:

```text
click(x=800, y=420)
```

The public inspection surface is **semantic**, not an AppKit object graph: it MUST NOT expose `NSView` pointers/classes as protocol identity.

There is an important security distinction:

- SRUI forbids **server-supplied executable application code** in the renderer.
- It may permit **locally installed/trusted automation clients** to inspect the tree and request semantic actions.

Automation actions MUST travel through the normal SRUI event path and server authorization checks. Local automation does not mutate the authoritative semantic tree directly, does not bypass `enabled`/capability state, and does not gain clipboard/filesystem/device access merely because it can inspect UI semantics.

The XPC automation surface SHOULD be disabled or permission-gated by default until its authorization model is specified.

---

## 23. macOS renderer performance strategy

The renderer is retained-mode and mutation-driven.

A scalar update:

```text
SET_PROPERTY node=912 property=value value=0.72
```

should result in:

```text
decode
 -> semantic-store update
 -> dirty-node classification
 -> NSProgressIndicator.doubleValue update
 -> local compositor/display refresh
```

It should not rebuild the complete UI tree.

Dirty changes are classified as:

```text
content-only
appearance-role
layout-affecting
structure-affecting
accessibility-only
resource-arrival
```

This permits minimal invalidation.

Targets are measured, not assumed:

- 100 ordinary scalar changes: `<1 ms p50` semantic decode/apply target on contemporary Apple silicon;
- 1,000 scalar changes: `<5 ms p50` target before AppKit display scheduling;
- local interaction feedback: next local display frame;
- idle semantic UI: zero SRUI UI traffic;
- no synchronous render-related network RTT.

---

# Part IV — Security

## 24. Security objective

SRUI should have a security posture comparable to, and for local-resource access stricter than, a normal terminal session over SSH.

The important distinction is:

- **SSH authenticates and encrypts the transport.**
- **SRUI must still treat all remote UI data as untrusted parser input.**

An authenticated remote host is allowed to control its remote application state and displayed UI. It is not automatically allowed to read/write arbitrary local files, clipboard contents, camera, microphone, credentials, or execute code on the client.

---

## 25. SSH security boundary

The v0.1 implementation delegates cryptography and user/server identity to OpenSSH.

Security properties intentionally inherited from SSH:

- host-key verification;
- authenticated remote user account;
- encrypted/integrity-protected transport;
- normal SSH credential/agent handling;
- remote process executes with that user's remote OS privileges.

SRUI does not weaken host-key checks and does not define an “accept unknown host silently” mode.

A session-resume request is accepted only on a newly authenticated SSH connection for the same remote account and server-side authorization context. A `session_id` or resume ticket is **not** sufficient authentication by itself.

---

## 26. Client attack-surface controls

The base protocol permits no remote executable code.

Mandatory limits include:

```text
maximum frame size
maximum transaction operations
maximum tree depth
maximum node count
maximum string length
maximum model/item count per message
maximum resource encoded size
maximum decoded image dimensions/pixels
maximum update rate
maximum pending unacknowledged events
maximum terminal escape payload lengths
```

`maximum pending unacknowledged events` bounds the client's retry set, which is drained by
`SERVER EVENT_ACK` (§18.2). Reaching the bound means events are being discarded before they were
known to be processed, so an eviction there MUST be reported rather than silently dropped.

Further rules:

- resource hashes are verified before cache commit;
- decompression is bounded;
- malformed tree operations fail the transaction rather than corrupt state;
- unknown required semantics fail closed;
- node/event/session IDs are checked for scope and freshness;
- terminal OSC clipboard mutation is disabled by default;
- arbitrary URL opening, clipboard, local files, notifications, microphone, camera, and other local integrations require separate negotiated capabilities plus local policy/user approval;
- server paths are never interpreted as local paths;
- no server-provided native fonts in v0.1.

The macOS client SHOULD use Hardened Runtime and SHOULD isolate the VT parser/resource decoding from unnecessary local privileges when practical.

---

## 27. Server security controls

- `srui-sessiond` runs as the authenticated user, not root.
- The SSH bridge connects only to that user's private runtime socket.
- Application launch uses an argument vector or registered application ID, not shell interpolation.
- A fixed SSH subsystem is preferred over an arbitrary command string.
- Session IDs are random and unguessable, but authorization is still based on authenticated user identity.
- The server validates every client event against the current node/action capabilities and application authorization.
- Semantic action identifiers are never interpreted as shell commands, executable paths, or code by the client. Server adapters bind actions to in-process handlers or explicit safe application APIs.
- Stale node IDs and stale client instance IDs are rejected.
- Side-effect events are deduplicated.
- Resource and journal memory is bounded.
- Application-specific authorization remains the application's responsibility, exactly as it would for a command executed in the user's SSH shell.

---

# Part V — Reference implementation and conformance

## 28. Repository layout

```text
srui/
├── spec/
│   ├── core.md
│   ├── standard-widgets.md
│   ├── registries.md
│   ├── extensions.md
│   └── security.md
├── protocol/
│   ├── srui.proto
│   ├── registry.yaml
│   └── conformance-vectors/
├── client-macos/
│   ├── TransportSSH/
│   ├── Protocol/
│   ├── SemanticModel/
│   ├── Session/
│   ├── RendererAppKit/
│   ├── Collections/
│   ├── Text/
│   ├── Terminal/
│   ├── Resources/
│   ├── Accessibility/
│   └── Tests/
├── server-rust/
│   ├── ssh-bridge/
│   ├── sessiond/
│   ├── semantic-tree/
│   ├── journal/
│   ├── event-dedupe/
│   ├── resources/
│   ├── pty/
│   ├── sdk/
│   └── examples/
├── sdk/
│   └── second-language/       # early second implementation to catch Rust assumptions
├── examples/
│   ├── counter/
│   ├── process-monitor/
│   └── coding-agent-demo/
└── benchmarks/
    ├── parse-render/
    ├── mutation/
    ├── reconnect/
    ├── network/
    └── terminal/
```

---

## 29. Reference server SDK contract

An SRUI-native application should not need to know about SSH, Protobuf frames, retries, or AppKit.

Example conceptual API:

```rust
session.transaction(|ui| {
    ui.set(progress, VALUE, 0.72);
    ui.set(status, TEXT, "Running tests");
});

session.on(approve_button, ACTIVATE, |ctx, event| {
    // application logic
});
```

The SDK/runtime handles:

- node IDs;
- transaction revision allocation;
- encoding;
- journal append;
- per-client scheduling;
- ACK/resume state;
- event deduplication;
- resources.

Toolkit adapters sit above this API.

---

## 30. Example coding-agent UI

```text
Surface
└── Column
    ├── Row
    │   ├── Text(role=heading, "Remote coding session")
    │   └── Progress(value=.62)
    ├── Split
    │   ├── Tree(id=files)
    │   └── Column
    │       ├── RichText(id=conversation)
    │       ├── Terminal(id=terminal, extension=org.srui.terminal/1)
    │       └── Row
    │           ├── Button(id=approve, label="Approve", role=primary)
    │           └── Button(id=reject, label="Reject", role=destructive)
    └── TextArea(id=prompt, role=command)
```

A future coding profile can add:

```text
Diff
ToolCall
ApprovalRequest
Diagnostic
CodeDocument
FileReference
TaskPlan
```

Each extension node can carry a Standard Widget fallback.

---

## 31. Benchmark methodology

Benchmarks MUST separate layers.

### 31.1 Local renderer benchmark

Equivalent prebuilt representations are already in memory:

```text
HTML bytes -> warm WKWebView
SRUI bytes -> warm SRUI renderer
```

Measure:

```text
first visible paint
complete paint
CPU time
allocations
peak memory
```

This answers renderer/representation cost only.

### 31.2 Serialization benchmark

Start from the same abstract application UI state and separately measure server-side generation/serialization.

### 31.3 Steady-state mutation and frame-independence benchmark

Preload the UI, then update 1, 100, and 1,000 values. Measure bytes and decode-to-visible latency.

Run the same semantic workload while the local display/render loop is configured for different refresh cadences where the platform permits:

```text
60 Hz
120 Hz
144 Hz
240 Hz / uncapped synthetic renderer
```

For an identical application mutation stream, SRUI wire bytes and semantic message count MUST remain materially unchanged. Local repaint count may differ; protocol traffic must not scale with monitor refresh rate.

Also verify that multiple committed revisions can be coalesced for painting without changing the committed semantic state or event ordering.

### 31.4 Network and local-interaction benchmark

Use controlled RTT, bandwidth, loss, and interruption. Measure server-dependent input-to-visible latency and bytes.

Separately inject 0 ms, 100 ms, 300 ms, and 600 ms RTT while exercising interactions that are specified as local:

```text
text entry
caret movement
text selection
IME composition
scrolling
hover / pressed feedback
menu opening
```

Visible local feedback for these interactions MUST NOT acquire one-RTT latency merely because the remote connection is slow. Semantic outcomes may synchronize asynchronously.

### 31.5 Reconnect benchmark

Test failure at each boundary:

```text
mid-resource
mid-transaction
immediately before event receipt
immediately after side effect but before ACK
while detached for journal retention period
beyond journal retention period
```

The "after side effect but before ACK" boundary is the ambiguous window of §18.2: the client
replays the event on resume and the server must answer `DUPLICATE` from its result cache rather
than re-running the action.

Verify deterministic replay/resync and no duplicate side effects.

### 31.6 Terminal benchmark

Compare embedded Terminal behavior with a standalone terminal and explicitly test reconnect ring-buffer exhaustion.

---

## 32. Conformance suites

Protocol conformance is independent of rendering appearance.

Required suites:

1. **Core state-machine tests** — valid/invalid transactions, revisions, IDs.
2. **Widget semantic tests** — event/state meaning for each Standard Widget node.
3. **Semantic-not-paint tests** — Standard Widget Profile fixtures contain no frame cadence, server paint commands, or required absolute pixel geometry.
4. **Frame-independence tests** — identical semantic mutation streams produce identical protocol traffic independent of client refresh rate.
5. **Semantic-input tests** — ordinary controls emit semantic actions/changes rather than pointer coordinates; coordinate streams are accepted only for explicitly subscribed custom scene nodes.
6. **Local text-interaction tests** — caret, selection, IME composition, and visible typing remain local under injected network latency; server reconciliation still converges to authoritative state.
7. **Extension-negotiation tests** — fallback and must-understand behavior.
8. **Reconnect tests** — replay, snapshot, partial transaction discard, pending-edit reconciliation, event dedupe.
9. **Security limits** — oversized/deep/malformed payloads and forbidden executable/script payload classes.
10. **Renderer semantic tests** — accessibility roles/actions, enabled/disabled behavior, selection, text editing.
11. **Semantic inspection tests** — inspection exposes semantic identity rather than AppKit object identity; automation actions use the normal event/authorization path.
12. **Toolkit mapping tests** — informative test fixtures shared across macOS and future renderers.

Screenshots are useful for renderer regression but are not semantic conformance criteria.

---

## 33. MVP implementation sequence

1. Freeze Core v0.1 state/session/event model and registries.
2. Freeze Standard Widget Profile 1 semantics; mark required vs optional implementation tier.
3. Implement in-memory Swift semantic store and AppKit renderer without networking.
4. Benchmark prerecorded transactions before adding transport.
5. Implement Rust `sessiond` + local Unix-socket bridge; test detach/reattach locally.
6. Add SSH subsystem bridge and normal OpenSSH authentication/host verification.
7. Add reconnect journal and event deduplication tests.
8. Add resource CAS and image delivery.
9. Add virtualized Table/Tree models.
10. Add local text-edit semantics.
11. Add Terminal extension/PTY bridge.
12. Build coding-agent example with mixed semantic controls + terminal island.
13. Implement a minimal second client or headless conformance client in another language before protocol 1.0.
14. Add `VectorScene` only after real applications demonstrate a need.

---

## 34. Design decisions that should remain open until profiling

- Protobuf as permanent mandatory encoding versus one required reference encoding.
- Exact size of resource chunks over the SSH binding.
- Whole-text updates versus text deltas after v0.1.
- Range-fetch policy for huge collection models.
- Whether simple `Row`/`Column` should use `NSStackView` or a custom lightweight AppKit layout after profiling.
- Whether the first Terminal implementation needs a server-side VT checkpoint mechanism or can rely on replay/multiplexer behavior.
- Whether extension namespace IDs are assigned in the handshake or derived from a stable registry after 1.0.

These are implementation/performance questions, not reasons to weaken the layer boundaries.

---

## 35. One-sentence design

> **SRUI replicates a platform-neutral semantic UI and application-facing interaction contract over a secure transport; the remote host remains authoritative, while the local client owns presentation, native interaction, and rendering.**

---

# Appendix A — Core/renderer separation checklist

A proposed change belongs in **Protocol Core / Standard Widget Profile** only if the answer to these questions is yes:

- Can AppKit, Windows, GTK, and SWT reasonably implement the meaning?
- Does it describe user-visible semantics or distributed-state correctness rather than a paint technique?
- Can a renderer choose a different visual implementation without breaking the application?
- Can it be tested without inspecting exact pixels?

If not, it probably belongs in:

- a renderer-specific profile;
- an extension profile;
- the transport binding;
- or the application layer.

Examples:

| Concept | Correct layer |
|---|---|
| `Button.enabled` | Standard Widget Profile |
| destructive action role | Standard Widget Profile |
| 7 px corner radius | macOS renderer policy, not protocol |
| `NSButton.bezelStyle` | macOS renderer only |
| Protobuf field number | wire encoding |
| SSH host-key verification | transport binding |
| reconnect revision | Protocol Core |
| terminal OSC parser | Terminal profile/client implementation |
| syntax-highlighted Diff | coding extension profile |

---

# Appendix B — Server reconnect state machine

```text
               SSH attach
   +--------------------------------+
   |                                v
DETACHED -----------------------> ATTACHED
   ^                                |
   | transport loss                 | explicit DETACH
   +--------------------------------+
   |
   | TTL expires / explicit terminate
   v
TERMINATING -> EXPIRED
```

Connection loss never commits a partial transaction.

Event result and receive-window state:

```text
(client_instance_id, event_id) -> IN_FLIGHT {
  event_seq
}

(client_instance_id, event_id) -> SETTLED {
  event_seq,
  status,
  optional result,
  semantic_revision_after_effect,
  reject_reason
}

client_instance_id -> {
  last_contiguous_processed_seq,
  settled_out_of_order
}
```

`IN_FLIGHT` is never a source of terminal acknowledgement. If dispatch aborts or a handler panics,
the admission is removed so the client can retry. The settled fields are returned in
`SERVER EVENT_ACK` (§18.2); a replay is served from this cache rather than re-running the action.

`settled_out_of_order` is a bounded selective-ack set. Settling a sequence greater than
`last_contiguous_processed_seq + 1` records it without advancing the cumulative frontier. Settling
the missing next sequence advances the frontier and consumes every now-contiguous entry, exactly
like a TCP receiver consuming buffered data after a gap closes.

The reference receive/result window is bounded to 4,096 sequence slots per client instance.
Entries at or below the contiguous frontier are eligible for FIFO result eviction; `IN_FLIGHT` and
out-of-order settled entries are not evicted to admit a farther sequence. The client reference
window is 256 slots and applies backpressure instead of dropping an unacknowledged event.
`CLIENT RESUME.last_acked_event_seq` remains a retention hint rather than an authority to mark
unprocessed server events as settled. `SERVER RESUME_OK` and `SERVER RESYNC_REQUIRED` report the
server's contiguous frontier for the bound client before any replay. The dedupe state belongs to
the session incarnation; durable restoration must persist it with the authoritative state, while a
replacement incarnation explicitly causes the client to abandon its old pending set.

---

The intended comparison is:

| Property | SSH terminal | SRUI v0.1 |
|---|---|---|
| Host identity | SSH host key | SSH host key |
| User authentication | SSH | SSH |
| Transport crypto | SSH | SSH |
| Remote process privilege | authenticated remote user | authenticated remote user |
| PTY required | normally yes | no for protocol; optional inside Terminal node |
| Server-supplied local executable code | no | no |
| Remote access to local files | not inherent | not inherent; capability-gated if ever added |
| Remote clipboard control | terminal-dependent OSC may permit | disabled by default |
| Parser attack surface | VT/terminal parser | bounded SRUI decoder + optional VT parser |
| Connection resume | no, unless tmux/mosh-like layer | semantic session resume is built in |

SRUI is therefore designed to inherit SSH's identity/transport security while exposing **less implicit local-machine authority than many feature-rich terminal emulators**.

---

# Appendix D — Sources and design references

### RemoteUI lineage

- Daniel Thommes, Ansgar Gerlicher, Qi Wang, Christos Grecos. *RemoteUI: A High-Performance Remote User Interface System for Mobile Consumer Electronic Devices.* IEEE Transactions on Consumer Electronics 58(3), 2012. DOI: 10.1109/TCE.2012.6311361.  
  <https://research-portal.uws.ac.uk/en/publications/remoteui-a-high-performance-remote-user-interface-system-for-mobi/>
- Daniel Thommes. *The Remote UI System: A High Performance Remote User Interface System for Mobile Scenarios.* PhD thesis, University of the West of Scotland, 2016. DBLP/EThOS bibliographic record: <https://dblp.org/rec/phd/ethos/Thommes16>

### SSH

- RFC 4254, *The Secure Shell (SSH) Connection Protocol* — session channels, PTY requests, shell/exec/subsystem requests:  
  <https://www.rfc-editor.org/rfc/rfc4254.html>
- OpenSSH `ssh(1)` manual — `-T`, subsystems, host-key behavior, encrypted command/session transport:  
  <https://man.openbsd.org/ssh>

### macOS / AppKit

- Apple AppKit Views and Controls:  
  <https://developer.apple.com/documentation/appkit/views-and-controls>
- Apple text display (`NSTextField`, `NSTextView`):  
  <https://developer.apple.com/documentation/appkit/text-display>
- Apple Outline View / `NSOutlineView`:  
  <https://developer.apple.com/documentation/appkit/outline-view>

### Cross-toolkit widget convergence

- Microsoft Windows controls overview:  
  <https://learn.microsoft.com/en-us/windows/win32/uxguide/controls>
- GTK 4 widget documentation:  
  <https://docs.gtk.org/gtk4/>
- GTK 4 list widget overview:  
  <https://docs.gtk.org/gtk4/section-list-widget.html>
- Eclipse SWT controls overview:  
  <https://help.eclipse.org/latest/topic/org.eclipse.platform.doc.isv/guide/swt_widgets_controls.htm>

---

**End of SRUI design draft v0.6**
