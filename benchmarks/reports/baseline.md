# SRUI benchmark report

- Generated: 2026-09-11T17:16:29.285131+00:00
- Profile: full
- Host: macOS-26.4.1-arm64-arm-64bit-Mach-O / arm64
- Chip: Apple M2 Max
- Physical RAM: 34359738368 bytes
- Xcode: Xcode 26.2 / Build version 17C52
- Swift: Apple Swift version 6.2.3 (swiftlang-6.2.3.3.21 clang-1700.6.3.2) / Target: arm64-apple-macosx26.0 / swift-driver version: 1.127.14.1
- Rust: rustc 1.98.0 (88d9e12ae 2026-08-18)
- Python: 3.13.7
- Git: 1dea52080593cc1ed58d3cbdd404efc0976c5e6d (clean)
- Fixture: benchmarks/fixtures/coding-agent-ui.json
- Metric contract: schema 1 / sha256 98c8f350c9fd4afbf5ad13f2d7d5917b77e1d66db52b917cc8180e84f52ccbac

## §31.1 Local renderer

Samples:

- `macos.srui.render`: 20
- `macos.webkit.render`: 20

| Metric | Value | Statistic | Target |
|---|---:|---|---:|
| SRUI first on-screen paint crossing display refresh | 27.52 ms | p50 | — |
| SRUI first on-screen paint crossing display refresh | 27.8 ms | p95 | — |
| SRUI complete on-screen paint crossing display refresh | 78.45 ms | p50 | — |
| SRUI complete on-screen paint crossing display refresh | 120.2 ms | p95 | — |
| SRUI complete on-screen paint crossing display refresh | 161 ms | p99 | — |
| SRUI candidate process CPU time | 0.7338 ms | p50 | — |
| SRUI candidate process CPU time | 0.8287 ms | p95 | — |
| SRUI host-process net live allocation block delta | 1.944e+04 blocks | p50 | — |
| SRUI host-process net live allocation block delta | 1.974e+04 blocks | p95 | — |
| SRUI host-process net live allocation block delta | 1.98e+04 blocks | p99 | — |
| SRUI host-process net live allocation byte delta | 1.86e+06 bytes | p50 | — |
| SRUI host-process net live allocation byte delta | 1.876e+06 bytes | p95 | — |
| SRUI host-process net live allocation byte delta | 1.997e+06 bytes | p99 | — |
| SRUI host allocated footprint growth | 0.75 MiB | p50 | — |
| SRUI maximum concurrently sampled process footprint | 44.03 MiB | max | — |
| WKWebView first on-screen paint crossing display refresh | 145.1 ms | p50 | — |
| WKWebView first on-screen paint crossing display refresh | 187.3 ms | p95 | — |
| WKWebView complete on-screen paint crossing display refresh | 145 ms | p50 | — |
| WKWebView complete on-screen paint crossing display refresh | 203.2 ms | p95 | — |
| WKWebView host plus attributed helper CPU time | 0.5529 ms | p50 | — |
| WKWebView comparison host-process net live allocation block delta | 60 blocks | p50 | — |
| WKWebView comparison host-process net live allocation block delta | 343 blocks | p95 | — |
| WKWebView comparison host-process net live allocation block delta | 4067 blocks | p99 | — |
| WKWebView comparison host-process net live allocation byte delta | 2.408e+04 bytes | p50 | — |
| WKWebView comparison host-process net live allocation byte delta | 5.037e+04 bytes | p95 | — |
| WKWebView comparison host-process net live allocation byte delta | 3.229e+05 bytes | p99 | — |
| WKWebView host plus helpers allocated footprint growth | 0.125 MiB | p50 | — |
| WKWebView maximum concurrently sampled host-plus-helper footprint | 172.2 MiB | max | — |
| SRUI representation | 910 bytes | exact | — |
| HTML representation | 4420 bytes | exact | — |
| screen capture authorization | 1 boolean | exact | — |

Assertions:

- **PASS** representative fixture preserves exact parent, type, and property semantics — expected=20, native=20 semantic=true controls=true, WebKit=20 semantic=true elements-and-properties=true
- **PASS** candidate production state reaches a verified composited target-pixel frame — native presentations=40/40, pixels=40, authorized=true, content=20/20 samples passed; sample 19: full composited client-content proof: first=d8727085c87f795c complete=5b1884aeb62feef4 dimensions=958x718/958x718 unmasked=687844/687844 quantized-colors=308/88 non-dominant=158730/60918 nonblank-and-nonuniform=true material-pixels=687327/8 max-channel-delta=243/255 tolerance=2/255 materially-distinct=true same-geometry-and-normalization=true provenance=screencapturekit_first_complete_target_frame_status_level_25_appkit_inactive/screencapturekit_first_complete_target_frame_status_level_25_appkit_inactive; shared candidate lifecycle: srui/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | srui/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted; WebKit presentations=40/40, pixels=40, authorized=true, content=20/20 samples passed; sample 19: full composited client-content proof: first=83230ab95746e2bb complete=3c8f9b5fa15e08c8 dimensions=958x718/958x718 unmasked=687844/687844 quantized-colors=32/32 non-dominant=2659/17688 nonblank-and-nonuniform=true material-pixels=15847/8 max-channel-delta=255/255 tolerance=2/255 materially-distinct=true same-geometry-and-normalization=true provenance=screencapturekit_first_complete_target_frame_status_level_25_appkit_inactive/screencapturekit_first_complete_target_frame_status_level_25_appkit_inactive; shared candidate lifecycle: webkit/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/first_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted | webkit/complete_paint:warm_completed->reset_completed->timing_started->host_attached->representation_ingested->display_submitted; four disjoint passes per sample: the shared candidate lifecycle warms and resets before timing; host attachment, prebuilt protobuf ingestion, and display submission occur after the common timestamp; the accepted ScreenCaptureKit frame displayTime ends each visual interval; CPU/host-net-live-allocation/footprint-growth and peak use separate passes; four disjoint passes per sample: the shared candidate lifecycle warms and resets before timing; measured NSWindow creation and attachment, prebuilt HTML-byte ingestion, and display submission occur after the common timestamp; the accepted ScreenCaptureKit frame displayTime ends each visual interval; CPU/host-net-live-allocation/footprint-growth and peak use separate measured-host passes
- **PASS** WebKit helper resources use exact measured process attribution — host 48500, helpers [48502, 48503, 48504, 48559, 48560, 48613, 48614, 48643, 48647, 48676, 48677, 48728, 48729, 48758, 48759, 48785, 48786, 48814, 48815, 48880, 48881, 48888, 48889, 48918, 48919, 48973, 48974, 49002, 49004, 49032, 49033, 49072, 49098, 49124, 49125, 49157, 49158, 49185, 49186, 49237, 49238]; no process-name matching
- **PASS** signed net live allocation samples have exact host-process scope — SRUI blocks=20, bytes=20, scope=default malloc zone in the SRUI renderer host process only; signed after-minus-before net live state, not cumulative allocation events; WebKit control blocks=20, bytes=20, scope=default malloc zone in the WebKit comparison host process only; excludes WebContent, Network, and GPU helper processes; signed after-minus-before net live state, not cumulative allocation events
- **PASS** WindowServer isolation rejects an exact synthetic occluder — window isolation self-test passed: dock=20 status=25 ahead=26 popup=101 target=466015 occluder=466016
- **PASS** renderer candidate launch failures cannot leak a child process — forced candidate identity failure terminated the exact candidate process group and proved both the child and a real descendant gone; a forced descendant-identity publication failure and a forced internal candidate failure also proved both exact identities gone before the benchmark driver returned

Notes:

- Every accepted full-paint state requires an exact visible CGWindow, exact client/target geometry, unobscured z-order, and authorized nonblank/nonuniform ScreenCaptureKit pixels from one complete frame. Latency is action-start Mach time through that accepted frame's SCStream displayTime; callback receipt and pixel hashing are verifier metadata, not the presentation timestamp. The two reported timed states per logical sample produced native=40 and WebKit=40 verified captures; capture_authorization=true. Warm/resource/peak passes are not counted in that metric. No permission request is issued. Full renderer candidates use the benchmark-only accessory activation policy plus canJoinAllSpaces/canJoinAllApplications; those settings are not evidence—the exact on-screen WindowServer entry and accepted composited pixels are.
- Each native logical sample uses four separately warmed/reset renderer instances: first-state visual latency, complete two-transaction visual latency, CPU/host-net-live-allocation/footprint-growth through production display submission, and sampler-only peak footprint. Each WebKit sample resets one warmed view between the same four disjoint workloads while preserving exact helper PID identities. Neither resource pass creates ScreenCaptureKit buffers, and the footprint sampler never runs in the CPU/allocation pass.
- Native first timing decodes and applies revision 0→1 through production initial attach. Native complete timing decodes/applies 0→1 and then applies 1→2 through production incremental apply. WebKit uses structurally equivalent first and complete DOM states. The measured instance is checked immediately after each accepted presentation for exact semantic/control or DOM state. Full mode additionally requires nonblank, nonuniform, equal-geometry client-content fingerprints that differ between first and complete states.
- The shared candidate lifecycle requires host attach and representation ingest to fall inside the timed window for both candidates, so neither can hoist window construction out of its own measurement. The two readiness proofs are not equally cheap, and that asymmetry is inside the numbers: the native path applies its transaction and inspects state in process, while the WebKit path proves its DOM is ingested with evaluateJavaScript round trips to the WebContent process. After each failed readiness probe it yields and waits 2 ms before trying again, so the true probe interval also includes the cross-process round trip. That harness cost has no native counterpart, so the reported WebKit latency is an upper bound on WebKit's disadvantage rather than a measurement of WebKit rendering alone.
- Each candidate process group is birth-identity verified. WebKit CPU, footprint growth, and peak footprint aggregate the host with exact benchmark-only WebContent/network/GPU diagnostic PIDs. Allocation samples are signed default-zone malloc_zone_statistics after-minus-before deltas: blocks_in_use and size_in_use describe net live state, not cumulative allocation traffic. Allocation scope: default malloc zone in the SRUI renderer host process only; signed after-minus-before net live state, not cumulative allocation events. Control scope: default malloc zone in the WebKit comparison host process only; excludes WebContent, Network, and GPU helper processes; signed after-minus-before net live state, not cumulative allocation events. Peak footprint is the maximum simultaneous current-footprint sample at 1 ms cadence strictly inside its separate representative pass, not a sum of per-process lifetime maxima.
- Task 34 deliberately defers cumulative allocation-call and requested-byte counts. The supported malloc_history all-events export was rejected after the first real pre-workload SRUI snapshot expanded to 1,902,439,272 bytes; no value from that attempt entered this report. GitHub issue aizlabs/srui#48 tracks a benchmark-only Darwin allocator-interposition counter.
- The warmed WKWebView candidate and its host-only allocator deltas are comparison controls only; they are not the production SRUI renderer, do not cover WebKit helper-process allocations, and do not describe SRUI's native AppKit rendering path.

## §31.2 Serialization

Samples:

- `rust.generation`: 500
- `rust.serialization`: 500

| Metric | Value | Statistic | Target |
|---|---:|---|---:|
| abstract state generation | 0.00125 ms | p50 | — |
| abstract state generation | 0.001625 ms | p95 | — |
| abstract state generation | 0.003583 ms | p99 | — |
| protobuf serialization | 0.00275 ms | p50 | — |
| protobuf serialization | 0.003458 ms | p95 | — |
| protobuf serialization | 0.004583 ms | p99 | — |
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
| 1 updates semantic decode/apply | 0.01517 ms | p50 | — |
| 1 updates semantic decode/apply | 0.01879 ms | p95 | — |
| 1 updates semantic decode/apply | 0.09058 ms | p99 | — |
| 1 updates decode-to-visible | 19.32 ms | p50 | — |
| 1 updates decode-to-visible | 19.84 ms | p95 | — |
| 1 updates decode-to-visible | 19.9 ms | p99 | — |
| 1 updates wire bytes | 28 bytes | exact | — |
| 1 updates message count | 1 messages | exact | — |
| 100 updates semantic decode/apply | 0.1046 ms | p50 | ≤ 1 ms |
| 100 updates semantic decode/apply | 0.1145 ms | p95 | — |
| 100 updates semantic decode/apply | 0.1166 ms | p99 | — |
| 100 updates decode-to-visible | 19.09 ms | p50 | — |
| 100 updates decode-to-visible | 19.85 ms | p95 | — |
| 100 updates decode-to-visible | 19.88 ms | p99 | — |
| 100 updates wire bytes | 2109 bytes | exact | — |
| 100 updates message count | 1 messages | exact | — |
| 1000 updates semantic decode/apply | 0.8919 ms | p50 | ≤ 5 ms |
| 1000 updates semantic decode/apply | 1.649 ms | p95 | — |
| 1000 updates semantic decode/apply | 1.655 ms | p99 | — |
| 1000 updates decode-to-visible | 18.72 ms | p50 | — |
| 1000 updates decode-to-visible | 27.19 ms | p95 | — |
| 1000 updates decode-to-visible | 34.66 ms | p99 | — |
| 1000 updates wire bytes | 2.101e+04 bytes | exact | — |
| 1000 updates message count | 1 messages | exact | — |
| 1 updates at 60Hz decode-to-visible | 44.53 ms | sample | — |
| 1 updates at 60Hz total SRUI wire bytes | 322 bytes | exact | — |
| 1 updates at 60Hz total SRUI message count | 4 messages | exact | — |
| 1 updates at 60Hz inbound TRANSACTION bytes | 28 bytes | exact | — |
| 1 updates at 60Hz inbound TRANSACTION message count | 1 messages | exact | — |
| 1 updates at 60Hz outbound EVENT bytes | 294 bytes | exact | — |
| 1 updates at 60Hz outbound EVENT message count | 3 messages | exact | — |
| 1 updates at 60Hz synthetic change-gated repaint count | 1 repaints | exact | — |
| 1 updates at 120Hz decode-to-visible | 33.93 ms | sample | — |
| 1 updates at 120Hz total SRUI wire bytes | 322 bytes | exact | — |
| 1 updates at 120Hz total SRUI message count | 4 messages | exact | — |
| 1 updates at 120Hz inbound TRANSACTION bytes | 28 bytes | exact | — |
| 1 updates at 120Hz inbound TRANSACTION message count | 1 messages | exact | — |
| 1 updates at 120Hz outbound EVENT bytes | 294 bytes | exact | — |
| 1 updates at 120Hz outbound EVENT message count | 3 messages | exact | — |
| 1 updates at 120Hz synthetic change-gated repaint count | 1 repaints | exact | — |
| 1 updates at 144Hz decode-to-visible | 27.16 ms | sample | — |
| 1 updates at 144Hz total SRUI wire bytes | 322 bytes | exact | — |
| 1 updates at 144Hz total SRUI message count | 4 messages | exact | — |
| 1 updates at 144Hz inbound TRANSACTION bytes | 28 bytes | exact | — |
| 1 updates at 144Hz inbound TRANSACTION message count | 1 messages | exact | — |
| 1 updates at 144Hz outbound EVENT bytes | 294 bytes | exact | — |
| 1 updates at 144Hz outbound EVENT message count | 3 messages | exact | — |
| 1 updates at 144Hz synthetic change-gated repaint count | 1 repaints | exact | — |
| 1 updates at 240Hz decode-to-visible | 24.34 ms | sample | — |
| 1 updates at 240Hz total SRUI wire bytes | 322 bytes | exact | — |
| 1 updates at 240Hz total SRUI message count | 4 messages | exact | — |
| 1 updates at 240Hz inbound TRANSACTION bytes | 28 bytes | exact | — |
| 1 updates at 240Hz inbound TRANSACTION message count | 1 messages | exact | — |
| 1 updates at 240Hz outbound EVENT bytes | 294 bytes | exact | — |
| 1 updates at 240Hz outbound EVENT message count | 3 messages | exact | — |
| 1 updates at 240Hz synthetic change-gated repaint count | 1 repaints | exact | — |
| 100 updates at 60Hz decode-to-visible | 442.2 ms | sample | — |
| 100 updates at 60Hz total SRUI wire bytes | 3094 bytes | exact | — |
| 100 updates at 60Hz total SRUI message count | 103 messages | exact | — |
| 100 updates at 60Hz inbound TRANSACTION bytes | 2800 bytes | exact | — |
| 100 updates at 60Hz inbound TRANSACTION message count | 100 messages | exact | — |
| 100 updates at 60Hz outbound EVENT bytes | 294 bytes | exact | — |
| 100 updates at 60Hz outbound EVENT message count | 3 messages | exact | — |
| 100 updates at 60Hz synthetic change-gated repaint count | 14 repaints | exact | — |
| 100 updates at 120Hz decode-to-visible | 588.3 ms | sample | — |
| 100 updates at 120Hz total SRUI wire bytes | 3094 bytes | exact | — |
| 100 updates at 120Hz total SRUI message count | 103 messages | exact | — |
| 100 updates at 120Hz inbound TRANSACTION bytes | 2800 bytes | exact | — |
| 100 updates at 120Hz inbound TRANSACTION message count | 100 messages | exact | — |
| 100 updates at 120Hz outbound EVENT bytes | 294 bytes | exact | — |
| 100 updates at 120Hz outbound EVENT message count | 3 messages | exact | — |
| 100 updates at 120Hz synthetic change-gated repaint count | 25 repaints | exact | — |
| 100 updates at 144Hz decode-to-visible | 691.2 ms | sample | — |
| 100 updates at 144Hz total SRUI wire bytes | 3094 bytes | exact | — |
| 100 updates at 144Hz total SRUI message count | 103 messages | exact | — |
| 100 updates at 144Hz inbound TRANSACTION bytes | 2800 bytes | exact | — |
| 100 updates at 144Hz inbound TRANSACTION message count | 100 messages | exact | — |
| 100 updates at 144Hz outbound EVENT bytes | 294 bytes | exact | — |
| 100 updates at 144Hz outbound EVENT message count | 3 messages | exact | — |
| 100 updates at 144Hz synthetic change-gated repaint count | 30 repaints | exact | — |
| 100 updates at 240Hz decode-to-visible | 996 ms | sample | — |
| 100 updates at 240Hz total SRUI wire bytes | 3094 bytes | exact | — |
| 100 updates at 240Hz total SRUI message count | 103 messages | exact | — |
| 100 updates at 240Hz inbound TRANSACTION bytes | 2800 bytes | exact | — |
| 100 updates at 240Hz inbound TRANSACTION message count | 100 messages | exact | — |
| 100 updates at 240Hz outbound EVENT bytes | 294 bytes | exact | — |
| 100 updates at 240Hz outbound EVENT message count | 3 messages | exact | — |
| 100 updates at 240Hz synthetic change-gated repaint count | 49 repaints | exact | — |
| 1000 updates at 60Hz decode-to-visible | 6374 ms | sample | — |
| 1000 updates at 60Hz total SRUI wire bytes | 3.004e+04 bytes | exact | — |
| 1000 updates at 60Hz total SRUI message count | 1003 messages | exact | — |
| 1000 updates at 60Hz inbound TRANSACTION bytes | 2.975e+04 bytes | exact | — |
| 1000 updates at 60Hz inbound TRANSACTION message count | 1000 messages | exact | — |
| 1000 updates at 60Hz outbound EVENT bytes | 297 bytes | exact | — |
| 1000 updates at 60Hz outbound EVENT message count | 3 messages | exact | — |
| 1000 updates at 60Hz synthetic change-gated repaint count | 188 repaints | exact | — |
| 1000 updates at 120Hz decode-to-visible | 8792 ms | sample | — |
| 1000 updates at 120Hz total SRUI wire bytes | 3.004e+04 bytes | exact | — |
| 1000 updates at 120Hz total SRUI message count | 1003 messages | exact | — |
| 1000 updates at 120Hz inbound TRANSACTION bytes | 2.975e+04 bytes | exact | — |
| 1000 updates at 120Hz inbound TRANSACTION message count | 1000 messages | exact | — |
| 1000 updates at 120Hz outbound EVENT bytes | 297 bytes | exact | — |
| 1000 updates at 120Hz outbound EVENT message count | 3 messages | exact | — |
| 1000 updates at 120Hz synthetic change-gated repaint count | 294 repaints | exact | — |
| 1000 updates at 144Hz decode-to-visible | 1.281e+04 ms | sample | — |
| 1000 updates at 144Hz total SRUI wire bytes | 3.004e+04 bytes | exact | — |
| 1000 updates at 144Hz total SRUI message count | 1003 messages | exact | — |
| 1000 updates at 144Hz inbound TRANSACTION bytes | 2.975e+04 bytes | exact | — |
| 1000 updates at 144Hz inbound TRANSACTION message count | 1000 messages | exact | — |
| 1000 updates at 144Hz outbound EVENT bytes | 297 bytes | exact | — |
| 1000 updates at 144Hz outbound EVENT message count | 3 messages | exact | — |
| 1000 updates at 144Hz synthetic change-gated repaint count | 416 repaints | exact | — |
| 1000 updates at 240Hz decode-to-visible | 1.434e+04 ms | sample | — |
| 1000 updates at 240Hz total SRUI wire bytes | 3.004e+04 bytes | exact | — |
| 1000 updates at 240Hz total SRUI message count | 1003 messages | exact | — |
| 1000 updates at 240Hz inbound TRANSACTION bytes | 2.975e+04 bytes | exact | — |
| 1000 updates at 240Hz inbound TRANSACTION message count | 1000 messages | exact | — |
| 1000 updates at 240Hz outbound EVENT bytes | 297 bytes | exact | — |
| 1000 updates at 240Hz outbound EVENT message count | 3 messages | exact | — |
| 1000 updates at 240Hz synthetic change-gated repaint count | 528 repaints | exact | — |
| settled idle SRUI wire bytes | 0 bytes | observed max | — |
| settled idle SRUI message count | 0 messages | observed max | — |

Assertions:

- **PASS** decode-to-visible samples reach an unforced composited content change — every framed production SessionController sample matched isolated semantic state and changed the exact visible client-content fingerprint without benchmark-forced invalidation; every cadence stream ended in its expected native value with passive composited evidence
- **PASS** settled idle UI emits zero SRUI traffic — 1000ms after all production EVENT ACKs drained for every count/cadence; byte deltas [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], message deltas [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]. This does not claim natural AppKit idle invalidation behavior: the synthetic cadence gate suppresses unchanged-state draws by construction.
- **PASS** complete bidirectional wire bytes and message count are cadence independent — the same prebuilt 1/100/1000 inbound TRANSACTION frame sequences were reused at 60/120/144/240Hz; every outbound transport delta is exactly the three decoded production EVENT frames with no extra, dropped, or interrupted attempts; total, inbound, and outbound bytes/messages are reported separately and compared across cadences; ACKs are injected only after these counters and the presentation timestamp are frozen
- **PASS** synthetic native-change-gated repaint count varies independently — 1@60Hz=1, 1@120Hz=1, 1@144Hz=1, 1@240Hz=1, 100@60Hz=14, 100@120Hz=25, 100@144Hz=30, 100@240Hz=49, 1000@60Hz=188, 1000@120Hz=294, 1000@144Hz=416, 1000@240Hz=528. This diagnostic measures configured coalescing opportunities; it is not evidence of AppKit's natural invalidation count.
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
| caret movement at 0ms RTT | 16.4 ms | p50 | ≤ 8.33333 ms |
| caret movement at 0ms RTT | 24.61 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| caret movement at 0ms RTT | 27.54 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| hover at 0ms RTT | 15.77 ms | p50 | ≤ 8.33333 ms |
| hover at 0ms RTT | 20.01 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| hover at 0ms RTT | 20.75 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| ime composition at 0ms RTT | 17.66 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| ime composition at 0ms RTT | 23.57 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| ime composition at 0ms RTT | 24.87 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| menu opening at 0ms RTT | 46.9 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| menu opening at 0ms RTT | 58.48 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| menu opening at 0ms RTT | 140.7 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| pressed at 0ms RTT | 17.97 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| pressed at 0ms RTT | 19.23 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| pressed at 0ms RTT | 19.33 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| scrolling at 0ms RTT | 24.15 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| scrolling at 0ms RTT | 26.15 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| scrolling at 0ms RTT | 26.57 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| text entry at 0ms RTT | 21.86 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| text entry at 0ms RTT | 35.87 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| text entry at 0ms RTT | 37.99 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| text selection at 0ms RTT | 17.86 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| text selection at 0ms RTT | 26.69 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| text selection at 0ms RTT | 27.37 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| server-dependent input-to-visible at 0ms RTT | 11.05 ms | p50 | — |
| server-dependent input-to-visible at 0ms RTT | 18.88 ms | p95 | — |
| server-dependent input-to-visible at 0ms RTT | 18.88 ms | p99 | — |
| caret movement at 100ms RTT | 18.89 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| caret movement at 100ms RTT | 20.03 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| caret movement at 100ms RTT | 27.12 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| hover at 100ms RTT | 18.86 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| hover at 100ms RTT | 21.35 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| hover at 100ms RTT | 25.53 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| ime composition at 100ms RTT | 19.25 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| ime composition at 100ms RTT | 19.66 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| ime composition at 100ms RTT | 19.73 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| menu opening at 100ms RTT | 38.43 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| menu opening at 100ms RTT | 64.08 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| menu opening at 100ms RTT | 64.24 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| pressed at 100ms RTT | 19.36 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| pressed at 100ms RTT | 19.78 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| pressed at 100ms RTT | 20.02 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| scrolling at 100ms RTT | 19.44 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| scrolling at 100ms RTT | 27.2 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| scrolling at 100ms RTT | 27.43 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| text entry at 100ms RTT | 18.8 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| text entry at 100ms RTT | 19.57 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| text entry at 100ms RTT | 19.63 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| text selection at 100ms RTT | 19.26 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| text selection at 100ms RTT | 19.62 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| text selection at 100ms RTT | 19.84 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| server-dependent input-to-visible at 100ms RTT | 127.3 ms | p50 | — |
| server-dependent input-to-visible at 100ms RTT | 127.9 ms | p95 | — |
| server-dependent input-to-visible at 100ms RTT | 127.9 ms | p99 | — |
| caret movement at 300ms RTT | 11.04 ms | p50 | ≤ 8.33333 ms |
| caret movement at 300ms RTT | 17.25 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| caret movement at 300ms RTT | 17.51 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| hover at 300ms RTT | 14.03 ms | p50 | ≤ 8.33333 ms |
| hover at 300ms RTT | 19.74 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| hover at 300ms RTT | 21.09 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| ime composition at 300ms RTT | 16.49 ms | p50 | ≤ 8.33333 ms |
| ime composition at 300ms RTT | 18.26 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| ime composition at 300ms RTT | 18.48 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| menu opening at 300ms RTT | 45.44 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| menu opening at 300ms RTT | 61.64 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| menu opening at 300ms RTT | 155.9 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| pressed at 300ms RTT | 17.27 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| pressed at 300ms RTT | 19.05 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| pressed at 300ms RTT | 19.07 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| scrolling at 300ms RTT | 18.62 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| scrolling at 300ms RTT | 19.43 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| scrolling at 300ms RTT | 19.61 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| text entry at 300ms RTT | 18.75 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| text entry at 300ms RTT | 19.67 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| text entry at 300ms RTT | 19.98 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| text selection at 300ms RTT | 16.18 ms | p50 | ≤ 8.33333 ms |
| text selection at 300ms RTT | 18.09 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| text selection at 300ms RTT | 18.11 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| server-dependent input-to-visible at 300ms RTT | 327.6 ms | p50 | — |
| server-dependent input-to-visible at 300ms RTT | 335.3 ms | p95 | — |
| server-dependent input-to-visible at 300ms RTT | 335.3 ms | p99 | — |
| caret movement at 600ms RTT | 10.56 ms | p50 | ≤ 8.33333 ms |
| caret movement at 600ms RTT | 16.8 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| caret movement at 600ms RTT | 16.99 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| hover at 600ms RTT | 13.77 ms | p50 | ≤ 8.33333 ms |
| hover at 600ms RTT | 19.06 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| hover at 600ms RTT | 19.43 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| ime composition at 600ms RTT | 10.91 ms | p50 | ≤ 8.33333 ms |
| ime composition at 600ms RTT | 18.54 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| ime composition at 600ms RTT | 18.62 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| menu opening at 600ms RTT | 40.21 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| menu opening at 600ms RTT | 60.54 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| menu opening at 600ms RTT | 64.1 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| pressed at 600ms RTT | 18.64 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| pressed at 600ms RTT | 19.05 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| pressed at 600ms RTT | 19.2 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| scrolling at 600ms RTT | 10.36 ms | p50 | ≤ 8.33333 ms |
| scrolling at 600ms RTT | 17.48 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| scrolling at 600ms RTT | 18.27 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| text entry at 600ms RTT | 11.16 ms | p50 | ≤ 8.33333 ms |
| text entry at 600ms RTT | 18 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| text entry at 600ms RTT | 18.52 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| text selection at 600ms RTT | 11.03 ms | p50 | ≤ 8.33333 ms |
| text selection at 600ms RTT | 11.57 ms | p95 | ≤ 8.33333 ms |
| text selection at 600ms RTT | 16.31 ms | p99 | ≤ 8.33333 ms |
| server-dependent input-to-visible at 600ms RTT | 635.7 ms | p50 | — |
| server-dependent input-to-visible at 600ms RTT | 644 ms | p95 | — |
| server-dependent input-to-visible at 600ms RTT | 644 ms | p99 | — |
| 1MiB/s bandwidth-limited production event | 16.98 ms | p50 | — |
| 1MiB/s bandwidth-limited production event | 17.04 ms | p95 | — |
| bandwidth-limited delivered bytes | 4.948e+04 bytes | exact | — |
| deterministic production loss attempts | 2 messages | exact | — |
| deterministic production loss delivered messages | 1 messages | exact | — |
| controlled production interruption detection | 1.59 ms | p50 | — |
| measured production session wire bytes | 1.415e+05 bytes | exact | — |
| measured production session wire messages | 1240 messages | exact | — |
| maximum RTT-induced local latency delta | 2.903 ms | p50 | ≤ 8.33333 ms |
| maximum RTT-induced local latency delta | 14.23 ms | p95 | ≤ 8.33333 ms |
| maximum RTT-induced local latency delta | 97.39 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |

Assertions:

- **PASS** mounted local interactions do not acquire one RTT — largest p50 increase 2.9034 ms versus the measured local frame budget of 8.3333 ms; full compositor mode applies this numeric correctness gate; paired injected transaction remained blocked through local visible completion in 640/640 probes=true; configured delay state was verified at the exact action boundary in 640/640 probes=true, with a nonzero transport delay still active in 480/480 nonzero-RTT probes; local_state_checks=true; paired p95/p99 deltas were 14.2338/97.3854 ms; production renderer callbacks=240
- **PASS** native text entry emits and settles one exact production TEXT_EDIT — RTT 0ms: node=14 final_text="ééééééééééééééééééééview the changes and run tests.xxxxxxxxxxxxxxxxxxxx" edit_seq=40 observed_revision=81 matching_framed_events=1 exact_slot_ack=true stable_empty_tail=true; RTT 100ms: node=14 final_text="ééééééééééééééééééééview the changes and run tests.xxxxxxxxxxxxxxxxxxxx" edit_seq=40 observed_revision=81 matching_framed_events=1 exact_slot_ack=true stable_empty_tail=true; RTT 300ms: node=14 final_text="ééééééééééééééééééééview the changes and run tests.xxxxxxxxxxxxxxxxxxxx" edit_seq=40 observed_revision=80 matching_framed_events=1 exact_slot_ack=true stable_empty_tail=true; RTT 600ms: node=14 final_text="ééééééééééééééééééééview the changes and run tests.xxxxxxxxxxxxxxxxxxxx" edit_seq=40 observed_revision=80 matching_framed_events=1 exact_slot_ack=true stable_empty_tail=true
- **PASS** injected transport RTT affects production server-dependent feedback — SessionController EventOutbox sends and framed transaction responses tracked 100/300/600ms RTT
- **PASS** local visible completion does not await an injected transport response — Each local action began only after its exact compositor baseline was ready and while its configured BenchmarkTransport delay state was verified; 480/480 nonzero-RTT actions began during an active delay. Every paired production transaction remained blocked through visible completion; the gate was released only afterward. Separately, production server-dependent feedback tracked 100/300/600ms RTT.
- **PASS** bandwidth delay, loss, and interruption exercise session recovery — sample 1: 16493 framed bytes / 1 message: measured 17.0364 ms >= theoretical 15.7290 ms; exact_event=true; sample 2: 16493 framed bytes / 1 message: measured 16.9824 ms >= theoretical 15.7290 ms; exact_event=true; sample 3: 16493 framed bytes / 1 message: measured 16.9623 ms >= theoretical 15.7290 ms; exact_event=true; 49479 total bandwidth bytes; lost and interrupted events remained in EventOutbox and replayed through replacement SessionControllers

Notes:

- Controls are mounted renderer TextArea, ScrollView, and Button. the mounted renderer NSTextView supplied its native context menu, which was proven as a new exact owned menu-level WindowServer surface in the same ScreenCaptureKit frame used for its presentation timestamp.
- The unbundled full benchmark runs with AppKit accessory activation policy so benchmark-only hosts can join the active Space/application set without pretending that foreground activation succeeded. The mounted NSTextView must still become the window first responder with a live input context, and every timed visual transition still requires exact WindowServer and ScreenCaptureKit evidence.
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
| disconnect immediately before event receipt | 0.05667 ms | p50 | — |
| disconnect immediately before event receipt | 0.1157 ms | p95 | — |
| disconnect immediately before event receipt | 0.2413 ms | p99 | — |
| event receipt through settled side effect | 0.000542 ms | p50 | — |
| event receipt through settled side effect | 0.001 ms | p95 | — |
| event receipt through settled side effect | 0.004458 ms | p99 | — |
| in-process cached DUPLICATE response | 0.000208 ms | p50 | — |
| in-process cached DUPLICATE response | 0.000292 ms | p95 | — |
| in-process cached DUPLICATE response | 0.000541 ms | p99 | — |
| lost ACK wire reconnect through DUPLICATE acknowledgement | 0.05338 ms | p50 | — |
| lost ACK wire reconnect through DUPLICATE acknowledgement | 0.092 ms | p95 | — |
| lost ACK wire reconnect through DUPLICATE acknowledgement | 0.0925 ms | p99 | — |
| mid-resource reconnect and exact replay | 0.003792 ms | p50 | — |
| mid-resource reconnect and exact replay | 0.008208 ms | p95 | — |
| mid-resource reconnect and exact replay | 0.02438 ms | p99 | — |
| mid-transaction frame discard and atomic replay | 0.000875 ms | p50 | — |
| mid-transaction frame discard and atomic replay | 0.001583 ms | p95 | — |
| mid-transaction frame discard and atomic replay | 0.004833 ms | p99 | — |
| mid-transaction wire disconnect and exact atomic replay | 0.05946 ms | p50 | — |
| mid-transaction wire disconnect and exact atomic replay | 0.09658 ms | p95 | — |
| mid-transaction wire disconnect and exact atomic replay | 0.1357 ms | p99 | — |
| partial EVENT disconnect and one processed replay | 0.4123 ms | p50 | — |
| partial EVENT disconnect and one processed replay | 0.6676 ms | p95 | — |
| partial EVENT disconnect and one processed replay | 0.8915 ms | p99 | — |
| resume beyond journal retention | 0.000708 ms | p50 | — |
| resume beyond journal retention | 0.000958 ms | p95 | — |
| resume beyond journal retention | 0.002166 ms | p99 | — |
| resume within journal retention | 0.000625 ms | p50 | — |
| resume within journal retention | 0.000875 ms | p95 | — |
| resume within journal retention | 0.0015 ms | p99 | — |
| pre-receipt retained-event replay | 20.56 ms | p50 | — |
| pre-receipt retained-event replay | 20.75 ms | p95 | — |
| mid-resource reconnect recovery | 0.8696 ms | p50 | — |
| mid-resource reconnect recovery | 1.385 ms | p95 | — |
| superseded resume response handling | 0.1289 ms | p50 | — |
| superseded resume response handling | 0.3065 ms | p95 | — |
| active resume response handling | 0.2527 ms | p50 | — |
| active resume response handling | 0.3088 ms | p95 | — |
| production reconnect boundary suite | 3.602e+04 ms | wall | — |

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
| embedded SRUI PTY exact ANSI capture and framing | 9.442 ms | p50 | — |
| embedded SRUI PTY exact ANSI capture and framing | 12.32 ms | p95 | — |
| embedded SRUI PTY exact ANSI capture and framing | 14.2 ms | p99 | — |
| standalone PTY exact ANSI interaction | 9.155 ms | p50 | — |
| standalone PTY exact ANSI interaction | 10.5 ms | p95 | — |
| standalone PTY exact ANSI interaction | 13.46 ms | p99 | — |
| terminal reconnect retention-loss decision | 0.000417 ms | sample | — |
| terminal payload | 6912 bytes | exact | — |
| embedded terminal frame count | 154 messages | p50 | — |
| embedded terminal frame count | 246 messages | p95 | — |
| embedded terminal frame count | 252 messages | p99 | — |
| embedded SRUI Terminal decode-to-visible | 19 ms | p50 | — |
| embedded SRUI Terminal decode-to-visible | 19.85 ms | p95 | — |
| embedded SRUI Terminal decode-to-visible | 26.64 ms | p99 | — |
| embedded SRUI Terminal draw-only | 12.01 ms | p50 | — |
| embedded SRUI Terminal draw-only | 18.18 ms | p95 | — |
| embedded SRUI Terminal draw-only | 18.24 ms | p99 | — |
| standalone TerminalSession and TerminalView decode-to-visible | 18.95 ms | p50 | — |
| standalone TerminalSession and TerminalView decode-to-visible | 19.63 ms | p95 | — |
| standalone TerminalSession and TerminalView decode-to-visible | 19.64 ms | p99 | — |
| standalone TerminalView draw-only | 11.82 ms | p50 | — |
| standalone TerminalView draw-only | 12.71 ms | p95 | — |
| standalone TerminalView draw-only | 12.75 ms | p99 | — |
| embedded-to-standalone terminal decode-to-visible | 1.003 ratio | p50 | — |
| embedded-to-standalone terminal draw-only | 1.016 ratio | p50 | — |
| client terminal framed envelope | 6923 bytes | exact | — |
| embedded terminal draw completions | 20 frames | exact | — |
| standalone terminal draw completions | 20 frames | exact | — |

Assertions:

- **PASS** standalone and embedded PTYs emit the identical ANSI byte stream — both paths compared all 6912 payload bytes
- **PASS** standalone terminal command exits successfully — 20 bounded child exit statuses checked
- **PASS** embedded terminal reaches exact output and bounded natural EOF — 20 production OutputRing captures reached exact byte offset 6912 and then natural EOF before explicit shutdown
- **PASS** embedded terminal frames preserve exact offsets and bounds — 20 samples aggregated; p50 154 frames, each at most 16384 bytes
- **PASS** reconnect ring-buffer exhaustion maps to RETENTION_LOSS — PTYManager::subscribe returned Resync and catch_up emitted TerminalResyncRequired
- **PASS** framed embedded terminal payload and offsets remain exact — every active SessionController delivery decoded one framed SRUITerminalData carrying exactly 6912 payload bytes at offset zero; the renderer-owned TerminalSession ended at 6912; framed envelope byte counts: [6923]
- **PASS** standalone terminal payload and offsets remain exact — every direct production TerminalSession delivery consumed exactly 6912 payload bytes at offset zero and ended at offset 6912
- **PASS** standalone and embedded terminal visible completions are actual draws — 20/20 embedded and 20/20 standalone TerminalView completions crossed the configured draw-visible boundary and produced content-distinct bitmaps; full mode additionally required nonblank, nonuniform client-only composited captures; embedded visibility provenance: ["screencapturekit_same_complete_frame_target_roi_after_action_screencapturekit_composited_baseline_status_level_25_appkit_occlusion_8192"]; standalone visibility provenance: ["screencapturekit_same_complete_frame_target_roi_after_action_screencapturekit_composited_baseline_status_level_25_appkit_occlusion_8192"]; embedded composited fingerprints: ["0dc05dba9000af4570e1bf6a19a6f883d70c1ff158f4b7637614f131d0aaa393", "a1c897d8b65ac0fade800b013c3e6f94d3913c7a45282bdf807c4d29e69f83c7", "ba2dc6e19f34c045808a1a4f9742bf90fdb0ab08725b7eab7e64612a056ed05f"]; standalone composited fingerprints: ["1cf5986ec7901722e7350b92d3b0231ccad75e35867ed23c607fef3564d77e5f", "2922132788342273a5c1a6dde7d9458960872b81a3c9231479c72c7f4ccf0106", "2ff432b9624c460e213353e4f994c52764de2b61feb9c1b28b10060a1c92a3c3", "3eb8e3f082a77045767507bea3679befbf416247a44d6457275f26090a30cd94", "4292db3018525e0f615dd44798cb4a5c6d2f1e21451aebd934d877f0cb1ab4be", "4338970efc306dcee249f23fa7bc0f8cffba8739449bbd7c71a56ba0e5448b2f", "45cbef6594a6f29ca3838cf8c267b8d75a666b6ecc30ebf41cf5b5109931257c", "62d49ae7267521c6f53565eb0a3d1a9b3bd144f886f0e09ee9f5747800d87405", "653420d6cba8d2ab3a5fcce7cfb93b7834aab5c0e4a38177b196642c395f2b07", "6875924888d49829137c6a415eb19453e82edd23791bd0a4ccc38e426b2d0368", "6e11d0b34fe9c4da36bee5f7a276469dc392af1cf41df38cedf1618417e8e064", "75aeeee86a3534d4ac9ad99277a740db1cf0f53ea70d8be7af3312f6f40fe1f8", "913a4da64d7da5bf47a3bc41324c76653ba1e7b5032e62e2b1cf6c77d07241eb", "b98a9705fbb75dc9c477f21e3bcc5a53d628e5caa3a610fb2bade35fbb97f3d7", "c7b386c69d56dc40b26fcc454328c2714aa4cd0b3f8bdf0e8d6e76589598f74f", "c8757bdc623f7cadc6397ed2c37c2c4fecba370fa1716be5843c10758a2b4d86", "e527da809b8e333b4b6cede2fa3801819b44f9983f36588a02ad25939d49fd29"]
- **PASS** standalone and embedded terminal displays render the identical ANSI payload — both paths consumed payload SHA-256 e4461bba73f99c7bc70ba6041b859c3e59446d6c80d1549c25de7bcaf933a910, decoded exact 256-line content SHA-256 e1e85bdbda198ce2b94b317765b18ec90b981d25c0ef567ff83d102f6817fe8d, and produced byte-identical TerminalView rasters; embedded content digests: ["e1e85bdbda198ce2b94b317765b18ec90b981d25c0ef567ff83d102f6817fe8d"]; standalone content digests: ["e1e85bdbda198ce2b94b317765b18ec90b981d25c0ef567ff83d102f6817fe8d"]; embedded bitmap digests: ["e440b40246b19d42137da63cb2cc233d9e18702986ff6269b4f6ac33337dbab5"]; standalone bitmap digests: ["e440b40246b19d42137da63cb2cc233d9e18702986ff6269b4f6ac33337dbab5"]
- **PASS** standalone and production terminal samples begin from fresh parser and view state — embedded samples negotiate org.srui.terminal/1, register namespace 31, and mount through a framed transaction; standalone samples instantiate fresh production TerminalSession and TerminalView pairs; both reproduce exact content SHA-256 e1e85bdbda198ce2b94b317765b18ec90b981d25c0ef567ff83d102f6817fe8d

Notes:

- Both clocks start immediately before PTY allocation and stop when the identical exact ANSI byte count is first observed. Each path then drains to bounded natural EOF and rejects trailing bytes before reporting success. Standalone blocking I/O runs in spawn_blocking behind a five-second watchdog and one nonblocking-reader cleanup path; embedded observes production OutputRing frames and offsets through PTYManager subscriptions. Process teardown occurs after the timed boundary and EOF proof.
- Embedded decode-to-visible starts before a framed 6,912-byte SRUITerminalData enters BenchmarkTransport, then crosses SessionController, the renderer-owned TerminalSession, the renderer-created TerminalView snapshot subscription, and the configured presentation boundary.
- The standalone display baseline feeds the identical Data value at offset zero through fresh production TerminalSession and TerminalView instances, using the embedded view's exact grid and bounds. The reported ratios compare p50 display completion on like-for-like content and geometry.
- Full mode starts an exact TerminalView-ROI ScreenCaptureKit stream and verifies a nonblank, nonuniform baseline before timing. After the production payload action completes, it accepts only a later complete compositor sample whose same-frame ROI fingerprint changed; the reported latency is action-start mach time to that sample's ScreenCaptureKit display time. Exact WindowServer identity, display, client/ROI geometry, and absence of every intersecting visible nonzero-alpha window are checked before and after acceptance. Callback receipt remains metadata rather than the latency clock. Observed embedded provenance: ["screencapturekit_same_complete_frame_target_roi_after_action_screencapturekit_composited_baseline_status_level_25_appkit_occlusion_8192"]; standalone provenance: ["screencapturekit_same_complete_frame_target_roi_after_action_screencapturekit_composited_baseline_status_level_25_appkit_occlusion_8192"]. Smoke uses an explicitly named offscreen bitmap draw and is not presentation evidence.
- The Rust section separately compares exact standalone-versus-embedded PTY byte transport and exercises reconnect ring-buffer exhaustion; this Swift section compares terminal emulator and display completion.

## Follow-up flags

- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 caret movement at 0ms RTT (p95): 24.61 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 caret movement at 0ms RTT (p99): 27.54 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 hover at 0ms RTT (p95): 20.01 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 hover at 0ms RTT (p99): 20.75 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 ime composition at 0ms RTT (p50): 17.66 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 ime composition at 0ms RTT (p95): 23.57 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 ime composition at 0ms RTT (p99): 24.87 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 0ms RTT (p50): 46.9 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 0ms RTT (p95): 58.48 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 0ms RTT (p99): 140.7 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 pressed at 0ms RTT (p50): 17.97 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 pressed at 0ms RTT (p95): 19.23 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 pressed at 0ms RTT (p99): 19.33 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 0ms RTT (p50): 24.15 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 0ms RTT (p95): 26.15 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 0ms RTT (p99): 26.57 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text entry at 0ms RTT (p50): 21.86 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text entry at 0ms RTT (p95): 35.87 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text entry at 0ms RTT (p99): 37.99 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text selection at 0ms RTT (p50): 17.86 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text selection at 0ms RTT (p95): 26.69 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text selection at 0ms RTT (p99): 27.37 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 caret movement at 100ms RTT (p50): 18.89 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 caret movement at 100ms RTT (p95): 20.03 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 caret movement at 100ms RTT (p99): 27.12 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 hover at 100ms RTT (p50): 18.86 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 hover at 100ms RTT (p95): 21.35 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 hover at 100ms RTT (p99): 25.53 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 ime composition at 100ms RTT (p50): 19.25 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 ime composition at 100ms RTT (p95): 19.66 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 ime composition at 100ms RTT (p99): 19.73 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 100ms RTT (p50): 38.43 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 100ms RTT (p95): 64.08 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 100ms RTT (p99): 64.24 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 pressed at 100ms RTT (p50): 19.36 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 pressed at 100ms RTT (p95): 19.78 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 pressed at 100ms RTT (p99): 20.02 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 100ms RTT (p50): 19.44 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 100ms RTT (p95): 27.2 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 100ms RTT (p99): 27.43 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text entry at 100ms RTT (p50): 18.8 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text entry at 100ms RTT (p95): 19.57 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text entry at 100ms RTT (p99): 19.63 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text selection at 100ms RTT (p50): 19.26 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text selection at 100ms RTT (p95): 19.62 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text selection at 100ms RTT (p99): 19.84 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 caret movement at 300ms RTT (p95): 17.25 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 caret movement at 300ms RTT (p99): 17.51 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 hover at 300ms RTT (p95): 19.74 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 hover at 300ms RTT (p99): 21.09 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 ime composition at 300ms RTT (p95): 18.26 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 ime composition at 300ms RTT (p99): 18.48 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 300ms RTT (p50): 45.44 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 300ms RTT (p95): 61.64 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 300ms RTT (p99): 155.9 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 pressed at 300ms RTT (p50): 17.27 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 pressed at 300ms RTT (p95): 19.05 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 pressed at 300ms RTT (p99): 19.07 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 300ms RTT (p50): 18.62 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 300ms RTT (p95): 19.43 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 300ms RTT (p99): 19.61 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text entry at 300ms RTT (p50): 18.75 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text entry at 300ms RTT (p95): 19.67 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text entry at 300ms RTT (p99): 19.98 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text selection at 300ms RTT (p95): 18.09 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text selection at 300ms RTT (p99): 18.11 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 caret movement at 600ms RTT (p95): 16.8 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 caret movement at 600ms RTT (p99): 16.99 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 hover at 600ms RTT (p95): 19.06 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 hover at 600ms RTT (p99): 19.43 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 ime composition at 600ms RTT (p95): 18.54 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 ime composition at 600ms RTT (p99): 18.62 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 600ms RTT (p50): 40.21 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 600ms RTT (p95): 60.54 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 600ms RTT (p99): 64.1 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 pressed at 600ms RTT (p50): 18.64 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 pressed at 600ms RTT (p95): 19.05 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 pressed at 600ms RTT (p99): 19.2 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 600ms RTT (p95): 17.48 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 600ms RTT (p99): 18.27 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text entry at 600ms RTT (p95): 18 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text entry at 600ms RTT (p99): 18.52 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 maximum RTT-induced local latency delta (p99): 97.39 ms vs target 8.33333 ms
