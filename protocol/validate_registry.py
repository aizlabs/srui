#!/usr/bin/env python3
"""
Validation script for SRUI protocol registry (protocol/registry.yaml).

Checks performed:
1. Validates YAML structure and syntax (supports standard yaml module or fallback built-in parser).
2. Category ID integrity:
   - IDs must be positive integers.
   - No duplicate IDs within any category (node_types, properties, events, operations, and enum values).
   - No gaps in ID numbering (must be strictly contiguous starting at base ID 1: 1..N).
3. Name integrity:
   - No duplicate names within any category or enum.
4. Spec conformance:
   - Every §7.3 required-tier node type is present.
   - Every §7.3 SHOULD-tier node type is present.
   - Every §7.2 table node type is present (all 27 node types).
   - Every §7.4 common property is present across all 4 categories.
   - Every §7.5 standard enum and §7.2 Toggle presentation_hint enum is present with all required values.
   - Every §7.6 / §7.7 standard event type is present.
   - Every §13 core mutation operation is present (required, model, optimization).
"""

import sys
import os
import re
from pathlib import Path

# Required node types per §7.3
REQUIRED_NODE_TYPES = {
    "Surface", "Row", "Column", "Grid", "Spacer", "Separator",
    "Text", "RichText", "Button", "Toggle", "TextInput", "TextArea",
    "Progress", "Image", "Scroll", "List", "Table", "Tree"
}

# SHOULD tier node types per §7.3
SHOULD_NODE_TYPES = {
    "Select", "ChoiceGroup", "Slider", "NumberInput", "Tabs", "Split"
}

# Deferred & additional containers per §7.2
OTHER_STANDARD_NODE_TYPES = {
    "Dialog", "Menu", "Toolbar"
}

# Full set of 27 node types from §7.2 table
ALL_SECTION_7_2_NODE_TYPES = REQUIRED_NODE_TYPES | SHOULD_NODE_TYPES | OTHER_STANDARD_NODE_TYPES

# Required common properties per §7.4
REQUIRED_PROPERTIES_SECTION_7_4 = {
    # Identity / accessibility
    "label", "accessible_description", "role", "value_description", "actions",
    # Common state
    "visibility", "enabled", "read_only", "busy", "selected", "validation_state",
    # Content
    "text", "value", "placeholder", "resource", "items", "model_ref",
    # Layout intent
    "horizontal_alignment", "vertical_alignment", "grow", "shrink",
    "minimum_size", "maximum_size", "preferred_size", "spacing_role", "padding_role"
}

# Standard enums required per §7.5 and §7.2
REQUIRED_ENUM_VALUES = {
    "TextRole": {"title", "heading", "body", "caption", "code", "status", "warning", "error"},
    "ActionRole": {"normal", "primary", "destructive", "quiet"},
    "InputRole": {"plain", "search", "secure", "command"},
    "Importance": {"normal", "emphasized", "de_emphasized"},
    "TogglePresentationHint": {"automatic", "checkbox", "switch"},
}

# Standard event types per §7.6 and §7.7
REQUIRED_EVENTS = {
    "ACTIVATE", "VALUE_CHANGED", "SELECTION_CHANGED", "EXPANSION_CHANGED",
    "TEXT_EDIT", "VIEWPORT_CHANGED",
    "POINTER_DOWN", "POINTER_UP", "POINTER_MOVE", "POINTER_CANCEL", "POINTER_SCROLL"
}

# Core mutation operations per §13
REQUIRED_OPERATIONS = {
    # Required core
    "CREATE_NODE", "DELETE_NODE", "SET_PROPERTY", "CLEAR_PROPERTY", "COMMIT",
    # Standard model
    "CREATE_MODEL", "MODEL_INSERT", "MODEL_DELETE", "MODEL_UPDATE", "MODEL_RESET_RANGE",
    # Optimization
    "MOVE_NODE", "REORDER_CHILDREN", "BATCH_PROPERTY_SET"
}


def parse_yaml_fallback(text: str) -> dict:
    """
    Self-contained parser for registry.yaml supporting the structured YAML format
    used by SRUI registry files without requiring external packages.
    """
    data = {}
    current_section = None
    current_list = None
    current_item = None
    current_enum = None
    current_enum_values = None
    current_enum_item = None

    lines = text.splitlines()
    for line_num, raw_line in enumerate(lines, 1):
        # Strip comments while preserving quoted strings
        line = raw_line.rstrip()
        if not line or line.lstrip().startswith("#"):
            continue

        indent = len(line) - len(line.lstrip())
        stripped = line.strip()

        # Top-level scalar fields (namespace, version, etc.)
        if indent == 0 and not stripped.startswith("-") and ":" in stripped:
            key, val = stripped.split(":", 1)
            key = key.strip()
            val = val.strip().strip('"').strip("'")
            if val.isdigit():
                val = int(val)
            if key in ("node_types", "properties", "enums", "events", "operations"):
                current_section = key
                current_list = []
                data[key] = current_list
                current_item = None
                current_enum = None
            else:
                data[key] = val
                current_section = None
            continue

        # Section items (indent 2 for top-level list items)
        if current_section in ("node_types", "properties", "events", "operations"):
            if stripped.startswith("- id:"):
                val_id = int(stripped.split(":", 1)[1].strip())
                current_item = {"id": val_id}
                current_list.append(current_item)
            elif current_item is not None and ":" in stripped and indent >= 4:
                k, v = stripped.split(":", 1)
                k = k.strip()
                v = v.strip().strip('"').strip("'")
                if v.isdigit():
                    v = int(v)
                elif v.replace('.', '', 1).isdigit() and '.' in v:
                    v = float(v)
                current_item[k] = v

        elif current_section == "enums":
            if stripped.startswith("- name:"):
                name = stripped.split(":", 1)[1].strip().strip('"').strip("'")
                current_enum = {"name": name, "values": []}
                current_enum_values = current_enum["values"]
                current_list.append(current_enum)
                current_enum_item = None
            elif current_enum is not None:
                if stripped.startswith("description:") and indent == 4:
                    desc = stripped.split(":", 1)[1].strip().strip('"').strip("'")
                    current_enum["description"] = desc
                elif stripped.startswith("values:") and indent == 4:
                    pass  # nested values list header
                elif stripped.startswith("- id:") and indent >= 6:
                    val_id = int(stripped.split(":", 1)[1].strip())
                    current_enum_item = {"id": val_id}
                    current_enum_values.append(current_enum_item)
                elif current_enum_item is not None and ":" in stripped and indent >= 8:
                    k, v = stripped.split(":", 1)
                    k = k.strip()
                    v = v.strip().strip('"').strip("'")
                    current_enum_item[k] = v

    return data


def load_registry(filepath: Path) -> dict:
    with open(filepath, "r", encoding="utf-8") as f:
        content = f.read()

    try:
        import yaml
        return yaml.safe_load(content)
    except ImportError:
        return parse_yaml_fallback(content)


def validate_category_sequence(items: list, category_name: str, errors: list) -> tuple:
    """Validate IDs and names in a flat category list."""
    ids_seen = {}
    names_seen = {}
    max_id = 0

    for idx, item in enumerate(items):
        if not isinstance(item, dict):
            errors.append(f"[{category_name}] Item {idx} is not a valid dictionary object.")
            continue

        item_id = item.get("id")
        name = item.get("name")

        if item_id is None:
            errors.append(f"[{category_name}] Entry '{name}' is missing an 'id'.")
            continue

        if not isinstance(item_id, int) or item_id < 1:
            errors.append(f"[{category_name}] Entry '{name}' has invalid non-positive ID: {item_id}.")
            continue

        if item_id in ids_seen:
            errors.append(
                f"[{category_name}] Duplicate ID {item_id} found on '{name}' (previously on '{ids_seen[item_id]}')."
            )
        else:
            ids_seen[item_id] = name

        if name:
            if name in names_seen:
                errors.append(
                    f"[{category_name}] Duplicate name '{name}' found (IDs {names_seen[name]} and {item_id})."
                )
            else:
                names_seen[name] = item_id

        if item_id > max_id:
            max_id = item_id

    # Check for gaps in contiguous 1..max_id sequence
    expected_ids = set(range(1, max_id + 1))
    actual_ids = set(ids_seen.keys())
    missing_ids = expected_ids - actual_ids
    if missing_ids:
        errors.append(
            f"[{category_name}] Accidental gap detected! Missing IDs in contiguous 1..{max_id} sequence: {sorted(missing_ids)}"
        )

    return names_seen, ids_seen, max_id


def validate_registry(registry_path: Path) -> bool:
    print(f"==> Validating SRUI Registry: {registry_path.resolve()}")
    if not registry_path.exists():
        print(f"ERROR: Registry file not found at {registry_path}")
        return False

    errors = []
    registry = load_registry(registry_path)

    # 1. Top-level metadata
    namespace = registry.get("namespace")
    if namespace != 0:
        errors.append(f"Top-level 'namespace' must be 0 for standard registry, got: {namespace}")

    version = registry.get("version")
    if not version:
        errors.append("Top-level 'version' string is required.")

    # 2. Node Types Validation
    node_types = registry.get("node_types", [])
    if not node_types:
        errors.append("Category 'node_types' is missing or empty.")
    node_names, node_ids, max_node_id = validate_category_sequence(node_types, "node_types", errors)

    # Check required tier (§7.3)
    missing_required_nodes = REQUIRED_NODE_TYPES - set(node_names.keys())
    if missing_required_nodes:
        errors.append(f"[node_types] Missing required-tier (§7.3) node types: {sorted(missing_required_nodes)}")

    # Check SHOULD tier (§7.3)
    missing_should_nodes = SHOULD_NODE_TYPES - set(node_names.keys())
    if missing_should_nodes:
        errors.append(f"[node_types] Missing SHOULD-tier (§7.3) node types: {sorted(missing_should_nodes)}")

    # Check all §7.2 node types
    missing_7_2_nodes = ALL_SECTION_7_2_NODE_TYPES - set(node_names.keys())
    if missing_7_2_nodes:
        errors.append(f"[node_types] Missing §7.2 table node types: {sorted(missing_7_2_nodes)}")

    # 3. Properties Validation
    properties = registry.get("properties", [])
    if not properties:
        errors.append("Category 'properties' is missing or empty.")
    prop_names, prop_ids, max_prop_id = validate_category_sequence(properties, "properties", errors)

    missing_props = REQUIRED_PROPERTIES_SECTION_7_4 - set(prop_names.keys())
    if missing_props:
        errors.append(f"[properties] Missing §7.4 common properties: {sorted(missing_props)}")

    # 4. Enums Validation
    enums = registry.get("enums", [])
    if not enums:
        errors.append("Category 'enums' is missing or empty.")

    enums_seen = {}
    for enum_entry in enums:
        enum_name = enum_entry.get("name")
        if not enum_name:
            errors.append("[enums] Enum entry missing 'name'.")
            continue
        if enum_name in enums_seen:
            errors.append(f"[enums] Duplicate enum name '{enum_name}'.")
        enums_seen[enum_name] = enum_entry

        values = enum_entry.get("values", [])
        if not values:
            errors.append(f"[enums] Enum '{enum_name}' has no values defined.")
            continue

        val_names, val_ids, max_val_id = validate_category_sequence(values, f"enums.{enum_name}", errors)

        # Check required enum values if specified
        if enum_name in REQUIRED_ENUM_VALUES:
            expected_vals = REQUIRED_ENUM_VALUES[enum_name]
            missing_vals = expected_vals - set(val_names.keys())
            if missing_vals:
                errors.append(f"[enums.{enum_name}] Missing required values: {sorted(missing_vals)}")

    # Check that all required enums are defined
    missing_enums = set(REQUIRED_ENUM_VALUES.keys()) - set(enums_seen.keys())
    if missing_enums:
        errors.append(f"[enums] Missing required standard enum definitions: {sorted(missing_enums)}")

    # 5. Events Validation
    events = registry.get("events", [])
    if not events:
        errors.append("Category 'events' is missing or empty.")
    event_names, event_ids, max_event_id = validate_category_sequence(events, "events", errors)

    missing_events = REQUIRED_EVENTS - set(event_names.keys())
    if missing_events:
        errors.append(f"[events] Missing standard event types (§7.6, §7.7): {sorted(missing_events)}")

    # 6. Operations Validation
    operations = registry.get("operations", [])
    if not operations:
        errors.append("Category 'operations' is missing or empty.")
    op_names, op_ids, max_op_id = validate_category_sequence(operations, "operations", errors)

    missing_ops = REQUIRED_OPERATIONS - set(op_names.keys())
    if missing_ops:
        errors.append(f"[operations] Missing core mutation operations (§13): {sorted(missing_ops)}")

    # Report results
    if errors:
        print("\n❌ REGISTRY VALIDATION FAILED with the following errors:")
        for err in errors:
            print(f"  • {err}")
        return False

    print("\n✅ REGISTRY VALIDATION PASSED!")
    print(f"  • Namespace: {namespace} ({registry.get('namespace_name')}) v{version}")
    print(f"  • Node Types: {len(node_names)} defined (IDs 1..{max_node_id}, all {len(ALL_SECTION_7_2_NODE_TYPES)} §7.2 types present)")
    print(f"    - Required tier (§7.3): {len(REQUIRED_NODE_TYPES)}/{len(REQUIRED_NODE_TYPES)} verified")
    print(f"    - SHOULD tier (§7.3):   {len(SHOULD_NODE_TYPES)}/{len(SHOULD_NODE_TYPES)} verified")
    print(f"  • Properties: {len(prop_names)} defined (IDs 1..{max_prop_id}, all {len(REQUIRED_PROPERTIES_SECTION_7_4)} §7.4 properties present)")
    print(f"  • Enums:      {len(enums_seen)} defined ({sum(len(e.get('values', [])) for e in enums)} total enum tokens)")
    print(f"  • Events:     {len(event_names)} defined (IDs 1..{max_event_id}, all {len(REQUIRED_EVENTS)} events present)")
    print(f"  • Operations: {len(op_names)} defined (IDs 1..{max_op_id}, all {len(REQUIRED_OPERATIONS)} §13 ops present)")
    print("  • No duplicate IDs, no duplicate names, no accidental gaps in numbering.")
    return True


if __name__ == "__main__":
    script_dir = Path(__file__).parent
    reg_path = script_dir / "registry.yaml"
    if len(sys.argv) > 1:
        reg_path = Path(sys.argv[1])

    success = validate_registry(reg_path)
    sys.exit(0 if success else 1)
