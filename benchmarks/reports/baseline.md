# SRUI benchmark report

- Generated: 2026-09-10T14:52:31.071035+00:00
- Profile: full
- Host: macOS-26.4.1-arm64-arm-64bit-Mach-O / arm64
- Chip: Apple M2 Max
- Physical RAM: 34359738368 bytes
- Xcode: Xcode 26.2 / Build version 17C52
- Swift: Apple Swift version 6.2.3 (swiftlang-6.2.3.3.21 clang-1700.6.3.2) / Target: arm64-apple-macosx26.0 / swift-driver version: 1.127.14.1
- Rust: rustc 1.98.0 (88d9e12ae 2026-08-18)
- Python: 3.13.7
- Git: dcbab4ba380a0eb3bcd37e8db25721757dead371 (clean)
- Fixture: benchmarks/fixtures/coding-agent-ui.json

## §31.1 Local renderer

Samples:

- `macos.srui.render`: 20
- `macos.webkit.render`: 20

| Metric | Value | Statistic | Target |
|---|---:|---|---:|
| SRUI first on-screen paint crossing display refresh | 42.3 ms | p50 | — |
| SRUI first on-screen paint crossing display refresh | 52.33 ms | p95 | — |
| SRUI complete on-screen paint crossing display refresh | 58.57 ms | p50 | — |
| SRUI complete on-screen paint crossing display refresh | 61.62 ms | p95 | — |
| SRUI complete on-screen paint crossing display refresh | 139.2 ms | p99 | — |
| SRUI candidate process CPU time | 0.8305 ms | p50 | — |
| SRUI candidate process CPU time | 0.8573 ms | p95 | — |
| SRUI host-process net live allocation block delta | 1.928e+04 allocations | p50 | — |
| SRUI host-process net live allocation block delta | 1.985e+04 allocations | p95 | — |
| SRUI host-process net live allocation block delta | 1.986e+04 allocations | p99 | — |
| SRUI host-process net live allocation byte delta | 1.895e+06 bytes | p50 | — |
| SRUI host-process net live allocation byte delta | 1.922e+06 bytes | p95 | — |
| SRUI host-process net live allocation byte delta | 1.948e+06 bytes | p99 | — |
| SRUI host allocated footprint growth | 0.8438 MiB | p50 | — |
| SRUI maximum concurrently sampled process footprint | 45.64 MiB | max | — |
| WKWebView first on-screen paint crossing display refresh | 33.93 ms | p50 | — |
| WKWebView first on-screen paint crossing display refresh | 138 ms | p95 | — |
| WKWebView complete on-screen paint crossing display refresh | 124.6 ms | p50 | — |
| WKWebView complete on-screen paint crossing display refresh | 175.8 ms | p95 | — |
| WKWebView host plus attributed helper CPU time | 0.2762 ms | p50 | — |
| WKWebView comparison host-process net live allocation block delta | 229 allocations | p50 | — |
| WKWebView comparison host-process net live allocation block delta | 314 allocations | p95 | — |
| WKWebView comparison host-process net live allocation block delta | 345 allocations | p99 | — |
| WKWebView comparison host-process net live allocation byte delta | 3.93e+04 bytes | p50 | — |
| WKWebView comparison host-process net live allocation byte delta | 4.712e+04 bytes | p95 | — |
| WKWebView comparison host-process net live allocation byte delta | 4.918e+04 bytes | p99 | — |
| WKWebView host plus helpers allocated footprint growth | 0.6719 MiB | p50 | — |
| WKWebView maximum concurrently sampled host-plus-helper footprint | 185.6 MiB | max | — |
| SRUI representation | 910 bytes | exact | — |
| HTML representation | 4420 bytes | exact | — |
| screen capture authorization | 1 boolean | exact | — |

Assertions:

- **PASS** representative fixture preserves exact parent, type, and property semantics — 20 native store nodes and rendered control values plus 20 DOM nodes/elements/rendered properties matched exact fixture records
- **PASS** candidate production state reaches a verified composited target-pixel frame — 40 native and 40 WebKit completions; native content: 20/20 samples passed; sample 19: full composited client-content proof: first=649f334593940dd3 complete=5e0d920f983d4c9e dimensions=958x718/958x718 unmasked=687844/687844 quantized-colors=99/112 non-dominant=12847/60937 nonblank-and-nonuniform=true material-pixels=65826/8 max-channel-delta=219/255 tolerance=2/255 materially-distinct=true same-geometry-and-normalization=true provenance=screencapturekit_first_complete_target_frame_status_level_25_appkit_active/screencapturekit_first_complete_target_frame_status_level_25_appkit_active; WebKit content: 20/20 samples passed; sample 19: full composited client-content proof: first=cbf3face39a325f5 complete=6f710960c1598ce4 dimensions=958x718/958x718 unmasked=687844/687844 quantized-colors=34/34 non-dominant=2662/17694 nonblank-and-nonuniform=true material-pixels=15846/8 max-channel-delta=255/255 tolerance=2/255 materially-distinct=true same-geometry-and-normalization=true provenance=screencapturekit_first_complete_target_frame_status_level_25_appkit_inactive/screencapturekit_first_complete_target_frame_status_level_25_appkit_inactive; four disjoint passes per sample: hidden/offscreen warm and reset precede first-state and complete-sequence visual passes; each visual interval begins immediately before production protobuf decode/apply/render and ends at the accepted ScreenCaptureKit frame displayTime; CPU/host-net-live-allocation/footprint-growth use a separate production decode/apply/display-submission interval with no ScreenCaptureKit; peak footprint uses a sampler-only production decode/apply/display-submission pass; four disjoint passes per sample: hidden/offscreen warm and reset precede first-state and complete-sequence visual passes; each visual interval begins immediately before the corresponding hidden WebKit load sequence and ends at the accepted ScreenCaptureKit frame displayTime; CPU/host-net-live-allocation/footprint-growth use a separate hidden-load plus display-submission interval with no ScreenCaptureKit; peak footprint uses a sampler-only hidden-load plus display-submission pass
- **PASS** WebKit helper resources use exact measured process attribution — host 2813, helpers [2816, 2817, 2819, 2880, 2881, 2909, 2910, 2937, 2938, 2966, 2967, 2996, 2997, 3026, 3027, 3056, 3057, 3089, 3090, 3120, 3121, 3147, 3148, 3149, 3150, 3180, 3181, 3212, 3213, 3242, 3243, 3270, 3271, 3298, 3299, 3326, 3327, 3354, 3355, 3381, 3382]; no process-name matching
- **PASS** signed net live allocation samples have exact host-process scope — SRUI blocks=20, bytes=20, scope=default malloc zone in the SRUI renderer host process only; signed after-minus-before net live state, not cumulative allocation events; WebKit control blocks=20, bytes=20, scope=default malloc zone in the WebKit comparison host process only; excludes WebContent, Network, and GPU helper processes; signed after-minus-before net live state, not cumulative allocation events
- **PASS** WindowServer isolation rejects an exact synthetic occluder — window isolation self-test passed: dock=20 status=25 ahead=26 popup=101 target=443781 occluder=443782
- **PASS** renderer candidate launch failures cannot leak a child process — forced candidate identity failure terminated the exact candidate process group and proved both the child and a real descendant gone; a forced descendant-identity publication failure and a forced internal candidate failure also proved both exact identities gone before the benchmark driver returned

Notes:

- Every accepted full-paint state requires an exact visible CGWindow, exact client/target geometry, unobscured z-order, and authorized nonblank/nonuniform ScreenCaptureKit pixels from one complete frame. Latency is action-start Mach time through that accepted frame's SCStream displayTime; callback receipt and pixel hashing are verifier metadata, not the presentation timestamp. The two reported timed states per logical sample produced native=40 and WebKit=40 verified captures; capture_authorization=true. Warm/resource/peak passes are not counted in that metric. No permission request is issued.
- Each native logical sample uses four separately warmed/reset renderer instances: first-state visual latency, complete two-transaction visual latency, CPU/host-net-live-allocation/footprint-growth through production display submission, and sampler-only peak footprint. Each WebKit sample resets one warmed view between the same four disjoint workloads while preserving exact helper PID identities. Neither resource pass creates ScreenCaptureKit buffers, and the footprint sampler never runs in the CPU/allocation pass.
- Native first timing decodes and applies revision 0→1 through production initial attach. Native complete timing decodes/applies 0→1 and then applies 1→2 through production incremental apply. WebKit uses structurally equivalent first and complete DOM states. The measured instance is checked immediately after each accepted presentation for exact semantic/control or DOM state. Full mode additionally requires nonblank, nonuniform, equal-geometry client-content fingerprints that differ between first and complete states.
- Each candidate process group is birth-identity verified. WebKit CPU, footprint growth, and peak footprint aggregate the host with exact benchmark-only WebContent/network/GPU diagnostic PIDs. Allocation samples are signed default-zone malloc_zone_statistics after-minus-before deltas: blocks_in_use and size_in_use describe net live state, not cumulative allocation traffic. Allocation scope: default malloc zone in the SRUI renderer host process only; signed after-minus-before net live state, not cumulative allocation events. Control scope: default malloc zone in the WebKit comparison host process only; excludes WebContent, Network, and GPU helper processes; signed after-minus-before net live state, not cumulative allocation events. Peak footprint is the maximum simultaneous current-footprint sample at 1 ms cadence strictly inside its separate representative pass, not a sum of per-process lifetime maxima.
- The warmed WKWebView candidate and its host-only allocator deltas are comparison controls only; they are not the production SRUI renderer, do not cover WebKit helper-process allocations, and do not describe SRUI's native AppKit rendering path.

## §31.2 Serialization

Samples:

- `rust.generation`: 500
- `rust.serialization`: 500

| Metric | Value | Statistic | Target |
|---|---:|---|---:|
| abstract state generation | 0.003125 ms | p50 | — |
| abstract state generation | 0.006292 ms | p95 | — |
| abstract state generation | 0.01496 ms | p99 | — |
| protobuf serialization | 0.007583 ms | p50 | — |
| protobuf serialization | 0.01275 ms | p95 | — |
| protobuf serialization | 0.03571 ms | p99 | — |
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
| 1 updates semantic decode/apply | 0.01717 ms | p50 | — |
| 1 updates semantic decode/apply | 0.01854 ms | p95 | — |
| 1 updates semantic decode/apply | 0.09721 ms | p99 | — |
| 1 updates decode-to-visible | 16.46 ms | p50 | — |
| 1 updates decode-to-visible | 19.08 ms | p95 | — |
| 1 updates decode-to-visible | 27.1 ms | p99 | — |
| 1 updates wire bytes | 28 bytes | exact | — |
| 1 updates message count | 1 messages | exact | — |
| 100 updates semantic decode/apply | 0.1055 ms | p50 | ≤ 1 ms |
| 100 updates semantic decode/apply | 0.1097 ms | p95 | — |
| 100 updates semantic decode/apply | 0.1152 ms | p99 | — |
| 100 updates decode-to-visible | 18.16 ms | p50 | — |
| 100 updates decode-to-visible | 19.17 ms | p95 | — |
| 100 updates decode-to-visible | 19.29 ms | p99 | — |
| 100 updates wire bytes | 2109 bytes | exact | — |
| 100 updates message count | 1 messages | exact | — |
| 1000 updates semantic decode/apply | 0.8776 ms | p50 | ≤ 5 ms |
| 1000 updates semantic decode/apply | 0.9077 ms | p95 | — |
| 1000 updates semantic decode/apply | 0.9434 ms | p99 | — |
| 1000 updates decode-to-visible | 19.31 ms | p50 | — |
| 1000 updates decode-to-visible | 20.44 ms | p95 | — |
| 1000 updates decode-to-visible | 20.5 ms | p99 | — |
| 1000 updates wire bytes | 2.101e+04 bytes | exact | — |
| 1000 updates message count | 1 messages | exact | — |
| 1 updates at 60Hz decode-to-visible | 33.51 ms | sample | — |
| 1 updates at 60Hz total SRUI wire bytes | 322 bytes | exact | — |
| 1 updates at 60Hz total SRUI message count | 4 messages | exact | — |
| 1 updates at 60Hz inbound TRANSACTION bytes | 28 bytes | exact | — |
| 1 updates at 60Hz inbound TRANSACTION message count | 1 messages | exact | — |
| 1 updates at 60Hz outbound EVENT bytes | 294 bytes | exact | — |
| 1 updates at 60Hz outbound EVENT message count | 3 messages | exact | — |
| 1 updates at 60Hz synthetic change-gated repaint count | 1 repaints | exact | — |
| 1 updates at 120Hz decode-to-visible | 27.62 ms | sample | — |
| 1 updates at 120Hz total SRUI wire bytes | 322 bytes | exact | — |
| 1 updates at 120Hz total SRUI message count | 4 messages | exact | — |
| 1 updates at 120Hz inbound TRANSACTION bytes | 28 bytes | exact | — |
| 1 updates at 120Hz inbound TRANSACTION message count | 1 messages | exact | — |
| 1 updates at 120Hz outbound EVENT bytes | 294 bytes | exact | — |
| 1 updates at 120Hz outbound EVENT message count | 3 messages | exact | — |
| 1 updates at 120Hz synthetic change-gated repaint count | 1 repaints | exact | — |
| 1 updates at 144Hz decode-to-visible | 24.7 ms | sample | — |
| 1 updates at 144Hz total SRUI wire bytes | 322 bytes | exact | — |
| 1 updates at 144Hz total SRUI message count | 4 messages | exact | — |
| 1 updates at 144Hz inbound TRANSACTION bytes | 28 bytes | exact | — |
| 1 updates at 144Hz inbound TRANSACTION message count | 1 messages | exact | — |
| 1 updates at 144Hz outbound EVENT bytes | 294 bytes | exact | — |
| 1 updates at 144Hz outbound EVENT message count | 3 messages | exact | — |
| 1 updates at 144Hz synthetic change-gated repaint count | 1 repaints | exact | — |
| 1 updates at 240Hz decode-to-visible | 24.76 ms | sample | — |
| 1 updates at 240Hz total SRUI wire bytes | 322 bytes | exact | — |
| 1 updates at 240Hz total SRUI message count | 4 messages | exact | — |
| 1 updates at 240Hz inbound TRANSACTION bytes | 28 bytes | exact | — |
| 1 updates at 240Hz inbound TRANSACTION message count | 1 messages | exact | — |
| 1 updates at 240Hz outbound EVENT bytes | 294 bytes | exact | — |
| 1 updates at 240Hz outbound EVENT message count | 3 messages | exact | — |
| 1 updates at 240Hz synthetic change-gated repaint count | 1 repaints | exact | — |
| 100 updates at 60Hz decode-to-visible | 335.9 ms | sample | — |
| 100 updates at 60Hz total SRUI wire bytes | 3094 bytes | exact | — |
| 100 updates at 60Hz total SRUI message count | 103 messages | exact | — |
| 100 updates at 60Hz inbound TRANSACTION bytes | 2800 bytes | exact | — |
| 100 updates at 60Hz inbound TRANSACTION message count | 100 messages | exact | — |
| 100 updates at 60Hz outbound EVENT bytes | 294 bytes | exact | — |
| 100 updates at 60Hz outbound EVENT message count | 3 messages | exact | — |
| 100 updates at 60Hz synthetic change-gated repaint count | 13 repaints | exact | — |
| 100 updates at 120Hz decode-to-visible | 408.3 ms | sample | — |
| 100 updates at 120Hz total SRUI wire bytes | 3094 bytes | exact | — |
| 100 updates at 120Hz total SRUI message count | 103 messages | exact | — |
| 100 updates at 120Hz inbound TRANSACTION bytes | 2800 bytes | exact | — |
| 100 updates at 120Hz inbound TRANSACTION message count | 100 messages | exact | — |
| 100 updates at 120Hz outbound EVENT bytes | 294 bytes | exact | — |
| 100 updates at 120Hz outbound EVENT message count | 3 messages | exact | — |
| 100 updates at 120Hz synthetic change-gated repaint count | 23 repaints | exact | — |
| 100 updates at 144Hz decode-to-visible | 420.1 ms | sample | — |
| 100 updates at 144Hz total SRUI wire bytes | 3094 bytes | exact | — |
| 100 updates at 144Hz total SRUI message count | 103 messages | exact | — |
| 100 updates at 144Hz inbound TRANSACTION bytes | 2800 bytes | exact | — |
| 100 updates at 144Hz inbound TRANSACTION message count | 100 messages | exact | — |
| 100 updates at 144Hz outbound EVENT bytes | 294 bytes | exact | — |
| 100 updates at 144Hz outbound EVENT message count | 3 messages | exact | — |
| 100 updates at 144Hz synthetic change-gated repaint count | 26 repaints | exact | — |
| 100 updates at 240Hz decode-to-visible | 533 ms | sample | — |
| 100 updates at 240Hz total SRUI wire bytes | 3094 bytes | exact | — |
| 100 updates at 240Hz total SRUI message count | 103 messages | exact | — |
| 100 updates at 240Hz inbound TRANSACTION bytes | 2800 bytes | exact | — |
| 100 updates at 240Hz inbound TRANSACTION message count | 100 messages | exact | — |
| 100 updates at 240Hz outbound EVENT bytes | 294 bytes | exact | — |
| 100 updates at 240Hz outbound EVENT message count | 3 messages | exact | — |
| 100 updates at 240Hz synthetic change-gated repaint count | 42 repaints | exact | — |
| 1000 updates at 60Hz decode-to-visible | 6368 ms | sample | — |
| 1000 updates at 60Hz total SRUI wire bytes | 3.004e+04 bytes | exact | — |
| 1000 updates at 60Hz total SRUI message count | 1003 messages | exact | — |
| 1000 updates at 60Hz inbound TRANSACTION bytes | 2.975e+04 bytes | exact | — |
| 1000 updates at 60Hz inbound TRANSACTION message count | 1000 messages | exact | — |
| 1000 updates at 60Hz outbound EVENT bytes | 297 bytes | exact | — |
| 1000 updates at 60Hz outbound EVENT message count | 3 messages | exact | — |
| 1000 updates at 60Hz synthetic change-gated repaint count | 247 repaints | exact | — |
| 1000 updates at 120Hz decode-to-visible | 6360 ms | sample | — |
| 1000 updates at 120Hz total SRUI wire bytes | 3.004e+04 bytes | exact | — |
| 1000 updates at 120Hz total SRUI message count | 1003 messages | exact | — |
| 1000 updates at 120Hz inbound TRANSACTION bytes | 2.975e+04 bytes | exact | — |
| 1000 updates at 120Hz inbound TRANSACTION message count | 1000 messages | exact | — |
| 1000 updates at 120Hz outbound EVENT bytes | 297 bytes | exact | — |
| 1000 updates at 120Hz outbound EVENT message count | 3 messages | exact | — |
| 1000 updates at 120Hz synthetic change-gated repaint count | 368 repaints | exact | — |
| 1000 updates at 144Hz decode-to-visible | 6362 ms | sample | — |
| 1000 updates at 144Hz total SRUI wire bytes | 3.004e+04 bytes | exact | — |
| 1000 updates at 144Hz total SRUI message count | 1003 messages | exact | — |
| 1000 updates at 144Hz inbound TRANSACTION bytes | 2.975e+04 bytes | exact | — |
| 1000 updates at 144Hz inbound TRANSACTION message count | 1000 messages | exact | — |
| 1000 updates at 144Hz outbound EVENT bytes | 297 bytes | exact | — |
| 1000 updates at 144Hz outbound EVENT message count | 3 messages | exact | — |
| 1000 updates at 144Hz synthetic change-gated repaint count | 403 repaints | exact | — |
| 1000 updates at 240Hz decode-to-visible | 6361 ms | sample | — |
| 1000 updates at 240Hz total SRUI wire bytes | 3.004e+04 bytes | exact | — |
| 1000 updates at 240Hz total SRUI message count | 1003 messages | exact | — |
| 1000 updates at 240Hz inbound TRANSACTION bytes | 2.975e+04 bytes | exact | — |
| 1000 updates at 240Hz inbound TRANSACTION message count | 1000 messages | exact | — |
| 1000 updates at 240Hz outbound EVENT bytes | 297 bytes | exact | — |
| 1000 updates at 240Hz outbound EVENT message count | 3 messages | exact | — |
| 1000 updates at 240Hz synthetic change-gated repaint count | 478 repaints | exact | — |
| settled idle SRUI wire bytes | 0 bytes | observed max | — |
| settled idle SRUI message count | 0 messages | observed max | — |

Assertions:

- **PASS** decode-to-visible samples reach an unforced composited content change — every framed production SessionController sample matched isolated semantic state and changed the exact visible client-content fingerprint without benchmark-forced invalidation; every cadence stream ended in its expected native value with passive composited evidence
- **PASS** settled idle UI emits zero SRUI traffic — 1000ms after all production EVENT ACKs drained for every count/cadence; byte deltas [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], message deltas [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]. This does not claim natural AppKit idle invalidation behavior: the synthetic cadence gate suppresses unchanged-state draws by construction.
- **PASS** complete bidirectional wire bytes and message count are cadence independent — the same prebuilt 1/100/1000 inbound TRANSACTION frame sequences were reused at 60/120/144/240Hz; every outbound transport delta is exactly the three decoded production EVENT frames with no extra, dropped, or interrupted attempts; total, inbound, and outbound bytes/messages are reported separately and compared across cadences; ACKs are injected only after these counters and the presentation timestamp are frozen
- **PASS** synthetic native-change-gated repaint count varies independently — 1@60Hz=1, 1@120Hz=1, 1@144Hz=1, 1@240Hz=1, 100@60Hz=13, 100@120Hz=23, 100@144Hz=26, 100@240Hz=42, 1000@60Hz=247, 1000@120Hz=368, 1000@144Hz=403, 1000@240Hz=478. This diagnostic measures configured coalescing opportunities; it is not evidence of AppKit's natural invalidation count.
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
| caret movement at 0ms RTT | 19.02 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| caret movement at 0ms RTT | 19.54 ms | p95 | — |
| caret movement at 0ms RTT | 19.62 ms | p99 | — |
| hover at 0ms RTT | 15.12 ms | p50 | ≤ 8.33333 ms |
| hover at 0ms RTT | 19.06 ms | p95 | — |
| hover at 0ms RTT | 21.63 ms | p99 | — |
| ime composition at 0ms RTT | 18.87 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| ime composition at 0ms RTT | 19.36 ms | p95 | — |
| ime composition at 0ms RTT | 19.37 ms | p99 | — |
| menu opening at 0ms RTT | 41.52 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| menu opening at 0ms RTT | 49.34 ms | p95 | — |
| menu opening at 0ms RTT | 58.6 ms | p99 | — |
| pressed at 0ms RTT | 19.03 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| pressed at 0ms RTT | 19.93 ms | p95 | — |
| pressed at 0ms RTT | 20.01 ms | p99 | — |
| scrolling at 0ms RTT | 18.88 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| scrolling at 0ms RTT | 19.65 ms | p95 | — |
| scrolling at 0ms RTT | 19.69 ms | p99 | — |
| text entry at 0ms RTT | 18.73 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| text entry at 0ms RTT | 19.37 ms | p95 | — |
| text entry at 0ms RTT | 19.39 ms | p99 | — |
| text selection at 0ms RTT | 19.06 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| text selection at 0ms RTT | 19.75 ms | p95 | — |
| text selection at 0ms RTT | 19.78 ms | p99 | — |
| server-dependent input-to-visible at 0ms RTT | 11 ms | p50 | — |
| server-dependent input-to-visible at 0ms RTT | 19.95 ms | p95 | — |
| server-dependent input-to-visible at 0ms RTT | 19.95 ms | p99 | — |
| caret movement at 100ms RTT | 19.37 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| caret movement at 100ms RTT | 19.93 ms | p95 | — |
| caret movement at 100ms RTT | 19.97 ms | p99 | — |
| hover at 100ms RTT | 17.63 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| hover at 100ms RTT | 21.2 ms | p95 | — |
| hover at 100ms RTT | 23.35 ms | p99 | — |
| ime composition at 100ms RTT | 19.42 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| ime composition at 100ms RTT | 20.1 ms | p95 | — |
| ime composition at 100ms RTT | 20.55 ms | p99 | — |
| menu opening at 100ms RTT | 41.45 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| menu opening at 100ms RTT | 51.14 ms | p95 | — |
| menu opening at 100ms RTT | 60.93 ms | p99 | — |
| pressed at 100ms RTT | 18.08 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| pressed at 100ms RTT | 19.34 ms | p95 | — |
| pressed at 100ms RTT | 19.64 ms | p99 | — |
| scrolling at 100ms RTT | 19.68 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| scrolling at 100ms RTT | 25.43 ms | p95 | — |
| scrolling at 100ms RTT | 25.5 ms | p99 | — |
| text entry at 100ms RTT | 18.91 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| text entry at 100ms RTT | 19.29 ms | p95 | — |
| text entry at 100ms RTT | 19.43 ms | p99 | — |
| text selection at 100ms RTT | 19.25 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| text selection at 100ms RTT | 20.04 ms | p95 | — |
| text selection at 100ms RTT | 20.19 ms | p99 | — |
| server-dependent input-to-visible at 100ms RTT | 118.9 ms | p50 | — |
| server-dependent input-to-visible at 100ms RTT | 127.2 ms | p95 | — |
| server-dependent input-to-visible at 100ms RTT | 127.2 ms | p99 | — |
| caret movement at 300ms RTT | 19.17 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| caret movement at 300ms RTT | 19.53 ms | p95 | — |
| caret movement at 300ms RTT | 29.93 ms | p99 | — |
| hover at 300ms RTT | 19 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| hover at 300ms RTT | 25.56 ms | p95 | — |
| hover at 300ms RTT | 39.84 ms | p99 | — |
| ime composition at 300ms RTT | 18.86 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| ime composition at 300ms RTT | 19.64 ms | p95 | — |
| ime composition at 300ms RTT | 20.2 ms | p99 | — |
| menu opening at 300ms RTT | 41.94 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| menu opening at 300ms RTT | 48.66 ms | p95 | — |
| menu opening at 300ms RTT | 50.09 ms | p99 | — |
| pressed at 300ms RTT | 18.39 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| pressed at 300ms RTT | 18.97 ms | p95 | — |
| pressed at 300ms RTT | 19.56 ms | p99 | — |
| scrolling at 300ms RTT | 19.37 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| scrolling at 300ms RTT | 26.06 ms | p95 | — |
| scrolling at 300ms RTT | 26.22 ms | p99 | — |
| text entry at 300ms RTT | 18.94 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| text entry at 300ms RTT | 19.66 ms | p95 | — |
| text entry at 300ms RTT | 20.38 ms | p99 | — |
| text selection at 300ms RTT | 19.24 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| text selection at 300ms RTT | 20.12 ms | p95 | — |
| text selection at 300ms RTT | 20.28 ms | p99 | — |
| server-dependent input-to-visible at 300ms RTT | 328.1 ms | p50 | — |
| server-dependent input-to-visible at 300ms RTT | 336.8 ms | p95 | — |
| server-dependent input-to-visible at 300ms RTT | 336.8 ms | p99 | — |
| caret movement at 600ms RTT | 19.3 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| caret movement at 600ms RTT | 19.97 ms | p95 | — |
| caret movement at 600ms RTT | 20.35 ms | p99 | — |
| hover at 600ms RTT | 18.68 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| hover at 600ms RTT | 21.6 ms | p95 | — |
| hover at 600ms RTT | 21.71 ms | p99 | — |
| ime composition at 600ms RTT | 19.26 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| ime composition at 600ms RTT | 19.75 ms | p95 | — |
| ime composition at 600ms RTT | 19.83 ms | p99 | — |
| menu opening at 600ms RTT | 41 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| menu opening at 600ms RTT | 42.92 ms | p95 | — |
| menu opening at 600ms RTT | 49.76 ms | p99 | — |
| pressed at 600ms RTT | 19.13 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| pressed at 600ms RTT | 20.42 ms | p95 | — |
| pressed at 600ms RTT | 27.41 ms | p99 | — |
| scrolling at 600ms RTT | 19.42 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| scrolling at 600ms RTT | 26.81 ms | p95 | — |
| scrolling at 600ms RTT | 27.36 ms | p99 | — |
| text entry at 600ms RTT | 19.3 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| text entry at 600ms RTT | 20.16 ms | p95 | — |
| text entry at 600ms RTT | 20.67 ms | p99 | — |
| text selection at 600ms RTT | 19.2 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| text selection at 600ms RTT | 19.88 ms | p95 | — |
| text selection at 600ms RTT | 20.74 ms | p99 | — |
| server-dependent input-to-visible at 600ms RTT | 643.7 ms | p50 | — |
| server-dependent input-to-visible at 600ms RTT | 652.4 ms | p95 | — |
| server-dependent input-to-visible at 600ms RTT | 652.4 ms | p99 | — |
| 1MiB/s bandwidth-limited production event | 17.4 ms | p50 | — |
| 1MiB/s bandwidth-limited production event | 17.93 ms | p95 | — |
| bandwidth-limited delivered bytes | 4.948e+04 bytes | exact | — |
| deterministic production loss attempts | 2 messages | exact | — |
| deterministic production loss delivered messages | 1 messages | exact | — |
| controlled production interruption detection | 19.37 ms | p50 | — |
| measured production session wire bytes | 1.415e+05 bytes | exact | — |
| measured production session wire messages | 1240 messages | exact | — |
| maximum RTT-induced local latency delta | 3.871 ms | p50 | ≤ 8.33333 ms |
| maximum RTT-induced local latency delta | 7.161 ms | p95 | — |
| maximum RTT-induced local latency delta | 18.21 ms | p99 | — |

Assertions:

- **PASS** mounted local interactions do not acquire one RTT — largest p50 increase 3.8706 ms versus the measured local frame budget of 8.3333 ms; paired injected transaction remained blocked through local visible completion in 640/640 probes=true; configured delay state was verified at the exact action boundary in 640/640 probes=true, with a nonzero transport delay still active in 480/480 nonzero-RTT probes; local_state_checks=true; descriptive p95/p99 deltas were 7.1608/18.2087 ms; production renderer callbacks=240
- **PASS** native text entry emits and settles one exact production TEXT_EDIT — RTT 0ms: node=14 final_text="ééééééééééééééééééééview the changes and run tests.xxxxxxxxxxxxxxxxxxxx" edit_seq=40 observed_revision=81 matching_framed_events=1 exact_slot_ack=true stable_empty_tail=true; RTT 100ms: node=14 final_text="ééééééééééééééééééééview the changes and run tests.xxxxxxxxxxxxxxxxxxxx" edit_seq=40 observed_revision=81 matching_framed_events=1 exact_slot_ack=true stable_empty_tail=true; RTT 300ms: node=14 final_text="ééééééééééééééééééééview the changes and run tests.xxxxxxxxxxxxxxxxxxxx" edit_seq=40 observed_revision=80 matching_framed_events=1 exact_slot_ack=true stable_empty_tail=true; RTT 600ms: node=14 final_text="ééééééééééééééééééééview the changes and run tests.xxxxxxxxxxxxxxxxxxxx" edit_seq=40 observed_revision=80 matching_framed_events=1 exact_slot_ack=true stable_empty_tail=true
- **PASS** injected transport RTT affects production server-dependent feedback — SessionController EventOutbox sends and framed transaction responses tracked 100/300/600ms RTT
- **PASS** local visible completion does not await an injected transport response — Each local action began only after its exact compositor baseline was ready and while its configured BenchmarkTransport delay state was verified; 480/480 nonzero-RTT actions began during an active delay. Every paired production transaction remained blocked through visible completion; the gate was released only afterward. Separately, production server-dependent feedback tracked 100/300/600ms RTT.
- **PASS** bandwidth delay, loss, and interruption exercise session recovery — sample 1: 16493 framed bytes / 1 message: measured 17.9341 ms >= theoretical 15.7290 ms; exact_event=true; sample 2: 16493 framed bytes / 1 message: measured 17.3960 ms >= theoretical 15.7290 ms; exact_event=true; sample 3: 16493 framed bytes / 1 message: measured 16.6935 ms >= theoretical 15.7290 ms; exact_event=true; 49479 total bandwidth bytes; lost and interrupted events remained in EventOutbox and replayed through replacement SessionControllers

Notes:

- Controls are mounted renderer TextArea, ScrollView, and Button. renderer-produced NSButton context menu was proven as a new exact owned menu-level WindowServer surface in the same ScreenCaptureKit frame used for its presentation timestamp.
- Pressed state is observed between real NSWindow-dispatched mouseDown/mouseUp events and triggers the production ActionTrampoline. renderer-produced NSButton changed at least 1590 target-ROI pixels above the explicit 2/255 per-channel SCStream tolerance on the local AppKit hover path in 20/20 samples; mouseExited then restored every unmasked screenshot pixel within the explicit 5/255 same-API tolerance, with maximum observed channel delta 5/255. Permission-free hover invokes the renderer-produced NSButton's own AppKit entry/exit path and does not claim WindowServer pointer latency.
- Local frame budget 8.333333 ms came from CGDisplayMode.refreshRate for the benchmark NSScreen.
- All impairment traffic traverses SessionController, EventOutbox, SRUIFraming, and replacement-session resume/replay; no benchmark calls Transport.send directly.
- For each 1 MiB/s sample, the proof takes the exact outbound framed-byte delta around one awaited SessionController.sendValueChanged call, requires exactly one matching event frame, and requires elapsed wall time >= framed bytes / 1,048,576 bytes/s. Encoding and outbox overhead are inside the measured interval and can only increase that elapsed time.
- The RTT-independence correctness gate requires the worst p50 delta to stay within the measured local display-frame budget and also requires every one of the 640 exact held-response probes. That probe proves non-dependence on delivery of its exact paired transaction; it does not claim the configured one-way-delay interval remained active throughout the action. Unpaired p95/p99 WindowServer tails remain descriptive diagnostics and §23 follow-ups.

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
| disconnect immediately before event receipt | 6.875 ms | p50 | — |
| disconnect immediately before event receipt | 7.083 ms | p95 | — |
| disconnect immediately before event receipt | 7.331 ms | p99 | — |
| event receipt through settled side effect | 0.000542 ms | p50 | — |
| event receipt through settled side effect | 0.001709 ms | p95 | — |
| event receipt through settled side effect | 0.004042 ms | p99 | — |
| in-process cached DUPLICATE response | 0.000208 ms | p50 | — |
| in-process cached DUPLICATE response | 0.000458 ms | p95 | — |
| in-process cached DUPLICATE response | 0.000667 ms | p99 | — |
| lost ACK wire reconnect through DUPLICATE acknowledgement | 0.096 ms | p50 | — |
| lost ACK wire reconnect through DUPLICATE acknowledgement | 0.2084 ms | p95 | — |
| lost ACK wire reconnect through DUPLICATE acknowledgement | 0.3151 ms | p99 | — |
| mid-resource reconnect and exact replay | 0.00375 ms | p50 | — |
| mid-resource reconnect and exact replay | 0.01246 ms | p95 | — |
| mid-resource reconnect and exact replay | 0.02471 ms | p99 | — |
| mid-transaction frame discard and atomic replay | 0.000875 ms | p50 | — |
| mid-transaction frame discard and atomic replay | 0.002625 ms | p95 | — |
| mid-transaction frame discard and atomic replay | 0.008083 ms | p99 | — |
| mid-transaction wire disconnect and exact atomic replay | 6.874 ms | p50 | — |
| mid-transaction wire disconnect and exact atomic replay | 7.42 ms | p95 | — |
| mid-transaction wire disconnect and exact atomic replay | 8.908 ms | p99 | — |
| partial EVENT disconnect and one processed replay | 7.311 ms | p50 | — |
| partial EVENT disconnect and one processed replay | 8.366 ms | p95 | — |
| partial EVENT disconnect and one processed replay | 8.924 ms | p99 | — |
| resume beyond journal retention | 0.000708 ms | p50 | — |
| resume beyond journal retention | 0.001875 ms | p95 | — |
| resume beyond journal retention | 0.003 ms | p99 | — |
| resume within journal retention | 0.000625 ms | p50 | — |
| resume within journal retention | 0.001625 ms | p95 | — |
| resume within journal retention | 0.002417 ms | p99 | — |
| pre-receipt retained-event replay | 20.9 ms | p50 | — |
| pre-receipt retained-event replay | 21.36 ms | p95 | — |
| mid-resource reconnect recovery | 0.6801 ms | p50 | — |
| mid-resource reconnect recovery | 2.79 ms | p95 | — |
| superseded resume response handling | 0.1477 ms | p50 | — |
| superseded resume response handling | 0.4497 ms | p95 | — |
| active resume response handling | 0.2365 ms | p50 | — |
| active resume response handling | 0.3669 ms | p95 | — |
| production reconnect boundary suite | 3.245e+04 ms | wall | — |

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
| embedded SRUI PTY exact ANSI capture and framing | 10.2 ms | p50 | — |
| embedded SRUI PTY exact ANSI capture and framing | 11.84 ms | p95 | — |
| embedded SRUI PTY exact ANSI capture and framing | 15.23 ms | p99 | — |
| standalone PTY exact ANSI interaction | 8.883 ms | p50 | — |
| standalone PTY exact ANSI interaction | 9.813 ms | p95 | — |
| standalone PTY exact ANSI interaction | 24.17 ms | p99 | — |
| terminal reconnect retention-loss decision | 0.000416 ms | sample | — |
| terminal payload | 6912 bytes | exact | — |
| embedded terminal frame count | 1 messages | p50 | — |
| embedded terminal frame count | 1 messages | p95 | — |
| embedded terminal frame count | 1 messages | p99 | — |
| embedded SRUI Terminal decode-to-visible | 18.3 ms | p50 | — |
| embedded SRUI Terminal decode-to-visible | 19.75 ms | p95 | — |
| embedded SRUI Terminal decode-to-visible | 20.34 ms | p99 | — |
| embedded SRUI Terminal draw-only | 11.11 ms | p50 | — |
| embedded SRUI Terminal draw-only | 12.77 ms | p95 | — |
| embedded SRUI Terminal draw-only | 13.36 ms | p99 | — |
| standalone TerminalSession and TerminalView decode-to-visible | 17.98 ms | p50 | — |
| standalone TerminalSession and TerminalView decode-to-visible | 26.08 ms | p95 | — |
| standalone TerminalSession and TerminalView decode-to-visible | 27.75 ms | p99 | — |
| standalone TerminalView draw-only | 11.11 ms | p50 | — |
| standalone TerminalView draw-only | 25.18 ms | p95 | — |
| standalone TerminalView draw-only | 26.81 ms | p99 | — |
| embedded-to-standalone terminal decode-to-visible | 1.018 ratio | p50 | — |
| embedded-to-standalone terminal draw-only | 1 ratio | p50 | — |
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
- **PASS** standalone and embedded terminal visible completions are actual draws — 20/20 embedded and 20/20 standalone TerminalView completions crossed the configured draw-visible boundary and produced content-distinct bitmaps; full mode additionally required nonblank, nonuniform client-only composited captures; embedded visibility provenance: ["screencapturekit_same_complete_frame_target_roi_after_action_screencapturekit_composited_baseline_status_level_25_appkit_occlusion_8192"]; standalone visibility provenance: ["screencapturekit_same_complete_frame_target_roi_after_action_screencapturekit_composited_baseline_status_level_25_appkit_occlusion_8192"]; embedded composited fingerprints: ["155abc151eb49624f6986b71e8b8a2e624404402bcfea96a39a4fc6d07d0b9b0", "9107048765ca0676087d378bbc3408fcf7150b7edac6e4553160199db2d50680", "e12a10df9e31e7306d6595f1815ecd51e18d9b1753929c861a5c1b3004fe1119", "fc60cbce266e228114f3f652b127a27095a1e6eada28e3557786f75fcc6ed656"]; standalone composited fingerprints: ["081792e3d8512a4073f4a3d0399bf805dd51674d64d01fc12d5371ae38b3d91e", "2d614d0c9309419badeb0ae69584022aec10e25724d939960f966e72701cc63b", "4ac918017631538affcc03def5d6ae0355944300802c7cef241d4bcc138c07d1", "4e7d55df2dd7083d7d18bd16491516cb3344000430584e11f5bb0d8e51d446c8", "5e6f8fe8685105e30ba2d4f145526e52d89006b90268bbe9222e5dc7822ea43c", "6e7b552fa8d502e4613f7ef0c582ddd4edc190caba8b7eb89bd64cc636148ec8", "6ee3bee1919b11e2b38d6dba0499d416aa7e6f9d8e67108cd1da1720e6aee086", "71ae709cdd2ca95583ab7d5aef19995e3c97321d1c2ab38ecb9fcf1310f40dd2", "7ae139d053a5a8b0428abd4948035bac572376b62c2ff36e5972517eecb27620", "9971b36792fdf5a3dfacf2a3d08d2fcdfde06bb4b13d1d8acef46c1ef0f294b0", "9ff428d6cee5302e6a63fa6d160aa65683a9e0aebf84c30ffd808211abdd40ce", "a961ea7cd793b6469799f7d4d9a94108f6e132d9dec20f8573afafeb5c79371e", "ac5894e2c1d2b5b335588e09c4dd70c9780a27cd8581d5e0e026f9419f120fd8", "cc1fd55c91d1262d0aab6b6aba6b90efa90d0fe389480381e87ddfc269bb5493", "ced359078549d76d5ca402f5698cc3eb2aa1804a980522898e5456ed9b9eff19", "dc649dc018cb1f42b1071f140e46a2a81f182543a55b490c5c8de3453b023541", "dcd1ae50608bc81879655e248127f890745e4f4e910d57736ee88cca70f286c5", "dee2db4a4f774deb01474961cbd9e3a9a25b45b1ef23c6d76cda1f20fd766a77", "fbb4326489e087661ae6d00412fa1128ba81813921e9aad7e4ef772679f76891"]
- **PASS** standalone and embedded terminal displays render the identical ANSI payload — both paths consumed payload SHA-256 e4461bba73f99c7bc70ba6041b859c3e59446d6c80d1549c25de7bcaf933a910, decoded exact 256-line content SHA-256 e1e85bdbda198ce2b94b317765b18ec90b981d25c0ef567ff83d102f6817fe8d, and produced byte-identical TerminalView rasters; embedded content digests: ["e1e85bdbda198ce2b94b317765b18ec90b981d25c0ef567ff83d102f6817fe8d"]; standalone content digests: ["e1e85bdbda198ce2b94b317765b18ec90b981d25c0ef567ff83d102f6817fe8d"]; embedded bitmap digests: ["e440b40246b19d42137da63cb2cc233d9e18702986ff6269b4f6ac33337dbab5"]; standalone bitmap digests: ["e440b40246b19d42137da63cb2cc233d9e18702986ff6269b4f6ac33337dbab5"]
- **PASS** standalone and production terminal samples begin from fresh parser and view state — embedded samples negotiate org.srui.terminal/1, register namespace 31, and mount through a framed transaction; standalone samples instantiate fresh production TerminalSession and TerminalView pairs; both reproduce exact content SHA-256 e1e85bdbda198ce2b94b317765b18ec90b981d25c0ef567ff83d102f6817fe8d

Notes:

- Standalone and embedded samples both include production PTY spawn, identical shell execution, EOF, and exact ANSI output; embedded additionally captures and frames the bytes.
- Embedded decode-to-visible starts before a framed 6,912-byte SRUITerminalData enters BenchmarkTransport, then crosses SessionController, the renderer-owned TerminalSession, the renderer-created TerminalView snapshot subscription, and the configured presentation boundary.
- The standalone display baseline feeds the identical Data value at offset zero through fresh production TerminalSession and TerminalView instances, using the embedded view's exact grid and bounds. The reported ratios compare p50 display completion on like-for-like content and geometry.
- Full mode starts an exact TerminalView-ROI ScreenCaptureKit stream and verifies a nonblank, nonuniform baseline before timing. After the production payload action completes, it accepts only a later complete compositor sample whose same-frame ROI fingerprint changed; the reported latency is action-start mach time to that sample's ScreenCaptureKit display time. Exact WindowServer identity, display, client/ROI geometry, and absence of every intersecting visible nonzero-alpha window are checked before and after acceptance. Callback receipt remains metadata rather than the latency clock. Observed embedded provenance: ["screencapturekit_same_complete_frame_target_roi_after_action_screencapturekit_composited_baseline_status_level_25_appkit_occlusion_8192"]; standalone provenance: ["screencapturekit_same_complete_frame_target_roi_after_action_screencapturekit_composited_baseline_status_level_25_appkit_occlusion_8192"]. Smoke uses an explicitly named offscreen bitmap draw and is not presentation evidence.
- The Rust section separately compares exact standalone-versus-embedded PTY byte transport and exercises reconnect ring-buffer exhaustion; this Swift section compares terminal emulator and display completion.

## Follow-up flags

- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 caret movement at 0ms RTT: 19.02 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 ime composition at 0ms RTT: 18.87 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 0ms RTT: 41.52 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 pressed at 0ms RTT: 19.03 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 0ms RTT: 18.88 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text entry at 0ms RTT: 18.73 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text selection at 0ms RTT: 19.06 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 caret movement at 100ms RTT: 19.37 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 hover at 100ms RTT: 17.63 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 ime composition at 100ms RTT: 19.42 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 100ms RTT: 41.45 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 pressed at 100ms RTT: 18.08 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 100ms RTT: 19.68 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text entry at 100ms RTT: 18.91 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text selection at 100ms RTT: 19.25 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 caret movement at 300ms RTT: 19.17 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 hover at 300ms RTT: 19 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 ime composition at 300ms RTT: 18.86 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 300ms RTT: 41.94 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 pressed at 300ms RTT: 18.39 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 300ms RTT: 19.37 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text entry at 300ms RTT: 18.94 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text selection at 300ms RTT: 19.24 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 caret movement at 600ms RTT: 19.3 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 hover at 600ms RTT: 18.68 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 ime composition at 600ms RTT: 19.26 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 600ms RTT: 41 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 pressed at 600ms RTT: 19.13 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 600ms RTT: 19.42 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text entry at 600ms RTT: 19.3 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text selection at 600ms RTT: 19.2 ms vs target 8.33333 ms
