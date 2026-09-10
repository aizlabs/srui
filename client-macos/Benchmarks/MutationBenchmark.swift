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
func mutationRun(
    baseStore: SemanticStore,
    fixtureOperations: [SemanticModel.Operation],
    operations: [SemanticModel.Operation],
    fullPaint: Bool
) async throws -> (
    semanticLatency: Double,
    visibleLatency: Double,
    bytes: Int,
    messages: Int,
    classifications: [DirtyClassification],
    rasterized: Bool,
    stateParity: Bool
) {
    let transaction = Transaction(
        baseRevision: baseStore.revision,
        operations: operations
    )
    let wireBytes = try framed(transactionMessage(transaction))

    let semanticApplier = TransactionApplier(store: baseStore)
    let semanticStart = clock.now
    let decodedMessage = try SRUIFraming.decodeFramed(
        SRUIMessage.self,
        from: wireBytes
    )
    guard case .transaction(let wireTransaction)? = decodedMessage.msg else {
        throw BenchmarkFailure.message(
            "framed mutation did not contain a transaction"
        )
    }
    let decoded = try ProtocolDecoder().validateAndConvertTransaction(
        wire: wireTransaction
    )
    guard case .success = semanticApplier.apply(record: decoded) else {
        throw BenchmarkFailure.message(
            "mutation transaction did not apply through TransactionApplier"
        )
    }
    let semanticLatency = milliseconds(semanticStart.duration(to: clock.now))
    let semanticSnapshot = semanticApplier.currentSnapshot
    let classifications = DirtyClassifier.classify(decoded)
    guard case .float64(let expectedProgress)? =
        semanticSnapshot.store.getNode(NodeId(5))?.properties[.value] else {
        throw BenchmarkFailure.message(
            "semantic mutation produced no progress value"
        )
    }

    let transport = BenchmarkTransport()
    let renderer = AppKitRenderer()
    let controller = try await startActiveSession(
        transport: transport,
        renderer: renderer,
        fixtureOperations: fixtureOperations,
        sessionID: "mutation-\(operations.count)"
    )
    do {
        let warmObservation = try await observeNativePresentation(
            renderer,
            fullPaint: fullPaint
        )
        guard warmObservation.crossedDisplayRefresh else {
            throw BenchmarkFailure.message(
                "steady-state mutation renderer did not warm"
            )
        }

        let beforeWire = await transport.snapshot()
        let expectedRevision = Revision(baseStore.revision.value + 1)
        let applyProductionMutation: @MainActor () async throws -> Void = {
            try await transport.injectFromServer(wireBytes)
            try await waitForRevision(
                expectedRevision,
                controller: controller
            )
            try await waitUntil {
                guard let progress = renderer.registry.view(for: NodeId(5))
                    as? NSProgressIndicator else {
                    return false
                }
                return abs(progress.doubleValue - expectedProgress) < 0.000_001
            }
        }

        let presentationObservation: OnScreenPaintObservation
        let visibleLatency: Double
        if fullPaint {
            let windows = renderer.registry.surfaceHandles.compactMap(\.window)
            guard windows.count == 1,
                  let window = windows.first,
                  let progress = renderer.registry.view(for: NodeId(5))
                    as? NSProgressIndicator,
                  progress.window === window else {
                throw BenchmarkFailure.message(
                    "steady-state mutation fixture must expose one attached "
                        + "progress target in one presentation window"
                )
            }
            let measured = try await benchmarkMeasurePassiveCompositedChange(
                window,
                targetView: progress
            ) {
                try await applyProductionMutation()
            }
            presentationObservation = measured.observation
            visibleLatency = measured.presentationLatencyMilliseconds
        } else {
            let start = clock.now
            try await applyProductionMutation()
            presentationObservation = try await observeNativePresentation(
                renderer,
                fullPaint: false
            )
            visibleLatency = milliseconds(
                start.duration(to: presentationObservation.presentedAt)
            )
        }

        let afterWire = await transport.snapshot()
        let productionSnapshot = controller.applier.currentSnapshot
        let stateParity =
            productionSnapshot.revision == semanticSnapshot.revision
                && productionSnapshot.store.getNode(
                    NodeId(5)
                )?.properties[.value]
                    == semanticSnapshot.store.getNode(
                        NodeId(5)
                    )?.properties[.value]
        let measuredBytes =
            afterWire.inboundBytes - beforeWire.inboundBytes
        let measuredMessages =
            afterWire.inboundMessages - beforeWire.inboundMessages
        await controller.stop()
        closeRenderer(renderer)
        return (
            semanticLatency,
            visibleLatency,
            measuredBytes,
            measuredMessages,
            classifications,
            presentationObservation.crossedDisplayRefresh,
            stateParity
                && measuredBytes == wireBytes.count
                && measuredMessages == 1
        )
    } catch {
        await controller.stop()
        closeRenderer(renderer)
        throw error
    }
}

@MainActor
private final class CadenceProbe {
    var repaintCount = 0
    var pendingNativeUpdateIndex: Int?
    var paintedUpdateIndex: Int?
    var paintedRevision: Revision?
}

private enum CadenceProductionEventKind {
    case activate
    case valueChanged
    case selectionChanged
}

private struct CadenceEventMilestone {
    let updateIndex: Int
    let kind: CadenceProductionEventKind
}

private struct CadenceEventArgumentSignature: Hashable {
    let property: PropertyRef
    let value: SemanticModel.Value
}

private struct CadenceEventOrderSignature: Hashable {
    let sequence: UInt64
    let observedRevision: UInt64
    let eventType: TypeRef
    let nodeID: NodeId
    let arguments: [CadenceEventArgumentSignature]
}

private struct CapturedCadenceProductionEvent {
    let returned: SemanticModel.Event
    let captured: SemanticModel.Event
    let frame: CapturedTransportFrame
    let frameIndex: Int
}

private func cadenceEventMilestones(updateCount: Int) -> [CadenceEventMilestone] {
    [
        CadenceEventMilestone(updateIndex: 1, kind: .activate),
        CadenceEventMilestone(
            updateIndex: max(1, (updateCount + 1) / 2),
            kind: .valueChanged
        ),
        CadenceEventMilestone(
            updateIndex: updateCount,
            kind: .selectionChanged
        ),
    ]
}

private func cadenceEventOrderSignature(
    _ events: [SemanticModel.Event]
) -> [CadenceEventOrderSignature] {
    events.map { event in
        CadenceEventOrderSignature(
            sequence: event.eventSeq,
            observedRevision: event.observedRevision.value,
            eventType: event.eventType,
            nodeID: event.nodeId,
            arguments: event.arguments
                .map {
                    CadenceEventArgumentSignature(
                        property: $0.key,
                        value: $0.value
                    )
                }
                .sorted { $0.property < $1.property }
        )
    }
}

private func cadenceEventSignatureDigest(
    _ signature: [CadenceEventOrderSignature]
) -> String {
    let canonical = signature.map { event in
        let arguments = event.arguments.map {
            "\($0.property.namespaceID):\($0.property.localID)=\($0.value)"
        }.joined(separator: ",")
        return "\(event.sequence)@\(event.observedRevision):"
            + "\(event.eventType.namespaceID):\(event.eventType.localID):"
            + "\(event.nodeID.value):[\(arguments)]"
    }.joined(separator: "|")
    return SHA256.hash(data: Data(canonical.utf8))
        .map { String(format: "%02x", $0) }
        .joined()
}

private func cadenceEventFieldsMatch(
    returned: SemanticModel.Event,
    captured: SemanticModel.Event
) -> Bool {
    returned.eventId == captured.eventId
        && returned.eventSeq == captured.eventSeq
        && returned.observedRevision == captured.observedRevision
        && returned.eventType == captured.eventType
        && returned.nodeId == captured.nodeId
        && returned.arguments == captured.arguments
        && returned.clientInstanceId == captured.clientInstanceId
        && returned.editSeq == captured.editSeq
}

@MainActor
private func sendAndCaptureCadenceProductionEvent(
    kind: CadenceProductionEventKind,
    updateCount: Int,
    controller: SessionController,
    transport: BenchmarkTransport
) async throws -> CapturedCadenceProductionEvent {
    let firstPossibleFrameIndex = await transport.framesSent().count
    let returned: SemanticModel.Event
    switch kind {
    case .activate:
        returned = try await controller.sendActivate(nodeId: NodeId(16))
    case .valueChanged:
        returned = try await controller.sendValueChanged(
            nodeId: NodeId(5),
            value: .float64(Double(updateCount) / 1_000.0)
        )
    case .selectionChanged:
        returned = try await controller.sendSelectionChanged(
            nodeId: NodeId(7),
            itemId: ItemId(UInt64(updateCount))
        )
    }

    let frames = await transport.framesSent()
    for frameIndex in firstPossibleFrameIndex..<frames.count {
        let frame = frames[frameIndex]
        guard frame.logicalClass == .input else { continue }
        let message = try SRUIFraming.decodeFramed(
            SRUIMessage.self,
            from: frame.data
        )
        guard case .event(let wireEvent)? = message.msg else {
            throw BenchmarkFailure.message(
                "cadence input-channel frame was not an EVENT"
            )
        }
        let captured = try ProtocolDecoder().validateAndConvertEvent(
            wire: wireEvent
        )
        if cadenceEventFieldsMatch(returned: returned, captured: captured) {
            return CapturedCadenceProductionEvent(
                returned: returned,
                captured: captured,
                frame: frame,
                frameIndex: frameIndex
            )
        }
    }
    throw BenchmarkFailure.message(
        "production cadence event had no exact captured framed EVENT"
    )
}

private struct CadenceObservation {
    let updateCount: Int
    let hz: Int
    let inboundWireBytes: Int
    let inboundMessages: Int
    let outboundWireBytes: Int
    let outboundMessages: Int
    let repaintCount: Int
    let decodeToVisibleMilliseconds: Double
    let finalValue: Double
    let revisionsPreserved: Bool
    let idleBytes: Int
    let idleMessages: Int
    let eventFramesMatchReturned: Bool
    let eventOrderSignature: [CadenceEventOrderSignature]
    let eventSignatureDigest: String

    var wireBytes: Int {
        inboundWireBytes + outboundWireBytes
    }

    var messages: Int {
        inboundMessages + outboundMessages
    }
}

@MainActor
func mutationAndCadence(
    fixtureOperations: [SemanticModel.Operation],
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

    for count in [1, 100, 1_000] {
        let updates = (0..<count).map { index in
            SemanticModel.Operation.setProperty(
                id: NodeId(5),
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

    let configuredCadences = [60, 120, 144, 240]
    let idleObservationMilliseconds = 1_000
    var observations = [CadenceObservation]()

    for count in [1, 100, 1_000] {
        // The exact same pre-encoded transaction sequence is replayed at every cadence.
        let framedUpdates = try (1...count).map { index in
            let transaction = Transaction(
                baseRevision: Revision(UInt64(index)),
                operations: [
                    .setProperty(
                        id: NodeId(5),
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
                            renderer.registry.view(for: NodeId(5))
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
                            renderer.registry.view(for: NodeId(5))
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
                                renderer.registry.view(for: NodeId(5))
                                    as? NSProgressIndicator else {
                                return false
                            }
                            return abs(progress.doubleValue - expectedValue)
                                < 0.000_001
                        }
                        guard let progress =
                            renderer.registry.view(for: NodeId(5))
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
                    renderer.registry.view(for: NodeId(5))
                        as? NSProgressIndicator else {
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

    let wireInvariant = [1, 100, 1_000].allSatisfy { count in
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
    let repaintIndependent = [100, 1_000].allSatisfy { count in
        let repaintCounts = observations
            .filter { $0.updateCount == count }
            .map(\.repaintCount)
        return Set(repaintCounts).count > 1
            && repaintCounts.allSatisfy { $0 > 0 && $0 <= count }
    }
    let statePreserved = observations.allSatisfy {
        $0.revisionsPreserved && abs($0.finalValue - 1.0) < 0.000_001
    }
    let eventOrderPreserved = [1, 100, 1_000].allSatisfy { count in
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
