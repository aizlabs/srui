# SRUI benchmark report

- Generated: 2026-09-10T22:02:42.277792+00:00
- Profile: full
- Host: macOS-26.4.1-arm64-arm-64bit-Mach-O / arm64
- Chip: Apple M2 Max
- Physical RAM: 34359738368 bytes
- Xcode: Xcode 26.2 / Build version 17C52
- Swift: Apple Swift version 6.2.3 (swiftlang-6.2.3.3.21 clang-1700.6.3.2) / Target: arm64-apple-macosx26.0 / swift-driver version: 1.127.14.1
- Rust: rustc 1.98.0 (88d9e12ae 2026-08-18)
- Python: 3.13.7
- Git: 35ac2b783b14e2b5edcd880694f0fd43611dcfce (clean)
- Fixture: benchmarks/fixtures/coding-agent-ui.json

## §31.1 Local renderer

Samples:

- `macos.srui.render`: 20
- `macos.webkit.render`: 20

| Metric | Value | Statistic | Target |
|---|---:|---|---:|
| SRUI first on-screen paint crossing display refresh | 51.22 ms | p50 | — |
| SRUI first on-screen paint crossing display refresh | 57.54 ms | p95 | — |
| SRUI complete on-screen paint crossing display refresh | 62.17 ms | p50 | — |
| SRUI complete on-screen paint crossing display refresh | 68.81 ms | p95 | — |
| SRUI complete on-screen paint crossing display refresh | 150.2 ms | p99 | — |
| SRUI candidate process CPU time | 0.9746 ms | p50 | — |
| SRUI candidate process CPU time | 1.015 ms | p95 | — |
| SRUI host-process net live allocation block delta | 2.17e+04 blocks | p50 | — |
| SRUI host-process net live allocation block delta | 2.224e+04 blocks | p95 | — |
| SRUI host-process net live allocation block delta | 2.246e+04 blocks | p99 | — |
| SRUI host-process net live allocation byte delta | 2.153e+06 bytes | p50 | — |
| SRUI host-process net live allocation byte delta | 2.179e+06 bytes | p95 | — |
| SRUI host-process net live allocation byte delta | 2.233e+06 bytes | p99 | — |
| SRUI host allocated footprint growth | 0.9688 MiB | p50 | — |
| SRUI maximum concurrently sampled process footprint | 48.52 MiB | max | — |
| WKWebView first on-screen paint crossing display refresh | 87.5 ms | p50 | — |
| WKWebView first on-screen paint crossing display refresh | 93.55 ms | p95 | — |
| WKWebView complete on-screen paint crossing display refresh | 127.3 ms | p50 | — |
| WKWebView complete on-screen paint crossing display refresh | 177.1 ms | p95 | — |
| WKWebView host plus attributed helper CPU time | 0.2799 ms | p50 | — |
| WKWebView comparison host-process net live allocation block delta | 199 blocks | p50 | — |
| WKWebView comparison host-process net live allocation block delta | 278 blocks | p95 | — |
| WKWebView comparison host-process net live allocation block delta | 278 blocks | p99 | — |
| WKWebView comparison host-process net live allocation byte delta | 3.981e+04 bytes | p50 | — |
| WKWebView comparison host-process net live allocation byte delta | 4.648e+04 bytes | p95 | — |
| WKWebView comparison host-process net live allocation byte delta | 4.747e+04 bytes | p99 | — |
| WKWebView host plus helpers allocated footprint growth | 0.8281 MiB | p50 | — |
| WKWebView maximum concurrently sampled host-plus-helper footprint | 165.3 MiB | max | — |
| SRUI representation | 981 bytes | exact | — |
| HTML representation | 4652 bytes | exact | — |
| screen capture authorization | 1 boolean | exact | — |

Assertions:

- **PASS** representative fixture preserves exact parent, type, and property semantics — expected=21, native=21 semantic=true controls=true, WebKit=21 semantic=true elements-and-properties=true
- **PASS** candidate production state reaches a verified composited target-pixel frame — native presentations=40/40, pixels=40, authorized=true, content=20/20 samples passed; sample 19: full composited client-content proof: first=674c2cff013db0e3 complete=edeb4b4f35cd413f dimensions=958x718/958x718 unmasked=687844/687844 quantized-colors=100/111 non-dominant=14209/68720 nonblank-and-nonuniform=true material-pixels=69221/8 max-channel-delta=220/255 tolerance=2/255 materially-distinct=true same-geometry-and-normalization=true provenance=screencapturekit_first_complete_target_frame_status_level_25_appkit_active/screencapturekit_first_complete_target_frame_status_level_25_appkit_active; WebKit presentations=40/40, pixels=40, authorized=true, content=20/20 samples passed; sample 19: full composited client-content proof: first=b51e9bdedeb54f1f complete=8c5e6c10fb0d8819 dimensions=958x718/958x718 unmasked=687844/687844 quantized-colors=32/32 non-dominant=2661/19242 nonblank-and-nonuniform=true material-pixels=17400/8 max-channel-delta=255/255 tolerance=2/255 materially-distinct=true same-geometry-and-normalization=true provenance=screencapturekit_first_complete_target_frame_status_level_25_appkit_inactive/screencapturekit_first_complete_target_frame_status_level_25_appkit_inactive; four disjoint passes per sample: hidden/offscreen warm and reset precede first-state and complete-sequence visual passes; each visual interval begins immediately before production protobuf decode/apply/render and ends at the accepted ScreenCaptureKit frame displayTime; CPU/host-net-live-allocation/footprint-growth use a separate production decode/apply/display-submission interval with no ScreenCaptureKit; peak footprint uses a sampler-only production decode/apply/display-submission pass; four disjoint passes per sample: hidden/offscreen warm and reset precede first-state and complete-sequence visual passes; each visual interval begins immediately before the corresponding hidden WebKit load sequence and ends at the accepted ScreenCaptureKit frame displayTime; CPU/host-net-live-allocation/footprint-growth use a separate hidden-load plus display-submission interval with no ScreenCaptureKit; peak footprint uses a sampler-only hidden-load plus display-submission pass
- **PASS** WebKit helper resources use exact measured process attribution — host 941, helpers [954, 955, 964, 1047, 1051, 1107, 1109, 1142, 1143, 1169, 1170, 1197, 1198, 1225, 1226, 1253, 1254, 1283, 1284, 1315, 1316, 1343, 1344, 1370, 1371, 1397, 1398, 1426, 1427, 1454, 1455, 1483, 1484, 1511, 1512, 1542, 1543, 1570, 1571, 1601, 1602]; no process-name matching
- **PASS** signed net live allocation samples have exact host-process scope — SRUI blocks=20, bytes=20, scope=default malloc zone in the SRUI renderer host process only; signed after-minus-before net live state, not cumulative allocation events; WebKit control blocks=20, bytes=20, scope=default malloc zone in the WebKit comparison host process only; excludes WebContent, Network, and GPU helper processes; signed after-minus-before net live state, not cumulative allocation events
- **PASS** WindowServer isolation rejects an exact synthetic occluder — window isolation self-test passed: dock=20 status=25 ahead=26 popup=101 target=448887 occluder=448888
- **PASS** renderer candidate launch failures cannot leak a child process — forced candidate identity failure terminated the exact candidate process group and proved both the child and a real descendant gone; a forced descendant-identity publication failure and a forced internal candidate failure also proved both exact identities gone before the benchmark driver returned

Notes:

- Every accepted full-paint state requires an exact visible CGWindow, exact client/target geometry, unobscured z-order, and authorized nonblank/nonuniform ScreenCaptureKit pixels from one complete frame. Latency is action-start Mach time through that accepted frame's SCStream displayTime; callback receipt and pixel hashing are verifier metadata, not the presentation timestamp. The two reported timed states per logical sample produced native=40 and WebKit=40 verified captures; capture_authorization=true. Warm/resource/peak passes are not counted in that metric. No permission request is issued.
- Each native logical sample uses four separately warmed/reset renderer instances: first-state visual latency, complete two-transaction visual latency, CPU/host-net-live-allocation/footprint-growth through production display submission, and sampler-only peak footprint. Each WebKit sample resets one warmed view between the same four disjoint workloads while preserving exact helper PID identities. Neither resource pass creates ScreenCaptureKit buffers, and the footprint sampler never runs in the CPU/allocation pass.
- Native first timing decodes and applies revision 0→1 through production initial attach. Native complete timing decodes/applies 0→1 and then applies 1→2 through production incremental apply. WebKit uses structurally equivalent first and complete DOM states. The measured instance is checked immediately after each accepted presentation for exact semantic/control or DOM state. Full mode additionally requires nonblank, nonuniform, equal-geometry client-content fingerprints that differ between first and complete states.
- Each candidate process group is birth-identity verified. WebKit CPU, footprint growth, and peak footprint aggregate the host with exact benchmark-only WebContent/network/GPU diagnostic PIDs. Allocation samples are signed default-zone malloc_zone_statistics after-minus-before deltas: blocks_in_use and size_in_use describe net live state, not cumulative allocation traffic. Allocation scope: default malloc zone in the SRUI renderer host process only; signed after-minus-before net live state, not cumulative allocation events. Control scope: default malloc zone in the WebKit comparison host process only; excludes WebContent, Network, and GPU helper processes; signed after-minus-before net live state, not cumulative allocation events. Peak footprint is the maximum simultaneous current-footprint sample at 1 ms cadence strictly inside its separate representative pass, not a sum of per-process lifetime maxima.
- Task 34 deliberately defers cumulative allocation-call and requested-byte counts. The supported malloc_history all-events export was rejected after the first real pre-workload SRUI snapshot expanded to 1,902,439,272 bytes; no value from that attempt entered this report. GitHub issue aizlabs/srui#48 tracks a benchmark-only Darwin allocator-interposition counter.
- The warmed WKWebView candidate and its host-only allocator deltas are comparison controls only; they are not the production SRUI renderer, do not cover WebKit helper-process allocations, and do not describe SRUI's native AppKit rendering path.

## §31.2 Serialization

Samples:

- `rust.generation`: 500
- `rust.serialization`: 500

| Metric | Value | Statistic | Target |
|---|---:|---|---:|
| abstract state generation | 0.003625 ms | p50 | — |
| abstract state generation | 0.00525 ms | p95 | — |
| abstract state generation | 0.01417 ms | p99 | — |
| protobuf serialization | 0.008667 ms | p50 | — |
| protobuf serialization | 0.01263 ms | p95 | — |
| protobuf serialization | 0.02237 ms | p99 | — |
| serialized transaction size | 981 bytes | exact | — |

Assertions:

- **PASS** shared fixture produced SRUI protobuf — 21 nodes split 5 + 16 across revisions 0→1→2 and encoded into 981 framed bytes
- **PASS** canonical progressive transaction sequence bytes match — 3c0273c2eeb792e0cc99a9159c1a1b1fb7fc6cf9f947ec5ad5f33396fc9d8f50 / 981 exact canonical progressive sequence bytes

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
| 1 updates semantic decode/apply | 0.01471 ms | p50 | — |
| 1 updates semantic decode/apply | 0.01937 ms | p95 | — |
| 1 updates semantic decode/apply | 0.1522 ms | p99 | — |
| 1 updates decode-to-visible | 16.61 ms | p50 | — |
| 1 updates decode-to-visible | 18.59 ms | p95 | — |
| 1 updates decode-to-visible | 26.33 ms | p99 | — |
| 1 updates wire bytes | 28 bytes | exact | — |
| 1 updates message count | 1 messages | exact | — |
| 100 updates semantic decode/apply | 0.1014 ms | p50 | ≤ 1 ms |
| 100 updates semantic decode/apply | 0.1112 ms | p95 | — |
| 100 updates semantic decode/apply | 0.1148 ms | p99 | — |
| 100 updates decode-to-visible | 16.91 ms | p50 | — |
| 100 updates decode-to-visible | 18.79 ms | p95 | — |
| 100 updates decode-to-visible | 19.07 ms | p99 | — |
| 100 updates wire bytes | 2109 bytes | exact | — |
| 100 updates message count | 1 messages | exact | — |
| 1000 updates semantic decode/apply | 0.8502 ms | p50 | ≤ 5 ms |
| 1000 updates semantic decode/apply | 0.8813 ms | p95 | — |
| 1000 updates semantic decode/apply | 0.8976 ms | p99 | — |
| 1000 updates decode-to-visible | 17.56 ms | p50 | — |
| 1000 updates decode-to-visible | 19.91 ms | p95 | — |
| 1000 updates decode-to-visible | 20.4 ms | p99 | — |
| 1000 updates wire bytes | 2.101e+04 bytes | exact | — |
| 1000 updates message count | 1 messages | exact | — |
| 1 updates at 60Hz decode-to-visible | 20.5 ms | sample | — |
| 1 updates at 60Hz total SRUI wire bytes | 322 bytes | exact | — |
| 1 updates at 60Hz total SRUI message count | 4 messages | exact | — |
| 1 updates at 60Hz inbound TRANSACTION bytes | 28 bytes | exact | — |
| 1 updates at 60Hz inbound TRANSACTION message count | 1 messages | exact | — |
| 1 updates at 60Hz outbound EVENT bytes | 294 bytes | exact | — |
| 1 updates at 60Hz outbound EVENT message count | 3 messages | exact | — |
| 1 updates at 60Hz synthetic change-gated repaint count | 1 repaints | exact | — |
| 1 updates at 120Hz decode-to-visible | 25.86 ms | sample | — |
| 1 updates at 120Hz total SRUI wire bytes | 322 bytes | exact | — |
| 1 updates at 120Hz total SRUI message count | 4 messages | exact | — |
| 1 updates at 120Hz inbound TRANSACTION bytes | 28 bytes | exact | — |
| 1 updates at 120Hz inbound TRANSACTION message count | 1 messages | exact | — |
| 1 updates at 120Hz outbound EVENT bytes | 294 bytes | exact | — |
| 1 updates at 120Hz outbound EVENT message count | 3 messages | exact | — |
| 1 updates at 120Hz synthetic change-gated repaint count | 1 repaints | exact | — |
| 1 updates at 144Hz decode-to-visible | 24.77 ms | sample | — |
| 1 updates at 144Hz total SRUI wire bytes | 322 bytes | exact | — |
| 1 updates at 144Hz total SRUI message count | 4 messages | exact | — |
| 1 updates at 144Hz inbound TRANSACTION bytes | 28 bytes | exact | — |
| 1 updates at 144Hz inbound TRANSACTION message count | 1 messages | exact | — |
| 1 updates at 144Hz outbound EVENT bytes | 294 bytes | exact | — |
| 1 updates at 144Hz outbound EVENT message count | 3 messages | exact | — |
| 1 updates at 144Hz synthetic change-gated repaint count | 1 repaints | exact | — |
| 1 updates at 240Hz decode-to-visible | 24.47 ms | sample | — |
| 1 updates at 240Hz total SRUI wire bytes | 322 bytes | exact | — |
| 1 updates at 240Hz total SRUI message count | 4 messages | exact | — |
| 1 updates at 240Hz inbound TRANSACTION bytes | 28 bytes | exact | — |
| 1 updates at 240Hz inbound TRANSACTION message count | 1 messages | exact | — |
| 1 updates at 240Hz outbound EVENT bytes | 294 bytes | exact | — |
| 1 updates at 240Hz outbound EVENT message count | 3 messages | exact | — |
| 1 updates at 240Hz synthetic change-gated repaint count | 1 repaints | exact | — |
| 100 updates at 60Hz decode-to-visible | 341.6 ms | sample | — |
| 100 updates at 60Hz total SRUI wire bytes | 3094 bytes | exact | — |
| 100 updates at 60Hz total SRUI message count | 103 messages | exact | — |
| 100 updates at 60Hz inbound TRANSACTION bytes | 2800 bytes | exact | — |
| 100 updates at 60Hz inbound TRANSACTION message count | 100 messages | exact | — |
| 100 updates at 60Hz outbound EVENT bytes | 294 bytes | exact | — |
| 100 updates at 60Hz outbound EVENT message count | 3 messages | exact | — |
| 100 updates at 60Hz synthetic change-gated repaint count | 13 repaints | exact | — |
| 100 updates at 120Hz decode-to-visible | 418.9 ms | sample | — |
| 100 updates at 120Hz total SRUI wire bytes | 3094 bytes | exact | — |
| 100 updates at 120Hz total SRUI message count | 103 messages | exact | — |
| 100 updates at 120Hz inbound TRANSACTION bytes | 2800 bytes | exact | — |
| 100 updates at 120Hz inbound TRANSACTION message count | 100 messages | exact | — |
| 100 updates at 120Hz outbound EVENT bytes | 294 bytes | exact | — |
| 100 updates at 120Hz outbound EVENT message count | 3 messages | exact | — |
| 100 updates at 120Hz synthetic change-gated repaint count | 24 repaints | exact | — |
| 100 updates at 144Hz decode-to-visible | 441.2 ms | sample | — |
| 100 updates at 144Hz total SRUI wire bytes | 3094 bytes | exact | — |
| 100 updates at 144Hz total SRUI message count | 103 messages | exact | — |
| 100 updates at 144Hz inbound TRANSACTION bytes | 2800 bytes | exact | — |
| 100 updates at 144Hz inbound TRANSACTION message count | 100 messages | exact | — |
| 100 updates at 144Hz outbound EVENT bytes | 294 bytes | exact | — |
| 100 updates at 144Hz outbound EVENT message count | 3 messages | exact | — |
| 100 updates at 144Hz synthetic change-gated repaint count | 27 repaints | exact | — |
| 100 updates at 240Hz decode-to-visible | 593.1 ms | sample | — |
| 100 updates at 240Hz total SRUI wire bytes | 3094 bytes | exact | — |
| 100 updates at 240Hz total SRUI message count | 103 messages | exact | — |
| 100 updates at 240Hz inbound TRANSACTION bytes | 2800 bytes | exact | — |
| 100 updates at 240Hz inbound TRANSACTION message count | 100 messages | exact | — |
| 100 updates at 240Hz outbound EVENT bytes | 294 bytes | exact | — |
| 100 updates at 240Hz outbound EVENT message count | 3 messages | exact | — |
| 100 updates at 240Hz synthetic change-gated repaint count | 45 repaints | exact | — |
| 1000 updates at 60Hz decode-to-visible | 6358 ms | sample | — |
| 1000 updates at 60Hz total SRUI wire bytes | 3.004e+04 bytes | exact | — |
| 1000 updates at 60Hz total SRUI message count | 1003 messages | exact | — |
| 1000 updates at 60Hz inbound TRANSACTION bytes | 2.975e+04 bytes | exact | — |
| 1000 updates at 60Hz inbound TRANSACTION message count | 1000 messages | exact | — |
| 1000 updates at 60Hz outbound EVENT bytes | 297 bytes | exact | — |
| 1000 updates at 60Hz outbound EVENT message count | 3 messages | exact | — |
| 1000 updates at 60Hz synthetic change-gated repaint count | 245 repaints | exact | — |
| 1000 updates at 120Hz decode-to-visible | 6367 ms | sample | — |
| 1000 updates at 120Hz total SRUI wire bytes | 3.004e+04 bytes | exact | — |
| 1000 updates at 120Hz total SRUI message count | 1003 messages | exact | — |
| 1000 updates at 120Hz inbound TRANSACTION bytes | 2.975e+04 bytes | exact | — |
| 1000 updates at 120Hz inbound TRANSACTION message count | 1000 messages | exact | — |
| 1000 updates at 120Hz outbound EVENT bytes | 297 bytes | exact | — |
| 1000 updates at 120Hz outbound EVENT message count | 3 messages | exact | — |
| 1000 updates at 120Hz synthetic change-gated repaint count | 364 repaints | exact | — |
| 1000 updates at 144Hz decode-to-visible | 6368 ms | sample | — |
| 1000 updates at 144Hz total SRUI wire bytes | 3.004e+04 bytes | exact | — |
| 1000 updates at 144Hz total SRUI message count | 1003 messages | exact | — |
| 1000 updates at 144Hz inbound TRANSACTION bytes | 2.975e+04 bytes | exact | — |
| 1000 updates at 144Hz inbound TRANSACTION message count | 1000 messages | exact | — |
| 1000 updates at 144Hz outbound EVENT bytes | 297 bytes | exact | — |
| 1000 updates at 144Hz outbound EVENT message count | 3 messages | exact | — |
| 1000 updates at 144Hz synthetic change-gated repaint count | 392 repaints | exact | — |
| 1000 updates at 240Hz decode-to-visible | 6354 ms | sample | — |
| 1000 updates at 240Hz total SRUI wire bytes | 3.004e+04 bytes | exact | — |
| 1000 updates at 240Hz total SRUI message count | 1003 messages | exact | — |
| 1000 updates at 240Hz inbound TRANSACTION bytes | 2.975e+04 bytes | exact | — |
| 1000 updates at 240Hz inbound TRANSACTION message count | 1000 messages | exact | — |
| 1000 updates at 240Hz outbound EVENT bytes | 297 bytes | exact | — |
| 1000 updates at 240Hz outbound EVENT message count | 3 messages | exact | — |
| 1000 updates at 240Hz synthetic change-gated repaint count | 491 repaints | exact | — |
| settled idle SRUI wire bytes | 0 bytes | observed max | — |
| settled idle SRUI message count | 0 messages | observed max | — |

Assertions:

- **PASS** decode-to-visible samples reach an unforced composited content change — every framed production SessionController sample matched isolated semantic state and changed the exact visible client-content fingerprint without benchmark-forced invalidation; every cadence stream ended in its expected native value with passive composited evidence
- **PASS** settled idle UI emits zero SRUI traffic — 1000ms after all production EVENT ACKs drained for every count/cadence; byte deltas [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], message deltas [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]. This does not claim natural AppKit idle invalidation behavior: the synthetic cadence gate suppresses unchanged-state draws by construction.
- **PASS** complete bidirectional wire bytes and message count are cadence independent — the same prebuilt 1/100/1000 inbound TRANSACTION frame sequences were reused at 60/120/144/240Hz; every outbound transport delta is exactly the three decoded production EVENT frames with no extra, dropped, or interrupted attempts; total, inbound, and outbound bytes/messages are reported separately and compared across cadences; ACKs are injected only after these counters and the presentation timestamp are frozen
- **PASS** synthetic native-change-gated repaint count varies independently — 1@60Hz=1, 1@120Hz=1, 1@144Hz=1, 1@240Hz=1, 100@60Hz=13, 100@120Hz=24, 100@144Hz=27, 100@240Hz=45, 1000@60Hz=245, 1000@120Hz=364, 1000@144Hz=392, 1000@240Hz=491. This diagnostic measures configured coalescing opportunities; it is not evidence of AppKit's natural invalidation count.
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
| caret movement at 0ms RTT | 13.69 ms | p50 | ≤ 8.33333 ms |
| caret movement at 0ms RTT | 18.64 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| caret movement at 0ms RTT | 18.65 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| hover at 0ms RTT | 19.42 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| hover at 0ms RTT | 25.15 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| hover at 0ms RTT | 25.25 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| ime composition at 0ms RTT | 16.67 ms | p50 | ≤ 8.33333 ms |
| ime composition at 0ms RTT | 18.5 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| ime composition at 0ms RTT | 18.82 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| menu opening at 0ms RTT | 33.92 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| menu opening at 0ms RTT | 36.5 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| menu opening at 0ms RTT | 61.32 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| pressed at 0ms RTT | 18.1 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| pressed at 0ms RTT | 24.55 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| pressed at 0ms RTT | 25.02 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| scrolling at 0ms RTT | 17.01 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| scrolling at 0ms RTT | 18.72 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| scrolling at 0ms RTT | 19.05 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| text entry at 0ms RTT | 16.42 ms | p50 | ≤ 8.33333 ms |
| text entry at 0ms RTT | 19.13 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| text entry at 0ms RTT | 19.5 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| text selection at 0ms RTT | 17.46 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| text selection at 0ms RTT | 19.25 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| text selection at 0ms RTT | 25.43 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| server-dependent input-to-visible at 0ms RTT | 12.73 ms | p50 | — |
| server-dependent input-to-visible at 0ms RTT | 17.04 ms | p95 | — |
| server-dependent input-to-visible at 0ms RTT | 17.04 ms | p99 | — |
| caret movement at 100ms RTT | 17.52 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| caret movement at 100ms RTT | 18.99 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| caret movement at 100ms RTT | 19.97 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| hover at 100ms RTT | 16.57 ms | p50 | ≤ 8.33333 ms |
| hover at 100ms RTT | 22.62 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| hover at 100ms RTT | 22.65 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| ime composition at 100ms RTT | 13.31 ms | p50 | ≤ 8.33333 ms |
| ime composition at 100ms RTT | 18.99 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| ime composition at 100ms RTT | 20.76 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| menu opening at 100ms RTT | 34.81 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| menu opening at 100ms RTT | 36.49 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| menu opening at 100ms RTT | 48.45 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| pressed at 100ms RTT | 18.87 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| pressed at 100ms RTT | 21.49 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| pressed at 100ms RTT | 25.31 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| scrolling at 100ms RTT | 17.81 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| scrolling at 100ms RTT | 25.07 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| scrolling at 100ms RTT | 25.55 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| text entry at 100ms RTT | 13.64 ms | p50 | ≤ 8.33333 ms |
| text entry at 100ms RTT | 19.47 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| text entry at 100ms RTT | 19.59 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| text selection at 100ms RTT | 16.89 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| text selection at 100ms RTT | 18.48 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| text selection at 100ms RTT | 18.92 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| server-dependent input-to-visible at 100ms RTT | 121.6 ms | p50 | — |
| server-dependent input-to-visible at 100ms RTT | 125.2 ms | p95 | — |
| server-dependent input-to-visible at 100ms RTT | 125.2 ms | p99 | — |
| caret movement at 300ms RTT | 16.95 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| caret movement at 300ms RTT | 19.21 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| caret movement at 300ms RTT | 20.47 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| hover at 300ms RTT | 17.57 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| hover at 300ms RTT | 20.45 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| hover at 300ms RTT | 23.06 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| ime composition at 300ms RTT | 18.36 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| ime composition at 300ms RTT | 21.2 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| ime composition at 300ms RTT | 21.33 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| menu opening at 300ms RTT | 33.17 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| menu opening at 300ms RTT | 42 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| menu opening at 300ms RTT | 48.6 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| pressed at 300ms RTT | 17.64 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| pressed at 300ms RTT | 18.42 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| pressed at 300ms RTT | 20.11 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| scrolling at 300ms RTT | 21.54 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| scrolling at 300ms RTT | 26.17 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| scrolling at 300ms RTT | 26.22 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| text entry at 300ms RTT | 16.28 ms | p50 | ≤ 8.33333 ms |
| text entry at 300ms RTT | 19.8 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| text entry at 300ms RTT | 20.14 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| text selection at 300ms RTT | 13.33 ms | p50 | ≤ 8.33333 ms |
| text selection at 300ms RTT | 18.23 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| text selection at 300ms RTT | 19.09 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| server-dependent input-to-visible at 300ms RTT | 329.8 ms | p50 | — |
| server-dependent input-to-visible at 300ms RTT | 338.2 ms | p95 | — |
| server-dependent input-to-visible at 300ms RTT | 338.2 ms | p99 | — |
| caret movement at 600ms RTT | 18.57 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| caret movement at 600ms RTT | 21.21 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| caret movement at 600ms RTT | 21.31 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| hover at 600ms RTT | 16.52 ms | p50 | ≤ 8.33333 ms |
| hover at 600ms RTT | 20.19 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| hover at 600ms RTT | 20.68 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| ime composition at 600ms RTT | 16.66 ms | p50 | ≤ 8.33333 ms |
| ime composition at 600ms RTT | 20.42 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| ime composition at 600ms RTT | 21.4 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| menu opening at 600ms RTT | 34.83 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| menu opening at 600ms RTT | 38.05 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| menu opening at 600ms RTT | 53.44 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| pressed at 600ms RTT | 18.27 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| pressed at 600ms RTT | 21.43 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| pressed at 600ms RTT | 21.48 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| scrolling at 600ms RTT | 20.98 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| scrolling at 600ms RTT | 26.13 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| scrolling at 600ms RTT | 27.23 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| text entry at 600ms RTT | 13.6 ms | p50 | ≤ 8.33333 ms |
| text entry at 600ms RTT | 21.11 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| text entry at 600ms RTT | 21.25 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| text selection at 600ms RTT | 17.59 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| text selection at 600ms RTT | 20.7 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| text selection at 600ms RTT | 20.91 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| server-dependent input-to-visible at 600ms RTT | 636.1 ms | p50 | — |
| server-dependent input-to-visible at 600ms RTT | 646.3 ms | p95 | — |
| server-dependent input-to-visible at 600ms RTT | 646.3 ms | p99 | — |
| 1MiB/s bandwidth-limited production event | 16.96 ms | p50 | — |
| 1MiB/s bandwidth-limited production event | 17.16 ms | p95 | — |
| bandwidth-limited delivered bytes | 4.948e+04 bytes | exact | — |
| deterministic production loss attempts | 2 messages | exact | — |
| deterministic production loss delivered messages | 1 messages | exact | — |
| controlled production interruption detection | 7.799 ms | p50 | — |
| measured production session wire bytes | 1.421e+05 bytes | exact | — |
| measured production session wire messages | 1240 messages | exact | — |
| maximum RTT-induced local latency delta | 5.905 ms | p50 | ≤ 8.33333 ms |
| maximum RTT-induced local latency delta | 13.21 ms | p95 | ≤ 8.33333 ms |
| maximum RTT-induced local latency delta | 14.03 ms | p99 | ≤ 8.33333 ms |

Assertions:

- **PASS** mounted local interactions do not acquire one RTT — largest p50 increase 5.9045 ms versus the measured local frame budget of 8.3333 ms; full compositor mode applies this numeric correctness gate; paired injected transaction remained blocked through local visible completion in 640/640 probes=true; configured delay state was verified at the exact action boundary in 640/640 probes=true, with a nonzero transport delay still active in 480/480 nonzero-RTT probes; local_state_checks=truepaired p95/p99 deltas were 13.2104/14.0325 ms; production renderer callbacks=240
- **PASS** native text entry emits and settles one exact production TEXT_EDIT — RTT 0ms: node=14 final_text="ééééééééééééééééééééview the changes and run tests.xxxxxxxxxxxxxxxxxxxx" edit_seq=40 observed_revision=81 matching_framed_events=1 exact_slot_ack=true stable_empty_tail=true; RTT 100ms: node=14 final_text="ééééééééééééééééééééview the changes and run tests.xxxxxxxxxxxxxxxxxxxx" edit_seq=40 observed_revision=81 matching_framed_events=1 exact_slot_ack=true stable_empty_tail=true; RTT 300ms: node=14 final_text="ééééééééééééééééééééview the changes and run tests.xxxxxxxxxxxxxxxxxxxx" edit_seq=40 observed_revision=80 matching_framed_events=1 exact_slot_ack=true stable_empty_tail=true; RTT 600ms: node=14 final_text="ééééééééééééééééééééview the changes and run tests.xxxxxxxxxxxxxxxxxxxx" edit_seq=40 observed_revision=80 matching_framed_events=1 exact_slot_ack=true stable_empty_tail=true
- **PASS** injected transport RTT affects production server-dependent feedback — SessionController EventOutbox sends and framed transaction responses tracked 100/300/600ms RTT
- **PASS** local visible completion does not await an injected transport response — Each local action began only after its exact compositor baseline was ready and while its configured BenchmarkTransport delay state was verified; 480/480 nonzero-RTT actions began during an active delay. Every paired production transaction remained blocked through visible completion; the gate was released only afterward. Separately, production server-dependent feedback tracked 100/300/600ms RTT.
- **PASS** bandwidth delay, loss, and interruption exercise session recovery — sample 1: 16493 framed bytes / 1 message: measured 17.1586 ms >= theoretical 15.7290 ms; exact_event=true; sample 2: 16493 framed bytes / 1 message: measured 16.9617 ms >= theoretical 15.7290 ms; exact_event=true; sample 3: 16493 framed bytes / 1 message: measured 16.9600 ms >= theoretical 15.7290 ms; exact_event=true; 49479 total bandwidth bytes; lost and interrupted events remained in EventOutbox and replayed through replacement SessionControllers

Notes:

- Controls are mounted renderer TextArea, ScrollView, and Button. renderer-produced NSButton context menu was proven as a new exact owned menu-level WindowServer surface in the same ScreenCaptureKit frame used for its presentation timestamp.
- Pressed state is observed between real NSWindow-dispatched mouseDown/mouseUp events and triggers the production ActionTrampoline. renderer-produced NSButton changed at least 1590 target-ROI pixels above the explicit 2/255 per-channel SCStream tolerance on the local AppKit hover path in 20/20 samples; mouseExited then restored every unmasked screenshot pixel within the explicit 5/255 same-API tolerance, with maximum observed channel delta 5/255. Permission-free hover invokes the renderer-produced NSButton's own AppKit entry/exit path and does not claim WindowServer pointer latency.
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
| disconnect immediately before event receipt | 6.925 ms | p50 | — |
| disconnect immediately before event receipt | 7.547 ms | p95 | — |
| disconnect immediately before event receipt | 8.832 ms | p99 | — |
| event receipt through settled side effect | 0.000541 ms | p50 | — |
| event receipt through settled side effect | 0.001417 ms | p95 | — |
| event receipt through settled side effect | 0.005792 ms | p99 | — |
| in-process cached DUPLICATE response | 0.000208 ms | p50 | — |
| in-process cached DUPLICATE response | 0.0005 ms | p95 | — |
| in-process cached DUPLICATE response | 0.000833 ms | p99 | — |
| lost ACK wire reconnect through DUPLICATE acknowledgement | 0.1165 ms | p50 | — |
| lost ACK wire reconnect through DUPLICATE acknowledgement | 0.2539 ms | p95 | — |
| lost ACK wire reconnect through DUPLICATE acknowledgement | 0.6162 ms | p99 | — |
| mid-resource reconnect and exact replay | 0.003667 ms | p50 | — |
| mid-resource reconnect and exact replay | 0.009416 ms | p95 | — |
| mid-resource reconnect and exact replay | 0.03271 ms | p99 | — |
| mid-transaction frame discard and atomic replay | 0.000834 ms | p50 | — |
| mid-transaction frame discard and atomic replay | 0.002709 ms | p95 | — |
| mid-transaction frame discard and atomic replay | 0.008667 ms | p99 | — |
| mid-transaction wire disconnect and exact atomic replay | 6.868 ms | p50 | — |
| mid-transaction wire disconnect and exact atomic replay | 6.978 ms | p95 | — |
| mid-transaction wire disconnect and exact atomic replay | 8.192 ms | p99 | — |
| partial EVENT disconnect and one processed replay | 7.39 ms | p50 | — |
| partial EVENT disconnect and one processed replay | 8.17 ms | p95 | — |
| partial EVENT disconnect and one processed replay | 9.374 ms | p99 | — |
| resume beyond journal retention | 0.000667 ms | p50 | — |
| resume beyond journal retention | 0.00175 ms | p95 | — |
| resume beyond journal retention | 0.003875 ms | p99 | — |
| resume within journal retention | 0.000625 ms | p50 | — |
| resume within journal retention | 0.0015 ms | p95 | — |
| resume within journal retention | 0.003375 ms | p99 | — |
| pre-receipt retained-event replay | 20.32 ms | p50 | — |
| pre-receipt retained-event replay | 25.76 ms | p95 | — |
| mid-resource reconnect recovery | 0.6295 ms | p50 | — |
| mid-resource reconnect recovery | 1.189 ms | p95 | — |
| superseded resume response handling | 0.1168 ms | p50 | — |
| superseded resume response handling | 0.1612 ms | p95 | — |
| active resume response handling | 0.2234 ms | p50 | — |
| active resume response handling | 0.3482 ms | p95 | — |
| production reconnect boundary suite | 2.722e+04 ms | wall | — |

Assertions:

- **PASS** mid-resource reconnect restarts production transfer at offset zero — 32805-byte CAS object interrupted after one real chunk and reconstructed exactly
- **PASS** mid-transaction wire disconnect exposes no partial state — 50 capacity-one handle_connection streams decoded no partial frame, left the replica at revision 0, then replayed exactly one atomic revision
- **PASS** pre-receipt and partial EVENT disconnects are inert before one processed replay — 50 pre-receipt and capacity-one partial-frame handle_connection reconnects each dispatched zero events on the interrupted connection, then decoded one Processed acknowledgement and one resulting transaction
- **PASS** lost ACK wire replay is DUPLICATE without a second side effect — 50 duplex reconnects decoded ServerEventAck::Duplicate with cached revision 2; handler count and state stayed at one effect
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
| embedded SRUI PTY exact ANSI capture and framing | 9.85 ms | p50 | — |
| embedded SRUI PTY exact ANSI capture and framing | 15.68 ms | p95 | — |
| embedded SRUI PTY exact ANSI capture and framing | 29.81 ms | p99 | — |
| standalone PTY exact ANSI interaction | 9.792 ms | p50 | — |
| standalone PTY exact ANSI interaction | 38.45 ms | p95 | — |
| standalone PTY exact ANSI interaction | 45.34 ms | p99 | — |
| terminal reconnect retention-loss decision | 0.000916 ms | sample | — |
| terminal payload | 6912 bytes | exact | — |
| embedded terminal frame count | 1 messages | p50 | — |
| embedded terminal frame count | 1 messages | p95 | — |
| embedded terminal frame count | 1 messages | p99 | — |
| embedded SRUI Terminal decode-to-visible | 19.52 ms | p50 | — |
| embedded SRUI Terminal decode-to-visible | 21.81 ms | p95 | — |
| embedded SRUI Terminal decode-to-visible | 27.33 ms | p99 | — |
| embedded SRUI Terminal draw-only | 17.44 ms | p50 | — |
| embedded SRUI Terminal draw-only | 19.46 ms | p95 | — |
| embedded SRUI Terminal draw-only | 26.34 ms | p99 | — |
| standalone TerminalSession and TerminalView decode-to-visible | 20.19 ms | p50 | — |
| standalone TerminalSession and TerminalView decode-to-visible | 25.41 ms | p95 | — |
| standalone TerminalSession and TerminalView decode-to-visible | 27.16 ms | p99 | — |
| standalone TerminalView draw-only | 14.48 ms | p50 | — |
| standalone TerminalView draw-only | 24.55 ms | p95 | — |
| standalone TerminalView draw-only | 26.25 ms | p99 | — |
| embedded-to-standalone terminal decode-to-visible | 0.967 ratio | p50 | — |
| embedded-to-standalone terminal draw-only | 1.204 ratio | p50 | — |
| client terminal framed envelope | 6922 bytes | exact | — |
| embedded terminal draw completions | 20 frames | exact | — |
| standalone terminal draw completions | 20 frames | exact | — |

Assertions:

- **PASS** standalone and embedded PTYs emit the identical ANSI byte stream — both paths compared all 6912 payload bytes
- **PASS** standalone terminal command exits successfully — 20 child exit statuses checked
- **PASS** embedded terminal reaches natural successful EOF with no trailing bytes — 20 production PTY exit statuses checked after the final exact offset
- **PASS** embedded terminal frames preserve exact offsets and bounds — 20 samples aggregated; p50 1 frames, each at most 16384 bytes
- **PASS** reconnect ring-buffer exhaustion maps to RETENTION_LOSS — PTYManager::subscribe returned Resync and catch_up emitted TerminalResyncRequired
- **PASS** framed embedded terminal payload and offsets remain exact — every active SessionController delivery decoded one framed SRUITerminalData carrying exactly 6912 payload bytes at offset zero; the renderer-owned TerminalSession ended at 6912; framed envelope byte counts: [6922]
- **PASS** standalone terminal payload and offsets remain exact — every direct production TerminalSession delivery consumed exactly 6912 payload bytes at offset zero and ended at offset 6912
- **PASS** standalone and embedded terminal visible completions are actual draws — 20/20 embedded and 20/20 standalone TerminalView completions crossed the configured draw-visible boundary and produced content-distinct bitmaps; full mode additionally required nonblank, nonuniform client-only composited captures; embedded visibility provenance: ["screencapturekit_same_complete_frame_target_roi_after_action_screencapturekit_composited_baseline_status_level_25_appkit_occlusion_8192"]; standalone visibility provenance: ["screencapturekit_same_complete_frame_target_roi_after_action_screencapturekit_composited_baseline_status_level_25_appkit_occlusion_8192"]; embedded composited fingerprints: ["510a5ce15560d979e94dbff08d3a78bd287e1fb9fe5788cb4835e6d1a5dca4aa", "6f524e26b53bc7e7b84a56aea9d8696f96da7e27143af961283fb430f52d7557"]; standalone composited fingerprints: ["01c66fbccdc8688857d79f1f186d48fe45e9b7bc218bca3c8c722310bb4b0114", "1471bab98eb223daca8ddad352120a83c96d5406811adc94244ff2a93da84be0", "2172b15a2c370b49f3e1dff76eb2c6525ca318624e0ef8091f9740eebab44363", "21f261f63d0f955c1ff277595a97feba695f9363da9bc7034915876c307654d4", "2b6e49e43c4022670de1e682ef1a24bd6f0eca11f385b9487610632373a9c9f6", "321a8387bd55fac91a6c2967de3ba919f6a56c609fd459e68e78ba04782ecab3", "374b1c7ff33bdc6d408bbaccef157af5af6e7fd8f97dd2fc98530ed292461586", "4073f01879a9fafc1c84c84b6bb19ee9a6be13982f024e24e080d01d37cd0cb5", "48068db4c38d5e7ed4d4f9120f608abdec69f635f15aa394eb96e628880f91b0", "4cb53331f4477cc0323d664267dd021170180e2fb386561c8c0c2f2bd6cf09c5", "5352a963d1f0560ba970ff9fb1ea5ae3d44d1699f8c81486353535f6e00f4bb3", "792f8a2103de28758e18d85da373008900d210b9b7bafdd19c144bf14889460d", "7bf6919ef651b708eae65c4906c0a46c11a4b21fd2a16150e296151e2002cabf", "931322f7e2c2d97beb2c1a22117b87ff813c924384ad67dffbcaaee19af1a7bd", "94d9178fce5ecd2925bdb4f62e08ec9a3a7600948255ac36b3752ce15bffa15b", "a622821d5bba5e2898fca3fdd5d68967e7cf9954f3831018454ddccb052b0e55", "be0211fa3c6369f40f2c5847ae17b9183a68ec59c585835a624005f88d8c99cd", "be88a53b0f7f647dd46fa45a6098bec031fd3372ff55e10ec27eea63f48f6005", "c252f1a0dda87cada32311420fbcbd13b74b1cb043aa92fe597d4a5944e3f9d4", "c5b4cb237a2d6628271667aafb69044831f08b5c2102f98b571fb691705ad423"]
- **PASS** standalone and embedded terminal displays render the identical ANSI payload — both paths consumed payload SHA-256 e4461bba73f99c7bc70ba6041b859c3e59446d6c80d1549c25de7bcaf933a910, decoded exact 256-line content SHA-256 e1e85bdbda198ce2b94b317765b18ec90b981d25c0ef567ff83d102f6817fe8d, and produced byte-identical TerminalView rasters; embedded content digests: ["e1e85bdbda198ce2b94b317765b18ec90b981d25c0ef567ff83d102f6817fe8d"]; standalone content digests: ["e1e85bdbda198ce2b94b317765b18ec90b981d25c0ef567ff83d102f6817fe8d"]; embedded bitmap digests: ["e440b40246b19d42137da63cb2cc233d9e18702986ff6269b4f6ac33337dbab5"]; standalone bitmap digests: ["e440b40246b19d42137da63cb2cc233d9e18702986ff6269b4f6ac33337dbab5"]
- **PASS** standalone and production terminal samples begin from fresh parser and view state — embedded samples negotiate org.srui.terminal/1, register namespace 31, and mount through a framed transaction; standalone samples instantiate fresh production TerminalSession and TerminalView pairs; both reproduce exact content SHA-256 e1e85bdbda198ce2b94b317765b18ec90b981d25c0ef567ff83d102f6817fe8d

Notes:

- Standalone and embedded samples both include production PTY spawn, identical shell execution, EOF, and exact ANSI output; embedded additionally captures and frames the bytes. The embedded EOF boundary is event-driven and does not poll process state.
- Embedded decode-to-visible starts before a framed 6,912-byte SRUITerminalData enters BenchmarkTransport, then crosses SessionController, the renderer-owned TerminalSession, the renderer-created TerminalView snapshot subscription, and the configured presentation boundary.
- The standalone display baseline feeds the identical Data value at offset zero through fresh production TerminalSession and TerminalView instances, using the embedded view's exact grid and bounds. The reported ratios compare p50 display completion on like-for-like content and geometry.
- Full mode starts an exact TerminalView-ROI ScreenCaptureKit stream and verifies a nonblank, nonuniform baseline before timing. After the production payload action completes, it accepts only a later complete compositor sample whose same-frame ROI fingerprint changed; the reported latency is action-start mach time to that sample's ScreenCaptureKit display time. Exact WindowServer identity, display, client/ROI geometry, and absence of every intersecting visible nonzero-alpha window are checked before and after acceptance. Callback receipt remains metadata rather than the latency clock. Observed embedded provenance: ["screencapturekit_same_complete_frame_target_roi_after_action_screencapturekit_composited_baseline_status_level_25_appkit_occlusion_8192"]; standalone provenance: ["screencapturekit_same_complete_frame_target_roi_after_action_screencapturekit_composited_baseline_status_level_25_appkit_occlusion_8192"]. Smoke uses an explicitly named offscreen bitmap draw and is not presentation evidence.
- The Rust section separately compares exact standalone-versus-embedded PTY byte transport and exercises reconnect ring-buffer exhaustion; this Swift section compares terminal emulator and display completion.

## Follow-up flags

- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 caret movement at 0ms RTT (p95): 18.64 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 caret movement at 0ms RTT (p99): 18.65 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 hover at 0ms RTT (p50): 19.42 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 hover at 0ms RTT (p95): 25.15 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 hover at 0ms RTT (p99): 25.25 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 ime composition at 0ms RTT (p95): 18.5 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 ime composition at 0ms RTT (p99): 18.82 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 0ms RTT (p50): 33.92 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 0ms RTT (p95): 36.5 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 0ms RTT (p99): 61.32 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 pressed at 0ms RTT (p50): 18.1 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 pressed at 0ms RTT (p95): 24.55 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 pressed at 0ms RTT (p99): 25.02 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 0ms RTT (p50): 17.01 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 0ms RTT (p95): 18.72 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 0ms RTT (p99): 19.05 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text entry at 0ms RTT (p95): 19.13 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text entry at 0ms RTT (p99): 19.5 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text selection at 0ms RTT (p50): 17.46 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text selection at 0ms RTT (p95): 19.25 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text selection at 0ms RTT (p99): 25.43 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 caret movement at 100ms RTT (p50): 17.52 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 caret movement at 100ms RTT (p95): 18.99 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 caret movement at 100ms RTT (p99): 19.97 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 hover at 100ms RTT (p95): 22.62 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 hover at 100ms RTT (p99): 22.65 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 ime composition at 100ms RTT (p95): 18.99 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 ime composition at 100ms RTT (p99): 20.76 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 100ms RTT (p50): 34.81 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 100ms RTT (p95): 36.49 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 100ms RTT (p99): 48.45 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 pressed at 100ms RTT (p50): 18.87 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 pressed at 100ms RTT (p95): 21.49 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 pressed at 100ms RTT (p99): 25.31 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 100ms RTT (p50): 17.81 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 100ms RTT (p95): 25.07 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 100ms RTT (p99): 25.55 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text entry at 100ms RTT (p95): 19.47 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text entry at 100ms RTT (p99): 19.59 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text selection at 100ms RTT (p50): 16.89 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text selection at 100ms RTT (p95): 18.48 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text selection at 100ms RTT (p99): 18.92 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 caret movement at 300ms RTT (p50): 16.95 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 caret movement at 300ms RTT (p95): 19.21 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 caret movement at 300ms RTT (p99): 20.47 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 hover at 300ms RTT (p50): 17.57 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 hover at 300ms RTT (p95): 20.45 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 hover at 300ms RTT (p99): 23.06 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 ime composition at 300ms RTT (p50): 18.36 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 ime composition at 300ms RTT (p95): 21.2 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 ime composition at 300ms RTT (p99): 21.33 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 300ms RTT (p50): 33.17 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 300ms RTT (p95): 42 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 300ms RTT (p99): 48.6 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 pressed at 300ms RTT (p50): 17.64 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 pressed at 300ms RTT (p95): 18.42 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 pressed at 300ms RTT (p99): 20.11 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 300ms RTT (p50): 21.54 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 300ms RTT (p95): 26.17 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 300ms RTT (p99): 26.22 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text entry at 300ms RTT (p95): 19.8 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text entry at 300ms RTT (p99): 20.14 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text selection at 300ms RTT (p95): 18.23 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text selection at 300ms RTT (p99): 19.09 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 caret movement at 600ms RTT (p50): 18.57 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 caret movement at 600ms RTT (p95): 21.21 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 caret movement at 600ms RTT (p99): 21.31 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 hover at 600ms RTT (p95): 20.19 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 hover at 600ms RTT (p99): 20.68 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 ime composition at 600ms RTT (p95): 20.42 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 ime composition at 600ms RTT (p99): 21.4 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 600ms RTT (p50): 34.83 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 600ms RTT (p95): 38.05 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 600ms RTT (p99): 53.44 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 pressed at 600ms RTT (p50): 18.27 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 pressed at 600ms RTT (p95): 21.43 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 pressed at 600ms RTT (p99): 21.48 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 600ms RTT (p50): 20.98 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 600ms RTT (p95): 26.13 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 600ms RTT (p99): 27.23 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text entry at 600ms RTT (p95): 21.11 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text entry at 600ms RTT (p99): 21.25 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text selection at 600ms RTT (p50): 17.59 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text selection at 600ms RTT (p95): 20.7 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text selection at 600ms RTT (p99): 20.91 ms vs target 8.33333 ms
