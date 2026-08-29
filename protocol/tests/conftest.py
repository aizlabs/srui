from __future__ import annotations

import copy
import sys
from pathlib import Path

import pytest

from validate.loader import load_registry

PROTOCOL_DIR = Path(__file__).resolve().parent.parent
if str(PROTOCOL_DIR) not in sys.path:
    sys.path.insert(0, str(PROTOCOL_DIR))

REPO_ROOT = PROTOCOL_DIR.parent
REGISTRY_PATH = PROTOCOL_DIR / "registry.yaml"
COMMITTED_SWIFT_REGISTRY = REPO_ROOT / "client-macos" / "SemanticModel" / "RegistryTables.swift"


@pytest.fixture
def registry_path() -> Path:
    return REGISTRY_PATH


@pytest.fixture
def committed_swift_registry() -> Path:
    return COMMITTED_SWIFT_REGISTRY


@pytest.fixture
def canonical_registry(registry_path: Path) -> dict:
    return load_registry(registry_path)


@pytest.fixture
def registry_copy(canonical_registry: dict) -> dict:
    return copy.deepcopy(canonical_registry)
