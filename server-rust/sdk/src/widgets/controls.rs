//! Interactive and presentation control widget handles and builders (§7.2, §7.3 Required Tier).

use crate::widgets::*;
use crate::StoreMut;
use srui_semantic_tree::{EnumToken, ResourceHash};

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

// =============================================================================
// 3. Button (§7.2, §7.3 Required Tier, TypeRef::BUTTON / id 11)
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
// 4. Toggle (§7.2, §7.3 Required Tier, TypeRef::TOGGLE / id 12)
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

// =============================================================================
// 5. TextInput (§7.2, §7.3 Required Tier, TypeRef::TEXT_INPUT / id 13)
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
// 6. TextArea (§7.2, §7.3 Required Tier, TypeRef::TEXT_AREA / id 14)
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

// =============================================================================
// 7. Progress (§7.2, §7.3 Required Tier, TypeRef::PROGRESS / id 15)
// =============================================================================

impl_widget_boilerplate!(
    Progress,
    ProgressBuilder,
    TypeRef::PROGRESS,
    "Determinate or indeterminate progress indicator (§7.2, §7.3 Required Tier)."
);
impl_f64_prop!(
    Progress,
    ProgressBuilder,
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
    "Determinate progress value in [0.0, 1.0] (§7.4)."
);
impl_string_prop!(
    Progress,
    ProgressBuilder,
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
    "Human-readable progress description (e.g. '62%') (§7.4)."
);
impl_string_prop!(
    Progress,
    ProgressBuilder,
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
    "Progress label text (§7.4)."
);
impl_string_prop!(
    Progress,
    ProgressBuilder,
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
impl_common_state_props!(Progress, ProgressBuilder);
impl_common_layout_props!(Progress, ProgressBuilder);

// =============================================================================
// 8. Image (§7.2, §7.3 Required Tier, TypeRef::IMAGE / id 16)
// =============================================================================

impl_widget_boilerplate!(
    Image,
    ImageBuilder,
    TypeRef::IMAGE,
    "Raster or vector image resource display (§7.2, §7.3 Required Tier, §14)."
);

impl Image {
    #[doc = "Returns the content-addressed [`ResourceHash`] if set (§14)."]
    #[inline]
    pub fn resource(&self, store: &SemanticStore) -> Option<ResourceHash> {
        store.get_node(self.id).and_then(Self::resource_of)
    }

    #[doc = "Returns the content-addressed [`ResourceHash`] from a [`Node`] (§14)."]
    #[inline]
    pub fn resource_of(node: &Node) -> Option<ResourceHash> {
        node.get_property(PropertyRef::RESOURCE)
            .and_then(Value::as_resource_hash)
    }

    #[doc = "Returns the content-addressed [`ResourceHash`] for node `id` from `store` (§14)."]
    #[inline]
    pub fn get_resource(store: &SemanticStore, id: NodeId) -> Option<ResourceHash> {
        store.get_node(id).and_then(Self::resource_of)
    }

    #[doc = "Sets the `resource` property on this widget in `store` (§13 SET_PROPERTY, §14)."]
    #[inline]
    pub fn set_resource(
        &self,
        store: &mut impl StoreMut,
        hash: impl Into<ResourceHash>,
    ) -> Result<Option<Value>, StoreError> {
        Self::set_resource_for(store, self.id, hash)
    }

    #[doc = "Sets the `resource` property on the given node in `store` (§13 SET_PROPERTY, §14)."]
    #[inline]
    pub fn set_resource_for(
        store: &mut impl StoreMut,
        id: NodeId,
        hash: impl Into<ResourceHash>,
    ) -> Result<Option<Value>, StoreError> {
        store.set_property(id, PropertyRef::RESOURCE, Value::ResourceHash(hash.into()))
    }

    #[doc = "Returns a [`Operation::SetProperty`] operation setting `resource` on this widget (§13, §14)."]
    #[inline]
    pub fn op_set_resource(&self, hash: impl Into<ResourceHash>) -> Operation {
        Self::op_set_resource_for(self.id, hash)
    }

    #[doc = "Returns a [`Operation::SetProperty`] operation setting `resource` (§13, §14)."]
    #[inline]
    pub fn op_set_resource_for(id: NodeId, hash: impl Into<ResourceHash>) -> Operation {
        Operation::set_property(id, PropertyRef::RESOURCE, Value::ResourceHash(hash.into()))
    }

    #[doc = "Clears the `resource` property on this widget in `store` (§13 CLEAR_PROPERTY)."]
    #[inline]
    pub fn clear_resource(&self, store: &mut impl StoreMut) -> Result<Option<Value>, StoreError> {
        Self::clear_resource_for(store, self.id)
    }

    #[doc = "Clears the `resource` property on the given node in `store` (§13 CLEAR_PROPERTY)."]
    #[inline]
    pub fn clear_resource_for(
        store: &mut impl StoreMut,
        id: NodeId,
    ) -> Result<Option<Value>, StoreError> {
        store.clear_property(id, PropertyRef::RESOURCE)
    }

    #[doc = "Returns a [`Operation::ClearProperty`] operation clearing `resource` on this widget (§13)."]
    #[inline]
    pub fn op_clear_resource(&self) -> Operation {
        Self::op_clear_resource_for(self.id)
    }

    #[doc = "Returns a [`Operation::ClearProperty`] operation clearing `resource` (§13)."]
    #[inline]
    pub fn op_clear_resource_for(id: NodeId) -> Operation {
        Operation::clear_property(id, PropertyRef::RESOURCE)
    }
}

impl ImageBuilder {
    #[doc = "Sets the image [`ResourceHash`] for the new widget (§14)."]
    #[inline]
    #[must_use]
    pub fn resource(mut self, hash: impl Into<ResourceHash>) -> Self {
        self.properties
            .push((PropertyRef::RESOURCE, Value::ResourceHash(hash.into())));
        self
    }
}

impl_string_prop!(
    Image,
    ImageBuilder,
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
    "Image alt text or accessibility label (§7.4)."
);
impl_string_prop!(
    Image,
    ImageBuilder,
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
impl_common_state_props!(Image, ImageBuilder);
impl_common_layout_props!(Image, ImageBuilder);
