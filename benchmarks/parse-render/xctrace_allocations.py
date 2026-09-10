"""Strict Xcode 26 Allocations exports for the Task 34 renderer benchmark.

The xctrace CLI exposes Allocations as view-level ``detail`` queries.  The
Allocations List contains the live heap-and-anonymous-VM records at capture end;
Statistics contains whole-trace persistent/transient aggregates.  This module
deliberately keeps those two meanings separate.
"""

from __future__ import annotations

import datetime as dt
import re
import xml.etree.ElementTree as ET
from pathlib import Path
from typing import Any

STATISTICS_XPATH = (
    '/trace-toc/run[@number="1"]/tracks/track[@name="Allocations"]/'
    'details/detail[@name="Statistics"]'
)
ALLOCATIONS_LIST_XPATH = (
    '/trace-toc/run[@number="1"]/tracks/track[@name="Allocations"]/'
    'details/detail[@name="Allocations List"]'
)
ALLOCATION_DETAIL_NAMES = ("Statistics", "Allocations List")
ALLOCATION_LIST_TIMESTAMP_RESOLUTION_NS = 1_000


class AllocationExportError(ValueError):
    """The trace or exported allocation evidence does not meet its contract."""


def _local_name(element: ET.Element) -> str:
    return element.tag.rsplit("}", 1)[-1]


def _parse_xml_text(xml: str, label: str) -> ET.Element:
    try:
        return ET.fromstring(xml)
    except ET.ParseError as error:
        raise AllocationExportError(f"{label} is invalid XML: {error}") from error


def _parse_xml_path(path: Path, label: str) -> ET.Element:
    try:
        return ET.parse(path).getroot()
    except (OSError, ET.ParseError) as error:
        raise AllocationExportError(f"{label} is invalid XML: {error}") from error


def _single(elements: list[ET.Element], label: str) -> ET.Element:
    if len(elements) != 1:
        raise AllocationExportError(f"{label} must occur exactly once")
    return elements[0]


def _nonnegative_decimal(raw: str | None, label: str) -> int:
    if raw is None or re.fullmatch(r"[0-9]+", raw) is None:
        raise AllocationExportError(f"{label} must be a non-negative decimal integer")
    return int(raw)


_XCTRACE_START_DATE = re.compile(
    r"^(?P<clock>\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2})"
    r"(?:\.(?P<fraction>\d{1,9}))?"
    r"(?P<zone>Z|[+-]\d{2}:?\d{2})$"
)


def _parse_start_date(raw: str) -> tuple[int, int]:
    match = _XCTRACE_START_DATE.fullmatch(raw)
    if match is None:
        raise AllocationExportError(f"xctrace start-date is not ISO-8601: {raw!r}")
    zone = match.group("zone")
    if zone == "Z":
        zone = "+00:00"
    elif ":" not in zone:
        zone = f"{zone[:3]}:{zone[3:]}"
    try:
        parsed = dt.datetime.fromisoformat(f"{match.group('clock')}{zone}")
    except ValueError as error:
        raise AllocationExportError(f"xctrace start-date is invalid: {raw!r}") from error
    utc = parsed.astimezone(dt.timezone.utc)
    delta = utc - dt.datetime(1970, 1, 1, tzinfo=dt.timezone.utc)
    whole_seconds = delta.days * 86_400 + delta.seconds
    fraction_raw = match.group("fraction") or ""
    fraction = fraction_raw.ljust(9, "0")
    unix_ns = whole_seconds * 1_000_000_000 + (int(fraction) if fraction else 0)
    resolution_ns = 10 ** (9 - len(fraction_raw)) if fraction_raw else 1_000_000_000
    return unix_ns, resolution_ns


def validate_allocation_toc(toc: str, *, requested_pid: int) -> dict[str, Any]:
    """Validate that a trace is one Allocations attachment to ``requested_pid``."""

    if isinstance(requested_pid, bool) or not isinstance(requested_pid, int) or requested_pid <= 0:
        raise AllocationExportError(f"requested PID is invalid: {requested_pid!r}")
    root = _parse_xml_text(toc, "xctrace TOC")
    runs = [element for element in root.iter() if _local_name(element) == "run"]
    run = _single(runs, "xctrace run")
    if run.attrib.get("number") != "1":
        raise AllocationExportError("xctrace run must have number=1")

    attached = [
        element
        for element in run.iter()
        if _local_name(element) == "process" and element.attrib.get("type") == "attached"
    ]
    process = _single(attached, "attached target process")
    attached_pid = _nonnegative_decimal(process.attrib.get("pid"), "attached target pid")
    if attached_pid != requested_pid:
        raise AllocationExportError(
            f"xctrace attached PID {attached_pid} does not match requested PID {requested_pid}"
        )
    process_name = process.attrib.get("name", "").strip()
    if not process_name:
        raise AllocationExportError("attached target process has no name")

    tracks = [
        element
        for element in run.iter()
        if _local_name(element) == "track" and element.attrib.get("name") == "Allocations"
    ]
    track = _single(tracks, "Allocations track")
    details = [element for element in track.iter() if _local_name(element) == "detail"]
    for detail_name in ALLOCATION_DETAIL_NAMES:
        detail = _single(
            [element for element in details if element.attrib.get("name") == detail_name],
            f"Allocations/{detail_name} detail",
        )
        if detail.attrib.get("kind") != "table":
            raise AllocationExportError(
                f"Allocations/{detail_name} detail must be a table"
            )

    start_dates = [
        (element.text or "").strip()
        for element in run.iter()
        if _local_name(element) == "start-date" and (element.text or "").strip()
    ]
    start_date = _single(
        [ET.Element("start-date", {"value": value}) for value in start_dates],
        "xctrace run start-date",
    ).attrib["value"]
    trace_started_unix_ns, start_resolution_ns = _parse_start_date(start_date)

    template_names = [
        (element.text or "").strip()
        for element in run.iter()
        if _local_name(element) == "template-name" and (element.text or "").strip()
    ]
    if template_names != ["Allocations"]:
        raise AllocationExportError("xctrace template-name must be exactly Allocations")
    instrument_versions = [
        (element.text or "").strip()
        for element in run.iter()
        if _local_name(element) == "instruments-version" and (element.text or "").strip()
    ]
    if len(instrument_versions) != 1:
        raise AllocationExportError("xctrace TOC must identify one Instruments version")
    devices = [element for element in run.iter() if _local_name(element) == "device"]
    device = _single(devices, "xctrace device")

    return {
        "attached_pid": attached_pid,
        "attached_process_name": process_name,
        "trace_start_date": start_date,
        "trace_started_unix_ns": trace_started_unix_ns,
        "trace_start_timestamp_resolution_ns": start_resolution_ns,
        "allocation_list_timestamp_resolution_ns": (
            ALLOCATION_LIST_TIMESTAMP_RESOLUTION_NS
        ),
        "timestamp_boundary_uncertainty_ns": (
            start_resolution_ns + ALLOCATION_LIST_TIMESTAMP_RESOLUTION_NS
        ),
        "instruments_version": instrument_versions[0],
        "platform": device.attrib.get("platform", "unknown"),
        "os_version": device.attrib.get("os-version", "unknown"),
        "statistics_xpath": STATISTICS_XPATH,
        "allocations_list_xpath": ALLOCATIONS_LIST_XPATH,
    }


_STATISTIC_FIELDS = {
    "persistent_allocations": "count-persistent",
    "persistent_bytes": "persistent-bytes",
    "transient_allocations": "count-transient",
    "transient_bytes": "transient-bytes",
    "total_allocations": "count-total",
    "total_bytes": "total-bytes",
    "event_count": "count-events",
}


_STATISTIC_CATEGORIES = {
    "heap_and_anonymous_vm": "All Heap & Anonymous VM",
    "heap": "All Heap Allocations",
    "anonymous_vm": "All Anonymous VM",
}


def _parse_statistics_row(row: ET.Element, *, label: str) -> dict[str, int]:
    statistics = {
        output_name: _nonnegative_decimal(
            row.attrib.get(attribute), f"{label} {attribute}"
        )
        for output_name, attribute in _STATISTIC_FIELDS.items()
    }
    if (
        statistics["persistent_allocations"] + statistics["transient_allocations"]
        != statistics["total_allocations"]
    ):
        raise AllocationExportError(
            f"{label} allocation count Statistics fields do not reconcile"
        )
    if (
        statistics["persistent_bytes"] + statistics["transient_bytes"]
        != statistics["total_bytes"]
    ):
        raise AllocationExportError(
            f"{label} allocation byte Statistics fields do not reconcile"
        )
    return statistics


def parse_allocation_statistics(path: Path) -> dict[str, dict[str, int]]:
    root = _parse_xml_path(path, "Allocations/Statistics export")
    nodes = [element for element in root.iter() if _local_name(element) == "node"]
    node = _single(nodes, "Allocations/Statistics result node")
    rows = [element for element in node if _local_name(element) == "row"]
    statistics = {
        key: _parse_statistics_row(
            _single(
                [row for row in rows if row.attrib.get("category") == category],
                f"{category} statistics row",
            ),
            label=category,
        )
        for key, category in _STATISTIC_CATEGORIES.items()
    }
    combined = statistics["heap_and_anonymous_vm"]
    heap = statistics["heap"]
    anonymous_vm = statistics["anonymous_vm"]
    for field in _STATISTIC_FIELDS:
        if combined[field] != heap[field] + anonymous_vm[field]:
            raise AllocationExportError(
                "All Heap & Anonymous VM Statistics do not equal All Heap "
                f"Allocations plus All Anonymous VM for {field}"
            )
    return statistics


_ELAPSED_TIMESTAMP = re.compile(
    r"^(?:(?P<hours>[0-9]+):)?(?P<minutes>[0-9]+):"
    r"(?P<seconds>[0-5][0-9])\.(?P<milliseconds>[0-9]{3})\."
    r"(?P<microseconds>[0-9]{3})$"
)


def parse_elapsed_timestamp_ns(raw: str) -> int:
    match = _ELAPSED_TIMESTAMP.fullmatch(raw)
    if match is None:
        raise AllocationExportError(
            f"allocation-list timestamp has unsupported format: {raw!r}"
        )
    hours = int(match.group("hours") or 0)
    minutes = int(match.group("minutes"))
    if match.group("hours") is not None and minutes >= 60:
        raise AllocationExportError(
            f"allocation-list timestamp has invalid minutes: {raw!r}"
        )
    seconds = int(match.group("seconds"))
    milliseconds = int(match.group("milliseconds"))
    microseconds = int(match.group("microseconds"))
    return (
        ((hours * 60 + minutes) * 60 + seconds) * 1_000_000_000
        + milliseconds * 1_000_000
        + microseconds * 1_000
    )


def parse_allocation_list(path: Path) -> list[dict[str, Any]]:
    root = _parse_xml_path(path, "Allocations/Allocations List export")
    nodes = [element for element in root.iter() if _local_name(element) == "node"]
    node = _single(nodes, "Allocations/Allocations List result node")
    rows: list[dict[str, Any]] = []
    identifiers: set[str] = set()
    for index, row in enumerate(
        element for element in node if _local_name(element) == "row"
    ):
        missing = {"timestamp", "size", "live", "identifier", "category"} - set(
            row.attrib
        )
        if missing:
            raise AllocationExportError(
                f"allocation-list row {index} is missing {sorted(missing)}"
            )
        if row.attrib["live"] != "true":
            raise AllocationExportError(
                f"allocation-list row {index} is not a live allocation"
            )
        identifier = row.attrib["identifier"]
        if not identifier or identifier in identifiers:
            raise AllocationExportError(
                f"allocation-list row {index} has a missing or duplicate identifier"
            )
        identifiers.add(identifier)
        rows.append(
            {
                "identifier": identifier,
                "category": row.attrib["category"],
                "timestamp_relative_ns": parse_elapsed_timestamp_ns(
                    row.attrib["timestamp"]
                ),
                "size": _nonnegative_decimal(
                    row.attrib["size"], f"allocation-list row {index} size"
                ),
            }
        )
    return rows


def summarize_allocation_exports(
    *,
    statistics_path: Path,
    allocations_list_path: Path,
    toc_metadata: dict[str, Any],
    target_pid: int,
    target_birth_unix_ns: int,
    observed_alive_through_unix_ns: int,
    measurement_started_unix_ns: int,
    measurement_ended_unix_ns: int,
) -> dict[str, Any]:
    """Return honest interval-retained evidence and whole-trace diagnostics."""

    if toc_metadata.get("attached_pid") != target_pid:
        raise AllocationExportError("TOC attachment does not match allocation target")
    integer_values = (
        target_pid,
        target_birth_unix_ns,
        observed_alive_through_unix_ns,
        measurement_started_unix_ns,
        measurement_ended_unix_ns,
    )
    if any(isinstance(value, bool) or not isinstance(value, int) or value <= 0 for value in integer_values):
        raise AllocationExportError("allocation target identity or interval is invalid")
    if not (
        target_birth_unix_ns
        <= measurement_started_unix_ns
        < measurement_ended_unix_ns
        <= observed_alive_through_unix_ns
    ):
        raise AllocationExportError("allocation measurement exceeds exact-process lifetime")
    statistics = parse_allocation_statistics(statistics_path)
    rows = parse_allocation_list(allocations_list_path)
    combined = statistics["heap_and_anonymous_vm"]
    list_bytes = sum(row["size"] for row in rows)
    vm_category_rows = [
        row for row in rows if row["category"].startswith("VM:")
    ]
    vm_category_bytes = sum(row["size"] for row in vm_category_rows)
    vm_categories: dict[str, tuple[int, int]] = {}
    for row in vm_category_rows:
        count, allocated_bytes = vm_categories.get(row["category"], (0, 0))
        vm_categories[row["category"]] = (
            count + 1,
            allocated_bytes + row["size"],
        )
    if (
        len(rows) != combined["persistent_allocations"]
        or list_bytes != combined["persistent_bytes"]
    ):
        raise AllocationExportError(
            "complete Allocations List does not reconcile with persistent All "
            "Heap & Anonymous VM Statistics totals: "
            f"list={len(rows)} allocations/{list_bytes} bytes, "
            f"VM:-category-diagnostic={len(vm_category_rows)} allocations/"
            f"{vm_category_bytes} bytes {vm_categories!r}, "
            f"statistics={combined['persistent_allocations']} allocations/"
            f"{combined['persistent_bytes']} bytes"
        )

    trace_started_unix_ns = toc_metadata.get("trace_started_unix_ns")
    uncertainty_ns = toc_metadata.get("timestamp_boundary_uncertainty_ns")
    if (
        isinstance(trace_started_unix_ns, bool)
        or not isinstance(trace_started_unix_ns, int)
        or trace_started_unix_ns <= 0
        or isinstance(uncertainty_ns, bool)
        or not isinstance(uncertainty_ns, int)
        or uncertainty_ns <= 0
    ):
        raise AllocationExportError("TOC timestamp metadata is invalid")

    nominal: list[dict[str, Any]] = []
    definite: list[dict[str, Any]] = []
    possible: list[dict[str, Any]] = []
    attach_baseline: list[dict[str, Any]] = []
    for row in rows:
        timestamp_unix_ns = trace_started_unix_ns + row["timestamp_relative_ns"]
        if row["timestamp_relative_ns"] == 0:
            attach_baseline.append(row)
        if measurement_started_unix_ns <= timestamp_unix_ns <= measurement_ended_unix_ns:
            nominal.append(row)
        if (
            measurement_started_unix_ns + uncertainty_ns
            < timestamp_unix_ns
            < measurement_ended_unix_ns - uncertainty_ns
        ):
            definite.append(row)
        if (
            measurement_started_unix_ns - uncertainty_ns
            <= timestamp_unix_ns
            <= measurement_ended_unix_ns + uncertainty_ns
        ):
            possible.append(row)

    def total_bytes(selected: list[dict[str, Any]]) -> int:
        return sum(row["size"] for row in selected)

    lower_count = len(definite)
    upper_count = len(possible)
    lower_bytes = total_bytes(definite)
    upper_bytes = total_bytes(possible)
    return {
        "allocation_rows": len(rows),
        "allocation_list_bytes": list_bytes,
        "allocation_list_reconciled": True,
        "vm_category_rows": len(vm_category_rows),
        "vm_category_bytes": vm_category_bytes,
        "whole_trace_statistics": statistics,
        "attach_baseline_allocations": len(attach_baseline),
        "attach_baseline_bytes": total_bytes(attach_baseline),
        "retained_allocations": len(nominal),
        "retained_bytes": total_bytes(nominal),
        "retained_allocations_lower_bound": lower_count,
        "retained_allocations_upper_bound": upper_count,
        "retained_bytes_lower_bound": lower_bytes,
        "retained_bytes_upper_bound": upper_bytes,
        "boundary_ambiguous_allocations": upper_count - lower_count,
        "boundary_ambiguous_bytes": upper_bytes - lower_bytes,
        "excluded_outside_measurement_interval_rows": len(rows) - len(nominal),
        "process_total": {
            "pid": target_pid,
            "birth_unix_ns": target_birth_unix_ns,
            "observed_alive_through_unix_ns": observed_alive_through_unix_ns,
            "names": [toc_metadata["attached_process_name"]],
            "retained_allocations": len(nominal),
            "retained_bytes": total_bytes(nominal),
            "retained_allocations_lower_bound": lower_count,
            "retained_allocations_upper_bound": upper_count,
            "retained_bytes_lower_bound": lower_bytes,
            "retained_bytes_upper_bound": upper_bytes,
            "boundary_ambiguous_allocations": upper_count - lower_count,
            "boundary_ambiguous_bytes": upper_bytes - lower_bytes,
        },
    }
