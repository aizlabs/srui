from __future__ import annotations

import importlib.util
import json
import os
import signal
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
        "metrics": [metric()],
        "assertions": [{"id": "correct", "name": "correct", "passed": True}],
        "notes": [],
    }


def artifact(digest: str = "a" * 64) -> dict[str, Any]:
    return {
        "canonical_transaction_sha256": digest,
        "canonical_transaction_bytes": 42,
    }


def valid_report() -> dict[str, Any]:
    sections: dict[str, dict[str, Any]] = {}
    artifacts: dict[str, dict[str, Any]] = {}
    for driver in valid_manifest()["drivers"]:
        payload = payload_for_driver(driver)
        artifacts[driver["name"]] = payload["artifacts"]
        for emitted in payload["sections"]:
            section_id = emitted["id"]
            if section_id in sections:
                sections[section_id]["metrics"].extend(emitted["metrics"])
                sections[section_id]["assertions"].extend(emitted["assertions"])
            else:
                sections[section_id] = emitted
    benchmark_run.append_parity_assertion(sections, artifacts)
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
    return {
        "schema_version": 1,
        "generated_at": "2026-01-01T00:00:00Z",
        "profile": "full",
        "fixture": "benchmarks/fixtures/test.json",
        "environment": {
            "platform": "test",
            "machine": "test",
            "python": "3.14",
        },
        "driver_artifacts": artifacts,
        "sections": list(sections.values()),
    }


def valid_manifest() -> dict[str, Any]:
    return {
        "schema_version": 1,
        "fixture": "benchmarks/fixtures/test.json",
        "required_sections": list(benchmark_run.EXPECTED_SECTIONS),
        "drivers": [
            {
                "name": "rust",
                "sections": ["31.2", "31.5", "31.6"],
                "command": ["rust-driver"],
            },
            {
                "name": "macos",
                "sections": ["31.1", "31.3", "31.4", "31.5", "31.6"],
                "platform": "darwin",
                "command": ["macos-driver"],
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


def test_manifest_enforces_driver_declarations() -> None:
    manifest = valid_manifest()
    benchmark_run.validate_manifest(manifest)

    manifest["drivers"][0]["sections"].remove("31.5")
    with pytest.raises(benchmark_run.BenchmarkError, match="rust driver sections"):
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

    _elapsed, passed, detail = benchmark_run.run_verification(
        valid_manifest()["verification_commands"][0],
        default_timeout=1,
    )
    assert passed is False
    assert "missing required output" in detail


def payload_for_driver(driver: dict[str, Any]) -> dict[str, Any]:
    expected = benchmark_run.EXPECTED_DRIVER_INVENTORY[driver["name"]]
    frame_budget = 1000.0 / 120.0
    sections = []
    for section_id in driver["sections"]:
        inventory = expected[section_id]
        sections.append(
            {
                "id": section_id,
                "name": f"Section {section_id}",
                "metrics": [
                    {
                        "id": metric_id,
                        "name": metric_id,
                        "value": (
                            frame_budget
                            if (metric_id, statistic)
                            == (benchmark_run.LOCAL_FRAME_BUDGET_ID, "exact")
                            else 1.0
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
                "started_unix_ns": 1,
                "ended_unix_ns": 2,
                "helper_pid_source": "no helper processes",
                "process_identities": [
                    {"pid": 43, "birth_unix_ns": 1_001},
                ],
            },
            {
                "candidate": "webkit",
                "driver_pid": 42,
                "host_pid": 44,
                "helper_pids": [45],
                "started_unix_ns": 3,
                "ended_unix_ns": 4,
                "helper_pid_source": "WKWebView diagnostic process identifiers",
                "process_identities": [
                    {"pid": 44, "birth_unix_ns": 1_002},
                    {"pid": 45, "birth_unix_ns": 1_003},
                ],
            },
        ]
    return {"artifacts": artifacts, "sections": sections}


@pytest.mark.parametrize("driver_name", ["rust", "macos"])
def test_driver_inventory_accepts_only_complete_declared_measurements(
    driver_name: str,
) -> None:
    driver = next(
        item for item in valid_manifest()["drivers"] if item["name"] == driver_name
    )
    payload = payload_for_driver(driver)
    benchmark_run.validate_driver_output(payload, driver)

    payload["sections"][0]["metrics"].pop()
    with pytest.raises(benchmark_run.BenchmarkError, match="metric inventory"):
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


def test_dynamic_local_frame_budget_controls_every_local_p50_target() -> None:
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


def test_run_driver_waits_for_exact_attributed_process_identities(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    driver = next(
        item for item in valid_manifest()["drivers"] if item["name"] == "macos"
    )
    payload = payload_for_driver(driver)
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
    assert "exact bytes" in sections["31.2"]["assertions"][-1]["detail"]


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
        "run_verification",
        lambda _spec, _timeout: (1.0, True, "exit 0"),
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

    cleanup_errors = benchmark_xctrace.cleanup_capture(
        driver=None,
        recorder=None,
        watcher=PersistentWatcher(),
        trace=tmp_path / "capture.trace",
        sidecar=tmp_path / "capture.trace.summary.json",
        retain_outputs=False,
    )
    assert len(cleanup_errors) == 1
    assert isinstance(cleanup_errors[0], BaseExceptionGroup)

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


def test_xctrace_profiles_all_processes() -> None:
    command = benchmark_xctrace.trace_command(Path("out.trace"), "notification")
    assert "--all-processes" in command
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
