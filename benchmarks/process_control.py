"""Run a command under a process-group sentinel so every descendant can be reaped safely."""

from __future__ import annotations

import json
import os
import signal
import subprocess
import sys
import tempfile
import time
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path
from typing import Any

RUNNER = Path(__file__).resolve().with_name("process_group_runner.py")
DEFAULT_CLEANUP_GRACE_SECONDS = 1.0
POLL_SECONDS = 0.02


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


def terminate_supervised_process(
    process: subprocess.Popen[str],
    grace_seconds: float = DEFAULT_CLEANUP_GRACE_SECONDS,
) -> tuple[str, str]:
    """Reap a sentinel-pinned process group without ever signaling a reused group id."""

    if process.poll() is not None:
        # Once waitpid has reaped the sentinel, its numeric PID no longer pins the
        # process-group identity. Never inspect or signal that number as a PGID.
        return process.communicate(timeout=5)

    try:
        process_group = os.getpgid(process.pid)
    except ProcessLookupError:
        return process.communicate(timeout=5)
    if process_group != process.pid:
        process.kill()
        process.communicate(timeout=5)
        raise ManagedCommandError(
            f"supervisor {process.pid} is not its process-group leader ({process_group})"
        )

    # Freeze the live sentinel before signaling the group. Its unreaped PID pins the
    # group identity throughout both membership checks, so PID reuse cannot redirect
    # either signal to an unrelated process tree.
    os.kill(process.pid, signal.SIGSTOP)
    _signal_group(process_group, signal.SIGTERM)
    deadline = time.monotonic() + grace_seconds
    while time.monotonic() < deadline:
        members = process_group_members(process_group)
        if members <= {process.pid}:
            break
        time.sleep(POLL_SECONDS)

    members = process_group_members(process_group)
    if members:
        _signal_group(process_group, signal.SIGKILL)
    try:
        return process.communicate(timeout=5)
    except subprocess.TimeoutExpired as error:
        raise ManagedCommandError(
            f"could not reap supervised process group {process_group}"
        ) from error


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
        self._closed = False

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
            managed.supervisor = subprocess.Popen(
                [
                    sys.executable,
                    str(RUNNER),
                    "--ready",
                    str(managed.ready_path),
                    "--status",
                    str(managed.status_path),
                    "--stdout",
                    str(managed.stdout_path),
                    "--stderr",
                    str(managed.stderr_path),
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
        except BaseException:
            managed._temporary.cleanup()
            raise
        return managed

    @property
    def closed(self) -> bool:
        return self._closed

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

    def signal(self, signum: int) -> None:
        if self._closed or self.supervisor is None or self.supervisor.poll() is not None:
            return
        _signal_group(self.supervisor.pid, signum)

    def terminate(self) -> None:
        if self._closed:
            return
        try:
            if self.supervisor is not None:
                self._supervisor_output = terminate_supervised_process(
                    self.supervisor,
                    self.cleanup_grace_seconds,
                )
        finally:
            self._closed = True
            self._temporary.cleanup()

    def wait(
        self,
        timeout: float,
        *,
        poll_hook: Callable[[ManagedProcess], None] | None = None,
    ) -> CommandResult:
        if self._closed:
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
        finally:
            try:
                if self.supervisor is not None:
                    self._supervisor_output = terminate_supervised_process(
                        self.supervisor,
                        self.cleanup_grace_seconds,
                    )
            finally:
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
                self._closed = True
                self._temporary.cleanup()

        if payload is None:
            raise ManagedCommandError(f"{self.label} produced no status")
        if "error" in payload:
            supervisor_detail = (self._supervisor_output[1] or self._supervisor_output[0]).strip()
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
    return process.wait(timeout, poll_hook=poll_hook)
