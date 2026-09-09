# SRUI benchmarks

This suite implements the layer-separated methodology in design §31 and compares the macOS
renderer measurements with the numeric targets in §23.

Run a repeatable smoke measurement from the repository root:

    scripts/run-benchmarks --profile smoke

Results go to .benchmark-results/latest.json and latest.md. The command runs native release
drivers, writes each driver result to a private temporary file, enforces process-group timeouts,
merges the measurements, and then times the production §32 reconnect suite. Correctness failures
make the command fail. Performance misses remain successful measurements and are called out as
follow-up work when they exceed a §23 target by more than 2x.

For a logged-in macOS session with WindowServer, use:

    scripts/run-benchmarks --profile full

Full mode increases repetitions and forces AppKit display after layout. Record a reviewed,
machine-specific baseline only with:

    scripts/run-benchmarks --profile full --record-baseline

That explicit switch is the only path that overwrites benchmarks/reports/baseline.json and
baseline.md.

The representative coding-agent state is benchmarks/fixtures/coding-agent-ui.json. Both the Rust
serializer and Swift renderer consume it. PTY process startup is excluded from serialization.

Allocation attribution requires Instruments. After a normal full run, capture the driver with:

    benchmarks/parse-render/profile-allocations.sh /tmp/srui-allocations.trace

The JSON report always includes the allocator live-block delta and process resident peak. The
Instruments trace is intentionally not committed because it is large and host-specific.

Measurement policy:

- Smoke paint means mount plus layout and is safe for unattended runs.
- Full paint adds AppKit display and requires WindowServer.
- p50 values use deterministic nearest-index selection over native-driver samples.
- Wire byte and message counts are exact, not sampled.
- Refresh cadence changes only local presentation grouping; the encoded transaction stream is
  constructed once and reused.
- Network delay is outside the local interaction timing window by design: the benchmark proves
  text editing, caret/selection, IME, scrolling, hover/pressed state, and menu preparation do not
  wait for it.
- The report is evidence, not an optimizer. A measured shortfall becomes follow-up work.
