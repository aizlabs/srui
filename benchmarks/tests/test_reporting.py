from __future__ import annotations

import json
import signal
import threading
from pathlib import Path
from typing import Any

import pytest

from benchmarks import reporting as benchmark_reporting
from benchmarks import run as benchmark_run
from benchmarks.tests.support import (
    artifact,
    payload_for_driver,
    section,
    valid_manifest,
    valid_report,
    window_isolation_assertion,
)
def test_driver_schema_requires_stable_measurement_ids() -> None:
    driver = valid_manifest()["drivers"][0]
    payload = payload_for_driver(driver)
    del payload["sections"][0]["metrics"][0]["id"]
    with pytest.raises(benchmark_run.BenchmarkError, match="schema violation"):
        benchmark_run.validate_driver_output(payload, driver)


def test_driver_output_must_match_declared_sections() -> None:
    driver = valid_manifest()["drivers"][0]
    payload = payload_for_driver(driver)
    payload["sections"] = [
        item for item in payload["sections"] if item["id"] != "31.5"
    ]
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
    monkeypatch.setattr(benchmark_run.sys, "platform", "darwin")
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
    original_replace = benchmark_reporting.os.replace
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

    monkeypatch.setattr(
        benchmark_reporting.os,
        "replace",
        fail_markdown_install,
    )
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
    original_replace = benchmark_reporting.os.replace
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
        benchmark_reporting.os,
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
def test_signal_handler_raises_a_cleanup_safe_exception() -> None:
    with pytest.raises(benchmark_run.TerminationRequested) as error:
        benchmark_run._raise_termination(signal.SIGTERM, None)
    assert error.value.signum == signal.SIGTERM
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
