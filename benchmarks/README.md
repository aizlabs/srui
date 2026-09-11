# SRUI benchmarks

This suite implements the layer-separated methodology in design §31 and compares the macOS
renderer measurements with the numeric targets in §23.

`benchmarks/metric-contract.json` is the authoritative metric contract. It owns driver commands,
metric and assertion IDs, display names, units, statistics, targets and directions, profile
iterations, sample-count formulas, and percentile semantics. `benchmarks/schema.json` separately
owns the JSON report shape. Every driver and merged report declares `contract_schema_version` and
the canonical compact/sorted UTF-8 `contract_sha256`; mixed or stale producers fail validation.
Run `python3 benchmarks/generate_metric_contract.py --check` to verify the checked-in Swift and
Rust contract descriptors.

Run a repeatable smoke measurement from the repository root:

    scripts/run-benchmarks --profile smoke

Results go to .benchmark-results/latest.json and latest.md. The command runs native release
drivers, validates the manifest, driver payloads, and merged report against
benchmarks/schema.json, enforces every stable metric/assertion ID, unit, and §23 target contract,
then times the production §32 reconnect suite. Renderer candidates have bounded process-group
cleanup plus exact process-birth exit postconditions. JSON and Markdown are staged, fsynced, and
published as one rollback-protected pair. The harness requires 12 GiB of free space before and
during a run by default; set
`SRUI_BENCHMARK_MIN_FREE_BYTES` to another positive byte count for a constrained benchmark host.
Correctness failures make the command fail. Performance misses remain successful measurements and
are called out as follow-up work when they exceed a §23 target by more than 2x. The complete
six-section report is macOS-only while §31.1/31.3/31.4 require AppKit. On Linux, the runner rejects
the platform before starting Rust and writes no misleading partial report. Reports also fail
closed unless they record the exact chip, physical RAM, Xcode, Swift, Rust, Git commit, and Git
dirty state. Every driver publishes named sample-count groups, including one-shot boundary probes;
the runner requires each count to equal the selected smoke/full profile before merging or recording.

Full mode increases repetitions and requires native/WebKit presentation completion plus exact
ScreenCaptureKit client-content evidence. On macOS, `scripts/run-benchmarks` runs the suite under
a lifetime-bounded `caffeinate -d -i -u` assertion: it wakes an online display and prevents idle
display/system sleep while the benchmark owns the process. It does not bypass a locked login
session or Screen Recording authorization. When the public session dictionary reports
`CGSSessionScreenIsLocked`, the driver fails before opening a measurement window; if that
diagnostic key is absent, the exact WindowServer/pixel checks still fail closed. The responsible
Codex or terminal app must already have Screen Recording permission; the suite checks authorization
and fails without prompting. Developer Tools permission is not required; the rejected xctrace
prototype is not runnable.

Every full-mode benchmark host resolves its WindowServer stratum at runtime with
`CGWindowLevelForKey(.statusWindow)`. Before ordering the host, the driver resolves
`.dockWindow`, `.statusWindow`, `.popUpMenuWindow`, and
`.screenSaverWindow` and requires `dock < status < popup < screen-saver`. This puts the
evidence window above the Dock-owned desktop surface that can otherwise precede it in the window
list while keeping it below real AppKit pop-up menus. Exact target and z-order inventories use
`CGWindowListCopyWindowInfo(.optionOnScreenOnly, ...)`; membership in that filtered result is the
on-screen proof. WindowServer may omit the redundant optional `kCGWindowIsOnscreen` dictionary
field, so its absence is not treated as missing evidence. Exact window ID, owner PID, resolved
layer, alpha, bounds, display, client-content geometry, and target geometry remain mandatory.

Full passive §31.3, §31.4, and §31.6 runs save the exact Quartz pointer location, park it at
`display.maxX - 160` and the display's vertical midpoint, and restore it on section exit,
including failure. Their deterministic host is left-side, so this interior right-side point stays
outside the target while avoiding Dock, menu-bar, and hot-corner edge activation zones. Pointer
relocation can still start the retraction of an already activated Dock or menu surface. Untimed
preparation therefore allows up to 10 seconds to acquire the exact `.optionOnScreenOnly`
WindowServer identity. A separate 10-second geometry-settling deadline must contain two identical
observations of that exact identity, window bounds, and client-content ROI spaced 300 ms apart.
Preparation then allows up to 10 seconds for a genuinely clear z-order before capture. A geometry
timeout reports the prepared AppKit frame and current WindowServer entry fields.

Full §31.1 instead has two nested guards. The top-level parent saves the user's exact Quartz
location, parks at `display.minX + 160` and the vertical midpoint before spawning either
ordinary visual candidate, then restores the user's location after both candidates or on failure.
Each visual SRUI/WebKit subprocess independently saves its inherited pointer location, repeats
the same left-interior park immediately before entering its candidate measurement function, and
restores that child-local value on exit. Every full paint then refreshes that park after accepting
the pre-action ScreenCaptureKit baseline and immediately before the action-start timestamp.
WebKit positions its already-created hidden window at that point; the native path creates and
positions its hidden window during the timed production attach. This closes physical pointer
movement during child warm-up or capture startup without moving the cursor inside a reported
interval. Each park and restore also posts the matching public session-level `mouseMoved` event:
Quartz warping alone emits no mouse event and can otherwise leave the previously hovered app\'s
tooltip frozen ahead of the benchmark after that app deactivates. Non-compositor smoke passes use deterministic hidden geometry without requiring or moving the
pointer. The child-side guard is
authoritative: it closes the parent-to-child build/spawn race and completes before any candidate
measurement interval starts.

That interior position remains outside the right-corner 960-point renderer ROI. Each hidden native
or WebKit window chooses the visible-frame corner farthest from the parked pointer with 64 points
of clearance before it is ordered. If no position is cursor-free, the failure reports the exact
pointer location and every candidate frame.

These placements are not owner, Dock, pointer, or cursor whitelists. `kCGWindowAlpha` is
whole-window metadata, not per-pixel opacity, and every intersecting nonzero-alpha surface
ahead—including a WindowServer cursor or `loginwindow` shield—still makes timed, pre-action,
accepted-frame, post-action, and restoration checks fail closed.

A standalone diagnostic proves both sides with real overlapping opaque windows: a Dock-level
window must remain below the status host, while the same exact window at `status + 1` must be
rejected by WindowServer ID and layer:

    env SRUI_BENCHMARK_PHASES=1 SRUI_BENCHMARK_WINDOW_ISOLATION_SELF_TEST=1 client-macos/Benchmarks/.build/release/BenchmarkDriver --fixture benchmarks/fixtures/coding-agent-ui.json --output /tmp/srui-window-isolation-unused.json --profile full

The diagnostic exits before running benchmark sections, so its temporary windows and activation
state cannot contaminate reported timing or pixels; the required output argument is intentionally
unused.

ScreenCaptureKit supplies the exact composited frame, WindowServer identity/geometry, target
pixels, and display timestamp used for visible latency. Signed in-process allocator endpoint
samples supply the authoritative allocation metrics. Xctrace data is excluded from the report and is never treated as proof for either measurement.

Record a reviewed, machine-specific baseline only with:

    scripts/run-benchmarks --profile full --record-baseline

Do not replace the committed baseline until every correctness assertion passes. The
committed-baseline check applies the same
full-profile and recordability gate.

The representative coding-agent state is benchmarks/fixtures/coding-agent-ui.json. Its declared
first-paint boundary splits the state into revision 0→1 and 1→2 transactions. Swift rendering and
Rust serialization publish the SHA-256 and byte count of the same deterministic
`[u64 big-endian length][protobuf]` pair; the runner requires exact artifact parity. WKWebView is
deliberately warmed and measured as a comparison control. It is not an SRUI production renderer,
and its paint or allocation results are not interchangeable with the native SRUI result. PTY
process startup is excluded from serialization.

## Allocation measurement

The reported §31.1 allocation metrics are signed host/default-zone endpoint deltas from
`malloc_zone_statistics` around the representative CPU/resource pass:

    after.blocks_in_use - before.blocks_in_use
    after.size_in_use   - before.size_in_use

The report publishes p50/p95/p99
`{srui,webkit}.host_net_live_allocation_{blocks,bytes}` values. They describe net live state,
not cumulative allocation calls or traffic; negative values are valid and are never clamped. The
count metric is reported in live `blocks`, never as a count of allocation calls.
SRUI's value covers its renderer host. WKWebView is an explicitly host-only comparison control and
excludes WebContent, Networking, GPU, and other helper allocations. The scope assertion and metric
names make that limitation machine-readable.

Task 34 deliberately does not claim a cumulative allocation-event count or total requested bytes.
A real `MallocStackLoggingNoCompact=1` SRUI-host trial showed that the first pre-workload
`malloc_history -allEvents` export alone expanded to 1,902,439,272 bytes. The supported CLI has no
time-range or no-stack mode; producing six full histories for three interval samples would be an
unsafe default and was rejected rather than silently weakened. The concrete follow-up is [issue #48](https://github.com/aizlabs/srui/issues/48): a
benchmark-only Darwin allocator-interposition counter, following Apple SwiftNIO's established
pattern, with atomic counters bracketing the separate resource pass. Until that exists, do not cite
the endpoint deltas as allocation-call counts or as complete cumulative-allocation evidence.

Xctrace is not used by smoke/full runs and does not populate or gate the committed report. Real
Xcode 26 captures disproved the required interval timestamp alignment and exact List/Statistics
reconciliation, while attaching to warmed WebKit could stall inside JavaScriptCore allocator
enumeration. The experimental runner, entitlement, exporter, and tests were removed so a future
agent cannot accidentally cite that diagnostic as §31.1 evidence. Full visual runs require Screen
Recording for ScreenCaptureKit; no benchmark mode requires Developer Tools permission.

For the complete evidence, failed approaches, exact observed values and stack locations,
authorization/signing behavior, cleanup and disk safeguards, reproduction guidance, and
Linux/Windows portability notes, see the
[technical findings](parse-render/INSTRUMENTATION_FINDINGS.md). The
[parse/render guide](parse-render/README.md) stays focused on operation.
- Smoke paint uses deterministic offscreen presentation and is safe for unattended runs.
- Full first/complete paint uses both parent-side and authoritative candidate-local pointer
  park/restore guards. The candidate-local guard runs after spawn and entirely before its
  measurement function, closing the parent-to-child race without entering a timed interval. The
  benchmark then starts ScreenCaptureKit before the production child action, keeps the target
  hidden through decode/load, apply, and geometry, and performs one animation-free order-front
  submission. A frame is eligible
  only after exact target identity, client/target geometry, unobscured z-order, nonblank/nonuniform
  pixels, and at least eight normalized ROI pixels with any RGBA channel delta greater than the
  explicit 2/255 tolerance versus the same-stream baseline are proven. The full result separately
  requires the accepted complete state to differ from the accepted first state by the same material
  threshold. WebKit enforces that prior-state comparison while selecting its complete frame, so a
  stale partially populated surface remains ineligible. SHA-256 fingerprints are diagnostics, not
  the distinctness predicate.
- Full passive mutation/local-interaction/terminal preparation saves and parks the real pointer at
  `display.maxX - 160`, uses deterministic left-side placement, permits bounded untimed
  WindowServer settling, requires stable exact geometry, and waits for a clear z-order before
  starting ScreenCaptureKit. This parking keeps cursor/system UI outside the ROI; §31.4 hover state
  itself is driven separately through a deterministic benchmark SPI pointer context into the
  production `HoverFeedbackButton` reconciliation path. Exact `.optionOnScreenOnly` identity
  acquisition, the geometry interval, and clear-z-order acquisition each have a 10-second bound.
  Geometry requires identical exact identity, bounds, and client-content ROI across 300 ms. The
  original real pointer location is restored on exit. A complete-frame baseline is acquired before
  the action. Persistent interactions accept only frames after action completion; transient hover
  and pressed feedback may accept frames after action start. In either case, a frame is eligible
  only when geometry is identical, content remains nonblank/nonuniform, and at least eight unmasked
  ROI pixels have any RGBA channel change greater than the explicit 2/255 SCStream tolerance.
  Latency uses action-start Mach ticks through that frame's `SCStreamFrameInfo.displayTime`;
  callback receipt is verifier metadata. Exact window, client-content, target geometry, display,
  ownership, and z-order are checked before and after. Hover then drives the production reconciler
  to an outside context and compares same-API ScreenCaptureKit screenshots; restoration requires
  zero unmasked pixels with any channel delta greater than 5/255.
- Full menu timing detects a new current-process popup-level WindowServer surface, verifies its
  exact crop in the same frame, and likewise uses its display timestamp before cancelling menu
  tracking.
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
