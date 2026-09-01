//! Button and toggle control widget handles and builders (§7.2, §7.3 Required Tier).

use crate::widgets::*;
use crate::StoreMut;
use srui_semantic_tree::EnumToken;

// =============================================================================
// 1. Button (§7.2, §7.3 Required Tier, TypeRef::BUTTON / id 11)
// =============================================================================

impl_widget_boilerplate!(
    Button,
    ButtonBuilder,
    TypeRef::BUTTON,
    "Momentary action trigger (§7.2, §7.3 Required Tier)."
);
impl_string_prop!(
    Button,
    ButtonBuilder,
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
    "Button label text (§7.4)."
);
impl_enum_prop!(
    Button,
    ButtonBuilder,
    ActionRole,
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
    "Semantic action role (§7.5)."
);
impl_enum_prop!(
    Button,
    ButtonBuilder,
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
    Button,
    ButtonBuilder,
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
    "Opaque semantic action key (§7.7)."
);
impl_list_prop!(
    Button,
    ButtonBuilder,
    actions,
    actions_of,
    get_actions,
    set_actions,
    set_actions_for,
    op_set_actions,
    op_set_actions_for,
    clear_actions,
    clear_actions_for,
    op_clear_actions,
    op_clear_actions_for,
    PropertyRef::ACTIONS,
    "List of supported semantic actions (§7.4)."
);
impl_string_prop!(
    Button,
    ButtonBuilder,
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
    Button,
    ButtonBuilder,
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
    "Selected state (§7.4)."
);
impl_common_state_props!(Button, ButtonBuilder);
impl_common_layout_props!(Button, ButtonBuilder);

// =============================================================================
// 2. Toggle (§7.2, §7.3 Required Tier, TypeRef::TOGGLE / id 12)
// =============================================================================

impl_widget_boilerplate!(
    Toggle,
    ToggleBuilder,
    TypeRef::TOGGLE,
    "User-editable boolean state control (§7.2, §7.3 Required Tier).\n\n\
     Per §7.2, checkbox and switch share the exact same semantic state machine (`TypeRef::TOGGLE`)\n\
     with `value: bool` and an advisory `presentation_hint` enum."
);

impl Toggle {
    /// Convenience constructor creating a [`ToggleBuilder`] with [`TogglePresentationHint::Checkbox`].
    #[inline]
    pub fn checkbox(id: impl Into<NodeId>) -> ToggleBuilder {
        ToggleBuilder::new(id).presentation_hint(TogglePresentationHint::Checkbox)
    }

    /// Convenience constructor creating a [`ToggleBuilder`] with [`TogglePresentationHint::Switch`].
    #[inline]
    pub fn switch(id: impl Into<NodeId>) -> ToggleBuilder {
        ToggleBuilder::new(id).presentation_hint(TogglePresentationHint::Switch)
    }

    /// Convenience constructor creating a [`ToggleBuilder`] with [`TogglePresentationHint::Automatic`].
    #[inline]
    pub fn automatic(id: impl Into<NodeId>) -> ToggleBuilder {
        ToggleBuilder::new(id).presentation_hint(TogglePresentationHint::Automatic)
    }
}

impl_bool_prop!(
    Toggle,
    ToggleBuilder,
    value,
    value_of,
    get_value,
    is_value_set,
    is_value_set_of,
    is_value_set_for,
    set_value,
    set_value_for,
    op_set_value,
    op_set_value_for,
    clear_value,
    clear_value_for,
    op_clear_value,
    op_clear_value_for,
    PropertyRef::VALUE,
    "Boolean toggle value (§7.2, §7.4)."
);
impl_enum_prop!(
    Toggle,
    ToggleBuilder,
    TogglePresentationHint,
    presentation_hint,
    presentation_hint_of,
    get_presentation_hint,
    set_presentation_hint,
    set_presentation_hint_for,
    op_set_presentation_hint,
    op_set_presentation_hint_for,
    clear_presentation_hint,
    clear_presentation_hint_for,
    op_clear_presentation_hint,
    op_clear_presentation_hint_for,
    PropertyRef::PRESENTATION_HINT,
    "Advisory presentation hint (checkbox | switch | automatic) (§7.2)."
);
impl_string_prop!(
    Toggle,
    ToggleBuilder,
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
    "Toggle label text (§7.4)."
);
impl_enum_prop!(
    Toggle,
    ToggleBuilder,
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
    Toggle,
    ToggleBuilder,
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
    Toggle,
    ToggleBuilder,
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
    Toggle,
    ToggleBuilder,
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
    Toggle,
    ToggleBuilder,
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
    Toggle,
    ToggleBuilder,
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
    "Whether the toggle is read-only (§7.4)."
);
impl_common_state_props!(Toggle, ToggleBuilder);
impl_common_layout_props!(Toggle, ToggleBuilder);
