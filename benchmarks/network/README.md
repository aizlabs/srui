# §31.4 network and local-interaction benchmark

Owner: `client-macos/Benchmarks/NetworkBenchmark.swift::networkAndLocalInteraction`. The benchmark
runs framed traffic through the production `SessionController` transport boundary while injecting
0, 100, 300, and 600 ms RTT. The same production AppKit editor/renderer path performs text entry,
caret movement, selection, marked-text IME composition, scrolling, hover, pressed feedback, and
menu opening. Local control updates and server-dependent feedback are timed independently.

Task 34 exposed two prerequisite renderer gaps. Stock `NSButton` did not provide a stable visible
hover transition for the production Button path, so `RendererAppKit.ControlFactory` now constructs
its internal `HoverFeedbackButton`; `ControlFactoryTests` proves enter changes its raster and exit
restores it. The canonical semantic fixture now also contains a `Menu` node with inline `.items`,
which `ControlFactory` mounts as an `NSPopUpButton`; the menu-opening trial presents that exact
renderer-owned `NSMenu` instead of constructing benchmark-only menu state. Its production test
proves initial item materialization, incremental replacement, and clearing. These are explicit
production local-feedback prerequisites under §22.5, not forced invalidations in the timed
observer; the measured subject includes both corrections.

In full mode, the harness resolves `CGWindowLevelForKey(.dockWindow)`,
`CGWindowLevelForKey(.statusWindow)`, `CGWindowLevelForKey(.popUpMenuWindow)`, and
`CGWindowLevelForKey(.screenSaverWindow)`, requires
`dock < status < popup < screen-saver`, and places the benchmark host at the resolved status
level. This avoids the Dock-owned desktop surface without placing the host above the production
menu being measured. Exact inventories use `.optionOnScreenOnly`; membership is the on-screen
proof because WindowServer may omit the redundant optional `kCGWindowIsOnscreen` field. Window
ID, owner PID, resolved layer, alpha, bounds, display, client-content geometry, and target geometry
remain mandatory. This is not a Dock, owner, pointer, or cursor whitelist:
`kCGWindowAlpha` is whole-window metadata rather than pixel opacity, and every intersecting
nonzero-alpha surface ahead—including a WindowServer cursor or `loginwindow`—still rejects the
sample.

Before untimed preparation, the full section saves the exact Quartz pointer location and parks it
at `display.maxX - 160` and the display's vertical midpoint; it restores the original location
on exit, including failure. The deterministic host is left-side, so the interior right-side park
stays outside the measured target without entering Dock, menu-bar, or hot-corner edge activation
zones. Because pointer relocation can nevertheless start the retraction of a previously activated
Dock or menu surface, exact `.optionOnScreenOnly` WindowServer identity acquisition has a
bounded 10-second untimed settling window. A separate 10-second geometry deadline must contain
identical exact WindowServer identity, window bounds, and client-content ROI observations spaced
300 ms apart. Preparation independently waits up to 10 seconds for a genuinely clear z-order
instead of accepting or whitelisting a transient surface. The extended geometry deadline
accommodates slow AppKit/WindowServer settling observed while cycling the 600 ms RTT case; it does
not move the later action-start or display-latency boundaries. A timeout reports the prepared
AppKit frame and current WindowServer entry fields. Only after these checks does the harness start
ScreenCaptureKit and obtain a complete-frame baseline for the exact visible target-control ROI.
Only after that baseline and its pre-action recheck succeed does the harness start the real
`BenchmarkTransport` injected response, verify that every nonzero RTT has an active delayed
operation, and begin the timed local action. Frames delivered while that action is still running
are ineligible. Once the action returns, the observer arms a fresh host-time cutoff and accepts
only a later complete frame with identical target geometry, nonblank/nonuniform content, and at
least eight unmasked ROI pixels whose RGBA value differs from the baseline by more than the
explicit 2/255 per-channel SCStream tolerance. The output callback records the visible timestamp
before locking or comparing pixels. Exact window identity, owner, resolved status layer, alpha,
bounds, display, client-content bounds, target ROI, and absence of any intersecting window above
are all re-derived after the accepted sample. The observer performs no invalidation, layout,
activation, ordering, display, or transaction flush after timing starts; a missing production
repaint therefore fails.

Hover adds a cleanup proof after its timed frame is accepted: the continuous stream is stopped,
`mouseExited` restores local state, and an explicit ScreenCaptureKit screenshot is compared with
a pre-action screenshot from the same API and normalization. Restoration requires zero unmasked
pixels with any channel delta greater than 5/255. The report publishes both the minimum material
hover-change pixel count and the maximum observed restoration channel delta; it does not present
an exact-image fingerprint match as the hover/restoration criterion.
It requires one new current-process window at the independently resolved pop-up level, proves that
level is above the prepared status-level host, crops that window from the same delivered frame used
for the timestamp, verifies nonblank/nonuniform pixels and exact identity/geometry/z-order, and
only then cancels AppKit menu tracking.
Smoke mode remains an explicitly named offscreen raster fallback with no compositor claim. Its
whole-host raster can charge an unrelated released remote invalidation to a later local sample, so
its cross-RTT latency delta is diagnostic rather than a correctness gate. Full mode enforces the
p50 delta against the measured display-frame budget. Both profiles still require every exact
held-response/delay-boundary/state proof. The transport also applies a 1 MiB/s limit,
deterministic frame loss/retry, and interruption.

Focused command:

    client-macos/.build/release/BenchmarkDriver --fixture benchmarks/fixtures/coding-agent-ui.json --profile smoke --only-section 31.4 --output /tmp/srui-31.4.json

Metrics are `interaction.{kind}.rtt.{0,100,300,600}`, `server_feedback.rtt.*`,
`local_rtt_delta`, `display.frame_budget`, `session_wire.{bytes,messages}`, and the
`impairment.*` family. In full mode, `local_latency_independent` requires the local p50 delta
to stay within the measured display-frame budget; smoke publishes the same delta only as an
offscreen-raster diagnostic. The p50, p95, and p99 metrics all carry the §23 next-frame target so
tail misses appear in follow-up reporting, but the paired p95/p99 cross-RTT deltas are not
assertion gates. In both profiles, `local_latency_independent` and `no_sync_rtt` require the exact
causal proof: each local trial starts one production response at the action boundary, verifies
the configured nonzero one-way delay is active at that boundary, holds delivery through exact
local visible completion, and releases the gate only afterwards. This demonstrates both genuine
overlap with the injected transport impairment and non-dependence on its response.
`server_latency_tracks_rtt` and `impairments_use_session` separately validate the
remote/control side.
