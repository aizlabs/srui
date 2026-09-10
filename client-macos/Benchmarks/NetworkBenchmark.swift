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

struct LocalTextEditCallback {
    let nodeID: NodeId
    let text: String
    let editSeq: EditSeq
    let observedRevision: UInt64
}
struct LocalInteractionResult {
    let samples: [String: [Double]]
    let stateChecksPassed: Bool
    let everyInjectedResponseUnfinishedThroughVisibleCompletion: Bool
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
func rendererTextView(_ renderer: AppKitRenderer) -> NSTextView? {
    guard let handle = renderer.registry.handle(for: NodeId(14)) else { return nil }
    if let view = handle.view as? NSTextView { return view }
    return (handle.view as? NSScrollView)?.documentView as? NSTextView
}

@MainActor
func withHeldInjectedResponse(
    rttMilliseconds: Int,
    controller: SessionController,
    transport: BenchmarkTransport,
    body: @MainActor () async throws -> Void
) async throws -> Bool {
    let baseRevision = controller.applier.lastAppliedRevision
    let nextRevision = Revision(baseRevision.value + 1)
    let progress = Double(nextRevision.value % 100) / 100.0
    let response = Transaction(
        baseRevision: baseRevision,
        operations: [
            .setProperty(
                id: NodeId(5),
                property: .value,
                value: .float64(progress)
            )
        ]
    )
    let responseFrame = try framed(transactionMessage(response))
    let deliveryGate = BenchmarkDeliveryGate()
    benchmarkTrace("31.4 rtt=\(rttMilliseconds) held receive start")
    let receiveTask = Task {
        try await transport.injectFromServer(
            responseFrame,
            deliveryGate: deliveryGate
        )
    }
    do {
        try await waitUntil {
            await deliveryGate.snapshot().started
        }
        try await body()
        benchmarkTrace("31.4 rtt=\(rttMilliseconds) held body end")
        let visibleSnapshot = await deliveryGate.snapshot()
        await deliveryGate.release()
        try await receiveTask.value
        try await waitForRevision(nextRevision, controller: controller)
        benchmarkTrace("31.4 rtt=\(rttMilliseconds) held receive end")
        return visibleSnapshot.started && visibleSnapshot.finished == false
    } catch {
        receiveTask.cancel()
        await deliveryGate.release()
        _ = try? await receiveTask.value
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
        pressure: type == .leftMouseDown ? 1 : 0
    ) else {
        throw BenchmarkFailure.message("could not construct AppKit mouse event")
    }
    return event
}

@MainActor
func benchmarkTrackingEvent(
    _ type: NSEvent.EventType,
    window: NSWindow,
    location: NSPoint,
    eventNumber: Int
) throws -> NSEvent {
    guard let event = NSEvent.enterExitEvent(
        with: type,
        location: location,
        modifierFlags: [],
        timestamp: ProcessInfo.processInfo.systemUptime,
        windowNumber: window.windowNumber,
        context: nil,
        eventNumber: eventNumber,
        trackingNumber: 1,
        userData: nil
    ) else {
        throw BenchmarkFailure.message("could not construct AppKit tracking event")
    }
    return event
}
@MainActor
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

    @objc func observePresentationAndCancel(_ menu: NSMenu) {
        if let host, rasterize(host) {
            presentationCount += 1
            observedAt = clock.now
        }
        menu.cancelTrackingWithoutAnimation()
    }
}
@MainActor
func localInteractionSamples(
    renderer: AppKitRenderer,
    controller: SessionController,
    transport: BenchmarkTransport,
    sessionID: String,
    rttMilliseconds: Int,
    iterations: Int,
    fullPaint: Bool
) async throws -> LocalInteractionResult {
    let textHandle = renderer.registry.handle(for: NodeId(14))
    let textView = rendererTextView(renderer)
    let textScroll = (textHandle?.view as? NSScrollView) ?? textView?.enclosingScrollView
    let button = renderer.registry.view(for: NodeId(16)) as? NSButton
    let surface = renderer.registry.handle(for: NodeId(1))
    let window = surface?.window
    let host = window?.contentView
    let productionHandler = renderer.onInteraction
    guard let textView,
          let textScroll,
          let button,
          let window,
          let host,
          let productionHandler else {
        throw BenchmarkFailure.message(
            "representative native interaction controls did not mount: "
                + "text_handle=\(textHandle != nil), text_view=\(textView != nil), "
                + "text_scroll=\(textScroll != nil), button=\(button != nil), "
                + "surface=\(surface != nil), window=\(window != nil), "
                + "host=\(host != nil), production_handler=\(productionHandler != nil)"
        )
    }
    if fullPaint {
        window.makeKeyAndOrderFront(nil)
        guard window.makeFirstResponder(textView),
              window.firstResponder === textView,
              textView.inputContext != nil else {
            throw BenchmarkFailure.message(
                "full local text interaction benchmark requires the production "
                    + "NSTextView as first responder with an input context"
            )
        }
    }
    host.layoutSubtreeIfNeeded()
    textView.frame.size.height = max(textView.frame.height, 5_000)
    let previousMenu = button.menu

    var callbackCount = 0
    var textEditCallbacks = [LocalTextEditCallback]()
    renderer.onInteraction = { interaction in
        callbackCount += 1
        if case .textEdit(let nodeID, let text, let editSeq, _) = interaction {
            textEditCallbacks.append(
                LocalTextEditCallback(
                    nodeID: nodeID,
                    text: text,
                    editSeq: editSeq,
                    observedRevision: controller.applier.lastAppliedRevision.value
                )
            )
        }
        productionHandler(interaction)
    }
    defer {
        renderer.onInteraction = productionHandler
        button.menu = previousMenu
    }

    var samples = [String: [Double]]()
    var checks = true
    var everyInjectedResponseUnfinishedThroughVisibleCompletion = true
    var heldResponseProbeCount = 0
    var menuOpened = 0
    var hoverVisualChanges = 0

    func record(
        _ id: String,
        targetView: NSView,
        action: @escaping () throws -> Bool,
        cleanup: () -> Bool = { true }
    ) async throws {
        benchmarkTrace("31.4 rtt=\(rttMilliseconds) \(id) start")
        var sample = 0.0
        var samplePassed = false
        let responseUnfinishedThroughVisibleCompletion =
            try await withHeldInjectedResponse(
                rttMilliseconds: rttMilliseconds,
                controller: controller,
                transport: transport
            ) {
                do {
                    let stateCorrect: Bool
                    let pixels: Bool
                    let latencyMilliseconds: Double
                    if fullPaint {
                        var actionStateCorrect = false
                        let measured =
                            try await benchmarkMeasurePassiveCompositedChange(
                                window,
                                targetView: targetView
                            ) {
                                benchmarkTrace(
                                    "31.4 rtt=\(rttMilliseconds) \(id) "
                                        + "action start"
                                )
                                actionStateCorrect = try action()
                                benchmarkTrace(
                                    "31.4 rtt=\(rttMilliseconds) \(id) "
                                        + "action end"
                                )
                            }
                        stateCorrect = actionStateCorrect
                        pixels = measured.observation.crossedDisplayRefresh
                            && measured.observation.captureAuthorization
                            && measured.observation.pixelCaptureVerified
                        latencyMilliseconds =
                            measured.presentationLatencyMilliseconds
                    } else {
                        let start = clock.now
                        benchmarkTrace(
                            "31.4 rtt=\(rttMilliseconds) \(id) action start"
                        )
                        stateCorrect = try action()
                        benchmarkTrace(
                            "31.4 rtt=\(rttMilliseconds) \(id) action end"
                        )
                        pixels = rasterize(host)
                        latencyMilliseconds = milliseconds(
                            start.duration(to: clock.now)
                        )
                    }
                    benchmarkTrace(
                        "31.4 rtt=\(rttMilliseconds) \(id) paint end"
                    )
                    let cleanupCorrect = cleanup()
                    sample = latencyMilliseconds
                    samplePassed =
                        stateCorrect && pixels && cleanupCorrect
                } catch {
                    _ = cleanup()
                    throw error
                }
            }
        samples[id, default: []].append(sample)
        checks = checks && samplePassed
        everyInjectedResponseUnfinishedThroughVisibleCompletion =
            everyInjectedResponseUnfinishedThroughVisibleCompletion
                && responseUnfinishedThroughVisibleCompletion
        heldResponseProbeCount += 1
        benchmarkTrace("31.4 rtt=\(rttMilliseconds) \(id) end")
    }

    for _ in 0..<iterations {
        try await record(
            "text_entry",
            targetView: textScroll.contentView
        ) {
            let before = (textView.string as NSString).length
            textView.insertText(
                "x",
                replacementRange: NSRange(location: before, length: 0)
            )
            return (textView.string as NSString).length == before + 1
        }
    }
    for index in 0..<iterations {
        let textLength = (textView.string as NSString).length
        guard textLength > 0 else {
            throw BenchmarkFailure.message(
                "caret benchmark requires nonempty production editor text"
            )
        }
        let baselineLocation = index % textLength
        let location = (baselineLocation + 1) % (textLength + 1)
        textView.setSelectedRange(
            NSRange(location: baselineLocation, length: 1)
        )
        try await record(
            "caret_movement",
            targetView: textScroll.contentView
        ) {
            textView.setSelectedRange(
                NSRange(location: location, length: 0)
            )
            return textView.selectedRange().location == location
                && textView.selectedRange().length == 0
        }
    }
    for index in 0..<iterations {
        try await record(
            "text_selection",
            targetView: textScroll.contentView
        ) {
            let length = min(2, (textView.string as NSString).length)
            let availableStarts = max(
                1,
                min(4, (textView.string as NSString).length - length + 1)
            )
            let location = (index + 1) % availableStarts
            textView.setSelectedRange(
                NSRange(location: location, length: length)
            )
            return textView.selectedRange()
                == NSRange(location: location, length: length)
        }
    }
    for _ in 0..<iterations {
        try await record(
            "ime_composition",
            targetView: textScroll.contentView,
            action: {
                textView.setMarkedText(
                    "é",
                    selectedRange: NSRange(location: 1, length: 0),
                    replacementRange: NSRange(location: NSNotFound, length: 0)
                )
                return textView.hasMarkedText()
            },
            cleanup: {
                textView.unmarkText()
                return textView.hasMarkedText() == false
            }
        )
    }
    for index in 0..<iterations {
        try await record(
            "scrolling",
            targetView: textScroll.contentView
        ) {
            let target = NSPoint(
                x: 0,
                y: min(4_000, Double((index + 1) * 23))
            )
            textScroll.contentView.scroll(to: target)
            textScroll.reflectScrolledClipView(textScroll.contentView)
            return abs(textScroll.contentView.bounds.origin.y - target.y) < 1
        }
    }
    for index in 0..<iterations {
        let center = button.convert(
            NSPoint(x: button.bounds.midX, y: button.bounds.midY),
            to: nil
        )
        let hover = try benchmarkTrackingEvent(
            .mouseEntered,
            window: window,
            location: center,
            eventNumber: index * 4
        )
        let exit = try benchmarkTrackingEvent(
            .mouseExited,
            window: window,
            location: center,
            eventNumber: index * 4 + 1
        )
        let down = try benchmarkMouseEvent(
            .leftMouseDown,
            window: window,
            location: center,
            eventNumber: index * 4 + 2
        )
        let up = try benchmarkMouseEvent(
            .leftMouseUp,
            window: window,
            location: center,
            eventNumber: index * 4 + 3
        )
        try await record(
            "hover",
            targetView: button,
            action: {
                button.mouseEntered(with: hover)
                guard button.needsDisplay else {
                    return false
                }
                hoverVisualChanges += 1
                return true
            },
            cleanup: {
                button.mouseExited(with: exit)
                let exitInvalidated = button.needsDisplay
                _ = rasterize(button)
                return exitInvalidated
            }
        )

        let callbacksBefore = callbackCount
        try await record(
            "pressed",
            targetView: button,
            action: {
                window.sendEvent(down)
                return button.isHighlighted
            },
            cleanup: {
                window.sendEvent(up)
                return button.isHighlighted == false
                    && callbackCount == callbacksBefore + 1
            }
        )
    }
    for index in 0..<iterations {
        let menu = NSMenu(title: "Benchmark local menu")
        for title in ["One", "Two", "Three"] {
            menu.addItem(
                withTitle: title,
                action: nil,
                keyEquivalent: ""
            )
        }
        let menuProbe = SmokeMenuTrackingProbe()
        button.menu = menu

        benchmarkTrace(
            "31.4 rtt=\(rttMilliseconds) menu_opening start"
        )
        var sample = 0.0
        var samplePassed = false
        let responseUnfinishedThroughVisibleCompletion =
            try await withHeldInjectedResponse(
                rttMilliseconds: rttMilliseconds,
                controller: controller,
                transport: transport
            ) {
                if fullPaint {
                    let measured =
                        try await benchmarkMeasureOwnedMenuPresentation(
                            menu,
                            positioningItem: menu.items.first,
                            at: NSPoint(
                                x: button.bounds.minX,
                                y: button.bounds.maxY
                            ),
                            in: button
                        )
                    menuOpened += 1
                    sample = measured.presentationLatencyMilliseconds
                    samplePassed = menuOpened == index + 1
                        && measured.menuWindowID != 0
                        && measured.observation.crossedDisplayRefresh
                        && measured.observation.captureAuthorization
                        && measured.observation.pixelCaptureVerified
                } else {
                    menu.delegate = menuProbe
                    let start = clock.now
                    let opensBefore = menuProbe.openCount
                    let presentationsBefore = menuProbe.presentationCount
                    menuProbe.prepare(host: host)
                    let observeSelector = #selector(
                        SmokeMenuTrackingProbe
                            .observePresentationAndCancel(_:)
                    )
                    RunLoop.main.perform(
                        observeSelector,
                        target: menuProbe,
                        argument: menu,
                        order: 0,
                        modes: [.eventTracking]
                    )
                    _ = menu.popUp(
                        positioning: menu.items.first,
                        at: NSPoint(
                            x: button.bounds.minX,
                            y: button.bounds.maxY
                        ),
                        in: button
                    )
                    menuOpened += 1
                    if let observedAt = menuProbe.observedAt {
                        sample = milliseconds(
                            start.duration(to: observedAt)
                        )
                    }
                    samplePassed = menuOpened == index + 1
                        && menuProbe.openCount == opensBefore + 1
                        && menuProbe.presentationCount
                            == presentationsBefore + 1
                        && menuProbe.observedAt != nil
                }
            }
        samples["menu_opening", default: []].append(sample)
        checks = checks && samplePassed
        everyInjectedResponseUnfinishedThroughVisibleCompletion =
            everyInjectedResponseUnfinishedThroughVisibleCompletion
                && responseUnfinishedThroughVisibleCompletion
        heldResponseProbeCount += 1
        button.menu = previousMenu
        menu.delegate = nil
        benchmarkTrace(
            "31.4 rtt=\(rttMilliseconds) menu_opening end"
        )
    }

    // Flush outside every timed local-paint interval so the benchmark can separately prove that
    // native editor state crossed the production semantic-event and framed-transport boundary.
    renderer.textEditingSession.flushAllPending()
    try await waitUntil(timeout: .seconds(10)) {
        await transport.snapshot().activeDelayedOperations == 0
    }
    return LocalInteractionResult(
        samples: samples,
        stateChecksPassed: checks,
        everyInjectedResponseUnfinishedThroughVisibleCompletion:
            everyInjectedResponseUnfinishedThroughVisibleCompletion,
        heldResponseProbeCount: heldResponseProbeCount,
        productionCallbacks: callbackCount,
        textEditCallbacks: textEditCallbacks,
        finalText: textView.string,
        hoverMode: fullPaint
            ? "renderer-produced NSButton changed its exact composited target "
                + "ROI on the local AppKit hover path and restored on exit in "
                + "\(hoverVisualChanges)/\(iterations) samples"
            : "renderer-produced NSButton completed the smoke offscreen hover "
                + "path in \(hoverVisualChanges)/\(iterations) samples",
        menuMode: fullPaint
            ? "renderer-produced NSButton context menu was proven as a new "
                + "exact owned menu-level WindowServer surface in the same "
                + "ScreenCaptureKit frame used for its presentation timestamp"
            : "renderer-produced NSButton context NSMenu opened and was "
                + "deterministically cancelled through AppKit event tracking "
                + "in smoke mode"
    )
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

@MainActor
func networkAndLocalInteraction(
    fixtureOperations: [SemanticModel.Operation],
    iterations: Int,
    fullPaint: Bool
) async throws -> Section {
    let frameBudget = benchmarkLocalFrameBudget(fullPaint: fullPaint)
    var metrics = [
        metric(
            "local display frame budget",
            frameBudget.milliseconds,
            "ms",
            "exact",
            id: "display.frame_budget"
        )
    ]
    var localByRTT = [Int: [String: [Double]]]()
    var dependentByRTT = [Int: [Double]]()
    var allLocalStateChecks = true
    var allInjectedResponsesUnfinishedThroughVisibleCompletion = true
    var allProductionTextEditsExact = true
    var productionCallbackCount = 0
    var productionTextEditCallbackCount = 0
    var totalHeldResponseProbeCount = 0
    var textEditProofDetails = [String]()
    var hoverModes = Set<String>()
    var menuModes = Set<String>()
    var measuredWireBytes = 0
    var measuredWireMessages = 0
    for rtt in [0, 100, 300, 600] {
        benchmarkTrace("31.4 rtt=\(rtt) begin")
        let transport = BenchmarkTransport(rttMilliseconds: rtt)
        let renderer = AppKitRenderer()
        let sessionID = "network-\(rtt)"
        let controller = try await startActiveSession(
            transport: transport,
            renderer: renderer,
            fixtureOperations: fixtureOperations,
            sessionID: sessionID
        )

        let local = try await localInteractionSamples(
            renderer: renderer,
            controller: controller,
            transport: transport,
            sessionID: sessionID,
            rttMilliseconds: rtt,
            iterations: iterations,
            fullPaint: fullPaint
        )
        benchmarkPhase("31.4 rtt=\(rtt) local interactions finished")
        localByRTT[rtt] = local.samples
        allLocalStateChecks = allLocalStateChecks && local.stateChecksPassed
        allInjectedResponsesUnfinishedThroughVisibleCompletion =
            allInjectedResponsesUnfinishedThroughVisibleCompletion
                && local
                    .everyInjectedResponseUnfinishedThroughVisibleCompletion
        totalHeldResponseProbeCount += local.heldResponseProbeCount
        productionCallbackCount += local.productionCallbacks
        productionTextEditCallbackCount += local.textEditCallbacks.count
        hoverModes.insert(local.hoverMode)
        menuModes.insert(local.menuMode)

        renderer.textEditingSession.flushAllPending()
        let expectedTextCallback = local.textEditCallbacks
            .filter { $0.nodeID == NodeId(14) && $0.text == local.finalText }
            .max { $0.editSeq < $1.editSeq }
        func isExactFinalTextEvent(_ captured: CapturedEvent) -> Bool {
            guard let expectedTextCallback else { return false }
            return captured.eventType == .EVENT_TEXT_EDIT
                && captured.nodeID == NodeId(14)
                && captured.arguments[.TEXT] == .string(local.finalText)
                && captured.editSeq == expectedTextCallback.editSeq
                && (captured.editSeq?.rawValue ?? 0) > 0
                && captured.observedRevision == expectedTextCallback.observedRevision
        }

        // A production editor lane permits only one unacknowledged TEXT_EDIT. Drain each exact
        // sequence slot outside the timed paint interval so later coalesced edits can enter the
        // outbox; require a stable empty tail before stopping the session.
        var acknowledgedSequences = Set<UInt64>()
        var exactTextSlotAcknowledged = false
        var stableEmptyPasses = 0
        let drainDeadline = clock.now + .seconds(20)
        while clock.now < drainDeadline {
            let interactionEvents = try await capturedEvents(in: transport)
            let unacknowledged = interactionEvents
                .filter { acknowledgedSequences.contains($0.sequence) == false }
                .sorted { $0.sequence < $1.sequence }
            for event in unacknowledged {
                try await transport.injectFromServer(
                    try framed(
                        acknowledgementForCapturedEvent(
                            event,
                            outbox: controller.outbox,
                            sessionID: sessionID,
                            revision: controller.applier.lastAppliedRevision
                        )
                    )
                )
                acknowledgedSequences.insert(event.sequence)
                if isExactFinalTextEvent(event) {
                    exactTextSlotAcknowledged = true
                }
            }

            let pending = await controller.outbox.pendingCount
            let delayed = await transport.snapshot().activeDelayedOperations
            if pending == 0 && delayed == 0 && unacknowledged.isEmpty {
                stableEmptyPasses += 1
                if stableEmptyPasses >= 5 {
                    break
                }
            } else {
                stableEmptyPasses = 0
            }
            try await Task.sleep(for: .milliseconds(10))
        }
        let interactionEvents = try await capturedEvents(in: transport)
        let exactTextEvents = interactionEvents.filter(isExactFinalTextEvent)
        let finalPendingCount = await controller.outbox.pendingCount
        let textEditExact = expectedTextCallback != nil
            && exactTextEvents.count == 1
            && exactTextSlotAcknowledged
            && finalPendingCount == 0
            && stableEmptyPasses >= 5
        allProductionTextEditsExact = allProductionTextEditsExact && textEditExact
        textEditProofDetails.append(
            "RTT \(rtt)ms: node=14 final_text=\(String(reflecting: local.finalText)) "
                + "edit_seq=\(expectedTextCallback?.editSeq.rawValue ?? 0) "
                + "observed_revision=\(expectedTextCallback?.observedRevision ?? 0) "
                + "matching_framed_events=\(exactTextEvents.count) "
                + "exact_slot_ack=\(exactTextSlotAcknowledged) "
                + "stable_empty_tail=\(stableEmptyPasses >= 5)"
        )
        benchmarkTrace(
            "31.4 rtt=\(rtt) drained \(interactionEvents.count) renderer events"
        )
        benchmarkPhase("31.4 rtt=\(rtt) renderer events acknowledged")

        for (kind, values) in local.samples.sorted(by: { $0.key < $1.key }) {
            let id = "interaction.\(kind).rtt.\(rtt)"
            let displayName = kind.replacingOccurrences(of: "_", with: " ")
            metrics.append(
                metric(
                    "\(displayName) at \(rtt)ms RTT",
                    p50(values),
                    target: frameBudget.milliseconds,
                    id: id
                )
            )
            metrics.append(
                metric(
                    "\(displayName) at \(rtt)ms RTT",
                    percentile(values, 0.95),
                    "ms",
                    "p95",
                    id: id
                )
            )
            metrics.append(
                metric(
                    "\(displayName) at \(rtt)ms RTT",
                    percentile(values, 0.99),
                    "ms",
                    "p99",
                    id: id
                )
            )
        }

        let localTransportStats = await transport.snapshot()
        renderer.onInteraction = nil
        await controller.stop()
        closeRenderer(renderer)

        let feedbackTransport = BenchmarkTransport(rttMilliseconds: rtt)
        let feedbackRenderer = AppKitRenderer()
        let feedbackSessionID = "network-feedback-\(rtt)"
        let feedbackController = try await startActiveSession(
            transport: feedbackTransport,
            renderer: feedbackRenderer,
            fixtureOperations: fixtureOperations,
            sessionID: feedbackSessionID
        )
        let feedbackWarmVisible: Bool
        if fullPaint {
            guard let window = feedbackRenderer.registry.surfaceHandles.first?.window else {
                throw BenchmarkFailure.message("network feedback has no warm presentation window")
            }
            feedbackWarmVisible = try await benchmarkObserveOnScreenPaint(window)
                .crossedDisplayRefresh
        } else {
            feedbackWarmVisible = rasterizeRenderer(feedbackRenderer, showWindows: false)
        }
        guard feedbackWarmVisible else {
            throw BenchmarkFailure.message("network feedback renderer did not warm")
        }

        var dependent = [Double]()
        let responseIterations = max(3, min(7, iterations))
        for sample in 0..<responseIterations {
            benchmarkTrace("31.4 rtt=\(rtt) server feedback \(sample) start")
            let baseRevision =
                feedbackController.applier.lastAppliedRevision
            let expectedValue =
                Double(sample + 1) / Double(responseIterations)
            let nextRevision = Revision(baseRevision.value + 1)
            let eventsBefore = try await capturedEvents(
                in: feedbackTransport
            ).count
            let performFeedback:
                @MainActor () async throws -> SemanticModel.Event = {
                    let event =
                        try await feedbackController.sendValueChanged(
                            nodeId: NodeId(5),
                            value: .float64(expectedValue)
                        )
                    let eventsAfter =
                        try await capturedEvents(in: feedbackTransport)
                    guard eventsAfter.count == eventsBefore + 1,
                          let capturedEvent = eventsAfter.last,
                          capturedEvent.id == event.eventId.bytes,
                          capturedEvent.sequence == event.eventSeq,
                          capturedEvent.observedRevision
                            == baseRevision.value else {
                        throw BenchmarkFailure.message(
                            "server-feedback trial did not emit exactly one "
                                + "matching framed production event"
                        )
                    }
                    let response = Transaction(
                        baseRevision: baseRevision,
                        operations: [
                            .setProperty(
                                id: NodeId(5),
                                property: .value,
                                value: .float64(expectedValue)
                            )
                        ]
                    )
                    try await feedbackTransport.injectFromServer(
                        try framed(transactionMessage(response))
                    )
                    try await waitForRevision(
                        nextRevision,
                        controller: feedbackController
                    )
                    try await waitUntil {
                        guard let progress =
                            feedbackRenderer.registry.view(for: NodeId(5))
                                as? NSProgressIndicator else {
                            return false
                        }
                        return abs(
                            progress.doubleValue - expectedValue
                        ) < 0.000_001
                    }
                    return event
                }

            let trial: (
                event: SemanticModel.Event,
                latencyMilliseconds: Double,
                visible: Bool
            )
            if fullPaint {
                let windows =
                    feedbackRenderer.registry.surfaceHandles
                        .compactMap(\.window)
                guard windows.count == 1,
                      let window = windows.first,
                      let progress =
                        feedbackRenderer.registry.view(for: NodeId(5))
                            as? NSProgressIndicator,
                      progress.window === window else {
                    throw BenchmarkFailure.message(
                        "network feedback must expose one attached progress "
                            + "target in one presentation window"
                    )
                }
                var actionEvent: SemanticModel.Event?
                let measured =
                    try await benchmarkMeasurePassiveCompositedChange(
                        window,
                        targetView: progress
                    ) {
                        actionEvent = try await performFeedback()
                    }
                guard let actionEvent else {
                    throw BenchmarkFailure.message(
                        "network feedback action returned no production event"
                    )
                }
                trial = (
                    actionEvent,
                    measured.presentationLatencyMilliseconds,
                    measured.observation.crossedDisplayRefresh
                        && measured.observation.captureAuthorization
                        && measured.observation.pixelCaptureVerified
                )
            } else {
                let start = clock.now
                let event = try await performFeedback()
                trial = (
                    event,
                    milliseconds(start.duration(to: clock.now)),
                    rasterizeRenderer(
                        feedbackRenderer,
                        showWindows: false
                    )
                )
            }
            guard trial.visible else {
                throw BenchmarkFailure.message(
                    "network response did not reach the measured visible boundary"
                )
            }
            dependent.append(trial.latencyMilliseconds)
            try await feedbackTransport.injectFromServer(
                try framed(
                    acknowledgeMessage(
                        trial.event,
                        outbox: feedbackController.outbox,
                        sessionID: feedbackSessionID,
                        revision: nextRevision
                    )
                )
            )
            try await waitUntil(timeout: .seconds(20)) {
                await feedbackController.outbox.pendingCount == 0
            }
            benchmarkTrace(
                "31.4 rtt=\(rtt) server feedback \(sample) end"
            )
        }
        benchmarkPhase("31.4 rtt=\(rtt) server feedback finished")
        dependentByRTT[rtt] = dependent
        let feedbackID = "server_feedback.rtt.\(rtt)"
        metrics.append(metric("server-dependent input-to-visible at \(rtt)ms RTT", p50(dependent), id: feedbackID))
        metrics.append(metric("server-dependent input-to-visible at \(rtt)ms RTT", percentile(dependent, 0.95), "ms", "p95", id: feedbackID))
        metrics.append(metric("server-dependent input-to-visible at \(rtt)ms RTT", percentile(dependent, 0.99), "ms", "p99", id: feedbackID))

        let feedbackTransportStats = await feedbackTransport.snapshot()
        measuredWireBytes += localTransportStats.outboundBytes
            + localTransportStats.inboundBytes
            + feedbackTransportStats.outboundBytes
            + feedbackTransportStats.inboundBytes
        measuredWireMessages += localTransportStats.outboundMessages
            + localTransportStats.inboundMessages
            + feedbackTransportStats.outboundMessages
            + feedbackTransportStats.inboundMessages
        await feedbackController.stop()
        closeRenderer(feedbackRenderer)
        benchmarkTrace("31.4 rtt=\(rtt) complete")
        benchmarkPhase("31.4 rtt=\(rtt) complete")
    }

    benchmarkTrace("31.4 impairments begin")
    benchmarkPhase("31.4 impairments begin")
    let bandwidthBytesPerSecond = 1_048_576
    let bandwidthTransport = BenchmarkTransport(
        bytesPerSecond: bandwidthBytesPerSecond
    )
    let bandwidthRenderer = AppKitRenderer()
    let bandwidthController = try await startActiveSession(
        transport: bandwidthTransport,
        renderer: bandwidthRenderer,
        fixtureOperations: fixtureOperations,
        sessionID: "bandwidth"
    )
    let bandwidthBefore = await bandwidthTransport.snapshot()
    let largeValue = SemanticModel.Value.string(String(repeating: "x", count: 16_384))
    var bandwidthSamples = [Double]()
    var bandwidthProofSamples = [BandwidthLimiterProofSample]()
    for _ in 0..<3 {
        let sampleBefore = await bandwidthTransport.snapshot()
        let eventsBefore = try await capturedEvents(in: bandwidthTransport)
        let start = clock.now
        let event = try await bandwidthController.sendValueChanged(
            nodeId: NodeId(16),
            value: largeValue
        )
        let measuredMilliseconds = milliseconds(start.duration(to: clock.now))
        let sampleAfter = await bandwidthTransport.snapshot()
        let eventsAfter = try await capturedEvents(in: bandwidthTransport)
        let framedBytes = sampleAfter.outboundBytes - sampleBefore.outboundBytes
        let outboundMessages = sampleAfter.outboundMessages
            - sampleBefore.outboundMessages
        let appendedEvents = eventsAfter.dropFirst(eventsBefore.count)
        let exactEventMatched = eventsAfter.count == eventsBefore.count + 1
            && appendedEvents.count == 1
            && appendedEvents.first?.id == event.eventId.bytes
            && appendedEvents.first?.sequence == event.eventSeq
        let theoreticalMinimumMilliseconds = Double(framedBytes)
            / Double(bandwidthBytesPerSecond) * 1_000.0
        bandwidthSamples.append(measuredMilliseconds)
        bandwidthProofSamples.append(
            BandwidthLimiterProofSample(
                framedBytes: framedBytes,
                outboundMessages: outboundMessages,
                measuredMilliseconds: measuredMilliseconds,
                theoreticalMinimumMilliseconds: theoreticalMinimumMilliseconds,
                exactEventMatched: exactEventMatched
            )
        )
        try await bandwidthTransport.injectFromServer(
            try framed(
                acknowledgeMessage(
                    event,
                    outbox: bandwidthController.outbox,
                    sessionID: "bandwidth",
                    revision: bandwidthController.applier.lastAppliedRevision
                )
            )
        )
        try await waitUntil(timeout: .seconds(5)) {
            await bandwidthController.outbox.pendingCount == 0
        }
    }
    let bandwidthAfter = await bandwidthTransport.snapshot()
    let bandwidthDeliveredBytes = bandwidthAfter.outboundBytes - bandwidthBefore.outboundBytes
    let bandwidthLimiterDelayProven = bandwidthProofSamples.count == 3
        && bandwidthProofSamples.allSatisfy(\.passed)
    let bandwidthProofDetail = bandwidthProofSamples.enumerated().map {
        "sample \($0.offset + 1): \($0.element.detail)"
    }.joined(separator: "; ")
    measuredWireBytes += bandwidthDeliveredBytes
    measuredWireMessages += bandwidthAfter.outboundMessages - bandwidthBefore.outboundMessages
    await bandwidthController.stop()
    closeRenderer(bandwidthRenderer)
    metrics.append(metric("1MiB/s bandwidth-limited production event", p50(bandwidthSamples), id: "impairment.bandwidth_transfer"))
    metrics.append(metric("1MiB/s bandwidth-limited production event", percentile(bandwidthSamples, 0.95), "ms", "p95", id: "impairment.bandwidth_transfer"))
    metrics.append(metric("bandwidth-limited delivered bytes", Double(bandwidthDeliveredBytes), "bytes", "exact", id: "impairment.bandwidth_delivered_bytes"))
    var completedLossTrials = 0
    let lossOutbox = EventOutbox()
    let lossTransport = BenchmarkTransport(dropOutboundOrdinals: [2])
    let lossRenderer = AppKitRenderer()
    let lossController = try await startActiveSession(
        transport: lossTransport,
        renderer: lossRenderer,
        fixtureOperations: fixtureOperations,
        sessionID: "loss",
        outbox: lossOutbox
    )
    let lossBefore = await lossTransport.snapshot()
    let lostEvent = try await lossController.sendActivate(nodeId: NodeId(16))
    let lossAfter = await lossTransport.snapshot()
    let retainedAfterLoss = await lossOutbox.pendingCount == 1
    await lossController.stop()
    closeRenderer(lossRenderer)

    let lossRecoveryTransport = BenchmarkTransport()
    let lossRecoveryController = SessionController(
        transport: lossRecoveryTransport,
        outbox: lossOutbox,
        sessionId: "loss"
    )
    try await lossRecoveryController.start()
    let lossRecoveryBefore = await lossRecoveryTransport.snapshot()
    try await lossRecoveryTransport.injectFromServer(
        try framed(resumeOKMessage(sessionID: "loss"))
    )
    try await waitUntil {
        (try? await capturedEvents(in: lossRecoveryTransport).count) == 1
    }
    let recoveredLossEvents = try await capturedEvents(in: lossRecoveryTransport)
    let replayedLostEvent = recoveredLossEvents.first?.id == lostEvent.eventId.bytes
    try await lossRecoveryTransport.injectFromServer(
        try framed(
            acknowledgementForCapturedEvent(
                recoveredLossEvents[0],
                outbox: lossOutbox,
                sessionID: "loss",
                revision: lossRecoveryController.applier.lastAppliedRevision
            )
        )
    )
    try await waitUntil {
        await lossOutbox.pendingCount == 0
    }
    let lossRecoveryAfter = await lossRecoveryTransport.snapshot()
    await lossRecoveryController.stop()
    completedLossTrials += 1
    let lossAttempts = (lossAfter.outboundAttempts - lossBefore.outboundAttempts)
        + (lossRecoveryAfter.outboundAttempts - lossRecoveryBefore.outboundAttempts)
    let lossDeliveredMessages = (lossAfter.outboundMessages - lossBefore.outboundMessages)
        + (lossRecoveryAfter.outboundMessages - lossRecoveryBefore.outboundMessages)
    metrics.append(metric("deterministic production loss attempts", Double(lossAttempts), "messages", "exact", id: "impairment.loss_attempts"))
    metrics.append(metric("deterministic production loss delivered messages", Double(lossDeliveredMessages), "messages", "exact", id: "impairment.loss_delivered_messages"))

    var completedInterruptionTrials = 0
    let interruptionOutbox = EventOutbox()
    let interruptionTransport = BenchmarkTransport(interruptOutboundOrdinals: [2])
    let interruptionRenderer = AppKitRenderer()
    let interruptionController = try await startActiveSession(
        transport: interruptionTransport,
        renderer: interruptionRenderer,
        fixtureOperations: fixtureOperations,
        sessionID: "interruption",
        outbox: interruptionOutbox
    )
    let interruptionFailures = TransportFailureObservation()
    interruptionController.onFailure = { failure in
        Task {
            await interruptionFailures.record(failure)
        }
    }
    let interruptionStart = clock.now
    var interruptionFailed = false
    do {
        _ = try await interruptionController.sendActivate(nodeId: NodeId(16))
    } catch TransportError.closed {
        interruptionFailed = true
    }
    try await waitUntil(timeout: .seconds(10)) {
        let observed = await interruptionFailures.snapshot()
        let stats = await interruptionTransport.snapshot()
        return interruptionController.isDiverged
            && observed.count == 1
            && observed.transportEnded
            && stats.isClosed
            && stats.closeCalls >= 1
    }
    let interruptionLatency = milliseconds(interruptionStart.duration(to: clock.now))
    let interruptionPending = await interruptionOutbox.pendingCount == 1
    let interruptionFailureSnapshot = await interruptionFailures.snapshot()
    let interruptionStats = await interruptionTransport.snapshot()
    let interruptionLifecycleObserved = interruptionController.isDiverged
        && interruptionFailureSnapshot.count == 1
        && interruptionFailureSnapshot.transportEnded
        && interruptionStats.isClosed
        && interruptionStats.closeCalls >= 1
    await interruptionController.stop()
    closeRenderer(interruptionRenderer)

    let interruptionRecoveryTransport = BenchmarkTransport()
    let interruptionRecoveryController = SessionController(
        transport: interruptionRecoveryTransport,
        outbox: interruptionOutbox,
        sessionId: "interruption"
    )
    try await interruptionRecoveryController.start()
    let interruptionRecoveryBefore = await interruptionRecoveryTransport.snapshot()
    try await interruptionRecoveryTransport.injectFromServer(
        try framed(resumeOKMessage(sessionID: "interruption"))
    )
    try await waitUntil {
        (try? await capturedEvents(in: interruptionRecoveryTransport).count) == 1
    }
    let recoveredInterruptionEvents = try await capturedEvents(
        in: interruptionRecoveryTransport
    )
    try await interruptionRecoveryTransport.injectFromServer(
        try framed(
            acknowledgementForCapturedEvent(
                recoveredInterruptionEvents[0],
                outbox: interruptionOutbox,
                sessionID: "interruption",
                revision: interruptionRecoveryController.applier.lastAppliedRevision
            )
        )
    )
    try await waitUntil {
        await interruptionOutbox.pendingCount == 0
    }
    let interruptionRecoveryAfter = await interruptionRecoveryTransport.snapshot()
    await interruptionRecoveryController.stop()
    completedInterruptionTrials += 1
    metrics.append(metric("controlled production interruption detection", interruptionLatency, id: "impairment.interruption_detection"))

    measuredWireBytes += (lossAfter.outboundBytes - lossBefore.outboundBytes)
        + (lossRecoveryAfter.outboundBytes - lossRecoveryBefore.outboundBytes)
        + (lossRecoveryAfter.inboundBytes - lossRecoveryBefore.inboundBytes)
        + interruptionStats.outboundBytes + interruptionStats.inboundBytes
        + (interruptionRecoveryAfter.outboundBytes - interruptionRecoveryBefore.outboundBytes)
        + (interruptionRecoveryAfter.inboundBytes - interruptionRecoveryBefore.inboundBytes)
    measuredWireMessages += (lossAfter.outboundMessages - lossBefore.outboundMessages)
        + (lossRecoveryAfter.outboundMessages - lossRecoveryBefore.outboundMessages)
        + (lossRecoveryAfter.inboundMessages - lossRecoveryBefore.inboundMessages)
        + interruptionStats.outboundMessages + interruptionStats.inboundMessages
        + (interruptionRecoveryAfter.outboundMessages - interruptionRecoveryBefore.outboundMessages)
        + (interruptionRecoveryAfter.inboundMessages - interruptionRecoveryBefore.inboundMessages)
    metrics.append(metric("measured production session wire bytes", Double(measuredWireBytes), "bytes", "exact", id: "session_wire.bytes"))
    metrics.append(metric("measured production session wire messages", Double(measuredWireMessages), "messages", "exact", id: "session_wire.messages"))

    let baseline = localByRTT[0] ?? [:]
    var p50Added = [Double]()
    var p95Added = [Double]()
    var p99Added = [Double]()
    for (rtt, samples) in localByRTT where rtt != 0 {
        for (kind, values) in samples {
            guard let base = baseline[kind] else { continue }
            p50Added.append(p50(values) - p50(base))
            p95Added.append(percentile(values, 0.95) - percentile(base, 0.95))
            p99Added.append(percentile(values, 0.99) - percentile(base, 0.99))
        }
    }
    let worstP50Added = max(0, p50Added.max() ?? .infinity)
    let worstP95Added = max(0, p95Added.max() ?? .infinity)
    let worstP99Added = max(0, p99Added.max() ?? .infinity)
    metrics.append(metric("maximum RTT-induced local latency delta", worstP50Added, "ms", "p50", target: frameBudget.milliseconds, id: "local_rtt_delta"))
    metrics.append(metric("maximum RTT-induced local latency delta", worstP95Added, "ms", "p95", id: "local_rtt_delta"))
    metrics.append(metric("maximum RTT-induced local latency delta", worstP99Added, "ms", "p99", id: "local_rtt_delta"))

    let serverTracksRTT = [100, 300, 600].allSatisfy {
        guard let samples = dependentByRTT[$0] else { return false }
        return p50(samples) >= Double($0) * 0.80
    }
    let lossPendingCleared = await lossOutbox.pendingCount == 0
    let interruptionPendingCleared = await interruptionOutbox.pendingCount == 0
    let impairmentsApplied = bandwidthLimiterDelayProven
        && bandwidthDeliveredBytes > 16_384 * 3
        && lossAttempts == 2
        && lossDeliveredMessages == 1
        && lossAfter.droppedMessages == 1
        && retainedAfterLoss
        && replayedLostEvent
        && interruptionFailed
        && interruptionPending
        && interruptionStats.interruptions == 1
        && interruptionLifecycleObserved
        && recoveredInterruptionEvents.count == 1
        && lossPendingCleared
        && interruptionPendingCleared
    benchmarkTrace("31.4 complete")
    benchmarkPhase("31.4 complete")
    var sampleCounts = [String: Int]()
    for (rtt, interactions) in localByRTT {
        for (interaction, values) in interactions {
            sampleCounts["macos.interaction.\(interaction).rtt.\(rtt)"] =
                values.count
        }
    }
    for (rtt, values) in dependentByRTT {
        sampleCounts["macos.server_feedback.rtt.\(rtt)"] = values.count
    }
    sampleCounts["macos.local_held_response"] =
        totalHeldResponseProbeCount
    sampleCounts["macos.bandwidth"] = bandwidthSamples.count
    sampleCounts["macos.loss"] = completedLossTrials
    sampleCounts["macos.interruption"] = completedInterruptionTrials
    return Section(
        id: "31.4",
        name: "Network and local interaction",
        sampleCounts: sampleCounts,
        metrics: metrics,
        assertions: [
            Assertion(
                id: "local_latency_independent",
                name: "mounted local interactions do not acquire one RTT",
                passed: worstP50Added <= frameBudget.milliseconds
                    && allLocalStateChecks
                    && allInjectedResponsesUnfinishedThroughVisibleCompletion
                    && totalHeldResponseProbeCount == iterations * 8 * 4
                    && productionCallbackCount >= iterations * 4,
                detail: "largest p50 increase "
                    + "\(String(format: "%.4f", worstP50Added)) ms versus "
                    + "the measured local frame budget of "
                    + "\(String(format: "%.4f", frameBudget.milliseconds)) "
                    + "ms; paired injected transaction remained blocked "
                    + "through local visible completion in "
                    + "\(totalHeldResponseProbeCount)/\(iterations * 8 * 4) "
                    + "probes="
                    + "\(allInjectedResponsesUnfinishedThroughVisibleCompletion); "
                    + "descriptive p95/p99 deltas were "
                    + "\(String(format: "%.4f", worstP95Added))/"
                    + "\(String(format: "%.4f", worstP99Added)) ms; "
                    + "production renderer callbacks="
                    + "\(productionCallbackCount)"
            ),
            Assertion(
                id: "production_text_edit_framed",
                name: "native text entry emits and settles one exact production TEXT_EDIT",
                passed: allProductionTextEditsExact
                    && productionTextEditCallbackCount >= 4,
                detail: textEditProofDetails.joined(separator: "; ")
            ),
            Assertion(
                id: "server_latency_tracks_rtt",
                name: "injected transport RTT affects production server-dependent feedback",
                passed: serverTracksRTT,
                detail: "SessionController EventOutbox sends and framed transaction responses tracked 100/300/600ms RTT"
            ),
            Assertion(
                id: "no_sync_rtt",
                name: "local visible completion does not await an injected transport response",
                passed:
                    allInjectedResponsesUnfinishedThroughVisibleCompletion
                    && totalHeldResponseProbeCount == iterations * 8 * 4
                    && serverTracksRTT,
                detail: "Each local action reached its visible boundary while "
                    + "its paired injected production transaction remained "
                    + "blocked before delivery; the gate was released only "
                    + "afterward. Separately, production server-dependent "
                    + "feedback tracked 100/300/600ms RTT."
            ),
            Assertion(
                id: "impairments_use_session",
                name: "bandwidth delay, loss, and interruption exercise session recovery",
                passed: impairmentsApplied,
                detail: bandwidthProofDetail
                    + "; \(bandwidthDeliveredBytes) total bandwidth bytes; "
                    + "lost and interrupted events remained in EventOutbox and replayed "
                    + "through replacement SessionControllers"
            ),
        ],
        notes: [
            "Controls are mounted renderer TextArea, ScrollView, and Button. \(menuModes.sorted().joined(separator: "; ")).",
            "Pressed state is observed between real NSWindow-dispatched mouseDown/mouseUp events and triggers the production ActionTrampoline. \(hoverModes.sorted().joined(separator: "; ")). Permission-free hover invokes the renderer-produced NSButton's own AppKit entry/exit path and does not claim WindowServer pointer latency.",
            "Local frame budget \(String(format: "%.6f", frameBudget.milliseconds)) ms came from \(frameBudget.source).",
            "All impairment traffic traverses SessionController, EventOutbox, SRUIFraming, and replacement-session resume/replay; no benchmark calls Transport.send directly.",
            "For each 1 MiB/s sample, the proof takes the exact outbound framed-byte delta around one awaited SessionController.sendValueChanged call, requires exactly one matching event frame, and requires elapsed wall time >= framed bytes / 1,048,576 bytes/s. Encoding and outbox overhead are inside the measured interval and can only increase that elapsed time.",
            "The RTT-independence correctness gate requires the worst p50 "
                + "delta to stay within the measured local display-frame "
                + "budget and also requires every one of the "
                + "\(totalHeldResponseProbeCount) exact held-response probes. "
                + "That probe proves non-dependence on delivery of its exact "
                + "paired transaction; it does not claim the configured "
                + "one-way-delay interval remained active throughout the "
                + "action. Unpaired p95/p99 WindowServer tails remain "
                + "descriptive diagnostics and §23 follow-ups.",
        ]
    )
}
