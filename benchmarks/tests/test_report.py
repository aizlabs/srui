from __future__ import annotations

import importlib.util
import json
import signal
import sys
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
    return {"name": name, "value": 1.0, "unit": "ms", "statistic": "p50"}


def section(section_id: str) -> dict[str, Any]:
    return {
        "id": section_id,
        "name": f"Section {section_id}",
        "metrics": [metric()],
        "assertions": [{"name": "correct", "passed": True}],
        "notes": [],
    }


def artifact(digest: str = "a" * 64) -> dict[str, Any]:
    return {
        "canonical_transaction_sha256": digest,
        "canonical_transaction_bytes": 42,
    }


def valid_report() -> dict[str, Any]:
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
        "driver_artifacts": {"rust": artifact(), "macos": artifact()},
        "sections": [section(section_id) for section_id in benchmark_run.EXPECTED_SECTIONS],
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
    benchmark_run.append_parity_assertion(
        sections,
        {"rust": artifact(), "macos": artifact()},
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
        emitted = []
        for section_id in driver["sections"]:
            item = section(section_id)
            item["metrics"][0]["name"] = f"{driver['name']} measurement"
            if driver["name"] == "rust" and section_id == "31.2":
                item["assertions"][0]["passed"] = False
            emitted.append(item)
        return {"artifacts": artifact(), "sections": emitted}

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
