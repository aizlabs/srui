"""Validity of the §32 conformance suite manifest and its generated fixtures.

These run in the cheap Linux `registry` CI job rather than only inside the macOS conformance
runner, so a malformed manifest or a stale generated fixture fails in seconds.
"""

from __future__ import annotations

import json
import re
from pathlib import Path

import pytest

PROTOCOL_DIR = Path(__file__).resolve().parent.parent
REPO_ROOT = PROTOCOL_DIR.parent
VECTORS_DIR = PROTOCOL_DIR / "conformance-vectors"
SUITES_DIR = VECTORS_DIR / "suites"
MANIFEST_PATH = SUITES_DIR / "manifest.json"

EXPECTED_SUITE_COUNT = 12


@pytest.fixture
def manifest() -> dict:
    return json.loads(MANIFEST_PATH.read_text(encoding="utf-8"))


# ---------------------------------------------------------------------------
# Manifest shape (§32)
# ---------------------------------------------------------------------------


def test_manifest_declares_exactly_twelve_suites(manifest: dict) -> None:
    assert len(manifest["suites"]) == EXPECTED_SUITE_COUNT


def test_suite_ids_are_exactly_one_through_twelve(manifest: dict) -> None:
    ids = sorted(suite["id"] for suite in manifest["suites"])
    assert ids == list(range(1, EXPECTED_SUITE_COUNT + 1))


def test_suite_slugs_and_names_are_unique(manifest: dict) -> None:
    slugs = [suite["slug"] for suite in manifest["suites"]]
    names = [suite["name"] for suite in manifest["suites"]]
    assert len(set(slugs)) == len(slugs)
    assert len(set(names)) == len(names)


def test_every_suite_cites_spec_sections(manifest: dict) -> None:
    for suite in manifest["suites"]:
        sections = suite["spec_sections"]
        assert sections, f"suite {suite['id']} cites no spec sections"
        assert all(s.startswith("§") for s in sections), (
            f"suite {suite['id']} has a malformed spec section reference: {sections}"
        )


def test_every_suite_has_a_valid_status(manifest: dict) -> None:
    for suite in manifest["suites"]:
        assert suite["status"] in {"active", "known_gap"}


def test_every_gap_documents_a_reason_and_owning_task(manifest: dict) -> None:
    for suite in manifest["suites"]:
        for gap in suite.get("gaps", []):
            assert gap.get("scenario"), f"suite {suite['id']} gap has no scenario"
            assert gap.get("reason"), f"suite {suite['id']} gap has no reason"
            assert gap.get("future_task"), f"suite {suite['id']} gap has no future_task"


def test_every_gap_carries_a_closure_probe(manifest: dict) -> None:
    """A gap with no probe can be silently fixed while the runner still prints GAP."""
    for suite in manifest["suites"]:
        for gap in suite.get("gaps", []):
            probe = gap.get("gap_probe")
            assert probe, (
                f"suite {suite['id']} gap '{gap['scenario'][:60]}...' has no gap_probe"
            )
            assert probe.get("file") and probe.get("absent_pattern")
            target = REPO_ROOT / probe["file"]
            assert target.is_file(), (
                f"suite {suite['id']} gap_probe targets {probe['file']}, which does not exist; "
                "a probe that can never match cannot detect closure"
            )
            # The probe must not already match, or the gap is stale.
            assert not re.search(probe["absent_pattern"], target.read_text(encoding="utf-8")), (
                f"suite {suite['id']} gap_probe already matches {probe['file']}: the gap looks "
                "closed and the manifest needs updating"
            )


def test_every_suite_accounts_for_both_implementations(manifest: dict) -> None:
    """A suite must declare a runner per language, or record why that language is not applicable.

    Without this, one language silently covers for the other's absence in a `both` run.
    """
    for suite in manifest["suites"]:
        not_applicable = suite.get("not_applicable", {})
        for language in ("rust", "swift"):
            has_runner = bool(suite.get(language))
            excused = language in not_applicable
            assert has_runner or excused, (
                f"suite {suite['id']} declares no {language} runner and does not record "
                f"{language} as not applicable"
            )
            assert not (has_runner and excused), (
                f"suite {suite['id']} both declares a {language} runner and calls it not applicable"
            )
            if excused:
                assert not_applicable[language].strip(), (
                    f"suite {suite['id']} must explain why {language} is not applicable"
                )


def test_known_gap_suites_document_their_gap(manifest: dict) -> None:
    for suite in manifest["suites"]:
        if suite["status"] == "known_gap":
            assert suite.get("gaps"), (
                f"suite {suite['id']} is known_gap but documents no gap; a suite must never be "
                "silently absent (§32)"
            )


def test_every_suite_has_at_least_one_runner_or_is_a_known_gap(manifest: dict) -> None:
    for suite in manifest["suites"]:
        runners = suite.get("rust", []) + suite.get("swift", [])
        if suite["status"] == "active":
            assert runners, f"active suite {suite['id']} declares no runner"


def test_runner_commands_are_argument_arrays(manifest: dict) -> None:
    """Commands must be argv arrays so the runner never shell-evaluates manifest content."""
    for suite in manifest["suites"]:
        for language in ("rust", "swift"):
            for command in suite.get(language, []):
                assert isinstance(command, list) and command, (
                    f"suite {suite['id']} {language} command must be a non-empty array"
                )
                assert all(isinstance(arg, str) for arg in command)


# ---------------------------------------------------------------------------
# Declared paths must exist
# ---------------------------------------------------------------------------


def test_declared_vector_directories_exist(manifest: dict) -> None:
    for suite in manifest["suites"]:
        vectors = suite.get("vectors")
        if not vectors:
            continue
        directory = VECTORS_DIR / vectors["dir"]
        assert directory.is_dir(), (
            f"suite {suite['id']} declares vector dir {vectors['dir']} which does not exist"
        )


def test_declared_vector_counts_match_the_tree(manifest: dict) -> None:
    """The exact-count contract the Rust and Swift loaders also enforce.

    Checked in both directions: a missing file means coverage was lost in a move, an extra file
    means a fixture no runner accounts for.
    """
    for suite in manifest["suites"]:
        vectors = suite.get("vectors")
        if not vectors or vectors.get("count") is None:
            continue
        directory = VECTORS_DIR / vectors["dir"]
        found = sorted(p.name for p in directory.glob("*.json"))
        assert len(found) == vectors["count"], (
            f"suite {suite['id']} declares {vectors['count']} vectors but {vectors['dir']} "
            f"contains {len(found)}: {found}"
        )


def test_generated_fixtures_exist(manifest: dict) -> None:
    for suite in manifest["suites"]:
        vectors = suite.get("vectors")
        if not vectors:
            continue
        for generated in vectors.get("generated", []):
            path = VECTORS_DIR / vectors["dir"] / generated
            assert path.is_file(), (
                f"suite {suite['id']} declares generated fixture {generated} which does not "
                "exist; run ./protocol/generate_proto.sh"
            )


def test_every_suite_has_a_readme(manifest: dict) -> None:
    """No suite may be silently absent from the tree, including code-driven ones."""
    readmes = {p.parent.name for p in SUITES_DIR.glob("*/README.md")}
    assert len(readmes) == EXPECTED_SUITE_COUNT, (
        f"expected a README per suite directory, found {sorted(readmes)}"
    )


def test_the_49_original_state_machine_vectors_are_all_still_present(manifest: dict) -> None:
    """Suites 1 and 3 together must still hold every vector from the pre-split layout."""
    suite_one = next(s for s in manifest["suites"] if s["id"] == 1)
    suite_three = next(s for s in manifest["suites"] if s["id"] == 3)
    total = suite_one["vectors"]["count"] + suite_three["vectors"]["count"]
    assert total == 49, f"the core state-machine corpus is 49 vectors, manifest accounts for {total}"


# ---------------------------------------------------------------------------
# Registry-derived event matrix (§7.6)
# ---------------------------------------------------------------------------


def test_emits_matches_the_section_7_6_table(canonical_registry: dict) -> None:
    """`emits` transcribes §7.6 exactly; nothing may be added without amending the design doc.

    This is the one place the §7.6 table is restated, and it is restated against the registry
    rather than against a generated fixture. Suites 2 and 5 assert behaviour, not this table.
    """
    expected = {
        "Surface": ["VIEWPORT_CHANGED"],
        "Button": ["ACTIVATE"],
        "Toggle": ["VALUE_CHANGED"],
        "TextInput": ["TEXT_EDIT"],
        "TextArea": ["TEXT_EDIT"],
        "List": ["SELECTION_CHANGED"],
        "Table": ["SELECTION_CHANGED"],
        "Tree": ["SELECTION_CHANGED", "EXPANSION_CHANGED"],
        "Select": ["SELECTION_CHANGED"],
        "ChoiceGroup": ["SELECTION_CHANGED"],
        "Slider": ["VALUE_CHANGED"],
        "NumberInput": ["VALUE_CHANGED"],
        "Tabs": ["SELECTION_CHANGED"],
        "Split": ["VALUE_CHANGED"],
    }

    for entry in canonical_registry["node_types"]:
        declared = list(entry.get("emits", []))
        assert declared == expected.get(entry["name"], []), (
            f"node type '{entry['name']}' declares emits={declared}, which disagrees with the "
            "§7.6 Standard events table"
        )


def test_coordinate_events_are_never_emitted_by_a_node_type(canonical_registry: dict) -> None:
    coordinate = {
        event["name"]
        for event in canonical_registry["events"]
        if event.get("kind") == "coordinate"
    }
    assert coordinate, "§7.7 declares a coordinate event family"

    for entry in canonical_registry["node_types"]:
        overlap = set(entry.get("emits", [])) & coordinate
        assert not overlap, (
            f"node type '{entry['name']}' emits coordinate events {sorted(overlap)}; coordinates "
            "are reserved for subscribed custom scenes (§7.7, §32.5)"
        )


def test_the_coordinate_family_matches_the_suite_constants(canonical_registry: dict) -> None:
    """Cross-checks the constant both language suites hard-code against the registry."""
    coordinate = sorted(
        event["name"]
        for event in canonical_registry["events"]
        if event.get("kind") == "coordinate"
    )
    assert coordinate == [
        "POINTER_CANCEL",
        "POINTER_DOWN",
        "POINTER_MOVE",
        "POINTER_SCROLL",
        "POINTER_UP",
    ]
