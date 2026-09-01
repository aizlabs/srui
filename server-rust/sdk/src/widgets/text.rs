//! Text and formatted text control widget handles and builders (§7.2, §7.3 Required Tier).

use crate::widgets::*;
use crate::StoreMut;
use srui_semantic_tree::EnumToken;

// =============================================================================
// 1. Text (§7.2, §7.3 Required Tier, TypeRef::TEXT / id 9)
// =============================================================================

impl_widget_boilerplate!(
    Text,
    TextBuilder,
    TypeRef::TEXT,
    "Non-editable static or dynamic text label (§7.2, §7.3 Required Tier)."
);
impl_string_prop!(
    Text,
    TextBuilder,
    text,
    text_of,
    get_text,
    set_text,
    set_text_for,
    op_set_text,
    op_set_text_for,
    clear_text,
    clear_text_for,
    op_clear_text,
    op_clear_text_for,
    PropertyRef::TEXT,
    "Primary text content (§7.4)."
);
impl_enum_prop!(
    Text,
    TextBuilder,
    TextRole,
    role,
    role_of,
    get_role,
    set_role,
    set_role_for,
    op_set_role,
    op_set_role_for,
    clear_role,
    clear_role_for,
    op_clear_role,
    op_clear_role_for,
    PropertyRef::ROLE,
    "Semantic text role (§7.5)."
);
impl_enum_prop!(
    Text,
    TextBuilder,
    Importance,
    importance,
    importance_of,
    get_importance,
    set_importance,
    set_importance_for,
    op_set_importance,
    op_set_importance_for,
    clear_importance,
    clear_importance_for,
    op_clear_importance,
    op_clear_importance_for,
    PropertyRef::ROLE,
    "Semantic emphasis level (§7.5)."
);
impl_string_prop!(
    Text,
    TextBuilder,
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
    "Primary label or accessibility name (§7.4)."
);
impl_string_prop!(
    Text,
    TextBuilder,
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
impl_string_prop!(
    Text,
    TextBuilder,
    value_description,
    value_description_of,
    get_value_description,
    set_value_description,
    set_value_description_for,
    op_set_value_description,
    op_set_value_description_for,
    clear_value_description,
    clear_value_description_for,
    op_clear_value_description,
    op_clear_value_description_for,
    PropertyRef::VALUE_DESCRIPTION,
    "Human-readable value description (§7.4)."
);
impl_common_state_props!(Text, TextBuilder);
impl_common_layout_props!(Text, TextBuilder);

// =============================================================================
// 2. RichText (§7.2, §7.3 Required Tier, TypeRef::RICHTEXT / id 10)
// =============================================================================

impl_widget_boilerplate!(
    RichText,
    RichTextBuilder,
    TypeRef::RICHTEXT,
    "Selectable structured text with semantic annotations (§7.2, §7.3 Required Tier, §9)."
);
impl_string_prop!(
    RichText,
    RichTextBuilder,
    text,
    text_of,
    get_text,
    set_text,
    set_text_for,
    op_set_text,
    op_set_text_for,
    clear_text,
    clear_text_for,
    op_clear_text,
    op_clear_text_for,
    PropertyRef::TEXT,
    "Primary structured text content (§7.4, §9)."
);
impl_enum_prop!(
    RichText,
    RichTextBuilder,
    TextRole,
    role,
    role_of,
    get_role,
    set_role,
    set_role_for,
    op_set_role,
    op_set_role_for,
    clear_role,
    clear_role_for,
    op_clear_role,
    op_clear_role_for,
    PropertyRef::ROLE,
    "Semantic text role (§7.5)."
);
impl_enum_prop!(
    RichText,
    RichTextBuilder,
    Importance,
    importance,
    importance_of,
    get_importance,
    set_importance,
    set_importance_for,
    op_set_importance,
    op_set_importance_for,
    clear_importance,
    clear_importance_for,
    op_clear_importance,
    op_clear_importance_for,
    PropertyRef::ROLE,
    "Semantic emphasis level (§7.5)."
);
impl_string_prop!(
    RichText,
    RichTextBuilder,
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
    "Accessibility label (§7.4)."
);
impl_string_prop!(
    RichText,
    RichTextBuilder,
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
impl_bool_prop!(
    RichText,
    RichTextBuilder,
    read_only,
    read_only_of,
    get_read_only,
    is_read_only,
    is_read_only_of,
    is_read_only_for,
    set_read_only,
    set_read_only_for,
    op_set_read_only,
    op_set_read_only_for,
    clear_read_only,
    clear_read_only_for,
    op_clear_read_only,
    op_clear_read_only_for,
    PropertyRef::READ_ONLY,
    "Whether the text is read-only (§7.4)."
);
impl_common_state_props!(RichText, RichTextBuilder);
impl_common_layout_props!(RichText, RichTextBuilder);
