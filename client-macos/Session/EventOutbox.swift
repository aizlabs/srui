//
// EventOutbox.swift
// Session
//
// Outbound semantic event queue, monotonic sequence tracking, and retry-safe event dispatch (§7.7, §16, §18.2, §22).
//

import Foundation
import SemanticModel
import Protocol
import TransportSSH

/// Actor managing outbound semantic event generation, monotonic sequencing, and wire transmission (§7.7, §16, §18.2, §22).
public actor EventOutbox {
    public nonisolated let clientInstanceId: ClientInstanceId
    private var currentEventSeq: UInt64 = 0
    private var pendingEvents: [EventId: Event] = [:]

    public init(clientInstanceId: ClientInstanceId = ClientInstanceId(string: UUID().uuidString)) {
        self.clientInstanceId = clientInstanceId
    }

    /// Returns the current monotonic event sequence number.
    public var eventSeq: UInt64 {
        currentEventSeq
    }

    /// Allocates the next monotonic event sequence number (§7.7, §16).
    public func nextEventSeq() -> UInt64 {
        currentEventSeq += 1
        return currentEventSeq
    }

    /// Generates a globally unique, retry-safe event identifier (§7.7, §18.2).
    public func generateEventId() -> EventId {
        EventId(string: UUID().uuidString)
    }

    /// Constructs a client-originated momentary activation event (`ACTIVATE`, §7.6, §7.7).
    public func makeActivateEvent(nodeId: NodeId, observedRevision: Revision) -> Event {
        let seq = nextEventSeq()
        let id = generateEventId()
        return Event.activate(
            eventSeq: seq,
            eventId: id,
            observedRevision: observedRevision,
            nodeId: nodeId
        ).withClientInstanceId(clientInstanceId)
    }

    /// Constructs a client-originated value changed event (`VALUE_CHANGED`, §7.6).
    public func makeValueChangedEvent(nodeId: NodeId, observedRevision: Revision, value: Value) -> Event {
        let seq = nextEventSeq()
        let id = generateEventId()
        return Event.valueChanged(
            eventSeq: seq,
            eventId: id,
            observedRevision: observedRevision,
            nodeId: nodeId,
            value: value
        ).withClientInstanceId(clientInstanceId)
    }

    /// Serializes and sends an [`Event`] over the given transport (§16, §22).
    public func sendEvent(_ event: Event, via transport: any Transport) async throws {
        var msg = SRUIMessage()
        msg.event = event.toWire()
        let framedBytes = try SRUIFraming.encodeFramed(msg)
        try await transport.send(data: framedBytes)
        pendingEvents[event.eventId] = event
    }

    /// Constructs and sends an `ACTIVATE` event in one atomic operation.
    @discardableResult
    public func sendActivate(nodeId: NodeId, observedRevision: Revision, via transport: any Transport) async throws -> Event {
        let event = makeActivateEvent(nodeId: nodeId, observedRevision: observedRevision)
        try await sendEvent(event, via: transport)
        return event
    }

    /// Acknowledges event delivery by sequence number or event ID.
    public func acknowledgeEvent(id: EventId) {
        pendingEvents.removeValue(forKey: id)
    }

    /// Returns the count of pending unacknowledged events.
    public var pendingCount: Int {
        pendingEvents.count
    }
}
