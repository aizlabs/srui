from __future__ import annotations

import json
from pathlib import Path
from types import SimpleNamespace
from typing import Any

import pytest

from benchmarks import run as benchmark_run
from benchmarks import validation as benchmark_validation
from benchmarks.tests.support import (
    payload_for_driver,
    valid_manifest,
    valid_report,
    window_isolation_assertion,
)
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
    binary = tmp_path / "client-macos/Benchmarks/.build/release/BenchmarkDriver"
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
        benchmark_validation,
        "wait_for_process_identities_gone",
        observe,
    )
    benchmark_run.run_driver(driver, tmp_path / "fixture.json", "smoke", 1)
    assert sorted(observed) == [(43, 1_001), (44, 1_002), (45, 1_003)]


def test_candidate_cleanup_probe_waits_for_exact_identity_and_removes_tempdir(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    binary = tmp_path / "client-macos/Benchmarks/.build/release/BenchmarkDriver"
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
