from __future__ import annotations

import importlib.util
import json
from pathlib import Path

MODULE_PATH = Path(__file__).resolve().parents[1] / "run.py"
SPEC = importlib.util.spec_from_file_location("benchmark_run", MODULE_PATH)
assert SPEC and SPEC.loader
benchmark_run = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(benchmark_run)


def test_percentile_is_deterministic() -> None:
    assert benchmark_run.percentile([9, 1, 5, 3, 7], 0.5) == 5
    assert benchmark_run.percentile([9, 1, 5, 3, 7], 0.95) == 9


def test_over_2x_honors_direction() -> None:
    assert benchmark_run.over_2x({"value": 2.01, "target": 1.0, "target_direction": "max"})
    assert not benchmark_run.over_2x({"value": 2.0, "target": 1.0, "target_direction": "max"})
    assert benchmark_run.over_2x({"value": 4.9, "target": 10.0, "target_direction": "min"})


def test_report_calls_out_performance_followup() -> None:
    report = {
        "generated_at": "2026-01-01T00:00:00Z",
        "profile": "smoke",
        "fixture": "fixture.json",
        "environment": {"platform": "test", "machine": "test"},
        "sections": [{
            "id": "31.3",
            "name": "Mutation",
            "metrics": [{
                "name": "100 updates",
                "value": 2.1,
                "unit": "ms",
                "statistic": "p50",
                "target": 1.0,
                "target_direction": "max",
            }],
            "assertions": [{"name": "wire independence", "passed": True}],
        }],
    }
    rendered = benchmark_run.markdown(report)
    assert "PERFORMANCE FOLLOW-UP (>2x)" in rendered
    assert "WARNING >2x" in rendered


def test_committed_baseline_has_every_required_section() -> None:
    root = Path(__file__).resolve().parents[2]
    report = json.loads((root / "benchmarks/reports/baseline.json").read_text())
    manifest = json.loads((root / "benchmarks/manifest.json").read_text())
    benchmark_run.validate_report(report, manifest["required_sections"])
