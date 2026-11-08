"""Third, independent oracle for the hierarchical model shape rules (PX-018-G01, §8, §26).

`protocol/conformance-vectors/model_tree_shape_cases.json` is asserted row for row by the Rust store
(`server-rust/semantic-tree/tests/model_tree_test.rs`) and the Swift replica
(`client-macos/Tests/SemanticModelTests/ModelTreeStoreTests.swift`). This reference recomputes every
expected verdict from the rules alone, so a table edited to match one implementation's bug fails here.
"""

from __future__ import annotations

import json
from pathlib import Path

CASES = Path(__file__).resolve().parents[1] / "conformance-vectors" / "model_tree_shape_cases.json"


def _shape_value(raw: object) -> tuple[int | None, str | None]:
    if raw is None:
        return 0, None
    if isinstance(raw, bool) or not isinstance(raw, int) or raw < 0:
        return None, "invalid_model_tree_property"
    return raw, None


def reference_verdict(item_count: int, max_depth: int, rows: list[list]) -> tuple[str, int] | None:
    """First violation as (conformance code, index), or None when the cache is accepted."""
    if not any(depth is not None or count is not None for _, depth, count in rows):
        return None
    open_spans: list[tuple[int, int, int]] = []
    previous: tuple[int, int] | None = None
    for index, raw_depth, raw_count in sorted(rows, key=lambda row: row[0]):
        depth, error = _shape_value(raw_depth)
        if error:
            return error, index
        descendants, error = _shape_value(raw_count)
        if error:
            return error, index
        assert depth is not None and descendants is not None
        if depth >= max_depth:
            return "max_model_tree_depth_exceeded", index
        if descendants >= max(item_count - index, 0):
            return "model_tree_descendant_count_out_of_range", index
        previous_depth = previous[1] if previous and previous[0] + 1 == index else None
        if (index == 0 and depth != 0) or (previous_depth is not None and depth > previous_depth + 1):
            return "model_tree_depth_jump", index
        while open_spans and open_spans[-1][2] < index:
            _, owner_depth, end = open_spans[-1]
            if end + 1 == index and depth > owner_depth:
                return "model_tree_outside_parent_span", index
            open_spans.pop()
        end = index + descendants
        if open_spans:
            _, owner_depth, owner_end = open_spans[-1]
            if depth <= owner_depth or end > owner_end:
                return "model_tree_outside_parent_span", index
        if descendants > 0:
            open_spans.append((index, depth, end))
        previous = (index, depth)
    return None


def test_every_case_matches_the_reference() -> None:
    doc = json.loads(CASES.read_text(encoding="utf-8"))
    cases = doc["cases"]
    assert len(cases) == 315, "update the Rust and Swift loaders' pinned count with the table"
    names = [case["name"] for case in cases]
    assert len(set(names)) == len(names), "case names must be unique"
    codes = set()
    for case in cases:
        verdict = reference_verdict(case["item_count"], case["max_model_tree_depth"], case["items"])
        expected = case["expected"]
        actual = None if verdict is None else {"code": verdict[0], "index": verdict[1]}
        assert actual == expected, case["name"]
        codes.add("ok" if expected is None else expected["code"])
    # Every rule must be exercised by the table, or a regression in it would pass unseen.
    assert codes == {
        "ok",
        "invalid_model_tree_property",
        "max_model_tree_depth_exceeded",
        "model_tree_descendant_count_out_of_range",
        "model_tree_depth_jump",
        "model_tree_outside_parent_span",
    }
