from __future__ import annotations

import importlib.util
import json
import os
import signal
import subprocess
import sys
import threading
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

TRACE_STARTED_UNIX_NS = 1_700_000_000_000_000_000


def assert_process_gone(pid: int) -> None:
    with pytest.raises(ProcessLookupError):
        os.kill(pid, 0)


def attribution(driver_pid: int = 42) -> list[dict[str, Any]]:
    return [
        {
            "candidate": "srui",
            "driver_pid": driver_pid,
            "host_pid": 123,
            "helper_pids": [],
            "started_unix_ns": TRACE_STARTED_UNIX_NS + 1_000,
            "ended_unix_ns": TRACE_STARTED_UNIX_NS + 2_000,
            "helper_pid_source": "no helper processes",
        },
        {
            "candidate": "webkit",
            "driver_pid": driver_pid,
            "host_pid": 456,
            "helper_pids": [789],
            "started_unix_ns": TRACE_STARTED_UNIX_NS + 3_000,
            "ended_unix_ns": TRACE_STARTED_UNIX_NS + 4_000,
            "helper_pid_source": "WKWebView diagnostic process identifiers",
        },
    ]


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


def test_renderer_attribution_is_tied_to_launched_driver(tmp_path: Path) -> None:
    result = tmp_path / "result.json"
    result.write_text(
        json.dumps(
            {"artifacts": {"renderer_process_attribution": attribution(driver_pid=42)}}
        ),
        encoding="utf-8",
    )

    assert benchmark_xctrace.load_renderer_process_attribution(
        result,
        driver_pid=42,
    ) == attribution(driver_pid=42)

    with pytest.raises(benchmark_xctrace.CaptureError, match="does not match"):
        benchmark_xctrace.load_renderer_process_attribution(
            result,
            driver_pid=43,
        )


def test_trace_start_date_converts_to_exact_unix_nanoseconds() -> None:
    toc = """\
<trace-toc>
  <run><info><summary>
    <start-date>1970-01-01T02:00:01.123456789+02:00</start-date>
  </summary></info></run>
</trace-toc>
"""
    assert benchmark_xctrace.trace_start_unix_ns(toc) == 1_123_456_789


def test_allocation_summary_uses_only_explicit_pids_inside_candidate_intervals(
    tmp_path: Path,
) -> None:
    allocation_xml = tmp_path / "allocations.xml"
    allocation_xml.write_text(
        """\
<trace-query-result>
  <node>
    <schema name="allocations">
      <col><mnemonic>time</mnemonic><engineering-type>start-time</engineering-type></col>
      <col><mnemonic>process</mnemonic><engineering-type>process</engineering-type></col>
      <col><mnemonic>size</mnemonic><engineering-type>size</engineering-type></col>
    </schema>
    <row>
      <start-time>500</start-time>
      <process fmt="old owner of reused PID (123)" />
      <size>4 KiB</size>
    </row>
    <row>
      <start-time>1000</start-time>
      <process id="srui" fmt="SRUI candidate (123)" />
      <size>1 KiB</size>
    </row>
    <row>
      <start-time>1500</start-time>
      <process ref="srui" />
      <size fmt="2 KiB" />
    </row>
    <row>
      <start-time>2500</start-time>
      <process fmt="new owner of reused PID (123)" />
      <size>8 KiB</size>
    </row>
    <row>
      <start-time>3000</start-time>
      <process name="WebKit candidate host" pid="456" />
      <size>512</size>
    </row>
    <row>
      <start-time>3500</start-time>
      <process name="com.apple.WebKit.WebContent" pid="789" />
      <size>256</size>
    </row>
    <row>
      <start-time>3500</start-time>
      <process name="com.apple.WebKit.WebContent" pid="999" />
      <size>128</size>
    </row>
  </node>
</trace-query-result>
""",
        encoding="utf-8",
    )

    summary = benchmark_xctrace.parse_allocation_totals(
        allocation_xml,
        candidate_attribution=attribution(),
        trace_started_unix_ns=TRACE_STARTED_UNIX_NS,
    )

    candidates = {
        candidate["candidate"]: candidate
        for candidate in summary["candidate_processes"]
    }
    assert candidates["srui"]["host_process_totals"] == {
        "pid": 123,
        "names": ["SRUI candidate"],
        "cumulative_allocations": 2,
        "cumulative_bytes": 3 * 1024,
    }
    assert candidates["webkit"]["host_process_totals"]["pid"] == 456
    assert [
        process["pid"]
        for process in candidates["webkit"]["helper_process_totals"]
    ] == [789]
    assert candidates["webkit"]["helpers_without_allocation_rows"] == []
    assert summary["excluded_unrelated_process_rows"] == 1
    assert summary["excluded_outside_candidate_interval_rows"] == 2
    assert summary["trace_started_unix_ns"] == TRACE_STARTED_UNIX_NS


def test_allocation_summary_fails_closed_without_timestamp_column(
    tmp_path: Path,
) -> None:
    allocation_xml = tmp_path / "allocations.xml"
    allocation_xml.write_text(
        """\
<trace-query-result><node>
  <schema name="allocations">
    <col><mnemonic>process</mnemonic><engineering-type>process</engineering-type></col>
    <col><mnemonic>size</mnemonic><engineering-type>size</engineering-type></col>
  </schema>
  <row><process fmt="SRUI (123)" /><size>1</size></row>
</node></trace-query-result>
""",
        encoding="utf-8",
    )
    with pytest.raises(benchmark_xctrace.CaptureError, match="no nanosecond timestamp"):
        benchmark_xctrace.parse_allocation_totals(
            allocation_xml,
            candidate_attribution=attribution(),
            trace_started_unix_ns=TRACE_STARTED_UNIX_NS,
        )


def test_allocation_summary_fails_closed_on_unparseable_row_timestamp(
    tmp_path: Path,
) -> None:
    allocation_xml = tmp_path / "allocations.xml"
    allocation_xml.write_text(
        """\
<trace-query-result><node>
  <schema name="allocations">
    <col><mnemonic>time</mnemonic><engineering-type>start-time</engineering-type></col>
    <col><mnemonic>process</mnemonic><engineering-type>process</engineering-type></col>
    <col><mnemonic>size</mnemonic><engineering-type>size</engineering-type></col>
  </schema>
  <row><start-time fmt="not raw" /><process fmt="SRUI (123)" /><size>1</size></row>
</node></trace-query-result>
""",
        encoding="utf-8",
    )
    with pytest.raises(benchmark_xctrace.CaptureError, match="unparseable"):
        benchmark_xctrace.parse_allocation_totals(
            allocation_xml,
            candidate_attribution=attribution(),
            trace_started_unix_ns=TRACE_STARTED_UNIX_NS,
        )


def test_second_spawn_failure_reaps_watcher_and_removes_partial_trace(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    trace = tmp_path / "capture.trace"
    watcher_processes: list[subprocess.Popen[str]] = []

    def launch_watcher(_notification: str) -> subprocess.Popen[str]:
        process = subprocess.Popen(
            [sys.executable, "-c", "import time; time.sleep(60)"],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            start_new_session=True,
            close_fds=True,
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

    monkeypatch.setattr(
        benchmark_xctrace,
        "spawn_notification_watcher",
        launch_watcher,
    )
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


def test_watcher_spawn_signal_window_registers_then_reaps_real_child(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    trace = tmp_path / "capture.trace"
    watchers: list[subprocess.Popen[str]] = []

    def launch_then_signal(_notification: str) -> subprocess.Popen[str]:
        watcher = subprocess.Popen(
            [sys.executable, "-c", "import time; time.sleep(60)"],
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            start_new_session=True,
            close_fds=True,
        )
        watchers.append(watcher)
        signal.pthread_kill(threading.get_ident(), signal.SIGTERM)
        return watcher

    monkeypatch.setattr(
        benchmark_xctrace,
        "spawn_notification_watcher",
        launch_then_signal,
    )

    with pytest.raises(benchmark_xctrace.TerminationRequested):
        with benchmark_xctrace.termination_handlers():
            benchmark_xctrace.run(
                trace,
                tmp_path / "BenchmarkDriver",
                tmp_path / "fixture.json",
                tmp_path / "result.json",
                max_trace_bytes=1024 * 1024,
                max_export_bytes=1024,
                min_remaining_bytes=1,
            )

    assert len(watchers) == 1
    assert watchers[0].poll() is not None
    assert_process_gone(watchers[0].pid)
    assert not trace.exists()


def test_capture_cleanup_attempts_every_resource_and_removes_failed_capture(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    attempted: list[str] = []

    class FailingManaged:
        closed = False

        def __init__(self, name: str) -> None:
            self.name = name

        def terminate(self) -> None:
            attempted.append(self.name)
            raise RuntimeError(f"{self.name} failed")

    trace = tmp_path / "capture.trace"
    trace.mkdir()
    (trace / "partial").write_bytes(b"x")
    sidecar = tmp_path / "capture.trace.summary.json"
    sidecar.write_text("partial", encoding="utf-8")

    def fail_watcher(_watcher: object) -> tuple[str, str]:
        attempted.append("watcher")
        raise RuntimeError("watcher failed")

    monkeypatch.setattr(benchmark_xctrace, "terminate_direct_process", fail_watcher)
    errors = benchmark_xctrace.cleanup_capture(
        driver=FailingManaged("driver"),
        recorder=FailingManaged("recorder"),
        watcher=object(),
        trace=trace,
        sidecar=sidecar,
        retain_outputs=True,
    )

    assert attempted == ["driver", "recorder", "watcher"]
    assert len(errors) == 3
    assert not trace.exists()
    assert not sidecar.exists()


def test_capture_cleanup_preserves_termination_class_after_all_attempts(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    class CleanupTermination(BaseException):
        pass

    interruption = CleanupTermination()
    trace = tmp_path / "capture.trace"
    trace.mkdir()
    sidecar = tmp_path / "capture.trace.summary.json"
    sidecar.write_text("partial", encoding="utf-8")

    def interrupt_after_attempt(_watcher: object) -> tuple[str, str]:
        raise interruption

    monkeypatch.setattr(
        benchmark_xctrace,
        "terminate_direct_process",
        interrupt_after_attempt,
    )
    errors = benchmark_xctrace.cleanup_capture(
        driver=None,
        recorder=None,
        watcher=object(),
        trace=trace,
        sidecar=sidecar,
        retain_outputs=False,
    )

    assert errors == [interruption]
    assert not trace.exists()
    assert not sidecar.exists()
