"""Deterministic Swift registry codegen checks."""

from __future__ import annotations

from pathlib import Path

import pytest

from generate_swift_registry import generate_swift_registry


@pytest.fixture
def generated_swift_bytes(registry_path: Path, tmp_path: Path) -> bytes:
    output_path = tmp_path / "RegistryTables.swift"
    generate_swift_registry(registry_path, output_path)
    return output_path.read_bytes()


def test_swift_registry_codegen_is_deterministic(registry_path: Path, tmp_path: Path) -> None:
    first_output = tmp_path / "first.swift"
    second_output = tmp_path / "second.swift"

    generate_swift_registry(registry_path, first_output)
    generate_swift_registry(registry_path, second_output)

    assert first_output.read_bytes() == second_output.read_bytes()


def test_swift_registry_codegen_matches_committed_output(
    generated_swift_bytes: bytes,
    committed_swift_registry: Path,
) -> None:
    assert committed_swift_registry.exists(), f"missing committed output: {committed_swift_registry}"
    assert generated_swift_bytes == committed_swift_registry.read_bytes()
