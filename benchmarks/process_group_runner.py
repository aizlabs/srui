#!/usr/bin/env python3
"""Keep a process-group sentinel alive until the benchmark harness reaps the tree."""

from __future__ import annotations

import json
import os
import signal
import subprocess
import sys
import time
from pathlib import Path
from typing import Any

from process_control import ManagedCommandError, process_birth_unix_ns


def atomic_write(path: Path, payload: dict[str, Any]) -> None:
    temporary = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    temporary.write_text(json.dumps(payload), encoding="utf-8")
    os.replace(temporary, path)


def exec_command(command: list[str]) -> int:
    signal.signal(signal.SIGTERM, signal.SIG_DFL)
    signal.signal(signal.SIGINT, signal.SIG_DFL)
    try:
        os.execvp(command[0], command)
    except OSError as error:
        print(f"cannot exec {command[0]}: {error}", file=sys.stderr)
        return 127
    return 127


def supervise(arguments: list[str]) -> int:
    owner_pid: int
    owner_birth_unix_ns: int | None = None

    def owner_identity_matches() -> bool:
        if os.getppid() != owner_pid:
            return False
        if owner_birth_unix_ns is None:
            return True
        try:
            return process_birth_unix_ns(owner_pid) == owner_birth_unix_ns
        except ManagedCommandError:
            return False

    def terminate_if_owner_gone() -> None:
        if owner_identity_matches():
            return
        # This sentinel is the still-live process-group leader. Killing its
        # exact group handles children that have not established their own
        # watchdog group; detached candidates observe this sentinel disappear
        # and terminate their separately owned group.
        try:
            os.killpg(os.getpgrp(), signal.SIGKILL)
        finally:
            os._exit(128 + signal.SIGKILL)

    try:
        separator = arguments.index("--")
        options = arguments[:separator]
        command = arguments[separator + 1 :]
        for required in (
            "--owner-pid",
            "--ready",
            "--status",
            "--stdout",
            "--stderr",
        ):
            if options.count(required) != 1:
                raise ValueError
        if options.count("--owner-birth-unix-ns") not in {0, 1}:
            raise ValueError
        owner_pid = int(options[options.index("--owner-pid") + 1])
        if "--owner-birth-unix-ns" in options:
            owner_birth_unix_ns = int(
                options[options.index("--owner-birth-unix-ns") + 1]
            )
        ready = Path(options[options.index("--ready") + 1])
        status = Path(options[options.index("--status") + 1])
        stdout_path = Path(options[options.index("--stdout") + 1])
        stderr_path = Path(options[options.index("--stderr") + 1])
    except (ValueError, IndexError):
        print(
            "usage: process_group_runner.py --owner-pid PID "
            "[--owner-birth-unix-ns NS] --ready PATH "
            "--status PATH --stdout PATH --stderr PATH -- COMMAND...",
            file=sys.stderr,
        )
        return 2
    if (
        owner_pid <= 1
        or (owner_birth_unix_ns is not None and owner_birth_unix_ns <= 0)
        or not owner_identity_matches()
    ):
        print(
            "supervisor owner disappeared or changed identity before "
            "sentinel initialization",
            file=sys.stderr,
            flush=True,
        )
        return 125

    # The sentinel pins the process-group identity. The actual command is an exec-mode
    # child that restores normal signal handling before exec.
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
    signal.signal(signal.SIGINT, signal.SIG_IGN)
    payload: dict[str, Any]
    try:
        with stdout_path.open("wb") as stdout, stderr_path.open("wb") as stderr:
            child = subprocess.Popen(
                [sys.executable, str(Path(__file__).resolve()), "--exec", *command],
                stdin=subprocess.DEVNULL,
                stdout=stdout,
                stderr=stderr,
                close_fds=True,
            )
            atomic_write(ready, {"child_pid": child.pid})
            while child.poll() is None:
                terminate_if_owner_gone()
                time.sleep(0.05)
            payload = {"returncode": child.returncode, "child_pid": child.pid}
    except BaseException as error:
        payload = {
            "error": f"{type(error).__name__}: {error}",
            "child_pid": locals().get("child").pid if "child" in locals() else None,
        }

    try:
        atomic_write(status, payload)
    except BaseException as error:
        # Never drop the process-group pin merely because the control volume is
        # full or unavailable. The parent will time out and authoritatively
        # kill/reap this still-live sentinel and every process in its group.
        print(
            f"cannot publish supervisor status; retaining group pin: "
            f"{type(error).__name__}: {error}",
            file=sys.stderr,
            flush=True,
        )
    while True:
        terminate_if_owner_gone()
        time.sleep(0.05)


def main() -> int:
    # ManagedProcess blocks handled termination signals across Popen and handle
    # registration. The child does not inherit that parent-only critical section.
    signal.pthread_sigmask(
        signal.SIG_UNBLOCK,
        {signal.SIGINT, signal.SIGTERM},
    )
    arguments = sys.argv[1:]
    if arguments and arguments[0] == "--exec":
        return exec_command(arguments[1:])
    return supervise(arguments)


if __name__ == "__main__":
    raise SystemExit(main())
