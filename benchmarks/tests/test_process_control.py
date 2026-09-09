from __future__ import annotations

import os
import signal
import sys
import threading
import time
from pathlib import Path

import pytest

BENCHMARKS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(BENCHMARKS))

import process_control  # noqa: E402
from process_control import ManagedCommandTimeout, run_managed_command  # noqa: E402

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
