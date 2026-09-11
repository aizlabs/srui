# SRUI benchmark report

- Generated: 2026-09-11T01:36:28.092414+00:00
- Profile: full
- Host: macOS-26.4.1-arm64-arm-64bit-Mach-O / arm64
- Chip: Apple M2 Max
- Physical RAM: 34359738368 bytes
- Xcode: Xcode 26.2 / Build version 17C52
- Swift: Apple Swift version 6.2.3 (swiftlang-6.2.3.3.21 clang-1700.6.3.2) / Target: arm64-apple-macosx26.0 / swift-driver version: 1.127.14.1
- Rust: rustc 1.98.0 (88d9e12ae 2026-08-18)
- Python: 3.13.7
- Git: c21925e6a59512c9d0ff390f764d3375e90d3269 (clean)
- Fixture: benchmarks/fixtures/coding-agent-ui.json

## §31.1 Local renderer

Samples:

- `macos.srui.render`: 20
- `macos.webkit.render`: 20

| Metric | Value | Statistic | Target |
|---|---:|---|---:|
| SRUI first on-screen paint crossing display refresh | 50.31 ms | p50 | — |
| SRUI first on-screen paint crossing display refresh | 53.47 ms | p95 | — |
| SRUI complete on-screen paint crossing display refresh | 62.86 ms | p50 | — |
| SRUI complete on-screen paint crossing display refresh | 68.97 ms | p95 | — |
| SRUI complete on-screen paint crossing display refresh | 183.8 ms | p99 | — |
| SRUI candidate process CPU time | 1.021 ms | p50 | — |
| SRUI candidate process CPU time | 1.047 ms | p95 | — |
| SRUI host-process net live allocation block delta | 2.202e+04 blocks | p50 | — |
| SRUI host-process net live allocation block delta | 2.233e+04 blocks | p95 | — |
| SRUI host-process net live allocation block delta | 2.243e+04 blocks | p99 | — |
| SRUI host-process net live allocation byte delta | 2.173e+06 bytes | p50 | — |
| SRUI host-process net live allocation byte delta | 2.19e+06 bytes | p95 | — |
| SRUI host-process net live allocation byte delta | 2.247e+06 bytes | p99 | — |
| SRUI host allocated footprint growth | 0.9844 MiB | p50 | — |
| SRUI maximum concurrently sampled process footprint | 50.24 MiB | max | — |
| WKWebView first on-screen paint crossing display refresh | 34.51 ms | p50 | — |
| WKWebView first on-screen paint crossing display refresh | 99.89 ms | p95 | — |
| WKWebView complete on-screen paint crossing display refresh | 125.9 ms | p50 | — |
| WKWebView complete on-screen paint crossing display refresh | 137.3 ms | p95 | — |
| WKWebView host plus attributed helper CPU time | 0.2731 ms | p50 | — |
| WKWebView comparison host-process net live allocation block delta | 302 blocks | p50 | — |
| WKWebView comparison host-process net live allocation block delta | 381 blocks | p95 | — |
| WKWebView comparison host-process net live allocation block delta | 399 blocks | p99 | — |
| WKWebView comparison host-process net live allocation byte delta | 4.486e+04 bytes | p50 | — |
| WKWebView comparison host-process net live allocation byte delta | 5.037e+04 bytes | p95 | — |
| WKWebView comparison host-process net live allocation byte delta | 5.099e+04 bytes | p99 | — |
| WKWebView host plus helpers allocated footprint growth | 0.6406 MiB | p50 | — |
| WKWebView maximum concurrently sampled host-plus-helper footprint | 163.1 MiB | max | — |
| SRUI representation | 981 bytes | exact | — |
| HTML representation | 4652 bytes | exact | — |
| screen capture authorization | 1 boolean | exact | — |

Assertions:

- **PASS** representative fixture preserves exact parent, type, and property semantics — expected=21, native=21 semantic=true controls=true, WebKit=21 semantic=true elements-and-properties=true
- **PASS** candidate production state reaches a verified composited target-pixel frame — native presentations=40/40, pixels=40, authorized=true, content=20/20 samples passed; sample 19: full composited client-content proof: first=b5035007cea49854 complete=5322788e1ad2b066 dimensions=958x718/958x718 unmasked=687844/687844 quantized-colors=100/113 non-dominant=12845/63782 nonblank-and-nonuniform=true material-pixels=68991/8 max-channel-delta=219/255 tolerance=2/255 materially-distinct=true same-geometry-and-normalization=true provenance=screencapturekit_first_complete_target_frame_status_level_25_appkit_active/screencapturekit_first_complete_target_frame_status_level_25_appkit_active; WebKit presentations=40/40, pixels=40, authorized=true, content=20/20 samples passed; sample 19: full composited client-content proof: first=b52ca9ee70e20959 complete=e91a297af7e9d8b1 dimensions=958x718/958x718 unmasked=687844/687844 quantized-colors=33/33 non-dominant=2661/19242 nonblank-and-nonuniform=true material-pixels=17400/8 max-channel-delta=255/255 tolerance=2/255 materially-distinct=true same-geometry-and-normalization=true provenance=screencapturekit_first_complete_target_frame_status_level_25_appkit_inactive/screencapturekit_first_complete_target_frame_status_level_25_appkit_inactive; four disjoint passes per sample: hidden/offscreen warm and reset precede first-state and complete-sequence visual passes; each visual interval begins immediately before production protobuf decode/apply/render and ends at the accepted ScreenCaptureKit frame displayTime; CPU/host-net-live-allocation/footprint-growth use a separate production decode/apply/display-submission interval with no ScreenCaptureKit; peak footprint uses a sampler-only production decode/apply/display-submission pass; four disjoint passes per sample: hidden/offscreen warm and reset precede first-state and complete-sequence visual passes; each visual interval begins immediately before the corresponding hidden WebKit load sequence and ends at the accepted ScreenCaptureKit frame displayTime; CPU/host-net-live-allocation/footprint-growth use a separate hidden-load plus display-submission interval with no ScreenCaptureKit; peak footprint uses a sampler-only hidden-load plus display-submission pass
- **PASS** WebKit helper resources use exact measured process attribution — host 24615, helpers [24656, 24657, 24658, 24715, 24716, 24774, 24775, 24820, 24821, 24849, 24850, 24877, 24878, 24904, 24905, 24932, 24933, 24959, 24960, 24987, 24988, 25016, 25017, 25048, 25049, 25066, 25076, 25078, 25079, 25105, 25106, 25133, 25134, 25160, 25161, 25187, 25188, 25214, 25215, 25242, 25243]; no process-name matching
- **PASS** signed net live allocation samples have exact host-process scope — SRUI blocks=20, bytes=20, scope=default malloc zone in the SRUI renderer host process only; signed after-minus-before net live state, not cumulative allocation events; WebKit control blocks=20, bytes=20, scope=default malloc zone in the WebKit comparison host process only; excludes WebContent, Network, and GPU helper processes; signed after-minus-before net live state, not cumulative allocation events
- **PASS** WindowServer isolation rejects an exact synthetic occluder — window isolation self-test passed: dock=20 status=25 ahead=26 popup=101 target=449985 occluder=449986
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
| abstract state generation | 0.006084 ms | p95 | — |
| abstract state generation | 0.01333 ms | p99 | — |
| protobuf serialization | 0.008833 ms | p50 | — |
| protobuf serialization | 0.01358 ms | p95 | — |
| protobuf serialization | 0.03971 ms | p99 | — |
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
| 1 updates semantic decode/apply | 0.016 ms | p50 | — |
| 1 updates semantic decode/apply | 0.01846 ms | p95 | — |
| 1 updates semantic decode/apply | 0.1457 ms | p99 | — |
| 1 updates decode-to-visible | 13.13 ms | p50 | — |
| 1 updates decode-to-visible | 17.87 ms | p95 | — |
| 1 updates decode-to-visible | 17.97 ms | p99 | — |
| 1 updates wire bytes | 28 bytes | exact | — |
| 1 updates message count | 1 messages | exact | — |
| 100 updates semantic decode/apply | 0.1038 ms | p50 | ≤ 1 ms |
| 100 updates semantic decode/apply | 0.11 ms | p95 | — |
| 100 updates semantic decode/apply | 0.1902 ms | p99 | — |
| 100 updates decode-to-visible | 12.13 ms | p50 | — |
| 100 updates decode-to-visible | 17.83 ms | p95 | — |
| 100 updates decode-to-visible | 18.27 ms | p99 | — |
| 100 updates wire bytes | 2109 bytes | exact | — |
| 100 updates message count | 1 messages | exact | — |
| 1000 updates semantic decode/apply | 0.8495 ms | p50 | ≤ 5 ms |
| 1000 updates semantic decode/apply | 0.8828 ms | p95 | — |
| 1000 updates semantic decode/apply | 0.9687 ms | p99 | — |
| 1000 updates decode-to-visible | 19.35 ms | p50 | — |
| 1000 updates decode-to-visible | 20.42 ms | p95 | — |
| 1000 updates decode-to-visible | 20.64 ms | p99 | — |
| 1000 updates wire bytes | 2.101e+04 bytes | exact | — |
| 1000 updates message count | 1 messages | exact | — |
| 1 updates at 60Hz decode-to-visible | 37.26 ms | sample | — |
| 1 updates at 60Hz total SRUI wire bytes | 322 bytes | exact | — |
| 1 updates at 60Hz total SRUI message count | 4 messages | exact | — |
| 1 updates at 60Hz inbound TRANSACTION bytes | 28 bytes | exact | — |
| 1 updates at 60Hz inbound TRANSACTION message count | 1 messages | exact | — |
| 1 updates at 60Hz outbound EVENT bytes | 294 bytes | exact | — |
| 1 updates at 60Hz outbound EVENT message count | 3 messages | exact | — |
| 1 updates at 60Hz synthetic change-gated repaint count | 1 repaints | exact | — |
| 1 updates at 120Hz decode-to-visible | 29.26 ms | sample | — |
| 1 updates at 120Hz total SRUI wire bytes | 322 bytes | exact | — |
| 1 updates at 120Hz total SRUI message count | 4 messages | exact | — |
| 1 updates at 120Hz inbound TRANSACTION bytes | 28 bytes | exact | — |
| 1 updates at 120Hz inbound TRANSACTION message count | 1 messages | exact | — |
| 1 updates at 120Hz outbound EVENT bytes | 294 bytes | exact | — |
| 1 updates at 120Hz outbound EVENT message count | 3 messages | exact | — |
| 1 updates at 120Hz synthetic change-gated repaint count | 1 repaints | exact | — |
| 1 updates at 144Hz decode-to-visible | 24.93 ms | sample | — |
| 1 updates at 144Hz total SRUI wire bytes | 322 bytes | exact | — |
| 1 updates at 144Hz total SRUI message count | 4 messages | exact | — |
| 1 updates at 144Hz inbound TRANSACTION bytes | 28 bytes | exact | — |
| 1 updates at 144Hz inbound TRANSACTION message count | 1 messages | exact | — |
| 1 updates at 144Hz outbound EVENT bytes | 294 bytes | exact | — |
| 1 updates at 144Hz outbound EVENT message count | 3 messages | exact | — |
| 1 updates at 144Hz synthetic change-gated repaint count | 1 repaints | exact | — |
| 1 updates at 240Hz decode-to-visible | 19.28 ms | sample | — |
| 1 updates at 240Hz total SRUI wire bytes | 322 bytes | exact | — |
| 1 updates at 240Hz total SRUI message count | 4 messages | exact | — |
| 1 updates at 240Hz inbound TRANSACTION bytes | 28 bytes | exact | — |
| 1 updates at 240Hz inbound TRANSACTION message count | 1 messages | exact | — |
| 1 updates at 240Hz outbound EVENT bytes | 294 bytes | exact | — |
| 1 updates at 240Hz outbound EVENT message count | 3 messages | exact | — |
| 1 updates at 240Hz synthetic change-gated repaint count | 1 repaints | exact | — |
| 100 updates at 60Hz decode-to-visible | 337 ms | sample | — |
| 100 updates at 60Hz total SRUI wire bytes | 3094 bytes | exact | — |
| 100 updates at 60Hz total SRUI message count | 103 messages | exact | — |
| 100 updates at 60Hz inbound TRANSACTION bytes | 2800 bytes | exact | — |
| 100 updates at 60Hz inbound TRANSACTION message count | 100 messages | exact | — |
| 100 updates at 60Hz outbound EVENT bytes | 294 bytes | exact | — |
| 100 updates at 60Hz outbound EVENT message count | 3 messages | exact | — |
| 100 updates at 60Hz synthetic change-gated repaint count | 13 repaints | exact | — |
| 100 updates at 120Hz decode-to-visible | 421 ms | sample | — |
| 100 updates at 120Hz total SRUI wire bytes | 3094 bytes | exact | — |
| 100 updates at 120Hz total SRUI message count | 103 messages | exact | — |
| 100 updates at 120Hz inbound TRANSACTION bytes | 2800 bytes | exact | — |
| 100 updates at 120Hz inbound TRANSACTION message count | 100 messages | exact | — |
| 100 updates at 120Hz outbound EVENT bytes | 294 bytes | exact | — |
| 100 updates at 120Hz outbound EVENT message count | 3 messages | exact | — |
| 100 updates at 120Hz synthetic change-gated repaint count | 24 repaints | exact | — |
| 100 updates at 144Hz decode-to-visible | 445.5 ms | sample | — |
| 100 updates at 144Hz total SRUI wire bytes | 3094 bytes | exact | — |
| 100 updates at 144Hz total SRUI message count | 103 messages | exact | — |
| 100 updates at 144Hz inbound TRANSACTION bytes | 2800 bytes | exact | — |
| 100 updates at 144Hz inbound TRANSACTION message count | 100 messages | exact | — |
| 100 updates at 144Hz outbound EVENT bytes | 294 bytes | exact | — |
| 100 updates at 144Hz outbound EVENT message count | 3 messages | exact | — |
| 100 updates at 144Hz synthetic change-gated repaint count | 27 repaints | exact | — |
| 100 updates at 240Hz decode-to-visible | 624.8 ms | sample | — |
| 100 updates at 240Hz total SRUI wire bytes | 3094 bytes | exact | — |
| 100 updates at 240Hz total SRUI message count | 103 messages | exact | — |
| 100 updates at 240Hz inbound TRANSACTION bytes | 2800 bytes | exact | — |
| 100 updates at 240Hz inbound TRANSACTION message count | 100 messages | exact | — |
| 100 updates at 240Hz outbound EVENT bytes | 294 bytes | exact | — |
| 100 updates at 240Hz outbound EVENT message count | 3 messages | exact | — |
| 100 updates at 240Hz synthetic change-gated repaint count | 48 repaints | exact | — |
| 1000 updates at 60Hz decode-to-visible | 6367 ms | sample | — |
| 1000 updates at 60Hz total SRUI wire bytes | 3.004e+04 bytes | exact | — |
| 1000 updates at 60Hz total SRUI message count | 1003 messages | exact | — |
| 1000 updates at 60Hz inbound TRANSACTION bytes | 2.975e+04 bytes | exact | — |
| 1000 updates at 60Hz inbound TRANSACTION message count | 1000 messages | exact | — |
| 1000 updates at 60Hz outbound EVENT bytes | 297 bytes | exact | — |
| 1000 updates at 60Hz outbound EVENT message count | 3 messages | exact | — |
| 1000 updates at 60Hz synthetic change-gated repaint count | 243 repaints | exact | — |
| 1000 updates at 120Hz decode-to-visible | 6363 ms | sample | — |
| 1000 updates at 120Hz total SRUI wire bytes | 3.004e+04 bytes | exact | — |
| 1000 updates at 120Hz total SRUI message count | 1003 messages | exact | — |
| 1000 updates at 120Hz inbound TRANSACTION bytes | 2.975e+04 bytes | exact | — |
| 1000 updates at 120Hz inbound TRANSACTION message count | 1000 messages | exact | — |
| 1000 updates at 120Hz outbound EVENT bytes | 297 bytes | exact | — |
| 1000 updates at 120Hz outbound EVENT message count | 3 messages | exact | — |
| 1000 updates at 120Hz synthetic change-gated repaint count | 367 repaints | exact | — |
| 1000 updates at 144Hz decode-to-visible | 6362 ms | sample | — |
| 1000 updates at 144Hz total SRUI wire bytes | 3.004e+04 bytes | exact | — |
| 1000 updates at 144Hz total SRUI message count | 1003 messages | exact | — |
| 1000 updates at 144Hz inbound TRANSACTION bytes | 2.975e+04 bytes | exact | — |
| 1000 updates at 144Hz inbound TRANSACTION message count | 1000 messages | exact | — |
| 1000 updates at 144Hz outbound EVENT bytes | 297 bytes | exact | — |
| 1000 updates at 144Hz outbound EVENT message count | 3 messages | exact | — |
| 1000 updates at 144Hz synthetic change-gated repaint count | 386 repaints | exact | — |
| 1000 updates at 240Hz decode-to-visible | 6362 ms | sample | — |
| 1000 updates at 240Hz total SRUI wire bytes | 3.004e+04 bytes | exact | — |
| 1000 updates at 240Hz total SRUI message count | 1003 messages | exact | — |
| 1000 updates at 240Hz inbound TRANSACTION bytes | 2.975e+04 bytes | exact | — |
| 1000 updates at 240Hz inbound TRANSACTION message count | 1000 messages | exact | — |
| 1000 updates at 240Hz outbound EVENT bytes | 297 bytes | exact | — |
| 1000 updates at 240Hz outbound EVENT message count | 3 messages | exact | — |
| 1000 updates at 240Hz synthetic change-gated repaint count | 495 repaints | exact | — |
| settled idle SRUI wire bytes | 0 bytes | observed max | — |
| settled idle SRUI message count | 0 messages | observed max | — |

Assertions:

- **PASS** decode-to-visible samples reach an unforced composited content change — every framed production SessionController sample matched isolated semantic state and changed the exact visible client-content fingerprint without benchmark-forced invalidation; every cadence stream ended in its expected native value with passive composited evidence
- **PASS** settled idle UI emits zero SRUI traffic — 1000ms after all production EVENT ACKs drained for every count/cadence; byte deltas [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], message deltas [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]. This does not claim natural AppKit idle invalidation behavior: the synthetic cadence gate suppresses unchanged-state draws by construction.
- **PASS** complete bidirectional wire bytes and message count are cadence independent — the same prebuilt 1/100/1000 inbound TRANSACTION frame sequences were reused at 60/120/144/240Hz; every outbound transport delta is exactly the three decoded production EVENT frames with no extra, dropped, or interrupted attempts; total, inbound, and outbound bytes/messages are reported separately and compared across cadences; ACKs are injected only after these counters and the presentation timestamp are frozen
- **PASS** synthetic native-change-gated repaint count varies independently — 1@60Hz=1, 1@120Hz=1, 1@144Hz=1, 1@240Hz=1, 100@60Hz=13, 100@120Hz=24, 100@144Hz=27, 100@240Hz=48, 1000@60Hz=243, 1000@120Hz=367, 1000@144Hz=386, 1000@240Hz=495. This diagnostic measures configured coalescing opportunities; it is not evidence of AppKit's natural invalidation count.
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
| caret movement at 0ms RTT | 13.54 ms | p50 | ≤ 8.33333 ms |
| caret movement at 0ms RTT | 17.06 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| caret movement at 0ms RTT | 17.93 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| hover at 0ms RTT | 21.52 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| hover at 0ms RTT | 24.59 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| hover at 0ms RTT | 25.23 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| ime composition at 0ms RTT | 16.37 ms | p50 | ≤ 8.33333 ms |
| ime composition at 0ms RTT | 20.5 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| ime composition at 0ms RTT | 20.61 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| menu opening at 0ms RTT | 32.94 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| menu opening at 0ms RTT | 35.95 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| menu opening at 0ms RTT | 63.64 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| pressed at 0ms RTT | 18.75 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| pressed at 0ms RTT | 21.64 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| pressed at 0ms RTT | 24.66 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| scrolling at 0ms RTT | 16.53 ms | p50 | ≤ 8.33333 ms |
| scrolling at 0ms RTT | 19.23 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| scrolling at 0ms RTT | 19.45 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| text entry at 0ms RTT | 13.44 ms | p50 | ≤ 8.33333 ms |
| text entry at 0ms RTT | 17.33 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| text entry at 0ms RTT | 17.56 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| text selection at 0ms RTT | 13.21 ms | p50 | ≤ 8.33333 ms |
| text selection at 0ms RTT | 17.84 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| text selection at 0ms RTT | 24.93 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| server-dependent input-to-visible at 0ms RTT | 10.52 ms | p50 | — |
| server-dependent input-to-visible at 0ms RTT | 16.73 ms | p95 | — |
| server-dependent input-to-visible at 0ms RTT | 16.73 ms | p99 | — |
| caret movement at 100ms RTT | 16.83 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| caret movement at 100ms RTT | 19.33 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| caret movement at 100ms RTT | 22.18 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| hover at 100ms RTT | 15.46 ms | p50 | ≤ 8.33333 ms |
| hover at 100ms RTT | 18.39 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| hover at 100ms RTT | 18.42 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| ime composition at 100ms RTT | 16.47 ms | p50 | ≤ 8.33333 ms |
| ime composition at 100ms RTT | 21.04 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| ime composition at 100ms RTT | 26 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| menu opening at 100ms RTT | 33.9 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| menu opening at 100ms RTT | 36.49 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| menu opening at 100ms RTT | 37.08 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| pressed at 100ms RTT | 18.71 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| pressed at 100ms RTT | 25.07 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| pressed at 100ms RTT | 25.19 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| scrolling at 100ms RTT | 16.91 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| scrolling at 100ms RTT | 17.99 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| scrolling at 100ms RTT | 18.55 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| text entry at 100ms RTT | 16.33 ms | p50 | ≤ 8.33333 ms |
| text entry at 100ms RTT | 20.11 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| text entry at 100ms RTT | 21.69 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| text selection at 100ms RTT | 16.83 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| text selection at 100ms RTT | 18.54 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| text selection at 100ms RTT | 19.52 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| server-dependent input-to-visible at 100ms RTT | 121.1 ms | p50 | — |
| server-dependent input-to-visible at 100ms RTT | 121.5 ms | p95 | — |
| server-dependent input-to-visible at 100ms RTT | 121.5 ms | p99 | — |
| caret movement at 300ms RTT | 16.89 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| caret movement at 300ms RTT | 19.54 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| caret movement at 300ms RTT | 20.01 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| hover at 300ms RTT | 15.1 ms | p50 | ≤ 8.33333 ms |
| hover at 300ms RTT | 19.98 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| hover at 300ms RTT | 20.57 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| ime composition at 300ms RTT | 17.47 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| ime composition at 300ms RTT | 21.29 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| ime composition at 300ms RTT | 21.45 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| menu opening at 300ms RTT | 34.13 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| menu opening at 300ms RTT | 36.63 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| menu opening at 300ms RTT | 38.14 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| pressed at 300ms RTT | 13.37 ms | p50 | ≤ 8.33333 ms |
| pressed at 300ms RTT | 18.37 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| pressed at 300ms RTT | 19.53 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| scrolling at 300ms RTT | 21.26 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| scrolling at 300ms RTT | 25.39 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| scrolling at 300ms RTT | 25.7 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| text entry at 300ms RTT | 16.23 ms | p50 | ≤ 8.33333 ms |
| text entry at 300ms RTT | 19.54 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| text entry at 300ms RTT | 20.06 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| text selection at 300ms RTT | 14.09 ms | p50 | ≤ 8.33333 ms |
| text selection at 300ms RTT | 17.69 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| text selection at 300ms RTT | 17.7 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| server-dependent input-to-visible at 300ms RTT | 329.4 ms | p50 | — |
| server-dependent input-to-visible at 300ms RTT | 334 ms | p95 | — |
| server-dependent input-to-visible at 300ms RTT | 334 ms | p99 | — |
| caret movement at 600ms RTT | 16.36 ms | p50 | ≤ 8.33333 ms |
| caret movement at 600ms RTT | 21.12 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| caret movement at 600ms RTT | 21.77 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| hover at 600ms RTT | 17.4 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| hover at 600ms RTT | 20.66 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| hover at 600ms RTT | 20.86 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| ime composition at 600ms RTT | 17.61 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| ime composition at 600ms RTT | 21.45 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| ime composition at 600ms RTT | 21.57 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| menu opening at 600ms RTT | 33.46 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| menu opening at 600ms RTT | 40.15 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| menu opening at 600ms RTT | 41.15 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| pressed at 600ms RTT | 17.86 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| pressed at 600ms RTT | 21.43 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| pressed at 600ms RTT | 21.5 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| scrolling at 600ms RTT | 21.31 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| scrolling at 600ms RTT | 25.15 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| scrolling at 600ms RTT | 25.21 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| text entry at 600ms RTT | 16.85 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| text entry at 600ms RTT | 21.07 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| text entry at 600ms RTT | 21.13 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| text selection at 600ms RTT | 17.32 ms **WARNING >2x** | p50 | ≤ 8.33333 ms |
| text selection at 600ms RTT | 21.01 ms **WARNING >2x** | p95 | ≤ 8.33333 ms |
| text selection at 600ms RTT | 21.12 ms **WARNING >2x** | p99 | ≤ 8.33333 ms |
| server-dependent input-to-visible at 600ms RTT | 637.9 ms | p50 | — |
| server-dependent input-to-visible at 600ms RTT | 653.1 ms | p95 | — |
| server-dependent input-to-visible at 600ms RTT | 653.1 ms | p99 | — |
| 1MiB/s bandwidth-limited production event | 17.01 ms | p50 | — |
| 1MiB/s bandwidth-limited production event | 17.52 ms | p95 | — |
| bandwidth-limited delivered bytes | 4.948e+04 bytes | exact | — |
| deterministic production loss attempts | 2 messages | exact | — |
| deterministic production loss delivered messages | 1 messages | exact | — |
| controlled production interruption detection | 0.9573 ms | p50 | — |
| measured production session wire bytes | 1.421e+05 bytes | exact | — |
| measured production session wire messages | 1240 messages | exact | — |
| maximum RTT-induced local latency delta | 5.188 ms | p50 | ≤ 8.33333 ms |
| maximum RTT-induced local latency delta | 9.502 ms | p95 | ≤ 8.33333 ms |
| maximum RTT-induced local latency delta | 12.51 ms | p99 | ≤ 8.33333 ms |

Assertions:

- **PASS** mounted local interactions do not acquire one RTT — largest p50 increase 5.1883 ms versus the measured local frame budget of 8.3333 ms; full compositor mode applies this numeric correctness gate; paired injected transaction remained blocked through local visible completion in 640/640 probes=true; configured delay state was verified at the exact action boundary in 640/640 probes=true, with a nonzero transport delay still active in 480/480 nonzero-RTT probes; local_state_checks=truepaired p95/p99 deltas were 9.5023/12.5107 ms; production renderer callbacks=240
- **PASS** native text entry emits and settles one exact production TEXT_EDIT — RTT 0ms: node=14 final_text="ééééééééééééééééééééview the changes and run tests.xxxxxxxxxxxxxxxxxxxx" edit_seq=40 observed_revision=81 matching_framed_events=1 exact_slot_ack=true stable_empty_tail=true; RTT 100ms: node=14 final_text="ééééééééééééééééééééview the changes and run tests.xxxxxxxxxxxxxxxxxxxx" edit_seq=40 observed_revision=81 matching_framed_events=1 exact_slot_ack=true stable_empty_tail=true; RTT 300ms: node=14 final_text="ééééééééééééééééééééview the changes and run tests.xxxxxxxxxxxxxxxxxxxx" edit_seq=40 observed_revision=80 matching_framed_events=1 exact_slot_ack=true stable_empty_tail=true; RTT 600ms: node=14 final_text="ééééééééééééééééééééview the changes and run tests.xxxxxxxxxxxxxxxxxxxx" edit_seq=40 observed_revision=80 matching_framed_events=1 exact_slot_ack=true stable_empty_tail=true
- **PASS** injected transport RTT affects production server-dependent feedback — SessionController EventOutbox sends and framed transaction responses tracked 100/300/600ms RTT
- **PASS** local visible completion does not await an injected transport response — Each local action began only after its exact compositor baseline was ready and while its configured BenchmarkTransport delay state was verified; 480/480 nonzero-RTT actions began during an active delay. Every paired production transaction remained blocked through visible completion; the gate was released only afterward. Separately, production server-dependent feedback tracked 100/300/600ms RTT.
- **PASS** bandwidth delay, loss, and interruption exercise session recovery — sample 1: 16493 framed bytes / 1 message: measured 17.5221 ms >= theoretical 15.7290 ms; exact_event=true; sample 2: 16493 framed bytes / 1 message: measured 17.0053 ms >= theoretical 15.7290 ms; exact_event=true; sample 3: 16493 framed bytes / 1 message: measured 16.9598 ms >= theoretical 15.7290 ms; exact_event=true; 49479 total bandwidth bytes; lost and interrupted events remained in EventOutbox and replayed through replacement SessionControllers

Notes:

- Controls are mounted renderer TextArea, ScrollView, and Button. renderer-produced NSButton context menu was proven as a new exact owned menu-level WindowServer surface in the same ScreenCaptureKit frame used for its presentation timestamp.
- Pressed state is observed between real NSWindow-dispatched mouseDown/mouseUp events and triggers the production ActionTrampoline. renderer-produced NSButton changed at least 1590 target-ROI pixels above the explicit 2/255 per-channel SCStream tolerance on the local AppKit hover path in 20/20 samples; mouseExited then restored every unmasked screenshot pixel within the explicit 5/255 same-API tolerance, with maximum observed channel delta 4/255; renderer-produced NSButton changed at least 1590 target-ROI pixels above the explicit 2/255 per-channel SCStream tolerance on the local AppKit hover path in 20/20 samples; mouseExited then restored every unmasked screenshot pixel within the explicit 5/255 same-API tolerance, with maximum observed channel delta 5/255. Permission-free hover invokes the renderer-produced NSButton's own AppKit entry/exit path and does not claim WindowServer pointer latency.
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
| disconnect immediately before event receipt | 6.883 ms | p50 | — |
| disconnect immediately before event receipt | 7.078 ms | p95 | — |
| disconnect immediately before event receipt | 19.4 ms | p99 | — |
| event receipt through settled side effect | 0.000542 ms | p50 | — |
| event receipt through settled side effect | 0.002 ms | p95 | — |
| event receipt through settled side effect | 0.004375 ms | p99 | — |
| in-process cached DUPLICATE response | 0.000208 ms | p50 | — |
| in-process cached DUPLICATE response | 0.0005 ms | p95 | — |
| in-process cached DUPLICATE response | 0.000666 ms | p99 | — |
| lost ACK wire reconnect through DUPLICATE acknowledgement | 0.1091 ms | p50 | — |
| lost ACK wire reconnect through DUPLICATE acknowledgement | 0.2573 ms | p95 | — |
| lost ACK wire reconnect through DUPLICATE acknowledgement | 0.4275 ms | p99 | — |
| mid-resource reconnect and exact replay | 0.003792 ms | p50 | — |
| mid-resource reconnect and exact replay | 0.02133 ms | p95 | — |
| mid-resource reconnect and exact replay | 0.02621 ms | p99 | — |
| mid-transaction frame discard and atomic replay | 0.000875 ms | p50 | — |
| mid-transaction frame discard and atomic replay | 0.003125 ms | p95 | — |
| mid-transaction frame discard and atomic replay | 0.008875 ms | p99 | — |
| mid-transaction wire disconnect and exact atomic replay | 6.894 ms | p50 | — |
| mid-transaction wire disconnect and exact atomic replay | 7.13 ms | p95 | — |
| mid-transaction wire disconnect and exact atomic replay | 7.371 ms | p99 | — |
| partial EVENT disconnect and one processed replay | 7.449 ms | p50 | — |
| partial EVENT disconnect and one processed replay | 8.687 ms | p95 | — |
| partial EVENT disconnect and one processed replay | 10.6 ms | p99 | — |
| resume beyond journal retention | 0.000708 ms | p50 | — |
| resume beyond journal retention | 0.001958 ms | p95 | — |
| resume beyond journal retention | 0.002917 ms | p99 | — |
| resume within journal retention | 0.000625 ms | p50 | — |
| resume within journal retention | 0.001709 ms | p95 | — |
| resume within journal retention | 0.002834 ms | p99 | — |
| pre-receipt retained-event replay | 23.9 ms | p50 | — |
| pre-receipt retained-event replay | 37.2 ms | p95 | — |
| mid-resource reconnect recovery | 0.817 ms | p50 | — |
| mid-resource reconnect recovery | 1.743 ms | p95 | — |
| superseded resume response handling | 0.1119 ms | p50 | — |
| superseded resume response handling | 0.1292 ms | p95 | — |
| active resume response handling | 0.2172 ms | p50 | — |
| active resume response handling | 0.3331 ms | p95 | — |
| production reconnect boundary suite | 2.944e+04 ms | wall | — |

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
| embedded SRUI PTY exact ANSI capture and framing | 11.15 ms | p50 | — |
| embedded SRUI PTY exact ANSI capture and framing | 12.44 ms | p95 | — |
| embedded SRUI PTY exact ANSI capture and framing | 13.71 ms | p99 | — |
| standalone PTY exact ANSI interaction | 9.587 ms | p50 | — |
| standalone PTY exact ANSI interaction | 15.31 ms | p95 | — |
| standalone PTY exact ANSI interaction | 25.87 ms | p99 | — |
| terminal reconnect retention-loss decision | 0.001 ms | sample | — |
| terminal payload | 6912 bytes | exact | — |
| embedded terminal frame count | 1 messages | p50 | — |
| embedded terminal frame count | 1 messages | p95 | — |
| embedded terminal frame count | 1 messages | p99 | — |
| embedded SRUI Terminal decode-to-visible | 19.92 ms | p50 | — |
| embedded SRUI Terminal decode-to-visible | 22.01 ms | p95 | — |
| embedded SRUI Terminal decode-to-visible | 24.43 ms | p99 | — |
| embedded SRUI Terminal draw-only | 14.96 ms | p50 | — |
| embedded SRUI Terminal draw-only | 20.35 ms | p95 | — |
| embedded SRUI Terminal draw-only | 23.24 ms | p99 | — |
| standalone TerminalSession and TerminalView decode-to-visible | 20.5 ms | p50 | — |
| standalone TerminalSession and TerminalView decode-to-visible | 25.27 ms | p95 | — |
| standalone TerminalSession and TerminalView decode-to-visible | 25.43 ms | p99 | — |
| standalone TerminalView draw-only | 17.51 ms | p50 | — |
| standalone TerminalView draw-only | 24.37 ms | p95 | — |
| standalone TerminalView draw-only | 24.53 ms | p99 | — |
| embedded-to-standalone terminal decode-to-visible | 0.9718 ratio | p50 | — |
| embedded-to-standalone terminal draw-only | 0.8543 ratio | p50 | — |
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
- **PASS** standalone and embedded terminal visible completions are actual draws — 20/20 embedded and 20/20 standalone TerminalView completions crossed the configured draw-visible boundary and produced content-distinct bitmaps; full mode additionally required nonblank, nonuniform client-only composited captures; embedded visibility provenance: ["screencapturekit_same_complete_frame_target_roi_after_action_screencapturekit_composited_baseline_status_level_25_appkit_occlusion_8192"]; standalone visibility provenance: ["screencapturekit_same_complete_frame_target_roi_after_action_screencapturekit_composited_baseline_status_level_25_appkit_occlusion_8192"]; embedded composited fingerprints: ["4fa708a5da1d635c15ad19010349023e3064a3d9407b73b317beb149e64cd8c1"]; standalone composited fingerprints: ["12b12e48b07e6c24a820f342eb502fd874855c3e8d4609cdc06292cd54c9e016", "28f12e79bc64c59b5fb18c3121f4c8bc4e40e3245cb7c0d1968ad2fd75e93dbe", "2fe3ae5122549135ac918ebb23c6f878322cde23454549ceba76c3547f058b2e", "5141d4a957f992073bb98c0cc49ef15c7d5c68e87c1e4f84d21623ce3996ad2a", "664f8ce4b2b7e7de2f5167fb2b8fd7bd7792b812ad2153bc49e39a6784cff805", "77bc8876723fdc2239e955971e6dcfc923aa9ae3d5ab4deb5091532c0f6dd82f", "862a83f16208e85a3439664d96ca2c1336b9657b805bd2b3f0f175fe807445a2", "96165413fe372f11d697dbd5d4cc67c0b983fb74dd9da2387207efbbd4830a39", "978ae248382719a20268bcb32a52506fa06acd3be9551f07129068b54208adc5", "99dd133b137136c03c3b7123f1cc2869f4a21225c43b095c8a11d077ed0f322f", "a21edb6dbe47256facd053d7912001e13b6c60a9ccce9306e43dc39eb0b6fce5", "a9a615464ccc0daf27ffcc147929d06abf0828cd8e4ee1334742d24eb6bacbc9", "ac7948e1eaddad5b6dd776c8742f7f5acf502ba3ae07b9feface42f46e67868d", "ad48ebcae6fec79569e8e90a3949f8fe552f6f39953ec172e7f9a65ce5d48baf", "b50a0710a9e8fb8b0500240f36122f4b2de2ee1a688c52751f6fd0b00406ffbb", "cacaa40ed3dc94d6b65a9dab9296bfe9f59e1a182df7eb4c9b3634ba81481869", "d0cda6b6c3bd620eb225faecbdbdc7d8998728d86eb9461b38c29aafb82cf28c", "e3efd89a87a17515df86bb03df0ba7d0d5f6c273790f0ee941b4e1b73be7069f"]
- **PASS** standalone and embedded terminal displays render the identical ANSI payload — both paths consumed payload SHA-256 e4461bba73f99c7bc70ba6041b859c3e59446d6c80d1549c25de7bcaf933a910, decoded exact 256-line content SHA-256 e1e85bdbda198ce2b94b317765b18ec90b981d25c0ef567ff83d102f6817fe8d, and produced byte-identical TerminalView rasters; embedded content digests: ["e1e85bdbda198ce2b94b317765b18ec90b981d25c0ef567ff83d102f6817fe8d"]; standalone content digests: ["e1e85bdbda198ce2b94b317765b18ec90b981d25c0ef567ff83d102f6817fe8d"]; embedded bitmap digests: ["e440b40246b19d42137da63cb2cc233d9e18702986ff6269b4f6ac33337dbab5"]; standalone bitmap digests: ["e440b40246b19d42137da63cb2cc233d9e18702986ff6269b4f6ac33337dbab5"]
- **PASS** standalone and production terminal samples begin from fresh parser and view state — embedded samples negotiate org.srui.terminal/1, register namespace 31, and mount through a framed transaction; standalone samples instantiate fresh production TerminalSession and TerminalView pairs; both reproduce exact content SHA-256 e1e85bdbda198ce2b94b317765b18ec90b981d25c0ef567ff83d102f6817fe8d

Notes:

- Standalone and embedded samples both include production PTY spawn, identical shell execution, EOF, and exact ANSI output; embedded additionally captures and frames the bytes. The embedded EOF boundary is event-driven and does not poll process state.
- Embedded decode-to-visible starts before a framed 6,912-byte SRUITerminalData enters BenchmarkTransport, then crosses SessionController, the renderer-owned TerminalSession, the renderer-created TerminalView snapshot subscription, and the configured presentation boundary.
- The standalone display baseline feeds the identical Data value at offset zero through fresh production TerminalSession and TerminalView instances, using the embedded view's exact grid and bounds. The reported ratios compare p50 display completion on like-for-like content and geometry.
- Full mode starts an exact TerminalView-ROI ScreenCaptureKit stream and verifies a nonblank, nonuniform baseline before timing. After the production payload action completes, it accepts only a later complete compositor sample whose same-frame ROI fingerprint changed; the reported latency is action-start mach time to that sample's ScreenCaptureKit display time. Exact WindowServer identity, display, client/ROI geometry, and absence of every intersecting visible nonzero-alpha window are checked before and after acceptance. Callback receipt remains metadata rather than the latency clock. Observed embedded provenance: ["screencapturekit_same_complete_frame_target_roi_after_action_screencapturekit_composited_baseline_status_level_25_appkit_occlusion_8192"]; standalone provenance: ["screencapturekit_same_complete_frame_target_roi_after_action_screencapturekit_composited_baseline_status_level_25_appkit_occlusion_8192"]. Smoke uses an explicitly named offscreen bitmap draw and is not presentation evidence.
- The Rust section separately compares exact standalone-versus-embedded PTY byte transport and exercises reconnect ring-buffer exhaustion; this Swift section compares terminal emulator and display completion.

## Follow-up flags

- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 caret movement at 0ms RTT (p95): 17.06 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 caret movement at 0ms RTT (p99): 17.93 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 hover at 0ms RTT (p50): 21.52 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 hover at 0ms RTT (p95): 24.59 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 hover at 0ms RTT (p99): 25.23 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 ime composition at 0ms RTT (p95): 20.5 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 ime composition at 0ms RTT (p99): 20.61 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 0ms RTT (p50): 32.94 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 0ms RTT (p95): 35.95 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 0ms RTT (p99): 63.64 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 pressed at 0ms RTT (p50): 18.75 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 pressed at 0ms RTT (p95): 21.64 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 pressed at 0ms RTT (p99): 24.66 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 0ms RTT (p95): 19.23 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 0ms RTT (p99): 19.45 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text entry at 0ms RTT (p95): 17.33 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text entry at 0ms RTT (p99): 17.56 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text selection at 0ms RTT (p95): 17.84 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text selection at 0ms RTT (p99): 24.93 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 caret movement at 100ms RTT (p50): 16.83 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 caret movement at 100ms RTT (p95): 19.33 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 caret movement at 100ms RTT (p99): 22.18 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 hover at 100ms RTT (p95): 18.39 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 hover at 100ms RTT (p99): 18.42 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 ime composition at 100ms RTT (p95): 21.04 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 ime composition at 100ms RTT (p99): 26 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 100ms RTT (p50): 33.9 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 100ms RTT (p95): 36.49 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 100ms RTT (p99): 37.08 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 pressed at 100ms RTT (p50): 18.71 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 pressed at 100ms RTT (p95): 25.07 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 pressed at 100ms RTT (p99): 25.19 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 100ms RTT (p50): 16.91 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 100ms RTT (p95): 17.99 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 100ms RTT (p99): 18.55 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text entry at 100ms RTT (p95): 20.11 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text entry at 100ms RTT (p99): 21.69 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text selection at 100ms RTT (p50): 16.83 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text selection at 100ms RTT (p95): 18.54 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text selection at 100ms RTT (p99): 19.52 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 caret movement at 300ms RTT (p50): 16.89 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 caret movement at 300ms RTT (p95): 19.54 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 caret movement at 300ms RTT (p99): 20.01 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 hover at 300ms RTT (p95): 19.98 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 hover at 300ms RTT (p99): 20.57 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 ime composition at 300ms RTT (p50): 17.47 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 ime composition at 300ms RTT (p95): 21.29 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 ime composition at 300ms RTT (p99): 21.45 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 300ms RTT (p50): 34.13 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 300ms RTT (p95): 36.63 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 300ms RTT (p99): 38.14 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 pressed at 300ms RTT (p95): 18.37 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 pressed at 300ms RTT (p99): 19.53 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 300ms RTT (p50): 21.26 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 300ms RTT (p95): 25.39 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 300ms RTT (p99): 25.7 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text entry at 300ms RTT (p95): 19.54 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text entry at 300ms RTT (p99): 20.06 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text selection at 300ms RTT (p95): 17.69 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text selection at 300ms RTT (p99): 17.7 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 caret movement at 600ms RTT (p95): 21.12 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 caret movement at 600ms RTT (p99): 21.77 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 hover at 600ms RTT (p50): 17.4 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 hover at 600ms RTT (p95): 20.66 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 hover at 600ms RTT (p99): 20.86 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 ime composition at 600ms RTT (p50): 17.61 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 ime composition at 600ms RTT (p95): 21.45 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 ime composition at 600ms RTT (p99): 21.57 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 600ms RTT (p50): 33.46 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 600ms RTT (p95): 40.15 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 menu opening at 600ms RTT (p99): 41.15 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 pressed at 600ms RTT (p50): 17.86 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 pressed at 600ms RTT (p95): 21.43 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 pressed at 600ms RTT (p99): 21.5 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 600ms RTT (p50): 21.31 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 600ms RTT (p95): 25.15 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 scrolling at 600ms RTT (p99): 25.21 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text entry at 600ms RTT (p50): 16.85 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text entry at 600ms RTT (p95): 21.07 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text entry at 600ms RTT (p99): 21.13 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text selection at 600ms RTT (p50): 17.32 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text selection at 600ms RTT (p95): 21.01 ms vs target 8.33333 ms
- **PERFORMANCE FOLLOW-UP (>2x):** §31.4 text selection at 600ms RTT (p99): 21.12 ms vs target 8.33333 ms
