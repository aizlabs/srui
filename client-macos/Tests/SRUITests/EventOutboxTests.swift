//
// EventOutboxTests.swift
// SRUITests
//
// Unit tests for EventOutbox semantic event queue and serialization (§7.7, §16, §18.2).
//

import Testing
import Foundation
import SemanticModel
import Protocol
import Session
import TransportSSH

@Suite("EventOutbox Tests")
struct EventOutboxTests {

    @Test("EventOutbox allocates monotonically increasing sequence numbers")
    func monotonicSequenceNumbers() async throws {
        let outbox = EventOutbox()

        let seq1 = await outbox.nextEventSeq()
        let seq2 = await outbox.nextEventSeq()
        let seq3 = await outbox.nextEventSeq()

        #expect(seq1 == 1)
        #expect(seq2 == 2)
        #expect(seq3 == 3)
    }

    @Test("EventOutbox generates unique retry-safe event IDs")
    func uniqueEventIds() async throws {
        let outbox = EventOutbox()

        let id1 = await outbox.generateEventId()
        let id2 = await outbox.generateEventId()

        #expect(!id1.isEmpty)
        #expect(!id2.isEmpty)
        #expect(id1 != id2)
    }

    @Test("EventOutbox creates and serializes ACTIVATE event")
    func activateEventSerialization() async throws {
        let clientInstanceId = ClientInstanceId(string: "client-test-42")
        let outbox = EventOutbox(clientInstanceId: clientInstanceId)

        let nodeId = NodeId(183)
        let observedRevision = Revision(104)

        let event = await outbox.makeActivateEvent(nodeId: nodeId, observedRevision: observedRevision)

        #expect(event.eventSeq == 1)
        #expect(event.nodeId == nodeId)
        #expect(event.observedRevision == observedRevision)
        #expect(event.eventType == .EVENT_ACTIVATE)
        #expect(event.clientInstanceId == clientInstanceId)

        // Verify wire encoding roundtrip
        var msg = SRUIMessage()
        msg.event = event.toWire()
        let framedBytes = try SRUIFraming.encodeFramed(msg)

        let decodedMsg = try decodeFramedMessage(from: framedBytes)
        guard case .event(let wireEvent) = decodedMsg.msg else {
            Issue.record("Expected event message payload")
            return
        }

        let decodedEvent = try ProtocolDecoder().validateAndConvertEvent(wire: wireEvent)
        #expect(decodedEvent.eventSeq == 1)
        #expect(decodedEvent.nodeId == nodeId)
        #expect(decodedEvent.observedRevision == observedRevision)
        #expect(decodedEvent.eventType == .EVENT_ACTIVATE)
        #expect(decodedEvent.clientInstanceId == clientInstanceId)
    }

    @Test("EventOutbox sendActivate transmits framed event over Transport")
    func sendActivateOverTransport() async throws {
        let (client, server) = await PipeTransport.createPair()
        let outbox = EventOutbox()

        let serverStream = server.receiveStream()

        let sendTask = Task {
            try await outbox.sendActivate(
                nodeId: NodeId(7),
                observedRevision: Revision(10),
                via: client
            )
        }

        var receivedData: Data?
        for try await chunk in serverStream {
            receivedData = chunk
            break
        }

        let sentEvent = try await sendTask.value
        let nonNilData = try #require(receivedData)

        let decodedMsg = try decodeFramedMessage(from: nonNilData)
        guard case .event(let wireEvent) = decodedMsg.msg else {
            Issue.record("Expected event message payload")
            return
        }
        let decodedEvent = try ProtocolDecoder().validateAndConvertEvent(wire: wireEvent)

        #expect(decodedEvent.eventSeq == sentEvent.eventSeq)
        #expect(decodedEvent.eventId == sentEvent.eventId)
        #expect(decodedEvent.nodeId == NodeId(7))
        #expect(decodedEvent.observedRevision == Revision(10))

        await client.close()
        await server.close()
    }
}
