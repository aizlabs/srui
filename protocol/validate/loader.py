from __future__ import annotations

from pathlib import Path

import yaml


class RegistryLoadError(Exception):
    """Raised when registry YAML cannot be loaded."""


def load_registry(filepath: Path) -> dict:
    with open(filepath, "r", encoding="utf-8") as f:
        content = f.read()

    try:
        registry = yaml.safe_load(content)
    except yaml.YAMLError as exc:
        raise RegistryLoadError(f"Invalid YAML in {filepath}: {exc}") from exc

    if registry is None:
        raise RegistryLoadError(f"Registry file is empty: {filepath}")
    if not isinstance(registry, dict):
        raise RegistryLoadError(f"Registry root must be a mapping: {filepath}")

    return registry
