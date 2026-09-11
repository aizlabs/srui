"""Benchmark report assembly, rendering, and atomic publication."""

from __future__ import annotations

import json
import os
import tempfile
from pathlib import Path
from typing import Any

from benchmarks.errors import BenchmarkError
from benchmarks.process_control import blocked_termination_signals


def append_parity_assertion(
    sections: dict[str, dict[str, Any]],
    artifacts: dict[str, dict[str, Any]],
) -> None:
    rust = artifacts["rust"]
    macos = artifacts["macos"]
    canonical_keys = (
        "canonical_transaction_sha256",
        "canonical_transaction_bytes",
    )
    matches = all(rust[key] == macos[key] for key in canonical_keys)
    if matches:
        detail = (
            f"{rust['canonical_transaction_sha256']} / "
            f"{rust['canonical_transaction_bytes']} exact canonical progressive "
            "sequence bytes"
        )
    else:
        detail = (
            "rust canonical progressive sequence "
            f"{rust['canonical_transaction_sha256']} / "
            f"{rust['canonical_transaction_bytes']} bytes; "
            "macos canonical progressive sequence "
            f"{macos['canonical_transaction_sha256']} / "
            f"{macos['canonical_transaction_bytes']} bytes"
        )
    sections["31.2"]["assertions"].append(
        {
            "id": "canonical_transaction_parity",
            "name": "canonical progressive transaction sequence bytes match",
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
    environment = report["environment"]

    def one_line(value: Any) -> str:
        return str(value).replace("\n", " / ")

    authoritative = report["profile"] == "full"
    target_heading = "Target" if authoritative else "Diagnostic reference"
    lines = [
        "# SRUI benchmark report",
        "",
        f"- Generated: {report['generated_at']}",
        f"- Profile: {report['profile']}",
        *(
            []
            if authoritative
            else [
                "- Evidence class: diagnostic only; smoke timing is not §23 evidence",
            ]
        ),
        f"- Host: {environment['platform']} / {environment['machine']}",
        f"- Chip: {environment['chip']}",
        f"- Physical RAM: {environment['physical_ram_bytes']} bytes",
        f"- Xcode: {one_line(environment['xcode'])}",
        f"- Swift: {one_line(environment['swift'])}",
        f"- Rust: {one_line(environment['rust'])}",
        f"- Python: {environment['python']}",
        f"- Git: {environment['git_commit']} "
        f"({'dirty' if environment['git_dirty'] else 'clean'})",
        f"- Fixture: {report['fixture']}",
        f"- Metric contract: schema {report['contract_schema_version']} / "
        f"sha256 {report['contract_sha256']}",
        "",
    ]
    followups: list[str] = []
    correctness_failures: list[str] = []
    for section in sorted(report["sections"], key=lambda item: item["id"]):
        lines += [
            f"## §{section['id']} {section['name']}",
            "",
            "Samples:",
            "",
            *[
                f"- \x60{group}\x60: {count}"
                for group, count in sorted(section["sample_counts"].items())
            ],
            "",
            f"| Metric | Value | Statistic | {target_heading} |",
            "|---|---:|---|---:|",
        ]
        for metric in section["metrics"]:
            target = "—"
            if "target" in metric:
                operator = (
                    "≤"
                    if metric.get("target_direction", "max") == "max"
                    else "≥"
                )
                target = f"{operator} {metric['target']:g} {metric['unit']}"
            marker = ""
            if over_2x(metric):
                marker = (
                    " **WARNING >2x**"
                    if authoritative
                    else " **DIAGNOSTIC >2x**"
                )
            lines.append(
                f"| {metric['name']} | {metric['value']:.4g} "
                f"{metric['unit']}{marker} | "
                f"{metric['statistic']} | {target} |"
            )
            if over_2x(metric):
                followups.append(
                    f"§{section['id']} {metric['name']} "
                    f"({metric['statistic']}): "
                    f"{metric['value']:.4g} {metric['unit']} vs target "
                    f"{metric['target']:g} {metric['unit']}"
                )
        lines += ["", "Assertions:", ""]
        for assertion in section["assertions"]:
            mark = "PASS" if assertion["passed"] else "FAIL"
            detail = (
                f" — {assertion.get('detail', '')}"
                if assertion.get("detail")
                else ""
            )
            lines.append(f"- **{mark}** {assertion['name']}{detail}")
            if not assertion["passed"]:
                correctness_failures.append(
                    f"§{section['id']} {assertion['name']}"
                )
        if section.get("notes"):
            lines += ["", "Notes:", ""]
            lines += [f"- {note}" for note in section["notes"]]
        lines.append("")
    lines += ["## Follow-up flags", ""]
    if authoritative:
        if followups:
            lines += [
                f"- **PERFORMANCE FOLLOW-UP (>2x):** {item}"
                for item in followups
            ]
        else:
            lines.append("- No §23 target was missed by more than 2x.")
    elif followups:
        lines += [
            f"- **SMOKE DIAGNOSTIC (>2x; not §23 evidence):** {item}"
            for item in followups
        ]
    else:
        lines.append(
            "- Smoke profile is diagnostic only; run the full profile for §23 evidence."
        )
    if correctness_failures:
        lines += [
            "",
            *[
                f"- **CORRECTNESS FAILURE:** {item}"
                for item in correctness_failures
            ],
        ]
    lines.append("")
    return "\n".join(lines)


def failed_assertions(report: dict[str, Any]) -> list[tuple[str, str]]:
    return [
        (section["id"], assertion["name"])
        for section in report["sections"]
        for assertion in section["assertions"]
        if not assertion["passed"]
    ]


def ensure_baseline_start_clean(environment: dict[str, Any]) -> None:
    if environment.get("git_dirty") is not False:
        raise BenchmarkError(
            "refusing to overwrite the committed baseline from a dirty Git tree"
        )


def ensure_baseline_recordable(
    report: dict[str, Any],
    *,
    ending_environment: dict[str, Any] | None = None,
) -> None:
    if report.get("profile") != "full":
        raise BenchmarkError(
            "refusing to overwrite the committed baseline with a non-full profile"
        )
    environment = report.get("environment", {})
    ensure_baseline_start_clean(environment)
    if ending_environment is not None and (
        ending_environment.get("git_dirty") is not False
        or ending_environment.get("git_commit")
        != environment.get("git_commit")
    ):
        raise BenchmarkError(
            "refusing to overwrite the committed baseline after Git state changed "
            "during the benchmark"
        )
    failures = failed_assertions(report)
    if failures:
        formatted = ", ".join(
            f"§{section} {name}"
            for section, name in failures
        )
        raise BenchmarkError(
            "refusing to overwrite the committed baseline with failed assertions: "
            + formatted
        )


def _stage_report_file(path: Path, content: str) -> Path:
    descriptor, raw_path = tempfile.mkstemp(
        dir=path.parent,
        prefix=f".{path.name}.stage-",
    )
    temporary = Path(raw_path)
    try:
        with os.fdopen(descriptor, "w", encoding="utf-8") as output:
            output.write(content)
            output.flush()
            os.fsync(output.fileno())
    except BaseException:
        temporary.unlink(missing_ok=True)
        raise
    return temporary


def _reserve_report_backup(path: Path) -> Path:
    descriptor, raw_path = tempfile.mkstemp(
        dir=path.parent,
        prefix=f".{path.name}.backup-",
    )
    os.close(descriptor)
    backup = Path(raw_path)
    backup.unlink()
    return backup


def _fsync_directory(path: Path) -> None:
    descriptor = os.open(
        path,
        os.O_RDONLY | getattr(os, "O_DIRECTORY", 0),
    )
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def write_report(
    report: dict[str, Any],
    output_dir: Path,
    stem: str,
) -> tuple[Path, Path]:
    """Publish the JSON/Markdown pair atomically with rollback."""

    output_dir.mkdir(parents=True, exist_ok=True)
    json_path = output_dir / f"{stem}.json"
    markdown_path = output_dir / f"{stem}.md"
    paths_and_content = (
        (
            json_path,
            json.dumps(
                report,
                indent=2,
                sort_keys=True,
                allow_nan=False,
            )
            + "\n",
        ),
        (markdown_path, markdown(report)),
    )
    staged: dict[Path, Path] = {}
    backups: dict[Path, Path] = {}
    installed: set[Path] = set()
    preserve_backups = False
    try:
        for path, content in paths_and_content:
            staged[path] = _stage_report_file(path, content)
            backups[path] = _reserve_report_backup(path)
        _fsync_directory(output_dir)

        with blocked_termination_signals():
            try:
                for path, _content in paths_and_content:
                    if path.exists():
                        os.replace(path, backups[path])
                for path, _content in paths_and_content:
                    os.replace(staged[path], path)
                    installed.add(path)
                _fsync_directory(output_dir)
            except BaseException as primary:
                rollback_errors: list[BaseException] = []
                for path, _content in reversed(paths_and_content):
                    try:
                        if backups[path].exists():
                            os.replace(backups[path], path)
                        elif path in installed:
                            path.unlink(missing_ok=True)
                    except BaseException as rollback_error:
                        rollback_errors.append(rollback_error)
                try:
                    _fsync_directory(output_dir)
                except BaseException as rollback_error:
                    rollback_errors.append(rollback_error)
                if rollback_errors:
                    preserve_backups = True
                    recovery = BenchmarkError(
                        "report rollback is incomplete; recovery backups retained at "
                        + ", ".join(
                            str(path)
                            for path in backups.values()
                            if path.exists()
                        )
                    )
                    raise BaseExceptionGroup(
                        "report publication and rollback failed",
                        [primary, *rollback_errors, recovery],
                    )
                raise

            cleanup_errors: list[BaseException] = []
            for backup in backups.values():
                try:
                    backup.unlink(missing_ok=True)
                except BaseException as cleanup_error:
                    cleanup_errors.append(cleanup_error)
            try:
                _fsync_directory(output_dir)
            except BaseException as cleanup_error:
                cleanup_errors.append(cleanup_error)
            if cleanup_errors:
                preserve_backups = True
                raise BaseExceptionGroup(
                    "report published but backup cleanup failed",
                    cleanup_errors,
                )
    finally:
        cleanup_paths = list(staged.values())
        if not preserve_backups:
            cleanup_paths.extend(backups.values())
        for temporary in cleanup_paths:
            temporary.unlink(missing_ok=True)
    return markdown_path, json_path
