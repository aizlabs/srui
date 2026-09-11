import AppKit
import Foundation
import Protocol
import QuartzCore
import RendererAppKit
import SemanticModel
import Session
import SwiftProtobuf

enum LocalInteractionKind: String, CaseIterable {
    case textEntry = "text_entry"
    case caretMovement = "caret_movement"
    case textSelection = "text_selection"
    case imeComposition = "ime_composition"
    case scrolling
    case hover
    case pressed
    case menuOpening = "menu_opening"
}

enum NetworkBenchmarkConfiguration {
    static let roundTripTimesMilliseconds = [0, 100, 300, 600]
    static let nonzeroRoundTripTimesMilliseconds = [100, 300, 600]
}

struct LocalTextEditCallback {
    let nodeID: NodeId
    let text: String
    let editSeq: EditSeq
    let observedRevision: UInt64
}

struct LocalInteractionResult {
    let samples: [String: [Double]]
    let stateChecksPassed: Bool
    let stateCheckFailures: [String]
    let everyInjectedResponseUnfinishedThroughVisibleCompletion: Bool
    let everyConfiguredDelayStateVerifiedAtActionStart: Bool
    let nonzeroDelayActiveAtActionStartProbeCount: Int
    let heldResponseProbeCount: Int
    let productionCallbacks: Int
    let textEditCallbacks: [LocalTextEditCallback]
    let finalText: String
    let hoverMode: String
    let menuMode: String
}

struct BandwidthLimiterProofSample {
    let framedBytes: Int
    let outboundMessages: Int
    let measuredMilliseconds: Double
    let theoreticalMinimumMilliseconds: Double
    let exactEventMatched: Bool

    var passed: Bool {
        framedBytes > 0
            && outboundMessages == 1
            && measuredMilliseconds.isFinite
            && theoreticalMinimumMilliseconds.isFinite
            && theoreticalMinimumMilliseconds > 0
            && measuredMilliseconds >= theoreticalMinimumMilliseconds
            && exactEventMatched
    }

    var detail: String {
        "\(framedBytes) framed bytes / \(outboundMessages) message: "
            + "measured \(String(format: "%.4f", measuredMilliseconds)) ms >= "
            + "theoretical \(String(format: "%.4f", theoreticalMinimumMilliseconds)) ms; "
            + "exact_event=\(exactEventMatched)"
    }
}

actor TransportFailureObservation {
    private var failures = [SessionFailure]()

    func record(_ failure: SessionFailure) {
        failures.append(failure)
    }

    func snapshot() -> (count: Int, transportEnded: Bool) {
        (
            failures.count,
            failures.contains {
                if case .transportEnded = $0 { return true }
                return false
            }
        )
    }
}

@MainActor
func rendererTextView(
    _ renderer: AppKitRenderer,
    nodeID: NodeId
) -> NSTextView? {
    guard let handle = renderer.registry.handle(for: nodeID) else {
        return nil
    }
    if let view = handle.view as? NSTextView {
        return view
    }
    return (handle.view as? NSScrollView)?.documentView as? NSTextView
}

struct HeldInjectedResponseProof {
    let responseUnfinishedThroughVisibleCompletion: Bool
    let configuredDelayStateVerifiedAtActionStart: Bool
    let nonzeroDelayActiveAtActionStart: Bool
}

typealias StartInjectedResponse =
    @MainActor () async throws -> Void

@MainActor
func withHeldInjectedResponse(
    rttMilliseconds: Int,
    progressNodeID: NodeId,
    controller: SessionController,
    transport: BenchmarkTransport,
    body: @MainActor (
        _ startInjection: @escaping StartInjectedResponse
    ) async throws -> Void
) async throws -> HeldInjectedResponseProof {
    let baseRevision = controller.applier.lastAppliedRevision
    let nextRevision = Revision(baseRevision.value + 1)
    let progress = Double(nextRevision.value % 100) / 100.0
    let response = Transaction(
        baseRevision: baseRevision,
        operations: [
            .setProperty(
                id: progressNodeID,
                property: .value,
                value: .float64(progress)
            )
        ]
    )
    let responseFrame = try framed(transactionMessage(response))
    let deliveryGate = BenchmarkDeliveryGate()
    var receiveTask: Task<Void, Error>?
    var configuredDelayStateVerifiedAtActionStart = false
    var nonzeroDelayActiveAtActionStart = false

    let startInjection: StartInjectedResponse = {
        guard receiveTask == nil else {
            throw BenchmarkFailure.message(
                "held response injection started more than once"
            )
        }
        benchmarkTrace(
            "31.4 rtt=\(rttMilliseconds) held receive start at action boundary"
        )
        let task = Task {
            try await transport.injectFromServer(
                responseFrame,
                deliveryGate: deliveryGate
            )
        }
        receiveTask = task
        try await waitUntil {
            await deliveryGate.snapshot().started
        }
        let delaySnapshot = await transport.snapshot()
        if rttMilliseconds == 0 {
            configuredDelayStateVerifiedAtActionStart =
                delaySnapshot.activeDelayedOperations == 0
        } else {
            nonzeroDelayActiveAtActionStart =
                delaySnapshot.activeDelayedOperations > 0
            configuredDelayStateVerifiedAtActionStart =
                nonzeroDelayActiveAtActionStart
        }
    }

    do {
        try await body(startInjection)
        guard let receiveTask else {
            throw BenchmarkFailure.message(
                "held response injection never reached the action boundary"
            )
        }
        benchmarkTrace("31.4 rtt=\(rttMilliseconds) held body end")
        let visibleSnapshot = await deliveryGate.snapshot()
        await deliveryGate.release()
        try await receiveTask.value
        try await waitForRevision(nextRevision, controller: controller)
        benchmarkTrace("31.4 rtt=\(rttMilliseconds) held receive end")
        return HeldInjectedResponseProof(
            responseUnfinishedThroughVisibleCompletion:
                visibleSnapshot.started
                    && visibleSnapshot.finished == false,
            configuredDelayStateVerifiedAtActionStart:
                configuredDelayStateVerifiedAtActionStart,
            nonzeroDelayActiveAtActionStart:
                nonzeroDelayActiveAtActionStart
        )
    } catch {
        receiveTask?.cancel()
        await deliveryGate.release()
        if let receiveTask {
            _ = try? await receiveTask.value
        }
        throw error
    }
}

@MainActor
func benchmarkMouseEvent(
    _ type: NSEvent.EventType,
    window: NSWindow,
    location: NSPoint,
    eventNumber: Int
) throws -> NSEvent {
    guard let event = NSEvent.mouseEvent(
        with: type,
        location: location,
        modifierFlags: [],
        timestamp: ProcessInfo.processInfo.systemUptime,
        windowNumber: window.windowNumber,
        context: nil,
        eventNumber: eventNumber,
        clickCount: 1,
        pressure: [.leftMouseDown, .rightMouseDown, .otherMouseDown]
            .contains(type) ? 1 : 0
    ) else {
        throw BenchmarkFailure.message(
            "could not construct AppKit mouse event"
        )
    }
    return event
}
@MainActor
func benchmarkSettleLocalFeedback(_ view: NSView) {
    view.displayIfNeeded()
    view.window?.displayIfNeeded()
    CATransaction.flush()
    pumpRunLoop(for: 0.03)
}
final class SmokeMenuTrackingProbe: NSObject, NSMenuDelegate {
    private(set) var openCount = 0
    private(set) var presentationCount = 0
    private(set) var observedAt: ContinuousClock.Instant?
    private weak var host: NSView?

    func prepare(host: NSView) {
        self.host = host
        observedAt = nil
    }

    func menuWillOpen(_ menu: NSMenu) {
        openCount += 1
    }

    @MainActor
    @objc func observePresentationAndCancel(_ menu: NSMenu) {
        if let host, rasterize(host) {
            presentationCount += 1
            observedAt = clock.now
        }
        menu.cancelTrackingWithoutAnimation()
    }
}

struct LocalInteractionMeasurement {
    let latencyMilliseconds: Double
    let passed: Bool
    let checkDetail: String
}

@MainActor
final class LocalInteractionRecorder {
    let window: NSWindow
    let host: NSView
    let progressNodeID: NodeId
    let controller: SessionController
    let transport: BenchmarkTransport
    let rttMilliseconds: Int
    let fullPaint: Bool

    private(set) var samples = [String: [Double]]()
    private(set) var stateChecksPassed = true
    private(set) var stateCheckFailures = [String]()
    private(set) var
        everyInjectedResponseUnfinishedThroughVisibleCompletion = true
    private(set) var everyConfiguredDelayStateVerifiedAtActionStart = true
    private(set) var nonzeroDelayActiveAtActionStartProbeCount = 0
    private(set) var heldResponseProbeCount = 0
    private(set) var hoverActionMaterialPixelCounts = [Int]()
    private(set) var hoverRestorationMaximumChannelDeltas = [Int]()

    init(
        window: NSWindow,
        host: NSView,
        progressNodeID: NodeId,
        controller: SessionController,
        transport: BenchmarkTransport,
        rttMilliseconds: Int,
        fullPaint: Bool
    ) {
        self.window = window
        self.host = host
        self.progressNodeID = progressNodeID
        self.controller = controller
        self.transport = transport
        self.rttMilliseconds = rttMilliseconds
        self.fullPaint = fullPaint
    }

    func record(
        _ kind: LocalInteractionKind,
        measure: @escaping @MainActor (
            _ startInjection: @escaping StartInjectedResponse
        ) async throws -> LocalInteractionMeasurement
    ) async throws {
        let id = kind.rawValue
        benchmarkTrace("31.4 rtt=\(rttMilliseconds) \(id) start")
        var measurement: LocalInteractionMeasurement?
        let responseProof = try await withHeldInjectedResponse(
            rttMilliseconds: rttMilliseconds,
            progressNodeID: progressNodeID,
            controller: controller,
            transport: transport
        ) { startInjection in
            measurement = try await measure(startInjection)
        }
        guard let measurement else {
            throw BenchmarkFailure.message(
                "\(id) produced no local interaction measurement"
            )
        }

        samples[id, default: []].append(
            measurement.latencyMilliseconds
        )
        if measurement.passed == false {
            stateCheckFailures.append(
                "\(id): \(measurement.checkDetail)"
            )
        }
        stateChecksPassed =
            stateChecksPassed && measurement.passed
        everyInjectedResponseUnfinishedThroughVisibleCompletion =
            everyInjectedResponseUnfinishedThroughVisibleCompletion
                && responseProof
                    .responseUnfinishedThroughVisibleCompletion
        everyConfiguredDelayStateVerifiedAtActionStart =
            everyConfiguredDelayStateVerifiedAtActionStart
                && responseProof
                    .configuredDelayStateVerifiedAtActionStart
        if responseProof.nonzeroDelayActiveAtActionStart {
            nonzeroDelayActiveAtActionStartProbeCount += 1
        }
        heldResponseProbeCount += 1
        benchmarkTrace("31.4 rtt=\(rttMilliseconds) \(id) end")
    }

    func recordView(
        _ kind: LocalInteractionKind,
        targetView: NSView,
        requiresExactCompositedRestoration: Bool = false,
        acceptFramesDuringAction: Bool = false,
        action: @escaping @MainActor () throws -> Bool,
        cleanup: @escaping @MainActor () -> Bool = { true }
    ) async throws {
        try await record(kind) { startInjection in
            var cleanupResult: Bool?
            do {
                let stateCorrect: Bool
                let pixels: Bool
                let latencyMilliseconds: Double

                if self.fullPaint {
                    var actionStateCorrect = false
                    let restorationAction:
                        (@MainActor () throws -> Bool)?
                    if requiresExactCompositedRestoration {
                        restorationAction = {
                            let result = cleanup()
                            cleanupResult = result
                            return result
                        }
                    } else {
                        restorationAction = nil
                    }
                    let measured =
                        try await benchmarkMeasurePassiveCompositedChange(
                            self.window,
                            targetView: targetView,
                            acceptFramesDuringAction:
                                acceptFramesDuringAction,
                            onActionStarting: startInjection,
                            restorationAction: restorationAction
                        ) {
                            benchmarkTrace(
                                "31.4 rtt=\(self.rttMilliseconds) "
                                    + "\(kind.rawValue) action start"
                            )
                            actionStateCorrect = try action()
                            benchmarkTrace(
                                "31.4 rtt=\(self.rttMilliseconds) "
                                    + "\(kind.rawValue) action end"
                            )
                        }
                    stateCorrect = actionStateCorrect
                    pixels =
                        measured.observation.crossedDisplayRefresh
                            && measured.observation.captureAuthorization
                            && measured.observation.pixelCaptureVerified
                    latencyMilliseconds =
                        measured.presentationLatencyMilliseconds

                    if requiresExactCompositedRestoration {
                        guard let actionDelta =
                                measured.contentDeltaEvidence,
                              let restorationDelta =
                                measured.restorationDeltaEvidence else {
                            throw BenchmarkFailure.message(
                                "composited restoration measurement omitted "
                                    + "its action or restoration delta evidence"
                            )
                        }
                        self.hoverActionMaterialPixelCounts.append(
                            actionDelta.materiallyDifferentPixelCount
                        )
                        self.hoverRestorationMaximumChannelDeltas.append(
                            restorationDelta.maximumChannelDelta
                        )
                    }
                } else {
                    try await startInjection()
                    let start = clock.now
                    benchmarkTrace(
                        "31.4 rtt=\(self.rttMilliseconds) "
                            + "\(kind.rawValue) action start"
                    )
                    stateCorrect = try action()
                    benchmarkTrace(
                        "31.4 rtt=\(self.rttMilliseconds) "
                            + "\(kind.rawValue) action end"
                    )
                    pixels = rasterize(self.host)
                    latencyMilliseconds = milliseconds(
                        start.duration(to: clock.now)
                    )
                }

                benchmarkTrace(
                    "31.4 rtt=\(self.rttMilliseconds) "
                        + "\(kind.rawValue) paint end"
                )
                let cleanupCorrect: Bool
                if let cleanupResult {
                    cleanupCorrect = cleanupResult
                } else {
                    let result = cleanup()
                    cleanupResult = result
                    cleanupCorrect = result
                }
                return LocalInteractionMeasurement(
                    latencyMilliseconds: latencyMilliseconds,
                    passed:
                        stateCorrect && pixels && cleanupCorrect,
                    checkDetail:
                        "action=\(stateCorrect) pixels=\(pixels) "
                            + "cleanup=\(cleanupCorrect)"
                )
            } catch {
                if cleanupResult == nil {
                    _ = cleanup()
                }
                throw error
            }
        }
    }
}

func acknowledgementForCapturedEvent(
    _ event: CapturedEvent,
    outbox: EventOutbox,
    sessionID: String,
    revision: Revision
) -> SRUIMessage {
    var acknowledgement = SRUIServerEventAck()
    acknowledgement.sessionID = sessionID
    acknowledgement.clientInstanceID = outbox.clientInstanceId.bytes
    acknowledgement.eventID = event.id
    acknowledgement.lastProcessedEventSeq = event.sequence
    acknowledgement.settledEventSeq = event.sequence
    acknowledgement.status = .processed
    acknowledgement.revisionAfterEffect = revision.value
    var message = SRUIMessage()
    message.serverEventAck = acknowledgement
    return message
}
