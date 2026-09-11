from __future__ import annotations

import subprocess
from pathlib import Path
from types import SimpleNamespace
from typing import Any

import pytest

from benchmarks import run as benchmark_run
from benchmarks.tests.support import (
    payload_for_driver,
    section,
    valid_manifest,
    valid_report,
)


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
    assert (
        f"- Metric contract: schema 1 / sha256 {benchmark_run.CONTRACT_SHA256}"
        in rendered
    )
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


def test_full_report_calls_out_performance_followup() -> None:
    report = valid_report()
    report["sections"][2]["metrics"][0].update(
        {"value": 2.1, "target": 1.0, "target_direction": "max"}
    )
    rendered = benchmark_run.markdown(report)
    assert "PERFORMANCE FOLLOW-UP (>2x)" in rendered
    assert "WARNING >2x" in rendered
    assert "SMOKE DIAGNOSTIC" not in rendered


def test_smoke_report_labels_target_comparisons_as_diagnostic() -> None:
    report = valid_report()
    report["profile"] = "smoke"
    report["sections"][2]["metrics"][0].update(
        {"value": 2.1, "target": 1.0, "target_direction": "max"}
    )
    rendered = benchmark_run.markdown(report)
    assert "Evidence class: diagnostic only" in rendered
    assert "Diagnostic reference" in rendered
    assert "DIAGNOSTIC >2x" in rendered
    assert "SMOKE DIAGNOSTIC (>2x; not §23 evidence)" in rendered
    assert "PERFORMANCE FOLLOW-UP" not in rendered
    assert "WARNING >2x" not in rendered


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
    tmp_path: Path,
) -> None:
    started: list[str] = []
    output_dir = tmp_path / "partial-linux-report"

    def unexpected_driver(*_args: Any, **_kwargs: Any) -> dict[str, Any]:
        started.append("driver")
        raise AssertionError("unsupported-host main started a benchmark driver")

    monkeypatch.setattr(benchmark_run.sys, "platform", "linux")
    monkeypatch.setattr(benchmark_run, "run_driver", unexpected_driver)

    with pytest.raises(
        benchmark_run.BenchmarkError,
        match="complete six-section benchmark cannot run on linux.*no drivers were started",
    ):
        benchmark_run.main(["--output-dir", str(output_dir)])

    assert started == []
    assert not output_dir.exists()




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


def test_macos_attribution_interval_count_matches_render_samples() -> None:
    driver = next(
        item for item in valid_manifest()["drivers"] if item["name"] == "macos"
    )
    payload = payload_for_driver(driver, profile="full")
    attribution = payload["artifacts"]["renderer_process_attribution"][0]
    assert attribution["candidate"] == "srui"
    attribution["measurement_intervals"].pop()

    with pytest.raises(
        benchmark_run.BenchmarkError,
        match=(
            r"srui measurement interval count must equal "
            r"macos\.srui\.render sample count 20; got 19"
        ),
    ):
        benchmark_run.validate_driver_output(payload, driver, profile="full")


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
