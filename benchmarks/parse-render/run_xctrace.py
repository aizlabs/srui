#!/usr/bin/env python3
"""Run xctrace with a hard watchdog; Instruments can block on host privacy authorization."""

from __future__ import annotations

import os
import signal
import subprocess
import sys
from pathlib import Path


def main() -> int:
    if len(sys.argv) < 5:
        print("usage: run_xctrace.py TRACE BINARY FIXTURE RESULT", file=sys.stderr)
        return 2
    trace, binary, fixture, result = map(Path, sys.argv[1:5])
    command = [
        "xcrun",
        "xctrace",
        "record",
        "--template",
        "Allocations",
        "--time-limit",
        "30s",
        "--no-prompt",
        "--output",
        str(trace),
        "--launch",
        "--",
        str(binary),
        "--fixture",
        str(fixture),
        "--profile",
        "full",
        "--output",
        str(result),
    ]
    process = subprocess.Popen(command, start_new_session=True)
    try:
        return process.wait(timeout=60)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGTERM)
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
        print(
            "xctrace did not start/finish within 60s; grant Instruments automation and "
            "Developer Tools privacy access for this terminal, then retry",
            file=sys.stderr,
        )
        return 124


if __name__ == "__main__":
    raise SystemExit(main())
