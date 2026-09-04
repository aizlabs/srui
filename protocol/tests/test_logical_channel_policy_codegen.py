"""Single-source logical-channel scheduling policy codegen checks."""

from __future__ import annotations

from pathlib import Path

import generate_logical_channel_policy as policy_codegen
import pytest


def generate_into(
    policy_path: Path, output_dir: Path
) -> tuple[policy_codegen.LogicalChannelPolicy, Path, Path]:
    rust_output = output_dir / "logical_channel_policy.generated.rs"
    swift_output = output_dir / "LogicalChannelPolicy.generated.swift"
    policy = policy_codegen.generate_logical_channel_policy(
        policy_path, rust_output, swift_output
    )
    return policy, rust_output, swift_output


def test_policy_codegen_is_deterministic(tmp_path: Path) -> None:
    first = tmp_path / "first"
    second = tmp_path / "second"

    _, first_rust, first_swift = generate_into(policy_codegen.POLICY_PATH, first)
    _, second_rust, second_swift = generate_into(policy_codegen.POLICY_PATH, second)

    assert first_rust.read_bytes() == second_rust.read_bytes()
    assert first_swift.read_bytes() == second_swift.read_bytes()


def test_policy_codegen_matches_both_committed_outputs(tmp_path: Path) -> None:
    _, rust_output, swift_output = generate_into(policy_codegen.POLICY_PATH, tmp_path)

    assert rust_output.read_bytes() == policy_codegen.RUST_OUTPUT.read_bytes()
    assert swift_output.read_bytes() == policy_codegen.SWIFT_OUTPUT.read_bytes()


def test_policy_derives_documented_circular_service_gaps() -> None:
    policy = policy_codegen.load_policy(policy_codegen.POLICY_PATH)

    assert policy.max_service_gaps == {
        "control": 5,
        "input": 5,
        "ui": 8,
        "terminal_high": 12,
        "terminal_normal": 14,
        "resource": 24,
    }


def test_invalid_policy_never_touches_outputs(tmp_path: Path) -> None:
    policy_path = tmp_path / "invalid.yaml"
    policy_path.write_text(
        "version: 1\nclasses: [control]\nservice_cycle: [unknown]\n",
        encoding="utf-8",
    )
    rust_output = tmp_path / "policy.rs"
    swift_output = tmp_path / "Policy.swift"
    rust_output.write_text("known rust\n", encoding="utf-8")
    swift_output.write_text("known swift\n", encoding="utf-8")

    with pytest.raises(ValueError):
        policy_codegen.generate_logical_channel_policy(
            policy_path, rust_output, swift_output
        )

    assert rust_output.read_text(encoding="utf-8") == "known rust\n"
    assert swift_output.read_text(encoding="utf-8") == "known swift\n"
    assert list(tmp_path.glob(".*.tmp")) == []
