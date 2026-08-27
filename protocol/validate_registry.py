#!/usr/bin/env python3
"""Backward-compatible entry point for SRUI registry validation."""

from validate.cli import main, validate_registry

__all__ = ["main", "validate_registry"]

if __name__ == "__main__":
    raise SystemExit(main())
