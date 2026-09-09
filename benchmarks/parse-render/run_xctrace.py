#!/usr/bin/env python3
"""Record system-wide allocations with a hard watchdog and deterministic cleanup."""

from __future__ import annotations

import contextlib
import os
import signal
import subprocess
import sys
from collections.abc import Iterator
from pathlib import Path
from typing import Any


class TerminationRequested(BaseException):
    def __init__(self, signum: int) -> None:
        super().__init__(signum)
        self.signum = signum


def _signal_process_group(process: subprocess.Popen[str], signum: int) -> None:
    try:
        os.killpg(process.pid, signum)
        return
    except ProcessLookupError:
        return
    except PermissionError:
        pass
    if process.poll() is None:
        try:
            process.send_signal(signum)
        except ProcessLookupError:
            pass


def terminate_process_group(
    process: subprocess.Popen[str],
    grace_seconds: float = 5.0,
) -> tuple[str, str]:
    _signal_process_group(process, signal.SIGTERM)
    try:
        return process.communicate(timeout=grace_seconds)
    except subprocess.TimeoutExpired:
        _signal_process_group(process, signal.SIGKILL)
        try:
            return process.communicate(timeout=grace_seconds)
        except subprocess.TimeoutExpired as error:
            raise RuntimeError(f"could not reap process group {process.pid}") from error


def trace_command(trace: Path, notification: str) -> list[str]:
    return [
        "xcrun",
        "xctrace",
        "record",
        "--template",
        "Allocations",
        "--time-limit",
        "60s",
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


def run(trace: Path, binary: Path, fixture: Path, result: Path) -> int:
    notification = f"dev.srui.benchmark.xctrace.{os.getpid()}"
    processes: list[tuple[subprocess.Popen[str], bool]] = []
    watcher = subprocess.Popen(
        ["notifyutil", "-1", notification],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        start_new_session=True,
    )
    processes.append((watcher, False))
    recorder = subprocess.Popen(
        trace_command(trace, notification),
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        start_new_session=True,
    )
    processes.append((recorder, False))

    try:
        try:
            _watch_stdout, watch_stderr = watcher.communicate(timeout=15)
            processes[0] = (watcher, True)
        except subprocess.TimeoutExpired:
            recorder_stdout, recorder_stderr = terminate_process_group(recorder)
            processes[1] = (recorder, True)
            detail = (recorder_stderr or recorder_stdout).strip()
            print(
                "xctrace did not begin recording within 15s; grant Instruments automation "
                f"and Developer Tools privacy access, then retry. {detail[-1000:]}",
                file=sys.stderr,
            )
            return 124
        if watcher.returncode:
            print(
                f"notifyutil failed ({watcher.returncode}): {watch_stderr[-1000:]}",
                file=sys.stderr,
            )
            return watcher.returncode

        driver = subprocess.Popen(
            driver_command(binary, fixture, result),
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            start_new_session=True,
        )
        processes.append((driver, False))
        try:
            driver_stdout, driver_stderr = driver.communicate(timeout=55)
            processes[2] = (driver, True)
        except subprocess.TimeoutExpired:
            terminate_process_group(driver)
            processes[2] = (driver, True)
            print("BenchmarkDriver timed out after 55s", file=sys.stderr)
            return 124
        if driver.returncode:
            terminate_process_group(recorder)
            processes[1] = (recorder, True)
            detail = (driver_stderr or driver_stdout).strip()
            print(
                f"BenchmarkDriver failed ({driver.returncode}): {detail[-2000:]}",
                file=sys.stderr,
            )
            return driver.returncode

        try:
            recorder_stdout, recorder_stderr = recorder.communicate(timeout=75)
            processes[1] = (recorder, True)
        except subprocess.TimeoutExpired:
            terminate_process_group(recorder)
            processes[1] = (recorder, True)
            print("xctrace did not finish within its 60s recording window", file=sys.stderr)
            return 124
        if recorder.returncode:
            detail = (recorder_stderr or recorder_stdout).strip()
            print(
                f"xctrace failed ({recorder.returncode}): {detail[-2000:]}",
                file=sys.stderr,
            )
        return recorder.returncode
    finally:
        for process, completed in reversed(processes):
            if not completed:
                try:
                    terminate_process_group(process)
                except Exception as error:
                    print(
                        f"failed to reap process group {process.pid}: {error}",
                        file=sys.stderr,
                    )


def main(argv: list[str] | None = None) -> int:
    arguments = sys.argv[1:] if argv is None else argv
    if len(arguments) != 4:
        print("usage: run_xctrace.py TRACE BINARY FIXTURE RESULT", file=sys.stderr)
        return 2
    trace, binary, fixture, result = map(Path, arguments)
    if trace.exists():
        print(f"trace output already exists: {trace}", file=sys.stderr)
        return 2
    if not binary.is_file():
        print(f"BenchmarkDriver does not exist: {binary}", file=sys.stderr)
        return 2
    if not fixture.is_file():
        print(f"fixture does not exist: {fixture}", file=sys.stderr)
        return 2
    return run(trace, binary, fixture, result)


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


if __name__ == "__main__":
    raise SystemExit(cli())
