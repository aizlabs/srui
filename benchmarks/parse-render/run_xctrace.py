#!/usr/bin/env python3
"""Capture bounded cross-process allocation data and export exact per-process totals."""

from __future__ import annotations

import contextlib
import json
import os
import plistlib
import shutil
import signal
import sys
import tempfile
import time
from collections.abc import Iterator
from pathlib import Path
from typing import Any

PARSE_RENDER_DIR = Path(__file__).resolve().parent
BENCHMARKS_DIR = PARSE_RENDER_DIR.parent
sys.path.insert(0, str(PARSE_RENDER_DIR))
sys.path.insert(0, str(BENCHMARKS_DIR))
from process_control import (  # noqa: E402
    ManagedCommandError,
    ManagedCommandTimeout,
    ManagedProcess,
    blocked_termination_signals,
    non_termination_exceptions,
    process_birth_unix_ns,
    run_managed_command,
    terminate_managed_process_with_retry,
    termination_exceptions,
    wait_for_process_identities_gone,
)
from xctrace_allocations import (  # noqa: E402
    ALLOCATIONS_LIST_XPATH,
    STATISTICS_XPATH,
    AllocationExportError,
    summarize_allocation_exports,
    validate_allocation_toc,
)

DEFAULT_MAX_TRACE_BYTES = 2 * 1024 * 1024 * 1024
DEFAULT_MAX_EXPORT_BYTES = 256 * 1024 * 1024
DEFAULT_MIN_REMAINING_BYTES = 4 * 1024 * 1024 * 1024
POLL_SECONDS = 0.1
RECORDING_READINESS_TIMEOUT_SECONDS = 15
TARGET_CAPTURE_TIMEOUT_SECONDS = 60
CONTROL_FILE_TIMEOUT_SECONDS = 60
CANDIDATE_EXIT_TIMEOUT_SECONDS = 30
PROFILE_SAMPLE_COUNTS = {"smoke": 3, "full": 20}
WEBKIT_TARGET_ROLES = ("host", "webcontent", "network", "gpu")
CAPTURE_PASSES = (
    ("srui", "host"),
    *(("webkit", role) for role in WEBKIT_TARGET_ROLES),
)
OWNERSHIP_SENTINEL = ".srui-xctrace-owner"
BENCHMARK_DRIVER_ENTITLEMENTS = PARSE_RENDER_DIR / "BenchmarkDriver.entitlements"


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


class CaptureWorkspace:
    __slots__ = ("ownership_token", "path")

    def __init__(self, *, path: Path, ownership_token: str) -> None:
        self.path = path
        self.ownership_token = ownership_token


def _path_exists(path: Path) -> bool:
    return path.exists() or path.is_symlink()


def validate_output_destinations(
    trace: Path,
    sidecar: Path,
    result: Path,
) -> None:
    destinations = {
        "trace": trace,
        "summary": sidecar,
        "result": result,
    }
    canonical = [path.resolve(strict=False) for path in destinations.values()]
    if len(set(canonical)) != len(canonical):
        raise CaptureError("capture output paths must be distinct")
    for label, path in destinations.items():
        if not path.parent.is_dir():
            raise CaptureError(f"{label} output parent does not exist: {path.parent}")
        if _path_exists(path):
            raise CaptureError(f"{label} output already exists: {path}")


def create_capture_workspace(parent: Path) -> CaptureWorkspace:
    path = Path(tempfile.mkdtemp(prefix=".srui-xctrace-", dir=parent))
    token = os.urandom(32).hex()
    sentinel = path / OWNERSHIP_SENTINEL
    try:
        sentinel.write_text(token, encoding="ascii")
    except BaseException:
        # mkdtemp returned this exact directory, so it is safe to remove before
        # its ownership token has been published to the rest of the workflow.
        remove_capture(path)
        raise
    return CaptureWorkspace(path=path, ownership_token=token)


def remove_owned_capture(workspace: CaptureWorkspace) -> None:
    sentinel = workspace.path / OWNERSHIP_SENTINEL
    try:
        observed_token = sentinel.read_text(encoding="ascii")
    except OSError as error:
        raise CaptureError(
            f"refusing to remove unverified capture workspace: {workspace.path}"
        ) from error
    if observed_token != workspace.ownership_token:
        raise CaptureError(
            f"refusing to remove capture workspace with mismatched owner: "
            f"{workspace.path}"
        )
    remove_capture(workspace.path)


def _publish_file_exclusive(source: Path, destination: Path) -> None:
    try:
        os.link(source, destination, follow_symlinks=False)
    except OSError as error:
        raise CaptureError(
            f"could not publish {destination} exclusively; source and "
            "destination must be on the same filesystem"
        ) from error


def _rollback_linked_file(source: Path, destination: Path) -> None:
    if not _path_exists(destination):
        return
    try:
        source_status = source.stat(follow_symlinks=False)
        destination_status = destination.stat(follow_symlinks=False)
    except OSError as error:
        raise CaptureError(
            f"refusing to remove unverified published output: {destination}"
        ) from error
    if (
        source_status.st_dev,
        source_status.st_ino,
    ) != (
        destination_status.st_dev,
        destination_status.st_ino,
    ):
        raise CaptureError(
            f"refusing to remove replaced published output: {destination}"
        )
    destination.unlink()


def _publish_trace_exclusive(
    source: Path,
    destination: Path,
    *,
    owner_token: str,
) -> None:
    if source.is_file():
        _publish_file_exclusive(source, destination)
        return
    if not source.is_dir():
        raise CaptureError(f"staged allocation trace is missing: {source}")
    try:
        destination.mkdir(mode=0o700)
    except FileExistsError as error:
        raise CaptureError(f"trace output already exists: {destination}") from error
    sentinel = destination / OWNERSHIP_SENTINEL
    try:
        sentinel.write_text(owner_token, encoding="ascii")
        for child in source.iterdir():
            published_child = destination / child.name
            if child.is_symlink():
                published_child.symlink_to(os.readlink(child))
            elif child.is_dir():
                shutil.copytree(
                    child,
                    published_child,
                    symlinks=True,
                    copy_function=os.link,
                )
            else:
                os.link(child, published_child)
    except BaseException:
        _rollback_published_trace(source, destination, owner_token=owner_token)
        raise


def _rollback_published_trace(
    source: Path,
    destination: Path,
    *,
    owner_token: str,
) -> None:
    if not _path_exists(destination):
        return
    if source.is_file():
        _rollback_linked_file(source, destination)
        return
    sentinel = destination / OWNERSHIP_SENTINEL
    try:
        observed_token = sentinel.read_text(encoding="ascii")
    except OSError as error:
        raise CaptureError(
            f"refusing to remove unverified published trace: {destination}"
        ) from error
    if observed_token != owner_token:
        raise CaptureError(
            f"refusing to remove replaced published trace: {destination}"
        )
    remove_capture(destination)


def _finalize_published_trace(
    source: Path,
    destination: Path,
    *,
    owner_token: str,
) -> None:
    if source.is_file():
        return
    sentinel = destination / OWNERSHIP_SENTINEL
    try:
        observed_token = sentinel.read_text(encoding="ascii")
    except OSError as error:
        raise CaptureError(
            f"published trace lost its ownership sentinel: {destination}"
        ) from error
    if observed_token != owner_token:
        raise CaptureError(
            f"published trace ownership changed before commit: {destination}"
        )
    sentinel.unlink()


def publish_capture_outputs(
    *,
    staged_trace: Path,
    staged_sidecar: Path,
    staged_result: Path,
    trace: Path,
    sidecar: Path,
    result: Path,
) -> None:
    validate_output_destinations(trace, sidecar, result)
    if not staged_sidecar.is_file():
        raise CaptureError(f"staged allocation summary is missing: {staged_sidecar}")
    if not staged_result.is_file():
        raise CaptureError(f"staged BenchmarkDriver result is missing: {staged_result}")

    trace_owner_token = os.urandom(32).hex()
    try:
        _publish_trace_exclusive(
            staged_trace,
            trace,
            owner_token=trace_owner_token,
        )
        _publish_file_exclusive(staged_sidecar, sidecar)
        _publish_file_exclusive(staged_result, result)
        _finalize_published_trace(
            staged_trace,
            trace,
            owner_token=trace_owner_token,
        )
    except BaseException as publication_error:
        cleanup_errors: list[BaseException] = []
        for label, action in (
            (
                "published result rollback",
                lambda: _rollback_linked_file(staged_result, result),
            ),
            (
                "published summary rollback",
                lambda: _rollback_linked_file(staged_sidecar, sidecar),
            ),
            (
                "published trace rollback",
                lambda: _rollback_published_trace(
                    staged_trace,
                    trace,
                    owner_token=trace_owner_token,
                ),
            ),
        ):
            try:
                action()
            except BaseException as cleanup_error:
                cleanup_errors.append(_cleanup_failure(label, cleanup_error))
        if cleanup_errors:
            raise BaseExceptionGroup(
                "capture publication and rollback failed",
                [publication_error, *cleanup_errors],
            )
        raise


def notification_watcher_command(notification: str) -> list[str]:
    return ["notifyutil", "-1", notification]


def _managed_process_output_tail(
    process: ManagedProcess,
    *,
    maximum_bytes: int = 8 * 1024,
) -> str:
    chunks: list[str] = []
    for path in (process.stderr_path, process.stdout_path):
        try:
            with path.open("rb") as stream:
                stream.seek(0, os.SEEK_END)
                size = stream.tell()
                stream.seek(max(0, size - maximum_bytes))
                chunk = stream.read(maximum_bytes).decode(
                    "utf-8",
                    errors="replace",
                ).strip()
        except OSError:
            continue
        if chunk:
            chunks.append(chunk)
    return "\n".join(chunks)[-maximum_bytes:]


def _managed_process_timeout_diagnostic(process: ManagedProcess) -> str:
    try:
        child_pid: int | str | None = process.child_pid
    except Exception as error:
        child_pid = f"unavailable ({type(error).__name__}: {error})"
    if child_pid is None:
        child_pid = "unavailable"

    supervisor = process.supervisor
    if supervisor is None:
        supervisor_pid: int | str = "unavailable"
        supervisor_state = "not-started"
    else:
        supervisor_pid = supervisor.pid
        try:
            returncode = supervisor.poll()
        except Exception as error:
            supervisor_state = f"unknown ({type(error).__name__}: {error})"
        else:
            supervisor_state = (
                "running" if returncode is None else f"exited({returncode})"
            )

    output_tail = _managed_process_output_tail(process)
    return (
        f"{process.label}: child_pid={child_pid}, "
        f"supervisor_pid={supervisor_pid}, "
        f"supervisor_state={supervisor_state}, "
        f"ready_file={'present' if process.ready_path.exists() else 'absent'}, "
        f"status_file={'present' if process.status_path.exists() else 'absent'}, "
        f"output_tail={output_tail!r}"
    )


def wait_for_recording_readiness(
    watcher: ManagedProcess,
    recorder: ManagedProcess,
    capture_root: Path,
    *,
    max_trace_bytes: int,
    min_remaining_bytes: int,
) -> str:
    def monitor(_process: ManagedProcess) -> None:
        ensure_capture_budget(
            capture_root,
            max_trace_bytes=max_trace_bytes,
            min_remaining_bytes=min_remaining_bytes,
        )
        completed = _managed_process_result_if_exited(recorder)
        if completed is not None:
            returncode, detail = completed
            raise CaptureError(
                "xctrace exited before its recording-start notification "
                f"({returncode}): {detail[-2000:]}"
            )

    try:
        result = watcher.wait(
            RECORDING_READINESS_TIMEOUT_SECONDS,
            poll_hook=monitor,
        )
    except ManagedCommandTimeout as error:
        output = _managed_process_output_tail(recorder)
        detail = f"; recorder output: {output}" if output else ""
        raise CaptureError(
            "xctrace did not begin recording within "
            f"{RECORDING_READINESS_TIMEOUT_SECONDS}s; grant Instruments "
            "automation and Developer Tools privacy access"
            f"{detail}"
        ) from error
    except ManagedCommandError as error:
        raise CaptureError(
            f"xctrace recording notification watcher failed: {error}"
        ) from error

    ensure_capture_budget(
        capture_root,
        max_trace_bytes=max_trace_bytes,
        min_remaining_bytes=min_remaining_bytes,
    )
    if result.returncode:
        detail = (result.stderr or result.stdout).strip()
        raise CaptureError(f"notifyutil failed ({result.returncode}): {detail[-1000:]}")
    return "darwin_notification"


def trace_command(
    trace: Path,
    notification: str,
    *,
    target_pid: int,
) -> list[str]:
    if (
        isinstance(target_pid, bool)
        or not isinstance(target_pid, int)
        or target_pid <= 0
    ):
        raise CaptureError(f"invalid xctrace target PID: {target_pid!r}")
    time_limit = f"{TARGET_CAPTURE_TIMEOUT_SECONDS}s"
    return [
        "xcrun",
        "xctrace",
        "record",
        "--template",
        "Allocations",
        "--time-limit",
        time_limit,
        "--window",
        time_limit,
        "--no-prompt",
        "--notify-tracing-started",
        notification,
        "--output",
        str(trace),
        "--attach",
        str(target_pid),
    ]


def stage_instrumentable_binary(
    binary: Path,
    workspace: CaptureWorkspace,
) -> tuple[Path, dict[str, Any]]:
    """Sign a private copy for xctrace attachment; never mutate the build output."""

    expected_entitlements = {"com.apple.security.get-task-allow": True}
    try:
        declared_entitlements = plistlib.loads(
            BENCHMARK_DRIVER_ENTITLEMENTS.read_bytes()
        )
    except (OSError, plistlib.InvalidFileException) as error:
        raise CaptureError(
            f"cannot read benchmark entitlements: {BENCHMARK_DRIVER_ENTITLEMENTS}"
        ) from error
    if declared_entitlements != expected_entitlements:
        raise CaptureError(
            "BenchmarkDriver entitlements must contain only get-task-allow=true"
        )

    staged_directory = workspace.path / "instrumentable-binary"
    staged_directory.mkdir()
    staged_binary = staged_directory / binary.name
    try:
        shutil.copy2(binary, staged_binary)
    except OSError as error:
        raise CaptureError(f"cannot stage BenchmarkDriver copy: {error}") from error

    sign = run_managed_command(
        [
            "codesign",
            "--force",
            "--sign",
            "-",
            "--entitlements",
            str(BENCHMARK_DRIVER_ENTITLEMENTS),
            str(staged_binary),
        ],
        cwd=staged_directory,
        timeout=60,
        label="stage instrumentable BenchmarkDriver",
    )
    if sign.returncode:
        detail = (sign.stderr or sign.stdout).strip()
        raise CaptureError(
            f"codesign failed for staged BenchmarkDriver ({sign.returncode}): "
            f"{detail[-2000:]}"
        )
    verify = run_managed_command(
        ["codesign", "--verify", "--strict", str(staged_binary)],
        cwd=staged_directory,
        timeout=30,
        label="verify staged BenchmarkDriver signature",
    )
    if verify.returncode:
        detail = (verify.stderr or verify.stdout).strip()
        raise CaptureError(
            f"staged BenchmarkDriver signature is invalid ({verify.returncode}): "
            f"{detail[-2000:]}"
        )
    display = run_managed_command(
        [
            "codesign",
            "--display",
            "--entitlements",
            "-",
            "--xml",
            str(staged_binary),
        ],
        cwd=staged_directory,
        timeout=30,
        label="read staged BenchmarkDriver entitlements",
    )
    if display.returncode:
        detail = (display.stderr or display.stdout).strip()
        raise CaptureError(
            f"cannot read staged BenchmarkDriver entitlements "
            f"({display.returncode}): {detail[-2000:]}"
        )
    try:
        observed_entitlements = plistlib.loads(display.stdout.encode("utf-8"))
    except plistlib.InvalidFileException as error:
        raise CaptureError(
            "codesign returned malformed staged BenchmarkDriver entitlements"
        ) from error
    if observed_entitlements != expected_entitlements:
        raise CaptureError(
            "staged BenchmarkDriver does not have exactly get-task-allow=true"
        )
    return staged_binary, {
        "strategy": "private_copy_ad_hoc_codesigned_for_instrumentation",
        "source_binary_unchanged": True,
        "signature_verified": True,
        "entitlements": expected_entitlements,
    }


def candidate_command(
    binary: Path,
    fixture: Path,
    result: Path,
    control_directory: Path,
    *,
    profile: str,
    candidate: str,
    target_role: str,
) -> list[str]:
    if profile not in PROFILE_SAMPLE_COUNTS:
        raise CaptureError(f"unsupported benchmark profile: {profile}")
    if candidate not in {"srui", "webkit"}:
        raise CaptureError(f"unsupported allocation candidate: {candidate}")
    if target_role not in WEBKIT_TARGET_ROLES:
        raise CaptureError(f"unsupported allocation target role: {target_role}")
    if candidate == "srui" and target_role != "host":
        raise CaptureError("srui allocation candidate supports only the host role")
    return [
        str(binary),
        "--fixture",
        str(fixture),
        "--profile",
        profile,
        "--candidate",
        candidate,
        "--allocation-target-role",
        target_role,
        "--allocation-control-dir",
        str(control_directory),
        "--supervised-parent",
        "--output",
        str(result),
    ]


CONTROL_REQUEST_BASE_KEYS = {
    "schema_version",
    "candidate",
    "sample_index",
    "host_pid",
    "target_role",
    "target_present",
    "available_targets",
}
CONTROL_REQUEST_TARGET_KEYS = {"target_pid", "target_birth_unix_ns"}
AVAILABLE_TARGET_KEYS = {"role", "pid", "birth_unix_ns"}
CONTROL_DONE_BASE_KEYS = {
    "schema_version",
    "target_present",
    "started_unix_ns",
    "ended_unix_ns",
}
CONTROL_DONE_TARGET_KEYS = {"observed_alive_through_unix_ns"}
ATTRIBUTION_KEYS = {
    "candidate",
    "driver_pid",
    "host_pid",
    "helper_pids",
    "started_unix_ns",
    "ended_unix_ns",
    "measurement_intervals",
    "helper_pid_source",
    "process_identities",
}


def _positive_integer(value: Any) -> bool:
    return isinstance(value, int) and not isinstance(value, bool) and value > 0


def _managed_process_result_if_exited(
    process: ManagedProcess,
) -> tuple[int, str] | None:
    supervisor_exited = (
        process.supervisor is not None and process.supervisor.poll() is not None
    )
    if not process.status_path.exists() and not supervisor_exited:
        return None
    result = process.wait(5)
    return result.returncode, (result.stderr or result.stdout).strip()


def wait_for_control_payload(
    path: Path,
    *,
    processes: list[ManagedProcess],
    timeout: float,
    label: str,
    poll_hook: Any,
) -> dict[str, Any]:
    deadline = time.monotonic() + timeout
    while True:
        if path.is_file():
            try:
                payload = json.loads(path.read_text(encoding="utf-8"))
            except (OSError, json.JSONDecodeError) as error:
                raise CaptureError(f"{label} is not valid JSON: {error}") from error
            if not isinstance(payload, dict):
                raise CaptureError(f"{label} must be a JSON object")
            return payload
        for process in processes:
            completed = _managed_process_result_if_exited(process)
            if completed is not None:
                returncode, detail = completed
                raise CaptureError(
                    f"{process.label} exited before {label} "
                    f"({returncode}): {detail[-2000:]}"
                )
        poll_hook()
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            process_diagnostics = "; ".join(
                _managed_process_timeout_diagnostic(process)
                for process in processes
            )
            control_state = "present" if path.is_file() else "absent"
            raise CaptureError(
                f"timed out after {timeout:g}s waiting for {label}; "
                f"control_file={path} ({control_state}); "
                f"process_diagnostics=[{process_diagnostics}]"
            )
        time.sleep(min(POLL_SECONDS, remaining))


def _write_control_signal(path: Path) -> None:
    try:
        descriptor = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    except FileExistsError as error:
        raise CaptureError(
            f"allocation control signal already exists: {path}"
        ) from error
    os.close(descriptor)


def _verify_process_identity(pid: int, birth_unix_ns: int, *, label: str) -> None:
    try:
        observed_birth = process_birth_unix_ns(pid)
    except ManagedCommandError as error:
        raise CaptureError(f"{label} identity check failed: {error}") from error
    if observed_birth != birth_unix_ns:
        raise CaptureError(
            f"{label} exact process identity changed: expected "
            f"{pid}/{birth_unix_ns}, observed {pid}/{observed_birth}"
        )


def _remember_process_identity(
    identities: dict[int, int],
    *,
    pid: int,
    birth_unix_ns: int,
    label: str,
) -> None:
    if not _positive_integer(pid) or not _positive_integer(birth_unix_ns):
        raise CaptureError(f"{label} has an invalid exact process identity")
    existing = identities.get(pid)
    if existing is not None and existing != birth_unix_ns:
        raise CaptureError(
            f"{label} reused numeric PID {pid} across exact process identities"
        )
    identities[pid] = birth_unix_ns


def validate_control_request(
    payload: dict[str, Any],
    *,
    expected_candidate: str,
    expected_sample_index: int,
    expected_target_role: str,
    expected_host_pid: int,
) -> dict[str, Any]:
    target_present = payload.get("target_present")
    expected_keys = set(CONTROL_REQUEST_BASE_KEYS)
    if target_present is True:
        expected_keys.update(CONTROL_REQUEST_TARGET_KEYS)
    if set(payload) != expected_keys:
        raise CaptureError(
            "allocation control request has an invalid field contract"
        )
    if payload["schema_version"] != 2:
        raise CaptureError("allocation control request has an unsupported schema")
    if (
        not isinstance(target_present, bool)
        or payload["candidate"] != expected_candidate
        or payload["sample_index"] != expected_sample_index
        or payload["target_role"] != expected_target_role
        or payload["host_pid"] != expected_host_pid
    ):
        raise CaptureError(
            "allocation control request does not match the expected pass"
        )

    available_targets = payload["available_targets"]
    allowed_roles = (
        {"host"} if expected_candidate == "srui" else set(WEBKIT_TARGET_ROLES)
    )
    if not isinstance(available_targets, list) or not available_targets:
        raise CaptureError(
            "allocation control request has no available target identities"
        )
    targets_by_role: dict[str, dict[str, Any]] = {}
    target_births_by_pid: dict[int, int] = {}
    for target in available_targets:
        if (
            not isinstance(target, dict)
            or set(target) != AVAILABLE_TARGET_KEYS
            or target["role"] not in allowed_roles
            or target["role"] in targets_by_role
            or not _positive_integer(target["pid"])
            or not _positive_integer(target["birth_unix_ns"])
        ):
            raise CaptureError(
                "allocation control request has invalid available targets"
            )
        previous_birth = target_births_by_pid.setdefault(
            target["pid"], target["birth_unix_ns"]
        )
        if previous_birth != target["birth_unix_ns"]:
            raise CaptureError(
                "allocation control request maps one PID to multiple births"
            )
        _verify_process_identity(
            target["pid"],
            target["birth_unix_ns"],
            label=(
                f"{expected_candidate}/{expected_target_role} available "
                f"{target['role']} target"
            ),
        )
        targets_by_role[target["role"]] = target

    host_target = targets_by_role.get("host")
    if host_target is None or host_target["pid"] != expected_host_pid:
        raise CaptureError(
            "allocation control request does not identify its candidate host"
        )
    if any(
        role != "host" and target["pid"] == expected_host_pid
        for role, target in targets_by_role.items()
    ):
        raise CaptureError(
            "allocation control request aliases a helper role to the host"
        )
    if expected_candidate == "srui" and set(targets_by_role) != {"host"}:
        raise CaptureError("srui allocation request claimed helper targets")
    if expected_candidate == "webkit" and "webcontent" not in targets_by_role:
        raise CaptureError("WebKit allocation request has no WebContent target")

    selected_target = targets_by_role.get(expected_target_role)
    if target_present != (selected_target is not None):
        raise CaptureError(
            "allocation control request target presence disagrees with "
            "available_targets"
        )
    if target_present:
        if (
            payload["target_pid"] != selected_target["pid"]
            or payload["target_birth_unix_ns"]
            != selected_target["birth_unix_ns"]
        ):
            raise CaptureError(
                "allocation control request target identity does not match "
                "available_targets"
            )
        if expected_target_role == "host" and payload["target_pid"] != expected_host_pid:
            raise CaptureError(
                "host allocation request did not target its candidate process"
            )
        if (
            expected_target_role != "host"
            and payload["target_pid"] == expected_host_pid
        ):
            raise CaptureError(
                f"{expected_target_role} allocation request targeted "
                "the candidate host"
            )
    elif expected_target_role not in {"network", "gpu"}:
        raise CaptureError(
            f"{expected_target_role} is mandatory and cannot be absent"
        )
    return payload


def validate_control_done(
    payload: dict[str, Any],
    *,
    request: dict[str, Any],
) -> dict[str, Any]:
    expected_keys = set(CONTROL_DONE_BASE_KEYS)
    if request["target_present"]:
        expected_keys.update(CONTROL_DONE_TARGET_KEYS)
    if set(payload) != expected_keys:
        raise CaptureError(
            "allocation control completion has an invalid field contract"
        )
    if (
        payload["schema_version"] != 2
        or payload["target_present"] is not request["target_present"]
    ):
        raise CaptureError(
            "allocation control completion does not match its request"
        )
    started_unix_ns = payload["started_unix_ns"]
    ended_unix_ns = payload["ended_unix_ns"]
    if (
        not _positive_integer(started_unix_ns)
        or not _positive_integer(ended_unix_ns)
        or started_unix_ns >= ended_unix_ns
    ):
        raise CaptureError("allocation control completion has invalid timestamps")
    if not request["target_present"]:
        return payload

    observed_alive_through_unix_ns = payload[
        "observed_alive_through_unix_ns"
    ]
    if (
        not _positive_integer(observed_alive_through_unix_ns)
        or ended_unix_ns > observed_alive_through_unix_ns
        or started_unix_ns < request["target_birth_unix_ns"]
    ):
        raise CaptureError(
            "allocation control completion has invalid target lifetime bounds"
        )
    _verify_process_identity(
        request["target_pid"],
        request["target_birth_unix_ns"],
        label=(
            f"{request['candidate']}/{request['target_role']} "
            f"sample {request['sample_index']} completion"
        ),
    )
    return payload


def candidate_result_process_identities(
    payload: dict[str, Any],
    *,
    expected_candidate: str,
    expected_host_pid: int,
) -> list[tuple[int, int]]:
    attribution = payload.get("attribution")
    if (
        not isinstance(attribution, dict)
        or set(attribution) != ATTRIBUTION_KEYS
        or attribution.get("candidate") != expected_candidate
        or attribution.get("host_pid") != expected_host_pid
        or not _positive_integer(attribution.get("driver_pid"))
        or not isinstance(attribution.get("helper_pids"), list)
        or any(not _positive_integer(pid) for pid in attribution["helper_pids"])
    ):
        raise CaptureError(
            f"{expected_candidate} candidate result has invalid process attribution"
        )
    claimed_pids = {
        attribution["host_pid"],
        *attribution["helper_pids"],
    }
    if len(claimed_pids) != 1 + len(attribution["helper_pids"]):
        raise CaptureError(
            f"{expected_candidate} candidate result reuses an attributed PID"
        )
    identities = attribution.get("process_identities")
    if not isinstance(identities, list):
        raise CaptureError(
            f"{expected_candidate} candidate result has no process identities"
        )
    exact: dict[int, int] = {}
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
            or identity["observed_alive_through_unix_ns"] < identity["birth_unix_ns"]
        ):
            raise CaptureError(
                f"{expected_candidate} candidate result has an invalid "
                "exact process identity"
            )
        _remember_process_identity(
            exact,
            pid=identity["pid"],
            birth_unix_ns=identity["birth_unix_ns"],
            label=f"{expected_candidate} candidate result",
        )
    if set(exact) != claimed_pids:
        raise CaptureError(
            f"{expected_candidate} candidate result identities do not exactly "
            "cover its host and helper PIDs"
        )
    return sorted(exact.items())


def _cleanup_started_processes(
    resources: list[tuple[str, ManagedProcess | None]],
) -> list[BaseException]:
    errors: list[BaseException] = []
    for label, process in resources:
        if process is None or process.closed:
            continue
        try:
            terminate_managed_process_with_retry(process)
        except BaseException as error:
            errors.append(_cleanup_failure(label, error))
    return errors


def _raise_with_cleanup(
    label: str,
    failure: BaseException | None,
    cleanup_errors: list[BaseException],
) -> None:
    errors = ([failure] if failure is not None else []) + cleanup_errors
    if len(errors) == 1:
        raise errors[0]
    if errors:
        raise BaseExceptionGroup(label, errors)


def _raise_termination(signum: int, _frame: Any) -> None:
    raise TerminationRequested(signum)


@contextlib.contextmanager
def termination_handlers() -> Iterator[None]:
    previous = {
        signum: signal.getsignal(signum) for signum in (signal.SIGINT, signal.SIGTERM)
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
        raise CaptureError(
            f"xctrace export failed ({result.returncode}): {detail[-2000:]}"
        )
    return result.stdout




def _export_allocation_detail(
    trace: Path,
    destination: Path,
    *,
    xpath: str,
    label: str,
    max_export_bytes: int,
    min_remaining_bytes: int,
) -> None:
    def monitor_export(_process: ManagedProcess) -> None:
        _bounded_file(destination, max_export_bytes, label)
        ensure_free_reserve(trace.parent, min_remaining_bytes)

    result = run_managed_command(
        [
            "xcrun",
            "xctrace",
            "export",
            "--input",
            str(trace),
            "--output",
            str(destination),
            "--xpath",
            xpath,
        ],
        cwd=trace.parent,
        timeout=300,
        label=label,
        poll_hook=monitor_export,
    )
    if result.returncode:
        detail = (result.stderr or result.stdout).strip()
        raise CaptureError(f"{label} failed ({result.returncode}): {detail[-2000:]}")
    _bounded_file(destination, max_export_bytes, label)
    ensure_free_reserve(trace.parent, min_remaining_bytes)


def export_allocation_summary(
    trace: Path,
    sidecar: Path,
    *,
    target_pid: int,
    target_birth_unix_ns: int,
    observed_alive_through_unix_ns: int,
    measurement_started_unix_ns: int,
    measurement_ended_unix_ns: int,
    max_export_bytes: int,
    min_remaining_bytes: int,
    reported_trace: Path | None = None,
) -> None:
    toc = _export_command(
        trace,
        ["--toc"],
        timeout=60,
        maximum_output_bytes=4 * 1024 * 1024,
    )
    try:
        toc_metadata = validate_allocation_toc(toc, requested_pid=target_pid)
    except AllocationExportError as error:
        raise CaptureError(str(error)) from error

    temporary_paths: list[Path] = []
    try:
        for suffix in (".statistics.xml", ".allocations-list.xml"):
            with tempfile.NamedTemporaryFile(
                dir=trace.parent,
                suffix=suffix,
                delete=False,
            ) as exported:
                temporary_paths.append(Path(exported.name))
        statistics_path, allocations_list_path = temporary_paths
        _export_allocation_detail(
            trace,
            statistics_path,
            xpath=STATISTICS_XPATH,
            label="Allocations/Statistics export",
            max_export_bytes=max_export_bytes,
            min_remaining_bytes=min_remaining_bytes,
        )
        _export_allocation_detail(
            trace,
            allocations_list_path,
            xpath=ALLOCATIONS_LIST_XPATH,
            label="Allocations/Allocations List export",
            max_export_bytes=max_export_bytes,
            min_remaining_bytes=min_remaining_bytes,
        )
        try:
            allocation = summarize_allocation_exports(
                statistics_path=statistics_path,
                allocations_list_path=allocations_list_path,
                toc_metadata=toc_metadata,
                target_pid=target_pid,
                target_birth_unix_ns=target_birth_unix_ns,
                observed_alive_through_unix_ns=observed_alive_through_unix_ns,
                measurement_started_unix_ns=measurement_started_unix_ns,
                measurement_ended_unix_ns=measurement_ended_unix_ns,
            )
        except AllocationExportError as error:
            raise CaptureError(str(error)) from error
    finally:
        for temporary_path in temporary_paths:
            temporary_path.unlink(missing_ok=True)

    payload = {
        "schema_version": 3,
        "capture_scope": "exact_process",
        "capture_method": "xctrace Allocations --attach to one validated exact PID",
        "trace": str(reported_trace if reported_trace is not None else trace),
        "trace_bytes": trace_size_bytes(trace),
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
            "TOC attached PID equals the requested PID and the benchmark "
            "handshake bounds the same PID by birth and observed liveness"
        ),
        "trace_started_unix_ns": toc_metadata["trace_started_unix_ns"],
        "trace_start_timestamp_resolution_ns": toc_metadata[
            "trace_start_timestamp_resolution_ns"
        ],
        "allocation_list_timestamp_resolution_ns": toc_metadata[
            "allocation_list_timestamp_resolution_ns"
        ],
        "timestamp_boundary_uncertainty_ns": toc_metadata[
            "timestamp_boundary_uncertainty_ns"
        ],
        "xctrace": {
            "instruments_version": toc_metadata["instruments_version"],
            "platform": toc_metadata["platform"],
            "os_version": toc_metadata["os_version"],
            "attached_pid": toc_metadata["attached_pid"],
            "attached_process_name": toc_metadata["attached_process_name"],
            "statistics_xpath": toc_metadata["statistics_xpath"],
            "allocations_list_xpath": toc_metadata["allocations_list_xpath"],
        },
        **allocation,
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


def capture_target_sample(
    *,
    candidate_process: ManagedProcess,
    candidate: str,
    target_role: str,
    sample_index: int,
    control_directory: Path,
    trace_directory: Path,
    summary_directory: Path,
    max_trace_bytes: int,
    max_export_bytes: int,
    min_remaining_bytes: int,
    known_process_identities: dict[int, int],
) -> dict[str, Any]:
    request_path = (
        control_directory / f"request-{sample_index}-{target_role}.json"
    )
    done_path = control_directory / f"done-{sample_index}-{target_role}.json"
    go_path = control_directory / f"go-{sample_index}-{target_role}"
    captured_path = (
        control_directory / f"captured-{sample_index}-{target_role}"
    )

    def monitor() -> None:
        ensure_capture_budget(
            trace_directory,
            max_trace_bytes=max_trace_bytes,
            min_remaining_bytes=min_remaining_bytes,
        )

    host_pid = candidate_process.child_pid
    if host_pid is None:
        raise CaptureError(f"{candidate}/{target_role} candidate has no child PID")
    request = validate_control_request(
        wait_for_control_payload(
            request_path,
            processes=[candidate_process],
            timeout=CONTROL_FILE_TIMEOUT_SECONDS,
            label=f"{candidate}/{target_role} sample {sample_index} request",
            poll_hook=monitor,
        ),
        expected_candidate=candidate,
        expected_sample_index=sample_index,
        expected_target_role=target_role,
        expected_host_pid=host_pid,
    )
    for available_target in request["available_targets"]:
        _remember_process_identity(
            known_process_identities,
            pid=available_target["pid"],
            birth_unix_ns=available_target["birth_unix_ns"],
            label=(
                f"{candidate}/{target_role} sample {sample_index} "
                f"available {available_target['role']} target"
            ),
        )

    if not request["target_present"]:
        failure: BaseException | None = None
        output: dict[str, Any] | None = None
        try:
            _write_control_signal(go_path)
            done = validate_control_done(
                wait_for_control_payload(
                    done_path,
                    processes=[candidate_process],
                    timeout=CONTROL_FILE_TIMEOUT_SECONDS,
                    label=(
                        f"{candidate}/{target_role} sample {sample_index} "
                        "absent-target completion"
                    ),
                    poll_hook=monitor,
                ),
                request=request,
            )
            _write_control_signal(captured_path)
            output = {
                "candidate": candidate,
                "sample_index": sample_index,
                "target_role": target_role,
                "target_present": False,
                "host_pid": request["host_pid"],
                "available_targets": request["available_targets"],
                "started_unix_ns": done["started_unix_ns"],
                "ended_unix_ns": done["ended_unix_ns"],
            }
        except BaseException as error:
            failure = error
        _raise_with_cleanup(
            f"{candidate}/{target_role} sample {sample_index} "
            "absent-target handshake failed",
            failure,
            [],
        )
        if output is None:
            raise CaptureError(
                f"{candidate}/{target_role} sample {sample_index} "
                "produced no absence evidence"
            )
        return output

    segment_name = f"{candidate}-{target_role}-{sample_index:03d}.trace"
    segment_trace = trace_directory / segment_name
    segment_sidecar = summary_directory / f"{segment_name}.summary.json"
    notification = (
        f"dev.srui.benchmark.xctrace.{os.getpid()}."
        f"{candidate}.{target_role}.{sample_index}.{time.monotonic_ns()}"
    )
    watcher: ManagedProcess | None = None
    recorder: ManagedProcess | None = None
    failure = None
    output = None

    try:
        with blocked_termination_signals():
            watcher = ManagedProcess.start(
                notification_watcher_command(notification),
                cwd=trace_directory,
                label=f"{candidate}/{target_role} allocation notification watcher",
                require_exact_owner_identity=True,
            )
        watcher.wait_until_started(5, poll_hook=lambda _process: monitor())
        with blocked_termination_signals():
            recorder = ManagedProcess.start(
                trace_command(
                    segment_trace,
                    notification,
                    target_pid=request["target_pid"],
                ),
                cwd=trace_directory,
                label=(
                    f"{candidate}/{target_role} sample {sample_index} "
                    "allocation recorder"
                ),
                require_exact_owner_identity=True,
            )
        recording_readiness_basis = wait_for_recording_readiness(
            watcher,
            recorder,
            trace_directory,
            max_trace_bytes=max_trace_bytes,
            min_remaining_bytes=min_remaining_bytes,
        )
        _verify_process_identity(
            request["target_pid"],
            request["target_birth_unix_ns"],
            label=f"{candidate}/{target_role} target after xctrace attach",
        )
        _write_control_signal(go_path)
        done = validate_control_done(
            wait_for_control_payload(
                done_path,
                processes=[candidate_process, recorder],
                timeout=CONTROL_FILE_TIMEOUT_SECONDS,
                label=f"{candidate}/{target_role} sample {sample_index} completion",
                poll_hook=monitor,
            ),
            request=request,
        )

        recorder.signal(signal.SIGINT)
        recorder_result = recorder.wait(60, poll_hook=lambda _process: monitor())
        if recorder_result.returncode:
            detail = (recorder_result.stderr or recorder_result.stdout).strip()
            raise CaptureError(
                f"xctrace failed for {candidate}/{target_role} sample "
                f"{sample_index} ({recorder_result.returncode}): {detail[-2000:]}"
            )
        ensure_capture_budget(
            trace_directory,
            max_trace_bytes=max_trace_bytes,
            min_remaining_bytes=min_remaining_bytes,
        )
        if not segment_trace.exists():
            raise CaptureError(
                f"xctrace produced no segment for {candidate}/{target_role} "
                f"sample {sample_index}"
            )

        export_allocation_summary(
            segment_trace,
            segment_sidecar,
            target_pid=request["target_pid"],
            target_birth_unix_ns=request["target_birth_unix_ns"],
            observed_alive_through_unix_ns=done[
                "observed_alive_through_unix_ns"
            ],
            measurement_started_unix_ns=done["started_unix_ns"],
            measurement_ended_unix_ns=done["ended_unix_ns"],
            max_export_bytes=max_export_bytes,
            min_remaining_bytes=min_remaining_bytes,
            reported_trace=Path(segment_name),
        )
        try:
            segment_summary = json.loads(
                segment_sidecar.read_text(encoding="utf-8")
            )
            process_total = segment_summary["process_total"]
        except (OSError, json.JSONDecodeError, KeyError, TypeError) as error:
            raise CaptureError(
                f"targeted allocation summary is incomplete for "
                f"{candidate}/{target_role} sample {sample_index}"
            ) from error
        if (
            segment_summary.get("schema_version") != 3
            or segment_summary.get("capture_scope") != "exact_process"
            or process_total.get("pid") != request["target_pid"]
            or process_total.get("birth_unix_ns")
            != request["target_birth_unix_ns"]
            or segment_summary.get("allocation_list_reconciled") is not True
            or segment_summary.get("xctrace", {}).get("attached_pid")
            != request["target_pid"]
        ):
            raise CaptureError(
                f"targeted allocation evidence does not exactly match "
                f"{candidate}/{target_role} sample {sample_index}"
            )

        _write_control_signal(captured_path)
        output = {
            "candidate": candidate,
            "sample_index": sample_index,
            "target_role": target_role,
            "target_present": True,
            "host_pid": request["host_pid"],
            "available_targets": request["available_targets"],
            "target_pid": request["target_pid"],
            "target_birth_unix_ns": request["target_birth_unix_ns"],
            "started_unix_ns": done["started_unix_ns"],
            "ended_unix_ns": done["ended_unix_ns"],
            "observed_alive_through_unix_ns": (
                done["observed_alive_through_unix_ns"]
            ),
            "trace_segment": segment_name,
            "recording_readiness_basis": recording_readiness_basis,
            "trace_bytes": segment_summary["trace_bytes"],
            "allocation_export_basis": segment_summary[
                "allocation_export_basis"
            ],
            "allocation_timestamp_basis": segment_summary[
                "allocation_timestamp_basis"
            ],
            "metric_semantics": segment_summary["metric_semantics"],
            "whole_trace_statistics_semantics": segment_summary[
                "whole_trace_statistics_semantics"
            ],
            "process_identity_basis": segment_summary["process_identity_basis"],
            "allocation_rows": segment_summary["allocation_rows"],
            "allocation_list_bytes": segment_summary["allocation_list_bytes"],
            "excluded_vm_rows": segment_summary["excluded_vm_rows"],
            "excluded_vm_bytes": segment_summary["excluded_vm_bytes"],
            "allocation_list_reconciled": segment_summary[
                "allocation_list_reconciled"
            ],
            "whole_trace_statistics": segment_summary[
                "whole_trace_statistics"
            ],
            "attach_baseline_allocations": segment_summary[
                "attach_baseline_allocations"
            ],
            "attach_baseline_bytes": segment_summary["attach_baseline_bytes"],
            "retained_allocations": segment_summary["retained_allocations"],
            "retained_bytes": segment_summary["retained_bytes"],
            "retained_allocations_lower_bound": segment_summary[
                "retained_allocations_lower_bound"
            ],
            "retained_allocations_upper_bound": segment_summary[
                "retained_allocations_upper_bound"
            ],
            "retained_bytes_lower_bound": segment_summary[
                "retained_bytes_lower_bound"
            ],
            "retained_bytes_upper_bound": segment_summary[
                "retained_bytes_upper_bound"
            ],
            "boundary_ambiguous_allocations": segment_summary[
                "boundary_ambiguous_allocations"
            ],
            "boundary_ambiguous_bytes": segment_summary[
                "boundary_ambiguous_bytes"
            ],
            "excluded_outside_measurement_interval_rows": segment_summary[
                "excluded_outside_measurement_interval_rows"
            ],
            "trace_started_unix_ns": segment_summary["trace_started_unix_ns"],
            "trace_start_timestamp_resolution_ns": segment_summary[
                "trace_start_timestamp_resolution_ns"
            ],
            "allocation_list_timestamp_resolution_ns": segment_summary[
                "allocation_list_timestamp_resolution_ns"
            ],
            "timestamp_boundary_uncertainty_ns": segment_summary[
                "timestamp_boundary_uncertainty_ns"
            ],
            "xctrace": segment_summary["xctrace"],
            "process_total": process_total,
        }
    except BaseException as error:
        failure = error

    cleanup_errors = _cleanup_started_processes(
        [
            ("targeted allocation recorder cleanup", recorder),
            ("targeted allocation watcher cleanup", watcher),
        ]
    )
    _raise_with_cleanup(
        f"{candidate}/{target_role} sample {sample_index} capture and cleanup failed",
        failure,
        cleanup_errors,
    )
    if output is None:
        raise CaptureError(
            f"{candidate}/{target_role} sample {sample_index} produced no output"
        )
    return output


def run_candidate_pass(
    *,
    binary: Path,
    fixture: Path,
    profile: str,
    candidate: str,
    target_role: str,
    workspace: CaptureWorkspace,
    trace_directory: Path,
    summary_directory: Path,
    max_trace_bytes: int,
    max_export_bytes: int,
    min_remaining_bytes: int,
) -> dict[str, Any]:
    pass_name = f"{candidate}-{target_role}"
    control_directory = workspace.path / "controls" / pass_name
    control_directory.mkdir(parents=True)
    result_directory = workspace.path / "candidate-results"
    result_directory.mkdir(exist_ok=True)
    candidate_result_path = result_directory / f"{pass_name}.json"
    candidate_process: ManagedProcess | None = None
    known_process_identities: dict[int, int] = {}
    failure: BaseException | None = None
    output: dict[str, Any] | None = None

    try:
        with blocked_termination_signals():
            candidate_process = ManagedProcess.start(
                candidate_command(
                    binary,
                    fixture,
                    candidate_result_path,
                    control_directory,
                    profile=profile,
                    candidate=candidate,
                    target_role=target_role,
                ),
                cwd=workspace.path,
                label=f"{pass_name} allocation candidate",
                require_exact_owner_identity=True,
            )
        host_pid = candidate_process.wait_until_started(
            5,
            poll_hook=lambda _process: ensure_capture_budget(
                trace_directory,
                max_trace_bytes=max_trace_bytes,
                min_remaining_bytes=min_remaining_bytes,
            ),
        )
        try:
            host_birth_unix_ns = process_birth_unix_ns(host_pid)
        except ManagedCommandError as error:
            raise CaptureError(
                f"{pass_name} candidate identity check failed: {error}"
            ) from error
        if host_birth_unix_ns is None:
            completed = _managed_process_result_if_exited(candidate_process)
            if completed is not None:
                returncode, detail = completed
                raise CaptureError(
                    f"{pass_name} candidate exited before identity validation "
                    f"({returncode}): {detail[-2000:]}"
                )
            raise CaptureError(
                f"{pass_name} candidate identity was unavailable after startup"
            )
        _remember_process_identity(
            known_process_identities,
            pid=host_pid,
            birth_unix_ns=host_birth_unix_ns,
            label=f"{pass_name} candidate",
        )
        samples = [
            capture_target_sample(
                candidate_process=candidate_process,
                candidate=candidate,
                target_role=target_role,
                sample_index=sample_index,
                control_directory=control_directory,
                trace_directory=trace_directory,
                summary_directory=summary_directory,
                max_trace_bytes=max_trace_bytes,
                max_export_bytes=max_export_bytes,
                min_remaining_bytes=min_remaining_bytes,
                known_process_identities=known_process_identities,
            )
            for sample_index in range(PROFILE_SAMPLE_COUNTS[profile])
        ]
        candidate_result = candidate_process.wait(
            CANDIDATE_EXIT_TIMEOUT_SECONDS,
            poll_hook=lambda _process: ensure_capture_budget(
                trace_directory,
                max_trace_bytes=max_trace_bytes,
                min_remaining_bytes=min_remaining_bytes,
            ),
        )
        if candidate_result.returncode:
            detail = (candidate_result.stderr or candidate_result.stdout).strip()
            raise CaptureError(
                f"{pass_name} allocation candidate failed "
                f"({candidate_result.returncode}): {detail[-2000:]}"
            )
        try:
            result_payload = json.loads(
                candidate_result_path.read_text(encoding="utf-8")
            )
        except (OSError, json.JSONDecodeError) as error:
            raise CaptureError(
                f"{pass_name} allocation candidate wrote no valid result"
            ) from error
        if (
            not isinstance(result_payload, dict)
            or result_payload.get("candidate") != candidate
            or result_payload.get("succeeded") is not True
            or any(sample["host_pid"] != host_pid for sample in samples)
        ):
            raise CaptureError(
                f"{pass_name} allocation candidate result failed validation"
            )
        for identity_pid, identity_birth in candidate_result_process_identities(
            result_payload,
            expected_candidate=candidate,
            expected_host_pid=host_pid,
        ):
            _remember_process_identity(
                known_process_identities,
                pid=identity_pid,
                birth_unix_ns=identity_birth,
                label=f"{pass_name} candidate result",
            )
        output = {
            "candidate": candidate,
            "target_role": target_role,
            "host_pid": host_pid,
            "samples": samples,
            "candidate_result": result_payload,
        }
    except BaseException as error:
        failure = error

    cleanup_errors = _cleanup_started_processes(
        [("allocation candidate cleanup", candidate_process)]
    )
    if known_process_identities:
        try:
            wait_for_process_identities_gone(
                sorted(known_process_identities.items()),
                label=f"{pass_name} exact renderer processes",
            )
        except BaseException as error:
            cleanup_errors.append(
                _cleanup_failure(
                    f"{pass_name} exact renderer identity postcondition",
                    error,
                )
            )
    _raise_with_cleanup(
        f"{pass_name} allocation pass and cleanup failed",
        failure,
        cleanup_errors,
    )
    if output is None:
        raise CaptureError(f"{pass_name} allocation pass produced no output")
    return output


SEGMENT_COUNTER_FIELDS = (
    "attach_baseline_allocations",
    "attach_baseline_bytes",
    "boundary_ambiguous_allocations",
    "boundary_ambiguous_bytes",
    "excluded_outside_measurement_interval_rows",
    "vm_category_rows",
    "vm_category_bytes",
)
WHOLE_TRACE_STATISTIC_GROUPS = (
    "heap_and_anonymous_vm",
    "heap",
    "anonymous_vm",
)
WHOLE_TRACE_STATISTIC_FIELDS = (
    "persistent_allocations",
    "persistent_bytes",
    "transient_allocations",
    "transient_bytes",
    "total_allocations",
    "total_bytes",
    "event_count",
)
PROCESS_ALLOCATION_FIELDS = (
    "retained_allocations",
    "retained_bytes",
    "retained_allocations_lower_bound",
    "retained_allocations_upper_bound",
    "retained_bytes_lower_bound",
    "retained_bytes_upper_bound",
    "boundary_ambiguous_allocations",
    "boundary_ambiguous_bytes",
)


def _nonnegative_integer(value: Any) -> bool:
    return not isinstance(value, bool) and isinstance(value, int) and value >= 0


def _validate_statistics_row(value: Any, *, label: str) -> dict[str, int]:
    if not isinstance(value, dict) or set(value) != set(WHOLE_TRACE_STATISTIC_FIELDS):
        raise CaptureError(f"{label} whole-trace Statistics contract is invalid")
    if any(not _nonnegative_integer(value[field]) for field in value):
        raise CaptureError(f"{label} whole-trace Statistics contains invalid values")
    if (
        value["persistent_allocations"] + value["transient_allocations"]
        != value["total_allocations"]
        or value["persistent_bytes"] + value["transient_bytes"]
        != value["total_bytes"]
    ):
        raise CaptureError(f"{label} whole-trace Statistics do not reconcile")
    return value


def _validate_whole_trace_statistics(
    value: Any,
) -> dict[str, dict[str, int]]:
    if not isinstance(value, dict) or set(value) != set(WHOLE_TRACE_STATISTIC_GROUPS):
        raise CaptureError("whole-trace allocation Statistics contract is invalid")
    validated = {
        group: _validate_statistics_row(value[group], label=group)
        for group in WHOLE_TRACE_STATISTIC_GROUPS
    }
    combined = validated["heap_and_anonymous_vm"]
    heap = validated["heap"]
    anonymous_vm = validated["anonymous_vm"]
    if any(
        combined[field] != heap[field] + anonymous_vm[field]
        for field in WHOLE_TRACE_STATISTIC_FIELDS
    ):
        raise CaptureError(
            "combined whole-trace Statistics do not equal heap plus anonymous VM"
        )
    return validated
def _validated_process_total(total: Any) -> dict[str, Any]:
    if not isinstance(total, dict):
        raise CaptureError("targeted process total has an invalid contract")
    try:
        pid = total["pid"]
        birth_unix_ns = total["birth_unix_ns"]
        observed_alive_through_unix_ns = total["observed_alive_through_unix_ns"]
        names = total["names"]
    except KeyError as error:
        raise CaptureError("targeted process total has an invalid contract") from error
    if (
        not _positive_integer(pid)
        or not _positive_integer(birth_unix_ns)
        or not _positive_integer(observed_alive_through_unix_ns)
        or observed_alive_through_unix_ns < birth_unix_ns
        or not isinstance(names, list)
        or not names
        or any(not isinstance(name, str) or not name for name in names)
        or any(
            field not in total or not _nonnegative_integer(total[field])
            for field in PROCESS_ALLOCATION_FIELDS
        )
        or total["retained_allocations_lower_bound"]
        > total["retained_allocations"]
        or total["retained_allocations"]
        > total["retained_allocations_upper_bound"]
        or total["retained_bytes_lower_bound"] > total["retained_bytes"]
        or total["retained_bytes"] > total["retained_bytes_upper_bound"]
        or total["boundary_ambiguous_allocations"]
        != total["retained_allocations_upper_bound"]
        - total["retained_allocations_lower_bound"]
        or total["boundary_ambiguous_bytes"]
        != total["retained_bytes_upper_bound"]
        - total["retained_bytes_lower_bound"]
    ):
        raise CaptureError("targeted process total has invalid retained values")
    return total


def _merge_exact_process_totals(
    totals: list[dict[str, Any]],
) -> list[dict[str, Any]]:
    merged: dict[tuple[int, int], dict[str, Any]] = {}
    numeric_pid_births: dict[int, int] = {}
    for raw_total in totals:
        total = _validated_process_total(raw_total)
        pid = total["pid"]
        birth_unix_ns = total["birth_unix_ns"]
        previous_birth = numeric_pid_births.setdefault(pid, birth_unix_ns)
        if previous_birth != birth_unix_ns:
            raise CaptureError(
                f"targeted passes reused numeric pid {pid} across exact identities"
            )
        identity = (pid, birth_unix_ns)
        existing = merged.get(identity)
        if existing is None:
            merged[identity] = {
                "pid": pid,
                "birth_unix_ns": birth_unix_ns,
                "observed_alive_through_unix_ns": total[
                    "observed_alive_through_unix_ns"
                ],
                "names": sorted(set(total["names"])),
                **{field: total[field] for field in PROCESS_ALLOCATION_FIELDS},
            }
            continue
        existing["observed_alive_through_unix_ns"] = max(
            existing["observed_alive_through_unix_ns"],
            total["observed_alive_through_unix_ns"],
        )
        existing["names"] = sorted({*existing["names"], *total["names"]})
        for field in PROCESS_ALLOCATION_FIELDS:
            existing[field] += total[field]
    return sorted(merged.values(), key=lambda item: (item["pid"], item["birth_unix_ns"]))


def _validate_pass_result(
    item: dict[str, Any],
    *,
    candidate: str,
    target_role: str,
) -> list[dict[str, Any]]:
    if (
        not isinstance(item, dict)
        or item.get("candidate") != candidate
        or item.get("target_role") != target_role
        or not _positive_integer(item.get("host_pid"))
        or not isinstance(item.get("samples"), list)
        or not item["samples"]
        or not isinstance(item.get("candidate_result"), dict)
        or item["candidate_result"].get("candidate") != candidate
        or item["candidate_result"].get("succeeded") is not True
    ):
        raise CaptureError(
            f"{candidate}/{target_role} targeted pass has an invalid contract"
        )
    samples = item["samples"]
    present_fields = {
        "target_pid",
        "target_birth_unix_ns",
        "observed_alive_through_unix_ns",
        "trace_segment",
        "recording_readiness_basis",
        "trace_bytes",
        "allocation_export_basis",
        "allocation_rows",
        "allocation_list_bytes",
        "allocation_list_reconciled",
        "allocation_timestamp_basis",
        "metric_semantics",
        "whole_trace_statistics_semantics",
        "process_identity_basis",
        "whole_trace_statistics",
        "trace_started_unix_ns",
        "trace_start_timestamp_resolution_ns",
        "allocation_list_timestamp_resolution_ns",
        "timestamp_boundary_uncertainty_ns",
        "xctrace",
        "process_total",
        *PROCESS_ALLOCATION_FIELDS,
        *SEGMENT_COUNTER_FIELDS,
    }
    for sample_index, sample in enumerate(samples):
        if (
            not isinstance(sample, dict)
            or sample.get("candidate") != candidate
            or sample.get("target_role") != target_role
            or sample.get("sample_index") != sample_index
            or sample.get("host_pid") != item["host_pid"]
            or not isinstance(sample.get("target_present"), bool)
            or not isinstance(sample.get("available_targets"), list)
            or not _positive_integer(sample.get("started_unix_ns"))
            or not _positive_integer(sample.get("ended_unix_ns"))
            or sample["started_unix_ns"] >= sample["ended_unix_ns"]
        ):
            raise CaptureError(
                f"{candidate}/{target_role} sample {sample_index} "
                "has an invalid targeted-capture contract"
            )
        allowed_roles = (
            {"host"} if candidate == "srui" else set(WEBKIT_TARGET_ROLES)
        )
        targets_by_role: dict[str, dict[str, Any]] = {}
        target_births_by_pid: dict[int, int] = {}
        for target in sample["available_targets"]:
            if (
                not isinstance(target, dict)
                or set(target) != AVAILABLE_TARGET_KEYS
                or target["role"] not in allowed_roles
                or target["role"] in targets_by_role
                or not _positive_integer(target["pid"])
                or not _positive_integer(target["birth_unix_ns"])
            ):
                raise CaptureError(
                    f"{candidate}/{target_role} sample {sample_index} "
                    "has invalid available target evidence"
                )
            previous_birth = target_births_by_pid.setdefault(
                target["pid"], target["birth_unix_ns"]
            )
            if previous_birth != target["birth_unix_ns"]:
                raise CaptureError(
                    f"{candidate}/{target_role} sample {sample_index} "
                    "maps one available PID to multiple births"
                )
            targets_by_role[target["role"]] = target

        host_target = targets_by_role.get("host")
        if (
            host_target is None
            or host_target["pid"] != sample["host_pid"]
            or (candidate == "srui" and set(targets_by_role) != {"host"})
            or (candidate == "webkit" and "webcontent" not in targets_by_role)
            or any(
                role != "host" and target["pid"] == sample["host_pid"]
                for role, target in targets_by_role.items()
            )
        ):
            raise CaptureError(
                f"{candidate}/{target_role} sample {sample_index} "
                "has invalid available target evidence"
            )
        selected_target = targets_by_role.get(target_role)
        present = sample["target_present"]
        if present != (selected_target is not None):
            raise CaptureError(
                f"{candidate}/{target_role} sample {sample_index} target "
                "presence disagrees with its advertised available target"
            )
        if present and any(field not in sample for field in present_fields):
            raise CaptureError(
                f"{candidate}/{target_role} sample {sample_index} "
                "is missing exact allocation evidence"
            )
        if not present and any(field in sample for field in present_fields):
            raise CaptureError(
                f"{candidate}/{target_role} sample {sample_index} "
                "claimed allocation evidence for an absent target"
            )
        if not present:
            if target_role not in {"network", "gpu"}:
                raise CaptureError(
                    f"{candidate}/{target_role} sample {sample_index} "
                    "claims a mandatory target is absent"
                )
            continue

        if (
            sample["target_pid"] != selected_target["pid"]
            or sample["target_birth_unix_ns"]
            != selected_target["birth_unix_ns"]
        ):
            raise CaptureError(
                f"{candidate}/{target_role} sample {sample_index} target "
                "identity does not match its advertised available target"
            )
        if (
            not _positive_integer(sample["target_pid"])
            or not _positive_integer(sample["target_birth_unix_ns"])
            or not _positive_integer(sample["observed_alive_through_unix_ns"])
            or not (
                sample["target_birth_unix_ns"]
                <= sample["started_unix_ns"]
                < sample["ended_unix_ns"]
                <= sample["observed_alive_through_unix_ns"]
            )
            or not isinstance(sample["trace_segment"], str)
            or not sample["trace_segment"]
            or sample["recording_readiness_basis"] != "darwin_notification"
            or not _positive_integer(sample["trace_bytes"])
            or not _positive_integer(sample["allocation_rows"])
            or not _nonnegative_integer(sample["allocation_list_bytes"])
            or any(
                not _nonnegative_integer(sample[field])
                for field in SEGMENT_COUNTER_FIELDS
            )
            or sample["allocation_list_reconciled"] is not True
            or not _positive_integer(sample["trace_started_unix_ns"])
            or not _positive_integer(sample["trace_start_timestamp_resolution_ns"])
            or not _positive_integer(sample["allocation_list_timestamp_resolution_ns"])
            or sample["timestamp_boundary_uncertainty_ns"]
            != sample["trace_start_timestamp_resolution_ns"]
            + sample["allocation_list_timestamp_resolution_ns"]
            or any(
                not isinstance(sample[key], str) or not sample[key].strip()
                for key in (
                    "allocation_export_basis",
                    "allocation_timestamp_basis",
                    "metric_semantics",
                    "whole_trace_statistics_semantics",
                    "process_identity_basis",
                )
            )
        ):
            raise CaptureError(
                f"{candidate}/{target_role} sample {sample_index} "
                "has invalid exact allocation evidence"
            )
        statistics = _validate_whole_trace_statistics(
            sample["whole_trace_statistics"]
        )
        combined_statistics = statistics["heap_and_anonymous_vm"]
        if (
            sample["allocation_rows"]
            != combined_statistics["persistent_allocations"]
            or sample["allocation_list_bytes"]
            != combined_statistics["persistent_bytes"]
            or sample["attach_baseline_allocations"] > sample["allocation_rows"]
            or sample["attach_baseline_bytes"] > sample["allocation_list_bytes"]
            or sample["excluded_outside_measurement_interval_rows"]
            != sample["allocation_rows"] - sample["retained_allocations"]
            or sample["retained_allocations_lower_bound"]
            > sample["retained_allocations"]
            or sample["retained_allocations"]
            > sample["retained_allocations_upper_bound"]
            or sample["retained_bytes_lower_bound"] > sample["retained_bytes"]
            or sample["retained_bytes"] > sample["retained_bytes_upper_bound"]
            or sample["boundary_ambiguous_allocations"]
            != sample["retained_allocations_upper_bound"]
            - sample["retained_allocations_lower_bound"]
            or sample["boundary_ambiguous_bytes"]
            != sample["retained_bytes_upper_bound"]
            - sample["retained_bytes_lower_bound"]
        ):
            raise CaptureError(
                f"{candidate}/{target_role} sample {sample_index} "
                "has irreconcilable allocation totals"
            )
        xctrace = sample["xctrace"]
        if (
            not isinstance(xctrace, dict)
            or xctrace.get("attached_pid") != sample["target_pid"]
            or xctrace.get("statistics_xpath") != STATISTICS_XPATH
            or xctrace.get("allocations_list_xpath") != ALLOCATIONS_LIST_XPATH
            or any(
                not isinstance(xctrace.get(key), str) or not xctrace[key]
                for key in (
                    "instruments_version",
                    "platform",
                    "os_version",
                    "attached_process_name",
                )
            )
        ):
            raise CaptureError(
                f"{candidate}/{target_role} sample {sample_index} "
                "has invalid xctrace target evidence"
            )
        process_total = _validated_process_total(sample["process_total"])
        if (
            process_total["pid"] != sample["target_pid"]
            or process_total["birth_unix_ns"]
            != sample["target_birth_unix_ns"]
            or process_total["observed_alive_through_unix_ns"]
            != sample["observed_alive_through_unix_ns"]
            or any(
                process_total[field] != sample[field]
                for field in PROCESS_ALLOCATION_FIELDS
            )
        ):
            raise CaptureError(
                f"{candidate}/{target_role} sample {sample_index} "
                "process identity or retained totals do not match its handshake"
            )
    return samples


def _candidate_equivalence(
    candidate: str,
    role_passes: list[dict[str, Any]],
) -> dict[str, Any]:
    evidence: list[dict[str, Any]] = []
    for role_pass in role_passes:
        result = role_pass["candidate_result"]
        representation_bytes = result.get("representationBytes")
        rendered_node_count = result.get("renderedNodeCount")
        if (
            not _positive_integer(representation_bytes)
            or not _positive_integer(rendered_node_count)
            or result.get("semanticParityPassed") is not True
            or result.get("elementKindsPassed") is not True
            or result.get("succeeded") is not True
        ):
            raise CaptureError(
                f"{candidate}/{role_pass['target_role']} did not prove "
                "an equivalent successful renderer workload"
            )
        evidence.append(
            {
                "target_role": role_pass["target_role"],
                "representation_bytes": representation_bytes,
                "rendered_node_count": rendered_node_count,
                "semantic_parity_passed": True,
                "element_kinds_passed": True,
            }
        )
    if len({(item["representation_bytes"], item["rendered_node_count"]) for item in evidence}) != 1:
        raise CaptureError(
            f"{candidate} targeted passes did not render equivalent representations"
        )
    return {
        "equivalent_representation_bytes": evidence[0]["representation_bytes"],
        "equivalent_rendered_node_count": evidence[0]["rendered_node_count"],
        "target_passes": evidence,
    }


def _role_partition_from_available_targets(
    available_targets: list[dict[str, Any]],
    *,
    role_order: tuple[str, ...],
) -> tuple[tuple[str, ...], ...]:
    identity_by_role = {
        target["role"]: (target["pid"], target["birth_unix_ns"])
        for target in available_targets
    }
    roles_by_identity: dict[tuple[int, int], list[str]] = {}
    for role in role_order:
        identity = identity_by_role.get(role)
        if identity is not None:
            roles_by_identity.setdefault(identity, []).append(role)
    return tuple(tuple(roles) for roles in roles_by_identity.values())


def _combine_candidate_passes(
    candidate: str,
    *,
    role_passes: dict[str, dict[str, Any]],
) -> dict[str, Any]:
    expected_roles = ("host",) if candidate == "srui" else WEBKIT_TARGET_ROLES
    if set(role_passes) != set(expected_roles):
        raise CaptureError(
            f"{candidate} targeted capture must contain exactly roles "
            f"{list(expected_roles)}"
        )
    samples_by_role = {
        role: _validate_pass_result(
            role_passes[role], candidate=candidate, target_role=role
        )
        for role in expected_roles
    }
    sample_counts = {len(samples) for samples in samples_by_role.values()}
    if len(sample_counts) != 1:
        raise CaptureError(
            f"{candidate} targeted passes have different logical sample counts"
        )
    sample_count = next(iter(sample_counts))
    host_pid = role_passes["host"]["host_pid"]
    if any(
        not sample["target_present"] or sample["target_pid"] != host_pid
        for sample in samples_by_role["host"]
    ):
        raise CaptureError(
            f"{candidate} host pass did not consistently target its candidate host"
        )

    measurement_samples: list[dict[str, Any]] = []
    contribution_totals_by_role: dict[str, list[dict[str, Any]]] = {
        role: [] for role in expected_roles
    }
    helper_pids: set[int] = set()
    helper_pids_by_role: dict[str, set[int]] = {
        role: set() for role in expected_roles if role != "host"
    }
    captured_process_identities: dict[tuple[int, int], int] = {}
    role_intervals_all: list[dict[str, Any]] = []
    mandatory_roles = {"host"} if candidate == "srui" else {"host", "webcontent"}
    for sample_index in range(sample_count):
        logical_samples_by_role = {
            role: samples_by_role[role][sample_index] for role in expected_roles
        }
        available_by_pass = {
            role: {
                target["role"]: target
                for target in logical_samples_by_role[role]["available_targets"]
            }
            for role in expected_roles
        }
        authoritative_roles = set(available_by_pass[expected_roles[0]])
        if not mandatory_roles <= authoritative_roles:
            raise CaptureError(
                f"{candidate} logical sample {sample_index} lacks mandatory roles"
            )
        for role in expected_roles:
            sample = logical_samples_by_role[role]
            observed_roles = set(available_by_pass[role])
            if (
                observed_roles != authoritative_roles
                or sample["target_present"] != (role in authoritative_roles)
            ):
                raise CaptureError(
                    f"{candidate} logical sample {sample_index} role evidence "
                    "disagrees across equivalent passes"
                )

        partitions_by_pass = {
            role: _role_partition_from_available_targets(
                logical_samples_by_role[role]["available_targets"],
                role_order=expected_roles,
            )
            for role in expected_roles
        }
        authoritative_partition = partitions_by_pass[expected_roles[0]]
        if any(
            partition != authoritative_partition
            for partition in partitions_by_pass.values()
        ):
            raise CaptureError(
                f"{candidate} logical sample {sample_index} alias topology "
                "disagrees across equivalent passes"
            )

        canonical_by_role = {
            role: group[0]
            for group in authoritative_partition
            for role in group
        }
        present_samples = [
            logical_samples_by_role[role]
            for role in expected_roles
            if logical_samples_by_role[role]["target_present"]
        ]
        contribution_samples = [
            sample
            for sample in present_samples
            if canonical_by_role[sample["target_role"]] == sample["target_role"]
        ]
        for sample in present_samples:
            identity = (sample["target_pid"], sample["target_birth_unix_ns"])
            captured_process_identities[identity] = max(
                captured_process_identities.get(identity, 0),
                sample["observed_alive_through_unix_ns"],
            )
            if sample["target_role"] != "host":
                helper_pids.add(sample["target_pid"])
                helper_pids_by_role[sample["target_role"]].add(
                    sample["target_pid"]
                )
        for sample in contribution_samples:
            contribution_totals_by_role[sample["target_role"]].append(
                sample["process_total"]
            )

        role_aliases = []
        for group in authoritative_partition:
            if len(group) == 1:
                continue
            pass_advertised_identities = []
            for pass_role in expected_roles:
                advertised_targets = available_by_pass[pass_role]
                group_identities = {
                    (
                        advertised_targets[role]["pid"],
                        advertised_targets[role]["birth_unix_ns"],
                    )
                    for role in group
                }
                if len(group_identities) != 1:
                    raise CaptureError(
                        f"{candidate} logical sample {sample_index} alias "
                        f"evidence is inconsistent in the {pass_role} pass"
                    )
                pid, birth_unix_ns = next(iter(group_identities))
                pass_advertised_identities.append(
                    {
                        "target_role": pass_role,
                        "pid": pid,
                        "birth_unix_ns": birth_unix_ns,
                    }
                )
            role_aliases.append(
                {
                    "canonical_target_role": group[0],
                    "aliased_target_roles": list(group[1:]),
                    "pass_advertised_identities": pass_advertised_identities,
                }
            )

        process_totals = [
            _validated_process_total(sample["process_total"])
            for sample in contribution_samples
        ]
        role_intervals: list[dict[str, Any]] = []
        for role in expected_roles:
            sample = logical_samples_by_role[role]
            interval: dict[str, Any] = {
                "sample_index": sample_index,
                "target_role": sample["target_role"],
                "target_present": sample["target_present"],
                "started_unix_ns": sample["started_unix_ns"],
                "ended_unix_ns": sample["ended_unix_ns"],
                "contribution_included": False,
            }
            if sample["target_present"]:
                canonical_role = canonical_by_role[sample["target_role"]]
                included = sample["target_role"] == canonical_role
                interval.update(
                    {
                        "target_pid": sample["target_pid"],
                        "target_birth_unix_ns": sample["target_birth_unix_ns"],
                        "observed_alive_through_unix_ns": sample[
                            "observed_alive_through_unix_ns"
                        ],
                        "trace_segment": sample["trace_segment"],
                        "recording_readiness_basis": sample[
                            "recording_readiness_basis"
                        ],
                        "timestamp_boundary_uncertainty_ns": sample[
                            "timestamp_boundary_uncertainty_ns"
                        ],
                        "required_allocation_pids": (
                            [sample["target_pid"]] if included else []
                        ),
                        "contribution_included": included,
                    }
                )
                if not included:
                    interval["alias_of_target_role"] = canonical_role
            role_intervals.append(interval)
            role_intervals_all.append(interval)

        sample_totals = {
            field: sum(total[field] for total in process_totals)
            for field in PROCESS_ALLOCATION_FIELDS
        }
        if (
            sample_totals["retained_allocations_lower_bound"]
            > sample_totals["retained_allocations"]
            or sample_totals["retained_allocations"]
            > sample_totals["retained_allocations_upper_bound"]
        ):
            raise CaptureError(
                f"{candidate} sample {sample_index} retained bounds do not reconcile"
            )
        measurement_samples.append(
            {
                "sample_index": sample_index,
                "measurement_mode": "equivalent_exact_process_role_passes",
                "role_intervals": role_intervals,
                "role_aliases": role_aliases,
                "absent_target_roles": [
                    logical_samples_by_role[role]["target_role"]
                    for role in expected_roles
                    if not logical_samples_by_role[role]["target_present"]
                ],
                "required_allocation_pids": [
                    sample["target_pid"] for sample in contribution_samples
                ],
                **sample_totals,
                "process_totals": process_totals,
            }
        )

    merged_totals_by_role = {
        role: _merge_exact_process_totals(totals)
        for role, totals in contribution_totals_by_role.items()
        if totals
    }
    host_role_totals = merged_totals_by_role.get("host", [])
    if len(host_role_totals) != 1:
        raise CaptureError(f"{candidate} host has no single exact-process total")
    host_process_totals = host_role_totals[0]
    helper_process_totals = [
        total
        for role in expected_roles
        if role != "host"
        for total in merged_totals_by_role.get(role, [])
    ]
    all_process_totals = [host_process_totals, *helper_process_totals]
    aggregate = {
        field: sum(sample[field] for sample in measurement_samples)
        for field in PROCESS_ALLOCATION_FIELDS
    }
    if any(
        aggregate[field] != sum(total[field] for total in all_process_totals)
        for field in PROCESS_ALLOCATION_FIELDS
    ):
        raise CaptureError(f"{candidate} retained process totals do not reconcile")

    ordered_passes = [role_passes[role] for role in expected_roles]
    return {
        "candidate": candidate,
        "driver_pid": os.getpid(),
        "host_pid": host_pid,
        "helper_pids": sorted(helper_pids),
        "helper_target_roles": [
            {
                "target_role": role,
                "pids": sorted(helper_pids_by_role[role]),
                "present_sample_count": sum(
                    sample["target_present"] for sample in samples_by_role[role]
                ),
                "absent_sample_count": sum(
                    not sample["target_present"] for sample in samples_by_role[role]
                ),
            }
            for role in expected_roles
            if role != "host"
        ],
        "started_unix_ns": min(
            interval["started_unix_ns"] for interval in role_intervals_all
        ),
        "ended_unix_ns": max(
            interval["ended_unix_ns"] for interval in role_intervals_all
        ),
        "measurement_mode": "equivalent_exact_process_role_passes",
        "recording_readiness_bases": sorted(
            {
                sample["recording_readiness_basis"]
                for samples in samples_by_role.values()
                for sample in samples
                if sample["target_present"]
            }
        ),
        "role_measurement_intervals": sorted(
            role_intervals_all, key=lambda interval: interval["started_unix_ns"]
        ),
        "helper_pid_source": (
            "none; exact SRUI host attachment"
            if candidate == "srui"
            else "exact WKWebView helper-role PID from pre-measurement handshake"
        ),
        "process_identities": [
            {
                "pid": pid,
                "birth_unix_ns": birth_unix_ns,
                "observed_alive_through_unix_ns": observed_alive_through_unix_ns,
            }
            for (pid, birth_unix_ns), observed_alive_through_unix_ns in sorted(
                captured_process_identities.items()
            )
        ],
        "measurement_sample_count": len(measurement_samples),
        "measurement_samples": measurement_samples,
        **aggregate,
        "host_retained_allocations": host_process_totals["retained_allocations"],
        "host_retained_bytes": host_process_totals["retained_bytes"],
        "helper_retained_allocations": sum(
            total["retained_allocations"] for total in helper_process_totals
        ),
        "helper_retained_bytes": sum(
            total["retained_bytes"] for total in helper_process_totals
        ),
        "host_process_totals": host_process_totals,
        "helper_process_totals": helper_process_totals,
        "helpers_without_retained_rows": sorted(
            {
                total["pid"]
                for total in helper_process_totals
                if total["retained_allocations"] == 0
            }
        ),
        **_candidate_equivalence(candidate, ordered_passes),
    }


def build_exact_process_summary(
    pass_results: list[dict[str, Any]],
    *,
    trace_directory: Path,
    reported_trace: Path,
    instrumentation: dict[str, Any],
) -> dict[str, Any]:
    indexed: dict[tuple[str, str], dict[str, Any]] = {}
    for item in pass_results:
        if not isinstance(item, dict):
            raise CaptureError("targeted allocation pass result is not an object")
        key = (item.get("candidate"), item.get("target_role"))
        if key in indexed:
            raise CaptureError(f"duplicate targeted allocation pass: {key}")
        indexed[key] = item
    if set(indexed) != set(CAPTURE_PASSES):
        raise CaptureError(
            "targeted capture must contain SRUI host plus WebKit host, "
            "WebContent, Network, and GPU role passes"
        )
    candidates = [
        _combine_candidate_passes(
            "srui", role_passes={"host": indexed[("srui", "host")]}
        ),
        _combine_candidate_passes(
            "webkit",
            role_passes={
                role: indexed[("webkit", role)] for role in WEBKIT_TARGET_ROLES
            },
        ),
    ]
    role_samples = [
        sample for pass_result in pass_results for sample in pass_result["samples"]
    ]
    target_samples = [sample for sample in role_samples if sample["target_present"]]
    if not target_samples:
        raise CaptureError("targeted xctrace capture contains no present targets")
    common_string_fields = (
        "allocation_export_basis",
        "allocation_timestamp_basis",
        "metric_semantics",
        "whole_trace_statistics_semantics",
    )
    common_values: dict[str, str] = {}
    for field in common_string_fields:
        values = {sample[field] for sample in target_samples}
        if len(values) != 1:
            raise CaptureError(f"targeted xctrace segments disagree on {field}")
        common_values[field] = next(iter(values))
    readiness_bases = sorted(
        {sample["recording_readiness_basis"] for sample in target_samples}
    )
    resolutions = {
        sample["timestamp_boundary_uncertainty_ns"] for sample in target_samples
    }
    if len(resolutions) != 1:
        raise CaptureError("targeted xctrace segments disagree on timestamp precision")
    allocation_rows = sum(sample["allocation_rows"] for sample in target_samples)
    if allocation_rows <= 0:
        raise CaptureError("targeted xctrace capture contains no live allocation rows")
    statistics_aggregate = {
        group: {
            field: sum(
                sample["whole_trace_statistics"][group][field]
                for sample in target_samples
            )
            for field in WHOLE_TRACE_STATISTIC_FIELDS
        }
        for group in WHOLE_TRACE_STATISTIC_GROUPS
    }
    _validate_whole_trace_statistics(statistics_aggregate)
    included_trace_segments = {
        interval["trace_segment"]
        for candidate in candidates
        for sample in candidate["measurement_samples"]
        for interval in sample["role_intervals"]
        if interval["target_present"] and interval["contribution_included"]
    }
    xctrace_versions = sorted(
        {sample["xctrace"]["instruments_version"] for sample in target_samples}
    )
    platforms = sorted({sample["xctrace"]["platform"] for sample in target_samples})
    os_versions = sorted({sample["xctrace"]["os_version"] for sample in target_samples})
    return {
        "schema_version": 3,
        "capture_scope": "exact_processes",
        "capture_method": (
            "xctrace Allocations --attach per exact target process, exported "
            "through Statistics and Allocations List view details"
        ),
        **common_values,
        "recording_readiness_bases": readiness_bases,
        "instrumentation": instrumentation,
        "xctrace_environment": {
            "instruments_versions": xctrace_versions,
            "platforms": platforms,
            "os_versions": os_versions,
            "statistics_xpath": STATISTICS_XPATH,
            "allocations_list_xpath": ALLOCATIONS_LIST_XPATH,
        },
        "trace": str(reported_trace),
        "trace_layout": "directory of per-target per-sample trace bundles",
        "trace_bytes": trace_size_bytes(trace_directory),
        "role_measurement_count": len(role_samples),
        "target_capture_count": len(target_samples),
        "deduplicated_alias_capture_count": (
            len(target_samples) - len(included_trace_segments)
        ),
        "trace_start_timestamp_resolution_ns": max(
            sample["trace_start_timestamp_resolution_ns"]
            for sample in target_samples
        ),
        "allocation_list_timestamp_resolution_ns": max(
            sample["allocation_list_timestamp_resolution_ns"]
            for sample in target_samples
        ),
        "timestamp_boundary_uncertainty_ns": next(iter(resolutions)),
        "process_identity_basis": (
            "each segment TOC names exactly one attached PID equal to the "
            "requested PID; benchmark birth/liveness handshakes bound it"
        ),
        "trace_started_unix_ns": min(
            sample["trace_started_unix_ns"] for sample in target_samples
        ),
        "allocation_rows": allocation_rows,
        "allocation_list_bytes": sum(
            sample["allocation_list_bytes"] for sample in target_samples
        ),
        "allocation_list_reconciled": all(
            sample["allocation_list_reconciled"] for sample in target_samples
        ),
        "whole_trace_statistics_aggregate": statistics_aggregate,
        **{
            field: sum(sample[field] for sample in target_samples)
            for field in SEGMENT_COUNTER_FIELDS
        },
        "exact_process_identity_count": sum(
            len(candidate["process_identities"]) for candidate in candidates
        ),
        "absent_role_measurements": [
            {
                "candidate": sample["candidate"],
                "sample_index": sample["sample_index"],
                "target_role": sample["target_role"],
                "started_unix_ns": sample["started_unix_ns"],
                "ended_unix_ns": sample["ended_unix_ns"],
                "available_target_roles": [
                    target["role"] for target in sample["available_targets"]
                ],
            }
            for sample in role_samples
            if not sample["target_present"]
        ],
        "target_captures": [
            {
                "candidate": sample["candidate"],
                "sample_index": sample["sample_index"],
                "target_role": sample["target_role"],
                "target_pid": sample["target_pid"],
                "target_birth_unix_ns": sample["target_birth_unix_ns"],
                "started_unix_ns": sample["started_unix_ns"],
                "ended_unix_ns": sample["ended_unix_ns"],
                "observed_alive_through_unix_ns": sample[
                    "observed_alive_through_unix_ns"
                ],
                "trace_segment": sample["trace_segment"],
                "recording_readiness_basis": sample[
                    "recording_readiness_basis"
                ],
                "included_in_candidate_total": (
                    sample["trace_segment"] in included_trace_segments
                ),
                "trace_bytes": sample["trace_bytes"],
                "allocation_rows": sample["allocation_rows"],
                "allocation_list_bytes": sample["allocation_list_bytes"],
                "vm_category_rows": sample["vm_category_rows"],
                "vm_category_bytes": sample["vm_category_bytes"],
                "retained_allocations": sample["retained_allocations"],
                "retained_bytes": sample["retained_bytes"],
                "retained_allocations_lower_bound": sample[
                    "retained_allocations_lower_bound"
                ],
                "retained_allocations_upper_bound": sample[
                    "retained_allocations_upper_bound"
                ],
                "retained_bytes_lower_bound": sample[
                    "retained_bytes_lower_bound"
                ],
                "retained_bytes_upper_bound": sample[
                    "retained_bytes_upper_bound"
                ],
                "boundary_ambiguous_allocations": sample[
                    "boundary_ambiguous_allocations"
                ],
                "boundary_ambiguous_bytes": sample[
                    "boundary_ambiguous_bytes"
                ],
                "whole_trace_statistics": sample["whole_trace_statistics"],
                "xctrace": sample["xctrace"],
            }
            for sample in target_samples
        ],
        "candidate_processes": candidates,
    }


def _write_json_atomic(path: Path, payload: Any) -> None:
    temporary = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    try:
        temporary.write_text(
            json.dumps(payload, indent=2, sort_keys=True, allow_nan=False) + "\n",
            encoding="utf-8",
        )
        os.replace(temporary, path)
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
    if isinstance(error, BaseExceptionGroup):
        error.add_note(f"{label} failed; detailed recovery members are preserved")
        return error
    if termination_exceptions(error):
        error.add_note(f"{label} was attempted before this termination propagated")
        return error
    return CaptureError(f"{label}: {type(error).__name__}: {error}")


def cleanup_capture(
    *,
    driver: ManagedProcess | None,
    recorder: ManagedProcess | None,
    watcher: ManagedProcess | None,
    workspace: CaptureWorkspace,
    retain_outputs: bool,
) -> list[BaseException]:
    errors: list[BaseException] = []
    process_actions: list[tuple[str, Any]] = []
    if driver is not None and not driver.closed:
        process_actions.append(
            (
                "BenchmarkDriver cleanup",
                lambda: terminate_managed_process_with_retry(driver),
            )
        )
    if recorder is not None and not recorder.closed:
        process_actions.append(
            (
                "xctrace recorder cleanup",
                lambda: terminate_managed_process_with_retry(recorder),
            )
        )
    if watcher is not None and not watcher.closed:
        process_actions.append(
            (
                "notification watcher cleanup",
                lambda: terminate_managed_process_with_retry(watcher),
            )
        )

    for label, action in process_actions:
        try:
            action()
        except BaseException as error:
            errors.append(_cleanup_failure(label, error))

    if not retain_outputs or errors:
        try:
            remove_owned_capture(workspace)
        except BaseException as error:
            errors.append(_cleanup_failure("owned capture workspace removal", error))
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
    profile: str = "full",
) -> int:
    if profile not in PROFILE_SAMPLE_COUNTS:
        raise CaptureError(f"unsupported benchmark profile: {profile}")
    binary = binary.resolve(strict=False)
    fixture = fixture.resolve(strict=False)
    sidecar = trace.with_name(f"{trace.name}.summary.json")
    validate_output_destinations(trace, sidecar, result)
    preflight_capture(
        trace,
        max_trace_bytes=max_trace_bytes,
        max_export_bytes=max_export_bytes,
        min_remaining_bytes=min_remaining_bytes,
    )
    workspace = create_capture_workspace(trace.parent)
    staged_trace = workspace.path / "capture.trace"
    staged_sidecar = workspace.path / "capture.trace.summary.json"
    staged_result = workspace.path / "renderer-result.json"
    summary_directory = workspace.path / "segment-summaries"
    staged_trace.mkdir()
    summary_directory.mkdir()
    failure: BaseException | None = None

    try:
        staged_binary, instrumentation = stage_instrumentable_binary(binary, workspace)
        pass_results = [
            run_candidate_pass(
                binary=staged_binary,
                fixture=fixture,
                profile=profile,
                candidate=candidate,
                target_role=target_role,
                workspace=workspace,
                trace_directory=staged_trace,
                summary_directory=summary_directory,
                max_trace_bytes=max_trace_bytes,
                max_export_bytes=max_export_bytes,
                min_remaining_bytes=min_remaining_bytes,
            )
            for candidate, target_role in CAPTURE_PASSES
        ]
        summary = build_exact_process_summary(
            pass_results,
            trace_directory=staged_trace,
            reported_trace=trace,
            instrumentation=instrumentation,
        )
        _write_json_atomic(staged_sidecar, summary)
        _write_json_atomic(
            staged_result,
            {
                "schema_version": 1,
                "capture_scope": "exact_processes",
                "profile": profile,
                "instrumentation": instrumentation,
                "candidate_passes": [
                    {
                        "candidate": item["candidate"],
                        "target_role": item["target_role"],
                        "candidate_host_pid": item["host_pid"],
                        "candidate_result": item["candidate_result"],
                    }
                    for item in pass_results
                ],
            },
        )
        publish_capture_outputs(
            staged_trace=staged_trace,
            staged_sidecar=staged_sidecar,
            staged_result=staged_result,
            trace=trace,
            sidecar=sidecar,
            result=result,
        )
    except BaseException as error:
        failure = error

    cleanup_errors: list[BaseException] = []
    try:
        remove_owned_capture(workspace)
    except BaseException as error:
        cleanup_errors.append(
            _cleanup_failure("owned capture workspace removal", error)
        )
    _raise_with_cleanup(
        "targeted allocation capture and cleanup failed",
        failure,
        cleanup_errors,
    )
    return 0


def main(argv: list[str] | None = None) -> int:
    arguments = sys.argv[1:] if argv is None else argv
    if len(arguments) not in {4, 5}:
        print(
            "usage: run_xctrace.py TRACE BINARY FIXTURE RESULT [smoke|full]",
            file=sys.stderr,
        )
        return 2
    trace, binary, fixture, result = map(Path, arguments[:4])
    profile = arguments[4] if len(arguments) == 5 else "full"
    if profile not in {"smoke", "full"}:
        print(f"unsupported benchmark profile: {profile}", file=sys.stderr)
        return 2
    sidecar = trace.with_name(f"{trace.name}.summary.json")
    try:
        validate_output_destinations(trace, sidecar, result)
    except CaptureError as error:
        print(str(error), file=sys.stderr)
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
        profile=profile,
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
        companions = non_termination_exceptions(error)
        if terminations:
            if companions:
                print(
                    "allocation capture failures accompanying interruption: "
                    + "; ".join(
                        f"{type(companion).__name__}: {companion}"
                        for companion in companions
                    ),
                    file=sys.stderr,
                )
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
