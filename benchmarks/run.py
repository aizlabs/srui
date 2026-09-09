#!/usr/bin/env python3
"""Run SRUI's layered §31 benchmark suite and write JSON plus Markdown reports."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import platform
import signal
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parent.parent
MANIFEST = ROOT / "benchmarks/manifest.json"


class BenchmarkError(RuntimeError):
    pass


def percentile(values: list[float], fraction: float) -> float:
    if not values:
        raise ValueError("percentile requires at least one value")
    ordered = sorted(values)
    index = min(len(ordered) - 1, max(0, round((len(ordered) - 1) * fraction)))
    return ordered[index]


def run_driver(driver: dict[str, Any], fixture: Path, profile: str, timeout: int) -> list[dict[str, Any]]:
    with tempfile.NamedTemporaryFile(suffix=".json", delete=False) as output:
        output_path = Path(output.name)
    command = [
        *driver["command"],
        "--fixture", str(fixture),
        "--profile", profile,
        "--output", str(output_path),
    ]
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
            os.killpg(process.pid, signal.SIGTERM)
            try:
                process.communicate(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.communicate()
            raise BenchmarkError(f"{driver['name']} timed out after {timeout}s") from error
        if process.returncode:
            detail = (stderr or stdout).strip()
            raise BenchmarkError(f"{driver['name']} failed ({process.returncode}): {detail[-4000:]}")
        try:
            payload = json.loads(output_path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            raise BenchmarkError(f"{driver['name']} did not write valid result JSON") from error
        return payload["sections"]
    finally:
        output_path.unlink(missing_ok=True)


def run_verification(spec: dict[str, Any], default_timeout: int) -> tuple[float, bool, str]:
    started = dt.datetime.now(dt.timezone.utc)
    process = subprocess.Popen(
        spec["command"],
        cwd=ROOT,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        start_new_session=True,
    )
    timeout = spec.get("timeout", default_timeout)
    try:
        stdout, stderr = process.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGTERM)
        try:
            stdout, stderr = process.communicate(timeout=5)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            stdout, stderr = process.communicate()
        elapsed = (dt.datetime.now(dt.timezone.utc) - started).total_seconds() * 1_000
        return elapsed, False, f"timed out after {timeout}s"
    elapsed = (dt.datetime.now(dt.timezone.utc) - started).total_seconds() * 1_000
    combined = (stdout + "\n" + stderr).strip()
    passed = process.returncode == 0 and "executed no tests" not in combined
    detail = f"exit {process.returncode}; {combined[-500:]}" if combined else f"exit {process.returncode}"
    return elapsed, passed, detail


def validate_report(report: dict[str, Any], required: list[str]) -> None:
    seen: set[str] = set()
    for section in report["sections"]:
        section_id = section.get("id")
        if section_id in seen:
            raise BenchmarkError(f"duplicate section {section_id}")
        seen.add(section_id)
        if not section.get("metrics"):
            raise BenchmarkError(f"section {section_id} has no numeric metrics")
        for metric in section["metrics"]:
            if not isinstance(metric.get("value"), (int, float)):
                raise BenchmarkError(f"{section_id}.{metric.get('name')} is not numeric")
    missing = set(required) - seen
    if missing:
        raise BenchmarkError(f"missing required section(s): {', '.join(sorted(missing))}")


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


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--profile", choices=("smoke", "full"), default="smoke")
    parser.add_argument("--output-dir", type=Path)
    parser.add_argument("--record-baseline", action="store_true")
    parser.add_argument("--timeout", type=int, default=600)
    args = parser.parse_args(argv)

    manifest = json.loads(MANIFEST.read_text(encoding="utf-8"))
    fixture = ROOT / manifest["fixture"]
    sections: dict[str, dict[str, Any]] = {}
    for driver in manifest["drivers"]:
        if driver.get("platform") and driver["platform"] != sys.platform:
            raise BenchmarkError(
                f"{driver['name']} requires {driver['platform']}; current platform is {sys.platform}"
            )
        for section in run_driver(driver, fixture, args.profile, args.timeout):
            section_id = section["id"]
            if section_id in sections:
                sections[section_id]["metrics"].extend(section["metrics"])
                sections[section_id]["assertions"].extend(section["assertions"])
                sections[section_id].setdefault("notes", []).extend(section.get("notes", []))
            else:
                sections[section_id] = section

    renderer_size = next(
        metric["value"]
        for metric in sections["31.1"]["metrics"]
        if metric["name"] == "SRUI representation"
    )
    serializer_size = next(
        metric["value"]
        for metric in sections["31.2"]["metrics"]
        if metric["name"] == "serialized transaction size"
    )
    sections["31.2"]["assertions"].append({
        "name": "renderer and serializer consume the same abstract state",
        "passed": renderer_size == serializer_size,
        "detail": f"{renderer_size:g} bytes on both paths",
    })

    for verification in manifest.get("verification_commands", []):
        elapsed, passed, detail = run_verification(verification, args.timeout)
        section = sections[verification["section"]]
        section["metrics"].append({
            "name": verification["name"],
            "value": elapsed,
            "unit": "ms",
            "statistic": "wall",
        })
        section["assertions"].append({
            "name": verification["name"],
            "passed": passed,
            "detail": detail,
        })

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
        "sections": list(sections.values()),
    }
    validate_report(report, manifest["required_sections"])

    if args.record_baseline:
        output_dir = ROOT / "benchmarks/reports"
        stem = "baseline"
    else:
        output_dir = args.output_dir or ROOT / ".benchmark-results"
        stem = "latest"
    output_dir.mkdir(parents=True, exist_ok=True)
    json_path = output_dir / f"{stem}.json"
    markdown_path = output_dir / f"{stem}.md"
    json_path.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    markdown_path.write_text(markdown(report), encoding="utf-8")
    print(markdown_path)
    print(json_path)

    failed = [
        (section["id"], assertion["name"])
        for section in report["sections"]
        for assertion in section["assertions"]
        if not assertion["passed"]
    ]
    return 1 if failed else 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except BenchmarkError as error:
        print(f"benchmark error: {error}", file=sys.stderr)
        raise SystemExit(2)
