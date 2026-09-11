//! Guided mutation sequence (§12.1, §13, §14, §23).
//!
//! # Architecture & Design Invariants
//!
//! - **Every scene is reversible**: a scene declares [`Scene::apply`] and an exactly matching
//!   [`Scene::revert`]. A transition is `revert(current)` followed by `apply(next)` inside one
//!   transaction, so the graph after any path through the scene list depends only on the scene
//!   that is currently applied — never on how it was reached (§12.1).
//! - **Reset is not a rebuild**: resetting reverts the applied scene and nothing more. Node ids,
//!   model ids, and item ids survive, so the client keeps every view it already has (§6.2, §23).
//! - **Incremental by construction**: no scene emits `CREATE_NODE` for a node that already exists
//!   or `MODEL_RESET_RANGE` for a change expressible as insert/update/delete. Steady-state traffic
//!   is `SET_PROPERTY` plus targeted model operations.
//! - **Extending**: add a variant, a `name`/`summary` arm, an `apply`/`revert` arm, and a test.
//!   Nothing else in the crate needs to change.

use srui_sdk::*;
use srui_semantic_tree::ModelItem;

use crate::ids;
use crate::ui;

/// Per-session scene bookkeeping: the published resource and the transient node allocator.
///
/// Held by [`crate::GalleryState`] and threaded through every `apply`/`revert` so scenes stay
/// free functions over an open transaction rather than reaching back into the application.
#[derive(Debug, Clone, Default)]
pub struct SceneContext {
    /// Hash of the published gallery image, restored by the image scene's revert (§14).
    image: Option<ResourceHash>,
    /// Next free id in the transient block.
    next_transient: u64,
    /// Nodes the applied scene created, deleted again on revert.
    transient: Vec<NodeId>,
}

impl SceneContext {
    /// Creates a context for a session whose gallery image hashes to `image`.
    pub fn new(image: Option<ResourceHash>) -> Self {
        Self {
            image,
            next_transient: ids::TRANSIENT_ID_BASE,
            transient: Vec::new(),
        }
    }

    /// Hash of the published gallery image, if publication succeeded.
    pub fn image(&self) -> Option<ResourceHash> {
        self.image
    }

    /// Nodes the currently applied scene created.
    pub fn transient(&self) -> &[NodeId] {
        &self.transient
    }

    /// Reserves a never-before-used node id for a scene-created node (§6.2).
    fn allocate(&mut self) -> Result<NodeId, StoreError> {
        // `next_transient` is zero on a defaulted context; start the block where it belongs.
        if self.next_transient < ids::TRANSIENT_ID_BASE {
            self.next_transient = ids::TRANSIENT_ID_BASE;
        }
        let id = NodeId::new(self.next_transient);
        self.next_transient = self.next_transient.checked_add(1).ok_or_else(|| {
            StoreError::OperationError("transient node id space is exhausted".to_string())
        })?;
        self.transient.push(id);
        Ok(id)
    }

    /// Hands back every id the applied scene created and clears the list.
    fn take_transient(&mut self) -> Vec<NodeId> {
        std::mem::take(&mut self.transient)
    }
}

/// One step of the guided tour.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Scene {
    /// The graph exactly as [`crate::ui::build_initial_ui`] created it.
    #[default]
    Baseline,
    /// In-place text, label, role, and progress changes.
    Content,
    /// `enabled`, `read_only`, `visibility`, `busy`, and `validation_state` changes.
    State,
    /// Image resource cleared; advancing restores it from the client's cache.
    ImageOffline,
    /// `MODEL_INSERT`, `MODEL_UPDATE`, and `MODEL_DELETE` across the list and the table.
    Models,
    /// `CREATE_NODE`, `MOVE_NODE`, `REORDER_CHILDREN`, and `DELETE_NODE`.
    Structure,
    /// Spacing, padding, alignment, and size changes.
    Layout,
}

/// Every scene, in tour order. Index 0 is always the baseline.
pub const SCENES: [Scene; 7] = [
    Scene::Baseline,
    Scene::Content,
    Scene::State,
    Scene::ImageOffline,
    Scene::Models,
    Scene::Structure,
    Scene::Layout,
];

const SCENE_CONTENT_TITLE: &str = "Scene 2 \u{b7} Content changed in place";
const SCENE_CONTENT_BLURB: &str = concat!(
    "Only the properties that changed were sent. The client mutated the existing views \u{2014} no ",
    "node was recreated, no window was remounted, and the scroll position is untouched."
);
const SCENE_CONTENT_PROGRESS: f64 = 0.82;
const SCENE_CONTENT_PROGRESS_DESCRIPTION: &str = "82% \u{b7} advanced by the content scene";
const SCENE_CONTENT_PRIMARY_LABEL: &str = "Committed";

const SCENE_LIST_INSERTED: [&str; 2] = [
    "Inserted by MODEL_INSERT at index 1",
    "Inserted by MODEL_INSERT at index 2",
];
const SCENE_LIST_UPDATED: &str = "Transactions \u{b7} updated by MODEL_UPDATE";
const SCENE_TABLE_INSERTED: [&str; 3] = ["Gallery", "\u{a7}7.3", "inserted row"];
const SCENE_TABLE_UPDATED: [&str; 3] = ["Journal", "\u{a7}18.1", "updated by MODEL_UPDATE"];
const SCENE_BADGE_TEXT: &str = "Temporary node \u{b7} created, moved, then deleted";

const SCENE_SIZE_PREFERRED: Size = Size::new(320.0, 44.0);

impl Scene {
    /// Position of this scene in [`SCENES`].
    pub fn index(self) -> usize {
        SCENES
            .iter()
            .position(|scene| *scene == self)
            .expect("every Scene variant is listed in SCENES")
    }

    /// Scene at `index`, wrapping around the tour.
    pub fn from_index(index: usize) -> Self {
        SCENES[index % SCENES.len()]
    }

    /// Next scene in the tour, wrapping back to [`Scene::Baseline`].
    pub fn next(self) -> Self {
        Self::from_index(self.index() + 1)
    }

    /// Previous scene in the tour, wrapping to the last scene.
    pub fn previous(self) -> Self {
        Self::from_index(self.index() + SCENES.len() - 1)
    }

    /// Short scene name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Baseline => "Baseline",
            Self::Content => "Content",
            Self::State => "State",
            Self::ImageOffline => "Image resource",
            Self::Models => "Collection models",
            Self::Structure => "Structure",
            Self::Layout => "Layout",
        }
    }

    /// What the scene demonstrates, shown in the toolbar.
    pub fn summary(self) -> &'static str {
        match self {
            Self::Baseline => "the gallery as first published",
            Self::Content => "SET_PROPERTY on text, labels, roles, and progress",
            Self::State => "enabled, read_only, visibility, busy, validation_state",
            Self::ImageOffline => "resource cleared; advancing restores it from the client cache",
            Self::Models => "MODEL_INSERT, MODEL_UPDATE, MODEL_DELETE",
            Self::Structure => "CREATE_NODE, MOVE_NODE, REORDER_CHILDREN, DELETE_NODE",
            Self::Layout => "spacing, padding, alignment, and size hints",
        }
    }

    /// Toolbar label: position, name, and summary.
    pub fn label(self) -> String {
        format!(
            "Scene {}/{} \u{b7} {} \u{2014} {}",
            self.index() + 1,
            SCENES.len(),
            self.name(),
            self.summary()
        )
    }

    /// Applies this scene's mutations to an open transaction.
    pub fn apply(self, ui: &mut UiTransaction, ctx: &mut SceneContext) -> Result<(), StoreError> {
        match self {
            Self::Baseline => Ok(()),
            Self::Content => apply_content(ui),
            Self::State => apply_state(ui),
            Self::ImageOffline => apply_image_offline(ui),
            Self::Models => apply_models(ui),
            Self::Structure => apply_structure(ui, ctx),
            Self::Layout => apply_layout(ui),
        }
    }

    /// Undoes exactly what [`Scene::apply`] did, restoring the baseline graph.
    pub fn revert(self, ui: &mut UiTransaction, ctx: &mut SceneContext) -> Result<(), StoreError> {
        match self {
            Self::Baseline => Ok(()),
            Self::Content => revert_content(ui),
            Self::State => revert_state(ui),
            Self::ImageOffline => revert_image_offline(ui, ctx.image()),
            Self::Models => revert_models(ui),
            Self::Structure => revert_structure(ui, ctx),
            Self::Layout => revert_layout(ui),
        }
    }
}

// =============================================================================
// Content — scalar SET_PROPERTY only
// =============================================================================

fn apply_content(ui: &mut UiTransaction) -> Result<(), StoreError> {
    Text::set_text_for(ui, ids::HERO_TITLE, SCENE_CONTENT_TITLE)?;
    RichText::set_text_for(ui, ids::HERO_BLURB, SCENE_CONTENT_BLURB)?;
    Text::set_role_for(ui, ids::TYPO_BODY, TextRole::Warning)?;
    Button::set_label_for(ui, ids::BTN_PRIMARY, SCENE_CONTENT_PRIMARY_LABEL)?;
    Progress::set_value_for(ui, ids::PROGRESS_DETERMINATE, SCENE_CONTENT_PROGRESS)?;
    Progress::set_value_description_for(
        ui,
        ids::PROGRESS_DETERMINATE,
        SCENE_CONTENT_PROGRESS_DESCRIPTION,
    )?;
    Ok(())
}

fn revert_content(ui: &mut UiTransaction) -> Result<(), StoreError> {
    Text::set_text_for(ui, ids::HERO_TITLE, ui::BASELINE_HERO_TITLE)?;
    RichText::set_text_for(ui, ids::HERO_BLURB, ui::BASELINE_HERO_BLURB)?;
    Text::set_role_for(ui, ids::TYPO_BODY, TextRole::Body)?;
    Button::set_label_for(ui, ids::BTN_PRIMARY, ui::BASELINE_PRIMARY_LABEL)?;
    Progress::set_value_for(ui, ids::PROGRESS_DETERMINATE, ui::BASELINE_PROGRESS_VALUE)?;
    Progress::set_value_description_for(
        ui,
        ids::PROGRESS_DETERMINATE,
        ui::BASELINE_PROGRESS_DESCRIPTION,
    )?;
    Ok(())
}

// =============================================================================
// State — interactivity and visibility flags
// =============================================================================

fn apply_state(ui: &mut UiTransaction) -> Result<(), StoreError> {
    Button::set_enabled_for(ui, ids::BTN_QUIET, false)?;
    TextInput::set_read_only_for(ui, ids::INPUT_PLAIN, true)?;
    TextInput::set_validation_state_for(ui, ids::INPUT_INVALID, ValidationState::Error)?;
    TextInput::set_value_description_for(
        ui,
        ids::INPUT_INVALID,
        "Rejected by the server: value fails validation",
    )?;
    Toggle::set_visibility_for(ui, ids::TOGGLE_SWITCH, Visibility::Hidden)?;
    Text::set_visibility_for(ui, ids::TYPO_ERROR, Visibility::Collapsed)?;
    Progress::set_busy_for(ui, ids::PROGRESS_BUSY, true)?;
    Progress::set_value_description_for(ui, ids::PROGRESS_BUSY, "busy")?;
    Ok(())
}

fn revert_state(ui: &mut UiTransaction) -> Result<(), StoreError> {
    Button::set_enabled_for(ui, ids::BTN_QUIET, true)?;
    TextInput::set_read_only_for(ui, ids::INPUT_PLAIN, false)?;
    TextInput::set_validation_state_for(ui, ids::INPUT_INVALID, ValidationState::Valid)?;
    TextInput::set_value_description_for(
        ui,
        ids::INPUT_INVALID,
        "Validation state is server-authoritative",
    )?;
    Toggle::set_visibility_for(ui, ids::TOGGLE_SWITCH, Visibility::Visible)?;
    Text::set_visibility_for(ui, ids::TYPO_ERROR, Visibility::Visible)?;
    Progress::set_busy_for(ui, ids::PROGRESS_BUSY, false)?;
    Progress::set_value_description_for(ui, ids::PROGRESS_BUSY, "idle")?;
    Ok(())
}

// =============================================================================
// Image resource — clear, fall back, restore from cache (§14)
// =============================================================================

fn apply_image_offline(ui: &mut UiTransaction) -> Result<(), StoreError> {
    Image::clear_resource_for(ui, ids::HERO_IMAGE)?;
    Image::set_visibility_for(ui, ids::HERO_IMAGE, Visibility::Collapsed)?;
    Text::set_visibility_for(ui, ids::HERO_IMAGE_PLACEHOLDER, Visibility::Visible)?;
    Text::set_text_for(
        ui,
        ids::HERO_STATUS,
        "Image resource cleared \u{b7} the bytes stay in the client cache",
    )?;
    Ok(())
}

fn revert_image_offline(
    ui: &mut UiTransaction,
    image: Option<ResourceHash>,
) -> Result<(), StoreError> {
    // Setting the same hash again costs no transfer: the client already holds the bytes in its
    // content-addressed cache, so this is a hash reference, not a re-send (§14, §19.2).
    if let Some(hash) = image {
        Image::set_resource_for(ui, ids::HERO_IMAGE, hash)?;
    }
    Image::set_visibility_for(ui, ids::HERO_IMAGE, Visibility::Visible)?;
    Text::set_visibility_for(ui, ids::HERO_IMAGE_PLACEHOLDER, Visibility::Collapsed)?;
    Text::set_text_for(ui, ids::HERO_STATUS, ui::BASELINE_HERO_STATUS)?;
    Ok(())
}

// =============================================================================
// Collection models (§8, §13)
// =============================================================================

fn list_item(id: ItemId, text: &str) -> ModelItem {
    ModelItem::new(id, Value::String(text.to_string()), [])
}

fn table_item(id: ItemId, cells: &[&str; 3]) -> ModelItem {
    ModelItem::new(id, ui::table_row(cells), [])
}

fn apply_models(ui: &mut UiTransaction) -> Result<(), StoreError> {
    ui.apply_op(&Operation::model_insert(
        ids::LIST_MODEL,
        1,
        [
            list_item(ids::LIST_SCENE_ITEMS[0], SCENE_LIST_INSERTED[0]),
            list_item(ids::LIST_SCENE_ITEMS[1], SCENE_LIST_INSERTED[1]),
        ],
    ))?;
    ui.apply_op(&Operation::model_update(
        ids::LIST_MODEL,
        None,
        [list_item(ids::LIST_ITEMS[1], SCENE_LIST_UPDATED)],
    ))?;
    ui.apply_op(&Operation::model_delete_items(
        ids::LIST_MODEL,
        [ids::LIST_ITEMS[3]],
    ))?;

    ui.apply_op(&Operation::model_insert(
        ids::TABLE_MODEL,
        ui::BASELINE_TABLE_ROWS.len() as u64,
        [table_item(ids::TABLE_SCENE_ITEM, &SCENE_TABLE_INSERTED)],
    ))?;
    ui.apply_op(&Operation::model_update(
        ids::TABLE_MODEL,
        None,
        [table_item(ids::TABLE_ITEMS[1], &SCENE_TABLE_UPDATED)],
    ))?;
    ui.apply_op(&Operation::model_delete_items(
        ids::TABLE_MODEL,
        [ids::TABLE_ITEMS[3]],
    ))?;
    Ok(())
}

fn revert_models(ui: &mut UiTransaction) -> Result<(), StoreError> {
    ui.apply_op(&Operation::model_delete_items(
        ids::LIST_MODEL,
        ids::LIST_SCENE_ITEMS,
    ))?;
    ui.apply_op(&Operation::model_update(
        ids::LIST_MODEL,
        None,
        [list_item(ids::LIST_ITEMS[1], ui::BASELINE_LIST_ROWS[1].1)],
    ))?;
    ui.apply_op(&Operation::model_insert(
        ids::LIST_MODEL,
        3,
        [list_item(ids::LIST_ITEMS[3], ui::BASELINE_LIST_ROWS[3].1)],
    ))?;

    ui.apply_op(&Operation::model_delete_items(
        ids::TABLE_MODEL,
        [ids::TABLE_SCENE_ITEM],
    ))?;
    ui.apply_op(&Operation::model_update(
        ids::TABLE_MODEL,
        None,
        [table_item(
            ids::TABLE_ITEMS[1],
            &ui::BASELINE_TABLE_ROWS[1].1,
        )],
    ))?;
    ui.apply_op(&Operation::model_insert(
        ids::TABLE_MODEL,
        3,
        [table_item(
            ids::TABLE_ITEMS[3],
            &ui::BASELINE_TABLE_ROWS[3].1,
        )],
    ))?;
    Ok(())
}

// =============================================================================
// Structure (§13 CREATE_NODE / MOVE_NODE / REORDER_CHILDREN / DELETE_NODE)
// =============================================================================

fn apply_structure(ui: &mut UiTransaction, ctx: &mut SceneContext) -> Result<(), StoreError> {
    // A fresh id each time: the previous badge was deleted, and the store refuses to resurrect a
    // retired node id (§6.2).
    let badge = ctx.allocate()?;
    Text::builder(badge)
        .parent(ids::LAYOUT_COLUMN)
        .text(SCENE_BADGE_TEXT)
        .role(TextRole::Status)
        .create(ui)?;
    ui.move_node(badge, Some(ids::CTRL_COLUMN), Some(1))?;

    let reversed: Vec<NodeId> = ids::CTRL_BUTTON_ORDER.iter().rev().copied().collect();
    ui.reorder_children(ids::CTRL_BUTTON_ROW, &reversed)?;
    Ok(())
}

fn revert_structure(ui: &mut UiTransaction, ctx: &mut SceneContext) -> Result<(), StoreError> {
    ui.reorder_children(ids::CTRL_BUTTON_ROW, &ids::CTRL_BUTTON_ORDER)?;
    for node in ctx.take_transient() {
        ui.delete(node)?;
    }
    Ok(())
}

// =============================================================================
// Layout
// =============================================================================

fn apply_layout(ui: &mut UiTransaction) -> Result<(), StoreError> {
    Row::set_spacing_role_for(ui, ids::LAYOUT_ROW, SpacingRole::Relaxed)?;
    Column::set_padding_role_for(ui, ids::LAYOUT_COLUMN, PaddingRole::Relaxed)?;
    Grid::set_spacing_role_for(ui, ids::LAYOUT_GRID, SpacingRole::Relaxed)?;
    Text::set_horizontal_alignment_for(
        ui,
        ids::LAYOUT_ALIGN_LEADING,
        HorizontalAlignment::Trailing,
    )?;
    Text::set_preferred_size_for(ui, ids::LAYOUT_SIZE_PREFERRED, SCENE_SIZE_PREFERRED)?;
    Ok(())
}

fn revert_layout(ui: &mut UiTransaction) -> Result<(), StoreError> {
    Row::set_spacing_role_for(ui, ids::LAYOUT_ROW, SpacingRole::Normal)?;
    Column::set_padding_role_for(ui, ids::LAYOUT_COLUMN, PaddingRole::Normal)?;
    Grid::set_spacing_role_for(ui, ids::LAYOUT_GRID, SpacingRole::Tight)?;
    Text::set_horizontal_alignment_for(
        ui,
        ids::LAYOUT_ALIGN_LEADING,
        HorizontalAlignment::Leading,
    )?;
    Text::set_preferred_size_for(ui, ids::LAYOUT_SIZE_PREFERRED, ui::BASELINE_SIZE_PREFERRED)?;
    Ok(())
}
