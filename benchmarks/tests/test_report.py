from __future__ import annotations

import importlib.util
import json
import signal
import subprocess
import sys
import threading
from pathlib import Path
from types import SimpleNamespace
from typing import Any

import pytest

MODULE_PATH = Path(__file__).resolve().parents[1] / "run.py"
sys.path.insert(0, str(MODULE_PATH.parent))
SPEC = importlib.util.spec_from_file_location("benchmark_run", MODULE_PATH)
assert SPEC and SPEC.loader
benchmark_run = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(benchmark_run)

XCTRACE_PATH = Path(__file__).resolve().parents[1] / "parse-render/run_xctrace.py"
XCTRACE_SPEC = importlib.util.spec_from_file_location("benchmark_xctrace", XCTRACE_PATH)
assert XCTRACE_SPEC and XCTRACE_SPEC.loader
benchmark_xctrace = importlib.util.module_from_spec(XCTRACE_SPEC)
sys.modules[XCTRACE_SPEC.name] = benchmark_xctrace
XCTRACE_SPEC.loader.exec_module(benchmark_xctrace)


def metric(name: str = "measurement") -> dict[str, Any]:
    return {
        "id": "measurement",
        "name": name,
        "value": 1.0,
        "unit": "ms",
        "statistic": "p50",
    }


def section(section_id: str) -> dict[str, Any]:
    return {
        "id": section_id,
        "name": f"Section {section_id}",
        "sample_counts": {"test.sample": 1},
        "metrics": [metric()],
        "assertions": [{"id": "correct", "name": "correct", "passed": True}],
        "notes": [],
    }


def artifact(digest: str = "a" * 64) -> dict[str, Any]:
    return {
        "canonical_transaction_sha256": digest,
        "canonical_transaction_bytes": 42,
    }


def window_isolation_assertion() -> dict[str, Any]:
    return {
        "id": "window_isolation_fail_closed",
        "name": "WindowServer isolation rejects an exact synthetic occluder",
        "passed": True,
        "detail": (
            "window isolation self-test passed: dock=20 status=25 ahead=26 "
            "popup=101 target=123 occluder=124"
        ),
    }




def valid_report() -> dict[str, Any]:
    sections: dict[str, dict[str, Any]] = {}
    artifacts: dict[str, dict[str, Any]] = {}
    for driver in valid_manifest()["drivers"]:
        payload = payload_for_driver(driver)
        artifacts[driver["name"]] = payload["artifacts"]
        for emitted in payload["sections"]:
            benchmark_run._merge_driver_section(sections, emitted)
    benchmark_run.append_parity_assertion(sections, artifacts)
    sections["31.1"]["assertions"].append(
        {
            "id": "candidate_failure_cleanup",
            "name": "candidate failure cleanup",
            "passed": True,
        }
    )
    sections["31.1"]["assertions"].append(window_isolation_assertion())
    sections["31.5"]["metrics"].append(
        {
            "id": "production_reconnect_suite_ms",
            "name": "production reconnect boundary suite",
            "value": 1.0,
            "unit": "ms",
            "statistic": "wall",
        }
    )
    sections["31.5"]["assertions"].append(
        {
            "id": "production_reconnect_suite",
            "name": "production reconnect boundary suite",
            "passed": True,
        }
    )
    sections["31.5"]["sample_counts"]["runner.production_conformance"] = 9
    return {
        "schema_version": 1,
        "generated_at": "2026-01-01T00:00:00Z",
        "profile": "full",
        "fixture": benchmark_run.CANONICAL_FIXTURE,
        "environment": {
            "platform": "macOS-26.4.1-arm64-arm-64bit",
            "machine": "arm64",
            "python": "3.14",
            "chip": "Apple M4 Max",
            "physical_ram_bytes": 137_438_953_472,
            "xcode": "Xcode 26.4\nBuild version 17E30",
            "swift": "Apple Swift version 6.2.3",
            "rust": "rustc 1.93.1",
            "git_commit": "a" * 40,
            "git_dirty": False,
        },
        "driver_artifacts": artifacts,
        "sections": list(sections.values()),
    }


def valid_manifest() -> dict[str, Any]:
    return {
        "schema_version": 1,
        "fixture": benchmark_run.CANONICAL_FIXTURE,
        "required_sections": list(benchmark_run.EXPECTED_SECTIONS),
        "drivers": [
            {
                "name": "rust",
                "sections": ["31.2", "31.5", "31.6"],
                "command": list(benchmark_run.EXPECTED_DRIVER_COMMANDS["rust"]),
            },
            {
                "name": "macos",
                "sections": ["31.1", "31.3", "31.4", "31.5", "31.6"],
                "platform": "darwin",
                "command": list(benchmark_run.EXPECTED_DRIVER_COMMANDS["macos"]),
            },
        ],
        "verification_commands": [
            {
                "name": "production reconnect boundary suite",
                "section": "31.5",
                "command": [
                    "scripts/run-conformance",
                    "--suite",
                    "8",
                    "--implementation",
                    "both",
                ],
                "timeout": 1200,
                "required_output": [
                    "8  reconnect",
                    "PASS    9 runner(s)",
                    "1 passed, 0 failed",
                ],
            }
        ],
    }


def test_percentile_is_deterministic() -> None:
    assert benchmark_run.percentile([9, 1, 5, 3, 7], 0.5) == 5
    assert benchmark_run.percentile([9, 1, 5, 3, 7], 0.95) == 9


def test_environment_metadata_is_checked_complete_and_includes_untracked(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    outputs = {
        ("/usr/sbin/sysctl", "-n", "machdep.cpu.brand_string"): "Apple M4 Max",
        ("/usr/sbin/sysctl", "-n", "hw.memsize"): "137438953472",
        ("/usr/bin/xcodebuild", "-version"): "Xcode 26.4\nBuild version 17E30",
        ("/usr/bin/swift", "--version"): "Apple Swift version 6.2.3",
        ("rustc", "--version"): "rustc 1.93.1",
        ("git", "rev-parse", "HEAD"): "a" * 40,
        (
            "git",
            "status",
            "--porcelain=v1",
            "--untracked-files=normal",
        ): "?? untracked-benchmark-input",
    }
    calls: list[tuple[str, ...]] = []

    def fake_runner(command: list[str], **kwargs: Any) -> SimpleNamespace:
        calls.append(tuple(command))
        assert kwargs == {
            "cwd": benchmark_run.ROOT,
            "capture_output": True,
            "text": True,
            "check": True,
            "timeout": benchmark_run.METADATA_COMMAND_TIMEOUT_SECONDS,
        }
        return SimpleNamespace(stdout=outputs[tuple(command)], stderr="")

    monkeypatch.setattr(benchmark_run.platform, "platform", lambda: "macOS-test")
    monkeypatch.setattr(benchmark_run.platform, "machine", lambda: "arm64")
    monkeypatch.setattr(benchmark_run.platform, "python_version", lambda: "3.14")

    environment = benchmark_run.benchmark_environment(runner=fake_runner)

    assert environment == {
        "platform": "macOS-test",
        "machine": "arm64",
        "python": "3.14",
        "chip": "Apple M4 Max",
        "physical_ram_bytes": 137_438_953_472,
        "xcode": "Xcode 26.4\nBuild version 17E30",
        "swift": "Apple Swift version 6.2.3",
        "rust": "rustc 1.93.1",
        "git_commit": "a" * 40,
        "git_dirty": True,
    }
    assert (
        "git",
        "status",
        "--porcelain=v1",
        "--untracked-files=normal",
    ) in calls


def test_environment_metadata_command_failure_has_no_unknown_fallback() -> None:
    def failing_runner(_command: list[str], **_kwargs: Any) -> SimpleNamespace:
        raise subprocess.TimeoutExpired(["metadata"], 10)

    with pytest.raises(
        benchmark_run.BenchmarkError,
        match="environment metadata command failed",
    ):
        benchmark_run.benchmark_environment(runner=failing_runner)


@pytest.mark.parametrize("profile", ["smoke", "full"])
@pytest.mark.parametrize("driver_name", ["rust", "macos"])
def test_driver_sample_counts_are_profile_exact(
    profile: str,
    driver_name: str,
) -> None:
    driver = next(
        item for item in valid_manifest()["drivers"] if item["name"] == driver_name
    )
    payload = payload_for_driver(driver, profile=profile)
    benchmark_run.validate_driver_output(payload, driver, profile=profile)

    first_section = payload["sections"][0]
    first_key = next(iter(first_section["sample_counts"]))
    first_section["sample_counts"][first_key] += 1
    with pytest.raises(benchmark_run.BenchmarkError, match="sample counts"):
        benchmark_run.validate_driver_output(payload, driver, profile=profile)
@pytest.mark.parametrize("profile", ["smoke", "full"])
def test_network_causal_and_recovery_trials_have_explicit_sample_counts(
    profile: str,
) -> None:
    counts = benchmark_run.expected_driver_sample_counts("macos", profile)["31.4"]
    iterations = max(
        5,
        benchmark_run.PROFILE_DRIVER_ITERATIONS[profile]["macos"],
    )
    assert counts["macos.local_held_response"] == (
        iterations * len(benchmark_run.LOCAL_INTERACTIONS) * 4
    )
    assert counts["macos.loss"] == 1
    assert counts["macos.interruption"] == 1

def test_shared_section_sample_counts_merge_without_silent_collisions() -> None:
    sections: dict[str, dict[str, Any]] = {}
    rust = section("31.5")
    rust["sample_counts"] = {"rust.boundary": 25}
    macos = section("31.5")
    macos["sample_counts"] = {"macos.boundary": 3}

    benchmark_run._merge_driver_section(sections, rust)
    benchmark_run._merge_driver_section(sections, macos)
    assert sections["31.5"]["sample_counts"] == {
        "rust.boundary": 25,
        "macos.boundary": 3,
    }

    collision = section("31.5")
    collision["sample_counts"] = {"rust.boundary": 1}
    with pytest.raises(benchmark_run.BenchmarkError, match="colliding"):
        benchmark_run._merge_driver_section(sections, collision)


def test_report_schema_requires_complete_environment_metadata() -> None:
    report = valid_report()
    del report["environment"]["xcode"]
    with pytest.raises(benchmark_run.BenchmarkError, match="schema violation"):
        benchmark_run.validate_report(report, list(benchmark_run.EXPECTED_SECTIONS))


def test_markdown_renders_environment_and_sample_counts() -> None:
    rendered = benchmark_run.markdown(valid_report())
    assert "- Chip: Apple M4 Max" in rendered
    assert "- Physical RAM: 137438953472 bytes" in rendered
    assert "- Git: " + "a" * 40 + " (clean)" in rendered
    assert "macos.srui.render" in rendered
    assert "runner.production_conformance" in rendered


def test_over_2x_honors_direction() -> None:
    assert benchmark_run.over_2x(
        {"value": 2.01, "target": 1.0, "target_direction": "max"}
    )
    assert not benchmark_run.over_2x(
        {"value": 2.0, "target": 1.0, "target_direction": "max"}
    )
    assert benchmark_run.over_2x(
        {"value": 4.9, "target": 10.0, "target_direction": "min"}
    )


def test_report_calls_out_performance_followup() -> None:
    report = valid_report()
    report["profile"] = "smoke"
    report["sections"][2]["metrics"][0].update(
        {"value": 2.1, "target": 1.0, "target_direction": "max"}
    )
    rendered = benchmark_run.markdown(report)
    assert "PERFORMANCE FOLLOW-UP (>2x)" in rendered
    assert "WARNING >2x" in rendered


def test_report_flags_tail_local_latency_when_median_is_below_2x() -> None:
    report = valid_report()
    network = next(section for section in report["sections"] if section["id"] == "31.4")
    budget = next(
        metric["value"]
        for metric in network["metrics"]
        if metric["id"] == benchmark_run.LOCAL_FRAME_BUDGET_ID
    )
    interaction = {
        metric["statistic"]: metric
        for metric in network["metrics"]
        if metric["id"] == "interaction.hover.rtt.0"
    }
    interaction["p50"]["value"] = budget * 1.5
    interaction["p95"]["value"] = budget * 2.1
    interaction["p99"]["value"] = budget * 2.2

    benchmark_run.validate_report(report, list(benchmark_run.EXPECTED_SECTIONS))
    rendered = benchmark_run.markdown(report)
    assert "§31.4 hover at 0ms RTT (p95):" in rendered
    assert "§31.4 hover at 0ms RTT (p99):" in rendered
    assert "§31.4 hover at 0ms RTT (p50):" not in rendered


def test_report_schema_rejects_unknown_and_non_numeric_fields() -> None:
    report = valid_report()
    report["unexpected"] = True
    with pytest.raises(benchmark_run.BenchmarkError, match="schema violation"):
        benchmark_run.validate_report(report, list(benchmark_run.EXPECTED_SECTIONS))

    report = valid_report()
    report["sections"][0]["metrics"][0]["value"] = True
    with pytest.raises(benchmark_run.BenchmarkError, match="schema violation"):
        benchmark_run.validate_report(report, list(benchmark_run.EXPECTED_SECTIONS))


def test_report_schema_requires_all_six_sections() -> None:
    report = valid_report()
    report["sections"].pop()
    with pytest.raises(benchmark_run.BenchmarkError, match="schema violation"):
        benchmark_run.validate_report(report, list(benchmark_run.EXPECTED_SECTIONS))


def test_report_rejects_noncanonical_fixture_and_metric_name() -> None:
    report = valid_report()
    report["fixture"] = "benchmarks/fixtures/substitute.json"
    with pytest.raises(benchmark_run.BenchmarkError, match="report fixture must be exactly"):
        benchmark_run.validate_report(report, list(benchmark_run.EXPECTED_SECTIONS))

    report = valid_report()
    report["sections"][0]["metrics"][0]["name"] = "plausible but substituted"
    with pytest.raises(benchmark_run.BenchmarkError, match="metric display name"):
        benchmark_run.validate_report(report, list(benchmark_run.EXPECTED_SECTIONS))


def test_report_enforces_merged_inventory_and_canonical_parity() -> None:
    report = valid_report()
    benchmark_run.validate_report(report, list(benchmark_run.EXPECTED_SECTIONS))

    report["sections"][0]["metrics"].pop()
    with pytest.raises(benchmark_run.BenchmarkError, match="metric inventory"):
        benchmark_run.validate_report(report, list(benchmark_run.EXPECTED_SECTIONS))

    report = valid_report()
    report["driver_artifacts"]["macos"]["canonical_transaction_sha256"] = "b" * 64
    with pytest.raises(benchmark_run.BenchmarkError, match="inconsistent"):
        benchmark_run.validate_report(report, list(benchmark_run.EXPECTED_SECTIONS))

    report = valid_report()
    reconnect = next(item for item in report["sections"] if item["id"] == "31.5")
    reconnect["metrics"] = [
        item
        for item in reconnect["metrics"]
        if item["id"] != "production_reconnect_suite_ms"
    ]
    with pytest.raises(benchmark_run.BenchmarkError, match="metric inventory"):
        benchmark_run.validate_report(report, list(benchmark_run.EXPECTED_SECTIONS))


def test_macos_cadence_inventory_is_per_update_count_and_cadence() -> None:
    inventory = benchmark_run.EXPECTED_DRIVER_INVENTORY["macos"]["31.3"]["metrics"]
    actual = {
        identity: metadata
        for identity, metadata in inventory.items()
        if identity[0].startswith("cadence.")
    }
    expected = {}
    for count in (1, 100, 1000):
        for cadence in (60, 120, 144, 240):
            expected[(f"cadence.{count}.{cadence}.visible", "sample")] = (
                "ms",
                None,
                None,
            )
            expected[(f"cadence.{count}.{cadence}.bytes", "exact")] = (
                "bytes",
                None,
                None,
            )
            expected[(f"cadence.{count}.{cadence}.messages", "exact")] = (
                "messages",
                None,
                None,
            )
            for direction in ("inbound", "outbound"):
                expected[(f"cadence.{count}.{cadence}.{direction}_bytes", "exact")] = (
                    "bytes",
                    None,
                    None,
                )
                expected[(f"cadence.{count}.{cadence}.{direction}_messages", "exact")] = (
                    "messages",
                    None,
                    None,
                )
            expected[(f"cadence.{count}.{cadence}.repaints", "exact")] = (
                "repaints",
                None,
                None,
            )
    assert actual == expected


def test_macos_terminal_inventory_covers_standalone_display_comparison() -> None:
    inventory = benchmark_run.EXPECTED_DRIVER_INVENTORY["macos"]["31.6"]
    assert inventory["metrics"][
        ("standalone_terminal.decode_visible", "p50")
    ][0] == "ms"
    assert inventory["metrics"][
        ("standalone_terminal.draw_only", "p95")
    ][0] == "ms"
    assert inventory["metrics"][
        ("standalone_terminal.raster_completions", "exact")
    ][0] == "frames"
    assert inventory["metrics"][
        ("terminal_display.embedded_to_standalone_decode_ratio", "p50")
    ][0] == "ratio"
    assert inventory["metrics"][
        ("terminal_display.embedded_to_standalone_draw_ratio", "p50")
    ][0] == "ratio"
    assert inventory["assertions"] == {
        "terminal_offsets_exact",
        "standalone_terminal_offsets_exact",
        "terminal_display_draw_completion",
        "terminal_display_equivalent",
        "terminal_fresh_state",
    }




def test_driver_rejects_passing_local_latency_assertion_above_frame_budget() -> None:
    driver = next(
        item for item in valid_manifest()["drivers"] if item["name"] == "macos"
    )
    payload = payload_for_driver(driver)
    network = next(
        section for section in payload["sections"] if section["id"] == "31.4"
    )
    frame_budget = next(
        metric["value"]
        for metric in network["metrics"]
        if (metric["id"], metric["statistic"])
        == (benchmark_run.LOCAL_FRAME_BUDGET_ID, "exact")
    )
    for metric in network["metrics"]:
        if metric["id"] == "local_rtt_delta":
            metric["value"] = frame_budget + 0.001

    with pytest.raises(
        benchmark_run.BenchmarkError,
        match="local_latency_independent assertion contradicts",
    ):
        benchmark_run.validate_driver_output(payload, driver)


def test_smoke_driver_accepts_diagnostic_local_delta_above_frame_budget() -> None:
    driver = next(
        item for item in valid_manifest()["drivers"] if item["name"] == "macos"
    )
    payload = payload_for_driver(driver, profile="smoke")
    network = next(
        section for section in payload["sections"] if section["id"] == "31.4"
    )
    frame_budget = next(
        metric["value"]
        for metric in network["metrics"]
        if (metric["id"], metric["statistic"])
        == (benchmark_run.LOCAL_FRAME_BUDGET_ID, "exact")
    )
    for index, statistic in enumerate(benchmark_run.DISTRIBUTION, start=1):
        metric = next(
            metric
            for metric in network["metrics"]
            if (metric["id"], metric["statistic"])
            == ("local_rtt_delta", statistic)
        )
        metric["value"] = frame_budget + index * 0.001

    benchmark_run.validate_driver_output(payload, driver, profile="smoke")


def test_manifest_enforces_driver_declarations() -> None:
    manifest = valid_manifest()
    benchmark_run.validate_manifest(manifest)

    manifest["drivers"][0]["sections"].remove("31.5")
    with pytest.raises(benchmark_run.BenchmarkError, match="rust driver sections"):
        benchmark_run.validate_manifest(manifest)


def test_platform_mismatch_is_rejected_before_driver_execution() -> None:
    manifest = valid_manifest()
    benchmark_run.validate_runtime_platforms(manifest, current_platform="darwin")

    with pytest.raises(
        benchmark_run.BenchmarkError,
        match="no drivers were started",
    ):
        benchmark_run.validate_runtime_platforms(manifest, current_platform="linux")


def test_main_rejects_unsupported_platform_before_starting_drivers(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    started: list[str] = []

    def unexpected_driver(*_args: Any, **_kwargs: Any) -> dict[str, Any]:
        started.append("driver")
        raise AssertionError("unsupported-host main started a benchmark driver")

    monkeypatch.setattr(benchmark_run.sys, "platform", "linux")
    monkeypatch.setattr(benchmark_run, "run_driver", unexpected_driver)

    with pytest.raises(
        benchmark_run.BenchmarkError,
        match="complete six-section benchmark cannot run on linux.*no drivers were started",
    ):
        benchmark_run.main([])

    assert started == []




def test_manifest_requires_canonical_fixture_and_locked_rust_build() -> None:
    manifest = valid_manifest()
    manifest["fixture"] = "benchmarks/fixtures/substitute.json"
    with pytest.raises(benchmark_run.BenchmarkError, match="fixture must be exactly"):
        benchmark_run.validate_manifest(manifest)

    rust_command = benchmark_run.EXPECTED_DRIVER_COMMANDS["rust"]
    assert rust_command[:2] == ["cargo", "run"]
    assert "--locked" in rust_command
    macos_command = benchmark_run.EXPECTED_DRIVER_COMMANDS["macos"]
    assert macos_command[:2] == ["swift", "run"]
    assert "--disable-automatic-resolution" in macos_command


@pytest.mark.parametrize("driver_name", ["rust", "macos"])
def test_manifest_rejects_substitute_driver_command(driver_name: str) -> None:
    manifest = valid_manifest()
    driver = next(item for item in manifest["drivers"] if item["name"] == driver_name)
    driver["command"] = ["canned-json-writer"]

    with pytest.raises(
        benchmark_run.BenchmarkError,
        match=f"{driver_name} driver command",
    ):
        benchmark_run.validate_manifest(manifest)


def test_manifest_rejects_substitute_reconnect_command() -> None:
    manifest = valid_manifest()
    manifest["verification_commands"][0]["command"] = ["true"]
    with pytest.raises(
        benchmark_run.BenchmarkError,
        match="production suite 8 command",
    ):
        benchmark_run.validate_manifest(manifest)


def test_zero_exit_without_conformance_contract_fails_verification(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    result = SimpleNamespace(
        returncode=0,
        stdout="",
        stderr="",
        child_pid=123,
    )
    monkeypatch.setattr(
        benchmark_run,
        "run_managed_command",
        lambda *_args, **_kwargs: result,
    )
    monkeypatch.setattr(benchmark_run, "ensure_free_space", lambda _path: None)

    _elapsed, passed, detail, sample_count = benchmark_run.run_verification(
        valid_manifest()["verification_commands"][0],
        default_timeout=1,
    )
    assert passed is False
    assert sample_count is None
    assert "missing required output" in detail


def test_reconnect_verification_parses_current_suite_table(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    output = (
        "SRUI §32 conformance — both\n"
        " 8  reconnect                PASS    9 runner(s)\n"
        "1 passed, 0 failed, 0 documented gap(s), 0 not applicable\n"
    )
    monkeypatch.setattr(
        benchmark_run,
        "run_managed_command",
        lambda *_args, **_kwargs: SimpleNamespace(
            returncode=0,
            stdout=output,
            stderr="",
            child_pid=123,
        ),
    )
    monkeypatch.setattr(benchmark_run, "ensure_free_space", lambda _path: None)

    _elapsed, passed, _detail, sample_count = benchmark_run.run_verification(
        valid_manifest()["verification_commands"][0],
        default_timeout=1,
    )

    assert passed is True
    assert sample_count == 9


def valid_metric_value(
    metric_id: str,
    statistic: str,
    frame_budget: float,
) -> float:
    if (metric_id, statistic) == (benchmark_run.LOCAL_FRAME_BUDGET_ID, "exact"):
        return frame_budget
    if metric_id in {"idle.bytes", "idle.messages"}:
        return 0.0
    if not metric_id.startswith("cadence."):
        return 1.0

    update_count = float(metric_id.split(".")[1])
    suffix_values = {
        "inbound_bytes": 10.0,
        "outbound_bytes": 5.0,
        "bytes": 15.0,
        "inbound_messages": update_count,
        "outbound_messages": 3.0,
        "messages": update_count + 3.0,
    }
    suffix = metric_id.rsplit(".", 1)[-1]
    return suffix_values.get(suffix, 1.0)


def payload_for_driver(
    driver: dict[str, Any],
    *,
    profile: str = "full",
) -> dict[str, Any]:
    expected = benchmark_run.EXPECTED_DRIVER_INVENTORY[driver["name"]]
    frame_budget = 1000.0 / 120.0
    sections = []
    for section_id in driver["sections"]:
        inventory = expected[section_id]
        sections.append(
            {
                "id": section_id,
                "name": f"Section {section_id}",
                "sample_counts": benchmark_run.expected_driver_sample_counts(
                    driver["name"], profile
                )[section_id],
                "metrics": [
                    {
                        "id": metric_id,
                        "name": benchmark_run.expected_metric_display_name(
                            metric_id, profile
                        ),
                        "value": valid_metric_value(
                            metric_id,
                            statistic,
                            frame_budget,
                        ),
                        "unit": metadata[0],
                        "statistic": statistic,
                        **(
                            {
                                "target": (
                                    frame_budget
                                    if metadata[1]
                                    == benchmark_run.LOCAL_FRAME_BUDGET_ID
                                    else metadata[1]
                                ),
                                "target_direction": metadata[2],
                            }
                            if metadata[1] is not None
                            else {}
                        ),
                    }
                    for (metric_id, statistic), metadata in sorted(
                        inventory["metrics"].items()
                    )
                ],
                "assertions": [
                    {
                        "id": assertion_id,
                        "name": assertion_id,
                        "passed": True,
                    }
                    for assertion_id in sorted(inventory["assertions"])
                ],
                "notes": [],
            }
        )
    artifacts = artifact()
    if driver["name"] == "macos":
        artifacts["renderer_process_attribution"] = [
            {
                "candidate": "srui",
                "driver_pid": 42,
                "host_pid": 43,
                "helper_pids": [],
                "started_unix_ns": 1_000,
                "ended_unix_ns": 2_000,
                "measurement_intervals": [
                    {
                        "started_unix_ns": 1_200,
                        "ended_unix_ns": 1_500,
                        "required_allocation_pids": [],
                    },
                ],
                "helper_pid_source": "no helper processes",
                "process_identities": [
                    {
                        "pid": 43,
                        "birth_unix_ns": 1_001,
                        "observed_alive_through_unix_ns": 1_600,
                    },
                ],
            },
            {
                "candidate": "webkit",
                "driver_pid": 42,
                "host_pid": 44,
                "helper_pids": [45],
                "started_unix_ns": 3_000,
                "ended_unix_ns": 4_000,
                "measurement_intervals": [
                    {
                        "started_unix_ns": 3_200,
                        "ended_unix_ns": 3_500,
                        "required_allocation_pids": [],
                    },
                ],
                "helper_pid_source": "WKWebView diagnostic process identifiers",
                "process_identities": [
                    {
                        "pid": 44,
                        "birth_unix_ns": 1_002,
                        "observed_alive_through_unix_ns": 3_600,
                    },
                    {
                        "pid": 45,
                        "birth_unix_ns": 1_003,
                        "observed_alive_through_unix_ns": 3_600,
                    },
                ],
            },
        ]
    return {"artifacts": artifacts, "sections": sections}


@pytest.mark.parametrize("profile", ["smoke", "full"])
@pytest.mark.parametrize("driver_name", ["rust", "macos"])
def test_driver_inventory_accepts_only_complete_declared_measurements(
    driver_name: str,
    profile: str,
) -> None:
    driver = next(
        item for item in valid_manifest()["drivers"] if item["name"] == driver_name
    )
    payload = payload_for_driver(driver, profile=profile)
    benchmark_run.validate_driver_output(payload, driver, profile=profile)

    payload["sections"][0]["metrics"].pop()
    with pytest.raises(benchmark_run.BenchmarkError, match="metric inventory"):
        benchmark_run.validate_driver_output(payload, driver, profile=profile)


def test_macos_allocation_inventory_uses_signed_host_endpoint_deltas() -> None:
    inventory = benchmark_run.EXPECTED_DRIVER_INVENTORY["macos"]["31.1"]
    expected = {
        (
            f"{candidate}.host_net_live_allocation_{suffix}",
            statistic,
        ): (unit, None, None)
        for candidate in ("srui", "webkit")
        for suffix, unit in (("blocks", "blocks"), ("bytes", "bytes"))
        for statistic in benchmark_run.DISTRIBUTION
    }
    assert {
        identity: metadata
        for identity, metadata in inventory["metrics"].items()
        if identity[0] in benchmark_run.SIGNED_METRIC_IDS
    } == expected
    assert "host_net_live_allocation_scope" in inventory["assertions"]
    assert not any(
        "retained_allocations" in metric_id
        for metric_id, _statistic in inventory["metrics"]
    )


@pytest.mark.parametrize(
    "metric_id",
    sorted(benchmark_run.SIGNED_METRIC_IDS),
)
def test_signed_net_live_allocation_deltas_allow_negative_integers(
    metric_id: str,
) -> None:
    driver = next(
        item for item in valid_manifest()["drivers"] if item["name"] == "macos"
    )
    payload = payload_for_driver(driver)
    measured = next(
        metric
        for section in payload["sections"]
        for metric in section["metrics"]
        if metric["id"] == metric_id and metric["statistic"] == "p50"
    )
    measured["value"] = -1.0
    benchmark_run.validate_driver_output(payload, driver)

    measured["value"] = -1.5
    with pytest.raises(
        benchmark_run.BenchmarkError,
        match="must be a whole count",
    ):
        benchmark_run.validate_driver_output(payload, driver)


def test_report_rejects_obsolete_embedded_xctrace_evidence() -> None:
    report = valid_report()
    report["driver_artifacts"]["macos"]["allocation_capture"] = {}
    with pytest.raises(
        benchmark_run.BenchmarkError,
        match="schema violation",
    ):
        benchmark_run.validate_report(report, list(benchmark_run.EXPECTED_SECTIONS))


@pytest.mark.parametrize(
    ("metric_id", "statistic", "value", "message"),
    [
        ("abstract_state_generation_ms", "p50", -0.1, "must be nonnegative"),
        ("abstract_state_generation_ms", "p50", float("nan"), "not finite numeric"),
        ("abstract_state_generation_ms", "p50", float("inf"), "not finite numeric"),
        ("serialized_transaction_bytes", "exact", 1.5, "must be a whole count"),
    ],
)
def test_driver_rejects_invalid_metric_values(
    metric_id: str,
    statistic: str,
    value: float,
    message: str,
) -> None:
    driver = next(
        item for item in valid_manifest()["drivers"] if item["name"] == "rust"
    )
    payload = payload_for_driver(driver)
    target = next(
        metric
        for section in payload["sections"]
        for metric in section["metrics"]
        if metric["id"] == metric_id and metric["statistic"] == statistic
    )
    target["value"] = value
    with pytest.raises(benchmark_run.BenchmarkError, match=message):
        benchmark_run.validate_driver_output(payload, driver)


def test_driver_rejects_reversed_percentiles_and_substitute_name() -> None:
    driver = next(
        item for item in valid_manifest()["drivers"] if item["name"] == "rust"
    )
    payload = payload_for_driver(driver)
    distribution = {
        metric["statistic"]: metric
        for section in payload["sections"]
        for metric in section["metrics"]
        if metric["id"] == "abstract_state_generation_ms"
    }
    distribution["p50"]["value"] = 100.0
    distribution["p95"]["value"] = 10.0
    distribution["p99"]["value"] = 1.0
    with pytest.raises(benchmark_run.BenchmarkError, match="percentile ordering"):
        benchmark_run.validate_driver_output(payload, driver)

    payload = payload_for_driver(driver)
    payload["sections"][0]["metrics"][0]["name"] = "arbitrary timer"
    with pytest.raises(benchmark_run.BenchmarkError, match="metric display name"):
        benchmark_run.validate_driver_output(payload, driver)


def test_mutation_assertions_are_recomputed_from_emitted_wire_metrics() -> None:
    driver = next(
        item for item in valid_manifest()["drivers"] if item["name"] == "macos"
    )
    payload = payload_for_driver(driver)
    mutation = next(section for section in payload["sections"] if section["id"] == "31.3")
    idle_bytes = next(
        metric for metric in mutation["metrics"] if metric["id"] == "idle.bytes"
    )
    idle_bytes["value"] = 7.0
    with pytest.raises(benchmark_run.BenchmarkError, match="idle_zero_traffic contradicts"):
        benchmark_run.validate_driver_output(payload, driver)

    payload = payload_for_driver(driver)
    mutation = next(section for section in payload["sections"] if section["id"] == "31.3")
    cadence_bytes = next(
        metric
        for metric in mutation["metrics"]
        if metric["id"] == "cadence.100.60.bytes"
    )
    cadence_bytes["value"] = 999_999.0
    with pytest.raises(benchmark_run.BenchmarkError, match="cadence_wire_invariant contradicts"):
        benchmark_run.validate_driver_output(payload, driver)

    payload = payload_for_driver(driver)
    mutation = next(section for section in payload["sections"] if section["id"] == "31.3")
    coherent_substitutes = {
        "bytes": 10.0,
        "inbound_bytes": 4.0,
        "outbound_bytes": 6.0,
        "messages": 2.0,
        "inbound_messages": 1.0,
        "outbound_messages": 1.0,
    }
    for cadence in (60, 120, 144, 240):
        prefix = f"cadence.100.{cadence}."
        for metric in mutation["metrics"]:
            if metric["id"].startswith(prefix):
                suffix = metric["id"].removeprefix(prefix)
                if suffix in coherent_substitutes:
                    metric["value"] = coherent_substitutes[suffix]
    with pytest.raises(benchmark_run.BenchmarkError, match="cadence_wire_invariant contradicts"):
        benchmark_run.validate_driver_output(payload, driver)


def test_driver_inventory_constrains_units_and_required_target_metadata() -> None:
    driver = next(
        item for item in valid_manifest()["drivers"] if item["name"] == "macos"
    )

    payload = payload_for_driver(driver)
    serialized = next(
        metric
        for section in payload["sections"]
        for metric in section["metrics"]
        if metric["id"] == "updates.1.bytes"
    )
    serialized["unit"] = "ms"
    with pytest.raises(benchmark_run.BenchmarkError, match="metric metadata"):
        benchmark_run.validate_driver_output(payload, driver)

    payload = payload_for_driver(driver)
    targeted = next(
        metric
        for section in payload["sections"]
        for metric in section["metrics"]
        if metric["id"] == "updates.100.semantic"
        and metric["statistic"] == "p50"
    )
    targeted.pop("target")
    targeted.pop("target_direction")
    with pytest.raises(benchmark_run.BenchmarkError, match="metric metadata"):
        benchmark_run.validate_driver_output(payload, driver)

    payload = payload_for_driver(driver)
    targeted = next(
        metric
        for section in payload["sections"]
        for metric in section["metrics"]
        if metric["id"] == "local_rtt_delta" and metric["statistic"] == "p50"
    )
    targeted["target_direction"] = "min"
    with pytest.raises(benchmark_run.BenchmarkError, match="metric metadata"):
        benchmark_run.validate_driver_output(payload, driver)


def test_macos_attribution_is_bound_to_launched_driver_pid_and_birth_identity() -> None:
    driver = next(
        item for item in valid_manifest()["drivers"] if item["name"] == "macos"
    )
    payload = payload_for_driver(driver)
    benchmark_run.validate_driver_output(payload, driver, launched_pid=42)

    with pytest.raises(benchmark_run.BenchmarkError, match="does not match launched"):
        benchmark_run.validate_driver_output(payload, driver, launched_pid=99)

    payload = payload_for_driver(driver)
    payload["artifacts"]["renderer_process_attribution"][1][
        "process_identities"
    ].pop()
    with pytest.raises(benchmark_run.BenchmarkError, match="exactly cover"):
        benchmark_run.validate_driver_output(payload, driver, launched_pid=42)


def test_ordinary_macos_attribution_rejects_targeted_pid_requirements() -> None:
    driver = next(
        item for item in valid_manifest()["drivers"] if item["name"] == "macos"
    )
    payload = payload_for_driver(driver)
    payload["artifacts"]["renderer_process_attribution"][0][
        "measurement_intervals"
    ][0]["required_allocation_pids"] = [43]

    with pytest.raises(
        benchmark_run.BenchmarkError,
        match="ordinary renderer interval 0",
    ):
        benchmark_run.validate_driver_output(payload, driver, launched_pid=42)


def test_dynamic_local_frame_budget_controls_every_local_percentile_target() -> None:
    driver = next(
        item for item in valid_manifest()["drivers"] if item["name"] == "macos"
    )
    payload = payload_for_driver(driver)
    network = next(item for item in payload["sections"] if item["id"] == "31.4")
    budget = next(
        item
        for item in network["metrics"]
        if item["id"] == benchmark_run.LOCAL_FRAME_BUDGET_ID
    )
    budget["value"] = 7.5
    with pytest.raises(benchmark_run.BenchmarkError, match="metric metadata"):
        benchmark_run.validate_driver_output(payload, driver)

    for item in network["metrics"]:
        if item.get("target_direction") == "max":
            item["target"] = 7.5
    benchmark_run.validate_driver_output(payload, driver)

    budget["value"] = 0
    for item in network["metrics"]:
        if item.get("target_direction") == "max":
            item["target"] = 0
    with pytest.raises(benchmark_run.BenchmarkError, match="must be positive"):
        benchmark_run.validate_driver_output(payload, driver)


def test_window_isolation_report_assertion_is_full_profile_only() -> None:
    assertion_id = benchmark_run.WINDOW_ISOLATION_ASSERTION_ID
    assert assertion_id in benchmark_run.EXPECTED_REPORT_INVENTORY["31.1"][
        "assertions"
    ]
    assert assertion_id in benchmark_run.expected_report_inventory_for_profile(
        "31.1", "full"
    )["assertions"]
    assert assertion_id not in benchmark_run.expected_report_inventory_for_profile(
        "31.1", "smoke"
    )["assertions"]


def test_window_isolation_self_test_parses_exact_evidence() -> None:
    line = (
        "window isolation self-test passed: dock=20 status=25 ahead=26 "
        "popup=101 target=123 occluder=124"
    )
    assert benchmark_run.parse_window_isolation_self_test_output(
        "unrelated stdout\n",
        f"unrelated stderr\n{line}\n",
    ) == {
        "dock": 20,
        "status": 25,
        "ahead": 26,
        "popup": 101,
        "target": 123,
        "occluder": 124,
    }


def test_window_isolation_self_test_rejects_malformed_or_duplicate_lines() -> None:
    malformed = (
        "window isolation self-test passed: dock=20 status=25 ahead=26 "
        "popup=101 target=abc occluder=124"
    )
    with pytest.raises(benchmark_run.BenchmarkError, match="malformed result line"):
        benchmark_run.parse_window_isolation_self_test_output("", malformed)

    valid = (
        "window isolation self-test passed: dock=20 status=25 ahead=26 "
        "popup=101 target=123 occluder=124"
    )
    with pytest.raises(benchmark_run.BenchmarkError, match="exactly one result line"):
        benchmark_run.parse_window_isolation_self_test_output(valid, valid)


def test_window_isolation_self_test_rejects_invalid_level_order() -> None:
    line = (
        "window isolation self-test passed: dock=20 status=25 ahead=101 "
        "popup=101 target=123 occluder=124"
    )
    with pytest.raises(benchmark_run.BenchmarkError, match="invalid level ordering"):
        benchmark_run.parse_window_isolation_self_test_output("", line)


@pytest.mark.parametrize(
    "identity",
    ("target=0 occluder=124", "target=123 occluder=123"),
)
def test_window_isolation_self_test_rejects_invalid_window_identity(
    identity: str,
) -> None:
    line = (
        "window isolation self-test passed: dock=20 status=25 ahead=26 "
        f"popup=101 {identity}"
    )
    with pytest.raises(
        benchmark_run.BenchmarkError,
        match="distinct positive window IDs",
    ):
        benchmark_run.parse_window_isolation_self_test_output("", line)


def test_window_isolation_self_test_invokes_built_release_driver(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    binary = tmp_path / "client-macos/.build/release/BenchmarkDriver"
    binary.parent.mkdir(parents=True)
    binary.write_bytes(b"binary")
    fixture = tmp_path / "fixture.json"
    fixture.write_text("{}", encoding="utf-8")
    calls: list[tuple[list[str], dict[str, Any]]] = []
    output_path: Path | None = None

    def fake_command(command: list[str], **kwargs: Any) -> SimpleNamespace:
        nonlocal output_path
        calls.append((command, kwargs))
        output_path = Path(command[command.index("--output") + 1])
        assert output_path.is_file()
        return SimpleNamespace(
            returncode=0,
            stdout="",
            stderr=(
                "window isolation self-test passed: dock=20 status=25 ahead=26 "
                "popup=101 target=123 occluder=124\n"
            ),
            child_pid=42,
        )

    monkeypatch.setattr(benchmark_run, "ROOT", tmp_path)
    monkeypatch.setattr(benchmark_run, "run_managed_command", fake_command)
    monkeypatch.setattr(benchmark_run, "ensure_free_space", lambda _path: None)

    assertion = benchmark_run.run_window_isolation_self_test(fixture, 17)
    assert assertion == window_isolation_assertion()
    assert len(calls) == 1
    command, kwargs = calls[0]
    assert command[:4] == [
        "/usr/bin/env",
        "SRUI_BENCHMARK_PHASES=1",
        "SRUI_BENCHMARK_WINDOW_ISOLATION_SELF_TEST=1",
        str(binary),
    ]
    assert command[4:8] == [
        "--fixture",
        str(fixture),
        "--profile",
        "full",
    ]
    assert command[8] == "--output"
    assert kwargs["cwd"] == tmp_path
    assert kwargs["timeout"] == 17
    assert kwargs["label"] == "window isolation self-test"
    assert output_path is not None
    assert not output_path.exists()


def test_main_runs_window_isolation_after_normal_macos_driver(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    manifest = valid_manifest()
    fixture = tmp_path / manifest["fixture"]
    fixture.parent.mkdir(parents=True)
    fixture.write_text("{}", encoding="utf-8")
    manifest_path = tmp_path / "benchmarks/manifest.json"
    manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
    events: list[str] = []

    def fake_driver(
        driver: dict[str, Any],
        _fixture: Path,
        profile: str,
        _timeout: int,
    ) -> dict[str, Any]:
        events.append(f"driver:{driver['name']}")
        return payload_for_driver(driver, profile=profile)

    def stop_after_self_test(_fixture: Path, timeout: int) -> dict[str, Any]:
        events.append(f"self-test:{timeout}")
        raise benchmark_run.BenchmarkError("self-test sentinel")

    monkeypatch.setattr(benchmark_run, "ROOT", tmp_path)
    monkeypatch.setattr(benchmark_run, "MANIFEST", manifest_path)
    monkeypatch.setattr(benchmark_run.sys, "platform", "darwin")
    monkeypatch.setattr(benchmark_run, "ensure_free_space", lambda _path: None)
    monkeypatch.setattr(
        benchmark_run,
        "benchmark_environment",
        lambda: valid_report()["environment"],
    )
    monkeypatch.setattr(benchmark_run, "run_driver", fake_driver)
    monkeypatch.setattr(
        benchmark_run,
        "run_window_isolation_self_test",
        stop_after_self_test,
    )

    with pytest.raises(benchmark_run.BenchmarkError, match="self-test sentinel"):
        benchmark_run.main(["--profile", "full", "--timeout", "120"])
    assert events == ["driver:rust", "driver:macos", "self-test:30"]


def test_run_driver_waits_for_exact_attributed_process_identities(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    driver = next(
        item for item in valid_manifest()["drivers"] if item["name"] == "macos"
    )
    payload = payload_for_driver(driver, profile="smoke")
    observed: list[tuple[int, int]] = []

    def fake_command(command: list[str], **_kwargs: Any) -> SimpleNamespace:
        output_path = Path(command[command.index("--output") + 1])
        output_path.write_text(json.dumps(payload), encoding="utf-8")
        return SimpleNamespace(returncode=0, stdout="", stderr="", child_pid=42)

    def observe(
        identities: list[tuple[int, int]],
        *,
        label: str,
    ) -> None:
        assert label == "macos renderer candidates"
        observed.extend(identities)

    monkeypatch.setattr(benchmark_run, "run_managed_command", fake_command)
    monkeypatch.setattr(benchmark_run, "ensure_free_space", lambda _path: None)
    monkeypatch.setattr(
        benchmark_run,
        "wait_for_process_identities_gone",
        observe,
    )
    benchmark_run.run_driver(driver, tmp_path / "fixture.json", "smoke", 1)
    assert sorted(observed) == [(43, 1_001), (44, 1_002), (45, 1_003)]


def test_candidate_cleanup_probe_waits_for_exact_identity_and_removes_tempdir(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    binary = tmp_path / "client-macos/.build/release/BenchmarkDriver"
    binary.parent.mkdir(parents=True)
    binary.write_bytes(b"binary")
    fixture = tmp_path / "fixture.json"
    fixture.write_text("{}", encoding="utf-8")
    observed: list[tuple[int, int]] = []
    capture_directory: Path | None = None

    def fake_command(command: list[str], **_kwargs: Any) -> SimpleNamespace:
        nonlocal capture_directory
        forced_identity_failure = (
            "SRUI_BENCHMARK_FORCE_CANDIDATE_IDENTITY_FAILURE=1" in command
        )
        forced_internal_failure = (
            "SRUI_BENCHMARK_FORCE_CANDIDATE_INTERNAL_FAILURE=1" in command
        )
        identity_argument = next(
            item
            for item in command
            if item.startswith("SRUI_BENCHMARK_CANDIDATE_IDENTITY_PATH=")
        )
        identity_path = Path(identity_argument.split("=", 1)[1])
        capture_directory = identity_path.parent
        identity_path.write_text(
            json.dumps(
                {
                    "pid": (
                        77
                        if forced_identity_failure
                        else 81 if forced_internal_failure else 79
                    ),
                    "birth_unix_ns": (
                        123
                        if forced_identity_failure
                        else 127 if forced_internal_failure else 125
                    ),
                    "observed_alive_through_unix_ns": 456,
                }
            ),
            encoding="utf-8",
        )
        descendant_prefix = (
            "SRUI_BENCHMARK_CANDIDATE_DESCENDANT_IDENTITY_PATH="
            if forced_identity_failure or forced_internal_failure
            else "SRUI_BENCHMARK_CANDIDATE_DESCENDANT_OBSERVER_PATH="
        )
        descendant_argument = next(
            item for item in command if item.startswith(descendant_prefix)
        )
        descendant_path = Path(descendant_argument.split("=", 1)[1])
        descendant_path.write_text(
            json.dumps(
                {
                    "pid": (
                        78
                        if forced_identity_failure
                        else 82 if forced_internal_failure else 80
                    ),
                    "birth_unix_ns": (
                        124
                        if forced_identity_failure
                        else 128 if forced_internal_failure else 126
                    ),
                    "observed_alive_through_unix_ns": 457,
                }
            ),
            encoding="utf-8",
        )
        return SimpleNamespace(
            returncode=1,
            stdout="",
            stderr=(
                (
                    "BenchmarkDriver failed: forced renderer candidate internal "
                    "failure after descendant setup"
                    if forced_internal_failure
                    else "BenchmarkDriver failed: renderer candidate srui "
                    + (
                        "birth identity was unavailable"
                        if forced_identity_failure
                        else "exited with status 1"
                    )
                )
            ),
            child_pid=(
                66
                if forced_identity_failure
                else 68 if forced_internal_failure else 67
            ),
        )

    monkeypatch.setattr(benchmark_run, "ROOT", tmp_path)
    monkeypatch.setattr(benchmark_run, "run_managed_command", fake_command)
    monkeypatch.setattr(benchmark_run, "ensure_free_space", lambda _path: None)
    monkeypatch.setattr(
        benchmark_run,
        "wait_for_process_identities_gone",
        lambda identities, *, label: observed.extend(identities),
    )

    detail = benchmark_run.run_candidate_cleanup_probe(fixture, 30)
    assert "proved both the child and a real descendant gone" in detail
    assert observed == [
        (77, 123),
        (78, 124),
        (79, 125),
        (80, 126),
        (81, 127),
        (82, 128),
    ]
    assert capture_directory is not None
    assert not capture_directory.exists()




def test_driver_schema_requires_stable_measurement_ids() -> None:
    driver = valid_manifest()["drivers"][0]
    payload = payload_for_driver(driver)
    del payload["sections"][0]["metrics"][0]["id"]
    with pytest.raises(benchmark_run.BenchmarkError, match="schema violation"):
        benchmark_run.validate_driver_output(payload, driver)


def test_driver_output_must_match_declared_sections() -> None:
    driver = valid_manifest()["drivers"][0]
    payload = {
        "artifacts": artifact(),
        "sections": [section("31.2"), section("31.6")],
    }
    with pytest.raises(benchmark_run.BenchmarkError, match="manifest declares"):
        benchmark_run.validate_driver_output(payload, driver)


def test_canonical_parity_compares_digest_and_byte_count() -> None:
    sections = {"31.2": section("31.2")}
    benchmark_run.append_parity_assertion(
        sections,
        {"rust": artifact("a" * 64), "macos": artifact("b" * 64)},
    )
    assert sections["31.2"]["assertions"][-1]["passed"] is False

    sections = {"31.2": section("31.2")}
    macos_artifact = artifact()
    macos_artifact["renderer_process_attribution"] = []
    benchmark_run.append_parity_assertion(
        sections,
        {"rust": artifact(), "macos": macos_artifact},
    )
    assert sections["31.2"]["assertions"][-1]["passed"] is True
    assert (
        "exact canonical progressive sequence bytes"
        in sections["31.2"]["assertions"][-1]["detail"]
    )


def test_smoke_profile_cannot_record_baseline(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    called = False

    def run_driver(*_args: Any, **_kwargs: Any) -> dict[str, Any]:
        nonlocal called
        called = True
        return {}

    monkeypatch.setattr(benchmark_run, "run_driver", run_driver)
    with pytest.raises(SystemExit) as error:
        benchmark_run.main(["--profile", "smoke", "--record-baseline"])
    assert error.value.code == 2
    assert called is False


def test_record_baseline_rejects_dirty_tree_before_starting_driver(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    started: list[str] = []
    environment = valid_report()["environment"]
    environment["git_dirty"] = True

    monkeypatch.setattr(benchmark_run, "validate_runtime_platforms", lambda _manifest: None)
    monkeypatch.setattr(benchmark_run, "ensure_free_space", lambda _path: None)
    monkeypatch.setattr(benchmark_run, "benchmark_environment", lambda: environment)
    monkeypatch.setattr(
        benchmark_run,
        "run_driver",
        lambda *_args, **_kwargs: started.append("driver"),
    )

    with pytest.raises(benchmark_run.BenchmarkError, match="dirty Git tree"):
        benchmark_run.main(["--profile", "full", "--record-baseline"])
    assert started == []


def test_failed_assertions_cannot_replace_baseline() -> None:
    report = valid_report()
    report["sections"][0]["assertions"][0]["passed"] = False
    with pytest.raises(benchmark_run.BenchmarkError, match="refusing to overwrite"):
        benchmark_run.ensure_baseline_recordable(report)


def test_ensure_baseline_recordable_rejects_smoke_directly() -> None:
    report = valid_report()
    report["profile"] = "smoke"
    with pytest.raises(benchmark_run.BenchmarkError, match="non-full profile"):
        benchmark_run.ensure_baseline_recordable(report)


def test_dirty_or_changed_git_state_cannot_record_baseline() -> None:
    report = valid_report()
    report["environment"]["git_dirty"] = True
    with pytest.raises(benchmark_run.BenchmarkError, match="dirty Git tree"):
        benchmark_run.ensure_baseline_recordable(report)

    report = valid_report()
    ending_environment = dict(report["environment"])
    ending_environment["git_commit"] = "b" * 40
    with pytest.raises(benchmark_run.BenchmarkError, match="Git state changed"):
        benchmark_run.ensure_baseline_recordable(
            report,
            ending_environment=ending_environment,
        )

    ending_environment = dict(report["environment"])
    ending_environment["git_dirty"] = True
    with pytest.raises(benchmark_run.BenchmarkError, match="Git state changed"):
        benchmark_run.ensure_baseline_recordable(
            report,
            ending_environment=ending_environment,
        )


def test_full_failed_run_leaves_existing_baseline_untouched(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    manifest = valid_manifest()
    fixture = tmp_path / manifest["fixture"]
    fixture.parent.mkdir(parents=True)
    fixture.write_text("{}")
    manifest_path = tmp_path / "benchmarks/manifest.json"
    manifest_path.write_text(json.dumps(manifest))
    report_dir = tmp_path / "benchmarks/reports"
    report_dir.mkdir(parents=True)
    baseline_json = report_dir / "baseline.json"
    baseline_markdown = report_dir / "baseline.md"
    baseline_json.write_text("existing-json")
    baseline_markdown.write_text("existing-markdown")

    def fake_driver(
        driver: dict[str, Any],
        _fixture: Path,
        _profile: str,
        _timeout: int,
    ) -> dict[str, Any]:
        payload = payload_for_driver(driver)
        if driver["name"] == "rust":
            payload["sections"][0]["assertions"][0]["passed"] = False
        return payload

    monkeypatch.setattr(benchmark_run, "ROOT", tmp_path)
    monkeypatch.setattr(benchmark_run, "MANIFEST", manifest_path)
    monkeypatch.setattr(benchmark_run, "run_driver", fake_driver)
    monkeypatch.setattr(
        benchmark_run,
        "run_window_isolation_self_test",
        lambda _fixture, _timeout: window_isolation_assertion(),
    )
    monkeypatch.setattr(
        benchmark_run,
        "benchmark_environment",
        lambda: valid_report()["environment"],
    )
    monkeypatch.setattr(
        benchmark_run,
        "run_candidate_cleanup_probe",
        lambda _fixture, _timeout: "reaped",
    )
    monkeypatch.setattr(
        benchmark_run,
        "run_verification",
        lambda _spec, _timeout: (1.0, True, "exit 0", 9),
    )

    with pytest.raises(benchmark_run.BenchmarkError, match="refusing to overwrite"):
        benchmark_run.main(["--profile", "full", "--record-baseline"])
    assert baseline_json.read_text() == "existing-json"
    assert baseline_markdown.read_text() == "existing-markdown"


def test_report_pair_rolls_back_if_second_replacement_fails(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    json_path = tmp_path / "latest.json"
    markdown_path = tmp_path / "latest.md"
    json_path.write_text("existing-json", encoding="utf-8")
    markdown_path.write_text("existing-markdown", encoding="utf-8")
    original_replace = benchmark_run.os.replace
    failed = False

    def fail_markdown_install(source: Any, destination: Any) -> None:
        nonlocal failed
        source_path = Path(source)
        destination_path = Path(destination)
        if (
            not failed
            and destination_path == markdown_path
            and source_path.name.startswith(".latest.md.stage-")
        ):
            failed = True
            raise OSError("synthetic markdown replacement failure")
        original_replace(source, destination)

    monkeypatch.setattr(benchmark_run.os, "replace", fail_markdown_install)
    with pytest.raises(OSError, match="synthetic markdown replacement failure"):
        benchmark_run.write_report(valid_report(), tmp_path, "latest")

    assert json_path.read_text(encoding="utf-8") == "existing-json"
    assert markdown_path.read_text(encoding="utf-8") == "existing-markdown"
    assert not list(tmp_path.glob(".*.stage-*"))
    assert not list(tmp_path.glob(".*.backup-*"))


def test_report_pair_remains_consistent_when_signal_arrives_during_replacement(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    report = valid_report()
    json_path = tmp_path / "latest.json"
    markdown_path = tmp_path / "latest.md"
    original_replace = benchmark_run.os.replace
    signaled = False

    def signal_during_json_install(source: Any, destination: Any) -> None:
        nonlocal signaled
        source_path = Path(source)
        destination_path = Path(destination)
        if (
            not signaled
            and destination_path == json_path
            and source_path.name.startswith(".latest.json.stage-")
        ):
            signaled = True
            signal.pthread_kill(threading.get_ident(), signal.SIGTERM)
        original_replace(source, destination)

    monkeypatch.setattr(
        benchmark_run.os,
        "replace",
        signal_during_json_install,
    )
    with pytest.raises(benchmark_run.TerminationRequested):
        with benchmark_run.termination_handlers():
            benchmark_run.write_report(report, tmp_path, "latest")

    published = json.loads(json_path.read_text(encoding="utf-8"))
    rendered = markdown_path.read_text(encoding="utf-8")
    assert published["generated_at"] == report["generated_at"]
    assert report["generated_at"] in rendered
    assert not list(tmp_path.glob(".*.stage-*"))
    assert not list(tmp_path.glob(".*.backup-*"))


def test_benchmark_cli_reports_companion_failure_with_signal_exit(
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    recovery = benchmark_run.BenchmarkError(
        "renderer identity persisted; retained supervisor/PGID 123; "
        "control directory /tmp/recovery"
    )

    def fail(_argv: list[str] | None = None) -> int:
        raise BaseExceptionGroup(
            "termination plus recovery failure",
            [
                benchmark_run.TerminationRequested(signal.SIGTERM),
                recovery,
            ],
        )

    monkeypatch.setattr(benchmark_run, "main", fail)
    assert benchmark_run.cli([]) == 128 + signal.SIGTERM
    stderr = capsys.readouterr().err
    assert "benchmark failures accompanying interruption" in stderr
    assert "retained supervisor/PGID 123" in stderr
    assert "control directory /tmp/recovery" in stderr
    assert "benchmark interrupted by SIGTERM" in stderr


def test_xctrace_cli_reports_actual_cleanup_recovery_with_signal_exit(
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    tmp_path: Path,
) -> None:
    recovery_directory = tmp_path / "watcher-controls"

    class PersistentWatcher:
        closed = False
        label = "notification watcher"
        supervisor = SimpleNamespace(pid=456)
        ready_path = recovery_directory / "ready.json"

        @staticmethod
        def terminate() -> None:
            raise RuntimeError("synthetic persistent watcher cleanup failure")

    workspace = benchmark_xctrace.create_capture_workspace(tmp_path)
    cleanup_errors = benchmark_xctrace.cleanup_capture(
        driver=None,
        recorder=None,
        watcher=PersistentWatcher(),
        workspace=workspace,
        retain_outputs=False,
    )
    assert len(cleanup_errors) == 1
    assert isinstance(cleanup_errors[0], BaseExceptionGroup)
    assert not workspace.path.exists()

    def fail() -> int:
        raise BaseExceptionGroup(
            "termination plus watcher failure",
            [
                benchmark_xctrace.TerminationRequested(signal.SIGINT),
                *cleanup_errors,
            ],
        )

    monkeypatch.setattr(benchmark_xctrace, "main", fail)
    assert benchmark_xctrace.cli() == 130
    stderr = capsys.readouterr().err
    assert "allocation capture failures accompanying interruption" in stderr
    assert "retained supervisor/PGID 456" in stderr
    assert f"control directory {recovery_directory}" in stderr
    assert "allocation capture interrupted by SIGINT" in stderr


def test_signal_handler_raises_a_cleanup_safe_exception() -> None:
    with pytest.raises(benchmark_run.TerminationRequested) as error:
        benchmark_run._raise_termination(signal.SIGTERM, None)
    assert error.value.signum == signal.SIGTERM


def test_xctrace_attaches_one_exact_process() -> None:
    command = benchmark_xctrace.trace_command(
        Path("out.trace"),
        "notification",
        target_pid=431,
    )
    assert "--attach" in command
    assert command[command.index("--attach") + 1] == "431"
    assert "--all-processes" not in command
    assert "--launch" not in command
    assert "--notify-tracing-started" in command


def test_committed_baseline_has_every_required_section() -> None:
    root = Path(__file__).resolve().parents[2]
    report = json.loads((root / "benchmarks/reports/baseline.json").read_text())
    manifest = json.loads((root / "benchmarks/manifest.json").read_text())
    benchmark_run.validate_manifest(manifest)
    benchmark_run.validate_report(report, manifest["required_sections"])
    assert report["profile"] == "full"
    benchmark_run.ensure_baseline_recordable(report)
    committed_markdown = (
        root / "benchmarks/reports/baseline.md"
    ).read_text(encoding="utf-8")
    assert committed_markdown == benchmark_run.markdown(report)
