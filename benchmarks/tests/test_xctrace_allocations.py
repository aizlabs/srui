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
    <data><table schema="tick" /></data>
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
    anonymous_transient_count: int = 0,
    anonymous_transient_bytes: int = 0,
    anonymous_event_count: int | None = None,
    combined_persistent_bytes: int | None = None,
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
        "transient_allocations": anonymous_transient_count,
        "transient_bytes": anonymous_transient_bytes,
        "total_allocations": anonymous_persistent_count + anonymous_transient_count,
        "total_bytes": anonymous_persistent_bytes + anonymous_transient_bytes,
        "event_count": (
            anonymous_persistent_count + 2 * anonymous_transient_count
            if anonymous_event_count is None
            else anonymous_event_count
        ),
    }
    combined = {
        key: heap[key] + anonymous_vm[key]
        for key in heap
    }
    if combined_persistent_bytes is not None:
        difference = combined_persistent_bytes - combined["persistent_bytes"]
        combined["persistent_bytes"] = combined_persistent_bytes
        combined["total_bytes"] += difference

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


def list_xml(rows: list[tuple[str, int, str]]) -> str:
    rendered = "\n".join(
        f'<row timestamp="{timestamp}" size="{size}" live="true" '
        f'identifier="{identifier}" category="Malloc" />'
        for timestamp, size, identifier in rows
    )
    return f"<trace-query-result><node xpath=\"list\">{rendered}</node></trace-query-result>"


def write_exports(
    tmp_path: Path,
    rows: list[tuple[str, int, str]],
    *,
    statistics: str | None = None,
) -> tuple[Path, Path]:
    statistics_path = tmp_path / "statistics.xml"
    list_path = tmp_path / "list.xml"
    byte_total = sum(size for _timestamp, size, _identifier in rows)
    statistics_path.write_text(
        statistics
        or statistics_xml(
            persistent_count=len(rows),
            persistent_bytes=byte_total,
        ),
        encoding="utf-8",
    )
    list_path.write_text(list_xml(rows), encoding="utf-8")
    return statistics_path, list_path


def test_toc_requires_exact_attached_pid_and_view_details() -> None:
    metadata = allocations.validate_allocation_toc(toc_xml(), requested_pid=321)
    assert metadata["attached_pid"] == 321
    assert metadata["attached_process_name"] == "BenchmarkDriver"
    assert metadata["trace_started_unix_ns"] == 1_123_000_000
    assert metadata["trace_start_timestamp_resolution_ns"] == 1_000_000
    assert metadata["allocation_list_timestamp_resolution_ns"] == 1_000
    assert metadata["timestamp_boundary_uncertainty_ns"] == 1_001_000
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
    ).replace(
        '<data><table schema="tick" /></data>',
        '<data><table schema="allocations" /></data>',
    )
    with pytest.raises(allocations.AllocationExportError, match="Allocations List"):
        allocations.validate_allocation_toc(toc, requested_pid=321)


def test_statistics_parses_exact_heap_row_and_reconciles(tmp_path: Path) -> None:
    path = tmp_path / "statistics.xml"
    path.write_text(
        statistics_xml(persistent_count=11, persistent_bytes=4096),
        encoding="utf-8",
    )
    heap = {
        "persistent_allocations": 11,
        "persistent_bytes": 4096,
        "transient_allocations": 2,
        "transient_bytes": 96,
        "total_allocations": 13,
        "total_bytes": 4192,
        "event_count": 15,
    }
    assert allocations.parse_allocation_statistics(path) == {
        "heap_and_anonymous_vm": heap,
        "heap": heap,
        "anonymous_vm": {
            "persistent_allocations": 0,
            "persistent_bytes": 0,
            "transient_allocations": 0,
            "transient_bytes": 0,
            "total_allocations": 0,
            "total_bytes": 0,
            "event_count": 0,
        },
    }

@pytest.mark.parametrize(
    "override, message",
    [
        ({"total_count": 99}, "count Statistics"),
        ({"total_bytes": 99}, "byte Statistics"),
    ],
)
def test_statistics_rejects_broken_arithmetic(
    tmp_path: Path,
    override: dict[str, int],
    message: str,
) -> None:
    path = tmp_path / "statistics.xml"
    path.write_text(
        statistics_xml(
            persistent_count=2,
            persistent_bytes=64,
            **override,
        ),
        encoding="utf-8",
    )
    with pytest.raises(allocations.AllocationExportError, match=message):
        allocations.parse_allocation_statistics(path)


def test_statistics_keeps_independent_event_count(tmp_path: Path) -> None:
    path = tmp_path / "statistics.xml"
    path.write_text(
        statistics_xml(
            persistent_count=2,
            persistent_bytes=64,
            event_count=99,
        ),
        encoding="utf-8",
    )
    assert allocations.parse_allocation_statistics(path)["heap"]["event_count"] == 99


def test_statistics_rejects_combined_row_that_does_not_equal_parts(
    tmp_path: Path,
) -> None:
    path = tmp_path / "statistics.xml"
    path.write_text(
        statistics_xml(
            persistent_count=2,
            persistent_bytes=64,
            combined_persistent_bytes=65,
        ),
        encoding="utf-8",
    )
    with pytest.raises(allocations.AllocationExportError, match="do not equal"):
        allocations.parse_allocation_statistics(path)


def test_allocation_list_parses_real_flat_rows_and_rejects_non_live(
    tmp_path: Path,
) -> None:
    path = tmp_path / "list.xml"
    path.write_text(
        list_xml(
            [
                ("00:00.000.000", 48, "0"),
                ("01:02.003.004", 64, "1"),
            ]
        ),
        encoding="utf-8",
    )
    assert allocations.parse_allocation_list(path) == [
        {
            "identifier": "0",
            "category": "Malloc",
            "timestamp_relative_ns": 0,
            "size": 48,
        },
        {
            "identifier": "1",
            "category": "Malloc",
            "timestamp_relative_ns": 62_003_004_000,
            "size": 64,
        },
    ]
    path.write_text(
        list_xml([("00:00.001.000", 16, "1")]).replace(
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
            'live="true" identifier="1" category="Malloc" '
            '/></node></trace-query-result>',
            "unsupported format",
        ),
        (
            '<trace-query-result><node><row timestamp="00:00.001.000" '
            'size="1" live="true" identifier="1" category="Malloc" />'
            '<row timestamp="00:00.002.000" size="2" live="true" '
            'identifier="1" category="Malloc" /></node></trace-query-result>',
            "duplicate identifier",
        ),
        (
            '<trace-query-result><node><row timestamp="00:00.001.000" '
            'live="true" identifier="1" category="Malloc" '
            '/></node></trace-query-result>',
            "missing",
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


def test_summary_reports_nominal_and_boundary_bounds(tmp_path: Path) -> None:
    rows = [
        ("00:00.000.000", 100, "baseline"),
        ("00:00.003.000", 10, "outside-before"),
        ("00:00.004.500", 20, "possible-before"),
        ("00:00.005.500", 30, "nominal-start-boundary"),
        ("00:00.007.000", 40, "definite"),
        ("00:00.009.500", 50, "nominal-end-boundary"),
        ("00:00.010.500", 60, "possible-after"),
        ("00:00.012.000", 70, "outside-after"),
    ]
    statistics_path, list_path = write_exports(tmp_path, rows)
    trace_start = 10_000_000_000
    summary = allocations.summarize_allocation_exports(
        statistics_path=statistics_path,
        allocations_list_path=list_path,
        toc_metadata={
            "attached_pid": 321,
            "attached_process_name": "BenchmarkDriver",
            "trace_started_unix_ns": trace_start,
            "timestamp_boundary_uncertainty_ns": 1_001_000,
        },
        target_pid=321,
        target_birth_unix_ns=trace_start - 1,
        observed_alive_through_unix_ns=trace_start + 20_000_000,
        measurement_started_unix_ns=trace_start + 5_000_000,
        measurement_ended_unix_ns=trace_start + 10_000_000,
    )
    assert summary["allocation_list_reconciled"] is True
    assert summary["vm_category_rows"] == 0
    assert summary["vm_category_bytes"] == 0
    assert summary["attach_baseline_allocations"] == 1
    assert summary["retained_allocations"] == 3
    assert summary["retained_bytes"] == 120
    assert summary["retained_allocations_lower_bound"] == 1
    assert summary["retained_allocations_upper_bound"] == 5
    assert summary["retained_bytes_lower_bound"] == 40
    assert summary["retained_bytes_upper_bound"] == 200
    assert summary["boundary_ambiguous_allocations"] == 4
    assert summary["boundary_ambiguous_bytes"] == 160
    assert summary["excluded_outside_measurement_interval_rows"] == 5
    assert summary["process_total"]["pid"] == 321




def test_summary_keeps_vm_categories_in_combined_list_reconciliation(
    tmp_path: Path,
) -> None:
    statistics_path = tmp_path / "statistics.xml"
    list_path = tmp_path / "list.xml"
    statistics_path.write_text(
        statistics_xml(persistent_count=2, persistent_bytes=4_112),
        encoding="utf-8",
    )
    list_path.write_text(
        list_xml(
            [
                ("00:00.001.000", 16, "heap"),
                ("00:00.001.500", 4_096, "vm"),
            ]
        ).replace(
            'identifier="vm" category="Malloc"',
            'identifier="vm" category="VM: Foundation"',
        ),
        encoding="utf-8",
    )
    summary = allocations.summarize_allocation_exports(
        statistics_path=statistics_path,
        allocations_list_path=list_path,
        toc_metadata={
            "attached_pid": 321,
            "attached_process_name": "BenchmarkDriver",
            "trace_started_unix_ns": 10_000_000_000,
            "timestamp_boundary_uncertainty_ns": 1_001_000,
        },
        target_pid=321,
        target_birth_unix_ns=9_000_000_000,
        observed_alive_through_unix_ns=12_000_000_000,
        measurement_started_unix_ns=10_000_000_000,
        measurement_ended_unix_ns=11_000_000_000,
    )
    assert summary["allocation_rows"] == 2
    assert summary["allocation_list_bytes"] == 4_112
    assert summary["vm_category_rows"] == 1
    assert summary["vm_category_bytes"] == 4_096
    assert summary["retained_allocations"] == 2
    assert summary["retained_bytes"] == 4_112


def test_summary_rejects_statistics_list_mismatch(tmp_path: Path) -> None:
    rows = [("00:00.001.000", 16, "1")]
    statistics_path, list_path = write_exports(
        tmp_path,
        rows,
        statistics=statistics_xml(persistent_count=2, persistent_bytes=16),
    )
    with pytest.raises(allocations.AllocationExportError, match="does not reconcile"):
        allocations.summarize_allocation_exports(
            statistics_path=statistics_path,
            allocations_list_path=list_path,
            toc_metadata={
                "attached_pid": 321,
                "attached_process_name": "BenchmarkDriver",
                "trace_started_unix_ns": 10_000_000_000,
                "timestamp_boundary_uncertainty_ns": 1_001_000,
            },
            target_pid=321,
            target_birth_unix_ns=9_000_000_000,
            observed_alive_through_unix_ns=12_000_000_000,
            measurement_started_unix_ns=10_000_000_000,
            measurement_ended_unix_ns=11_000_000_000,
        )
