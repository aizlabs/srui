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

OWNER_CONTROLLER = """
import sys
import time
from pathlib import Path

sys.path.insert(0, sys.argv[1])
import process_control

root = Path(sys.argv[4])
descendant_path = Path(sys.argv[5])
state_path = Path(sys.argv[6])
exact_owner = sys.argv[7] == "exact"
supervisor = process_control.spawn_supervisor(
    [sys.executable, "-c", sys.argv[2], sys.argv[3], str(descendant_path)],
    cwd=root,
    ready_path=root / "owner-ready.json",
    status_path=root / "owner-status.json",
    stdout_path=root / "owner-command.stdout",
    stderr_path=root / "owner-command.stderr",
    require_exact_owner_identity=exact_owner,
)
deadline = time.monotonic() + 5
while not descendant_path.exists():
    if time.monotonic() >= deadline:
        raise SystemExit("owner controller descendant did not start")
    time.sleep(0.01)
state_path.write_text(
    f"{supervisor.pid},{descendant_path.read_text(encoding='utf-8')}",
    encoding="utf-8",
)
time.sleep(60)
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


def wait_for_two_pids(path: Path) -> tuple[int, int]:
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        if path.exists():
            parts = path.read_text(encoding="utf-8").split(",")
            if len(parts) == 2:
                return int(parts[0]), int(parts[1])
        time.sleep(0.01)
    raise AssertionError(f"two-PID state file was not written: {path}")


def test_process_identity_wait_accepts_numeric_pid_reuse() -> None:
    process_control.wait_for_process_identities_gone(
        [(123, 456)],
        label="reused candidate",
        timeout=0,
        identity_reader=lambda pid: 789 if pid == 123 else None,
    )


def test_process_identity_wait_rejects_same_identity_persistence() -> None:
    with pytest.raises(ManagedCommandError, match="123@456"):
        process_control.wait_for_process_identities_gone(
            [(123, 456)],
            label="persistent candidate",
            timeout=0,
            identity_reader=lambda pid: 456 if pid == 123 else None,
        )


def test_identity_wait_defers_real_sigterm_until_real_process_exits() -> None:
    child = subprocess.Popen(
        [sys.executable, "-c", "import time; time.sleep(0.15)"],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        close_fds=True,
    )
    first_lookup = threading.Event()
    caller_thread = threading.get_ident()
    previous_handler = signal.getsignal(signal.SIGTERM)

    def identity_reader(pid: int) -> int | None:
        assert pid == child.pid
        first_lookup.set()
        return 456 if child.poll() is None else None

    def send_during_wait() -> None:
        assert first_lookup.wait(timeout=2)
        signal.pthread_kill(caller_thread, signal.SIGTERM)

    def raise_termination(_signum: int, _frame: object) -> None:
        raise RequestedTermination

    sender = threading.Thread(target=send_during_wait)
    signal.signal(signal.SIGTERM, raise_termination)
    sender.start()
    try:
        with pytest.raises(RequestedTermination):
            process_control.wait_for_process_identities_gone(
                [(child.pid, 456)],
                label="real exiting candidate",
                timeout=2,
                identity_reader=identity_reader,
            )
    finally:
        sender.join(timeout=2)
        signal.signal(signal.SIGTERM, previous_handler)
        if child.poll() is None:
            child.kill()
            child.wait(timeout=2)
    assert child.returncode == 0


def test_identity_wait_preserves_timeout_and_deferred_termination() -> None:
    child = subprocess.Popen(
        [sys.executable, "-c", "import time; time.sleep(60)"],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        close_fds=True,
    )
    first_lookup = threading.Event()
    caller_thread = threading.get_ident()
    previous_handler = signal.getsignal(signal.SIGTERM)

    def identity_reader(pid: int) -> int | None:
        assert pid == child.pid
        first_lookup.set()
        return 789

    def send_during_wait() -> None:
        assert first_lookup.wait(timeout=2)
        signal.pthread_kill(caller_thread, signal.SIGTERM)

    def raise_termination(_signum: int, _frame: object) -> None:
        raise RequestedTermination

    sender = threading.Thread(target=send_during_wait)
    signal.signal(signal.SIGTERM, raise_termination)
    sender.start()
    try:
        with pytest.raises(BaseExceptionGroup) as raised:
            process_control.wait_for_process_identities_gone(
                [(child.pid, 789)],
                label="real persistent candidate",
                timeout=0.1,
                identity_reader=identity_reader,
            )
    finally:
        sender.join(timeout=2)
        signal.signal(signal.SIGTERM, previous_handler)
        child.kill()
        child.wait(timeout=2)

    nested = raised.value.exceptions
    assert any(isinstance(error, ManagedCommandError) for error in nested)
    assert any(isinstance(error, RequestedTermination) for error in nested)


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
    original_killpg = process_control.os.killpg
    previous_handler = signal.getsignal(signal.SIGTERM)
    kill_attempts = 0

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

    def fail_first_group_kill(process_group: int, signum: int) -> None:
        nonlocal kill_attempts
        if signum == signal.SIGKILL:
            kill_attempts += 1
            if kill_attempts == 1:
                raise PermissionError("synthetic spawn cleanup group kill failure")
        original_killpg(process_group, signum)

    signal.signal(signal.SIGTERM, raise_termination)
    monkeypatch.setattr(process_control, "spawn_supervisor", spawn_then_signal)
    monkeypatch.setattr(process_control.os, "killpg", fail_first_group_kill)
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
    assert kill_attempts == 2
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
    with pytest.raises(ManagedCommandTimeout, match="timed out"):
        run_managed_command(
            [sys.executable, "-c", SLEEPER, str(pid_file)],
            cwd=tmp_path,
            timeout=0.2,
            label="wrapper cleanup retry",
            cleanup_grace_seconds=0.05,
        )

    assert kill_attempts == 2
    assert_process_gone(wait_for_pid(pid_file))


def test_shared_cleanup_failure_retains_recovery_identity(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    pid_file = tmp_path / "retained-cleanup-child.pid"
    managed = ManagedProcess.start(
        [sys.executable, "-c", SLEEPER, str(pid_file)],
        cwd=tmp_path,
        label="retained cleanup",
        cleanup_grace_seconds=0.02,
    )
    child_pid = wait_for_pid(pid_file)
    controls = managed.ready_path.parent
    supervisor_pid = managed.supervisor.pid if managed.supervisor is not None else -1
    original_killpg = process_control.os.killpg

    def reject_group_kill(_process_group: int, signum: int) -> None:
        if signum == signal.SIGKILL:
            raise PermissionError("synthetic persistent group kill failure")
        original_killpg(supervisor_pid, signum)

    monkeypatch.setattr(process_control.os, "killpg", reject_group_kill)
    try:
        with pytest.raises(BaseExceptionGroup) as raised:
            process_control.terminate_managed_process_with_retry(managed)
        failures = [
            nested
            for nested in raised.value.exceptions
            if isinstance(nested, ManagedCommandError)
            and "retained supervisor/PGID" in str(nested)
        ]
        assert len(failures) == 1
        recovery = failures[0]
        assert str(supervisor_pid) in str(recovery)
        assert str(controls) in str(recovery)
        assert getattr(recovery, "process_handle") is managed
        assert controls.exists()
        assert not managed.closed
    finally:
        monkeypatch.setattr(process_control.os, "killpg", original_killpg)
        process_control.terminate_managed_process_with_retry(managed)

    assert_process_gone(child_pid)


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

def test_runner_refuses_stale_explicit_owner_before_spawning_child(
    tmp_path: Path,
) -> None:
    child_pid_path = tmp_path / "startup-race-child.pid"
    result = subprocess.run(
        [
            sys.executable,
            str(process_control.RUNNER),
            "--owner-pid",
            str(os.getpid() + 1_000_000),
            "--ready",
            str(tmp_path / "startup-ready.json"),
            "--status",
            str(tmp_path / "startup-status.json"),
            "--stdout",
            str(tmp_path / "startup.stdout"),
            "--stderr",
            str(tmp_path / "startup.stderr"),
            "--",
            sys.executable,
            "-c",
            SLEEPER,
            str(child_pid_path),
        ],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        start_new_session=True,
        close_fds=True,
        timeout=5,
        check=False,
    )
    assert result.returncode == 125
    assert "owner disappeared or changed identity" in result.stderr
    assert not child_pid_path.exists()


@pytest.mark.skipif(sys.platform != "darwin", reason="Darwin birth identity")
def test_runner_refuses_live_owner_with_wrong_birth_before_spawning_child(
    tmp_path: Path,
) -> None:
    child_pid_path = tmp_path / "wrong-birth-child.pid"
    owner_birth = process_control.process_birth_unix_ns(os.getpid())
    assert owner_birth is not None
    result = subprocess.run(
        [
            sys.executable,
            str(process_control.RUNNER),
            "--owner-pid",
            str(os.getpid()),
            "--owner-birth-unix-ns",
            str(owner_birth + 1),
            "--ready",
            str(tmp_path / "wrong-birth-ready.json"),
            "--status",
            str(tmp_path / "wrong-birth-status.json"),
            "--stdout",
            str(tmp_path / "wrong-birth.stdout"),
            "--stderr",
            str(tmp_path / "wrong-birth.stderr"),
            "--",
            sys.executable,
            "-c",
            SLEEPER,
            str(child_pid_path),
        ],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        start_new_session=True,
        close_fds=True,
        timeout=5,
        check=False,
    )
    assert result.returncode == 125
    assert "owner disappeared or changed identity" in result.stderr
    assert not child_pid_path.exists()


@pytest.mark.parametrize(
    "exact_owner",
    [
        False,
        pytest.param(
            True,
            marks=pytest.mark.skipif(
                sys.platform != "darwin",
                reason="Darwin birth identity",
            ),
        ),
    ],
)
def test_hard_owner_death_reaps_established_sentinel_and_descendant(
    exact_owner: bool,
    tmp_path: Path,
) -> None:
    state_path = tmp_path / "owner-state.txt"
    descendant_path = tmp_path / "owner-descendant.pid"
    controller = subprocess.Popen(
        [
            sys.executable,
            "-c",
            OWNER_CONTROLLER,
            str(BENCHMARKS),
            SPAWNER,
            SLEEPER,
            str(tmp_path),
            str(descendant_path),
            str(state_path),
            "exact" if exact_owner else "pid-only",
        ],
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        close_fds=True,
    )
    sentinel_pid: int | None = None
    descendant_pid: int | None = None
    try:
        sentinel_pid, descendant_pid = wait_for_two_pids(state_path)
        os.kill(controller.pid, signal.SIGKILL)
        controller.wait(timeout=5)
        assert_process_gone(sentinel_pid)
        assert_process_gone(descendant_pid)
    finally:
        if controller.poll() is None:
            controller.kill()
            controller.wait(timeout=5)
        for pid in (sentinel_pid, descendant_pid):
            if pid is not None:
                try:
                    os.kill(pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass


@pytest.mark.parametrize("failed_publication", ["ready", "status"])
def test_control_publication_failure_keeps_group_pinned_until_tree_is_gone(
    failed_publication: str,
    tmp_path: Path,
) -> None:
    pid_file = tmp_path / f"{failed_publication}-publication-child.pid"
    ready_path = tmp_path / "ready.json"
    status_path = tmp_path / "status.json"
    failed_path = ready_path if failed_publication == "ready" else status_path
    failed_path.mkdir()
    command = (
        [sys.executable, "-c", SLEEPER, str(pid_file)]
        if failed_publication == "ready"
        else [sys.executable, "-c", SPAWNER, SLEEPER, str(pid_file)]
    )
    supervisor = process_control.spawn_supervisor(
        command,
        cwd=tmp_path,
        ready_path=ready_path,
        status_path=status_path,
        stdout_path=tmp_path / "command.stdout",
        stderr_path=tmp_path / "command.stderr",
    )
    child_pid = wait_for_pid(pid_file)
    time.sleep(0.1)
    assert supervisor.poll() is None

    _stdout, supervisor_stderr = process_control.terminate_supervised_process(
        supervisor,
        grace_seconds=0.05,
    )

    assert_process_gone(child_pid)
    assert_process_gone(supervisor.pid)
    if failed_publication == "status":
        assert "retaining group pin" in supervisor_stderr


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
