#!/usr/bin/env python3
"""Run SRUI's layered §31 benchmark suite and write JSON plus Markdown reports."""

from __future__ import annotations

import argparse
import contextlib
import datetime as dt
import json
import platform
import re
import signal
import subprocess
import sys
import tempfile
import time
from collections.abc import Iterator
from pathlib import Path
from typing import Any

if __package__ in (None, ""):
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))

from benchmarks.contract import (
    CANONICAL_FIXTURE as CANONICAL_FIXTURE,
    CONTRACT,
    CONTRACT_SHA256,
    DISTRIBUTION as DISTRIBUTION,
    EXPECTED_DRIVER_COMMANDS as EXPECTED_DRIVER_COMMANDS,
    EXPECTED_DRIVER_INVENTORY as EXPECTED_DRIVER_INVENTORY,
    EXPECTED_DRIVER_SECTIONS as EXPECTED_DRIVER_SECTIONS,
    EXPECTED_METRIC_DISPLAY_NAMES as EXPECTED_METRIC_DISPLAY_NAMES,
    EXPECTED_RECONNECT_VERIFICATION as EXPECTED_RECONNECT_VERIFICATION,
    EXPECTED_REPORT_INVENTORY as EXPECTED_REPORT_INVENTORY,
    EXPECTED_SECTIONS as EXPECTED_SECTIONS,
    LOCAL_FRAME_BUDGET_ID as LOCAL_FRAME_BUDGET_ID,
    LOCAL_INTERACTIONS as LOCAL_INTERACTIONS,
    PROFILE_DRIVER_ITERATIONS as PROFILE_DRIVER_ITERATIONS,
    PROFILED_METRIC_DISPLAY_NAMES as PROFILED_METRIC_DISPLAY_NAMES,
    PRODUCTION_CONFORMANCE_SAMPLE_COUNT as PRODUCTION_CONFORMANCE_SAMPLE_COUNT,
    SIGNED_METRIC_IDS as SIGNED_METRIC_IDS,
    WINDOW_ISOLATION_ASSERTION_ID,
    contract_sha256 as contract_sha256,
    expected_driver_sample_counts as expected_driver_sample_counts,
    expected_metric_display_name as expected_metric_display_name,
    expected_report_inventory_for_profile as expected_report_inventory_for_profile,
    expected_report_sample_counts as expected_report_sample_counts,
    percentile as percentile,
)
from benchmarks.errors import BenchmarkError
from benchmarks.process_control import (
    ManagedCommandError,
    ManagedCommandTimeout,
    non_termination_exceptions,
    run_managed_command,
    termination_exceptions,
    wait_for_process_identities_gone,
)
from benchmarks.reporting import (
    append_parity_assertion,
    ensure_baseline_recordable,
    ensure_baseline_start_clean,
    failed_assertions,
    markdown as markdown,
    over_2x as over_2x,
    write_report,
)
from benchmarks.validation import (
    SCHEMA as SCHEMA,
    _merge_driver_section,
    configured_byte_limit as configured_byte_limit,
    ensure_free_space,
    parse_window_isolation_self_test_output,
    renderer_process_identities as renderer_process_identities,
    schema_bundle as schema_bundle,
    validate_document as validate_document,
    validate_driver_output,
    validate_manifest,
    validate_renderer_process_attribution as validate_renderer_process_attribution,
    validate_report,
    validate_runtime_platforms,
    wait_for_renderer_processes_to_exit,
)
ROOT = Path(__file__).resolve().parent.parent
MANIFEST = ROOT / "benchmarks/manifest.json"
METADATA_COMMAND_TIMEOUT_SECONDS = 10
WINDOW_ISOLATION_SELF_TEST_TIMEOUT_SECONDS = 30
WINDOW_ISOLATION_SELF_TEST_PREFIX = "window isolation self-test passed:"
WINDOW_ISOLATION_SELF_TEST_PATTERN = re.compile(
    r"^window isolation self-test passed: "
    r"dock=(?P<dock>[0-9]+) status=(?P<status>[0-9]+) "
    r"ahead=(?P<ahead>[0-9]+) popup=(?P<popup>[0-9]+) "
    r"target=(?P<target>[0-9]+) occluder=(?P<occluder>[0-9]+)$"
)
DEFAULT_MIN_FREE_BYTES = 12 * 1024 * 1024 * 1024
DEFAULT_DRIVER_TIMEOUT_SECONDS = {
    "smoke": 600,
    # Full §31.4 executes 640 local-interaction probes, including deliberately
    # held 100/300/600 ms responses, in addition to the other five sections.
    # Keep the complete run bounded without treating its valid workload as a hang.
    "full": 1_800,
}


def effective_driver_timeout(profile: str, requested: int | None) -> int:
    return requested if requested is not None else DEFAULT_DRIVER_TIMEOUT_SECONDS[profile]


def _macos_release_driver_path() -> Path:
    command = EXPECTED_DRIVER_COMMANDS["macos"]
    package_path = command[command.index("--package-path") + 1]
    return ROOT / package_path / ".build/release/BenchmarkDriver"


def _checked_metadata_command(
    command: list[str],
    *,
    runner: Any = subprocess.run,
    allow_empty: bool = False,
) -> str:
    try:
        result = runner(
            command,
            cwd=ROOT,
            capture_output=True,
            text=True,
            check=True,
            timeout=METADATA_COMMAND_TIMEOUT_SECONDS,
        )
    except (OSError, subprocess.SubprocessError) as error:
        raise BenchmarkError(
            f"environment metadata command failed: {' '.join(command)}"
        ) from error
    stdout = (result.stdout or "").strip()
    stderr = (result.stderr or "").strip()
    output = "\n".join(part for part in (stdout, stderr) if part)
    if not output and not allow_empty:
        raise BenchmarkError(
            f"environment metadata command returned no data: {' '.join(command)}"
        )
    return output


def benchmark_environment(*, runner: Any = subprocess.run) -> dict[str, Any]:
    platform_name = platform.platform()
    machine = platform.machine()
    python = platform.python_version()
    if not platform_name or not machine or not python:
        raise BenchmarkError("Python platform metadata is incomplete")

    chip = _checked_metadata_command(
        ["/usr/sbin/sysctl", "-n", "machdep.cpu.brand_string"],
        runner=runner,
    )
    physical_ram_text = _checked_metadata_command(
        ["/usr/sbin/sysctl", "-n", "hw.memsize"],
        runner=runner,
    )
    try:
        physical_ram_bytes = int(physical_ram_text)
    except ValueError as error:
        raise BenchmarkError(
            f"physical RAM metadata is not an integer: {physical_ram_text!r}"
        ) from error
    if physical_ram_bytes <= 0:
        raise BenchmarkError("physical RAM metadata must be positive")

    git_commit = _checked_metadata_command(
        ["git", "rev-parse", "HEAD"],
        runner=runner,
    )
    if re.fullmatch(r"[0-9a-f]{40,64}", git_commit) is None:
        raise BenchmarkError(f"git commit metadata is invalid: {git_commit!r}")
    git_status = _checked_metadata_command(
        ["git", "status", "--porcelain=v1", "--untracked-files=normal"],
        runner=runner,
        allow_empty=True,
    )
    return {
        "platform": platform_name,
        "machine": machine,
        "python": python,
        "chip": chip,
        "physical_ram_bytes": physical_ram_bytes,
        "xcode": _checked_metadata_command(
            ["/usr/bin/xcodebuild", "-version"],
            runner=runner,
        ),
        "swift": _checked_metadata_command(
            ["/usr/bin/swift", "--version"],
            runner=runner,
        ),
        "rust": _checked_metadata_command(
            ["rustc", "--version"],
            runner=runner,
        ),
        "git_commit": git_commit,
        "git_dirty": bool(git_status),
    }


def exception_group_detail(error: BaseExceptionGroup) -> str:
    details: list[str] = []

    def collect(item: BaseException) -> None:
        if isinstance(item, BaseExceptionGroup):
            for nested in item.exceptions:
                collect(nested)
        else:
            details.append(f"{type(item).__name__}: {item}")

    collect(error)
    return "; ".join(details)


class TerminationRequested(BaseException):
    def __init__(self, signum: int) -> None:
        super().__init__(signum)
        self.signum = signum




def _positive_integer(value: Any) -> bool:
    return not isinstance(value, bool) and isinstance(value, int) and value > 0


def run_driver(
    driver: dict[str, Any],
    fixture: Path,
    profile: str,
    timeout: int,
) -> dict[str, Any]:
    with tempfile.NamedTemporaryFile(suffix=".json", delete=False) as output:
        output_path = Path(output.name)
    command = [
        *driver["command"],
        "--fixture",
        str(fixture),
        "--profile",
        profile,
        "--output",
        str(output_path),
    ]
    ensure_free_space(ROOT)
    try:
        try:
            result = run_managed_command(
                command,
                cwd=ROOT,
                timeout=timeout,
                label=f"{driver['name']} driver",
                poll_hook=lambda _process: ensure_free_space(ROOT),
            )
        except ManagedCommandTimeout as error:
            raise BenchmarkError(f"{driver['name']} timed out after {timeout}s") from error
        except ManagedCommandError as error:
            raise BenchmarkError(f"{driver['name']} process supervision failed: {error}") from error

        if result.returncode:
            detail = (result.stderr or result.stdout).strip()
            raise BenchmarkError(
                f"{driver['name']} failed ({result.returncode}): {detail[-4000:]}"
            )
        try:
            payload = json.loads(output_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            raise BenchmarkError(f"{driver['name']} did not write valid result JSON") from error
        validated = validate_driver_output(
            payload,
            driver,
            profile=profile,
            launched_pid=result.child_pid,
        )
        if driver["name"] == "macos":
            wait_for_renderer_processes_to_exit(
                validated["artifacts"],
                label="macos renderer candidates",
            )
        return validated
    finally:
        output_path.unlink(missing_ok=True)




def run_window_isolation_self_test(
    fixture: Path,
    timeout: int,
) -> dict[str, Any]:
    binary = _macos_release_driver_path()
    if not binary.is_file():
        raise BenchmarkError(
            "window isolation self-test requires the built release BenchmarkDriver"
        )
    with tempfile.NamedTemporaryFile(suffix=".json", delete=False) as output:
        output_path = Path(output.name)
    command = [
        "/usr/bin/env",
        "SRUI_BENCHMARK_PHASES=1",
        "SRUI_BENCHMARK_WINDOW_ISOLATION_SELF_TEST=1",
        str(binary),
        "--fixture",
        str(fixture),
        "--profile",
        "full",
        "--output",
        str(output_path),
    ]
    ensure_free_space(ROOT)
    try:
        try:
            result = run_managed_command(
                command,
                cwd=ROOT,
                timeout=timeout,
                label="window isolation self-test",
                poll_hook=lambda _process: ensure_free_space(ROOT),
            )
        except ManagedCommandTimeout as error:
            raise BenchmarkError(
                f"window isolation self-test timed out after {timeout}s"
            ) from error
        except ManagedCommandError as error:
            raise BenchmarkError(
                f"window isolation self-test process supervision failed: {error}"
            ) from error
        if result.returncode:
            detail = (result.stderr or result.stdout).strip()
            raise BenchmarkError(
                "window isolation self-test failed "
                f"({result.returncode}): {detail[-4000:]}"
            )
        evidence = parse_window_isolation_self_test_output(
            result.stdout,
            result.stderr,
        )
        detail = (
            f"window isolation self-test passed: dock={evidence['dock']} "
            f"status={evidence['status']} ahead={evidence['ahead']} "
            f"popup={evidence['popup']} target={evidence['target']} "
            f"occluder={evidence['occluder']}"
        )
        return {
            "id": WINDOW_ISOLATION_ASSERTION_ID,
            "name": "WindowServer isolation rejects an exact synthetic occluder",
            "passed": True,
            "detail": detail,
        }
    finally:
        output_path.unlink(missing_ok=True)


def run_candidate_cleanup_probe(
    fixture: Path,
    timeout: int,
) -> str:
    binary = _macos_release_driver_path()
    if not binary.is_file():
        raise BenchmarkError(
            "release BenchmarkDriver is missing before candidate cleanup probe"
        )
    ensure_free_space(ROOT)
    with tempfile.TemporaryDirectory(prefix="srui-candidate-cleanup-") as directory:
        probe_directory = Path(directory)
        identity_path = probe_directory / "candidate-identity.json"
        descendant_identity_path = probe_directory / "descendant-identity.json"
        output_path = probe_directory / "driver.json"
        command = [
            "/usr/bin/env",
            "SRUI_BENCHMARK_FORCE_CANDIDATE_IDENTITY_FAILURE=1",
            f"SRUI_BENCHMARK_CANDIDATE_IDENTITY_PATH={identity_path}",
            (
                "SRUI_BENCHMARK_CANDIDATE_DESCENDANT_IDENTITY_PATH="
                f"{descendant_identity_path}"
            ),
            str(binary),
            "--fixture",
            str(fixture),
            "--profile",
            "smoke",
            "--only-section",
            "31.1",
            "--output",
            str(output_path),
        ]
        probe_timeout = min(timeout, 30)
        try:
            result = run_managed_command(
                command,
                cwd=ROOT,
                timeout=probe_timeout,
                label="renderer candidate cleanup fault probe",
                poll_hook=lambda _process: ensure_free_space(ROOT),
            )
        except ManagedCommandTimeout as error:
            raise BenchmarkError(
                f"candidate cleanup probe timed out after {probe_timeout}s"
            ) from error
        except ManagedCommandError as error:
            raise BenchmarkError(
                f"candidate cleanup probe supervision failed: {error}"
            ) from error
        combined = (result.stdout + "\n" + result.stderr).strip()
        if result.returncode == 0:
            raise BenchmarkError(
                "candidate cleanup probe unexpectedly succeeded"
            )
        if (
            "renderer candidate srui birth identity was unavailable"
            not in combined
            or "candidate cleanup failed" in combined
        ):
            raise BenchmarkError(
                "candidate cleanup probe did not exercise the expected "
                f"successful cleanup path: {combined[-2000:]}"
            )
        identities: list[tuple[int, int]] = []
        try:
            for label, path in (
                ("candidate", identity_path),
                ("descendant", descendant_identity_path),
            ):
                identity = json.loads(path.read_text(encoding="utf-8"))
                pid = identity["pid"]
                birth = identity["birth_unix_ns"]
                if not _positive_integer(pid) or not _positive_integer(birth):
                    raise BenchmarkError(
                        f"candidate cleanup probe {label} identity has invalid values"
                    )
                identities.append((pid, birth))
        except (OSError, json.JSONDecodeError, KeyError, TypeError) as error:
            raise BenchmarkError(
                "candidate cleanup probe did not record both valid identities"
            ) from error
        if len({pid for pid, _birth in identities}) != 2:
            raise BenchmarkError(
                "candidate cleanup probe identities do not name distinct processes"
            )
        try:
            wait_for_process_identities_gone(
                identities,
                label="forced-identity-failure renderer candidate group",
            )
        except ManagedCommandError as error:
            raise BenchmarkError(str(error)) from error

        publication_candidate_path = probe_directory / "publication-candidate.json"
        publication_observer_path = probe_directory / "publication-descendant.json"
        blocked_destination = probe_directory / "blocked-descendant-output"
        blocked_destination.mkdir()
        publication_command = [
            "/usr/bin/env",
            (
                "SRUI_BENCHMARK_CANDIDATE_IDENTITY_PATH="
                f"{publication_candidate_path}"
            ),
            (
                "SRUI_BENCHMARK_CANDIDATE_DESCENDANT_IDENTITY_PATH="
                f"{blocked_destination}"
            ),
            (
                "SRUI_BENCHMARK_CANDIDATE_DESCENDANT_OBSERVER_PATH="
                f"{publication_observer_path}"
            ),
            str(binary),
            "--fixture",
            str(fixture),
            "--profile",
            "smoke",
            "--only-section",
            "31.1",
            "--output",
            str(probe_directory / "publication-driver.json"),
        ]
        try:
            publication_result = run_managed_command(
                publication_command,
                cwd=ROOT,
                timeout=probe_timeout,
                label="renderer descendant-publication cleanup fault probe",
                poll_hook=lambda _process: ensure_free_space(ROOT),
            )
        except (ManagedCommandTimeout, ManagedCommandError) as error:
            raise BenchmarkError(
                "candidate descendant-publication cleanup probe failed supervision"
            ) from error
        publication_detail = (
            publication_result.stdout + "\n" + publication_result.stderr
        ).strip()
        if (
            publication_result.returncode == 0
            or "renderer candidate srui exited with status" not in publication_detail
            or "candidate cleanup failed" in publication_detail
        ):
            raise BenchmarkError(
                "candidate descendant-publication probe did not exercise the "
                f"expected cleanup path: {publication_detail[-2000:]}"
            )
        publication_identities: list[tuple[int, int]] = []
        try:
            for path in (publication_candidate_path, publication_observer_path):
                identity = json.loads(path.read_text(encoding="utf-8"))
                pid = identity["pid"]
                birth = identity["birth_unix_ns"]
                if not _positive_integer(pid) or not _positive_integer(birth):
                    raise BenchmarkError(
                        "descendant-publication probe identity has invalid values"
                    )
                publication_identities.append((pid, birth))
        except (OSError, json.JSONDecodeError, KeyError, TypeError) as error:
            raise BenchmarkError(
                "descendant-publication probe did not record both identities"
            ) from error
        if len({pid for pid, _birth in publication_identities}) != 2:
            raise BenchmarkError(
                "descendant-publication probe identities are not distinct"
            )
        try:
            wait_for_process_identities_gone(
                publication_identities,
                label="descendant-publication-failure renderer candidate group",
            )
        except ManagedCommandError as error:
            raise BenchmarkError(str(error)) from error

        internal_candidate_path = probe_directory / "internal-candidate.json"
        internal_descendant_path = probe_directory / "internal-descendant.json"
        internal_command = [
            "/usr/bin/env",
            "SRUI_BENCHMARK_FORCE_CANDIDATE_INTERNAL_FAILURE=1",
            (
                "SRUI_BENCHMARK_CANDIDATE_IDENTITY_PATH="
                f"{internal_candidate_path}"
            ),
            (
                "SRUI_BENCHMARK_CANDIDATE_DESCENDANT_IDENTITY_PATH="
                f"{internal_descendant_path}"
            ),
            str(binary),
            "--fixture",
            str(fixture),
            "--profile",
            "smoke",
            "--only-section",
            "31.1",
            "--output",
            str(probe_directory / "internal-driver.json"),
        ]
        try:
            internal_result = run_managed_command(
                internal_command,
                cwd=ROOT,
                timeout=probe_timeout,
                label="renderer internal-failure cleanup fault probe",
                poll_hook=lambda _process: ensure_free_space(ROOT),
            )
        except (ManagedCommandTimeout, ManagedCommandError) as error:
            raise BenchmarkError(
                "candidate internal-failure cleanup probe failed supervision"
            ) from error
        internal_detail = (
            internal_result.stdout + "\n" + internal_result.stderr
        ).strip()
        if (
            internal_result.returncode == 0
            or (
                "forced renderer candidate internal failure after descendant setup"
                not in internal_detail
            )
            or "candidate cleanup failed" in internal_detail
        ):
            raise BenchmarkError(
                "candidate internal-failure probe did not exercise the "
                f"expected cleanup path: {internal_detail[-2000:]}"
            )
        internal_identities: list[tuple[int, int]] = []
        try:
            for path in (internal_candidate_path, internal_descendant_path):
                identity = json.loads(path.read_text(encoding="utf-8"))
                pid = identity["pid"]
                birth = identity["birth_unix_ns"]
                if not _positive_integer(pid) or not _positive_integer(birth):
                    raise BenchmarkError(
                        "internal-failure probe identity has invalid values"
                    )
                internal_identities.append((pid, birth))
        except (OSError, json.JSONDecodeError, KeyError, TypeError) as error:
            raise BenchmarkError(
                "internal-failure probe did not record both identities"
            ) from error
        if len({pid for pid, _birth in internal_identities}) != 2:
            raise BenchmarkError(
                "internal-failure probe identities are not distinct"
            )
        try:
            wait_for_process_identities_gone(
                internal_identities,
                label="internal-failure renderer candidate group",
            )
        except ManagedCommandError as error:
            raise BenchmarkError(str(error)) from error
    return (
        "forced candidate identity failure terminated the exact candidate "
        "process group and proved both the child and a real descendant gone; "
        "a forced descendant-identity publication failure and a forced internal "
        "candidate failure also proved both exact identities gone before the "
        "benchmark driver returned"
    )




def run_verification(
    spec: dict[str, Any],
    default_timeout: int,
) -> tuple[float, bool, str, int | None]:
    started = time.monotonic()
    timeout = spec.get("timeout", default_timeout)
    ensure_free_space(ROOT)
    try:
        result = run_managed_command(
            spec["command"],
            cwd=ROOT,
            timeout=timeout,
            label=spec["name"],
            poll_hook=lambda _process: ensure_free_space(ROOT),
        )
    except ManagedCommandTimeout:
        elapsed = (time.monotonic() - started) * 1_000
        return elapsed, False, f"timed out after {timeout}s", None
    except ManagedCommandError as error:
        elapsed = (time.monotonic() - started) * 1_000
        return elapsed, False, f"process supervision failed: {error}", None

    elapsed = (time.monotonic() - started) * 1_000
    combined = (result.stdout + "\n" + result.stderr).strip()
    missing_output = [
        required
        for required in spec["required_output"]
        if required not in combined
    ]
    runner_match = re.search(
        r"(?m)^\s*8\s+reconnect\s+(?P<status>PASS|FAIL)\s+"
        r"(?P<count>[1-9][0-9]*)\s+runner\(s\)(?:;.*)?$",
        combined,
    )
    sample_count = (
        int(runner_match.group("count"))
        if runner_match
        else None
    )
    passed = result.returncode == 0 and not missing_output and sample_count is not None
    detail_parts = [f"exit {result.returncode}"]
    if missing_output:
        detail_parts.append("missing required output: " + ", ".join(missing_output))
    if sample_count is None:
        detail_parts.append("missing exact production conformance runner count")
    if combined:
        detail_parts.append(combined[-500:])
    return elapsed, passed, "; ".join(detail_parts), sample_count






def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--profile", choices=("smoke", "full"), default="smoke")
    parser.add_argument("--output-dir", type=Path)
    parser.add_argument("--record-baseline", action="store_true")
    parser.add_argument(
        "--timeout",
        type=int,
        help="per-driver timeout in seconds (default: smoke 600, full 1800)",
    )
    args = parser.parse_args(argv)
    if args.record_baseline and args.profile != "full":
        parser.error("--record-baseline requires --profile full")
    if args.timeout is not None and args.timeout <= 0:
        parser.error("--timeout must be positive")
    driver_timeout = effective_driver_timeout(args.profile, args.timeout)

    try:
        manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise BenchmarkError(f"cannot load benchmark manifest {MANIFEST}: {error}") from error
    validate_manifest(manifest)
    validate_runtime_platforms(manifest)

    fixture = ROOT / manifest["fixture"]
    if not fixture.is_file():
        raise BenchmarkError(f"benchmark fixture does not exist: {fixture}")
    ensure_free_space(ROOT)
    environment = benchmark_environment()
    if args.record_baseline:
        ensure_baseline_start_clean(environment)

    sections: dict[str, dict[str, Any]] = {}
    driver_artifacts: dict[str, dict[str, Any]] = {}
    for driver in manifest["drivers"]:
        if driver.get("platform") and driver["platform"] != sys.platform:
            raise BenchmarkError(
                f"{driver['name']} requires {driver['platform']}; current platform is {sys.platform}"
            )
        payload = run_driver(driver, fixture, args.profile, driver_timeout)
        if driver["name"] == "macos" and args.profile == "full":
            parse_render = next(
                section for section in payload["sections"] if section["id"] == "31.1"
            )
            parse_render["assertions"].append(
                run_window_isolation_self_test(
                    fixture,
                    min(driver_timeout, WINDOW_ISOLATION_SELF_TEST_TIMEOUT_SECONDS),
                )
            )
        driver_artifacts[driver["name"]] = payload["artifacts"]
        for section in payload["sections"]:
            _merge_driver_section(sections, section)

    append_parity_assertion(sections, driver_artifacts)

    candidate_cleanup_detail = run_candidate_cleanup_probe(
        fixture,
        driver_timeout,
    )
    sections["31.1"]["assertions"].append(
        {
            "id": "candidate_failure_cleanup",
            "name": "renderer candidate launch failures cannot leak a child process",
            "passed": True,
            "detail": candidate_cleanup_detail,
        }
    )

    for verification in manifest["verification_commands"]:
        elapsed, passed, detail, conformance_count = run_verification(
            verification, driver_timeout
        )
        if conformance_count is None:
            raise BenchmarkError(
                f"{verification['name']} did not emit an exact runner count"
            )
        section = sections[verification["section"]]
        section["metrics"].append(
            {
                "id": "production_reconnect_suite_ms",
                "name": verification["name"],
                "value": elapsed,
                "unit": "ms",
                "statistic": "wall",
            }
        )
        section["assertions"].append(
            {
                "id": "production_reconnect_suite",
                "name": verification["name"],
                "passed": passed,
                "detail": detail,
            }
        )
        section["sample_counts"][
            "runner.production_conformance"
        ] = conformance_count

    report = {
        "schema_version": 1,
        "contract_schema_version": CONTRACT["schema_version"],
        "contract_sha256": CONTRACT_SHA256,
        "generated_at": dt.datetime.now(dt.timezone.utc).isoformat(),
        "profile": args.profile,
        "fixture": str(fixture.relative_to(ROOT)),
        "environment": environment,
        "driver_artifacts": driver_artifacts,
        "sections": list(sections.values()),
    }
    validate_report(report, manifest["required_sections"])

    failures = failed_assertions(report)
    if args.record_baseline:
        ensure_baseline_recordable(
            report,
            ending_environment=benchmark_environment(),
        )
        output_dir = ROOT / "benchmarks/reports"
        stem = "baseline"
    else:
        output_dir = args.output_dir or ROOT / ".benchmark-results"
        stem = "latest"

    ensure_free_space(ROOT)
    markdown_path, json_path = write_report(report, output_dir, stem)
    print(markdown_path)
    print(json_path)
    return 1 if failures else 0


def _raise_termination(signum: int, _frame: Any) -> None:
    raise TerminationRequested(signum)


@contextlib.contextmanager
def termination_handlers() -> Iterator[None]:
    previous = {
        signum: signal.getsignal(signum)
        for signum in (signal.SIGINT, signal.SIGTERM)
    }
    for signum in previous:
        signal.signal(signum, _raise_termination)
    try:
        yield
    finally:
        for signum, handler in previous.items():
            signal.signal(signum, handler)


def cli(argv: list[str] | None = None) -> int:
    try:
        with termination_handlers():
            return main(argv)
    except TerminationRequested as error:
        print(
            f"benchmark interrupted by {signal.Signals(error.signum).name}",
            file=sys.stderr,
        )
        return 128 + error.signum
    except KeyboardInterrupt:
        print("benchmark interrupted by SIGINT", file=sys.stderr)
        return 130
    except BenchmarkError as error:
        print(f"benchmark error: {error}", file=sys.stderr)
        return 2
    except BaseExceptionGroup as error:
        terminations = termination_exceptions(error)
        companions = non_termination_exceptions(error)
        if terminations:
            if companions:
                print(
                    "benchmark failures accompanying interruption: "
                    + "; ".join(
                        f"{type(companion).__name__}: {companion}"
                        for companion in companions
                    ),
                    file=sys.stderr,
                )
            first = terminations[0]
            if isinstance(first, TerminationRequested):
                print(
                    f"benchmark interrupted by {signal.Signals(first.signum).name}",
                    file=sys.stderr,
                )
                return 128 + first.signum
            if isinstance(first, KeyboardInterrupt):
                print("benchmark interrupted by SIGINT", file=sys.stderr)
                return 130
            raise first
        print(
            f"benchmark failed during process cleanup: {exception_group_detail(error)}",
            file=sys.stderr,
        )
        return 2


if __name__ == "__main__":
    raise SystemExit(cli())
