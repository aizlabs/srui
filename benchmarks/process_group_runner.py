#!/usr/bin/env python3
"""Keep a process-group sentinel alive until the benchmark harness reaps the tree."""

from __future__ import annotations

import json
import os
import signal
import subprocess
import sys
from pathlib import Path
from typing import Any


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
    try:
        separator = arguments.index("--")
        options = arguments[:separator]
        command = arguments[separator + 1 :]
        ready = Path(options[options.index("--ready") + 1])
        status = Path(options[options.index("--status") + 1])
        stdout_path = Path(options[options.index("--stdout") + 1])
        stderr_path = Path(options[options.index("--stderr") + 1])
    except (ValueError, IndexError):
        print(
            "usage: process_group_runner.py --ready PATH --status PATH "
            "--stdout PATH --stderr PATH -- COMMAND...",
            file=sys.stderr,
        )
        return 2
    if not command:
        print("supervised command is empty", file=sys.stderr)
        return 2

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
            payload = {"returncode": child.wait(), "child_pid": child.pid}
    except BaseException as error:
        payload = {
            "error": f"{type(error).__name__}: {error}",
            "child_pid": locals().get("child").pid if "child" in locals() else None,
        }

    atomic_write(status, payload)
    while True:
        signal.pause()


def main() -> int:
    arguments = sys.argv[1:]
    if arguments and arguments[0] == "--exec":
        return exec_command(arguments[1:])
    return supervise(arguments)


if __name__ == "__main__":
    raise SystemExit(main())
