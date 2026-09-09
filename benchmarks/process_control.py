"""Run a command under a process-group sentinel so every descendant can be reaped safely."""

from __future__ import annotations

import contextlib
import json
import os
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
POLL_SECONDS = 0.02
TERMINATION_SIGNALS = frozenset((signal.SIGINT, signal.SIGTERM))


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


def spawn_supervisor(
    command: list[str],
    *,
    cwd: Path,
    ready_path: Path,
    status_path: Path,
    stdout_path: Path,
    stderr_path: Path,
) -> subprocess.Popen[str]:
    return subprocess.Popen(
        [
            sys.executable,
            str(RUNNER),
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
        self._temporary = tempfile.TemporaryDirectory(prefix="srui-benchmark-process-")
        directory = Path(self._temporary.name)
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
                )
                managed.supervisor = supervisor
        except BaseException as primary:
            cleanup_errors: list[BaseException] = []
            try:
                if managed.supervisor is None:
                    managed._discard_controls()
                else:
                    managed.terminate()
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
        return self._reaped

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
        self._temporary.cleanup()
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
                self.terminate()
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
                # wait() already tried once. Retry while the sentinel still pins
                # the original group; terminate() remains bounded.
                process.terminate()
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
