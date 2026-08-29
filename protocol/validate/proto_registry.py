"""Cross-check protocol/srui.proto enum numeric IDs against protocol/registry.yaml and Python conformance oracle."""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from pathlib import Path

from validate.conformance import (
    ALL_SECTION_7_2_NODE_TYPES,
    REQUIRED_ENUM_VALUES,
    REQUIRED_EVENTS,
    REQUIRED_OPERATIONS,
    REQUIRED_STANDARD_PROPERTIES,
)

PROTO_PATH = Path(__file__).resolve().parent.parent / "srui.proto"
SWIFT_PB_PATH = Path(__file__).resolve().parent.parent.parent / "client-macos" / "Protocol" / "srui.pb.swift"

# Operation oneof field numbers match registry operation IDs 1..13 1:1.
OPERATION_WIRE_VARIANTS: dict[str, tuple[str, int]] = {
    "CREATE_NODE": ("create_node", 1),
    "DELETE_NODE": ("delete_node", 2),
    "SET_PROPERTY": ("set_property", 3),
    "CLEAR_PROPERTY": ("clear_property", 4),
    "COMMIT": ("commit", 5),
    "CREATE_MODEL": ("create_model", 6),
    "MODEL_INSERT": ("model_insert", 7),
    "MODEL_DELETE": ("model_delete", 8),
    "MODEL_UPDATE": ("model_update", 9),
    "MODEL_RESET_RANGE": ("model_reset_range", 10),
    "MOVE_NODE": ("move_node", 11),
    "REORDER_CHILDREN": ("reorder_children", 12),
    "BATCH_PROPERTY_SET": ("batch_property_set", 13),
}


@dataclass
class ProtoRegistrySyncResult:
    errors: list[str] = field(default_factory=list)

    @property
    def ok(self) -> bool:
        return not self.errors


def _parse_proto_enums(proto_text: str) -> dict[str, dict[str, int]]:
    enums: dict[str, dict[str, int]] = {}
    current: str | None = None

    for raw_line in proto_text.splitlines():
        line = raw_line.split("//", 1)[0].strip()
        if not line:
            continue

        enum_match = re.match(r"enum\s+(\w+)\s*\{", line)
        if enum_match:
            current = enum_match.group(1)
            enums[current] = {}
            continue

        if current and line == "}":
            current = None
            continue

        if current:
            value_match = re.match(r"(\w+)\s*=\s*(\d+)\s*;?", line)
            if value_match:
                name, value = value_match.group(1), int(value_match.group(2))
                enums[current][name] = value

    return enums


# Proto-local enums that are transport bookkeeping, not namespace-0 semantic values, and so have
# no registry.yaml counterpart. Adding one here must not change the registry counts asserted by
# server-rust/semantic-tree/build.rs or the generated RegistryTables.swift.
#   - EventAckStatus: settlement status of a `ServerEventAck` (§18.2), never carried in a Value.
PROTO_SKIP_ENUMS = frozenset({"NullValue", "EventAckStatus"})


def _normalize_symbol(name: str) -> str:
    return re.sub(r"[^a-z0-9]", "", name.lower())


def _proto_node_type_suffix(proto_name: str) -> str | None:
    if not proto_name.startswith("NODE_TYPE_"):
        return None
    suffix = proto_name[len("NODE_TYPE_") :]
    if suffix == "UNSPECIFIED":
        return None
    return suffix


def _proto_property_to_registry(proto_name: str) -> str | None:
    if not proto_name.startswith("PROPERTY_"):
        return None
    suffix = proto_name[len("PROPERTY_") :]
    if suffix == "UNSPECIFIED":
        return None
    return suffix.lower()


def _proto_event_to_registry(proto_name: str) -> str | None:
    if not proto_name.startswith("EVENT_"):
        return None
    suffix = proto_name[len("EVENT_") :]
    if suffix == "UNSPECIFIED":
        return None
    return suffix


def _proto_operation_to_registry(proto_name: str) -> str | None:
    if not proto_name.startswith("OPERATION_"):
        return None
    suffix = proto_name[len("OPERATION_") :]
    if suffix == "UNSPECIFIED":
        return None
    return suffix


def _proto_standard_enum_to_registry(proto_name: str) -> str | None:
    if not proto_name.startswith("ENUM_"):
        return None
    suffix = proto_name[len("ENUM_") :]
    if suffix == "UNSPECIFIED":
        return None
    return _normalize_symbol(suffix)


def _proto_enum_value_to_registry(enum_type_name: str, proto_value_name: str) -> str | None:
    if proto_value_name.endswith("_UNSPECIFIED"):
        return None

    prefix_candidates = [
        _camel_to_snake_upper(enum_type_name) + "_",
        _enum_specific_prefix(enum_type_name),
    ]
    for prefix in prefix_candidates:
        if proto_value_name.startswith(prefix):
            suffix = proto_value_name[len(prefix) :]
            return suffix.lower().replace("_", "_")

    return None


def _camel_to_snake_upper(name: str) -> str:
    parts = re.sub(r"([a-z0-9])([A-Z])", r"\1_\2", name).split("_")
    return "_".join(part.upper() for part in parts)


def _enum_specific_prefix(enum_type_name: str) -> str:
    mapping = {
        "TogglePresentationHint": "TOGGLE_HINT_",
        "HorizontalAlignment": "HALIGN_",
        "VerticalAlignment": "VALIGN_",
        "SpacingRole": "SPACING_",
        "PaddingRole": "PADDING_",
        "ValidationState": "VALIDATION_",
    }
    return mapping.get(enum_type_name, _camel_to_snake_upper(enum_type_name) + "_")


def _parse_operation_oneof_fields(proto_text: str) -> dict[str, int]:
    match = re.search(r"message Operation\s*\{.*?oneof op\s*\{(.*?)\n\s*\}", proto_text, re.S)
    if not match:
        return {}

    fields: dict[str, int] = {}
    for line in match.group(1).splitlines():
        field_match = re.match(r"\s*(\w+)\s+(\w+)\s*=\s*(\d+)\s*;", line.strip())
        if field_match:
            message_type, field_name, field_number = field_match.groups()
            del message_type
            fields[field_name] = int(field_number)
    return fields


def validate_proto_conformance_py_sync(proto_path: Path = PROTO_PATH) -> ProtoRegistrySyncResult:
    """Assert direct parity between protocol/srui.proto and validate/conformance.py oracle sets."""
    errors: list[str] = []

    if not proto_path.exists():
        return ProtoRegistrySyncResult(errors=[f"Proto file not found: {proto_path}"])

    proto_text = proto_path.read_text(encoding="utf-8")
    proto_enums = _parse_proto_enums(proto_text)

    # 1. Node types
    node_proto = proto_enums.get("StandardNodeType", {})
    proto_node_names = {
        _proto_node_type_suffix(k) for k in node_proto if _proto_node_type_suffix(k) is not None
    }
    # Match case-insensitively
    proto_norm = {_normalize_symbol(n or "") for n in proto_node_names}
    conformance_norm = {_normalize_symbol(n) for n in ALL_SECTION_7_2_NODE_TYPES}
    missing_nodes = conformance_norm - proto_norm
    if missing_nodes:
        errors.append(f"[proto↔conformance.py] StandardNodeType missing nodes: {sorted(missing_nodes)}")

    # 2. Properties
    prop_proto = proto_enums.get("StandardProperty", {})
    proto_props = {_proto_property_to_registry(k) for k in prop_proto if _proto_property_to_registry(k) is not None}
    missing_props = REQUIRED_STANDARD_PROPERTIES - proto_props
    if missing_props:
        errors.append(f"[proto↔conformance.py] StandardProperty missing properties: {sorted(missing_props)}")

    # 3. Events
    event_proto = proto_enums.get("StandardEvent", {})
    proto_events = {_proto_event_to_registry(k) for k in event_proto if _proto_event_to_registry(k) is not None}
    missing_events = REQUIRED_EVENTS - proto_events
    if missing_events:
        errors.append(f"[proto↔conformance.py] StandardEvent missing events: {sorted(missing_events)}")

    # 4. Operations
    op_proto = proto_enums.get("StandardOperation", {})
    proto_ops = {_proto_operation_to_registry(k) for k in op_proto if _proto_operation_to_registry(k) is not None}
    missing_ops = REQUIRED_OPERATIONS - proto_ops
    if missing_ops:
        errors.append(f"[proto↔conformance.py] StandardOperation missing operations: {sorted(missing_ops)}")

    # 5. Enums & Enum Values
    std_enum_proto = proto_enums.get("StandardEnum", {})
    proto_std_enums = {
        _proto_standard_enum_to_registry(k) for k in std_enum_proto if _proto_standard_enum_to_registry(k) is not None
    }
    expected_enum_names = {_normalize_symbol(k) for k in REQUIRED_ENUM_VALUES.keys()}
    missing_enums = expected_enum_names - proto_std_enums
    if missing_enums:
        errors.append(f"[proto↔conformance.py] StandardEnum missing enum types: {sorted(missing_enums)}")

    for enum_name, expected_values in REQUIRED_ENUM_VALUES.items():
        proto_enum = proto_enums.get(enum_name)
        if proto_enum is None:
            errors.append(f"[proto↔conformance.py] Missing enum {enum_name} in proto")
            continue
        proto_val_names = {
            _proto_enum_value_to_registry(enum_name, k)
            for k in proto_enum
            if _proto_enum_value_to_registry(enum_name, k) is not None
        }
        missing_vals = expected_values - proto_val_names
        if missing_vals:
            errors.append(
                f"[proto↔conformance.py] Enum {enum_name} missing values: {sorted(missing_vals)}"
            )

    return ProtoRegistrySyncResult(errors=errors)


def validate_proto_registry_sync(
    registry: dict,
    proto_path: Path = PROTO_PATH,
    swift_pb_path: Path = SWIFT_PB_PATH,
) -> ProtoRegistrySyncResult:
    errors: list[str] = []

    if not proto_path.exists():
        return ProtoRegistrySyncResult(errors=[f"Proto file not found: {proto_path}"])

    # First verify proto against Python conformance oracle sets
    py_sync = validate_proto_conformance_py_sync(proto_path)
    errors.extend(py_sync.errors)

    proto_text = proto_path.read_text(encoding="utf-8")
    proto_enums = _parse_proto_enums(proto_text)
    oneof_fields = _parse_operation_oneof_fields(proto_text)

    # 1. Validate Node Types
    node_proto = proto_enums.get("StandardNodeType", {})
    for entry in registry.get("node_types", []):
        registry_name = entry["name"]
        registry_id = entry["id"]
        registry_norm = _normalize_symbol(registry_name)
        proto_key = next(
            (
                k
                for k in node_proto
                if _proto_node_type_suffix(k) is not None
                and _normalize_symbol(_proto_node_type_suffix(k) or "") == registry_norm
            ),
            None,
        )
        if proto_key is None:
            errors.append(f"[proto↔registry] node_types '{registry_name}' missing from StandardNodeType")
            continue
        if node_proto[proto_key] != registry_id:
            errors.append(
                f"[proto↔registry] node_types '{registry_name}' id {registry_id} != "
                f"StandardNodeType.{proto_key}={node_proto[proto_key]}"
            )

    # 2. Validate Properties
    property_proto = proto_enums.get("StandardProperty", {})
    for entry in registry.get("properties", []):
        registry_name = entry["name"]
        registry_id = entry["id"]
        proto_key = next(
            (k for k, v in property_proto.items() if _proto_property_to_registry(k) == registry_name),
            None,
        )
        if proto_key is None:
            errors.append(f"[proto↔registry] properties '{registry_name}' missing from StandardProperty")
            continue
        if property_proto[proto_key] != registry_id:
            errors.append(
                f"[proto↔registry] properties '{registry_name}' id {registry_id} != "
                f"StandardProperty.{proto_key}={property_proto[proto_key]}"
            )

    # 3. Validate Events
    event_proto = proto_enums.get("StandardEvent", {})
    for entry in registry.get("events", []):
        registry_name = entry["name"]
        registry_id = entry["id"]
        proto_key = next(
            (k for k, v in event_proto.items() if _proto_event_to_registry(k) == registry_name),
            None,
        )
        if proto_key is None:
            errors.append(f"[proto↔registry] events '{registry_name}' missing from StandardEvent")
            continue
        if event_proto[proto_key] != registry_id:
            errors.append(
                f"[proto↔registry] events '{registry_name}' id {registry_id} != "
                f"StandardEvent.{proto_key}={event_proto[proto_key]}"
            )

    # 4. Validate Operations & Wire Tags (1..13 parity)
    operation_proto = proto_enums.get("StandardOperation", {})
    for entry in registry.get("operations", []):
        registry_name = entry["name"]
        registry_id = entry["id"]
        proto_key = next(
            (k for k, v in operation_proto.items() if _proto_operation_to_registry(k) == registry_name),
            None,
        )
        if proto_key is None:
            errors.append(f"[proto↔registry] operations '{registry_name}' missing from StandardOperation")
            continue
        if operation_proto[proto_key] != registry_id:
            errors.append(
                f"[proto↔registry] operations '{registry_name}' id {registry_id} != "
                f"StandardOperation.{proto_key}={operation_proto[proto_key]}"
            )

        wire_variant = OPERATION_WIRE_VARIANTS.get(registry_name)
        if wire_variant is None:
            errors.append(
                f"[proto↔registry] operations '{registry_name}' missing from OPERATION_WIRE_VARIANTS map"
            )
            continue

        oneof_name, expected_field = wire_variant
        actual_field = oneof_fields.get(oneof_name)
        if actual_field is None:
            errors.append(
                f"[proto↔registry] Operation oneof missing field '{oneof_name}' for '{registry_name}'"
            )
        elif actual_field != expected_field:
            errors.append(
                f"[proto↔registry] Operation.{oneof_name} field {actual_field} != "
                f"expected wire field {expected_field}"
            )

    # 5. Validate StandardEnum Type IDs and Individual Enum Values
    registry_enums = registry.get("enums", [])
    registry_enum_by_name = {entry["name"]: entry for entry in registry_enums}

    standard_enum_proto = proto_enums.get("StandardEnum", {})
    for entry in registry_enums:
        enum_name = entry["name"]
        enum_id = entry.get("id")
        if enum_id is None:
            errors.append(f"[proto↔registry] enums.{enum_name} missing 'id' in registry.yaml")
            continue
        norm_name = _normalize_symbol(enum_name)
        proto_enum_key = next(
            (k for k in standard_enum_proto if _proto_standard_enum_to_registry(k) == norm_name),
            None,
        )
        if proto_enum_key is None:
            errors.append(f"[proto↔registry] StandardEnum missing entry for '{enum_name}'")
        elif standard_enum_proto[proto_enum_key] != enum_id:
            errors.append(
                f"[proto↔registry] StandardEnum.{proto_enum_key}={standard_enum_proto[proto_enum_key]} != "
                f"registry enum '{enum_name}' id {enum_id}"
            )

    for enum_type_name, values in proto_enums.items():
        if enum_type_name.startswith("Standard") or enum_type_name in PROTO_SKIP_ENUMS:
            continue
        registry_enum = registry_enum_by_name.get(enum_type_name)
        if registry_enum is None:
            errors.append(f"[proto↔registry] proto enum '{enum_type_name}' missing from registry.yaml enums")
            continue

        registry_values = {value["name"]: value["id"] for value in registry_enum.get("values", [])}
        for proto_value_name, proto_value_id in values.items():
            registry_value_name = _proto_enum_value_to_registry(enum_type_name, proto_value_name)
            if registry_value_name is None:
                continue
            registry_value_id = registry_values.get(registry_value_name)
            if registry_value_id is None:
                errors.append(
                    f"[proto↔registry] enums.{enum_type_name} value '{registry_value_name}' "
                    f"from {proto_value_name} missing in registry"
                )
                continue
            if registry_value_id != proto_value_id:
                errors.append(
                    f"[proto↔registry] enums.{enum_type_name}.{registry_value_name} id {registry_value_id} != "
                    f"{enum_type_name}.{proto_value_name}={proto_value_id}"
                )

    # 6. Validate Swift Codegen Freshness Check
    if not swift_pb_path.exists():
        errors.append(f"[swift-codegen] Missing generated Swift file at {swift_pb_path}")
    else:
        swift_text = swift_pb_path.read_text(encoding="utf-8")
        if len(swift_text) < 1000:
            errors.append(f"[swift-codegen] Generated Swift file at {swift_pb_path} is suspiciously small/truncated")
        required_swift_symbols = [
            "Srui_Protocol_StandardNodeType",
            "Srui_Protocol_StandardProperty",
            "Srui_Protocol_StandardEnum",
            "Srui_Protocol_StandardEvent",
            "Srui_Protocol_StandardOperation",
            "Srui_Protocol_CommitOp",
            "Srui_Protocol_NodeRecord",
            "Srui_Protocol_Transaction",
            "Srui_Protocol_Event",
            "Srui_Protocol_ClientHello",
            "Srui_Protocol_ServerWelcome",
        ]
        for sym in required_swift_symbols:
            if sym not in swift_text:
                errors.append(f"[swift-codegen] Generated Swift file missing expected symbol '{sym}'")

    return ProtoRegistrySyncResult(errors=errors)
