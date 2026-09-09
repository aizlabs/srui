#!/usr/bin/env python3
"""Capture bounded cross-process allocation data and export exact per-process totals."""

from __future__ import annotations

import contextlib
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
    run_managed_command,
)

DEFAULT_MAX_TRACE_BYTES = 2 * 1024 * 1024 * 1024
DEFAULT_MAX_EXPORT_BYTES = 256 * 1024 * 1024
DEFAULT_MIN_REMAINING_BYTES = 4 * 1024 * 1024 * 1024
POLL_SECONDS = 0.1


class CaptureError(RuntimeError):
    pass


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
    if process.poll() is None:
        _signal_direct_group(process, signal.SIGTERM)
    try:
        return process.communicate(timeout=grace_seconds)
    except subprocess.TimeoutExpired:
        _signal_direct_group(process, signal.SIGKILL)
        try:
            return process.communicate(timeout=grace_seconds)
        except subprocess.TimeoutExpired as error:
            raise CaptureError(f"could not reap process group {process.pid}") from error


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


def parse_allocation_totals(
    xml_path: Path,
    *,
    target_pid: int,
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

    aggregates: dict[tuple[str, int | None], dict[str, Any]] = {}
    unattributed_rows = 0
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
        total_rows += 1
        name, pid = _process_identity(row, identities)
        if pid is None and name == "unattributed":
            unattributed_rows += 1
            continue
        key = (name, pid)
        aggregate = aggregates.setdefault(
            key,
            {
                "name": name,
                "pid": pid,
                "cumulative_allocations": 0,
                "cumulative_bytes": 0,
            },
        )
        aggregate["cumulative_allocations"] += 1
        aggregate["cumulative_bytes"] += size

    target = [
        aggregate
        for aggregate in aggregates.values()
        if aggregate["pid"] == target_pid
    ]
    helpers = [
        aggregate
        for aggregate in aggregates.values()
        if aggregate["pid"] is not None
        and "webkit" in aggregate["name"].lower()
    ]
    if not target:
        raise CaptureError(
            f"allocation export contains no rows attributable to BenchmarkDriver pid {target_pid}"
        )

    key = lambda item: (
        item["name"],
        item["pid"] if item["pid"] is not None else -1,
    )
    return {
        "allocation_rows": total_rows,
        "unattributed_rows": unattributed_rows,
        "processes_with_allocations": len(aggregates),
        "target_processes": sorted(target, key=key),
        "webkit_helper_processes": sorted(helpers, key=key),
    }


def export_allocation_summary(
    trace: Path,
    sidecar: Path,
    *,
    target_pid: int,
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
        attribution = parse_allocation_totals(export_path, target_pid=target_pid)
    finally:
        export_path.unlink(missing_ok=True)

    payload = {
        "schema_version": 1,
        "capture_scope": "all_processes",
        "trace": str(trace),
        "trace_bytes": trace_size_bytes(trace),
        "allocation_schema": allocation_schema,
        "target_pid": target_pid,
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

    def monitor(_process: ManagedProcess) -> None:
        ensure_capture_budget(
            trace,
            max_trace_bytes=max_trace_bytes,
            min_remaining_bytes=min_remaining_bytes,
        )

    try:
        watcher = subprocess.Popen(
            ["notifyutil", "-1", notification],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            start_new_session=True,
            close_fds=True,
        )
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
        try:
            driver_result = driver.wait(55, poll_hook=monitor)
        finally:
            if not driver.closed:
                driver.terminate()
        if driver_result.returncode:
            detail = (driver_result.stderr or driver_result.stdout).strip()
            raise CaptureError(
                f"BenchmarkDriver failed ({driver_result.returncode}): {detail[-2000:]}"
            )

        target_pid = driver_result.child_pid
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
            target_pid=target_pid,
            max_export_bytes=max_export_bytes,
            min_remaining_bytes=min_remaining_bytes,
        )
        capture_complete = True
        return 0
    finally:
        if driver is not None and not driver.closed:
            driver.terminate()
        if recorder is not None and not recorder.closed:
            recorder.terminate()
        if watcher is not None:
            terminate_direct_process(watcher)
        if not capture_complete:
            remove_capture(trace)
            remove_capture(sidecar)


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


if __name__ == "__main__":
    raise SystemExit(cli())
