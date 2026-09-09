#!/usr/bin/env python3
"""Run SRUI's layered §31 benchmark suite and write JSON plus Markdown reports."""

from __future__ import annotations

import argparse
import contextlib
import datetime as dt
import functools
import json
import math
import os
import platform
import shutil
import signal
import sys
import tempfile
from collections.abc import Iterator
from pathlib import Path
from typing import Any

from jsonschema import Draft202012Validator, FormatChecker

from process_control import (
    ManagedCommandError,
    ManagedCommandTimeout,
    blocked_termination_signals,
    non_termination_exceptions,
    run_managed_command,
    termination_exceptions,
    wait_for_process_identities_gone,
)

ROOT = Path(__file__).resolve().parent.parent
MANIFEST = ROOT / "benchmarks/manifest.json"
SCHEMA = ROOT / "benchmarks/schema.json"
EXPECTED_SECTIONS = ("31.1", "31.2", "31.3", "31.4", "31.5", "31.6")
EXPECTED_DRIVER_SECTIONS = {
    "rust": {"31.2", "31.5", "31.6"},
    "macos": {"31.1", "31.3", "31.4", "31.5", "31.6"},
}
EXPECTED_RECONNECT_VERIFICATION = {
    "name": "production reconnect boundary suite",
    "section": "31.5",
    "command": ["scripts/run-conformance", "--suite", "8", "--implementation", "both"],
    "timeout": 1200,
    "required_output": [
        "8  reconnect",
        "PASS    9 runner(s)",
        "1 passed, 0 failed",
    ],
}
DEFAULT_MIN_FREE_BYTES = 12 * 1024 * 1024 * 1024
DISTRIBUTION = ("p50", "p95", "p99")
LOCAL_INTERACTIONS = (
    "text_entry",
    "caret_movement",
    "text_selection",
    "ime_composition",
    "scrolling",
    "hover_pressed",
    "menu_opening",
)
MetricIdentity = tuple[str, str]
LOCAL_FRAME_BUDGET_ID = "display.frame_budget"
MetricTarget = float | str | None
MetricMetadata = tuple[str, MetricTarget, str | None]


def _metric_inventory(
    specification: dict[str, tuple[str, tuple[str, ...]]],
    targets: dict[MetricIdentity, tuple[float | str, str]] | None = None,
) -> dict[MetricIdentity, MetricMetadata]:
    target_contracts = targets or {}
    identities = {
        (metric_id, statistic)
        for metric_id, (_unit, statistics) in specification.items()
        for statistic in statistics
    }
    unknown_targets = set(target_contracts) - identities
    if unknown_targets:
        raise ValueError(f"targets reference unknown metrics: {sorted(unknown_targets)!r}")
    return {
        identity: (
            specification[identity[0]][0],
            target_contracts[identity][0] if identity in target_contracts else None,
            target_contracts[identity][1] if identity in target_contracts else None,
        )
        for identity in identities
    }


def _coverage(
    metrics: dict[str, tuple[str, tuple[str, ...]]],
    assertions: tuple[str, ...],
    targets: dict[MetricIdentity, tuple[float | str, str]] | None = None,
) -> dict[str, Any]:
    return {
        "metrics": _metric_inventory(metrics, targets),
        "assertions": frozenset(assertions),
    }


EXPECTED_DRIVER_INVENTORY = {
    "rust": {
        "31.2": _coverage(
            {
                "abstract_state_generation_ms": ("ms", DISTRIBUTION),
                "protobuf_serialization_ms": ("ms", DISTRIBUTION),
                "serialized_transaction_bytes": ("bytes", ("exact",)),
            },
            ("fixture_protobuf_valid",),
        ),
        "31.5": _coverage(
            {
                metric_id: ("ms", DISTRIBUTION)
                for metric_id in (
                    "disconnect_before_event_receipt_ms",
                    "event_to_settled_side_effect_ms",
                    "cached_duplicate_response_ms",
                    "lost_ack_wire_duplicate_ms",
                    "mid_resource_reconnect_ms",
                    "mid_transaction_codec_replay_ms",
                    "mid_transaction_wire_replay_ms",
                    "partial_event_wire_replay_ms",
                    "resume_beyond_retention_ms",
                    "resume_within_retention_ms",
                )
            },
            (
                "mid_resource_exact_restart",
                "mid_transaction_wire_atomic",
                "partial_event_wire_once",
                "lost_ack_wire_duplicate_once",
                "journal_retention_boundary",
            ),
        ),
        "31.6": _coverage(
            {
                "embedded_pty_interaction_ms": ("ms", DISTRIBUTION),
                "standalone_pty_interaction_ms": ("ms", DISTRIBUTION),
                "terminal_retention_loss_ms": ("ms", ("sample",)),
                "terminal_payload_bytes": ("bytes", ("exact",)),
                "embedded_terminal_frame_count": ("messages", DISTRIBUTION),
            },
            (
                "pty_payload_identical",
                "standalone_pty_exit_success",
                "embedded_pty_eof_exact",
                "embedded_terminal_frame_bounds",
                "terminal_ring_retention_loss",
            ),
        ),
    },
    "macos": {
        "31.1": _coverage(
            {
                "srui.first_paint": ("ms", ("p50", "p95")),
                "srui.complete_paint": ("ms", DISTRIBUTION),
                "srui.cpu": ("ms", ("p50", "p95")),
                "srui.host_retained_allocations": ("allocations", ("p50",)),
                "srui.process_footprint_peak": ("MiB", ("max",)),
                "srui.process_footprint_growth": ("MiB", ("last-first",)),
                "webkit.first_paint": ("ms", ("p50", "p95")),
                "webkit.complete_paint": ("ms", ("p50", "p95")),
                "webkit.cpu": ("ms", ("p50",)),
                "webkit.host_retained_allocations": ("allocations", ("p50",)),
                "webkit.process_footprint_peak": ("MiB", ("max",)),
                "webkit.process_footprint_growth": ("MiB", ("last-first",)),
                "representation.srui_bytes": ("bytes", ("exact",)),
                "representation.html_bytes": ("bytes", ("exact",)),
                "paint.capture_authorization": ("boolean", ("exact",)),
            },
            (
                "semantic_representation_parity",
                "paint_completion_observed",
                "webkit_helpers_attributed",
            ),
        ),
        "31.3": _coverage(
            {
                **{
                    f"updates.{count}.semantic": ("ms", DISTRIBUTION)
                    for count in (1, 100, 1000)
                },
                **{
                    f"updates.{count}.visible": ("ms", DISTRIBUTION)
                    for count in (1, 100, 1000)
                },
                **{
                    f"updates.{count}.bytes": ("bytes", ("exact",))
                    for count in (1, 100, 1000)
                },
                **{
                    f"updates.{count}.messages": ("messages", ("exact",))
                    for count in (1, 100, 1000)
                },
                **{
                    f"cadence.{cadence}.bytes": ("bytes", ("exact",))
                    for cadence in (60, 120, 144, 240)
                },
                **{
                    f"cadence.{cadence}.messages": ("messages", ("exact",))
                    for cadence in (60, 120, 144, 240)
                },
                **{
                    f"cadence.{cadence}.repaints": ("repaints", ("exact",))
                    for cadence in (60, 120, 144, 240)
                },
                "idle.bytes": ("bytes", ("observed max",)),
                "idle.messages": ("messages", ("observed max",)),
            },
            (
                "mutation_raster_completion",
                "idle_zero_traffic",
                "cadence_wire_invariant",
                "cadence_repaint_independent",
                "cadence_state_event_order",
            ),
            {
                ("updates.100.semantic", "p50"): (1.0, "max"),
                ("updates.1000.semantic", "p50"): (5.0, "max"),
            },
        ),
        "31.4": _coverage(
            {
                **{
                    f"interaction.{interaction}.rtt.{rtt}": ("ms", DISTRIBUTION)
                    for interaction in LOCAL_INTERACTIONS
                    for rtt in (0, 100, 300, 600)
                },
                **{
                    f"server_feedback.rtt.{rtt}": ("ms", DISTRIBUTION)
                    for rtt in (0, 100, 300, 600)
                },
                "impairment.bandwidth_transfer": ("ms", ("p50", "p95")),
                "impairment.bandwidth_delivered_bytes": ("bytes", ("exact",)),
                "impairment.loss_attempts": ("messages", ("exact",)),
                "impairment.loss_delivered_messages": ("messages", ("exact",)),
                "impairment.interruption_detection": ("ms", ("p50",)),
                "session_wire.bytes": ("bytes", ("exact",)),
                "session_wire.messages": ("messages", ("exact",)),
                "local_rtt_delta": ("ms", DISTRIBUTION),
                LOCAL_FRAME_BUDGET_ID: ("ms", ("exact",)),
            },
            (
                "local_latency_independent",
                "server_latency_tracks_rtt",
                "no_sync_rtt",
                "impairments_use_session",
            ),
            {
                **{
                    (f"interaction.{interaction}.rtt.{rtt}", "p50"): (
                        LOCAL_FRAME_BUDGET_ID,
                        "max",
                    )
                    for interaction in LOCAL_INTERACTIONS
                    for rtt in (0, 100, 300, 600)
                },
                ("local_rtt_delta", "p50"): (LOCAL_FRAME_BUDGET_ID, "max"),
            },
        ),
        "31.5": _coverage(
            {
                "mid_resource_recovery": ("ms", ("p50", "p95")),
                "superseded_response": ("ms", ("p50", "p95")),
                "active_response": ("ms", ("p50", "p95")),
            },
            (
                "mid_resource_recovery",
                "superseded_response_inert",
            ),
        ),
        "31.6": _coverage(
            {
                "client_terminal.decode_visible": ("ms", DISTRIBUTION),
                "client_terminal.draw_only": ("ms", DISTRIBUTION),
                "client_terminal.frame_bytes": ("bytes", ("exact",)),
                "client_terminal.raster_completions": ("frames", ("exact",)),
            },
            (
                "terminal_offsets_exact",
                "terminal_draw_completion",
                "terminal_fresh_state",
            ),
        ),
    },
}


def _merged_report_inventory() -> dict[str, dict[str, Any]]:
    merged = {
        section_id: {"metrics": {}, "assertions": set()}
        for section_id in EXPECTED_SECTIONS
    }
    for driver_sections in EXPECTED_DRIVER_INVENTORY.values():
        for section_id, inventory in driver_sections.items():
            section = merged[section_id]
            duplicate_metrics = set(section["metrics"]) & set(inventory["metrics"])
            duplicate_assertions = section["assertions"] & set(inventory["assertions"])
            if duplicate_metrics or duplicate_assertions:
                raise ValueError(
                    f"driver inventories overlap in §{section_id}: "
                    f"{sorted(duplicate_metrics)!r}, {sorted(duplicate_assertions)!r}"
                )
            section["metrics"].update(inventory["metrics"])
            section["assertions"].update(inventory["assertions"])
    merged["31.2"]["assertions"].add("canonical_transaction_parity")
    merged["31.5"]["metrics"][("production_reconnect_suite_ms", "wall")] = (
        "ms",
        None,
        None,
    )
    merged["31.5"]["assertions"].add("production_reconnect_suite")
    return {
        section_id: {
            "metrics": inventory["metrics"],
            "assertions": frozenset(inventory["assertions"]),
        }
        for section_id, inventory in merged.items()
    }


EXPECTED_REPORT_INVENTORY = _merged_report_inventory()


class BenchmarkError(RuntimeError):
    pass


def exception_group_detail(error: BaseExceptionGroup) -> str:
    details: list[str] = []

    def collect(item: BaseException) -> None:
        if isinstance(item, BaseExceptionGroup):
            for nested in item.exceptions:
                collect(nested)
        else:
            details.append(f"{type(item).__name__}: {item}")

    collect(error)
    return "; ".join(details)


class TerminationRequested(BaseException):
    def __init__(self, signum: int) -> None:
        super().__init__(signum)
        self.signum = signum


def percentile(values: list[float], fraction: float) -> float:
    if not values:
        raise ValueError("percentile requires at least one value")
    ordered = sorted(values)
    index = min(len(ordered) - 1, max(0, round((len(ordered) - 1) * fraction)))
    return ordered[index]


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

    required = tuple(manifest["required_sections"])
    if required != EXPECTED_SECTIONS:
        raise BenchmarkError(
            "benchmark manifest required_sections must be exactly "
            + ", ".join(EXPECTED_SECTIONS)
        )

    drivers = {driver["name"]: driver for driver in manifest["drivers"]}
    if set(drivers) != set(EXPECTED_DRIVER_SECTIONS):
        raise BenchmarkError("benchmark manifest must declare exactly the rust and macos drivers")
    for name, expected in EXPECTED_DRIVER_SECTIONS.items():
        declared = set(drivers[name]["sections"])
        if declared != expected:
            raise BenchmarkError(
                f"{name} driver sections must be exactly {', '.join(sorted(expected))}"
            )
    if drivers["macos"].get("platform") != "darwin":
        raise BenchmarkError("macos driver must declare platform darwin")
    if "platform" in drivers["rust"]:
        raise BenchmarkError("rust driver must remain platform-independent")

    covered = set().union(*(set(driver["sections"]) for driver in manifest["drivers"]))
    if covered != set(EXPECTED_SECTIONS):
        raise BenchmarkError("driver declarations must cover every §31 subsection")
    if manifest["verification_commands"] != [EXPECTED_RECONNECT_VERIFICATION]:
        raise BenchmarkError(
            "manifest reconnect verification must use the production suite 8 command "
            "and required PASS contract"
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
    label: str,
) -> None:
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
            raise BenchmarkError(f"{label} local display frame budget must be positive")
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
        identity = (metric["id"], metric["statistic"])
        expected_unit, expected_target, expected_direction = expected_metrics[identity]
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

    assertion_items = [assertion["id"] for assertion in section["assertions"]]
    assertion_set = frozenset(assertion_items)
    if len(assertion_items) != len(assertion_set):
        raise BenchmarkError(f"{label} emitted duplicate assertion IDs")
    if assertion_set != expected["assertions"]:
        raise BenchmarkError(
            f"{label} assertion inventory: "
            f"{_inventory_difference(expected['assertions'], assertion_set)}"
        )


def validate_renderer_process_attribution(
    artifacts: dict[str, Any],
    *,
    expected_driver_pid: int | None,
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
        if expected_driver_pid is not None and item["driver_pid"] != expected_driver_pid:
            raise BenchmarkError(
                f"{candidate} attribution driver_pid {item['driver_pid']} does not "
                f"match launched BenchmarkDriver pid {expected_driver_pid}"
            )
        if item["host_pid"] == item["driver_pid"]:
            raise BenchmarkError(f"{candidate} candidate must run in a child process")
        if item["started_unix_ns"] > item["ended_unix_ns"]:
            raise BenchmarkError(f"{candidate} process attribution interval is reversed")
        if item["host_pid"] in item["helper_pids"]:
            raise BenchmarkError(f"{candidate} helper PIDs include its host PID")
        if candidate == "srui" and item["helper_pids"]:
            raise BenchmarkError("srui candidate must not claim helper processes")
        item_pids = {item["host_pid"], *item["helper_pids"]}
        identities = item["process_identities"]
        identity_pids = [identity["pid"] for identity in identities]
        if len(identity_pids) != len(set(identity_pids)):
            raise BenchmarkError(f"{candidate} process identities contain duplicate PIDs")
        if set(identity_pids) != item_pids:
            raise BenchmarkError(
                f"{candidate} process identities must exactly cover host and helper PIDs"
            )
        if item_pids & claimed_pids:
            raise BenchmarkError("renderer process attribution reuses a claimed PID")
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
    try:
        wait_for_process_identities_gone(
            renderer_process_identities(artifacts),
            label=label,
        )
    except ManagedCommandError as error:
        raise BenchmarkError(str(error)) from error


def validate_driver_output(
    payload: Any,
    driver: dict[str, Any],
    *,
    launched_pid: int | None = None,
) -> dict[str, Any]:
    validate_document(payload, "driver_output", f"{driver['name']} driver output")
    if driver["name"] == "macos":
        validate_renderer_process_attribution(
            payload["artifacts"],
            expected_driver_pid=launched_pid,
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
    for section in payload["sections"]:
        _validate_section_inventory(
            section,
            expected_sections[section["id"]],
            label=f"{driver['name']} §{section['id']}",
        )
    return payload


def run_driver(
    driver: dict[str, Any],
    fixture: Path,
    profile: str,
    timeout: int,
) -> dict[str, Any]:
    with tempfile.NamedTemporaryFile(suffix=".json", delete=False) as output:
        output_path = Path(output.name)
    command = [
        *driver["command"],
        "--fixture",
        str(fixture),
        "--profile",
        profile,
        "--output",
        str(output_path),
    ]
    ensure_free_space(ROOT)
    try:
        try:
            result = run_managed_command(
                command,
                cwd=ROOT,
                timeout=timeout,
                label=f"{driver['name']} driver",
                poll_hook=lambda _process: ensure_free_space(ROOT),
            )
        except ManagedCommandTimeout as error:
            raise BenchmarkError(f"{driver['name']} timed out after {timeout}s") from error
        except ManagedCommandError as error:
            raise BenchmarkError(f"{driver['name']} process supervision failed: {error}") from error

        if result.returncode:
            detail = (result.stderr or result.stdout).strip()
            raise BenchmarkError(
                f"{driver['name']} failed ({result.returncode}): {detail[-4000:]}"
            )
        try:
            payload = json.loads(output_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            raise BenchmarkError(f"{driver['name']} did not write valid result JSON") from error
        validated = validate_driver_output(
            payload,
            driver,
            launched_pid=result.child_pid,
        )
        if driver["name"] == "macos":
            wait_for_renderer_processes_to_exit(
                validated["artifacts"],
                label="macos renderer candidates",
            )
        return validated
    finally:
        output_path.unlink(missing_ok=True)


def run_verification(
    spec: dict[str, Any],
    default_timeout: int,
) -> tuple[float, bool, str]:
    started = dt.datetime.now(dt.timezone.utc)
    timeout = spec.get("timeout", default_timeout)
    ensure_free_space(ROOT)
    try:
        result = run_managed_command(
            spec["command"],
            cwd=ROOT,
            timeout=timeout,
            label=spec["name"],
            poll_hook=lambda _process: ensure_free_space(ROOT),
        )
    except ManagedCommandTimeout:
        elapsed = (dt.datetime.now(dt.timezone.utc) - started).total_seconds() * 1_000
        return elapsed, False, f"timed out after {timeout}s"
    except ManagedCommandError as error:
        elapsed = (dt.datetime.now(dt.timezone.utc) - started).total_seconds() * 1_000
        return elapsed, False, f"process supervision failed: {error}"

    elapsed = (dt.datetime.now(dt.timezone.utc) - started).total_seconds() * 1_000
    combined = (result.stdout + "\n" + result.stderr).strip()
    missing_output = [
        required
        for required in spec["required_output"]
        if required not in combined
    ]
    passed = result.returncode == 0 and not missing_output
    detail_parts = [f"exit {result.returncode}"]
    if missing_output:
        detail_parts.append("missing required output: " + ", ".join(missing_output))
    if combined:
        detail_parts.append(combined[-500:])
    return elapsed, passed, "; ".join(detail_parts)


def validate_report(report: dict[str, Any], required: list[str]) -> None:
    validate_document(report, "report", "benchmark report")
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
        raise BenchmarkError("benchmark report sections: " + "; ".join(detail))

    sections_by_id = {section["id"]: section for section in report["sections"]}
    for section_id, section in sections_by_id.items():
        _validate_section_inventory(
            section,
            EXPECTED_REPORT_INVENTORY[section_id],
            label=f"report §{section_id}",
        )

    artifacts = report["driver_artifacts"]
    if "renderer_process_attribution" in artifacts["rust"]:
        raise BenchmarkError("rust report artifact must not claim renderer processes")
    validate_renderer_process_attribution(
        artifacts["macos"],
        expected_driver_pid=None,
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


def append_parity_assertion(
    sections: dict[str, dict[str, Any]],
    artifacts: dict[str, dict[str, Any]],
) -> None:
    rust = artifacts["rust"]
    macos = artifacts["macos"]
    canonical_keys = (
        "canonical_transaction_sha256",
        "canonical_transaction_bytes",
    )
    matches = all(rust[key] == macos[key] for key in canonical_keys)
    if matches:
        detail = (
            f"{rust['canonical_transaction_sha256']} / "
            f"{rust['canonical_transaction_bytes']} exact bytes"
        )
    else:
        detail = (
            "rust "
            f"{rust['canonical_transaction_sha256']} / {rust['canonical_transaction_bytes']} bytes; "
            "macos "
            f"{macos['canonical_transaction_sha256']} / {macos['canonical_transaction_bytes']} bytes"
        )
    sections["31.2"]["assertions"].append(
        {
            "id": "canonical_transaction_parity",
            "name": "renderer and serializer canonical transaction bytes match",
            "passed": matches,
            "detail": detail,
        }
    )


def over_2x(metric: dict[str, Any]) -> bool:
    target = metric.get("target")
    if target is None:
        return False
    value = metric["value"]
    if metric.get("target_direction", "max") == "max":
        return value > 2 * target
    return value < target / 2


def markdown(report: dict[str, Any]) -> str:
    lines = [
        "# SRUI benchmark report",
        "",
        f"- Generated: {report['generated_at']}",
        f"- Profile: {report['profile']}",
        f"- Host: {report['environment']['platform']} / {report['environment']['machine']}",
        f"- Fixture: {report['fixture']}",
        "",
    ]
    followups: list[str] = []
    correctness_failures: list[str] = []
    for section in sorted(report["sections"], key=lambda item: item["id"]):
        lines += [
            f"## §{section['id']} {section['name']}",
            "",
            "| Metric | Value | Statistic | Target |",
            "|---|---:|---|---:|",
        ]
        for metric in section["metrics"]:
            target = "—"
            if "target" in metric:
                operator = "≤" if metric.get("target_direction", "max") == "max" else "≥"
                target = f"{operator} {metric['target']:g} {metric['unit']}"
            marker = " **WARNING >2x**" if over_2x(metric) else ""
            lines.append(
                f"| {metric['name']} | {metric['value']:.4g} {metric['unit']}{marker} | "
                f"{metric['statistic']} | {target} |"
            )
            if over_2x(metric):
                followups.append(
                    f"§{section['id']} {metric['name']}: {metric['value']:.4g} "
                    f"{metric['unit']} vs target {metric['target']:g} {metric['unit']}"
                )
        lines += ["", "Assertions:", ""]
        for assertion in section["assertions"]:
            mark = "PASS" if assertion["passed"] else "FAIL"
            detail = f" — {assertion.get('detail', '')}" if assertion.get("detail") else ""
            lines.append(f"- **{mark}** {assertion['name']}{detail}")
            if not assertion["passed"]:
                correctness_failures.append(f"§{section['id']} {assertion['name']}")
        if section.get("notes"):
            lines += ["", "Notes:", ""]
            lines += [f"- {note}" for note in section["notes"]]
        lines.append("")
    lines += ["## Follow-up flags", ""]
    if followups:
        lines += [f"- **PERFORMANCE FOLLOW-UP (>2x):** {item}" for item in followups]
    else:
        lines.append("- No §23 target was missed by more than 2x.")
    if correctness_failures:
        lines += ["", *[f"- **CORRECTNESS FAILURE:** {item}" for item in correctness_failures]]
    lines.append("")
    return "\n".join(lines)


def failed_assertions(report: dict[str, Any]) -> list[tuple[str, str]]:
    return [
        (section["id"], assertion["name"])
        for section in report["sections"]
        for assertion in section["assertions"]
        if not assertion["passed"]
    ]


def ensure_baseline_recordable(report: dict[str, Any]) -> None:
    if report.get("profile") != "full":
        raise BenchmarkError(
            "refusing to overwrite the committed baseline with a non-full profile"
        )
    failures = failed_assertions(report)
    if failures:
        formatted = ", ".join(f"§{section} {name}" for section, name in failures)
        raise BenchmarkError(
            "refusing to overwrite the committed baseline with failed assertions: " + formatted
        )


def _stage_report_file(path: Path, content: str) -> Path:
    descriptor, raw_path = tempfile.mkstemp(
        dir=path.parent,
        prefix=f".{path.name}.stage-",
    )
    temporary = Path(raw_path)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as output:
            output.write(content)
            output.flush()
            os.fsync(output.fileno())
    except BaseException:
        temporary.unlink(missing_ok=True)
        raise
    return temporary


def _reserve_report_backup(path: Path) -> Path:
    descriptor, raw_path = tempfile.mkstemp(
        dir=path.parent,
        prefix=f".{path.name}.backup-",
    )
    os.close(descriptor)
    backup = Path(raw_path)
    backup.unlink()
    return backup


def _fsync_directory(path: Path) -> None:
    descriptor = os.open(path, os.O_RDONLY | getattr(os, "O_DIRECTORY", 0))
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def write_report(report: dict[str, Any], output_dir: Path, stem: str) -> tuple[Path, Path]:
    output_dir.mkdir(parents=True, exist_ok=True)
    json_path = output_dir / f"{stem}.json"
    markdown_path = output_dir / f"{stem}.md"
    paths_and_content = (
        (
            json_path,
            json.dumps(report, indent=2, sort_keys=True, allow_nan=False) + "\n",
        ),
        (markdown_path, markdown(report)),
    )
    staged: dict[Path, Path] = {}
    backups: dict[Path, Path] = {}
    installed: set[Path] = set()
    preserve_backups = False
    try:
        for path, content in paths_and_content:
            staged[path] = _stage_report_file(path, content)
            backups[path] = _reserve_report_backup(path)
        _fsync_directory(output_dir)

        with blocked_termination_signals():
            try:
                for path, _content in paths_and_content:
                    if path.exists():
                        os.replace(path, backups[path])
                for path, _content in paths_and_content:
                    os.replace(staged[path], path)
                    installed.add(path)
                _fsync_directory(output_dir)
            except BaseException as primary:
                rollback_errors: list[BaseException] = []
                for path, _content in reversed(paths_and_content):
                    try:
                        if backups[path].exists():
                            os.replace(backups[path], path)
                        elif path in installed:
                            path.unlink(missing_ok=True)
                    except BaseException as rollback_error:
                        rollback_errors.append(rollback_error)
                try:
                    _fsync_directory(output_dir)
                except BaseException as rollback_error:
                    rollback_errors.append(rollback_error)
                if rollback_errors:
                    preserve_backups = True
                    recovery = BenchmarkError(
                        "report rollback is incomplete; recovery backups retained at "
                        + ", ".join(str(path) for path in backups.values() if path.exists())
                    )
                    raise BaseExceptionGroup(
                        "report publication and rollback failed",
                        [primary, *rollback_errors, recovery],
                    )
                raise

            cleanup_errors: list[BaseException] = []
            for backup in backups.values():
                try:
                    backup.unlink(missing_ok=True)
                except BaseException as cleanup_error:
                    cleanup_errors.append(cleanup_error)
            try:
                _fsync_directory(output_dir)
            except BaseException as cleanup_error:
                cleanup_errors.append(cleanup_error)
            if cleanup_errors:
                preserve_backups = True
                raise BaseExceptionGroup(
                    "report published but backup cleanup failed",
                    cleanup_errors,
                )
    finally:
        cleanup_paths = list(staged.values())
        if not preserve_backups:
            cleanup_paths.extend(backups.values())
        for temporary in cleanup_paths:
            temporary.unlink(missing_ok=True)
    return markdown_path, json_path


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--profile", choices=("smoke", "full"), default="smoke")
    parser.add_argument("--output-dir", type=Path)
    parser.add_argument("--record-baseline", action="store_true")
    parser.add_argument("--timeout", type=int, default=600)
    args = parser.parse_args(argv)
    if args.record_baseline and args.profile != "full":
        parser.error("--record-baseline requires --profile full")
    if args.timeout <= 0:
        parser.error("--timeout must be positive")

    try:
        manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise BenchmarkError(f"cannot load benchmark manifest {MANIFEST}: {error}") from error
    validate_manifest(manifest)

    fixture = ROOT / manifest["fixture"]
    if not fixture.is_file():
        raise BenchmarkError(f"benchmark fixture does not exist: {fixture}")
    ensure_free_space(ROOT)

    sections: dict[str, dict[str, Any]] = {}
    driver_artifacts: dict[str, dict[str, Any]] = {}
    for driver in manifest["drivers"]:
        if driver.get("platform") and driver["platform"] != sys.platform:
            raise BenchmarkError(
                f"{driver['name']} requires {driver['platform']}; current platform is {sys.platform}"
            )
        payload = run_driver(driver, fixture, args.profile, args.timeout)
        driver_artifacts[driver["name"]] = payload["artifacts"]
        for section in payload["sections"]:
            section_id = section["id"]
            if section_id in sections:
                if sections[section_id]["name"] != section["name"]:
                    raise BenchmarkError(
                        f"drivers disagree on the name of section {section_id}"
                    )
                sections[section_id]["metrics"].extend(section["metrics"])
                sections[section_id]["assertions"].extend(section["assertions"])
                sections[section_id].setdefault("notes", []).extend(
                    section.get("notes", [])
                )
            else:
                sections[section_id] = section

    append_parity_assertion(sections, driver_artifacts)

    for verification in manifest["verification_commands"]:
        elapsed, passed, detail = run_verification(verification, args.timeout)
        section = sections[verification["section"]]
        section["metrics"].append(
            {
                "id": "production_reconnect_suite_ms",
                "name": verification["name"],
                "value": elapsed,
                "unit": "ms",
                "statistic": "wall",
            }
        )
        section["assertions"].append(
            {
                "id": "production_reconnect_suite",
                "name": verification["name"],
                "passed": passed,
                "detail": detail,
            }
        )

    report = {
        "schema_version": 1,
        "generated_at": dt.datetime.now(dt.timezone.utc).isoformat(),
        "profile": args.profile,
        "fixture": str(fixture.relative_to(ROOT)),
        "environment": {
            "platform": platform.platform(),
            "machine": platform.machine(),
            "python": platform.python_version(),
        },
        "driver_artifacts": driver_artifacts,
        "sections": list(sections.values()),
    }
    validate_report(report, manifest["required_sections"])

    failures = failed_assertions(report)
    if args.record_baseline:
        ensure_baseline_recordable(report)
        output_dir = ROOT / "benchmarks/reports"
        stem = "baseline"
    else:
        output_dir = args.output_dir or ROOT / ".benchmark-results"
        stem = "latest"

    ensure_free_space(ROOT)
    markdown_path, json_path = write_report(report, output_dir, stem)
    print(markdown_path)
    print(json_path)
    return 1 if failures else 0


def _raise_termination(signum: int, _frame: Any) -> None:
    raise TerminationRequested(signum)


@contextlib.contextmanager
def termination_handlers() -> Iterator[None]:
    previous = {
        signum: signal.getsignal(signum)
        for signum in (signal.SIGINT, signal.SIGTERM)
    }
    for signum in previous:
        signal.signal(signum, _raise_termination)
    try:
        yield
    finally:
        for signum, handler in previous.items():
            signal.signal(signum, handler)


def cli(argv: list[str] | None = None) -> int:
    try:
        with termination_handlers():
            return main(argv)
    except TerminationRequested as error:
        print(
            f"benchmark interrupted by {signal.Signals(error.signum).name}",
            file=sys.stderr,
        )
        return 128 + error.signum
    except KeyboardInterrupt:
        print("benchmark interrupted by SIGINT", file=sys.stderr)
        return 130
    except BenchmarkError as error:
        print(f"benchmark error: {error}", file=sys.stderr)
        return 2
    except BaseExceptionGroup as error:
        terminations = termination_exceptions(error)
        companions = non_termination_exceptions(error)
        if terminations:
            if companions:
                print(
                    "benchmark failures accompanying interruption: "
                    + "; ".join(
                        f"{type(companion).__name__}: {companion}"
                        for companion in companions
                    ),
                    file=sys.stderr,
                )
            first = terminations[0]
            if isinstance(first, TerminationRequested):
                print(
                    f"benchmark interrupted by {signal.Signals(first.signum).name}",
                    file=sys.stderr,
                )
                return 128 + first.signum
            if isinstance(first, KeyboardInterrupt):
                print("benchmark interrupted by SIGINT", file=sys.stderr)
                return 130
            raise first
        print(
            f"benchmark failed during process cleanup: {exception_group_detail(error)}",
            file=sys.stderr,
        )
        return 2


if __name__ == "__main__":
    raise SystemExit(cli())
