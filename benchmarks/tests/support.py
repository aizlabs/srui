"""Shared builders for benchmark contract and runner tests."""

from __future__ import annotations

from typing import Any

from benchmarks import run as benchmark_run


def metric(name: str = "measurement") -> dict[str, Any]:
    return {
        "id": "measurement",
        "name": name,
        "value": 1.0,
        "unit": "ms",
        "statistic": "p50",
    }


def section(section_id: str) -> dict[str, Any]:
    return {
        "id": section_id,
        "name": f"Section {section_id}",
        "sample_counts": {"test.sample": 1},
        "metrics": [metric()],
        "assertions": [
            {"id": "correct", "name": "correct", "passed": True}
        ],
        "notes": [],
    }


def artifact(digest: str = "a" * 64) -> dict[str, Any]:
    return {
        "canonical_transaction_sha256": digest,
        "canonical_transaction_bytes": 42,
    }


def renderer_measurement_intervals(
    candidate: str,
    profile: str,
    *,
    started_unix_ns: int,
) -> list[dict[str, int]]:
    count = benchmark_run.expected_driver_sample_counts(
        "macos", profile
    )["31.1"][f"macos.{candidate}.render"]
    return [
        {
            "started_unix_ns": started_unix_ns + index * 20,
            "ended_unix_ns": started_unix_ns + index * 20 + 10,
        }
        for index in range(count)
    ]


def window_isolation_assertion() -> dict[str, Any]:
    return {
        "id": "window_isolation_fail_closed",
        "name": "WindowServer isolation rejects an exact synthetic occluder",
        "passed": True,
        "detail": (
            "window isolation self-test passed: dock=20 status=25 ahead=26 "
            "popup=101 target=123 occluder=124"
        ),
    }


def valid_manifest() -> dict[str, Any]:
    return {
        "schema_version": 1,
        "fixture": benchmark_run.CANONICAL_FIXTURE,
        "required_sections": list(benchmark_run.EXPECTED_SECTIONS),
        "drivers": [
            {
                "name": "rust",
                "sections": ["31.2", "31.5", "31.6"],
                "command": list(
                    benchmark_run.EXPECTED_DRIVER_COMMANDS["rust"]
                ),
            },
            {
                "name": "macos",
                "sections": [
                    "31.1",
                    "31.3",
                    "31.4",
                    "31.5",
                    "31.6",
                ],
                "platform": "darwin",
                "command": list(
                    benchmark_run.EXPECTED_DRIVER_COMMANDS["macos"]
                ),
            },
        ],
        "verification_commands": [
            dict(benchmark_run.EXPECTED_RECONNECT_VERIFICATION)
        ],
    }


def valid_metric_value(
    metric_id: str,
    statistic: str,
    frame_budget: float,
) -> float:
    if (
        metric_id,
        statistic,
    ) == (
        benchmark_run.LOCAL_FRAME_BUDGET_ID,
        "exact",
    ):
        return frame_budget
    if metric_id in {"idle.bytes", "idle.messages"}:
        return 0.0
    if not metric_id.startswith("cadence."):
        return 1.0
    update_count = float(metric_id.split(".")[1])
    suffix_values = {
        "inbound_bytes": 10.0,
        "outbound_bytes": 5.0,
        "bytes": 15.0,
        "inbound_messages": update_count,
        "outbound_messages": 3.0,
        "messages": update_count + 3.0,
    }
    return suffix_values.get(
        metric_id.rsplit(".", 1)[-1],
        1.0,
    )


def payload_for_driver(
    driver: dict[str, Any],
    *,
    profile: str = "full",
) -> dict[str, Any]:
    expected = benchmark_run.EXPECTED_DRIVER_INVENTORY[driver["name"]]
    frame_budget = 1000.0 / 120.0
    sections = []
    for section_id in driver["sections"]:
        inventory = expected[section_id]
        sections.append(
            {
                "id": section_id,
                "name": f"Section {section_id}",
                "sample_counts": (
                    benchmark_run.expected_driver_sample_counts(
                        driver["name"],
                        profile,
                    )[section_id]
                ),
                "metrics": [
                    {
                        "id": metric_id,
                        "name": (
                            benchmark_run.expected_metric_display_name(
                                metric_id,
                                profile,
                            )
                        ),
                        "value": valid_metric_value(
                            metric_id,
                            statistic,
                            frame_budget,
                        ),
                        "unit": metadata[0],
                        "statistic": statistic,
                        **(
                            {
                                "target": (
                                    frame_budget
                                    if metadata[1]
                                    == benchmark_run.LOCAL_FRAME_BUDGET_ID
                                    else metadata[1]
                                ),
                                "target_direction": metadata[2],
                            }
                            if metadata[1] is not None
                            else {}
                        ),
                    }
                    for (
                        metric_id,
                        statistic,
                    ), metadata in sorted(
                        inventory["metrics"].items()
                    )
                ],
                "assertions": [
                    {
                        "id": assertion_id,
                        "name": assertion_id,
                        "passed": True,
                    }
                    for assertion_id in sorted(
                        inventory["assertions"]
                    )
                ],
                "notes": [],
            }
        )
    artifacts = artifact()
    if driver["name"] == "macos":
        artifacts["renderer_process_attribution"] = [
            {
                "candidate": "srui",
                "driver_pid": 42,
                "host_pid": 43,
                "helper_pids": [],
                "started_unix_ns": 1_000,
                "ended_unix_ns": 2_000,
                "measurement_intervals": renderer_measurement_intervals(
                    "srui", profile, started_unix_ns=1_200
                ),
                "helper_pid_source": "no helper processes",
                "process_identities": [
                    {
                        "pid": 43,
                        "birth_unix_ns": 1_001,
                        "observed_alive_through_unix_ns": 1_600,
                    },
                ],
            },
            {
                "candidate": "webkit",
                "driver_pid": 42,
                "host_pid": 44,
                "helper_pids": [45],
                "started_unix_ns": 3_000,
                "ended_unix_ns": 4_000,
                "measurement_intervals": renderer_measurement_intervals(
                    "webkit", profile, started_unix_ns=3_200
                ),
                "helper_pid_source": (
                    "WKWebView diagnostic process identifiers"
                ),
                "process_identities": [
                    {
                        "pid": 44,
                        "birth_unix_ns": 1_002,
                        "observed_alive_through_unix_ns": 3_600,
                    },
                    {
                        "pid": 45,
                        "birth_unix_ns": 1_003,
                        "observed_alive_through_unix_ns": 3_600,
                    },
                ],
            },
        ]
    return {
        "contract_schema_version": benchmark_run.CONTRACT["schema_version"],
        "contract_sha256": benchmark_run.CONTRACT_SHA256,
        "artifacts": artifacts,
        "sections": sections,
    }


def valid_report() -> dict[str, Any]:
    sections: dict[str, dict[str, Any]] = {}
    artifacts: dict[str, dict[str, Any]] = {}
    for driver in valid_manifest()["drivers"]:
        payload = payload_for_driver(driver)
        artifacts[driver["name"]] = payload["artifacts"]
        for emitted in payload["sections"]:
            benchmark_run._merge_driver_section(sections, emitted)
    benchmark_run.append_parity_assertion(sections, artifacts)
    sections["31.1"]["assertions"].append(
        {
            "id": "candidate_failure_cleanup",
            "name": "candidate failure cleanup",
            "passed": True,
        }
    )
    sections["31.1"]["assertions"].append(
        window_isolation_assertion()
    )
    sections["31.5"]["metrics"].append(
        {
            "id": "production_reconnect_suite_ms",
            "name": "production reconnect boundary suite",
            "value": 1.0,
            "unit": "ms",
            "statistic": "wall",
        }
    )
    sections["31.5"]["assertions"].append(
        {
            "id": "production_reconnect_suite",
            "name": "production reconnect boundary suite",
            "passed": True,
        }
    )
    sections["31.5"]["sample_counts"][
        "runner.production_conformance"
    ] = 9
    return {
        "schema_version": 1,
        "contract_schema_version": benchmark_run.CONTRACT[
            "schema_version"
        ],
        "contract_sha256": benchmark_run.CONTRACT_SHA256,
        "generated_at": "2026-01-01T00:00:00Z",
        "profile": "full",
        "fixture": benchmark_run.CANONICAL_FIXTURE,
        "environment": {
            "platform": "macOS-26.4.1-arm64-arm-64bit",
            "machine": "arm64",
            "python": "3.14",
            "chip": "Apple M4 Max",
            "physical_ram_bytes": 137_438_953_472,
            "xcode": "Xcode 26.4\nBuild version 17E30",
            "swift": "Apple Swift version 6.2.3",
            "rust": "rustc 1.93.1",
            "git_commit": "a" * 40,
            "git_dirty": False,
        },
        "driver_artifacts": artifacts,
        "sections": list(sections.values()),
    }
