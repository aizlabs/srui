"""Parameterized negative validation cases for protocol/registry.yaml."""

from __future__ import annotations

from collections.abc import Callable
from pathlib import Path
from typing import Any

import pytest

from validate.loader import RegistryLoadError, load_registry
from validate.validate import validate_registry_data

FLAT_CATEGORIES = ("node_types", "properties", "events", "operations")


def _assert_fails_with(
    registry: dict,
    *substrings: str,
    check_proto: bool = True,
) -> None:
    result = validate_registry_data(registry, check_proto=check_proto)
    assert not result.ok, "expected validation to fail"
    for substring in substrings:
        assert any(substring in err for err in result.errors), (
            f"expected substring {substring!r} in errors, got: {result.errors}"
        )


def _duplicate_first_id(registry: dict, category: str) -> None:
    items = registry[category]
    items[1]["id"] = items[0]["id"]


def _duplicate_first_name(registry: dict, category: str) -> None:
    items = registry[category]
    items[1]["name"] = items[0]["name"]


def _drop_first_item(registry: dict, category: str) -> None:
    registry[category] = registry[category][1:]


@pytest.mark.parametrize("category", FLAT_CATEGORIES)
def test_duplicate_id_fails(registry_copy: dict, category: str) -> None:
    _duplicate_first_id(registry_copy, category)
    _assert_fails_with(registry_copy, "Duplicate ID", f"[{category}]", check_proto=False)


@pytest.mark.parametrize("category", FLAT_CATEGORIES)
def test_duplicate_name_fails(registry_copy: dict, category: str) -> None:
    _duplicate_first_name(registry_copy, category)
    _assert_fails_with(registry_copy, "Duplicate name", f"[{category}]", check_proto=False)


@pytest.mark.parametrize("category", FLAT_CATEGORIES)
def test_gap_in_ids_fails(registry_copy: dict, category: str) -> None:
    _drop_first_item(registry_copy, category)
    _assert_fails_with(registry_copy, "Accidental gap detected", f"[{category}]", check_proto=False)


def test_duplicate_enum_type_id_fails(registry_copy: dict) -> None:
    registry_copy["enums"][1]["id"] = registry_copy["enums"][0]["id"]
    _assert_fails_with(registry_copy, "Duplicate ID", "[enums]", check_proto=False)


def test_duplicate_enum_type_name_fails(registry_copy: dict) -> None:
    registry_copy["enums"][1]["name"] = registry_copy["enums"][0]["name"]
    _assert_fails_with(registry_copy, "Duplicate enum name", check_proto=False)


def test_gap_in_enum_type_ids_fails(registry_copy: dict) -> None:
    registry_copy["enums"] = registry_copy["enums"][1:]
    _assert_fails_with(registry_copy, "Accidental gap detected", "[enums]", check_proto=False)


def test_duplicate_enum_value_name_fails(registry_copy: dict) -> None:
    values = registry_copy["enums"][0]["values"]
    values[1]["name"] = values[0]["name"]
    _assert_fails_with(
        registry_copy,
        "Duplicate name",
        f"[enums.{registry_copy['enums'][0]['name']}]",
        check_proto=False,
    )


def test_gap_in_enum_value_ids_fails(registry_copy: dict) -> None:
    visibility = next(entry for entry in registry_copy["enums"] if entry["name"] == "Visibility")
    visibility["values"] = visibility["values"][1:]
    _assert_fails_with(registry_copy, "Accidental gap detected", "[enums.Visibility]", check_proto=False)


@pytest.mark.parametrize(
    ("mutator", "expected_substrings"),
    [
        pytest.param(
            lambda registry: registry.update({"namespace": 1}),
            ("Top-level 'namespace' must be 0",),
            id="invalid_namespace_id",
        ),
        pytest.param(
            lambda registry: registry.update({"namespace_name": "   "}),
            ("namespace_name",),
            id="missing_namespace_name",
        ),
        pytest.param(
            lambda registry: registry.pop("namespace_name", None),
            ("namespace_name",),
            id="absent_namespace_name",
        ),
        pytest.param(
            lambda registry: registry["enums"].pop(
                next(i for i, entry in enumerate(registry["enums"]) if entry["name"] == "Visibility")
            ),
            ("requires a defined enum among ['Visibility']", "Missing required standard enum definitions: ['Visibility']"),
            id="missing_enum_for_property_ref",
        ),
    ],
)
def test_namespace_reference_failures(
    registry_copy: dict,
    mutator: Callable[[dict], Any],
    expected_substrings: tuple[str, ...],
) -> None:
    mutator(registry_copy)
    _assert_fails_with(registry_copy, *expected_substrings, check_proto=False)


@pytest.mark.parametrize(
    ("category", "mutator", "expected_substrings"),
    [
        pytest.param(
            "node_types",
            lambda items: items[0].update({"id": 999}),
            ("[proto↔registry] node_types", "StandardNodeType"),
            id="node_type_proto_mismatch",
        ),
        pytest.param(
            "properties",
            lambda items: items[0].update({"id": 999}),
            ("[proto↔registry] properties", "StandardProperty"),
            id="property_proto_mismatch",
        ),
        pytest.param(
            "events",
            lambda items: items[0].update({"id": 999}),
            ("[proto↔registry] events", "StandardEvent"),
            id="event_proto_mismatch",
        ),
        pytest.param(
            "operations",
            lambda items: items[0].update({"id": 999}),
            ("[proto↔registry] operations", "StandardOperation"),
            id="operation_proto_mismatch",
        ),
        pytest.param(
            "enums",
            lambda items: items[0].update({"id": 999}),
            ("[proto↔registry] StandardEnum",),
            id="enum_type_proto_mismatch",
        ),
    ],
)
def test_proto_registry_mismatch_fails(
    registry_copy: dict,
    category: str,
    mutator: Callable[[list], Any],
    expected_substrings: tuple[str, ...],
) -> None:
    if category == "enums":
        mutator(registry_copy["enums"])
    else:
        mutator(registry_copy[category])
    _assert_fails_with(registry_copy, *expected_substrings)


def test_proto_enum_value_mismatch_fails(registry_copy: dict) -> None:
    visibility = next(entry for entry in registry_copy["enums"] if entry["name"] == "Visibility")
    visibility["values"][0]["id"] = 999
    _assert_fails_with(registry_copy, "[proto↔registry] enums.Visibility", "visible")


@pytest.mark.parametrize(
    ("mutator", "expected_substrings"),
    [
        pytest.param(
            lambda registry: registry["node_types"][0].update({"tier": "should"}),
            ("Entry 'Surface' has tier 'should', expected 'required'.",),
            id="invalid_node_tier",
        ),
        pytest.param(
            lambda registry: registry["node_types"][0].update({"category": "bogus"}),
            ("invalid category 'bogus'",),
            id="invalid_node_category",
        ),
        pytest.param(
            lambda registry: registry["node_types"][0].pop("description"),
            ("must include non-empty 'description'",),
            id="missing_node_description",
        ),
        pytest.param(
            lambda registry: registry["properties"][0].update({"category": "bogus"}),
            ("invalid category 'bogus'",),
            id="invalid_property_category",
        ),
        pytest.param(
            lambda registry: next(
                prop for prop in registry["properties"] if prop["name"] == "enabled"
            ).update({"value_type": "integer"}),
            ("invalid value_type 'integer'",),
            id="invalid_property_value_type",
        ),
        pytest.param(
            lambda registry: next(
                event for event in registry["events"] if event["name"] == "ACTIVATE"
            ).update({"kind": "pointer"}),
            ("invalid kind 'pointer'",),
            id="invalid_event_kind",
        ),
        pytest.param(
            lambda registry: registry["operations"][0].update({"category": "bogus"}),
            ("invalid category 'bogus'",),
            id="invalid_operation_category",
        ),
    ],
)
def test_invalid_metadata_fails(
    registry_copy: dict,
    mutator: Callable[[dict], Any],
    expected_substrings: tuple[str, ...],
) -> None:
    mutator(registry_copy)
    _assert_fails_with(registry_copy, *expected_substrings, check_proto=False)


@pytest.mark.parametrize(
    ("yaml_text", "expected_match"),
    [
        pytest.param("node_types:\n  - id: [unclosed\n", "Invalid YAML", id="malformed_yaml_syntax"),
        pytest.param("", "empty", id="empty_yaml_file"),
        pytest.param("- not a mapping\n", "Registry root must be a mapping", id="non_mapping_root"),
    ],
)
def test_load_registry_rejects_malformed_yaml(
    tmp_path: Path,
    yaml_text: str,
    expected_match: str,
) -> None:
    bad_path = tmp_path / "bad.yaml"
    bad_path.write_text(yaml_text, encoding="utf-8")
    with pytest.raises(RegistryLoadError, match=expected_match):
        load_registry(bad_path)


@pytest.mark.parametrize(
    ("mutator", "expected_substrings"),
    [
        pytest.param(
            lambda registry: registry["node_types"].insert(0, "not-a-dict"),
            ("[node_types] Item 0 is not a valid dictionary object.",),
            id="node_types_non_dict_entry",
        ),
        pytest.param(
            lambda registry: registry["properties"].insert(0, 42),
            ("[properties] Item 0 is not a valid dictionary object.",),
            id="properties_non_dict_entry",
        ),
        pytest.param(
            lambda registry: registry["enums"][0].update({"values": {"bad": 1}}),
            ("values must be a list",),
            id="enum_values_not_list",
        ),
        pytest.param(
            lambda registry: registry["events"].insert(0, None),
            ("[events] Item 0 is not a valid dictionary object.",),
            id="events_non_dict_entry",
        ),
        pytest.param(
            lambda registry: registry["operations"].insert(0, ["bad"]),
            ("[operations] Item 0 is not a valid dictionary object.",),
            id="operations_non_dict_entry",
        ),
    ],
)
def test_malformed_registry_entry_types_fail(
    registry_copy: dict,
    mutator: Callable[[dict], Any],
    expected_substrings: tuple[str, ...],
) -> None:
    mutator(registry_copy)
    _assert_fails_with(registry_copy, *expected_substrings, check_proto=False)
