//! Single-line and multi-line text input control widget handles and builders (§7.2, §7.3 Required Tier, §22.6).

use crate::widgets::*;
use crate::StoreMut;
use srui_semantic_tree::EnumToken;

// =============================================================================
// 1. TextInput (§7.2, §7.3 Required Tier, TypeRef::TEXT_INPUT / id 13)
// =============================================================================

impl_widget_boilerplate!(
    TextInput,
    TextInputBuilder,
    TypeRef::TEXT_INPUT,
    "Single-line text editing field (§7.2, §7.3 Required Tier, §22.6)."
);
impl_string_prop!(
    TextInput,
    TextInputBuilder,
    value,
    value_of,
    get_value,
    set_value,
    set_value_for,
    op_set_value,
    op_set_value_for,
    clear_value,
    clear_value_for,
    op_clear_value,
    op_clear_value_for,
    PropertyRef::VALUE,
    "Current text input value (§7.4)."
);
impl_string_prop!(
    TextInput,
    TextInputBuilder,
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
    "Text content alias (§7.4)."
);
impl_string_prop!(
    TextInput,
    TextInputBuilder,
    placeholder,
    placeholder_of,
    get_placeholder,
    set_placeholder,
    set_placeholder_for,
    op_set_placeholder,
    op_set_placeholder_for,
    clear_placeholder,
    clear_placeholder_for,
    op_clear_placeholder,
    op_clear_placeholder_for,
    PropertyRef::PLACEHOLDER,
    "Placeholder text (§7.4)."
);
impl_enum_prop!(
    TextInput,
    TextInputBuilder,
    InputRole,
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
    "Semantic input role (plain | search | secure | command) (§7.5)."
);
impl_enum_prop!(
    TextInput,
    TextInputBuilder,
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
    TextInput,
    TextInputBuilder,
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
    "Input field label (§7.4)."
);
impl_string_prop!(
    TextInput,
    TextInputBuilder,
    action_key,
    action_key_of,
    get_action_key,
    set_action_key,
    set_action_key_for,
    op_set_action_key,
    op_set_action_key_for,
    clear_action_key,
    clear_action_key_for,
    op_clear_action_key,
    op_clear_action_key_for,
    PropertyRef::ACTION_KEY,
    "Opaque action key (§7.7)."
);
impl_string_prop!(
    TextInput,
    TextInputBuilder,
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
    TextInput,
    TextInputBuilder,
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
impl_enum_prop!(
    TextInput,
    TextInputBuilder,
    ValidationState,
    validation_state,
    validation_state_of,
    get_validation_state,
    set_validation_state,
    set_validation_state_for,
    op_set_validation_state,
    op_set_validation_state_for,
    clear_validation_state,
    clear_validation_state_for,
    op_clear_validation_state,
    op_clear_validation_state_for,
    PropertyRef::VALIDATION_STATE,
    "Validation state (§7.4)."
);
impl_bool_prop!(
    TextInput,
    TextInputBuilder,
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
    "Whether the input is read-only (§7.4)."
);
impl_common_state_props!(TextInput, TextInputBuilder);
impl_common_layout_props!(TextInput, TextInputBuilder);

// =============================================================================
// 2. TextArea (§7.2, §7.3 Required Tier, TypeRef::TEXT_AREA / id 14)
// =============================================================================

impl_widget_boilerplate!(
    TextArea,
    TextAreaBuilder,
    TypeRef::TEXT_AREA,
    "Multi-line text editing view (§7.2, §7.3 Required Tier, §22.6)."
);
impl_string_prop!(
    TextArea,
    TextAreaBuilder,
    value,
    value_of,
    get_value,
    set_value,
    set_value_for,
    op_set_value,
    op_set_value_for,
    clear_value,
    clear_value_for,
    op_clear_value,
    op_clear_value_for,
    PropertyRef::VALUE,
    "Current text area value (§7.4)."
);
impl_string_prop!(
    TextArea,
    TextAreaBuilder,
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
    "Text content alias (§7.4)."
);
impl_string_prop!(
    TextArea,
    TextAreaBuilder,
    placeholder,
    placeholder_of,
    get_placeholder,
    set_placeholder,
    set_placeholder_for,
    op_set_placeholder,
    op_set_placeholder_for,
    clear_placeholder,
    clear_placeholder_for,
    op_clear_placeholder,
    op_clear_placeholder_for,
    PropertyRef::PLACEHOLDER,
    "Placeholder text (§7.4)."
);
impl_enum_prop!(
    TextArea,
    TextAreaBuilder,
    InputRole,
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
    "Semantic input role (§7.5)."
);
impl_enum_prop!(
    TextArea,
    TextAreaBuilder,
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
    TextArea,
    TextAreaBuilder,
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
    "Text area label (§7.4)."
);
impl_string_prop!(
    TextArea,
    TextAreaBuilder,
    action_key,
    action_key_of,
    get_action_key,
    set_action_key,
    set_action_key_for,
    op_set_action_key,
    op_set_action_key_for,
    clear_action_key,
    clear_action_key_for,
    op_clear_action_key,
    op_clear_action_key_for,
    PropertyRef::ACTION_KEY,
    "Opaque action key (§7.7)."
);
impl_string_prop!(
    TextArea,
    TextAreaBuilder,
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
    TextArea,
    TextAreaBuilder,
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
impl_enum_prop!(
    TextArea,
    TextAreaBuilder,
    ValidationState,
    validation_state,
    validation_state_of,
    get_validation_state,
    set_validation_state,
    set_validation_state_for,
    op_set_validation_state,
    op_set_validation_state_for,
    clear_validation_state,
    clear_validation_state_for,
    op_clear_validation_state,
    op_clear_validation_state_for,
    PropertyRef::VALIDATION_STATE,
    "Validation state (§7.4)."
);
impl_bool_prop!(
    TextArea,
    TextAreaBuilder,
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
    "Whether the text area is read-only (§7.4)."
);
impl_common_state_props!(TextArea, TextAreaBuilder);
impl_common_layout_props!(TextArea, TextAreaBuilder);
