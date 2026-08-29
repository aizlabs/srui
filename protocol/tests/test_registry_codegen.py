"""Deterministic and failure-atomic Swift registry codegen checks."""

from __future__ import annotations

from pathlib import Path

import pytest

import generate_swift_registry as registry_codegen
from generate_swift_registry import generate_swift_registry


@pytest.fixture
def generated_swift_bytes(registry_path: Path, tmp_path: Path) -> bytes:
    output_path = tmp_path / "RegistryTables.swift"
    generate_swift_registry(registry_path, output_path)
    return output_path.read_bytes()


def test_swift_registry_codegen_is_deterministic(
    registry_path: Path, tmp_path: Path
) -> None:
    first_output = tmp_path / "first.swift"
    second_output = tmp_path / "second.swift"

    generate_swift_registry(registry_path, first_output)
    generate_swift_registry(registry_path, second_output)

    assert first_output.read_bytes() == second_output.read_bytes()


def test_swift_registry_codegen_matches_committed_output(
    generated_swift_bytes: bytes,
    committed_swift_registry: Path,
) -> None:
    assert committed_swift_registry.exists(), (
        f"missing committed output: {committed_swift_registry}"
    )
    assert generated_swift_bytes == committed_swift_registry.read_bytes()


def test_swift_registry_codegen_replaces_existing_output_atomically(
    registry_path: Path, tmp_path: Path
) -> None:
    output_path = tmp_path / "generated" / "RegistryTables.swift"
    output_path.parent.mkdir()
    output_path.write_text("stale output\n", encoding="utf-8")
    output_path.chmod(0o640)

    generate_swift_registry(registry_path, output_path)

    assert output_path.stat().st_mode & 0o777 == 0o640
    assert output_path.read_bytes() != b"stale output\n"
    assert output_path.read_text(encoding="utf-8").endswith("\n")
    assert list(output_path.parent.glob(f".{output_path.name}.*.tmp")) == []


def test_swift_registry_codegen_preserves_previous_output_when_replace_fails(
    registry_path: Path, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    output_path = tmp_path / "RegistryTables.swift"
    original = b"known-good committed output\n"
    output_path.write_bytes(original)

    def fail_replace(_source: Path, _destination: Path) -> None:
        raise OSError("simulated atomic replace failure")

    monkeypatch.setattr(registry_codegen.os, "replace", fail_replace)

    with pytest.raises(OSError, match="simulated atomic replace failure"):
        generate_swift_registry(registry_path, output_path)

    assert output_path.read_bytes() == original
    assert list(tmp_path.glob(f".{output_path.name}.*.tmp")) == []


def test_swift_registry_codegen_invalid_input_never_touches_existing_output(
    tmp_path: Path,
) -> None:
    registry_path = tmp_path / "invalid.yaml"
    registry_path.write_text("node_types: [unterminated\n", encoding="utf-8")
    output_path = tmp_path / "RegistryTables.swift"
    original = b"known-good output\n"
    output_path.write_bytes(original)

    with pytest.raises(Exception):
        generate_swift_registry(registry_path, output_path)

    assert output_path.read_bytes() == original
    assert list(tmp_path.glob(f".{output_path.name}.*.tmp")) == []
