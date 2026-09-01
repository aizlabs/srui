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
    private var pendingEventReplayLoop: PendingEventReplayLoop
    private var replayLease: PendingEventReplayLoop.Lease?
    private var activeSessionId: String?
    /// Resume-handshake latch used only to reject superseded server decisions. It is cleared
    /// once RESUME_OK commits, or after a required snapshot finishes.
    ///
    /// Generations are local to this outbox and strictly increasing, so a decision is not merely
    /// "equal or not": it is active, already superseded, or never issued at all (§18).
    private var activeResumeGeneration: UInt64?
    /// Strictly increasing source of resume generations; never reused within this outbox (§18).
    private var lastIssuedResumeGeneration: UInt64 = 0
    /// Ownership retained between outbox completion and the controller committing its state.
    private var pendingResumeFinalizationGeneration: UInt64?
    private var acceptsNewEvents = true
    private var currentEventSeq: UInt64 = 0
    private var pendingEvents: [EventId: Event] = [:]
    /// Send order of `pendingEvents`, so replay preserves allocation order.
    private var pendingOrder: [EventId] = []
    private var _lastAckedEventSeq: UInt64 = 0
    /// Selectively acknowledged sequences above the cumulative frontier.
    private var acknowledgedOutOfOrder: Set<UInt64> = []
    /// Tail of the FIFO transport-write chain. Actor isolation alone is insufficient because
    /// `transport.send` is a reentrancy point; each new write task awaits this tail. Retained as
    /// the writer itself (not a result-swallowing wrapper) so an abandoned generation can cancel
    /// it instead of merely dropping the reference (§18).
    private var sendTail: Task<Void, any Error>?

    public init(
        clientInstanceId: ClientInstanceId = ClientInstanceId(string: UUID().uuidString),
        maxPendingEvents: Int = EventOutbox.defaultMaxPendingEvents
    ) {
        self.clientInstanceId = clientInstanceId
        self.maxPendingEvents = max(1, maxPendingEvents)
        self.pendingEventReplayLoop = PendingEventReplayLoop()
    }

    init(
        clientInstanceId: ClientInstanceId = ClientInstanceId(string: UUID().uuidString),
        maxPendingEvents: Int = EventOutbox.defaultMaxPendingEvents,
        replayRetryInitialDelay: Duration,
        replayRetryMaximumDelay: Duration
    ) {
        self.clientInstanceId = clientInstanceId
        self.maxPendingEvents = max(1, maxPendingEvents)
        self.pendingEventReplayLoop = PendingEventReplayLoop(
            initialDelay: replayRetryInitialDelay,
            maximumDelay: replayRetryMaximumDelay
        )
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

    /// Constructs a client-originated selection-changed event (§7.6).
    public func makeSelectionChangedEvent(nodeId: NodeId, observedRevision: Revision, itemId: ItemId) -> Event {
        let seq = nextEventSeq()
        let id = generateEventId()
        return Event.selectionChanged(
            eventSeq: seq,
            eventId: id,
            observedRevision: observedRevision,
            nodeId: nodeId,
            itemId: itemId
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

    /// Constructs and sends a `VALUE_CHANGED` event without creating a sequence beyond the window.
    @discardableResult
    public func sendValueChanged(nodeId: NodeId, observedRevision: Revision, value: Value, via transport: any Transport) async throws -> Event {
        guard acceptsNewEvents else {
            throw EventOutboxError.resumeNotConfirmed
        }
        try ensureSequenceWindowCapacity()
        let event = makeValueChangedEvent(nodeId: nodeId, observedRevision: observedRevision, value: value)
        try await sendEvent(event, via: transport)
        return event
    }

    /// Constructs and sends a `SELECTION_CHANGED` event without creating a sequence beyond the window.
    @discardableResult
    public func sendSelectionChanged(nodeId: NodeId, observedRevision: Revision, itemId: ItemId, via transport: any Transport) async throws -> Event {
        guard acceptsNewEvents else {
            throw EventOutboxError.resumeNotConfirmed
        }
        try ensureSequenceWindowCapacity()
        let event = makeSelectionChangedEvent(nodeId: nodeId, observedRevision: observedRevision, itemId: itemId)
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
                try Task.checkCancellation()
                var msg = SRUIMessage()
                msg.event = event.toWire()
                try await transport.send(data: try SRUIFraming.encodeFramed(msg))
            }
        }
        try await withTaskCancellationHandler {
            try await send.value
        } onCancel: {
            send.cancel()
        }
    }

    /// Selectively acknowledges one event ID. A later sequence does not cross an earlier gap.
    public func acknowledgeEvent(id: EventId) {
        guard let event = pendingEvents.removeValue(forKey: id) else { return }
        pendingOrder.removeAll { $0 == id }
        recordSelectiveAcknowledgement(event.eventSeq)
        cancelReplayRetryLoopIfSettled()
    }

    /// Starts a reconnect generation and prevents every controller sharing this outbox from
    /// allocating new events until that generation receives an authoritative decision.
    ///
    /// Issuing a generation immediately supersedes every older attempt, so a delayed response
    /// from an abandoned connection is discarded rather than replayed (§18).
    func beginResumeAttempt() -> UInt64 {
        cancelReplayRetryLoop()
        cancelPendingWrites()
        pendingResumeFinalizationGeneration = nil
        lastIssuedResumeGeneration += 1
        activeResumeGeneration = lastIssuedResumeGeneration
        acceptsNewEvents = false
        return lastIssuedResumeGeneration
    }

    /// Whether this outbox ever minted `generation` (§18).
    ///
    /// Generations are strictly increasing, so a value above the last issued one cannot be a
    /// stale decision from an abandoned attempt — it was never issued at all, which means the
    /// caller is bound to a different outbox than the one that minted it (§4 inv. 13).
    func hasIssuedResumeGeneration(_ generation: UInt64) -> Bool {
        generation >= 1 && generation <= lastIssuedResumeGeneration
    }

    /// Runs `body` only while `generation` still owns the reconnect decision (§18).
    ///
    /// The check and `body` share this single actor-isolated critical section, so a concurrent
    /// `beginResumeAttempt()` cannot slip between them. A caller that checked first and applied
    /// after a suspension would still let a superseded snapshot reach the shared replica.
    ///
    /// A `nil` generation means "this controller has no attempt outstanding", which is the
    /// live-resync case: it matches only while no other controller holds the latch either.
    func withUnsupersededResume<T: Sendable>(
        _ generation: UInt64?,
        _ body: @Sendable () -> T
    ) -> T? {
        guard activeResumeGeneration == generation else { return nil }
        return body()
    }

    /// Completes a same-session decision only if no newer controller superseded this attempt.
    func completeSameSessionResume(
        id: String,
        lastProcessedEventSeq: UInt64,
        generation: UInt64,
        via transport: any Transport,
        enableNewEventsAfterReplay: Bool,
        onReplayFailure: (@Sendable (String) async -> Void)? = nil
    ) async throws -> Bool {
        guard activeResumeGeneration == generation else { return false }
        activeSessionId = id
        acknowledgeEvents(throughSeq: lastProcessedEventSeq)
        try await resendPendingEvents(via: transport)
        guard activeResumeGeneration == generation else { return false }
        acceptsNewEvents = enableNewEventsAfterReplay
        startReplayRetryLoop(
            generation: generation,
            via: transport,
            onFailure: onReplayFailure
        )
        if enableNewEventsAfterReplay {
            activeResumeGeneration = nil
            pendingResumeFinalizationGeneration = generation
        }
        return true
    }

    /// Binds a fresh HELLO handshake that did not carry an old retry set.
    func confirmFreshSession(id: String) -> Bool {
        guard activeResumeGeneration == nil else { return false }
        // The current HELLO generation owns this decision: retire the prior transport lease while
        // retaining unsettled intents for explicit acknowledgement or a later resume.
        cancelReplayRetryLoop()
        pendingResumeFinalizationGeneration = nil
        activeSessionId = id
        acceptsNewEvents = true
        return true
    }

    /// Blocks new event allocation until a bootstrap or resync snapshot commits (§15, §18).
    func suspendNewEvents() {
        acceptsNewEvents = false
    }

    /// Applies the event frontier for a live-session resync without an outstanding resume attempt.
    ///
    /// Does not cancel the replay retry loop: unsettled events keep retrying until settlement
    /// advances the frontier (§18.2).
    func applyLiveResyncFrontier(lastProcessedEventSeq: UInt64) {
        acceptsNewEvents = false
        acknowledgeEvents(throughSeq: lastProcessedEventSeq)
    }

    /// Abandons pending intents and binds a replacement incarnation without a resume attempt (§18).
    func applyReplacementFrontier(id: String, lastProcessedEventSeq: UInt64) {
        cancelReplayRetryLoop()
        pendingResumeFinalizationGeneration = nil
        activeSessionId = id
        acceptsNewEvents = false
        currentEventSeq = lastProcessedEventSeq
        _lastAckedEventSeq = lastProcessedEventSeq
        pendingEvents.removeAll(keepingCapacity: true)
        pendingOrder.removeAll(keepingCapacity: true)
        acknowledgedOutOfOrder.removeAll(keepingCapacity: true)
        cancelPendingWrites()
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
        cancelReplayRetryLoopIfSettled()
    }

    /// Abandons every intent from an expired session and aligns sequencing with the
    /// authoritative replacement session's receive frontier (§18).
    func prepareReplacedSession(
        id: String,
        lastProcessedEventSeq: UInt64,
        generation: UInt64
    ) -> Bool {
        guard activeResumeGeneration == generation else { return false }
        applyReplacementFrontier(id: id, lastProcessedEventSeq: lastProcessedEventSeq)
        return true
    }

    /// Re-enables allocation after a HELLO catch-up snapshot with no resume attempt (§15, §18).
    ///
    /// Refuses while any controller holds an outstanding reconnect generation: that latch exists
    /// precisely to keep new events from being allocated before the continuity decision arrives,
    /// and re-opening it from an unrelated catch-up would bypass it (§18).
    @discardableResult
    func allowNewEvents() -> Bool {
        guard activeResumeGeneration == nil else { return false }
        acceptsNewEvents = true
        return true
    }

    /// Enables new events only after the snapshot for the current reconnect generation commits.
    func finishResync(generation: UInt64) -> Bool {
        guard activeResumeGeneration == generation else { return false }
        acceptsNewEvents = true
        activeResumeGeneration = nil
        pendingResumeFinalizationGeneration = generation
        return true
    }

    /// Returns the count of events still requiring replay.
    public var pendingCount: Int {
        pendingEvents.count
    }

    /// Whether retry scheduling is active for unsettled events. This becomes false as soon
    /// as the lease is invalidated, while an in-flight send may still be unwinding cancellation.
    var isRetryingPendingEvents: Bool {
        pendingEventReplayLoop.isRunning
    }

    /// Releases the short ownership window after controller state commits.
    func commitResumeWork(generation: UInt64) {
        guard pendingResumeFinalizationGeneration == generation else { return }
        pendingResumeFinalizationGeneration = nil
    }

    /// Cancels only handshake/finalization/retry work owned by this resume generation.
    ///
    /// Scoped by ownership, so a controller that was already superseded cannot release the latch
    /// a newer attempt holds and strand it with every decision rejected (§18).
    func stopResumeWork(generation: UInt64) {
        guard ownsResumeWork(generation: generation) else { return }
        if activeResumeGeneration == generation {
            activeResumeGeneration = nil
        }
        if pendingResumeFinalizationGeneration == generation {
            pendingResumeFinalizationGeneration = nil
        }
        if replayLease?.resumeScope == generation {
            cancelReplayRetryLoop()
        }
        acceptsNewEvents = false
    }

    private func ownsResumeWork(generation: UInt64) -> Bool {
        activeResumeGeneration == generation
            || pendingResumeFinalizationGeneration == generation
            || replayLease?.resumeScope == generation
    }

    private func startReplayRetryLoop(
        generation: UInt64,
        via transport: any Transport,
        onFailure: (@Sendable (String) async -> Void)?
    ) {
        cancelReplayRetryLoop()
        guard activeResumeGeneration == generation, pendingEvents.isEmpty == false else { return }

        // The transport is valid for this lease's lifetime. SessionController invalidates resume
        // work before replacing or closing the transport; the replay loop never owns teardown.
        replayLease = pendingEventReplayLoop.start(
            resumeScope: generation,
            replay: { [weak self] lease in
                guard let self else { return false }
                return try await self.replayPendingEvents(for: lease, via: transport)
            },
            onFailure: onFailure,
            onFinish: { [weak self] lease in
                await self?.finishReplayRetryLoop(lease)
            }
        )
    }

    private func replayPendingEvents(
        for lease: PendingEventReplayLoop.Lease,
        via transport: any Transport
    ) async throws -> Bool {
        guard replayLease == lease,
              pendingEventReplayLoop.isActive(lease),
              pendingEvents.isEmpty == false else {
            return false
        }
        try Task.checkCancellation()
        try await resendPendingEvents(via: transport)
        guard replayLease == lease,
              pendingEventReplayLoop.isActive(lease),
              Task.isCancelled == false else {
            return false
        }
        return pendingEvents.isEmpty == false
    }

    /// Natural completion only: cancellation clears `replayLease` in `cancelReplayRetryLoop()` first,
    /// so a stale `onFinish` from an invalidated task exits on the guard below.
    private func finishReplayRetryLoop(_ lease: PendingEventReplayLoop.Lease) {
        guard replayLease == lease else { return }
        replayLease = nil
        pendingEventReplayLoop.finish(lease)
    }

    private func cancelReplayRetryLoopIfSettled() {
        guard pendingEvents.isEmpty else { return }
        cancelReplayRetryLoop()
    }

    private func cancelReplayRetryLoop() {
        let lease = replayLease
        replayLease = nil
        if let lease {
            pendingEventReplayLoop.invalidate(lease)
        } else {
            pendingEventReplayLoop.invalidate()
        }
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
            // A failed or cancelled predecessor must not abort this write: ordering is the
            // invariant here, not shared success.
            _ = try? await predecessor?.value
            try await operation()
        }
        sendTail = task
        return task
    }

    /// Cancels the outstanding transport-write chain and drops it (§18).
    ///
    /// A write already committed to a dead transport can never complete, so a new reconnect
    /// generation must not chain its replay behind it. Cancellation is cooperative: the replay
    /// loop checks it between frames, but a `transport.send` already in flight still runs to
    /// completion, so this bounds the overlap rather than eliminating it.
    private func cancelPendingWrites() {
        sendTail?.cancel()
        sendTail = nil
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
