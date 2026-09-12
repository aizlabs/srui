#!/usr/bin/env python3
"""Validate the versioned PX-000 htop feature ledger."""

from __future__ import annotations

import json
import re
from urllib.parse import urlsplit
from pathlib import Path
from typing import Any

REQUIRED_ENTRY_FIELDS = {
    "feature_id",
    "feature",
    "scope",
    "source_refs",
    "owner",
    "status",
    "evidence",
}
ALLOWED_STATUS = {"planned", "in_progress", "verified", "deferred"}


def valid_evidence(item: Any) -> bool:
    """Evidence is a URL/file reference, or a record with a reference field.

    This checks reference structure, not whether the cited artifact proves parity.
    """
    reference = item.get("reference") if isinstance(item, dict) else item
    if not isinstance(reference, str) or not reference.strip():
        return False
    reference = reference.strip()
    if any(character.isspace() for character in reference):
        return False
    try:
        url = urlsplit(reference)
    except ValueError:
        return False
    if url.scheme:
        return url.scheme in {"http", "https"} and bool(url.hostname)
    # Local artifact references need a filename extension; bare claims and
    # placeholders such as "TODO", "pending", and "N/A" are not evidence.
    return bool(re.fullmatch(r"[^?#]+\.[A-Za-z0-9]+(?:#[^\s]+)?", reference))


def validate_ledger(
    document: dict[str, Any], ticket_ids: set[str] | None = None
) -> list[str]:
    errors: list[str] = []
    if ticket_ids is None:
        try:
            index = json.loads(
                Path(__file__).with_name("task-index.json").read_text(encoding="utf-8")
            )
            ticket_ids = {task["id"] for task in index["tasks"]}
        except (OSError, json.JSONDecodeError, KeyError, TypeError) as error:
            return [f"cannot read ticket index: {error}"]
    if document.get("schema_version") != 1:
        errors.append("schema_version must be 1")
    upstream = document.get("upstream")
    if not isinstance(upstream, dict):
        errors.append("upstream metadata is required")
        upstream = {}
    references = upstream.get("references")
    if not isinstance(references, dict) or not references:
        errors.append("upstream.references must be a non-empty object")
        references = {}
    entries = document.get("entries")
    if not isinstance(entries, list) or not entries:
        errors.append("entries must be a non-empty array")
        entries = []

    seen: set[str] = set()
    for index, entry in enumerate(entries):
        prefix = f"entry {index + 1}"
        if not isinstance(entry, dict):
            errors.append(f"{prefix} must be an object")
            continue
        missing = REQUIRED_ENTRY_FIELDS - entry.keys()
        if missing:
            errors.append(f"{prefix} missing fields: {', '.join(sorted(missing))}")
        feature_id = entry.get("feature_id")
        if not isinstance(feature_id, str) or not feature_id:
            errors.append(f"{prefix} has an invalid feature_id")
        elif feature_id in seen:
            errors.append(f"duplicate feature_id: {feature_id}")
        else:
            seen.add(feature_id)
        refs = entry.get("source_refs")
        if not isinstance(refs, list) or not refs:
            errors.append(f"{prefix} must cite at least one source reference")
        else:
            for ref in refs:
                if not isinstance(ref, str) or ref not in references:
                    errors.append(f"{prefix} has missing upstream reference: {ref}")
        status = entry.get("status")
        if status not in ALLOWED_STATUS:
            errors.append(f"{prefix} has invalid status: {status}")
        evidence = entry.get("evidence")
        if not isinstance(evidence, list):
            errors.append(f"{prefix} evidence must be an array")
        else:
            if status == "verified" and not evidence:
                errors.append(f"{prefix} verified entry has no evidence")
            for item in evidence:
                if not valid_evidence(item):
                    errors.append(f"{prefix} has invalid evidence reference: {item!r}")
        owner = entry.get("owner")
        if not isinstance(owner, str) or not owner.strip():
            errors.append(f"{prefix} must have an owner")
        else:
            for ticket_id in owner.split("/"):
                ticket_id = ticket_id.strip()
                if ticket_id not in ticket_ids:
                    errors.append(f"{prefix} has unknown owner: {ticket_id!r}")

    return errors


def main() -> int:
    path = Path(__file__).with_name("feature-ledger.px000.json")
    try:
        document = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        print(f"FAIL: cannot read ledger: {error}")
        return 1
    errors = validate_ledger(document)
    if errors:
        for error in errors:
            print(f"FAIL: {error}")
        return 1
    verified = sum(entry.get("status") == "verified" for entry in document["entries"])
    print(f"PASS: {len(document['entries'])} ledger entries; {verified} verified; references and evidence valid.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
