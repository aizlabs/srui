"""Conformance oracle for namespace 0 v0.4.0 derived directly from protocol/registry.yaml."""

from __future__ import annotations

from pathlib import Path
from typing import Any

from validate.loader import load_registry

REGISTRY_PATH = Path(__file__).resolve().parent.parent / "registry.yaml"

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


def _derive_oracle_from_registry(registry_path: Path = REGISTRY_PATH) -> dict[str, Any]:
    if not registry_path.exists():
        return {}
    reg = load_registry(registry_path)

    node_types = reg.get("node_types", [])
    required_nodes = {n["name"] for n in node_types if n.get("tier") == "required"}
    should_nodes = {n["name"] for n in node_types if n.get("tier") == "should"}
    other_nodes = {n["name"] for n in node_types if n.get("tier") in ("standard", "deferred")}
    all_nodes = {n["name"] for n in node_types}
    node_tiers = {n["name"]: n.get("tier") for n in node_types}

    properties = reg.get("properties", [])
    standard_props = {p["name"] for p in properties}
    sec_7_4_props = {
        p["name"] for p in properties if p.get("category") != "control_specific"
    }
    control_props = {
        p["name"] for p in properties if p.get("category") == "control_specific"
    }

    enums = reg.get("enums", [])
    enum_values = {
        e["name"]: {v["name"] for v in e.get("values", [])} for e in enums
    }

    events = reg.get("events", [])
    event_names = {ev["name"] for ev in events}

    operations = reg.get("operations", [])
    op_names = {op["name"] for op in operations}

    return {
        "REQUIRED_NODE_TYPES": required_nodes,
        "SHOULD_NODE_TYPES": should_nodes,
        "OTHER_STANDARD_NODE_TYPES": other_nodes,
        "ALL_SECTION_7_2_NODE_TYPES": all_nodes,
        "EXPECTED_NODE_TIERS": node_tiers,
        "REQUIRED_PROPERTIES_SECTION_7_4": sec_7_4_props,
        "REQUIRED_CONTROL_SPECIFIC_PROPERTIES": control_props,
        "REQUIRED_STANDARD_PROPERTIES": standard_props,
        "REQUIRED_ENUM_VALUES": enum_values,
        "REQUIRED_EVENTS": event_names,
        "REQUIRED_OPERATIONS": op_names,
    }


_DERIVED = _derive_oracle_from_registry()

# Conformance oracle sets derived from registry.yaml
REQUIRED_NODE_TYPES: set[str] = _DERIVED.get("REQUIRED_NODE_TYPES", set())
SHOULD_NODE_TYPES: set[str] = _DERIVED.get("SHOULD_NODE_TYPES", set())
OTHER_STANDARD_NODE_TYPES: set[str] = _DERIVED.get("OTHER_STANDARD_NODE_TYPES", set())
ALL_SECTION_7_2_NODE_TYPES: set[str] = _DERIVED.get("ALL_SECTION_7_2_NODE_TYPES", set())
EXPECTED_NODE_TIERS: dict[str, str] = _DERIVED.get("EXPECTED_NODE_TIERS", {})

REQUIRED_PROPERTIES_SECTION_7_4: set[str] = _DERIVED.get("REQUIRED_PROPERTIES_SECTION_7_4", set())
REQUIRED_CONTROL_SPECIFIC_PROPERTIES: set[str] = _DERIVED.get("REQUIRED_CONTROL_SPECIFIC_PROPERTIES", set())
REQUIRED_STANDARD_PROPERTIES: set[str] = _DERIVED.get("REQUIRED_STANDARD_PROPERTIES", set())

REQUIRED_ENUM_VALUES: dict[str, set[str]] = _DERIVED.get("REQUIRED_ENUM_VALUES", {})
REQUIRED_EVENTS: set[str] = _DERIVED.get("REQUIRED_EVENTS", set())
REQUIRED_OPERATIONS: set[str] = _DERIVED.get("REQUIRED_OPERATIONS", set())

REQUIRED_TIER_NODE_COUNT = len(REQUIRED_NODE_TYPES)
SHOULD_TIER_NODE_COUNT = len(SHOULD_NODE_TYPES)
TOTAL_NODE_TYPE_COUNT = len(ALL_SECTION_7_2_NODE_TYPES)
