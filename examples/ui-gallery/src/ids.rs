//! Stable node, model, item, and action-key identifiers grouped by gallery section (§6.2, §7.7, §8).
//!
//! # Architecture & Design Invariants
//!
//! - **§6.2 Stable identity**: every id here is a compile-time constant. Nothing in the gallery
//!   allocates a node id at runtime except [`TRANSIENT_BADGE`], which the structure scene creates
//!   and deletes, and the trace item ids, which are model item ids rather than node ids.
//! - **Section ranges**: each section owns one 100-wide block. A new section takes the next free
//!   block; a new widget inside a section takes the next free id in that block. Ids are never
//!   reused across sections, so a stale client handle can never alias a different widget.
//! - **§7.7 `action_key` is opaque data**: the keys below are transported as metadata for the
//!   client and for logs. The server never parses, dispatches, or executes them; every handler is
//!   bound to a `(NodeId, TypeRef)` pair through [`srui_sessiond::Session::on`].

use srui_sdk::{ItemId, ModelId, NodeId};

// =============================================================================
// Frame and scene toolbar (1..99)
// =============================================================================

/// Root surface hosting the whole gallery (§7.2).
pub const SURFACE: NodeId = NodeId::new(1);
/// Outermost column: toolbar above, scrolling gallery below.
pub const ROOT_COLUMN: NodeId = NodeId::new(2);
/// Fixed scene toolbar row.
pub const TOOLBAR: NodeId = NodeId::new(3);
/// Scroll container holding every gallery section.
pub const ROOT_SCROLL: NodeId = NodeId::new(4);
/// Column inside the scroll view; direct parent of every section column.
pub const GALLERY_COLUMN: NodeId = NodeId::new(5);

pub const BTN_PREV: NodeId = NodeId::new(10);
pub const BTN_NEXT: NodeId = NodeId::new(11);
pub const BTN_RESET: NodeId = NodeId::new(12);
pub const TOGGLE_AUTOPLAY: NodeId = NodeId::new(13);
pub const TOOLBAR_SPACER: NodeId = NodeId::new(14);
pub const SCENE_LABEL: NodeId = NodeId::new(15);

// =============================================================================
// Hero (100..199)
// =============================================================================

pub const HERO_COLUMN: NodeId = NodeId::new(100);
pub const HERO_HEADING: NodeId = NodeId::new(101);
pub const HERO_SEPARATOR: NodeId = NodeId::new(102);
pub const HERO_ROW: NodeId = NodeId::new(103);
pub const HERO_IMAGE: NodeId = NodeId::new(104);
pub const HERO_IMAGE_PLACEHOLDER: NodeId = NodeId::new(105);
pub const HERO_TEXT_COLUMN: NodeId = NodeId::new(106);
pub const HERO_TITLE: NodeId = NodeId::new(107);
pub const HERO_BLURB: NodeId = NodeId::new(108);
pub const HERO_CAPTION: NodeId = NodeId::new(109);
pub const HERO_STATUS: NodeId = NodeId::new(110);

// =============================================================================
// Typography (200..299) — one node per standard text role (§7.5)
// =============================================================================

pub const TYPO_COLUMN: NodeId = NodeId::new(200);
pub const TYPO_HEADING: NodeId = NodeId::new(201);
pub const TYPO_SEPARATOR: NodeId = NodeId::new(202);
pub const TYPO_TITLE: NodeId = NodeId::new(210);
pub const TYPO_SUBHEADING: NodeId = NodeId::new(211);
pub const TYPO_BODY: NodeId = NodeId::new(212);
pub const TYPO_CAPTION: NodeId = NodeId::new(213);
pub const TYPO_CODE: NodeId = NodeId::new(214);
pub const TYPO_STATUS: NodeId = NodeId::new(215);
pub const TYPO_WARNING: NodeId = NodeId::new(216);
pub const TYPO_ERROR: NodeId = NodeId::new(217);
pub const TYPO_RICH: NodeId = NodeId::new(218);

/// The eight standard text roles, in registry order, paired with their demo node.
pub const TYPO_ROLE_NODES: [NodeId; 8] = [
    TYPO_TITLE,
    TYPO_SUBHEADING,
    TYPO_BODY,
    TYPO_CAPTION,
    TYPO_CODE,
    TYPO_STATUS,
    TYPO_WARNING,
    TYPO_ERROR,
];

// =============================================================================
// Controls (300..399)
// =============================================================================

pub const CTRL_COLUMN: NodeId = NodeId::new(300);
pub const CTRL_HEADING: NodeId = NodeId::new(301);
pub const CTRL_SEPARATOR: NodeId = NodeId::new(302);

pub const CTRL_BUTTON_ROW: NodeId = NodeId::new(310);
pub const BTN_NORMAL: NodeId = NodeId::new(311);
pub const BTN_PRIMARY: NodeId = NodeId::new(312);
pub const BTN_DESTRUCTIVE: NodeId = NodeId::new(313);
pub const BTN_QUIET: NodeId = NodeId::new(314);

/// Baseline child order of [`CTRL_BUTTON_ROW`]; the structure scene reverses it.
pub const CTRL_BUTTON_ORDER: [NodeId; 4] = [BTN_NORMAL, BTN_PRIMARY, BTN_DESTRUCTIVE, BTN_QUIET];

pub const CTRL_TOGGLE_ROW: NodeId = NodeId::new(320);
pub const TOGGLE_CHECKBOX: NodeId = NodeId::new(321);
pub const TOGGLE_SWITCH: NodeId = NodeId::new(322);
pub const TOGGLE_AUTOMATIC: NodeId = NodeId::new(323);

pub const CTRL_INPUT_GRID: NodeId = NodeId::new(330);
pub const INPUT_PLAIN: NodeId = NodeId::new(331);
pub const INPUT_SEARCH: NodeId = NodeId::new(332);
pub const INPUT_SECURE: NodeId = NodeId::new(333);
pub const INPUT_COMMAND: NodeId = NodeId::new(334);
pub const INPUT_READ_ONLY: NodeId = NodeId::new(335);
pub const INPUT_INVALID: NodeId = NodeId::new(336);

pub const TEXT_AREA: NodeId = NodeId::new(340);
pub const TEXT_AREA_NOTE: NodeId = NodeId::new(341);

pub const CTRL_PROGRESS_ROW: NodeId = NodeId::new(350);
pub const PROGRESS_DETERMINATE: NodeId = NodeId::new(351);
pub const PROGRESS_BUSY: NodeId = NodeId::new(352);

pub const CTRL_STATUS: NodeId = NodeId::new(360);
pub const MENU: NodeId = NodeId::new(361);
pub const MENU_NOTE: NodeId = NodeId::new(362);

// =============================================================================
// Layout (400..499)
// =============================================================================

pub const LAYOUT_COLUMN: NodeId = NodeId::new(400);
pub const LAYOUT_HEADING: NodeId = NodeId::new(401);
pub const LAYOUT_SEPARATOR: NodeId = NodeId::new(402);

pub const LAYOUT_ROW: NodeId = NodeId::new(410);
pub const LAYOUT_ROW_A: NodeId = NodeId::new(411);
pub const LAYOUT_ROW_B: NodeId = NodeId::new(412);
pub const LAYOUT_SPACER: NodeId = NodeId::new(413);
pub const LAYOUT_ROW_C: NodeId = NodeId::new(414);

pub const LAYOUT_GRID: NodeId = NodeId::new(420);
pub const LAYOUT_GRID_CELLS: [NodeId; 6] = [
    NodeId::new(421),
    NodeId::new(422),
    NodeId::new(423),
    NodeId::new(424),
    NodeId::new(425),
    NodeId::new(426),
];

pub const LAYOUT_NESTED_ROW: NodeId = NodeId::new(430);
pub const LAYOUT_NESTED_LEFT: NodeId = NodeId::new(431);
pub const LAYOUT_NESTED_LEFT_A: NodeId = NodeId::new(432);
pub const LAYOUT_NESTED_LEFT_B: NodeId = NodeId::new(433);
pub const LAYOUT_NESTED_SEPARATOR: NodeId = NodeId::new(434);
pub const LAYOUT_NESTED_RIGHT: NodeId = NodeId::new(435);
pub const LAYOUT_NESTED_RIGHT_A: NodeId = NodeId::new(436);
pub const LAYOUT_NESTED_RIGHT_B: NodeId = NodeId::new(437);

pub const LAYOUT_SIZE_ROW: NodeId = NodeId::new(440);
pub const LAYOUT_SIZE_PREFERRED: NodeId = NodeId::new(441);
pub const LAYOUT_SIZE_MINIMUM: NodeId = NodeId::new(442);
pub const LAYOUT_SIZE_MAXIMUM: NodeId = NodeId::new(443);

pub const LAYOUT_ALIGN_ROW: NodeId = NodeId::new(450);
pub const LAYOUT_ALIGN_LEADING: NodeId = NodeId::new(451);
pub const LAYOUT_ALIGN_CENTER: NodeId = NodeId::new(452);
pub const LAYOUT_ALIGN_TRAILING: NodeId = NodeId::new(453);

// =============================================================================
// Collections (500..599)
// =============================================================================

pub const COLL_COLUMN: NodeId = NodeId::new(500);
pub const COLL_HEADING: NodeId = NodeId::new(501);
pub const COLL_SEPARATOR: NodeId = NodeId::new(502);

pub const LIST: NodeId = NodeId::new(510);
pub const LIST_CAPTION: NodeId = NodeId::new(511);
pub const TABLE: NodeId = NodeId::new(520);
pub const TABLE_CAPTION: NodeId = NodeId::new(521);
pub const TREE: NodeId = NodeId::new(530);
pub const TREE_NOTE: NodeId = NodeId::new(531);
pub const COLL_SELECTION: NodeId = NodeId::new(540);

// =============================================================================
// Protocol inspector (600..699)
// =============================================================================

pub const INSPECT_COLUMN: NodeId = NodeId::new(600);
pub const INSPECT_HEADING: NodeId = NodeId::new(601);
pub const INSPECT_SEPARATOR: NodeId = NodeId::new(602);
pub const TRACE_TABLE: NodeId = NodeId::new(610);
pub const TRACE_NOTE: NodeId = NodeId::new(611);
pub const INSPECT_LAST_EVENT: NodeId = NodeId::new(620);
pub const INSPECT_REVISION: NodeId = NodeId::new(621);
pub const INSPECT_SCENE: NodeId = NodeId::new(622);

// =============================================================================
// Connection statistics (700..799)
// =============================================================================

pub const CONN_COLUMN: NodeId = NodeId::new(700);
pub const CONN_HEADING: NodeId = NodeId::new(701);
pub const CONN_SEPARATOR: NodeId = NodeId::new(702);
pub const CONN_CLIENTS: NodeId = NodeId::new(710);
pub const CONN_REVISION: NodeId = NodeId::new(711);
pub const CONN_QUEUE: NodeId = NodeId::new(712);
pub const CONN_RETAINED: NodeId = NodeId::new(713);
pub const CONN_THROUGHPUT: NodeId = NodeId::new(714);
pub const CONN_RESOURCE: NodeId = NodeId::new(715);
pub const CONN_LATENCY: NodeId = NodeId::new(716);
pub const CONN_LAG: NodeId = NodeId::new(717);

pub const CONN_HIST_HEADING: NodeId = NodeId::new(720);
pub const CONN_HIST_COLUMN: NodeId = NodeId::new(721);
/// One `Progress` bar per transaction-size bucket; see [`crate::stats::SIZE_BUCKETS`].
pub const CONN_HIST_BARS: [NodeId; 5] = [
    NodeId::new(722),
    NodeId::new(723),
    NodeId::new(724),
    NodeId::new(725),
    NodeId::new(726),
];
pub const CONN_NOTE: NodeId = NodeId::new(730);

/// Node ids in `TELEMETRY_ID_FLOOR..TELEMETRY_ID_CEILING` carry live telemetry, not gallery
/// content.
///
/// Telemetry counters advance monotonically with traffic, so a baseline comparison across a scene
/// cycle must exclude them. Tests use this range instead of enumerating individual ids.
pub const TELEMETRY_ID_FLOOR: u64 = 600;
/// Exclusive upper bound of the telemetry range.
pub const TELEMETRY_ID_CEILING: u64 = 900;

/// Whether `id` names a telemetry node rather than gallery content.
pub fn is_telemetry(id: NodeId) -> bool {
    (TELEMETRY_ID_FLOOR..TELEMETRY_ID_CEILING).contains(&id.get())
}

// =============================================================================
// Transient nodes created by scenes (900 and upward)
// =============================================================================

/// First id in the transient block used by scenes that create nodes.
///
/// The block grows upward and ids are never reused: `DELETE_NODE` retires a node id permanently
/// for the session incarnation, and the store rejects a `CREATE_NODE` that resurrects one (§6.2).
/// A scene that creates a node therefore allocates a fresh id every time it is applied.
pub const TRANSIENT_ID_BASE: u64 = 900;

// =============================================================================
// Models (§8)
// =============================================================================

pub const LIST_MODEL: ModelId = ModelId::new(1);
pub const TABLE_MODEL: ModelId = ModelId::new(2);
pub const TRACE_MODEL: ModelId = ModelId::new(3);

/// Baseline list item ids, in baseline index order.
pub const LIST_ITEMS: [ItemId; 5] = [
    ItemId::new(1),
    ItemId::new(2),
    ItemId::new(3),
    ItemId::new(4),
    ItemId::new(5),
];
/// Baseline table item ids, in baseline index order.
pub const TABLE_ITEMS: [ItemId; 4] = [
    ItemId::new(101),
    ItemId::new(102),
    ItemId::new(103),
    ItemId::new(104),
];
/// Item ids the model scene inserts into [`LIST_MODEL`] and removes again on revert.
pub const LIST_SCENE_ITEMS: [ItemId; 2] = [ItemId::new(90), ItemId::new(91)];
/// Item id the model scene inserts into [`TABLE_MODEL`] and removes again on revert.
pub const TABLE_SCENE_ITEM: ItemId = ItemId::new(190);
/// First item id handed out by the protocol inspector; trace ids only ever increase.
pub const TRACE_ITEM_BASE: u64 = 1_000;

// =============================================================================
// Action keys (§7.7 — opaque client metadata, never dispatched server-side)
// =============================================================================

pub const ACTION_PREV_SCENE: &str = "gallery.scene.previous";
pub const ACTION_NEXT_SCENE: &str = "gallery.scene.next";
pub const ACTION_RESET: &str = "gallery.scene.reset";
pub const ACTION_AUTOPLAY: &str = "gallery.scene.autoplay";
pub const ACTION_BUTTON_NORMAL: &str = "gallery.button.normal";
pub const ACTION_BUTTON_PRIMARY: &str = "gallery.button.primary";
pub const ACTION_BUTTON_DESTRUCTIVE: &str = "gallery.button.destructive";
pub const ACTION_BUTTON_QUIET: &str = "gallery.button.quiet";
pub const ACTION_TOGGLE_CHECKBOX: &str = "gallery.toggle.checkbox";
pub const ACTION_TOGGLE_SWITCH: &str = "gallery.toggle.switch";
pub const ACTION_TOGGLE_AUTOMATIC: &str = "gallery.toggle.automatic";
