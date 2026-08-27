from __future__ import annotations

NODE_TIERS = frozenset({"required", "should", "standard", "deferred"})
NODE_CATEGORIES = frozenset(
    {"container", "layout", "text", "control", "content", "collection", "shell"}
)
PROPERTY_CATEGORIES = frozenset(
    {
        "identity_accessibility",
        "common_state",
        "content",
        "layout_intent",
        "control_specific",
    }
)
PROPERTY_VALUE_TYPES = frozenset(
    {"string", "bool", "enum", "list", "value", "resource_hash", "uint64", "float64", "size"}
)
EVENT_KINDS = frozenset({"semantic", "coordinate"})
OPERATION_CATEGORIES = frozenset({"required", "model", "optimization"})

PROPERTY_ENUM_REFERENCES = {
    "role": {"TextRole", "ActionRole", "InputRole", "Importance"},
    "visibility": {"Visibility"},
    "validation_state": {"ValidationState"},
    "horizontal_alignment": {"HorizontalAlignment"},
    "vertical_alignment": {"VerticalAlignment"},
    "spacing_role": {"SpacingRole"},
    "padding_role": {"PaddingRole"},
    "presentation_hint": {"TogglePresentationHint"},
    "selection_mode": {"SelectionMode"},
}

# Conformance oracle for namespace 0 v0.4.0. Keep in sync with protocol/registry.yaml.
# tests/test_validate_registry.py verifies these sets match the canonical registry file.
REQUIRED_NODE_TYPES = {
    "Surface", "Row", "Column", "Grid", "Spacer", "Separator",
    "Text", "RichText", "Button", "Toggle", "TextInput", "TextArea",
    "Progress", "Image", "Scroll", "List", "Table", "Tree",
}
SHOULD_NODE_TYPES = {
    "Select", "ChoiceGroup", "Slider", "NumberInput", "Tabs", "Split",
}
OTHER_STANDARD_NODE_TYPES = {"Dialog", "Menu", "Toolbar"}
ALL_SECTION_7_2_NODE_TYPES = REQUIRED_NODE_TYPES | SHOULD_NODE_TYPES | OTHER_STANDARD_NODE_TYPES

EXPECTED_NODE_TIERS = {
    "Surface": "required",
    "Dialog": "standard",
    "Row": "required",
    "Column": "required",
    "Grid": "required",
    "Spacer": "required",
    "Separator": "required",
    "Scroll": "required",
    "Text": "required",
    "RichText": "required",
    "Button": "required",
    "Toggle": "required",
    "TextInput": "required",
    "TextArea": "required",
    "Progress": "required",
    "Image": "required",
    "List": "required",
    "Table": "required",
    "Tree": "required",
    "Select": "should",
    "ChoiceGroup": "should",
    "Slider": "should",
    "NumberInput": "should",
    "Tabs": "should",
    "Split": "should",
    "Menu": "deferred",
    "Toolbar": "deferred",
}

REQUIRED_PROPERTIES_SECTION_7_4 = {
    "label", "accessible_description", "role", "value_description", "actions",
    "visibility", "enabled", "read_only", "busy", "selected", "validation_state",
    "text", "value", "placeholder", "resource", "items", "model_ref",
    "horizontal_alignment", "vertical_alignment", "grow", "shrink",
    "minimum_size", "maximum_size", "preferred_size", "spacing_role", "padding_role",
}
REQUIRED_CONTROL_SPECIFIC_PROPERTIES = {
    "presentation_hint", "action_key", "columns", "selection_mode",
}
REQUIRED_STANDARD_PROPERTIES = REQUIRED_PROPERTIES_SECTION_7_4 | REQUIRED_CONTROL_SPECIFIC_PROPERTIES

REQUIRED_ENUM_VALUES = {
    "TextRole": {"title", "heading", "body", "caption", "code", "status", "warning", "error"},
    "ActionRole": {"normal", "primary", "destructive", "quiet"},
    "InputRole": {"plain", "search", "secure", "command"},
    "Importance": {"normal", "emphasized", "de_emphasized"},
    "TogglePresentationHint": {"automatic", "checkbox", "switch"},
    "Visibility": {"visible", "hidden", "collapsed"},
    "SpacingRole": {"none", "tight", "normal", "relaxed"},
    "PaddingRole": {"none", "tight", "normal", "relaxed"},
    "HorizontalAlignment": {"leading", "center", "trailing", "fill"},
    "VerticalAlignment": {"top", "center", "bottom", "fill"},
    "SelectionMode": {"none", "single", "multiple"},
    "ValidationState": {"valid", "warning", "error"},
}

REQUIRED_EVENTS = {
    "ACTIVATE", "VALUE_CHANGED", "SELECTION_CHANGED", "EXPANSION_CHANGED",
    "TEXT_EDIT", "VIEWPORT_CHANGED",
    "POINTER_DOWN", "POINTER_UP", "POINTER_MOVE", "POINTER_CANCEL", "POINTER_SCROLL",
}

REQUIRED_OPERATIONS = {
    "CREATE_NODE", "DELETE_NODE", "SET_PROPERTY", "CLEAR_PROPERTY", "COMMIT",
    "CREATE_MODEL", "MODEL_INSERT", "MODEL_DELETE", "MODEL_UPDATE", "MODEL_RESET_RANGE",
    "MOVE_NODE", "REORDER_CHILDREN", "BATCH_PROPERTY_SET",
}

REQUIRED_TIER_NODE_COUNT = len(REQUIRED_NODE_TYPES)
SHOULD_TIER_NODE_COUNT = len(SHOULD_NODE_TYPES)
TOTAL_NODE_TYPE_COUNT = len(ALL_SECTION_7_2_NODE_TYPES)
