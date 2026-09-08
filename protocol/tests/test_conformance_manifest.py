"""Validity of the §32 conformance suite manifest and its generated fixtures.

These run in the cheap Linux `registry` CI job rather than only inside the macOS conformance
runner, so a malformed manifest or a stale generated fixture fails in seconds.
"""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from generate_conformance_matrix import (
    APPKIT_MAPPINGS,
    build_event_matrix,
    build_toolkit_mappings,
    build_widget_matrix,
    generate_conformance_matrix,
)

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
# Generated conformance matrix (§32 suites 2, 5, 12)
# ---------------------------------------------------------------------------


def test_conformance_matrix_codegen_is_deterministic(registry_path: Path, tmp_path: Path) -> None:
    first = generate_conformance_matrix(registry_path, tmp_path / "first")
    second = generate_conformance_matrix(registry_path, tmp_path / "second")

    for (first_path, first_payload), (_, second_payload) in zip(
        sorted(first.items()), sorted(second.items())
    ):
        assert first_payload == second_payload, f"non-deterministic output for {first_path.name}"


def test_conformance_matrix_matches_committed_output(registry_path: Path, tmp_path: Path) -> None:
    """The freshness gate CI also enforces via `git diff` after generate_proto.sh."""
    generated = generate_conformance_matrix(registry_path, tmp_path)

    for path, _ in generated.items():
        committed = SUITES_DIR / path.relative_to(tmp_path)
        assert committed.exists(), f"missing committed fixture: {committed}"
        assert path.read_bytes() == committed.read_bytes(), (
            f"{committed} is stale; run ./protocol/generate_proto.sh and commit the result"
        )


def test_appkit_mapping_table_covers_every_registry_node_type(canonical_registry: dict) -> None:
    """A registry node type with no mapping must fail codegen, not produce a partial fixture."""
    registry_names = {entry["name"] for entry in canonical_registry["node_types"]}
    missing = registry_names - set(APPKIT_MAPPINGS)
    assert not missing, (
        f"APPKIT_MAPPINGS is missing {sorted(missing)}; add an informative mapping in "
        "protocol/generate_conformance_matrix.py"
    )

    stale = set(APPKIT_MAPPINGS) - registry_names
    assert not stale, f"APPKIT_MAPPINGS has entries for unknown node types: {sorted(stale)}"


def test_widget_matrix_is_derived_from_the_registry(canonical_registry: dict) -> None:
    matrix = build_widget_matrix(canonical_registry)
    registry_by_name = {e["name"]: e for e in canonical_registry["node_types"]}

    assert len(matrix["node_types"]) == len(registry_by_name)
    for row in matrix["node_types"]:
        source = registry_by_name[row["name"]]
        assert row["id"] == source["id"]
        assert row["tier"] == source["tier"]
        assert row["category"] == source["category"]
        assert row["emits"] == list(source["emits"])


def test_event_matrix_forbids_coordinates_for_every_standard_node(canonical_registry: dict) -> None:
    matrix = build_event_matrix(canonical_registry)

    assert matrix["coordinate_events"], "§7.7 declares a coordinate event family"
    for row in matrix["node_event_matrix"]:
        assert row["forbidden"] == matrix["coordinate_events"]
        overlap = set(row["allowed"]) & set(matrix["coordinate_events"])
        assert not overlap, (
            f"node '{row['node']}' is allowed to emit coordinate events {sorted(overlap)}; "
            "coordinates are reserved for subscribed custom scenes (§7.7, §32.5)"
        )


def test_toolkit_mapping_is_marked_informative(canonical_registry: dict) -> None:
    mappings = build_toolkit_mappings(canonical_registry)
    assert mappings["normative"] is False, "§22.4 makes native mappings informative"
    assert len(mappings["mappings"]) == len(canonical_registry["node_types"])


def test_generator_fails_closed_on_an_unmapped_node_type(canonical_registry: dict) -> None:
    """Adding a node type without an AppKit mapping must break codegen loudly."""
    registry = dict(canonical_registry)
    registry["node_types"] = canonical_registry["node_types"] + [
        {
            "id": 999,
            "name": "HypotheticalWidget",
            "tier": "should",
            "category": "control",
            "emits": [],
            "description": "not in APPKIT_MAPPINGS",
        }
    ]

    with pytest.raises(KeyError, match="HypotheticalWidget"):
        build_toolkit_mappings(registry)
