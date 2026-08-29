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

/// Failure to place a new event inside the bounded, contiguous send window.
public enum EventOutboxError: Error, Equatable, Sendable {
    case sequenceWindowExhausted(limit: Int)
    case eventSequenceAlreadyAcknowledged(eventSeq: UInt64)
    case resumeNotConfirmed
}

/// Actor managing outbound semantic event generation, sequencing, and wire transmission.
///
/// Retry safety (§18.2): every application-side-effect event carries a stable `event_id` and
/// `event_seq`. Selective acknowledgements may settle later events first, but
/// `lastAckedEventSeq` advances only across a contiguous settled prefix, like a TCP cumulative
/// acknowledgement.
public actor EventOutbox {
    /// Maximum span between the contiguous ack frontier and the newest allocated event (§18.2).
    public static let defaultMaxPendingEvents = 256

    public nonisolated let clientInstanceId: ClientInstanceId
    private let maxPendingEvents: Int
    private var activeSessionId: String?
    private var activeResumeAttemptId: UUID?
    private var acceptsNewEvents = true
    private var currentEventSeq: UInt64 = 0
    private var pendingEvents: [EventId: Event] = [:]
    /// Send order of `pendingEvents`, so replay preserves allocation order.
    private var pendingOrder: [EventId] = []
    private var _lastAckedEventSeq: UInt64 = 0
    /// Selectively acknowledged sequences above the cumulative frontier.
    private var acknowledgedOutOfOrder: Set<UInt64> = []
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

    /// Highest contiguous event sequence acknowledged by the server (§18, §18.2).
    public var lastAckedEventSeq: UInt64 {
        _lastAckedEventSeq
    }

    /// Allocates the next sequence. Callers must send or retain the resulting event without
    /// abandoning it; the combined send APIs enforce the bounded window before allocation.
    public func nextEventSeq() -> UInt64 {
        currentEventSeq += 1
        return currentEventSeq
    }

    /// Generates a globally unique, retry-safe event identifier (§7.7, §18.2).
    public func generateEventId() -> EventId {
        EventId(string: UUID().uuidString)
    }

    /// Constructs a client-originated momentary activation event (§7.6, §7.7).
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

    /// Constructs a client-originated value-changed event (§7.6).
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

    /// Serializes and sends an event over the given transport (§16, §22).
    public func sendEvent(_ event: Event, via transport: any Transport) async throws {
        var msg = SRUIMessage()
        msg.event = event.toWire()
        let framedBytes = try SRUIFraming.encodeFramed(msg)

        // Retain before the first suspension: a fast acknowledgement may arrive while send is
        // awaiting transport completion and must be able to remove this entry exactly once.
        try retainPending(event)
        let send = enqueueSend {
            try await transport.send(data: framedBytes)
        }
        try await send.value
    }

    /// Constructs and sends an `ACTIVATE` event without creating a sequence beyond the window.
    @discardableResult
    public func sendActivate(nodeId: NodeId, observedRevision: Revision, via transport: any Transport) async throws -> Event {
        guard acceptsNewEvents else {
            throw EventOutboxError.resumeNotConfirmed
        }
        try ensureSequenceWindowCapacity()
        let event = makeActivateEvent(nodeId: nodeId, observedRevision: observedRevision)
        try await sendEvent(event, via: transport)
        return event
    }

    /// Replays every unacknowledged event in original send order with its original identity.
    ///
    /// Failures propagate to the caller so resume cannot enable new events until every retained
    /// write succeeds (§18.2).
    public func resendPendingEvents(via transport: any Transport) async throws {
        let replay = pendingOrder.compactMap { pendingEvents[$0] }
        let send = enqueueSend {
            for event in replay {
                var msg = SRUIMessage()
                msg.event = event.toWire()
                try await transport.send(data: try SRUIFraming.encodeFramed(msg))
            }
        }
        try await send.value
    }

    /// Selectively acknowledges one event ID. A later sequence does not cross an earlier gap.
    public func acknowledgeEvent(id: EventId) {
        guard let event = pendingEvents.removeValue(forKey: id) else { return }
        pendingOrder.removeAll { $0 == id }
        recordSelectiveAcknowledgement(event.eventSeq)
    }

    /// Starts a reconnect generation and prevents every controller sharing this outbox from
    /// allocating new events until that generation receives an authoritative decision.
    func beginResumeAttempt() -> UUID {
        let attemptId = UUID()
        activeResumeAttemptId = attemptId
        acceptsNewEvents = false
        return attemptId
    }

    /// Completes a same-session decision only if no newer controller superseded this attempt.
    func completeSameSessionResume(
        id: String,
        lastProcessedEventSeq: UInt64,
        attemptId: UUID,
        via transport: any Transport,
        enableNewEventsAfterReplay: Bool
    ) async throws -> Bool {
        guard activeResumeAttemptId == attemptId else { return false }
        activeSessionId = id
        acknowledgeEvents(throughSeq: lastProcessedEventSeq)
        try await resendPendingEvents(via: transport)
        guard activeResumeAttemptId == attemptId else { return false }
        acceptsNewEvents = enableNewEventsAfterReplay
        return true
    }

    /// Binds a fresh HELLO handshake that did not carry an old retry set.
    func confirmFreshSession(id: String) -> Bool {
        guard activeResumeAttemptId == nil else { return false }
        activeSessionId = id
        acceptsNewEvents = true
        return true
    }

    /// Applies one selective acknowledgement plus the server's contiguous cumulative frontier.
    /// Returns false when a draining connection delivers an ack from an expired incarnation.
    @discardableResult
    public func settleAcknowledgement(
        eventId: EventId,
        throughSeq seq: UInt64,
        sessionId: String? = nil
    ) -> Bool {
        if let sessionId, let activeSessionId, sessionId != activeSessionId {
            return false
        }
        acknowledgeEvents(throughSeq: seq)
        acknowledgeEvent(id: eventId)
        return true
    }

    /// Acknowledges every event through the server's highest contiguous settled sequence.
    public func acknowledgeEvents(throughSeq seq: UInt64) {
        guard seq > _lastAckedEventSeq, seq <= currentEventSeq else { return }

        _lastAckedEventSeq = seq
        acknowledgedOutOfOrder = Set(acknowledgedOutOfOrder.filter { $0 > seq })
        for (id, event) in pendingEvents where event.eventSeq <= seq {
            pendingEvents.removeValue(forKey: id)
        }
        pendingOrder.removeAll { pendingEvents[$0] == nil }
        advanceContiguousAcknowledgement()
    }

    /// Abandons every intent from an expired session and aligns sequencing with the
    /// authoritative replacement session's receive frontier (§18).
    func prepareReplacedSession(
        id: String,
        lastProcessedEventSeq: UInt64,
        attemptId: UUID
    ) -> Bool {
        guard activeResumeAttemptId == attemptId else { return false }
        activeSessionId = id
        acceptsNewEvents = false
        currentEventSeq = lastProcessedEventSeq
        _lastAckedEventSeq = lastProcessedEventSeq
        pendingEvents.removeAll(keepingCapacity: true)
        pendingOrder.removeAll(keepingCapacity: true)
        acknowledgedOutOfOrder.removeAll(keepingCapacity: true)
        sendTail = nil
        return true
    }

    /// Enables new events only after the snapshot for the current reconnect generation commits.
    func finishResync(attemptId: UUID) -> Bool {
        guard activeResumeAttemptId == attemptId else { return false }
        acceptsNewEvents = true
        return true
    }

    /// Returns the count of events still requiring replay.
    public var pendingCount: Int {
        pendingEvents.count
    }

    private func recordSelectiveAcknowledgement(_ eventSeq: UInt64) {
        guard eventSeq > _lastAckedEventSeq, eventSeq <= currentEventSeq else { return }
        acknowledgedOutOfOrder.insert(eventSeq)
        advanceContiguousAcknowledgement()
    }

    private func advanceContiguousAcknowledgement() {
        while _lastAckedEventSeq < currentEventSeq {
            let next = _lastAckedEventSeq + 1
            guard acknowledgedOutOfOrder.remove(next) != nil else { break }
            _lastAckedEventSeq = next
        }
    }

    private func ensureSequenceWindowCapacity() throws {
        let outstandingSpan = currentEventSeq - _lastAckedEventSeq
        guard currentEventSeq < UInt64.max,
              outstandingSpan < UInt64(maxPendingEvents) else {
            throw EventOutboxError.sequenceWindowExhausted(limit: maxPendingEvents)
        }
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

    /// Retains a new event without evicting an earlier unacknowledged sequence.
    private func retainPending(_ event: Event) throws {
        if pendingEvents[event.eventId] != nil {
            pendingEvents[event.eventId] = event
            return
        }
        guard event.eventSeq > _lastAckedEventSeq else {
            throw EventOutboxError.eventSequenceAlreadyAcknowledged(eventSeq: event.eventSeq)
        }
        guard event.eventSeq - _lastAckedEventSeq <= UInt64(maxPendingEvents) else {
            throw EventOutboxError.sequenceWindowExhausted(limit: maxPendingEvents)
        }

        pendingEvents[event.eventId] = event
        pendingOrder.append(event.eventId)
    }
}
