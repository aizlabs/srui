from __future__ import annotations

import importlib.util
import os
import subprocess
import sys
from pathlib import Path
from types import SimpleNamespace
from typing import Any

import pytest

BENCHMARKS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(BENCHMARKS))
MODULE_PATH = BENCHMARKS / "parse-render/run_xctrace.py"
SPEC = importlib.util.spec_from_file_location("benchmark_xctrace_tests", MODULE_PATH)
assert SPEC and SPEC.loader
benchmark_xctrace = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(benchmark_xctrace)


def assert_process_gone(pid: int) -> None:
    with pytest.raises(ProcessLookupError):
        os.kill(pid, 0)


def test_trace_command_is_bounded_and_system_wide() -> None:
    command = benchmark_xctrace.trace_command(Path("out.trace"), "notification")
    assert "--all-processes" in command
    assert "--launch" not in command
    assert command[command.index("--time-limit") + 1] == "90s"
    assert command[command.index("--window") + 1] == "30s"
    assert "--no-prompt" in command
    assert "--notify-tracing-started" in command


def test_preflight_reserves_trace_export_and_free_space(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    monkeypatch.setattr(
        benchmark_xctrace.shutil,
        "disk_usage",
        lambda _path: SimpleNamespace(free=29),
    )
    with pytest.raises(benchmark_xctrace.CaptureError, match="requires 30 free bytes"):
        benchmark_xctrace.preflight_capture(
            tmp_path / "capture.trace",
            max_trace_bytes=10,
            max_export_bytes=5,
            min_remaining_bytes=15,
        )


def test_live_budget_rejects_oversize_trace(tmp_path: Path) -> None:
    trace = tmp_path / "capture.trace"
    trace.mkdir()
    (trace / "data").write_bytes(b"x" * 17)

    with pytest.raises(benchmark_xctrace.CaptureError, match="exceeded 16 bytes"):
        benchmark_xctrace.ensure_capture_budget(
            trace,
            max_trace_bytes=16,
            min_remaining_bytes=1,
        )


def test_allocation_summary_uses_exact_process_ids(tmp_path: Path) -> None:
    allocation_xml = tmp_path / "allocations.xml"
    allocation_xml.write_text(
        """\
<trace-query-result>
  <node>
    <schema name="allocations">
      <col><mnemonic>process</mnemonic><engineering-type>process</engineering-type></col>
      <col><mnemonic>size</mnemonic><engineering-type>size</engineering-type></col>
    </schema>
    <row>
      <process id="driver" fmt="BenchmarkDriver (123)" />
      <size>1 KiB</size>
    </row>
    <row>
      <process ref="driver" />
      <size fmt="2 KiB" />
    </row>
    <row>
      <process name="com.apple.WebKit.WebContent" pid="456" />
      <size>512</size>
    </row>
    <row>
      <process name="BenchmarkDriver" />
      <size>999</size>
    </row>
  </node>
</trace-query-result>
""",
        encoding="utf-8",
    )

    summary = benchmark_xctrace.parse_allocation_totals(
        allocation_xml,
        target_pid=123,
    )

    assert summary["target_processes"] == [
        {
            "name": "BenchmarkDriver",
            "pid": 123,
            "cumulative_allocations": 2,
            "cumulative_bytes": 3 * 1024,
        }
    ]
    assert summary["webkit_helper_processes"] == [
        {
            "name": "com.apple.WebKit.WebContent",
            "pid": 456,
            "cumulative_allocations": 1,
            "cumulative_bytes": 512,
        }
    ]


def test_second_spawn_failure_reaps_watcher_and_removes_partial_trace(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    trace = tmp_path / "capture.trace"
    watcher_processes: list[subprocess.Popen[str]] = []
    real_popen = subprocess.Popen

    def launch_watcher(_command: list[str], **kwargs: Any) -> subprocess.Popen[str]:
        process = real_popen(
            [sys.executable, "-c", "import time; time.sleep(60)"],
            **kwargs,
        )
        watcher_processes.append(process)
        return process

    def fail_recorder_start(
        _cls: type[Any],
        _command: list[str],
        **_kwargs: Any,
    ) -> Any:
        trace.mkdir()
        (trace / "incomplete").write_bytes(b"partial")
        raise OSError("second spawn failed")

    monkeypatch.setattr(benchmark_xctrace.subprocess, "Popen", launch_watcher)
    monkeypatch.setattr(
        benchmark_xctrace.ManagedProcess,
        "start",
        classmethod(fail_recorder_start),
    )

    with pytest.raises(OSError, match="second spawn failed"):
        benchmark_xctrace.run(
            trace,
            tmp_path / "BenchmarkDriver",
            tmp_path / "fixture.json",
            tmp_path / "result.json",
            max_trace_bytes=1024 * 1024,
            max_export_bytes=1024,
            min_remaining_bytes=1,
        )

    assert len(watcher_processes) == 1
    assert watcher_processes[0].poll() is not None
    assert_process_gone(watcher_processes[0].pid)
    assert not trace.exists()
    assert not (tmp_path / "capture.trace.summary.json").exists()
