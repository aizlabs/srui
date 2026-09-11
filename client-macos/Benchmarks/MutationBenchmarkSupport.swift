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

enum MutationBenchmarkConfiguration {
    static let updateCounts = [1, 100, 1_000]
    static let coalescingUpdateCounts = [100, 1_000]
    static let configuredCadences = [60, 120, 144, 240]
    static let idleObservationMilliseconds = 1_000
}

@MainActor
func mutationRun(
    baseStore: SemanticStore,
    fixtureOperations: [SemanticModel.Operation],
    fixtureIndex: BenchmarkFixtureIndex,
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
    let wireBytes = try framed(
        transactionMessage(transaction)
    )

    let semanticApplier =
        TransactionApplier(store: baseStore)
    let semanticStart = clock.now
    let decodedMessage = try SRUIFraming.decodeFramed(
        SRUIMessage.self,
        from: wireBytes
    )
    guard case .transaction(let wireTransaction)? =
            decodedMessage.msg else {
        throw BenchmarkFailure.message(
            "framed mutation did not contain a transaction"
        )
    }
    let decoded =
        try ProtocolDecoder()
            .validateAndConvertTransaction(
                wire: wireTransaction
            )
    guard case .success =
            semanticApplier.apply(record: decoded) else {
        throw BenchmarkFailure.message(
            "mutation transaction did not apply through "
                + "TransactionApplier"
        )
    }
    let semanticLatency = milliseconds(
        semanticStart.duration(to: clock.now)
    )
    let semanticSnapshot =
        semanticApplier.currentSnapshot
    let classifications =
        DirtyClassifier.classify(decoded)
    guard case .float64(let expectedProgress)? =
            semanticSnapshot.store
                .getNode(fixtureIndex.progress)?
                .properties[.value] else {
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
        fixtureIndex: fixtureIndex,
        sessionID: "mutation-\(operations.count)"
    )
    do {
        let warmObservation =
            try await observeNativePresentation(
                renderer,
                fullPaint: fullPaint
            )
        guard warmObservation.crossedDisplayRefresh else {
            throw BenchmarkFailure.message(
                "steady-state mutation renderer did not warm"
            )
        }

        let beforeWire = await transport.snapshot()
        let expectedRevision =
            Revision(baseStore.revision.value + 1)
        let applyProductionMutation:
            @MainActor () async throws -> Void = {
                try await transport.injectFromServer(
                    wireBytes
                )
                try await waitForRevision(
                    expectedRevision,
                    controller: controller
                )
                try await waitUntil {
                    guard let progress =
                            renderer.registry.view(
                                for: fixtureIndex.progress
                            ) as? NSProgressIndicator else {
                        return false
                    }
                    return abs(
                        progress.doubleValue
                            - expectedProgress
                    ) < 0.000_001
                }
            }

        let presentationObservation:
            OnScreenPaintObservation
        let visibleLatency: Double
        if fullPaint {
            let windows =
                renderer.registry.surfaceHandles
                    .compactMap(\.window)
            guard windows.count == 1,
                  let window = windows.first,
                  let progress =
                    renderer.registry.view(
                        for: fixtureIndex.progress
                    ) as? NSProgressIndicator,
                  progress.window === window else {
                throw BenchmarkFailure.message(
                    "steady-state mutation fixture must expose "
                        + "one attached progress target in one "
                        + "presentation window"
                )
            }
            let measured =
                try await benchmarkMeasurePassiveCompositedChange(
                    window,
                    targetView: progress
                ) {
                    try await applyProductionMutation()
                }
            presentationObservation =
                measured.observation
            visibleLatency =
                measured.presentationLatencyMilliseconds
        } else {
            let start = clock.now
            try await applyProductionMutation()
            presentationObservation =
                try await observeNativePresentation(
                    renderer,
                    fullPaint: false
                )
            visibleLatency = milliseconds(
                start.duration(
                    to: presentationObservation.presentedAt
                )
            )
        }

        let afterWire = await transport.snapshot()
        let productionSnapshot =
            controller.applier.currentSnapshot
        let stateParity =
            productionSnapshot.revision
                == semanticSnapshot.revision
                && productionSnapshot.store
                    .getNode(fixtureIndex.progress)?
                    .properties[.value]
                    == semanticSnapshot.store
                        .getNode(fixtureIndex.progress)?
                        .properties[.value]
        let measuredBytes =
            afterWire.inboundBytes
                - beforeWire.inboundBytes
        let measuredMessages =
            afterWire.inboundMessages
                - beforeWire.inboundMessages
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
final class CadenceProbe {
    var repaintCount = 0
    var pendingNativeUpdateIndex: Int?
    var paintedUpdateIndex: Int?
    var paintedRevision: Revision?
}

enum CadenceProductionEventKind {
    case activate
    case valueChanged
    case selectionChanged
}

struct CadenceEventMilestone {
    let updateIndex: Int
    let kind: CadenceProductionEventKind
}

struct CadenceEventArgumentSignature: Hashable {
    let property: PropertyRef
    let value: SemanticModel.Value
}

struct CadenceEventOrderSignature: Hashable {
    let sequence: UInt64
    let observedRevision: UInt64
    let eventType: TypeRef
    let nodeID: NodeId
    let arguments: [CadenceEventArgumentSignature]
}

struct CapturedCadenceProductionEvent {
    let returned: SemanticModel.Event
    let captured: SemanticModel.Event
    let frame: CapturedTransportFrame
    let frameIndex: Int
}

func cadenceEventMilestones(
    updateCount: Int
) -> [CadenceEventMilestone] {
    [
        CadenceEventMilestone(
            updateIndex: 1,
            kind: .activate
        ),
        CadenceEventMilestone(
            updateIndex: max(
                1,
                (updateCount + 1) / 2
            ),
            kind: .valueChanged
        ),
        CadenceEventMilestone(
            updateIndex: updateCount,
            kind: .selectionChanged
        ),
    ]
}

func cadenceEventOrderSignature(
    _ events: [SemanticModel.Event]
) -> [CadenceEventOrderSignature] {
    events.map { event in
        CadenceEventOrderSignature(
            sequence: event.eventSeq,
            observedRevision:
                event.observedRevision.value,
            eventType: event.eventType,
            nodeID: event.nodeId,
            arguments: event.arguments
                .map {
                    CadenceEventArgumentSignature(
                        property: $0.key,
                        value: $0.value
                    )
                }
                .sorted {
                    $0.property < $1.property
                }
        )
    }
}

func cadenceEventSignatureDigest(
    _ signature: [CadenceEventOrderSignature]
) -> String {
    let canonical = signature.map { event in
        let arguments = event.arguments.map {
            "\($0.property.namespaceID):"
                + "\($0.property.localID)=\($0.value)"
        }.joined(separator: ",")
        return "\(event.sequence)@"
            + "\(event.observedRevision):"
            + "\(event.eventType.namespaceID):"
            + "\(event.eventType.localID):"
            + "\(event.nodeID.value):[\(arguments)]"
    }.joined(separator: "|")
    return SHA256.hash(data: Data(canonical.utf8))
        .map { String(format: "%02x", $0) }
        .joined()
}

func cadenceEventFieldsMatch(
    returned: SemanticModel.Event,
    captured: SemanticModel.Event
) -> Bool {
    returned.eventId == captured.eventId
        && returned.eventSeq == captured.eventSeq
        && returned.observedRevision
            == captured.observedRevision
        && returned.eventType == captured.eventType
        && returned.nodeId == captured.nodeId
        && returned.arguments == captured.arguments
        && returned.clientInstanceId
            == captured.clientInstanceId
        && returned.editSeq == captured.editSeq
}

@MainActor
func sendAndCaptureCadenceProductionEvent(
    kind: CadenceProductionEventKind,
    updateCount: Int,
    fixtureIndex: BenchmarkFixtureIndex,
    controller: SessionController,
    transport: BenchmarkTransport
) async throws -> CapturedCadenceProductionEvent {
    let firstPossibleFrameIndex =
        await transport.framesSent().count
    let returned: SemanticModel.Event
    switch kind {
    case .activate:
        returned = try await controller.sendActivate(
            nodeId: fixtureIndex.primaryAction
        )
    case .valueChanged:
        returned =
            try await controller.sendValueChanged(
                nodeId: fixtureIndex.progress,
                value: .float64(
                    Double(updateCount) / 1_000.0
                )
            )
    case .selectionChanged:
        returned =
            try await controller.sendSelectionChanged(
                nodeId: fixtureIndex.fileTree,
                itemId: ItemId(UInt64(updateCount))
            )
    }

    let frames = await transport.framesSent()
    for frameIndex in
        firstPossibleFrameIndex..<frames.count {
        let frame = frames[frameIndex]
        guard frame.logicalClass == .input else {
            continue
        }
        let message = try SRUIFraming.decodeFramed(
            SRUIMessage.self,
            from: frame.data
        )
        guard case .event(let wireEvent)? =
                message.msg else {
            throw BenchmarkFailure.message(
                "cadence input-channel frame was not an EVENT"
            )
        }
        let captured =
            try ProtocolDecoder().validateAndConvertEvent(
                wire: wireEvent
            )
        if cadenceEventFieldsMatch(
            returned: returned,
            captured: captured
        ) {
            return CapturedCadenceProductionEvent(
                returned: returned,
                captured: captured,
                frame: frame,
                frameIndex: frameIndex
            )
        }
    }
    throw BenchmarkFailure.message(
        "production cadence event had no exact captured "
            + "framed EVENT"
    )
}

struct CadenceObservation {
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
    let eventOrderSignature:
        [CadenceEventOrderSignature]
    let eventSignatureDigest: String

    var wireBytes: Int {
        inboundWireBytes + outboundWireBytes
    }

    var messages: Int {
        inboundMessages + outboundMessages
    }
}
