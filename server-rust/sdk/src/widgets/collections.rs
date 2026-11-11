//! Model-backed collection widget handles and builders, including server-promoted selection (§6.3, §7.2, §7.3 Required Tier, §8).
//!
//! `Tree` reads `columns` like `Table` (the outline column first) and reports local disclosure
//! through [`Tree::on_expansion_changed`] (§7.6, §22.5); its model is built by
//! [`crate::TreeModel`].

use crate::widgets::*;
use crate::StoreMut;
use srui_semantic_tree::EnumToken;

// =============================================================================
// 1. List (§7.2, §7.3 Required Tier, TypeRef::LIST / id 17)
// =============================================================================

impl_widget_boilerplate!(
    List,
    ListBuilder,
    TypeRef::LIST,
    "Virtualized one-dimensional collection of items (§7.2, §7.3 Required Tier, §8)."
);
impl_model_ref_prop!(List, ListBuilder);
impl_list_prop!(
    List,
    ListBuilder,
    items,
    items_of,
    get_items,
    set_items,
    set_items_for,
    op_set_items,
    op_set_items_for,
    clear_items,
    clear_items_for,
    op_clear_items,
    op_clear_items_for,
    PropertyRef::ITEMS,
    "Inline item list for small un-virtualized collections (§7.4)."
);
impl_enum_prop!(
    List,
    ListBuilder,
    SelectionMode,
    selection_mode,
    selection_mode_of,
    get_selection_mode,
    set_selection_mode,
    set_selection_mode_for,
    op_set_selection_mode,
    op_set_selection_mode_for,
    clear_selection_mode,
    clear_selection_mode_for,
    op_clear_selection_mode,
    op_clear_selection_mode_for,
    PropertyRef::SELECTION_MODE,
    "Selection mode (none | single | multiple) (§8)."
);
impl_bool_prop!(
    List,
    ListBuilder,
    selected,
    selected_of,
    get_selected,
    is_selected,
    is_selected_of,
    is_selected_for,
    set_selected,
    set_selected_for,
    op_set_selected,
    op_set_selected_for,
    clear_selected,
    clear_selected_for,
    op_clear_selected,
    op_clear_selected_for,
    PropertyRef::SELECTED,
    "Selection state (§7.4)."
);
impl_string_prop!(
    List,
    ListBuilder,
    label,
    label_of,
    get_label,
    set_label,
    set_label_for,
    op_set_label,
    op_set_label_for,
    clear_label,
    clear_label_for,
    op_clear_label,
    op_clear_label_for,
    PropertyRef::LABEL,
    "Collection label or title (§7.4)."
);
impl_string_prop!(
    List,
    ListBuilder,
    accessible_description,
    accessible_description_of,
    get_accessible_description,
    set_accessible_description,
    set_accessible_description_for,
    op_set_accessible_description,
    op_set_accessible_description_for,
    clear_accessible_description,
    clear_accessible_description_for,
    op_clear_accessible_description,
    op_clear_accessible_description_for,
    PropertyRef::ACCESSIBLE_DESCRIPTION,
    "Secondary accessibility description (§7.4)."
);
impl_shortcut_prop!(
    List,
    ListBuilder,
    "Keyboard shortcut moving local focus to the list; no event is sent (§22.5)."
);
impl_common_state_props!(List, ListBuilder);
impl_common_layout_props!(List, ListBuilder);

// =============================================================================
// 2. Table (§7.2, §7.3 Required Tier, TypeRef::TABLE / id 18)
// =============================================================================

impl_widget_boilerplate!(
    Table,
    TableBuilder,
    TypeRef::TABLE,
    "Multi-column row-based collection view (§7.2, §7.3 Required Tier, §8)."
);
impl_model_ref_prop!(Table, TableBuilder);
impl_list_prop!(
    Table,
    TableBuilder,
    columns,
    columns_of,
    get_columns,
    set_columns,
    set_columns_for,
    op_set_columns,
    op_set_columns_for,
    clear_columns,
    clear_columns_for,
    op_clear_columns,
    op_clear_columns_for,
    PropertyRef::COLUMNS,
    "Column definitions list (§7.4, §8)."
);
impl_enum_prop!(
    Table,
    TableBuilder,
    SelectionMode,
    selection_mode,
    selection_mode_of,
    get_selection_mode,
    set_selection_mode,
    set_selection_mode_for,
    op_set_selection_mode,
    op_set_selection_mode_for,
    clear_selection_mode,
    clear_selection_mode_for,
    op_clear_selection_mode,
    op_clear_selection_mode_for,
    PropertyRef::SELECTION_MODE,
    "Selection mode (§8)."
);
impl_bool_prop!(
    Table,
    TableBuilder,
    selected,
    selected_of,
    get_selected,
    is_selected,
    is_selected_of,
    is_selected_for,
    set_selected,
    set_selected_for,
    op_set_selected,
    op_set_selected_for,
    clear_selected,
    clear_selected_for,
    op_clear_selected,
    op_clear_selected_for,
    PropertyRef::SELECTED,
    "Selection state (§7.4)."
);
impl_string_prop!(
    Table,
    TableBuilder,
    label,
    label_of,
    get_label,
    set_label,
    set_label_for,
    op_set_label,
    op_set_label_for,
    clear_label,
    clear_label_for,
    op_clear_label,
    op_clear_label_for,
    PropertyRef::LABEL,
    "Table accessibility label (§7.4)."
);
impl_string_prop!(
    Table,
    TableBuilder,
    accessible_description,
    accessible_description_of,
    get_accessible_description,
    set_accessible_description,
    set_accessible_description_for,
    op_set_accessible_description,
    op_set_accessible_description_for,
    clear_accessible_description,
    clear_accessible_description_for,
    op_clear_accessible_description,
    op_clear_accessible_description_for,
    PropertyRef::ACCESSIBLE_DESCRIPTION,
    "Secondary accessibility description (§7.4)."
);
impl_shortcut_prop!(
    Table,
    TableBuilder,
    "Keyboard shortcut moving local focus to the table; no event is sent (§22.5)."
);
impl_common_state_props!(Table, TableBuilder);
impl_common_layout_props!(Table, TableBuilder);

// =============================================================================
// 3. Tree (§7.2, §7.3 Required Tier, TypeRef::TREE / id 19)
// =============================================================================

impl_widget_boilerplate!(
    Tree,
    TreeBuilder,
    TypeRef::TREE,
    "Hierarchical outline collection view with expandable nodes (§7.2, §7.3 Required Tier, §8)."
);
impl_model_ref_prop!(Tree, TreeBuilder);
impl_list_prop!(
    Tree,
    TreeBuilder,
    columns,
    columns_of,
    get_columns,
    set_columns,
    set_columns_for,
    op_set_columns,
    op_set_columns_for,
    clear_columns,
    clear_columns_for,
    op_clear_columns,
    op_clear_columns_for,
    PropertyRef::COLUMNS,
    "Column definitions list, projected like a Table's with the outline column first (§7.4, §8)."
);
impl_enum_prop!(
    Tree,
    TreeBuilder,
    SelectionMode,
    selection_mode,
    selection_mode_of,
    get_selection_mode,
    set_selection_mode,
    set_selection_mode_for,
    op_set_selection_mode,
    op_set_selection_mode_for,
    clear_selection_mode,
    clear_selection_mode_for,
    op_clear_selection_mode,
    op_clear_selection_mode_for,
    PropertyRef::SELECTION_MODE,
    "Selection mode (§8)."
);
impl_bool_prop!(
    Tree,
    TreeBuilder,
    selected,
    selected_of,
    get_selected,
    is_selected,
    is_selected_of,
    is_selected_for,
    set_selected,
    set_selected_for,
    op_set_selected,
    op_set_selected_for,
    clear_selected,
    clear_selected_for,
    op_clear_selected,
    op_clear_selected_for,
    PropertyRef::SELECTED,
    "Selection state (§7.4)."
);
impl_string_prop!(
    Tree,
    TreeBuilder,
    label,
    label_of,
    get_label,
    set_label,
    set_label_for,
    op_set_label,
    op_set_label_for,
    clear_label,
    clear_label_for,
    op_clear_label,
    op_clear_label_for,
    PropertyRef::LABEL,
    "Tree accessibility label (§7.4)."
);
impl_string_prop!(
    Tree,
    TreeBuilder,
    accessible_description,
    accessible_description_of,
    get_accessible_description,
    set_accessible_description,
    set_accessible_description_for,
    op_set_accessible_description,
    op_set_accessible_description_for,
    clear_accessible_description,
    clear_accessible_description_for,
    op_clear_accessible_description,
    op_clear_accessible_description_for,
    PropertyRef::ACCESSIBLE_DESCRIPTION,
    "Secondary accessibility description (§7.4)."
);
impl_shortcut_prop!(
    Tree,
    TreeBuilder,
    "Keyboard shortcut moving local focus to the tree; no event is sent (§22.5)."
);
impl_common_state_props!(Tree, TreeBuilder);
impl_common_layout_props!(Tree, TreeBuilder);

// =============================================================================
// 4. Server-promoted selection (§6.3, §7.6, §8; see `crate::selection`)
// =============================================================================

macro_rules! impl_promoted_selection {
    ($widget:ident) => {
        impl $widget {
            /// Promotes `item` to this collection's semantic selection (§6.3, §8).
            ///
            /// See [`crate::UiTransaction::promote_selection`] for the recorded operations and
            /// the explicit errors; an unknown `ItemId` never falls back to a row index.
            pub fn promote_selection(
                &self,
                ui: &mut crate::UiTransaction,
                item: impl Into<ItemId>,
            ) -> Result<(), StoreError> {
                ui.promote_selection(self.id(), item)
            }

            /// Withdraws this collection's promoted selection, returning the item it named.
            pub fn clear_promoted_selection(
                &self,
                ui: &mut crate::UiTransaction,
            ) -> Result<Option<ItemId>, StoreError> {
                ui.clear_promoted_selection(self.id())
            }

            /// Returns this collection's promoted selection, if any (§6.3).
            pub fn promoted_selection(&self, store: &SemanticStore) -> Option<ItemId> {
                store.get_node(self.id())?.promoted_selection()
            }

            /// Hints that the client keep `item` in view, never touching selection (§6.3, §8).
            ///
            /// See [`crate::UiTransaction::set_keep_visible`] for the recorded operations and
            /// the explicit errors.
            pub fn set_keep_visible(
                &self,
                ui: &mut crate::UiTransaction,
                item: impl Into<ItemId>,
            ) -> Result<(), StoreError> {
                ui.set_keep_visible(self.id(), item)
            }

            /// Clears this collection's `keep_visible` hint, returning the item it named.
            pub fn clear_keep_visible(
                &self,
                ui: &mut crate::UiTransaction,
            ) -> Result<Option<ItemId>, StoreError> {
                ui.clear_keep_visible(self.id())
            }

            /// Returns this collection's `keep_visible` hint, if any (§6.3).
            pub fn keep_visible(&self, store: &SemanticStore) -> Option<ItemId> {
                store.get_node(self.id())?.keep_visible()
            }
        }
    };
}

impl_promoted_selection!(List);
impl_promoted_selection!(Table);
impl_promoted_selection!(Tree);

// =============================================================================
// 5. Local disclosure reports (§7.6, §22.5)
// =============================================================================

/// One `EXPANSION_CHANGED` report from a `Tree` (§7.6).
///
/// Disclosure is local presentation state (§22.5): the client has already expanded or collapsed
/// the item and never waits for a reply. The report names the item by identity, never by row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExpansionChange {
    /// The `Tree` node that reported it.
    pub node: NodeId,
    /// The item whose disclosure changed.
    pub item: ItemId,
    /// `true` when the item is now expanded.
    pub expanded: bool,
    /// The revision the client observed when the user acted.
    pub observed_revision: srui_semantic_tree::Revision,
}

impl ExpansionChange {
    /// Reads an `EXPANSION_CHANGED` event; `None` for any other or a malformed event.
    pub fn from_event(event: &srui_semantic_tree::Event) -> Option<Self> {
        let (item, expanded) = event.expansion_change()?;
        Some(Self {
            node: event.node_id,
            item,
            expanded,
            observed_revision: event.observed_revision,
        })
    }
}

impl Tree {
    /// Registers `handler` for this tree's `EXPANSION_CHANGED` reports (§7.6, §22.5).
    ///
    /// A malformed report (no boolean `value`, or `items` not exactly one `ItemId`) never reaches
    /// the handler. The handler runs outside the session lock, like every [`crate::Session::on`]
    /// handler, and may open transactions.
    pub fn on_expansion_changed<F>(&self, session: &crate::Session, handler: F)
    where
        F: Fn(&crate::Session, ExpansionChange) + Send + Sync + 'static,
    {
        session.on(
            self.id(),
            TypeRef::EVENT_EXPANSION_CHANGED,
            move |ctx, event| {
                if let Some(change) = ExpansionChange::from_event(event) {
                    handler(ctx, change);
                }
            },
        );
    }
}
