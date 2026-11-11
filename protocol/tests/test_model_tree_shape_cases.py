"""Oracles for the hierarchical model shape rules (PX-018-G01, §8, §26).

`protocol/conformance-vectors/model_tree_shape_cases.json` is asserted row for row by the Rust store
(`server-rust/semantic-tree/tests/model_tree_test.rs`) and the Swift replica
(`client-macos/Tests/SemanticModelTests/ModelTreeStoreTests.swift`). This file holds three oracles:

- `reference_verdict` restates the stores' algorithm, so the table pins the exact first violation
  (code and index) both stores must report;
- `forest_consistent` is a *true-forest* oracle that does not share that algorithm: a cache is
  valid iff some forest of `item_count` items (roots at depth 0, every child exactly one level
  below its parent, every depth below the limit) agrees with every cached item. It is solved as
  per-index depth bounds plus the preorder ramp `d[j+1] <= d[j] + 1`;
- `brute_force_forests` enumerates real forests from parent pointers and subtree sizes, and
  checks `forest_consistent` on every small cache.

Every table verdict must agree with the true-forest oracle, and an exhaustive sweep of every cache
of at most four items (full and sparse) checks the reference against brute force. Round 1 of the
independent verification found a false accept (a deeper item after a cached leaf) that a
restated-algorithm oracle could not catch; these oracles exist so that cannot recur.
"""

from __future__ import annotations

import json
from pathlib import Path

CASES = Path(__file__).resolve().parents[1] / "conformance-vectors" / "model_tree_shape_cases.json"
SWEEP_DEPTHS = (0, 1, 2, 3)
SWEEP_LIMITS = (3, 4)


def _shape_value(raw: object) -> int | None:
    if raw is None:
        return 0
    if isinstance(raw, bool) or not isinstance(raw, int) or raw < 0:
        return None
    return raw


def reference_verdict(item_count: int, max_depth: int, rows: list) -> tuple[str, int] | None:
    """First violation as (conformance code, index), or None. Mirrors both stores exactly."""
    if not any(depth is not None or count is not None for _, depth, count in rows):
        return None
    spans: list[tuple[int, int, int]] = []  # (owner, depth, last index of span)
    pos, reach = -1, -1  # deepest depth reachable at `pos`
    for index, raw_depth, raw_count in sorted(rows, key=lambda row: row[0]):
        depth = _shape_value(raw_depth)
        if depth is None:
            return "invalid_model_tree_property", index
        count = _shape_value(raw_count)
        if count is None:
            return "invalid_model_tree_property", index
        if depth >= max_depth or (count > 0 and depth + 1 >= max_depth):
            return "max_model_tree_depth_exceeded", index
        if count >= max(item_count - index, 0):
            return "model_tree_descendant_count_out_of_range", index
        bound_here = None
        while spans and spans[-1][2] < index:
            after = spans[-1][2] + 1
            bound = None
            while spans and spans[-1][2] == after - 1:
                bound = spans[-1][1] if bound is None else min(bound, spans[-1][1])
                spans.pop()
            if after == index:
                bound_here = bound
            else:
                reach = min(bound, max_depth - 1, reach + (after - pos))
                pos = after
        if depth > min(max_depth - 1, reach + (index - pos)):
            return "model_tree_depth_jump", index
        if bound_here is not None and depth > bound_here:
            return "model_tree_outside_parent_span", index
        if spans and (depth <= spans[-1][1] or index + count > spans[-1][2]):
            return "model_tree_outside_parent_span", index
        spans.append((index, depth, index + count))
        pos, reach = index, depth
    return None


def forest_consistent(item_count: int, max_depth: int, rows: list) -> bool:
    """True-forest oracle: some valid forest agrees with every cached item."""
    lower = [0] * item_count
    upper = [max_depth - 1] * item_count
    for index, raw_depth, raw_count in rows:
        depth, count = _shape_value(raw_depth), _shape_value(raw_count)
        if depth is None or count is None or index + count >= item_count:
            return False
        lower[index] = max(lower[index], depth)
        upper[index] = min(upper[index], depth)
        for inside in range(index + 1, index + count + 1):
            lower[inside] = max(lower[inside], depth + 1)
        if index + count + 1 < item_count:
            upper[index + count + 1] = min(upper[index + count + 1], depth)
    deepest = -1
    for j in range(item_count):
        deepest = min(upper[j], deepest + 1)
        if lower[j] > deepest:
            return False
    return True


def brute_force_forests(item_count: int, max_depth: int) -> list[tuple[tuple[int, int], ...]]:
    """Every forest of `item_count` items in preorder as (depth, descendant_count) per item,
    derived from parent pointers and real subtree sizes."""
    forests = []

    def extend(depths: list[int]) -> None:
        if len(depths) == item_count:
            parent: list[int | None] = []
            open_path: list[int] = []
            for i, depth in enumerate(depths):
                while open_path and depths[open_path[-1]] >= depth:
                    open_path.pop()
                parent.append(open_path[-1] if open_path else None)
                open_path.append(i)
            for i, p in enumerate(parent):
                assert (p is None and depths[i] == 0) or (p is not None and depths[p] + 1 == depths[i])
            size = [1] * item_count
            for i in range(item_count - 1, -1, -1):
                if parent[i] is not None:
                    size[parent[i]] += size[i]
            forests.append(tuple((depths[i], size[i] - 1) for i in range(item_count)))
            return
        deepest = 0 if not depths else min(depths[-1] + 1, max_depth - 1)
        for depth in range(deepest + 1):
            extend(depths + [depth])

    if max_depth > 0:
        extend([])
    return forests


def enumerate_sweep(max_items: int):
    """Every cache of 1..max_items items, depths SWEEP_DEPTHS, every descendant count below the
    item count, full and sparse, under each SWEEP_LIMITS. The Rust and Swift sweep hooks
    (`SRUI_MODEL_TREE_SWEEP_OUT`) enumerate in exactly this order, one verdict letter each."""
    for max_depth in SWEEP_LIMITS:
        for n in range(1, max_items + 1):
            base = len(SWEEP_DEPTHS) * n
            for mask in range(1, 1 << n):
                cached = [i for i in range(n) if mask >> i & 1]
                for t in range(base ** len(cached)):
                    rows = []
                    for index in cached:
                        digit = t % base
                        t //= base
                        rows.append((index, SWEEP_DEPTHS[digit // n], digit % n))
                    yield max_depth, n, mask, cached, rows


def test_every_case_matches_the_reference_and_the_true_forest_oracle() -> None:
    doc = json.loads(CASES.read_text(encoding="utf-8"))
    cases = doc["cases"]
    assert len(cases) == 323, "update the Rust and Swift loaders' pinned count with the table"
    names = [case["name"] for case in cases]
    assert len(set(names)) == len(names), "case names must be unique"
    codes = set()
    for case in cases:
        verdict = reference_verdict(case["item_count"], case["max_model_tree_depth"], case["items"])
        actual = None if verdict is None else {"code": verdict[0], "index": verdict[1]}
        assert actual == case["expected"], case["name"]
        assert forest_consistent(
            case["item_count"], case["max_model_tree_depth"], case["items"]
        ) == (case["expected"] is None), f"{case['name']}: verdict disagrees with the true forest"
        codes.add("ok" if case["expected"] is None else case["expected"]["code"])
    assert codes == {
        "ok",
        "invalid_model_tree_property",
        "max_model_tree_depth_exceeded",
        "model_tree_descendant_count_out_of_range",
        "model_tree_depth_jump",
        "model_tree_outside_parent_span",
    }


def test_exhaustive_small_caches_match_brute_force_forests() -> None:
    """Every full and sparse cache of at most four items: no false accept, no false reject."""
    projections = {}
    for max_depth in SWEEP_LIMITS:
        for n in range(1, 5):
            forests = brute_force_forests(n, max_depth)
            for mask in range(1, 1 << n):
                cached = [i for i in range(n) if mask >> i & 1]
                projections[(max_depth, n, mask)] = {
                    tuple(forest[i] for i in cached) for forest in forests
                }
    total = 0
    for max_depth, n, mask, _, rows in enumerate_sweep(4):
        truth = tuple((d, k) for _, d, k in rows) in projections[(max_depth, n, mask)]
        assert (reference_verdict(n, max_depth, rows) is None) == truth, (max_depth, n, rows)
        assert forest_consistent(n, max_depth, rows) == truth, (max_depth, n, rows)
        total += 1
    assert total == 171_600
