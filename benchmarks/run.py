#!/usr/bin/env python3
"""Run SRUI's layered §31 benchmark suite and write JSON plus Markdown reports."""

from __future__ import annotations

import argparse
import contextlib
import datetime as dt
import functools
import json
import math
import os
import platform
import signal
import subprocess
import sys
import tempfile
from collections.abc import Iterator
from pathlib import Path
from typing import Any

from jsonschema import Draft202012Validator, FormatChecker

ROOT = Path(__file__).resolve().parent.parent
MANIFEST = ROOT / "benchmarks/manifest.json"
SCHEMA = ROOT / "benchmarks/schema.json"
EXPECTED_SECTIONS = ("31.1", "31.2", "31.3", "31.4", "31.5", "31.6")
EXPECTED_DRIVER_SECTIONS = {
    "rust": {"31.2", "31.5", "31.6"},
    "macos": {"31.1", "31.3", "31.4", "31.5", "31.6"},
}


class BenchmarkError(RuntimeError):
    pass


class TerminationRequested(BaseException):
    def __init__(self, signum: int) -> None:
        super().__init__(signum)
        self.signum = signum


def percentile(values: list[float], fraction: float) -> float:
    if not values:
        raise ValueError("percentile requires at least one value")
    ordered = sorted(values)
    index = min(len(ordered) - 1, max(0, round((len(ordered) - 1) * fraction)))
    return ordered[index]


@functools.lru_cache(maxsize=1)
def schema_bundle() -> dict[str, Any]:
    try:
        schema = json.loads(SCHEMA.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise BenchmarkError(f"cannot load benchmark schema {SCHEMA}: {error}") from error
    Draft202012Validator.check_schema(schema)
    return schema


def validate_document(document: Any, definition: str, label: str) -> None:
    schema = dict(schema_bundle())
    schema["$ref"] = f"#/$defs/{definition}"
    validator = Draft202012Validator(schema, format_checker=FormatChecker())
    errors = sorted(
        validator.iter_errors(document),
        key=lambda error: "/".join(str(part) for part in error.absolute_path),
    )
    if not errors:
        return
    error = errors[0]
    location = "$"
    for part in error.absolute_path:
        location += f"[{part}]" if isinstance(part, int) else f".{part}"
    raise BenchmarkError(f"{label} schema violation at {location}: {error.message}")


def validate_manifest(manifest: dict[str, Any]) -> None:
    validate_document(manifest, "manifest", "benchmark manifest")

    required = tuple(manifest["required_sections"])
    if required != EXPECTED_SECTIONS:
        raise BenchmarkError(
            "benchmark manifest required_sections must be exactly "
            + ", ".join(EXPECTED_SECTIONS)
        )

    drivers = {driver["name"]: driver for driver in manifest["drivers"]}
    if set(drivers) != set(EXPECTED_DRIVER_SECTIONS):
        raise BenchmarkError("benchmark manifest must declare exactly the rust and macos drivers")
    for name, expected in EXPECTED_DRIVER_SECTIONS.items():
        declared = set(drivers[name]["sections"])
        if declared != expected:
            raise BenchmarkError(
                f"{name} driver sections must be exactly {', '.join(sorted(expected))}"
            )
    if drivers["macos"].get("platform") != "darwin":
        raise BenchmarkError("macos driver must declare platform darwin")
    if "platform" in drivers["rust"]:
        raise BenchmarkError("rust driver must remain platform-independent")

    covered = set().union(*(set(driver["sections"]) for driver in manifest["drivers"]))
    if covered != set(EXPECTED_SECTIONS):
        raise BenchmarkError("driver declarations must cover every §31 subsection")
    if not any(
        verification["section"] == "31.5"
        for verification in manifest["verification_commands"]
    ):
        raise BenchmarkError("manifest must verify the production reconnect boundary suite")


def _signal_process_group(process: subprocess.Popen[str], signum: int) -> None:
    try:
        os.killpg(process.pid, signum)
        return
    except ProcessLookupError:
        return
    except PermissionError:
        pass

    if process.poll() is None:
        try:
            process.send_signal(signum)
        except ProcessLookupError:
            pass


def terminate_process_group(
    process: subprocess.Popen[str],
    grace_seconds: float = 5.0,
) -> tuple[str, str]:
    """Terminate and reap a subprocess plus every descendant in its new session."""

    _signal_process_group(process, signal.SIGTERM)
    try:
        return process.communicate(timeout=grace_seconds)
    except subprocess.TimeoutExpired:
        _signal_process_group(process, signal.SIGKILL)
        try:
            return process.communicate(timeout=grace_seconds)
        except subprocess.TimeoutExpired as error:
            raise BenchmarkError(f"could not reap process group {process.pid}") from error


def validate_driver_output(payload: Any, driver: dict[str, Any]) -> dict[str, Any]:
    validate_document(payload, "driver_output", f"{driver['name']} driver output")
    actual = [section["id"] for section in payload["sections"]]
    if len(actual) != len(set(actual)):
        raise BenchmarkError(f"{driver['name']} driver emitted duplicate sections")
    if set(actual) != set(driver["sections"]):
        raise BenchmarkError(
            f"{driver['name']} driver emitted {', '.join(sorted(actual))}; "
            f"manifest declares {', '.join(sorted(driver['sections']))}"
        )
    return payload


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
    process: subprocess.Popen[str] | None = None
    completed_successfully = False
    try:
        process = subprocess.Popen(
            command,
            cwd=ROOT,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            start_new_session=True,
        )
        try:
            stdout, stderr = process.communicate(timeout=timeout)
        except subprocess.TimeoutExpired as error:
            raise BenchmarkError(f"{driver['name']} timed out after {timeout}s") from error

        if process.returncode:
            detail = (stderr or stdout).strip()
            raise BenchmarkError(
                f"{driver['name']} failed ({process.returncode}): {detail[-4000:]}"
            )
        try:
            payload = json.loads(output_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            raise BenchmarkError(f"{driver['name']} did not write valid result JSON") from error
        validated = validate_driver_output(payload, driver)
        completed_successfully = True
        return validated
    finally:
        if process is not None and not completed_successfully:
            terminate_process_group(process)
        output_path.unlink(missing_ok=True)


def run_verification(
    spec: dict[str, Any],
    default_timeout: int,
) -> tuple[float, bool, str]:
    started = dt.datetime.now(dt.timezone.utc)
    process: subprocess.Popen[str] | None = None
    completed_successfully = False
    timeout = spec.get("timeout", default_timeout)
    try:
        process = subprocess.Popen(
            spec["command"],
            cwd=ROOT,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            start_new_session=True,
        )
        try:
            stdout, stderr = process.communicate(timeout=timeout)
        except subprocess.TimeoutExpired:
            elapsed = (dt.datetime.now(dt.timezone.utc) - started).total_seconds() * 1_000
            return elapsed, False, f"timed out after {timeout}s"

        elapsed = (dt.datetime.now(dt.timezone.utc) - started).total_seconds() * 1_000
        combined = (stdout + "\n" + stderr).strip()
        passed = process.returncode == 0 and "executed no tests" not in combined
        detail = (
            f"exit {process.returncode}; {combined[-500:]}"
            if combined
            else f"exit {process.returncode}"
        )
        completed_successfully = process.returncode == 0
        return elapsed, passed, detail
    finally:
        if process is not None and not completed_successfully:
            terminate_process_group(process)


def validate_report(report: dict[str, Any], required: list[str]) -> None:
    validate_document(report, "report", "benchmark report")
    seen = [section["id"] for section in report["sections"]]
    if len(seen) != len(set(seen)):
        raise BenchmarkError("benchmark report contains duplicate sections")
    if set(seen) != set(required):
        missing = set(required) - set(seen)
        unexpected = set(seen) - set(required)
        detail = []
        if missing:
            detail.append("missing " + ", ".join(sorted(missing)))
        if unexpected:
            detail.append("unexpected " + ", ".join(sorted(unexpected)))
        raise BenchmarkError("benchmark report sections: " + "; ".join(detail))

    for section in report["sections"]:
        metric_keys: set[tuple[str, str]] = set()
        for metric in section["metrics"]:
            value = metric["value"]
            if isinstance(value, bool) or not isinstance(value, (int, float)):
                raise BenchmarkError(f"{section['id']}.{metric['name']} is not numeric")
            if not math.isfinite(value):
                raise BenchmarkError(f"{section['id']}.{metric['name']} is not finite")
            key = (metric["name"], metric["statistic"])
            if key in metric_keys:
                raise BenchmarkError(
                    f"{section['id']} duplicates metric {metric['name']} ({metric['statistic']})"
                )
            metric_keys.add(key)


def append_parity_assertion(
    sections: dict[str, dict[str, Any]],
    artifacts: dict[str, dict[str, Any]],
) -> None:
    rust = artifacts["rust"]
    macos = artifacts["macos"]
    matches = rust == macos
    if matches:
        detail = (
            f"{rust['canonical_transaction_sha256']} / "
            f"{rust['canonical_transaction_bytes']} exact bytes"
        )
    else:
        detail = (
            "rust "
            f"{rust['canonical_transaction_sha256']} / {rust['canonical_transaction_bytes']} bytes; "
            "macos "
            f"{macos['canonical_transaction_sha256']} / {macos['canonical_transaction_bytes']} bytes"
        )
    sections["31.2"]["assertions"].append(
        {
            "name": "renderer and serializer canonical transaction bytes match",
            "passed": matches,
            "detail": detail,
        }
    )


def over_2x(metric: dict[str, Any]) -> bool:
    target = metric.get("target")
    if target is None:
        return False
    value = metric["value"]
    if metric.get("target_direction", "max") == "max":
        return value > 2 * target
    return value < target / 2


def markdown(report: dict[str, Any]) -> str:
    lines = [
        "# SRUI benchmark report",
        "",
        f"- Generated: {report['generated_at']}",
        f"- Profile: {report['profile']}",
        f"- Host: {report['environment']['platform']} / {report['environment']['machine']}",
        f"- Fixture: {report['fixture']}",
        "",
    ]
    followups: list[str] = []
    correctness_failures: list[str] = []
    for section in sorted(report["sections"], key=lambda item: item["id"]):
        lines += [
            f"## §{section['id']} {section['name']}",
            "",
            "| Metric | Value | Statistic | Target |",
            "|---|---:|---|---:|",
        ]
        for metric in section["metrics"]:
            target = "—"
            if "target" in metric:
                operator = "≤" if metric.get("target_direction", "max") == "max" else "≥"
                target = f"{operator} {metric['target']:g} {metric['unit']}"
            marker = " **WARNING >2x**" if over_2x(metric) else ""
            lines.append(
                f"| {metric['name']} | {metric['value']:.4g} {metric['unit']}{marker} | "
                f"{metric['statistic']} | {target} |"
            )
            if over_2x(metric):
                followups.append(
                    f"§{section['id']} {metric['name']}: {metric['value']:.4g} "
                    f"{metric['unit']} vs target {metric['target']:g} {metric['unit']}"
                )
        lines += ["", "Assertions:", ""]
        for assertion in section["assertions"]:
            mark = "PASS" if assertion["passed"] else "FAIL"
            detail = f" — {assertion.get('detail', '')}" if assertion.get("detail") else ""
            lines.append(f"- **{mark}** {assertion['name']}{detail}")
            if not assertion["passed"]:
                correctness_failures.append(f"§{section['id']} {assertion['name']}")
        if section.get("notes"):
            lines += ["", "Notes:", ""]
            lines += [f"- {note}" for note in section["notes"]]
        lines.append("")
    lines += ["## Follow-up flags", ""]
    if followups:
        lines += [f"- **PERFORMANCE FOLLOW-UP (>2x):** {item}" for item in followups]
    else:
        lines.append("- No §23 target was missed by more than 2x.")
    if correctness_failures:
        lines += ["", *[f"- **CORRECTNESS FAILURE:** {item}" for item in correctness_failures]]
    lines.append("")
    return "\n".join(lines)


def failed_assertions(report: dict[str, Any]) -> list[tuple[str, str]]:
    return [
        (section["id"], assertion["name"])
        for section in report["sections"]
        for assertion in section["assertions"]
        if not assertion["passed"]
    ]


def ensure_baseline_recordable(report: dict[str, Any]) -> None:
    failures = failed_assertions(report)
    if failures:
        formatted = ", ".join(f"§{section} {name}" for section, name in failures)
        raise BenchmarkError(
            "refusing to overwrite the committed baseline with failed assertions: " + formatted
        )


def write_report(report: dict[str, Any], output_dir: Path, stem: str) -> tuple[Path, Path]:
    output_dir.mkdir(parents=True, exist_ok=True)
    json_path = output_dir / f"{stem}.json"
    markdown_path = output_dir / f"{stem}.md"
    json_path.write_text(
        json.dumps(report, indent=2, sort_keys=True, allow_nan=False) + "\n",
        encoding="utf-8",
    )
    markdown_path.write_text(markdown(report), encoding="utf-8")
    return markdown_path, json_path


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--profile", choices=("smoke", "full"), default="smoke")
    parser.add_argument("--output-dir", type=Path)
    parser.add_argument("--record-baseline", action="store_true")
    parser.add_argument("--timeout", type=int, default=600)
    args = parser.parse_args(argv)
    if args.record_baseline and args.profile != "full":
        parser.error("--record-baseline requires --profile full")
    if args.timeout <= 0:
        parser.error("--timeout must be positive")

    try:
        manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise BenchmarkError(f"cannot load benchmark manifest {MANIFEST}: {error}") from error
    validate_manifest(manifest)

    fixture = ROOT / manifest["fixture"]
    if not fixture.is_file():
        raise BenchmarkError(f"benchmark fixture does not exist: {fixture}")

    sections: dict[str, dict[str, Any]] = {}
    driver_artifacts: dict[str, dict[str, Any]] = {}
    for driver in manifest["drivers"]:
        if driver.get("platform") and driver["platform"] != sys.platform:
            raise BenchmarkError(
                f"{driver['name']} requires {driver['platform']}; current platform is {sys.platform}"
            )
        payload = run_driver(driver, fixture, args.profile, args.timeout)
        driver_artifacts[driver["name"]] = payload["artifacts"]
        for section in payload["sections"]:
            section_id = section["id"]
            if section_id in sections:
                if sections[section_id]["name"] != section["name"]:
                    raise BenchmarkError(
                        f"drivers disagree on the name of section {section_id}"
                    )
                sections[section_id]["metrics"].extend(section["metrics"])
                sections[section_id]["assertions"].extend(section["assertions"])
                sections[section_id].setdefault("notes", []).extend(
                    section.get("notes", [])
                )
            else:
                sections[section_id] = section

    append_parity_assertion(sections, driver_artifacts)

    for verification in manifest["verification_commands"]:
        elapsed, passed, detail = run_verification(verification, args.timeout)
        section = sections[verification["section"]]
        section["metrics"].append(
            {
                "name": verification["name"],
                "value": elapsed,
                "unit": "ms",
                "statistic": "wall",
            }
        )
        section["assertions"].append(
            {
                "name": verification["name"],
                "passed": passed,
                "detail": detail,
            }
        )

    report = {
        "schema_version": 1,
        "generated_at": dt.datetime.now(dt.timezone.utc).isoformat(),
        "profile": args.profile,
        "fixture": str(fixture.relative_to(ROOT)),
        "environment": {
            "platform": platform.platform(),
            "machine": platform.machine(),
            "python": platform.python_version(),
        },
        "driver_artifacts": driver_artifacts,
        "sections": list(sections.values()),
    }
    validate_report(report, manifest["required_sections"])

    failures = failed_assertions(report)
    if args.record_baseline:
        ensure_baseline_recordable(report)
        output_dir = ROOT / "benchmarks/reports"
        stem = "baseline"
    else:
        output_dir = args.output_dir or ROOT / ".benchmark-results"
        stem = "latest"

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


if __name__ == "__main__":
    raise SystemExit(cli())
