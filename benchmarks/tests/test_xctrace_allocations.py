from __future__ import annotations

import importlib.util
from pathlib import Path

import pytest

BENCHMARKS = Path(__file__).resolve().parents[1]
MODULE_PATH = BENCHMARKS / "parse-render/xctrace_allocations.py"
SPEC = importlib.util.spec_from_file_location("xctrace_allocations_tests", MODULE_PATH)
assert SPEC and SPEC.loader
allocations = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(allocations)


def toc_xml(*, pid: int = 321, extra_detail: str = "") -> str:
    return f"""\
<trace-toc>
  <run number="1">
    <info>
      <target>
        <device platform="macOS" os-version="26.4.1 (25E253)" />
        <process type="attached" name="BenchmarkDriver" pid="{pid}" />
      </target>
      <summary>
        <start-date>1970-01-01T00:00:01.123+00:00</start-date>
        <instruments-version>26.0 (17C52)</instruments-version>
        <template-name>Allocations</template-name>
      </summary>
    </info>
    <tracks>
      <track name="Allocations"><details>
        <detail name="Statistics" kind="table" />
        <detail name="Allocations List" kind="table" />
        {extra_detail}
      </details></track>
    </tracks>
  </run>
</trace-toc>
"""


def statistics_xml(
    *,
    persistent_count: int,
    persistent_bytes: int,
    transient_count: int = 2,
    transient_bytes: int = 96,
    total_count: int | None = None,
    total_bytes: int | None = None,
    event_count: int | None = None,
    anonymous_persistent_count: int = 0,
    anonymous_persistent_bytes: int = 0,
) -> str:
    heap = {
        "persistent_allocations": persistent_count,
        "persistent_bytes": persistent_bytes,
        "transient_allocations": transient_count,
        "transient_bytes": transient_bytes,
        "total_allocations": (
            persistent_count + transient_count if total_count is None else total_count
        ),
        "total_bytes": (
            persistent_bytes + transient_bytes if total_bytes is None else total_bytes
        ),
        "event_count": (
            persistent_count + 2 * transient_count
            if event_count is None
            else event_count
        ),
    }
    anonymous_vm = {
        "persistent_allocations": anonymous_persistent_count,
        "persistent_bytes": anonymous_persistent_bytes,
        "transient_allocations": 0,
        "transient_bytes": 0,
        "total_allocations": anonymous_persistent_count,
        "total_bytes": anonymous_persistent_bytes,
        "event_count": anonymous_persistent_count,
    }
    combined = {key: heap[key] + anonymous_vm[key] for key in heap}

    def row(category: str, values: dict[str, int]) -> str:
        return (
            f'<row category="{category}" '
            f'persistent-bytes="{values["persistent_bytes"]}" '
            f'count-persistent="{values["persistent_allocations"]}" '
            f'total-bytes="{values["total_bytes"]}" '
            f'transient-bytes="{values["transient_bytes"]}" '
            f'count-events="{values["event_count"]}" '
            f'count-transient="{values["transient_allocations"]}" '
            f'count-total="{values["total_allocations"]}" />'
        )

    return (
        '<trace-query-result><node xpath="statistics">\n  '
        + row("All Heap &amp; Anonymous VM", combined)
        + "\n  "
        + row("All Heap Allocations", heap)
        + "\n  "
        + row("All Anonymous VM", anonymous_vm)
        + "\n</node></trace-query-result>\n"
    )


def list_xml(rows: list[tuple[str, int, str, str]]) -> str:
    rendered = "\n".join(
        f'<row timestamp="{timestamp}" size="{size}" live="true" '
        f'identifier="{identifier}" category="{category}" />'
        for timestamp, size, identifier, category in rows
    )
    return f'<trace-query-result><node xpath="list">{rendered}</node></trace-query-result>'


def write_exports(
    tmp_path: Path,
    rows: list[tuple[str, int, str, str]],
    *,
    statistics_count: int | None = None,
    statistics_bytes: int | None = None,
) -> tuple[Path, Path]:
    statistics_path = tmp_path / "statistics.xml"
    list_path = tmp_path / "list.xml"
    byte_total = sum(size for _timestamp, size, _identifier, _category in rows)
    statistics_path.write_text(
        statistics_xml(
            persistent_count=(len(rows) if statistics_count is None else statistics_count),
            persistent_bytes=(byte_total if statistics_bytes is None else statistics_bytes),
        ),
        encoding="utf-8",
    )
    list_path.write_text(list_xml(rows), encoding="utf-8")
    return statistics_path, list_path


def summarize(
    tmp_path: Path,
    rows: list[tuple[str, int, str, str]],
    *,
    statistics_count: int | None = None,
    statistics_bytes: int | None = None,
) -> dict[str, object]:
    statistics_path, list_path = write_exports(
        tmp_path,
        rows,
        statistics_count=statistics_count,
        statistics_bytes=statistics_bytes,
    )
    return allocations.summarize_allocation_exports(
        statistics_path=statistics_path,
        allocations_list_path=list_path,
        toc_metadata={
            "attached_pid": 321,
            "attached_process_name": "BenchmarkDriver",
        },
        target_pid=321,
        target_birth_unix_ns=9_000_000_000,
        observed_alive_through_unix_ns=12_000_000_000,
        measurement_started_unix_ns=10_000_000_000,
        measurement_ended_unix_ns=11_000_000_000,
    )


def test_toc_requires_exact_attached_pid_and_view_details() -> None:
    metadata = allocations.validate_allocation_toc(toc_xml(), requested_pid=321)
    assert metadata["attached_pid"] == 321
    assert metadata["attached_process_name"] == "BenchmarkDriver"
    assert "trace_started_unix_ns" not in metadata
    assert "timestamp_boundary_uncertainty_ns" not in metadata
    assert metadata["statistics_xpath"] == allocations.STATISTICS_XPATH
    assert metadata["allocations_list_xpath"] == allocations.ALLOCATIONS_LIST_XPATH

    with pytest.raises(allocations.AllocationExportError, match="does not match"):
        allocations.validate_allocation_toc(toc_xml(pid=999), requested_pid=321)
    duplicate = toc_xml(
        extra_detail='<detail name="Allocations List" kind="table" />'
    )
    with pytest.raises(allocations.AllocationExportError, match="exactly once"):
        allocations.validate_allocation_toc(duplicate, requested_pid=321)


def test_toc_does_not_accept_generic_allocation_schema_as_view_evidence() -> None:
    toc = toc_xml().replace(
        '<detail name="Allocations List" kind="table" />',
        '<detail name="Other" kind="table" />',
    )
    with pytest.raises(allocations.AllocationExportError, match="Allocations List"):
        allocations.validate_allocation_toc(toc, requested_pid=321)


def test_statistics_parses_and_validates_internal_arithmetic(tmp_path: Path) -> None:
    path = tmp_path / "statistics.xml"
    path.write_text(
        statistics_xml(persistent_count=11, persistent_bytes=4096),
        encoding="utf-8",
    )
    result = allocations.parse_allocation_statistics(path)
    assert result["heap_and_anonymous_vm"]["persistent_allocations"] == 11
    assert result["heap_and_anonymous_vm"]["event_count"] == 15

    path.write_text(
        statistics_xml(
            persistent_count=2,
            persistent_bytes=64,
            total_count=99,
        ),
        encoding="utf-8",
    )
    with pytest.raises(allocations.AllocationExportError, match="count Statistics"):
        allocations.parse_allocation_statistics(path)


def test_allocation_list_parses_elapsed_values_but_only_as_row_metadata(
    tmp_path: Path,
) -> None:
    path = tmp_path / "list.xml"
    path.write_text(
        list_xml(
            [
                ("00:00.000.000", 48, "0", "Malloc"),
                ("01:02.003.004", 64, "1", "Malloc"),
            ]
        ),
        encoding="utf-8",
    )
    assert allocations.parse_allocation_list(path)[1]["timestamp_relative_ns"] == 62_003_004_000

    path.write_text(
        list_xml([("00:00.001.000", 16, "1", "Malloc")]).replace(
            'live="true"', 'live="false"'
        ),
        encoding="utf-8",
    )
    with pytest.raises(allocations.AllocationExportError, match="not a live"):
        allocations.parse_allocation_list(path)


@pytest.mark.parametrize(
    "xml, message",
    [
        (
            '<trace-query-result><node><row timestamp="bad" size="1" '
            'live="true" identifier="1" category="Malloc" /></node></trace-query-result>',
            "unsupported format",
        ),
        (
            '<trace-query-result><node><row timestamp="00:00.001.000" size="1" '
            'live="true" identifier="1" category="Malloc" />'
            '<row timestamp="00:00.002.000" size="2" live="true" '
            'identifier="1" category="Malloc" /></node></trace-query-result>',
            "duplicate identifier",
        ),
    ],
)
def test_allocation_list_rejects_malformed_rows(
    tmp_path: Path,
    xml: str,
    message: str,
) -> None:
    path = tmp_path / "list.xml"
    path.write_text(xml, encoding="utf-8")
    with pytest.raises(allocations.AllocationExportError, match=message):
        allocations.parse_allocation_list(path)


def test_summary_reports_all_final_live_rows_without_interval_attribution(
    tmp_path: Path,
) -> None:
    rows = [
        ("00:00.000.000", 100, "baseline", "Malloc"),
        ("99:00:00.000.000", 40, "well-after-workload-clock", "Malloc"),
    ]
    summary = summarize(
        tmp_path,
        rows,
        statistics_count=3,
        statistics_bytes=172,
    )
    assert summary["final_live_list"] == {
        "allocations": 2,
        "bytes": 140,
        "vm_category_allocations": 0,
        "vm_category_bytes": 0,
        "vm_categories": {},
        "maximum_elapsed_timestamp_ns": 356_400_000_000_000,
    }
    assert summary["statistics_minus_final_live_list"] == {
        "persistent_allocations": 1,
        "persistent_bytes": 32,
    }
    assert summary["workload_window"]["used_for_allocation_attribution"] is False
    assert "retained_allocations" not in summary
    assert summary["exact_process_identity"]["pid"] == 321


def test_summary_preserves_signed_negative_discrepancy(tmp_path: Path) -> None:
    summary = summarize(
        tmp_path,
        [
            ("00:00.001.000", 16, "one", "Malloc"),
            ("00:00.002.000", 32, "two", "Malloc"),
        ],
        statistics_count=1,
        statistics_bytes=8,
    )
    assert summary["statistics_minus_final_live_list"] == {
        "persistent_allocations": -1,
        "persistent_bytes": -40,
    }


def test_summary_keeps_vm_categories_as_final_list_diagnostics(tmp_path: Path) -> None:
    summary = summarize(
        tmp_path,
        [
            ("00:00.001.000", 16, "heap", "Malloc"),
            ("00:00.001.500", 4096, "vm", "VM: Foundation"),
        ],
    )
    assert summary["final_live_list"]["vm_category_allocations"] == 1
    assert summary["final_live_list"]["vm_category_bytes"] == 4096
    assert summary["final_live_list"]["vm_categories"] == {
        "VM: Foundation": {"allocations": 1, "bytes": 4096}
    }


def test_summary_rejects_workload_window_outside_exact_process_lifetime(
    tmp_path: Path,
) -> None:
    statistics_path, list_path = write_exports(
        tmp_path,
        [("00:00.001.000", 16, "one", "Malloc")],
    )
    with pytest.raises(allocations.AllocationExportError, match="exceeds"):
        allocations.summarize_allocation_exports(
            statistics_path=statistics_path,
            allocations_list_path=list_path,
            toc_metadata={
                "attached_pid": 321,
                "attached_process_name": "BenchmarkDriver",
            },
            target_pid=321,
            target_birth_unix_ns=10,
            observed_alive_through_unix_ns=100,
            measurement_started_unix_ns=20,
            measurement_ended_unix_ns=101,
        )
