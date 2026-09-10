# SRUI benchmarks

This suite implements the layer-separated methodology in design §31 and compares the macOS
renderer measurements with the numeric targets in §23.

Run a repeatable smoke measurement from the repository root:

    scripts/run-benchmarks --profile smoke

Results go to .benchmark-results/latest.json and latest.md. The command runs native release
drivers, captures mandatory §31.1 Allocations evidence with Instruments, writes intermediate
results only to private temporary locations, validates the manifest, driver payloads, allocation
summary, and merged report against benchmarks/schema.json, enforces every stable metric/assertion
ID, unit, and §23 target contract, then times the production §32 reconnect suite. Renderer
candidates and capture processes have bounded process-group cleanup plus exact process-birth exit
postconditions. JSON and Markdown are staged, fsynced, and published as one rollback-protected
pair. The harness requires 12 GiB of free space before and during a run by default; set
`SRUI_BENCHMARK_MIN_FREE_BYTES` to another positive byte count for a constrained benchmark host.
Correctness failures make the command fail. Performance misses remain successful measurements and
are called out as follow-up work when they exceed a §23 target by more than 2x. Reports also fail
closed unless they record the exact chip, physical RAM, Xcode, Swift, Rust, Git commit, and Git
dirty state. Every driver publishes named sample-count groups, including one-shot boundary probes;
the runner requires each count to equal the selected smoke/full profile before merging or recording
Full mode increases repetitions and requires native/WebKit presentation completion plus exact
ScreenCaptureKit client-content evidence. On macOS, `scripts/run-benchmarks` runs the suite under
a lifetime-bounded `caffeinate -d -i -u` assertion: it wakes an online display and prevents idle
display/system sleep while the benchmark owns the process. It does not bypass a locked login
session or Screen Recording authorization. The responsible Codex or terminal app must already have
Screen Recording permission; the suite checks authorization and fails without prompting. This is
separate from the Developer Tools permission used by xctrace below. Record a reviewed,
machine-specific baseline only with:
    scripts/run-benchmarks --profile full

Full mode increases repetitions and requires native/WebKit presentation completion plus exact
ScreenCaptureKit client-content evidence. The responsible Codex or terminal app must already have
Screen Recording permission; the suite checks authorization and fails without prompting. This is
separate from the Developer Tools permission used by xctrace below. Record a reviewed,
machine-specific baseline only with:

    scripts/run-benchmarks --profile full --record-baseline

The runner rejects baseline recording from smoke mode and refuses to overwrite the committed
baseline until every correctness assertion passes. The committed-baseline check applies the same
full-profile and recordability gate.

The representative coding-agent state is benchmarks/fixtures/coding-agent-ui.json. Its declared
first-paint boundary splits the state into revision 0→1 and 1→2 transactions. Swift rendering and
Rust serialization publish the SHA-256 and byte count of the same deterministic
`[u64 big-endian length][protobuf]` pair; the runner requires exact artifact parity. WKWebView is
deliberately warmed and measured as a comparison control. It is not an SRUI production renderer,
and its paint or allocation results are not interchangeable with the native SRUI result. PTY
process startup is excluded from serialization.

## Allocation instrumentation

Allocation attribution is mandatory in consolidated smoke and full runs. The pinned reference
behavior is `xctrace version 26.0 (17C52)`. That version exposes Allocations through view-level
details under the Allocations track. The supported exports used for validation are:

    /trace-toc/run[@number="1"]/tracks/track[@name="Allocations"]/details/detail[@name="Statistics"]
    /trace-toc/run[@number="1"]/tracks/track[@name="Allocations"]/details/detail[@name="Allocations List"]

These are not the generic raw-data query
`/trace-toc/run/data/table[@schema="allocations"]`. Always inspect a trace's own table of
contents before assuming another Xcode build has the same views.

The runner creates a token-owned temporary capture workspace and records equivalent exact-process
passes for the SRUI host and the warmed WebKit control's host, WebContent process, and every
available Network/GPU helper. Each target is identified by PID plus Darwin process birth time.
Measurement cannot begin until xctrace emits the requested Darwin recording-start notification;
progress text or a partially initialized trace bundle is not readiness evidence. An optional
Network/GPU role may be absent, but absence must be published by the pre-measurement handshake. PID
aliases across roles are reported and counted once. Rows from another process birth or unrelated
same-named process are never attributed.

The primary interval metric is the count and byte sum of all Allocations List rows whose allocation
timestamp falls inside the candidate's measured interval and whose allocation is still live when
the trace is finalized. It is therefore an **interval-created-and-still-live** heap-and-anonymous-
VM measurement. It is not total allocation traffic, allocator call count, heap growth, or a count
of allocations that were created and freed during the interval. The List is live-only in 17C52
and must reconcile exactly with `All Heap & Anonymous VM` persistent totals. The parser also proves
that this combined Statistics row equals `All Heap Allocations` plus `All Anonymous VM` for all
seven exported fields. `VM:` List-category counts remain diagnostics only: retained probes proved
that the prefix is not a valid partition of those two Statistics aggregates, so no row is silently
excluded or reclassified by name.

Statistics is retained as a whole-trace consistency check. Each of the three validated aggregate
rows (`All Heap & Anonymous VM`, `All Heap Allocations`, and `All Anonymous VM`) obeys:

    persistent-bytes + transient-bytes = total-bytes
    count-persistent + count-transient = count-total

`count-events` is an independent count of allocation and deallocation events, not an allocation
count and not a value derivable from the other fields for an attached process. In particular, an
object allocated before attachment can be deallocated during recording. Furthermore, `--attach`
seeds Statistics with the live heap present at attachment. Because 17C52 provides no validated
range-scoped Statistics export for the inner benchmark interval, its `total-*` and `count-total`
values must not be reported as interval cumulative allocations.

Candidate interval timestamps are Unix-epoch nanoseconds; List timestamps are trace-relative
nanoseconds anchored to the TOC start date. The parser publishes
`timestamp_boundary_uncertainty_ns` from the TOC start-date resolution plus the List's 1 µs display
resolution. On the validated 17C52 traces this is a 1.001 ms boundary caveat. Each sample therefore
carries nominal retained values, definite lower bounds, possible upper bounds, and the
boundary-ambiguous difference; exact PID attribution does not make the independent clocks
sub-millisecond-exact.

Exact-process attachment has two independent authorization gates:

- System developer mode must report enabled from
  `/usr/sbin/DevToolsSecurity -status`; if necessary, an administrator can enable it with
  `sudo /usr/sbin/DevToolsSecurity -enable`.
- The responsible terminal or Codex host must be enabled in **System Settings → Privacy &
  Security → Developer Tools**. Restart that app after changing the grant.

Full Disk Access is not required and should not be granted as an attachment workaround. The target
copy also needs the minimal `com.apple.security.get-task-allow=true` entitlement. The harness
copies the release BenchmarkDriver into its private staging workspace and ad-hoc signs that copy;
it must never re-sign or otherwise mutate `client-macos/.build/release/BenchmarkDriver`. This
keeps SwiftPM's build artifact and concurrent builds untouched. The staged signature is benchmark
instrumentation only and must not be distributed.

The merged report contains p50/p95/p99 interval-created-and-still-live heap-and-anonymous-VM counts and bytes; the
per-sample evidence retains `retained_allocations`, `retained_bytes`, their lower/upper bounds, and
`boundary_ambiguous_*` values. It also carries exact-role coverage, List/Statistics reconciliation,
readiness evidence, and the timestamp-boundary caveat. It contains no machine-local trace path.
Normal runs remove raw traces and XML exports before publication.

For retained diagnostic investigation, use a destination that does not already exist:

    benchmarks/parse-render/profile-allocations.sh /tmp/srui-allocations.trace

This uses the same bounded capture/parser path and publishes the trace,
`/tmp/srui-allocations.trace.summary.json`, and the requested driver result. A `.trace` is a
directory bundle: `stat -f %z /tmp/srui-allocations.trace` reports only the directory entry, not
the capture size. The harness's `trace_bytes` recursively sums contained file sizes without
following symlinks; `du -sk /tmp/srui-allocations.trace` is a useful on-disk diagnostic but is not
the same quantity. Default limits are a 2 GiB recursive trace size, a 256 MiB export, and a 4 GiB
free-space reserve; override them with `SRUI_XCTRACE_MAX_BYTES`,
`SRUI_XCTRACE_MAX_EXPORT_BYTES`, and `SRUI_XCTRACE_MIN_FREE_BYTES`. The normal suite also
requires 12 GiB free by default.

See benchmarks/parse-render/README.md for the exact export, signing, reconciliation, and failure
diagnostics and for the Apple/manpage references behind this contract.

The consolidated suite is macOS-only because §31.1, §31.3, §31.4, and the client half of §31.5/6
exercise AppKit, WebKit, WindowServer, and Instruments. Unsupported hosts fail during platform
validation before any Rust or Swift benchmark driver starts.

The methodology has a deliberate portability boundary. Protocol generation/serialization, exact
wire counters, mutation ordering, reconnect boundaries, result-cache idempotency, PTY byte
comparison, and ring-exhaustion scenarios can be reused on Linux and Windows. Renderer presentation,
local input, process-allocation, and peak-memory evidence need native adapters: for example a
GTK/Qt frame-clock plus compositor/PipeWire capture and `perf`/heaptrack-class instrumentation on
Linux, or a WinUI/WPF target plus DWM/ETW presentation and Windows heap tooling. Those adapters must
preserve this suite's same-frame target-pixel and exact-process-attribution contracts. §23's numeric
targets are macOS renderer targets and must not silently become cross-platform acceptance thresholds;
each future renderer needs separately justified platform targets while retaining the common report
schema.

## Measurement policy

- Smoke paint uses deterministic offscreen presentation and is safe for unattended runs.
- Full first/complete paint starts ScreenCaptureKit before the production action, keeps the target
  hidden through decode/load, apply, and geometry, then performs one animation-free order-front
  submission. It accepts the first later complete frame that proves exact target identity,
  client/target geometry, unobscured z-order, and changed nonblank/nonuniform target pixels.
- Full passive mutation/local-interaction timing starts ScreenCaptureKit before the action, obtains
  a complete-frame baseline for the exact visible target-control ROI, ignores frames through action
  completion, and accepts only a later complete frame with identical pixel geometry and a changed
  nonblank/nonuniform ROI. Latency uses action-start Mach ticks through that frame's
  `SCStreamFrameInfo.displayTime`; callback receipt is retained only as verifier metadata. Exact
  window, client-content, target geometry, display, ownership, and z-order are rechecked.
- Full menu timing detects a new current-process popup-level WindowServer surface, verifies its exact
  crop in the same frame, and likewise uses its display timestamp before cancelling menu tracking.
- p50/p95/p99 values use deterministic nearest-index selection over native-driver samples.
- Wire byte and message counts come from the protocol transport tap, not estimates.
- For each 1/100/1,000-update case, the same pre-encoded one-operation transaction sequence is
  replayed at 60/120/144/240 Hz. The source latches only values already visible in the production
  AppKit control; cadence ticks coalesce those native-value changes, and the painted revision is
  read back from that control after the draw.
- Network impairment is injected at the transport boundary. Local control updates are timed
  separately from server-dependent feedback and must not acquire the injected RTT. Every local
  trial must also reach its visible boundary while one exact paired production transaction remains
  blocked before delivery. This proves non-dependence on that response; it does not claim the
  configured one-way-delay interval stayed active for the whole action. Local p50 targets use the
  positive display-frame budget emitted for that run instead of a hard-coded 60 Hz interval.
- The report is evidence, not an optimizer. A measured shortfall becomes follow-up work.
