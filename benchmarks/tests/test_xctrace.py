from __future__ import annotations

import importlib.util
import json
import os
import signal
import sys
import threading
import time
from pathlib import Path
from types import SimpleNamespace
from typing import Any

import pytest

BENCHMARKS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(BENCHMARKS))

import process_control  # noqa: E402

MODULE_PATH = BENCHMARKS / "parse-render/run_xctrace.py"
SPEC = importlib.util.spec_from_file_location("benchmark_xctrace_tests", MODULE_PATH)
assert SPEC and SPEC.loader
benchmark_xctrace = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(benchmark_xctrace)

TRACE_STARTED_UNIX_NS = 1_700_000_000_000_000_000
TEST_INSTRUMENTATION = {
    "strategy": "private_copy_ad_hoc_codesigned_for_instrumentation",
    "source_binary_unchanged": True,
    "signature_verified": True,
    "entitlements": {"com.apple.security.get-task-allow": True},
}


def bypass_staging(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(
        benchmark_xctrace,
        "stage_instrumentable_binary",
        lambda binary, _workspace: (binary, TEST_INSTRUMENTATION),
    )


def assert_process_gone(pid: int) -> None:
    with pytest.raises(ProcessLookupError):
        os.kill(pid, 0)




def whole_trace_statistics(
    persistent_allocations: int,
    persistent_bytes: int,
    transient_allocations: int,
    transient_bytes: int,
) -> dict[str, dict[str, int]]:
    heap = {
        "persistent_allocations": persistent_allocations,
        "persistent_bytes": persistent_bytes,
        "transient_allocations": transient_allocations,
        "transient_bytes": transient_bytes,
        "total_allocations": persistent_allocations + transient_allocations,
        "total_bytes": persistent_bytes + transient_bytes,
        "event_count": persistent_allocations + 2 * transient_allocations,
    }
    zero = {field: 0 for field in heap}
    return {
        "heap_and_anonymous_vm": dict(heap),
        "heap": dict(heap),
        "anonymous_vm": zero,
    }


def targeted_pass(
    candidate: str,
    target_role: str,
    *,
    host_pid: int,
    target_pid: int,
    target_birth_unix_ns: int,
    sample_count: int = 2,
    available_roles: tuple[str, ...] | None = None,
    alias_groups: tuple[tuple[str, ...], ...] = (),
) -> dict[str, Any]:
    representation_bytes = 1_000 if candidate == "srui" else 2_000
    role_order = (
        ("host",) if candidate == "srui" else benchmark_xctrace.WEBKIT_TARGET_ROLES
    )
    roles = role_order if available_roles is None else available_roles
    host_birth_unix_ns = (
        target_birth_unix_ns
        if target_role == "host"
        else target_birth_unix_ns - 1
    )
    normalized_alias_groups: list[tuple[str, ...]] = []
    aliased_roles: set[str] = set()
    for raw_group in alias_groups:
        group_roles = set(raw_group)
        assert len(group_roles) > 1
        assert group_roles <= set(roles)
        assert "host" not in group_roles
        assert not aliased_roles.intersection(group_roles)
        normalized_alias_groups.append(
            tuple(role for role in role_order if role in group_roles)
        )
        aliased_roles.update(group_roles)

    role_groups: list[tuple[str, ...]] = []
    emitted_roles: set[str] = set()
    aliases_by_role = {
        role: group for group in normalized_alias_groups for role in group
    }
    for role in role_order:
        if role not in roles or role in emitted_roles:
            continue
        group = aliases_by_role.get(role, (role,))
        role_groups.append(group)
        emitted_roles.update(group)

    role_targets: dict[str, dict[str, Any]] = {}
    for group in role_groups:
        if group == ("host",):
            identity = (host_pid, host_birth_unix_ns)
        elif target_role in group:
            identity = (target_pid, target_birth_unix_ns)
        else:
            role_index = role_order.index(group[0])
            identity = (host_pid * 10 + role_index, host_birth_unix_ns + role_index)
        for role in group:
            role_targets[role] = {
                "role": role,
                "pid": identity[0],
                "birth_unix_ns": identity[1],
            }
    available_targets = [role_targets[role] for role in roles]
    target = role_targets.get(target_role)
    target_present = target is not None

    samples: list[dict[str, Any]] = []
    for sample_index in range(sample_count):
        started_unix_ns = (
            TRACE_STARTED_UNIX_NS
            + target_birth_unix_ns
            + 2_000_000
            + sample_index * 2_000_000
        )
        ended_unix_ns = started_unix_ns + 500_000
        sample: dict[str, Any] = {
            "candidate": candidate,
            "sample_index": sample_index,
            "target_role": target_role,
            "target_present": target_present,
            "host_pid": host_pid,
            "available_targets": available_targets,
            "started_unix_ns": started_unix_ns,
            "ended_unix_ns": ended_unix_ns,
        }
        if target_present:
            observed_alive_through_unix_ns = ended_unix_ns + 100_000
            retained_allocations = sample_index + 1
            retained_bytes = (sample_index + 1) * 256
            allocation_rows = retained_allocations + 10
            allocation_list_bytes = retained_bytes + 4_096
            transient_allocations = 5
            transient_bytes = 2_048
            lower_allocations = max(0, retained_allocations - 1)
            upper_allocations = retained_allocations + 1
            lower_bytes = max(0, retained_bytes - 64)
            upper_bytes = retained_bytes + 64
            process_fields = {
                "retained_allocations": retained_allocations,
                "retained_bytes": retained_bytes,
                "retained_allocations_lower_bound": lower_allocations,
                "retained_allocations_upper_bound": upper_allocations,
                "retained_bytes_lower_bound": lower_bytes,
                "retained_bytes_upper_bound": upper_bytes,
                "boundary_ambiguous_allocations": (
                    upper_allocations - lower_allocations
                ),
                "boundary_ambiguous_bytes": upper_bytes - lower_bytes,
            }
            sample.update(
                {
                    "target_pid": target["pid"],
                    "target_birth_unix_ns": target["birth_unix_ns"],
                    "observed_alive_through_unix_ns": (
                        observed_alive_through_unix_ns
                    ),
                    "trace_segment": (
                        f"{candidate}-{target_role}-{sample_index:03d}.trace"
                    ),
                    "recording_readiness_basis": "darwin_notification",
                    "trace_bytes": 512,
                    "allocation_export_basis": "Statistics plus Allocations List",
                    "allocation_rows": allocation_rows,
                    "allocation_list_bytes": allocation_list_bytes,
                    "vm_category_rows": 0,
                    "vm_category_bytes": 0,
                    "allocation_list_reconciled": True,
                    "allocation_timestamp_basis": "list time plus TOC start",
                    "metric_semantics": "interval-created and live at capture end",
                    "whole_trace_statistics_semantics": "diagnostic attach-inclusive",
                    "process_identity_basis": "exact targeted identity",
                    "whole_trace_statistics": whole_trace_statistics(
                        allocation_rows,
                        allocation_list_bytes,
                        transient_allocations,
                        transient_bytes,
                    ),
                    "attach_baseline_allocations": 10,
                    "attach_baseline_bytes": 4_096,
                    "excluded_outside_measurement_interval_rows": (
                        allocation_rows - retained_allocations
                    ),
                    "trace_started_unix_ns": started_unix_ns - 1_000_000,
                    "trace_start_timestamp_resolution_ns": 1_000_000,
                    "allocation_list_timestamp_resolution_ns": 1_000,
                    "timestamp_boundary_uncertainty_ns": 1_001_000,
                    "xctrace": {
                        "instruments_version": "26.0 (17C52)",
                        "platform": "macOS",
                        "os_version": "26.4.1",
                        "attached_pid": target["pid"],
                        "attached_process_name": f"{candidate}-{target_role}",
                        "statistics_xpath": benchmark_xctrace.STATISTICS_XPATH,
                        "allocations_list_xpath": (
                            benchmark_xctrace.ALLOCATIONS_LIST_XPATH
                        ),
                    },
                    **process_fields,
                    "process_total": {
                        "pid": target["pid"],
                        "birth_unix_ns": target["birth_unix_ns"],
                        "observed_alive_through_unix_ns": (
                            observed_alive_through_unix_ns
                        ),
                        "names": [f"{candidate}-{target_role}"],
                        **process_fields,
                    },
                }
            )
        samples.append(sample)

    final_observation = samples[-1]["ended_unix_ns"] + 100_000
    unique_targets = {
        (available["pid"], available["birth_unix_ns"]): available
        for available in available_targets
    }
    process_identities = [
        {
            "pid": available["pid"],
            "birth_unix_ns": available["birth_unix_ns"],
            "observed_alive_through_unix_ns": final_observation,
        }
        for available in unique_targets.values()
    ]
    return {
        "candidate": candidate,
        "target_role": target_role,
        "host_pid": host_pid,
        "samples": samples,
        "candidate_result": {
            "candidate": candidate,
            "representationBytes": representation_bytes,
            "renderedNodeCount": 50,
            "semanticParityPassed": True,
            "elementKindsPassed": True,
            "captureAuthorization": False,
            "pixelCaptureCompletions": 0,
            "paintCompletionMode": (
                "non-compositor synthetic allocation workload"
            ),
            "succeeded": True,
            "attribution": {
                "candidate": candidate,
                "driver_pid": 999,
                "host_pid": host_pid,
                "helper_pids": sorted(
                    {
                        available["pid"]
                        for available in available_targets
                        if available["role"] != "host"
                    }
                ),
                "started_unix_ns": samples[0]["started_unix_ns"],
                "ended_unix_ns": samples[-1]["ended_unix_ns"],
                "measurement_intervals": [
                    {
                        "started_unix_ns": sample["started_unix_ns"],
                        "ended_unix_ns": sample["ended_unix_ns"],
                        "required_allocation_pids": (
                            [sample["target_pid"]]
                            if sample["target_present"]
                            else []
                        ),
                    }
                    for sample in samples
                ],
                "helper_pid_source": f"synthetic {target_role}",
                "process_identities": process_identities,
            },
        },
    }
@pytest.mark.parametrize(
    ("field", "value"),
    [
        ("captureAuthorization", True),
        ("pixelCaptureCompletions", 1),
        ("paintCompletionMode", "on-screen compositor claim"),
    ],
)
def test_allocation_candidate_cannot_claim_compositor_evidence(
    field: str,
    value: object,
) -> None:
    role_pass = targeted_pass(
        "srui",
        "host",
        host_pid=101,
        target_pid=101,
        target_birth_unix_ns=10,
    )
    role_pass["candidate_result"][field] = value

    with pytest.raises(
        benchmark_xctrace.CaptureError,
        match="equivalent successful renderer workload",
    ):
        benchmark_xctrace._candidate_equivalence("srui", [role_pass])


def test_trace_command_is_bounded_and_targets_exact_process() -> None:
    command = benchmark_xctrace.trace_command(
        Path("out.trace"),
        "notification",
        target_pid=321,
    )
    assert command[command.index("--attach") + 1] == "321"
    assert "--all-processes" not in command
    assert "--launch" not in command
    assert command[command.index("--time-limit") + 1] == "60s"
    assert command[command.index("--window") + 1] == "60s"
    assert "--no-prompt" in command
    assert "--notify-tracing-started" in command
    with pytest.raises(benchmark_xctrace.CaptureError, match="invalid"):
        benchmark_xctrace.trace_command(
            Path("out.trace"),
            "notification",
            target_pid=0,
        )


def test_recording_readiness_requires_darwin_notification(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    capture_root = tmp_path / "capture.trace"
    stdout_path = tmp_path / "xctrace.stdout"
    stderr_path = tmp_path / "xctrace.stderr"
    stdout_path.write_text("", encoding="utf-8")
    stderr_path.write_text("", encoding="utf-8")

    class SuccessfulWatcher:
        def wait(
            self,
            timeout: float,
            *,
            poll_hook: Any,
        ) -> SimpleNamespace:
            assert timeout == benchmark_xctrace.RECORDING_READINESS_TIMEOUT_SECONDS
            poll_hook(self)
            return SimpleNamespace(returncode=0, stdout="", stderr="")

    recorder = SimpleNamespace(
        stdout_path=stdout_path,
        stderr_path=stderr_path,
    )
    monkeypatch.setattr(
        benchmark_xctrace,
        "_managed_process_result_if_exited",
        lambda _process: None,
    )

    assert (
        benchmark_xctrace.wait_for_recording_readiness(
            SuccessfulWatcher(),
            recorder,
            capture_root,
            max_trace_bytes=1024 * 1024,
            min_remaining_bytes=1,
        )
        == "darwin_notification"
    )


def test_recording_readiness_rejects_preliminary_trace_without_notification(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    capture_root = tmp_path / "capture.trace"
    event_store = (
        capture_root
        / "srui-host-000.trace"
        / "Trace1.run"
        / "event_data_321.oa"
    )
    event_store.parent.mkdir(parents=True)
    event_store.write_bytes(b"\0" * 4096)
    stdout_path = tmp_path / "xctrace.stdout"
    stderr_path = tmp_path / "xctrace.stderr"
    stdout_path.write_text(
        "Starting recording with the Allocations template.\r"
        "Attaching to: BenchmarkDriver (321).\r"
        "Time limit: 60.0 s\r",
        encoding="utf-8",
    )
    stderr_path.write_text("", encoding="utf-8")

    class TimedOutWatcher:
        def wait(self, _timeout: float, **_kwargs: Any) -> None:
            raise process_control.ManagedCommandTimeout("synthetic timeout")

    recorder = SimpleNamespace(
        stdout_path=stdout_path,
        stderr_path=stderr_path,
    )
    monkeypatch.setattr(
        benchmark_xctrace,
        "_managed_process_result_if_exited",
        lambda _process: None,
    )

    with pytest.raises(
        benchmark_xctrace.CaptureError,
        match="did not begin recording",
    ):
        benchmark_xctrace.wait_for_recording_readiness(
            TimedOutWatcher(),
            recorder,
            capture_root,
            max_trace_bytes=1024 * 1024,
            min_remaining_bytes=1,
        )


def test_control_timeout_reports_every_process_status_and_output_tail(
    tmp_path: Path,
) -> None:
    class RunningSupervisor:
        def __init__(self, pid: int) -> None:
            self.pid = pid

        def poll(self) -> None:
            return None

    def fake_process(
        label: str,
        *,
        child_pid: int,
        supervisor_pid: int,
        stdout: str,
        stderr: str,
    ) -> SimpleNamespace:
        directory = tmp_path / label.replace("/", "-").replace(" ", "-")
        directory.mkdir()
        ready_path = directory / "ready.json"
        status_path = directory / "status.json"
        stdout_path = directory / "stdout"
        stderr_path = directory / "stderr"
        ready_path.write_text(
            json.dumps({"child_pid": child_pid}),
            encoding="utf-8",
        )
        stdout_path.write_text(stdout, encoding="utf-8")
        stderr_path.write_text(stderr, encoding="utf-8")
        return SimpleNamespace(
            label=label,
            child_pid=child_pid,
            supervisor=RunningSupervisor(supervisor_pid),
            ready_path=ready_path,
            status_path=status_path,
            stdout_path=stdout_path,
            stderr_path=stderr_path,
        )

    candidate = fake_process(
        "webkit-host allocation candidate",
        child_pid=321,
        supervisor_pid=320,
        stdout="candidate stdout phase",
        stderr="candidate stderr phase",
    )
    recorder = fake_process(
        "webkit/host sample 0 allocation recorder",
        child_pid=421,
        supervisor_pid=420,
        stdout="recorder stdout phase",
        stderr="recorder stderr phase",
    )
    control_file = tmp_path / "done-0-host.json"

    with pytest.raises(benchmark_xctrace.CaptureError) as captured:
        benchmark_xctrace.wait_for_control_payload(
            control_file,
            processes=[candidate, recorder],
            timeout=0,
            label="webkit/host sample 0 completion",
            poll_hook=lambda: None,
        )

    detail = str(captured.value)
    assert "timed out after 0s waiting for webkit/host sample 0 completion" in detail
    assert f"control_file={control_file} (absent)" in detail
    assert (
        "webkit-host allocation candidate: child_pid=321, "
        "supervisor_pid=320, supervisor_state=running, "
        "ready_file=present, status_file=absent"
    ) in detail
    assert repr("candidate stderr phase\ncandidate stdout phase")[1:-1] in detail
    assert (
        "webkit/host sample 0 allocation recorder: child_pid=421, "
        "supervisor_pid=420, supervisor_state=running, "
        "ready_file=present, status_file=absent"
    ) in detail
    assert repr("recorder stderr phase\nrecorder stdout phase")[1:-1] in detail


def test_candidate_command_uses_supervised_parent_and_targeted_control() -> None:
    command = benchmark_xctrace.candidate_command(
        Path("BenchmarkDriver"),
        Path("fixture.json"),
        Path("result.json"),
        Path("control"),
        profile="full",
        candidate="srui",
        target_role="host",
    )
    assert command[command.index("--profile") + 1] == "full"
    assert command[command.index("--candidate") + 1] == "srui"
    assert command[command.index("--allocation-target-role") + 1] == "host"
    assert command[command.index("--allocation-control-dir") + 1] == "control"
    assert "--supervised-parent" in command
    assert "--driver-pid" not in command
    assert "--driver-birth-unix-ns" not in command

    webcontent = benchmark_xctrace.candidate_command(
        Path("BenchmarkDriver"),
        Path("fixture.json"),
        Path("result.json"),
        Path("control"),
        profile="smoke",
        candidate="webkit",
        target_role="webcontent",
    )
    assert webcontent[webcontent.index("--profile") + 1] == "smoke"
    with pytest.raises(benchmark_xctrace.CaptureError, match="only the host"):
        benchmark_xctrace.candidate_command(
            Path("BenchmarkDriver"),
            Path("fixture.json"),
            Path("result.json"),
            Path("control"),
            profile="full",
            candidate="srui",
            target_role="webcontent",
        )


def test_staged_signing_uses_private_copy_and_exact_entitlement(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    source = tmp_path / "BenchmarkDriver"
    source.write_bytes(b"original-build-output")
    source.chmod(0o755)
    commands: list[list[str]] = []
    entitlement_xml = (
        '<?xml version="1.0"?><plist version="1.0"><dict>'
        '<key>com.apple.security.get-task-allow</key><true/></dict></plist>'
    )

    def fake_run(command: list[str], **_kwargs: Any) -> SimpleNamespace:
        commands.append(command)
        stdout = entitlement_xml if "--display" in command else ""
        return SimpleNamespace(returncode=0, stdout=stdout, stderr="")

    monkeypatch.setattr(benchmark_xctrace, "run_managed_command", fake_run)
    workspace = benchmark_xctrace.create_capture_workspace(tmp_path)
    staged, evidence = benchmark_xctrace.stage_instrumentable_binary(
        source, workspace
    )

    assert staged != source
    assert staged.parent.is_relative_to(workspace.path)
    assert staged.read_bytes() == b"original-build-output"
    assert source.read_bytes() == b"original-build-output"
    assert commands[0][:4] == ["codesign", "--force", "--sign", "-"]
    assert "--entitlements" in commands[0]
    assert commands[1][1:3] == ["--verify", "--strict"]
    assert "--display" in commands[2]
    assert evidence == TEST_INSTRUMENTATION
    benchmark_xctrace.remove_owned_capture(workspace)


def test_main_accepts_optional_profile_and_defaults_to_full(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    binary = tmp_path / "BenchmarkDriver"
    fixture = tmp_path / "fixture.json"
    binary.write_bytes(b"binary")
    fixture.write_text("{}", encoding="utf-8")
    observed_profiles: list[str] = []

    def observe_run(
        _trace: Path,
        _binary: Path,
        _fixture: Path,
        _result: Path,
        **arguments: Any,
    ) -> int:
        observed_profiles.append(arguments["profile"])
        return 0

    monkeypatch.setattr(benchmark_xctrace, "run", observe_run)
    assert (
        benchmark_xctrace.main(
            [
                str(tmp_path / "default.trace"),
                str(binary),
                str(fixture),
                str(tmp_path / "default-result.json"),
            ]
        )
        == 0
    )
    assert (
        benchmark_xctrace.main(
            [
                str(tmp_path / "smoke.trace"),
                str(binary),
                str(fixture),
                str(tmp_path / "smoke-result.json"),
                "smoke",
            ]
        )
        == 0
    )
    assert observed_profiles == ["full", "smoke"]
    assert (
        benchmark_xctrace.main(
            [
                str(tmp_path / "invalid.trace"),
                str(binary),
                str(fixture),
                str(tmp_path / "invalid-result.json"),
                "fast",
            ]
        )
        == 2
    )


def test_preflight_reserves_trace_export_and_free_space(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    monkeypatch.setattr(
        benchmark_xctrace.shutil,
        "disk_usage",
        lambda _path: SimpleNamespace(free=29),
    )
    with pytest.raises(benchmark_xctrace.CaptureError, match="requires 30 free bytes"):
        benchmark_xctrace.preflight_capture(
            tmp_path / "capture.trace",
            max_trace_bytes=10,
            max_export_bytes=5,
            min_remaining_bytes=15,
        )


def test_live_budget_rejects_oversize_trace(tmp_path: Path) -> None:
    trace = tmp_path / "capture.trace"
    trace.mkdir()
    (trace / "data").write_bytes(b"x" * 17)

    with pytest.raises(benchmark_xctrace.CaptureError, match="exceeded 16 bytes"):
        benchmark_xctrace.ensure_capture_budget(
            trace,
            max_trace_bytes=16,
            min_remaining_bytes=1,
        )


def test_candidate_spawn_failure_removes_only_owned_staging(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    trace = tmp_path / "capture.trace"
    bypass_staging(monkeypatch)
    starts = 0

    def fail_start(
        _cls: type[Any],
        _command: list[str],
        **_kwargs: Any,
    ) -> process_control.ManagedProcess:
        nonlocal starts
        starts += 1
        raise OSError("candidate spawn failed")

    monkeypatch.setattr(
        benchmark_xctrace.ManagedProcess,
        "start",
        classmethod(fail_start),
    )

    with pytest.raises(OSError, match="candidate spawn failed"):
        benchmark_xctrace.run(
            trace,
            tmp_path / "BenchmarkDriver",
            tmp_path / "fixture.json",
            tmp_path / "result.json",
            max_trace_bytes=1024 * 1024,
            max_export_bytes=1024,
            min_remaining_bytes=1,
        )

    assert starts == 1
    assert not trace.exists()
    assert not (tmp_path / "capture.trace.summary.json").exists()
    assert list(tmp_path.glob(".srui-xctrace-*")) == []


@pytest.mark.parametrize("occupied_kind", ["trace", "summary", "result"])
def test_direct_run_preserves_every_preexisting_output_before_spawn(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
    occupied_kind: str,
) -> None:
    trace = tmp_path / "capture.trace"
    sidecar = tmp_path / "capture.trace.summary.json"
    result = tmp_path / "result.json"
    occupied = {"trace": trace, "summary": sidecar, "result": result}[occupied_kind]
    if occupied_kind == "trace":
        occupied.mkdir()
        sentinel = occupied / "keep.txt"
    else:
        sentinel = occupied
    sentinel.write_text(f"preserve-{occupied_kind}", encoding="utf-8")
    starts = 0

    def must_not_start(
        _cls: type[Any],
        _command: list[str],
        **_kwargs: Any,
    ) -> process_control.ManagedProcess:
        nonlocal starts
        starts += 1
        raise AssertionError("capture process started before output ownership check")

    monkeypatch.setattr(
        benchmark_xctrace.ManagedProcess,
        "start",
        classmethod(must_not_start),
    )
    with pytest.raises(benchmark_xctrace.CaptureError, match="already exists"):
        benchmark_xctrace.run(
            trace,
            tmp_path / "BenchmarkDriver",
            tmp_path / "fixture.json",
            result,
            max_trace_bytes=1024 * 1024,
            max_export_bytes=1024,
            min_remaining_bytes=1,
        )

    assert starts == 0
    assert sentinel.read_text(encoding="utf-8") == f"preserve-{occupied_kind}"
    assert list(tmp_path.glob(".srui-xctrace-*")) == []


def test_owned_workspace_refuses_cleanup_after_token_mismatch(tmp_path: Path) -> None:
    workspace = benchmark_xctrace.create_capture_workspace(tmp_path)
    payload = workspace.path / "payload"
    payload.write_text("preserve", encoding="utf-8")
    (workspace.path / benchmark_xctrace.OWNERSHIP_SENTINEL).write_text(
        "different owner",
        encoding="ascii",
    )

    with pytest.raises(benchmark_xctrace.CaptureError, match="mismatched owner"):
        benchmark_xctrace.remove_owned_capture(workspace)

    assert payload.read_text(encoding="utf-8") == "preserve"


def test_publish_capture_outputs_preserves_result_and_trace_path(
    tmp_path: Path,
) -> None:
    workspace = benchmark_xctrace.create_capture_workspace(tmp_path)
    staged_trace = workspace.path / "capture.trace"
    staged_trace.mkdir()
    (staged_trace / "data").write_bytes(b"trace")
    staged_sidecar = workspace.path / "capture.trace.summary.json"
    staged_sidecar.write_text(
        json.dumps({"trace": str(tmp_path / "final.trace")}),
        encoding="utf-8",
    )
    staged_result = workspace.path / "renderer-result.json"
    staged_result.write_text("result", encoding="utf-8")
    trace = tmp_path / "final.trace"
    sidecar = tmp_path / "final.trace.summary.json"
    result = tmp_path / "final-result.json"

    benchmark_xctrace.publish_capture_outputs(
        staged_trace=staged_trace,
        staged_sidecar=staged_sidecar,
        staged_result=staged_result,
        trace=trace,
        sidecar=sidecar,
        result=result,
    )
    benchmark_xctrace.remove_owned_capture(workspace)

    assert (trace / "data").read_bytes() == b"trace"
    assert json.loads(sidecar.read_text(encoding="utf-8"))["trace"] == str(trace)
    assert result.read_text(encoding="utf-8") == "result"
    assert not workspace.path.exists()


def test_publish_failure_after_trace_and_sidecar_rolls_back_owned_outputs(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    workspace = benchmark_xctrace.create_capture_workspace(tmp_path)
    staged_trace = workspace.path / "capture.trace"
    staged_trace.mkdir()
    (staged_trace / "data").write_bytes(b"trace")
    staged_sidecar = workspace.path / "capture.trace.summary.json"
    staged_sidecar.write_text("summary", encoding="utf-8")
    staged_result = workspace.path / "renderer-result.json"
    staged_result.write_text("result", encoding="utf-8")
    trace = tmp_path / "final.trace"
    sidecar = tmp_path / "final.trace.summary.json"
    result = tmp_path / "final-result.json"
    original_publish = benchmark_xctrace._publish_file_exclusive

    def fail_result_publication(source: Path, destination: Path) -> None:
        if destination == result:
            raise OSError("synthetic result publication failure")
        original_publish(source, destination)

    monkeypatch.setattr(
        benchmark_xctrace,
        "_publish_file_exclusive",
        fail_result_publication,
    )
    with pytest.raises(OSError, match="synthetic result publication failure"):
        benchmark_xctrace.publish_capture_outputs(
            staged_trace=staged_trace,
            staged_sidecar=staged_sidecar,
            staged_result=staged_result,
            trace=trace,
            sidecar=sidecar,
            result=result,
        )

    assert not trace.exists()
    assert not sidecar.exists()
    assert not result.exists()
    assert (staged_trace / "data").read_bytes() == b"trace"
    assert staged_sidecar.read_text(encoding="utf-8") == "summary"
    assert staged_result.read_text(encoding="utf-8") == "result"
    benchmark_xctrace.remove_owned_capture(workspace)


def test_publication_rollback_preserves_raced_sidecar_replacement(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    workspace = benchmark_xctrace.create_capture_workspace(tmp_path)
    staged_trace = workspace.path / "capture.trace"
    staged_trace.mkdir()
    (staged_trace / "data").write_bytes(b"trace")
    staged_sidecar = workspace.path / "capture.trace.summary.json"
    staged_sidecar.write_text("summary", encoding="utf-8")
    staged_result = workspace.path / "renderer-result.json"
    staged_result.write_text("result", encoding="utf-8")
    trace = tmp_path / "final.trace"
    sidecar = tmp_path / "final.trace.summary.json"
    result = tmp_path / "final-result.json"
    original_publish = benchmark_xctrace._publish_file_exclusive

    def race_then_fail(source: Path, destination: Path) -> None:
        if destination == result:
            sidecar.unlink()
            sidecar.write_text("foreign replacement", encoding="utf-8")
            raise OSError("synthetic result publication failure")
        original_publish(source, destination)

    monkeypatch.setattr(
        benchmark_xctrace,
        "_publish_file_exclusive",
        race_then_fail,
    )
    with pytest.raises(
        BaseExceptionGroup,
        match="capture publication and rollback failed",
    ):
        benchmark_xctrace.publish_capture_outputs(
            staged_trace=staged_trace,
            staged_sidecar=staged_sidecar,
            staged_result=staged_result,
            trace=trace,
            sidecar=sidecar,
            result=result,
        )

    assert not trace.exists()
    assert sidecar.read_text(encoding="utf-8") == "foreign replacement"
    assert not result.exists()
    assert (staged_trace / "data").read_bytes() == b"trace"
    benchmark_xctrace.remove_owned_capture(workspace)


def diagnostic_srui_pass(*, sample_count: int = 2) -> dict[str, Any]:
    role_pass = targeted_pass(
        "srui",
        "host",
        host_pid=101,
        target_pid=101,
        target_birth_unix_ns=10,
        sample_count=sample_count,
    )
    samples = []
    for sample_index, old_sample in enumerate(role_pass["samples"]):
        started = old_sample["started_unix_ns"]
        ended = old_sample["ended_unix_ns"]
        observed = old_sample["observed_alive_through_unix_ns"]
        live_allocations = 10 + sample_index
        live_bytes = 4096 + sample_index * 16
        statistics = whole_trace_statistics(
            live_allocations + 3,
            live_bytes + 32,
            5,
            2048,
        )
        samples.append(
            {
                "candidate": "srui",
                "sample_index": sample_index,
                "target_role": "host",
                "target_present": True,
                "host_pid": 101,
                "available_targets": [
                    {"role": "host", "pid": 101, "birth_unix_ns": 10}
                ],
                "target_pid": 101,
                "target_birth_unix_ns": 10,
                "started_unix_ns": started,
                "ended_unix_ns": ended,
                "observed_alive_through_unix_ns": observed,
                "trace_segment": f"srui-host-{sample_index:03d}.trace",
                "recording_readiness_basis": "darwin_notification",
                "trace_bytes": 512,
                "allocation_export_basis": "independently materialized views",
                "metric_semantics": "whole-trace and final-live diagnostic",
                "whole_trace_statistics_semantics": "whole-trace attach-inclusive",
                "final_live_list_semantics": "final live rows",
                "process_identity_basis": "exact targeted identity",
                "whole_trace_statistics": statistics,
                "final_live_list": {
                    "allocations": live_allocations,
                    "bytes": live_bytes,
                    "vm_category_allocations": 0,
                    "vm_category_bytes": 0,
                    "vm_categories": {},
                    "maximum_elapsed_timestamp_ns": 1_000_000,
                },
                "statistics_minus_final_live_list": {
                    "persistent_allocations": 3,
                    "persistent_bytes": 32,
                },
                "workload_window": {
                    "started_unix_ns": started,
                    "ended_unix_ns": ended,
                    "used_for_allocation_attribution": False,
                },
                "xctrace": {
                    "instruments_version": "26.0 (17C52)",
                    "platform": "macOS",
                    "os_version": "26.4.1",
                    "attached_pid": 101,
                    "attached_process_name": "BenchmarkDriver",
                    "statistics_xpath": benchmark_xctrace.STATISTICS_XPATH,
                    "allocations_list_xpath": benchmark_xctrace.ALLOCATIONS_LIST_XPATH,
                },
                "exact_process_identity": {
                    "pid": 101,
                    "birth_unix_ns": 10,
                    "observed_alive_through_unix_ns": observed,
                    "names": ["BenchmarkDriver"],
                },
            }
        )
    role_pass["samples"] = samples
    return role_pass


def test_exact_process_summary_is_srui_host_only_diagnostic(tmp_path: Path) -> None:
    assert benchmark_xctrace.CAPTURE_PASSES == (("srui", "host"),)
    trace_directory = tmp_path / "capture.trace"
    trace_directory.mkdir()
    (trace_directory / "segment-data").write_bytes(b"trace")

    summary = benchmark_xctrace.build_exact_process_summary(
        [diagnostic_srui_pass()],
        trace_directory=trace_directory,
        reported_trace=tmp_path / "published.trace",
        instrumentation=TEST_INSTRUMENTATION,
    )

    assert summary["schema_version"] == 4
    assert summary["diagnostic_only"] is True
    assert summary["authoritative_benchmark_metric"] is False
    assert summary["diagnostic_target"] == "srui_host"
    assert summary["target_capture_count"] == 2
    assert len(summary["candidate_processes"]) == 1
    assert summary["candidate_processes"][0]["candidate"] == "srui"
    assert summary["target_captures"][0][
        "statistics_minus_final_live_list"
    ] == {"persistent_allocations": 3, "persistent_bytes": 32}
    assert summary["target_captures"][0]["workload_window"][
        "used_for_allocation_attribution"
    ] is False


def test_exact_process_summary_rejects_webkit_capture(tmp_path: Path) -> None:
    role_pass = diagnostic_srui_pass(sample_count=1)
    role_pass["candidate"] = "webkit"
    with pytest.raises(
        benchmark_xctrace.CaptureError,
        match="only the SRUI host pass",
    ):
        benchmark_xctrace.build_exact_process_summary(
            [role_pass],
            trace_directory=tmp_path,
            reported_trace=tmp_path / "published.trace",
            instrumentation=TEST_INSTRUMENTATION,
        )


def test_exact_process_summary_validates_signed_discrepancy(tmp_path: Path) -> None:
    role_pass = diagnostic_srui_pass(sample_count=1)
    role_pass["samples"][0]["statistics_minus_final_live_list"][
        "persistent_bytes"
    ] = 0
    with pytest.raises(
        benchmark_xctrace.CaptureError,
        match="signed discrepancy",
    ):
        benchmark_xctrace.build_exact_process_summary(
            [role_pass],
            trace_directory=tmp_path,
            reported_trace=tmp_path / "published.trace",
            instrumentation=TEST_INSTRUMENTATION,
        )


def test_exact_process_summary_rejects_interval_attribution_claim(tmp_path: Path) -> None:
    role_pass = diagnostic_srui_pass(sample_count=1)
    role_pass["samples"][0]["workload_window"][
        "used_for_allocation_attribution"
    ] = True
    with pytest.raises(
        benchmark_xctrace.CaptureError,
        match="misstates allocation attribution",
    ):
        benchmark_xctrace.build_exact_process_summary(
            [role_pass],
            trace_directory=tmp_path,
            reported_trace=tmp_path / "published.trace",
            instrumentation=TEST_INSTRUMENTATION,
        )




def test_control_handshake_accepts_helper_roles_aliased_to_one_exact_process(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    births = {301: 30, 401: 40}
    monkeypatch.setattr(
        benchmark_xctrace,
        "process_birth_unix_ns",
        lambda pid: births.get(pid),
    )
    request = {
        "schema_version": 2,
        "candidate": "webkit",
        "sample_index": 0,
        "host_pid": 301,
        "target_role": "network",
        "target_present": True,
        "target_pid": 401,
        "target_birth_unix_ns": 40,
        "available_targets": [
            {"role": "host", "pid": 301, "birth_unix_ns": 30},
            {"role": "webcontent", "pid": 401, "birth_unix_ns": 40},
            {"role": "network", "pid": 401, "birth_unix_ns": 40},
        ],
    }

    assert (
        benchmark_xctrace.validate_control_request(
            request,
            expected_candidate="webkit",
            expected_sample_index=0,
            expected_target_role="network",
            expected_host_pid=301,
        )
        == request
    )


def test_absent_optional_control_handshake_is_fail_closed(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    births = {301: 30, 401: 40}
    monkeypatch.setattr(
        benchmark_xctrace,
        "process_birth_unix_ns",
        lambda pid: births.get(pid),
    )
    request = {
        "schema_version": 2,
        "candidate": "webkit",
        "sample_index": 0,
        "host_pid": 301,
        "target_role": "network",
        "target_present": False,
        "available_targets": [
            {"role": "host", "pid": 301, "birth_unix_ns": 30},
            {"role": "webcontent", "pid": 401, "birth_unix_ns": 40},
        ],
    }

    assert (
        benchmark_xctrace.validate_control_request(
            request,
            expected_candidate="webkit",
            expected_sample_index=0,
            expected_target_role="network",
            expected_host_pid=301,
        )
        == request
    )
    done = {
        "schema_version": 2,
        "target_present": False,
        "started_unix_ns": 100,
        "ended_unix_ns": 200,
    }
    assert benchmark_xctrace.validate_control_done(done, request=request) == done

    invalid = {**request, "target_pid": 501}
    with pytest.raises(benchmark_xctrace.CaptureError, match="field contract"):
        benchmark_xctrace.validate_control_request(
            invalid,
            expected_candidate="webkit",
            expected_sample_index=0,
            expected_target_role="network",
            expected_host_pid=301,
        )


@pytest.mark.skipif(
    sys.platform != "darwin",
    reason="exact managed-process birth identity is available only on macOS",
)
def test_managed_watcher_assignment_window_reaps_real_child_on_signal(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    trace = tmp_path / "capture.trace"
    bypass_staging(monkeypatch)
    watchers: list[process_control.ManagedProcess] = []
    child_pids: list[int] = []
    original_start = benchmark_xctrace.ManagedProcess.start

    def launch_then_signal(
        _cls: type[Any],
        _command: list[str],
        **kwargs: Any,
    ) -> process_control.ManagedProcess:
        watcher = original_start(
            [sys.executable, "-c", "import time; time.sleep(60)"],
            **kwargs,
        )
        child_pid = watcher.wait_until_started(2)
        watchers.append(watcher)
        child_pids.append(child_pid)
        signal.pthread_kill(threading.get_ident(), signal.SIGTERM)
        return watcher

    monkeypatch.setattr(
        benchmark_xctrace.ManagedProcess,
        "start",
        classmethod(launch_then_signal),
    )

    with pytest.raises(benchmark_xctrace.TerminationRequested):
        with benchmark_xctrace.termination_handlers():
            benchmark_xctrace.run(
                trace,
                tmp_path / "BenchmarkDriver",
                tmp_path / "fixture.json",
                tmp_path / "result.json",
                max_trace_bytes=1024 * 1024,
                max_export_bytes=1024,
                min_remaining_bytes=1,
            )

    assert len(watchers) == 1
    assert watchers[0].closed
    assert_process_gone(child_pids[0])
    assert not trace.exists()


def test_capture_cleanup_retries_every_managed_resource_and_retains_handles(
    tmp_path: Path,
) -> None:
    attempted: list[str] = []

    class FailingManaged:
        closed = False

        def __init__(self, name: str) -> None:
            self.name = name
            self.label = name
            self.supervisor = None
            self.ready_path = tmp_path / name / "ready.json"

        def terminate(self) -> None:
            attempted.append(self.name)
            raise RuntimeError(f"{self.name} failed")

    workspace = benchmark_xctrace.create_capture_workspace(tmp_path)
    trace = workspace.path / "capture.trace"
    trace.mkdir()
    (trace / "partial").write_bytes(b"x")
    sidecar = workspace.path / "capture.trace.summary.json"
    sidecar.write_text("partial", encoding="utf-8")
    driver = FailingManaged("driver")
    recorder = FailingManaged("recorder")
    watcher = FailingManaged("watcher")

    errors = benchmark_xctrace.cleanup_capture(
        driver=driver,
        recorder=recorder,
        watcher=watcher,
        workspace=workspace,
        retain_outputs=True,
    )

    assert attempted == [
        "driver",
        "driver",
        "recorder",
        "recorder",
        "watcher",
        "watcher",
    ]
    assert len(errors) == 3
    for error, expected_handle in zip(
        errors,
        (driver, recorder, watcher),
        strict=True,
    ):
        cleanup_group = error
        assert isinstance(cleanup_group, BaseExceptionGroup)
        retained = [
            nested
            for nested in cleanup_group.exceptions
            if getattr(nested, "process_handle", None) is expected_handle
        ]
        assert len(retained) == 1
    assert not workspace.path.exists()


def test_capture_cleanup_retries_real_transient_final_group_kill(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    pid_path = tmp_path / "capture-cleanup-child.pid"
    sleeper = """
import os
import signal
import sys
import time
signal.signal(signal.SIGTERM, signal.SIG_IGN)
with open(sys.argv[1], "w", encoding="utf-8") as output:
    output.write(str(os.getpid()))
    output.flush()
time.sleep(60)
"""
    managed = process_control.ManagedProcess.start(
        [sys.executable, "-c", sleeper, str(pid_path)],
        cwd=tmp_path,
        label="capture cleanup retry",
        cleanup_grace_seconds=0.05,
    )
    deadline = __import__("time").monotonic() + 5
    while not pid_path.exists():
        if __import__("time").monotonic() >= deadline:
            pytest.fail("capture cleanup child did not start")
        __import__("time").sleep(0.01)
    child_pid = int(pid_path.read_text(encoding="utf-8"))
    original_killpg = process_control.os.killpg
    kill_attempts = 0

    def fail_first_sigkill(process_group: int, signum: int) -> None:
        nonlocal kill_attempts
        if signum == signal.SIGKILL:
            kill_attempts += 1
            if kill_attempts == 1:
                raise PermissionError("synthetic final group kill failure")
        original_killpg(process_group, signum)

    monkeypatch.setattr(process_control.os, "killpg", fail_first_sigkill)
    workspace = benchmark_xctrace.create_capture_workspace(tmp_path)
    try:
        errors = benchmark_xctrace.cleanup_capture(
            driver=managed,
            recorder=None,
            watcher=None,
            workspace=workspace,
            retain_outputs=False,
        )
        assert errors == []
        assert kill_attempts == 2
        assert managed.closed
        assert_process_gone(child_pid)
        assert not workspace.path.exists()
    finally:
        monkeypatch.setattr(process_control.os, "killpg", original_killpg)
        if not managed.closed:
            managed.terminate()


def test_capture_cleanup_retries_real_managed_notification_watcher(
    monkeypatch: pytest.MonkeyPatch,
    tmp_path: Path,
) -> None:
    pid_path = tmp_path / "managed-notification-watcher.pid"
    sleeper = """
import os
import signal
import sys
import time
signal.signal(signal.SIGTERM, signal.SIG_IGN)
with open(sys.argv[1], "w", encoding="utf-8") as output:
    output.write(str(os.getpid()))
    output.flush()
time.sleep(60)
"""
    watcher = process_control.ManagedProcess.start(
        [sys.executable, "-c", sleeper, str(pid_path)],
        cwd=tmp_path,
        label="managed notification watcher",
        cleanup_grace_seconds=0.05,
    )
    deadline = time.monotonic() + 5
    while not pid_path.exists():
        if time.monotonic() >= deadline:
            pytest.fail("managed notification watcher did not start")
        time.sleep(0.01)
    child_pid = int(pid_path.read_text(encoding="utf-8"))
    original_killpg = process_control.os.killpg
    group_kill_attempts = 0

    def fail_first_group_kill(process_group: int, signum: int) -> None:
        nonlocal group_kill_attempts
        if signum == signal.SIGKILL:
            group_kill_attempts += 1
            if group_kill_attempts == 1:
                raise PermissionError("synthetic watcher group kill failure")
        original_killpg(process_group, signum)

    monkeypatch.setattr(process_control.os, "killpg", fail_first_group_kill)
    workspace = benchmark_xctrace.create_capture_workspace(tmp_path)
    try:
        errors = benchmark_xctrace.cleanup_capture(
            driver=None,
            recorder=None,
            watcher=watcher,
            workspace=workspace,
            retain_outputs=False,
        )
        assert errors == []
        assert group_kill_attempts == 2
        assert watcher.closed
        assert_process_gone(child_pid)
        assert not workspace.path.exists()
    finally:
        monkeypatch.setattr(process_control.os, "killpg", original_killpg)
        if not watcher.closed:
            process_control.terminate_managed_process_with_retry(watcher)


def test_capture_cleanup_preserves_termination_class_after_all_attempts(
    tmp_path: Path,
) -> None:
    class CleanupTermination(BaseException):
        pass

    class InterruptingManaged:
        closed = False
        label = "notification watcher"
        supervisor = None
        ready_path = tmp_path / "watcher" / "ready.json"

        def terminate(self) -> None:
            raise interruption

    interruption = CleanupTermination()
    workspace = benchmark_xctrace.create_capture_workspace(tmp_path)
    trace = workspace.path / "capture.trace"
    trace.mkdir()
    sidecar = workspace.path / "capture.trace.summary.json"
    sidecar.write_text("partial", encoding="utf-8")

    errors = benchmark_xctrace.cleanup_capture(
        driver=None,
        recorder=None,
        watcher=InterruptingManaged(),
        workspace=workspace,
        retain_outputs=False,
    )

    assert len(errors) == 1
    assert benchmark_xctrace.termination_exceptions(errors[0]) == [
        interruption,
        interruption,
    ]
    assert not workspace.path.exists()
