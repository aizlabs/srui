# §31.4 network and local-interaction benchmark

Owners: `client-macos/Benchmarks/NetworkBenchmark.swift::networkAndLocalInteraction`,
`LocalInteractionBenchmark.swift::localInteractionSamples`, and
`NetworkInteractionSupport.swift::LocalInteractionRecorder`.

The section mounts the representative state through the production `AppKitRenderer`, then runs
framed traffic through `SessionController`, `EventOutbox`, `SRUIFraming`, and
`BenchmarkTransport` at 0, 100, 300, and 600 ms configured RTT. Each local trial starts one exact
paired production transaction at its action boundary, proves the configured nonzero delay is
active, holds that response through local visible completion, and releases it only afterward.
Server-dependent feedback is timed separately and must track the injected RTT.

## Measured controls and interaction boundary

The measured controls are the renderer-mounted production `NSTextView`, its `NSScrollView`, and
the renderer-mounted `HoverFeedbackButton`. No semantic `Menu` node or `NSPopUpButton` is added for
the benchmark. Menu opening asks the mounted `NSTextView` for its native context menu and presents
that exact menu.

Text entry, caret movement, selection, marked-text IME composition, and scrolling invoke the
corresponding native control APIs on those mounted controls. Text entry must also produce and settle
one exact framed production `TEXT_EDIT`, including node ID, edit sequence, observed revision, ACK,
and an empty stable outbox tail.

Hover injects an inside/outside pointer location, application-active state, and window-visible state
through benchmark SPI into the production `HoverFeedbackButton.reconcilePointerState()` method.
The timed path is the real renderer-owned state change, invalidation, draw, and—in full mode—the
composited target pixels. Cleanup drives the same production reconciliation with an outside
location. The production button also reconciles on AppKit enter/exit, application activation,
window move/resize/minimize/occlusion notifications, and before drawing; production tests cover a
missed exit plus inactive, invisible, and detached contexts.

Pressed feedback calls `performClick(nil)` on that mounted button. This uses AppKit's native
programmatic click behavior and must invoke the production `ActionTrampoline` exactly once. Because
the highlighted state is transient while `performClick` is running, full mode allows a frame after
the action-start cutoff but before the call returns.

These hover and pressed trials deliberately exclude hardware-event and WindowServer input-routing
latency. Attempts to drive real session-level mouse events from the unbundled SwiftPM executable
were rejected: supported macOS versions did not reliably grant that executable foreground/key
activation even with event-posting access, so such a path could not produce repeatable evidence.
The suite measures the SRUI contribution from local native state to visible output; it does not
claim to benchmark the OS's common input-dispatch cost. This scope boundary is explicit in every
fresh report rather than being disguised as a real mouse event.

## Full compositor evidence

Full mode resolves the Dock, status, pop-up-menu, and screen-saver levels at runtime and requires
`dock < status < popup < screen-saver`. The host uses the resolved status level, below the native
menu surface. Window inventories use `.optionOnScreenOnly`; membership is the on-screen proof
because `kCGWindowIsOnscreen` is optional. Exact window ID, owner PID, layer, alpha, bounds,
display, client-content geometry, target geometry, and unobscured z-order remain mandatory.

Before each timed action, ScreenCaptureKit supplies a complete baseline for the exact target-control
ROI. Ordinary persistent interactions accept only a complete frame after action completion. Hover
and pressed may accept a complete frame after action start because their interesting state can be
transient during the action. In both cases the accepted frame must keep identical target geometry,
contain nonblank/nonuniform content, and differ by at least eight unmasked pixels above the explicit
2/255 per-channel tolerance. Its `SCStreamFrameInfo.displayTime`, not callback receipt, ends the
visible-latency interval. Exact identity, geometry, crop, and z-order are re-derived after pixel
verification. The observer performs no forced invalidation after timing starts.

Hover additionally restores the outside state and compares same-API ScreenCaptureKit screenshots.
Restoration permits no unmasked pixel with a channel delta above 5/255 and publishes both the
minimum material action delta and maximum restoration delta.

Menu timing requires one new current-process window at the independently resolved pop-up level,
crops that surface from the same frame used for its timestamp, verifies its pixels and exact
identity/geometry/z-order, then cancels menu tracking. The optional `kCGWindowIsOnscreen` dictionary
field is never required or used.

Smoke mode is an explicitly named offscreen raster fallback and makes no compositor claim. Its
cross-RTT latency delta is diagnostic because a whole-host raster can charge a previously released
invalidation to a later action. Full mode gates the worst local p50 increase against the measured
display-frame budget. Both profiles require every local state, held-response, delay-boundary,
framed-text-edit, and impairment proof.

The impairment cases also exercise a 1 MiB/s transport limit, deterministic loss/retry, and
interruption/resume. All impairment traffic remains above the transport interface; the benchmark
does not call `Transport.send` directly.

Focused command:

    client-macos/Benchmarks/.build/release/BenchmarkDriver --fixture benchmarks/fixtures/coding-agent-ui.json --profile smoke --only-section 31.4 --output /tmp/srui-31.4.json

Metrics are `interaction.{kind}.rtt.{0,100,300,600}`, `server_feedback.rtt.*`,
`local_rtt_delta`, `display.frame_budget`, `session_wire.{bytes,messages}`, and the
`impairment.*` family. Interaction p50/p95/p99 values carry the §23 next-frame target so material
absolute misses are reported. Paired p95/p99 cross-RTT deltas remain diagnostic; the correctness
gate uses the paired p50 delta and the exact causal held-response proof.
