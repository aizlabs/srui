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
SIGNED_METRIC_IDS = frozenset(
    {
        "srui.host_net_live_allocation_blocks",
        "srui.host_net_live_allocation_bytes",
        "webkit.host_net_live_allocation_blocks",
        "webkit.host_net_live_allocation_bytes",
    }
)
PROFILE_DRIVER_ITERATIONS = {
    "smoke": {"rust": 25, "macos": 3},
    "full": {"rust": 500, "macos": 20},
}
PRODUCTION_CONFORMANCE_SAMPLE_COUNT = 9
METADATA_COMMAND_TIMEOUT_SECONDS = 10
WINDOW_ISOLATION_SELF_TEST_TIMEOUT_SECONDS = 30
WINDOW_ISOLATION_ASSERTION_ID = "window_isolation_fail_closed"
WINDOW_ISOLATION_SELF_TEST_PREFIX = "window isolation self-test passed:"
WINDOW_ISOLATION_SELF_TEST_PATTERN = re.compile(
    r"^window isolation self-test passed: "
    r"dock=(?P<dock>[0-9]+) status=(?P<status>[0-9]+) "
    r"ahead=(?P<ahead>[0-9]+) popup=(?P<popup>[0-9]+) "
    r"target=(?P<target>[0-9]+) occluder=(?P<occluder>[0-9]+)$"
)
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
                "srui.host_net_live_allocation_blocks": (
                    "blocks",
                    DISTRIBUTION,
                ),
                "srui.host_net_live_allocation_bytes": ("bytes", DISTRIBUTION),
                "srui.process_footprint_peak": ("MiB", ("max",)),
                "srui.process_footprint_growth": ("MiB", ("p50",)),
                "webkit.first_paint": ("ms", ("p50", "p95")),
                "webkit.complete_paint": ("ms", ("p50", "p95")),
                "webkit.cpu": ("ms", ("p50",)),
                "webkit.host_net_live_allocation_blocks": (
                    "blocks",
                    DISTRIBUTION,
                ),
                "webkit.host_net_live_allocation_bytes": ("bytes", DISTRIBUTION),
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
                "host_net_live_allocation_scope",
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
                    (f"interaction.{interaction}.rtt.{rtt}", statistic): (
                        LOCAL_FRAME_BUDGET_ID,
                        "max",
                    )
                    for interaction in LOCAL_INTERACTIONS
                    for rtt in (0, 100, 300, 600)
                    for statistic in DISTRIBUTION
                },
                **{
                    ("local_rtt_delta", statistic): (
                        LOCAL_FRAME_BUDGET_ID,
                        "max",
                    )
                    for statistic in DISTRIBUTION
                },
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
    merged["31.1"]["assertions"].update(
        {
            "candidate_failure_cleanup",
            WINDOW_ISOLATION_ASSERTION_ID,
        }
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


def expected_report_inventory_for_profile(
    section_id: str,
    profile: str,
) -> dict[str, Any]:
    inventory = EXPECTED_REPORT_INVENTORY[section_id]
    assertions = inventory["assertions"]
    if profile == "smoke" and section_id == "31.1":
        assertions = assertions - {WINDOW_ISOLATION_ASSERTION_ID}
    return {"metrics": inventory["metrics"], "assertions": assertions}


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
        "srui.host_net_live_allocation_blocks": (
            "SRUI host-process net live allocation block delta"
        ),
        "srui.host_net_live_allocation_bytes": (
            "SRUI host-process net live allocation byte delta"
        ),
        "srui.process_footprint_peak": (
            "SRUI maximum concurrently sampled process footprint"
        ),
        "srui.process_footprint_growth": (
            "SRUI host allocated footprint growth"
        ),
        "webkit.cpu": "WKWebView host plus attributed helper CPU time",
        "webkit.host_net_live_allocation_blocks": (
            "WKWebView comparison host-process net live allocation block delta"
        ),
        "webkit.host_net_live_allocation_bytes": (
            "WKWebView comparison host-process net live allocation byte delta"
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
    evidence = {key: int(value) for key, value in match.groupdict().items()}
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


def run_window_isolation_self_test(
    fixture: Path,
    timeout: int,
) -> dict[str, Any]:
    binary = ROOT / "client-macos/.build/release/BenchmarkDriver"
    if not binary.is_file():
        raise BenchmarkError(
            "window isolation self-test requires the built release BenchmarkDriver"
        )
    with tempfile.NamedTemporaryFile(suffix=".json", delete=False) as output:
        output_path = Path(output.name)
    command = [
        "/usr/bin/env",
        "SRUI_BENCHMARK_PHASES=1",
        "SRUI_BENCHMARK_WINDOW_ISOLATION_SELF_TEST=1",
        str(binary),
        "--fixture",
        str(fixture),
        "--profile",
        "full",
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
                label="window isolation self-test",
                poll_hook=lambda _process: ensure_free_space(ROOT),
            )
        except ManagedCommandTimeout as error:
            raise BenchmarkError(
                f"window isolation self-test timed out after {timeout}s"
            ) from error
        except ManagedCommandError as error:
            raise BenchmarkError(
                f"window isolation self-test process supervision failed: {error}"
            ) from error
        if result.returncode:
            detail = (result.stderr or result.stdout).strip()
            raise BenchmarkError(
                "window isolation self-test failed "
                f"({result.returncode}): {detail[-4000:]}"
            )
        evidence = parse_window_isolation_self_test_output(
            result.stdout,
            result.stderr,
        )
        detail = (
            f"window isolation self-test passed: dock={evidence['dock']} "
            f"status={evidence['status']} ahead={evidence['ahead']} "
            f"popup={evidence['popup']} target={evidence['target']} "
            f"occluder={evidence['occluder']}"
        )
        return {
            "id": WINDOW_ISOLATION_ASSERTION_ID,
            "name": "WindowServer isolation rejects an exact synthetic occluder",
            "passed": True,
            "detail": detail,
        }
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
        r"(?m)^\s*8\s+reconnect\s+PASS\s+([1-9][0-9]*)\s+runner\(s\)\s*$",
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
            expected_report_inventory_for_profile(section_id, report["profile"]),
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
            raise BenchmarkError("window isolation self-test assertion must pass")
        isolation_detail = window_isolation.get("detail")
        if not isinstance(isolation_detail, str):
            raise BenchmarkError(
                "window isolation self-test assertion must retain exact numeric detail"
            )
        parse_window_isolation_self_test_output("", isolation_detail)

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
                    f"§{section['id']} {metric['name']} ({metric['statistic']}): "
                    f"{metric['value']:.4g} {metric['unit']} vs target "
                    f"{metric['target']:g} {metric['unit']}"
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
    validate_runtime_platforms(manifest)

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
        if driver["name"] == "macos" and args.profile == "full":
            parse_render = next(
                section for section in payload["sections"] if section["id"] == "31.1"
            )
            parse_render["assertions"].append(
                run_window_isolation_self_test(
                    fixture,
                    min(args.timeout, WINDOW_ISOLATION_SELF_TEST_TIMEOUT_SECONDS),
                )
            )
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
