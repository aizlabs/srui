//
// EventOutbox.swift
// Session
//
// Outbound semantic event queue, monotonic sequence tracking, and retry-safe event dispatch (§7.7, §16, §18.2, §22, §26).
//

import Foundation
import SemanticModel
import Protocol
import TransportSSH

/// Actor managing outbound semantic event generation, monotonic sequencing, and wire transmission (§7.7, §16, §18.2, §22).
///
/// Retry safety (§18.2): every application-side-effect event carries a stable `event_id`. When a
/// connection dies before the acknowledgement arrives, the event is replayed **with its original
/// `event_id`** so the server's dedupe cache recognizes it as a retry and returns the prior result
/// instead of re-running the action. Minting a fresh id on retry would turn one "Delete" into two.
public actor EventOutbox {
    /// Upper bound on unacknowledged events retained for replay (§18.2 "bounded" cache, §26).
    public static let defaultMaxPendingEvents = 256

    public nonisolated let clientInstanceId: ClientInstanceId
    private let maxPendingEvents: Int
    private var currentEventSeq: UInt64 = 0
    private var pendingEvents: [EventId: Event] = [:]
    /// Send order of `pendingEvents`, so replay preserves ordering and eviction drops the oldest.
    private var pendingOrder: [EventId] = []
    private var _lastAckedEventSeq: UInt64 = 0
    /// Tail of the FIFO transport-write chain. Actor isolation alone is insufficient because
    /// `transport.send` is a reentrancy point; each new write task awaits this tail.
    private var sendTail: Task<Void, Never>?

    public init(
        clientInstanceId: ClientInstanceId = ClientInstanceId(string: UUID().uuidString),
        maxPendingEvents: Int = EventOutbox.defaultMaxPendingEvents
    ) {
        self.clientInstanceId = clientInstanceId
        self.maxPendingEvents = max(1, maxPendingEvents)
    }

    /// Returns the current monotonic event sequence number.
    public var eventSeq: UInt64 {
        currentEventSeq
    }

    /// Highest event sequence acknowledged by the server, reported in `CLIENT RESUME` (§18).
    public var lastAckedEventSeq: UInt64 {
        _lastAckedEventSeq
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

        // Retain before the first suspension: a fast acknowledgement may arrive while send is
        // awaiting transport completion and must be able to remove this entry exactly once.
        retainPending(event)
        let send = enqueueSend {
            try await transport.send(data: framedBytes)
        }
        try await send.value
    }

    /// Constructs and sends an `ACTIVATE` event in one atomic operation.
    @discardableResult
    public func sendActivate(nodeId: NodeId, observedRevision: Revision, via transport: any Transport) async throws -> Event {
        let event = makeActivateEvent(nodeId: nodeId, observedRevision: observedRevision)
        try await sendEvent(event, via: transport)
        return event
    }

    /// Replays every unacknowledged event, in original send order and with its original `event_id`,
    /// after a transport was re-established (§18, §18.2).
    ///
    /// Re-delivery of the same `event_id` returns the prior acknowledgement/result on the server and
    /// does not re-run the action, which is what makes an ambiguous disconnect retry-safe.
    public func resendPendingEvents(via transport: any Transport) async {
        let replay = pendingOrder.compactMap { pendingEvents[$0] }
        let send = enqueueSend {
            for event in replay {
                var msg = SRUIMessage()
                msg.event = event.toWire()
                try await transport.send(data: try SRUIFraming.encodeFramed(msg))
            }
        }
        // A failed replay remains pending for the next resume.
        _ = try? await send.value
    }

    /// Acknowledges event delivery by event ID.
    public func acknowledgeEvent(id: EventId) {
        guard let event = pendingEvents.removeValue(forKey: id) else { return }
        pendingOrder.removeAll { $0 == id }
        _lastAckedEventSeq = max(_lastAckedEventSeq, event.eventSeq)
    }

    /// Applies one server acknowledgement: bulk-drains by cumulative seq, then settles by id (§18.2).
    ///
    /// Cumulative `last_processed_event_seq` must be applied before per-id settlement so a guard
    /// on `_lastAckedEventSeq` cannot skip retiring lower-sequence pending events.
    public func settleAcknowledgement(eventId: EventId, throughSeq seq: UInt64) {
        acknowledgeEvents(throughSeq: seq)
        acknowledgeEvent(id: eventId)
    }

    /// Acknowledges every event up to and including `seq` (§18: `last_acked_event_seq`).
    ///
    /// A sequence beyond `currentEventSeq` is ignored. The server's `last_processed_event_seq` is
    /// cumulative per `client_instance_id` and outlives the connection, while a freshly constructed
    /// `EventOutbox` restarts its own counter at zero; honoring a stale-high mark would retire
    /// events this outbox has only just sent and that were never acknowledged.
    public func acknowledgeEvents(throughSeq seq: UInt64) {
        guard seq > _lastAckedEventSeq, seq <= currentEventSeq else { return }
        _lastAckedEventSeq = seq
        for (id, event) in pendingEvents where event.eventSeq <= seq {
            pendingEvents.removeValue(forKey: id)
        }
        pendingOrder.removeAll { pendingEvents[$0] == nil }
    }

    /// Returns the count of pending unacknowledged events.
    public var pendingCount: Int {
        pendingEvents.count
    }

    private func enqueueSend(
        _ operation: @escaping @Sendable () async throws -> Void
    ) -> Task<Void, any Error> {
        let predecessor = sendTail
        let task = Task {
            await predecessor?.value
            try await operation()
        }
        sendTail = Task {
            _ = try? await task.value
        }
        return task
    }

    /// Records a sent event for retry, evicting the oldest entries beyond the configured bound.
    private func retainPending(_ event: Event) {
        if pendingEvents.updateValue(event, forKey: event.eventId) == nil {
            pendingOrder.append(event.eventId)
        }
        while pendingOrder.count > maxPendingEvents {
            let evicted = pendingOrder.removeFirst()
            let dropped = pendingEvents.removeValue(forKey: evicted)
            // §26 bounds "maximum pending unacknowledged events", but an eviction here discards an
            // event that may never have been processed, so it must be visible rather than silent.
            SessionDiagnostics.error(
                "Pending event outbox full at \(maxPendingEvents); dropping unacknowledged event \(evicted) (seq \(dropped?.eventSeq ?? 0))"
            )
        }
    }
}
