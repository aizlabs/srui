"""Schema and semantic validation for benchmark manifests and results."""

from __future__ import annotations

import functools
import json
import math
import os
import re
import shutil
import sys
from pathlib import Path
from typing import Any

from jsonschema import Draft202012Validator, FormatChecker

from benchmarks.contract import (
    CANONICAL_FIXTURE,
    CONTRACT,
    CONTRACT_SHA256,
    DISTRIBUTION,
    EXPECTED_DRIVER_COMMANDS,
    EXPECTED_DRIVER_INVENTORY,
    EXPECTED_DRIVER_SECTIONS,
    EXPECTED_RECONNECT_VERIFICATION,
    EXPECTED_SECTIONS,
    LOCAL_FRAME_BUDGET_ID,
    SIGNED_METRIC_IDS,
    WINDOW_ISOLATION_ASSERTION_ID,
    expected_driver_sample_counts,
    expected_metric_display_name,
    expected_report_inventory_for_profile,
    expected_report_sample_counts,
)
from benchmarks.errors import BenchmarkError
from benchmarks.process_control import (
    ManagedCommandError,
    wait_for_process_identities_gone,
)

ROOT = Path(__file__).resolve().parent.parent
SCHEMA = ROOT / "benchmarks/schema.json"
DEFAULT_MIN_FREE_BYTES = 12 * 1024 * 1024 * 1024
WINDOW_ISOLATION_SELF_TEST_PREFIX = "window isolation self-test passed:"
WINDOW_ISOLATION_SELF_TEST_PATTERN = re.compile(
    r"^window isolation self-test passed: "
    r"dock=(?P<dock>[0-9]+) status=(?P<status>[0-9]+) "
    r"ahead=(?P<ahead>[0-9]+) popup=(?P<popup>[0-9]+) "
    r"target=(?P<target>[0-9]+) occluder=(?P<occluder>[0-9]+)$"
)


@functools.lru_cache(maxsize=1)
def schema_bundle() -> dict[str, Any]:
    try:
        schema = json.loads(SCHEMA.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise BenchmarkError(f"cannot load benchmark schema {SCHEMA}: {error}") from error
    Draft202012Validator.check_schema(schema)
    return schema


def validate_document(document: Any, definition: str, label: str) -> None:
    schema = dict(schema_bundle())
    schema["$ref"] = f"#/$defs/{definition}"
    validator = Draft202012Validator(schema, format_checker=FormatChecker())
    errors = sorted(
        validator.iter_errors(document),
        key=lambda error: "/".join(str(part) for part in error.absolute_path),
    )
    if not errors:
        return
    error = errors[0]
    location = "$"
    for part in error.absolute_path:
        location += f"[{part}]" if isinstance(part, int) else f".{part}"
    raise BenchmarkError(f"{label} schema violation at {location}: {error.message}")


def validate_manifest(manifest: dict[str, Any]) -> None:
    validate_document(manifest, "manifest", "benchmark manifest")
    if manifest["fixture"] != CANONICAL_FIXTURE:
        raise BenchmarkError(
            f"benchmark manifest fixture must be exactly {CANONICAL_FIXTURE}"
        )
    if tuple(manifest["required_sections"]) != EXPECTED_SECTIONS:
        raise BenchmarkError(
            "benchmark manifest required_sections must be exactly "
            + ", ".join(EXPECTED_SECTIONS)
        )
    drivers = {driver["name"]: driver for driver in manifest["drivers"]}
    if set(drivers) != set(EXPECTED_DRIVER_SECTIONS):
        raise BenchmarkError(
            "benchmark manifest must declare exactly the rust and macos drivers"
        )
    for name, expected in EXPECTED_DRIVER_SECTIONS.items():
        if set(drivers[name]["sections"]) != expected:
            raise BenchmarkError(
                f"{name} driver sections must be exactly "
                f"{', '.join(sorted(expected))}"
            )
        if drivers[name]["command"] != EXPECTED_DRIVER_COMMANDS[name]:
            raise BenchmarkError(
                f"{name} driver command must invoke the pinned production benchmark target"
            )
    if drivers["macos"].get("platform") != "darwin":
        raise BenchmarkError("macos driver must declare platform darwin")
    if "platform" in drivers["rust"]:
        raise BenchmarkError("rust driver must remain platform-independent")
    covered = set().union(
        *(set(driver["sections"]) for driver in manifest["drivers"])
    )
    if covered != set(EXPECTED_SECTIONS):
        raise BenchmarkError("driver declarations must cover every §31 subsection")
    if manifest["verification_commands"] != [EXPECTED_RECONNECT_VERIFICATION]:
        raise BenchmarkError(
            "manifest reconnect verification must use the production suite 8 command "
            "and required PASS contract"
        )


def validate_runtime_platforms(
    manifest: dict[str, Any],
    *,
    current_platform: str | None = None,
) -> None:
    platform_name = current_platform if current_platform is not None else sys.platform
    incompatible = [
        f"{driver['name']} requires {driver['platform']}"
        for driver in manifest["drivers"]
        if driver.get("platform") and driver["platform"] != platform_name
    ]
    if incompatible:
        raise BenchmarkError(
            "complete six-section benchmark cannot run on "
            f"{platform_name}: {', '.join(incompatible)}; no drivers were started"
        )


def configured_byte_limit(name: str, default: int) -> int:
    raw = os.environ.get(name)
    if raw is None:
        return default
    try:
        value = int(raw)
    except ValueError as error:
        raise BenchmarkError(f"{name} must be a positive integer byte count") from error
    if value <= 0:
        raise BenchmarkError(f"{name} must be a positive integer byte count")
    return value


def ensure_free_space(path: Path, minimum_bytes: int | None = None) -> None:
    required = minimum_bytes or configured_byte_limit(
        "SRUI_BENCHMARK_MIN_FREE_BYTES",
        DEFAULT_MIN_FREE_BYTES,
    )
    free = shutil.disk_usage(path).free
    if free < required:
        raise BenchmarkError(
            f"benchmark requires at least {required} free bytes at {path}; "
            f"only {free} bytes are available"
        )


def _inventory_difference(
    expected: frozenset[Any],
    actual: frozenset[Any],
) -> str:
    details = []
    if expected - actual:
        details.append(f"missing {sorted(expected - actual)!r}")
    if actual - expected:
        details.append(f"unexpected {sorted(actual - expected)!r}")
    return "; ".join(details)


def _validate_section_inventory(
    section: dict[str, Any],
    expected: dict[str, Any],
    *,
    expected_sample_counts: dict[str, int],
    label: str,
    profile: str,
) -> None:
    actual_sample_counts = section["sample_counts"]
    if actual_sample_counts != expected_sample_counts:
        missing = set(expected_sample_counts) - set(actual_sample_counts)
        unexpected = set(actual_sample_counts) - set(expected_sample_counts)
        mismatched = {
            key: (expected_sample_counts[key], actual_sample_counts[key])
            for key in set(expected_sample_counts) & set(actual_sample_counts)
            if expected_sample_counts[key] != actual_sample_counts[key]
        }
        details = []
        if missing:
            details.append(f"missing {sorted(missing)!r}")
        if unexpected:
            details.append(f"unexpected {sorted(unexpected)!r}")
        if mismatched:
            details.append(f"mismatched {mismatched!r}")
        raise BenchmarkError(f"{label} sample counts: {'; '.join(details)}")

    metric_items = [
        (metric["id"], metric["statistic"])
        for metric in section["metrics"]
    ]
    metric_set = frozenset(metric_items)
    expected_metrics = expected["metrics"]
    expected_metric_set = frozenset(expected_metrics)
    if len(metric_items) != len(metric_set):
        raise BenchmarkError(f"{label} emitted duplicate metric identities")
    if metric_set != expected_metric_set:
        raise BenchmarkError(
            f"{label} metric inventory: "
            f"{_inventory_difference(expected_metric_set, metric_set)}"
        )

    frame_budget: float | int | None = None
    if any(
        metadata[1] == LOCAL_FRAME_BUDGET_ID
        for metadata in expected_metrics.values()
    ):
        budget_metric = next(
            metric
            for metric in section["metrics"]
            if (metric["id"], metric["statistic"])
            == (LOCAL_FRAME_BUDGET_ID, "exact")
        )
        budget_value = budget_metric["value"]
        if (
            isinstance(budget_value, bool)
            or not isinstance(budget_value, (int, float))
            or not math.isfinite(budget_value)
            or budget_value <= 0
        ):
            raise BenchmarkError(
                f"{label} local display frame budget must be positive"
            )
        frame_budget = budget_value

    for metric in section["metrics"]:
        value = metric["value"]
        if (
            isinstance(value, bool)
            or not isinstance(value, (int, float))
            or not math.isfinite(value)
        ):
            raise BenchmarkError(
                f"{label}.{metric['id']} ({metric['statistic']}) is not finite numeric"
            )
        if value < 0 and metric["id"] not in SIGNED_METRIC_IDS:
            raise BenchmarkError(
                f"{label}.{metric['id']} ({metric['statistic']}) must be nonnegative"
            )
        if metric["unit"] in {
            "allocations",
            "blocks",
            "boolean",
            "bytes",
            "frames",
            "messages",
            "repaints",
        } and not float(value).is_integer():
            raise BenchmarkError(
                f"{label}.{metric['id']} ({metric['statistic']}) must be a whole count"
            )
        if metric["unit"] == "boolean" and value not in (0, 1):
            raise BenchmarkError(
                f"{label}.{metric['id']} ({metric['statistic']}) must be zero or one"
            )
        expected_name = expected_metric_display_name(metric["id"], profile)
        if metric["name"] != expected_name:
            raise BenchmarkError(
                f"{label} metric display name for {metric['id']!r}: "
                f"expected {expected_name!r}, got {metric['name']!r}"
            )
        identity = (metric["id"], metric["statistic"])
        expected_unit, expected_target, expected_direction = expected_metrics[
            identity
        ]
        if expected_target == LOCAL_FRAME_BUDGET_ID:
            expected_target = frame_budget
        actual_metadata = (
            metric["unit"],
            metric.get("target"),
            metric.get("target_direction"),
        )
        expected_metadata = (
            expected_unit,
            expected_target,
            expected_direction,
        )
        if actual_metadata != expected_metadata:
            raise BenchmarkError(
                f"{label} metric metadata for {identity!r}: "
                f"expected {expected_metadata!r}, got {actual_metadata!r}"
            )

    metrics_by_id: dict[str, dict[str, float]] = {}
    for metric in section["metrics"]:
        metrics_by_id.setdefault(metric["id"], {})[metric["statistic"]] = metric[
            "value"
        ]
    for metric_id, statistics in metrics_by_id.items():
        ordered = [statistics[key] for key in DISTRIBUTION if key in statistics]
        if any(lower > upper for lower, upper in zip(ordered, ordered[1:])):
            raise BenchmarkError(
                f"{label}.{metric_id} percentile ordering must satisfy p50 <= p95 <= p99"
            )

    assertion_items = [assertion["id"] for assertion in section["assertions"]]
    assertion_set = frozenset(assertion_items)
    if len(assertion_items) != len(assertion_set):
        raise BenchmarkError(f"{label} emitted duplicate assertion IDs")
    if assertion_set != expected["assertions"]:
        raise BenchmarkError(
            f"{label} assertion inventory: "
            f"{_inventory_difference(expected['assertions'], assertion_set)}"
        )

    if section["id"] == "31.3":
        assertions = {
            assertion["id"]: assertion["passed"]
            for assertion in section["assertions"]
        }
        idle_is_zero = all(
            metrics_by_id[metric_id]["observed max"] == 0
            for metric_id in ("idle.bytes", "idle.messages")
        )
        if assertions["idle_zero_traffic"] is not idle_is_zero:
            raise BenchmarkError(
                f"{label} idle_zero_traffic contradicts measured idle bytes/messages"
            )

        wire_invariant = True
        for count in (1, 100, 1000):
            for suffix in (
                "bytes",
                "messages",
                "inbound_bytes",
                "inbound_messages",
                "outbound_bytes",
                "outbound_messages",
            ):
                values = {
                    metrics_by_id[
                        f"cadence.{count}.{cadence}.{suffix}"
                    ]["exact"]
                    for cadence in (60, 120, 144, 240)
                }
                wire_invariant = wire_invariant and len(values) == 1
            for cadence in (60, 120, 144, 240):
                prefix = f"cadence.{count}.{cadence}"
                wire_invariant = wire_invariant and (
                    metrics_by_id[f"{prefix}.bytes"]["exact"]
                    == metrics_by_id[f"{prefix}.inbound_bytes"]["exact"]
                    + metrics_by_id[f"{prefix}.outbound_bytes"]["exact"]
                    and metrics_by_id[f"{prefix}.messages"]["exact"]
                    == metrics_by_id[f"{prefix}.inbound_messages"]["exact"]
                    + metrics_by_id[f"{prefix}.outbound_messages"]["exact"]
                    and metrics_by_id[
                        f"{prefix}.inbound_messages"
                    ]["exact"] == count
                    and metrics_by_id[
                        f"{prefix}.outbound_messages"
                    ]["exact"] == 3
                )
        if assertions["cadence_wire_invariant"] is not wire_invariant:
            raise BenchmarkError(
                f"{label} cadence_wire_invariant contradicts cadence wire metrics"
            )


def validate_renderer_process_attribution(
    artifacts: dict[str, Any],
    *,
    expected_driver_pid: int | None,
    profile: str,
) -> None:
    items = artifacts.get("renderer_process_attribution")
    if not isinstance(items, list):
        raise BenchmarkError("macos driver must emit renderer_process_attribution")
    candidates = [item["candidate"] for item in items]
    if sorted(candidates) != ["srui", "webkit"]:
        raise BenchmarkError(
            "renderer_process_attribution must contain exactly srui and webkit"
        )
    claimed_pids: set[int] = set()
    for item in items:
        candidate = item["candidate"]
        if (
            expected_driver_pid is not None
            and item["driver_pid"] != expected_driver_pid
        ):
            raise BenchmarkError(
                f"{candidate} attribution driver_pid {item['driver_pid']} does not "
                f"match launched BenchmarkDriver pid {expected_driver_pid}"
            )
        if item["host_pid"] == item["driver_pid"]:
            raise BenchmarkError(f"{candidate} candidate must run in a child process")
        if item["started_unix_ns"] >= item["ended_unix_ns"]:
            raise BenchmarkError(
                f"{candidate} process attribution interval is empty or reversed"
            )
        intervals = item["measurement_intervals"]
        expected_interval_count = expected_driver_sample_counts(
            "macos", profile
        )["31.1"][f"macos.{candidate}.render"]
        if len(intervals) != expected_interval_count:
            raise BenchmarkError(
                f"{candidate} measurement interval count must equal "
                f"macos.{candidate}.render sample count {expected_interval_count}; "
                f"got {len(intervals)}"
            )
        previous_end: int | None = None
        for index, interval in enumerate(intervals):
            started = interval["started_unix_ns"]
            ended = interval["ended_unix_ns"]
            if started >= ended:
                raise BenchmarkError(
                    f"{candidate} measurement interval {index} is empty or reversed"
                )
            if started < item["started_unix_ns"] or ended > item["ended_unix_ns"]:
                raise BenchmarkError(
                    f"{candidate} measurement interval {index} escapes candidate lifetime"
                )
            if previous_end is not None and started < previous_end:
                raise BenchmarkError(
                    f"{candidate} measurement intervals overlap or are out of order"
                )
            previous_end = ended
        if item["host_pid"] in item["helper_pids"]:
            raise BenchmarkError(f"{candidate} helper PIDs include its host PID")
        if candidate == "srui" and item["helper_pids"]:
            raise BenchmarkError("srui candidate must not claim helper processes")
        item_pids = {item["host_pid"], *item["helper_pids"]}
        identities = item["process_identities"]
        identity_pids = [identity["pid"] for identity in identities]
        if len(identity_pids) != len(set(identity_pids)):
            raise BenchmarkError(
                f"{candidate} process identities contain duplicate PIDs"
            )
        if set(identity_pids) != item_pids:
            raise BenchmarkError(
                f"{candidate} process identities must exactly cover host and helper PIDs"
            )
        if any(
            identity["birth_unix_ns"]
            > identity["observed_alive_through_unix_ns"]
            for identity in identities
        ):
            raise BenchmarkError(
                f"{candidate} process identity has an invalid observed lifetime"
            )
        host_identity = next(
            identity
            for identity in identities
            if identity["pid"] == item["host_pid"]
        )
        if any(
            host_identity["birth_unix_ns"] > interval["started_unix_ns"]
            or host_identity["observed_alive_through_unix_ns"]
            < interval["ended_unix_ns"]
            for interval in intervals
        ):
            raise BenchmarkError(
                f"{candidate} host identity does not span every measurement interval"
            )
        if item_pids & claimed_pids:
            raise BenchmarkError(
                "renderer process attribution reuses a claimed PID"
            )
        claimed_pids.update(item_pids)


def renderer_process_identities(
    artifacts: dict[str, Any],
) -> list[tuple[int, int]]:
    return [
        (identity["pid"], identity["birth_unix_ns"])
        for item in artifacts["renderer_process_attribution"]
        for identity in item["process_identities"]
    ]


def wait_for_renderer_processes_to_exit(
    artifacts: dict[str, Any],
    *,
    label: str,
) -> None:
    """Prove every exact renderer PID/birth identity has exited."""

    try:
        wait_for_process_identities_gone(
            renderer_process_identities(artifacts),
            label=label,
        )
    except ManagedCommandError as error:
        raise BenchmarkError(str(error)) from error


def _merge_driver_section(
    sections: dict[str, dict[str, Any]],
    incoming: dict[str, Any],
) -> None:
    section_id = incoming["id"]
    if section_id not in sections:
        sections[section_id] = incoming
        return
    current = sections[section_id]
    if current["name"] != incoming["name"]:
        raise BenchmarkError(f"drivers disagree on the name of section {section_id}")
    collisions = set(current["sample_counts"]) & set(incoming["sample_counts"])
    if collisions:
        raise BenchmarkError(
            f"drivers emitted colliding sample-count groups in §{section_id}: "
            f"{sorted(collisions)!r}"
        )
    current["sample_counts"].update(incoming["sample_counts"])
    current["metrics"].extend(incoming["metrics"])
    current["assertions"].extend(incoming["assertions"])
    current.setdefault("notes", []).extend(incoming.get("notes", []))


def _validate_contract_identity(payload: dict[str, Any], label: str) -> None:
    expected = (CONTRACT["schema_version"], CONTRACT_SHA256)
    actual = (
        payload["contract_schema_version"],
        payload["contract_sha256"],
    )
    if actual != expected:
        raise BenchmarkError(
            f"{label} metric contract identity: expected {expected!r}, got {actual!r}"
        )


def validate_driver_output(
    payload: Any,
    driver: dict[str, Any],
    *,
    profile: str = "full",
    launched_pid: int | None = None,
) -> dict[str, Any]:
    validate_document(payload, "driver_output", f"{driver['name']} driver output")
    _validate_contract_identity(payload, f"{driver['name']} driver output")
    if driver["name"] == "macos":
        validate_renderer_process_attribution(
            payload["artifacts"],
            expected_driver_pid=launched_pid,
            profile=profile,
        )
    elif "renderer_process_attribution" in payload["artifacts"]:
        raise BenchmarkError("rust driver must not emit renderer process attribution")
    actual = [section["id"] for section in payload["sections"]]
    if len(actual) != len(set(actual)):
        raise BenchmarkError(f"{driver['name']} driver emitted duplicate sections")
    if set(actual) != set(driver["sections"]):
        raise BenchmarkError(
            f"{driver['name']} driver emitted {', '.join(sorted(actual))}; "
            f"manifest declares {', '.join(sorted(driver['sections']))}"
        )

    expected_sections = EXPECTED_DRIVER_INVENTORY[driver["name"]]
    expected_counts = expected_driver_sample_counts(driver["name"], profile)
    for section in payload["sections"]:
        _validate_section_inventory(
            section,
            expected_sections[section["id"]],
            expected_sample_counts=expected_counts[section["id"]],
            label=f"{driver['name']} §{section['id']}",
            profile=profile,
        )
    if driver["name"] == "macos":
        network = next(
            section
            for section in payload["sections"]
            if section["id"] == "31.4"
        )
        metrics = {
            (metric["id"], metric["statistic"]): metric
            for metric in network["metrics"]
        }
        local_latency = next(
            assertion
            for assertion in network["assertions"]
            if assertion["id"] == "local_latency_independent"
        )
        local_delta = metrics[("local_rtt_delta", "p50")]["value"]
        frame_budget = metrics[(LOCAL_FRAME_BUDGET_ID, "exact")]["value"]
        if (
            profile == "full"
            and local_latency["passed"]
            and local_delta > frame_budget
        ):
            raise BenchmarkError(
                "macos §31.4 local_latency_independent assertion contradicts "
                "the full-compositor display.frame_budget target"
            )
    return payload


def parse_window_isolation_self_test_output(
    stdout: str,
    stderr: str,
) -> dict[str, int]:
    candidate_lines = [
        line
        for stream in (stdout, stderr)
        for line in stream.splitlines()
        if WINDOW_ISOLATION_SELF_TEST_PREFIX in line
    ]
    if len(candidate_lines) != 1:
        raise BenchmarkError(
            "window isolation self-test must emit exactly one result line; "
            f"observed {len(candidate_lines)}"
        )
    line = candidate_lines[0]
    match = WINDOW_ISOLATION_SELF_TEST_PATTERN.fullmatch(line)
    if match is None:
        raise BenchmarkError(
            f"window isolation self-test emitted malformed result line: {line!r}"
        )
    evidence = {
        key: int(value)
        for key, value in match.groupdict().items()
    }
    if not (
        evidence["dock"]
        < evidence["status"]
        < evidence["ahead"]
        < evidence["popup"]
    ):
        raise BenchmarkError(
            "window isolation self-test reported invalid level ordering: "
            f"dock={evidence['dock']} status={evidence['status']} "
            f"ahead={evidence['ahead']} popup={evidence['popup']}"
        )
    target = evidence["target"]
    occluder = evidence["occluder"]
    if target <= 0 or occluder <= 0 or target == occluder:
        raise BenchmarkError(
            "window isolation self-test requires distinct positive window IDs: "
            f"target={target} occluder={occluder}"
        )
    return evidence


def validate_report(report: dict[str, Any], required: list[str]) -> None:
    validate_document(report, "report", "benchmark report")
    _validate_contract_identity(report, "benchmark report")
    if report["fixture"] != CANONICAL_FIXTURE:
        raise BenchmarkError(
            f"benchmark report fixture must be exactly {CANONICAL_FIXTURE}"
        )
    seen = [section["id"] for section in report["sections"]]
    if len(seen) != len(set(seen)):
        raise BenchmarkError("benchmark report contains duplicate sections")
    if set(seen) != set(required):
        missing = set(required) - set(seen)
        unexpected = set(seen) - set(required)
        detail = []
        if missing:
            detail.append("missing " + ", ".join(sorted(missing)))
        if unexpected:
            detail.append("unexpected " + ", ".join(sorted(unexpected)))
        raise BenchmarkError(
            "benchmark report sections: " + "; ".join(detail)
        )

    sections_by_id = {
        section["id"]: section for section in report["sections"]
    }
    expected_counts = expected_report_sample_counts(report["profile"])
    for section_id, section in sections_by_id.items():
        _validate_section_inventory(
            section,
            expected_report_inventory_for_profile(
                section_id,
                report["profile"],
            ),
            expected_sample_counts=expected_counts[section_id],
            label=f"report §{section_id}",
            profile=report["profile"],
        )

    if report["profile"] == "full":
        window_isolation = next(
            assertion
            for assertion in sections_by_id["31.1"]["assertions"]
            if assertion["id"] == WINDOW_ISOLATION_ASSERTION_ID
        )
        if window_isolation["passed"] is not True:
            raise BenchmarkError(
                "window isolation self-test assertion must pass"
            )
        isolation_detail = window_isolation.get("detail")
        if not isinstance(isolation_detail, str):
            raise BenchmarkError(
                "window isolation self-test assertion must retain exact numeric detail"
            )
        parse_window_isolation_self_test_output("", isolation_detail)

    artifacts = report["driver_artifacts"]
    if "renderer_process_attribution" in artifacts["rust"]:
        raise BenchmarkError(
            "rust report artifact must not claim renderer processes"
        )
    validate_renderer_process_attribution(
        artifacts["macos"],
        expected_driver_pid=None,
        profile=report["profile"],
    )
    canonical_keys = (
        "canonical_transaction_sha256",
        "canonical_transaction_bytes",
    )
    artifacts_match = all(
        artifacts["rust"][key] == artifacts["macos"][key]
        for key in canonical_keys
    )
    parity = next(
        assertion
        for assertion in sections_by_id["31.2"]["assertions"]
        if assertion["id"] == "canonical_transaction_parity"
    )
    if parity["passed"] is not artifacts_match:
        raise BenchmarkError(
            "canonical_transaction_parity is inconsistent with driver artifacts"
        )
