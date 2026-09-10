# §31.3 mutation and frame-independence benchmark

Owner: `client-macos/Benchmarks/MutationBenchmark.swift::mutationAndCadence`. A preloaded production
AppKit renderer receives framed transactions containing 1, 100, and 1,000 scalar value updates.
The driver records exact inbound transaction bytes/messages, semantic decode/apply time, and
decode-to-visible latency.
In the full profile, decode-to-visible starts a ScreenCaptureKit stream and obtains a complete
baseline frame for the exact visible `NSProgressIndicator` ROI before timing. Frames emitted while
the asynchronous production action is still running are ignored. After `SessionController` and
`AppKitRenderer` have applied the mutation, a fresh cutoff is armed and the benchmark accepts only
a later complete stream frame whose same-sized target ROI has a different nonblank, nonuniform
fingerprint. Decode-to-visible uses the action-start Mach tick and the accepted frame's
`SCStreamFrameInfo.displayTime` Mach tick; callback receipt is diagnostic metadata only. Exact
window, client-content, target geometry, display, ownership, and z-order are rechecked afterward. No
benchmark invalidation, layout, display, or transaction flush occurs after timing starts. Smoke is
explicitly an offscreen AppKit raster fallback and makes no WindowServer/compositor claim.

For cadence independence, each update count is encoded once as a sequence of one-operation SRUI
transactions. That exact `Data` sequence is replayed unchanged at 60, 120, 144, and 240 Hz. After
each production revision, the source confirms the mounted `NSProgressIndicator.doubleValue`.
A synthetic cadence task then samples pending native-value changes at the configured rate and
rasterizes only when the pending value differs from the last sampled value. Its repaint number is
a coalescing diagnostic, not a measurement of AppKit's natural invalidation policy.

Each cadence trial also emits three distinct production client events through
`SessionController` and `EventOutbox`: `ACTIVATE`, `VALUE_CHANGED`, and `SELECTION_CHANGED`.
They are interleaved at deterministic early, middle, and final applied revisions before those
revisions are exposed to the synthetic cadence paint gate (for one update, all three occur after
revision 2 and before its cadence paint). The benchmark captures the exact outbound framed EVENT
created by each send and compares its event ID, sequence, observed revision, type, node, arguments,
client instance, and edit sequence to the returned semantic event in emission order. The captured
EVENT indices must cover the entire outbound transport delta, outbound attempts must equal delivered
frames, and captured frame bytes must equal the outbound byte delta, so unreported extra, dropped,
or interrupted frames fail the trial. A canonical sequence/revision/type/node/argument signature
must match across all four cadences for each update count. Total, inbound, and outbound bytes and
message counts are frozen and reported before EVENT ACKs are injected and the outbox is drained.

The following settled one-second interval measures only SRUI transport traffic. It must add zero
bytes and zero messages after event settlement. It deliberately makes no claim about natural local
idle repainting: the synthetic cadence task suppresses unchanged-state draws by construction.

Focused command:

    client-macos/.build/release/BenchmarkDriver --fixture benchmarks/fixtures/coding-agent-ui.json --profile smoke --only-section 31.3 --output /tmp/srui-31.3.json

Metric families are `updates.{1,100,1000}.{semantic,visible,bytes,messages}` and
`cadence.{1,100,1000}.{60,120,144,240}`. For each cadence, `.bytes`/`.messages` are
the complete bidirectional totals, `.inbound_bytes`/`.inbound_messages` identify exact
TRANSACTION traffic, `.outbound_bytes`/`.outbound_messages` identify exact EVENT traffic,
and `.visible`/`.repaints` report timing and local draws. `idle.bytes` and `idle.messages`
cover the settled interval. Separate sample-count entries
`macos.cadence.events.{count}.{hz}` report the three exact EVENT-frame comparisons in every
cadence trial. Assertions `cadence_wire_invariant`, `cadence_repaint_independent`,
`cadence_state_event_order`, `mutation_raster_completion`, and `idle_zero_traffic` distinguish
complete bidirectional transport invariance, typed client-event order, synthetic repaint grouping,
passive full-paint evidence, and settled zero-traffic evidence.
