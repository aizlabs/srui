import AppKit
import CryptoKit
import Darwin
import Foundation
import Protocol
import RendererAppKit
import Resources
import SemanticModel
import Session
import SwiftProtobuf
import Terminal
import TransportSSH
import WebKit

@MainActor
func mutationAndCadence(
    fixtureOperations: [SemanticModel.Operation],
    fixtureIndex: BenchmarkFixtureIndex,
    iterations: Int,
    fullPaint: Bool
) async throws -> Section {
    let baseApplier = TransactionApplier()
    guard case .success = baseApplier.apply(
        baseRevision: .initial,
        operations: fixtureOperations
    ) else {
        throw BenchmarkFailure.message("mutation benchmark base fixture did not commit")
    }
    let baseStore = baseApplier.store
    var metrics = [Metric]()
    var sampleCounts = [String: Int]()
    var scalarOnly = true
    var allMutationRastersCompleted = true
    var allMutationStateParityPassed = true

    for count in MutationBenchmarkConfiguration.updateCounts {
        let updates = (0..<count).map { index in
            SemanticModel.Operation.setProperty(
                id: fixtureIndex.progress,
                property: .value,
                value: .float64(Double(index + 1) / Double(count))
            )
        }
        var semanticLatencies = [Double]()
        var visibleLatencies = [Double]()
        var bytes = 0
        var messages = 0
        for _ in 0..<iterations {
            let result = try await mutationRun(
                baseStore: baseStore,
                fixtureOperations: fixtureOperations,
                fixtureIndex: fixtureIndex,
                operations: updates,
                fullPaint: fullPaint
            )
            semanticLatencies.append(result.semanticLatency)
            visibleLatencies.append(result.visibleLatency)
            bytes = result.bytes
            messages = result.messages
            allMutationRastersCompleted = allMutationRastersCompleted && result.rasterized
            allMutationStateParityPassed = allMutationStateParityPassed
                && result.stateParity
            scalarOnly = scalarOnly && result.classifications.allSatisfy {
                if case .structureAffecting = $0 { return false }
                return true
            }
        }
        sampleCounts["macos.mutation.\(count)"] = semanticLatencies.count
        let target = count == 100 ? 1.0 : (count == 1_000 ? 5.0 : nil)
        let semanticID = "updates.\(count).semantic"
        let visibleID = "updates.\(count).visible"
        metrics.append(
            metric(
                "\(count) updates semantic decode/apply",
                p50(semanticLatencies),
                target: target,
                id: semanticID
            )
        )
        metrics.append(
            metric(
                "\(count) updates semantic decode/apply",
                percentile(semanticLatencies, 0.95),
                "ms",
                "p95",
                id: semanticID
            )
        )
        metrics.append(
            metric(
                "\(count) updates semantic decode/apply",
                percentile(semanticLatencies, 0.99),
                "ms",
                "p99",
                id: semanticID
            )
        )
        metrics.append(
            metric(
                "\(count) updates decode-to-visible",
                p50(visibleLatencies),
                id: visibleID
            )
        )
        metrics.append(
            metric(
                "\(count) updates decode-to-visible",
                percentile(visibleLatencies, 0.95),
                "ms",
                "p95",
                id: visibleID
            )
        )
        metrics.append(
            metric(
                "\(count) updates decode-to-visible",
                percentile(visibleLatencies, 0.99),
                "ms",
                "p99",
                id: visibleID
            )
        )
        metrics.append(
            metric(
                "\(count) updates wire bytes",
                Double(bytes),
                "bytes",
                "exact",
                id: "updates.\(count).bytes"
            )
        )
        metrics.append(
            metric(
                "\(count) updates message count",
                Double(messages),
                "messages",
                "exact",
                id: "updates.\(count).messages"
            )
        )
    }
    let configuredCadences =
        MutationBenchmarkConfiguration.configuredCadences
    let idleObservationMilliseconds =
        MutationBenchmarkConfiguration.idleObservationMilliseconds
    var observations = [CadenceObservation]()

    for count in MutationBenchmarkConfiguration.updateCounts {
        // The exact same pre-encoded transaction sequence is replayed at every cadence.
        let framedUpdates = try (1...count).map { index in
            let transaction = Transaction(
                baseRevision: Revision(UInt64(index)),
                operations: [
                    .setProperty(
                        id: fixtureIndex.progress,
                        property: .value,
                        value: .float64(Double(index) / Double(count))
                    )
                ]
            )
            return try framed(transactionMessage(transaction))
        }
        let expectedWireBytes = framedUpdates.reduce(into: 0) { $0 += $1.count }
        let eventMilestones = cadenceEventMilestones(updateCount: count)

        for hz in configuredCadences {
            let renderer = AppKitRenderer()
            let transport = BenchmarkTransport()
            let outbox = EventOutbox()
            let sessionID = "cadence-\(count)-\(hz)"
            let controller = try await startActiveSession(
                transport: transport,
                renderer: renderer,
                fixtureOperations: fixtureOperations,
                fixtureIndex: fixtureIndex,
                sessionID: sessionID,
                outbox: outbox
            )
            var cadenceTask: Task<Void, any Error>?
            do {
                let warmObservation = try await observeNativePresentation(
                    renderer,
                    fullPaint: fullPaint
                )
                guard warmObservation.crossedDisplayRefresh else {
                    throw BenchmarkFailure.message(
                        "steady-state cadence renderer did not warm"
                    )
                }
                let fullPresentationTarget: (
                    window: NSWindow,
                    view: NSProgressIndicator
                )?
                if fullPaint {
                    let windows =
                        renderer.registry.surfaceHandles.compactMap(\.window)
                    guard windows.count == 1,
                          let window = windows.first,
                          let progress =
                            renderer.registry.view(for: fixtureIndex.progress)
                                as? NSProgressIndicator,
                          progress.window === window else {
                        throw BenchmarkFailure.message(
                            "cadence fixture must expose one attached progress "
                                + "target in one presentation window"
                        )
                    }
                    fullPresentationTarget = (window, progress)
                } else {
                    for surface in renderer.registry.surfaceHandles {
                        surface.window?.orderOut(nil)
                    }
                    fullPresentationTarget = nil
                }

                let probe = CadenceProbe()
                cadenceTask = Task { @MainActor in
                    let interval = Duration.milliseconds(1_000.0 / Double(hz))
                    while Task.isCancelled == false {
                        do {
                            try await Task.sleep(for: interval)
                        } catch is CancellationError {
                            return
                        }
                        guard Task.isCancelled == false else {
                            return
                        }
                        guard let pendingUpdateIndex =
                            probe.pendingNativeUpdateIndex,
                              pendingUpdateIndex != probe.paintedUpdateIndex else {
                            continue
                        }
                        guard rasterizeRenderer(
                            renderer,
                            showWindows: false
                        ) else {
                            throw BenchmarkFailure.message(
                                "synthetic native-change-gated cadence raster failed"
                            )
                        }
                        guard let progress =
                            renderer.registry.view(for: fixtureIndex.progress)
                                as? NSProgressIndicator else {
                            throw BenchmarkFailure.message(
                                "cadence renderer has no native progress control"
                            )
                        }
                        let paintedIndex = Int(
                            (progress.doubleValue * Double(count)).rounded()
                        )
                        guard (1...count).contains(paintedIndex) else {
                            throw BenchmarkFailure.message(
                                "cadence renderer painted unmappable value \(progress.doubleValue)"
                            )
                        }
                        probe.repaintCount += 1
                        probe.paintedUpdateIndex = paintedIndex
                        probe.paintedRevision =
                            Revision(UInt64(paintedIndex + 1))
                    }
                }

                let beforeStream = await transport.snapshot()
                var observedRevisions = [UInt64]()
                var returnedEvents = [SemanticModel.Event]()
                var capturedEvents = [SemanticModel.Event]()
                var capturedEventFrames = [CapturedTransportFrame]()
                var capturedEventFrameIndices = [Int]()
                let finalRevision = Revision(UInt64(count + 1))

                let applyCadenceStream: @MainActor () async throws -> Void = {
                    for (offset, frame) in framedUpdates.enumerated() {
                        try await Task.sleep(for: .milliseconds(1))
                        try await transport.injectFromServer(frame)
                        let revision = Revision(UInt64(offset + 2))
                        try await waitForRevision(
                            revision,
                            controller: controller
                        )
                        let expectedValue =
                            Double(offset + 1) / Double(count)
                        try await waitUntil {
                            guard let progress =
                                renderer.registry.view(for: fixtureIndex.progress)
                                    as? NSProgressIndicator else {
                                return false
                            }
                            return abs(progress.doubleValue - expectedValue)
                                < 0.000_001
                        }
                        guard let progress =
                            renderer.registry.view(for: fixtureIndex.progress)
                                as? NSProgressIndicator else {
                            throw BenchmarkFailure.message(
                                "cadence renderer lost its native progress control"
                            )
                        }
                        let nativeUpdateIndex = Int(
                            (
                                progress.doubleValue * Double(count)
                            ).rounded()
                        )
                        guard nativeUpdateIndex == offset + 1 else {
                            throw BenchmarkFailure.message(
                                "native cadence state did not match the "
                                    + "committed update"
                            )
                        }
                        observedRevisions.append(
                            controller.applier.lastAppliedRevision.value
                        )

                        // Keep this revision invisible to the synthetic cadence
                        // gate until its deterministic client events have entered
                        // the production EventOutbox and exact transport stream.
                        for milestone in eventMilestones
                        where milestone.updateIndex == nativeUpdateIndex {
                            let event =
                                try await sendAndCaptureCadenceProductionEvent(
                                    kind: milestone.kind,
                                    updateCount: count,
                                    fixtureIndex: fixtureIndex,
                                    controller: controller,
                                    transport: transport
                                )
                            guard event.returned.observedRevision == revision else {
                                throw BenchmarkFailure.message(
                                    "cadence EVENT observed the wrong committed "
                                        + "revision"
                                )
                            }
                            returnedEvents.append(event.returned)
                            capturedEvents.append(event.captured)
                            capturedEventFrames.append(event.frame)
                            capturedEventFrameIndices.append(event.frameIndex)
                        }
                        probe.pendingNativeUpdateIndex = nativeUpdateIndex
                    }

                    try await waitUntil(timeout: .seconds(10)) {
                        probe.paintedUpdateIndex == count
                            && probe.paintedRevision == finalRevision
                    }
                }

                let decodeToVisible: Double
                if let fullPresentationTarget {
                    let measured =
                        try await benchmarkMeasurePassiveCompositedChange(
                            fullPresentationTarget.window,
                            targetView: fullPresentationTarget.view
                        ) {
                            try await applyCadenceStream()
                        }
                    decodeToVisible =
                        measured.presentationLatencyMilliseconds
                } else {
                    let visibleStart = clock.now
                    try await applyCadenceStream()
                    decodeToVisible = milliseconds(
                        visibleStart.duration(to: clock.now)
                    )
                }
                let afterStream = await transport.snapshot()
                let inboundWireBytes =
                    afterStream.inboundBytes - beforeStream.inboundBytes
                let inboundMessages =
                    afterStream.inboundMessages - beforeStream.inboundMessages
                let outboundWireBytes =
                    afterStream.outboundBytes - beforeStream.outboundBytes
                let outboundMessages =
                    afterStream.outboundMessages - beforeStream.outboundMessages
                let outboundAttempts =
                    afterStream.outboundAttempts - beforeStream.outboundAttempts
                let totalWireBytes = inboundWireBytes + outboundWireBytes
                let totalMessages = inboundMessages + outboundMessages

                let returnedSignature =
                    cadenceEventOrderSignature(returnedEvents)
                let capturedSignature =
                    cadenceEventOrderSignature(capturedEvents)
                let expectedSequences = (1...eventMilestones.count).map(UInt64.init)
                let expectedObservedRevisions = eventMilestones.map {
                    UInt64($0.updateIndex + 1)
                }
                let expectedOutboundFrameIndices = Array(
                    beforeStream.outboundMessages..<afterStream.outboundMessages
                )
                let eventFramesMatchReturned =
                    returnedEvents.count == eventMilestones.count
                        && capturedEvents.count == returnedEvents.count
                        && capturedEventFrames.count == returnedEvents.count
                        && capturedEventFrameIndices
                            == expectedOutboundFrameIndices
                        && zip(returnedEvents, capturedEvents).allSatisfy {
                            cadenceEventFieldsMatch(
                                returned: $0.0,
                                captured: $0.1
                            )
                        }
                        && returnedEvents.map(\.eventSeq)
                            == expectedSequences
                        && returnedEvents.map {
                            $0.observedRevision.value
                        } == expectedObservedRevisions
                        && returnedSignature == capturedSignature
                let capturedEventWireBytes =
                    capturedEventFrames.reduce(into: 0) {
                        $0 += $1.data.count
                    }

                guard await outbox.pendingCount == returnedEvents.count else {
                    throw BenchmarkFailure.message(
                        "cadence production EVENT stream was not retained before ACK"
                    )
                }
                // ACK after the presentation timestamp and mutation transport
                // snapshot, so event settlement is outside decode-to-visible
                // timing and excluded from inbound mutation-frame counters.
                for event in returnedEvents {
                    try await transport.injectFromServer(
                        try framed(
                            acknowledgeMessage(
                                event,
                                outbox: outbox,
                                sessionID: sessionID,
                                revision: finalRevision
                            )
                        )
                    )
                }
                try await waitUntil {
                    let pendingCount = await outbox.pendingCount
                    let lastAcked = await outbox.lastAckedEventSeq
                    return pendingCount == 0
                        && lastAcked == returnedEvents.last?.eventSeq
                }

                let idleStart = await transport.snapshot()
                try await Task.sleep(
                    for: .milliseconds(idleObservationMilliseconds)
                )
                let idleEnd = await transport.snapshot()
                let idleBytes =
                    (idleEnd.inboundBytes - idleStart.inboundBytes)
                        + (idleEnd.outboundBytes - idleStart.outboundBytes)
                let idleMessages =
                    (idleEnd.inboundMessages - idleStart.inboundMessages)
                        + (idleEnd.outboundMessages
                            - idleStart.outboundMessages)

                cadenceTask?.cancel()
                try await cadenceTask?.value
                cadenceTask = nil
                guard let progress =
                    renderer.registry.view(
                        for: fixtureIndex.progress
                    ) as? NSProgressIndicator else {
                    throw BenchmarkFailure.message(
                        "cadence renderer lost its native progress control"
                    )
                }
                let revisionsPreserved = observedRevisions
                    == (2...UInt64(count + 1)).map { $0 }
                    && probe.paintedRevision == finalRevision
                let observation = CadenceObservation(
                    updateCount: count,
                    hz: hz,
                    inboundWireBytes: inboundWireBytes,
                    inboundMessages: inboundMessages,
                    outboundWireBytes: outboundWireBytes,
                    outboundMessages: outboundMessages,
                    repaintCount: probe.repaintCount,
                    decodeToVisibleMilliseconds: decodeToVisible,
                    finalValue: progress.doubleValue,
                    revisionsPreserved: revisionsPreserved,
                    idleBytes: idleBytes,
                    idleMessages: idleMessages,
                    eventFramesMatchReturned: eventFramesMatchReturned,
                    eventOrderSignature: returnedSignature,
                    eventSignatureDigest:
                        cadenceEventSignatureDigest(returnedSignature)
                )
                observations.append(observation)
                sampleCounts["macos.cadence.\(count).\(hz)"] = 1
                sampleCounts[
                    "macos.cadence.events.\(count).\(hz)"
                ] = returnedEvents.count

                let prefix = "cadence.\(count).\(hz)"
                metrics.append(
                    metric(
                        "\(count) updates at \(hz)Hz decode-to-visible",
                        decodeToVisible,
                        "ms",
                        "sample",
                        id: "\(prefix).visible"
                    )
                )
                metrics.append(
                    metric(
                        "\(count) updates at \(hz)Hz total SRUI wire bytes",
                        Double(totalWireBytes),
                        "bytes",
                        "exact",
                        id: "\(prefix).bytes"
                    )
                )
                metrics.append(
                    metric(
                        "\(count) updates at \(hz)Hz total SRUI message count",
                        Double(totalMessages),
                        "messages",
                        "exact",
                        id: "\(prefix).messages"
                    )
                )
                metrics.append(
                    metric(
                        "\(count) updates at \(hz)Hz inbound TRANSACTION bytes",
                        Double(inboundWireBytes),
                        "bytes",
                        "exact",
                        id: "\(prefix).inbound_bytes"
                    )
                )
                metrics.append(
                    metric(
                        "\(count) updates at \(hz)Hz inbound TRANSACTION message count",
                        Double(inboundMessages),
                        "messages",
                        "exact",
                        id: "\(prefix).inbound_messages"
                    )
                )
                metrics.append(
                    metric(
                        "\(count) updates at \(hz)Hz outbound EVENT bytes",
                        Double(outboundWireBytes),
                        "bytes",
                        "exact",
                        id: "\(prefix).outbound_bytes"
                    )
                )
                metrics.append(
                    metric(
                        "\(count) updates at \(hz)Hz outbound EVENT message count",
                        Double(outboundMessages),
                        "messages",
                        "exact",
                        id: "\(prefix).outbound_messages"
                    )
                )
                metrics.append(
                    metric(
                        "\(count) updates at \(hz)Hz synthetic change-gated repaint count",
                        Double(probe.repaintCount),
                        "repaints",
                        "exact",
                        id: "\(prefix).repaints"
                    )
                )

                guard inboundWireBytes == expectedWireBytes,
                      inboundMessages == count else {
                    throw BenchmarkFailure.message(
                        "\(count)-update cadence trial inbound TRANSACTION counters disagreed with the prebuilt frames"
                    )
                }
                guard outboundWireBytes == capturedEventWireBytes,
                      outboundMessages == eventMilestones.count,
                      outboundAttempts == outboundMessages,
                      eventFramesMatchReturned else {
                    throw BenchmarkFailure.message(
                        "\(count)-update cadence trial outbound EVENT delta contained missing, extra, dropped, or non-identical frames"
                    )
                }
                await controller.stop()
                closeRenderer(renderer)
            } catch {
                cadenceTask?.cancel()
                _ = try? await cadenceTask?.value
                await controller.stop()
                closeRenderer(renderer)
                throw error
            }
        }
    }
    let idleBytes = observations.map(\.idleBytes)
    let idleMessages = observations.map(\.idleMessages)
    metrics.append(
        metric(
            "settled idle SRUI wire bytes",
            Double(idleBytes.max() ?? -1),
            "bytes",
            "observed max",
            id: "idle.bytes"
        )
    )
    metrics.append(
        metric(
            "settled idle SRUI message count",
            Double(idleMessages.max() ?? -1),
            "messages",
            "observed max",
            id: "idle.messages"
        )
    )

    let wireInvariant =
        MutationBenchmarkConfiguration.updateCounts.allSatisfy { count in
        let trials = observations.filter { $0.updateCount == count }
        return trials.count == configuredCadences.count
            && Set(trials.map(\.inboundWireBytes)).count == 1
            && Set(trials.map(\.inboundMessages)).count == 1
            && Set(trials.map(\.outboundWireBytes)).count == 1
            && Set(trials.map(\.outboundMessages)).count == 1
            && Set(trials.map(\.wireBytes)).count == 1
            && Set(trials.map(\.messages)).count == 1
            && trials.allSatisfy {
                $0.inboundMessages == count
                    && $0.outboundMessages
                        == cadenceEventMilestones(updateCount: count).count
            }
    }
    let repaintIndependent =
        MutationBenchmarkConfiguration.coalescingUpdateCounts
            .allSatisfy { count in
        let repaintCounts = observations
            .filter { $0.updateCount == count }
            .map(\.repaintCount)
        return Set(repaintCounts).count > 1
            && repaintCounts.allSatisfy { $0 > 0 && $0 <= count }
    }
    let statePreserved = observations.allSatisfy {
        $0.revisionsPreserved && abs($0.finalValue - 1.0) < 0.000_001
    }
    let eventOrderPreserved =
        MutationBenchmarkConfiguration.updateCounts.allSatisfy { count in
        let trials = observations.filter { $0.updateCount == count }
        guard trials.count == configuredCadences.count,
              let reference = trials.first?.eventOrderSignature else {
            return false
        }
        return trials.allSatisfy {
            $0.outboundMessages == 3
                && $0.eventFramesMatchReturned
                && $0.eventOrderSignature == reference
        }
    }
    let cadenceDetails = observations.map {
        "\($0.updateCount)@\($0.hz)Hz=\($0.repaintCount)"
    }.joined(separator: ", ")
    let eventDetails = observations.map {
        "\($0.updateCount)@\($0.hz)Hz=\($0.outboundMessages) EVENT frames/"
            + "\($0.outboundWireBytes)B signature="
            + String($0.eventSignatureDigest.prefix(16))
    }.joined(separator: ", ")
    return Section(
        id: "31.3",
        name: "Mutation and frame independence",
        sampleCounts: sampleCounts,
        metrics: metrics,
        assertions: [
            Assertion(
                id: "mutation_raster_completion",
                name: fullPaint
                    ? "decode-to-visible samples reach an unforced composited content change"
                    : "decode-to-visible smoke samples complete the offscreen raster fallback",
                passed: allMutationRastersCompleted
                    && allMutationStateParityPassed
                    && statePreserved,
                detail: fullPaint
                    ? "every framed production SessionController sample matched isolated semantic state and changed the exact visible client-content fingerprint without benchmark-forced invalidation; every cadence stream ended in its expected native value with passive composited evidence"
                    : "every framed production SessionController smoke sample matched isolated semantic state and produced an AppKit bitmap; every cadence stream ended in its expected native value"
            ),
            Assertion(
                id: "idle_zero_traffic",
                name: "settled idle UI emits zero SRUI traffic",
                passed: idleBytes.allSatisfy { $0 == 0 }
                    && idleMessages.allSatisfy { $0 == 0 },
                detail: "\(idleObservationMilliseconds)ms after all production EVENT ACKs drained for every count/cadence; byte deltas \(idleBytes), message deltas \(idleMessages). This does not claim natural AppKit idle invalidation behavior: the synthetic cadence gate suppresses unchanged-state draws by construction."
            ),
            Assertion(
                id: "cadence_wire_invariant",
                name: "complete bidirectional wire bytes and message count are cadence independent",
                passed: wireInvariant,
                detail: "the same prebuilt 1/100/1000 inbound TRANSACTION frame sequences were reused at 60/120/144/240Hz; every outbound transport delta is exactly the three decoded production EVENT frames with no extra, dropped, or interrupted attempts; total, inbound, and outbound bytes/messages are reported separately and compared across cadences; ACKs are injected only after these counters and the presentation timestamp are frozen"
            ),
            Assertion(
                id: "cadence_repaint_independent",
                name: "synthetic native-change-gated repaint count varies independently",
                passed: repaintIndependent,
                detail: "\(cadenceDetails). This diagnostic measures configured coalescing opportunities; it is not evidence of AppKit's natural invalidation count."
            ),
            Assertion(
                id: "cadence_state_event_order",
                name: "committed state and production EVENT order are cadence independent",
                passed: scalarOnly
                    && statePreserved
                    && eventOrderPreserved,
                detail: "ACTIVATE, VALUE_CHANGED, and SELECTION_CHANGED were emitted through SessionController/EventOutbox at deterministic early/mid/final applied revisions before those revisions entered the cadence paint gate. Every returned ID/sequence/observed-revision/type/node/argument set exactly matched its captured framed EVENT in emission order, and the typed order signature matched across 60/120/144/240Hz for each update count: \(eventDetails)"
            ),
        ],
        notes: [
            "The updates.*.semantic trial times ProtocolDecoder plus TransactionApplier only. A byte-identical framed aggregate trial traverses BenchmarkTransport and SessionController into the pre-presented warm AppKitRenderer for updates.*.visible; final revision/value parity and exact inbound TRANSACTION bytes/messages are asserted.",
            "Full decode-to-visible measurements establish a visible baseline before timing and await a passive WindowServer framebuffer/content-fingerprint change after production mutation; the timed path does not set needsDisplay or call the forced presentation observer. Smoke remains an explicitly offscreen AppKit raster fallback.",
            "For each update count, one pre-encoded TRANSACTION sequence is reused unchanged at 60/120/144/240Hz. Three distinct production client events are interleaved at deterministic applied revisions before the corresponding native state is exposed to the synthetic cadence gate. The complete transport delta is frozen before ACK: inbound, outbound, and total bytes/messages are reported separately; exact outbound EVENT frames cover every outbound index, are decoded, and are compared field-for-field with returned events, proving no unreported frames. ACKs drain outside decode-to-visible timing.",
            "The cadence repaint number is a synthetic change-gated coalescing diagnostic. Because that task intentionally skips unchanged pending state, it cannot establish natural renderer idle invalidation. The settled \(idleObservationMilliseconds)ms assertion is limited to zero SRUI byte/message deltas after EVENT settlement."
        ]
    )
}
