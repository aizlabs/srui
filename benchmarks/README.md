# SRUI benchmarks

This suite implements the layer-separated methodology in design §31 and compares the macOS
renderer measurements with the numeric targets in §23.

Run a repeatable smoke measurement from the repository root:

    scripts/run-benchmarks --profile smoke

Results go to .benchmark-results/latest.json and latest.md. The command runs native release
drivers, writes each driver result to a private temporary file, validates the manifest, native
driver payloads, and merged report against benchmarks/schema.json, enforces each driver's and
the merged report's exact stable metric/assertion-ID, unit, and §23 target-metadata inventory,
enforces process-group timeouts and renderer process-birth exit postconditions, merges the
measurements, and then times the production §32 reconnect suite. JSON and Markdown are staged,
fsynced, and published as one rollback-protected pair. The harness
requires 12 GiB of free space before and during a run by default; set
`SRUI_BENCHMARK_MIN_FREE_BYTES` to another positive byte count for a constrained benchmark host.
Correctness failures make the command fail. Performance misses remain successful measurements and
are called out as follow-up work when they exceed a §23 target by more than 2x.

For a logged-in macOS session with WindowServer, use:

    scripts/run-benchmarks --profile full

Full mode increases repetitions and observes native and web presentation completion. Record a
reviewed, machine-specific baseline only with:

    scripts/run-benchmarks --profile full --record-baseline

The runner rejects baseline recording from smoke mode and refuses to overwrite the committed
baseline until every correctness assertion passes. The committed-baseline check applies the same
full-profile and recordability gate.

The representative coding-agent state is benchmarks/fixtures/coding-agent-ui.json. Both native
drivers consume it and publish the SHA-256 and byte count of the exact canonical transaction; the
runner requires both artifacts to match. PTY process startup is excluded from serialization.

Allocation attribution requires Instruments. After a normal full run, capture every involved
process with:

    benchmarks/parse-render/profile-allocations.sh /tmp/srui-allocations.trace

The helper records the Allocations template with xctrace `--all-processes`, starts a full-profile
driver scoped to §31.1 only after xctrace reports that recording began, and stops/finalizes the
recorder as soon as the driver finishes. It cleans up the recorder, notification watcher, driver, and incomplete trace on every
failure or interruption. The default live limits are a 2 GiB trace, a 256 MiB allocation export,
and a 4 GiB free-space reserve; override them with `SRUI_XCTRACE_MAX_BYTES`,
`SRUI_XCTRACE_MAX_EXPORT_BYTES`, and `SRUI_XCTRACE_MIN_FREE_BYTES`.

A successful capture also writes `TRACE.summary.json`. Each renderer runs in a fresh child
process and reports its host PID, benchmark interval, outer-driver PID, helper-PID source, and a
Darwin process-birth token for every host/helper PID. After the driver completes, the harness waits
until each exact process identity has exited; numeric PID reuse satisfies this postcondition, but a
same-birth process that remains alive fails the run. Attributed PIDs are never signaled directly.
Candidate subgroups install a parent-birth watchdog before renderer work so outer-driver
interruption also terminates an in-flight candidate before it can publish attribution.
The sidecar contains cumulative allocation counts and bytes only for those explicitly reported
candidate PIDs and only for timestamped rows inside each candidate's reported Unix-epoch
nanosecond interval. It converts xctrace's relative nanosecond timestamps using the trace
`start-date` and fails closed if either timestamp source is unavailable or unparseable. WebKit
helper rows are accepted only for the diagnostic process identifiers reported by that candidate;
a same-named WebKit process elsewhere on the system—or the same numeric PID before or after the
candidate interval—is classified as unrelated, never attributed by name or PID alone. Missing
helper rows and unattributed rows are reported rather than guessed. Unrelated system processes remain in the host-specific trace because capture is
intentionally system-wide. Neither the trace nor its sidecar is committed or folded into the main
benchmark report.

Measurement policy:

- Smoke paint uses deterministic offscreen presentation and is safe for unattended runs.
- Full paint uses WindowServer-backed presentation markers.
- p50/p95/p99 values use deterministic nearest-index selection over native-driver samples.
- Wire byte and message counts come from the protocol transport tap, not estimates.
- Configured renderer cadence changes only presentation scheduling; the independently emitted
  mutation stream is observed at the wire tap for every cadence.
- Network impairment is injected at the transport boundary. Local control updates are timed
  separately from server-dependent feedback and must not acquire the injected RTT. Their p50
  targets use the positive local-display frame budget emitted for that run, rather than a
  hard-coded 60 Hz interval.
- The report is evidence, not an optimizer. A measured shortfall becomes follow-up work.
