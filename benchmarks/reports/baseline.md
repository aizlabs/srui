# SRUI benchmark report

- Generated: 2026-09-09T12:55:08.134230+00:00
- Profile: full
- Host: macOS-26.4.1-arm64-arm-64bit-Mach-O / arm64
- Fixture: benchmarks/fixtures/coding-agent-ui.json

## §31.1 Local renderer

| Metric | Value | Statistic | Target |
|---|---:|---|---:|
| SRUI first on-screen paint crossing display refresh | 61.8 ms | p50 | — |
| SRUI first on-screen paint crossing display refresh | 69.24 ms | p95 | — |
| SRUI complete on-screen paint crossing display refresh | 81.24 ms | p50 | — |
| SRUI complete on-screen paint crossing display refresh | 89.56 ms | p95 | — |
| SRUI complete on-screen paint crossing display refresh | 97.86 ms | p99 | — |
| SRUI candidate process CPU time | 1.03 ms | p50 | — |
| SRUI candidate process CPU time | 1.263 ms | p95 | — |
| SRUI host retained allocation delta | 1057 allocations | p50 | — |
| SRUI host allocated footprint growth | 0.1719 MiB | last-first | — |
| SRUI lifetime process footprint peak | 33.99 MiB | max | — |
| WKWebView first on-screen paint crossing display refresh | 33.45 ms | p50 | — |
| WKWebView first on-screen paint crossing display refresh | 41.03 ms | p95 | — |
| WKWebView complete on-screen paint crossing display refresh | 49.54 ms | p50 | — |
| WKWebView complete on-screen paint crossing display refresh | 57.28 ms | p95 | — |
| WKWebView host plus attributed helper CPU time | 0.3415 ms | p50 | — |
| WKWebView host retained allocation delta | 0 allocations | p50 | — |
| WKWebView host plus helper allocated footprint growth | 12.92 MiB | last-first | — |
| WKWebView host plus helper lifetime footprint peak | 115.4 MiB | max | — |
| SRUI representation | 890 bytes | exact | — |
| HTML representation | 3274 bytes | exact | — |
| screen capture authorization | 1 boolean | exact | — |

Assertions:

- **PASS** representative fixture preserves exact parent, type, and property semantics — 20 native store nodes and rendered control values plus 20 DOM nodes/elements/rendered properties matched exact fixture records
- **PASS** first and complete window submissions each advance the display framebuffer — 20 native and 20 WebKit completions; window submitted and display framebuffer advanced; representative DOM sentinel and animation frame preceded each window submission and display framebuffer advance
- **PASS** WebKit helper resources use exact measured process attribution — host 57694, helpers [57698, 57699, 57726, 57759, 57760, 57761, 57763, 57797, 57798, 57799, 57800, 57801, 57805, 57828, 57829, 57830, 57831, 57859, 57860, 57864, 57865, 57892, 57893, 57894, 57895, 57897, 57898, 57924, 57925, 57927, 57928, 57930, 57958, 57960, 57961, 57962, 57963, 57991, 57992, 57993, 57994]; no process-name matching

Notes:

- Full timing requires visible, unoccluded exact CGWindow presence and, for each submitted state, displayIfNeeded/CATransaction flush followed by an NSScreen framebuffer timestamp advance. One optional preauthorized ScreenCaptureKit pixel check per candidate runs after the timed/resource-attribution interval: native=1, WebKit=1, capture_authorization=true. No permission request is issued.
- Every measured sample uses a fresh candidate instance. That same instance first renders a tiny neutral representation, resets outside timing, takes CPU/allocation baselines, then consumes the representative in-memory transaction bytes or HTML through presentation. Native timing includes protobuf decode, ProtocolDecoder validation, TransactionApplier-equivalent semantic commit, renderer mount, and paint; WebKit waits for the representative DOM sentinel and, in full mode, requestAnimationFrame before presentation.
- Each candidate runs in a fresh verified process group. WebKit CPU and allocated-footprint metrics aggregate the host with exact benchmark-only WebContent/network/GPU diagnostic PIDs; retained malloc block counts are explicitly host-only.

## §31.2 Serialization

| Metric | Value | Statistic | Target |
|---|---:|---|---:|
| abstract state generation | 0.0025 ms | p50 | — |
| abstract state generation | 0.002875 ms | p95 | — |
| abstract state generation | 0.006125 ms | p99 | — |
| protobuf serialization | 0.01004 ms | p50 | — |
| protobuf serialization | 0.01112 ms | p95 | — |
| protobuf serialization | 0.02025 ms | p99 | — |
| serialized transaction size | 890 bytes | exact | — |

Assertions:

- **PASS** shared fixture produced SRUI protobuf — 20 nodes encoded into 890 bytes
- **PASS** renderer and serializer canonical transaction bytes match — 2e399122ac11783ba8f02a8ffed840faa8a5116bde7f48b1980bc3e2da04526c / 890 exact bytes

Notes:

- PTY spawn and renderer work are excluded from server serialization timing.

## §31.3 Mutation and frame independence

| Metric | Value | Statistic | Target |
|---|---:|---|---:|
| 1 updates semantic decode/apply | 0.01146 ms | p50 | — |
| 1 updates semantic decode/apply | 0.01775 ms | p95 | — |
| 1 updates semantic decode/apply | 0.04521 ms | p99 | — |
| 1 updates decode-to-visible | 18.58 ms | p50 | — |
| 1 updates decode-to-visible | 21.39 ms | p95 | — |
| 1 updates decode-to-visible | 21.65 ms | p99 | — |
| 1 updates wire bytes | 28 bytes | exact | — |
| 1 updates message count | 1 messages | exact | — |
| 100 updates semantic decode/apply | 0.09729 ms | p50 | ≤ 1 ms |
| 100 updates semantic decode/apply | 0.104 ms | p95 | — |
| 100 updates semantic decode/apply | 0.1131 ms | p99 | — |
| 100 updates decode-to-visible | 19.14 ms | p50 | — |
| 100 updates decode-to-visible | 21.29 ms | p95 | — |
| 100 updates decode-to-visible | 21.72 ms | p99 | — |
| 100 updates wire bytes | 2109 bytes | exact | — |
| 100 updates message count | 1 messages | exact | — |
| 1000 updates semantic decode/apply | 0.8274 ms | p50 | ≤ 5 ms |
| 1000 updates semantic decode/apply | 0.8763 ms | p95 | — |
| 1000 updates semantic decode/apply | 0.883 ms | p99 | — |
| 1000 updates decode-to-visible | 19.37 ms | p50 | — |
| 1000 updates decode-to-visible | 23.85 ms | p95 | — |
| 1000 updates decode-to-visible | 24.02 ms | p99 | — |
| 1000 updates wire bytes | 2.101e+04 bytes | exact | — |
| 1000 updates message count | 1 messages | exact | — |
| 60Hz bidirectional wire bytes | 1245 bytes | exact | — |
| 60Hz bidirectional message count | 30 messages | exact | — |
| 60Hz live-clock repaint count | 7 repaints | exact | — |
| 120Hz bidirectional wire bytes | 1245 bytes | exact | — |
| 120Hz bidirectional message count | 30 messages | exact | — |
| 120Hz live-clock repaint count | 15 repaints | exact | — |
| 144Hz bidirectional wire bytes | 1245 bytes | exact | — |
| 144Hz bidirectional message count | 30 messages | exact | — |
| 144Hz live-clock repaint count | 15 repaints | exact | — |
| 240Hz bidirectional wire bytes | 1245 bytes | exact | — |
| 240Hz bidirectional message count | 30 messages | exact | — |
| 240Hz live-clock repaint count | 28 repaints | exact | — |
| idle UI wire bytes | 0 bytes | observed max | — |
| idle UI message count | 0 messages | observed max | — |

Assertions:

- **PASS** decode-to-visible samples cross the display framebuffer boundary — every framed production SessionController sample matched the isolated TransactionApplier state and awaited an on-screen framebuffer advance
- **PASS** idle semantic UI emits zero observed SRUI traffic — 20ms idle observation deltas: bytes [0, 0, 0, 0], messages [0, 0, 0, 0]
- **PASS** wire bytes and message count are cadence independent — bidirectional bytes [1245, 1245, 1245, 1245], messages [30, 30, 30, 30]
- **PASS** live local repaint count varies independently — independent renderer-task repaint counts [7, 15, 15, 28]; at least one cadence coalesced the 24 mutations
- **PASS** presentation preserves committed revisions, state, and event order — all cadences committed revisions 2...25, emitted ordered events, and rendered progress 1.0

Notes:

- The updates.*.semantic trial times ProtocolDecoder plus TransactionApplier only. A byte-identical framed trial traverses BenchmarkTransport and SessionController into the pre-presented warm AppKitRenderer for updates.*.visible; final revision/value parity and captured frame bytes/messages are asserted.
- A live independent renderer task sleeps at 60/120/144/240Hz while a separate 4ms mutation source traverses production framing, SessionController, EventOutbox, decoding, semantic apply, and AppKitRenderer. Both inbound transactions/acks and outbound events are counted.

## §31.4 Network and local interaction

| Metric | Value | Statistic | Target |
|---|---:|---|---:|
| local display frame budget | 8.333 ms | exact | — |
| caret movement at 0ms RTT | 16.24 ms | p50 | ≤ 8.33333 ms |
| caret movement at 0ms RTT | 16.59 ms | p95 | — |
| caret movement at 0ms RTT | 16.64 ms | p99 | — |
| hover pressed at 0ms RTT | 16.01 ms | p50 | ≤ 8.33333 ms |
| hover pressed at 0ms RTT | 16.14 ms | p95 | — |
| hover pressed at 0ms RTT | 16.29 ms | p99 | — |
| ime composition at 0ms RTT | 15.13 ms | p50 | ≤ 8.33333 ms |
| ime composition at 0ms RTT | 19.62 ms | p95 | — |
| ime composition at 0ms RTT | 21.18 ms | p99 | — |
| menu opening at 0ms RTT | 31.45 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| menu opening at 0ms RTT | 37.11 ms | p95 | — |
| menu opening at 0ms RTT | 40.14 ms | p99 | — |
| scrolling at 0ms RTT | 16.22 ms | p50 | ≤ 8.33333 ms |
| scrolling at 0ms RTT | 16.58 ms | p95 | — |
| scrolling at 0ms RTT | 17.5 ms | p99 | — |
| text entry at 0ms RTT | 16.34 ms | p50 | ≤ 8.33333 ms |
| text entry at 0ms RTT | 20.73 ms | p95 | — |
| text entry at 0ms RTT | 41.34 ms | p99 | — |
| text selection at 0ms RTT | 16.36 ms | p50 | ≤ 8.33333 ms |
| text selection at 0ms RTT | 16.61 ms | p95 | — |
| text selection at 0ms RTT | 16.73 ms | p99 | — |
| server-dependent input-to-visible at 0ms RTT | 23.73 ms | p50 | — |
| server-dependent input-to-visible at 0ms RTT | 26.49 ms | p95 | — |
| server-dependent input-to-visible at 0ms RTT | 26.49 ms | p99 | — |
| caret movement at 100ms RTT | 21.19 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| caret movement at 100ms RTT | 25.47 ms | p95 | — |
| caret movement at 100ms RTT | 39.25 ms | p99 | — |
| hover pressed at 100ms RTT | 21.31 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| hover pressed at 100ms RTT | 23.06 ms | p95 | — |
| hover pressed at 100ms RTT | 23.2 ms | p99 | — |
| ime composition at 100ms RTT | 21.77 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| ime composition at 100ms RTT | 28.04 ms | p95 | — |
| ime composition at 100ms RTT | 31.52 ms | p99 | — |
| menu opening at 100ms RTT | 4.674 ms | p50 | ≤ 8.33333 ms |
| menu opening at 100ms RTT | 11.87 ms | p95 | — |
| menu opening at 100ms RTT | 12.05 ms | p99 | — |
| scrolling at 100ms RTT | 21.61 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| scrolling at 100ms RTT | 25.74 ms | p95 | — |
| scrolling at 100ms RTT | 40.85 ms | p99 | — |
| text entry at 100ms RTT | 21.06 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| text entry at 100ms RTT | 24.58 ms | p95 | — |
| text entry at 100ms RTT | 44.19 ms | p99 | — |
| text selection at 100ms RTT | 20.25 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| text selection at 100ms RTT | 24.52 ms | p95 | — |
| text selection at 100ms RTT | 29.97 ms | p99 | — |
| server-dependent input-to-visible at 100ms RTT | 128.3 ms | p50 | — |
| server-dependent input-to-visible at 100ms RTT | 134.1 ms | p95 | — |
| server-dependent input-to-visible at 100ms RTT | 134.1 ms | p99 | — |
| caret movement at 300ms RTT | 27.16 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| caret movement at 300ms RTT | 31.23 ms | p95 | — |
| caret movement at 300ms RTT | 35.69 ms | p99 | — |
| hover pressed at 300ms RTT | 26.66 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| hover pressed at 300ms RTT | 30.97 ms | p95 | — |
| hover pressed at 300ms RTT | 32.24 ms | p99 | — |
| ime composition at 300ms RTT | 25.56 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| ime composition at 300ms RTT | 29.22 ms | p95 | — |
| ime composition at 300ms RTT | 30.34 ms | p99 | — |
| menu opening at 300ms RTT | 6.865 ms | p50 | ≤ 8.33333 ms |
| menu opening at 300ms RTT | 9.635 ms | p95 | — |
| menu opening at 300ms RTT | 9.657 ms | p99 | — |
| scrolling at 300ms RTT | 26.39 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| scrolling at 300ms RTT | 28.91 ms | p95 | — |
| scrolling at 300ms RTT | 29.79 ms | p99 | — |
| text entry at 300ms RTT | 27.6 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| text entry at 300ms RTT | 29.91 ms | p95 | — |
| text entry at 300ms RTT | 45.38 ms | p99 | — |
| text selection at 300ms RTT | 25.38 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| text selection at 300ms RTT | 31.5 ms | p95 | — |
| text selection at 300ms RTT | 34.07 ms | p99 | — |
| server-dependent input-to-visible at 300ms RTT | 346.7 ms | p50 | — |
| server-dependent input-to-visible at 300ms RTT | 350.3 ms | p95 | — |
| server-dependent input-to-visible at 300ms RTT | 350.3 ms | p99 | — |
| caret movement at 600ms RTT | 27.23 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| caret movement at 600ms RTT | 33.67 ms | p95 | — |
| caret movement at 600ms RTT | 49.53 ms | p99 | — |
| hover pressed at 600ms RTT | 28.12 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| hover pressed at 600ms RTT | 32.33 ms | p95 | — |
| hover pressed at 600ms RTT | 33.94 ms | p99 | — |
| ime composition at 600ms RTT | 26.23 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| ime composition at 600ms RTT | 29.98 ms | p95 | — |
| ime composition at 600ms RTT | 30.1 ms | p99 | — |
| menu opening at 600ms RTT | 7.665 ms | p50 | ≤ 8.33333 ms |
| menu opening at 600ms RTT | 9.37 ms | p95 | — |
| menu opening at 600ms RTT | 11.58 ms | p99 | — |
| scrolling at 600ms RTT | 27.32 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| scrolling at 600ms RTT | 30.21 ms | p95 | — |
| scrolling at 600ms RTT | 32.3 ms | p99 | — |
| text entry at 600ms RTT | 26.45 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| text entry at 600ms RTT | 29.56 ms | p95 | — |
| text entry at 600ms RTT | 45.19 ms | p99 | — |
| text selection at 600ms RTT | 24.87 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| text selection at 600ms RTT | 30.09 ms | p95 | — |
| text selection at 600ms RTT | 30.19 ms | p99 | — |
| server-dependent input-to-visible at 600ms RTT | 667.9 ms | p50 | — |
| server-dependent input-to-visible at 600ms RTT | 682.1 ms | p95 | — |
| server-dependent input-to-visible at 600ms RTT | 682.1 ms | p99 | — |
| 1MiB/s bandwidth-limited production event | 16.94 ms | p50 | — |
| 1MiB/s bandwidth-limited production event | 16.94 ms | p95 | — |
| bandwidth-limited delivered bytes | 4.948e+04 bytes | exact | — |
| deterministic production loss attempts | 2 messages | exact | — |
| deterministic production loss delivered messages | 1 messages | exact | — |
| controlled production interruption detection | 0.7734 ms | p50 | — |
| measured production session wire bytes | 8.3e+04 bytes | exact | — |
| measured production session wire messages | 688 messages | exact | — |
| maximum RTT-induced local latency delta | 12.11 ms | p50 | ≤ 8.33333 ms |
| maximum RTT-induced local latency delta | 17.08 ms | p95 | — |
| maximum RTT-induced local latency delta | 32.89 ms | p99 | — |

Assertions:

- **PASS** mounted local interactions do not acquire one RTT — largest p50 increase 12.1060 ms, below the minimum injected one-way delay of 50.0 ms; every exact framed-response probe remained held through local visible completion=true; descriptive p95/p99 deltas were 17.0803/32.8890 ms; production renderer callbacks=400
- **PASS** injected transport RTT affects production server-dependent feedback — SessionController EventOutbox sends and framed transaction responses tracked 100/300/600ms RTT
- **PASS** render and local-feedback paths perform no synchronous network RTT — every exact per-sample framed response remained deliberately unfinished until after the local visible boundary, while production server-dependent feedback tracked 100/300/600ms RTT; p95/p99 local deltas remain descriptive rather than causal gates
- **PASS** bandwidth, loss, and interruption exercise session recovery — 49479 bandwidth bytes; lost and interrupted events remained in EventOutbox and replayed through replacement SessionControllers

Notes:

- Controls are mounted renderer TextArea, ScrollView, and Button. renderer-produced NSButton context NSMenu opened and deterministically cancelled through AppKit event tracking.
- Pressed state is observed between real NSWindow-dispatched mouseDown/mouseUp events and triggers the production ActionTrampoline. renderer-produced NSButton handled AppKit hover entry/exit but exposed no distinct hover raster; no visual-hover claim is made. Permission-free hover invokes the renderer-produced NSButton's own AppKit entry/exit path and does not claim WindowServer pointer latency.
- Local frame budget 8.333333 ms came from CGDisplayMode.refreshRate for the benchmark NSScreen.
- All impairment traffic traverses SessionController, EventOutbox, SRUIFraming, and replacement-session resume/replay; no benchmark calls Transport.send directly.
- The RTT-independence correctness gate uses the conservative p50 delta against the minimum injected one-way delay plus an exact held-response causal probe. Unpaired p95/p99 WindowServer tails remain reported as diagnostics and §23 follow-ups, but do not masquerade as evidence of network coupling.

## §31.5 Reconnect

| Metric | Value | Statistic | Target |
|---|---:|---|---:|
| disconnect immediately before event receipt | 6.874 ms | p50 | — |
| disconnect immediately before event receipt | 7.076 ms | p95 | — |
| disconnect immediately before event receipt | 7.223 ms | p99 | — |
| event receipt through settled side effect | 0.0005 ms | p50 | — |
| event receipt through settled side effect | 0.001375 ms | p95 | — |
| event receipt through settled side effect | 0.004 ms | p99 | — |
| in-process cached DUPLICATE response | 0.000167 ms | p50 | — |
| in-process cached DUPLICATE response | 0.000417 ms | p95 | — |
| in-process cached DUPLICATE response | 0.000583 ms | p99 | — |
| lost ACK wire reconnect through DUPLICATE acknowledgement | 0.102 ms | p50 | — |
| lost ACK wire reconnect through DUPLICATE acknowledgement | 0.1962 ms | p95 | — |
| lost ACK wire reconnect through DUPLICATE acknowledgement | 0.2188 ms | p99 | — |
| mid-resource reconnect and exact replay | 0.003583 ms | p50 | — |
| mid-resource reconnect and exact replay | 0.01025 ms | p95 | — |
| mid-resource reconnect and exact replay | 0.01958 ms | p99 | — |
| mid-transaction frame discard and atomic replay | 0.000833 ms | p50 | — |
| mid-transaction frame discard and atomic replay | 0.002167 ms | p95 | — |
| mid-transaction frame discard and atomic replay | 0.003291 ms | p99 | — |
| mid-transaction wire disconnect and exact atomic replay | 6.821 ms | p50 | — |
| mid-transaction wire disconnect and exact atomic replay | 7.005 ms | p95 | — |
| mid-transaction wire disconnect and exact atomic replay | 7.014 ms | p99 | — |
| partial EVENT disconnect and one processed replay | 7.354 ms | p50 | — |
| partial EVENT disconnect and one processed replay | 7.945 ms | p95 | — |
| partial EVENT disconnect and one processed replay | 8.001 ms | p99 | — |
| resume beyond journal retention | 0.000667 ms | p50 | — |
| resume beyond journal retention | 0.001667 ms | p95 | — |
| resume beyond journal retention | 0.002375 ms | p99 | — |
| resume within journal retention | 0.000625 ms | p50 | — |
| resume within journal retention | 0.001541 ms | p95 | — |
| resume within journal retention | 0.002 ms | p99 | — |
| mid-resource reconnect recovery | 0.5662 ms | p50 | — |
| mid-resource reconnect recovery | 1.012 ms | p95 | — |
| superseded resume response handling | 0.07579 ms | p50 | — |
| superseded resume response handling | 0.1192 ms | p95 | — |
| active resume response handling | 0.1585 ms | p50 | — |
| active resume response handling | 0.1721 ms | p95 | — |
| production reconnect boundary suite | 3.085e+04 ms | wall | — |

Assertions:

- **PASS** mid-resource reconnect restarts production transfer at offset zero — 32805-byte CAS object interrupted after one real chunk and reconstructed exactly
- **PASS** mid-transaction wire disconnect exposes no partial state — 50 capacity-one handle_connection streams decoded no partial frame, left the replica at revision 0, then replayed exactly one atomic revision
- **PASS** pre-receipt and partial EVENT disconnects are inert before one processed replay — 50 pre-receipt and capacity-one partial-frame handle_connection reconnects each dispatched zero events on the interrupted connection, then decoded one Processed acknowledgement and one resulting transaction
- **PASS** lost ACK wire replay is DUPLICATE without a second side effect — 50 duplex reconnects decoded ServerEventAck::Duplicate with cached revision 2; handler count and state stayed at one effect
- **PASS** journal retention boundary selects replay versus same-session resync — revision 6 replayed 6→8; revision 0 produced SAME_SESSION snapshot at 8
- **PASS** mid-resource disconnect discards partial bytes and retransmission commits — SessionController framing and ownership retired invisible partials; replacement replayed metadata and contiguous chunks from offset zero
- **PASS** superseded response affects only the expected old lifecycle teardown — new_action_identity_and_order_preserved=true, new_failure_not_called_by_old_response=true, new_lifecycle_unaffected_by_old_stop=true, new_response_replays_original_event=true, new_semantic_state_unchanged_by_old_response=true, new_wire_unchanged_by_old_response=true, old_lifecycle_closed=true, old_lifecycle_dispatch_blocked=true, old_lifecycle_diverged=true, old_lifecycle_handshake_failed=true, old_lifecycle_reports_superseded=true, old_replay_is_empty=true, old_response_sent_only_to_old_transport=true, pending_unchanged_by_old_response=true; old_failures=["superseded by a newer reconnect attempt: SERVER RESUME_OK answered a superseded resume attempt"]
- **PASS** production reconnect boundary suite — exit 0; SRUI §32 conformance — both
==============================================================================
 #  suite                    result  notes
------------------------------------------------------------------------------
 8  reconnect                PASS    9 runner(s)
==============================================================================
1 passed, 0 failed, 0 documented gap(s), 0 not applicable

Notes:

- Superseded resume-attempt inertness is measured against the Task 23 client generation guard in the macOS driver.
- The old resume response is inert with respect to active/new wire, event/action identity, semantic state, and lifecycle. Task 23 intentionally reports .superseded, marks the old controller diverged, and closes only its transport; those expected old-lifecycle effects are measured explicitly.

## §31.6 Terminal

| Metric | Value | Statistic | Target |
|---|---:|---|---:|
| embedded SRUI PTY exact ANSI capture and framing | 8.583 ms | p50 | — |
| embedded SRUI PTY exact ANSI capture and framing | 8.98 ms | p95 | — |
| embedded SRUI PTY exact ANSI capture and framing | 10.25 ms | p99 | — |
| standalone PTY exact ANSI interaction | 7.901 ms | p50 | — |
| standalone PTY exact ANSI interaction | 8.411 ms | p95 | — |
| standalone PTY exact ANSI interaction | 21.91 ms | p99 | — |
| terminal reconnect retention-loss decision | 0.000125 ms | sample | — |
| terminal payload | 6912 bytes | exact | — |
| embedded terminal frame count | 1 messages | p50 | — |
| embedded terminal frame count | 1 messages | p95 | — |
| embedded terminal frame count | 1 messages | p99 | — |
| client Terminal decode-to-visible | 56.27 ms | p50 | — |
| client Terminal decode-to-visible | 63.73 ms | p95 | — |
| client Terminal decode-to-visible | 64.12 ms | p99 | — |
| client Terminal draw-only | 52.39 ms | p50 | — |
| client Terminal draw-only | 58.54 ms | p95 | — |
| client Terminal draw-only | 58.96 ms | p99 | — |
| client terminal frame | 6912 bytes | exact | — |
| client terminal draw completions | 20 frames | exact | — |

Assertions:

- **PASS** standalone and embedded PTYs emit the identical ANSI byte stream — both paths compared all 6912 payload bytes
- **PASS** standalone terminal command exits successfully — 20 child exit statuses checked
- **PASS** embedded terminal reaches natural successful EOF with no trailing bytes — 20 production PTY exit statuses checked after the final exact offset
- **PASS** embedded terminal frames preserve exact offsets and bounds — 20 samples aggregated; p50 1 frames, each at most 16384 bytes
- **PASS** reconnect ring-buffer exhaustion maps to RETENTION_LOSS — PTYManager::subscribe returned Resync and catch_up emitted TerminalResyncRequired
- **PASS** fresh embedded terminal offsets remain exact — every fresh TerminalSession accepted byte offset zero and ended at 6912
- **PASS** embedded terminal visible completion is an actual draw — 20/20 TerminalView completions crossed the configured visible boundary and produced a content-distinct bitmap
- **PASS** terminal samples begin from fresh parser and view state — each sample constructs a new TerminalSession, TerminalView, and NSWindow, then reproduces the exact 256-line plain text with SHA-256 e1e85bdbda198ce2b94b317765b18ec90b981d25c0ef567ff83d102f6817fe8d; observed digests: ["e1e85bdbda198ce2b94b317765b18ec90b981d25c0ef567ff83d102f6817fe8d"]

Notes:

- Standalone and embedded samples both include production PTY spawn, identical shell execution, EOF, and exact ANSI output; embedded additionally captures and frames the bytes.
- Client parse/apply/decode-to-visible and draw-only components are reported separately. The Rust section provides the like-for-like standalone-versus-embedded end-to-end PTY comparison with the byte-identical ANSI payload; the consolidated report keeps these client-only components distinct and avoids cumulative-state bias.
- Smoke uses an explicitly named offscreen bitmap draw; full mode uses the visible-window framebuffer-advance boundary.

## Follow-up flags

- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 0ms RTT: 31.45 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 caret movement at 100ms RTT: 21.19 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 hover pressed at 100ms RTT: 21.31 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 ime composition at 100ms RTT: 21.77 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 100ms RTT: 21.61 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text entry at 100ms RTT: 21.06 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text selection at 100ms RTT: 20.25 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 caret movement at 300ms RTT: 27.16 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 hover pressed at 300ms RTT: 26.66 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 ime composition at 300ms RTT: 25.56 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 300ms RTT: 26.39 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text entry at 300ms RTT: 27.6 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text selection at 300ms RTT: 25.38 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 caret movement at 600ms RTT: 27.23 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 hover pressed at 600ms RTT: 28.12 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 ime composition at 600ms RTT: 26.23 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 600ms RTT: 27.32 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text entry at 600ms RTT: 26.45 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text selection at 600ms RTT: 24.87 ms vs target 8.33333 ms
