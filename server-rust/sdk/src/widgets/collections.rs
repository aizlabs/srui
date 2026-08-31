//! Model-backed collection widget handles and builders (§7.2, §7.3 Required Tier, §8).

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
impl_common_state_props!(Tree, TreeBuilder);
impl_common_layout_props!(Tree, TreeBuilder);
