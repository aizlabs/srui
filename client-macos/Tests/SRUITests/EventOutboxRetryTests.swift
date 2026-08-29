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
import Session
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
        await outbox.resendPendingEvents(via: client)

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
        await outbox.resendPendingEvents(via: client)

        await client.close()
        await server.close()
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
        await outbox.resendPendingEvents(via: client)

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
            var ack = SRUIServerEventAck()
            ack.clientInstanceID = outbox.clientInstanceId.bytes
            ack.eventID = event.eventId.bytes
            ack.lastProcessedEventSeq = event.eventSeq
            ack.status = .processed
            var message = SRUIMessage()
            message.serverEventAck = ack
            await controller.handleIncomingMessage(message)
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
