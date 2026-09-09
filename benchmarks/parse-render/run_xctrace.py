#!/usr/bin/env python3
"""Capture bounded cross-process allocation data and export exact per-process totals."""

from __future__ import annotations

import contextlib
import datetime as dt
import json
import os
import re
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import xml.etree.ElementTree as ET
from collections.abc import Iterator
from pathlib import Path
from typing import Any

BENCHMARKS_DIR = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(BENCHMARKS_DIR))

from process_control import (  # noqa: E402
    ManagedCommandError,
    ManagedCommandTimeout,
    ManagedProcess,
    TERMINATION_SIGNALS,
    blocked_termination_signals,
    raise_termination_exceptions,
    run_managed_command,
    termination_exceptions,
    wait_for_process_identities_gone,
)

DEFAULT_MAX_TRACE_BYTES = 2 * 1024 * 1024 * 1024
DEFAULT_MAX_EXPORT_BYTES = 256 * 1024 * 1024
DEFAULT_MIN_REMAINING_BYTES = 4 * 1024 * 1024 * 1024
POLL_SECONDS = 0.1


class CaptureError(RuntimeError):
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


def configured_bytes(name: str, default: int) -> int:
    raw = os.environ.get(name)
    if raw is None:
        return default
    try:
        value = int(raw)
    except ValueError as error:
        raise CaptureError(f"{name} must be a positive integer byte count") from error
    if value <= 0:
        raise CaptureError(f"{name} must be a positive integer byte count")
    return value


def trace_size_bytes(path: Path) -> int:
    if not path.exists():
        return 0
    if path.is_file() or path.is_symlink():
        return path.stat().st_size
    total = 0
    for directory, _subdirectories, files in os.walk(path, followlinks=False):
        for name in files:
            candidate = Path(directory) / name
            try:
                total += candidate.stat().st_size
            except FileNotFoundError:
                continue
    return total


def ensure_free_reserve(path: Path, min_remaining_bytes: int) -> None:
    free = shutil.disk_usage(path).free
    if free < min_remaining_bytes:
        raise CaptureError(
            f"allocation capture stopped with {free} free bytes; "
            f"minimum reserve is {min_remaining_bytes} bytes"
        )


def ensure_capture_budget(
    trace: Path,
    *,
    max_trace_bytes: int,
    min_remaining_bytes: int,
) -> None:
    size = trace_size_bytes(trace)
    if size > max_trace_bytes:
        raise CaptureError(
            f"allocation trace exceeded {max_trace_bytes} bytes: {size} bytes"
        )
    ensure_free_reserve(trace.parent, min_remaining_bytes)


def preflight_capture(
    trace: Path,
    *,
    max_trace_bytes: int,
    max_export_bytes: int,
    min_remaining_bytes: int,
) -> None:
    free = shutil.disk_usage(trace.parent).free
    required = max_trace_bytes + max_export_bytes + min_remaining_bytes
    if free < required:
        raise CaptureError(
            f"allocation capture requires {required} free bytes at {trace.parent}; "
            f"only {free} bytes are available"
        )


def remove_capture(path: Path) -> None:
    if not path.exists() and not path.is_symlink():
        return
    if path.is_dir() and not path.is_symlink():
        shutil.rmtree(path)
    else:
        path.unlink(missing_ok=True)


def spawn_notification_watcher(notification: str) -> subprocess.Popen[str]:
    return subprocess.Popen(
        ["notifyutil", "-1", notification],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        start_new_session=True,
        close_fds=True,
    )


def _signal_direct_group(process: subprocess.Popen[str], signum: int) -> None:
    if process.poll() is not None:
        return
    try:
        process_group = os.getpgid(process.pid)
    except ProcessLookupError:
        return
    if process_group != process.pid:
        raise CaptureError(
            f"process {process.pid} is not its process-group leader ({process_group})"
        )
    try:
        os.killpg(process_group, signum)
    except ProcessLookupError:
        pass


def terminate_direct_process(
    process: subprocess.Popen[str],
    grace_seconds: float = 2.0,
) -> tuple[str, str]:
    if process.poll() is not None:
        return process.communicate(timeout=grace_seconds)

    graceful_errors: list[BaseException] = []
    output: tuple[str, str] | None = None
    try:
        _signal_direct_group(process, signal.SIGTERM)
    except BaseException as error:
        graceful_errors.append(error)
    try:
        output = process.communicate(timeout=grace_seconds)
    except subprocess.TimeoutExpired:
        pass
    except BaseException as error:
        graceful_errors.append(error)

    if output is not None:
        raise_termination_exceptions(
            graceful_errors,
            label=f"multiple termination requests while cleaning process {process.pid}",
        )
        return output

    final_errors: list[BaseException] = []
    group_killed = False
    try:
        _signal_direct_group(process, signal.SIGKILL)
        group_killed = True
    except BaseException as error:
        final_errors.append(error)
        try:
            process.kill()
        except ProcessLookupError:
            pass
        except BaseException as direct_error:
            final_errors.append(direct_error)
    try:
        output = process.communicate(timeout=grace_seconds)
    except BaseException as error:
        final_errors.append(error)
        output = None

    errors = [*graceful_errors, *final_errors]
    if not group_killed or output is None or process.poll() is None:
        failure = CaptureError(
            f"could not confirm reap of direct process group {process.pid}: "
            + "; ".join(f"{type(error).__name__}: {error}" for error in errors)
        )
        terminations = [
            termination
            for error in errors
            for termination in termination_exceptions(error)
        ]
        if terminations:
            raise BaseExceptionGroup(
                f"termination requested and direct process group {process.pid} cleanup failed",
                [*terminations, failure],
            )
        raise failure

    raise_termination_exceptions(
        errors,
        label=f"multiple termination requests while cleaning process {process.pid}",
    )
    return output


def wait_for_recording_notification(
    watcher: subprocess.Popen[str],
    trace: Path,
    *,
    max_trace_bytes: int,
    min_remaining_bytes: int,
) -> None:
    deadline = time.monotonic() + 15
    while watcher.poll() is None:
        ensure_capture_budget(
            trace,
            max_trace_bytes=max_trace_bytes,
            min_remaining_bytes=min_remaining_bytes,
        )
        if time.monotonic() >= deadline:
            raise CaptureError(
                "xctrace did not begin recording within 15s; grant Instruments "
                "automation and Developer Tools privacy access"
            )
        time.sleep(POLL_SECONDS)

    _watch_stdout, watch_stderr = watcher.communicate()
    ensure_capture_budget(
        trace,
        max_trace_bytes=max_trace_bytes,
        min_remaining_bytes=min_remaining_bytes,
    )
    if watcher.returncode:
        raise CaptureError(
            f"notifyutil failed ({watcher.returncode}): {watch_stderr[-1000:]}"
        )


def trace_command(trace: Path, notification: str) -> list[str]:
    return [
        "xcrun",
        "xctrace",
        "record",
        "--template",
        "Allocations",
        "--time-limit",
        "90s",
        "--window",
        "30s",
        "--no-prompt",
        "--notify-tracing-started",
        notification,
        "--output",
        str(trace),
        "--all-processes",
    ]


def driver_command(binary: Path, fixture: Path, result: Path) -> list[str]:
    return [
        str(binary),
        "--fixture",
        str(fixture),
        "--profile",
        "full",
        "--only-section",
        "31.1",
        "--output",
        str(result),
    ]


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


def _bounded_file(path: Path, maximum: int, label: str) -> None:
    size = path.stat().st_size if path.exists() else 0
    if size > maximum:
        raise CaptureError(f"{label} exceeded {maximum} bytes: {size} bytes")


def _export_command(
    trace: Path,
    arguments: list[str],
    *,
    timeout: float,
    maximum_output_bytes: int,
) -> str:
    result = run_managed_command(
        ["xcrun", "xctrace", "export", "--input", str(trace), *arguments],
        cwd=trace.parent,
        timeout=timeout,
        label="xctrace export",
        poll_hook=lambda process: _bounded_file(
            process.stdout_path,
            maximum_output_bytes,
            "xctrace export",
        ),
    )
    if result.returncode:
        detail = (result.stderr or result.stdout).strip()
        raise CaptureError(f"xctrace export failed ({result.returncode}): {detail[-2000:]}")
    return result.stdout


def _schema_names(toc: str) -> list[str]:
    try:
        root = ET.fromstring(toc)
    except ET.ParseError as error:
        raise CaptureError(f"xctrace TOC is invalid XML: {error}") from error
    return [
        element.attrib["schema"]
        for element in root.iter()
        if element.tag.rsplit("}", 1)[-1] == "table" and "schema" in element.attrib
    ]


_XCTRACE_START_DATE = re.compile(
    r"^(?P<clock>\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2})"
    r"(?:\.(?P<fraction>\d{1,9}))?"
    r"(?P<zone>Z|[+-]\d{2}:?\d{2})$"
)


def trace_start_unix_ns(toc: str) -> int:
    """Convert xctrace's run start-date to exact Unix epoch nanoseconds."""

    try:
        root = ET.fromstring(toc)
    except ET.ParseError as error:
        raise CaptureError(f"xctrace TOC is invalid XML: {error}") from error
    values = [
        (element.text or "").strip()
        for element in root.iter()
        if element.tag.rsplit("}", 1)[-1] == "start-date"
        and (element.text or "").strip()
    ]
    if len(values) != 1:
        raise CaptureError(
            "xctrace TOC must contain exactly one timestamped run start-date"
        )
    match = _XCTRACE_START_DATE.fullmatch(values[0])
    if match is None:
        raise CaptureError(f"xctrace start-date is not ISO-8601: {values[0]!r}")
    zone = match.group("zone")
    if zone == "Z":
        zone = "+00:00"
    elif ":" not in zone:
        zone = f"{zone[:3]}:{zone[3:]}"
    try:
        parsed = dt.datetime.fromisoformat(f"{match.group('clock')}{zone}")
    except ValueError as error:
        raise CaptureError(f"xctrace start-date is invalid: {values[0]!r}") from error
    utc = parsed.astimezone(dt.timezone.utc)
    delta = utc - dt.datetime(1970, 1, 1, tzinfo=dt.timezone.utc)
    whole_seconds = delta.days * 86_400 + delta.seconds
    fraction = (match.group("fraction") or "").ljust(9, "0")
    return whole_seconds * 1_000_000_000 + (int(fraction) if fraction else 0)


def _element_text(element: ET.Element) -> str | None:
    formatted = element.attrib.get("fmt") or element.attrib.get("name")
    if formatted:
        return formatted
    if element.text and element.text.strip():
        return element.text.strip()
    return None


def _resolve(element: ET.Element, identities: dict[str, ET.Element]) -> ET.Element:
    reference = element.attrib.get("ref")
    return identities.get(reference, element) if reference else element


def _numeric_value(element: ET.Element, identities: dict[str, ET.Element]) -> int | None:
    resolved = _resolve(element, identities)
    raw = resolved.text.strip() if resolved.text and resolved.text.strip() else None
    for candidate in (raw, resolved.attrib.get("fmt")):
        if not candidate:
            continue
        normalized = candidate.replace(",", "").strip()
        try:
            return max(0, int(float(normalized)))
        except ValueError:
            match = re.fullmatch(
                r"([0-9]+(?:\.[0-9]+)?)\s*(bytes?|[KMGT]i?B)",
                normalized,
                re.IGNORECASE,
            )
            if not match:
                continue
            multiplier = {
                "byte": 1,
                "bytes": 1,
                "kb": 1000,
                "kib": 1024,
                "mb": 1000**2,
                "mib": 1024**2,
                "gb": 1000**3,
                "gib": 1024**3,
                "tb": 1000**4,
                "tib": 1024**4,
            }[match.group(2).lower()]
            return int(float(match.group(1)) * multiplier)
    return None


def _timestamp_value(
    element: ET.Element,
    identities: dict[str, ET.Element],
) -> int | None:
    """Read xctrace engineering time, whose raw integer unit is nanoseconds."""

    resolved = _resolve(element, identities)
    raw = resolved.text.strip() if resolved.text and resolved.text.strip() else None
    if raw is None or re.fullmatch(r"[0-9]+", raw) is None:
        return None
    return int(raw)


def _timestamp_column(columns: dict[str, str]) -> tuple[str, str]:
    for preferred in ("start-time", "event-time", "time", "timestamp"):
        for logical, engineering in columns.items():
            if preferred in {logical.lower(), engineering.lower()}:
                return logical, engineering
    raise CaptureError("allocation export schema has no nanosecond timestamp column")


def _parse_process_label(value: str) -> tuple[str, int | None]:
    value = value.strip()
    match = re.match(r"^(.*?)\s*[\[(](\d+)[\])]$", value)
    if match:
        return match.group(1).strip(), int(match.group(2))
    return value, None


def _process_identity(
    row: ET.Element,
    identities: dict[str, ET.Element],
) -> tuple[str, int | None]:
    candidates: list[ET.Element] = []
    for child in row:
        resolved = _resolve(child, identities)
        candidates.append(resolved)
        candidates.extend(resolved.iter())
    for element in candidates:
        tag = element.tag.rsplit("}", 1)[-1].lower()
        if "process" not in tag:
            continue
        name = element.attrib.get("name")
        pid_text = element.attrib.get("pid")
        formatted = element.attrib.get("fmt") or _element_text(element)
        pid = int(pid_text) if pid_text and pid_text.isdigit() else None
        if name:
            return name, pid
        if formatted:
            parsed_name, parsed_pid = _parse_process_label(formatted)
            return parsed_name, pid if pid is not None else parsed_pid

    for element in candidates:
        formatted = element.attrib.get("fmt")
        if formatted and re.search(r"[\[(]\d+[\])]$", formatted):
            return _parse_process_label(formatted)
    return "unattributed", None


ATTRIBUTION_KEYS = {
    "candidate",
    "driver_pid",
    "host_pid",
    "helper_pids",
    "started_unix_ns",
    "ended_unix_ns",
    "helper_pid_source",
    "process_identities",
}


def load_renderer_process_attribution(
    result_path: Path,
    *,
    driver_pid: int,
) -> list[dict[str, Any]]:
    try:
        payload = json.loads(result_path.read_text(encoding="utf-8"))
        attribution = payload["artifacts"]["renderer_process_attribution"]
    except (OSError, json.JSONDecodeError, KeyError, TypeError) as error:
        raise CaptureError(
            "BenchmarkDriver result lacks renderer_process_attribution"
        ) from error
    if not isinstance(attribution, list) or len(attribution) != 2:
        raise CaptureError(
            "renderer_process_attribution must contain exactly srui and webkit"
        )

    validated: list[dict[str, Any]] = []
    seen_candidates: set[str] = set()
    claimed_pids: set[int] = set()
    for item in attribution:
        if not isinstance(item, dict) or set(item) != ATTRIBUTION_KEYS:
            raise CaptureError(
                "renderer_process_attribution item has an invalid field contract"
            )
        candidate = item["candidate"]
        if candidate not in {"srui", "webkit"} or candidate in seen_candidates:
            raise CaptureError(
                "renderer_process_attribution candidates must be unique srui and webkit"
            )
        integer_fields = (
            item["driver_pid"],
            item["host_pid"],
            item["started_unix_ns"],
            item["ended_unix_ns"],
        )
        if any(
            isinstance(value, bool) or not isinstance(value, int) or value <= 0
            for value in integer_fields
        ):
            raise CaptureError(
                f"{candidate} renderer process attribution has invalid PID/timestamp values"
            )
        if item["driver_pid"] != driver_pid:
            raise CaptureError(
                f"{candidate} attribution driver_pid {item['driver_pid']} "
                f"does not match launched BenchmarkDriver pid {driver_pid}"
            )
        if item["host_pid"] == driver_pid or item["host_pid"] in claimed_pids:
            raise CaptureError(f"{candidate} attribution has an invalid candidate host PID")
        if item["started_unix_ns"] > item["ended_unix_ns"]:
            raise CaptureError(f"{candidate} attribution interval is reversed")
        helper_pids = item["helper_pids"]
        if (
            not isinstance(helper_pids, list)
            or any(
                isinstance(pid, bool) or not isinstance(pid, int) or pid <= 0
                for pid in helper_pids
            )
            or len(helper_pids) != len(set(helper_pids))
            or item["host_pid"] in helper_pids
        ):
            raise CaptureError(f"{candidate} attribution helper_pids are invalid")
        if candidate == "srui" and helper_pids:
            raise CaptureError("srui attribution must not claim helper processes")
        item_pids = {item["host_pid"], *helper_pids}
        identities = item["process_identities"]
        if (
            not isinstance(identities, list)
            or any(
                not isinstance(identity, dict)
                or set(identity) != {"pid", "birth_unix_ns"}
                or isinstance(identity["pid"], bool)
                or not isinstance(identity["pid"], int)
                or identity["pid"] <= 0
                or isinstance(identity["birth_unix_ns"], bool)
                or not isinstance(identity["birth_unix_ns"], int)
                or identity["birth_unix_ns"] <= 0
                for identity in identities
            )
        ):
            raise CaptureError(f"{candidate} process identities are invalid")
        identity_pids = [identity["pid"] for identity in identities]
        if len(identity_pids) != len(set(identity_pids)) or set(identity_pids) != item_pids:
            raise CaptureError(
                f"{candidate} process identities must exactly cover host and helper PIDs"
            )
        if item_pids & claimed_pids:
            raise CaptureError("renderer process attribution reuses a claimed PID")
        if (
            not isinstance(item["helper_pid_source"], str)
            or not item["helper_pid_source"].strip()
        ):
            raise CaptureError(f"{candidate} attribution helper_pid_source is empty")
        seen_candidates.add(candidate)
        claimed_pids.update(item_pids)
        validated.append(item)

    if seen_candidates != {"srui", "webkit"}:
        raise CaptureError(
            "renderer_process_attribution must contain exactly srui and webkit"
        )
    return sorted(validated, key=lambda item: item["candidate"])


def parse_allocation_totals(
    xml_path: Path,
    *,
    candidate_attribution: list[dict[str, Any]],
    trace_started_unix_ns: int,
) -> dict[str, Any]:
    try:
        root = ET.parse(xml_path).getroot()
    except ET.ParseError as error:
        raise CaptureError(f"allocation export is invalid XML: {error}") from error

    identities = {
        element.attrib["id"]: element
        for element in root.iter()
        if "id" in element.attrib
    }
    schema = next(
        (
            element
            for element in root.iter()
            if element.tag.rsplit("}", 1)[-1] == "schema"
        ),
        None,
    )
    columns: dict[str, str] = {}
    if schema is not None:
        for column in schema:
            if column.tag.rsplit("}", 1)[-1] != "col":
                continue
            mnemonic = column.findtext("mnemonic")
            engineering = column.findtext("engineering-type")
            if mnemonic:
                columns[mnemonic] = engineering or mnemonic
    timestamp_logical, timestamp_engineering = _timestamp_column(columns)

    claimed_intervals = {
        pid: (item["started_unix_ns"], item["ended_unix_ns"])
        for item in candidate_attribution
        for pid in [item["host_pid"], *item["helper_pids"]]
    }
    aggregates: dict[int, dict[str, Any]] = {}
    unattributed_rows = 0
    excluded_unrelated_rows = 0
    excluded_outside_interval_rows = 0
    total_rows = 0
    for row in root.iter():
        if row.tag.rsplit("}", 1)[-1] != "row":
            continue
        cells = {child.tag.rsplit("}", 1)[-1]: child for child in row}
        size_element: ET.Element | None = None
        for logical, engineering in columns.items():
            if logical in {"size", "allocated-size", "allocation-size"}:
                size_element = cells.get(logical)
                if size_element is None:
                    size_element = cells.get(engineering)
                if size_element is not None:
                    break
        if size_element is None:
            for name, cell in cells.items():
                if name in {"size", "allocated-size", "allocation-size"}:
                    size_element = cell
                    break
        if size_element is None:
            continue
        size = _numeric_value(size_element, identities)
        if size is None:
            continue

        timestamp_element = cells.get(timestamp_logical)
        if timestamp_element is None:
            timestamp_element = cells.get(timestamp_engineering)
        timestamp_relative_ns = (
            _timestamp_value(timestamp_element, identities)
            if timestamp_element is not None
            else None
        )
        if timestamp_relative_ns is None:
            raise CaptureError(
                "allocation export row has an unavailable or unparseable "
                "nanosecond timestamp"
            )
        timestamp_unix_ns = trace_started_unix_ns + timestamp_relative_ns
        total_rows += 1

        name, pid = _process_identity(row, identities)
        if pid is None:
            unattributed_rows += 1
            continue
        interval = claimed_intervals.get(pid)
        if interval is None:
            excluded_unrelated_rows += 1
            continue
        if not interval[0] <= timestamp_unix_ns <= interval[1]:
            # Numeric PIDs can be reused during an all-process capture. Rows from
            # outside the candidate's explicit lifetime are never attributed.
            excluded_outside_interval_rows += 1
            continue

        aggregate = aggregates.setdefault(
            pid,
            {
                "pid": pid,
                "names": [],
                "cumulative_allocations": 0,
                "cumulative_bytes": 0,
            },
        )
        if name not in aggregate["names"]:
            aggregate["names"].append(name)
        aggregate["cumulative_allocations"] += 1
        aggregate["cumulative_bytes"] += size

    candidates: list[dict[str, Any]] = []
    for item in candidate_attribution:
        host_pid = item["host_pid"]
        if host_pid not in aggregates:
            raise CaptureError(
                f"allocation export contains no in-interval rows for "
                f"{item['candidate']} candidate host pid {host_pid}"
            )
        helper_totals = [
            aggregates[pid]
            for pid in item["helper_pids"]
            if pid in aggregates
        ]
        candidates.append(
            {
                **item,
                "host_process_totals": aggregates[host_pid],
                "helper_process_totals": sorted(
                    helper_totals,
                    key=lambda value: value["pid"],
                ),
                "helpers_without_allocation_rows": sorted(
                    pid for pid in item["helper_pids"] if pid not in aggregates
                ),
            }
        )

    return {
        "allocation_rows": total_rows,
        "allocation_timestamp_basis": (
            "xctrace relative nanoseconds added to trace start-date"
        ),
        "trace_started_unix_ns": trace_started_unix_ns,
        "unattributed_rows": unattributed_rows,
        "excluded_unrelated_process_rows": excluded_unrelated_rows,
        "excluded_outside_candidate_interval_rows": excluded_outside_interval_rows,
        "processes_with_allocations": len(aggregates),
        "candidate_processes": sorted(
            candidates,
            key=lambda item: item["candidate"],
        ),
    }


def export_allocation_summary(
    trace: Path,
    sidecar: Path,
    *,
    candidate_attribution: list[dict[str, Any]],
    max_export_bytes: int,
    min_remaining_bytes: int,
) -> None:
    toc = _export_command(
        trace,
        ["--toc"],
        timeout=60,
        maximum_output_bytes=4 * 1024 * 1024,
    )
    schemas = _schema_names(toc)
    trace_started_unix_ns = trace_start_unix_ns(toc)
    allocation_schema = next(
        (schema for schema in schemas if schema == "allocations"),
        next((schema for schema in schemas if "allocation" in schema.lower()), None),
    )
    if allocation_schema is None:
        raise CaptureError("xctrace TOC contains no allocation schema")

    with tempfile.NamedTemporaryFile(
        dir=trace.parent,
        suffix=".allocations.xml",
        delete=False,
    ) as exported:
        export_path = Path(exported.name)
    try:
        def monitor_export(_process: ManagedProcess) -> None:
            _bounded_file(
                export_path,
                max_export_bytes,
                "allocation XML export",
            )
            ensure_free_reserve(trace.parent, min_remaining_bytes)

        result = run_managed_command(
            [
                "xcrun",
                "xctrace",
                "export",
                "--input",
                str(trace),
                "--output",
                str(export_path),
                "--xpath",
                f'/trace-toc/run/data/table[@schema="{allocation_schema}"]',
            ],
            cwd=trace.parent,
            timeout=300,
            label="allocation table export",
            poll_hook=monitor_export,
        )
        if result.returncode:
            detail = (result.stderr or result.stdout).strip()
            raise CaptureError(
                f"allocation table export failed ({result.returncode}): {detail[-2000:]}"
            )
        _bounded_file(export_path, max_export_bytes, "allocation XML export")
        ensure_free_reserve(trace.parent, min_remaining_bytes)
        attribution = parse_allocation_totals(
            export_path,
            candidate_attribution=candidate_attribution,
            trace_started_unix_ns=trace_started_unix_ns,
        )
    finally:
        export_path.unlink(missing_ok=True)

    payload = {
        "schema_version": 1,
        "capture_scope": "all_processes",
        "trace": str(trace),
        "trace_bytes": trace_size_bytes(trace),
        "allocation_schema": allocation_schema,
        **attribution,
    }
    temporary = sidecar.with_name(f".{sidecar.name}.{os.getpid()}.tmp")
    try:
        temporary.write_text(
            json.dumps(payload, indent=2, sort_keys=True, allow_nan=False) + "\n",
            encoding="utf-8",
        )
        os.replace(temporary, sidecar)
    finally:
        temporary.unlink(missing_ok=True)


def wait_for_attributed_processes_to_exit(
    candidate_attribution: list[dict[str, Any]],
) -> None:
    identities = [
        (identity["pid"], identity["birth_unix_ns"])
        for item in candidate_attribution
        for identity in item["process_identities"]
    ]
    try:
        wait_for_process_identities_gone(
            identities,
            label="renderer allocation candidates",
        )
    except ManagedCommandError as error:
        raise CaptureError(str(error)) from error


def _cleanup_failure(label: str, error: BaseException) -> BaseException:
    if termination_exceptions(error):
        error.add_note(f"{label} was attempted before this termination propagated")
        return error
    failure = CaptureError(f"{label}: {type(error).__name__}: {error}")
    failure.__cause__ = error
    return failure


def _terminate_managed_with_retry(
    process: ManagedProcess,
    *,
    label: str,
) -> None:
    errors: list[BaseException] = []
    for _attempt in range(2):
        if process.closed:
            break
        try:
            process.terminate()
        except BaseException as error:
            errors.append(error)
    if process.closed:
        raise_termination_exceptions(
            errors,
            label=f"multiple termination requests while cleaning {label}",
        )
        return

    supervisor_pid = (
        process.supervisor.pid
        if process.supervisor is not None
        else "unregistered"
    )
    failure = CaptureError(
        f"{label} remains pinned after two bounded cleanup attempts; "
        f"supervisor PID {supervisor_pid}, controls {process.ready_path.parent}"
    )
    failure.process_handle = process  # type: ignore[attr-defined]
    if errors:
        raise BaseExceptionGroup(
            f"{label} cleanup retry failed",
            [*errors, failure],
        )
    raise failure


def _drain_direct_process_with_retry(
    process: subprocess.Popen[str],
    *,
    label: str,
) -> None:
    errors: list[BaseException] = []
    for _attempt in range(2):
        try:
            terminate_direct_process(process)
        except BaseException as error:
            errors.append(error)
        else:
            raise_termination_exceptions(
                errors,
                label=f"multiple termination requests while cleaning {label}",
            )
            return

        try:
            reaped = process.poll() is not None
        except BaseException as error:
            errors.append(error)
            continue
        if reaped:
            try:
                process.communicate(timeout=2)
            except BaseException as error:
                errors.append(error)
            else:
                raise_termination_exceptions(
                    errors,
                    label=f"multiple termination requests while cleaning {label}",
                )
                return

    failure = CaptureError(
        f"{label} remains alive after two bounded cleanup attempts; "
        f"process PID {process.pid} retained"
    )
    failure.process_handle = process  # type: ignore[attr-defined]
    raise BaseExceptionGroup(
        f"{label} cleanup retry failed",
        [*errors, failure],
    )


def _terminate_direct_with_retry(
    process: subprocess.Popen[str],
    *,
    label: str,
) -> None:
    previous_mask = signal.pthread_sigmask(
        signal.SIG_BLOCK,
        TERMINATION_SIGNALS,
    )
    cleanup_error: BaseException | None = None
    try:
        _drain_direct_process_with_retry(process, label=label)
    except BaseException as error:
        cleanup_error = error

    deferred_termination: BaseException | None = None
    try:
        signal.pthread_sigmask(signal.SIG_SETMASK, previous_mask)
    except BaseException as error:
        deferred_termination = error

    if cleanup_error is not None and deferred_termination is not None:
        raise BaseExceptionGroup(
            f"{label} cleanup and deferred termination both failed",
            [cleanup_error, deferred_termination],
        )
    if cleanup_error is not None:
        raise cleanup_error
    if deferred_termination is not None:
        raise deferred_termination


def cleanup_capture(
    *,
    driver: ManagedProcess | None,
    recorder: ManagedProcess | None,
    watcher: subprocess.Popen[str] | None,
    trace: Path,
    sidecar: Path,
    retain_outputs: bool,
) -> list[BaseException]:
    errors: list[BaseException] = []
    process_actions: list[tuple[str, Any]] = []
    if driver is not None and not driver.closed:
        process_actions.append(
            (
                "BenchmarkDriver cleanup",
                lambda: _terminate_managed_with_retry(
                    driver,
                    label="BenchmarkDriver",
                ),
            )
        )
    if recorder is not None and not recorder.closed:
        process_actions.append(
            (
                "xctrace recorder cleanup",
                lambda: _terminate_managed_with_retry(
                    recorder,
                    label="xctrace recorder",
                ),
            )
        )
    if watcher is not None:
        process_actions.append(
            (
                "notification watcher cleanup",
                lambda: _terminate_direct_with_retry(
                    watcher,
                    label="notification watcher",
                ),
            )
        )

    for label, action in process_actions:
        try:
            action()
        except BaseException as error:
            errors.append(_cleanup_failure(label, error))

    if not retain_outputs or errors:
        for label, path in (
            ("incomplete trace removal", trace),
            ("incomplete sidecar removal", sidecar),
        ):
            try:
                remove_capture(path)
            except BaseException as error:
                errors.append(_cleanup_failure(label, error))
    return errors


def run(
    trace: Path,
    binary: Path,
    fixture: Path,
    result: Path,
    *,
    max_trace_bytes: int,
    max_export_bytes: int,
    min_remaining_bytes: int,
) -> int:
    sidecar = trace.with_name(f"{trace.name}.summary.json")
    preflight_capture(
        trace,
        max_trace_bytes=max_trace_bytes,
        max_export_bytes=max_export_bytes,
        min_remaining_bytes=min_remaining_bytes,
    )
    notification = f"dev.srui.benchmark.xctrace.{os.getpid()}"
    watcher: subprocess.Popen[str] | None = None
    recorder: ManagedProcess | None = None
    driver: ManagedProcess | None = None
    capture_complete = False
    failure: BaseException | None = None

    def monitor(_process: ManagedProcess) -> None:
        ensure_capture_budget(
            trace,
            max_trace_bytes=max_trace_bytes,
            min_remaining_bytes=min_remaining_bytes,
        )

    try:
        # The CLI installs handled signal exceptions. Defer them until the real
        # watcher handle is assigned so every spawned process remains reachable.
        with blocked_termination_signals():
            spawned_watcher = spawn_notification_watcher(notification)
            watcher = spawned_watcher

        recorder = ManagedProcess.start(
            trace_command(trace, notification),
            cwd=trace.parent,
            label="xctrace recorder",
        )

        wait_for_recording_notification(
            watcher,
            trace,
            max_trace_bytes=max_trace_bytes,
            min_remaining_bytes=min_remaining_bytes,
        )

        driver = ManagedProcess.start(
            driver_command(binary, fixture, result),
            cwd=trace.parent,
            label="BenchmarkDriver allocation workload",
        )
        driver_result = driver.wait(55, poll_hook=monitor)
        if driver_result.returncode:
            detail = (driver_result.stderr or driver_result.stdout).strip()
            raise CaptureError(
                f"BenchmarkDriver failed ({driver_result.returncode}): {detail[-2000:]}"
            )

        candidate_attribution = load_renderer_process_attribution(
            result,
            driver_pid=driver_result.child_pid,
        )
        wait_for_attributed_processes_to_exit(candidate_attribution)
        recorder.signal(signal.SIGINT)
        recorder_result = recorder.wait(60, poll_hook=monitor)
        if recorder_result.returncode:
            detail = (recorder_result.stderr or recorder_result.stdout).strip()
            raise CaptureError(
                f"xctrace failed ({recorder_result.returncode}): {detail[-2000:]}"
            )
        ensure_capture_budget(
            trace,
            max_trace_bytes=max_trace_bytes,
            min_remaining_bytes=min_remaining_bytes,
        )
        if not trace.exists():
            raise CaptureError("xctrace completed without a trace bundle")

        export_allocation_summary(
            trace,
            sidecar,
            candidate_attribution=candidate_attribution,
            max_export_bytes=max_export_bytes,
            min_remaining_bytes=min_remaining_bytes,
        )
        capture_complete = True
    except BaseException as error:
        failure = error

    cleanup_errors = cleanup_capture(
        driver=driver,
        recorder=recorder,
        watcher=watcher,
        trace=trace,
        sidecar=sidecar,
        retain_outputs=capture_complete and failure is None,
    )
    errors = ([failure] if failure is not None else []) + cleanup_errors
    if len(errors) == 1:
        raise errors[0]
    if errors:
        raise BaseExceptionGroup("allocation capture and cleanup failed", errors)
    return 0


def main(argv: list[str] | None = None) -> int:
    arguments = sys.argv[1:] if argv is None else argv
    if len(arguments) != 4:
        print("usage: run_xctrace.py TRACE BINARY FIXTURE RESULT", file=sys.stderr)
        return 2
    trace, binary, fixture, result = map(Path, arguments)
    sidecar = trace.with_name(f"{trace.name}.summary.json")
    if trace.exists() or sidecar.exists():
        print(f"capture output already exists: {trace} or {sidecar}", file=sys.stderr)
        return 2
    if not trace.parent.is_dir():
        print(f"trace parent does not exist: {trace.parent}", file=sys.stderr)
        return 2
    if not binary.is_file():
        print(f"BenchmarkDriver does not exist: {binary}", file=sys.stderr)
        return 2
    if not fixture.is_file():
        print(f"fixture does not exist: {fixture}", file=sys.stderr)
        return 2

    return run(
        trace,
        binary,
        fixture,
        result,
        max_trace_bytes=configured_bytes(
            "SRUI_XCTRACE_MAX_BYTES",
            DEFAULT_MAX_TRACE_BYTES,
        ),
        max_export_bytes=configured_bytes(
            "SRUI_XCTRACE_MAX_EXPORT_BYTES",
            DEFAULT_MAX_EXPORT_BYTES,
        ),
        min_remaining_bytes=configured_bytes(
            "SRUI_XCTRACE_MIN_FREE_BYTES",
            DEFAULT_MIN_REMAINING_BYTES,
        ),
    )


def cli() -> int:
    try:
        with termination_handlers():
            return main()
    except TerminationRequested as error:
        print(
            f"allocation capture interrupted by {signal.Signals(error.signum).name}",
            file=sys.stderr,
        )
        return 128 + error.signum
    except KeyboardInterrupt:
        print("allocation capture interrupted by SIGINT", file=sys.stderr)
        return 130
    except (CaptureError, ManagedCommandError, ManagedCommandTimeout, OSError) as error:
        print(f"allocation capture failed: {error}", file=sys.stderr)
        return 2
    except BaseExceptionGroup as error:
        terminations = termination_exceptions(error)
        if terminations:
            first = terminations[0]
            if isinstance(first, TerminationRequested):
                print(
                    f"allocation capture interrupted by {signal.Signals(first.signum).name}",
                    file=sys.stderr,
                )
                return 128 + first.signum
            if isinstance(first, KeyboardInterrupt):
                print("allocation capture interrupted by SIGINT", file=sys.stderr)
                return 130
            raise first
        print(
            f"allocation capture cleanup failed: {exception_group_detail(error)}",
            file=sys.stderr,
        )
        return 2


if __name__ == "__main__":
    raise SystemExit(cli())
