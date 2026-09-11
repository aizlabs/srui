from __future__ import annotations

import json
from pathlib import Path
from types import SimpleNamespace
from typing import Any

import pytest

from benchmarks import run as benchmark_run
from benchmarks.errors import (
    BenchmarkError,
    CandidateCleanupAssertionError,
)
from benchmarks.process_control import ManagedCommandError, ManagedCommandTimeout
from benchmarks.tests.support import (
    payload_for_driver,
    valid_manifest,
    valid_report,
)


def _main_fixture(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> Path:
    """Stub every driver and probe so `main` exercises only report assembly."""

    manifest = valid_manifest()
    fixture = tmp_path / manifest["fixture"]
    fixture.parent.mkdir(parents=True)
    fixture.write_text("{}", encoding="utf-8")
    manifest_path = tmp_path / "benchmarks/manifest.json"
    manifest_path.write_text(json.dumps(manifest), encoding="utf-8")

    def fake_driver(
        driver: dict[str, Any],
        _fixture: Path,
        profile: str,
        _timeout: int,
    ) -> dict[str, Any]:
        return payload_for_driver(driver, profile=profile)

    monkeypatch.setattr(benchmark_run, "ROOT", tmp_path)
    monkeypatch.setattr(benchmark_run, "MANIFEST", manifest_path)
    monkeypatch.setattr(benchmark_run.sys, "platform", "darwin")
    monkeypatch.setattr(benchmark_run, "run_driver", fake_driver)
    monkeypatch.setattr(
        benchmark_run,
        "ensure_free_space",
        lambda _path: None,
    )
    monkeypatch.setattr(
        benchmark_run,
        "benchmark_environment",
        lambda: valid_report()["environment"],
    )
    return tmp_path / "results"


def test_fail_row_preserves_runner_count_for_failed_report(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    output = (
        "SRUI §32 conformance — both\n"
        " 8  reconnect                "
        "FAIL    9 runner(s); failed: swift\n"
        "0 passed, 1 failed, 0 documented gap(s), 0 not applicable\n"
    )
    monkeypatch.setattr(
        benchmark_run,
        "run_managed_command",
        lambda *_args, **_kwargs: SimpleNamespace(
            returncode=1,
            stdout=output,
            stderr="",
            child_pid=123,
        ),
    )
    monkeypatch.setattr(
        benchmark_run,
        "ensure_free_space",
        lambda _path: None,
    )
    _elapsed, passed, detail, sample_count = (
        benchmark_run.run_verification(
            valid_manifest()["verification_commands"][0],
            default_timeout=1,
        )
    )
    assert passed is False
    assert sample_count == 9
    assert "exit 1" in detail


def test_main_writes_failed_report_when_production_conformance_executed(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    output_dir = _main_fixture(monkeypatch, tmp_path)
    monkeypatch.setattr(
        benchmark_run,
        "run_candidate_cleanup_probe",
        lambda *_args: "reaped",
    )
    monkeypatch.setattr(
        benchmark_run,
        "run_verification",
        lambda *_args: (
            1.0,
            False,
            "exit 1; production runner failed",
            9,
        ),
    )

    result = benchmark_run.main(
        [
            "--profile",
            "smoke",
            "--output-dir",
            str(output_dir),
        ]
    )
    assert result == 1
    report = json.loads(
        (output_dir / "latest.json").read_text(
            encoding="utf-8"
        )
    )
    section = next(
        item
        for item in report["sections"]
        if item["id"] == "31.5"
    )
    assertion = next(
        item
        for item in section["assertions"]
        if item["id"] == "production_reconnect_suite"
    )
    assert assertion["passed"] is False
    assert (
        section["sample_counts"][
            "runner.production_conformance"
        ]
        == 9
    )


def test_leaked_candidate_child_is_recorded_instead_of_aborting(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    output_dir = _main_fixture(monkeypatch, tmp_path)

    def leaked(*_args: Any) -> str:
        raise CandidateCleanupAssertionError(
            "forced-identity-failure renderer candidate group survived"
        )

    monkeypatch.setattr(
        benchmark_run,
        "run_candidate_cleanup_probe",
        leaked,
    )
    monkeypatch.setattr(
        benchmark_run,
        "run_verification",
        lambda *_args: (1.0, True, "exit 0", 9),
    )

    result = benchmark_run.main(
        [
            "--profile",
            "smoke",
            "--output-dir",
            str(output_dir),
        ]
    )
    assert result == 1
    report = json.loads(
        (output_dir / "latest.json").read_text(encoding="utf-8")
    )
    section = next(
        item for item in report["sections"] if item["id"] == "31.1"
    )
    assertion = next(
        item
        for item in section["assertions"]
        if item["id"] == "candidate_failure_cleanup"
    )
    assert assertion["passed"] is False
    assert "survived" in assertion["detail"]


def test_cleanup_probe_infrastructure_fault_still_aborts(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    output_dir = _main_fixture(monkeypatch, tmp_path)

    def missing_driver(*_args: Any) -> str:
        raise BenchmarkError(
            "release BenchmarkDriver is missing before candidate cleanup probe"
        )

    monkeypatch.setattr(
        benchmark_run,
        "run_candidate_cleanup_probe",
        missing_driver,
    )

    with pytest.raises(BenchmarkError, match="release BenchmarkDriver is missing"):
        benchmark_run.main(
            [
                "--profile",
                "smoke",
                "--output-dir",
                str(output_dir),
            ]
        )


def test_missing_runner_count_aborts_with_the_runner_diagnostic(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    output_dir = _main_fixture(monkeypatch, tmp_path)
    monkeypatch.setattr(
        benchmark_run,
        "run_candidate_cleanup_probe",
        lambda *_args: "reaped",
    )
    monkeypatch.setattr(
        benchmark_run,
        "run_verification",
        lambda *_args: (1.0, False, "timed out after 600s", None),
    )

    with pytest.raises(BenchmarkError, match="timed out after 600s"):
        benchmark_run.main(
            [
                "--profile",
                "smoke",
                "--output-dir",
                str(output_dir),
            ]
        )


@pytest.mark.parametrize(
    ("failure", "expected"),
    (
        (
            ManagedCommandTimeout("expired"),
            "candidate cleanup probe timed out after 7s",
        ),
        (
            ManagedCommandError("sentinel broke"),
            "candidate cleanup probe supervision failed: sentinel broke",
        ),
    ),
)
def test_cleanup_probe_classifies_infrastructure_failures(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    failure: BaseException,
    expected: str,
) -> None:
    binary = tmp_path / "BenchmarkDriver"
    binary.touch()
    monkeypatch.setattr(
        benchmark_run,
        "_macos_release_driver_path",
        lambda: binary,
    )
    monkeypatch.setattr(
        benchmark_run,
        "ensure_free_space",
        lambda _path: None,
    )

    def fail_supervision(*_args: Any, **_kwargs: Any) -> Any:
        raise failure

    monkeypatch.setattr(
        benchmark_run,
        "run_managed_command",
        fail_supervision,
    )

    with pytest.raises(BenchmarkError, match=expected) as caught:
        benchmark_run.run_candidate_cleanup_probe(
            tmp_path / "fixture.json",
            timeout=7,
        )
    assert not isinstance(
        caught.value,
        CandidateCleanupAssertionError,
    )


def test_cleanup_probe_classifies_surviving_identity_as_assertion(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    binary = tmp_path / "BenchmarkDriver"
    binary.touch()
    monkeypatch.setattr(
        benchmark_run,
        "_macos_release_driver_path",
        lambda: binary,
    )
    monkeypatch.setattr(
        benchmark_run,
        "ensure_free_space",
        lambda _path: None,
    )

    def fail_after_recording_identities(
        command: list[str],
        **_kwargs: Any,
    ) -> Any:
        prefixes = (
            "SRUI_BENCHMARK_CANDIDATE_IDENTITY_PATH=",
            "SRUI_BENCHMARK_CANDIDATE_DESCENDANT_IDENTITY_PATH=",
        )
        for index, prefix in enumerate(prefixes, start=1):
            path = Path(
                next(
                    argument.removeprefix(prefix)
                    for argument in command
                    if argument.startswith(prefix)
                )
            )
            path.write_text(
                json.dumps(
                    {
                        "pid": 100 + index,
                        "birth_unix_ns": 1_000 + index,
                    }
                ),
                encoding="utf-8",
            )
        return SimpleNamespace(
            returncode=1,
            stdout="",
            stderr=("renderer candidate srui birth identity was unavailable"),
            child_pid=99,
        )

    def identity_survived(
        _identities: list[tuple[int, int]],
        *,
        label: str,
    ) -> None:
        raise ManagedCommandError(
            f"{label} still has original process identities alive"
        )

    monkeypatch.setattr(
        benchmark_run,
        "run_managed_command",
        fail_after_recording_identities,
    )
    monkeypatch.setattr(
        benchmark_run,
        "wait_for_process_identities_gone",
        identity_survived,
    )

    with pytest.raises(
        CandidateCleanupAssertionError,
        match="still has original process identities alive",
    ):
        benchmark_run.run_candidate_cleanup_probe(
            tmp_path / "fixture.json",
            timeout=7,
        )


@pytest.mark.parametrize(
    ("failure", "expected"),
    (
        (ManagedCommandTimeout("expired"), "timed out after 7s"),
        (
            ManagedCommandError("sentinel broke"),
            "process supervision failed: sentinel broke",
        ),
    ),
)
def test_verification_retains_infrastructure_diagnostic(
    monkeypatch: pytest.MonkeyPatch,
    failure: BaseException,
    expected: str,
) -> None:
    monkeypatch.setattr(
        benchmark_run,
        "ensure_free_space",
        lambda _path: None,
    )

    def fail_supervision(*_args: Any, **_kwargs: Any) -> Any:
        raise failure

    monkeypatch.setattr(
        benchmark_run,
        "run_managed_command",
        fail_supervision,
    )
    spec = valid_manifest()["verification_commands"][0].copy()
    spec["timeout"] = 7

    _elapsed, passed, detail, sample_count = benchmark_run.run_verification(
        spec, default_timeout=99
    )

    assert passed is False
    assert sample_count is None
    assert detail == expected
