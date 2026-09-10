# §31.4 network and local-interaction benchmark

Owner: `client-macos/Benchmarks/NetworkBenchmark.swift::networkAndLocalInteraction`. The benchmark
runs framed traffic through the production `SessionController` transport boundary while injecting
0, 100, 300, and 600 ms RTT. The same production AppKit editor/renderer path performs text entry,
caret movement, selection, marked-text IME composition, scrolling, hover, pressed feedback, and
menu opening. Local control updates and server-dependent feedback are timed independently.

In full mode, the harness starts ScreenCaptureKit and obtains a complete-frame baseline for the
exact visible target-control ROI before the timed action. Frames delivered while the asynchronous
action is still running are ineligible. Once the action returns, the observer arms a fresh host-time
cutoff and accepts only a later complete frame whose target ROI has identical pixel geometry and a
different nonblank, nonuniform fingerprint. The output callback records the visible timestamp
before locking or hashing pixels. Exact window identity, owner, layer, alpha, bounds, display,
client-content bounds, target ROI, and absence of any intersecting window above are all re-derived
after the accepted sample. The observer performs no invalidation, layout, activation, ordering,
display, or transaction flush after timing starts; a missing production repaint therefore fails.

Menu opening uses a separate full-display stream because the menu is a new WindowServer surface.
It requires one new current-process popup-level window not present in the baseline, crops that
window from the same delivered frame used for the timestamp, verifies nonblank/nonuniform pixels
and exact identity/geometry/z-order, and only then cancels AppKit menu tracking. Smoke mode remains
an explicitly named offscreen raster fallback with no compositor claim. The transport also applies
a 1 MiB/s limit, deterministic frame loss/retry, and interruption.
Focused command:

    client-macos/.build/release/BenchmarkDriver --fixture benchmarks/fixtures/coding-agent-ui.json --profile smoke --only-section 31.4 --output /tmp/srui-31.4.json

Metrics are `interaction.{kind}.rtt.{0,100,300,600}`, `server_feedback.rtt.*`,
`local_rtt_delta`, `display.frame_budget`, `session_wire.{bytes,messages}`, and the
`impairment.*` family. `local_latency_independent` and `no_sync_rtt` require local p50 work to stay
within the measured display frame budget without tracking injected RTT. Each local trial also
blocks one exact production transaction before delivery and requires the local visible boundary to
complete while that transaction is still unfinished; this proves non-dependence on that response,
not that the configured one-way-delay interval itself lasted for the whole action. The gate is
released after visibility. `server_latency_tracks_rtt` and `impairments_use_session` separately
validate the remote/control side.
