from __future__ import annotations

from validate.conformance import (
    COORDINATE_EVENTS,
    EVENT_KINDS,
    EXPECTED_NODE_TIERS,
    NODE_CATEGORIES,
    NODE_TIERS,
    OPERATION_CATEGORIES,
    PROPERTY_CATEGORIES,
    PROPERTY_ENUM_REFERENCES,
    PROPERTY_VALUE_TYPES,
    SEMANTIC_EVENTS,
)


def _require_string_field(
    entry: dict,
    field: str,
    category_name: str,
    entry_name: str,
    errors: list[str],
) -> None:
    value = entry.get(field)
    if not isinstance(value, str) or not value.strip():
        errors.append(f"[{category_name}] Entry '{entry_name}' must include non-empty '{field}'.")


def validate_node_type_entry(entry: dict, errors: list[str]) -> None:
    name = entry.get("name", "<unknown>")
    _require_string_field(entry, "description", "node_types", name, errors)

    tier = entry.get("tier")
    if tier not in NODE_TIERS:
        errors.append(f"[node_types] Entry '{name}' has invalid tier '{tier}'.")
    elif name in EXPECTED_NODE_TIERS and tier != EXPECTED_NODE_TIERS[name]:
        errors.append(
            f"[node_types] Entry '{name}' has tier '{tier}', expected '{EXPECTED_NODE_TIERS[name]}'."
        )

    category = entry.get("category")
    if category not in NODE_CATEGORIES:
        errors.append(f"[node_types] Entry '{name}' has invalid category '{category}'.")

    validate_node_emits(entry, errors)


def validate_node_emits(entry: dict, errors: list[str]) -> None:
    """Validate a node type's semantic event emission list (§7.6, §7.7, §32.5).

    Every node type must declare `emits` explicitly — an empty list means "originates no
    semantic events", which is a different and weaker claim than "nobody wrote the field yet".
    Coordinate events (§7.7) may never appear: they are reserved for explicitly subscribed
    custom scene nodes, never for Standard Widget Profile controls.
    """
    name = entry.get("name", "<unknown>")
    emits = entry.get("emits")

    if not isinstance(emits, list):
        errors.append(
            f"[node_types] Entry '{name}' must declare an 'emits' list (use [] for none)."
        )
        return

    if len(set(emits)) != len(emits):
        errors.append(f"[node_types] Entry '{name}' has duplicate entries in 'emits'.")

    for event_name in emits:
        if event_name in COORDINATE_EVENTS:
            errors.append(
                f"[node_types] Entry '{name}' emits coordinate event '{event_name}'; "
                "coordinate events are reserved for subscribed custom scenes (§7.7, §32.5)."
            )
        elif event_name not in SEMANTIC_EVENTS:
            errors.append(
                f"[node_types] Entry '{name}' emits unknown event '{event_name}'."
            )


def validate_property_entry(entry: dict, enum_names: set[str], errors: list[str]) -> None:
    name = entry.get("name", "<unknown>")
    _require_string_field(entry, "description", "properties", name, errors)

    category = entry.get("category")
    if category not in PROPERTY_CATEGORIES:
        errors.append(f"[properties] Entry '{name}' has invalid category '{category}'.")

    value_type = entry.get("value_type")
    if value_type not in PROPERTY_VALUE_TYPES:
        errors.append(f"[properties] Entry '{name}' has invalid value_type '{value_type}'.")
    elif value_type == "enum" and name in PROPERTY_ENUM_REFERENCES:
        allowed_enums = PROPERTY_ENUM_REFERENCES[name]
        if not allowed_enums & enum_names:
            errors.append(
                f"[properties] Entry '{name}' with value_type 'enum' requires a defined enum among "
                f"{sorted(allowed_enums)}."
            )


def validate_event_entry(entry: dict, errors: list[str]) -> None:
    name = entry.get("name", "<unknown>")
    _require_string_field(entry, "description", "events", name, errors)

    kind = entry.get("kind")
    if kind not in EVENT_KINDS:
        errors.append(f"[events] Entry '{name}' has invalid kind '{kind}'.")


def validate_operation_entry(entry: dict, errors: list[str]) -> None:
    name = entry.get("name", "<unknown>")
    _require_string_field(entry, "description", "operations", name, errors)

    category = entry.get("category")
    if category not in OPERATION_CATEGORIES:
        errors.append(f"[operations] Entry '{name}' has invalid category '{category}'.")


def validate_enum_entry(entry: dict, errors: list[str]) -> None:
    enum_name = entry.get("name", "<unknown>")
    _require_string_field(entry, "description", "enums", enum_name, errors)

    enum_id = entry.get("id")
    if enum_id is None:
        errors.append(f"[enums] Enum '{enum_name}' is missing enum type 'id'.")

    values = entry.get("values", [])
    if not isinstance(values, list):
        errors.append(f"[enums] Enum '{enum_name}' values must be a list.")
