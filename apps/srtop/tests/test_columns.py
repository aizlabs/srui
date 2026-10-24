"""PX-068 bridge: the column chooser's Rust acceptance suite, run from pytest.

`apps/srtop/tests/columns_test.rs` drives the published session: showing,
hiding, reordering and restoring the default columns, both through
`ProcessView` and by activating the published chooser buttons through the
session's event path, each checked against the table's headings and every row's
cells, with item identities, the table and model a native selection is anchored
to, and the active sort preserved, and with a client's selection — replayed
through every committed transaction as the native table adapter applies them —
still on the same process. This test builds and runs exactly that suite.

Cargo builds into a directory under the system temporary directory, never into
`apps/srtop/target`, so running this test leaves the worktree untouched.
"""

from __future__ import annotations

import os
import re
import shutil
import subprocess
import tempfile
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[3]
MANIFEST = REPO_ROOT / "apps" / "srtop" / "Cargo.toml"
RUST_SUITE = REPO_ROOT / "apps" / "srtop" / "tests" / "columns_test.rs"
TARGET_DIR = Path(tempfile.gettempdir()) / "srtop-px068-columns-target"

# Every case of the Rust suite; each must run and pass.
CASES = (
    "the_default_layout_is_the_catalogued_process_list",
    "hiding_a_column_drops_its_heading_and_its_cell_from_every_row",
    "reordering_columns_projects_cells_in_the_new_sequence",
    "a_hidden_column_can_be_shown_again_at_a_chosen_position",
    "restoring_defaults_brings_back_the_catalogued_layout",
    "a_refresh_publishes_new_samples_in_the_chosen_layout",
    "hiding_the_sorted_column_keeps_the_sort_and_every_identity",
    "a_layout_naming_a_column_twice_is_rejected_and_publishes_nothing",
    "reordering_columns_under_an_active_sort_keeps_the_order_and_every_identity",
    "chooser_controls_hide_reorder_show_and_restore_through_the_session",
    "a_replayed_chooser_click_changes_nothing",
    "chooser_clicks_under_an_active_sort_keep_the_sort_and_every_identity",
    "wide_columns_keep_snapshot_delivery_membership_and_selection_after_restore",
)


def run_columns_suite() -> subprocess.CompletedProcess[str]:
    env = dict(os.environ)
    env["CARGO_TARGET_DIR"] = str(TARGET_DIR)
    return subprocess.run(
        [
            "cargo",
            "test",
            "--locked",
            "--manifest-path",
            str(MANIFEST),
            "--test",
            "columns_test",
            "--",
            "--test-threads=1",
        ],
        cwd=REPO_ROOT,
        env=env,
        capture_output=True,
        text=True,
        timeout=1800,
    )


@pytest.fixture(scope="module")
def columns_suite() -> subprocess.CompletedProcess[str]:
    if shutil.which("cargo") is None:
        pytest.fail("cargo is required to run the PX-068 column chooser suite")
    return run_columns_suite()


def first_errors(output: str) -> str:
    """The distinct compiler errors and failed cases, first, so they survive
    pytest's truncation of a long assertion message."""
    seen: list[str] = []
    for line in output.splitlines():
        line = line.strip()
        if (line.startswith("error[") or line.startswith("---- ")) and line not in seen:
            seen.append(line)
    return "; ".join(seen)


def test_column_chooser_suite_passes(columns_suite: subprocess.CompletedProcess[str]) -> None:
    output = columns_suite.stdout + columns_suite.stderr
    assert columns_suite.returncode == 0, (
        f"the PX-068 column chooser suite failed: {first_errors(output)}\n{output[-60000:]}"
    )
    for case in CASES:
        assert f"test {case} ... ok" in output, f"{case} did not run and pass:\n{output[-60000:]}"
    unlisted = sorted(set(re.findall(r"^test (\w+) \.\.\. ok$", output, re.MULTILINE)) - set(CASES))
    assert f"{len(CASES)} passed" in output, (
        f"the suite ran cases the bridge does not list: {unlisted}\n{output[-60000:]}"
    )


def rust_cases() -> list[str]:
    """Every `#[test]` function declared in the Rust column chooser suite."""
    source = RUST_SUITE.read_text(encoding="utf-8")
    return re.findall(r"#\[test\]\s*fn\s+(\w+)\s*\(", source)


def test_bridge_lists_exactly_the_rust_suite_cases() -> None:
    declared = rust_cases()
    assert declared, f"no #[test] cases found in {RUST_SUITE}"
    assert len(set(CASES)) == len(CASES), f"the bridge lists a case twice: {CASES}"
    missing = sorted(set(declared) - set(CASES))
    assert not missing, f"the bridge does not list Rust suite cases: {missing}"
    extra = sorted(set(CASES) - set(declared))
    assert not extra, f"the bridge lists cases the Rust suite does not declare: {extra}"
    assert len(CASES) == len(declared) == 13, (
        f"expected the 13-case column chooser suite, bridge lists {len(CASES)}, "
        f"Rust declares {len(declared)}"
    )
