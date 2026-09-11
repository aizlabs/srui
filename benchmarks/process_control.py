"""Run a command under a process-group sentinel so every descendant can be reaped safely."""

from __future__ import annotations

import contextlib
import ctypes
import errno
import functools
import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from collections.abc import Callable, Iterator
from dataclasses import dataclass
from pathlib import Path
from typing import Any

RUNNER = Path(__file__).resolve().with_name("process_group_runner.py")
DEFAULT_CLEANUP_GRACE_SECONDS = 1.0
DEFAULT_IDENTITY_EXIT_TIMEOUT_SECONDS = 5.0
POLL_SECONDS = 0.02
TERMINATION_SIGNALS = frozenset((signal.SIGINT, signal.SIGTERM))
PROC_PIDTBSDINFO = 3


class ManagedCommandError(RuntimeError):
    pass


class ManagedCommandTimeout(TimeoutError):
    pass


@dataclass(frozen=True)
class CommandResult:
    returncode: int
    stdout: str
    stderr: str
    child_pid: int


@contextlib.contextmanager
def blocked_termination_signals() -> Iterator[None]:
    """Defer handled termination until a spawned process handle is registered."""

    previous = signal.pthread_sigmask(signal.SIG_BLOCK, TERMINATION_SIGNALS)
    try:
        yield
    finally:
        signal.pthread_sigmask(signal.SIG_SETMASK, previous)


class _ProcBSDInfo(ctypes.Structure):
    _fields_ = [
        ("pbi_flags", ctypes.c_uint32),
        ("pbi_status", ctypes.c_uint32),
        ("pbi_xstatus", ctypes.c_uint32),
        ("pbi_pid", ctypes.c_uint32),
        ("pbi_ppid", ctypes.c_uint32),
        ("pbi_uid", ctypes.c_uint32),
        ("pbi_gid", ctypes.c_uint32),
        ("pbi_ruid", ctypes.c_uint32),
        ("pbi_rgid", ctypes.c_uint32),
        ("pbi_svuid", ctypes.c_uint32),
        ("pbi_svgid", ctypes.c_uint32),
        ("pbi_rfu_1", ctypes.c_uint32),
        ("pbi_comm", ctypes.c_char * 16),
        ("pbi_name", ctypes.c_char * 32),
        ("pbi_nfiles", ctypes.c_uint32),
        ("pbi_pgid", ctypes.c_uint32),
        ("pbi_pjobc", ctypes.c_uint32),
        ("e_tdev", ctypes.c_uint32),
        ("e_tpgid", ctypes.c_uint32),
        ("pbi_nice", ctypes.c_int32),
        ("pbi_start_tvsec", ctypes.c_uint64),
        ("pbi_start_tvusec", ctypes.c_uint64),
    ]


@functools.lru_cache(maxsize=1)
def _proc_pidinfo() -> Any:
    if sys.platform != "darwin":
        raise ManagedCommandError("process birth identity is available only on macOS")
    library = ctypes.CDLL("/usr/lib/libproc.dylib", use_errno=True)
    function = library.proc_pidinfo
    function.argtypes = [
        ctypes.c_int,
        ctypes.c_int,
        ctypes.c_uint64,
        ctypes.c_void_p,
        ctypes.c_int,
    ]
    function.restype = ctypes.c_int
    return function


def process_birth_unix_ns(pid: int) -> int | None:
    """Return a Darwin process birth token, or None when that PID is absent."""

    if isinstance(pid, bool) or not isinstance(pid, int) or pid <= 0:
        raise ManagedCommandError(f"invalid process PID for birth identity: {pid!r}")
    info = _ProcBSDInfo()
    ctypes.set_errno(0)
    result = _proc_pidinfo()(
        pid,
        PROC_PIDTBSDINFO,
        0,
        ctypes.byref(info),
        ctypes.sizeof(info),
    )
    if result <= 0:
        error_number = ctypes.get_errno()
        if error_number in (0, errno.ESRCH):
            return None
        raise ManagedCommandError(
            f"cannot read birth identity for process {pid}: "
            f"{os.strerror(error_number)}"
        )
    if result != ctypes.sizeof(info) or info.pbi_pid != pid:
        raise ManagedCommandError(
            f"process {pid} returned an incomplete or mismatched birth identity"
        )
    birth_unix_ns = (
        int(info.pbi_start_tvsec) * 1_000_000_000
        + int(info.pbi_start_tvusec) * 1_000
    )
    if birth_unix_ns <= 0:
        raise ManagedCommandError(f"process {pid} returned an invalid birth identity")
    return birth_unix_ns


def wait_for_process_identities_gone(
    identities: list[tuple[int, int]],
    *,
    label: str,
    timeout: float = DEFAULT_IDENTITY_EXIT_TIMEOUT_SECONDS,
    identity_reader: Callable[[int], int | None] | None = None,
) -> None:
    """Drain exact identities before propagating handled termination signals."""

    previous_mask = signal.pthread_sigmask(signal.SIG_BLOCK, TERMINATION_SIGNALS)
    postcondition_error: BaseException | None = None
    try:
        if timeout < 0:
            raise ValueError("process identity timeout must be non-negative")
        expected: dict[int, int] = {}
        for pid, birth_unix_ns in identities:
            if (
                isinstance(pid, bool)
                or not isinstance(pid, int)
                or pid <= 0
                or isinstance(birth_unix_ns, bool)
                or not isinstance(birth_unix_ns, int)
                or birth_unix_ns <= 0
            ):
                raise ManagedCommandError(f"{label} contains an invalid process identity")
            if pid in expected:
                raise ManagedCommandError(f"{label} contains duplicate process PID {pid}")
            expected[pid] = birth_unix_ns

        reader = identity_reader or process_birth_unix_ns
        deadline = time.monotonic() + timeout
        pending = dict(expected)
        while pending:
            for pid, original_birth in list(pending.items()):
                current_birth = reader(pid)
                if current_birth is None or current_birth != original_birth:
                    del pending[pid]
            if not pending:
                break
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                details = ", ".join(
                    f"{pid}@{birth}" for pid, birth in sorted(pending.items())
                )
                raise ManagedCommandError(
                    f"{label} still has original process identities alive: {details}"
                )
            time.sleep(min(POLL_SECONDS, remaining))
    except BaseException as error:
        postcondition_error = error

    deferred_termination: BaseException | None = None
    try:
        signal.pthread_sigmask(signal.SIG_SETMASK, previous_mask)
    except BaseException as error:
        deferred_termination = error

    if postcondition_error is not None and deferred_termination is not None:
        raise BaseExceptionGroup(
            f"{label} identity postcondition and deferred termination both failed",
            [postcondition_error, deferred_termination],
        )
    if postcondition_error is not None:
        raise postcondition_error
    if deferred_termination is not None:
        raise deferred_termination


def spawn_supervisor(
    command: list[str],
    *,
    cwd: Path,
    ready_path: Path,
    status_path: Path,
    stdout_path: Path,
    stderr_path: Path,
    require_exact_owner_identity: bool = False,
) -> subprocess.Popen[str]:
    if not isinstance(require_exact_owner_identity, bool):
        raise ManagedCommandError("exact-owner requirement must be boolean")
    owner_pid = os.getpid()
    owner_arguments = ["--owner-pid", str(owner_pid)]
    if require_exact_owner_identity:
        owner_birth_unix_ns = process_birth_unix_ns(owner_pid)
        if owner_birth_unix_ns is None:
            raise ManagedCommandError(
                "cannot establish exact owner identity before supervisor spawn"
            )
        owner_arguments.extend(
            ["--owner-birth-unix-ns", str(owner_birth_unix_ns)]
        )
    return subprocess.Popen(
        [
            sys.executable,
            str(RUNNER),
            *owner_arguments,
            "--ready",
            str(ready_path),
            "--status",
            str(status_path),
            "--stdout",
            str(stdout_path),
            "--stderr",
            str(stderr_path),
            "--",
            *command,
        ],
        cwd=cwd,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        start_new_session=True,
        close_fds=True,
    )


def process_group_members(process_group: int) -> set[int]:
    result = subprocess.run(
        ["ps", "-axo", "pid=,pgid="],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        check=False,
        timeout=5,
    )
    if result.returncode:
        raise ManagedCommandError(
            f"cannot enumerate process group {process_group}: {result.stderr.strip()}"
        )
    members: set[int] = set()
    for line in result.stdout.splitlines():
        fields = line.split()
        if len(fields) != 2:
            continue
        try:
            pid, pgid = map(int, fields)
        except ValueError:
            continue
        if pgid == process_group:
            members.add(pid)
    return members


def _signal_group(process_group: int, signum: int) -> None:
    try:
        os.killpg(process_group, signum)
    except ProcessLookupError:
        pass


def _cleanup_detail(errors: list[tuple[str, BaseException]]) -> str:
    return "; ".join(
        f"{label}: {type(error).__name__}: {error}" for label, error in errors
    )


def termination_exceptions(error: BaseException) -> list[BaseException]:
    """Return non-Exception termination requests, including nested groups."""

    if isinstance(error, BaseExceptionGroup):
        return [
            termination
            for nested in error.exceptions
            for termination in termination_exceptions(nested)
        ]
    return [error] if not isinstance(error, Exception) else []


def non_termination_exceptions(error: BaseException) -> list[Exception]:
    """Flatten ordinary companion failures from a BaseException tree."""

    if isinstance(error, BaseExceptionGroup):
        return [
            companion
            for nested in error.exceptions
            for companion in non_termination_exceptions(nested)
        ]
    return [error] if isinstance(error, Exception) else []


def raise_termination_exceptions(
    errors: list[BaseException],
    *,
    label: str,
) -> None:
    terminations = [
        termination
        for error in errors
        for termination in termination_exceptions(error)
    ]
    if len(terminations) == 1:
        raise terminations[0]
    if terminations:
        raise BaseExceptionGroup(label, terminations)


def terminate_supervised_process(
    process: subprocess.Popen[str],
    grace_seconds: float = DEFAULT_CLEANUP_GRACE_SECONDS,
) -> tuple[str, str]:
    """Kill and reap a sentinel-pinned group, even if graceful cleanup fails."""

    if process.poll() is not None:
        # Once waitpid has reaped the sentinel, its numeric PID no longer pins the
        # process-group identity. Never inspect or signal that number as a PGID.
        return process.communicate(timeout=5)

    try:
        process_group = os.getpgid(process.pid)
    except ProcessLookupError:
        return process.communicate(timeout=5)
    if process_group != process.pid:
        try:
            process.kill()
        finally:
            process.communicate(timeout=5)
        raise ManagedCommandError(
            f"supervisor {process.pid} is not its process-group leader ({process_group})"
        )

    try:
        # The stopped, unreaped sentinel pins the group ID until the final group kill.
        os.kill(process.pid, signal.SIGSTOP)
    except ProcessLookupError:
        return process.communicate(timeout=5)
    except BaseException as error:
        try:
            process.kill()
        finally:
            process.communicate(timeout=5)
        raise ManagedCommandError(
            f"could not pin supervised process group {process_group}: "
            f"{type(error).__name__}: {error}"
        ) from error

    recoverable_errors: list[tuple[str, BaseException]] = []
    try:
        _signal_group(process_group, signal.SIGTERM)
    except BaseException as error:
        recoverable_errors.append(("SIGTERM", error))

    if not recoverable_errors:
        try:
            deadline = time.monotonic() + grace_seconds
            while time.monotonic() < deadline:
                if process_group_members(process_group) <= {process.pid}:
                    break
                time.sleep(POLL_SECONDS)
        except BaseException as error:
            recoverable_errors.append(("process-group enumeration", error))

    try:
        # A failed final group kill leaves the stopped sentinel alive. Its unreaped
        # PID is the only proof that a retry still addresses the original group.
        os.killpg(process_group, signal.SIGKILL)
    except BaseException as error:
        errors = [*recoverable_errors, ("pinned-group SIGKILL", error)]
        failure = ManagedCommandError(
            f"could not confirm cleanup of supervised process group {process_group}: "
            f"{_cleanup_detail(errors)}; stopped sentinel retained for retry"
        )
        terminations = [
            termination
            for _label, item in errors
            for termination in termination_exceptions(item)
        ]
        if terminations:
            raise BaseExceptionGroup(
                f"termination requested while process group {process_group} "
                "remains pinned for retry",
                [*terminations, failure],
            )
        raise failure from error

    final_errors: list[tuple[str, BaseException]] = []
    output: tuple[str, str] | None = None
    try:
        output = process.communicate(timeout=5)
    except BaseException as error:
        final_errors.append(("supervisor reap", error))
        try:
            process.kill()
        except ProcessLookupError:
            pass
        except BaseException as direct_error:
            final_errors.append(("fallback supervisor SIGKILL", direct_error))
        try:
            output = process.communicate(timeout=5)
        except BaseException as reap_error:
            final_errors.append(("fallback supervisor reap", reap_error))

    errors = [*recoverable_errors, *final_errors]
    if output is None or process.poll() is None:
        failure = ManagedCommandError(
            f"could not confirm cleanup of supervised process group {process_group}: "
            f"{_cleanup_detail(errors)}"
        )
        terminations = [
            termination
            for _label, item in errors
            for termination in termination_exceptions(item)
        ]
        if terminations:
            raise BaseExceptionGroup(
                f"termination requested and process group {process_group} cleanup failed",
                [*terminations, failure],
            )
        raise failure

    # Ordinary graceful-cleanup failures are superseded by the authoritative
    # pinned-group SIGKILL/reap. Termination requests still propagate afterward.
    raise_termination_exceptions(
        [error for _label, error in errors],
        label=f"multiple termination requests while cleaning process group {process_group}",
    )
    return output


class ManagedProcess:
    def __init__(
        self,
        command: list[str],
        cwd: Path,
        label: str,
        cleanup_grace_seconds: float,
    ) -> None:
        self.command = command
        self.cwd = cwd
        self.label = label
        self.cleanup_grace_seconds = cleanup_grace_seconds
        self._temporary = Path(tempfile.mkdtemp(prefix="srui-benchmark-process-"))
        directory = self._temporary
        self.ready_path = directory / "ready.json"
        self.status_path = directory / "status.json"
        self.stdout_path = directory / "stdout"
        self.stderr_path = directory / "stderr"
        self.supervisor: subprocess.Popen[str] | None = None
        self._supervisor_output = ("", "")
        self._reaped = False
        self._controls_cleaned = False

    @classmethod
    def start(
        cls,
        command: list[str],
        *,
        cwd: Path,
        label: str,
        cleanup_grace_seconds: float = DEFAULT_CLEANUP_GRACE_SECONDS,
        require_exact_owner_identity: bool = False,
    ) -> ManagedProcess:
        managed = cls(command, cwd, label, cleanup_grace_seconds)
        try:
            # Signals handled by the caller are deferred across both Popen and
            # registration. A pending handler can run only after supervisor is set.
            with blocked_termination_signals():
                supervisor = spawn_supervisor(
                    command,
                    cwd=cwd,
                    ready_path=managed.ready_path,
                    status_path=managed.status_path,
                    stdout_path=managed.stdout_path,
                    stderr_path=managed.stderr_path,
                    require_exact_owner_identity=require_exact_owner_identity,
                )
                managed.supervisor = supervisor
        except BaseException as primary:
            cleanup_errors: list[BaseException] = []
            try:
                if managed.supervisor is None:
                    managed._discard_controls()
                else:
                    terminate_managed_process_with_retry(managed)
            except BaseException as cleanup_error:
                cleanup_errors.append(cleanup_error)
            if cleanup_errors:
                raise BaseExceptionGroup(
                    f"{label} spawn and cleanup failed",
                    [primary, *cleanup_errors],
                )
            raise
        return managed

    @property
    def closed(self) -> bool:
        return self._reaped and self._controls_cleaned

    @property
    def child_pid(self) -> int | None:
        if not self.ready_path.exists():
            return None
        try:
            payload = json.loads(self.ready_path.read_text(encoding="utf-8"))
            child_pid = payload.get("child_pid")
            return child_pid if isinstance(child_pid, int) else None
        except (OSError, json.JSONDecodeError):
            return None

    def wait_until_started(
        self,
        timeout: float,
        *,
        poll_hook: Callable[[ManagedProcess], None] | None = None,
    ) -> int:
        """Wait for a live exec child to remain registered across one poll interval."""

        if timeout <= 0:
            raise ValueError("managed process start timeout must be positive")
        deadline = time.monotonic() + timeout
        observed_pid: int | None = None
        observed_at = 0.0
        while True:
            status = self._status()
            if status is not None:
                raise ManagedCommandError(
                    f"{self.label} exited before its child became ready"
                )
            child_pid = self.child_pid
            now = time.monotonic()
            if child_pid is not None:
                if child_pid != observed_pid:
                    observed_pid = child_pid
                    observed_at = now
                elif now - observed_at >= POLL_SECONDS:
                    return child_pid
            if poll_hook is not None:
                poll_hook(self)
            if now >= deadline:
                raise ManagedCommandTimeout(
                    f"{self.label} did not start within {timeout:g}s"
                )
            time.sleep(min(POLL_SECONDS, deadline - now))

    def signal(self, signum: int) -> None:
        """Signal the pinned supervised group; the sentinel ignores INT and TERM."""

        if self._reaped or self.supervisor is None or self.supervisor.poll() is not None:
            raise ManagedCommandError(f"{self.label} is not running")
        try:
            process_group = os.getpgid(self.supervisor.pid)
        except ProcessLookupError as error:
            raise ManagedCommandError(f"{self.label} process group disappeared") from error
        if process_group != self.supervisor.pid:
            raise ManagedCommandError(
                f"{self.label} supervisor is not its process-group leader"
            )
        os.killpg(process_group, signum)

    def _status(self) -> dict[str, Any] | None:
        if self.status_path.exists():
            try:
                return json.loads(self.status_path.read_text(encoding="utf-8"))
            except (OSError, json.JSONDecodeError) as error:
                raise ManagedCommandError(
                    f"{self.label} supervisor wrote invalid status: {error}"
                ) from error
        if self.supervisor is not None and self.supervisor.poll() is not None:
            stdout, stderr = self.supervisor.communicate(timeout=5)
            raise ManagedCommandError(
                f"{self.label} supervisor exited before status "
                f"({self.supervisor.returncode}): {(stderr or stdout).strip()[-2000:]}"
            )
        return None
    def _reap(self) -> None:
        if self._reaped:
            return
        try:
            if self.supervisor is not None:
                self._supervisor_output = terminate_supervised_process(
                    self.supervisor,
                    self.cleanup_grace_seconds,
                )
        except BaseException:
            # An interruption may be re-raised after an authoritative group reap.
            if self.supervisor is None or self.supervisor.poll() is not None:
                self._reaped = True
            raise
        self._reaped = True

    def _discard_controls(self) -> None:
        if self._controls_cleaned:
            return
        if self._temporary.exists():
            shutil.rmtree(self._temporary)
        self._controls_cleaned = True

    def terminate(self) -> None:
        # Defer handled signals until group cleanup and the control-file decision
        # complete. A pending handler runs when this context exits.
        with blocked_termination_signals():
            try:
                if not self._reaped:
                    self._reap()
            finally:
                # A failed group kill preserves the stopped sentinel and controls.
                if self._reaped:
                    self._discard_controls()
        self._discard_controls()

    def wait(
        self,
        timeout: float,
        *,
        poll_hook: Callable[[ManagedProcess], None] | None = None,
    ) -> CommandResult:
        if self._reaped:
            raise ManagedCommandError(f"{self.label} is already closed")
        deadline = time.monotonic() + timeout
        payload: dict[str, Any] | None = None
        try:
            while payload is None:
                payload = self._status()
                if payload is not None:
                    break
                if poll_hook is not None:
                    poll_hook(self)
                if time.monotonic() >= deadline:
                    raise ManagedCommandTimeout(
                        f"{self.label} timed out after {timeout:g}s"
                    )
                time.sleep(POLL_SECONDS)
        except BaseException as primary:
            try:
                terminate_managed_process_with_retry(self)
            except BaseException as cleanup_error:
                raise BaseExceptionGroup(
                    f"{self.label} failed and cleanup also failed",
                    [primary, cleanup_error],
                )
            raise

        try:
            self._reap()
            stdout = (
                self.stdout_path.read_text(encoding="utf-8", errors="replace")
                if self.stdout_path.exists()
                else ""
            )
            stderr = (
                self.stderr_path.read_text(encoding="utf-8", errors="replace")
                if self.stderr_path.exists()
                else ""
            )
        finally:
            if self._reaped:
                self._discard_controls()

        if payload is None:
            raise ManagedCommandError(f"{self.label} produced no status")
        if "error" in payload:
            supervisor_detail = (
                self._supervisor_output[1] or self._supervisor_output[0]
            ).strip()
            raise ManagedCommandError(
                f"{self.label} supervisor failed: {payload['error']}; "
                f"{supervisor_detail[-1000:]}"
            )
        returncode = payload.get("returncode")
        child_pid = payload.get("child_pid")
        if not isinstance(returncode, int) or not isinstance(child_pid, int):
            raise ManagedCommandError(f"{self.label} supervisor status is incomplete")
        return CommandResult(
            returncode=returncode,
            stdout=stdout,
            stderr=stderr,
            child_pid=child_pid,
        )


def _drain_managed_process_with_retry(
    process: ManagedProcess,
    *,
    attempts: int,
) -> None:
    if attempts <= 0:
        raise ValueError("managed process cleanup attempts must be positive")
    errors: list[BaseException] = []
    for _attempt in range(attempts):
        if process.closed:
            break
        try:
            process.terminate()
        except BaseException as error:
            errors.append(error)
    if process.closed:
        raise_termination_exceptions(
            errors,
            label=f"multiple termination requests while cleaning {process.label}",
        )
        return

    supervisor_identity = (
        str(process.supervisor.pid)
        if process.supervisor is not None
        else "unregistered"
    )
    failure = ManagedCommandError(
        f"{process.label} remains pinned after {attempts} bounded cleanup attempts; "
        f"retained supervisor/PGID {supervisor_identity}; "
        f"control directory {process.ready_path.parent}; "
        "retry with the retained ManagedProcess handle"
    )
    failure.process_handle = process  # type: ignore[attr-defined]
    if errors:
        raise BaseExceptionGroup(
            f"{process.label} cleanup retry failed",
            [*errors, failure],
        )
    raise failure


def terminate_managed_process_with_retry(
    process: ManagedProcess,
    *,
    attempts: int = 2,
) -> None:
    """Run bounded cleanup before propagating any handled termination signal."""

    previous_mask = signal.pthread_sigmask(signal.SIG_BLOCK, TERMINATION_SIGNALS)
    cleanup_error: BaseException | None = None
    try:
        _drain_managed_process_with_retry(process, attempts=attempts)
    except BaseException as error:
        cleanup_error = error

    deferred_termination: BaseException | None = None
    try:
        signal.pthread_sigmask(signal.SIG_SETMASK, previous_mask)
    except BaseException as error:
        deferred_termination = error

    if cleanup_error is not None and deferred_termination is not None:
        raise BaseExceptionGroup(
            f"{process.label} cleanup and deferred termination both failed",
            [cleanup_error, deferred_termination],
        )
    if cleanup_error is not None:
        raise cleanup_error
    if deferred_termination is not None:
        raise deferred_termination


def run_managed_command(
    command: list[str],
    *,
    cwd: Path,
    timeout: float,
    label: str,
    cleanup_grace_seconds: float = DEFAULT_CLEANUP_GRACE_SECONDS,
    poll_hook: Callable[[ManagedProcess], None] | None = None,
) -> CommandResult:
    process = ManagedProcess.start(
        command,
        cwd=cwd,
        label=label,
        cleanup_grace_seconds=cleanup_grace_seconds,
    )
    try:
        return process.wait(timeout, poll_hook=poll_hook)
    except BaseException as primary:
        cleanup_errors: list[BaseException] = []
        if not process.closed:
            try:
                terminate_managed_process_with_retry(process)
            except BaseException as cleanup_error:
                cleanup_errors.append(cleanup_error)

        errors = [primary, *cleanup_errors]
        if process.closed:
            raise_termination_exceptions(
                errors,
                label=f"multiple termination requests while cleaning {label}",
            )
        if cleanup_errors:
            raise BaseExceptionGroup(
                f"{label} failed and bounded cleanup retry also failed",
                errors,
            )
        raise
