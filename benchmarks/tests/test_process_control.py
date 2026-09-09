from __future__ import annotations

import os
import signal
import subprocess
import sys
import threading
import time
from pathlib import Path

import pytest

BENCHMARKS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(BENCHMARKS))

import process_control  # noqa: E402
from process_control import (  # noqa: E402
    ManagedCommandError,
    ManagedCommandTimeout,
    ManagedProcess,
    run_managed_command,
)

SLEEPER = """
import os
import signal
import sys
import time

signal.signal(signal.SIGTERM, signal.SIG_IGN)
with open(sys.argv[1], "w", encoding="utf-8") as output:
    output.write(str(os.getpid()))
    output.flush()
time.sleep(60)
"""

SPAWNER = """
import subprocess
import sys
import time
from pathlib import Path

pid_file = Path(sys.argv[2])
subprocess.Popen(
    [sys.executable, "-c", sys.argv[1], str(pid_file)],
    stdin=subprocess.DEVNULL,
    stdout=subprocess.DEVNULL,
    stderr=subprocess.DEVNULL,
    close_fds=True,
)
deadline = time.monotonic() + 5
while not pid_file.exists():
    if time.monotonic() >= deadline:
        raise SystemExit("descendant did not start")
    time.sleep(0.01)
"""


def wait_for_pid(path: Path) -> int:
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        if path.exists():
            return int(path.read_text())
        time.sleep(0.01)
    raise AssertionError(f"PID file was not written: {path}")


def assert_process_gone(pid: int) -> None:
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            return
        time.sleep(0.02)
    pytest.fail(f"process {pid} survived supervised cleanup")


def test_finished_supervisor_pid_is_never_treated_as_a_live_process_group(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    class FinishedSupervisor:
        pid = 12345

        @staticmethod
        def poll() -> int:
            return 0

        @staticmethod
        def communicate(timeout: float) -> tuple[str, str]:
            assert timeout == 5
            return "", ""

    def forbidden(*_args: object, **_kwargs: object) -> None:
        pytest.fail("a reaped supervisor PID must never be inspected or signaled as a PGID")

    monkeypatch.setattr(process_control, "process_group_members", forbidden)
    monkeypatch.setattr(process_control, "_signal_group", forbidden)

    assert process_control.terminate_supervised_process(FinishedSupervisor()) == ("", "")


def test_spawn_signal_window_registers_then_reaps_real_process_tree(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    pid_file = tmp_path / "spawn-window-child.pid"
    supervisor_pids: list[int] = []
    child_pids: list[int] = []
    original_spawn = process_control.spawn_supervisor
    previous_handler = signal.getsignal(signal.SIGTERM)

    def raise_termination(_signum: int, _frame: object) -> None:
        raise RequestedTermination

    def spawn_then_signal(
        command: list[str],
        **kwargs: object,
    ) -> subprocess.Popen[str]:
        process = original_spawn(command, **kwargs)
        supervisor_pids.append(process.pid)
        child_pids.append(wait_for_pid(pid_file))
        signal.pthread_kill(threading.get_ident(), signal.SIGTERM)
        return process

    signal.signal(signal.SIGTERM, raise_termination)
    monkeypatch.setattr(process_control, "spawn_supervisor", spawn_then_signal)
    try:
        with pytest.raises(RequestedTermination):
            ManagedProcess.start(
                [sys.executable, "-c", SLEEPER, str(pid_file)],
                cwd=tmp_path,
                label="spawn signal window",
                cleanup_grace_seconds=0.05,
            )
    finally:
        signal.signal(signal.SIGTERM, previous_handler)

    assert len(supervisor_pids) == 1
    assert len(child_pids) == 1
    assert_process_gone(supervisor_pids[0])
    assert_process_gone(child_pids[0])


def test_enumeration_failure_still_kills_and_reaps_pinned_group(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    pid_file = tmp_path / "enumeration-child.pid"
    managed = ManagedProcess.start(
        [sys.executable, "-c", SLEEPER, str(pid_file)],
        cwd=tmp_path,
        label="enumeration failure",
        cleanup_grace_seconds=0.05,
    )
    child_pid = wait_for_pid(pid_file)

    def fail_enumeration(_process_group: int) -> set[int]:
        raise ManagedCommandError("synthetic ps failure")

    monkeypatch.setattr(process_control, "process_group_members", fail_enumeration)
    managed.terminate()

    assert managed.closed
    assert_process_gone(child_pid)


def test_failed_reap_keeps_control_files_until_retry(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    managed = ManagedProcess(
        [sys.executable, "-c", "pass"],
        tmp_path,
        "retry cleanup",
        0.05,
    )
    class UnreapedSupervisor:
        @staticmethod
        def poll() -> None:
            return None

    managed.supervisor = UnreapedSupervisor()  # type: ignore[assignment]
    controls = managed.ready_path.parent
    attempts = 0

    def flaky_reap(
        _process: object,
        _grace_seconds: float,
    ) -> tuple[str, str]:
        nonlocal attempts
        attempts += 1
        if attempts == 1:
            raise ManagedCommandError("synthetic reap failure")
        return "", ""

    monkeypatch.setattr(process_control, "terminate_supervised_process", flaky_reap)
    with pytest.raises(ManagedCommandError, match="synthetic reap failure"):
        managed.terminate()

    assert not managed.closed
    assert controls.exists()

    managed.terminate()
    assert managed.closed
    assert not controls.exists()


def test_failed_group_kill_preserves_real_sentinel_and_child_for_retry(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    pid_file = tmp_path / "failed-kill-child.pid"
    managed = ManagedProcess.start(
        [sys.executable, "-c", SLEEPER, str(pid_file)],
        cwd=tmp_path,
        label="failed group kill",
        cleanup_grace_seconds=0.05,
    )
    child_pid = wait_for_pid(pid_file)
    controls = managed.ready_path.parent
    original_killpg = process_control.os.killpg
    failures_remaining = 1

    def fail_first_group_kill(process_group: int, signum: int) -> None:
        nonlocal failures_remaining
        if signum == signal.SIGKILL and failures_remaining:
            failures_remaining -= 1
            raise PermissionError("synthetic group kill failure")
        original_killpg(process_group, signum)

    monkeypatch.setattr(process_control.os, "killpg", fail_first_group_kill)
    try:
        with pytest.raises(ManagedCommandError, match="pinned-group SIGKILL"):
            managed.terminate()

        assert not managed.closed
        assert controls.exists()
        assert managed.supervisor is not None
        assert managed.supervisor.poll() is None
        os.kill(child_pid, 0)

        managed.terminate()
        assert managed.closed
        assert not controls.exists()
        assert_process_gone(child_pid)
    finally:
        monkeypatch.setattr(process_control.os, "killpg", original_killpg)
        if not managed.closed:
            managed.terminate()


def test_run_managed_command_retries_transient_group_kill_failure(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    pid_file = tmp_path / "wrapper-retry-child.pid"
    original_killpg = process_control.os.killpg
    failed = False
    kill_attempts = 0

    def fail_first_group_kill(process_group: int, signum: int) -> None:
        nonlocal failed, kill_attempts
        if signum == signal.SIGKILL:
            kill_attempts += 1
            if not failed:
                failed = True
                raise PermissionError("synthetic group kill failure")
        original_killpg(process_group, signum)

    monkeypatch.setattr(process_control.os, "killpg", fail_first_group_kill)
    with pytest.raises(BaseExceptionGroup, match="cleanup"):
        run_managed_command(
            [sys.executable, "-c", SLEEPER, str(pid_file)],
            cwd=tmp_path,
            timeout=0.2,
            label="wrapper cleanup retry",
            cleanup_grace_seconds=0.05,
        )

    assert kill_attempts == 2
    assert_process_gone(wait_for_pid(pid_file))


def test_termination_during_cleanup_is_reraised_after_real_group_reap(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    pid_file = tmp_path / "cleanup-interruption-child.pid"
    managed = ManagedProcess.start(
        [sys.executable, "-c", SLEEPER, str(pid_file)],
        cwd=tmp_path,
        label="cleanup interruption",
        cleanup_grace_seconds=0.05,
    )
    child_pid = wait_for_pid(pid_file)

    def interrupt_enumeration(_process_group: int) -> set[int]:
        raise RequestedTermination

    monkeypatch.setattr(process_control, "process_group_members", interrupt_enumeration)
    with pytest.raises(RequestedTermination):
        managed.terminate()

    assert managed.closed
    assert not managed.ready_path.parent.exists()
    assert_process_gone(child_pid)


def test_successful_command_reaps_residual_descendant(tmp_path: Path) -> None:
    pid_file = tmp_path / "success-child.pid"
    result = run_managed_command(
        [sys.executable, "-c", SPAWNER, SLEEPER, str(pid_file)],
        cwd=tmp_path,
        timeout=5,
        label="successful tree",
        cleanup_grace_seconds=0.05,
    )
    assert result.returncode == 0
    assert_process_gone(wait_for_pid(pid_file))


def test_timeout_reaps_signal_ignoring_process_tree(tmp_path: Path) -> None:
    pid_file = tmp_path / "timeout-child.pid"
    with pytest.raises(ManagedCommandTimeout, match="timed out"):
        run_managed_command(
            [sys.executable, "-c", SLEEPER, str(pid_file)],
            cwd=tmp_path,
            timeout=0.2,
            label="timed out tree",
            cleanup_grace_seconds=0.05,
        )
    assert_process_gone(wait_for_pid(pid_file))


class RequestedTermination(BaseException):
    pass


def test_sigterm_during_wait_reaps_process_tree(tmp_path: Path) -> None:
    pid_file = tmp_path / "signal-child.pid"
    previous = signal.getsignal(signal.SIGTERM)

    def raise_termination(_signum: int, _frame: object) -> None:
        raise RequestedTermination

    def send_when_started() -> None:
        wait_for_pid(pid_file)
        os.kill(os.getpid(), signal.SIGTERM)

    signal.signal(signal.SIGTERM, raise_termination)
    sender = threading.Thread(target=send_when_started)
    sender.start()
    try:
        with pytest.raises(RequestedTermination):
            run_managed_command(
                [sys.executable, "-c", SLEEPER, str(pid_file)],
                cwd=tmp_path,
                timeout=10,
                label="interrupted tree",
                cleanup_grace_seconds=0.05,
            )
    finally:
        sender.join(timeout=5)
        signal.signal(signal.SIGTERM, previous)
    assert_process_gone(wait_for_pid(pid_file))
