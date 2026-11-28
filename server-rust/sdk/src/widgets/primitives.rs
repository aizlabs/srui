//! Layout and container widget handles and builders (§7.2, §7.3 Required Tier), plus the
//! SHOULD-tier [`Split`] container (§7.3, §10, §22.4).

use crate::widgets::*;
use crate::StoreMut;
use srui_semantic_tree::EnumToken;

// =============================================================================
// 1. Surface (§7.2, §7.3 Required Tier, TypeRef::SURFACE / id 1)
// =============================================================================

impl_widget_boilerplate!(
    Surface,
    SurfaceBuilder,
    TypeRef::SURFACE,
    "Top-level window or surface content root (§7.2, §7.3 Required Tier)."
);
impl_string_prop!(
    Surface,
    SurfaceBuilder,
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
    "Window title or surface accessibility label."
);
impl_string_prop!(
    Surface,
    SurfaceBuilder,
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
    "Secondary accessibility description."
);
impl_common_state_props!(Surface, SurfaceBuilder);
impl_container_spacing_props!(Surface, SurfaceBuilder);
impl_common_layout_props!(Surface, SurfaceBuilder);

// =============================================================================
// 2. Row (§7.2, §7.3 Required Tier, TypeRef::ROW / id 3)
// =============================================================================

impl_widget_boilerplate!(
    Row,
    RowBuilder,
    TypeRef::ROW,
    "Ordered horizontal layout container (§7.2, §7.3 Required Tier)."
);
impl_container_spacing_props!(Row, RowBuilder);
impl_common_state_props!(Row, RowBuilder);
impl_common_layout_props!(Row, RowBuilder);
impl_string_prop!(
    Row,
    RowBuilder,
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
    "Secondary accessibility description."
);

// =============================================================================
// 3. Column (§7.2, §7.3 Required Tier, TypeRef::COLUMN / id 4)
// =============================================================================

impl_widget_boilerplate!(
    Column,
    ColumnBuilder,
    TypeRef::COLUMN,
    "Ordered vertical layout container (§7.2, §7.3 Required Tier)."
);
impl_container_spacing_props!(Column, ColumnBuilder);
impl_common_state_props!(Column, ColumnBuilder);
impl_common_layout_props!(Column, ColumnBuilder);
impl_string_prop!(
    Column,
    ColumnBuilder,
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
    "Secondary accessibility description."
);

// =============================================================================
// 4. Grid (§7.2, §7.3 Required Tier, TypeRef::GRID / id 5)
// =============================================================================

impl_widget_boilerplate!(
    Grid,
    GridBuilder,
    TypeRef::GRID,
    "Two-dimensional row/column layout container (§7.2, §7.3 Required Tier)."
);
impl_container_spacing_props!(Grid, GridBuilder);
impl_common_state_props!(Grid, GridBuilder);
impl_common_layout_props!(Grid, GridBuilder);
impl_list_prop!(
    Grid,
    GridBuilder,
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
    "Grid column specifications or metadata (§7.4)."
);
impl_string_prop!(
    Grid,
    GridBuilder,
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
    "Secondary accessibility description."
);

// =============================================================================
// 5. Spacer (§7.2, §7.3 Required Tier, TypeRef::SPACER / id 6)
// =============================================================================

impl_widget_boilerplate!(
    Spacer,
    SpacerBuilder,
    TypeRef::SPACER,
    "Flexible empty layout item for spacing and alignment (§7.2, §7.3 Required Tier)."
);
impl_common_layout_props!(Spacer, SpacerBuilder);
impl_enum_prop!(
    Spacer,
    SpacerBuilder,
    Visibility,
    visibility,
    visibility_of,
    get_visibility,
    set_visibility,
    set_visibility_for,
    op_set_visibility,
    op_set_visibility_for,
    clear_visibility,
    clear_visibility_for,
    op_clear_visibility,
    op_clear_visibility_for,
    PropertyRef::VISIBILITY,
    "Visibility and layout participation (§7.4)."
);

// =============================================================================
// 6. Separator (§7.2, §7.3 Required Tier, TypeRef::SEPARATOR / id 7)
// =============================================================================

impl_widget_boilerplate!(
    Separator,
    SeparatorBuilder,
    TypeRef::SEPARATOR,
    "Semantic visual grouping line or divider (§7.2, §7.3 Required Tier)."
);
impl_common_layout_props!(Separator, SeparatorBuilder);
impl_common_state_props!(Separator, SeparatorBuilder);
impl_string_prop!(
    Separator,
    SeparatorBuilder,
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
    "Secondary accessibility description."
);

// =============================================================================
// 7. Scroll (§7.2, §7.3 Required Tier, TypeRef::SCROLL / id 8)
// =============================================================================

impl_widget_boilerplate!(
    Scroll,
    ScrollBuilder,
    TypeRef::SCROLL,
    "Scrollable viewport container (§7.2, §7.3 Required Tier)."
);
impl_container_spacing_props!(Scroll, ScrollBuilder);
impl_common_state_props!(Scroll, ScrollBuilder);
impl_common_layout_props!(Scroll, ScrollBuilder);
impl_string_prop!(
    Scroll,
    ScrollBuilder,
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

// =============================================================================
// 8. Split (§7.2, §7.3 SHOULD Tier, TypeRef::SPLIT / id 25)
// =============================================================================

impl_widget_boilerplate!(
    Split,
    SplitBuilder,
    TypeRef::SPLIT,
    "User-resizable pane division container (§7.2, §7.3 SHOULD Tier, §10, §22.4).\n\n\
     Each child is one pane, in child order, at most [`Split::MAX_PANES`]: create a pane with \
     any widget builder's `parent(split_id)`. `axis` is the standard `split_axis` property \
     (absent means [`SplitAxis::SideBySide`]). Pane sizes come from each pane's own layout \
     hints (`minimum_size`, `maximum_size`, `preferred_size`, `grow`), never from pixel \
     geometry; a pane whose `visibility` is `collapsed` collapses. Divider positions are local \
     to the client and never reach the server (§7.6, §22.5), so this SDK offers no divider \
     position and no `VALUE_CHANGED` handler for a Split."
);
impl_enum_prop!(
    Split,
    SplitBuilder,
    SplitAxis,
    axis,
    axis_of,
    get_axis,
    set_axis,
    set_axis_for,
    op_set_axis,
    op_set_axis_for,
    clear_axis,
    clear_axis_for,
    op_clear_axis,
    op_clear_axis_for,
    PropertyRef::SPLIT_AXIS,
    "Pane axis (`split_axis`): side by side or stacked; absent means side by side (§10, §22.4)."
);
impl_string_prop!(
    Split,
    SplitBuilder,
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
    "Accessibility label of the split group (§22.8)."
);
impl_string_prop!(
    Split,
    SplitBuilder,
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
impl_common_state_props!(Split, SplitBuilder);
impl_common_layout_props!(Split, SplitBuilder);

impl Split {
    /// Most panes one Split holds; a ninth child is refused (§26).
    pub const MAX_PANES: usize = srui_semantic_tree::MAX_SPLIT_PANES;

    /// The panes (children) of this Split in `store`, in pane order. Empty if the node is absent.
    pub fn panes<'a>(&self, store: &'a SemanticStore) -> &'a [NodeId] {
        store.children_of(self.id).unwrap_or(&[])
    }

    /// The axis the panes are laid out on: `axis`, or [`SplitAxis::SideBySide`] when absent.
    pub fn effective_axis(&self, store: &SemanticStore) -> SplitAxis {
        self.axis(store).unwrap_or(SplitAxis::SideBySide)
    }
}
