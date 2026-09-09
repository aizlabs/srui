# SRUI benchmarks

This suite implements the layer-separated methodology in design §31 and compares the macOS
renderer measurements with the numeric targets in §23.

Run a repeatable smoke measurement from the repository root:

    scripts/run-benchmarks --profile smoke

Results go to .benchmark-results/latest.json and latest.md. The command runs native release
drivers, writes each driver result to a private temporary file, validates the manifest, native
driver payloads, and merged report against benchmarks/schema.json, enforces process-group
timeouts, merges the measurements, and then times the production §32 reconnect suite. Correctness
failures make the command fail. Performance misses remain successful measurements and are called
out as follow-up work when they exceed a §23 target by more than 2x.

For a logged-in macOS session with WindowServer, use:

    scripts/run-benchmarks --profile full

Full mode increases repetitions and observes native and web presentation completion. Record a
reviewed, machine-specific baseline only with:

    scripts/run-benchmarks --profile full --record-baseline

The runner rejects baseline recording from smoke mode and refuses to overwrite the committed
baseline until every correctness assertion passes.

The representative coding-agent state is benchmarks/fixtures/coding-agent-ui.json. Both native
drivers consume it and publish the SHA-256 and byte count of the exact canonical transaction; the
runner requires both artifacts to match. PTY process startup is excluded from serialization.

Allocation attribution requires Instruments. After a normal full run, capture every involved
process with:

    benchmarks/parse-render/profile-allocations.sh /tmp/srui-allocations.trace

The helper records the Allocations template with xctrace `--all-processes`, starts the driver only
after xctrace reports that recording began, and cleans up the recorder, notification watcher, and
driver on timeout or interruption. Filter the trace by `BenchmarkDriver` and the WebKit helper
processes when comparing renderer allocations; unrelated system processes are present because the
capture is intentionally system-wide. The trace is host-specific and is not committed or folded
into the JSON report.

Measurement policy:

- Smoke paint uses deterministic offscreen presentation and is safe for unattended runs.
- Full paint uses WindowServer-backed presentation markers.
- p50/p95/p99 values use deterministic nearest-index selection over native-driver samples.
- Wire byte and message counts come from the protocol transport tap, not estimates.
- Configured renderer cadence changes only presentation scheduling; the independently emitted
  mutation stream is observed at the wire tap for every cadence.
- Network impairment is injected at the transport boundary. Local control updates are timed
  separately from server-dependent feedback and must not acquire the injected RTT.
- The report is evidence, not an optimizer. A measured shortfall becomes follow-up work.
