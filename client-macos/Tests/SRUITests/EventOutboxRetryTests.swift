//
// EventOutboxRetryTests.swift
// SRUITests
//
// Retry-safe event replay and acknowledgement state carried by CLIENT RESUME (§18, §18.2, App. B).
//

import Testing
import Foundation
import SemanticModel
import Protocol
@testable import Session
import TransportSSH

typealias OutboxWireCollector = ResyncWireCollector

/// Transport that records each send before suspending it until the test releases that write.
private actor GatedTransport: Transport {
    private let stream: AsyncThrowingStream<Data, Error>
    private let streamContinuation: AsyncThrowingStream<Data, Error>.Continuation
    private let sendReleaseStream: AsyncStream<Void>
    private let sendReleaseContinuation: AsyncStream<Void>.Continuation
    private var sentFrames: [Data] = []

    init() {
        let (stream, continuation) = AsyncThrowingStream<Data, Error>.makeStream()
        self.stream = stream
        self.streamContinuation = continuation
        let (sendReleaseStream, sendReleaseContinuation) = AsyncStream<Void>.makeStream()
        self.sendReleaseStream = sendReleaseStream
        self.sendReleaseContinuation = sendReleaseContinuation
    }

    var sentFrameCount: Int { sentFrames.count }

    func frame(at index: Int) -> Data? {
        sentFrames.indices.contains(index) ? sentFrames[index] : nil
    }

    func waitForSendCount(_ count: Int) async {
        while sentFrames.count < count {
            await Task.yield()
        }
    }

    func releaseNextSend() {
        sendReleaseContinuation.yield()
    }

    func send(data: Data, logicalClass _: LogicalChannelClass) async throws {
        sentFrames.append(data)
        var releases = sendReleaseStream.makeAsyncIterator()
        _ = await releases.next()
    }

    nonisolated func receiveStream() -> AsyncThrowingStream<Data, Error> {
        stream
    }

    func close() async {
        sendReleaseContinuation.finish()
        streamContinuation.finish()
    }
}

/// Transport that fails every send with a fixed error (for replay failure tests).
private actor FailingTransport: Transport {
    private let stream: AsyncThrowingStream<Data, Error>
    private let streamContinuation: AsyncThrowingStream<Data, Error>.Continuation

    init() {
        let (stream, continuation) = AsyncThrowingStream<Data, Error>.makeStream()
        self.stream = stream
        self.streamContinuation = continuation
    }

    func send(data: Data, logicalClass _: LogicalChannelClass) async throws {
        throw TransportError.ioError("simulated replay transport failure")
    }

    nonisolated func receiveStream() -> AsyncThrowingStream<Data, Error> {
        stream
    }

    func close() async {
        streamContinuation.finish()
    }
}

/// Transport whose initial replay succeeds and whose background retry fails.
private actor FailAfterFirstSendTransport: Transport {
    private let stream: AsyncThrowingStream<Data, Error>
    private let streamContinuation: AsyncThrowingStream<Data, Error>.Continuation
    private var sendCount = 0
    private var closeCount = 0

    init() {
        let (stream, continuation) = AsyncThrowingStream<Data, Error>.makeStream()
        self.stream = stream
        self.streamContinuation = continuation
    }

    var closeCallCount: Int { closeCount }

    func send(data: Data, logicalClass _: LogicalChannelClass) async throws {
        sendCount += 1
        if sendCount > 1 {
            throw TransportError.ioError("simulated background replay failure")
        }
    }

    nonisolated func receiveStream() -> AsyncThrowingStream<Data, Error> {
        stream
    }

    func close() async {
        closeCount += 1
        streamContinuation.finish()
    }
}

@Suite("EventOutbox Retry Safety Tests")
struct EventOutboxRetryTests {

    private func events(in messages: [SRUIMessage]) throws -> [Event] {
        let decoder = ProtocolDecoder()
        return try messages.compactMap { message -> Event? in
            guard case .event(let wire) = message.msg else { return nil }
            return try decoder.validateAndConvertEvent(wire: wire)
        }
    }

    private func activeBinding(
        for outbox: EventOutbox
    ) async throws -> EventOutboxConnectionBinding {
        let binding = await outbox.beginConnectionBinding()
        let allowed = await outbox.allowNewEvents(binding: binding)
        try #require(allowed)
        return binding
    }

    private func resumeSameSession(
        _ outbox: EventOutbox,
        id: String,
        lastProcessedEventSeq: UInt64,
        generation: UInt64,
        binding: EventOutboxConnectionBinding,
        via transport: any Transport,
        enableNewEventsAfterReplay: Bool,
        onReplayFailure: (@Sendable (String) async -> Void)? = nil
    ) async throws -> Bool {
        guard let preparation = try await outbox.prepareSameSessionResume(
            id: id,
            lastProcessedEventSeq: lastProcessedEventSeq,
            generation: generation,
            binding: binding
        ) else {
            return false
        }
        return try await outbox.completeSameSessionResume(
            preparation,
            via: transport,
            enableNewEventsAfterReplay: enableNewEventsAfterReplay,
            onReplayFailure: onReplayFailure
        )
    }

    @Test("Replaying an unacknowledged event reuses its original identity (§18.2)")
    func replayReusesOriginalEventIdentity() async throws {
        let (client, server) = await PipeTransport.createPair()
        let collector = OutboxWireCollector()
        await collector.start(draining: server)

        let outbox = EventOutbox()
        let binding = try await activeBinding(for: outbox)
        let original = try await outbox.sendActivate(
            nodeId: NodeId(7),
            observedRevision: Revision(3),
            binding: binding,
            via: client
        )

        // The connection died before the acknowledgement arrived; the resume replays the event.
        // A fresh event_id here would be a second, semantically independent action (§18.2).
        try await outbox.resendPendingEvents(binding: binding, via: client)

        let messages = await collector.wait(forAtLeast: 2)
        let decoded = try events(in: messages)
        #expect(decoded.count == 2)

        let replayed = try #require(decoded.last)
        #expect(replayed.eventId == original.eventId)
        #expect(replayed.eventSeq == original.eventSeq)
        #expect(replayed.clientInstanceId == original.clientInstanceId)

        await collector.stop()
        await client.close()
        await server.close()
    }

    @Test("An acknowledged event is dropped and advances last_acked_event_seq (§18)")
    func acknowledgementAdvancesResumeState() async throws {
        let (client, server) = await PipeTransport.createPair()
        let outbox = EventOutbox()
        let binding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "session-ack", binding: binding))

        let event = try await outbox.sendActivate(
            nodeId: NodeId(7),
            observedRevision: Revision(3),
            binding: binding,
            via: client
        )
        #expect(await outbox.pendingCount == 1)

        #expect(await outbox.settleAcknowledgement(
            binding: binding,
            clientInstanceId: outbox.clientInstanceId,
            eventId: event.eventId,
            throughSeq: 0,
            sessionId: "session-ack"
        ).bound)

        #expect(await outbox.pendingCount == 0)
        #expect(await outbox.lastAckedEventSeq == event.eventSeq)

        // Nothing is pending, so a resume replays nothing.
        try await outbox.resendPendingEvents(binding: binding, via: client)

        await client.close()
        await server.close()
    }

    @Test("Replay failure blocks same-session resume from enabling new events")
    func replayFailureBlocksDispatchEnablement() async throws {
        let (seedClient, seedServer) = await PipeTransport.createPair()
        let outbox = EventOutbox()
        let seedBinding = try await activeBinding(for: outbox)
        _ = try await outbox.sendActivate(
            nodeId: NodeId(7),
            observedRevision: Revision(3),
            binding: seedBinding,
            via: seedClient
        )

        let resumedBinding = await outbox.beginConnectionBinding()
        let generation = try #require(
            await outbox.beginResumeAttempt(binding: resumedBinding)
        )
        let failing = FailingTransport()

        var replayFailed = false
        do {
            _ = try await resumeSameSession(
                outbox,
                id: "session-a",
                lastProcessedEventSeq: 0,
                generation: generation,
                binding: resumedBinding,
                via: failing,
                enableNewEventsAfterReplay: true
            )
        } catch let error as TransportError {
            replayFailed = true
            if case .ioError(let message) = error {
                #expect(message.contains("simulated replay transport failure"))
            } else {
                Issue.record("Expected ioError, got \(error)")
            }
        } catch {
            Issue.record("Expected TransportError, got \(error)")
        }
        #expect(replayFailed)

        await #expect(throws: EventOutboxError.resumeNotConfirmed) {
            try await outbox.sendActivate(
                nodeId: NodeId(8),
                observedRevision: Revision(3),
                binding: resumedBinding,
                via: seedClient
            )
        }
        #expect(await outbox.pendingCount == 1)

        await seedClient.close()
        await seedServer.close()
        await failing.close()
    }

    @Test(
        "A resumed pending event retries until the server settles it",
        .bug("https://github.com/aizlabs/srui/issues/17")
    )
    func resumedPendingEventRetriesUntilAcknowledged() async throws {
        let (seedClient, seedServer) = await PipeTransport.createPair()
        let outbox = EventOutbox(
            replayRetryInitialDelay: .zero,
            replayRetryMaximumDelay: .zero
        )
        let seedBinding = try await activeBinding(for: outbox)
        let pending = try await outbox.sendActivate(
            nodeId: NodeId(7),
            observedRevision: Revision(3),
            binding: seedBinding,
            via: seedClient
        )

        let resumedTransport = GatedTransport()
        let resumedBinding = await outbox.beginConnectionBinding()
        let generation = try #require(
            await outbox.beginResumeAttempt(binding: resumedBinding)
        )
        let resumeTask = Task {
            try await resumeSameSession(
                outbox,
                id: "session-a",
                lastProcessedEventSeq: 0,
                generation: generation,
                binding: resumedBinding,
                via: resumedTransport,
                enableNewEventsAfterReplay: true
            )
        }

        await resumedTransport.waitForSendCount(1)
        await resumedTransport.releaseNextSend()
        #expect(try await resumeTask.value)

        // The first replay overlapped the server's original in-flight delivery, so no ack arrived.
        // The retry loop must send the same semantic event again on the resumed connection.
        await resumedTransport.waitForSendCount(2)
        let initialReplayFrame = try #require(await resumedTransport.frame(at: 0))
        let retryFrame = try #require(await resumedTransport.frame(at: 1))
        let initialReplay = try #require(
            try events(in: [decodeFramedMessage(from: initialReplayFrame)]).first
        )
        let retry = try #require(
            try events(in: [decodeFramedMessage(from: retryFrame)]).first
        )
        #expect(initialReplay.eventId == pending.eventId)
        #expect(retry.eventId == pending.eventId)
        #expect(retry.eventSeq == pending.eventSeq)
        #expect(await outbox.isRetryingPendingEvents)

        _ = await outbox.settleAcknowledgement(
            binding: resumedBinding,
            clientInstanceId: outbox.clientInstanceId,
            eventId: pending.eventId,
            throughSeq: pending.eventSeq,
            sessionId: "session-a"
        )
        #expect(await outbox.pendingCount == 0)
        #expect(await outbox.isRetryingPendingEvents == false)

        await resumedTransport.releaseNextSend()
        await resumedTransport.close()
        await seedClient.close()
        await seedServer.close()
    }

    @Test(
        "A live resync frontier keeps retries active until the remaining event settles",
        .bug("https://github.com/aizlabs/srui/issues/20")
    )
    func liveResyncFrontierKeepsRetryLeaseForUnsettledEvents() async throws {
        let (seedClient, seedServer) = await PipeTransport.createPair()
        let outbox = EventOutbox(
            replayRetryInitialDelay: .zero,
            replayRetryMaximumDelay: .zero
        )
        let seedBinding = try await activeBinding(for: outbox)
        let first = try await outbox.sendActivate(
            nodeId: NodeId(7),
            observedRevision: Revision(3),
            binding: seedBinding,
            via: seedClient
        )
        let second = try await outbox.sendActivate(
            nodeId: NodeId(8),
            observedRevision: Revision(3),
            binding: seedBinding,
            via: seedClient
        )

        let resumedTransport = GatedTransport()
        let resumedBinding = await outbox.beginConnectionBinding()
        let generation = try #require(
            await outbox.beginResumeAttempt(binding: resumedBinding)
        )
        let resumeTask = Task {
            try await resumeSameSession(
                outbox,
                id: "session-a",
                lastProcessedEventSeq: 0,
                generation: generation,
                binding: resumedBinding,
                via: resumedTransport,
                enableNewEventsAfterReplay: true
            )
        }

        await resumedTransport.waitForSendCount(1)
        await resumedTransport.releaseNextSend()
        await resumedTransport.waitForSendCount(2)
        await resumedTransport.releaseNextSend()
        #expect(try await resumeTask.value)
        #expect(await outbox.isRetryingPendingEvents)

        await outbox.applyLiveResyncFrontier(
            lastProcessedEventSeq: first.eventSeq,
            binding: resumedBinding
        )

        #expect(await outbox.pendingCount == 1)
        #expect(await outbox.lastAckedEventSeq == first.eventSeq)
        #expect(await outbox.isRetryingPendingEvents)

        _ = await outbox.settleAcknowledgement(
            binding: resumedBinding,
            clientInstanceId: outbox.clientInstanceId,
            eventId: second.eventId,
            throughSeq: second.eventSeq,
            sessionId: "session-a"
        )
        #expect(await outbox.pendingCount == 0)
        #expect(await outbox.isRetryingPendingEvents == false)

        await resumedTransport.releaseNextSend()
        await resumedTransport.close()
        await seedClient.close()
        await seedServer.close()
    }

    @Test(
        "Background replay failure is surfaced without closing controller-owned transport",
        .bug("https://github.com/aizlabs/srui/issues/17")
    )
    func backgroundReplayFailureIsSurfacedWithoutClosingTransport() async throws {
        let (seedClient, seedServer) = await PipeTransport.createPair()
        let outbox = EventOutbox(
            replayRetryInitialDelay: .zero,
            replayRetryMaximumDelay: .zero
        )
        let seedBinding = try await activeBinding(for: outbox)
        _ = try await outbox.sendActivate(
            nodeId: NodeId(7),
            observedRevision: Revision(3),
            binding: seedBinding,
            via: seedClient
        )

        let resumedTransport = FailAfterFirstSendTransport()
        let (failures, failureContinuation) = AsyncStream<String>.makeStream()
        let resumedBinding = await outbox.beginConnectionBinding()
        let generation = try #require(
            await outbox.beginResumeAttempt(binding: resumedBinding)
        )
        let accepted = try await resumeSameSession(
            outbox,
            id: "session-a",
            lastProcessedEventSeq: 0,
            generation: generation,
            binding: resumedBinding,
            via: resumedTransport,
            enableNewEventsAfterReplay: true,
            onReplayFailure: { error in
                failureContinuation.yield(error)
                failureContinuation.finish()
            }
        )
        #expect(accepted)

        var failureIterator = failures.makeAsyncIterator()
        let failure = await failureIterator.next()
        #expect(failure?.contains("simulated background replay failure") == true)
        #expect(await resumedTransport.closeCallCount == 0)

        await outbox.stopResumeWork(generation: generation)
        await resumedTransport.close()
        await seedClient.close()
        await seedServer.close()
    }

    @Test(
        "A completed resume no longer occupies the handshake latch",
        .bug("https://github.com/aizlabs/srui/issues/17")
    )
    func completedResumeReleasesHandshakeLatch() async throws {
        let (seedClient, seedServer) = await PipeTransport.createPair()
        let outbox = EventOutbox()
        let seedBinding = try await activeBinding(for: outbox)
        _ = try await outbox.sendActivate(
            nodeId: NodeId(7),
            observedRevision: Revision(3),
            binding: seedBinding,
            via: seedClient
        )

        let (resumedClient, resumedServer) = await PipeTransport.createPair()
        let collector = OutboxWireCollector()
        await collector.start(draining: resumedServer)
        let resumedBinding = await outbox.beginConnectionBinding()
        let generation = try #require(
            await outbox.beginResumeAttempt(binding: resumedBinding)
        )
        let accepted = try await resumeSameSession(
            outbox,
            id: "session-a",
            lastProcessedEventSeq: 0,
            generation: generation,
            binding: resumedBinding,
            via: resumedClient,
            enableNewEventsAfterReplay: true
        )
        #expect(accepted)
        #expect(await outbox.isRetryingPendingEvents)
        #expect(await outbox.confirmFreshSession(id: "session-fresh", binding: resumedBinding))
        #expect(await outbox.isRetryingPendingEvents == false)

        await collector.stop()
        await resumedClient.close()
        await resumedServer.close()
        await seedClient.close()
        await seedServer.close()
    }

    @Test("A SERVER EVENT_ACK settles the event and raises last_acked_event_seq (§18.2)")
    func serverEventAckDrainsPendingEvent() async throws {
        let (client, server) = await PipeTransport.createPair()
        let outbox = EventOutbox()
        let controller = SessionController(transport: client, outbox: outbox)
        try await controller.start()
        await controller.handleIncomingMessage(HandshakeFixtures.welcomeMessage())
        let candidateBinding = await outbox.activeConnectionBindingForTesting
        let binding = try #require(candidateBinding)

        let event = try await outbox.sendActivate(
            nodeId: NodeId(7),
            observedRevision: Revision(3),
            binding: binding,
            via: client
        )
        #expect(await outbox.pendingCount == 1)
        #expect(await outbox.lastAckedEventSeq == 0)

        var ack = SRUIServerEventAck()
        ack.clientInstanceID = outbox.clientInstanceId.bytes
        ack.eventID = event.eventId.bytes
        ack.lastProcessedEventSeq = event.eventSeq
        ack.status = .processed
        ack.revisionAfterEffect = 4
        ack.sessionID = "test-session"

        var message = SRUIMessage()
        message.serverEventAck = ack
        await controller.handleIncomingMessage(message)

        #expect(await outbox.pendingCount == 0)
        #expect(await outbox.lastAckedEventSeq == event.eventSeq)

        await controller.stop()
        await client.close()
        await server.close()
    }

    @Test("Cumulative seq drain retires lower-seq pending events (§18.2)")
    func cumulativeSeqDrainRetiresLowerSeqEvents() async throws {
        let (client, server) = await PipeTransport.createPair()
        let collector = OutboxWireCollector()
        await collector.start(draining: server)

        let outbox = EventOutbox()
        let controller = SessionController(transport: client, outbox: outbox)
        try await controller.start()
        await controller.handleIncomingMessage(HandshakeFixtures.welcomeMessage())
        let candidateBinding = await outbox.activeConnectionBindingForTesting
        let binding = try #require(candidateBinding)

        let first = try await outbox.sendActivate(
            nodeId: NodeId(1),
            observedRevision: Revision(1),
            binding: binding,
            via: client
        )
        let second = try await outbox.sendActivate(
            nodeId: NodeId(2),
            observedRevision: Revision(1),
            binding: binding,
            via: client
        )
        #expect(await outbox.pendingCount == 2)
        let sentBeforeAck = try events(in: await collector.wait(forAtLeast: 3)).count

        var ack = SRUIServerEventAck()
        ack.clientInstanceID = outbox.clientInstanceId.bytes
        ack.eventID = second.eventId.bytes
        ack.lastProcessedEventSeq = second.eventSeq
        ack.status = .processed
        ack.sessionID = "test-session"

        var message = SRUIMessage()
        message.serverEventAck = ack
        await controller.handleIncomingMessage(message)

        #expect(await outbox.pendingCount == 0)
        #expect(await outbox.lastAckedEventSeq == second.eventSeq)

        try await outbox.resendPendingEvents(binding: binding, via: client)
        let sentAfterAck = try events(
            in: await collector.wait(forAtLeast: sentBeforeAck + 1, timeout: 0.5)
        ).count
        #expect(sentAfterAck == sentBeforeAck)

        await controller.stop()
        await collector.stop()
        await client.close()
        await server.close()
        _ = first
    }

    @Test("A selective ack beyond a gap retains the missing event (§18.2)")
    func selectiveAckDoesNotCrossMissingSequence() async throws {
        let (client, server) = await PipeTransport.createPair()
        let collector = OutboxWireCollector()
        await collector.start(draining: server)

        let outbox = EventOutbox()
        let controller = SessionController(transport: client, outbox: outbox)
        try await controller.start()
        await controller.handleIncomingMessage(HandshakeFixtures.welcomeMessage())
        let candidateBinding = await outbox.activeConnectionBindingForTesting
        let binding = try #require(candidateBinding)
        let first = try await outbox.sendActivate(
            nodeId: NodeId(1),
            observedRevision: Revision(1),
            binding: binding,
            via: client
        )
        let second = try await outbox.sendActivate(
            nodeId: NodeId(2),
            observedRevision: Revision(1),
            binding: binding,
            via: client
        )

        var secondAck = SRUIServerEventAck()
        secondAck.clientInstanceID = outbox.clientInstanceId.bytes
        secondAck.eventID = second.eventId.bytes
        secondAck.lastProcessedEventSeq = 0
        secondAck.status = .processed
        secondAck.sessionID = "test-session"
        var secondMessage = SRUIMessage()
        secondMessage.serverEventAck = secondAck
        await controller.handleIncomingMessage(secondMessage)

        #expect(await outbox.pendingCount == 1)
        #expect(await outbox.lastAckedEventSeq == 0)

        try await outbox.resendPendingEvents(binding: binding, via: client)
        let replayedMessages = await collector.wait(forAtLeast: 4)
        let replayed = try #require(try events(in: replayedMessages).last)
        #expect(replayed.eventId == first.eventId)

        var firstAck = SRUIServerEventAck()
        firstAck.clientInstanceID = outbox.clientInstanceId.bytes
        firstAck.eventID = first.eventId.bytes
        firstAck.lastProcessedEventSeq = 2
        firstAck.status = .processed
        firstAck.sessionID = "test-session"
        var firstMessage = SRUIMessage()
        firstMessage.serverEventAck = firstAck
        await controller.handleIncomingMessage(firstMessage)

        #expect(await outbox.pendingCount == 0)
        #expect(await outbox.lastAckedEventSeq == 2)

        await controller.stop()
        await collector.stop()
        await client.close()
        await server.close()
    }

    @Test("A REJECTED ack settles the event so it is never replayed (§18.2)")
    func rejectedAckDropsEventInsteadOfReplayingIt() async throws {
        let (client, server) = await PipeTransport.createPair()
        let collector = OutboxWireCollector()
        await collector.start(draining: server)

        let outbox = EventOutbox()
        let controller = SessionController(transport: client, outbox: outbox)
        try await controller.start()
        await controller.handleIncomingMessage(HandshakeFixtures.welcomeMessage())
        let candidateBinding = await outbox.activeConnectionBindingForTesting
        let binding = try #require(candidateBinding)

        let event = try await outbox.sendActivate(
            nodeId: NodeId(7),
            observedRevision: Revision(3),
            binding: binding,
            via: client
        )

        var ack = SRUIServerEventAck()
        ack.clientInstanceID = outbox.clientInstanceId.bytes
        ack.eventID = event.eventId.bytes
        ack.lastProcessedEventSeq = event.eventSeq
        ack.status = .rejected
        ack.rejectReason = "node 7 is disabled"
        ack.sessionID = "test-session"

        var message = SRUIMessage()
        message.serverEventAck = ack
        await controller.handleIncomingMessage(message)

        // Left pending, the rejected event would be replayed on every resume and refused every
        // time — an unbounded loop the ack exists to break.
        #expect(await outbox.pendingCount == 0)
        try await outbox.resendPendingEvents(binding: binding, via: client)

        let messages = await collector.wait(forAtLeast: 2)
        #expect(try events(in: messages).count == 1)

        await controller.stop()
        await collector.stop()
        await client.close()
        await server.close()
    }

    @Test("A resume after every event is acknowledged replays nothing (§18)")
    func resumeAfterAcksReplaysNothing() async throws {
        let (client, server) = await PipeTransport.createPair()
        let collector = OutboxWireCollector()
        await collector.start(draining: server)

        let outbox = EventOutbox()
        let controller = SessionController(transport: client, outbox: outbox, sessionId: "session-9")
        let binding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "session-9", binding: binding))

        for node in 1...3 {
            let event = try await outbox.sendActivate(
                nodeId: NodeId(UInt64(node)),
                observedRevision: Revision(1),
                binding: binding,
                via: client
            )
            _ = await outbox.settleAcknowledgement(
                binding: binding,
                clientInstanceId: outbox.clientInstanceId,
                eventId: event.eventId,
                throughSeq: event.eventSeq,
                sessionId: "session-9"
            )
        }
        #expect(await outbox.pendingCount == 0)
        #expect(await outbox.lastAckedEventSeq == 3)

        let beforeResume = try events(in: await collector.wait(forAtLeast: 3)).count
        try await controller.start()

        // The handshake frame arrives; nothing after it is a replayed event.
        let messages = await collector.wait(forAtLeast: beforeResume + 1)
        #expect(try events(in: messages).count == beforeResume)
        let resume = try #require(messages.compactMap { message -> SRUIClientResume? in
            guard case .clientResume(let resume) = message.msg else { return nil }
            return resume
        }.first)
        #expect(resume.lastAckedEventSeq == 3)

        await controller.stop()
        await collector.stop()
        await server.close()
    }

    @Test("Pending events wait for explicit same-session resume confirmation")
    func pendingEventsWaitForSameSessionConfirmation() async throws {
        let (seedClient, seedServer) = await PipeTransport.createPair()
        let outbox = EventOutbox()
        let seedBinding = try await activeBinding(for: outbox)
        let pending = try await outbox.sendActivate(
            nodeId: NodeId(7),
            observedRevision: Revision(3),
            binding: seedBinding,
            via: seedClient
        )

        let (client, server) = await PipeTransport.createPair()
        let collector = OutboxWireCollector()
        await collector.start(draining: server)
        let controller = SessionController(
            transport: client,
            outbox: outbox,
            sessionId: "session-old"
        )
        try await controller.start()

        let beforeDecision = await collector.wait(forAtLeast: 1)
        #expect(try events(in: beforeDecision).isEmpty)
        await #expect(throws: SessionDispatchError.self) {
            try await controller.sendActivate(nodeId: NodeId(8))
        }

        var resumeOk = SRUIServerResumeOk()
        resumeOk.sessionID = "session-old"
        resumeOk.lastProcessedEventSeq = 0
        var response = SRUIMessage()
        response.serverResumeOk = resumeOk
        await controller.handleIncomingMessage(response)

        let afterDecision = await collector.wait(forAtLeast: 2)
        let replayed = try #require(try events(in: afterDecision).last)
        #expect(replayed.eventId == pending.eventId)
        #expect(replayed.eventSeq == pending.eventSeq)
        #expect(await outbox.isRetryingPendingEvents)

        await controller.stop()
        #expect(await outbox.isRetryingPendingEvents == false)
        await collector.stop()
        await seedServer.close()
        await server.close()
    }

    @Test("A superseded resume response cannot replay or rebind the outbox")
    func supersededResumeResponseIsIgnored() async throws {
        let (seedClient, seedServer) = await PipeTransport.createPair()
        let outbox = EventOutbox()
        let seedBinding = try await activeBinding(for: outbox)
        let pending = try await outbox.sendActivate(
            nodeId: NodeId(7),
            observedRevision: Revision(3),
            binding: seedBinding,
            via: seedClient
        )

        let (firstClient, firstServer) = await PipeTransport.createPair()
        let firstCollector = OutboxWireCollector()
        await firstCollector.start(draining: firstServer)
        let firstController = SessionController(
            transport: firstClient,
            outbox: outbox,
            sessionId: "session-old"
        )
        try await firstController.start()
        _ = await firstCollector.wait(forAtLeast: 1)

        let (secondClient, secondServer) = await PipeTransport.createPair()
        let secondCollector = OutboxWireCollector()
        await secondCollector.start(draining: secondServer)
        let secondController = SessionController(
            transport: secondClient,
            outbox: outbox,
            sessionId: "session-old"
        )
        try await secondController.start()
        _ = await secondCollector.wait(forAtLeast: 1)

        var resumeOk = SRUIServerResumeOk()
        resumeOk.sessionID = "session-old"
        var response = SRUIMessage()
        response.serverResumeOk = resumeOk

        await firstController.handleIncomingMessage(response)
        let firstMessages = await firstCollector.wait(forAtLeast: 1)
        #expect(try events(in: firstMessages).isEmpty)

        await secondController.handleIncomingMessage(response)
        let secondMessages = await secondCollector.wait(forAtLeast: 2)
        let replayed = try #require(try events(in: secondMessages).last)
        #expect(replayed.eventId == pending.eventId)
        #expect(await outbox.isRetryingPendingEvents)

        await firstController.stop()
        #expect(await outbox.isRetryingPendingEvents)
        await secondController.stop()
        #expect(await outbox.isRetryingPendingEvents == false)
        await secondCollector.stop()
        await seedClient.close()
        await seedServer.close()
        await firstServer.close()
        await secondServer.close()
    }

    @Test("A replacement session abandons old pending intents and resets its sequence")
    func replacementSessionAbandonsPendingEvents() async throws {
        let (seedClient, seedServer) = await PipeTransport.createPair()
        let outbox = EventOutbox()
        let seedBinding = try await activeBinding(for: outbox)
        _ = try await outbox.sendActivate(
            nodeId: NodeId(7),
            observedRevision: Revision(3),
            binding: seedBinding,
            via: seedClient
        )

        let (client, server) = await PipeTransport.createPair()
        let controller = SessionController(
            transport: client,
            outbox: outbox,
            sessionId: "session-old"
        )
        try await controller.start()

        var resync = SRUIServerResyncRequired()
        resync.sessionID = "session-new"
        resync.snapshotRevision = 9
        resync.reason = "requested session expired"
        resync.continuity = .replaced
        resync.lastProcessedEventSeq = 4
        var response = SRUIMessage()
        response.serverResyncRequired = resync
        await controller.handleIncomingMessage(response)

        #expect(await outbox.pendingCount == 0)
        #expect(await outbox.lastAckedEventSeq == 4)
        #expect(await outbox.eventSeq == 4)

        await controller.stop()
        await seedClient.close()
        await seedServer.close()
        await server.close()
    }

    @Test("A high-water mark beyond this outbox's own sequence is ignored (§18)")
    func staleHighWaterMarkDoesNotRetireUnackedEvents() async throws {
        let (client, server) = await PipeTransport.createPair()
        let outbox = EventOutbox()
        let binding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "session-stale", binding: binding))

        _ = try await outbox.sendActivate(
            nodeId: NodeId(7),
            observedRevision: Revision(3),
            binding: binding,
            via: client
        )
        #expect(await outbox.pendingCount == 1)

        // The server tracks `last_processed_event_seq` per client_instance_id and it outlives the
        // connection, while a fresh outbox restarts its own counter at zero. Honoring a mark the
        // outbox never issued would retire an event that was never acknowledged.
        // The identity binds — this is the live session and this client instance — so the refusal
        // below is about the content of the acknowledgement, not about who sent it.
        #expect(await outbox.settleAcknowledgement(
            binding: binding,
            clientInstanceId: outbox.clientInstanceId,
            eventId: EventId(string: "event-from-a-prior-incarnation"),
            throughSeq: 5_000,
            sessionId: "session-stale"
        ).bound)

        #expect(await outbox.pendingCount == 1)
        #expect(await outbox.lastAckedEventSeq == 0)

        await client.close()
        await server.close()
    }

    @Test("An acknowledgement cannot overtake pending-event retention")
    func acknowledgementDuringSendDoesNotRequeueSettledEvent() async throws {
        let transport = GatedTransport()
        let outbox = EventOutbox()
        let binding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "test-session", binding: binding))

        let sendTask = Task {
            try await outbox.sendActivate(
                nodeId: NodeId(7),
                observedRevision: Revision(3),
                binding: binding,
                via: transport
            )
        }

        await transport.waitForSendCount(1)
        let frame = try #require(await transport.frame(at: 0))
        let message = try decodeFramedMessage(from: frame)
        let event = try #require(try events(in: [message]).first)

        _ = await outbox.settleAcknowledgement(
            binding: binding,
            clientInstanceId: outbox.clientInstanceId,
            eventId: event.eventId,
            throughSeq: event.eventSeq,
            sessionId: "test-session"
        )

        #expect(await outbox.pendingCount == 0)
        await transport.releaseNextSend()
        _ = try await sendTask.value
        #expect(await outbox.pendingCount == 0)

        await transport.close()
    }

    @Test("Replay writes finish before a newly allocated event reaches the wire")
    func replaySerializesConcurrentFreshSend() async throws {
        let (seedClient, seedServer) = await PipeTransport.createPair()
        let outbox = EventOutbox()
        let binding = try await activeBinding(for: outbox)
        let first = try await outbox.sendActivate(
            nodeId: NodeId(1),
            observedRevision: Revision(3),
            binding: binding,
            via: seedClient
        )
        let second = try await outbox.sendActivate(
            nodeId: NodeId(2),
            observedRevision: Revision(3),
            binding: binding,
            via: seedClient
        )

        let transport = GatedTransport()
        let replayTask = Task {
            try await outbox.resendPendingEvents(binding: binding, via: transport)
        }
        await transport.waitForSendCount(1)

        let freshTask = Task {
            try await outbox.sendActivate(
                nodeId: NodeId(3),
                observedRevision: Revision(3),
                binding: binding,
                via: transport
            )
        }
        while await outbox.eventSeq < 3 {
            await Task.yield()
        }

        #expect(await transport.sentFrameCount == 1)
        let firstReplayFrame = try #require(await transport.frame(at: 0))
        let firstReplay = try #require(
            try events(in: [decodeFramedMessage(from: firstReplayFrame)]).first
        )
        #expect(firstReplay.eventId == first.eventId)

        await transport.releaseNextSend()
        await transport.waitForSendCount(2)
        let secondReplayFrame = try #require(await transport.frame(at: 1))
        let secondReplay = try #require(
            try events(in: [decodeFramedMessage(from: secondReplayFrame)]).first
        )
        #expect(secondReplay.eventId == second.eventId)

        await transport.releaseNextSend()
        await transport.waitForSendCount(3)
        let freshFrame = try #require(await transport.frame(at: 2))
        let fresh = try #require(try events(in: [decodeFramedMessage(from: freshFrame)]).first)
        #expect(fresh.eventSeq == 3)

        await transport.releaseNextSend()
        try await replayTask.value
        _ = try await freshTask.value
        await transport.close()
        await seedClient.close()
        await seedServer.close()
    }

    @Test("An acknowledgement for another client instance is ignored")
    func mismatchedClientAcknowledgementDoesNotDrainOutbox() async throws {
        let (client, server) = await PipeTransport.createPair()
        let outbox = EventOutbox(clientInstanceId: ClientInstanceId(string: "client-a"))
        let controller = SessionController(transport: client, outbox: outbox)
        try await controller.start()
        await controller.handleIncomingMessage(HandshakeFixtures.welcomeMessage())
        let candidateBinding = await outbox.activeConnectionBindingForTesting
        let binding = try #require(candidateBinding)
        let event = try await outbox.sendActivate(
            nodeId: NodeId(7),
            observedRevision: Revision(3),
            binding: binding,
            via: client
        )

        var ack = SRUIServerEventAck()
        ack.clientInstanceID = ClientInstanceId(string: "client-b").bytes
        ack.eventID = event.eventId.bytes
        ack.lastProcessedEventSeq = event.eventSeq
        ack.status = .processed
        ack.sessionID = "test-session"
        var message = SRUIMessage()
        message.serverEventAck = ack
        await controller.handleIncomingMessage(message)

        #expect(await outbox.pendingCount == 1)
        #expect(await outbox.lastAckedEventSeq == 0)

        await controller.stop()
        await client.close()
        await server.close()
    }

    @Test("The resume handshake reports the last acknowledged event sequence (§18)")
    func handshakeReportsLastAckedEventSeq() async throws {
        let (client, server) = await PipeTransport.createPair()
        let collector = OutboxWireCollector()
        await collector.start(draining: server)

        let outbox = EventOutbox()
        let binding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "session-7", binding: binding))
        let event = try await outbox.sendActivate(
            nodeId: NodeId(7),
            observedRevision: Revision(3),
            binding: binding,
            via: client
        )
        #expect(await outbox.settleAcknowledgement(
            binding: binding,
            clientInstanceId: outbox.clientInstanceId,
            eventId: event.eventId,
            throughSeq: 0,
            sessionId: "session-7"
        ).bound)

        let controller = SessionController(
            transport: client,
            outbox: outbox,
            sessionId: "session-7"
        )
        try await controller.start()

        let messages = await collector.wait(forAtLeast: 2)
        let resume = try #require(messages.compactMap { message -> SRUIClientResume? in
            guard case .clientResume(let resume) = message.msg else { return nil }
            return resume
        }.first)

        #expect(resume.lastAckedEventSeq == event.eventSeq)
        #expect(resume.sessionID == "session-7")

        await controller.stop()
        await collector.stop()
        await server.close()
    }

    @Test("Stale, empty, and foreign-client acknowledgements mutate nothing (§18.2)")
    func acknowledgementIdentityMustBindBeforeAnyMutation() async throws {
        let (client, server) = await PipeTransport.createPair()
        let outbox = EventOutbox(clientInstanceId: ClientInstanceId(string: "client-a"))
        let binding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "session-live", binding: binding))

        let event = try await outbox.sendActivate(
            nodeId: NodeId(7),
            observedRevision: Revision(3),
            binding: binding,
            via: client
        )

        // An expired incarnation draining its socket must not retire an intent the live session
        // still owns.
        #expect(await outbox.settleAcknowledgement(
            binding: binding,
            clientInstanceId: outbox.clientInstanceId,
            eventId: event.eventId,
            throughSeq: event.eventSeq,
            sessionId: "session-expired"
        ) == .unbound)
        // An empty session_id proves nothing about which incarnation settled the event.
        #expect(await outbox.settleAcknowledgement(
            binding: binding,
            clientInstanceId: outbox.clientInstanceId,
            eventId: event.eventId,
            throughSeq: event.eventSeq,
            sessionId: ""
        ) == .unbound)
        #expect(await outbox.settleAcknowledgement(
            binding: binding,
            clientInstanceId: ClientInstanceId(string: "client-b"),
            eventId: event.eventId,
            throughSeq: event.eventSeq,
            sessionId: "session-live"
        ) == .unbound)

        #expect(await outbox.pendingCount == 1)
        #expect(await outbox.lastAckedEventSeq == 0)

        #expect(await outbox.settleAcknowledgement(
            binding: binding,
            clientInstanceId: outbox.clientInstanceId,
            eventId: event.eventId,
            throughSeq: event.eventSeq,
            sessionId: "session-live"
        ).bound)
        #expect(await outbox.pendingCount == 0)
        #expect(await outbox.lastAckedEventSeq == event.eventSeq)

        await client.close()
        await server.close()
    }

    @Test("A retained event_id cannot be rebound to a different sequence or payload (§18.2)")
    func pendingEventIdCannotBeReplaced() async throws {
        let (client, server) = await PipeTransport.createPair()
        let collector = OutboxWireCollector()
        await collector.start(draining: server)

        let outbox = EventOutbox()
        let binding = try await activeBinding(for: outbox)
        let original = try await outbox.sendActivate(
            nodeId: NodeId(7),
            observedRevision: Revision(3),
            binding: binding,
            via: client
        )

        // Reusing the id for a different intent would hand the second action the first one's
        // idempotency key: the server would answer it from the result cache, never running it.
        let resequenced = Event.activate(
            eventSeq: original.eventSeq + 1,
            eventId: original.eventId,
            observedRevision: original.observedRevision,
            nodeId: original.nodeId
        ).withClientInstanceId(outbox.clientInstanceId)
        await #expect(throws: EventOutboxError.pendingEventIdentityConflict(eventId: original.eventId)) {
            try await outbox.sendEvent(resequenced, binding: binding, via: client)
        }

        let repayloaded = Event.activate(
            eventSeq: original.eventSeq,
            eventId: original.eventId,
            observedRevision: original.observedRevision,
            nodeId: NodeId(8)
        ).withClientInstanceId(outbox.clientInstanceId)
        await #expect(throws: EventOutboxError.pendingEventIdentityConflict(eventId: original.eventId)) {
            try await outbox.sendEvent(repayloaded, binding: binding, via: client)
        }

        #expect(await outbox.pendingCount == 1)

        // The retained intent is still the original, byte for byte.
        try await outbox.resendPendingEvents(binding: binding, via: client)
        let messages = await collector.wait(forAtLeast: 2)
        let decoded = try events(in: messages)
        #expect(decoded.count == 2)
        let replayed = try #require(decoded.last)
        #expect(replayed == original)

        await collector.stop()
        await client.close()
        await server.close()
    }
}
