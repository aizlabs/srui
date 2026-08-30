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

/// Drains one side of a transport and decodes the framed messages it carries.
private actor OutboxWireCollector {
    private var messages: [SRUIMessage] = []
    private var task: Task<Void, Never>?

    func start(draining transport: any Transport) {
        guard task == nil else { return }
        let stream = transport.receiveStream()
        task = Task { [weak self] in
            var decoder = SRUIMessageStreamDecoder()
            do {
                for try await chunk in stream {
                    for message in try decoder.appendAndExtract(incoming: chunk) {
                        await self?.append(message)
                    }
                }
            } catch {
                // Stream teardown at end of test; whatever was collected stays valid.
            }
        }
    }

    private func append(_ message: SRUIMessage) {
        messages.append(message)
    }

    func stop() {
        task?.cancel()
        task = nil
    }

    func wait(forAtLeast count: Int, timeout: Double = 2.0) async -> [SRUIMessage] {
        let deadline = Date().addingTimeInterval(timeout)
        while messages.count < count && Date() < deadline {
            try? await Task.sleep(nanoseconds: 5_000_000)
        }
        return messages
    }
}

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

    func send(data: Data) async throws {
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

    func send(data: Data) async throws {
        throw TransportError.ioError("simulated replay transport failure")
    }

    nonisolated func receiveStream() -> AsyncThrowingStream<Data, Error> {
        stream
    }

    func close() async {
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

    @Test("Replaying an unacknowledged event reuses its original identity (§18.2)")
    func replayReusesOriginalEventIdentity() async throws {
        let (client, server) = await PipeTransport.createPair()
        let collector = OutboxWireCollector()
        await collector.start(draining: server)

        let outbox = EventOutbox()
        let original = try await outbox.sendActivate(
            nodeId: NodeId(7),
            observedRevision: Revision(3),
            via: client
        )

        // The connection died before the acknowledgement arrived; the resume replays the event.
        // A fresh event_id here would be a second, semantically independent action (§18.2).
        try await outbox.resendPendingEvents(via: client)

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

        let event = try await outbox.sendActivate(
            nodeId: NodeId(7),
            observedRevision: Revision(3),
            via: client
        )
        #expect(await outbox.pendingCount == 1)

        await outbox.acknowledgeEvent(id: event.eventId)

        #expect(await outbox.pendingCount == 0)
        #expect(await outbox.lastAckedEventSeq == event.eventSeq)

        // Nothing is pending, so a resume replays nothing.
        try await outbox.resendPendingEvents(via: client)

        await client.close()
        await server.close()
    }

    @Test("Replay failure blocks same-session resume from enabling new events")
    func replayFailureBlocksDispatchEnablement() async throws {
        let (seedClient, seedServer) = await PipeTransport.createPair()
        let outbox = EventOutbox()
        _ = try await outbox.sendActivate(
            nodeId: NodeId(7),
            observedRevision: Revision(3),
            via: seedClient
        )

        let generation = await outbox.beginResumeAttempt()
        let failing = FailingTransport()

        var replayFailed = false
        do {
            _ = try await outbox.completeSameSessionResume(
                id: "session-a",
                lastProcessedEventSeq: 0,
                generation: generation,
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
                via: seedClient
            )
        }
        #expect(await outbox.pendingCount == 1)

        await seedClient.close()
        await seedServer.close()
        await failing.close()
    }

    @Test("A SERVER EVENT_ACK settles the event and raises last_acked_event_seq (§18.2)")
    func serverEventAckDrainsPendingEvent() async throws {
        let (client, server) = await PipeTransport.createPair()
        let outbox = EventOutbox()
        let controller = SessionController(transport: client, outbox: outbox)

        let event = try await outbox.sendActivate(
            nodeId: NodeId(7),
            observedRevision: Revision(3),
            via: client
        )
        #expect(await outbox.pendingCount == 1)
        #expect(await outbox.lastAckedEventSeq == 0)

        await controller.handleIncomingMessage(HandshakeFixtures.welcomeMessage())

        var ack = SRUIServerEventAck()
        ack.clientInstanceID = outbox.clientInstanceId.bytes
        ack.eventID = event.eventId.bytes
        ack.lastProcessedEventSeq = event.eventSeq
        ack.status = .processed
        ack.revisionAfterEffect = 4

        var message = SRUIMessage()
        message.serverEventAck = ack
        await controller.handleIncomingMessage(message)

        #expect(await outbox.pendingCount == 0)
        #expect(await outbox.lastAckedEventSeq == event.eventSeq)

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

        let first = try await outbox.sendActivate(
            nodeId: NodeId(1),
            observedRevision: Revision(1),
            via: client
        )
        let second = try await outbox.sendActivate(
            nodeId: NodeId(2),
            observedRevision: Revision(1),
            via: client
        )
        #expect(await outbox.pendingCount == 2)
        let sentBeforeAck = try events(in: await collector.wait(forAtLeast: 2)).count

        await controller.handleIncomingMessage(HandshakeFixtures.welcomeMessage())

        var ack = SRUIServerEventAck()
        ack.clientInstanceID = outbox.clientInstanceId.bytes
        ack.eventID = second.eventId.bytes
        ack.lastProcessedEventSeq = second.eventSeq
        ack.status = .processed

        var message = SRUIMessage()
        message.serverEventAck = ack
        await controller.handleIncomingMessage(message)

        #expect(await outbox.pendingCount == 0)
        #expect(await outbox.lastAckedEventSeq == second.eventSeq)

        try await outbox.resendPendingEvents(via: client)
        let sentAfterAck = try events(in: await collector.wait(forAtLeast: sentBeforeAck, timeout: 0.5)).count
        #expect(sentAfterAck == sentBeforeAck)

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
        let first = try await outbox.sendActivate(
            nodeId: NodeId(1),
            observedRevision: Revision(1),
            via: client
        )
        let second = try await outbox.sendActivate(
            nodeId: NodeId(2),
            observedRevision: Revision(1),
            via: client
        )

        await controller.handleIncomingMessage(HandshakeFixtures.welcomeMessage())

        var secondAck = SRUIServerEventAck()
        secondAck.clientInstanceID = outbox.clientInstanceId.bytes
        secondAck.eventID = second.eventId.bytes
        secondAck.lastProcessedEventSeq = 0
        secondAck.status = .processed
        var secondMessage = SRUIMessage()
        secondMessage.serverEventAck = secondAck
        await controller.handleIncomingMessage(secondMessage)

        #expect(await outbox.pendingCount == 1)
        #expect(await outbox.lastAckedEventSeq == 0)

        try await outbox.resendPendingEvents(via: client)
        let replayedMessages = await collector.wait(forAtLeast: 3)
        let replayed = try #require(try events(in: replayedMessages).last)
        #expect(replayed.eventId == first.eventId)

        var firstAck = SRUIServerEventAck()
        firstAck.clientInstanceID = outbox.clientInstanceId.bytes
        firstAck.eventID = first.eventId.bytes
        firstAck.lastProcessedEventSeq = 2
        firstAck.status = .processed
        var firstMessage = SRUIMessage()
        firstMessage.serverEventAck = firstAck
        await controller.handleIncomingMessage(firstMessage)

        #expect(await outbox.pendingCount == 0)
        #expect(await outbox.lastAckedEventSeq == 2)

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

        let event = try await outbox.sendActivate(
            nodeId: NodeId(7),
            observedRevision: Revision(3),
            via: client
        )

        await controller.handleIncomingMessage(HandshakeFixtures.welcomeMessage())

        var ack = SRUIServerEventAck()
        ack.clientInstanceID = outbox.clientInstanceId.bytes
        ack.eventID = event.eventId.bytes
        ack.lastProcessedEventSeq = event.eventSeq
        ack.status = .rejected
        ack.rejectReason = "node 7 is disabled"

        var message = SRUIMessage()
        message.serverEventAck = ack
        await controller.handleIncomingMessage(message)

        // Left pending, the rejected event would be replayed on every resume and refused every
        // time — an unbounded loop the ack exists to break.
        #expect(await outbox.pendingCount == 0)
        try await outbox.resendPendingEvents(via: client)

        let messages = await collector.wait(forAtLeast: 1)
        #expect(try events(in: messages).count == 1)

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

        for node in 1...3 {
            let event = try await outbox.sendActivate(
                nodeId: NodeId(UInt64(node)),
                observedRevision: Revision(1),
                via: client
            )
            _ = await outbox.settleAcknowledgement(
                eventId: event.eventId,
                throughSeq: event.eventSeq
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
        let pending = try await outbox.sendActivate(
            nodeId: NodeId(7),
            observedRevision: Revision(3),
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

        await controller.stop()
        await collector.stop()
        await seedClient.close()
        await seedServer.close()
        await server.close()
    }

    @Test("A superseded resume response cannot replay or rebind the outbox")
    func supersededResumeResponseIsIgnored() async throws {
        let (seedClient, seedServer) = await PipeTransport.createPair()
        let outbox = EventOutbox()
        let pending = try await outbox.sendActivate(
            nodeId: NodeId(7),
            observedRevision: Revision(3),
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

        await firstController.stop()
        await secondController.stop()
        await firstCollector.stop()
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
        _ = try await outbox.sendActivate(
            nodeId: NodeId(7),
            observedRevision: Revision(3),
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

        _ = try await outbox.sendActivate(
            nodeId: NodeId(7),
            observedRevision: Revision(3),
            via: client
        )
        #expect(await outbox.pendingCount == 1)

        // The server tracks `last_processed_event_seq` per client_instance_id and it outlives the
        // connection, while a fresh outbox restarts its own counter at zero. Honoring a mark the
        // outbox never issued would retire an event that was never acknowledged.
        await outbox.acknowledgeEvents(throughSeq: 5_000)

        #expect(await outbox.pendingCount == 1)
        #expect(await outbox.lastAckedEventSeq == 0)

        await client.close()
        await server.close()
    }

    @Test("An acknowledgement cannot overtake pending-event retention")
    func acknowledgementDuringSendDoesNotRequeueSettledEvent() async throws {
        let transport = GatedTransport()
        let outbox = EventOutbox()
        let controller = SessionController(transport: transport, outbox: outbox)

        let sendTask = Task {
            try await outbox.sendActivate(
                nodeId: NodeId(7),
                observedRevision: Revision(3),
                via: transport
            )
        }

        await transport.waitForSendCount(1)
        let frame = try #require(await transport.frame(at: 0))
        let message = try decodeFramedMessage(from: frame)
        let event = try #require(try events(in: [message]).first)

        await controller.handleIncomingMessage(HandshakeFixtures.welcomeMessage())

        var ack = SRUIServerEventAck()
        ack.clientInstanceID = outbox.clientInstanceId.bytes
        ack.eventID = event.eventId.bytes
        ack.lastProcessedEventSeq = event.eventSeq
        ack.status = .processed
        var ackMessage = SRUIMessage()
        ackMessage.serverEventAck = ack
        await controller.handleIncomingMessage(ackMessage)

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
        let first = try await outbox.sendActivate(
            nodeId: NodeId(1),
            observedRevision: Revision(3),
            via: seedClient
        )
        let second = try await outbox.sendActivate(
            nodeId: NodeId(2),
            observedRevision: Revision(3),
            via: seedClient
        )

        let transport = GatedTransport()
        let replayTask = Task {
            try await outbox.resendPendingEvents(via: transport)
        }
        await transport.waitForSendCount(1)

        let freshTask = Task {
            try await outbox.sendActivate(
                nodeId: NodeId(3),
                observedRevision: Revision(3),
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
        let event = try await outbox.sendActivate(
            nodeId: NodeId(7),
            observedRevision: Revision(3),
            via: client
        )

        await controller.handleIncomingMessage(HandshakeFixtures.welcomeMessage())

        var ack = SRUIServerEventAck()
        ack.clientInstanceID = ClientInstanceId(string: "client-b").bytes
        ack.eventID = event.eventId.bytes
        ack.lastProcessedEventSeq = event.eventSeq
        ack.status = .processed
        var message = SRUIMessage()
        message.serverEventAck = ack
        await controller.handleIncomingMessage(message)

        #expect(await outbox.pendingCount == 1)
        #expect(await outbox.lastAckedEventSeq == 0)

        await client.close()
        await server.close()
    }

    @Test("The resume handshake reports the last acknowledged event sequence (§18)")
    func handshakeReportsLastAckedEventSeq() async throws {
        let (client, server) = await PipeTransport.createPair()
        let collector = OutboxWireCollector()
        await collector.start(draining: server)

        let outbox = EventOutbox()
        let event = try await outbox.sendActivate(
            nodeId: NodeId(7),
            observedRevision: Revision(3),
            via: client
        )
        await outbox.acknowledgeEvent(id: event.eventId)

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
}
