"""Load and expand the authoritative SRUI benchmark metric contract."""

from __future__ import annotations

import hashlib
import itertools
import json
import math
from pathlib import Path
from typing import Any, Iterator

from benchmarks.errors import BenchmarkError

CONTRACT_PATH = Path(__file__).with_name("metric-contract.json")
MetricIdentity = tuple[str, str]
MetricTarget = float | str | None
MetricMetadata = tuple[str, MetricTarget, str | None]


def _load_contract() -> dict[str, Any]:
    try:
        contract = json.loads(CONTRACT_PATH.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise RuntimeError(
            f"cannot load benchmark metric contract {CONTRACT_PATH}: {error}"
        ) from error
    required = {
        "schema_version", "fixture", "sections", "distribution",
        "percentile_semantics", "dimensions", "profiles", "signed_metric_ids",
        "drivers", "report_additions", "verification",
    }
    if set(contract) != required or contract["schema_version"] != 1:
        raise RuntimeError("benchmark metric contract has an unsupported top-level shape")
    return contract


CONTRACT = _load_contract()
_CONTRACT_CANONICAL_BYTES = json.dumps(
    CONTRACT,
    ensure_ascii=False,
    separators=(",", ":"),
    sort_keys=True,
).encode("utf-8")
CONTRACT_SHA256 = hashlib.sha256(_CONTRACT_CANONICAL_BYTES).hexdigest()


def contract_sha256() -> str:
    """Return the canonical sorted/compact UTF-8 contract digest."""

    return CONTRACT_SHA256


def contract_identity() -> tuple[int, str]:
    """Return the schema version and canonical digest emitted by drivers."""

    return CONTRACT["schema_version"], CONTRACT_SHA256


def percentile(values: list[float], fraction: float) -> float:
    """Apply the contract's finite-only nearest-index, half-up rule."""

    if not values:
        raise ValueError("percentile requires at least one value")
    if (
        not math.isfinite(fraction)
        or fraction < 0
        or fraction > 1
        or any(not math.isfinite(value) for value in values)
    ):
        raise ValueError(
            "percentile requires finite values and a fraction in 0..1"
        )
    ordered = sorted(values)
    rank = (len(ordered) - 1) * fraction
    index = min(len(ordered) - 1, math.floor(rank + 0.5))
    return ordered[index]


EXPECTED_SECTIONS = tuple(CONTRACT["sections"])
CANONICAL_FIXTURE = CONTRACT["fixture"]
DISTRIBUTION = tuple(CONTRACT["distribution"])
LOCAL_FRAME_BUDGET_ID = "display.frame_budget"
WINDOW_ISOLATION_ASSERTION_ID = "window_isolation_fail_closed"
DIMENSIONS = CONTRACT["dimensions"]
LOCAL_INTERACTIONS = tuple(
    item["interaction"] for item in DIMENSIONS["interaction"]
)
SIGNED_METRIC_IDS = frozenset(CONTRACT["signed_metric_ids"])
PROFILE_DRIVER_ITERATIONS = {
    profile: dict(settings["iterations"])
    for profile, settings in CONTRACT["profiles"].items()
}
EXPECTED_DRIVER_SECTIONS = {
    driver: set(spec["sections"])
    for driver, spec in CONTRACT["drivers"].items()
}
EXPECTED_DRIVER_COMMANDS = {
    driver: list(spec["command"])
    for driver, spec in CONTRACT["drivers"].items()
}
EXPECTED_RECONNECT_VERIFICATION = dict(CONTRACT["verification"])
PRODUCTION_CONFORMANCE_SAMPLE_COUNT = next(
    item["formula"]["constant"]
    for item in CONTRACT["report_additions"]["31.5"]["sample_counts"]
    if item["id"] == "runner.production_conformance"
)


def _dimension_contexts(names: list[str]) -> Iterator[dict[str, Any]]:
    domains = [DIMENSIONS[name] for name in names]
    for values in itertools.product(*domains):
        context: dict[str, Any] = {}
        for name, value in zip(names, values):
            if isinstance(value, dict):
                context.update(value)
            else:
                context[name] = value
        yield context


def _expanded(spec: dict[str, Any]) -> Iterator[tuple[str, str, dict[str, Any]]]:
    dimensions = spec.get("dimensions", [])
    contexts = _dimension_contexts(dimensions) if dimensions else iter(({},))
    for context in contexts:
        if "context" in spec and "direction" in context:
            context = context | spec["context"][context["direction"]]
        yield (
            spec["id"].format(**context),
            spec.get("name", "").format(**context),
            context,
        )


def _metric_inventory(
    section: dict[str, Any],
) -> tuple[
    dict[MetricIdentity, MetricMetadata],
    dict[str, str],
    dict[str, dict[str, str]],
]:
    inventory: dict[MetricIdentity, MetricMetadata] = {}
    names: dict[str, str] = {}
    profiled_names: dict[str, dict[str, str]] = {}
    for spec in section.get("metrics", []):
        for metric_id, display_name, _context in _expanded(spec):
            if "profile_names" in spec:
                profile_map = dict(spec["profile_names"])
                if set(profile_map) != set(PROFILE_DRIVER_ITERATIONS):
                    raise RuntimeError(f"metric {metric_id} does not name every profile")
                profiled_names[metric_id] = profile_map
            else:
                names[metric_id] = display_name
            target_map = dict(spec.get("targets", {}))
            target_map.update(spec.get("target_overrides", {}).get(metric_id, {}))
            for statistic in spec["statistics"]:
                identity = (metric_id, statistic)
                if identity in inventory:
                    raise RuntimeError(
                        f"duplicate benchmark metric identity {identity!r}"
                    )
                target = target_map.get(statistic, target_map.get("*"))
                inventory[identity] = (
                    spec["unit"],
                    target.get("value", target.get("metric")) if target else None,
                    target["direction"] if target else None,
                )
    return inventory, names, profiled_names


def _section_inventory(section: dict[str, Any]) -> dict[str, Any]:
    metrics, _names, _profiled = _metric_inventory(section)
    return {
        "metrics": metrics,
        "assertions": frozenset(section.get("assertions", [])),
    }


EXPECTED_DRIVER_INVENTORY = {
    driver: {
        section_id: _section_inventory(section)
        for section_id, section in driver_spec["sections"].items()
    }
    for driver, driver_spec in CONTRACT["drivers"].items()
}
EXPECTED_METRIC_DISPLAY_NAMES: dict[str, str] = {}
PROFILED_METRIC_DISPLAY_NAMES: dict[str, dict[str, str]] = {}
for driver_spec in CONTRACT["drivers"].values():
    for section in driver_spec["sections"].values():
        _inventory, names, profiled = _metric_inventory(section)
        overlap = set(EXPECTED_METRIC_DISPLAY_NAMES) & set(names)
        profile_overlap = set(PROFILED_METRIC_DISPLAY_NAMES) & set(profiled)
        if overlap or profile_overlap:
            raise RuntimeError(
                f"duplicate benchmark metric display names: "
                f"{sorted(overlap | profile_overlap)!r}"
            )
        EXPECTED_METRIC_DISPLAY_NAMES.update(names)
        PROFILED_METRIC_DISPLAY_NAMES.update(profiled)
for addition in CONTRACT["report_additions"].values():
    _inventory, names, profiled = _metric_inventory(addition)
    EXPECTED_METRIC_DISPLAY_NAMES.update(names)
    PROFILED_METRIC_DISPLAY_NAMES.update(profiled)


def _merged_report_inventory() -> dict[str, dict[str, Any]]:
    merged = {
        section_id: {"metrics": {}, "assertions": set()}
        for section_id in EXPECTED_SECTIONS
    }
    for driver_sections in EXPECTED_DRIVER_INVENTORY.values():
        for section_id, inventory in driver_sections.items():
            destination = merged[section_id]
            duplicate_metrics = (
                set(destination["metrics"]) & set(inventory["metrics"])
            )
            duplicate_assertions = (
                destination["assertions"] & set(inventory["assertions"])
            )
            if duplicate_metrics or duplicate_assertions:
                raise RuntimeError(f"driver inventories overlap in §{section_id}")
            destination["metrics"].update(inventory["metrics"])
            destination["assertions"].update(inventory["assertions"])
    for section_id, addition in CONTRACT["report_additions"].items():
        inventory = _section_inventory(addition)
        merged[section_id]["metrics"].update(inventory["metrics"])
        merged[section_id]["assertions"].update(inventory["assertions"])
    return {
        section_id: {
            "metrics": inventory["metrics"],
            "assertions": frozenset(inventory["assertions"]),
        }
        for section_id, inventory in merged.items()
    }


EXPECTED_REPORT_INVENTORY = _merged_report_inventory()
_EXPECTED_METRIC_IDS = {
    metric_id
    for section in EXPECTED_REPORT_INVENTORY.values()
    for metric_id, _statistic in section["metrics"]
}
if _EXPECTED_METRIC_IDS != (
    set(EXPECTED_METRIC_DISPLAY_NAMES) | set(PROFILED_METRIC_DISPLAY_NAMES)
):
    raise RuntimeError(
        "benchmark metric display-name contract does not exactly cover inventory"
    )


def expected_metric_display_name(metric_id: str, profile: str) -> str:
    profiled = PROFILED_METRIC_DISPLAY_NAMES.get(metric_id)
    if profiled is not None:
        return profiled[profile]
    return EXPECTED_METRIC_DISPLAY_NAMES[metric_id]


def expected_report_inventory_for_profile(
    section_id: str, profile: str
) -> dict[str, Any]:
    inventory = EXPECTED_REPORT_INVENTORY[section_id]
    excluded = frozenset(
        CONTRACT["report_additions"].get(section_id, {}).get(
            f"{profile}_excluded_assertions", []
        )
    )
    return {
        "metrics": inventory["metrics"],
        "assertions": inventory["assertions"] - excluded,
    }


def _sample_value(formula: dict[str, Any], iterations: int) -> int:
    if "constant" in formula:
        value = int(formula["constant"])
    elif formula.get("base") == "iterations":
        value = iterations
    else:
        raise RuntimeError(f"unsupported sample-count formula {formula!r}")
    value = max(int(formula.get("minimum", value)), value)
    value = min(int(formula.get("maximum", value)), value)
    for dimension in formula.get("multiply_dimension_sizes", []):
        value *= len(DIMENSIONS[dimension])
    return value


def expected_driver_sample_counts(
    driver_name: str, profile: str
) -> dict[str, dict[str, int]]:
    try:
        iterations = PROFILE_DRIVER_ITERATIONS[profile][driver_name]
        sections = CONTRACT["drivers"][driver_name]["sections"]
    except KeyError as error:
        raise BenchmarkError(
            f"unsupported benchmark sample-count contract: {driver_name}/{profile}"
        ) from error
    expanded: dict[str, dict[str, int]] = {}
    for section_id, section in sections.items():
        counts: dict[str, int] = {}
        for spec in section.get("sample_counts", []):
            for sample_id, _display, _context in _expanded(spec):
                if sample_id in counts:
                    raise RuntimeError(f"duplicate sample-count ID {sample_id!r}")
                counts[sample_id] = _sample_value(spec["formula"], iterations)
        expanded[section_id] = counts
    return expanded


def expected_report_sample_counts(profile: str) -> dict[str, dict[str, int]]:
    merged = {section_id: {} for section_id in EXPECTED_SECTIONS}
    for driver_name in EXPECTED_DRIVER_SECTIONS:
        for section_id, counts in expected_driver_sample_counts(
            driver_name, profile
        ).items():
            collisions = set(merged[section_id]) & set(counts)
            if collisions:
                raise BenchmarkError(
                    f"sample-count contracts overlap in §{section_id}: "
                    f"{sorted(collisions)!r}"
                )
            merged[section_id].update(counts)
    for section_id, addition in CONTRACT["report_additions"].items():
        for spec in addition.get("sample_counts", []):
            for sample_id, _display, _context in _expanded(spec):
                merged[section_id][sample_id] = _sample_value(
                    spec["formula"], 0
                )
    return merged
