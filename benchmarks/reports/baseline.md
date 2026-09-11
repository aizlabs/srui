# SRUI benchmark report

- Generated: 2026-09-11T07:52:45.508052+00:00
- Profile: full
- Host: macOS-26.4.1-arm64-arm-64bit-Mach-O / arm64
- Chip: Apple M2 Max
- Physical RAM: 34359738368 bytes
- Xcode: Xcode 26.2 / Build version 17C52
- Swift: Apple Swift version 6.2.3 (swiftlang-6.2.3.3.21 clang-1700.6.3.2) / Target: arm64-apple-macosx26.0 / swift-driver version: 1.127.14.1
- Rust: rustc 1.98.0 (88d9e12ae 2026-08-18)
- Python: 3.13.7
- Git: 654685be8de07c7bc9d6abd3d7ca4ea80f5fc2e2 (clean)
- Fixture: benchmarks/fixtures/coding-agent-ui.json
- Metric contract: schema 1 / sha256 98c8f350c9fd4afbf5ad13f2d7d5917b77e1d66db52b917cc8180e84f52ccbac

## §31.1 Local renderer

Samples:

- `macos.srui.render`: 20
- `macos.webkit.render`: 20

| Metric | Value | Statistic | Target |
|---|---:|---|---:|
| SRUI first on-screen paint crossing display refresh | 28.11 ms | p50 | — |
| SRUI first on-screen paint crossing display refresh | 61.39 ms | p95 | — |
| SRUI complete on-screen paint crossing display refresh | 69.34 ms | p50 | — |
| SRUI complete on-screen paint crossing display refresh | 117.7 ms | p95 | — |
| SRUI complete on-screen paint crossing display refresh | 135.9 ms | p99 | — |
| SRUI candidate process CPU time | 0.7459 ms | p50 | — |
| SRUI candidate process CPU time | 0.7841 ms | p95 | — |
| SRUI host-process net live allocation block delta | 1.913e+04 blocks | p50 | — |
| SRUI host-process net live allocation block delta | 1.967e+04 blocks | p95 | — |
| SRUI host-process net live allocation block delta | 1.969e+04 blocks | p99 | — |
| SRUI host-process net live allocation byte delta | 1.85e+06 bytes | p50 | — |
| SRUI host-process net live allocation byte delta | 1.88e+06 bytes | p95 | — |
| SRUI host-process net live allocation byte delta | 1.955e+06 bytes | p99 | — |
| SRUI host allocated footprint growth | 0.75 MiB | p50 | — |
| SRUI maximum concurrently sampled process footprint | 44.66 MiB | max | — |
| WKWebView first on-screen paint crossing display refresh | 133.5 ms | p50 | — |
| WKWebView first on-screen paint crossing display refresh | 183.4 ms | p95 | — |
| WKWebView complete on-screen paint crossing display refresh | 145.7 ms | p50 | — |
| WKWebView complete on-screen paint crossing display refresh | 179.7 ms | p95 | — |
| WKWebView host plus attributed helper CPU time | 0.558 ms | p50 | — |
| WKWebView comparison host-process net live allocation block delta | 94 blocks | p50 | — |
| WKWebView comparison host-process net live allocation block delta | 326 blocks | p95 | — |
| WKWebView comparison host-process net live allocation block delta | 421 blocks | p99 | — |
| WKWebView comparison host-process net live allocation byte delta | 2.531e+04 bytes | p50 | — |
| WKWebView comparison host-process net live allocation byte delta | 4.584e+04 bytes | p95 | — |
| WKWebView comparison host-process net live allocation byte delta | 5.336e+04 bytes | p99 | — |
| WKWebView host plus helpers allocated footprint growth | 0.125 MiB | p50 | — |
| WKWebView maximum concurrently sampled host-plus-helper footprint | 164 MiB | max | — |
| SRUI representation | 910 bytes | exact | — |
| HTML representation | 4420 bytes | exact | — |
| screen capture authorization | 1 boolean | exact | — |

Assertions:

- **PASS** representative fixture preserves exact parent, type, and property semantics — expected=20, native=20 semantic=true controls=true, WebKit=20 semantic=true elements-and-properties=true
- **PASS** candidate production state reaches a verified composited target-pixel frame — native presentations=40/40, pixels=40, authorized=true, content=20/20 samples passed; sample 19: full composited client-content proof: first=8bcb8a1cbdd05a51 complete=6b2df219cfb611c2 dimensions=958x718/958x718 unmasked=687844/687844 quantized-colors=130/89 non-dominant=139932/60918 nonblank-and-nonuniform=true material-pixels=687130/8 max-channel-delta=230/255 tolerance=2/255 materially-distinct=true same-geometry-and-normalization=true provenance=screencapturekit_first_complete_target_frame_status_level_25_appkit_inactive/screencapturekit_first_complete_target_frame_status_level_25_appkit_inactive; shared candidate lifecycle: srui/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted; WebKit presentations=40/40, pixels=40, authorized=true, content=20/20 samples passed; sample 19: full composited client-content proof: first=b52ca9ee70e20959 complete=662e12bb551dacf1 dimensions=958x718/958x718 unmasked=687844/687844 quantized-colors=33/33 non-dominant=2661/17695 nonblank-and-nonuniform=true material-pixels=15839/8 max-channel-delta=255/255 tolerance=2/255 materially-distinct=true same-geometry-and-normalization=true provenance=screencapturekit_first_complete_target_frame_status_level_25_appkit_inactive/screencapturekit_first_complete_target_frame_status_level_25_appkit_inactive; shared candidate lifecycle: webkit/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted; four disjoint passes per sample: the shared candidate lifecycle warms and resets before timing; host attachment, prebuilt protobuf ingestion, and display submission occur after the common timestamp; the accepted ScreenCaptureKit frame displayTime ends each visual interval; CPU/host-net-live-allocation/footprint-growth and peak use separate passes; four disjoint passes per sample: the shared candidate lifecycle warms and resets before timing; measured NSWindow creation and attachment, prebuilt HTML-byte ingestion, and display submission occur after the common timestamp; the accepted ScreenCaptureKit frame displayTime ends each visual interval; CPU/host-net-live-allocation/footprint-growth and peak use separate measured-host passes
- **PASS** WebKit helper resources use exact measured process attribution — host 77907, helpers [77908, 77909, 77910, 77964, 77965, 77994, 77995, 78023, 78024, 78055, 78056, 78110, 78111, 78137, 78138, 78165, 78166, 78219, 78220, 78247, 78248, 78275, 78276, 78304, 78305, 78358, 78359, 78387, 78388, 78414, 78415, 78444, 78445, 78498, 78500, 78526, 78527, 78553, 78554, 78581, 78582]; no process-name matching
- **PASS** signed net live allocation samples have exact host-process scope — SRUI blocks=20, bytes=20, scope=default malloc zone in the SRUI renderer host process only; signed after-minus-before net live state, not cumulative allocation events; WebKit control blocks=20, bytes=20, scope=default malloc zone in the WebKit comparison host process only; excludes WebContent, Network, and GPU helper processes; signed after-minus-before net live state, not cumulative allocation events
- **PASS** WindowServer isolation rejects an exact synthetic occluder — window isolation self-test passed: dock=20 status=25 ahead=26 popup=101 target=457309 occluder=457310
- **PASS** renderer candidate launch failures cannot leak a child process — forced candidate identity failure terminated the exact candidate process group and proved both the child and a real descendant gone; a forced descendant-identity publication failure and a forced internal candidate failure also proved both exact identities gone before the benchmark driver returned

Notes:

- Every accepted full-paint state requires an exact visible CGWindow, exact client/target geometry, unobscured z-order, and authorized nonblank/nonuniform ScreenCaptureKit pixels from one complete frame. Latency is action-start Mach time through that accepted frame's SCStream displayTime; callback receipt and pixel hashing are verifier metadata, not the presentation timestamp. The two reported timed states per logical sample produced native=40 and WebKit=40 verified captures; capture_authorization=true. Warm/resource/peak passes are not counted in that metric. No permission request is issued.
- Each native logical sample uses four separately warmed/reset renderer instances: first-state visual latency, complete two-transaction visual latency, CPU/host-net-live-allocation/footprint-growth through production display submission, and sampler-only peak footprint. Each WebKit sample resets one warmed view between the same four disjoint workloads while preserving exact helper PID identities. Neither resource pass creates ScreenCaptureKit buffers, and the footprint sampler never runs in the CPU/allocation pass.
- Native first timing decodes and applies revision 0→1 through production initial attach. Native complete timing decodes/applies 0→1 and then applies 1→2 through production incremental apply. WebKit uses structurally equivalent first and complete DOM states. The measured instance is checked immediately after each accepted presentation for exact semantic/control or DOM state. Full mode additionally requires nonblank, nonuniform, equal-geometry client-content fingerprints that differ between first and complete states.
- The shared candidate lifecycle requires host attach and representation ingest to fall inside the timed window for both candidates, so neither can hoist window construction out of its own measurement. The two readiness proofs are not equally cheap, and that asymmetry is inside the numbers: the native path applies its transaction and inspects state in process, while the WebKit path proves its DOM is ingested with evaluateJavaScript round trips to the WebContent process, polled at a 2 ms run-loop cadence. That harness cost has no native counterpart, so the reported WebKit latency is an upper bound on WebKit's disadvantage rather than a measurement of WebKit rendering alone.
- Each candidate process group is birth-identity verified. WebKit CPU, footprint growth, and peak footprint aggregate the host with exact benchmark-only WebContent/network/GPU diagnostic PIDs. Allocation samples are signed default-zone malloc_zone_statistics after-minus-before deltas: blocks_in_use and size_in_use describe net live state, not cumulative allocation traffic. Allocation scope: default malloc zone in the SRUI renderer host process only; signed after-minus-before net live state, not cumulative allocation events. Control scope: default malloc zone in the WebKit comparison host process only; excludes WebContent, Network, and GPU helper processes; signed after-minus-before net live state, not cumulative allocation events. Peak footprint is the maximum simultaneous current-footprint sample at 1 ms cadence strictly inside its separate representative pass, not a sum of per-process lifetime maxima.
- Task 34 deliberately defers cumulative allocation-call and requested-byte counts. The supported malloc_history all-events export was rejected after the first real pre-workload SRUI snapshot expanded to 1,902,439,272 bytes; no value from that attempt entered this report. GitHub issue aizlabs/srui#48 tracks a benchmark-only Darwin allocator-interposition counter.
- The warmed WKWebView candidate and its host-only allocator deltas are comparison controls only; they are not the production SRUI renderer, do not cover WebKit helper-process allocations, and do not describe SRUI's native AppKit rendering path.

## §31.2 Serialization

Samples:

- `rust.generation`: 500
- `rust.serialization`: 500

| Metric | Value | Statistic | Target |
|---|---:|---|---:|
| abstract state generation | 0.00325 ms | p50 | — |
| abstract state generation | 0.00425 ms | p95 | — |
| abstract state generation | 0.01171 ms | p99 | — |
| protobuf serialization | 0.007625 ms | p50 | — |
| protobuf serialization | 0.01158 ms | p95 | — |
| protobuf serialization | 0.02538 ms | p99 | — |
| serialized transaction size | 910 bytes | exact | — |

Assertions:

- **PASS** shared fixture produced SRUI protobuf — 20 nodes split 5 + 15 across revisions 0→1→2 and encoded into 910 framed bytes
- **PASS** canonical progressive transaction sequence bytes match — 8c7ebc3774fafdaa731d47e9dcb4611078a9efa1da649907db3c45cff03c40c6 / 910 exact canonical progressive sequence bytes

Notes:

- Generation constructs the same progressive two-transaction abstract UI plan used by the renderer benchmark.
- Serialization timing covers only the two production Transaction::to_wire_bytes calls. Deterministic u64 big-endian artifact framing/copying happens after the timer; PTY spawn and renderer work are excluded.

## §31.3 Mutation and frame independence

Samples:

- `macos.cadence.1.120`: 1
- `macos.cadence.1.144`: 1
- `macos.cadence.1.240`: 1
- `macos.cadence.1.60`: 1
- `macos.cadence.100.120`: 1
- `macos.cadence.100.144`: 1
- `macos.cadence.100.240`: 1
- `macos.cadence.100.60`: 1
- `macos.cadence.1000.120`: 1
- `macos.cadence.1000.144`: 1
- `macos.cadence.1000.240`: 1
- `macos.cadence.1000.60`: 1
- `macos.cadence.events.1.120`: 3
- `macos.cadence.events.1.144`: 3
- `macos.cadence.events.1.240`: 3
- `macos.cadence.events.1.60`: 3
- `macos.cadence.events.100.120`: 3
- `macos.cadence.events.100.144`: 3
- `macos.cadence.events.100.240`: 3
- `macos.cadence.events.100.60`: 3
- `macos.cadence.events.1000.120`: 3
- `macos.cadence.events.1000.144`: 3
- `macos.cadence.events.1000.240`: 3
- `macos.cadence.events.1000.60`: 3
- `macos.mutation.1`: 20
- `macos.mutation.100`: 20
- `macos.mutation.1000`: 20

| Metric | Value | Statistic | Target |
|---|---:|---|---:|
| 1 updates semantic decode/apply | 0.01558 ms | p50 | — |
| 1 updates semantic decode/apply | 0.01925 ms | p95 | — |
| 1 updates semantic decode/apply | 0.1147 ms | p99 | — |
| 1 updates decode-to-visible | 19.95 ms | p50 | — |
| 1 updates decode-to-visible | 20.59 ms | p95 | — |
| 1 updates decode-to-visible | 20.65 ms | p99 | — |
| 1 updates wire bytes | 28 bytes | exact | — |
| 1 updates message count | 1 messages | exact | — |
| 100 updates semantic decode/apply | 0.1057 ms | p50 | ≤ 1 ms |
| 100 updates semantic decode/apply | 0.1113 ms | p95 | — |
| 100 updates semantic decode/apply | 0.1157 ms | p99 | — |
| 100 updates decode-to-visible | 20.18 ms | p50 | — |
| 100 updates decode-to-visible | 20.68 ms | p95 | — |
| 100 updates decode-to-visible | 26.75 ms | p99 | — |
| 100 updates wire bytes | 2109 bytes | exact | — |
| 100 updates message count | 1 messages | exact | — |
| 1000 updates semantic decode/apply | 0.8672 ms | p50 | ≤ 5 ms |
| 1000 updates semantic decode/apply | 0.9297 ms | p95 | — |
| 1000 updates semantic decode/apply | 1.831 ms | p99 | — |
| 1000 updates decode-to-visible | 19.97 ms | p50 | — |
| 1000 updates decode-to-visible | 20.61 ms | p95 | — |
| 1000 updates decode-to-visible | 20.72 ms | p99 | — |
| 1000 updates wire bytes | 2.101e+04 bytes | exact | — |
| 1000 updates message count | 1 messages | exact | — |
| 1 updates at 60Hz decode-to-visible | 36.72 ms | sample | — |
| 1 updates at 60Hz total SRUI wire bytes | 322 bytes | exact | — |
| 1 updates at 60Hz total SRUI message count | 4 messages | exact | — |
| 1 updates at 60Hz inbound TRANSACTION bytes | 28 bytes | exact | — |
| 1 updates at 60Hz inbound TRANSACTION message count | 1 messages | exact | — |
| 1 updates at 60Hz outbound EVENT bytes | 294 bytes | exact | — |
| 1 updates at 60Hz outbound EVENT message count | 3 messages | exact | — |
| 1 updates at 60Hz synthetic change-gated repaint count | 1 repaints | exact | — |
| 1 updates at 120Hz decode-to-visible | 28.39 ms | sample | — |
| 1 updates at 120Hz total SRUI wire bytes | 322 bytes | exact | — |
| 1 updates at 120Hz total SRUI message count | 4 messages | exact | — |
| 1 updates at 120Hz inbound TRANSACTION bytes | 28 bytes | exact | — |
| 1 updates at 120Hz inbound TRANSACTION message count | 1 messages | exact | — |
| 1 updates at 120Hz outbound EVENT bytes | 294 bytes | exact | — |
| 1 updates at 120Hz outbound EVENT message count | 3 messages | exact | — |
| 1 updates at 120Hz synthetic change-gated repaint count | 1 repaints | exact | — |
| 1 updates at 144Hz decode-to-visible | 28.26 ms | sample | — |
| 1 updates at 144Hz total SRUI wire bytes | 322 bytes | exact | — |
| 1 updates at 144Hz total SRUI message count | 4 messages | exact | — |
| 1 updates at 144Hz inbound TRANSACTION bytes | 28 bytes | exact | — |
| 1 updates at 144Hz inbound TRANSACTION message count | 1 messages | exact | — |
| 1 updates at 144Hz outbound EVENT bytes | 294 bytes | exact | — |
| 1 updates at 144Hz outbound EVENT message count | 3 messages | exact | — |
| 1 updates at 144Hz synthetic change-gated repaint count | 1 repaints | exact | — |
| 1 updates at 240Hz decode-to-visible | 19.64 ms | sample | — |
| 1 updates at 240Hz total SRUI wire bytes | 322 bytes | exact | — |
| 1 updates at 240Hz total SRUI message count | 4 messages | exact | — |
| 1 updates at 240Hz inbound TRANSACTION bytes | 28 bytes | exact | — |
| 1 updates at 240Hz inbound TRANSACTION message count | 1 messages | exact | — |
| 1 updates at 240Hz outbound EVENT bytes | 294 bytes | exact | — |
| 1 updates at 240Hz outbound EVENT message count | 3 messages | exact | — |
| 1 updates at 240Hz synthetic change-gated repaint count | 1 repaints | exact | — |
| 100 updates at 60Hz decode-to-visible | 337.4 ms | sample | — |
| 100 updates at 60Hz total SRUI wire bytes | 3094 bytes | exact | — |
| 100 updates at 60Hz total SRUI message count | 103 messages | exact | — |
| 100 updates at 60Hz inbound TRANSACTION bytes | 2800 bytes | exact | — |
| 100 updates at 60Hz inbound TRANSACTION message count | 100 messages | exact | — |
| 100 updates at 60Hz outbound EVENT bytes | 294 bytes | exact | — |
| 100 updates at 60Hz outbound EVENT message count | 3 messages | exact | — |
| 100 updates at 60Hz synthetic change-gated repaint count | 13 repaints | exact | — |
| 100 updates at 120Hz decode-to-visible | 411.4 ms | sample | — |
| 100 updates at 120Hz total SRUI wire bytes | 3094 bytes | exact | — |
| 100 updates at 120Hz total SRUI message count | 103 messages | exact | — |
| 100 updates at 120Hz inbound TRANSACTION bytes | 2800 bytes | exact | — |
| 100 updates at 120Hz inbound TRANSACTION message count | 100 messages | exact | — |
| 100 updates at 120Hz outbound EVENT bytes | 294 bytes | exact | — |
| 100 updates at 120Hz outbound EVENT message count | 3 messages | exact | — |
| 100 updates at 120Hz synthetic change-gated repaint count | 24 repaints | exact | — |
| 100 updates at 144Hz decode-to-visible | 412.7 ms | sample | — |
| 100 updates at 144Hz total SRUI wire bytes | 3094 bytes | exact | — |
| 100 updates at 144Hz total SRUI message count | 103 messages | exact | — |
| 100 updates at 144Hz inbound TRANSACTION bytes | 2800 bytes | exact | — |
| 100 updates at 144Hz inbound TRANSACTION message count | 100 messages | exact | — |
| 100 updates at 144Hz outbound EVENT bytes | 294 bytes | exact | — |
| 100 updates at 144Hz outbound EVENT message count | 3 messages | exact | — |
| 100 updates at 144Hz synthetic change-gated repaint count | 26 repaints | exact | — |
| 100 updates at 240Hz decode-to-visible | 594.1 ms | sample | — |
| 100 updates at 240Hz total SRUI wire bytes | 3094 bytes | exact | — |
| 100 updates at 240Hz total SRUI message count | 103 messages | exact | — |
| 100 updates at 240Hz inbound TRANSACTION bytes | 2800 bytes | exact | — |
| 100 updates at 240Hz inbound TRANSACTION message count | 100 messages | exact | — |
| 100 updates at 240Hz outbound EVENT bytes | 294 bytes | exact | — |
| 100 updates at 240Hz outbound EVENT message count | 3 messages | exact | — |
| 100 updates at 240Hz synthetic change-gated repaint count | 47 repaints | exact | — |
| 1000 updates at 60Hz decode-to-visible | 6353 ms | sample | — |
| 1000 updates at 60Hz total SRUI wire bytes | 3.004e+04 bytes | exact | — |
| 1000 updates at 60Hz total SRUI message count | 1003 messages | exact | — |
| 1000 updates at 60Hz inbound TRANSACTION bytes | 2.975e+04 bytes | exact | — |
| 1000 updates at 60Hz inbound TRANSACTION message count | 1000 messages | exact | — |
| 1000 updates at 60Hz outbound EVENT bytes | 297 bytes | exact | — |
| 1000 updates at 60Hz outbound EVENT message count | 3 messages | exact | — |
| 1000 updates at 60Hz synthetic change-gated repaint count | 244 repaints | exact | — |
| 1000 updates at 120Hz decode-to-visible | 6353 ms | sample | — |
| 1000 updates at 120Hz total SRUI wire bytes | 3.004e+04 bytes | exact | — |
| 1000 updates at 120Hz total SRUI message count | 1003 messages | exact | — |
| 1000 updates at 120Hz inbound TRANSACTION bytes | 2.975e+04 bytes | exact | — |
| 1000 updates at 120Hz inbound TRANSACTION message count | 1000 messages | exact | — |
| 1000 updates at 120Hz outbound EVENT bytes | 297 bytes | exact | — |
| 1000 updates at 120Hz outbound EVENT message count | 3 messages | exact | — |
| 1000 updates at 120Hz synthetic change-gated repaint count | 375 repaints | exact | — |
| 1000 updates at 144Hz decode-to-visible | 6370 ms | sample | — |
| 1000 updates at 144Hz total SRUI wire bytes | 3.004e+04 bytes | exact | — |
| 1000 updates at 144Hz total SRUI message count | 1003 messages | exact | — |
| 1000 updates at 144Hz inbound TRANSACTION bytes | 2.975e+04 bytes | exact | — |
| 1000 updates at 144Hz inbound TRANSACTION message count | 1000 messages | exact | — |
| 1000 updates at 144Hz outbound EVENT bytes | 297 bytes | exact | — |
| 1000 updates at 144Hz outbound EVENT message count | 3 messages | exact | — |
| 1000 updates at 144Hz synthetic change-gated repaint count | 404 repaints | exact | — |
| 1000 updates at 240Hz decode-to-visible | 6366 ms | sample | — |
| 1000 updates at 240Hz total SRUI wire bytes | 3.004e+04 bytes | exact | — |
| 1000 updates at 240Hz total SRUI message count | 1003 messages | exact | — |
| 1000 updates at 240Hz inbound TRANSACTION bytes | 2.975e+04 bytes | exact | — |
| 1000 updates at 240Hz inbound TRANSACTION message count | 1000 messages | exact | — |
| 1000 updates at 240Hz outbound EVENT bytes | 297 bytes | exact | — |
| 1000 updates at 240Hz outbound EVENT message count | 3 messages | exact | — |
| 1000 updates at 240Hz synthetic change-gated repaint count | 467 repaints | exact | — |
| settled idle SRUI wire bytes | 0 bytes | observed max | — |
| settled idle SRUI message count | 0 messages | observed max | — |

Assertions:

- **PASS** decode-to-visible samples reach an unforced composited content change — every framed production SessionController sample matched isolated semantic state and changed the exact visible client-content fingerprint without benchmark-forced invalidation; every cadence stream ended in its expected native value with passive composited evidence
- **PASS** settled idle UI emits zero SRUI traffic — 1000ms after all production EVENT ACKs drained for every count/cadence; byte deltas [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], message deltas [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]. This does not claim natural AppKit idle invalidation behavior: the synthetic cadence gate suppresses unchanged-state draws by construction.
- **PASS** complete bidirectional wire bytes and message count are cadence independent — the same prebuilt 1/100/1000 inbound TRANSACTION frame sequences were reused at 60/120/144/240Hz; every outbound transport delta is exactly the three decoded production EVENT frames with no extra, dropped, or interrupted attempts; total, inbound, and outbound bytes/messages are reported separately and compared across cadences; ACKs are injected only after these counters and the presentation timestamp are frozen
- **PASS** synthetic native-change-gated repaint count varies independently — 1@60Hz=1, 1@120Hz=1, 1@144Hz=1, 1@240Hz=1, 100@60Hz=13, 100@120Hz=24, 100@144Hz=26, 100@240Hz=47, 1000@60Hz=244, 1000@120Hz=375, 1000@144Hz=404, 1000@240Hz=467. This diagnostic measures configured coalescing opportunities; it is not evidence of AppKit's natural invalidation count.
- **PASS** committed state and production EVENT order are cadence independent — ACTIVATE, VALUE_CHANGED, and SELECTION_CHANGED were emitted through SessionController/EventOutbox at deterministic early/mid/final applied revisions before those revisions entered the cadence paint gate. Every returned ID/sequence/observed-revision/type/node/argument set exactly matched its captured framed EVENT in emission order, and the typed order signature matched across 60/120/144/240Hz for each update count: 1@60Hz=3 EVENT frames/294B signature=6bec77ea34786e58, 1@120Hz=3 EVENT frames/294B signature=6bec77ea34786e58, 1@144Hz=3 EVENT frames/294B signature=6bec77ea34786e58, 1@240Hz=3 EVENT frames/294B signature=6bec77ea34786e58, 100@60Hz=3 EVENT frames/294B signature=d050587895c53065, 100@120Hz=3 EVENT frames/294B signature=d050587895c53065, 100@144Hz=3 EVENT frames/294B signature=d050587895c53065, 100@240Hz=3 EVENT frames/294B signature=d050587895c53065, 1000@60Hz=3 EVENT frames/297B signature=55ccc044e33c3be5, 1000@120Hz=3 EVENT frames/297B signature=55ccc044e33c3be5, 1000@144Hz=3 EVENT frames/297B signature=55ccc044e33c3be5, 1000@240Hz=3 EVENT frames/297B signature=55ccc044e33c3be5

Notes:

- The updates.*.semantic trial times ProtocolDecoder plus TransactionApplier only. A byte-identical framed aggregate trial traverses BenchmarkTransport and SessionController into the pre-presented warm AppKitRenderer for updates.*.visible; final revision/value parity and exact inbound TRANSACTION bytes/messages are asserted.
- Full decode-to-visible measurements establish a visible baseline before timing and await a passive WindowServer framebuffer/content-fingerprint change after production mutation; the timed path does not set needsDisplay or call the forced presentation observer. Smoke remains an explicitly offscreen AppKit raster fallback.
- For each update count, one pre-encoded TRANSACTION sequence is reused unchanged at 60/120/144/240Hz. Three distinct production client events are interleaved at deterministic applied revisions before the corresponding native state is exposed to the synthetic cadence gate. The complete transport delta is frozen before ACK: inbound, outbound, and total bytes/messages are reported separately; exact outbound EVENT frames cover every outbound index, are decoded, and are compared field-for-field with returned events, proving no unreported frames. ACKs drain outside decode-to-visible timing.
- The cadence repaint number is a synthetic change-gated coalescing diagnostic. Because that task intentionally skips unchanged pending state, it cannot establish natural renderer idle invalidation. The settled 1000ms assertion is limited to zero SRUI byte/message deltas after EVENT settlement.

## §31.4 Network and local interaction

Samples:

- `macos.bandwidth`: 3
- `macos.interaction.caret_movement.rtt.0`: 20
- `macos.interaction.caret_movement.rtt.100`: 20
- `macos.interaction.caret_movement.rtt.300`: 20
- `macos.interaction.caret_movement.rtt.600`: 20
- `macos.interaction.hover.rtt.0`: 20
- `macos.interaction.hover.rtt.100`: 20
- `macos.interaction.hover.rtt.300`: 20
- `macos.interaction.hover.rtt.600`: 20
- `macos.interaction.ime_composition.rtt.0`: 20
- `macos.interaction.ime_composition.rtt.100`: 20
- `macos.interaction.ime_composition.rtt.300`: 20
- `macos.interaction.ime_composition.rtt.600`: 20
- `macos.interaction.menu_opening.rtt.0`: 20
- `macos.interaction.menu_opening.rtt.100`: 20
- `macos.interaction.menu_opening.rtt.300`: 20
- `macos.interaction.menu_opening.rtt.600`: 20
- `macos.interaction.pressed.rtt.0`: 20
- `macos.interaction.pressed.rtt.100`: 20
- `macos.interaction.pressed.rtt.300`: 20
- `macos.interaction.pressed.rtt.600`: 20
- `macos.interaction.scrolling.rtt.0`: 20
- `macos.interaction.scrolling.rtt.100`: 20
- `macos.interaction.scrolling.rtt.300`: 20
- `macos.interaction.scrolling.rtt.600`: 20
- `macos.interaction.text_entry.rtt.0`: 20
- `macos.interaction.text_entry.rtt.100`: 20
- `macos.interaction.text_entry.rtt.300`: 20
- `macos.interaction.text_entry.rtt.600`: 20
- `macos.interaction.text_selection.rtt.0`: 20
- `macos.interaction.text_selection.rtt.100`: 20
- `macos.interaction.text_selection.rtt.300`: 20
- `macos.interaction.text_selection.rtt.600`: 20
- `macos.interruption`: 1
- `macos.local_held_response`: 640
- `macos.loss`: 1
- `macos.server_feedback.rtt.0`: 7
- `macos.server_feedback.rtt.100`: 7
- `macos.server_feedback.rtt.300`: 7
- `macos.server_feedback.rtt.600`: 7

| Metric | Value | Statistic | Target |
|---|---:|---|---:|
| local display frame budget | 8.333 ms | exact | — |
| caret movement at 0ms RTT | 20.67 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| caret movement at 0ms RTT | 21.66 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| caret movement at 0ms RTT | 22.2 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| hover at 0ms RTT | 18.79 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| hover at 0ms RTT | 20.81 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| hover at 0ms RTT | 21.11 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| ime composition at 0ms RTT | 21.29 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| ime composition at 0ms RTT | 22.06 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| ime composition at 0ms RTT | 22.23 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| menu opening at 0ms RTT | 39.87 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| menu opening at 0ms RTT | 51.86 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| menu opening at 0ms RTT | 68.47 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| pressed at 0ms RTT | 20.65 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| pressed at 0ms RTT | 21.47 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| pressed at 0ms RTT | 21.61 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| scrolling at 0ms RTT | 20.76 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| scrolling at 0ms RTT | 21.42 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| scrolling at 0ms RTT | 21.51 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| text entry at 0ms RTT | 20.6 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| text entry at 0ms RTT | 21.52 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| text entry at 0ms RTT | 21.6 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| text selection at 0ms RTT | 19.88 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| text selection at 0ms RTT | 21.48 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| text selection at 0ms RTT | 21.49 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| server-dependent input-to-visible at 0ms RTT | 12.33 ms | p50 | — |
| server-dependent input-to-visible at 0ms RTT | 13.5 ms | p95 | — |
| server-dependent input-to-visible at 0ms RTT | 13.5 ms | p99 | — |
| caret movement at 100ms RTT | 13.23 ms | p50 | ≤ 8.33333 ms |
| caret movement at 100ms RTT | 17.15 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| caret movement at 100ms RTT | 17.25 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| hover at 100ms RTT | 14.71 ms | p50 | ≤ 8.33333 ms |
| hover at 100ms RTT | 18.64 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| hover at 100ms RTT | 19.47 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| ime composition at 100ms RTT | 13.15 ms | p50 | ≤ 8.33333 ms |
| ime composition at 100ms RTT | 16.49 ms | p95 | ≤ 8.33333 ms |
| ime composition at 100ms RTT | 17.13 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| menu opening at 100ms RTT | 42.12 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| menu opening at 100ms RTT | 50.86 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| menu opening at 100ms RTT | 51.13 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| pressed at 100ms RTT | 13.27 ms | p50 | ≤ 8.33333 ms |
| pressed at 100ms RTT | 17.22 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| pressed at 100ms RTT | 18.12 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| scrolling at 100ms RTT | 13.23 ms | p50 | ≤ 8.33333 ms |
| scrolling at 100ms RTT | 18.21 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| scrolling at 100ms RTT | 18.68 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| text entry at 100ms RTT | 12.85 ms | p50 | ≤ 8.33333 ms |
| text entry at 100ms RTT | 16.92 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| text entry at 100ms RTT | 18.64 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| text selection at 100ms RTT | 13.51 ms | p50 | ≤ 8.33333 ms |
| text selection at 100ms RTT | 17.24 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| text selection at 100ms RTT | 17.63 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| server-dependent input-to-visible at 100ms RTT | 120.6 ms | p50 | — |
| server-dependent input-to-visible at 100ms RTT | 122.2 ms | p95 | — |
| server-dependent input-to-visible at 100ms RTT | 122.2 ms | p99 | — |
| caret movement at 300ms RTT | 12.67 ms | p50 | ≤ 8.33333 ms |
| caret movement at 300ms RTT | 16.56 ms | p95 | ≤ 8.33333 ms |
| caret movement at 300ms RTT | 17.42 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| hover at 300ms RTT | 14.65 ms | p50 | ≤ 8.33333 ms |
| hover at 300ms RTT | 17.43 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| hover at 300ms RTT | 18.42 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| ime composition at 300ms RTT | 12.65 ms | p50 | ≤ 8.33333 ms |
| ime composition at 300ms RTT | 16.71 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| ime composition at 300ms RTT | 16.99 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| menu opening at 300ms RTT | 41.28 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| menu opening at 300ms RTT | 52.55 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| menu opening at 300ms RTT | 53.52 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| pressed at 300ms RTT | 12.98 ms | p50 | ≤ 8.33333 ms |
| pressed at 300ms RTT | 16.87 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| pressed at 300ms RTT | 18.83 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| scrolling at 300ms RTT | 13.35 ms | p50 | ≤ 8.33333 ms |
| scrolling at 300ms RTT | 20.03 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| scrolling at 300ms RTT | 20.06 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| text entry at 300ms RTT | 12.92 ms | p50 | ≤ 8.33333 ms |
| text entry at 300ms RTT | 16.3 ms | p95 | ≤ 8.33333 ms |
| text entry at 300ms RTT | 16.71 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| text selection at 300ms RTT | 12.72 ms | p50 | ≤ 8.33333 ms |
| text selection at 300ms RTT | 16.88 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| text selection at 300ms RTT | 17.75 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| server-dependent input-to-visible at 300ms RTT | 328.1 ms | p50 | — |
| server-dependent input-to-visible at 300ms RTT | 339 ms | p95 | — |
| server-dependent input-to-visible at 300ms RTT | 339 ms | p99 | — |
| caret movement at 600ms RTT | 12.6 ms | p50 | ≤ 8.33333 ms |
| caret movement at 600ms RTT | 13.28 ms | p95 | ≤ 8.33333 ms |
| caret movement at 600ms RTT | 13.3 ms | p99 | ≤ 8.33333 ms |
| hover at 600ms RTT | 15.37 ms | p50 | ≤ 8.33333 ms |
| hover at 600ms RTT | 17.36 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| hover at 600ms RTT | 18.8 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| ime composition at 600ms RTT | 13.13 ms | p50 | ≤ 8.33333 ms |
| ime composition at 600ms RTT | 17.87 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| ime composition at 600ms RTT | 18.34 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| menu opening at 600ms RTT | 42.11 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| menu opening at 600ms RTT | 51.56 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| menu opening at 600ms RTT | 53.01 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| pressed at 600ms RTT | 13.2 ms | p50 | ≤ 8.33333 ms |
| pressed at 600ms RTT | 17.06 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| pressed at 600ms RTT | 17.12 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| scrolling at 600ms RTT | 12.86 ms | p50 | ≤ 8.33333 ms |
| scrolling at 600ms RTT | 16.98 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| scrolling at 600ms RTT | 18.34 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| text entry at 600ms RTT | 12.54 ms | p50 | ≤ 8.33333 ms |
| text entry at 600ms RTT | 17.26 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| text entry at 600ms RTT | 17.61 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| text selection at 600ms RTT | 12.96 ms | p50 | ≤ 8.33333 ms |
| text selection at 600ms RTT | 17.01 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| text selection at 600ms RTT | 17.52 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| server-dependent input-to-visible at 600ms RTT | 641.6 ms | p50 | — |
| server-dependent input-to-visible at 600ms RTT | 653.8 ms | p95 | — |
| server-dependent input-to-visible at 600ms RTT | 653.8 ms | p99 | — |
| 1MiB/s bandwidth-limited production event | 17 ms | p50 | — |
| 1MiB/s bandwidth-limited production event | 18.12 ms | p95 | — |
| bandwidth-limited delivered bytes | 4.948e+04 bytes | exact | — |
| deterministic production loss attempts | 2 messages | exact | — |
| deterministic production loss delivered messages | 1 messages | exact | — |
| controlled production interruption detection | 1.277 ms | p50 | — |
| measured production session wire bytes | 1.415e+05 bytes | exact | — |
| measured production session wire messages | 1240 messages | exact | — |
| maximum RTT-induced local latency delta | 1.868 ms | p50 | ≤ 8.33333 ms |
| maximum RTT-induced local latency delta | 12.22 ms | p95 | ≤ 8.33333 ms |
| maximum RTT-induced local latency delta | 14.03 ms | p99 | ≤ 8.33333 ms |

Assertions:

- **PASS** mounted local interactions do not acquire one RTT — largest p50 increase 1.8681 ms versus the measured local frame budget of 8.3333 ms; full compositor mode applies this numeric correctness gate; paired injected transaction remained blocked through local visible completion in 640/640 probes=true; configured delay state was verified at the exact action boundary in 640/640 probes=true, with a nonzero transport delay still active in 480/480 nonzero-RTT probes; local_state_checks=true; paired p95/p99 deltas were 12.2214/14.0343 ms; production renderer callbacks=240
- **PASS** native text entry emits and settles one exact production TEXT_EDIT — RTT 0ms: node=14 final_text="ééééééééééééééééééééview the changes and run tests.xxxxxxxxxxxxxxxxxxxx" edit_seq=40 observed_revision=81 matching_framed_events=1 exact_slot_ack=true stable_empty_tail=true; RTT 100ms: node=14 final_text="ééééééééééééééééééééview the changes and run tests.xxxxxxxxxxxxxxxxxxxx" edit_seq=40 observed_revision=81 matching_framed_events=1 exact_slot_ack=true stable_empty_tail=true; RTT 300ms: node=14 final_text="ééééééééééééééééééééview the changes and run tests.xxxxxxxxxxxxxxxxxxxx" edit_seq=40 observed_revision=80 matching_framed_events=1 exact_slot_ack=true stable_empty_tail=true; RTT 600ms: node=14 final_text="ééééééééééééééééééééview the changes and run tests.xxxxxxxxxxxxxxxxxxxx" edit_seq=40 observed_revision=80 matching_framed_events=1 exact_slot_ack=true stable_empty_tail=true
- **PASS** injected transport RTT affects production server-dependent feedback — SessionController EventOutbox sends and framed transaction responses tracked 100/300/600ms RTT
- **PASS** local visible completion does not await an injected transport response — Each local action began only after its exact compositor baseline was ready and while its configured BenchmarkTransport delay state was verified; 480/480 nonzero-RTT actions began during an active delay. Every paired production transaction remained blocked through visible completion; the gate was released only afterward. Separately, production server-dependent feedback tracked 100/300/600ms RTT.
- **PASS** bandwidth delay, loss, and interruption exercise session recovery — sample 1: 16493 framed bytes / 1 message: measured 16.2880 ms >= theoretical 15.7290 ms; exact_event=true; sample 2: 16493 framed bytes / 1 message: measured 18.1172 ms >= theoretical 15.7290 ms; exact_event=true; sample 3: 16493 framed bytes / 1 message: measured 16.9967 ms >= theoretical 15.7290 ms; exact_event=true; 49479 total bandwidth bytes; lost and interrupted events remained in EventOutbox and replayed through replacement SessionControllers

Notes:

- Controls are mounted renderer TextArea, ScrollView, and Button. the mounted renderer NSTextView supplied its native context menu, which was proven as a new exact owned menu-level WindowServer surface in the same ScreenCaptureKit frame used for its presentation timestamp.
- Pressed feedback uses performClick on the mounted renderer button, accepts its transient action-time composited frame, and triggers the production ActionTrampoline. renderer-produced NSButton changed at least 1590 target-ROI pixels above the explicit 2/255 per-channel SCStream tolerance after deterministic pointer-context injection into production hover reconciliation in 20/20 samples; the outside context then restored every unmasked screenshot pixel within the explicit 5/255 same-API tolerance, with maximum observed channel delta 5/255. Hover injects a deterministic pointer context through benchmark SPI into the production HoverFeedbackButton reconciliation path. These two trials measure SRUI local state-to-visible latency and explicitly exclude OS hardware-event routing latency.
- Local frame budget 8.333333 ms came from CGDisplayMode.refreshRate for the benchmark NSScreen.
- All impairment traffic traverses SessionController, EventOutbox, SRUIFraming, and replacement-session resume/replay; no benchmark calls Transport.send directly.
- For each 1 MiB/s sample, the proof takes the exact outbound framed-byte delta around one awaited SessionController.sendValueChanged call, requires exactly one matching event frame, and requires elapsed wall time >= framed bytes / 1,048,576 bytes/s. Encoding and outbox overhead are inside the measured interval and can only increase that elapsed time.
- In full compositor mode, the RTT-independence correctness gate requires the worst p50 delta to stay within the measured local display-frame budget. Smoke mode reports the offscreen raster delta diagnostically because unrelated remote invalidation can be charged to a later whole-host raster. Both profiles require every one of the 640 exact held-response probes. That probe proves non-dependence on delivery of its exact paired transaction; it does not claim the configured one-way-delay interval remained active throughout the action. Paired p95/p99 delta tails carry the §23 target for follow-up reporting but are not assertion gates.

## §31.5 Reconnect

Samples:

- `macos.active_response`: 5
- `macos.mid_resource`: 5
- `macos.pre_receipt`: 5
- `macos.superseded_response`: 5
- `runner.production_conformance`: 9
- `rust.cached_duplicate`: 500
- `rust.event_side_effect`: 500
- `rust.lost_ack_wire`: 50
- `rust.mid_resource`: 500
- `rust.mid_transaction_codec`: 500
- `rust.mid_transaction_wire`: 50
- `rust.partial_event_wire`: 50
- `rust.pre_receipt_wire`: 50
- `rust.resume_beyond_retention`: 500
- `rust.resume_within_retention`: 500

| Metric | Value | Statistic | Target |
|---|---:|---|---:|
| disconnect immediately before event receipt | 0.06421 ms | p50 | — |
| disconnect immediately before event receipt | 0.1196 ms | p95 | — |
| disconnect immediately before event receipt | 0.1477 ms | p99 | — |
| event receipt through settled side effect | 0.000542 ms | p50 | — |
| event receipt through settled side effect | 0.001834 ms | p95 | — |
| event receipt through settled side effect | 0.007625 ms | p99 | — |
| in-process cached DUPLICATE response | 0.000208 ms | p50 | — |
| in-process cached DUPLICATE response | 0.000459 ms | p95 | — |
| in-process cached DUPLICATE response | 0.000958 ms | p99 | — |
| lost ACK wire reconnect through DUPLICATE acknowledgement | 0.06288 ms | p50 | — |
| lost ACK wire reconnect through DUPLICATE acknowledgement | 0.09504 ms | p95 | — |
| lost ACK wire reconnect through DUPLICATE acknowledgement | 0.1459 ms | p99 | — |
| mid-resource reconnect and exact replay | 0.003666 ms | p50 | — |
| mid-resource reconnect and exact replay | 0.0145 ms | p95 | — |
| mid-resource reconnect and exact replay | 0.0245 ms | p99 | — |
| mid-transaction frame discard and atomic replay | 0.000875 ms | p50 | — |
| mid-transaction frame discard and atomic replay | 0.002875 ms | p95 | — |
| mid-transaction frame discard and atomic replay | 0.009167 ms | p99 | — |
| mid-transaction wire disconnect and exact atomic replay | 0.06242 ms | p50 | — |
| mid-transaction wire disconnect and exact atomic replay | 0.1288 ms | p95 | — |
| mid-transaction wire disconnect and exact atomic replay | 0.1575 ms | p99 | — |
| partial EVENT disconnect and one processed replay | 0.4528 ms | p50 | — |
| partial EVENT disconnect and one processed replay | 0.7001 ms | p95 | — |
| partial EVENT disconnect and one processed replay | 0.7827 ms | p99 | — |
| resume beyond journal retention | 0.000667 ms | p50 | — |
| resume beyond journal retention | 0.001875 ms | p95 | — |
| resume beyond journal retention | 0.002834 ms | p99 | — |
| resume within journal retention | 0.000625 ms | p50 | — |
| resume within journal retention | 0.00175 ms | p95 | — |
| resume within journal retention | 0.002834 ms | p99 | — |
| pre-receipt retained-event replay | 14.46 ms | p50 | — |
| pre-receipt retained-event replay | 28.27 ms | p95 | — |
| mid-resource reconnect recovery | 6.393 ms | p50 | — |
| mid-resource reconnect recovery | 10.82 ms | p95 | — |
| superseded resume response handling | 0.1745 ms | p50 | — |
| superseded resume response handling | 0.2999 ms | p95 | — |
| active resume response handling | 0.2179 ms | p50 | — |
| active resume response handling | 0.2317 ms | p95 | — |
| production reconnect boundary suite | 4.84e+04 ms | wall | — |

Assertions:

- **PASS** mid-resource reconnect restarts production transfer at offset zero — 32805-byte CAS object interrupted after one real chunk and reconstructed exactly
- **PASS** mid-transaction wire disconnect exposes no partial state — 50 capacity-one handle_connection streams decoded no partial frame, left the replica at revision 0, then replayed exactly one atomic revision
- **PASS** pre-receipt and partial EVENT disconnects are inert before one processed replay — 50 pre-receipt and capacity-one partial-frame handle_connection reconnects each dispatched zero events on the interrupted connection, then decoded one Processed acknowledgement and one resulting transaction
- **PASS** lost ACK wire replay is DUPLICATE without a second side effect — 50 duplex reconnects decoded ServerEventAck::Duplicate with cached revision 2; after connection shutdown the handler count, revision, and store remained at one effect
- **PASS** journal retention boundary selects replay versus same-session resync — revision 6 replayed 6→8; revision 0 produced SAME_SESSION snapshot at 8
- **PASS** event retained before server receipt replays with identical identity exactly once — event_allocated_before_disconnect=true, first_transport_delivered_zero_event_bytes=true, outbox_drained_after_ack=true, outbox_retained_before_disconnect=true, outbox_retained_until_exact_slot_ack=true, replacement_replayed_identical_event_once=true
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

- Wire negative proofs use decoded acknowledgements plus exact handler counters, revisions, and store state after bounded connection shutdown; they do not infer absence from short timeouts.
- Superseded resume-attempt inertness is measured against the Task 23 client generation guard in the macOS driver.
- The pre-receipt sample allocates the production EventOutbox identity before a deterministic transport loss delivers zero EVENT frames; a replacement SessionController then replays the exact retained id/sequence/revision and drains it only after an exact-slot acknowledgement.
- The old resume response is inert with respect to active/new wire, event/action identity, semantic state, and lifecycle. Task 23 intentionally reports .superseded, marks the old controller diverged, and closes only its transport; those expected old-lifecycle effects are measured explicitly.

## §31.6 Terminal

Samples:

- `macos.embedded_display`: 20
- `macos.standalone_display`: 20
- `rust.embedded_pty`: 20
- `rust.ring_exhaustion`: 1
- `rust.standalone_pty`: 20

| Metric | Value | Statistic | Target |
|---|---:|---|---:|
| embedded SRUI PTY exact ANSI capture and framing | 9.704 ms | p50 | — |
| embedded SRUI PTY exact ANSI capture and framing | 13.54 ms | p95 | — |
| embedded SRUI PTY exact ANSI capture and framing | 13.61 ms | p99 | — |
| standalone PTY exact ANSI interaction | 9.725 ms | p50 | — |
| standalone PTY exact ANSI interaction | 15.43 ms | p95 | — |
| standalone PTY exact ANSI interaction | 15.51 ms | p99 | — |
| terminal reconnect retention-loss decision | 0.000375 ms | sample | — |
| terminal payload | 6912 bytes | exact | — |
| embedded terminal frame count | 188 messages | p50 | — |
| embedded terminal frame count | 236 messages | p95 | — |
| embedded terminal frame count | 242 messages | p99 | — |
| embedded SRUI Terminal decode-to-visible | 19.58 ms | p50 | — |
| embedded SRUI Terminal decode-to-visible | 24.54 ms | p95 | — |
| embedded SRUI Terminal decode-to-visible | 24.6 ms | p99 | — |
| embedded SRUI Terminal draw-only | 12.34 ms | p50 | — |
| embedded SRUI Terminal draw-only | 23.47 ms | p95 | — |
| embedded SRUI Terminal draw-only | 23.53 ms | p99 | — |
| standalone TerminalSession and TerminalView decode-to-visible | 20.93 ms | p50 | — |
| standalone TerminalSession and TerminalView decode-to-visible | 29.67 ms | p95 | — |
| standalone TerminalSession and TerminalView decode-to-visible | 29.76 ms | p99 | — |
| standalone TerminalView draw-only | 14.17 ms | p50 | — |
| standalone TerminalView draw-only | 28.79 ms | p95 | — |
| standalone TerminalView draw-only | 28.91 ms | p99 | — |
| embedded-to-standalone terminal decode-to-visible | 0.9356 ratio | p50 | — |
| embedded-to-standalone terminal draw-only | 0.8704 ratio | p50 | — |
| client terminal framed envelope | 6923 bytes | exact | — |
| embedded terminal draw completions | 20 frames | exact | — |
| standalone terminal draw completions | 20 frames | exact | — |

Assertions:

- **PASS** standalone and embedded PTYs emit the identical ANSI byte stream — both paths compared all 6912 payload bytes
- **PASS** standalone terminal command exits successfully — 20 bounded child exit statuses checked
- **PASS** embedded terminal reaches exact output and bounded natural EOF — 20 production OutputRing captures reached exact byte offset 6912 and then natural EOF before explicit shutdown
- **PASS** embedded terminal frames preserve exact offsets and bounds — 20 samples aggregated; p50 188 frames, each at most 16384 bytes
- **PASS** reconnect ring-buffer exhaustion maps to RETENTION_LOSS — PTYManager::subscribe returned Resync and catch_up emitted TerminalResyncRequired
- **PASS** framed embedded terminal payload and offsets remain exact — every active SessionController delivery decoded one framed SRUITerminalData carrying exactly 6912 payload bytes at offset zero; the renderer-owned TerminalSession ended at 6912; framed envelope byte counts: [6923]
- **PASS** standalone terminal payload and offsets remain exact — every direct production TerminalSession delivery consumed exactly 6912 payload bytes at offset zero and ended at offset 6912
- **PASS** standalone and embedded terminal visible completions are actual draws — 20/20 embedded and 20/20 standalone TerminalView completions crossed the configured draw-visible boundary and produced content-distinct bitmaps; full mode additionally required nonblank, nonuniform client-only composited captures; embedded visibility provenance: ["screencapturekit_same_complete_frame_target_roi_after_action_screencapturekit_composited_baseline_status_level_25_appkit_occlusion_8192"]; standalone visibility provenance: ["screencapturekit_same_complete_frame_target_roi_after_action_screencapturekit_composited_baseline_status_level_25_appkit_occlusion_8192"]; embedded composited fingerprints: ["abe6263f7c7ea4cca57c73eb4a1fcbac4e884ce9b4826c62cc1927de900fccfb"]; standalone composited fingerprints: ["0090fa497d679e03ca86447e16374b8adf403b69a3dc6338cc567d0d89cf25be", "0c1100a3b09ce9ba73453d9c2666b5480e39d2d7c4af32503f83c48df858a9b2", "1b7b621ec110df7149ede6c9a42881b0a1c742449eb26e4e96d9d8706d83f970", "1c9dc786a1ee1d95b3d9332acb2e8a6a6d87e0e0c6ef30d33d02aa733455217a", "321bb213164defcf3cce24ab73e1661dfa44f0d2526a130af0913f7e8df064e4", "3338827a4eeb417e29bc6cc890de0d4c87135963c51405bd95eb94d7a1d6f620", "3b875e1cb283dbda6c15332798ea8382da0c8fa9f33a38923c12640c091127ad", "3ea346b28e5a93d166d0fd36cdec301cd437161a665138482d64843807fe7661", "61dd92e0e9b0b9755b9a2b8daeb5e7bdae4e0bbe1d719b7fce888fe93098b690", "642144b4d805790864bbe10dc049b960671e72219758e84bbedda26aa0e7704b", "7dc8c8d2173458098ce671c798323305f7b52220e159bc2057fe8dd438829e42", "a1ae551a4f5f9f7030162f58a1e887be16c7ccb8b0270cb34a7e20e16479e6ac", "b3ca480e281bc401c3bc4b57face63a06fab8213846d74d94f6ace97e31c0554", "c2626f31b6dd5c303d09bff6ea331dbad7d2392c5715d1a6817785f8e9078077", "d515fdeff51c72df14254636f2a6425c460e01e90371ce320a80d05358031be6", "ddbc694dcd7036e46fb6f43fd9bf45e9eb7b29f953f3850d98e99a8dd0ba5e7d", "e34b0c9d37208e691ebbb9564ca9221fb717b785bb618bdef93218d9b21eeee1", "f826780acf243fc2d400aa66257e037eae819e6fa4d8efb8e314e60fb8a8834b", "fe674082cffe90c299dc584250485ee770d07725b2871fcbab08ea15fc4dd824"]
- **PASS** standalone and embedded terminal displays render the identical ANSI payload — both paths consumed payload SHA-256 e4461bba73f99c7bc70ba6041b859c3e59446d6c80d1549c25de7bcaf933a910, decoded exact 256-line content SHA-256 e1e85bdbda198ce2b94b317765b18ec90b981d25c0ef567ff83d102f6817fe8d, and produced byte-identical TerminalView rasters; embedded content digests: ["e1e85bdbda198ce2b94b317765b18ec90b981d25c0ef567ff83d102f6817fe8d"]; standalone content digests: ["e1e85bdbda198ce2b94b317765b18ec90b981d25c0ef567ff83d102f6817fe8d"]; embedded bitmap digests: ["e440b40246b19d42137da63cb2cc233d9e18702986ff6269b4f6ac33337dbab5"]; standalone bitmap digests: ["e440b40246b19d42137da63cb2cc233d9e18702986ff6269b4f6ac33337dbab5"]
- **PASS** standalone and production terminal samples begin from fresh parser and view state — embedded samples negotiate org.srui.terminal/1, register namespace 31, and mount through a framed transaction; standalone samples instantiate fresh production TerminalSession and TerminalView pairs; both reproduce exact content SHA-256 e1e85bdbda198ce2b94b317765b18ec90b981d25c0ef567ff83d102f6817fe8d

Notes:

- Both clocks start immediately before PTY allocation and stop when the identical exact ANSI byte count is first observed. Each path then drains to bounded natural EOF and rejects trailing bytes before reporting success. Standalone blocking I/O runs in spawn_blocking behind a five-second watchdog and one nonblocking-reader cleanup path; embedded observes production OutputRing frames and offsets through PTYManager subscriptions. Process teardown occurs after the timed boundary and EOF proof.
- Embedded decode-to-visible starts before a framed 6,912-byte SRUITerminalData enters BenchmarkTransport, then crosses SessionController, the renderer-owned TerminalSession, the renderer-created TerminalView snapshot subscription, and the configured presentation boundary.
- The standalone display baseline feeds the identical Data value at offset zero through fresh production TerminalSession and TerminalView instances, using the embedded view's exact grid and bounds. The reported ratios compare p50 display completion on like-for-like content and geometry.
- Full mode starts an exact TerminalView-ROI ScreenCaptureKit stream and verifies a nonblank, nonuniform baseline before timing. After the production payload action completes, it accepts only a later complete compositor sample whose same-frame ROI fingerprint changed; the reported latency is action-start mach time to that sample's ScreenCaptureKit display time. Exact WindowServer identity, display, client/ROI geometry, and absence of every intersecting visible nonzero-alpha window are checked before and after acceptance. Callback receipt remains metadata rather than the latency clock. Observed embedded provenance: ["screencapturekit_same_complete_frame_target_roi_after_action_screencapturekit_composited_baseline_status_level_25_appkit_occlusion_8192"]; standalone provenance: ["screencapturekit_same_complete_frame_target_roi_after_action_screencapturekit_composited_baseline_status_level_25_appkit_occlusion_8192"]. Smoke uses an explicitly named offscreen bitmap draw and is not presentation evidence.
- The Rust section separately compares exact standalone-versus-embedded PTY byte transport and exercises reconnect ring-buffer exhaustion; this Swift section compares terminal emulator and display completion.

## Follow-up flags

- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 caret movement at 0ms RTT (p50): 20.67 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 caret movement at 0ms RTT (p95): 21.66 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 caret movement at 0ms RTT (p99): 22.2 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 hover at 0ms RTT (p50): 18.79 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 hover at 0ms RTT (p95): 20.81 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 hover at 0ms RTT (p99): 21.11 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 ime composition at 0ms RTT (p50): 21.29 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 ime composition at 0ms RTT (p95): 22.06 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 ime composition at 0ms RTT (p99): 22.23 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 0ms RTT (p50): 39.87 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 0ms RTT (p95): 51.86 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 0ms RTT (p99): 68.47 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 pressed at 0ms RTT (p50): 20.65 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 pressed at 0ms RTT (p95): 21.47 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 pressed at 0ms RTT (p99): 21.61 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 0ms RTT (p50): 20.76 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 0ms RTT (p95): 21.42 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 0ms RTT (p99): 21.51 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text entry at 0ms RTT (p50): 20.6 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text entry at 0ms RTT (p95): 21.52 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text entry at 0ms RTT (p99): 21.6 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text selection at 0ms RTT (p50): 19.88 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text selection at 0ms RTT (p95): 21.48 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text selection at 0ms RTT (p99): 21.49 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 caret movement at 100ms RTT (p95): 17.15 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 caret movement at 100ms RTT (p99): 17.25 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 hover at 100ms RTT (p95): 18.64 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 hover at 100ms RTT (p99): 19.47 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 ime composition at 100ms RTT (p99): 17.13 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 100ms RTT (p50): 42.12 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 100ms RTT (p95): 50.86 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 100ms RTT (p99): 51.13 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 pressed at 100ms RTT (p95): 17.22 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 pressed at 100ms RTT (p99): 18.12 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 100ms RTT (p95): 18.21 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 100ms RTT (p99): 18.68 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text entry at 100ms RTT (p95): 16.92 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text entry at 100ms RTT (p99): 18.64 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text selection at 100ms RTT (p95): 17.24 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text selection at 100ms RTT (p99): 17.63 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 caret movement at 300ms RTT (p99): 17.42 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 hover at 300ms RTT (p95): 17.43 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 hover at 300ms RTT (p99): 18.42 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 ime composition at 300ms RTT (p95): 16.71 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 ime composition at 300ms RTT (p99): 16.99 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 300ms RTT (p50): 41.28 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 300ms RTT (p95): 52.55 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 300ms RTT (p99): 53.52 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 pressed at 300ms RTT (p95): 16.87 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 pressed at 300ms RTT (p99): 18.83 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 300ms RTT (p95): 20.03 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 300ms RTT (p99): 20.06 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text entry at 300ms RTT (p99): 16.71 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text selection at 300ms RTT (p95): 16.88 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text selection at 300ms RTT (p99): 17.75 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 hover at 600ms RTT (p95): 17.36 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 hover at 600ms RTT (p99): 18.8 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 ime composition at 600ms RTT (p95): 17.87 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 ime composition at 600ms RTT (p99): 18.34 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 600ms RTT (p50): 42.11 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 600ms RTT (p95): 51.56 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 600ms RTT (p99): 53.01 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 pressed at 600ms RTT (p95): 17.06 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 pressed at 600ms RTT (p99): 17.12 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 600ms RTT (p95): 16.98 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 600ms RTT (p99): 18.34 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text entry at 600ms RTT (p95): 17.26 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text entry at 600ms RTT (p99): 17.61 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text selection at 600ms RTT (p95): 17.01 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text selection at 600ms RTT (p99): 17.52 ms vs target 8.33333 ms
