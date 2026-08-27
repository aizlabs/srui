from __future__ import annotations


def validate_category_sequence(items: list, category_name: str, errors: list) -> tuple[dict, dict, int]:
    """Validate IDs and names in a flat category list."""
    ids_seen: dict[int, str] = {}
    names_seen: dict[str, int] = {}
    max_id = 0

    for idx, item in enumerate(items):
        if not isinstance(item, dict):
            errors.append(f"[{category_name}] Item {idx} is not a valid dictionary object.")
            continue

        item_id = item.get("id")
        name = item.get("name")

        if item_id is None:
            errors.append(f"[{category_name}] Entry '{name}' is missing an 'id'.")
            continue

        if not isinstance(item_id, int) or item_id < 1:
            errors.append(f"[{category_name}] Entry '{name}' has invalid non-positive ID: {item_id}.")
            continue

        if item_id in ids_seen:
            errors.append(
                f"[{category_name}] Duplicate ID {item_id} found on '{name}' "
                f"(previously on '{ids_seen[item_id]}')."
            )
        else:
            ids_seen[item_id] = name

        if name:
            if name in names_seen:
                errors.append(
                    f"[{category_name}] Duplicate name '{name}' found "
                    f"(IDs {names_seen[name]} and {item_id})."
                )
            else:
                names_seen[name] = item_id

        if item_id > max_id:
            max_id = item_id

    expected_ids = set(range(1, max_id + 1))
    actual_ids = set(ids_seen.keys())
    missing_ids = expected_ids - actual_ids
    if missing_ids:
        errors.append(
            f"[{category_name}] Accidental gap detected! Missing IDs in contiguous 1..{max_id} "
            f"sequence: {sorted(missing_ids)}"
        )

    return names_seen, ids_seen, max_id
