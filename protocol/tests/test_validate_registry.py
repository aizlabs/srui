from __future__ import annotations

import json
from pathlib import Path

import pytest
import yaml

from validate.cli import validate_registry
from validate.conformance import (
    ALL_SECTION_7_2_NODE_TYPES,
    EXPECTED_NODE_TIERS,
    REQUIRED_CONTROL_SPECIFIC_PROPERTIES,
    REQUIRED_ENUM_VALUES,
    REQUIRED_EVENTS,
    REQUIRED_NODE_TYPES,
    REQUIRED_OPERATIONS,
    REQUIRED_PROPERTIES_SECTION_7_4,
    REQUIRED_STANDARD_PROPERTIES,
    SHOULD_NODE_TYPES,
)
from validate.loader import RegistryLoadError, load_registry
from validate.proto_registry import validate_proto_registry_sync
from validate.validate import validate_registry_data

REGISTRY_PATH = Path(__file__).resolve().parent.parent / "registry.yaml"


def test_valid_registry_passes() -> None:
    registry = load_registry(REGISTRY_PATH)
    result = validate_registry_data(registry)
    assert result.ok
    assert result.errors == []


def test_validate_registry_cli_success(capsys: pytest.CaptureFixture[str]) -> None:
    assert validate_registry(REGISTRY_PATH) is True
    captured = capsys.readouterr().out
    assert "REGISTRY VALIDATION PASSED" in captured


def test_validate_registry_json_success(capsys: pytest.CaptureFixture[str]) -> None:
    assert validate_registry(REGISTRY_PATH, json_output=True) is True
    payload = json.loads(capsys.readouterr().out)
    assert payload["ok"] is True
    assert payload["errors"] == []
    assert payload["summary"]["node_types"] == len(ALL_SECTION_7_2_NODE_TYPES)


def test_conformance_constants_match_registry() -> None:
    registry = load_registry(REGISTRY_PATH)

    node_names = {entry["name"] for entry in registry["node_types"]}
    assert node_names == ALL_SECTION_7_2_NODE_TYPES
    assert {entry["name"] for entry in registry["node_types"] if entry["tier"] == "required"} == REQUIRED_NODE_TYPES
    assert {entry["name"] for entry in registry["node_types"] if entry["tier"] == "should"} == SHOULD_NODE_TYPES
    assert {entry["name"]: entry["tier"] for entry in registry["node_types"]} == EXPECTED_NODE_TIERS

    prop_names = {entry["name"] for entry in registry["properties"]}
    assert REQUIRED_PROPERTIES_SECTION_7_4 <= prop_names
    assert REQUIRED_CONTROL_SPECIFIC_PROPERTIES <= prop_names
    assert prop_names >= REQUIRED_STANDARD_PROPERTIES

    enum_names = {entry["name"] for entry in registry["enums"]}
    assert enum_names >= set(REQUIRED_ENUM_VALUES.keys())
    for enum_name, expected_values in REQUIRED_ENUM_VALUES.items():
        values = next(entry for entry in registry["enums"] if entry["name"] == enum_name)["values"]
        assert {value["name"] for value in values} == expected_values

    event_names = {entry["name"] for entry in registry["events"]}
    assert event_names == REQUIRED_EVENTS

    operation_names = {entry["name"] for entry in registry["operations"]}
    assert operation_names == REQUIRED_OPERATIONS


def test_proto_registry_ids_match() -> None:
    registry = load_registry(REGISTRY_PATH)
    result = validate_proto_registry_sync(registry)
    assert result.ok, result.errors


def test_missing_control_specific_property_fails() -> None:
    registry = load_registry(REGISTRY_PATH)
    registry["properties"] = [
        prop for prop in registry["properties"] if prop["name"] != "columns"
    ]
    result = validate_registry_data(registry)
    assert not result.ok
    assert any("control-specific properties" in err and "columns" in err for err in result.errors)


def test_missing_supporting_enum_value_fails() -> None:
    registry = load_registry(REGISTRY_PATH)
    visibility = next(enum for enum in registry["enums"] if enum["name"] == "Visibility")
    visibility["values"] = [
        value for value in visibility["values"] if value["name"] != "collapsed"
    ]
    result = validate_registry_data(registry)
    assert not result.ok
    assert any("enums.Visibility" in err and "collapsed" in err for err in result.errors)


def test_duplicate_property_id_fails() -> None:
    registry = load_registry(REGISTRY_PATH)
    registry["properties"][1]["id"] = registry["properties"][0]["id"]
    result = validate_registry_data(registry)
    assert not result.ok
    assert any("Duplicate ID" in err for err in result.errors)


def test_gap_in_property_ids_fails() -> None:
    registry = load_registry(REGISTRY_PATH)
    registry["properties"] = registry["properties"][1:]
    result = validate_registry_data(registry)
    assert not result.ok
    assert any("Accidental gap detected" in err for err in result.errors)


def test_missing_required_node_type_fails() -> None:
    registry = load_registry(REGISTRY_PATH)
    registry["node_types"] = [
        node for node in registry["node_types"] if node["name"] != "Button"
    ]
    result = validate_registry_data(registry)
    assert not result.ok
    assert any("Missing required-tier" in err and "Button" in err for err in result.errors)


def test_invalid_node_tier_fails() -> None:
    registry = load_registry(REGISTRY_PATH)
    button = next(node for node in registry["node_types"] if node["name"] == "Button")
    button["tier"] = "should"
    result = validate_registry_data(registry)
    assert not result.ok
    assert any("Entry 'Button' has tier 'should', expected 'required'." in err for err in result.errors)


def test_invalid_property_value_type_fails() -> None:
    registry = load_registry(REGISTRY_PATH)
    enabled = next(prop for prop in registry["properties"] if prop["name"] == "enabled")
    enabled["value_type"] = "integer"
    result = validate_registry_data(registry)
    assert not result.ok
    assert any("Entry 'enabled' has invalid value_type 'integer'." in err for err in result.errors)


def test_invalid_event_kind_fails() -> None:
    registry = load_registry(REGISTRY_PATH)
    activate = next(event for event in registry["events"] if event["name"] == "ACTIVATE")
    activate["kind"] = "pointer"
    result = validate_registry_data(registry)
    assert not result.ok
    assert any("Entry 'ACTIVATE' has invalid kind 'pointer'." in err for err in result.errors)


def test_load_registry_rejects_invalid_yaml(tmp_path: Path) -> None:
    bad_path = tmp_path / "bad.yaml"
    bad_path.write_text("node_types:\n  - id: [unclosed\n", encoding="utf-8")
    with pytest.raises(RegistryLoadError, match="Invalid YAML"):
        load_registry(bad_path)


def test_load_registry_rejects_empty_file(tmp_path: Path) -> None:
    empty_path = tmp_path / "empty.yaml"
    empty_path.write_text("", encoding="utf-8")
    with pytest.raises(RegistryLoadError, match="empty"):
        load_registry(empty_path)


def test_validate_registry_cli_reports_load_error(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    bad_path = tmp_path / "bad.yaml"
    bad_path.write_text("{not valid yaml", encoding="utf-8")
    assert validate_registry(bad_path) is False
    captured = capsys.readouterr().out
    assert "Invalid YAML" in captured


def test_validate_registry_json_reports_errors(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    bad_path = tmp_path / "bad.yaml"
    bad_path.write_text("{not valid yaml", encoding="utf-8")
    assert validate_registry(bad_path, json_output=True) is False
    payload = json.loads(capsys.readouterr().out)
    assert payload["ok"] is False
    assert payload["errors"]


def test_round_trip_yaml_matches_canonical_registry() -> None:
    registry = load_registry(REGISTRY_PATH)
    round_tripped = yaml.safe_load(yaml.safe_dump(registry, sort_keys=False))
    result = validate_registry_data(round_tripped)
    assert result.ok
