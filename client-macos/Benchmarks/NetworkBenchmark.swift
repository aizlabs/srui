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
func networkAndLocalInteraction(
    fixtureOperations: [SemanticModel.Operation],
    fixtureIndex: BenchmarkFixtureIndex,
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
    var localStateCheckFailures = [String]()
    var allInjectedResponsesUnfinishedThroughVisibleCompletion = true
    var allConfiguredDelayStatesVerifiedAtActionStart = true
    var totalNonzeroDelayActiveAtActionStartProbeCount = 0
    var allProductionTextEditsExact = true
    var productionCallbackCount = 0
    var productionTextEditCallbackCount = 0
    var totalHeldResponseProbeCount = 0
    var textEditProofDetails = [String]()
    var hoverModes = Set<String>()
    var menuModes = Set<String>()
    var measuredWireBytes = 0
    var measuredWireMessages = 0
    for rtt in NetworkBenchmarkConfiguration.roundTripTimesMilliseconds {
        benchmarkTrace("31.4 rtt=\(rtt) begin")
        let transport = BenchmarkTransport(rttMilliseconds: rtt)
        let renderer = AppKitRenderer()
        let sessionID = "network-\(rtt)"
        let controller = try await startActiveSession(
            transport: transport,
            renderer: renderer,
            fixtureOperations: fixtureOperations,
            fixtureIndex: fixtureIndex,
            sessionID: sessionID
        )

        let local = try await localInteractionSamples(
            renderer: renderer,
            fixtureIndex: fixtureIndex,
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
        localStateCheckFailures.append(
            contentsOf: local.stateCheckFailures.map { "RTT \(rtt)ms \($0)" }
        )
        allInjectedResponsesUnfinishedThroughVisibleCompletion =
            allInjectedResponsesUnfinishedThroughVisibleCompletion
                && local
                    .everyInjectedResponseUnfinishedThroughVisibleCompletion
        allConfiguredDelayStatesVerifiedAtActionStart =
            allConfiguredDelayStatesVerifiedAtActionStart
                && local.everyConfiguredDelayStateVerifiedAtActionStart
        totalNonzeroDelayActiveAtActionStartProbeCount +=
            local.nonzeroDelayActiveAtActionStartProbeCount
        totalHeldResponseProbeCount += local.heldResponseProbeCount
        productionCallbackCount += local.productionCallbacks
        productionTextEditCallbackCount += local.textEditCallbacks.count
        hoverModes.insert(local.hoverMode)
        menuModes.insert(local.menuMode)

        renderer.textEditingSession.flushAllPending()
        let expectedTextCallback = local.textEditCallbacks
            .filter {
                $0.nodeID == fixtureIndex.textEditor
                    && $0.text == local.finalText
            }
            .max { $0.editSeq < $1.editSeq }
        func isExactFinalTextEvent(_ captured: CapturedEvent) -> Bool {
            guard let expectedTextCallback else { return false }
            return captured.eventType == .EVENT_TEXT_EDIT
                && captured.nodeID == fixtureIndex.textEditor
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
        // Each editor sequence slot is deliberately serialized until its
        // acknowledgement completes. Forty callbacks at 600 ms RTT can
        // legitimately need more than 20 seconds; this drain is untimed.
        let drainDeadline = clock.now + .seconds(60)
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
            "RTT \(rtt)ms: node=\(fixtureIndex.textEditor.value) "
                + "final_text=\(String(reflecting: local.finalText)) "
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
                    target: frameBudget.milliseconds,
                    id: id
                )
            )
            metrics.append(
                metric(
                    "\(displayName) at \(rtt)ms RTT",
                    percentile(values, 0.99),
                    "ms",
                    "p99",
                    target: frameBudget.milliseconds,
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
            fixtureIndex: fixtureIndex,
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
                            nodeId: fixtureIndex.progress,
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
                                id: fixtureIndex.progress,
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
                            feedbackRenderer.registry.view(for: fixtureIndex.progress)
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
                        feedbackRenderer.registry.view(for: fixtureIndex.progress)
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
        fixtureIndex: fixtureIndex,
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
            nodeId: fixtureIndex.primaryAction,
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
        fixtureIndex: fixtureIndex,
        sessionID: "loss",
        outbox: lossOutbox
    )
    let lossBefore = await lossTransport.snapshot()
    let lostEvent = try await lossController.sendActivate(
        nodeId: fixtureIndex.primaryAction
    )
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
        fixtureIndex: fixtureIndex,
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
        _ = try await interruptionController.sendActivate(
            nodeId: fixtureIndex.primaryAction
        )
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
            guard let base = baseline[kind] else {
                throw BenchmarkFailure.message(
                    "RTT \(rtt)ms interaction \(kind) has no 0ms baseline"
                )
            }
            guard values.count == base.count, values.isEmpty == false else {
                throw BenchmarkFailure.message(
                    "RTT \(rtt)ms interaction \(kind) has \(values.count) samples; "
                        + "0ms baseline has \(base.count)"
                )
            }
            let pairedDeltas = zip(values, base).map { sample, baselineSample in
                sample - baselineSample
            }
            p50Added.append(p50(pairedDeltas))
            p95Added.append(percentile(pairedDeltas, 0.95))
            p99Added.append(percentile(pairedDeltas, 0.99))
        }
    }
    let worstP50Added = max(0, p50Added.max() ?? .infinity)
    let worstP95Added = max(0, p95Added.max() ?? .infinity)
    let worstP99Added = max(0, p99Added.max() ?? .infinity)
    metrics.append(metric("maximum RTT-induced local latency delta", worstP50Added, "ms", "p50", target: frameBudget.milliseconds, id: "local_rtt_delta"))
    metrics.append(metric("maximum RTT-induced local latency delta", worstP95Added, "ms", "p95", target: frameBudget.milliseconds, id: "local_rtt_delta"))
    metrics.append(metric("maximum RTT-induced local latency delta", worstP99Added, "ms", "p99", target: frameBudget.milliseconds, id: "local_rtt_delta"))

    let serverTracksRTT =
        NetworkBenchmarkConfiguration.nonzeroRoundTripTimesMilliseconds
            .allSatisfy {
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

    let interactionKindCount = LocalInteractionKind.allCases.count
    let expectedHeldResponseProbeCount =
        iterations
            * interactionKindCount
            * NetworkBenchmarkConfiguration.roundTripTimesMilliseconds.count
    let expectedNonzeroDelayProbeCount =
        iterations
            * interactionKindCount
            * NetworkBenchmarkConfiguration
                .nonzeroRoundTripTimesMilliseconds.count
    let delayBoundaryProofPassed =
        allConfiguredDelayStatesVerifiedAtActionStart
            && totalNonzeroDelayActiveAtActionStartProbeCount
                == expectedNonzeroDelayProbeCount
            && totalHeldResponseProbeCount == expectedHeldResponseProbeCount
    let localLatencyIndependentPassed =
        (!fullPaint || worstP50Added <= frameBudget.milliseconds)
            && allLocalStateChecks
            && allInjectedResponsesUnfinishedThroughVisibleCompletion
            && delayBoundaryProofPassed
            && productionCallbackCount >= iterations * 4
    let localStateFailureDetail = localStateCheckFailures.isEmpty
        ? ""
        : " failures=["
            + localStateCheckFailures.joined(separator: "; ")
            + "]"
    var localLatencyDetail =
        "largest p50 increase "
            + "\(String(format: "%.4f", worstP50Added)) ms versus "
            + "the measured local frame budget of "
            + "\(String(format: "%.4f", frameBudget.milliseconds)) ms; "
            + (fullPaint
                ? "full compositor mode applies this numeric correctness gate; "
                : "smoke offscreen-raster mode reports this delta diagnostically and does not apply it as a correctness gate; ")
    localLatencyDetail +=
        "paired injected transaction remained blocked through local visible "
            + "completion in \(totalHeldResponseProbeCount)/"
            + "\(expectedHeldResponseProbeCount) probes="
            + "\(allInjectedResponsesUnfinishedThroughVisibleCompletion); "
    localLatencyDetail +=
        "configured delay state was verified at the exact action boundary in "
            + "\(totalHeldResponseProbeCount)/"
            + "\(expectedHeldResponseProbeCount) probes="
            + "\(allConfiguredDelayStatesVerifiedAtActionStart), with a "
            + "nonzero transport delay still active in "
            + "\(totalNonzeroDelayActiveAtActionStartProbeCount)/"
            + "\(expectedNonzeroDelayProbeCount) nonzero-RTT probes; "
    localLatencyDetail +=
        "local_state_checks=\(allLocalStateChecks)"
            + localStateFailureDetail
            + "; paired p95/p99 deltas were "
            + "\(String(format: "%.4f", worstP95Added))/"
            + "\(String(format: "%.4f", worstP99Added)) ms; "
            + "production renderer callbacks=\(productionCallbackCount)"

    let noSynchronousRTTDependencyPassed =
        allInjectedResponsesUnfinishedThroughVisibleCompletion
            && delayBoundaryProofPassed
            && serverTracksRTT
    var noSynchronousRTTDependencyDetail =
        "Each local action began only after its exact compositor baseline was "
            + "ready and while its configured BenchmarkTransport delay state "
            + "was verified; "
    noSynchronousRTTDependencyDetail +=
        "\(totalNonzeroDelayActiveAtActionStartProbeCount)/"
            + "\(expectedNonzeroDelayProbeCount) nonzero-RTT actions began "
            + "during an active delay. Every paired production transaction "
            + "remained blocked through visible completion; the gate was "
            + "released only afterward. Separately, production server-dependent "
            + "feedback tracked 100/300/600ms RTT."

    return Section(
        id: "31.4",
        name: "Network and local interaction",
        sampleCounts: sampleCounts,
        metrics: metrics,
        assertions: [
            Assertion(
                id: "local_latency_independent",
                name: "mounted local interactions do not acquire one RTT",
                passed: localLatencyIndependentPassed,
                detail: localLatencyDetail
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
                passed: noSynchronousRTTDependencyPassed,
                detail: noSynchronousRTTDependencyDetail
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
            "Pressed feedback uses performClick on the mounted renderer button, accepts its transient action-time composited frame, and triggers the production ActionTrampoline. \(hoverModes.sorted().joined(separator: "; ")). Hover injects a deterministic pointer context through benchmark SPI into the production HoverFeedbackButton reconciliation path. These two trials measure SRUI local state-to-visible latency and explicitly exclude OS hardware-event routing latency.",
            "Local frame budget \(String(format: "%.6f", frameBudget.milliseconds)) ms came from \(frameBudget.source).",
            "All impairment traffic traverses SessionController, EventOutbox, SRUIFraming, and replacement-session resume/replay; no benchmark calls Transport.send directly.",
            "For each 1 MiB/s sample, the proof takes the exact outbound framed-byte delta around one awaited SessionController.sendValueChanged call, requires exactly one matching event frame, and requires elapsed wall time >= framed bytes / 1,048,576 bytes/s. Encoding and outbox overhead are inside the measured interval and can only increase that elapsed time.",
            "In full compositor mode, the RTT-independence correctness gate "
                + "requires the worst p50 delta to stay within the measured "
                + "local display-frame budget. Smoke mode reports the offscreen "
                + "raster delta diagnostically because unrelated remote "
                + "invalidation can be charged to a later whole-host raster. "
                + "Both profiles require every one of the "
                + "\(totalHeldResponseProbeCount) exact held-response probes. "
                + "That probe proves non-dependence on delivery of its exact "
                + "paired transaction; it does not claim the configured "
                + "one-way-delay interval remained active throughout the "
                + "action. Paired p95/p99 delta tails carry the §23 target for "
                + "follow-up reporting but are not assertion gates.",
        ]
    )
}
