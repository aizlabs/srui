//! Layout and container widget handles and builders (§7.2, §7.3 Required Tier).

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
