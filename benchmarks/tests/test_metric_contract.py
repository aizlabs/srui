from __future__ import annotations

import hashlib
import json
import math
import subprocess
import sys
from pathlib import Path

import pytest

from benchmarks import contract
from benchmarks import run as benchmark_run
from benchmarks.tests.support import (
    payload_for_driver,
    valid_manifest,
)


def test_contract_digest_is_canonical_and_generated_descriptors_are_fresh() -> None:
    canonical = json.dumps(
        contract.CONTRACT,
        ensure_ascii=False,
        separators=(",", ":"),
        sort_keys=True,
    ).encode("utf-8")
    assert contract.CONTRACT_SHA256 == hashlib.sha256(canonical).hexdigest()
    assert contract.contract_sha256() == contract.CONTRACT_SHA256

    root = Path(__file__).resolve().parents[2]
    result = subprocess.run(
        [
            sys.executable,
            "benchmarks/generate_metric_contract.py",
            "--check",
        ],
        cwd=root,
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr


def test_percentile_contract_is_half_up_finite_only() -> None:
    assert benchmark_run.percentile([0.0, 1.0, 2.0], 0.25) == 1.0
    with pytest.raises(ValueError, match="finite values"):
        benchmark_run.percentile([0.0, math.nan], 0.5)
    with pytest.raises(ValueError, match="fraction"):
        benchmark_run.percentile([0.0], 1.1)


def test_only_named_semantic_update_targets_exist() -> None:
    inventory = benchmark_run.EXPECTED_DRIVER_INVENTORY["macos"]["31.3"]
    assert inventory["metrics"][("updates.1.semantic", "p50")][1] is None
    assert inventory["metrics"][("updates.100.semantic", "p50")][1] == 1.0
    assert inventory["metrics"][("updates.1000.semantic", "p50")][1] == 5.0


def test_driver_rejects_wrong_contract_digest() -> None:
    driver = valid_manifest()["drivers"][0]
    payload = payload_for_driver(driver)
    payload["contract_sha256"] = "0" * 64
    with pytest.raises(
        benchmark_run.BenchmarkError,
        match="metric contract identity",
    ):
        benchmark_run.validate_driver_output(payload, driver)
