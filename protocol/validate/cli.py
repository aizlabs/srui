from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from validate.conformance import (
    ALL_SECTION_7_2_NODE_TYPES,
    EXPECTED_NODE_TIERS,
    REQUIRED_ENUM_VALUES,
    REQUIRED_EVENTS,
    REQUIRED_NODE_TYPES,
    REQUIRED_OPERATIONS,
    REQUIRED_STANDARD_PROPERTIES,
    SHOULD_NODE_TYPES,
)
from validate.invariants import validate_category_sequence
from validate.loader import RegistryLoadError, load_registry
from validate.validate import ValidationResult, validate_registry_data


def _print_success(result: ValidationResult) -> None:
    registry_summary = result.summary
    node_types_count = int(registry_summary.get("node_types", 0))
    properties_count = int(registry_summary.get("properties", 0))
    enums_count = int(registry_summary.get("enums", 0))
    events_count = int(registry_summary.get("events", 0))
    operations_count = int(registry_summary.get("operations", 0))

    print("\n✅ REGISTRY VALIDATION PASSED!")
    print(
        f"  • Namespace: {registry_summary.get('namespace')} "
        f"({registry_summary.get('namespace_name')}) v{registry_summary.get('version')}"
    )
    print(
        f"  • Node Types: {node_types_count} defined "
        f"(all {len(ALL_SECTION_7_2_NODE_TYPES)} §7.2 types present)"
    )
    print(f"    - Required tier (§7.3): {len(REQUIRED_NODE_TYPES)}/{len(REQUIRED_NODE_TYPES)} verified")
    print(f"    - SHOULD tier (§7.3):   {len(SHOULD_NODE_TYPES)}/{len(SHOULD_NODE_TYPES)} verified")
    print(
        f"  • Properties: {properties_count} defined "
        f"(all {len(REQUIRED_STANDARD_PROPERTIES)} standard properties present)"
    )
    print(
        f"  • Enums:      {enums_count} defined "
        f"(all {len(REQUIRED_ENUM_VALUES)} standard enums verified)"
    )
    print(
        f"  • Events:     {events_count} defined "
        f"(all {len(REQUIRED_EVENTS)} events present)"
    )
    print(
        f"  • Operations: {operations_count} defined "
        f"(all {len(REQUIRED_OPERATIONS)} §13 ops present)"
    )
    print("  • No duplicate IDs, no duplicate names, no accidental gaps in numbering.")
    print(f"  • Tier metadata verified for {len(EXPECTED_NODE_TIERS)} node types.")
    print("  • Triple-oracle sync verified (registry.yaml ↔ srui.proto ↔ srui.pb.swift).")


def validate_registry(registry_path: Path, *, json_output: bool = False) -> bool:
    if not registry_path.exists():
        message = f"Registry file not found at {registry_path}"
        if json_output:
            print(json.dumps({"ok": False, "errors": [message]}, indent=2))
        else:
            print(f"==> Validating SRUI Registry: {registry_path.resolve()}")
            print(f"ERROR: {message}")
        return False

    if not json_output:
        print(f"==> Validating SRUI Registry: {registry_path.resolve()}")

    try:
        registry = load_registry(registry_path)
    except RegistryLoadError as exc:
        if json_output:
            print(json.dumps({"ok": False, "errors": [str(exc)]}, indent=2))
        else:
            print(f"ERROR: {exc}")
        return False

    result = validate_registry_data(registry)
    if result.errors:
        if json_output:
            payload = {
                "ok": False,
                "registry": str(registry_path.resolve()),
                "errors": result.errors,
                "summary": result.summary,
            }
            print(json.dumps(payload, indent=2))
        else:
            print("\n❌ REGISTRY VALIDATION FAILED with the following errors:")
            for err in result.errors:
                print(f"  • {err}")
        return False

    if json_output:
        payload = {
            "ok": True,
            "registry": str(registry_path.resolve()),
            "errors": [],
            "summary": result.summary,
        }
        print(json.dumps(payload, indent=2))
    else:
        _print_success(result)
    return True


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Validate SRUI protocol registry YAML.")
    parser.add_argument(
        "registry",
        nargs="?",
        default=str(Path(__file__).resolve().parent.parent / "registry.yaml"),
        help="Path to registry.yaml (defaults to protocol/registry.yaml)",
    )
    parser.add_argument("--json", action="store_true", help="Emit machine-readable JSON output.")
    args = parser.parse_args(argv)

    success = validate_registry(Path(args.registry), json_output=args.json)
    return 0 if success else 1


if __name__ == "__main__":
    sys.exit(main())
