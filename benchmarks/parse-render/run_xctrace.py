"""Capture a bounded, optional SRUI-host Allocations diagnostic."""

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
CAPTURE_PASSES = (("srui", "host"),)
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
        "schema_version": 4,
        "diagnostic_only": True,
        "capture_scope": "exact_process_diagnostic",
        "capture_method": "xctrace Allocations --attach to one validated SRUI host PID",
        "trace": str(reported_trace if reported_trace is not None else trace),
        "trace_bytes": trace_size_bytes(trace),
        "allocation_export_basis": (
            "independently materialized Xcode Allocations Statistics and final "
            "live Allocations List view details"
        ),
        "metric_semantics": (
            "whole-trace Statistics and a final live-list diagnostic; neither is "
            "benchmark-interval allocation traffic"
        ),
        "whole_trace_statistics_semantics": (
            "whole-trace heap and anonymous VM aggregates, including the "
            "attach-time live baseline"
        ),
        "final_live_list_semantics": (
            "live rows returned by the Allocations List export when that view "
            "was materialized; it is not required to equal Statistics"
        ),
        "process_identity_basis": (
            "TOC attached PID equals the requested PID and the benchmark "
            "handshake bounds the same PID by birth and observed liveness"
        ),
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
            exact_identity = segment_summary["exact_process_identity"]
        except (OSError, json.JSONDecodeError, KeyError, TypeError) as error:
            raise CaptureError(
                f"targeted allocation summary is incomplete for "
                f"{candidate}/{target_role} sample {sample_index}"
            ) from error
        if (
            segment_summary.get("schema_version") != 4
            or segment_summary.get("diagnostic_only") is not True
            or segment_summary.get("capture_scope") != "exact_process_diagnostic"
            or exact_identity.get("pid") != request["target_pid"]
            or exact_identity.get("birth_unix_ns")
            != request["target_birth_unix_ns"]
            or segment_summary.get("xctrace", {}).get("attached_pid")
            != request["target_pid"]
        ):
            raise CaptureError(
                f"targeted allocation diagnostic does not exactly match "
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
            "metric_semantics": segment_summary["metric_semantics"],
            "whole_trace_statistics_semantics": segment_summary[
                "whole_trace_statistics_semantics"
            ],
            "final_live_list_semantics": segment_summary[
                "final_live_list_semantics"
            ],
            "process_identity_basis": segment_summary["process_identity_basis"],
            "whole_trace_statistics": segment_summary[
                "whole_trace_statistics"
            ],
            "final_live_list": segment_summary["final_live_list"],
            "statistics_minus_final_live_list": segment_summary[
                "statistics_minus_final_live_list"
            ],
            "workload_window": segment_summary["workload_window"],
            "xctrace": segment_summary["xctrace"],
            "exact_process_identity": exact_identity,
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


def _nonnegative_integer(value: Any) -> bool:
    return not isinstance(value, bool) and isinstance(value, int) and value >= 0


def _signed_integer(value: Any) -> bool:
    return not isinstance(value, bool) and isinstance(value, int)


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


def _validate_exact_process_identity(value: Any) -> dict[str, Any]:
    if not isinstance(value, dict) or set(value) != {
        "pid",
        "birth_unix_ns",
        "observed_alive_through_unix_ns",
        "names",
    }:
        raise CaptureError("exact process identity has an invalid contract")
    if (
        not _positive_integer(value["pid"])
        or not _positive_integer(value["birth_unix_ns"])
        or not _positive_integer(value["observed_alive_through_unix_ns"])
        or value["observed_alive_through_unix_ns"] < value["birth_unix_ns"]
        or not isinstance(value["names"], list)
        or not value["names"]
        or any(not isinstance(name, str) or not name for name in value["names"])
    ):
        raise CaptureError("exact process identity contains invalid values")
    return value


def _validate_final_live_list(value: Any) -> dict[str, Any]:
    if not isinstance(value, dict) or set(value) != {
        "allocations",
        "bytes",
        "vm_category_allocations",
        "vm_category_bytes",
        "vm_categories",
        "maximum_elapsed_timestamp_ns",
    }:
        raise CaptureError("final live-list diagnostic has an invalid contract")
    numeric_fields = (
        "allocations",
        "bytes",
        "vm_category_allocations",
        "vm_category_bytes",
        "maximum_elapsed_timestamp_ns",
    )
    if any(not _nonnegative_integer(value[field]) for field in numeric_fields):
        raise CaptureError("final live-list diagnostic contains invalid values")
    categories = value["vm_categories"]
    if (
        not isinstance(categories, dict)
        or any(not isinstance(name, str) or not name.startswith("VM:") for name in categories)
        or any(
            not isinstance(total, dict)
            or set(total) != {"allocations", "bytes"}
            or not _nonnegative_integer(total["allocations"])
            or not _nonnegative_integer(total["bytes"])
            for total in categories.values()
        )
        or value["vm_category_allocations"]
        != sum(total["allocations"] for total in categories.values())
        or value["vm_category_bytes"]
        != sum(total["bytes"] for total in categories.values())
        or value["vm_category_allocations"] > value["allocations"]
        or value["vm_category_bytes"] > value["bytes"]
    ):
        raise CaptureError("final live-list VM diagnostics do not reconcile internally")
    return value


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
            or result.get("captureAuthorization") is not False
            or result.get("pixelCaptureCompletions") != 0
            or not isinstance(result.get("paintCompletionMode"), str)
            or not result["paintCompletionMode"].startswith("non-compositor ")
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
    if not evidence or len(
        {(item["representation_bytes"], item["rendered_node_count"]) for item in evidence}
    ) != 1:
        raise CaptureError(
            f"{candidate} targeted passes did not render equivalent representations"
        )
    return {
        "equivalent_representation_bytes": evidence[0]["representation_bytes"],
        "equivalent_rendered_node_count": evidence[0]["rendered_node_count"],
        "target_passes": evidence,
    }


def _validate_pass_result(item: Any) -> list[dict[str, Any]]:
    if (
        not isinstance(item, dict)
        or item.get("candidate") != "srui"
        or item.get("target_role") != "host"
        or not _positive_integer(item.get("host_pid"))
        or not isinstance(item.get("samples"), list)
        or not item["samples"]
        or not isinstance(item.get("candidate_result"), dict)
    ):
        raise CaptureError("SRUI host diagnostic pass has an invalid contract")
    samples = item["samples"]
    for sample_index, sample in enumerate(samples):
        if (
            not isinstance(sample, dict)
            or sample.get("candidate") != "srui"
            or sample.get("target_role") != "host"
            or sample.get("sample_index") != sample_index
            or sample.get("host_pid") != item["host_pid"]
            or sample.get("target_present") is not True
            or sample.get("target_pid") != item["host_pid"]
            or not isinstance(sample.get("available_targets"), list)
            or len(sample["available_targets"]) != 1
            or sample["available_targets"][0].get("role") != "host"
            or sample["available_targets"][0].get("pid") != item["host_pid"]
            or sample["available_targets"][0].get("birth_unix_ns")
            != sample.get("target_birth_unix_ns")
            or not _positive_integer(sample.get("target_birth_unix_ns"))
            or not _positive_integer(sample.get("observed_alive_through_unix_ns"))
            or not _positive_integer(sample.get("started_unix_ns"))
            or not _positive_integer(sample.get("ended_unix_ns"))
            or not (
                sample["target_birth_unix_ns"]
                <= sample["started_unix_ns"]
                < sample["ended_unix_ns"]
                <= sample["observed_alive_through_unix_ns"]
            )
            or not isinstance(sample.get("trace_segment"), str)
            or not sample["trace_segment"]
            or sample.get("recording_readiness_basis") != "darwin_notification"
            or not _positive_integer(sample.get("trace_bytes"))
            or any(
                not isinstance(sample.get(field), str) or not sample[field].strip()
                for field in (
                    "allocation_export_basis",
                    "metric_semantics",
                    "whole_trace_statistics_semantics",
                    "final_live_list_semantics",
                    "process_identity_basis",
                )
            )
        ):
            raise CaptureError(
                f"srui/host sample {sample_index} has an invalid diagnostic contract"
            )
        statistics = _validate_whole_trace_statistics(
            sample.get("whole_trace_statistics")
        )
        live_list = _validate_final_live_list(sample.get("final_live_list"))
        discrepancy = sample.get("statistics_minus_final_live_list")
        combined = statistics["heap_and_anonymous_vm"]
        if (
            not isinstance(discrepancy, dict)
            or set(discrepancy) != {"persistent_allocations", "persistent_bytes"}
            or any(not _signed_integer(value) for value in discrepancy.values())
            or discrepancy["persistent_allocations"]
            != combined["persistent_allocations"] - live_list["allocations"]
            or discrepancy["persistent_bytes"]
            != combined["persistent_bytes"] - live_list["bytes"]
        ):
            raise CaptureError(
                f"srui/host sample {sample_index} has invalid signed discrepancy diagnostics"
            )
        window = sample.get("workload_window")
        if window != {
            "started_unix_ns": sample["started_unix_ns"],
            "ended_unix_ns": sample["ended_unix_ns"],
            "used_for_allocation_attribution": False,
        }:
            raise CaptureError(
                f"srui/host sample {sample_index} misstates allocation attribution"
            )
        xctrace = sample.get("xctrace")
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
                f"srui/host sample {sample_index} has invalid xctrace target evidence"
            )
        identity = _validate_exact_process_identity(
            sample.get("exact_process_identity")
        )
        if (
            identity["pid"] != sample["target_pid"]
            or identity["birth_unix_ns"] != sample["target_birth_unix_ns"]
            or identity["observed_alive_through_unix_ns"]
            != sample["observed_alive_through_unix_ns"]
            or identity["names"] != [xctrace["attached_process_name"]]
        ):
            raise CaptureError(
                f"srui/host sample {sample_index} process identity does not match"
            )
    return samples


def build_exact_process_summary(
    pass_results: list[dict[str, Any]],
    *,
    trace_directory: Path,
    reported_trace: Path,
    instrumentation: dict[str, Any],
) -> dict[str, Any]:
    if len(pass_results) != 1 or (
        pass_results[0].get("candidate"), pass_results[0].get("target_role")
    ) != CAPTURE_PASSES[0]:
        raise CaptureError("diagnostic capture must contain only the SRUI host pass")
    role_pass = pass_results[0]
    samples = _validate_pass_result(role_pass)
    equivalence = _candidate_equivalence("srui", [role_pass])
    common_fields = (
        "allocation_export_basis",
        "metric_semantics",
        "whole_trace_statistics_semantics",
        "final_live_list_semantics",
        "process_identity_basis",
    )
    common_values: dict[str, str] = {}
    for field in common_fields:
        values = {sample[field] for sample in samples}
        if len(values) != 1:
            raise CaptureError(f"SRUI host xctrace segments disagree on {field}")
        common_values[field] = next(iter(values))
    xctrace_versions = sorted(
        {sample["xctrace"]["instruments_version"] for sample in samples}
    )
    platforms = sorted({sample["xctrace"]["platform"] for sample in samples})
    os_versions = sorted({sample["xctrace"]["os_version"] for sample in samples})
    target_captures = [
        {
            "candidate": "srui",
            "sample_index": sample["sample_index"],
            "target_role": "host",
            "target_pid": sample["target_pid"],
            "target_birth_unix_ns": sample["target_birth_unix_ns"],
            "observed_alive_through_unix_ns": sample[
                "observed_alive_through_unix_ns"
            ],
            "workload_window": sample["workload_window"],
            "trace_segment": sample["trace_segment"],
            "recording_readiness_basis": sample["recording_readiness_basis"],
            "trace_bytes": sample["trace_bytes"],
            "whole_trace_statistics": sample["whole_trace_statistics"],
            "final_live_list": sample["final_live_list"],
            "statistics_minus_final_live_list": sample[
                "statistics_minus_final_live_list"
            ],
            "xctrace": sample["xctrace"],
            "exact_process_identity": sample["exact_process_identity"],
        }
        for sample in samples
    ]
    identities: dict[tuple[int, int], int] = {}
    for sample in samples:
        identity = sample["exact_process_identity"]
        key = (identity["pid"], identity["birth_unix_ns"])
        identities[key] = max(
            identities.get(key, 0),
            identity["observed_alive_through_unix_ns"],
        )
    return {
        "schema_version": 4,
        "diagnostic_only": True,
        "authoritative_benchmark_metric": False,
        "capture_scope": "exact_process_diagnostic",
        "diagnostic_target": "srui_host",
        "capture_method": (
            "bounded xctrace Allocations --attach to one exact SRUI host PID "
            "per sample"
        ),
        **common_values,
        "diagnostic_limitations": (
            "Statistics and Allocations List are independently materialized "
            "whole-trace/final-live views. Their signed discrepancy is reported; "
            "neither view is attributed to the benchmark workload window."
        ),
        "recording_readiness_bases": sorted(
            {sample["recording_readiness_basis"] for sample in samples}
        ),
        "instrumentation": instrumentation,
        "xctrace_environment": {
            "instruments_versions": xctrace_versions,
            "platforms": platforms,
            "os_versions": os_versions,
            "statistics_xpath": STATISTICS_XPATH,
            "allocations_list_xpath": ALLOCATIONS_LIST_XPATH,
        },
        "trace": str(reported_trace),
        "trace_layout": "directory of per-sample SRUI-host trace bundles",
        "trace_bytes": trace_size_bytes(trace_directory),
        "target_capture_count": len(target_captures),
        "exact_process_identity_count": len(identities),
        "target_captures": target_captures,
        "candidate_processes": [
            {
                "candidate": "srui",
                "host_pid": role_pass["host_pid"],
                "measurement_sample_count": len(samples),
                "process_identities": [
                    {
                        "pid": pid,
                        "birth_unix_ns": birth,
                        "observed_alive_through_unix_ns": observed,
                    }
                    for (pid, birth), observed in sorted(identities.items())
                ],
                **equivalence,
            }
        ],
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
                "schema_version": 2,
                "diagnostic_only": True,
                "capture_scope": "exact_process_diagnostic",
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
