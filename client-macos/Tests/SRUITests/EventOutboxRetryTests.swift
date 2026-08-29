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
