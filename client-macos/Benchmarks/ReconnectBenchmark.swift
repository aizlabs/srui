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

let fixturePNG = Data([
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
    0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
    0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x60,
    0x60, 0x60, 0x00, 0x00, 0x00, 0x04, 0x00, 0x01, 0xF6, 0x17, 0x38, 0x55, 0x00, 0x00, 0x00,
    0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
])

func resourceMetadataMessage(hash: Data) -> SRUIMessage {
    var metadata = SRUIResourceMetadata()
    metadata.resourceHash = hash
    metadata.mediaType = "image/png"
    metadata.encodedLength = UInt64(fixturePNG.count)
    metadata.decodedWidth = 1
    metadata.decodedHeight = 1
    metadata.priority = .normal
    var message = SRUIMessage()
    message.resourceMetadata = metadata
    return message
}

func resourceChunkMessage(hash: Data, offset: Int, data: Data) -> SRUIMessage {
    var chunk = SRUIResourceChunk()
    chunk.resourceHash = hash
    chunk.byteOffset = UInt64(offset)
    chunk.data = data
    var message = SRUIMessage()
    message.resourceChunk = chunk
    return message
}

actor ResumeFailureObservation {
    private var failures = [SessionFailure]()

    func record(_ failure: SessionFailure) {
        failures.append(failure)
    }

    func snapshot() -> (count: Int, containsSuperseded: Bool, descriptions: [String]) {
        (
            failures.count,
            failures.contains {
                if case .superseded = $0 { return true }
                return false
            },
            failures.map(\.description)
        )
    }
}

@MainActor
func preReceiptEventReplaySample(
    fixtureOperations: [SemanticModel.Operation],
    fixtureIndex: BenchmarkFixtureIndex
) async throws -> (latency: Double, passed: Bool, detail: String) {
    let outbox = EventOutbox()
    let sessionID = "pre-receipt-session"
    let firstTransport = BenchmarkTransport(dropOutboundOrdinals: [2])
    let firstRenderer = AppKitRenderer()
    let first = try await startActiveSession(
        transport: firstTransport,
        renderer: firstRenderer,
        fixtureOperations: fixtureOperations,
        fixtureIndex: fixtureIndex,
        sessionID: sessionID,
        outbox: outbox
    )

    let event = try await first.sendActivate(
        nodeId: fixtureIndex.primaryAction
    )
    let firstEvents = try await capturedEvents(in: firstTransport)
    let firstSnapshot = await firstTransport.snapshot()
    let retainedBeforeDisconnect = await outbox.pendingCount == 1

    let started = clock.now
    await first.stop()
    closeRenderer(firstRenderer)

    let replacementTransport = BenchmarkTransport()
    let replacement = SessionController(
        transport: replacementTransport,
        outbox: outbox,
        sessionId: sessionID
    )
    try await replacement.start()
    try await replacementTransport.injectFromServer(
        try framed(resumeOKMessage(sessionID: sessionID))
    )
    try await waitUntil(timeout: .seconds(10)) {
        (try? await capturedEvents(in: replacementTransport).count) == 1
    }
    let replayedEvents = try await capturedEvents(in: replacementTransport)
    let replayed = replayedEvents.count == 1
        && replayedEvents[0].id == event.eventId.bytes
        && replayedEvents[0].sequence == event.eventSeq
        && replayedEvents[0].observedRevision == event.observedRevision.value
    let retainedUntilAcknowledged = await outbox.pendingCount == 1

    if let replayedEvent = replayedEvents.first {
        try await replacementTransport.injectFromServer(
            try framed(
                acknowledgementForCapturedEvent(
                    replayedEvent,
                    outbox: outbox,
                    sessionID: sessionID,
                    revision: replacement.applier.lastAppliedRevision
                )
            )
        )
    }
    try await waitUntil(timeout: .seconds(10)) {
        await outbox.pendingCount == 0
    }
    let pendingCleared = await outbox.pendingCount == 0
    let latency = milliseconds(started.duration(to: clock.now))
    await replacement.stop()

    let firstDeliveryWasZero = firstEvents.isEmpty
        && firstSnapshot.outboundAttempts == 2
        && firstSnapshot.outboundMessages == 1
        && firstSnapshot.droppedMessages == 1
    let checks = [
        "event_allocated_before_disconnect": event.eventSeq == 1,
        "first_transport_delivered_zero_event_bytes": firstDeliveryWasZero,
        "outbox_retained_before_disconnect": retainedBeforeDisconnect,
        "replacement_replayed_identical_event_once": replayed,
        "outbox_retained_until_exact_slot_ack": retainedUntilAcknowledged,
        "outbox_drained_after_ack": pendingCleared,
    ]
    let detail = checks.keys.sorted().map {
        "\($0)=\(checks[$0] == true)"
    }.joined(separator: ", ")
    return (latency, checks.values.allSatisfy { $0 }, detail)
}

@MainActor
func midResourceReconnectSample() async throws -> (latency: Double, passed: Bool) {
    let hash = Data(SHA256.hash(data: fixturePNG))
    let resourceHash = try ResourceHash(bytes: hash)
    let cache = ResourceCache()
    let outbox = EventOutbox()
    let firstTransport = BenchmarkTransport()
    let first = SessionController(
        transport: firstTransport,
        outbox: outbox,
        resourceCache: cache
    )
    try await first.start()
    try await firstTransport.injectFromServer(
        try framed(welcomeMessage(sessionID: "resource-session"))
    )
    try await waitUntil { first.isEventDispatchEnabled }
    let midpoint = fixturePNG.count / 2
    try await firstTransport.injectFromServer(
        try framed(resourceMetadataMessage(hash: hash))
    )
    try await firstTransport.injectFromServer(
        try framed(
            resourceChunkMessage(
                hash: hash,
                offset: 0,
                data: fixturePNG.prefix(midpoint)
            )
        )
    )
    try await waitUntil { await cache.retainedBytes() > 0 }
    let partialWasInvisible = await cache.contains(resourceHash) == false
    let partialBytes = await cache.retainedBytes()

    let started = clock.now
    await first.stop()
    let partialBytesAfterDisconnect = await cache.retainedBytes()

    let secondTransport = BenchmarkTransport()
    let second = SessionController(
        transport: secondTransport,
        outbox: outbox,
        resourceCache: cache,
        sessionId: "resource-session"
    )
    try await second.start()
    try await secondTransport.injectFromServer(
        try framed(resumeOKMessage(sessionID: "resource-session"))
    )
    try await waitUntil { second.isEventDispatchEnabled }
    try await secondTransport.injectFromServer(
        try framed(resourceMetadataMessage(hash: hash))
    )
    try await secondTransport.injectFromServer(
        try framed(
            resourceChunkMessage(
                hash: hash,
                offset: 0,
                data: fixturePNG.prefix(midpoint)
            )
        )
    )
    try await secondTransport.injectFromServer(
        try framed(
            resourceChunkMessage(
                hash: hash,
                offset: midpoint,
                data: fixturePNG.suffix(from: midpoint)
            )
        )
    )
    try await waitUntil { await cache.contains(resourceHash) }
    let committed = await cache.contains(resourceHash)
    let secondWasActive = second.isEventDispatchEnabled
    let latency = milliseconds(started.duration(to: clock.now))
    await second.stop()
    return (
        latency,
        partialWasInvisible
            && partialBytes > 0
            && partialBytesAfterDisconnect == 0
            && secondWasActive
            && committed
    )
}

@MainActor
func supersededResumeSample(
    fixtureIndex: BenchmarkFixtureIndex
) async throws -> (
    oldResponseLatency: Double,
    newResponseLatency: Double,
    passed: Bool,
    detail: String
) {
    let outbox = EventOutbox()
    let seedTransport = BenchmarkTransport()
    let seed = SessionController(transport: seedTransport, outbox: outbox)
    try await seed.start()
    try await seedTransport.injectFromServer(
        try framed(welcomeMessage(sessionID: "superseded-session"))
    )
    try await waitUntil { seed.isEventDispatchEnabled }
    let pending = try await seed.sendActivate(
        nodeId: fixtureIndex.primaryAction
    )
    await seed.stop()

    let oldFailures = ResumeFailureObservation()
    let oldTransport = BenchmarkTransport()
    let oldApplier = TransactionApplier()
    let oldController = SessionController(
        transport: oldTransport,
        applier: oldApplier,
        outbox: outbox,
        sessionId: "superseded-session"
    )
    oldController.onFailure = { failure in
        Task { await oldFailures.record(failure) }
    }
    try await oldController.start()

    let newFailures = ResumeFailureObservation()
    let newTransport = BenchmarkTransport()
    let newApplier = TransactionApplier()
    let newController = SessionController(
        transport: newTransport,
        applier: newApplier,
        outbox: outbox,
        sessionId: "superseded-session"
    )
    newController.onFailure = { failure in
        Task { await newFailures.record(failure) }
    }
    try await newController.start()

    let newWireBeforeOldResponse = await newTransport.snapshot()
    let pendingBeforeOldResponse = await outbox.pendingCount
    let newRevisionBeforeOldResponse = newApplier.lastAppliedRevision
    let oldStart = clock.now
    try await oldTransport.injectFromServer(
        try framed(resumeOKMessage(sessionID: "superseded-session"))
    )
    try await waitUntil {
        let failure = await oldFailures.snapshot()
        let transport = await oldTransport.snapshot()
        return failure.count == 1 && transport.isClosed
    }
    let oldLatency = milliseconds(oldStart.duration(to: clock.now))

    let oldWireAfterResponse = await oldTransport.snapshot()
    let oldEvents = try await capturedEvents(in: oldTransport)
    let oldFailure = await oldFailures.snapshot()
    let oldDivergedAfterResponse = oldController.isDiverged
    let oldHandshakeAfterResponse = oldController.isHandshakeComplete
    let oldDispatchAfterResponse = oldController.isEventDispatchEnabled
    let newFailureAfterOldResponse = await newFailures.snapshot()
    let newWireAfterOldResponse = await newTransport.snapshot()
    let pendingAfterOldResponse = await outbox.pendingCount
    let newRevisionAfterOldResponse = newApplier.lastAppliedRevision

    let newStart = clock.now
    try await newTransport.injectFromServer(
        try framed(resumeOKMessage(sessionID: "superseded-session"))
    )
    try await waitUntil {
        let events = try? await capturedEvents(in: newTransport)
        return events?.count == 1 && newController.isEventDispatchEnabled
    }
    let newLatency = milliseconds(newStart.duration(to: clock.now))
    var newEvents = try await capturedEvents(in: newTransport)
    let replayMatches = newEvents.first.map {
        $0.id == pending.eventId.bytes && $0.sequence == pending.eventSeq
    } ?? false
    try await newTransport.injectFromServer(
        try framed(
            acknowledgementForCapturedEvent(
                newEvents[0],
                outbox: outbox,
                sessionID: "superseded-session",
                revision: newApplier.lastAppliedRevision
            )
        )
    )
    try await waitUntil { await outbox.pendingCount == 0 }

    let fresh = try await newController.sendActivate(
        nodeId: fixtureIndex.primaryAction
    )
    try await waitUntil {
        (try? await capturedEvents(in: newTransport).count) == 2
    }
    newEvents = try await capturedEvents(in: newTransport)
    let freshActionMatches = newEvents.last.map {
        $0.id == fresh.eventId.bytes
            && $0.sequence == fresh.eventSeq
            && $0.observedRevision == fresh.observedRevision.value
    } ?? false
    try await newTransport.injectFromServer(
        try framed(
            acknowledgementForCapturedEvent(
                newEvents[1],
                outbox: outbox,
                sessionID: "superseded-session",
                revision: newApplier.lastAppliedRevision
            )
        )
    )
    try await waitUntil { await outbox.pendingCount == 0 }

    let activeWireBeforeOldStop = await newTransport.snapshot()
    await oldController.stop()
    let activeWireAfterOldStop = await newTransport.snapshot()
    let newFailureAtEnd = await newFailures.snapshot()
    let newLifecycleUnaffected = newController.isDiverged == false
        && newController.isEventDispatchEnabled
        && newFailureAtEnd.count == 0
        && newApplier.lastAppliedRevision == .initial
        && newApplier.store.nodeCount == 0
        && activeWireAfterOldStop.outboundBytes == activeWireBeforeOldStop.outboundBytes
        && activeWireAfterOldStop.outboundMessages == activeWireBeforeOldStop.outboundMessages
        && activeWireAfterOldStop.inboundBytes == activeWireBeforeOldStop.inboundBytes
        && activeWireAfterOldStop.inboundMessages == activeWireBeforeOldStop.inboundMessages

    let checks = [
        "old_response_sent_only_to_old_transport":
            oldWireAfterResponse.inboundMessages == 1,
        "old_replay_is_empty": oldEvents.isEmpty,
        "old_lifecycle_reports_superseded":
            oldFailure.count == 1 && oldFailure.containsSuperseded,
        "old_lifecycle_diverged": oldDivergedAfterResponse,
        "old_lifecycle_handshake_failed": oldHandshakeAfterResponse == false,
        "old_lifecycle_dispatch_blocked": oldDispatchAfterResponse == false,
        "old_lifecycle_closed": oldWireAfterResponse.closeCalls == 1
            && oldWireAfterResponse.isClosed,
        "pending_unchanged_by_old_response":
            pendingBeforeOldResponse == 1 && pendingAfterOldResponse == 1,
        "new_wire_unchanged_by_old_response":
            newWireAfterOldResponse.outboundBytes == newWireBeforeOldResponse.outboundBytes
                && newWireAfterOldResponse.outboundMessages
                    == newWireBeforeOldResponse.outboundMessages
                && newWireAfterOldResponse.inboundBytes == newWireBeforeOldResponse.inboundBytes
                && newWireAfterOldResponse.inboundMessages
                    == newWireBeforeOldResponse.inboundMessages,
        "new_semantic_state_unchanged_by_old_response":
            newRevisionAfterOldResponse == newRevisionBeforeOldResponse
                && newApplier.store.nodeCount == 0,
        "new_failure_not_called_by_old_response": newFailureAfterOldResponse.count == 0,
        "new_response_replays_original_event": replayMatches,
        "new_action_identity_and_order_preserved": freshActionMatches,
        "new_lifecycle_unaffected_by_old_stop": newLifecycleUnaffected,
    ]
    let passed = checks.values.allSatisfy { $0 }
    let detail = checks.keys.sorted().map {
        "\($0)=\(checks[$0] == true)"
    }.joined(separator: ", ")
        + "; old_failures=\(oldFailure.descriptions)"

    await newController.stop()
    return (oldLatency, newLatency, passed, detail)
}

@MainActor
func reconnect(
    iterations: Int,
    fixtureOperations: [SemanticModel.Operation],
    fixtureIndex: BenchmarkFixtureIndex
) async throws -> Section {
    var preReceiptLatencies = [Double]()
    var resourceLatencies = [Double]()
    var supersededLatencies = [Double]()
    var activeLatencies = [Double]()
    var preReceiptPassed = true
    var resourcePassed = true
    var supersededPassed = true
    var preReceiptDetails = Set<String>()
    var supersededDetails = Set<String>()
    for _ in 0..<max(2, min(5, iterations)) {
        let preReceipt = try await preReceiptEventReplaySample(
            fixtureOperations: fixtureOperations,
            fixtureIndex: fixtureIndex
        )
        preReceiptLatencies.append(preReceipt.latency)
        preReceiptPassed = preReceiptPassed && preReceipt.passed
        preReceiptDetails.insert(preReceipt.detail)

        let resource = try await midResourceReconnectSample()
        resourceLatencies.append(resource.latency)
        resourcePassed = resourcePassed && resource.passed

        let superseded = try await supersededResumeSample(
            fixtureIndex: fixtureIndex
        )
        supersededLatencies.append(superseded.oldResponseLatency)
        activeLatencies.append(superseded.newResponseLatency)
        supersededPassed = supersededPassed && superseded.passed
        supersededDetails.insert(superseded.detail)
    }
    let sampleCounts = [
        "macos.pre_receipt": preReceiptLatencies.count,
        "macos.mid_resource": resourceLatencies.count,
        "macos.superseded_response": supersededLatencies.count,
        "macos.active_response": activeLatencies.count,
    ]
    return Section(
        id: "31.5",
        name: "Reconnect",
        sampleCounts: sampleCounts,
        metrics: [
            metric("pre-receipt retained-event replay", p50(preReceiptLatencies), id: "pre_receipt_event_replay"),
            metric("pre-receipt retained-event replay", percentile(preReceiptLatencies, 0.95), "ms", "p95", id: "pre_receipt_event_replay"),
            metric("mid-resource reconnect recovery", p50(resourceLatencies), id: "mid_resource_recovery"),
            metric("mid-resource reconnect recovery", percentile(resourceLatencies, 0.95), "ms", "p95", id: "mid_resource_recovery"),
            metric("superseded resume response handling", p50(supersededLatencies), id: "superseded_response"),
            metric("superseded resume response handling", percentile(supersededLatencies, 0.95), "ms", "p95", id: "superseded_response"),
            metric("active resume response handling", p50(activeLatencies), id: "active_response"),
            metric("active resume response handling", percentile(activeLatencies, 0.95), "ms", "p95", id: "active_response"),
        ],
        assertions: [
            Assertion(
                id: "pre_receipt_pending_replay",
                name: "event retained before server receipt replays with identical identity exactly once",
                passed: preReceiptPassed,
                detail: preReceiptDetails.sorted().joined(separator: "; ")
            ),
            Assertion(
                id: "mid_resource_recovery",
                name: "mid-resource disconnect discards partial bytes and retransmission commits",
                passed: resourcePassed,
                detail: "SessionController framing and ownership retired invisible partials; replacement replayed metadata and contiguous chunks from offset zero"
            ),
            Assertion(
                id: "superseded_response_inert",
                name: "superseded response affects only the expected old lifecycle teardown",
                passed: supersededPassed,
                detail: supersededDetails.sorted().joined(separator: "; ")
            ),
        ],
        notes: [
            "The pre-receipt sample allocates the production EventOutbox identity before a deterministic transport loss delivers zero EVENT frames; a replacement SessionController then replays the exact retained id/sequence/revision and drains it only after an exact-slot acknowledgement.",
            "The old resume response is inert with respect to active/new wire, event/action identity, semantic state, and lifecycle. Task 23 intentionally reports .superseded, marks the old controller diverged, and closes only its transport; those expected old-lifecycle effects are measured explicitly."
        ]
    )
}
