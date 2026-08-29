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
@testable import Session
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

    @Test("EventOutbox creates and serializes VALUE_CHANGED event")
    func valueChangedEventSerialization() async throws {
        let clientInstanceId = ClientInstanceId(string: "client-test-val")
        let outbox = EventOutbox(clientInstanceId: clientInstanceId)

        let nodeId = NodeId(200)
        let observedRevision = Revision(50)
        let value = Value.bool(true)

        let event = await outbox.makeValueChangedEvent(nodeId: nodeId, observedRevision: observedRevision, value: value)

        #expect(event.eventSeq == 1)
        #expect(event.nodeId == nodeId)
        #expect(event.observedRevision == observedRevision)
        #expect(event.eventType == .EVENT_VALUE_CHANGED)
        #expect(event.boolArg == true)
        #expect(event.clientInstanceId == clientInstanceId)

        // Verify wire roundtrip
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
        #expect(decodedEvent.eventType == .EVENT_VALUE_CHANGED)
        #expect(decodedEvent.boolArg == true)
    }

    @Test("EventOutbox creates and serializes SELECTION_CHANGED event")
    func selectionChangedEventSerialization() async throws {
        let clientInstanceId = ClientInstanceId(string: "client-test-sel")
        let outbox = EventOutbox(clientInstanceId: clientInstanceId)

        let nodeId = NodeId(300)
        let observedRevision = Revision(75)
        let itemId = ItemId(999)

        let event = await outbox.makeSelectionChangedEvent(nodeId: nodeId, observedRevision: observedRevision, itemId: itemId)

        #expect(event.eventSeq == 1)
        #expect(event.nodeId == nodeId)
        #expect(event.observedRevision == observedRevision)
        #expect(event.eventType == .EVENT_SELECTION_CHANGED)
        #expect(event.itemIdArg == itemId)
        #expect(event.clientInstanceId == clientInstanceId)

        // Verify wire roundtrip
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
        #expect(decodedEvent.eventType == .EVENT_SELECTION_CHANGED)
        #expect(decodedEvent.itemIdArg == itemId)
    }

    @Test("Mixed event types share contiguous monotonic sequences and are retained")
    func mixedEventTypesContiguousSequences() async throws {
        let (client, server) = await PipeTransport.createPair()
        let outbox = EventOutbox()

        let ev1 = try await outbox.sendActivate(nodeId: NodeId(1), observedRevision: Revision(1), via: client)
        let ev2 = try await outbox.sendValueChanged(nodeId: NodeId(2), observedRevision: Revision(1), value: .bool(true), via: client)
        let ev3 = try await outbox.sendSelectionChanged(nodeId: NodeId(3), observedRevision: Revision(1), itemId: ItemId(42), via: client)

        #expect(ev1.eventSeq == 1)
        #expect(ev2.eventSeq == 2)
        #expect(ev3.eventSeq == 3)

        #expect(await outbox.eventSeq == 3)
        #expect(await outbox.pendingCount == 3)

        await client.close()
        await server.close()
    }

    @Test("Permission and window capacity behavior across event send APIs")
    func permissionAndCapacityBehavior() async throws {
        let (client, server) = await PipeTransport.createPair()
        let outbox = EventOutbox(maxPendingEvents: 2)

        // 1. Fill capacity (2 events)
        _ = try await outbox.sendActivate(nodeId: NodeId(1), observedRevision: Revision(1), via: client)
        _ = try await outbox.sendValueChanged(nodeId: NodeId(2), observedRevision: Revision(1), value: .bool(true), via: client)

        // 3rd event should throw sequenceWindowExhausted
        await #expect(throws: EventOutboxError.sequenceWindowExhausted(limit: 2)) {
            try await outbox.sendSelectionChanged(nodeId: NodeId(3), observedRevision: Revision(1), itemId: ItemId(10), via: client)
        }

        // 2. Suspended outbox throws resumeNotConfirmed
        await outbox.suspendNewEvents()
        await #expect(throws: EventOutboxError.resumeNotConfirmed) {
            try await outbox.sendActivate(nodeId: NodeId(1), observedRevision: Revision(1), via: client)
        }
        await #expect(throws: EventOutboxError.resumeNotConfirmed) {
            try await outbox.sendValueChanged(nodeId: NodeId(2), observedRevision: Revision(1), value: .bool(true), via: client)
        }
        await #expect(throws: EventOutboxError.resumeNotConfirmed) {
            try await outbox.sendSelectionChanged(nodeId: NodeId(3), observedRevision: Revision(1), itemId: ItemId(10), via: client)
        }

        await client.close()
        await server.close()
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
