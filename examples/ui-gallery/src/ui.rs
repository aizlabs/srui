//! Construction of the complete initial semantic tree (§7.2, §7.3, §7.4, §7.5, §8, §12.1, §14).
//!
//! # Architecture & Design Invariants
//!
//! - **One transaction**: the entire gallery — models, items, and every node — is created by a
//!   single all-or-nothing transaction, so a client either sees the whole gallery or none of it
//!   (§12.1).
//! - **Renderer support set**: every node type built here is one the AppKit `ControlFactory`
//!   actually renders: all required-tier types plus the explicitly implemented deferred `Menu`.
//!   Other optional/deferred registry types remain absent because advertising an unrenderable node
//!   would be exactly the silent degradation §4 inv. 13 forbids.
//! - **Section builders**: each `build_*` function owns one 100-wide id block from [`crate::ids`]
//!   and appends one column to the gallery. Adding a widget means extending one builder, adding
//!   one scene entry in [`crate::scenes`], and one test.
//! - **Baseline constants**: every string, role, and value a scene later mutates is declared here
//!   as a `BASELINE_*` constant, so a scene's revert restores the exact original state rather than
//!   a re-typed approximation.

use srui_sdk::*;
use srui_semantic_tree::ModelItem;
use srui_sessiond::{Session, SessionError};

use crate::ids;
use crate::trace::TRACE_COLUMNS;

// =============================================================================
// Baseline content shared with `scenes.rs`
// =============================================================================

pub const BASELINE_HERO_TITLE: &str = "SRUI Semantic UI Gallery";
pub const BASELINE_HERO_BLURB: &str = concat!(
    "Every node type the AppKit renderer supports, driven entirely by semantic state. ",
    "The server owns the tree; the client owns the pixels. Nothing here is a bitmap of a ",
    "remote window \u{2014} the image below arrived as content-addressed bytes and was decoded ",
    "locally."
);
pub const BASELINE_HERO_CAPTION: &str =
    "Blue Marble \u{b7} NASA AS17-148-22727 \u{b7} public domain \u{b7} see assets/NOTICE.md";
pub const BASELINE_HERO_STATUS: &str =
    "Session live \u{b7} resource published \u{b7} awaiting interaction";
pub const BASELINE_IMAGE_PLACEHOLDER: &str =
    "Image resource cleared \u{2014} the client falls back to its placeholder";

pub const BASELINE_TYPO_BODY: &str =
    "Body \u{b7} the default reading role for paragraphs of prose.";
pub const BASELINE_PROGRESS_VALUE: f64 = 0.35;
pub const BASELINE_PROGRESS_DESCRIPTION: &str = "35% \u{b7} determinate";
pub const BASELINE_PRIMARY_LABEL: &str = "Primary";
pub const BASELINE_AUTOPLAY: bool = false;
pub const BASELINE_TOGGLE_CHECKBOX: bool = true;
pub const BASELINE_TOGGLE_SWITCH: bool = false;
pub const BASELINE_TOGGLE_AUTOMATIC: bool = false;
pub const BASELINE_MENU_ITEMS: [&str; 3] = ["Inspect", "Reconnect", "Close"];
pub const BASELINE_CTRL_STATUS: &str = "No control has been activated yet";
pub const BASELINE_COLL_SELECTION: &str = "Nothing selected";

pub const BASELINE_SIZE_PREFERRED: Size = Size::new(200.0, 24.0);
pub const BASELINE_SIZE_MINIMUM: Size = Size::new(120.0, 24.0);
pub const BASELINE_SIZE_MAXIMUM: Size = Size::new(260.0, 24.0);

/// Baseline list rows: `(item id, value)`.
pub const BASELINE_LIST_ROWS: [(u64, &str); 5] = [
    (1, "Semantic tree \u{b7} authoritative on the server"),
    (2, "Transactions \u{b7} atomic, revision-ordered"),
    (3, "Models \u{b7} windowed, stable item identity"),
    (4, "Resources \u{b7} content-addressed, chunked"),
    (5, "Events \u{b7} deduplicated, revision-stamped"),
];

/// Baseline table rows: `(item id, [subsystem, spec section, status])`.
pub const BASELINE_TABLE_ROWS: [(u64, [&str; 3]); 4] = [
    (101, ["Framing", "\u{a7}26", "16 MiB max frame"]),
    (102, ["Journal", "\u{a7}18.1", "1024 transactions"]),
    (103, ["Dedupe", "\u{a7}18.2", "bounded per client"]),
    (104, ["Resources", "\u{a7}14", "16 KiB chunks"]),
];

pub const TABLE_COLUMNS: [&str; 3] = ["Subsystem", "Spec", "Status"];

/// The tree is presentation-only today: the renderer builds an `NSOutlineView` from a flat inline
/// `items` list, and no `EXPANSION_CHANGED` event is emitted back. Labelled honestly in the UI.
pub const BASELINE_TREE_ITEMS: [&str; 6] = [
    "protocol/",
    "    srui.proto",
    "    registry.yaml",
    "server-rust/",
    "    semantic-tree/",
    "    sessiond/",
];

/// Every node type this gallery instantiates, matching the AppKit `ControlFactory` support set:
/// all §7.3 required-tier types plus explicitly implemented deferred-tier `Menu`.
pub fn supported_node_types() -> Vec<TypeRef> {
    vec![
        Surface::NODE_TYPE,
        Scroll::NODE_TYPE,
        Column::NODE_TYPE,
        Row::NODE_TYPE,
        Grid::NODE_TYPE,
        Spacer::NODE_TYPE,
        Separator::NODE_TYPE,
        Text::NODE_TYPE,
        RichText::NODE_TYPE,
        Image::NODE_TYPE,
        Button::NODE_TYPE,
        Toggle::NODE_TYPE,
        TextInput::NODE_TYPE,
        TextArea::NODE_TYPE,
        Progress::NODE_TYPE,
        List::NODE_TYPE,
        Table::NODE_TYPE,
        Tree::NODE_TYPE,
        TypeRef::MENU,
    ]
}

/// Registry node types the AppKit renderer cannot build yet, which the gallery must never create.
pub fn unsupported_node_type_names() -> Vec<&'static str> {
    vec![
        "Dialog",
        "Select",
        "ChoiceGroup",
        "Slider",
        "NumberInput",
        "Tabs",
        "Split",
        "Toolbar",
    ]
}

// =============================================================================
// Entry point
// =============================================================================

/// Creates every model, seeds its items, and builds the whole node tree in one transaction (§12.1).
///
/// Returns the operations the transaction committed so the caller can seed the protocol inspector
/// and the traffic statistics with the real initial graph rather than an estimate.
pub fn build_initial_ui(
    session: &Session,
    image: Option<ResourceHash>,
    scene_label: String,
) -> Result<Vec<Operation>, SessionError> {
    session.transaction(move |ui| {
        build_models(ui)?;
        build_frame(ui, &scene_label)?;
        build_hero(ui, image)?;
        build_typography(ui)?;
        build_controls(ui)?;
        build_layout(ui)?;
        build_collections(ui)?;
        build_inspector(ui, &scene_label)?;
        build_connection(ui)?;
        Ok(ui.operations().to_vec())
    })
}

// =============================================================================
// Models (§8)
// =============================================================================

fn build_models(ui: &mut UiTransaction) -> Result<(), StoreError> {
    ui.apply_op(&Operation::create_model(
        ids::LIST_MODEL,
        List::NODE_TYPE,
        0,
    ))?;
    ui.apply_op(&Operation::model_insert(
        ids::LIST_MODEL,
        0,
        BASELINE_LIST_ROWS
            .iter()
            .map(|(id, text)| {
                ModelItem::new(ItemId::new(*id), Value::String((*text).to_string()), [])
            })
            .collect::<Vec<_>>(),
    ))?;

    ui.apply_op(&Operation::create_model(
        ids::TABLE_MODEL,
        Table::NODE_TYPE,
        0,
    ))?;
    ui.apply_op(&Operation::model_insert(
        ids::TABLE_MODEL,
        0,
        BASELINE_TABLE_ROWS
            .iter()
            .map(|(id, cells)| ModelItem::new(ItemId::new(*id), table_row(cells), []))
            .collect::<Vec<_>>(),
    ))?;

    ui.apply_op(&Operation::create_model(
        ids::TRACE_MODEL,
        Table::NODE_TYPE,
        0,
    ))?;
    Ok(())
}

/// Builds one table row value. A multi-column row is a `Value::List` of cell strings (§8).
pub fn table_row(cells: &[&str; 3]) -> Value {
    Value::List(
        cells
            .iter()
            .map(|cell| Value::String((*cell).to_string()))
            .collect(),
    )
}

// =============================================================================
// Frame and scene toolbar
// =============================================================================

fn build_frame(ui: &mut UiTransaction, scene_label: &str) -> Result<(), StoreError> {
    Surface::builder(ids::SURFACE)
        .label("SRUI UI Gallery")
        .accessible_description("Showcase of every semantic node type the renderer supports")
        .create(ui)?;

    Column::builder(ids::ROOT_COLUMN)
        .parent(ids::SURFACE)
        .spacing_role(SpacingRole::Normal)
        .padding_role(PaddingRole::Normal)
        .grow(1.0)
        .create(ui)?;

    Row::builder(ids::TOOLBAR)
        .parent(ids::ROOT_COLUMN)
        .spacing_role(SpacingRole::Tight)
        .accessible_description("Scene navigation")
        .create(ui)?;

    Button::builder(ids::BTN_PREV)
        .parent(ids::TOOLBAR)
        .label("\u{2039} Previous Scene")
        .role(ActionRole::Normal)
        .action_key(ids::ACTION_PREV_SCENE)
        .create(ui)?;

    Button::builder(ids::BTN_NEXT)
        .parent(ids::TOOLBAR)
        .label("Next Scene \u{203a}")
        .role(ActionRole::Primary)
        .action_key(ids::ACTION_NEXT_SCENE)
        .create(ui)?;

    Button::builder(ids::BTN_RESET)
        .parent(ids::TOOLBAR)
        .label("Reset")
        .role(ActionRole::Quiet)
        .action_key(ids::ACTION_RESET)
        .create(ui)?;

    Toggle::switch(ids::TOGGLE_AUTOPLAY)
        .parent(ids::TOOLBAR)
        .label("Auto Play")
        .value(BASELINE_AUTOPLAY)
        .action_key(ids::ACTION_AUTOPLAY)
        .accessible_description("Advance to the next scene every few seconds")
        .create(ui)?;

    Spacer::builder(ids::TOOLBAR_SPACER)
        .parent(ids::TOOLBAR)
        .grow(1.0)
        .create(ui)?;

    Text::builder(ids::SCENE_LABEL)
        .parent(ids::TOOLBAR)
        .text(scene_label)
        .role(TextRole::Status)
        .create(ui)?;

    // The renderer opens its window at a fixed default and then grows it to fit intrinsic
    // content. Without a size hint, the widest caption in the gallery would stretch the window
    // into a single very wide strip. `preferred_size` is advisory (§7.4): the client applies it
    // at `defaultHigh` priority and the user can still resize freely.
    Scroll::builder(ids::ROOT_SCROLL)
        .parent(ids::ROOT_COLUMN)
        .grow(1.0)
        .preferred_size(Size::new(940.0, 680.0))
        .accessible_description("Gallery sections")
        .create(ui)?;

    Column::builder(ids::GALLERY_COLUMN)
        .parent(ids::ROOT_SCROLL)
        .spacing_role(SpacingRole::Relaxed)
        .padding_role(PaddingRole::Normal)
        .grow(1.0)
        .maximum_size(Size::new(900.0, 20_000.0))
        .create(ui)?;

    Ok(())
}

/// Opens a gallery section: a column with a heading and a rule under it.
fn open_section(
    ui: &mut UiTransaction,
    column: NodeId,
    heading: NodeId,
    separator: NodeId,
    title: &str,
) -> Result<(), StoreError> {
    Column::builder(column)
        .parent(ids::GALLERY_COLUMN)
        .spacing_role(SpacingRole::Normal)
        .padding_role(PaddingRole::Normal)
        .create(ui)?;
    Text::builder(heading)
        .parent(column)
        .text(title)
        .role(TextRole::Heading)
        .create(ui)?;
    Separator::builder(separator).parent(column).create(ui)?;
    Ok(())
}

// =============================================================================
// Hero — image resource, title, rich description, status (§14)
// =============================================================================

fn build_hero(ui: &mut UiTransaction, image: Option<ResourceHash>) -> Result<(), StoreError> {
    open_section(
        ui,
        ids::HERO_COLUMN,
        ids::HERO_HEADING,
        ids::HERO_SEPARATOR,
        "Hero \u{b7} Image resource",
    )?;

    Row::builder(ids::HERO_ROW)
        .parent(ids::HERO_COLUMN)
        .spacing_role(SpacingRole::Relaxed)
        .vertical_alignment(VerticalAlignment::Top)
        .create(ui)?;

    let mut builder = Image::builder(ids::HERO_IMAGE)
        .parent(ids::HERO_ROW)
        .label("Blue Marble")
        .accessible_description(
            "Photograph of Earth taken by the Apollo 17 crew, delivered as a chunked SRUI resource",
        )
        // Set explicitly, not left to the client default: the image scene toggles this property,
        // so the baseline must declare the value its revert restores.
        .visibility(Visibility::Visible)
        .preferred_size(Size::new(240.0, 240.0));
    if let Some(hash) = image {
        builder = builder.resource(hash);
    }
    builder.create(ui)?;

    // Collapsed until the image scene clears the resource; kept in the graph so the fallback is a
    // scalar visibility flip rather than a structural rebuild (§23).
    Text::builder(ids::HERO_IMAGE_PLACEHOLDER)
        .parent(ids::HERO_ROW)
        .text(BASELINE_IMAGE_PLACEHOLDER)
        .role(TextRole::Warning)
        .visibility(Visibility::Collapsed)
        .create(ui)?;

    Column::builder(ids::HERO_TEXT_COLUMN)
        .parent(ids::HERO_ROW)
        .spacing_role(SpacingRole::Tight)
        .grow(1.0)
        .create(ui)?;

    Text::builder(ids::HERO_TITLE)
        .parent(ids::HERO_TEXT_COLUMN)
        .text(BASELINE_HERO_TITLE)
        .role(TextRole::Title)
        .create(ui)?;

    RichText::builder(ids::HERO_BLURB)
        .parent(ids::HERO_TEXT_COLUMN)
        .text(BASELINE_HERO_BLURB)
        .role(TextRole::Body)
        .read_only(true)
        .create(ui)?;

    Text::builder(ids::HERO_CAPTION)
        .parent(ids::HERO_TEXT_COLUMN)
        .text(BASELINE_HERO_CAPTION)
        .role(TextRole::Caption)
        .create(ui)?;

    Text::builder(ids::HERO_STATUS)
        .parent(ids::HERO_TEXT_COLUMN)
        .text(BASELINE_HERO_STATUS)
        .role(TextRole::Status)
        .create(ui)?;

    Ok(())
}

// =============================================================================
// Typography — all eight standard text roles (§7.5)
// =============================================================================

fn build_typography(ui: &mut UiTransaction) -> Result<(), StoreError> {
    open_section(
        ui,
        ids::TYPO_COLUMN,
        ids::TYPO_HEADING,
        ids::TYPO_SEPARATOR,
        "Typography \u{b7} Standard text roles",
    )?;

    let samples: [(NodeId, TextRole, &str); 8] = [
        (
            ids::TYPO_TITLE,
            TextRole::Title,
            "Title \u{b7} the largest role",
        ),
        (
            ids::TYPO_SUBHEADING,
            TextRole::Heading,
            "Heading \u{b7} section headers",
        ),
        (ids::TYPO_BODY, TextRole::Body, BASELINE_TYPO_BODY),
        (
            ids::TYPO_CAPTION,
            TextRole::Caption,
            "Caption \u{b7} secondary annotation",
        ),
        (
            ids::TYPO_CODE,
            TextRole::Code,
            "Code \u{b7} cargo test --manifest-path examples/ui-gallery/Cargo.toml",
        ),
        (
            ids::TYPO_STATUS,
            TextRole::Status,
            "Status \u{b7} ambient state, not an alert",
        ),
        (
            ids::TYPO_WARNING,
            TextRole::Warning,
            "Warning \u{b7} recoverable, needs attention",
        ),
        (
            ids::TYPO_ERROR,
            TextRole::Error,
            "Error \u{b7} the operation did not happen",
        ),
    ];

    for (node, role, text) in samples {
        // Every role node declares `visibility` explicitly: the state scene collapses one of them,
        // and a revert can only restore a value the baseline actually set.
        Text::builder(node)
            .parent(ids::TYPO_COLUMN)
            .text(text)
            .role(role)
            .visibility(Visibility::Visible)
            .create(ui)?;
    }

    RichText::builder(ids::TYPO_RICH)
        .parent(ids::TYPO_COLUMN)
        .text(concat!(
            "RichText is a selectable, structured text surface (\u{a7}9). It is distinct from Text: ",
            "Text is a label the client may lay out freely, RichText carries selectable content."
        ))
        .role(TextRole::Body)
        .read_only(true)
        .create(ui)?;

    Ok(())
}

// =============================================================================
// Controls — buttons, toggles, inputs, progress (§7.2, §7.4, §7.5)
// =============================================================================

fn build_controls(ui: &mut UiTransaction) -> Result<(), StoreError> {
    open_section(
        ui,
        ids::CTRL_COLUMN,
        ids::CTRL_HEADING,
        ids::CTRL_SEPARATOR,
        "Controls · Buttons, toggles, inputs, progress, menu",
    )?;

    Row::builder(ids::CTRL_BUTTON_ROW)
        .parent(ids::CTRL_COLUMN)
        .spacing_role(SpacingRole::Normal)
        .accessible_description("One button per standard action role")
        .create(ui)?;

    let buttons: [(NodeId, ActionRole, &str, &str); 4] = [
        (
            ids::BTN_NORMAL,
            ActionRole::Normal,
            "Normal",
            ids::ACTION_BUTTON_NORMAL,
        ),
        (
            ids::BTN_PRIMARY,
            ActionRole::Primary,
            BASELINE_PRIMARY_LABEL,
            ids::ACTION_BUTTON_PRIMARY,
        ),
        (
            ids::BTN_DESTRUCTIVE,
            ActionRole::Destructive,
            "Destructive",
            ids::ACTION_BUTTON_DESTRUCTIVE,
        ),
        (
            ids::BTN_QUIET,
            ActionRole::Quiet,
            "Quiet",
            ids::ACTION_BUTTON_QUIET,
        ),
    ];
    for (node, role, label, action_key) in buttons {
        Button::builder(node)
            .parent(ids::CTRL_BUTTON_ROW)
            .label(label)
            .role(role)
            .action_key(action_key)
            .enabled(true)
            .create(ui)?;
    }

    Row::builder(ids::CTRL_TOGGLE_ROW)
        .parent(ids::CTRL_COLUMN)
        .spacing_role(SpacingRole::Normal)
        .accessible_description("One Toggle per presentation hint")
        .create(ui)?;

    Toggle::checkbox(ids::TOGGLE_CHECKBOX)
        .parent(ids::CTRL_TOGGLE_ROW)
        .label("Checkbox hint")
        .value(BASELINE_TOGGLE_CHECKBOX)
        .action_key(ids::ACTION_TOGGLE_CHECKBOX)
        .create(ui)?;
    Toggle::switch(ids::TOGGLE_SWITCH)
        .parent(ids::CTRL_TOGGLE_ROW)
        .label("Switch hint")
        .value(BASELINE_TOGGLE_SWITCH)
        .action_key(ids::ACTION_TOGGLE_SWITCH)
        .visibility(Visibility::Visible)
        .create(ui)?;
    Toggle::automatic(ids::TOGGLE_AUTOMATIC)
        .parent(ids::CTRL_TOGGLE_ROW)
        .label("Automatic hint")
        .value(BASELINE_TOGGLE_AUTOMATIC)
        .action_key(ids::ACTION_TOGGLE_AUTOMATIC)
        .create(ui)?;

    Grid::builder(ids::CTRL_INPUT_GRID)
        .parent(ids::CTRL_COLUMN)
        .columns([Value::String("Input".into()), Value::String("Input".into())])
        .spacing_role(SpacingRole::Normal)
        .accessible_description("Text inputs across roles, states, and validation")
        .create(ui)?;

    let inputs: [(NodeId, InputRole, &str, &str); 4] = [
        (ids::INPUT_PLAIN, InputRole::Plain, "plain text", "Plain"),
        (ids::INPUT_SEARCH, InputRole::Search, "", "Search"),
        (ids::INPUT_SECURE, InputRole::Secure, "", "Secure"),
        (
            ids::INPUT_COMMAND,
            InputRole::Command,
            "cargo clippy --all-targets",
            "Command",
        ),
    ];
    for (node, role, value, label) in inputs {
        TextInput::builder(node)
            .parent(ids::CTRL_INPUT_GRID)
            .label(label)
            .role(role)
            .value(value)
            .placeholder(format!("{label} input"))
            .read_only(false)
            .validation_state(ValidationState::Valid)
            .create(ui)?;
    }

    TextInput::builder(ids::INPUT_READ_ONLY)
        .parent(ids::CTRL_INPUT_GRID)
        .label("Read-only")
        .role(InputRole::Plain)
        .value("server-owned value")
        .read_only(true)
        .validation_state(ValidationState::Valid)
        .create(ui)?;

    TextInput::builder(ids::INPUT_INVALID)
        .parent(ids::CTRL_INPUT_GRID)
        .label("Validated")
        .role(InputRole::Plain)
        .value("looks fine")
        .read_only(false)
        .validation_state(ValidationState::Valid)
        .value_description("Validation state is server-authoritative")
        .create(ui)?;

    TextArea::builder(ids::TEXT_AREA)
        .parent(ids::CTRL_COLUMN)
        .label("Notes")
        .value(concat!(
            "TextArea content is authoritative server state.\n",
            "Typing here changes the local NSTextView only."
        ))
        .placeholder("Multi-line notes")
        .read_only(false)
        .create(ui)?;

    Text::builder(ids::TEXT_AREA_NOTE)
        .parent(ids::CTRL_COLUMN)
        .text(concat!(
            "Honest limitation: text editing is native-local. The renderer does not emit ",
            "TEXT_EDIT yet, so keystrokes never reach the server and are discarded on the next ",
            "server-driven SET_PROPERTY."
        ))
        .role(TextRole::Caption)
        .create(ui)?;

    Row::builder(ids::CTRL_PROGRESS_ROW)
        .parent(ids::CTRL_COLUMN)
        .spacing_role(SpacingRole::Normal)
        .create(ui)?;

    Progress::builder(ids::PROGRESS_DETERMINATE)
        .parent(ids::CTRL_PROGRESS_ROW)
        .label("Determinate")
        .value(BASELINE_PROGRESS_VALUE)
        .value_description(BASELINE_PROGRESS_DESCRIPTION)
        .grow(1.0)
        .create(ui)?;

    Progress::builder(ids::PROGRESS_BUSY)
        .parent(ids::CTRL_PROGRESS_ROW)
        .label("Busy flag")
        .value(0.0)
        .value_description("idle")
        .busy(false)
        .grow(1.0)
        .create(ui)?;

    ui.create_node(
        ids::MENU,
        TypeRef::MENU,
        Some(ids::CTRL_COLUMN),
        None,
        [
            (LABEL, Value::String("Command menu".to_string())),
            (
                ITEMS,
                Value::List(
                    BASELINE_MENU_ITEMS
                        .iter()
                        .map(|item| Value::String((*item).to_string()))
                        .collect(),
                ),
            ),
            (
                ACCESSIBLE_DESCRIPTION,
                Value::String(
                    "Renderer-owned deferred-tier Menu; the standard registry declares no events"
                        .to_string(),
                ),
            ),
            (ENABLED, Value::Bool(true)),
        ],
    )?;

    Text::builder(ids::MENU_NOTE)
        .parent(ids::CTRL_COLUMN)
        .text("Menu is deferred-tier presentation only; the standard registry declares no events.")
        .role(TextRole::Caption)
        .create(ui)?;

    Text::builder(ids::CTRL_STATUS)
        .parent(ids::CTRL_COLUMN)
        .text(BASELINE_CTRL_STATUS)
        .role(TextRole::Status)
        .accessible_description("Outcome of the last control activation")
        .create(ui)?;

    Ok(())
}

// =============================================================================
// Layout — rows, columns, grids, spacing, padding, alignment, sizing (§7.4)
// =============================================================================

fn build_layout(ui: &mut UiTransaction) -> Result<(), StoreError> {
    open_section(
        ui,
        ids::LAYOUT_COLUMN,
        ids::LAYOUT_HEADING,
        ids::LAYOUT_SEPARATOR,
        "Layout \u{b7} Spacing, padding, alignment, sizing",
    )?;

    Row::builder(ids::LAYOUT_ROW)
        .parent(ids::LAYOUT_COLUMN)
        .spacing_role(SpacingRole::Normal)
        .accessible_description("Row with a growing spacer between the second and third item")
        .create(ui)?;
    Text::builder(ids::LAYOUT_ROW_A)
        .parent(ids::LAYOUT_ROW)
        .text("leading")
        .role(TextRole::Body)
        .create(ui)?;
    Text::builder(ids::LAYOUT_ROW_B)
        .parent(ids::LAYOUT_ROW)
        .text("next")
        .role(TextRole::Body)
        .create(ui)?;
    Spacer::builder(ids::LAYOUT_SPACER)
        .parent(ids::LAYOUT_ROW)
        .grow(1.0)
        .create(ui)?;
    Text::builder(ids::LAYOUT_ROW_C)
        .parent(ids::LAYOUT_ROW)
        .text("trailing")
        .role(TextRole::Body)
        .create(ui)?;

    Grid::builder(ids::LAYOUT_GRID)
        .parent(ids::LAYOUT_COLUMN)
        .columns([
            Value::String("A".into()),
            Value::String("B".into()),
            Value::String("C".into()),
        ])
        .spacing_role(SpacingRole::Tight)
        .accessible_description("Three-column grid of six cells")
        .create(ui)?;
    for (index, cell) in ids::LAYOUT_GRID_CELLS.iter().enumerate() {
        Text::builder(*cell)
            .parent(ids::LAYOUT_GRID)
            .text(format!("cell {}", index + 1))
            .role(TextRole::Caption)
            .create(ui)?;
    }

    Row::builder(ids::LAYOUT_NESTED_ROW)
        .parent(ids::LAYOUT_COLUMN)
        .spacing_role(SpacingRole::Relaxed)
        .accessible_description("Two nested columns divided by a separator")
        .create(ui)?;
    Column::builder(ids::LAYOUT_NESTED_LEFT)
        .parent(ids::LAYOUT_NESTED_ROW)
        .spacing_role(SpacingRole::Tight)
        .grow(1.0)
        .create(ui)?;
    Text::builder(ids::LAYOUT_NESTED_LEFT_A)
        .parent(ids::LAYOUT_NESTED_LEFT)
        .text("nested column \u{b7} first")
        .role(TextRole::Body)
        .create(ui)?;
    Text::builder(ids::LAYOUT_NESTED_LEFT_B)
        .parent(ids::LAYOUT_NESTED_LEFT)
        .text("nested column \u{b7} second")
        .role(TextRole::Body)
        .create(ui)?;
    Separator::builder(ids::LAYOUT_NESTED_SEPARATOR)
        .parent(ids::LAYOUT_NESTED_ROW)
        .create(ui)?;
    Column::builder(ids::LAYOUT_NESTED_RIGHT)
        .parent(ids::LAYOUT_NESTED_ROW)
        .spacing_role(SpacingRole::Tight)
        .grow(1.0)
        .create(ui)?;
    Text::builder(ids::LAYOUT_NESTED_RIGHT_A)
        .parent(ids::LAYOUT_NESTED_RIGHT)
        .text("sibling column \u{b7} first")
        .role(TextRole::Body)
        .create(ui)?;
    Text::builder(ids::LAYOUT_NESTED_RIGHT_B)
        .parent(ids::LAYOUT_NESTED_RIGHT)
        .text("sibling column \u{b7} second")
        .role(TextRole::Body)
        .create(ui)?;

    Row::builder(ids::LAYOUT_SIZE_ROW)
        .parent(ids::LAYOUT_COLUMN)
        .spacing_role(SpacingRole::Normal)
        .accessible_description("Preferred, minimum, and maximum size hints")
        .create(ui)?;
    Text::builder(ids::LAYOUT_SIZE_PREFERRED)
        .parent(ids::LAYOUT_SIZE_ROW)
        .text("preferred_size")
        .role(TextRole::Caption)
        .preferred_size(BASELINE_SIZE_PREFERRED)
        .create(ui)?;
    Text::builder(ids::LAYOUT_SIZE_MINIMUM)
        .parent(ids::LAYOUT_SIZE_ROW)
        .text("minimum_size")
        .role(TextRole::Caption)
        .minimum_size(BASELINE_SIZE_MINIMUM)
        .create(ui)?;
    Text::builder(ids::LAYOUT_SIZE_MAXIMUM)
        .parent(ids::LAYOUT_SIZE_ROW)
        .text("maximum_size")
        .role(TextRole::Caption)
        .maximum_size(BASELINE_SIZE_MAXIMUM)
        .create(ui)?;

    Row::builder(ids::LAYOUT_ALIGN_ROW)
        .parent(ids::LAYOUT_COLUMN)
        .spacing_role(SpacingRole::Normal)
        .accessible_description("Horizontal alignment values")
        .create(ui)?;
    Text::builder(ids::LAYOUT_ALIGN_LEADING)
        .parent(ids::LAYOUT_ALIGN_ROW)
        .text("leading")
        .role(TextRole::Caption)
        .horizontal_alignment(HorizontalAlignment::Leading)
        .create(ui)?;
    Text::builder(ids::LAYOUT_ALIGN_CENTER)
        .parent(ids::LAYOUT_ALIGN_ROW)
        .text("center")
        .role(TextRole::Caption)
        .horizontal_alignment(HorizontalAlignment::Center)
        .create(ui)?;
    Text::builder(ids::LAYOUT_ALIGN_TRAILING)
        .parent(ids::LAYOUT_ALIGN_ROW)
        .text("trailing")
        .role(TextRole::Caption)
        .horizontal_alignment(HorizontalAlignment::Trailing)
        .create(ui)?;

    Ok(())
}

// =============================================================================
// Collections — model-backed list and table, presentation-only tree (§8)
// =============================================================================

fn build_collections(ui: &mut UiTransaction) -> Result<(), StoreError> {
    open_section(
        ui,
        ids::COLL_COLUMN,
        ids::COLL_HEADING,
        ids::COLL_SEPARATOR,
        "Collections \u{b7} List, Table, Tree",
    )?;

    List::builder(ids::LIST)
        .parent(ids::COLL_COLUMN)
        .model_ref(ids::LIST_MODEL)
        .selection_mode(SelectionMode::Single)
        .label("Protocol concepts")
        .accessible_description("Model-backed list with stable item identity")
        .create(ui)?;
    Text::builder(ids::LIST_CAPTION)
        .parent(ids::COLL_COLUMN)
        .text("List \u{b7} model-backed, single selection, SELECTION_CHANGED reaches the server")
        .role(TextRole::Caption)
        .create(ui)?;

    Table::builder(ids::TABLE)
        .parent(ids::COLL_COLUMN)
        .model_ref(ids::TABLE_MODEL)
        .columns(
            TABLE_COLUMNS
                .iter()
                .map(|title| Value::String((*title).to_string())),
        )
        .selection_mode(SelectionMode::Single)
        .label("Subsystem limits")
        .accessible_description("Model-backed table; each row value is a list of cell strings")
        .create(ui)?;
    Text::builder(ids::TABLE_CAPTION)
        .parent(ids::COLL_COLUMN)
        .text("Table \u{b7} model-backed, three columns, single selection")
        .role(TextRole::Caption)
        .create(ui)?;

    // The renderer builds the outline from an inline `items` list of strings, not from a model:
    // there is no hierarchical model type yet. Set through the generic property escape hatch
    // because `TreeBuilder` intentionally exposes only `model_ref`.
    Tree::builder(ids::TREE)
        .parent(ids::COLL_COLUMN)
        .label("Repository layout")
        .accessible_description("Presentation-only outline built from a flat item list")
        .property(
            ITEMS,
            Value::List(
                BASELINE_TREE_ITEMS
                    .iter()
                    .map(|item| Value::String((*item).to_string()))
                    .collect(),
            ),
        )
        .create(ui)?;
    Text::builder(ids::TREE_NOTE)
        .parent(ids::COLL_COLUMN)
        .text(concat!(
            "Honest limitation: the tree is presentation-only. Hierarchical model data and ",
            "EXPANSION_CHANGED are not wired yet, so expanding a row changes nothing on the ",
            "server and no selection is reported."
        ))
        .role(TextRole::Caption)
        .create(ui)?;

    Text::builder(ids::COLL_SELECTION)
        .parent(ids::COLL_COLUMN)
        .text(BASELINE_COLL_SELECTION)
        .role(TextRole::Status)
        .accessible_description(
            "Most recent SELECTION_CHANGED resolved against authoritative state",
        )
        .create(ui)?;

    Ok(())
}

// =============================================================================
// Protocol inspector (§7.6, §12.1, §13)
// =============================================================================

fn build_inspector(ui: &mut UiTransaction, scene_label: &str) -> Result<(), StoreError> {
    open_section(
        ui,
        ids::INSPECT_COLUMN,
        ids::INSPECT_HEADING,
        ids::INSPECT_SEPARATOR,
        "Protocol inspector \u{b7} Semantic traffic, both directions",
    )?;

    Table::builder(ids::TRACE_TABLE)
        .parent(ids::INSPECT_COLUMN)
        .model_ref(ids::TRACE_MODEL)
        .columns(
            TRACE_COLUMNS
                .iter()
                .map(|title| Value::String((*title).to_string())),
        )
        .selection_mode(SelectionMode::None)
        .label("Semantic traffic")
        .accessible_description("Newest first: client events and the operations they produced")
        .grow(1.0)
        .create(ui)?;

    Text::builder(ids::TRACE_NOTE)
        .parent(ids::INSPECT_COLUMN)
        .text(concat!(
            "S\u{2192}C rows are operations this server committed; C\u{2192}S rows are events that ",
            "reached a handler. Each batch is appended inside the very transaction it describes, ",
            "so a row and its change arrive at the same revision. Framing, the handshake, ",
            "EVENT_ACK, and RESOURCE_* chunk frames are produced below the Session API and are ",
            "not visible to an application \u{2014} they are deliberately absent rather than faked."
        ))
        .role(TextRole::Caption)
        .create(ui)?;

    Text::builder(ids::INSPECT_LAST_EVENT)
        .parent(ids::INSPECT_COLUMN)
        .text("Last event: none")
        .role(TextRole::Code)
        .create(ui)?;
    Text::builder(ids::INSPECT_REVISION)
        .parent(ids::INSPECT_COLUMN)
        .text("Revision: 1")
        .role(TextRole::Status)
        .create(ui)?;
    Text::builder(ids::INSPECT_SCENE)
        .parent(ids::INSPECT_COLUMN)
        .text(scene_label)
        .role(TextRole::Status)
        .create(ui)?;

    Ok(())
}

// =============================================================================
// Connection statistics (§18.1, §19, §20.2, §26)
// =============================================================================

fn build_connection(ui: &mut UiTransaction) -> Result<(), StoreError> {
    open_section(
        ui,
        ids::CONN_COLUMN,
        ids::CONN_HEADING,
        ids::CONN_SEPARATOR,
        "Connection \u{b7} Throughput and server-side latency",
    )?;

    for node in [
        ids::CONN_CLIENTS,
        ids::CONN_REVISION,
        ids::CONN_QUEUE,
        ids::CONN_RETAINED,
        ids::CONN_THROUGHPUT,
        ids::CONN_RESOURCE,
        ids::CONN_LATENCY,
        ids::CONN_LAG,
    ] {
        Text::builder(node)
            .parent(ids::CONN_COLUMN)
            .text("collecting\u{2026}")
            .role(TextRole::Code)
            .create(ui)?;
    }

    Text::builder(ids::CONN_HIST_HEADING)
        .parent(ids::CONN_COLUMN)
        .text("Framed transaction size distribution")
        .role(TextRole::Caption)
        .create(ui)?;

    Column::builder(ids::CONN_HIST_COLUMN)
        .parent(ids::CONN_COLUMN)
        .spacing_role(SpacingRole::Tight)
        .create(ui)?;
    for (index, node) in ids::CONN_HIST_BARS.iter().enumerate() {
        Progress::builder(*node)
            .parent(ids::CONN_HIST_COLUMN)
            .label(crate::stats::SIZE_BUCKETS[index].0)
            .value(0.0)
            .value_description(format!("{} \u{b7} 0", crate::stats::SIZE_BUCKETS[index].0))
            .grow(1.0)
            .create(ui)?;
    }

    Text::builder(ids::CONN_NOTE)
        .parent(ids::CONN_COLUMN)
        .text(concat!(
            "Measured above the Session API. Throughput re-encodes each committed operation list ",
            "into the exact SruiMessage frame the connection writes, before SSH encryption. ",
            "Latency is server-side handling time only \u{2014} the server never sees the client's ",
            "clock, so this is not a round-trip measurement."
        ))
        .role(TextRole::Caption)
        .create(ui)?;

    Ok(())
}
