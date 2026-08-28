from __future__ import annotations

from dataclasses import dataclass, field

from validate.conformance import (
    ALL_SECTION_7_2_NODE_TYPES,
    REQUIRED_CONTROL_SPECIFIC_PROPERTIES,
    REQUIRED_ENUM_VALUES,
    REQUIRED_EVENTS,
    REQUIRED_NODE_TYPES,
    REQUIRED_OPERATIONS,
    REQUIRED_PROPERTIES_SECTION_7_4,
    REQUIRED_STANDARD_PROPERTIES,
    REQUIRED_TIER_NODE_COUNT,
    SHOULD_NODE_TYPES,
    SHOULD_TIER_NODE_COUNT,
    TOTAL_NODE_TYPE_COUNT,
)
from validate.invariants import validate_category_sequence
from validate.metadata import (
    validate_enum_entry,
    validate_event_entry,
    validate_node_type_entry,
    validate_operation_entry,
    validate_property_entry,
)
from validate.proto_registry import validate_proto_registry_sync


@dataclass
class ValidationResult:
    errors: list[str] = field(default_factory=list)
    summary: dict[str, int | str] = field(default_factory=dict)

    @property
    def ok(self) -> bool:
        return not self.errors


def validate_registry_data(registry: dict, *, check_proto: bool = True) -> ValidationResult:
    """Validate a loaded registry mapping."""
    errors: list[str] = []

    namespace = registry.get("namespace")
    if namespace != 0:
        errors.append(f"Top-level 'namespace' must be 0 for standard registry, got: {namespace}")

    version = registry.get("version")
    if not version:
        errors.append("Top-level 'version' string is required.")

    namespace_name = registry.get("namespace_name")
    if not isinstance(namespace_name, str) or not namespace_name.strip():
        errors.append("Top-level 'namespace_name' string is required.")

    node_types = registry.get("node_types", [])
    if not node_types:
        errors.append("Category 'node_types' is missing or empty.")
    node_names, _, _ = validate_category_sequence(node_types, "node_types", errors)
    for entry in node_types:
        if isinstance(entry, dict):
            validate_node_type_entry(entry, errors)

    if len(node_names) != TOTAL_NODE_TYPE_COUNT:
        errors.append(
            f"[node_types] Expected exactly {TOTAL_NODE_TYPE_COUNT} node types, found {len(node_names)}."
        )

    tier_counts = {}
    for entry in node_types:
        if isinstance(entry, dict):
            tier = entry.get("tier")
            tier_counts[tier] = tier_counts.get(tier, 0) + 1

    if tier_counts.get("required", 0) != REQUIRED_TIER_NODE_COUNT:
        errors.append(
            f"[node_types] Expected {REQUIRED_TIER_NODE_COUNT} required-tier nodes, "
            f"found {tier_counts.get('required', 0)}."
        )
    if tier_counts.get("should", 0) != SHOULD_TIER_NODE_COUNT:
        errors.append(
            f"[node_types] Expected {SHOULD_TIER_NODE_COUNT} SHOULD-tier nodes, "
            f"found {tier_counts.get('should', 0)}."
        )

    missing_required_nodes = REQUIRED_NODE_TYPES - set(node_names.keys())
    if missing_required_nodes:
        errors.append(f"[node_types] Missing required-tier (§7.3) node types: {sorted(missing_required_nodes)}")

    missing_should_nodes = SHOULD_NODE_TYPES - set(node_names.keys())
    if missing_should_nodes:
        errors.append(f"[node_types] Missing SHOULD-tier (§7.3) node types: {sorted(missing_should_nodes)}")

    missing_7_2_nodes = ALL_SECTION_7_2_NODE_TYPES - set(node_names.keys())
    if missing_7_2_nodes:
        errors.append(f"[node_types] Missing §7.2 table node types: {sorted(missing_7_2_nodes)}")

    properties = registry.get("properties", [])
    if not properties:
        errors.append("Category 'properties' is missing or empty.")
    prop_names, _, _ = validate_category_sequence(properties, "properties", errors)

    enums = registry.get("enums", [])
    enum_names = {entry.get("name") for entry in enums if isinstance(entry, dict) and entry.get("name")}

    for entry in properties:
        if isinstance(entry, dict):
            validate_property_entry(entry, enum_names, errors)

    missing_props = REQUIRED_PROPERTIES_SECTION_7_4 - set(prop_names.keys())
    if missing_props:
        errors.append(f"[properties] Missing §7.4 common properties: {sorted(missing_props)}")

    missing_control_props = REQUIRED_CONTROL_SPECIFIC_PROPERTIES - set(prop_names.keys())
    if missing_control_props:
        errors.append(
            "[properties] Missing control-specific properties (§8 / registries.md §4.2): "
            f"{sorted(missing_control_props)}"
        )

    if not enums:
        errors.append("Category 'enums' is missing or empty.")

    enum_type_names, _, _ = validate_category_sequence(enums, "enums", errors)

    enums_seen: dict[str, dict] = {}
    for enum_entry in enums:
        if not isinstance(enum_entry, dict):
            errors.append("[enums] Enum entry must be a dictionary object.")
            continue

        validate_enum_entry(enum_entry, errors)
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

        val_names, _, _ = validate_category_sequence(values, f"enums.{enum_name}", errors)

        if enum_name in REQUIRED_ENUM_VALUES:
            expected_vals = REQUIRED_ENUM_VALUES[enum_name]
            missing_vals = expected_vals - set(val_names.keys())
            if missing_vals:
                errors.append(f"[enums.{enum_name}] Missing required values: {sorted(missing_vals)}")

    missing_enums = set(REQUIRED_ENUM_VALUES.keys()) - set(enums_seen.keys())
    if missing_enums:
        errors.append(f"[enums] Missing required standard enum definitions: {sorted(missing_enums)}")

    events = registry.get("events", [])
    if not events:
        errors.append("Category 'events' is missing or empty.")
    event_names, _, _ = validate_category_sequence(events, "events", errors)
    for entry in events:
        if isinstance(entry, dict):
            validate_event_entry(entry, errors)

    missing_events = REQUIRED_EVENTS - set(event_names.keys())
    if missing_events:
        errors.append(f"[events] Missing standard event types (§7.6, §7.7): {sorted(missing_events)}")

    operations = registry.get("operations", [])
    if not operations:
        errors.append("Category 'operations' is missing or empty.")
    op_names, _, _ = validate_category_sequence(operations, "operations", errors)
    for entry in operations:
        if isinstance(entry, dict):
            validate_operation_entry(entry, errors)

    missing_ops = REQUIRED_OPERATIONS - set(op_names.keys())
    if missing_ops:
        errors.append(f"[operations] Missing core mutation operations (§13): {sorted(missing_ops)}")

    if check_proto:
        proto_sync = validate_proto_registry_sync(registry)
        errors.extend(proto_sync.errors)

    summary = {
        "namespace": namespace,
        "namespace_name": namespace_name,
        "version": version,
        "node_types": len(node_names),
        "properties": len(prop_names),
        "enums": len(enums_seen),
        "events": len(event_names),
        "operations": len(op_names),
        "required_properties": len(REQUIRED_STANDARD_PROPERTIES),
        "required_enums": len(REQUIRED_ENUM_VALUES),
        "proto_sync": "verified" if check_proto and not errors else "skipped",
    }
    return ValidationResult(errors=errors, summary=summary)
