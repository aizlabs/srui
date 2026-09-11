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
| Xcode Allocations prototype (removed) | Historical rejected `xctrace` experiment | Exact SRUI host PID and process birth, whole trace/final live view | §31.1 acceptance, committed metrics, WebKit comparison, and workload-interval attribution |

The most important outcome is that xctrace is not the authoritative allocation measurement. The
normal benchmark is complete without Developer Tools access. The prototype marked its output
`diagnostic_only=true` and `authoritative_benchmark_metric=false`; it was later removed so that
output cannot be mistaken for current §31.1 evidence.

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
4. Refresh the candidate-local cursor park. WebKit also selects its already-created hidden window
   position here. These operations are untimed and close physical pointer movement during warm-up
   or capture startup.
5. Record the action-start Mach timestamp.
6. Run the first-state or two-state production workload while the host remains hidden. The native
   path creates and positions its window during this timed attach.
7. Immediately before the sole order-front/display submission, arm a display-time cutoff.
8. Ignore incomplete frames and every frame whose `displayTime` does not cross that cutoff.
9. For each later candidate frame, resolve and validate the exact target/window geometry and
   z-order, crop the target content from that same frame, and apply the pixel predicates.
10. Requery identity, geometry, and z-order after pixel comparison to close the verification race.
11. Accept the frame only if all pre- and post-checks agree.
12. Report action-start through the accepted frame's `displayTime`.

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

#### CLI activation and active-Space association

A full run on the tested macOS/Xcode 26 host exposed a separate failure mode: ScreenCaptureKit kept
supplying complete display frames, `NSWindow.isVisible` was true, and
`CGWindowListCopyWindowInfo(.optionAll, ...)` contained the exact candidate window at layer 25,
alpha 1, and the expected 960×752 frame. The same window was absent from the
`.optionOnScreenOnly` inventory, so every one of 144–1,264 post-cutoff frames was correctly
rejected as `exact WindowServer target identity was unavailable`. The final diagnostic now also
records `NSWindow.isOnActiveSpace`, the collection-behavior mask, and the application activation
policy. This distinguishes Space/application-set association from Screen Recording authorization,
missing frames, geometry, or level selection.

Activation is not a valid repair or proof. Apple documents `NSApplication.activate()` as a request
whose success is not guaranteed. The former `activateIgnoringOtherApps` option is deprecated on
macOS 14 and later and the tested SDK states that it has no effect. `orderFrontRegardless()`
orders a window at the front of its level while the app is inactive, but does not promise to move
the window between Spaces. `.moveToActiveSpace` is mutually exclusive with
`.canJoinAllSpaces` and acts when the window becomes active, so it cannot make an unattended
activation request deterministic.

A reduced AppKit control reproduced the exact state without SRUI rendering. Under the failing
desktop state, a regular command-line application with a ScreenCaptureKit stream reported
`isVisible=true`, `isOnActiveSpace=false`, presence in `.optionAll`, and absence from
`.optionOnScreenOnly`; adding both `.canJoinAllSpaces` and `.canJoinAllApplications` did not
change that result. The otherwise identical accessory application reported
`isOnActiveSpace=true` and appeared in both inventories, with the capture stream running.
This experiment isolates the reliable policy-level difference but does not claim knowledge of
WindowServer's private Space-assignment algorithm.

A suspected process-supervisor cause was also disproved. The sentinel's
`start_new_session=True` predates the previous successful baseline. The current exact driver
failed identically when launched directly, through the sentinel, in a new POSIX session, and in a
new process group within the caller's session. POSIX session identity is not macOS Aqua login or
Space identity; changing it did not move the window.

The retained benchmark-only arrangement is therefore:

- all unbundled benchmark-driver processes use AppKit's `.accessory` activation policy and do not
  claim or require foreground activation;
- benchmark host windows opt into `.canJoinAllSpaces` and
  `.canJoinAllApplications` before ordering;
- §31.4 still requires the mounted production editor to become the window's first responder with
  an input context, and exercises production controls, menu presentation, and compositor evidence;
- no policy, first-responder value, or collection-behavior value is accepted as presentation
  evidence.

Every measured target must still appear in `.optionOnScreenOnly`, match the exact
PID/window/layer/display/geometry, have clear z-order, and produce accepted ScreenCaptureKit pixels.
Supervised full-profile focused runs for §31.1 and §31.4 completed with this arrangement after the
failure was reproduced using the same already-built driver path. The §31.4 run completed all 640
held-response local interaction probes across 0/100/300/600 ms RTT with no failed assertion. The
consolidated full run remains the authoritative confirmation.
Operationally, if `.optionAll` contains the exact expected window but `.optionOnScreenOnly` does
not, inspect `isOnActiveSpace`, activation policy, and collection behavior before changing capture
permissions or weakening identity checks. If neither inventory contains the window, investigate
ordering/lifetime instead. If the on-screen inventory contains it with different fields, preserve
the exact mismatch as the failure.

Apple references:

- [`NSApplication.activate()`](https://developer.apple.com/documentation/appkit/nsapplication/activate())
  — an activation request, not a guarantee;
- [`NSWindow.orderFrontRegardless()`](https://developer.apple.com/documentation/appkit/nswindow/orderfrontregardless())
  — front-of-level ordering while inactive, not a Space move;
- [`NSWindow.isOnActiveSpace`](https://developer.apple.com/documentation/appkit/nswindow/isonactivespace)
  — predicts active-Space placement even while hidden;
- [`canJoinAllSpaces`](https://developer.apple.com/documentation/appkit/nswindow/collectionbehavior-swift.struct/canjoinallspaces)
  and [`canJoinAllApplications`](https://developer.apple.com/documentation/appkit/nswindow/collectionbehavior-swift.struct/canjoinallapplications)
  — the public ordinary-Space and eligible application-set behaviors;
- [Apple's window collection-behavior guide](https://developer.apple.com/library/archive/documentation/Cocoa/Conceptual/WinPanel/Articles/SettingWindowCollectionBehavior.html)
  — documents the mutually exclusive Spaces-behavior group.

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
pointer. Every full paint refreshes the child park after accepting its pre-action display baseline
and before recording the action-start timestamp. Both guards restore the exact saved location
during normal return and failure unwinding.

`CGWarpMouseCursorPosition` deliberately changes location without emitting a mouse event. That can
leave the previously hovered application\'s tooltip alive after it deactivates. This occurred in a
real full run as a 43×19-point ChatGPT message-time tooltip (`23:24`) at WindowServer layer 103;
all 1,167 post-cutoff frames were correctly rejected because it overlapped the target. A controlled
Swift/Quartz probe showed that a matching `.mouseMoved` event posted at `.cgSessionEventTap`
dismissed the exact window. Park and restore therefore post that public session-level event after
warping. The event occurs only during untimed preparation; it does not whitelist the tooltip, and
an overlay that remains ahead still fails the existing z-order predicate.

The 960×720 renderer ROI is placed in a visible-frame corner away from the parked pointer, with a
64-point clearance requirement while hidden. Parking stays away from display edges to avoid Dock,
menu-bar, and hot-corner activation. The cursor is not removed from WindowServer evidence. If a
cursor or another nonzero-alpha surface intersects the target ahead, the sample remains invalid.

A locked or inactive login session can make the necessary display/window evidence unavailable.
A real failed full run showed the characteristic state: the window was `NSWindow.isVisible=true`
and present through `.optionIncludingWindow`, but the console dictionary reported
`CGSSessionScreenIsLocked=1`, `NSApplication.isActive=false`, `isKeyWindow=false`, and the exact
window was absent from `.optionOnScreenOnly`. The driver now rejects an explicitly locked session
before measurement; if that diagnostic dictionary key is unavailable, exact WindowServer and pixel
checks remain authoritative and fail closed. `caffeinate` can keep an already available display
awake for the runner's lifetime; it cannot unlock a session, grant Screen Recording, or override
secure UI.

### 2.6 §31.4 real-input-routing experiment and accepted scope

A late §31.4 prototype tried to extend the local-feedback benchmark from mounted-control state
changes to real session-level pointer delivery. It saved and warped the Quartz pointer, posted
`.mouseMoved`, `.leftMouseDown`, and `.leftMouseUp` events through public CoreGraphics event taps,
pumped `NSApplication` events, and attempted to prove the button's highlighted interval before an
independent watchdog released the mouse. `CGPreflightPostEventAccess()` returned true on the test
host, so denial of event-posting authorization was not the observed failure.

The failure was foreground ownership. The unbundled SwiftPM executable could create an exact
on-screen status-level WindowServer surface, but neither `NSApplication.activate()`, the legacy
`NSRunningApplication.activate(.activateIgnoringOtherApps)`, nor a posted title-bar click made the
process active and the window key on the macOS 14 test host. The fail-closed diagnostic reported
`post_access=true`, `active=false`, and `key=false`. Without active/key ownership, a posted pointer
sequence cannot honestly be called the control's normal AppKit input route.

An app-bundle launcher with a real `Info.plist` and LaunchServices lifecycle might make end-to-end
input routing measurable, but it would add a second packaging/launch system and could change the
subject. Task 34 rejected that expansion. It also rejected direct `sendEvent` as proof of
WindowServer delivery: constructing an `NSEvent` and calling the application directly bypasses the
boundary the experiment was supposed to establish.

The accepted §31.4 boundary is therefore explicit:

- text editing, selection, IME, scrolling, and menu work run on controls mounted by the production
  renderer;
- hover injects deterministic pointer/application/window context through benchmark SPI into the
  production `HoverFeedbackButton.reconcilePointerState()` implementation;
- pressed feedback invokes AppKit `performClick(nil)` on the mounted button and requires exactly
  one production `ActionTrampoline` callback;
- full mode still requires the real compositor-visible target transition and, for hover, exact
  bounded restoration;
- every local action overlaps an exact held production transaction at the configured RTT.

This measures whether SRUI adds a synchronous network dependency between native local state and
visible output. It does **not** measure keyboard/mouse hardware delivery or WindowServer/AppKit
event-dispatch latency. Fresh reports and the network README must preserve that non-claim. The
current accepted path does not post input events and therefore requires no input-event permission;
full pixel evidence still requires Screen Recording.

If a future task requires true end-to-end input latency, it must launch an identified app bundle,
prove active/key state before timing, preserve and restore the previous application and pointer,
correlate the injected event timestamp with the accepted compositor frame, bound mouse-up cleanup,
and treat any authorization or focus failure as missing evidence rather than zero latency.

### 2.7 Pixel acceptance

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

### 2.8 Precise presentation claims

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

## 3. Reported allocation evidence and deferred cumulative count

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
### 3.4 Why the endpoint source is valid for its narrow metric

The endpoint source is modest but defensible:

- both reads occur inside the measured candidate process;
- both use the same counter source and clock-independent subtraction;
- the boundary surrounds the production resource workload;
- no external attach is needed;
- the profiler cannot suspend or enumerate a second allocator process;
- the result has explicit signed, host-only, default-zone semantics.

The source does not become more accurate by giving it a broader name. Its value comes from a
narrow, repeatable, correctly described boundary. In particular, it is not complete evidence for
the separate cumulative allocation-event quantity requested by §31.1.

### 3.5 Deferred cumulative allocation-event count

Task 34 does not report cumulative allocation calls or total bytes requested from the allocator.
That omission is explicit in the generated §31.1 notes, both operational READMEs, the metric IDs,
and the units. The existing `blocks_in_use` delta must never be relabeled as “allocations.”

#### Exact experiment and rejection

A controlled C probe first established the semantics of Apple's history mechanism:

1. launch with `MallocStackLoggingNoCompact=1` while explicitly unsetting
   `MallocStackLogging`;
2. stop the exact PID and capture `malloc_history PID -allEvents`;
3. allocate 123, 456, and 623 bytes, free the 456-byte allocation, and stop again;
4. capture a second history and require the first event sequence to be an exact prefix.

The non-compacting history appended three `ALLOC` records and one `FREE` record, so an allocation
freed inside the interval remained countable. The compact mode reordered/elided history and failed
the prefix contract, as expected.

The same method was then run against the real release SRUI `BenchmarkDriver`, using the existing
supervised candidate handshake and exact PID/process-birth identity. It failed closed before
producing a benchmark number: the first *pre-workload* `malloc_history -allEvents` text export was
1,902,439,272 bytes. The implementation's safety cap was 536,870,912 bytes. No after snapshot was
taken, the temporary output was cleaned, and no value from this attempt entered a report.

This is intrinsic to the supported command surface, not a missing flag. Apple's
[`malloc_history(1)` documentation](https://github.com/vitorgalvao/macos-man-pages/blob/83bb649c874e3f76a0023279be4c48656d16b18d/macOS/26/man1/malloc_history.1)
states that `-allEvents` lists cumulative allocation/free events “up to the current time,” warns
that the output can be voluminous, and exposes neither a time-range filter nor a no-stack output
mode. `-q` only suppresses the process description header/footer. The undocumented
`-machineReadableOutput` observed in Xcode 26.2 is not an acceptable committed dependency.
Apple's
[`malloc(3)` documentation](https://github.com/apple-oss-distributions/libmalloc/blob/c49dafa25f1efe8607701ae6014a663ad2ee437f/man/malloc.3)
also confirms that `MallocStackLoggingNoCompact` retains adjacent allocation/free pairs and that
plain `MallocStackLogging` takes precedence if both are set.

Raising the cap would require at least six expanded histories for three before/after samples,
several gigabytes of concurrent temporary storage, repeated symbolication of process startup, and
substantial profiler perturbation. Streaming the same multi-gigabyte text would reduce temporary
storage but not the cumulative export or symbolication work. Neither is a responsible default for
a benchmark suite that previously exhausted local disk during redundant full builds.

#### Concrete follow-up

[Issue #48](https://github.com/aizlabs/srui/issues/48) tracks the replacement. The recommended
implementation is the pattern used by Apple's SwiftNIO allocation benchmarks:

- load a benchmark-only Darwin dynamic library through `DYLD_INSERT_LIBRARIES`;
- interpose the relevant allocation entry points through dyld's
  `__DATA,__interpose` mechanism;
- maintain atomic allocation-call and requested-byte counters;
- snapshot/reset those counters immediately around the separate production resource pass;
- keep the instrumented pass disjoint from paint and CPU timing;
- test allocate/free, calloc, realloc, alignment, failure, and multithreaded counter semantics;
- enumerate and justify the hooked API surface so unobserved allocation paths fail review rather
  than silently undercounting;
- retain exact candidate PID/birth attribution and report the instrumentation scope;
- treat WKWebView helpers as a separate multiprocess scope, never as part of the SRUI host count.

References:

- [SwiftNIO allocation-counter design](https://github.com/apple/swift-nio/blob/8c063f043d94c120d0f8d6303ef4fc7918e3561d/IntegrationTests/tests_04_performance/test_01_resources/README.md)
- [SwiftNIO Darwin interposer](https://github.com/apple/swift-nio/blob/8c063f043d94c120d0f8d6303ef4fc7918e3561d/IntegrationTests/allocation-counter-tests-framework/template/HookedFunctionsDoHook/Sources/HookedFunctions/src/hooked-functions-darwin.c)

Until that follow-up lands, the honest §31.1 allocation evidence is limited to signed net-live
default-zone endpoint deltas, footprint growth, and peak footprint. It is not a cumulative event
count.
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
simultaneous coherent multiprocess snapshot. The rejected prototype therefore targeted only the SRUI host and refused a WebKit capture.

### 4.6 AppKit activation-policy experiments

Early auxiliary candidates attempted `NSApplication.setActivationPolicy`. Raw value 0
(regular) and raw value 1 (accessory) both failed through LaunchServices policy modification in
the instrumented context. Switching between those values was not a reliable fix.

The prototype ultimately left AppKit's process-selected activation policy unchanged. It did not
call `activate` or order a foreground window; it performed the hidden production layout/draw path
and `CATransaction.flush()`. Ordinary full visual candidates remain
regular foreground applications and are the only candidates used for ScreenCaptureKit
presentation evidence.

Initializing `NSApplication.shared` can still emit a LaunchServices assertion. That log alone
does not determine failure. The exact subsequent render/control handshake, process identity, and
exit status determine whether the diagnostic candidate succeeded.

### 4.7 Disposition of the xctrace prototype

The capture prototype was diagnostic-only and never supplied a §31.1 metric. It proved that the
available Xcode 26 exports cannot support the required claim:

- Statistics covers the whole recording, including the attach-time live heap;
- Allocations List is a final live view and omits allocations freed before finalization;
- their timestamps were not shown to share the TOC or Swift clock origin;
- the two views were independently materialized and did not reconcile exactly;
- attaching to warmed WebKit materially stalled the comparison workload.

The runnable controller, exporter, entitlement, shell wrapper, and tests were therefore removed.
Keeping a polished command for a rejected method made it too easy to present diagnostic output as
benchmark evidence. Sections 3, 4, and 10 retain the exact observations so the experiment need not
be repeated. A future implementation must satisfy issue #48's bounded in-process event-counter
criteria before cumulative allocation traffic can become authoritative.

## 5. Process-attribution lessons retained

A numeric PID alone is not stable evidence because the operating system can reuse it. Any future
process-scoped diagnostic must bind the supervised child PID to process birth, prove liveness
through the measurement boundary, and prove that exact PID/birth identity is gone afterward.
Process-name matching is insufficient.

WindowServer attribution is a separate chain: AppKit window number, owner PID, dynamically
resolved layer, bounds, display, target crop, and z-order before and after pixel verification.
Neither process identity nor window identity can substitute for the other.

## 6. Permission and signing findings (historical)

The rejected prototype required three distinct mechanisms:

- Developer Tools privacy access for the responsible terminal or Codex application;
- system developer mode as reported by `/usr/sbin/DevToolsSecurity -status`;
- `com.apple.security.get-task-allow=true` on a private staged copy of the target.

Screen Recording is unrelated: it authorizes ScreenCaptureKit pixels for full visible-paint
measurements. Full Disk Access is required for neither measurement and must not be suggested as a
workaround. Changing a privacy grant may require restarting the responsible application.

The prototype never re-signed the SwiftPM product in place. It copied the executable into a
private, token-owned directory and signed only that copy. This avoided mutating a shared build
artifact. These details are historical constraints, not current benchmark setup steps: the normal
suite does not invoke xctrace or require Developer Tools access.

## 7. Recorder-lifecycle findings (historical)

A created trace directory and recorder progress text did not prove that instrumentation was ready.
The experiment used a unique Darwin tracing-started notification, exact identity checks, and a
bounded control handshake before releasing the workload. After the workload it used a synchronous
`access(2)`/`usleep(3)` acknowledgement loop so Swift concurrency wakeups did not add avoidable
heap-tail traffic during finalization.

That quiescence reduced noise but could not make the two exports simultaneous or turn whole-trace
and final-live data into an inner-workload event stream. Recorder, watcher, candidate, supervisor,
and process-group termination all needed independent bounds. Those requirements remain applicable
if a new profiler experiment is proposed.

## 8. Disk and artifact findings

A `.trace` is a directory bundle; `stat -f %z` reports only the directory entry, not recursive
content. A real non-compacting `malloc_history -allEvents` pre-workload export reached
1,902,439,272 bytes. Six interval snapshots would therefore have been an unsafe benchmark default.
Repeated retained traces and redundant full builds exhausted local disk during Task 34.

Future diagnostic artifacts must have recursive byte limits, a free-space reserve, ownership-token
checked cleanup, bounded exports, and interruption cleanup that preserves the original failure.
Normal benchmark runs create no xctrace data.

## 9. Supported reproduction and troubleshooting

Build and run the isolated nested Swift package from the repository root:

```sh
swift build --disable-automatic-resolution \
  --package-path client-macos/Benchmarks \
  -c release \
  --product BenchmarkDriver

client-macos/Benchmarks/.build/release/BenchmarkDriver \
  --fixture benchmarks/fixtures/coding-agent-ui.json \
  --profile smoke \
  --only-section 31.1 \
  --output /tmp/srui-31.1.json
```

Run the consolidated suite and its Python validation separately:

```sh
scripts/run-benchmarks --profile smoke
scripts/run-benchmarks --profile full
uv run --frozen pytest benchmarks/tests
python3 benchmarks/generate_metric_contract.py --check
```

Full mode needs an active unlocked display and Screen Recording permission. Neither mode requires
Developer Tools permission. There is no supported xctrace command in this repository.

| Symptom | Evidence boundary to inspect |
|---|---|
| No displays or frames in full mode | Screen Recording, active display, and unlocked session |
| Window never becomes eligible | Exact WindowServer identity, level, geometry, crop, and z-order |
| Dock or menu appears during preparation | Pointer park, bounded settling, and no surface whitelist |
| Candidate exits before measurement | Supervised stderr, exact process identity, and cleanup result |
| Trace/export data is proposed as §31.1 allocations | Reject it unless a new method proves the exact workload interval and event semantics |
| Disk pressure follows experiments | Identify trace bundles and build artifacts explicitly; do not repeat full captures |
| LaunchServices logs an assertion | Judge the exact render/control handshake and exit status, not the log alone |

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

## 14. Repository reference points

The live implementation and executable contracts are:

- `SRUI_Semantic_Remote_UI_Design_v0.6.md`, §31.1;
- `benchmarks/metric-contract.json` and `benchmarks/contract.py`;
- `benchmarks/schema.json`;
- `benchmarks/run.py`, `benchmarks/validation.py`, and `benchmarks/reporting.py`;
- `benchmarks/generate_metric_contract.py` and its checked-in Swift/Rust descriptors;
- `client-macos/Benchmarks/ParseRenderBenchmark.swift` and
  `LocalRendererBenchmark.swift`;
- `client-macos/Benchmarks/NativeRendererCandidate.swift`,
  `WebRendererCandidate.swift`, and `RendererCandidateGeometry.swift`;
- `client-macos/Benchmarks/FrameAcceptancePolicy.swift`,
  `BenchmarkExplicitPresentation.swift`, `BenchmarkPassivePresentation.swift`, and
  `BenchmarkMenuPresentation.swift`;
- `client-macos/Benchmarks/BenchmarkPlatformSupport.swift` and
  `BenchmarkSupport.swift`;
- `benchmarks/tests/`.

The deleted xctrace controller/exporter, entitlement, wrapper, and tests are intentionally not
repository reference points. Historical tool behavior in this document was observed with Xcode
26.0 (17C52); do not infer current flags or schemas from it. For a new investigation, inspect the
installed `xctrace(1)`, `codesign(1)`, `heap(1)`, and `malloc_history(1)` manuals first.
