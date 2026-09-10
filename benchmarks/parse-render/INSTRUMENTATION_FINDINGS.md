# Task 34 macOS measurement and instrumentation findings

This document records the technical conclusions reached while implementing and validating the
Task 34 macOS parse/render benchmark. It is intentionally more detailed than the operational
README. Its audience is future maintainers who need to change a measurement boundary, diagnose a
host-specific failure, or decide whether a profiler output is suitable as benchmark evidence.

The authoritative requirement remains §31.1 of
`SRUI_Semantic_Remote_UI_Design_v0.6.md`: equivalent prebuilt HTML and SRUI
representations are already in memory, the WebKit and SRUI renderers are warm, and the benchmark
measures first visible paint, complete paint, CPU time, allocations, and peak memory. The design
also says that benchmark layers must remain separate. Nothing in this file overrides that design.

## Decisions at a glance

| Quantity | Authoritative source | Scope | Important exclusion |
|---|---|---|---|
| First and complete visible paint, full profile | ScreenCaptureKit complete-frame `displayTime` plus exact WindowServer and pixel evidence | One ordinary foreground candidate | Callback receipt, screenshots taken after the fact, and profiler output |
| First and complete paint, smoke profile | Named offscreen AppKit raster or WKSnapshot fallback | One hidden candidate | Any claim of compositor visibility |
| CPU | Process CPU counters around the representative resource pass | SRUI host; WebKit host plus exact helper PIDs | Wall-clock delay and process-name matching |
| Allocations | Signed `malloc_zone_statistics` endpoint deltas | Candidate host's default malloc zone | Allocation-call traffic, every malloc zone, WebKit helper allocations, and xctrace |
| Peak memory | Periodic physical-footprint sampling in a separate pass | SRUI host; WebKit host plus exact helper PIDs | A sum of per-process lifetime maxima |
| Xcode Allocations data | Optional `xctrace` diagnostic | Exact SRUI host PID and process birth, whole trace/final live view | §31.1 acceptance, committed baseline metrics, WebKit comparison, and workload-interval attribution |

The most important outcome is that xctrace is not the authoritative allocation measurement.
The normal benchmark is complete without Developer Tools access. Xctrace remains useful for
investigation, but its output is explicitly marked `diagnostic_only=true` and
`authoritative_benchmark_metric=false`.

## 1. Authoritative §31.1 workload boundaries

### 1.1 Equivalent representations and warm state

The representative fixture is
`benchmarks/fixtures/coding-agent-ui.json`. Its declared
`first_paint_node_count` divides one semantic UI into two canonical transactions:

1. revision 0→1 constructs a useful first state containing the Surface, Column, Row, Text, and
   Progress subtree;
2. revision 1→2 constructs the remaining complete state.

Rust and Swift identify the same deterministic
`[u64 big-endian length][protobuf]` transaction pair by byte count and SHA-256.
The WKWebView control receives structurally equivalent prebuilt first and complete HTML/DOM
representations with exact sentinels. WKWebView is a warmed comparison control, not an SRUI
renderer.

The following work is outside every timed sample:

- fixture parsing and abstract-state construction;
- server-side generation and serialization, which belong to §31.2;
- compiling or launching the benchmark executable;
- initial renderer/WebView construction and warm-up;
- resetting a candidate to its empty hidden state;
- ScreenCaptureKit stream preparation and acquisition of its pre-action baseline;
- pointer parking and WindowServer geometry/z-order settling;
- report encoding and validation.

This separation prevents build, process-startup, serialization, capture setup, or verifier work
from being relabeled as renderer cost.

### 1.2 Four disjoint passes

Each logical sample uses separate workloads so one instrument does not contaminate another:

1. **First-state presentation pass.** Decode and apply revision 0→1 through the production native
   initial-attach path, or load the equivalent first WebKit state, then observe the first accepted
   presentation.
2. **Complete-state presentation pass.** Decode/apply 0→1 and incrementally apply 1→2, or load the
   two equivalent WebKit states, then observe a distinct accepted complete presentation.
3. **Resource pass.** Execute the same representative production decode/load, apply, hidden
   layout/draw, and display-submission work while measuring CPU, signed host allocator endpoints,
   and physical-footprint growth. No ScreenCaptureKit stream or periodic footprint sampler is
   active in this pass.
4. **Peak-footprint pass.** Execute the representative workload while sampling the relevant
   process footprint at 1 ms cadence. CPU/allocation figures do not come from this pass.

Native samples use separately warmed and reset renderer instances. WebKit resets its warmed view
between passes and checks that the exact helper-process topology used for CPU and footprint
accounting remains valid.

The resource pass submits real renderer work even though it is hidden. It is not a synthetic
model-only loop. Conversely, the hidden resource pass does not claim on-screen presentation.

### 1.3 Production paths exercised

The native first state enters through the production initial `attach(store:)` route. The
completion state enters through the production incremental
`apply(transaction:newStore:)` route. The same measured instance is inspected after accepted
presentation for exact semantic store, native-control, or DOM state. A successful visual signal
without matching semantic/control state is not a valid sample.

## 2. Full-profile presentation evidence

### 2.1 Why ScreenCaptureKit is needed

An AppKit method returning, a layer transaction being committed, or a draw callback firing does not
prove that the user could see the result. Full mode therefore observes a composited frame. The
measurement endpoint comes from `SCStreamFrameInfo.displayTime` on a complete
ScreenCaptureKit frame that independently passes target identity, geometry, z-order, and pixel
checks.

ScreenCaptureKit is not used in the resource or peak-footprint passes. That avoids charging pixel
conversion, hashing, or stream callbacks to CPU/allocation measurements.

Smoke mode deliberately uses offscreen AppKit bitmap rendering and WKSnapshot. Those are useful
repeatable draw-completion checks, but their metric names say “offscreen” and they make no visible
paint, WindowServer, or display-time claim.

### 2.2 Presentation timeline

The full-profile sequence is:

1. Resolve the target display and prepare the candidate while hidden.
2. Start the ScreenCaptureKit display stream.
3. Accept one complete pre-action baseline frame.
4. Record the action-start Mach timestamp.
5. Run the first-state or two-state production workload while the host remains hidden.
6. Immediately before the sole order-front/display submission, arm a display-time cutoff.
7. Ignore incomplete frames and every frame whose `displayTime` does not cross that cutoff.
8. For each later candidate frame, resolve and validate the exact target/window geometry and
   z-order, crop the target content from that same frame, and apply the pixel predicates.
9. Requery identity, geometry, and z-order after pixel comparison to close the verification race.
10. Accept the frame only if all pre- and post-checks agree.
11. Report action-start through the accepted frame's `displayTime`.

A refresh that appears before WindowServer has published the exact host identity is rejected; the
stream stays armed for a later eligible frame. A stale partially rendered WebKit state is likewise
ineligible for the complete metric.

Callback-receipt time is recorded only as diagnostic metadata. Image conversion, normalization,
pixel comparison, and hash calculation occur after ScreenCaptureKit supplied the display timestamp
and do not extend the reported visible-latency endpoint.

### 2.3 Exact WindowServer identity and geometry

The host does not trust a title or process name. It matches the AppKit window number in the
WindowServer inventory returned by
`CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID)`
and requires exactly one entry with:

- the expected window ID;
- owner PID equal to the candidate process;
- layer equal to the AppKit window's resolved level;
- finite positive bounds;
- whole-window alpha exactly 1;
- bounds wholly contained by the selected display;
- the expected client-content and target-view geometry.

Membership in the `.optionOnScreenOnly` result is the on-screen proof.
`kCGWindowIsOnscreen` is redundant metadata and can be absent even for entries returned by that
filtered query. Its absence is not converted into a false negative or an invented value.

### 2.4 Window levels and z-order

Numeric level constants are not assumed. Each run resolves:

- `CGWindowLevelForKey(.dockWindow)`;
- `CGWindowLevelForKey(.statusWindow)`;
- `CGWindowLevelForKey(.popUpMenuWindow)`;
- `CGWindowLevelForKey(.screenSaverWindow)`.

Measurement refuses to start unless
`dock < status < popup < screen-saver`. The benchmark host uses the resolved status stratum.
This places it above the Dock-owned desktop surface that otherwise appears ahead in the global
window list, while keeping it below real pop-up menus.

The WindowServer list is treated as front-to-back. Every entry before the exact target is parsed
fail-closed. If an ahead entry omits required identity, layer, alpha, or geometry, the sample is
rejected. If an ahead entry has nonzero whole-window alpha and nonempty bounds intersection with
the target, the sample is rejected.

There are no owner-name, Dock, cursor, window-number, or transparent-pixel whitelists.
`kCGWindowAlpha` describes the whole window, not the alpha of pixels over the target. A
full-display surface can report alpha 1 while its target-area pixels look transparent; interpreting
that visual appearance as safe would weaken the z-order proof. A `loginwindow` shield or any
other intersecting surface ahead therefore causes a hard failure.

The window-isolation self-test exercises both sides with real overlapping windows: a Dock-level
window must remain below the status host, while a window at `status + 1` must be detected as
ahead. This verifies the runtime level relationship rather than relying on a comment about it.

### 2.5 Pointer and cursor behavior

Moving or merely placing the measurement window can trigger an auto-hidden Dock, menu bar, hot
corner, hover state, or cursor overlay. Full parse/render mode uses two nested guards:

- the parent saves the user's Quartz pointer position and parks it 160 points inside the selected
  display's left edge at vertical midpoint before spawning either ordinary visual candidate;
- each candidate independently saves the inherited pointer position and repeats that park
  immediately before entering its measurement function.

The child-side guard closes the build/spawn interval during which the user or system can move the
pointer. Both guards restore the exact saved location during normal return and failure unwinding.

The 960×720 renderer ROI is placed in a visible-frame corner away from the parked pointer, with a
64-point clearance requirement while hidden. Parking stays away from display edges to avoid Dock,
menu-bar, and hot-corner activation. The cursor is not removed from WindowServer evidence. If a
cursor or another nonzero-alpha surface intersects the target ahead, the sample remains invalid.

A locked or inactive login session can make the necessary display/window evidence unavailable.
`caffeinate` can keep an already available display awake for the runner's lifetime; it cannot
unlock a session, grant Screen Recording, or override secure UI.

### 2.6 Pixel acceptance

A frame must be complete and its exact target-view crop must be:

- visible at the expected geometry;
- unobscured under the z-order rules above;
- nonblank;
- nonuniform;
- materially different from the same-stream pre-action baseline.

“Materially different” requires at least eight normalized pixels for which any RGBA channel differs
by more than 2/255. First and complete accepted crops must use equal normalization and geometry,
and the complete crop must also differ from the accepted first crop by that same predicate.
WebKit performs the first-to-complete comparison while selecting the complete frame rather than
accepting a stale surface and checking later.

SHA-256 image fingerprints are retained as diagnostics. Hash inequality alone is not evidence of a
material UI change: a one-pixel capture artifact could change a hash.

### 2.7 Precise presentation claims

A passing full metric supports this claim:

> The production workload beginning at the recorded action timestamp reached a later complete
> ScreenCaptureKit frame whose display timestamp crossed the submission cutoff, whose exact
> WindowServer target remained stable and unobscured, and whose target pixels satisfied the
> baseline and state-transition predicates.

It does not prove:

- the frame was the first internal Core Animation commit;
- every pixel outside the validated ROI was correct;
- callback delivery itself was low latency;
- the same result would occur with a locked session or different display topology;
- a smoke-profile offscreen result was visible.

## 3. Authoritative allocation evidence

### 3.1 Endpoint counters

The normal §31.1 allocation figures come from the public malloc-zone statistics mechanism used by
the benchmark host. Immediately before and after the disjoint resource pass, the candidate calls
`malloc_zone_statistics(malloc_default_zone(), &statistics)` and reads:

- `blocks_in_use`;
- `size_in_use`.

For each sample, the stored values are signed:

```text
net_live_blocks = after.blocks_in_use - before.blocks_in_use
net_live_bytes  = after.size_in_use   - before.size_in_use
```

The report exposes the distributions as:

- `srui.host_net_live_allocation_blocks`;
- `srui.host_net_live_allocation_bytes`;
- `webkit.host_net_live_allocation_blocks`;
- `webkit.host_net_live_allocation_bytes`.

The values remain integral. A negative delta is valid evidence that the endpoint contained fewer
live default-zone blocks or bytes after the workload. Clamping it to zero would falsify the
measurement. Non-finite and fractional values are invalid.

### 3.2 What the endpoint delta means

The delta measures the net change in currently live allocations in the host's default malloc zone
between two endpoints around the representative production resource pass.

It is not:

- the number of allocator calls;
- the number or bytes of allocations created during the pass;
- total allocation/free traffic;
- the number of objects retained specifically by the renderer;
- a stack-attributed ownership count;
- an all-zone process total;
- peak memory;
- resident-set or physical-footprint growth;
- a leak detector.

An allocate/free pair wholly inside the interval contributes zero at the endpoints. An allocation
created before the interval and freed during it can make the delta negative. Unrelated host work on
the same default zone can contribute noise. Repetition and percentile reporting characterize that
noise; terminology must not erase it.

### 3.3 Host and helper scope

SRUI's production AppKit renderer executes in the measured host, so the SRUI host delta covers the
renderer process, subject to the default-zone limitation.

WKWebView is multiprocess. Its authoritative endpoint value covers only the benchmark host's
default zone. It does not include WebContent, Networking, GPU, or other helpers. The WebKit metric
is therefore an explicitly host-only comparison control, not total WebKit allocation cost.

CPU, footprint growth, and peak footprint have a different scope: WebKit aggregates the host and
exact helper PIDs. The report's scope assertion and metric names must preserve this difference.
Never combine the WebKit host malloc delta with host-plus-helper footprint and label the result as
one uniform process scope.

### 3.4 Why this source is authoritative

The endpoint source is modest but defensible:

- both reads occur inside the measured candidate process;
- both use the same counter source and clock-independent subtraction;
- the boundary surrounds the production resource workload;
- no external attach is needed;
- the profiler cannot suspend or enumerate a second allocator process;
- the result has explicit signed, host-only, default-zone semantics.

The source does not become more accurate by giving it a broader name. Its value comes from a
narrow, repeatable, correctly described boundary.

## 4. Xcode 26 Allocations investigation

### 4.1 Why the investigation was performed

Task 34 originally attempted to use xctrace as mandatory allocation evidence and to derive
allocation totals for the inner benchmark interval. That required all of the following assumptions:

1. the CLI could export the relevant allocation records;
2. exported List timestamps shared a proven origin with the trace TOC and Swift workload window;
3. the final Allocations List and persistent Statistics represented one identical snapshot;
4. attaching the profiler did not materially perturb either candidate;
5. separate WebKit host/helper captures formed a meaningful multiprocess comparison.

Real captures disproved assumptions 2 through 5. The supported export surface was also narrower
than an all-event table. The implementation was therefore reduced to an optional SRUI-host
whole-trace diagnostic.

### 4.2 View-detail discovery

The tested tool identified itself as `xctrace version 26.0 (17C52)`. Its own trace table of
contents, obtained with:

```sh
xcrun xctrace export --input TRACE --toc
```

advertised the Allocations track's table details at:

```text
/trace-toc/run[@number="1"]/tracks/track[@name="Allocations"]/details/detail[@name="Statistics"]
/trace-toc/run[@number="1"]/tracks/track[@name="Allocations"]/details/detail[@name="Allocations List"]
```

Those details can be exported with `xcrun xctrace export --xpath`. The generic query
`/trace-toc/run/data/table[@schema="allocations"]` is not equivalent. Future Xcode versions must
be inspected through their own TOC; view names and fields are not assumed stable.

Statistics exposes rows named `All Heap & Anonymous VM`, `All Heap Allocations`, and
`All Anonymous VM`. The parser validates, within each row:

```text
persistent-bytes + transient-bytes = total-bytes
count-persistent + count-transient = count-total
```

It also validates that the combined Heap & Anonymous VM row equals Heap plus Anonymous VM for all
exported fields. `count-events` is independent allocation/deallocation event data. It is not
treated as an allocation count.

On the tested tool, every accepted Allocations List row was marked live. The export therefore
behaves as a final-live-object view, not an all-event history. Rows require valid identifier,
category, size, and displayed elapsed timestamp. Categories beginning `VM:` are retained only
as diagnostics; that string prefix is not used as a proven Heap/Anonymous VM partition.

### 4.3 Empirical timestamp mismatch

One finalized real trace reported:

```text
TOC duration:                         1.664726 s
maximum Allocations List timestamp:   2.347137 s
```

The List's displayed elapsed time exceeded the TOC duration by 0.682411 seconds. Therefore it was
not valid to assert that the displayed List timestamp shared the TOC duration/start-date basis.

The former reconstruction:

```text
allocation_wall_time = toc_start_date + list_elapsed_time
```

was removed. The optional diagnostic retains the Swift workload start/end timestamps only to
describe what workload ran. It explicitly emits
`workload_window.used_for_allocation_attribution=false`. It does not filter List rows into an
inner benchmark interval, publish interval-created totals, or assign clock-boundary uncertainty
bounds.

This observation is empirical for the captured Xcode 26 trace. The two numeric values are not
constants to encode into validation. A future exporter can be used for interval attribution only
after its time origin is independently demonstrated on real traces.

### 4.4 Empirical List/Statistics mismatch

The same capture produced:

```text
Allocations List:                           49,014 rows   9,292,560 bytes
Statistics persistent Heap + Anonymous VM: 49,017        9,296,080 bytes
Statistics minus List:                          3            3,520 bytes
```

Other captures produced different discrepancies, including equal counts with unequal byte totals.
Thus the two view details are independently materialized Instruments views, not guaranteed
encodings of one exact snapshot.

The parser now:

- validates Statistics arithmetic internally;
- validates List rows and List totals internally;
- reports the signed persistent Statistics-minus-List count and byte differences;
- does not require equality;
- does not use either source as an exact inner-workload allocation total.

The mismatch does not prove which view is “wrong,” nor does it prove a fixed race interval. It only
disproves the equality required by the earlier attribution method.

### 4.5 WebKit attachment perturbation

Attaching the Allocations instrument to a warmed WKWebView host stalled for at least 60 seconds.
A process sample taken during the stall showed the injected profiler thread in
`liboainject` at `_OAAttachAndInitialize`, blocked while JavaScriptCore's libpas allocator
was enumerating roots at
`pas_root_enumerate_for_libmalloc_with_root_after_zone`. The application main thread was idle.

That stack is a sample of where the instrumented process was stopped; it is not a proof of an Apple
or JavaScriptCore root-cause bug. It is sufficient evidence of material observer interference:
the instrumented workload did not make bounded progress and cannot be reported as a valid WebKit
sample.

Separate host, WebContent, Networking, and GPU attachments would also be sequential views, not one
simultaneous coherent multiprocess snapshot. The optional diagnostic therefore rejects a WebKit
capture and targets only the SRUI host.

### 4.6 AppKit activation-policy experiments

Early auxiliary candidates attempted `NSApplication.setActivationPolicy`. Raw value 0
(regular) and raw value 1 (accessory) both failed through LaunchServices policy modification in
the instrumented context. Switching between those values was not a reliable fix.

The diagnostic candidate now leaves AppKit's process-selected activation policy unchanged. It
does not call `activate` and does not order a foreground window. It performs the hidden
production layout/draw path and `CATransaction.flush()`. Ordinary full visual candidates remain
regular foreground applications and are the only candidates used for ScreenCaptureKit
presentation evidence.

Initializing `NSApplication.shared` can still emit a LaunchServices assertion. That log alone
does not determine failure. The exact subsequent render/control handshake, process identity, and
exit status determine whether the diagnostic candidate succeeded.

### 4.7 Resulting xctrace contract

The optional capture is intentionally narrow:

- one exact SRUI host process per sample;
- the Allocations template attached by PID;
- PID corroborated by Darwin process birth and liveness bounds;
- recording readiness proved by the requested Darwin notification;
- equivalent hidden resource workload;
- whole-trace Statistics, including the attach-time live baseline;
- final live Allocations List totals;
- signed Statistics-minus-List discrepancy;
- workload timestamps retained as descriptive metadata only;
- no WebKit capture;
- no committed §31.1 metric;
- no baseline gate.

The summary uses schema version 4 and records:

- `diagnostic_only=true`;
- `authoritative_benchmark_metric=false`;
- `diagnostic_target="srui_host"`;
- `capture_scope="exact_process_diagnostic"`;
- `workload_window.used_for_allocation_attribution=false`.

Any future code that changes one of those fields is changing the evidence claim, not merely
renaming output.

## 5. Exact process attribution

A numeric PID alone is not stable evidence because the operating system can reuse it. The
diagnostic protocol therefore binds:

1. the candidate child PID created under managed supervision;
2. its Darwin process-birth timestamp;
3. the target PID published by the candidate handshake;
4. the PID reported as attached in the trace TOC;
5. an observed-alive-through timestamp that spans the workload;
6. post-run proof that the exact PID/birth identity is gone.

A process name is retained only for diagnostics. It is never used to select or attribute a target.
A same-named unrelated process and a reused numeric PID must both be rejected.

The WindowServer proof uses a separate exact identity chain: AppKit window number plus current
owner PID, layer, bounds, display, and z-order. Process attribution cannot substitute for window
identity, and window identity cannot substitute for allocator-process attribution.

## 6. Staged signing and authorization

### 6.1 Why a private copy is signed

Exact-process attachment requires the target to permit task inspection. The harness must not
modify `client-macos/.build/release/BenchmarkDriver` in place because that is a SwiftPM build
artifact. Re-signing it can corrupt freshness/signature assumptions and race concurrent builds.

The diagnostic creates a token-owned private workspace, copies the release binary, and ad-hoc signs
only that copy with exactly:

```text
com.apple.security.get-task-allow = true
```

It then verifies both the signature and the observed entitlement. The staged binary is local
instrumentation and must never be distributed.

### 6.2 Distinct permission systems

Developer Tools and Screen Recording solve different problems:

- **Screen Recording** authorizes ScreenCaptureKit to obtain pixels for full visible-paint
  measurements. The benchmark checks access and fails without opening a permission prompt.
- **System developer mode** is reported by
  `/usr/sbin/DevToolsSecurity -status` and may be enabled administratively with
  `sudo /usr/sbin/DevToolsSecurity -enable`.
- **Per-application Developer Tools privacy access** must be granted to the terminal or Codex app
  responsible for xctrace. Changing this grant may require restarting that app.
- **The target entitlement** permits task attachment to the staged benchmark copy.

Developer mode alone does not imply the per-application privacy grant, and neither grants Screen
Recording. Screen Recording does not authorize task inspection. Full Disk Access is not required
for either measurement and should not be granted as a workaround.

The xctrace command uses `--no-prompt`; a missing grant should fail closed rather than leaving an
unattended benchmark behind a consent dialog.

## 7. Recorder lifecycle and quiescence

### 7.1 Readiness

A created trace directory or recorder progress text is not readiness evidence. Before releasing
the candidate workload, the harness:

1. starts a one-shot `notifyutil` watcher for a unique notification name;
2. starts xctrace with `--notify-tracing-started <notification>`;
3. waits up to 15 seconds for that Darwin notification;
4. verifies that the exact PID/birth identity still matches;
5. creates the candidate's exclusive `go` control signal.

The xctrace segment itself has a 60-second bound. Control-file waits are bounded at 60 seconds, and
candidate shutdown is independently bounded.

### 7.2 Synchronous post-workload acknowledgement

After the production resource work ends, the candidate publishes a `done` payload containing the
workload timestamps and liveness evidence. It then waits for a `captured` acknowledgement using a
synchronous `access(2)` loop with `usleep(3)`.

This is deliberate. An asynchronous `Task.sleep` loop wakes Swift concurrency machinery and can
continue allocating while xctrace stops and materializes Statistics and List views. The synchronous
post-workload loop reduces that avoidable heap-tail activity.

The sequence is:

1. candidate finishes the workload and publishes `done`;
2. controller sends SIGINT to xctrace;
3. xctrace finalizes the trace;
4. controller exports and validates both view details;
5. controller publishes `captured`;
6. candidate exits the synchronous wait.

This quiescence reduces observer noise. It does not make Statistics and List simultaneous, repair
their observed mismatch, or turn their whole-trace/final-live data into workload-interval data.

## 8. Disk, artifact, and cleanup safeguards

A `.trace` is a directory bundle. On macOS:

```sh
stat -f %z /tmp/srui-allocations.trace
```

reports the directory entry size, not the total size of its contents.
`du -sk` is useful for an approximate allocated-disk check. The harness's authoritative safety
counter recursively sums logical file sizes without following symlinks.

Default diagnostic limits are:

- `SRUI_XCTRACE_MAX_BYTES=2147483648` — 2 GiB recursive trace limit;
- `SRUI_XCTRACE_MAX_EXPORT_BYTES=268435456` — 256 MiB per XML export;
- `SRUI_XCTRACE_MIN_FREE_BYTES=4294967296` — 4 GiB free-space reserve.

The consolidated benchmark runner has its own larger free-space reserve, currently 12 GiB by
default. The limits answer different questions and should not be conflated.

The capture workspace contains an ownership sentinel. Destructive cleanup verifies that token and
refuses to remove a path if ownership changed. Candidate, recorder, notification watcher,
supervisor, and process-group lifecycles are bounded. Termination paths attempt cleanup while
preserving the original and cleanup failures. Publication of a retained standalone diagnostic is
exclusive and rollback-protected.

Normal benchmark runs do not create xctrace data. The standalone diagnostic publishes only the
trace explicitly requested by the operator, its summary, and its driver result. Temporary XML
exports and failed staging workspaces are removed. Retaining every failed trace is intentionally
avoided: redundant full captures exhausted local disk during Task 34.

## 9. Reproduction and troubleshooting

Run commands from the repository root unless noted otherwise.

### 9.1 Build the release benchmark driver

```sh
swift build --disable-automatic-resolution \
  --package-path client-macos \
  -c release \
  --product BenchmarkDriver
```

`--disable-automatic-resolution` ensures the measurement uses the committed SwiftPM resolution
instead of mutating dependency state.

### 9.2 Run focused §31.1 smoke evidence

```sh
client-macos/.build/release/BenchmarkDriver \
  --fixture benchmarks/fixtures/coding-agent-ui.json \
  --profile smoke \
  --only-section 31.1 \
  --output /tmp/srui-31.1.json
```

This validates the offscreen fallback and authoritative resource metrics. It does not claim visible
paint.

### 9.3 Run the normal benchmark suite

```sh
scripts/run-benchmarks --profile smoke
scripts/run-benchmarks --profile full
```

Full mode needs an active unlocked display and Screen Recording permission for the responsible
app. Neither command needs Developer Tools permission because xctrace is not part of the normal
suite.

### 9.4 Produce one optional retained xctrace diagnostic

Choose a destination that does not exist:

```sh
benchmarks/parse-render/profile-allocations.sh \
  /tmp/srui-allocations.trace \
  /tmp/srui-render-profile.json
```

The outputs are:

```text
/tmp/srui-allocations.trace
/tmp/srui-allocations.trace.summary.json
/tmp/srui-render-profile.json
```

Before running, verify system developer mode:

```sh
/usr/sbin/DevToolsSecurity -status
```

Also grant Developer Tools privacy access to the responsible terminal or Codex application.
Do not grant Full Disk Access as a substitute.

### 9.5 Inspect the tool and trace TOC

```sh
xcodebuild -version
xcrun xctrace version
xcrun xctrace help export
xcrun xctrace export --input /tmp/srui-allocations.trace --toc
```

Verify the installed trace actually advertises both Allocations view details before trying to
export them.

### 9.6 Export the two supported view details

```sh
xcrun xctrace export \
  --input /tmp/srui-allocations.trace \
  --output /tmp/srui-statistics.xml \
  --xpath '/trace-toc/run[@number="1"]/tracks/track[@name="Allocations"]/details/detail[@name="Statistics"]'

xcrun xctrace export \
  --input /tmp/srui-allocations.trace \
  --output /tmp/srui-list.xml \
  --xpath '/trace-toc/run[@number="1"]/tracks/track[@name="Allocations"]/details/detail[@name="Allocations List"]'
```

Do not replace these paths with the generic raw-table query and assume equivalent semantics.

### 9.7 Verify staged signing manually

Never sign the SwiftPM output in place. Use a disposable copy:

```sh
srui_sign_stage=$(mktemp -d /tmp/srui-sign-check.XXXXXX)
cp client-macos/.build/release/BenchmarkDriver "$srui_sign_stage/BenchmarkDriver"
codesign --force --sign - \
  --entitlements benchmarks/parse-render/BenchmarkDriver.entitlements \
  "$srui_sign_stage/BenchmarkDriver"
codesign --verify --strict --verbose=2 "$srui_sign_stage/BenchmarkDriver"
codesign --display --entitlements - --xml "$srui_sign_stage/BenchmarkDriver"
```

The displayed entitlement must contain only the expected task-allow grant. Remove the disposable
directory after inspection.

### 9.8 Sample a stalled process

When a diagnostic candidate stops making progress, first identify the exact supervised PID rather
than searching by process name, then capture a short stack sample:

```sh
sample PID 5 1
```

A stack sample is a point observation. Preserve the process identity, timestamp, xctrace version,
and candidate/control state alongside it. Do not promote one stack frame into a causal root-cause
claim.

### 9.9 Check disk use correctly

```sh
du -sk /tmp/srui-allocations.trace
df -h /tmp
```

The harness enforces recursive logical size and free-byte limits itself. These commands are
operator diagnostics, not replacements for the built-in checks.

### 9.10 Run focused validation tests

```sh
uv run --frozen pytest benchmarks/tests/test_xctrace.py
uv run --frozen pytest benchmarks/tests/test_report.py
```

The xctrace tests verify TOC/detail parsing, internal Statistics arithmetic, live-list parsing,
signed discrepancy handling, exact SRUI-host scope, explicit rejection of interval attribution,
staged signing behavior, lifecycle bounds, and cleanup.

### 9.11 Symptom guide

| Symptom | Likely contract involved | Next check |
|---|---|---|
| No displays or frames in full mode | Screen Recording, active display, or locked session | Check the responsible app's Screen Recording grant and unlock the session |
| Window never becomes eligible | Exact WindowServer identity, level, geometry, or z-order | Inspect the diagnostic entry list and ahead-surface intersection |
| A Dock or menu appears during preparation | Pointer-edge activation or unsettled WindowServer state | Confirm pointer park, wait for bounded settling, and do not whitelist the surface |
| xctrace never emits readiness | Developer Tools privacy, task attachment, or recorder startup | Check `DevToolsSecurity`, per-app Developer Tools access, staged entitlement, and recorder tail |
| Candidate exits before `go` | Failed render/control setup or identity publication | Read the supervised candidate stderr and control directory |
| WebKit freezes under Allocations attach | Known material observer interference | Do not use it as evidence; xctrace diagnostic scope is SRUI host only |
| List timestamps exceed TOC duration | Unproven timestamp basis | Do not convert to wall clock or slice by workload interval |
| Statistics and List differ | Independently materialized views | Validate each internally and retain the signed discrepancy |
| Trace appears tiny under `stat` | A `.trace` is a directory | Use recursive harness accounting or `du -sk` |
| Disk fills after experiments | Retained trace bundles or redundant builds | Remove only explicitly identified artifacts and rerun one bounded diagnostic |
| LaunchServices logs an assertion | AppKit policy initialization | Judge success by the exact render/control handshake and exit status, not the log alone |

## 10. Discarded approaches

The following approaches were tested or evaluated and must not be silently reintroduced.

### 10.1 Mandatory xctrace in the benchmark suite

Discarded because the available exports do not support the required interval attribution and
because WebKit attachment materially stalled. Developer Tools access is no longer a prerequisite
for normal benchmark or baseline generation.

### 10.2 TOC-start plus List-elapsed timestamp reconstruction

Discarded after the 1.664726 s TOC duration and 2.347137 s List timestamp observation. There is no
validated shared time origin for the tested export.

### 10.3 Exact List/Statistics equality

Discarded after the 49,014/9,292,560 versus 49,017/9,296,080 observation and later differing
discrepancies. Both views remain useful independently.

### 10.4 Treating Statistics totals as the workload interval

Discarded because Statistics describes the whole trace and an attached process includes the
attach-time live heap baseline. Whole-trace `total-*`, `count-total`, and `count-events`
cannot be relabeled as inner-workload allocations.

### 10.5 Treating the final live List as all allocation events

Discarded because the tested List contains live rows, not objects freed before finalization. It
cannot measure cumulative churn.

### 10.6 Multi-pass WebKit allocation totals

Discarded because host/helper attachments are sequential rather than simultaneous and the warmed
host stalled under attach. The authoritative WebKit endpoint delta is explicitly host-only.

### 10.7 Activation-policy switching in the diagnostic candidate

Discarded after raw values 0 and 1 both failed through policy modification. The hidden diagnostic
leaves the selected policy untouched.

### 10.8 Async waiting after the workload

Discarded because Swift concurrency wakeups allocate while Instruments finalizes. The
post-workload acknowledgement wait is synchronous. This reduces noise but does not repair
independent-view semantics.

### 10.9 Process-name or PID-only attribution

Discarded because names collide and PIDs are reused. Exact PID plus birth identity and liveness are
required.

### 10.10 Signing the build product in place

Discarded because it mutates a shared SwiftPM artifact. Only a private copy receives
`get-task-allow`.

### 10.11 Callback receipt as visible latency

Discarded because delivery and verification lag the display event. The accepted complete frame's
ScreenCaptureKit `displayTime` is the endpoint.

### 10.12 Hash inequality as visual proof

Discarded because hashes amplify immaterial changes. The benchmark uses explicit normalized pixel
counts and channel thresholds; hashes remain diagnostics.

### 10.13 Owner or Dock whitelists

Discarded because owner identity and whole-window alpha do not prove target pixels are unobscured.
Z-order is conservative and fail-closed.

### 10.14 Top-level trace file size

Discarded because `.trace` is a directory bundle. Safety accounting is recursive and does not
follow symlinks.

## 11. Precise non-claims

To keep reports interpretable, maintainers must preserve these non-claims:

- SRUI malloc endpoint deltas are not cumulative allocation traffic.
- A negative endpoint delta is not “zero allocations.”
- WKWebView host malloc deltas do not include helper processes.
- WebKit CPU/footprint helper aggregation does not broaden its malloc metric.
- xctrace Statistics and List are not exact copies of one snapshot.
- xctrace List timestamps are not proven to share the TOC or Swift clock origin.
- xctrace workload timestamps are descriptive and are not row filters.
- xctrace does not provide a WebKit allocation comparison.
- xctrace is not a baseline gate or §31.1 target metric.
- The sampled WebKit stack shows observed interference, not a proven vendor defect.
- `NSApplication.shared` logging is not by itself candidate failure.
- ScreenCaptureKit callback time is not presentation time.
- An offscreen smoke raster is not visible paint.
- A WindowServer entry's alpha is not per-pixel opacity.
- A passing target crop says nothing about pixels outside that crop.
- `caffeinate` does not grant permission or unlock secure UI.
- Screen Recording does not grant task inspection, and Developer Tools does not grant pixels.
- §23's macOS targets are not automatic Linux or Windows acceptance thresholds.

## 12. Linux and Windows translation

The findings separate portable measurement invariants from Apple-specific mechanisms.

### 12.1 Invariants to preserve

A future renderer benchmark on any platform should retain:

1. the same abstract fixture and equivalent prebuilt representations;
2. a warm renderer and exclusion of build/process-start/serialization work;
3. independent first, complete, resource, and peak passes;
4. an action-to-actually-presented boundary rather than callback completion;
5. exact target surface identity, geometry, visibility, and occlusion evidence;
6. semantic/control state verification on the measured renderer instance;
7. signed endpoint allocation metrics described separately from event traffic;
8. explicit host versus helper-process scope;
9. stable process identity stronger than name or PID alone;
10. disclosure and testing of observer interference;
11. bounded capture artifacts, disk reserves, and cleanup;
12. separately justified native-platform performance targets.

The wire, serialization, mutation, reconnect, terminal, and result-cache portions of Task 34 are
largely portable. The renderer presentation and memory adapters are not.

### 12.2 Linux direction

A Linux implementation needs a renderer-specific frame-clock and compositor-visible evidence path.
Depending on the chosen desktop stack, GTK or Qt presentation hooks and a compositor/portal capture
path such as PipeWire may be candidates. Heaptrack- or perf-class tooling may be useful as optional
diagnostics.

Those names are directions, not verified substitutions. Before use, a Linux adapter must
demonstrate:

- which timestamp represents compositor presentation;
- how the exact surface and crop are identified;
- how occlusion is proven under Wayland or X11;
- how process birth/reuse is handled;
- whether allocator counters cover the renderer's allocator and zones;
- whether profiling changes workload progress;
- what permissions and capture prompts occur.

Do not translate `SCStreamFrameInfo.displayTime`, WindowServer level keys, or
`malloc_zone_statistics` literally. Translate their evidence roles.

### 12.3 Windows direction

A Windows implementation likewise needs an actual presentation boundary and exact surface
identity. DWM/ETW, Windows Graphics Capture, WinUI/WPF rendering hooks, and Windows heap/process
instrumentation are possible areas to evaluate.

Before accepting any of them, prove:

- presentation timestamp semantics and clock relation;
- exact HWND/surface ownership and process creation identity;
- geometry, scaling, monitor, and occlusion behavior;
- pixel acquisition permissions and observer cost;
- host/helper allocation scope for multiprocess controls such as WebView2;
- whether heap event streams and endpoint counters describe the same or different quantities.

Again, these are investigation directions, not claims that a particular API already satisfies the
SRUI contract.

### 12.4 Platform targets

The numeric thresholds in design §23 are macOS renderer goals. Linux and Windows should reuse the
methodological separation and report shape, but must establish platform-specific targets from
their native renderer, compositor, hardware, and interaction expectations. Silently applying the
macOS numbers would create false cross-platform conformance.

## 13. Checklist for future instrumentation changes

Before promoting a new measurement source to authoritative evidence, answer all of these with real
captures and regression tests:

- Does the source measure the exact §31.1 workload boundary?
- Are warm-up, setup, serialization, verification, and cleanup excluded?
- Is the clock origin documented and empirically related to the action clock?
- Is the endpoint presentation rather than callback delivery?
- Is the target surface/window identity exact and stable before and after verification?
- Are geometry, display, z-order, and pixel predicates fail-closed?
- Is process identity stronger than PID or name alone?
- Are host and helper scopes explicit?
- Is the value an endpoint delta, live snapshot, retained set, or event stream?
- Can it be negative, and if so is the sign preserved?
- Has observer interference been measured on SRUI and the comparison control?
- Are two exported views proven simultaneous before equality is required?
- Are permissions distinct and documented?
- Are artifacts and processes bounded and cleaned after interruption?
- Does the report state what the metric cannot prove?
- Does the committed baseline work without optional diagnostic privileges?

If any answer is unknown, keep the source diagnostic until it is proven.

## 14. Repository reference points

The implementation and executable contracts live in:

- `SRUI_Semantic_Remote_UI_Design_v0.6.md`, §31.1;
- `client-macos/Benchmarks/ParseRenderBenchmark.swift`;
- `client-macos/Benchmarks/BenchmarkPlatformSupport.swift`;
- `client-macos/Benchmarks/BenchmarkSupport.swift`;
- `benchmarks/run.py`;
- `benchmarks/schema.json`;
- `benchmarks/parse-render/run_xctrace.py`;
- `benchmarks/parse-render/xctrace_allocations.py`;
- `benchmarks/parse-render/BenchmarkDriver.entitlements`;
- `benchmarks/parse-render/profile-allocations.sh`;
- `benchmarks/tests/test_report.py`;
- `benchmarks/tests/test_xctrace.py`.

For platform behavior, prefer the installed `xctrace(1)`, `DevToolsSecurity(8)`,
`codesign(1)`, `heap(1)`, and `malloc_history(1)` manuals over examples written for another
Xcode version. The repository's operational README also cites Apple Developer Forums threads
664347 and 799351 for TOC-discovered Allocations view exports.
