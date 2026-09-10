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
`srui.host_retained_allocations`, `srui.process_footprint_peak`, and the corresponding
`webkit.*` comparison-control metrics. Xctrace contributes separately measured
interval-created-and-still-live allocation counts and bytes. Do not describe those xctrace values
as cumulative allocations: allocations freed before trace finalization are absent.

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

The parent-side park protects candidate launch, while each candidate's authoritative local park
protects the measurement loop from launch-time pointer movement. Both occur before measured work
and restore their own saved Quartz location. The resulting interior point avoids edge-triggered
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

Xctrace is an independent allocation-evidence path, not the source of the paint timestamp. The
harness copies the release driver into a token-owned private workspace, ad-hoc signs only that copy
with `com.apple.security.get-task-allow=true`, launches the candidate, and identifies every
measured host/helper by PID plus Darwin process birth time. It attaches the Allocations template,
waits for xctrace's requested Darwin recording-start notification before releasing the candidate,
records the candidate's published wall-clock interval, stops and finalizes the trace, exports the
TOC-advertised `Statistics` and `Allocations List` details, reconciles them exactly, and then
selects live List rows whose allocation timestamps fall within the measured interval, including
the documented clock-boundary bounds. The report merges these allocation results with the native
driver's ScreenCaptureKit results only after both evidence paths validate; it never infers one from
the other.

## Reference tool and export contract

The allocation parser is pinned to the behavior verified with
`xctrace version 26.0 (17C52)`. Record the exact installed versions when diagnosing a new host:

    xcodebuild -version
    xcrun xctrace version
    xcrun xctrace help export
    xcrun xctrace export --input TRACE --toc

The trace TOC, rather than a guessed raw schema name, is authoritative. In 17C52 the Allocations
track advertises two view-level table details:

    <track name="Allocations">
      <details>
        <detail name="Statistics" kind="table"/>
        <detail name="Allocations List" kind="table"/>
      </details>
    </track>

Export them with these exact XPaths:

    xcrun xctrace export       --input TRACE       --xpath '/trace-toc/run[@number="1"]/tracks/track[@name="Allocations"]/details/detail[@name="Statistics"]'

    xcrun xctrace export       --input TRACE       --xpath '/trace-toc/run[@number="1"]/tracks/track[@name="Allocations"]/details/detail[@name="Allocations List"]'

Do not substitute a generic query such as
`/trace-toc/run/data/table[@schema="allocations"]`. Xctrace exports both raw tables and
view-level aggregates, and the Allocations values needed here live in the latter.

The Statistics export contains `All Heap & Anonymous VM`, `All Heap Allocations`, and
`All Anonymous VM` rows with these fields:

- `persistent-bytes`: bytes still allocated at the end of the trace.
- `count-persistent`: allocations still live at the end of the trace.
- `transient-bytes`: bytes allocated and freed by the end of the trace.
- `count-transient`: allocations freed by the end of the trace.
- `total-bytes`: persistent plus transient bytes over the complete trace.
- `count-total`: persistent plus transient allocations over the complete trace.
- `count-events`: allocation events plus deallocation events.

Each valid row satisfies these aggregation identities:

    persistent-bytes + transient-bytes = total-bytes
    count-persistent + count-transient = count-total

Never use `count-events` as an allocation count or derive it from the other fields. An attached
process can deallocate an object that existed before recording, so its event stream need not contain
a matching allocation event inside the trace.

The Allocations List export in 17C52 is a live-object list. Direct attach and launch probes showed
that every exported row was `live="true"`; freed allocations were omitted. The benchmark proves:

    every field in All Heap & Anonymous VM = All Heap Allocations + All Anonymous VM
    number of all Allocations List rows = combined Statistics count-persistent
    sum of all Allocations List row sizes = combined Statistics persistent-bytes

Full AppKit captures also contain categories beginning exactly `VM:`, but retained 17C52 probes
showed that this prefix does not reliably partition the `All Anonymous VM` and `All Heap
Allocations` aggregates. Those category counts and bytes are preserved only as diagnostics; they
never decide inclusion. This exact combined reconciliation detects a wrong XPath, incomplete
export, or a parser that skipped live rows. It does not turn the List into an all-event log.

## What the benchmark measures

Each logical sample is run in equivalent exact-process passes: SRUI host, WebKit host, WebContent,
and any Network/GPU helpers declared present by the pre-measurement handshake. A target is accepted
only as an exact PID/Darwin-birth identity before and after the measured interval. The recording
starts, xctrace emits the requested Darwin notification, the candidate receives its go signal, the
candidate publishes its interval, and xctrace is then stopped and finalized. PID aliases across
roles are explicit and included once.

The primary xctrace value for a role and sample is:

1. Start with the final Allocations List, which contains only objects still live at trace
   finalization.
2. Read each row's allocation size and trace-relative allocation timestamp.
3. Convert the timestamp to Unix epoch nanoseconds from the trace TOC start date.
4. Include the row only when its allocation timestamp is inside the candidate's reported
   `started_unix_ns ... ended_unix_ns` interval.
5. Count the included rows and sum their sizes.

The result is the number and bytes of heap-and-anonymous-VM allocations **created during the
measured interval and still live when recording ended**. `VM:` category rows remain visible as
non-classifying diagnostics. It is not any of the following:

- cumulative allocation calls;
- total allocation traffic;
- allocations created and freed during the interval;
- a before/after heap-size delta;
- peak resident memory;
- the native driver's allocator live-block delta.

Those are distinct measurements. Statistics `total-bytes` and `count-total` describe the whole
trace. With `--attach`, they also include the heap snapshot already live when attachment
completed. Xctrace 17C52 has no validated range-scoped Statistics export that isolates the inner
candidate interval, so subtracting or relabeling these totals as interval cumulative cost is
invalid. Statistics is used for whole-trace field validation and List reconciliation only.

List timestamps are rendered with 1 µs resolution. The harness anchors them to the TOC start date
and compares them with candidate wall-clock epoch timestamps. It reports
`timestamp_boundary_uncertainty_ns` as the TOC start-date resolution plus 1,000 ns. On the validated
17C52 traces this is a 1.001 ms boundary caveat. `retained_allocations` and `retained_bytes` are
the nominal closed-interval values; `retained_*_lower_bound` counts only rows definitely inside
after removing the uncertainty at both edges; `retained_*_upper_bound` includes all rows possibly
inside after expanding both edges; `boundary_ambiguous_*` is the upper/lower difference. Preserve
and disclose these bounds. Exact PID/birth checks solve process attribution, not clock-boundary
ambiguity.

Because the List is live-only, a workload that allocates and frees everything may legitimately
produce zero interval-created-and-still-live rows even though Statistics reports transient
traffic. Such a result is not evidence that no allocations occurred. If all-event churn is needed
for diagnosis, use malloc stack logging as described below; do not silently change the benchmark's
metric.

## Attachment authorization and staged signing

Three conditions are deliberately kept separate:

1. `/usr/sbin/DevToolsSecurity -status` must say
   `Developer mode is currently enabled.`. An administrator can enable it with
   `sudo /usr/sbin/DevToolsSecurity -enable`.
2. The terminal or Codex host responsible for xctrace must be enabled under **System Settings →
   Privacy & Security → Developer Tools**. Restart that app after granting access.
3. The benchmark target copy must carry
   `com.apple.security.get-task-allow=true` so the Apple-signed performance tool can acquire its
   task port.

Full Disk Access is not required. Do not grant it to work around a Developer Tools or target
entitlement failure.

The release binary under `client-macos/.build` is a SwiftPM artifact and must remain byte-for-byte
untouched. The capture path copies it into the invocation's token-owned temporary workspace,
creates a minimal entitlement plist there, ad-hoc signs only that staged copy, verifies the
signature, and launches the copy. This avoids corrupting SwiftPM freshness/signature state and
avoids racing another build. The entitlement is for local measurement only; never ship that
instrumented copy.

A manual diagnostic that follows the same rule is:

    srui_sign_stage=$(mktemp -d /tmp/srui-sign-check.XXXXXX)
    cp client-macos/.build/release/BenchmarkDriver "$srui_sign_stage/BenchmarkDriver"
    codesign --force --sign - --entitlements benchmarks/parse-render/BenchmarkDriver.entitlements "$srui_sign_stage/BenchmarkDriver"
    codesign --verify --strict --verbose=2 "$srui_sign_stage/BenchmarkDriver"
    codesign --display --entitlements - --xml "$srui_sign_stage/BenchmarkDriver"

The last command must show `com.apple.security.get-task-allow` as true. A valid structural
signature alone does not prove the contextual entitlement or privacy policy will allow attachment.

## Retained traces and size limits

Produce a diagnostic capture only at a destination that does not exist:

    benchmarks/parse-render/profile-allocations.sh /tmp/srui-allocations.trace

On success this publishes the trace directory,
`/tmp/srui-allocations.trace.summary.json`, and the requested driver result. Normal consolidated
runs retain none of these machine-local paths.

A `.trace` is a directory bundle, despite its filename suffix. This command is a trap:

    stat -f %z /tmp/srui-allocations.trace

It reports the top-level directory entry size, often only tens or hundreds of bytes, not the bytes
inside the capture. For a quick allocated-disk estimate use:

    du -sk /tmp/srui-allocations.trace

The benchmark's `trace_bytes` is different and deterministic: it recursively sums the logical
`st_size` of files beneath the trace directory without following symlinks. The same recursive
value enforces `SRUI_XCTRACE_MAX_BYTES`. Defaults are a 2 GiB trace, 256 MiB XML export, and
4 GiB free-space reserve. The consolidated runner separately requires 12 GiB free. Exceeding any
limit aborts capture and removes only the ownership-token-verified staging workspace.

## Failure diagnostics

Use this order; each check answers a different question:

1. Verify `xcrun xctrace version` and save `xcrun xctrace export --input TRACE --toc`. A missing
   Allocations track/detail is an Xcode/template compatibility failure, not an empty workload.
2. Run `/usr/sbin/DevToolsSecurity -status`, then check the responsible app's Developer Tools
   privacy grant. With `--no-prompt`, a missing grant fails closed rather than opening a consent
   dialog.
3. Keep the logged-in desktop unlocked and visible. `scripts/run-benchmarks` wraps the macOS run in
   `caffeinate -d -i -u`, which wakes an online display and prevents idle sleep only for the
   runner's lifetime. It cannot unlock the session. If `CGDisplayIsActive` is false,
   ScreenCaptureKit can legitimately publish zero displays. Full-mode hosts use the dynamically
   resolved status level only after proving `dock < status < popup`; this is compositor isolation,
   not permission to cover secure UI. Any intersecting, nonzero-alpha `loginwindow` or other
   ahead surface still makes the exact z-order proof fail rather than claiming content hidden
   behind the lock screen.
4. Inspect the staged target with
   `codesign -d --entitlements :- STAGED_BENCHMARK_DRIVER`. “Target is not debuggable,” inability
   to acquire the task port, or exit before the recording-start notification points to signing or
   Developer Tools authorization, not to the allocation parser.
5. If the runner reports
   `xctrace did not begin recording within 15s; grant Instruments automation and Developer Tools privacy access`,
   do not accept recorder progress text or a partially created trace as readiness. Fix the grant
   and retry.
6. Export both view-level details and check the arithmetic and List/Statistics reconciliation
   above. A zero List with non-zero `count-persistent`, or a size sum different from
   `persistent-bytes`, is a parser/export failure.
7. Check recursive size and free-space limits. Do not diagnose a large trace with top-level
   `stat`.

Do not infer an option from documentation for another Xcode release. On the verified 17C52 tool:

    xcrun xctrace record --template Allocations --show-recording-options

fails with:

    Command Parser error occured: unrecognized option '--show-recording-options'

No supported 17C52 recording option was found that makes Allocations List export freed rows. Call
Tree is also not advertised by the supplied Allocations TOC. The parser must fail clearly if a
future Xcode changes the view names or fields; it must not fall back to an unrelated raw table.

For a separate all-event investigation, launch the target with
`MallocStackLoggingNoCompact=1` and inspect it with `malloc_history PID -allEvents`.
`malloc_history(1)` defines `-allEvents` as allocation and free events, while
`MallocStackLoggingNoCompact` preserves immediate allocation/free pairs. For a current live heap
snapshot, use `heap PID`. These tools perturb execution and are diagnostics, not substitutes for
the §31.1 report.
## Sources

- [xctrace(1)](https://keith.github.io/xcode-man-pages/xctrace.1.html): recording, `--attach`,
  TOC discovery, XPath export, and view-level aggregated data. Prefer the installed
  `man 1 xctrace` and `xcrun xctrace help ...` when options differ by Xcode release.
- [Apple Developer Forums thread 664347](https://developer.apple.com/forums/thread/664347):
  allocation export support becomes visible through TOC XPath nodes; the later answer gives the
  Allocations List detail path.
- [Apple Developer Forums thread 799351](https://developer.apple.com/forums/thread/799351):
  Allocations List export is available while Call Tree export is not.
- [get-task-allow entitlement](https://developer.apple.com/documentation/bundleresources/entitlements/com.apple.security.get-task-allow):
  Apple's entitlement reference for debugger task access.
- Installed `DevToolsSecurity(8)` and `codesign(1)`: system developer authorization policy,
  ad-hoc signing, entitlement display, and signature verification.
- [malloc_history(1)](https://keith.github.io/xcode-man-pages/malloc_history.1.html) and
  [Apple's malloc debugging guide](https://developer.apple.com/library/archive/documentation/Performance/Conceptual/ManagingMemory/Articles/MallocDebug.html):
  all-event logging and `MallocStackLoggingNoCompact`.
- [heap(1)](https://keith.github.io/xcode-man-pages/heap.1.html): current live heap inspection.
