"""Behavioural tests for scripts/run-conformance itself.

The runner is the thing that decides whether §32 is satisfied, so its accounting rules need
their own coverage: a bug here reports a green conformance table over missing work.

Each test drives the real script against a temporary manifest, so the assertions are about
observable output and exit status rather than internals.
"""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parent.parent.parent
RUNNER = REPO_ROOT / "scripts/run-conformance"
MANIFEST = REPO_ROOT / "protocol/conformance-vectors/suites/manifest.json"

TRUE = [sys.executable, "-c", "raise SystemExit(0)"]
FALSE = [sys.executable, "-c", "raise SystemExit(1)"]
# What `swift test --filter` does when its regex matches nothing: warn, run nothing, exit 0.
NO_TESTS = [
    sys.executable,
    "-c",
    "print('warning: No matching test cases were run'); raise SystemExit(0)",
]


@pytest.fixture
def manifest_backup():
    """Restores the real manifest after a test rewrites it."""
    original = MANIFEST.read_bytes()
    yield
    MANIFEST.write_bytes(original)


def write_manifest(suites: list[dict]) -> None:
    MANIFEST.write_text(
        json.dumps({"version": 1, "spec_section": "§32", "suites": suites}, indent=2) + "\n",
        encoding="utf-8",
    )


def suite(suite_id: int, **overrides) -> dict:
    base = {
        "id": suite_id,
        "slug": f"suite-{suite_id}",
        "name": f"Suite {suite_id}",
        "spec_sections": [f"§32.{suite_id}"],
        "status": "active",
        "rust": [TRUE],
        "swift": [TRUE],
    }
    base.update(overrides)
    return base


def run(*args: str) -> subprocess.CompletedProcess:
    return subprocess.run(
        [sys.executable, str(RUNNER), *args],
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
        check=False,
    )


def test_all_green_manifest_exits_zero(manifest_backup) -> None:
    write_manifest([suite(i) for i in range(1, 13)])
    result = run()
    assert result.returncode == 0, result.stdout + result.stderr
    assert "12 passed" in result.stdout


def test_undeclared_missing_runner_exits_nonzero(manifest_backup) -> None:
    """One language must not silently cover for the other's absence."""
    suites = [suite(i) for i in range(1, 13)]
    del suites[9]["rust"]  # suite 10 loses its Rust half with no explanation
    write_manifest(suites)

    result = run()
    assert result.returncode == 1
    assert "no rust runner declared" in result.stdout
    assert "FAIL" in result.stdout


def test_documented_not_applicable_reports_na_and_exits_zero(manifest_backup) -> None:
    suites = [suite(i) for i in range(1, 13)]
    del suites[9]["rust"]
    suites[9]["not_applicable"] = {"rust": "AppKit-only; cannot build on Linux."}
    write_manifest(suites)

    result = run("--implementation", "rust", "--suite", "10")
    assert result.returncode == 0, result.stdout + result.stderr
    assert "N/A" in result.stdout
    assert "AppKit-only" in result.stdout, "an N/A must print the documented reason"


def test_a_both_run_cannot_hide_a_missing_language(manifest_backup) -> None:
    """The regression this rule exists for: Swift passing while Rust was never run."""
    suites = [suite(i) for i in range(1, 13)]
    del suites[3]["rust"]  # suite 4 has only a Swift runner, undeclared
    write_manifest(suites)

    result = run("--implementation", "both")
    assert result.returncode == 1, "a `both` run must not pass when a language is unaccounted for"
    assert "no rust runner declared" in result.stdout


def test_failing_runner_is_reported_without_fail_fast(manifest_backup) -> None:
    suites = [suite(i) for i in range(1, 13)]
    suites[0]["rust"] = [FALSE]
    suites[11]["swift"] = [FALSE]
    write_manifest(suites)

    result = run()
    assert result.returncode == 1
    # Both failures reported: one failing suite must not hide another.
    assert result.stdout.count("FAIL") >= 2


def test_runner_that_executes_no_tests_is_a_failure(manifest_backup) -> None:
    """A filter matching nothing exits 0, so a renamed test would silently turn a suite green."""
    suites = [suite(i) for i in range(1, 13)]
    suites[3]["swift"] = [NO_TESTS]
    write_manifest(suites)

    result = run()
    assert result.returncode == 1, "a suite that ran no tests must not report PASS"
    assert "executed no tests" in result.stdout


def test_open_gap_reports_gap_not_pass(manifest_backup) -> None:
    suites = [suite(i) for i in range(1, 13)]
    suites[4]["gaps"] = [
        {
            "scenario": "something required is unproven",
            "reason": "because",
            "future_task": "Task 99",
            "gap_probe": {
                "file": "protocol/registry.yaml",
                "absent_pattern": "name: DefinitelyNotARealNodeType",
            },
        }
    ]
    write_manifest(suites)

    result = run()
    assert result.returncode == 0, "an open, documented gap is not a failure"
    assert "GAP" in result.stdout
    assert "11 passed" in result.stdout and "1 documented gap" in result.stdout


def test_closed_gap_exits_nonzero(manifest_backup) -> None:
    """A gap that has been fixed must force a manifest update, not keep printing GAP."""
    suites = [suite(i) for i in range(1, 13)]
    suites[4]["gaps"] = [
        {
            "scenario": "already fixed",
            "reason": "because",
            "future_task": "Task 99",
            "gap_probe": {
                "file": "protocol/registry.yaml",
                "absent_pattern": "node_types:",  # certain to match
            },
        }
    ]
    write_manifest(suites)

    result = run()
    assert result.returncode == 1
    assert "gap appears closed" in result.stdout


def test_gap_without_probe_is_rejected(manifest_backup) -> None:
    suites = [suite(i) for i in range(1, 13)]
    suites[4]["gaps"] = [{"scenario": "s", "reason": "r", "future_task": "t"}]
    write_manifest(suites)

    result = run("--list")
    assert result.returncode == 1
    assert "gap_probe" in (result.stdout + result.stderr)


@pytest.mark.parametrize("count", [11, 13])
def test_manifest_must_declare_exactly_twelve_suites(manifest_backup, count: int) -> None:
    write_manifest([suite(i) for i in range(1, count + 1)])
    result = run("--list")
    assert result.returncode == 1
    assert "exactly 12 suites" in (result.stdout + result.stderr)


def test_unknown_suite_selector_exits_nonzero() -> None:
    result = run("--suite", "definitely-not-a-suite")
    assert result.returncode == 1


def test_list_runs_nothing_and_reports_every_suite() -> None:
    result = run("--list")
    assert result.returncode == 0
    for suite_id in range(1, 13):
        assert f"{suite_id:>2}  " in result.stdout
