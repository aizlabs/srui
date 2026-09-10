# §31.1 parse/render benchmark

Owners: `client-macos/Benchmarks/ParseRenderBenchmark.swift` (`localRenderer`,
`runSRUICandidate`, `runWebCandidate`, and `loadAndObserveWebStates`) and
`benchmarks/parse-render/run_xctrace.py` (capture, export, attribution, and reconciliation).
The same fixture is split at its declared `first_paint_node_count` into two canonical
transactions: revision 0→1 creates a useful Surface → Column → Row → Text + Progress subtree, and
revision 1→2 creates the remaining representative UI. Rust and Swift hash the identical
`[u64 big-endian length][protobuf]` pair. The native candidate decodes and applies the first
transaction through the production initial `attach(store:)` path, observes that presentation,
then applies the second through production `apply(transaction:newStore:)` and independently
observes complete presentation. The WKWebView comparison control presents equivalent first and
complete semantic DOM states with exact final-node sentinels; it is not a production SRUI
implementation.

Each logical sample uses four disjoint measured workloads so instrumentation does not measure
itself: a first-state visual-latency pass, a complete two-state visual-latency pass, a complete
two-state CPU/live-allocation/footprint-growth pass, and a separate 1 ms-cadence peak-footprint
pass. Native uses separately warmed and reset renderer instances; WebKit resets the same warmed
view between passes while requiring its exact helper PID-role topology to remain stable. Warm-up
and every reset remain hidden/offscreen. The resource and peak passes execute the same production
decode/load, apply, render, and display-submission work, but start no ScreenCaptureKit stream; the
periodic footprint sampler is never alive during the CPU/allocation pass.

Smoke mode observes separate real offscreen AppKit bitmap rasters and WKSnapshot outputs for the
first and complete states; it does not claim compositor-visible paint. Full mode uses two nested
pointer guards. Before spawning candidates, the parent records the user's exact Quartz location,
parks at `display.minX + 160` and the measured display's vertical midpoint, and restores the user
location after both candidates or during failure unwinding. Each full SRUI or WebKit subprocess
then independently saves its inherited pointer location, repeats the same left-interior park
immediately before entering its candidate measurement function, and restores that child-local
location on exit. This child-side guard is authoritative: it closes a parent-to-child build/spawn
race and completes before every candidate measurement interval.

The interior location avoids auto-hidden Dock, menu-bar, and hot-corner activation zones while
remaining outside the right-corner 960-point renderer ROI. While hidden, each candidate window
selects the visible-frame corner farthest from the parked pointer and requires 64 points of
clearance. If no position is cursor-free, the diagnostic includes the exact pointer location and
all four candidate frames. The pointer and WindowServer cursor surface are not whitelisted; any
reported intersecting nonzero-alpha surface ahead still fails closed.

Full mode prepares its ScreenCaptureKit stream and baseline before starting the workload. The
complete first-state or two-state production workload runs while the target window remains hidden;
the capture helper performs the sole window ordering and accepts a later complete display frame
only after proving the exact target. A refresh that races ahead of WindowServer publication is
rejected and does not become a false failure or timestamp. The reported visual latency is
calculated from the Mach action-start timestamp to the accepted frame's ScreenCaptureKit display
timestamp, not callback receipt or later image verification.

The exact target-view crop must be visible, unobscured, nonblank, nonuniform, and materially
different from its same-stream baseline: at least eight normalized pixels must have any RGBA
channel delta greater than the explicit 2/255 tolerance. The accepted first and complete captures
must have equal normalization and geometry, and at least eight normalized pixels must also exceed
that threshold between those two states. WebKit applies this first-state comparison while selecting
the complete frame, leaving the stream armed when a stale partial surface is observed. SHA-256
fingerprints remain in evidence details for diagnosis only; raw hash inequality is not the
distinctness criterion. Exact semantic/control/DOM state is then checked on that same measured
renderer or WebView instance. Full mode fails closed if capture authorization or any evidence is
unavailable; the suite never opens a permission prompt.

Focused driver command:

    client-macos/.build/release/BenchmarkDriver --fixture benchmarks/fixtures/coding-agent-ui.json --profile smoke --only-section 31.1 --output /tmp/srui-31.1.json

The renderer metric set includes `srui.first_paint`, `srui.complete_paint`, `srui.cpu`,
`srui.host_net_live_allocation_blocks`, `srui.host_net_live_allocation_bytes`,
`srui.process_footprint_peak`, and the corresponding `webkit.*` comparison-control metrics.
Allocation values are signed default-zone endpoint deltas, not cumulative allocation events; the
count metric's unit is live `blocks`, not allocation calls. Xctrace is diagnostic-only and
contributes no committed §31.1 metric or workload-interval total.

## Full-mode compositor isolation and evidence chain

Full-mode host windows use the level returned at runtime by
`CGWindowLevelForKey(.statusWindow)`; the benchmark does not depend on numeric level constants.
It also resolves `.dockWindow`, `.popUpMenuWindow`, and `.screenSaverWindow` and
refuses to measure unless `dock < status < popup < screen-saver`. The status stratum isolates
the host from the Dock-owned desktop surface while keeping it below the production `NSMenu`
surface. Exact WindowServer inventories use `.optionOnScreenOnly`; membership in that filtered
result is the on-screen proof even when WindowServer omits the redundant optional
`kCGWindowIsOnscreen` dictionary field. Exact identity, owner, resolved layer, alpha, bounds,
display, client-content geometry, and target geometry remain required. The host evidence must
report the resolved status layer, and menu evidence must report the independently resolved pop-up
layer above it.

The parent-side park protects ordinary visual candidate launch, while each visual candidate's
local park protects geometry setup from launch-time pointer movement. Both guards run before
measured work and restore their own saved Quartz location. Non-compositor smoke and allocation
passes use deterministic hidden geometry without a cursor-clear requirement, so they neither
depend on nor hold the user's pointer. The resulting interior point avoids edge-triggered
system windows and makes a cursor-free right corner available to the 960×720 renderer window.
Hidden-window placement avoids the parked cursor geometrically; it does not remove a cursor entry
from the z-order inventory. Candidate-frame and post-comparison checks still reject any
intersecting nonzero-alpha window ahead. No timing, visibility, or occlusion rule is relaxed.

The ScreenCaptureKit paint-evidence path is:

1. Start a display stream and accept a complete pre-action baseline.
2. Run the production decode/apply/render workload while the target host is hidden.
3. Arm a Mach display-time cutoff immediately before the sole order-front submission.
4. For each later complete frame, derive the exact target window identity, resolved status layer,
   display, bounds, client-content rectangle, and target-view crop from AppKit and WindowServer.
5. Reject the frame if any on-screen, nonzero-alpha window ahead intersects the target; then require
   nonblank/nonuniform content and at least eight normalized pixels with any RGBA channel delta
   greater than 2/255 versus the same-frame baseline crop.
6. Requery identity, geometry, crop, and z-order after the pixel comparison and accept only if they
   remain unchanged. Visible latency ends at that frame's ScreenCaptureKit
   `SCStreamFrameInfo.displayTime`, not callback receipt.

`kCGWindowAlpha` is the compositor's whole-window alpha metadata, not a per-pixel opacity map. In
particular, a full-display Dock-owned surface can report alpha 1 while its target-area pixels are
transparent. Treating owner `Dock` or alpha 1 as pixel coverage would be false evidence, so the
suite has no Dock, owner-name, or window-number exception. Raising the host only to the dynamically
resolved status stratum removes that surface from the ahead set; every remaining intersecting
nonzero-alpha entry still rejects the sample. This includes `loginwindow` or other shielding
surfaces, so an inactive or locked session remains a hard failure.
## Allocation measurement

The authoritative allocation metrics are signed default-zone endpoint deltas sampled immediately
before and after the separate representative CPU/resource pass:

    after.blocks_in_use - before.blocks_in_use
    after.size_in_use   - before.size_in_use

They measure host-process net live state, not cumulative allocator traffic. Negative deltas are
valid. SRUI's host contains the native renderer; WKWebView's allocation metrics are explicitly a
host-only comparison control and exclude WebContent, Networking, and GPU helper processes. The
report publishes p50/p95/p99 block and byte deltas and checks the declared scope and sample counts.

## Optional xctrace diagnostic

Xctrace is not part of the authoritative suite or committed baseline. Use it only to investigate
the SRUI host:

    benchmarks/parse-render/profile-allocations.sh /tmp/srui-allocations.trace

The destination must not already exist. The output sidecar uses schema version 4 and explicitly
sets `diagnostic_only=true` and `authoritative_benchmark_metric=false`. It contains exact
PID/process-birth/liveness evidence, tool metadata, internally validated whole-trace Statistics,
the final live Allocations List, and signed Statistics-minus-List discrepancies. The recorded
workload window is context only and has
`used_for_allocation_attribution=false`. No List timestamp is converted to wall-clock time and no
interval allocation total is reported.

On Xcode 26.0 (17C52), the supported Allocations view-detail paths are discovered from the trace
TOC and exported as:

    /trace-toc/run[@number="1"]/tracks/track[@name="Allocations"]/details/detail[@name="Statistics"]
    /trace-toc/run[@number="1"]/tracks/track[@name="Allocations"]/details/detail[@name="Allocations List"]

Do not substitute the generic `/trace-toc/run/data/table[@schema="allocations"]` path.
`count-events` is not an allocation count. List and Statistics are independently materialized
diagnostics and are not required to match.

The diagnostic deliberately instruments SRUI's host only. Attaching Allocations to warmed WebKit
was observed to stall during JavaScriptCore/libmalloc root enumeration and would materially
perturb the control. Separate helper traces would not be one coherent multiprocess sample.

## Authorization and safety

The optional diagnostic requires all of the following:

- developer mode enabled according to `/usr/sbin/DevToolsSecurity -status`;
- the responsible terminal or Codex app enabled in **System Settings → Privacy & Security →
  Developer Tools**;
- a staged target carrying `com.apple.security.get-task-allow=true`.

Full Disk Access is not required. Screen Recording is a separate permission used only by
ScreenCaptureKit full-paint evidence.

The harness copies the release driver into a token-owned private workspace, ad-hoc signs only the
copy, verifies its entitlement, and never modifies the SwiftPM build artifact. It enforces a 2 GiB
recursive trace limit, 256 MiB export limit, and 4 GiB free-space reserve by default. Override
these with `SRUI_XCTRACE_MAX_BYTES`, `SRUI_XCTRACE_MAX_EXPORT_BYTES`, and
`SRUI_XCTRACE_MIN_FREE_BYTES`. A trace is a directory; `stat` reports only its directory
entry, while `du -sk` is an approximate on-disk diagnostic. Failed staging and capture state is
ownership-token checked and removed; repeated failed traces are not retained.

If capture fails, inspect in this order:

1. `xcrun xctrace version` and the exported TOC;
2. advertised Statistics/List view paths;
3. developer-mode and app privacy authorization;
4. the staged `get-task-allow` entitlement;
5. exact PID plus process-birth identity;
6. recursive trace/export/free-space bounds.

Do not infer flags or schemas from a different Xcode release.

## Detailed findings

The full investigation record—including exact conflicting export values, the disproved timestamp
model, the WebKit stall stack, AppKit activation-policy failures, recorder-finalization behavior,
discarded approaches, reproduction commands, cleanup lessons, and Linux/Windows portability
guidance—is in [INSTRUMENTATION_FINDINGS.md](INSTRUMENTATION_FINDINGS.md). Future changes to the
allocation methodology should begin there rather than re-running the same experiments.
