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
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time
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
CANONICAL_FIXTURE = "benchmarks/fixtures/coding-agent-ui.json"
EXPECTED_DRIVER_SECTIONS = {
    "rust": {"31.2", "31.5", "31.6"},
    "macos": {"31.1", "31.3", "31.4", "31.5", "31.6"},
}
EXPECTED_DRIVER_COMMANDS = {
    "rust": [
        "cargo", "run", "--quiet", "--release", "--locked", "--manifest-path",
        "server-rust/Cargo.toml", "-p", "srui-benchmark-driver", "--",
    ],
    "macos": [
        "swift", "run", "--disable-automatic-resolution", "--package-path",
        "client-macos", "-c", "release", "BenchmarkDriver",
    ],
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
ALLOCATION_CANDIDATES = ("srui", "webkit")
ALLOCATION_TARGET_ROLES = {
    "srui": ("host",),
    "webkit": ("host", "webcontent", "network", "gpu"),
}
ALLOCATION_MANDATORY_ROLES = {
    "srui": frozenset({"host"}),
    "webkit": frozenset({"host", "webcontent"}),
}
ALLOCATION_RECORDING_READINESS_BASES = frozenset({"darwin_notification"})
ALLOCATION_CAPTURE_SEMANTICS = {
    "capture_method": (
        "xctrace Allocations --attach per exact target process, exported "
        "through Statistics and Allocations List view details"
    ),
    "allocation_export_basis": (
        "Xcode Allocations view details: complete live Allocations List "
        "reconciled to All Heap & Anonymous VM Statistics"
    ),
    "allocation_timestamp_basis": (
        "Allocations List elapsed timestamp plus TOC start-date"
    ),
    "metric_semantics": (
        "heap and anonymous VM allocations created nominally inside the "
        "measured interval that remain live at capture end"
    ),
    "whole_trace_statistics_semantics": (
        "diagnostic whole-trace heap and anonymous VM aggregates including "
        "the attach-time live baseline; never interpreted as interval "
        "allocation traffic"
    ),
    "process_identity_basis": (
        "each segment TOC names exactly one attached PID equal to the "
        "requested PID; benchmark birth/liveness handshakes bound it"
    ),
}
ALLOCATION_STATISTICS_XPATH = (
    "/trace-toc/run[@number=\"1\"]/tracks/track[@name=\"Allocations\"]"
    "/details/detail[@name=\"Statistics\"]"
)
ALLOCATION_LIST_XPATH = (
    "/trace-toc/run[@number=\"1\"]/tracks/track[@name=\"Allocations\"]"
    "/details/detail[@name=\"Allocations List\"]"
)
ALLOCATION_METRIC_KINDS = (
    ("retained_allocations", "retained_allocations", "allocations"),
    ("retained_bytes", "retained_allocation_bytes", "bytes"),
    (
        "retained_allocations_lower_bound",
        "retained_allocations_lower_bound",
        "allocations",
    ),
    (
        "retained_allocations_upper_bound",
        "retained_allocations_upper_bound",
        "allocations",
    ),
    (
        "retained_bytes_lower_bound",
        "retained_allocation_bytes_lower_bound",
        "bytes",
    ),
    (
        "retained_bytes_upper_bound",
        "retained_allocation_bytes_upper_bound",
        "bytes",
    ),
    (
        "boundary_ambiguous_allocations",
        "boundary_ambiguous_allocations",
        "allocations",
    ),
    ("boundary_ambiguous_bytes", "boundary_ambiguous_bytes", "bytes"),
)
ALLOCATION_PROCESS_FIELDS = (
    "retained_allocations",
    "retained_bytes",
    "retained_allocations_lower_bound",
    "retained_allocations_upper_bound",
    "retained_bytes_lower_bound",
    "retained_bytes_upper_bound",
    "boundary_ambiguous_allocations",
    "boundary_ambiguous_bytes",
)
ALLOCATION_STATISTIC_FIELDS = (
    "persistent_allocations",
    "persistent_bytes",
    "transient_allocations",
    "transient_bytes",
    "total_allocations",
    "total_bytes",
    "event_count",
)
ALLOCATION_STATISTIC_GROUPS = (
    "heap_and_anonymous_vm",
    "heap",
    "anonymous_vm",
)
ALLOCATION_DIAGNOSTIC_FIELDS = (
    "anonymous_vm_persistent_allocations",
    "anonymous_vm_persistent_bytes",
    "vm_category_rows",
    "vm_category_bytes",
)
ALLOCATION_CAPTURE_TIMEOUTS = {"smoke": 650, "full": 1_020}
ALLOCATION_PROFILE_SAMPLE_COUNTS = {"smoke": 3, "full": 20}
PROFILE_DRIVER_ITERATIONS = {
    "smoke": {"rust": 25, "macos": 3},
    "full": {"rust": 500, "macos": 20},
}
PRODUCTION_CONFORMANCE_SAMPLE_COUNT = 9
METADATA_COMMAND_TIMEOUT_SECONDS = 10
LOCAL_INTERACTIONS = (
    "text_entry",
    "caret_movement",
    "text_selection",
    "ime_composition",
    "scrolling",
    "hover",
    "pressed",
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
                "srui.process_footprint_growth": ("MiB", ("p50",)),
                "webkit.first_paint": ("ms", ("p50", "p95")),
                "webkit.complete_paint": ("ms", ("p50", "p95")),
                "webkit.cpu": ("ms", ("p50",)),
                "webkit.host_retained_allocations": ("allocations", ("p50",)),
                "webkit.process_footprint_peak": ("MiB", ("max",)),
                "webkit.process_footprint_growth": ("MiB", ("p50",)),
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
                    f"cadence.{count}.{cadence}.visible": ("ms", ("sample",))
                    for count in (1, 100, 1000)
                    for cadence in (60, 120, 144, 240)
                },
                **{
                    f"cadence.{count}.{cadence}.bytes": ("bytes", ("exact",))
                    for count in (1, 100, 1000)
                    for cadence in (60, 120, 144, 240)
                },
                **{
                    f"cadence.{count}.{cadence}.messages": (
                        "messages",
                        ("exact",),
                    )
                    for count in (1, 100, 1000)
                    for cadence in (60, 120, 144, 240)
                },
                **{
                    f"cadence.{count}.{cadence}.{direction}_bytes": (
                        "bytes",
                        ("exact",),
                    )
                    for count in (1, 100, 1000)
                    for cadence in (60, 120, 144, 240)
                    for direction in ("inbound", "outbound")
                },
                **{
                    f"cadence.{count}.{cadence}.{direction}_messages": (
                        "messages",
                        ("exact",),
                    )
                    for count in (1, 100, 1000)
                    for cadence in (60, 120, 144, 240)
                    for direction in ("inbound", "outbound")
                },
                **{
                    f"cadence.{count}.{cadence}.repaints": (
                        "repaints",
                        ("exact",),
                    )
                    for count in (1, 100, 1000)
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
                "production_text_edit_framed",
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
                "pre_receipt_event_replay": ("ms", ("p50", "p95")),
                "mid_resource_recovery": ("ms", ("p50", "p95")),
                "superseded_response": ("ms", ("p50", "p95")),
                "active_response": ("ms", ("p50", "p95")),
            },
            (
                "pre_receipt_pending_replay",
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
                "standalone_terminal.decode_visible": ("ms", DISTRIBUTION),
                "standalone_terminal.draw_only": ("ms", DISTRIBUTION),
                "standalone_terminal.raster_completions": (
                    "frames",
                    ("exact",),
                ),
                "terminal_display.embedded_to_standalone_decode_ratio": (
                    "ratio",
                    ("p50",),
                ),
                "terminal_display.embedded_to_standalone_draw_ratio": (
                    "ratio",
                    ("p50",),
                ),
            },
            (
                "terminal_offsets_exact",
                "standalone_terminal_offsets_exact",
                "terminal_display_draw_completion",
                "terminal_display_equivalent",
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
    for candidate in ALLOCATION_CANDIDATES:
        for _sample_field, metric_kind, unit in ALLOCATION_METRIC_KINDS:
            for statistic in DISTRIBUTION:
                merged["31.1"]["metrics"][
                    (f"{candidate}.{metric_kind}", statistic)
                ] = (unit, None, None)
    merged["31.1"]["assertions"].update(
        {"allocation_trace_attributed", "candidate_failure_cleanup"}
    )
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


def _build_metric_display_names() -> dict[str, str]:
    names = {
        "abstract_state_generation_ms": "abstract state generation",
        "protobuf_serialization_ms": "protobuf serialization",
        "serialized_transaction_bytes": "serialized transaction size",
        "disconnect_before_event_receipt_ms": (
            "disconnect immediately before event receipt"
        ),
        "event_to_settled_side_effect_ms": (
            "event receipt through settled side effect"
        ),
        "cached_duplicate_response_ms": "in-process cached DUPLICATE response",
        "lost_ack_wire_duplicate_ms": (
            "lost ACK wire reconnect through DUPLICATE acknowledgement"
        ),
        "mid_resource_reconnect_ms": "mid-resource reconnect and exact replay",
        "mid_transaction_codec_replay_ms": (
            "mid-transaction frame discard and atomic replay"
        ),
        "mid_transaction_wire_replay_ms": (
            "mid-transaction wire disconnect and exact atomic replay"
        ),
        "partial_event_wire_replay_ms": (
            "partial EVENT disconnect and one processed replay"
        ),
        "resume_beyond_retention_ms": "resume beyond journal retention",
        "resume_within_retention_ms": "resume within journal retention",
        "embedded_pty_interaction_ms": (
            "embedded SRUI PTY exact ANSI capture and framing"
        ),
        "standalone_pty_interaction_ms": "standalone PTY exact ANSI interaction",
        "terminal_retention_loss_ms": (
            "terminal reconnect retention-loss decision"
        ),
        "terminal_payload_bytes": "terminal payload",
        "embedded_terminal_frame_count": "embedded terminal frame count",
        "srui.cpu": "SRUI candidate process CPU time",
        "srui.host_retained_allocations": (
            "SRUI host retained allocation delta"
        ),
        "srui.process_footprint_peak": (
            "SRUI maximum concurrently sampled process footprint"
        ),
        "srui.process_footprint_growth": (
            "SRUI host allocated footprint growth"
        ),
        "webkit.cpu": "WKWebView host plus attributed helper CPU time",
        "webkit.host_retained_allocations": (
            "WKWebView host retained allocation delta"
        ),
        "webkit.process_footprint_peak": (
            "WKWebView maximum concurrently sampled host-plus-helper footprint"
        ),
        "webkit.process_footprint_growth": (
            "WKWebView host plus helpers allocated footprint growth"
        ),
        "representation.srui_bytes": "SRUI representation",
        "representation.html_bytes": "HTML representation",
        "paint.capture_authorization": "screen capture authorization",
        "idle.bytes": "settled idle SRUI wire bytes",
        "idle.messages": "settled idle SRUI message count",
        "display.frame_budget": "local display frame budget",
        "impairment.bandwidth_transfer": (
            "1MiB/s bandwidth-limited production event"
        ),
        "impairment.bandwidth_delivered_bytes": (
            "bandwidth-limited delivered bytes"
        ),
        "impairment.loss_attempts": "deterministic production loss attempts",
        "impairment.loss_delivered_messages": (
            "deterministic production loss delivered messages"
        ),
        "impairment.interruption_detection": (
            "controlled production interruption detection"
        ),
        "session_wire.bytes": "measured production session wire bytes",
        "session_wire.messages": "measured production session wire messages",
        "local_rtt_delta": "maximum RTT-induced local latency delta",
        "pre_receipt_event_replay": "pre-receipt retained-event replay",
        "mid_resource_recovery": "mid-resource reconnect recovery",
        "superseded_response": "superseded resume response handling",
        "active_response": "active resume response handling",
        "client_terminal.decode_visible": (
            "embedded SRUI Terminal decode-to-visible"
        ),
        "client_terminal.draw_only": "embedded SRUI Terminal draw-only",
        "client_terminal.frame_bytes": "client terminal framed envelope",
        "client_terminal.raster_completions": (
            "embedded terminal draw completions"
        ),
        "standalone_terminal.decode_visible": (
            "standalone TerminalSession and TerminalView decode-to-visible"
        ),
        "standalone_terminal.draw_only": "standalone TerminalView draw-only",
        "standalone_terminal.raster_completions": (
            "standalone terminal draw completions"
        ),
        "terminal_display.embedded_to_standalone_decode_ratio": (
            "embedded-to-standalone terminal decode-to-visible"
        ),
        "terminal_display.embedded_to_standalone_draw_ratio": (
            "embedded-to-standalone terminal draw-only"
        ),
        "production_reconnect_suite_ms": "production reconnect boundary suite",
    }
    allocation_names = {
        "retained_allocations": "interval-retained allocations",
        "retained_allocation_bytes": "interval-retained allocation bytes",
        "retained_allocations_lower_bound": (
            "definitely interval-retained allocations"
        ),
        "retained_allocations_upper_bound": (
            "possibly interval-retained allocations"
        ),
        "retained_allocation_bytes_lower_bound": (
            "definitely interval-retained bytes"
        ),
        "retained_allocation_bytes_upper_bound": (
            "possibly interval-retained bytes"
        ),
        "boundary_ambiguous_allocations": (
            "timestamp-boundary-ambiguous allocations"
        ),
        "boundary_ambiguous_bytes": "timestamp-boundary-ambiguous bytes",
    }
    for candidate, display_name in (
        ("srui", "SRUI"),
        ("webkit", "WKWebView control"),
    ):
        for kind, suffix in allocation_names.items():
            names[f"{candidate}.{kind}"] = f"{display_name} {suffix}"
    for count in (1, 100, 1000):
        names[f"updates.{count}.semantic"] = (
            f"{count} updates semantic decode/apply"
        )
        names[f"updates.{count}.visible"] = f"{count} updates decode-to-visible"
        names[f"updates.{count}.bytes"] = f"{count} updates wire bytes"
        names[f"updates.{count}.messages"] = f"{count} updates message count"
        for cadence in (60, 120, 144, 240):
            prefix = f"cadence.{count}.{cadence}"
            display_prefix = f"{count} updates at {cadence}Hz"
            names[f"{prefix}.visible"] = f"{display_prefix} decode-to-visible"
            names[f"{prefix}.bytes"] = f"{display_prefix} total SRUI wire bytes"
            names[f"{prefix}.messages"] = (
                f"{display_prefix} total SRUI message count"
            )
            names[f"{prefix}.inbound_bytes"] = (
                f"{display_prefix} inbound TRANSACTION bytes"
            )
            names[f"{prefix}.inbound_messages"] = (
                f"{display_prefix} inbound TRANSACTION message count"
            )
            names[f"{prefix}.outbound_bytes"] = (
                f"{display_prefix} outbound EVENT bytes"
            )
            names[f"{prefix}.outbound_messages"] = (
                f"{display_prefix} outbound EVENT message count"
            )
            names[f"{prefix}.repaints"] = (
                f"{display_prefix} synthetic change-gated repaint count"
            )
    for interaction in LOCAL_INTERACTIONS:
        display_name = interaction.replace("_", " ")
        for rtt in (0, 100, 300, 600):
            names[f"interaction.{interaction}.rtt.{rtt}"] = (
                f"{display_name} at {rtt}ms RTT"
            )
    for rtt in (0, 100, 300, 600):
        names[f"server_feedback.rtt.{rtt}"] = (
            f"server-dependent input-to-visible at {rtt}ms RTT"
        )
    return names


EXPECTED_METRIC_DISPLAY_NAMES = _build_metric_display_names()
PROFILED_METRIC_DISPLAY_NAMES = {
    "srui.first_paint": {
        "smoke": "SRUI first offscreen raster fallback",
        "full": "SRUI first on-screen paint crossing display refresh",
    },
    "srui.complete_paint": {
        "smoke": "SRUI complete offscreen raster fallback",
        "full": "SRUI complete on-screen paint crossing display refresh",
    },
    "webkit.first_paint": {
        "smoke": "WKWebView first offscreen snapshot fallback",
        "full": "WKWebView first on-screen paint crossing display refresh",
    },
    "webkit.complete_paint": {
        "smoke": "WKWebView complete offscreen snapshot fallback",
        "full": "WKWebView complete on-screen paint crossing display refresh",
    },
}
_EXPECTED_METRIC_IDS = {
    metric_id
    for section in EXPECTED_REPORT_INVENTORY.values()
    for metric_id, _statistic in section["metrics"]
}
if _EXPECTED_METRIC_IDS != (
    set(EXPECTED_METRIC_DISPLAY_NAMES) | set(PROFILED_METRIC_DISPLAY_NAMES)
):
    raise ValueError(
        "benchmark metric display-name contract does not exactly cover inventory"
    )


def expected_metric_display_name(metric_id: str, profile: str) -> str:
    profiled = PROFILED_METRIC_DISPLAY_NAMES.get(metric_id)
    if profiled is not None:
        return profiled[profile]
    return EXPECTED_METRIC_DISPLAY_NAMES[metric_id]


class BenchmarkError(RuntimeError):
    pass


def expected_driver_sample_counts(
    driver_name: str,
    profile: str,
) -> dict[str, dict[str, int]]:
    try:
        iterations = PROFILE_DRIVER_ITERATIONS[profile][driver_name]
    except KeyError as error:
        raise BenchmarkError(
            f"unsupported benchmark sample-count contract: {driver_name}/{profile}"
        ) from error

    if driver_name == "rust":
        wire_iterations = min(iterations, 50)
        terminal_iterations = min(iterations, 20)
        return {
            "31.2": {
                "rust.generation": iterations,
                "rust.serialization": iterations,
            },
            "31.5": {
                "rust.mid_resource": iterations,
                "rust.mid_transaction_codec": iterations,
                "rust.event_side_effect": iterations,
                "rust.cached_duplicate": iterations,
                "rust.resume_within_retention": iterations,
                "rust.resume_beyond_retention": iterations,
                "rust.pre_receipt_wire": wire_iterations,
                "rust.lost_ack_wire": wire_iterations,
                "rust.mid_transaction_wire": wire_iterations,
                "rust.partial_event_wire": wire_iterations,
            },
            "31.6": {
                "rust.standalone_pty": terminal_iterations,
                "rust.embedded_pty": terminal_iterations,
                "rust.ring_exhaustion": 1,
            },
        }

    interaction_iterations = max(5, iterations)
    feedback_iterations = max(3, min(7, interaction_iterations))
    reconnect_iterations = max(2, min(5, iterations))
    terminal_iterations = max(10, iterations)
    mutation_counts = {
        f"macos.mutation.{count}": iterations
        for count in (1, 100, 1_000)
    }
    cadence_counts = {
        f"macos.cadence.{count}.{cadence}": 1
        for count in (1, 100, 1_000)
        for cadence in (60, 120, 144, 240)
    }
    cadence_event_counts = {
        f"macos.cadence.events.{count}.{cadence}": 3
        for count in (1, 100, 1_000)
        for cadence in (60, 120, 144, 240)
    }
    interaction_counts = {
        f"macos.interaction.{interaction}.rtt.{rtt}": interaction_iterations
        for interaction in LOCAL_INTERACTIONS
        for rtt in (0, 100, 300, 600)
    }
    feedback_counts = {
        f"macos.server_feedback.rtt.{rtt}": feedback_iterations
        for rtt in (0, 100, 300, 600)
    }
    return {
        "31.1": {
            "macos.srui.render": iterations,
            "macos.webkit.render": iterations,
        },
        "31.3": mutation_counts | cadence_counts | cadence_event_counts,
        "31.4": interaction_counts
        | feedback_counts
        | {
            "macos.local_held_response":
                interaction_iterations * len(LOCAL_INTERACTIONS) * 4,
            "macos.bandwidth": 3,
            "macos.loss": 1,
            "macos.interruption": 1,
        },
        "31.5": {
            "macos.pre_receipt": reconnect_iterations,
            "macos.mid_resource": reconnect_iterations,
            "macos.superseded_response": reconnect_iterations,
            "macos.active_response": reconnect_iterations,
        },
        "31.6": {
            "macos.embedded_display": terminal_iterations,
            "macos.standalone_display": terminal_iterations,
        },
    }


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
    merged["31.1"].update(
        {
            f"runner.xctrace.{candidate}": ALLOCATION_PROFILE_SAMPLE_COUNTS[profile]
            for candidate in ALLOCATION_CANDIDATES
        }
    )
    merged["31.5"]["runner.production_conformance"] = (
        PRODUCTION_CONFORMANCE_SAMPLE_COUNT
    )
    return merged


def _checked_metadata_command(
    command: list[str],
    *,
    runner: Any = subprocess.run,
    allow_empty: bool = False,
) -> str:
    try:
        result = runner(
            command,
            cwd=ROOT,
            capture_output=True,
            text=True,
            check=True,
            timeout=METADATA_COMMAND_TIMEOUT_SECONDS,
        )
    except (OSError, subprocess.SubprocessError) as error:
        raise BenchmarkError(
            f"environment metadata command failed: {' '.join(command)}"
        ) from error
    stdout = (result.stdout or "").strip()
    stderr = (result.stderr or "").strip()
    output = "\n".join(part for part in (stdout, stderr) if part)
    if not output and not allow_empty:
        raise BenchmarkError(
            f"environment metadata command returned no data: {' '.join(command)}"
        )
    return output


def benchmark_environment(*, runner: Any = subprocess.run) -> dict[str, Any]:
    platform_name = platform.platform()
    machine = platform.machine()
    python = platform.python_version()
    if not platform_name or not machine or not python:
        raise BenchmarkError("Python platform metadata is incomplete")

    chip = _checked_metadata_command(
        ["/usr/sbin/sysctl", "-n", "machdep.cpu.brand_string"],
        runner=runner,
    )
    physical_ram_text = _checked_metadata_command(
        ["/usr/sbin/sysctl", "-n", "hw.memsize"],
        runner=runner,
    )
    try:
        physical_ram_bytes = int(physical_ram_text)
    except ValueError as error:
        raise BenchmarkError(
            f"physical RAM metadata is not an integer: {physical_ram_text!r}"
        ) from error
    if physical_ram_bytes <= 0:
        raise BenchmarkError("physical RAM metadata must be positive")

    git_commit = _checked_metadata_command(
        ["git", "rev-parse", "HEAD"],
        runner=runner,
    )
    if re.fullmatch(r"[0-9a-f]{40,64}", git_commit) is None:
        raise BenchmarkError(f"git commit metadata is invalid: {git_commit!r}")
    git_status = _checked_metadata_command(
        ["git", "status", "--porcelain=v1", "--untracked-files=normal"],
        runner=runner,
        allow_empty=True,
    )
    return {
        "platform": platform_name,
        "machine": machine,
        "python": python,
        "chip": chip,
        "physical_ram_bytes": physical_ram_bytes,
        "xcode": _checked_metadata_command(
            ["/usr/bin/xcodebuild", "-version"],
            runner=runner,
        ),
        "swift": _checked_metadata_command(
            ["/usr/bin/swift", "--version"],
            runner=runner,
        ),
        "rust": _checked_metadata_command(
            ["rustc", "--version"],
            runner=runner,
        ),
        "git_commit": git_commit,
        "git_dirty": bool(git_status),
    }


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


def _positive_integer(value: Any) -> bool:
    return not isinstance(value, bool) and isinstance(value, int) and value > 0


def _validated_role_aliases(
    value: Any,
    *,
    roles_by_name: dict[str, dict[str, Any]],
    expected_roles: tuple[str, ...],
    label: str,
) -> list[dict[str, Any]]:
    if not isinstance(value, list):
        raise BenchmarkError(f"{label} role alias topology is invalid")
    expected_role_set = set(expected_roles)
    grouped_roles: set[str] = set()
    aliased_roles: set[str] = set()
    identities_by_pass = {role: set() for role in expected_roles}
    compact_aliases: list[dict[str, Any]] = []
    required_keys = {
        "canonical_target_role",
        "aliased_target_roles",
        "pass_advertised_identities",
    }
    advertised_keys = {"target_role", "pid", "birth_unix_ns"}

    for item in value:
        if not isinstance(item, dict) or set(item) != required_keys:
            raise BenchmarkError(f"{label} role alias topology is invalid")
        canonical = item["canonical_target_role"]
        aliases = item["aliased_target_roles"]
        if (
            not isinstance(canonical, str)
            or canonical not in expected_role_set
            or not isinstance(aliases, list)
            or not aliases
            or any(
                not isinstance(role, str) or role not in expected_role_set
                for role in aliases
            )
            or len(aliases) != len(set(aliases))
            or canonical in aliases
        ):
            raise BenchmarkError(f"{label} role alias topology is invalid")
        group = [canonical, *aliases]
        group_set = set(group)
        if (
            group != [role for role in expected_roles if role in group_set]
            or grouped_roles & group_set
        ):
            raise BenchmarkError(f"{label} role alias topology is invalid")
        grouped_roles.update(group_set)

        canonical_interval = roles_by_name[canonical]
        if (
            canonical_interval["target_present"] is not True
            or canonical_interval["contribution_included"] is not True
            or "alias_of_target_role" in canonical_interval
        ):
            raise BenchmarkError(f"{label} role alias topology is invalid")
        for alias in aliases:
            interval = roles_by_name[alias]
            if (
                interval["target_present"] is not True
                or interval["contribution_included"] is not False
                or interval.get("alias_of_target_role") != canonical
            ):
                raise BenchmarkError(f"{label} role alias topology is invalid")
            aliased_roles.add(alias)

        advertised = item["pass_advertised_identities"]
        if not isinstance(advertised, list) or len(advertised) != len(expected_roles):
            raise BenchmarkError(f"{label} role alias topology is invalid")
        by_pass: dict[str, dict[str, Any]] = {}
        for identity in advertised:
            if (
                not isinstance(identity, dict)
                or set(identity) != advertised_keys
                or not isinstance(identity["target_role"], str)
                or identity["target_role"] not in expected_role_set
                or not _positive_integer(identity["pid"])
                or not _positive_integer(identity["birth_unix_ns"])
                or identity["target_role"] in by_pass
            ):
                raise BenchmarkError(f"{label} role alias topology is invalid")
            by_pass[identity["target_role"]] = identity
        if set(by_pass) != expected_role_set:
            raise BenchmarkError(f"{label} role alias topology is invalid")

        for role in group:
            interval = roles_by_name[role]
            own_pass_identity = by_pass[role]
            if (
                own_pass_identity["pid"] != interval["target_pid"]
                or own_pass_identity["birth_unix_ns"]
                != interval["target_birth_unix_ns"]
            ):
                raise BenchmarkError(f"{label} role alias topology is invalid")
        for pass_role in expected_roles:
            identity = by_pass[pass_role]
            identity_key = (identity["pid"], identity["birth_unix_ns"])
            if identity_key in identities_by_pass[pass_role]:
                raise BenchmarkError(f"{label} role alias topology is invalid")
            identities_by_pass[pass_role].add(identity_key)

        compact_aliases.append(
            {
                "canonical_target_role": canonical,
                "aliased_target_roles": list(aliases),
                "pass_advertised_identities": [
                    {
                        "target_role": role,
                        "pid": by_pass[role]["pid"],
                        "birth_unix_ns": by_pass[role]["birth_unix_ns"],
                    }
                    for role in expected_roles
                ],
            }
        )

    expected_aliases = {
        role
        for role, interval in roles_by_name.items()
        if interval["target_present"] and not interval["contribution_included"]
    }
    if aliased_roles != expected_aliases:
        raise BenchmarkError(f"{label} role alias topology is invalid")
    return compact_aliases


def fold_allocation_summary(
    summary: Any,
    *,
    profile: str,
) -> tuple[list[dict[str, Any]], dict[str, Any], dict[str, Any]]:
    if not isinstance(summary, dict) or summary.get("schema_version") != 3:
        raise BenchmarkError("allocation capture summary has an unsupported schema")
    if summary.get("capture_scope") != "exact_processes":
        raise BenchmarkError("allocation capture did not profile exact processes")
    required_strings = tuple(ALLOCATION_CAPTURE_SEMANTICS)
    if any(
        summary.get(key) != expected
        for key, expected in ALLOCATION_CAPTURE_SEMANTICS.items()
    ):
        raise BenchmarkError(
            "allocation capture summary does not match the interval-retained "
            "measurement semantics"
        )
    if (
        not _positive_integer(summary.get("allocation_rows"))
        or not isinstance(summary.get("allocation_list_bytes"), int)
        or isinstance(summary.get("allocation_list_bytes"), bool)
        or summary["allocation_list_bytes"] < 0
        or summary.get("allocation_list_reconciled") is not True
    ):
        raise BenchmarkError("allocation capture has no reconciled live-list evidence")

    def nonnegative(value: Any) -> bool:
        return not isinstance(value, bool) and isinstance(value, int) and value >= 0

    if any(
        not nonnegative(summary.get(field))
        for field in ALLOCATION_DIAGNOSTIC_FIELDS
    ):
        raise BenchmarkError("allocation capture has invalid VM diagnostics")

    statistic_fields = set(ALLOCATION_STATISTIC_FIELDS)
    statistic_groups = set(ALLOCATION_STATISTIC_GROUPS)

    def validate_statistics_row(value: Any, label: str) -> dict[str, int]:
        if (
            not isinstance(value, dict)
            or set(value) != statistic_fields
            or any(not nonnegative(value[field]) for field in statistic_fields)
            or value["persistent_allocations"] + value["transient_allocations"]
            != value["total_allocations"]
            or value["persistent_bytes"] + value["transient_bytes"]
            != value["total_bytes"]
        ):
            raise BenchmarkError(f"{label} allocation Statistics do not reconcile")
        return value

    def validate_statistics(
        value: Any,
        label: str,
    ) -> dict[str, dict[str, int]]:
        if not isinstance(value, dict) or set(value) != statistic_groups:
            raise BenchmarkError(f"{label} allocation Statistics are incomplete")
        rows = {
            group: validate_statistics_row(value[group], f"{label} {group}")
            for group in ALLOCATION_STATISTIC_GROUPS
        }
        if any(
            rows["heap_and_anonymous_vm"][field]
            != rows["heap"][field] + rows["anonymous_vm"][field]
            for field in ALLOCATION_STATISTIC_FIELDS
        ):
            raise BenchmarkError(
                f"{label} heap and anonymous VM Statistics do not reconcile"
            )
        return rows
    instrumentation = summary.get("instrumentation")
    if (
        not isinstance(instrumentation, dict)
        or instrumentation.get("strategy")
        != "private_copy_ad_hoc_codesigned_for_instrumentation"
        or instrumentation.get("source_binary_unchanged") is not True
        or instrumentation.get("signature_verified") is not True
        or instrumentation.get("entitlements")
        != {"com.apple.security.get-task-allow": True}
    ):
        raise BenchmarkError("allocation capture has invalid signing evidence")
    readiness_bases = summary.get("recording_readiness_bases")
    if (
        not isinstance(readiness_bases, list)
        or not readiness_bases
        or len(readiness_bases) != len(set(readiness_bases))
        or not set(readiness_bases) <= ALLOCATION_RECORDING_READINESS_BASES
    ):
        raise BenchmarkError("allocation capture has invalid recording readiness")
    if (
        not _positive_integer(summary.get("trace_start_timestamp_resolution_ns"))
        or not _positive_integer(
            summary.get("allocation_list_timestamp_resolution_ns")
        )
        or summary.get("timestamp_boundary_uncertainty_ns")
        != summary["trace_start_timestamp_resolution_ns"]
        + summary["allocation_list_timestamp_resolution_ns"]
    ):
        raise BenchmarkError("allocation capture has invalid timestamp precision")
    xctrace_environment = summary.get("xctrace_environment")
    if (
        not isinstance(xctrace_environment, dict)
        or any(
            not isinstance(xctrace_environment.get(key), list)
            or not xctrace_environment[key]
            or any(
                not isinstance(value, str) or not value
                for value in xctrace_environment[key]
            )
            for key in ("instruments_versions", "platforms", "os_versions")
        )
        or xctrace_environment.get("statistics_xpath")
        != ALLOCATION_STATISTICS_XPATH
        or xctrace_environment.get("allocations_list_xpath")
        != ALLOCATION_LIST_XPATH
    ):
        raise BenchmarkError("allocation capture has invalid xctrace environment")

    target_captures = summary.get("target_captures")
    if (
        not isinstance(target_captures, list)
        or not target_captures
        or summary.get("target_capture_count") != len(target_captures)
    ):
        raise BenchmarkError("allocation capture has incomplete target captures")
    aggregate_statistics = validate_statistics(
        summary.get("whole_trace_statistics_aggregate"), "aggregate whole-trace"
    )
    computed_statistics = {
        group: {field: 0 for field in ALLOCATION_STATISTIC_FIELDS}
        for group in ALLOCATION_STATISTIC_GROUPS
    }
    computed_rows = 0
    computed_list_bytes = 0
    computed_diagnostics = {field: 0 for field in ALLOCATION_DIAGNOSTIC_FIELDS}
    for capture in target_captures:
        if (
            not isinstance(capture, dict)
            or capture.get("candidate") not in ALLOCATION_CANDIDATES
            or capture.get("target_role")
            not in ALLOCATION_TARGET_ROLES[capture.get("candidate")]
            or not _positive_integer(capture.get("target_pid"))
            or not _positive_integer(capture.get("target_birth_unix_ns"))
            or not _positive_integer(capture.get("started_unix_ns"))
            or not _positive_integer(capture.get("ended_unix_ns"))
            or capture["started_unix_ns"] >= capture["ended_unix_ns"]
            or not _positive_integer(capture.get("allocation_rows"))
            or not nonnegative(capture.get("allocation_list_bytes"))
            or any(
                not nonnegative(capture.get(field))
                for field in ALLOCATION_DIAGNOSTIC_FIELDS
            )
        ):
            raise BenchmarkError("allocation target capture is invalid")
        statistics = validate_statistics(
            capture.get("whole_trace_statistics"), "target whole-trace"
        )
        combined = statistics["heap_and_anonymous_vm"]
        anonymous_vm = statistics["anonymous_vm"]
        if (
            combined["persistent_allocations"] != capture["allocation_rows"]
            or combined["persistent_bytes"] != capture["allocation_list_bytes"]
            or anonymous_vm["persistent_allocations"]
            != capture["anonymous_vm_persistent_allocations"]
            or anonymous_vm["persistent_bytes"]
            != capture["anonymous_vm_persistent_bytes"]
            or capture["vm_category_rows"] > capture["allocation_rows"]
            or capture["vm_category_bytes"] > capture["allocation_list_bytes"]
        ):
            raise BenchmarkError(
                "target complete live list does not match Statistics diagnostics"
            )
        computed_rows += capture["allocation_rows"]
        computed_list_bytes += capture["allocation_list_bytes"]
        for field in ALLOCATION_DIAGNOSTIC_FIELDS:
            computed_diagnostics[field] += capture[field]
        for group in ALLOCATION_STATISTIC_GROUPS:
            for field in ALLOCATION_STATISTIC_FIELDS:
                computed_statistics[group][field] += statistics[group][field]
    if (
        computed_rows != summary["allocation_rows"]
        or computed_list_bytes != summary["allocation_list_bytes"]
        or any(
            computed_diagnostics[field] != summary[field]
            for field in ALLOCATION_DIAGNOSTIC_FIELDS
        )
        or computed_statistics != aggregate_statistics
    ):
        raise BenchmarkError("top-level allocation evidence does not reconcile")

    candidate_items = summary.get("candidate_processes")
    if (
        not isinstance(candidate_items, list)
        or len(candidate_items) != len(ALLOCATION_CANDIDATES)
        or any(not isinstance(item, dict) for item in candidate_items)
    ):
        raise BenchmarkError("allocation capture has no candidate_processes")
    by_candidate = {item.get("candidate"): item for item in candidate_items}
    if set(by_candidate) != set(ALLOCATION_CANDIDATES):
        raise BenchmarkError(
            "allocation capture must contain exactly srui and webkit candidates"
        )

    process_fields = ALLOCATION_PROCESS_FIELDS

    def validate_process_total(value: Any, label: str) -> dict[str, Any]:
        if (
            not isinstance(value, dict)
            or not _positive_integer(value.get("pid"))
            or not _positive_integer(value.get("birth_unix_ns"))
            or not _positive_integer(value.get("observed_alive_through_unix_ns"))
            or value["birth_unix_ns"] > value["observed_alive_through_unix_ns"]
            or not isinstance(value.get("names"), list)
            or not value["names"]
            or any(not isinstance(name, str) or not name for name in value["names"])
            or any(not nonnegative(value.get(field)) for field in process_fields)
            or value["retained_allocations_lower_bound"]
            > value["retained_allocations"]
            or value["retained_allocations"]
            > value["retained_allocations_upper_bound"]
            or value["retained_bytes_lower_bound"] > value["retained_bytes"]
            or value["retained_bytes"] > value["retained_bytes_upper_bound"]
            or value["boundary_ambiguous_allocations"]
            != value["retained_allocations_upper_bound"]
            - value["retained_allocations_lower_bound"]
            or value["boundary_ambiguous_bytes"]
            != value["retained_bytes_upper_bound"]
            - value["retained_bytes_lower_bound"]
        ):
            raise BenchmarkError(f"{label} process-retained total is invalid")
        return value

    metrics: list[dict[str, Any]] = []
    candidate_totals: list[dict[str, Any]] = []
    sample_evidence: list[dict[str, Any]] = []
    observed_readiness_bases: set[str] = set()
    sample_counts: set[int] = set()
    for candidate in ALLOCATION_CANDIDATES:
        item = by_candidate[candidate]
        expected_roles = ALLOCATION_TARGET_ROLES[candidate]
        mandatory_roles = ALLOCATION_MANDATORY_ROLES[candidate]
        if item.get("measurement_mode") != "equivalent_exact_process_role_passes":
            raise BenchmarkError(f"{candidate} allocation measurement mode is invalid")
        samples = item.get("measurement_samples")
        sample_count = item.get("measurement_sample_count")
        if (
            not isinstance(samples, list)
            or not samples
            or not _positive_integer(sample_count)
            or sample_count != len(samples)
        ):
            raise BenchmarkError(f"{candidate} allocation samples are invalid")
        sample_counts.add(sample_count)
        host_pid = item.get("host_pid")
        helper_pids = item.get("helper_pids")
        if (
            not _positive_integer(host_pid)
            or not isinstance(helper_pids, list)
            or any(not _positive_integer(pid) for pid in helper_pids)
            or len(helper_pids) != len(set(helper_pids))
            or (candidate == "srui" and helper_pids)
            or (candidate == "webkit" and not helper_pids)
        ):
            raise BenchmarkError(f"{candidate} process attribution is invalid")
        claimed_pid_values = {host_pid, *helper_pids}
        identities = item.get("process_identities")
        claimed_identity_alive: dict[tuple[int, int], int] = {}
        if not isinstance(identities, list) or not identities:
            raise BenchmarkError(f"{candidate} exact identities are incomplete")
        for identity in identities:
            if (
                not isinstance(identity, dict)
                or set(identity)
                != {
                    "pid",
                    "birth_unix_ns",
                    "observed_alive_through_unix_ns",
                }
                or not _positive_integer(identity["pid"])
                or not _positive_integer(identity["birth_unix_ns"])
                or not _positive_integer(identity["observed_alive_through_unix_ns"])
                or identity["birth_unix_ns"]
                > identity["observed_alive_through_unix_ns"]
            ):
                raise BenchmarkError(f"{candidate} exact identities are incomplete")
            identity_key = (identity["pid"], identity["birth_unix_ns"])
            if identity_key in claimed_identity_alive:
                raise BenchmarkError(f"{candidate} exact identities are incomplete")
            claimed_identity_alive[identity_key] = identity[
                "observed_alive_through_unix_ns"
            ]
        if {pid for pid, _birth in claimed_identity_alive} != claimed_pid_values:
            raise BenchmarkError(f"{candidate} exact identities are incomplete")

        target_passes = item.get("target_passes")
        if (
            not _positive_integer(item.get("equivalent_representation_bytes"))
            or not _positive_integer(item.get("equivalent_rendered_node_count"))
            or not isinstance(target_passes, list)
            or len(target_passes) != len(expected_roles)
            or {
                entry.get("target_role")
                for entry in target_passes
                if isinstance(entry, dict)
            }
            != set(expected_roles)
            or any(
                not isinstance(entry, dict)
                or entry.get("representation_bytes")
                != item["equivalent_representation_bytes"]
                or entry.get("rendered_node_count")
                != item["equivalent_rendered_node_count"]
                or entry.get("semantic_parity_passed") is not True
                or entry.get("element_kinds_passed") is not True
                for entry in target_passes
            )
        ):
            raise BenchmarkError(f"{candidate} workload equivalence is incomplete")

        helper_role_items = item.get("helper_target_roles")
        helper_roles = (
            {
                entry.get("target_role"): entry
                for entry in helper_role_items
                if isinstance(entry, dict)
            }
            if isinstance(helper_role_items, list)
            else {}
        )
        if (
            not isinstance(helper_role_items, list)
            or len(helper_roles) != len(helper_role_items)
            or set(helper_roles) != set(expected_roles) - {"host"}
        ):
            raise BenchmarkError(f"{candidate} helper-role inventory is invalid")
        for role, entry in helper_roles.items():
            if (
                not isinstance(entry.get("pids"), list)
                or any(not _positive_integer(pid) for pid in entry["pids"])
                or len(entry["pids"]) != len(set(entry["pids"]))
                or any(pid not in helper_pids for pid in entry["pids"])
                or not nonnegative(entry.get("present_sample_count"))
                or not nonnegative(entry.get("absent_sample_count"))
                or entry["present_sample_count"] + entry["absent_sample_count"]
                != sample_count
                or (
                    role == "webcontent"
                    and entry["present_sample_count"] != sample_count
                )
            ):
                raise BenchmarkError(f"{candidate}/{role} role evidence is invalid")

        values = {field: [] for field in process_fields}
        aggregate = {field: 0 for field in process_fields}
        observed_identities: set[tuple[int, int]] = set()
        physical_identity_alive: dict[tuple[int, int], int] = {}
        physical_host_identities: set[tuple[int, int]] = set()
        physical_helper_identities: set[tuple[int, int]] = set()
        helper_allocations_by_identity: dict[tuple[int, int], int] = {}
        role_present_pids = {role: set() for role in expected_roles}
        role_present_counts = {role: 0 for role in expected_roles}
        host_retained_allocations = 0
        host_retained_bytes = 0
        helper_retained_allocations = 0
        helper_retained_bytes = 0
        present_role_count = 0
        absent_role_count = 0
        alias_count = 0
        measured_process_sample_count = 0
        flattened_intervals: list[dict[str, Any]] = []
        candidate_sample_evidence: list[dict[str, Any]] = []
        for sample_index, sample in enumerate(samples):
            if (
                not isinstance(sample, dict)
                or sample.get("sample_index") != sample_index
                or sample.get("measurement_mode")
                != "equivalent_exact_process_role_passes"
                or any(not nonnegative(sample.get(field)) for field in process_fields)
            ):
                raise BenchmarkError(f"{candidate} sample {sample_index} is invalid")
            intervals = sample.get("role_intervals")
            if not isinstance(intervals, list) or len(intervals) != len(expected_roles):
                raise BenchmarkError(
                    f"{candidate} sample {sample_index} role intervals are invalid"
                )
            by_role = {
                interval.get("target_role"): interval
                for interval in intervals
                if isinstance(interval, dict)
            }
            if len(by_role) != len(intervals) or set(by_role) != set(expected_roles):
                raise BenchmarkError(
                    f"{candidate} sample {sample_index} does not cover all roles"
                )

            canonical_identities: dict[tuple[int, int], tuple[str, int]] = {}
            canonical_pid_list: list[int] = []
            absent_roles: set[str] = set()
            role_evidence: list[dict[str, Any]] = []
            for role in expected_roles:
                interval = by_role[role]
                present = interval.get("target_present")
                started = interval.get("started_unix_ns")
                ended = interval.get("ended_unix_ns")
                included = interval.get("contribution_included")
                if (
                    not isinstance(present, bool)
                    or not _positive_integer(started)
                    or not _positive_integer(ended)
                    or started >= ended
                    or not isinstance(included, bool)
                ):
                    raise BenchmarkError(
                        f"{candidate}/{role} sample {sample_index} interval is invalid"
                    )
                compact_interval: dict[str, Any] = {
                    "target_role": role,
                    "target_present": present,
                    "started_unix_ns": started,
                    "ended_unix_ns": ended,
                    "contribution_included": included,
                }
                flattened_intervals.append(interval)
                if not present:
                    absent_role_count += 1
                    absent_roles.add(role)
                    forbidden = (
                        "target_pid",
                        "target_birth_unix_ns",
                        "observed_alive_through_unix_ns",
                        "recording_readiness_basis",
                        "timestamp_boundary_uncertainty_ns",
                        "required_allocation_pids",
                        "alias_of_target_role",
                    )
                    if (
                        role in mandatory_roles
                        or included
                        or any(key in interval for key in forbidden)
                    ):
                        raise BenchmarkError(
                            f"{candidate}/{role} mandatory allocation target is absent"
                        )
                    role_evidence.append(compact_interval)
                    continue

                present_role_count += 1
                role_present_counts[role] += 1
                pid = interval.get("target_pid")
                birth = interval.get("target_birth_unix_ns")
                alive = interval.get("observed_alive_through_unix_ns")
                readiness = interval.get("recording_readiness_basis")
                required_pids = interval.get("required_allocation_pids")
                uncertainty = interval.get("timestamp_boundary_uncertainty_ns")
                if (
                    not _positive_integer(pid)
                    or not _positive_integer(birth)
                    or not _positive_integer(alive)
                    or not (birth <= started < ended <= alive)
                    or readiness not in ALLOCATION_RECORDING_READINESS_BASES
                    or not isinstance(required_pids, list)
                    or any(not _positive_integer(value) for value in required_pids)
                    or uncertainty != summary["timestamp_boundary_uncertainty_ns"]
                ):
                    raise BenchmarkError(
                        f"{candidate}/{role} sample {sample_index} interval is invalid"
                    )
                identity_key = (pid, birth)
                physical_identity_alive[identity_key] = max(
                    physical_identity_alive.get(identity_key, 0),
                    alive,
                )
                if role == "host":
                    physical_host_identities.add(identity_key)
                else:
                    physical_helper_identities.add(identity_key)
                role_present_pids[role].add(pid)
                observed_readiness_bases.add(readiness)
                compact_interval.update(
                    {
                        "target_pid": pid,
                        "target_birth_unix_ns": birth,
                        "observed_alive_through_unix_ns": alive,
                        "recording_readiness_basis": readiness,
                        "timestamp_boundary_uncertainty_ns": uncertainty,
                    }
                )
                if included:
                    if (
                        required_pids != [pid]
                        or identity_key in canonical_identities
                        or "alias_of_target_role" in interval
                    ):
                        raise BenchmarkError(
                            f"{candidate} sample {sample_index} "
                            "canonical identity is invalid"
                        )
                    canonical_identities[identity_key] = (role, alive)
                    canonical_pid_list.append(pid)
                else:
                    alias_count += 1
                    alias_of = interval.get("alias_of_target_role")
                    if (
                        required_pids != []
                        or not isinstance(alias_of, str)
                        or alias_of not in expected_roles
                        or alias_of == role
                    ):
                        raise BenchmarkError(
                            f"{candidate}/{role} alias evidence is invalid"
                        )
                    compact_interval["alias_of_target_role"] = alias_of
                role_evidence.append(compact_interval)

            compact_aliases = _validated_role_aliases(
                sample.get("role_aliases"),
                roles_by_name={item["target_role"]: item for item in role_evidence},
                expected_roles=expected_roles,
                label=f"{candidate} sample {sample_index}",
            )
            expected_absent_roles = [
                role for role in expected_roles if role in absent_roles
            ]
            if sample.get("absent_target_roles") != expected_absent_roles:
                raise BenchmarkError(
                    f"{candidate} sample {sample_index} absent roles disagree"
                )
            process_totals = sample.get("process_totals")
            required_pids = sample.get("required_allocation_pids")
            if (
                not isinstance(process_totals, list)
                or not process_totals
                or not isinstance(required_pids, list)
                or required_pids != canonical_pid_list
            ):
                raise BenchmarkError(
                    f"{candidate} sample {sample_index} process evidence is incomplete"
                )

            sample_sum = {field: 0 for field in process_fields}
            process_identities: set[tuple[int, int]] = set()
            compact_process_totals: list[dict[str, Any]] = []
            for total_value in process_totals:
                total = validate_process_total(
                    total_value, f"{candidate} sample {sample_index}"
                )
                identity_key = (total["pid"], total["birth_unix_ns"])
                canonical = canonical_identities.get(identity_key)
                if (
                    identity_key in process_identities
                    or canonical is None
                    or canonical[1] != total["observed_alive_through_unix_ns"]
                ):
                    raise BenchmarkError(
                        f"{candidate} sample {sample_index} process identity is invalid"
                    )
                process_identities.add(identity_key)
                observed_identities.add(identity_key)
                measured_process_sample_count += 1
                canonical_role = canonical[0]
                for field in process_fields:
                    sample_sum[field] += total[field]
                if canonical_role == "host":
                    host_retained_allocations += total["retained_allocations"]
                    host_retained_bytes += total["retained_bytes"]
                else:
                    helper_retained_allocations += total["retained_allocations"]
                    helper_retained_bytes += total["retained_bytes"]
                    helper_allocations_by_identity[identity_key] = (
                        helper_allocations_by_identity.get(identity_key, 0)
                        + total["retained_allocations"]
                    )
                compact_process_totals.append(
                    {
                        "pid": total["pid"],
                        "birth_unix_ns": total["birth_unix_ns"],
                        "observed_alive_through_unix_ns": total[
                            "observed_alive_through_unix_ns"
                        ],
                        "names": list(total["names"]),
                        **{field: total[field] for field in process_fields},
                    }
                )
            if process_identities != set(canonical_identities) or any(
                sample_sum[field] != sample[field] for field in process_fields
            ):
                raise BenchmarkError(
                    f"{candidate} sample {sample_index} retained totals disagree"
                )
            candidate_sample_evidence.append(
                {
                    "sample_index": sample_index,
                    **{field: sample[field] for field in process_fields},
                    "roles": role_evidence,
                    "role_aliases": compact_aliases,
                    "process_totals": compact_process_totals,
                }
            )
            for field in process_fields:
                values[field].append(float(sample[field]))
                aggregate[field] += sample[field]

        if (
            physical_identity_alive != claimed_identity_alive
            or len(physical_host_identities) != 1
            or physical_host_identities & physical_helper_identities
            or {pid for pid, _birth in physical_host_identities} != {host_pid}
            or {pid for pid, _birth in physical_helper_identities}
            != set(helper_pids)
            or not observed_identities <= set(claimed_identity_alive)
        ):
            raise BenchmarkError(f"{candidate} does not capture every exact process")
        for role, entry in helper_roles.items():
            if (
                entry["pids"] != sorted(role_present_pids[role])
                or entry["present_sample_count"] != role_present_counts[role]
                or entry["absent_sample_count"]
                != sample_count - role_present_counts[role]
            ):
                raise BenchmarkError(f"{candidate}/{role} role evidence is invalid")
        if any(item.get(field) != aggregate[field] for field in process_fields):
            raise BenchmarkError(f"{candidate} retained aggregate is inconsistent")
        if (
            item.get("host_retained_allocations") != host_retained_allocations
            or item.get("host_retained_bytes") != host_retained_bytes
            or item.get("helper_retained_allocations")
            != helper_retained_allocations
            or item.get("helper_retained_bytes") != helper_retained_bytes
            or (
                candidate == "srui"
                and (helper_retained_allocations != 0 or helper_retained_bytes != 0)
            )
        ):
            raise BenchmarkError(f"{candidate} host/helper retained totals disagree")
        zero_helpers = sorted(
            {
                pid
                for (pid, _birth), allocations in helper_allocations_by_identity.items()
                if allocations == 0
            }
        )
        if item.get("helpers_without_retained_rows") != zero_helpers:
            raise BenchmarkError(f"{candidate} zero-retained helper evidence disagrees")
        if item.get("role_measurement_intervals") != sorted(
            flattened_intervals, key=lambda interval: interval["started_unix_ns"]
        ):
            raise BenchmarkError(f"{candidate} role interval summary disagrees")

        display_name = "SRUI" if candidate == "srui" else "WKWebView control"
        metric_names = {
            "retained_allocations": f"{display_name} interval-retained allocations",
            "retained_allocation_bytes": (
                f"{display_name} interval-retained allocation bytes"
            ),
            "retained_allocations_lower_bound": (
                f"{display_name} definitely interval-retained allocations"
            ),
            "retained_allocations_upper_bound": (
                f"{display_name} possibly interval-retained allocations"
            ),
            "retained_allocation_bytes_lower_bound": (
                f"{display_name} definitely interval-retained bytes"
            ),
            "retained_allocation_bytes_upper_bound": (
                f"{display_name} possibly interval-retained bytes"
            ),
            "boundary_ambiguous_allocations": (
                f"{display_name} timestamp-boundary-ambiguous allocations"
            ),
            "boundary_ambiguous_bytes": (
                f"{display_name} timestamp-boundary-ambiguous bytes"
            ),
        }
        for sample_field, metric_kind, unit in ALLOCATION_METRIC_KINDS:
            for statistic, fraction in (("p50", 0.5), ("p95", 0.95), ("p99", 0.99)):
                metrics.append(
                    {
                        "id": f"{candidate}.{metric_kind}",
                        "name": metric_names[metric_kind],
                        "value": percentile(values[sample_field], fraction),
                        "unit": unit,
                        "statistic": statistic,
                    }
                )

        readiness = sorted(
            {
                interval["recording_readiness_basis"]
                for interval in flattened_intervals
                if interval["target_present"]
            }
        )
        if item.get("recording_readiness_bases") != readiness:
            raise BenchmarkError(f"{candidate} readiness summary disagrees")
        sample_evidence.append(
            {
                "candidate": candidate,
                "samples": candidate_sample_evidence,
            }
        )
        candidate_totals.append(
            {
                "candidate": candidate,
                "measurement_mode": item["measurement_mode"],
                "measurement_sample_count": sample_count,
                "measured_process_sample_count": measured_process_sample_count,
                "claimed_process_count": len(claimed_identity_alive),
                "processes_with_live_list_evidence": len(observed_identities),
                "helper_process_count": len(physical_helper_identities),
                "helper_target_roles": sorted(set(expected_roles) - {"host"}),
                "present_role_measurement_count": present_role_count,
                "absent_role_measurement_count": absent_role_count,
                "aliased_role_measurement_count": alias_count,
                "recording_readiness_bases": readiness,
                "helpers_without_retained_rows": len(zero_helpers),
                **aggregate,
                "host_retained_allocations": host_retained_allocations,
                "host_retained_bytes": host_retained_bytes,
                "helper_retained_allocations": helper_retained_allocations,
                "helper_retained_bytes": helper_retained_bytes,
            }
        )

    if len(sample_counts) != 1:
        raise BenchmarkError(
            "SRUI and WebKit allocation captures have different sample counts"
        )
    if observed_readiness_bases != set(readiness_bases):
        raise BenchmarkError("allocation readiness evidence is inconsistent")

    artifact = {
        "schema_version": 3,
        "profile": profile,
        "capture_scope": summary["capture_scope"],
        **{key: summary[key] for key in required_strings},
        "recording_readiness_bases": sorted(observed_readiness_bases),
        "instrumentation": instrumentation,
        "xctrace_environment": xctrace_environment,
        "trace_start_timestamp_resolution_ns": summary[
            "trace_start_timestamp_resolution_ns"
        ],
        "allocation_list_timestamp_resolution_ns": summary[
            "allocation_list_timestamp_resolution_ns"
        ],
        "timestamp_boundary_uncertainty_ns": summary[
            "timestamp_boundary_uncertainty_ns"
        ],
        "allocation_rows": summary["allocation_rows"],
        "allocation_list_bytes": summary["allocation_list_bytes"],
        **{field: summary[field] for field in ALLOCATION_DIAGNOSTIC_FIELDS},
        "allocation_list_reconciled": True,
        "whole_trace_statistics_aggregate": aggregate_statistics,
        "process_attribution_complete": True,
        "candidate_totals": candidate_totals,
        "sample_evidence": sample_evidence,
    }
    detail = "; ".join(
        f"{item['candidate']}={item['measurement_sample_count']} logical samples/"
        f"{item['measured_process_sample_count']} exact-process samples/"
        f"{item['helpers_without_retained_rows']} zero-retained helpers"
        for item in candidate_totals
    )
    assertion = {
        "id": "allocation_trace_attributed",
        "name": (
            "interval-retained heap and anonymous VM allocations "
            "have exact-process attribution"
        ),
        "passed": True,
        "detail": (
            "one exact-PID xctrace attachment per equivalent role pass; "
            "complete live List reconciled to All Heap & Anonymous VM "
            "persistent Statistics; VM category totals remain diagnostic; "
            "whole-trace "
            "totals are diagnostic only; nominal/lower/upper values expose "
            f"{summary['timestamp_boundary_uncertainty_ns']} ns boundary "
            f"uncertainty; {detail}"
        ),
    }
    return metrics, assertion, artifact


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
        if drivers[name]["command"] != EXPECTED_DRIVER_COMMANDS[name]:
            raise BenchmarkError(
                f"{name} driver command must invoke the pinned production benchmark target"
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


def validate_runtime_platforms(
    manifest: dict[str, Any],
    *,
    current_platform: str | None = None,
) -> None:
    """Fail before starting any driver when a complete §31 run is impossible."""
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


def validate_allocation_capture_host(
    *,
    current_platform: str | None = None,
    status_runner: Any = subprocess.run,
) -> None:
    """Require macOS task-inspection permission before any benchmark build."""
    platform_name = current_platform if current_platform is not None else sys.platform
    if platform_name != "darwin":
        return
    command = ["/usr/sbin/DevToolsSecurity", "-status"]
    try:
        result = status_runner(
            command,
            capture_output=True,
            text=True,
            check=False,
            timeout=10,
        )
    except (OSError, subprocess.SubprocessError) as error:
        raise BenchmarkError(
            "cannot query Developer Tools security before allocation capture; "
            "no drivers were started"
        ) from error
    output = f"{result.stdout}\n{result.stderr}".strip()
    normalized = output.lower()
    if result.returncode != 0 or "currently enabled" not in normalized:
        detail = output or f"exit status {result.returncode}"
        raise BenchmarkError(
            "mandatory exact-process xctrace capture requires Developer Tools "
            "security to be enabled; run "
            "`sudo /usr/sbin/DevToolsSecurity -enable` and retry; "
            f"no drivers were started ({detail})"
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
        if value < 0:
            raise BenchmarkError(
                f"{label}.{metric['id']} ({metric['statistic']}) must be nonnegative"
            )
        if metric["unit"] in {
            "allocations",
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
                    metrics_by_id[f"cadence.{count}.{cadence}.{suffix}"]["exact"]
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
                    and metrics_by_id[f"{prefix}.inbound_messages"]["exact"]
                    == count
                    and metrics_by_id[f"{prefix}.outbound_messages"]["exact"]
                    == 3
                )
        if assertions["cadence_wire_invariant"] is not wire_invariant:
            raise BenchmarkError(
                f"{label} cadence_wire_invariant contradicts cadence wire metrics"
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
        if item["started_unix_ns"] >= item["ended_unix_ns"]:
            raise BenchmarkError(
                f"{candidate} process attribution interval is empty or reversed"
            )
        intervals = item["measurement_intervals"]
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
            required_allocation_pids = interval[
                "required_allocation_pids"
            ]
            if required_allocation_pids:
                raise BenchmarkError(
                    f"{candidate} ordinary renderer interval {index} must not "
                    "claim targeted allocation PIDs"
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
            raise BenchmarkError(f"{candidate} process identities contain duplicate PIDs")
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
            identity for identity in identities if identity["pid"] == item["host_pid"]
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


def validate_driver_output(
    payload: Any,
    driver: dict[str, Any],
    *,
    profile: str = "full",
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
            section for section in payload["sections"] if section["id"] == "31.4"
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
        if local_latency["passed"] and local_delta > frame_budget:
            raise BenchmarkError(
                "macos §31.4 local_latency_independent assertion contradicts "
                "the dynamic display.frame_budget target"
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
            profile=profile,
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


def run_candidate_cleanup_probe(
    fixture: Path,
    timeout: int,
) -> str:
    binary = ROOT / "client-macos/.build/release/BenchmarkDriver"
    if not binary.is_file():
        raise BenchmarkError(
            "release BenchmarkDriver is missing before candidate cleanup probe"
        )
    ensure_free_space(ROOT)
    with tempfile.TemporaryDirectory(prefix="srui-candidate-cleanup-") as directory:
        probe_directory = Path(directory)
        identity_path = probe_directory / "candidate-identity.json"
        descendant_identity_path = probe_directory / "descendant-identity.json"
        output_path = probe_directory / "driver.json"
        command = [
            "/usr/bin/env",
            "SRUI_BENCHMARK_FORCE_CANDIDATE_IDENTITY_FAILURE=1",
            f"SRUI_BENCHMARK_CANDIDATE_IDENTITY_PATH={identity_path}",
            (
                "SRUI_BENCHMARK_CANDIDATE_DESCENDANT_IDENTITY_PATH="
                f"{descendant_identity_path}"
            ),
            str(binary),
            "--fixture",
            str(fixture),
            "--profile",
            "smoke",
            "--only-section",
            "31.1",
            "--output",
            str(output_path),
        ]
        probe_timeout = min(timeout, 30)
        try:
            result = run_managed_command(
                command,
                cwd=ROOT,
                timeout=probe_timeout,
                label="renderer candidate cleanup fault probe",
                poll_hook=lambda _process: ensure_free_space(ROOT),
            )
        except ManagedCommandTimeout as error:
            raise BenchmarkError(
                f"candidate cleanup probe timed out after {probe_timeout}s"
            ) from error
        except ManagedCommandError as error:
            raise BenchmarkError(
                f"candidate cleanup probe supervision failed: {error}"
            ) from error
        combined = (result.stdout + "\n" + result.stderr).strip()
        if result.returncode == 0:
            raise BenchmarkError(
                "candidate cleanup probe unexpectedly succeeded"
            )
        if (
            "renderer candidate srui birth identity was unavailable"
            not in combined
            or "candidate cleanup failed" in combined
        ):
            raise BenchmarkError(
                "candidate cleanup probe did not exercise the expected "
                f"successful cleanup path: {combined[-2000:]}"
            )
        identities: list[tuple[int, int]] = []
        try:
            for label, path in (
                ("candidate", identity_path),
                ("descendant", descendant_identity_path),
            ):
                identity = json.loads(path.read_text(encoding="utf-8"))
                pid = identity["pid"]
                birth = identity["birth_unix_ns"]
                if not _positive_integer(pid) or not _positive_integer(birth):
                    raise BenchmarkError(
                        f"candidate cleanup probe {label} identity has invalid values"
                    )
                identities.append((pid, birth))
        except (OSError, json.JSONDecodeError, KeyError, TypeError) as error:
            raise BenchmarkError(
                "candidate cleanup probe did not record both valid identities"
            ) from error
        if len({pid for pid, _birth in identities}) != 2:
            raise BenchmarkError(
                "candidate cleanup probe identities do not name distinct processes"
            )
        try:
            wait_for_process_identities_gone(
                identities,
                label="forced-identity-failure renderer candidate group",
            )
        except ManagedCommandError as error:
            raise BenchmarkError(str(error)) from error

        publication_candidate_path = probe_directory / "publication-candidate.json"
        publication_observer_path = probe_directory / "publication-descendant.json"
        blocked_destination = probe_directory / "blocked-descendant-output"
        blocked_destination.mkdir()
        publication_command = [
            "/usr/bin/env",
            (
                "SRUI_BENCHMARK_CANDIDATE_IDENTITY_PATH="
                f"{publication_candidate_path}"
            ),
            (
                "SRUI_BENCHMARK_CANDIDATE_DESCENDANT_IDENTITY_PATH="
                f"{blocked_destination}"
            ),
            (
                "SRUI_BENCHMARK_CANDIDATE_DESCENDANT_OBSERVER_PATH="
                f"{publication_observer_path}"
            ),
            str(binary),
            "--fixture",
            str(fixture),
            "--profile",
            "smoke",
            "--only-section",
            "31.1",
            "--output",
            str(probe_directory / "publication-driver.json"),
        ]
        try:
            publication_result = run_managed_command(
                publication_command,
                cwd=ROOT,
                timeout=probe_timeout,
                label="renderer descendant-publication cleanup fault probe",
                poll_hook=lambda _process: ensure_free_space(ROOT),
            )
        except (ManagedCommandTimeout, ManagedCommandError) as error:
            raise BenchmarkError(
                "candidate descendant-publication cleanup probe failed supervision"
            ) from error
        publication_detail = (
            publication_result.stdout + "\n" + publication_result.stderr
        ).strip()
        if (
            publication_result.returncode == 0
            or "renderer candidate srui exited with status" not in publication_detail
            or "candidate cleanup failed" in publication_detail
        ):
            raise BenchmarkError(
                "candidate descendant-publication probe did not exercise the "
                f"expected cleanup path: {publication_detail[-2000:]}"
            )
        publication_identities: list[tuple[int, int]] = []
        try:
            for path in (publication_candidate_path, publication_observer_path):
                identity = json.loads(path.read_text(encoding="utf-8"))
                pid = identity["pid"]
                birth = identity["birth_unix_ns"]
                if not _positive_integer(pid) or not _positive_integer(birth):
                    raise BenchmarkError(
                        "descendant-publication probe identity has invalid values"
                    )
                publication_identities.append((pid, birth))
        except (OSError, json.JSONDecodeError, KeyError, TypeError) as error:
            raise BenchmarkError(
                "descendant-publication probe did not record both identities"
            ) from error
        if len({pid for pid, _birth in publication_identities}) != 2:
            raise BenchmarkError(
                "descendant-publication probe identities are not distinct"
            )
        try:
            wait_for_process_identities_gone(
                publication_identities,
                label="descendant-publication-failure renderer candidate group",
            )
        except ManagedCommandError as error:
            raise BenchmarkError(str(error)) from error

        internal_candidate_path = probe_directory / "internal-candidate.json"
        internal_descendant_path = probe_directory / "internal-descendant.json"
        internal_command = [
            "/usr/bin/env",
            "SRUI_BENCHMARK_FORCE_CANDIDATE_INTERNAL_FAILURE=1",
            (
                "SRUI_BENCHMARK_CANDIDATE_IDENTITY_PATH="
                f"{internal_candidate_path}"
            ),
            (
                "SRUI_BENCHMARK_CANDIDATE_DESCENDANT_IDENTITY_PATH="
                f"{internal_descendant_path}"
            ),
            str(binary),
            "--fixture",
            str(fixture),
            "--profile",
            "smoke",
            "--only-section",
            "31.1",
            "--output",
            str(probe_directory / "internal-driver.json"),
        ]
        try:
            internal_result = run_managed_command(
                internal_command,
                cwd=ROOT,
                timeout=probe_timeout,
                label="renderer internal-failure cleanup fault probe",
                poll_hook=lambda _process: ensure_free_space(ROOT),
            )
        except (ManagedCommandTimeout, ManagedCommandError) as error:
            raise BenchmarkError(
                "candidate internal-failure cleanup probe failed supervision"
            ) from error
        internal_detail = (
            internal_result.stdout + "\n" + internal_result.stderr
        ).strip()
        if (
            internal_result.returncode == 0
            or (
                "forced renderer candidate internal failure after descendant setup"
                not in internal_detail
            )
            or "candidate cleanup failed" in internal_detail
        ):
            raise BenchmarkError(
                "candidate internal-failure probe did not exercise the "
                f"expected cleanup path: {internal_detail[-2000:]}"
            )
        internal_identities: list[tuple[int, int]] = []
        try:
            for path in (internal_candidate_path, internal_descendant_path):
                identity = json.loads(path.read_text(encoding="utf-8"))
                pid = identity["pid"]
                birth = identity["birth_unix_ns"]
                if not _positive_integer(pid) or not _positive_integer(birth):
                    raise BenchmarkError(
                        "internal-failure probe identity has invalid values"
                    )
                internal_identities.append((pid, birth))
        except (OSError, json.JSONDecodeError, KeyError, TypeError) as error:
            raise BenchmarkError(
                "internal-failure probe did not record both identities"
            ) from error
        if len({pid for pid, _birth in internal_identities}) != 2:
            raise BenchmarkError(
                "internal-failure probe identities are not distinct"
            )
        try:
            wait_for_process_identities_gone(
                internal_identities,
                label="internal-failure renderer candidate group",
            )
        except ManagedCommandError as error:
            raise BenchmarkError(str(error)) from error
    return (
        "forced candidate identity failure terminated the exact candidate "
        "process group and proved both the child and a real descendant gone; "
        "a forced descendant-identity publication failure and a forced internal "
        "candidate failure also proved both exact identities gone before the "
        "benchmark driver returned"
    )


def run_allocation_capture(
    fixture: Path,
    profile: str,
    timeout: int,
) -> dict[str, Any]:
    binary = ROOT / "client-macos/.build/release/BenchmarkDriver"
    script = ROOT / "benchmarks/parse-render/run_xctrace.py"
    if not binary.is_file():
        raise BenchmarkError(
            "release BenchmarkDriver is missing before allocation capture"
        )
    if not script.is_file():
        raise BenchmarkError("allocation capture driver is missing")

    ensure_free_space(ROOT)
    with tempfile.TemporaryDirectory(prefix="srui-allocation-capture-") as directory:
        capture_directory = Path(directory)
        trace = capture_directory / "render.trace"
        result_path = capture_directory / "driver.json"
        sidecar = trace.with_name(f"{trace.name}.summary.json")
        command = [
            sys.executable,
            str(script),
            str(trace),
            str(binary),
            str(fixture),
            str(result_path),
            profile,
        ]
        try:
            result = run_managed_command(
                command,
                cwd=ROOT,
                timeout=timeout,
                label="mandatory §31.1 allocation capture",
                poll_hook=lambda _process: ensure_free_space(ROOT),
            )
        except ManagedCommandTimeout as error:
            raise BenchmarkError(
                f"allocation capture timed out after {timeout}s"
            ) from error
        except ManagedCommandError as error:
            raise BenchmarkError(
                f"allocation capture process supervision failed: {error}"
            ) from error
        if result.returncode:
            detail = (result.stderr or result.stdout).strip()
            raise BenchmarkError(
                f"allocation capture failed ({result.returncode}): {detail[-4000:]}"
            )
        try:
            summary = json.loads(sidecar.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            raise BenchmarkError(
                "allocation capture did not write a valid summary"
            ) from error
        return summary


def run_verification(
    spec: dict[str, Any],
    default_timeout: int,
) -> tuple[float, bool, str, int | None]:
    started = time.monotonic()
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
        elapsed = (time.monotonic() - started) * 1_000
        return elapsed, False, f"timed out after {timeout}s", None
    except ManagedCommandError as error:
        elapsed = (time.monotonic() - started) * 1_000
        return elapsed, False, f"process supervision failed: {error}", None

    elapsed = (time.monotonic() - started) * 1_000
    combined = (result.stdout + "\n" + result.stderr).strip()
    missing_output = [
        required
        for required in spec["required_output"]
        if required not in combined
    ]
    runner_match = re.search(
        r"(?m)^\s*PASS\s+([1-9][0-9]*)\s+runner\(s\)\s*$",
        combined,
    )
    sample_count = int(runner_match.group(1)) if runner_match else None
    passed = result.returncode == 0 and not missing_output and sample_count is not None
    detail_parts = [f"exit {result.returncode}"]
    if missing_output:
        detail_parts.append("missing required output: " + ", ".join(missing_output))
    if sample_count is None:
        detail_parts.append("missing exact production conformance runner count")
    if combined:
        detail_parts.append(combined[-500:])
    return elapsed, passed, "; ".join(detail_parts), sample_count


def _retained_bounds_reconcile(value: dict[str, Any]) -> bool:
    return (
        value["retained_allocations_lower_bound"]
        <= value["retained_allocations"]
        <= value["retained_allocations_upper_bound"]
        and value["retained_bytes_lower_bound"]
        <= value["retained_bytes"]
        <= value["retained_bytes_upper_bound"]
        and value["boundary_ambiguous_allocations"]
        == value["retained_allocations_upper_bound"]
        - value["retained_allocations_lower_bound"]
        and value["boundary_ambiguous_bytes"]
        == value["retained_bytes_upper_bound"]
        - value["retained_bytes_lower_bound"]
    )


def validate_allocation_sample_evidence(
    allocation_capture: dict[str, Any],
    candidate_totals: dict[str, dict[str, Any]],
) -> None:
    evidence_items = allocation_capture["sample_evidence"]
    evidence_by_candidate = {item["candidate"]: item for item in evidence_items}
    if set(evidence_by_candidate) != set(ALLOCATION_CANDIDATES):
        raise BenchmarkError(
            "allocation sample evidence must contain exactly srui and webkit"
        )

    for candidate, candidate_total in candidate_totals.items():
        samples = evidence_by_candidate[candidate]["samples"]
        expected_roles = ALLOCATION_TARGET_ROLES[candidate]
        mandatory_roles = ALLOCATION_MANDATORY_ROLES[candidate]
        if (
            len(samples) != candidate_total["measurement_sample_count"]
            or [sample["sample_index"] for sample in samples]
            != list(range(len(samples)))
        ):
            raise BenchmarkError(
                "allocation sample evidence contradicts candidate totals"
            )

        aggregate = {field: 0 for field in ALLOCATION_PROCESS_FIELDS}
        physical_identity_alive: dict[tuple[int, int], int] = {}
        process_identities_with_evidence: set[tuple[int, int]] = set()
        host_identities: set[tuple[int, int]] = set()
        helper_identities: set[tuple[int, int]] = set()
        helper_allocations: dict[tuple[int, int], int] = {}
        present_count = 0
        absent_count = 0
        alias_count = 0
        process_sample_count = 0
        host_retained_allocations = 0
        host_retained_bytes = 0
        helper_retained_allocations = 0
        helper_retained_bytes = 0
        readiness_bases: set[str] = set()

        for sample in samples:
            if not _retained_bounds_reconcile(sample):
                raise BenchmarkError(
                    "allocation sample evidence has invalid retained bounds"
                )
            roles = sample["roles"]
            roles_by_name = {role["target_role"]: role for role in roles}
            if len(roles_by_name) != len(roles) or set(roles_by_name) != set(
                expected_roles
            ):
                raise BenchmarkError(
                    "allocation sample evidence has incomplete role coverage"
                )

            canonical_by_identity: dict[
                tuple[int, int],
                tuple[str, int],
            ] = {}
            for role_name in expected_roles:
                role = roles_by_name[role_name]
                started = role["started_unix_ns"]
                ended = role["ended_unix_ns"]
                if started >= ended:
                    raise BenchmarkError(
                        "allocation sample evidence has invalid role interval"
                    )
                if not role["target_present"]:
                    absent_count += 1
                    if role_name in mandatory_roles or role["contribution_included"]:
                        raise BenchmarkError(
                            "allocation sample evidence omits a mandatory role"
                        )
                    continue

                present_count += 1
                pid = role["target_pid"]
                birth = role["target_birth_unix_ns"]
                alive = role["observed_alive_through_unix_ns"]
                identity_key = (pid, birth)
                if (
                    not birth <= started < ended <= alive
                    or role["timestamp_boundary_uncertainty_ns"]
                    != allocation_capture["timestamp_boundary_uncertainty_ns"]
                ):
                    raise BenchmarkError(
                        "allocation sample evidence has invalid process identity"
                    )
                physical_identity_alive[identity_key] = max(
                    physical_identity_alive.get(identity_key, 0),
                    alive,
                )
                readiness_bases.add(role["recording_readiness_basis"])
                if role_name == "host":
                    host_identities.add(identity_key)
                else:
                    helper_identities.add(identity_key)

                if role["contribution_included"]:
                    if (
                        identity_key in canonical_by_identity
                        or "alias_of_target_role" in role
                    ):
                        raise BenchmarkError(
                            "allocation sample evidence duplicates a contribution"
                        )
                    canonical_by_identity[identity_key] = (role_name, alive)
                else:
                    alias_count += 1

            _validated_role_aliases(
                sample["role_aliases"],
                roles_by_name=roles_by_name,
                expected_roles=expected_roles,
                label="allocation sample evidence",
            )

            process_totals = sample["process_totals"]
            process_by_identity = {
                (total["pid"], total["birth_unix_ns"]): total
                for total in process_totals
            }
            if (
                len(process_by_identity) != len(process_totals)
                or set(process_by_identity) != set(canonical_by_identity)
            ):
                raise BenchmarkError(
                    "allocation sample evidence has incomplete process totals"
                )
            sample_sum = {field: 0 for field in ALLOCATION_PROCESS_FIELDS}
            for identity_key, process_total in process_by_identity.items():
                canonical_role, alive = canonical_by_identity[identity_key]
                if (
                    process_total["observed_alive_through_unix_ns"] != alive
                    or not _retained_bounds_reconcile(process_total)
                ):
                    raise BenchmarkError(
                        "allocation sample evidence has invalid process totals"
                    )
                process_identities_with_evidence.add(identity_key)
                process_sample_count += 1
                for field in ALLOCATION_PROCESS_FIELDS:
                    sample_sum[field] += process_total[field]
                if canonical_role == "host":
                    host_retained_allocations += process_total["retained_allocations"]
                    host_retained_bytes += process_total["retained_bytes"]
                else:
                    helper_retained_allocations += process_total[
                        "retained_allocations"
                    ]
                    helper_retained_bytes += process_total["retained_bytes"]
                    helper_allocations[identity_key] = (
                        helper_allocations.get(identity_key, 0)
                        + process_total["retained_allocations"]
                    )

            if any(
                sample_sum[field] != sample[field]
                for field in ALLOCATION_PROCESS_FIELDS
            ):
                raise BenchmarkError(
                    "allocation sample evidence retained totals disagree"
                )
            for field in ALLOCATION_PROCESS_FIELDS:
                aggregate[field] += sample[field]

        zero_helper_pids = {
            pid
            for (pid, _birth), allocations in helper_allocations.items()
            if allocations == 0
        }
        if (
            len(host_identities) != 1
            or bool(host_identities & helper_identities)
            or candidate_total["claimed_process_count"]
            != len(physical_identity_alive)
            or candidate_total["processes_with_live_list_evidence"]
            != len(process_identities_with_evidence)
            or candidate_total["helper_process_count"] != len(helper_identities)
            or candidate_total["measured_process_sample_count"] != process_sample_count
            or candidate_total["present_role_measurement_count"] != present_count
            or candidate_total["absent_role_measurement_count"] != absent_count
            or candidate_total["aliased_role_measurement_count"] != alias_count
            or candidate_total["recording_readiness_bases"]
            != sorted(readiness_bases)
            or candidate_total["helpers_without_retained_rows"]
            != len(zero_helper_pids)
            or candidate_total["host_retained_allocations"]
            != host_retained_allocations
            or candidate_total["host_retained_bytes"] != host_retained_bytes
            or candidate_total["helper_retained_allocations"]
            != helper_retained_allocations
            or candidate_total["helper_retained_bytes"] != helper_retained_bytes
            or any(
                candidate_total[field] != aggregate[field]
                for field in ALLOCATION_PROCESS_FIELDS
            )
        ):
            raise BenchmarkError(
                "allocation sample evidence contradicts candidate totals"
            )


def validate_report(report: dict[str, Any], required: list[str]) -> None:
    validate_document(report, "report", "benchmark report")
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
        raise BenchmarkError("benchmark report sections: " + "; ".join(detail))

    sections_by_id = {section["id"]: section for section in report["sections"]}
    expected_counts = expected_report_sample_counts(report["profile"])
    for section_id, section in sections_by_id.items():
        _validate_section_inventory(
            section,
            EXPECTED_REPORT_INVENTORY[section_id],
            expected_sample_counts=expected_counts[section_id],
            label=f"report §{section_id}",
            profile=report["profile"],
        )

    artifacts = report["driver_artifacts"]
    if "renderer_process_attribution" in artifacts["rust"]:
        raise BenchmarkError("rust report artifact must not claim renderer processes")
    validate_renderer_process_attribution(
        artifacts["macos"],
        expected_driver_pid=None,
    )
    allocation_capture = artifacts["macos"].get("allocation_capture")
    if not isinstance(allocation_capture, dict):
        raise BenchmarkError(
            "macos report artifact must include mandatory allocation_capture"
        )
    if allocation_capture["profile"] != report["profile"]:
        raise BenchmarkError(
            "allocation capture profile does not match benchmark report"
        )
    statistics = allocation_capture["whole_trace_statistics_aggregate"]
    if any(
        statistics["heap_and_anonymous_vm"][field]
        != statistics["heap"][field] + statistics["anonymous_vm"][field]
        for field in ALLOCATION_STATISTIC_FIELDS
    ):
        raise BenchmarkError(
            "allocation capture Statistics diagnostics do not reconcile"
        )
    combined = statistics["heap_and_anonymous_vm"]
    anonymous_vm = statistics["anonymous_vm"]
    if (
        combined["persistent_allocations"] != allocation_capture["allocation_rows"]
        or combined["persistent_bytes"] != allocation_capture["allocation_list_bytes"]
        or anonymous_vm["persistent_allocations"]
        != allocation_capture["anonymous_vm_persistent_allocations"]
        or anonymous_vm["persistent_bytes"]
        != allocation_capture["anonymous_vm_persistent_bytes"]
        or allocation_capture["vm_category_rows"]
        > allocation_capture["allocation_rows"]
        or allocation_capture["vm_category_bytes"]
        > allocation_capture["allocation_list_bytes"]
    ):
        raise BenchmarkError(
            "allocation capture live List contradicts Statistics diagnostics"
        )
    allocation_candidates = {
        item["candidate"]: item
        for item in allocation_capture["candidate_totals"]
    }
    if set(allocation_candidates) != set(ALLOCATION_CANDIDATES):
        raise BenchmarkError(
            "allocation capture metadata must contain exactly srui and webkit"
        )
    for candidate, item in allocation_candidates.items():
        helper_count = item["helper_process_count"]
        sample_count = item["measurement_sample_count"]
        expected_helper_roles = sorted(
            set(ALLOCATION_TARGET_ROLES[candidate]) - {"host"}
        )
        retained_bounds_valid = (
            item["retained_allocations_lower_bound"]
            <= item["retained_allocations"]
            <= item["retained_allocations_upper_bound"]
            and item["retained_bytes_lower_bound"]
            <= item["retained_bytes"]
            <= item["retained_bytes_upper_bound"]
            and item["boundary_ambiguous_allocations"]
            == item["retained_allocations_upper_bound"]
            - item["retained_allocations_lower_bound"]
            and item["boundary_ambiguous_bytes"]
            == item["retained_bytes_upper_bound"]
            - item["retained_bytes_lower_bound"]
        )
        if (
            item["measurement_mode"]
            != "equivalent_exact_process_role_passes"
            or item["claimed_process_count"] != helper_count + 1
            or item["processes_with_live_list_evidence"]
            > item["claimed_process_count"]
            or item["measured_process_sample_count"]
            != item["present_role_measurement_count"]
            - item["aliased_role_measurement_count"]
            or item["present_role_measurement_count"]
            + item["absent_role_measurement_count"]
            != len(ALLOCATION_TARGET_ROLES[candidate]) * sample_count
            or item["helper_target_roles"] != expected_helper_roles
            or item["recording_readiness_bases"]
            != allocation_capture["recording_readiness_bases"]
            or item["helpers_without_retained_rows"] > helper_count
            or item["host_retained_allocations"]
            + item["helper_retained_allocations"]
            != item["retained_allocations"]
            or item["host_retained_bytes"] + item["helper_retained_bytes"]
            != item["retained_bytes"]
            or not retained_bounds_valid
            or (candidate == "srui" and helper_count != 0)
            or (candidate == "webkit" and helper_count < 1)
        ):
            raise BenchmarkError(
                "allocation capture attribution contradicts its retained evidence"
            )

    allocation_sample_counts = sections_by_id["31.1"]["sample_counts"]
    for candidate, item in allocation_candidates.items():
        reported_count = allocation_sample_counts[f"runner.xctrace.{candidate}"]
        if reported_count != item["measurement_sample_count"]:
            raise BenchmarkError(
                f"{candidate} xctrace sample count contradicts allocation evidence"
            )

    validate_allocation_sample_evidence(
        allocation_capture,
        allocation_candidates,
    )

    allocation_metric_ids = {
        f"{candidate}.{metric_kind}"
        for candidate in ALLOCATION_CANDIDATES
        for _sample_field, metric_kind, _unit in ALLOCATION_METRIC_KINDS
    }
    allocation_metrics = {
        (metric["id"], metric["statistic"]): metric["value"]
        for metric in sections_by_id["31.1"]["metrics"]
        if metric["id"] in allocation_metric_ids
    }
    if any(value < 0 for value in allocation_metrics.values()):
        raise BenchmarkError("allocation capture metrics must be nonnegative")
    evidence_by_candidate = {
        item["candidate"]: item["samples"]
        for item in allocation_capture["sample_evidence"]
    }
    for candidate in ALLOCATION_CANDIDATES:
        samples = evidence_by_candidate[candidate]
        for sample_field, metric_kind, _unit in ALLOCATION_METRIC_KINDS:
            values = [float(sample[sample_field]) for sample in samples]
            for statistic, fraction in (
                ("p50", 0.50),
                ("p95", 0.95),
                ("p99", 0.99),
            ):
                reported = allocation_metrics[
                    (f"{candidate}.{metric_kind}", statistic)
                ]
                computed = percentile(values, fraction)
                if reported != computed:
                    raise BenchmarkError(
                        "allocation capture metric distribution contradicts "
                        f"sample evidence for {candidate}.{metric_kind} {statistic}"
                    )
    for candidate in ALLOCATION_CANDIDATES:
        for statistic in DISTRIBUTION:
            for lower_kind, nominal_kind, upper_kind in (
                (
                    "retained_allocations_lower_bound",
                    "retained_allocations",
                    "retained_allocations_upper_bound",
                ),
                (
                    "retained_allocation_bytes_lower_bound",
                    "retained_allocation_bytes",
                    "retained_allocation_bytes_upper_bound",
                ),
            ):
                lower = allocation_metrics[(f"{candidate}.{lower_kind}", statistic)]
                nominal = allocation_metrics[(f"{candidate}.{nominal_kind}", statistic)]
                upper = allocation_metrics[(f"{candidate}.{upper_kind}", statistic)]
                if not lower <= nominal <= upper:
                    raise BenchmarkError(
                        "allocation capture metric bounds contradict nominal values"
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
            f"{rust['canonical_transaction_bytes']} exact canonical progressive "
            "sequence bytes"
        )
    else:
        detail = (
            "rust canonical progressive sequence "
            f"{rust['canonical_transaction_sha256']} / {rust['canonical_transaction_bytes']} bytes; "
            "macos canonical progressive sequence "
            f"{macos['canonical_transaction_sha256']} / {macos['canonical_transaction_bytes']} bytes"
        )
    sections["31.2"]["assertions"].append(
        {
            "id": "canonical_transaction_parity",
            "name": "canonical progressive transaction sequence bytes match",
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
    environment = report["environment"]

    def one_line(value: Any) -> str:
        return str(value).replace("\n", " / ")

    lines = [
        "# SRUI benchmark report",
        "",
        f"- Generated: {report['generated_at']}",
        f"- Profile: {report['profile']}",
        f"- Host: {environment['platform']} / {environment['machine']}",
        f"- Chip: {environment['chip']}",
        f"- Physical RAM: {environment['physical_ram_bytes']} bytes",
        f"- Xcode: {one_line(environment['xcode'])}",
        f"- Swift: {one_line(environment['swift'])}",
        f"- Rust: {one_line(environment['rust'])}",
        f"- Python: {environment['python']}",
        f"- Git: {environment['git_commit']} ({'dirty' if environment['git_dirty'] else 'clean'})",
        f"- Fixture: {report['fixture']}",
        "",
    ]
    followups: list[str] = []
    correctness_failures: list[str] = []
    for section in sorted(report["sections"], key=lambda item: item["id"]):
        lines += [
            f"## §{section['id']} {section['name']}",
            "",
            "Samples:",
            "",
            *[
                f"- `{group}`: {count}"
                for group, count in sorted(section["sample_counts"].items())
            ],
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


def ensure_baseline_start_clean(environment: dict[str, Any]) -> None:
    if environment.get("git_dirty") is not False:
        raise BenchmarkError(
            "refusing to overwrite the committed baseline from a dirty Git tree"
        )


def ensure_baseline_recordable(
    report: dict[str, Any],
    *,
    ending_environment: dict[str, Any] | None = None,
) -> None:
    if report.get("profile") != "full":
        raise BenchmarkError(
            "refusing to overwrite the committed baseline with a non-full profile"
        )
    environment = report.get("environment", {})
    ensure_baseline_start_clean(environment)
    if ending_environment is not None and (
        ending_environment.get("git_dirty") is not False
        or ending_environment.get("git_commit") != environment.get("git_commit")
    ):
        raise BenchmarkError(
            "refusing to overwrite the committed baseline after Git state changed "
            "during the benchmark"
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
    parser.add_argument(
        "--allocation-timeout",
        type=int,
        help=(
            "outer bound for the mandatory xctrace capture; defaults to a "
            "profile-specific bound covering record, export, and cleanup"
        ),
    )
    args = parser.parse_args(argv)
    if args.record_baseline and args.profile != "full":
        parser.error("--record-baseline requires --profile full")
    if args.timeout <= 0:
        parser.error("--timeout must be positive")
    if args.allocation_timeout is not None and args.allocation_timeout <= 0:
        parser.error("--allocation-timeout must be positive")
    allocation_timeout = (
        args.allocation_timeout
        if args.allocation_timeout is not None
        else ALLOCATION_CAPTURE_TIMEOUTS[args.profile]
    )

    try:
        manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise BenchmarkError(f"cannot load benchmark manifest {MANIFEST}: {error}") from error
    validate_manifest(manifest)
    validate_runtime_platforms(manifest)
    validate_allocation_capture_host()

    fixture = ROOT / manifest["fixture"]
    if not fixture.is_file():
        raise BenchmarkError(f"benchmark fixture does not exist: {fixture}")
    ensure_free_space(ROOT)
    environment = benchmark_environment()
    if args.record_baseline:
        ensure_baseline_start_clean(environment)

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
            _merge_driver_section(sections, section)

    append_parity_assertion(sections, driver_artifacts)

    candidate_cleanup_detail = run_candidate_cleanup_probe(
        fixture,
        args.timeout,
    )
    sections["31.1"]["assertions"].append(
        {
            "id": "candidate_failure_cleanup",
            "name": "renderer candidate launch failures cannot leak a child process",
            "passed": True,
            "detail": candidate_cleanup_detail,
        }
    )

    allocation_summary = run_allocation_capture(
        fixture,
        args.profile,
        allocation_timeout,
    )
    allocation_metrics, allocation_assertion, allocation_artifact = (
        fold_allocation_summary(allocation_summary, profile=args.profile)
    )
    sections["31.1"]["metrics"].extend(allocation_metrics)
    sections["31.1"]["assertions"].append(allocation_assertion)
    sections["31.1"].setdefault("notes", []).append(
        "Interval-created-and-still-live allocation count and byte distributions "
        "come from mandatory temporary exact-PID xctrace Allocations attachments over "
        "equivalent renderer-role passes. Measurement starts only after the "
        "Darwin recording-start notification. Rows are admitted only inside "
        "the role pass's decode-to-complete-presentation interval and exact "
        "PID birth/liveness bounds; raw traces and exports are deleted after "
        "aggregation. Host retained-block deltas remain a separate live-memory "
        "diagnostic."
    )
    driver_artifacts["macos"]["allocation_capture"] = allocation_artifact
    allocation_counts = {
        item["candidate"]: item["measurement_sample_count"]
        for item in allocation_artifact["candidate_totals"]
    }
    for candidate in ALLOCATION_CANDIDATES:
        sections["31.1"]["sample_counts"][
            f"runner.xctrace.{candidate}"
        ] = allocation_counts[candidate]

    for verification in manifest["verification_commands"]:
        elapsed, passed, detail, conformance_count = run_verification(
            verification, args.timeout
        )
        if conformance_count is None:
            raise BenchmarkError(
                f"{verification['name']} did not emit an exact runner count"
            )
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
        section["sample_counts"][
            "runner.production_conformance"
        ] = conformance_count

    report = {
        "schema_version": 1,
        "generated_at": dt.datetime.now(dt.timezone.utc).isoformat(),
        "profile": args.profile,
        "fixture": str(fixture.relative_to(ROOT)),
        "environment": environment,
        "driver_artifacts": driver_artifacts,
        "sections": list(sections.values()),
    }
    validate_report(report, manifest["required_sections"])

    failures = failed_assertions(report)
    if args.record_baseline:
        ensure_baseline_recordable(
            report,
            ending_environment=benchmark_environment(),
        )
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
