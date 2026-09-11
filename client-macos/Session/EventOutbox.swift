//
// EventOutbox.swift
// Session
//
// Outbound semantic event queue, monotonic sequence tracking, and retry-safe event dispatch (§7.7, §16, §18.2, §18.3, §22, §22.6, §26).
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
    /// A retained `event_id` was reused for a different sequence or payload (§18.2).
    case pendingEventIdentityConflict(eventId: EventId)
    /// Same-session resync discard list did not match assigned TEXT_EDIT identities (§18.3).
    case textEditDiscardMismatch
}

typealias TextEditCancellationHandler =
    @MainActor @Sendable ([PendingTextEditDescriptor]) -> Void
typealias ReplacementTextEditingResetHandler =
    @MainActor @Sendable (EventOutboxSessionIncarnation) -> Void
typealias TextAcknowledgementResolutionHandler =
    @MainActor @Sendable ([TextEditAcknowledgementBarrier]) -> Void

/// Assigned, unacknowledged `TEXT_EDIT` identity declared on resume (§18.3).
public struct PendingTextEditDescriptor: Hashable, Equatable, Sendable {
    public var eventId: EventId
    public var eventSeq: UInt64
    public var nodeId: NodeId
    public var editSeq: EditSeq

    public init(eventId: EventId, eventSeq: UInt64, nodeId: NodeId, editSeq: EditSeq) {
        self.eventId = eventId
        self.eventSeq = eventSeq
        self.nodeId = nodeId
        self.editSeq = editSeq
    }

    public func toWire() -> SRUIPendingTextEditRef {
        var ref = SRUIPendingTextEditRef()
        ref.eventID = eventId.bytes
        ref.eventSeq = eventSeq
        ref.nodeID = nodeId.value
        ref.editSeq = editSeq.rawValue
        return ref
    }

    public init?(wire: SRUIPendingTextEditRef) {
        guard let seq = EditSeq(wire.editSeq),
              let eventID = try? validateAndConvertEventID(wire.eventID),
              wire.eventSeq > 0 else {
            return nil
        }
        self.init(
            eventId: eventID,
            eventSeq: wire.eventSeq,
            nodeId: NodeId(wire.nodeID),
            editSeq: seq
        )
    }
}

/// Outcome of one identity-checked acknowledgement (§18.2, §22.6).
public struct EventAcknowledgementSettlement: Equatable, Sendable {
    /// Whether the controller connection that delivered the acknowledgement still owns the outbox.
    public var connectionBound: Bool
    public var bound: Bool
    /// Event named by the acknowledgement's ID or explicit sequence slot, when still retained.
    public var event: Event?
    /// Every event retired by the cumulative frontier or selective acknowledgement.
    public var settledEvents: [Event]

    public static let staleConnection = EventAcknowledgementSettlement(
        connectionBound: false,
        bound: false,
        event: nil,
        settledEvents: []
    )

    public static let unbound = EventAcknowledgementSettlement(
        connectionBound: true,
        bound: false,
        event: nil,
        settledEvents: []
    )
}

/// A terminal text acknowledgement whose authoritative revision has not necessarily rendered yet.
struct TextEditAcknowledgementBarrier: Equatable, Sendable {
    var nodeId: NodeId
    var eventId: EventId
    var revisionAfterEffect: UInt64
    var rejected: Bool
}

/// A committed snapshot must own the synchronous native render before its text boundary may apply.
struct ResyncSnapshotCommit<Value: Sendable>: Sendable {
    var result: Value
    var renderToken: UUID?
    var sessionIncarnation: EventOutboxSessionIncarnation?
}

struct LiveRenderCommit<Value: Sendable>: Sendable {
    var result: Value
    var renderToken: UUID?
}

private final class EventOutboxConnectionBindingEpochAllocator: @unchecked Sendable {
    static let shared = EventOutboxConnectionBindingEpochAllocator()

    private let lock = NSLock()
    private var lastIssuedEpoch: UInt64 = 0

    private init() {}

    func next() -> UInt64 {
        lock.withLock {
            precondition(
                lastIssuedEpoch < UInt64.max,
                "EventOutbox connection binding epoch exhausted"
            )
            lastIssuedEpoch += 1
            return lastIssuedEpoch
        }
    }
}

/// Process-wide monotonic ownership of one SessionController transport binding.
public struct EventOutboxConnectionBinding: Hashable, Sendable {
    fileprivate var epoch: UInt64

    /// Globally ordered epoch for binding external per-connection ownership leases.
    var resourceOwnershipEpoch: UInt64 { epoch }
}

/// Opaque authority for one server-session incarnation within a connection binding.
///
/// A replacement session can deliberately retain the same transport and connection binding, so
/// the binding alone cannot reject an interaction Task admitted by the expired incarnation.
public struct EventOutboxSessionIncarnation: Hashable, Sendable {
    fileprivate var binding: EventOutboxConnectionBinding
    fileprivate var sequence: UInt64
}

/// Retained TEXT_EDIT waiting for native assignment authorization before its FIFO send slot opens.
struct PreparedTextEdit: Sendable {
    var event: Event
    fileprivate var token: UUID
}

/// Actor-validated same-session state handed to MainActor before replay opens.
struct SameSessionResumePreparation: Sendable {
    var sessionId: String
    var generation: UInt64
    var binding: EventOutboxConnectionBinding
    var sessionIncarnation: EventOutboxSessionIncarnation
    /// Text edits settled by the cumulative frontier and therefore forbidden to replay (§18.2).
    var frontierSettledTextEdits: [Event]
    var assignedTextEdits: [Event]
}

private final class PreparedTextEditSendGate: @unchecked Sendable {
    private let lock = NSLock()
    private var decision: Bool?
    private var waiter: CheckedContinuation<Bool, Never>?

    func wait() async -> Bool {
        await withTaskCancellationHandler {
            await withCheckedContinuation { continuation in
                let decided = lock.withLock { () -> Bool? in
                    if let decision { return decision }
                    waiter = continuation
                    return nil
                }
                if let decided {
                    continuation.resume(returning: decided)
                }
            }
        } onCancel: {
            self.resolve(shouldSend: false)
        }
    }

    @discardableResult
    func resolve(shouldSend: Bool) -> Bool {
        var didResolve = false
        let continuation = lock.withLock { () -> CheckedContinuation<Bool, Never>? in
            guard decision == nil else { return nil }
            decision = shouldSend
            didResolve = true
            defer { waiter = nil }
            return waiter
        }
        continuation?.resume(returning: shouldSend)
        return didResolve
    }

    func cancelIfUnresolved() -> Bool {
        resolve(shouldSend: false)
    }
}

private struct PreparedTextEditSend: Sendable {
    var event: Event
    var gate: PreparedTextEditSendGate
    var task: Task<Void, any Error>
}

/// Result of applying a live same-session resync decision.
enum LiveSameSessionResyncDecision: Sendable {
    /// The server frontier proves every canceled edit reached the server.
    case applied(canceledTextEdits: [SRUIPendingTextEditRef])
    /// At least one assigned edit is above the server frontier and must be canceled by resume.
    case resumeRequired
    /// A reconnect generation already owns the outbox.
    case superseded
}

/// Serializes token mutation and synchronous snapshot rendering on MainActor. Because the remount
/// itself is MainActor-isolated, invalidation suspends until it finishes instead of blocking a
/// cooperative-pool thread on an NSLock.
@MainActor
final class ResyncRenderFence: Sendable {
    private var activeToken: UUID?
    private var renderedBoundaryEpoch: UInt64?
    private var retiredToken: UUID?
    private var retiredBoundaryEpoch: UInt64?

    nonisolated init() {}

    func activate(_ token: UUID) {
        activeToken = token
        renderedBoundaryEpoch = nil
        retiredToken = nil
        retiredBoundaryEpoch = nil
    }

    /// Invalidates the token and returns the text boundary recorded by a completed native render.
    ///
    /// The result stays readable for the same retired token so a lifecycle transition can inherit
    /// it when `consumeIfActive` and connection supersession race across actor hops.
    @discardableResult
    func invalidate(_ token: UUID) -> UInt64? {
        if activeToken == token {
            activeToken = nil
            retiredToken = token
            retiredBoundaryEpoch = renderedBoundaryEpoch
            renderedBoundaryEpoch = nil
            return retiredBoundaryEpoch
        }
        guard retiredToken == token else { return nil }
        return retiredBoundaryEpoch
    }

    func consumeIfActive(_ token: UUID) -> Bool {
        guard activeToken == token else { return false }
        activeToken = nil
        retiredToken = token
        retiredBoundaryEpoch = renderedBoundaryEpoch
        renderedBoundaryEpoch = nil
        return true
    }

    func performIfActive<Value>(
        _ token: UUID,
        boundaryEpoch: (Value) -> UInt64? = { _ in nil },
        _ body: () -> Value
    ) -> Value? {
        guard activeToken == token else { return nil }
        let result = body()
        guard activeToken == token else { return nil }
        renderedBoundaryEpoch = boundaryEpoch(result)
        return result
    }
}

private struct ResyncRenderOwnership: Sendable {
    var generation: UInt64?
    var binding: EventOutboxConnectionBinding
    var token: UUID
}

private struct LiveRenderOwnership: Sendable {
    var binding: EventOutboxConnectionBinding
    var sessionIncarnation: EventOutboxSessionIncarnation
    var token: UUID
}

private struct ResumeRecoveryRenderOwnership: Sendable {
    var generation: UInt64
    var binding: EventOutboxConnectionBinding
    var token: UUID
}

/// Fences MainActor lifecycle effects for assigned envelopes; unassigned text stays in TextEditingSession.
private final class TextLifecycleFence: @unchecked Sendable {
    private let lock = NSLock()
    private var activeSessionIncarnation: EventOutboxSessionIncarnation?

    func beginActivation(_ sessionIncarnation: EventOutboxSessionIncarnation) {
        lock.withLock {
            activeSessionIncarnation = sessionIncarnation
        }
    }

    func completeActivation(_ sessionIncarnation: EventOutboxSessionIncarnation) -> Bool {
        lock.withLock {
            activeSessionIncarnation == sessionIncarnation
        }
    }

    func activate(_ sessionIncarnation: EventOutboxSessionIncarnation) {
        beginActivation(sessionIncarnation)
    }

    @MainActor
    func performIfActive<Value: Sendable>(
        sessionIncarnation: EventOutboxSessionIncarnation,
        handler: @MainActor @Sendable () -> Value
    ) -> Value? {
        lock.lock()
        defer { lock.unlock() }
        guard activeSessionIncarnation == sessionIncarnation else { return nil }
        return handler()
    }
}

/// Lets a MainActor correction revoke a prepared send before its FIFO gate opens.
///
/// Records are registered at preparation and retired at authorization, rejection, cancellation,
/// or settlement, so the revocation set cannot grow without bound (§18.3, §22.6).
private final class PreparedTextEditAuthorizationFence: @unchecked Sendable {
    private let lock = NSLock()
    private var preparedEventIds: Set<EventId> = []
    private var assignedEventIds: Set<EventId> = []
    private var revokedEventIds: Set<EventId> = []

    func register(_ eventId: EventId) {
        lock.withLock {
            _ = preparedEventIds.insert(eventId)
        }
    }

    func markAssigned(_ eventId: EventId) {
        lock.withLock {
            guard preparedEventIds.contains(eventId) else { return }
            assignedEventIds.insert(eventId)
        }
    }

    func revoke(_ eventId: EventId) {
        lock.withLock {
            guard preparedEventIds.contains(eventId) else { return }
            revokedEventIds.insert(eventId)
        }
    }

    /// Promotes a native-assigned envelope across binding teardown. A prepared identity which
    /// never reached TextEditingSession, or was synchronously revoked by a correction, remains
    /// rollback-safe and is not advertised by the replacement connection.
    func consumeAssignedForLifecycle(_ eventId: EventId) -> Bool {
        lock.lock()
        defer {
            preparedEventIds.remove(eventId)
            assignedEventIds.remove(eventId)
            revokedEventIds.remove(eventId)
            lock.unlock()
        }
        return preparedEventIds.contains(eventId)
            && assignedEventIds.contains(eventId)
            && !revokedEventIds.contains(eventId)
    }

    /// Consults and retires one authorization record atomically with the caller's decision.
    func resolve<T>(_ eventId: EventId, _ body: (Bool) -> T) -> T {
        lock.lock()
        defer {
            preparedEventIds.remove(eventId)
            assignedEventIds.remove(eventId)
            revokedEventIds.remove(eventId)
            lock.unlock()
        }
        return body(revokedEventIds.contains(eventId))
    }

    func retire(_ eventId: EventId) {
        lock.withLock {
            preparedEventIds.remove(eventId)
            assignedEventIds.remove(eventId)
            revokedEventIds.remove(eventId)
        }
    }

    func reset() {
        lock.withLock {
            preparedEventIds.removeAll(keepingCapacity: false)
            assignedEventIds.removeAll(keepingCapacity: false)
            revokedEventIds.removeAll(keepingCapacity: false)
        }
    }

    var retainedRevocationCount: Int {
        lock.withLock { revokedEventIds.count }
    }
}

/// Actor managing outbound semantic event generation, sequencing, and wire transmission.
///
/// Retry safety (§18.2): every application-side-effect event carries a stable `event_id` and
/// `event_seq`. Selective acknowledgements may settle later events first, but
/// `lastAckedEventSeq` advances only across a contiguous settled prefix, like a TCP cumulative
/// acknowledgement. Unassigned whole-value edits remain owned by `TextEditingSession`; this
/// actor retains only envelopes whose generic event identity has been allocated.
public actor EventOutbox {
    /// Maximum span between the contiguous ack frontier and the newest allocated event (§18.2).
    public static let defaultMaxPendingEvents = 256

    public nonisolated let clientInstanceId: ClientInstanceId
    nonisolated let resyncRenderFence = ResyncRenderFence()
    private nonisolated let textLifecycleFence = TextLifecycleFence()
    private nonisolated let preparedTextEditAuthorizationFence = PreparedTextEditAuthorizationFence()
    private var resyncRenderOwnership: ResyncRenderOwnership?
    private var liveRenderOwnership: LiveRenderOwnership?
    private var resumeRecoveryRenderOwnership: ResumeRecoveryRenderOwnership?
    private let maxPendingEvents: Int
    private var pendingEventReplayLoop: PendingEventReplayLoop
    private var replayLease: PendingEventReplayLoop.Lease?
    private var activeSessionId: String?
    private var activeConnectionBinding: EventOutboxConnectionBinding?
    private var activeSessionIncarnation: EventOutboxSessionIncarnation?
    private var resyncBoundaryTransitionEpoch: UInt64 = 0
    private var pendingResyncBoundaryCleanup: PendingResyncBoundaryCleanup?
    private var pendingLiveRenderInvalidation: PendingLiveRenderInvalidation?
    private var pendingResumeRecoveryRenderInvalidation: PendingResumeRecoveryRenderInvalidation?
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
    /// it instead of merely dropping the reference (§18). Scheduler FIFO inside the input lane
    /// does not replace this: increasing `event_seq` must reach the transport in allocation order
    /// (§18.2).
    private var sendTail: Task<Void, any Error>?
    /// Closed slots that may still roll back after explicit native assignment failure.
    /// Unassigned text values never enter this actor; only allocated envelopes occupy this gate.
    private var preparedTextEditSends: [UUID: PreparedTextEditSend] = [:]
    /// Authorized slots whose transport task is still completing.
    private var authorizedPreparedTextEditSends: [UUID: PreparedTextEditSend] = [:]
    /// Lifecycle suspension canceled these closed slots before MainActor reported its decision.
    private var lifecycleSuspendedPreparedTextEdits: [UUID: Event] = [:]
    /// MainActor authorized these identities after their old transport had already been canceled.
    private var lifecycleAuthorizedPreparedTextEdits: [UUID: Event] = [:]
    /// A successor for a node cannot be promoted until this acknowledgement's authoritative
    /// revision has reached the rendered replica.
    private var textAcknowledgementBarriers: [NodeId: TextEditAcknowledgementBarrier] = [:]
    private var textLaneStateVersion: UInt64 = 0
    private var textLaneWaiters: [UUID: CheckedContinuation<Void, Never>] = [:]
    private var nativeTextLifecycleWillHopForTesting: (@Sendable () async -> Void)?

    private struct PendingResyncBoundaryCleanup: Sendable {
        var token: UUID
        var invalidation: Task<UInt64?, Never>
        var acknowledgementBarriersAtStart: [NodeId: TextEditAcknowledgementBarrier]
    }

    private struct PendingLiveRenderInvalidation: Sendable {
        var token: UUID
        var invalidation: Task<UInt64?, Never>
    }

    private struct PendingResumeRecoveryRenderInvalidation: Sendable {
        var generation: UInt64
        var token: UUID
        var invalidation: Task<UInt64?, Never>
    }

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

    /// Generates a globally unique, retry-safe event identifier (§7.7, §18.2).
    private func generateEventId() -> EventId {
        EventId(string: UUID().uuidString)
    }

    /// Admits, allocates, retains, and transmits one client-originated event (§7.6, §7.7, §18.2).
    ///
    /// The only path that mints a sequence, and private so it stays that way. Admission and
    /// backpressure run *before* allocation, and retention happens before the first suspension, so
    /// neither a refused send nor a failed write can leave a permanent hole in the contiguous send
    /// window that the server would then refuse to settle past.
    private func allocateAndSend(
        binding: EventOutboxConnectionBinding,
        sessionIncarnation: EventOutboxSessionIncarnation,
        via transport: any Transport,
        _ makeEvent: (UInt64, EventId) -> Event
    ) async throws -> Event {
        try await waitForPreparedTextEditResolution(
            binding: binding,
            sessionIncarnation: sessionIncarnation
        )
        guard activeConnectionBinding == binding,
              activeSessionIncarnation == sessionIncarnation,
              acceptsNewEvents else {
            throw EventOutboxError.resumeNotConfirmed
        }
        try ensureSequenceWindowCapacity()

        let nextEventSeq = currentEventSeq + 1
        let event = makeEvent(nextEventSeq, generateEventId())
            .withClientInstanceId(clientInstanceId)
        let framedBytes = try encodeEvent(event)

        // Framing and every fallible retention check precede sequence mutation, so an
        // unencodable event cannot burn a sequence in the contiguous send window.
        try retainPending(event)
        currentEventSeq = nextEventSeq

        try await transmitRetainedEvent(
            event,
            framedBytes: framedBytes,
            binding: binding,
            sessionIncarnation: sessionIncarnation,
            via: transport
        )
        return event
    }

    /// Serializes and sends an already-allocated event over the given transport (§16, §22).
    func sendEvent(
        _ event: Event,
        binding: EventOutboxConnectionBinding,
        sessionIncarnation suppliedIncarnation: EventOutboxSessionIncarnation? = nil,
        via transport: any Transport
    ) async throws {
        let sessionIncarnation = try resolveSessionIncarnation(
            suppliedIncarnation,
            binding: binding
        )
        try await waitForPreparedTextEditResolution(
            binding: binding,
            sessionIncarnation: sessionIncarnation
        )
        guard acceptsNewEvents else {
            throw EventOutboxError.resumeNotConfirmed
        }
        let framedBytes = try encodeEvent(event)

        // Retain before the first suspension: a fast acknowledgement may arrive while send is
        // awaiting transport completion and must be able to remove this entry exactly once.
        try retainPending(event)
        try await transmitRetainedEvent(
            event,
            framedBytes: framedBytes,
            binding: binding,
            sessionIncarnation: sessionIncarnation,
            via: transport
        )
    }

    private func encodeEvent(_ event: Event) throws -> Data {
        var message = SRUIMessage()
        message.event = event.toWire()
        return try SRUIFraming.encodeFramed(message)
    }

    private func transmitRetainedEvent(
        _ event: Event,
        framedBytes: Data,
        binding: EventOutboxConnectionBinding,
        sessionIncarnation: EventOutboxSessionIncarnation,
        via transport: any Transport
    ) async throws {
        let retainedResumeEpoch = lastIssuedResumeGeneration
        let retainedSessionId = activeSessionId

        // Join the FIFO before the transport suspension so retained events reach the wire in
        // allocation order even when earlier writes are slow.
        let send = enqueueSend { [weak self] in
            try Task.checkCancellation()
            guard let self,
                  await self.mayTransmitRetainedEvent(
                    event,
                    binding: binding,
                    sessionIncarnation: sessionIncarnation,
                    resumeEpoch: retainedResumeEpoch,
                    sessionId: retainedSessionId
                  ) else {
                return
            }
            _ = try await self.sendRetainedEventIfActive(
                event,
                framedBytes: framedBytes,
                binding: binding,
                sessionIncarnation: sessionIncarnation,
                resumeEpoch: retainedResumeEpoch,
                sessionId: retainedSessionId,
                via: transport
            )
        }
        try await send.value
    }
    @discardableResult
    public func sendActivate(
        nodeId: NodeId,
        observedRevision: Revision,
        binding: EventOutboxConnectionBinding,
        sessionIncarnation suppliedIncarnation: EventOutboxSessionIncarnation? = nil,
        via transport: any Transport
    ) async throws -> Event {
        let sessionIncarnation = try resolveSessionIncarnation(
            suppliedIncarnation,
            binding: binding
        )
        return try await allocateAndSend(
            binding: binding,
            sessionIncarnation: sessionIncarnation,
            via: transport
        ) { eventSeq, eventId in
            Event.activate(
                eventSeq: eventSeq,
                eventId: eventId,
                observedRevision: observedRevision,
                nodeId: nodeId
            )
        }
    }

    /// Constructs and sends a `VALUE_CHANGED` event without creating a sequence beyond the window.
    @discardableResult
    public func sendValueChanged(
        nodeId: NodeId,
        observedRevision: Revision,
        value: Value,
        binding: EventOutboxConnectionBinding,
        sessionIncarnation suppliedIncarnation: EventOutboxSessionIncarnation? = nil,
        via transport: any Transport
    ) async throws -> Event {
        let sessionIncarnation = try resolveSessionIncarnation(
            suppliedIncarnation,
            binding: binding
        )
        return try await allocateAndSend(
            binding: binding,
            sessionIncarnation: sessionIncarnation,
            via: transport
        ) { eventSeq, eventId in
            Event.valueChanged(
                eventSeq: eventSeq,
                eventId: eventId,
                observedRevision: observedRevision,
                nodeId: nodeId,
                value: value
            )
        }
    }

    /// Constructs and sends a `SELECTION_CHANGED` event without creating a sequence beyond the window.
    @discardableResult
    public func sendSelectionChanged(
        nodeId: NodeId,
        observedRevision: Revision,
        itemId: ItemId,
        binding: EventOutboxConnectionBinding,
        sessionIncarnation suppliedIncarnation: EventOutboxSessionIncarnation? = nil,
        via transport: any Transport
    ) async throws -> Event {
        let sessionIncarnation = try resolveSessionIncarnation(
            suppliedIncarnation,
            binding: binding
        )
        return try await allocateAndSend(
            binding: binding,
            sessionIncarnation: sessionIncarnation,
            via: transport
        ) { eventSeq, eventId in
            Event.selectionChanged(
                eventSeq: eventSeq,
                eventId: eventId,
                observedRevision: observedRevision,
                nodeId: nodeId,
                itemId: itemId
            )
        }
    }

    /// Allocates and retains a TEXT_EDIT, reserving its FIFO transport slot before returning.
    /// The caller must authorize only after MainActor records the exact native assignment; explicit
    /// assignment failure may reject and rewind this newest identity before any later allocation.
    /// Unassigned/coalesced values remain in TextEditingSession and never enter this actor.
    func prepareTextEdit(
        nodeId: NodeId,
        text: String,
        editSeq: EditSeq,
        observedRevision: Revision,
        binding: EventOutboxConnectionBinding,
        sessionIncarnation suppliedIncarnation: EventOutboxSessionIncarnation? = nil,
        via transport: any Transport
    ) async throws -> PreparedTextEdit? {
        let sessionIncarnation = try resolveSessionIncarnation(
            suppliedIncarnation,
            binding: binding
        )
        guard activeConnectionBinding == binding,
              activeSessionIncarnation == sessionIncarnation,
              acceptsNewEvents else {
            throw EventOutboxError.resumeNotConfirmed
        }
        guard preparedTextEditSends.isEmpty,
              assignedTextEvent(for: nodeId) == nil,
              textAcknowledgementBarriers[nodeId] == nil else {
            return nil
        }
        try ensureSequenceWindowCapacity()

        let nextEventSeq = currentEventSeq + 1
        let event = Event.textEdit(
            eventSeq: nextEventSeq,
            eventId: generateEventId(),
            observedRevision: observedRevision,
            nodeId: nodeId,
            text: text,
            editSeq: editSeq
        ).withClientInstanceId(clientInstanceId)
        let framedBytes = try encodeEvent(event)
        try retainPending(event)
        currentEventSeq = nextEventSeq

        let token = UUID()
        let gate = PreparedTextEditSendGate()
        let retainedResumeEpoch = lastIssuedResumeGeneration
        let retainedSessionId = activeSessionId
        let send = enqueueSend { [weak self] in
            guard await gate.wait() else { return }
            try Task.checkCancellation()
            guard let self else { return }
            _ = try await self.sendRetainedEventIfActive(
                event,
                framedBytes: framedBytes,
                binding: binding,
                sessionIncarnation: sessionIncarnation,
                resumeEpoch: retainedResumeEpoch,
                sessionId: retainedSessionId,
                via: transport
            )
        }
        preparedTextEditSends[token] = PreparedTextEditSend(
            event: event,
            gate: gate,
            task: send
        )
        preparedTextEditAuthorizationFence.register(event.eventId)
        return PreparedTextEdit(event: event, token: token)
    }

    /// Waits only on assigned-envelope state. Callers snapshot the MainActor draft *before*
    /// waiting so a later coalesced value cannot replace the identity this drain will send.
    func waitUntilTextEditLaneIsAvailable(
        nodeId: NodeId,
        binding: EventOutboxConnectionBinding,
        sessionIncarnation suppliedIncarnation: EventOutboxSessionIncarnation? = nil
    ) async throws {
        let sessionIncarnation = try resolveSessionIncarnation(
            suppliedIncarnation,
            binding: binding
        )
        while !preparedTextEditSends.isEmpty
            || assignedTextEvent(for: nodeId) != nil
            || textAcknowledgementBarriers[nodeId] != nil {
            guard activeConnectionBinding == binding,
                  activeSessionIncarnation == sessionIncarnation,
                  acceptsNewEvents else {
                throw EventOutboxError.resumeNotConfirmed
            }
            let version = textLaneStateVersion
            try await waitForTextLaneStateChange(binding: binding, after: version)
        }
    }

    /// Records that the exact prepared identity now owns native text state. Binding teardown
    /// uses this nonisolated fence to distinguish replayable assignments from rollback-safe slots.
    nonisolated func notePreparedTextEditAssigned(eventId: EventId) {
        preparedTextEditAuthorizationFence.markAssigned(eventId)
    }

    /// Marks a native assignment as retracted. `authorizePreparedTextEdit` consults this fence
    /// in the same lock that opens the send gate, so a correction cannot lose the race after
    /// `noteAssigned` and still transmit the stale envelope.
    nonisolated func revokeUnauthorizedPreparedTextEdit(eventId: EventId) {
        preparedTextEditAuthorizationFence.revoke(eventId)
    }

    /// Authorizes a prepared identity immediately after MainActor records the native assignment.
    ///
    /// Authorization opens the FIFO gate and makes rollback impossible. If teardown canceled the
    /// old transport during the actor hop, the exact retained envelope is authorized for replay.
    /// A correction that revoked this `event_id` rejects the still-closed slot instead.
    @discardableResult
    func authorizePreparedTextEdit(_ prepared: PreparedTextEdit) -> Bool {
        enum Decision {
            case authorizeAndSignal
            case alreadyAuthorized
            case reject
            case failed
        }
        let decision = preparedTextEditAuthorizationFence.resolve(
            prepared.event.eventId
        ) { revoked -> Decision in
            if revoked {
                return .reject
            }
            if let retained = preparedTextEditSends[prepared.token],
               retained.event == prepared.event {
                guard retained.gate.resolve(shouldSend: true) else { return .failed }
                preparedTextEditSends.removeValue(forKey: prepared.token)
                authorizedPreparedTextEditSends[prepared.token] = retained
                return .authorizeAndSignal
            }
            if lifecycleSuspendedPreparedTextEdits[prepared.token] == prepared.event,
               pendingEvents[prepared.event.eventId] == prepared.event {
                lifecycleSuspendedPreparedTextEdits.removeValue(forKey: prepared.token)
                lifecycleAuthorizedPreparedTextEdits[prepared.token] = prepared.event
                return .alreadyAuthorized
            }
            if authorizedPreparedTextEditSends[prepared.token]?.event == prepared.event
                || lifecycleAuthorizedPreparedTextEdits[prepared.token] == prepared.event {
                return .alreadyAuthorized
            }
            return .failed
        }
        switch decision {
        case .authorizeAndSignal:
            signalTextLaneStateChange()
            return true
        case .alreadyAuthorized:
            return true
        case .reject:
            _ = rejectPreparedTextEdit(prepared)
            return false
        case .failed:
            return false
        }
    }

    /// Waits for an authorized edit's first transmission. A lifecycle-canceled transport returns
    /// the retained identity immediately; same-session resume owns its next transmission.
    @discardableResult
    func releasePreparedTextEdit(_ prepared: PreparedTextEdit) async throws -> Event? {
        if let retained = authorizedPreparedTextEditSends.removeValue(forKey: prepared.token),
           retained.event == prepared.event {
            try await retained.task.value
            return prepared.event
        }
        if lifecycleAuthorizedPreparedTextEdits.removeValue(forKey: prepared.token) == prepared.event,
           pendingEvents[prepared.event.eventId] == prepared.event {
            return prepared.event
        }
        return nil
    }

    /// Rolls back only an explicitly rejected, still-unauthorized edit. Global prepared-slot
    /// exclusion makes it the newest allocation, so rewinding event_seq cannot create a hole.
    ///
    /// Even when lifecycle settlement already removed the pending identity, the closed send slot
    /// is always canceled and removed so it cannot wedge the outbound FIFO.
    @discardableResult
    func rejectPreparedTextEdit(_ prepared: PreparedTextEdit) -> Bool {
        let canceledBeforeSend: Bool
        if let retained = preparedTextEditSends.removeValue(forKey: prepared.token),
           retained.event == prepared.event {
            canceledBeforeSend = retained.gate.cancelIfUnresolved()
            retained.task.cancel()
        } else if lifecycleSuspendedPreparedTextEdits[prepared.token] == prepared.event {
            lifecycleSuspendedPreparedTextEdits.removeValue(forKey: prepared.token)
            canceledBeforeSend = true
        } else {
            preparedTextEditAuthorizationFence.retire(prepared.event.eventId)
            return false
        }

        preparedTextEditAuthorizationFence.retire(prepared.event.eventId)
        signalTextLaneStateChange()
        guard canceledBeforeSend,
              pendingOrder.last == prepared.event.eventId,
              pendingEvents[prepared.event.eventId] == prepared.event,
              currentEventSeq == prepared.event.eventSeq else {
            return false
        }

        pendingEvents.removeValue(forKey: prepared.event.eventId)
        pendingOrder.removeLast()
        currentEventSeq -= 1
        return true
    }

    /// Assigned, unacknowledged `TEXT_EDIT` events in send order (§18.3).
    public func assignedTextEditEvents() -> [Event] {
        pendingOrder.compactMap { id in
            guard let event = pendingEvents[id], event.eventType == .EVENT_TEXT_EDIT else {
                return nil
            }
            return event
        }
    }

    public func assignedTextEditDescriptors() -> [PendingTextEditDescriptor] {
        pendingOrder.compactMap { id in
            guard let event = pendingEvents[id],
                  event.eventType == .EVENT_TEXT_EDIT,
                  let editSeq = event.editSeq else {
                return nil
            }
            return PendingTextEditDescriptor(
                eventId: event.eventId,
                eventSeq: event.eventSeq,
                nodeId: event.nodeId,
                editSeq: editSeq
            )
        }
    }

    /// Validates the exact server echo before any native cancellation callback can run.
    private func textEditsToCancel(
        confirming refs: [PendingTextEditDescriptor],
        requireExactMatch: Bool
    ) throws -> [PendingTextEditDescriptor] {
        let assigned = assignedTextEditDescriptors()
        if requireExactMatch {
            let assignedKeys = Set(assigned.map(Self.identityKey))
            let echoKeys = Set(refs.map(Self.identityKey))
            guard assigned.count == refs.count, assignedKeys == echoKeys else {
                throw EventOutboxError.textEditDiscardMismatch
            }
            return refs
        }
        return assigned
    }

    /// Selectively cancels assigned `TEXT_EDIT` events. A required exact match fails closed (§18.3).
    func cancelAssignedTextEdits(
        confirming refs: [PendingTextEditDescriptor],
        requireExactMatch: Bool
    ) throws {
        let condemned = try textEditsToCancel(
            confirming: refs,
            requireExactMatch: requireExactMatch
        )
        for ref in condemned {
            preparedTextEditAuthorizationFence.retire(ref.eventId)
            if let event = pendingEvents.removeValue(forKey: ref.eventId) {
                pendingOrder.removeAll { $0 == ref.eventId }
                recordSelectiveAcknowledgement(event.eventSeq)
            } else {
                recordSelectiveAcknowledgement(ref.eventSeq)
            }
        }
        signalTextLaneStateChange()
        cancelReplayRetryLoopIfSettled()
    }

    /// Cancels assigned `TEXT_EDIT` events only while `generation` still owns the reconnect latch.
    @discardableResult
    func cancelAssignedTextEdits(
        confirming refs: [SRUIPendingTextEditRef],
        requireExactMatch: Bool,
        onlyIfResumeGeneration generation: UInt64?
    ) throws -> Bool {
        guard activeResumeGeneration == generation else { return false }
        let confirming = refs.compactMap(PendingTextEditDescriptor.init(wire:))
        if requireExactMatch, confirming.count != refs.count {
            throw EventOutboxError.textEditDiscardMismatch
        }
        try cancelAssignedTextEdits(confirming: confirming, requireExactMatch: requireExactMatch)
        return true
    }

    /// Applies the assigned-edit acknowledgement boundary after the native full resync.
    @discardableResult
    func applyFullResyncTextBoundary(
        generation: UInt64?,
        binding: EventOutboxConnectionBinding,
        renderToken: UUID
    ) async -> Bool {
        guard ownsResyncScope(generation: generation, binding: binding),
              let ownership = resyncRenderOwnership,
              ownership.generation == generation,
              ownership.binding == binding,
              ownership.token == renderToken else {
            return false
        }
        guard await resyncRenderFence.consumeIfActive(renderToken),
              ownsResyncScope(generation: generation, binding: binding),
              resyncRenderOwnership?.generation == generation,
              resyncRenderOwnership?.binding == binding,
              resyncRenderOwnership?.token == renderToken else {
            return false
        }

        applyFullResyncTextBoundaryState()
        resyncRenderOwnership = nil
        return true
    }

    /// Abandons ownership after a committed snapshot fails to render.
    @discardableResult
    func abortResyncRender(
        generation: UInt64?,
        binding: EventOutboxConnectionBinding,
        renderToken: UUID
    ) async -> Bool {
        guard ownsResyncScope(generation: generation, binding: binding),
              resyncRenderOwnership?.generation == generation,
              resyncRenderOwnership?.binding == binding,
              resyncRenderOwnership?.token == renderToken else {
            return false
        }

        acceptsNewEvents = false
        applyFullResyncTextBoundaryState()
        await resyncRenderFence.invalidate(renderToken)
        if resyncRenderOwnership?.generation == generation,
           resyncRenderOwnership?.binding == binding,
           resyncRenderOwnership?.token == renderToken {
            resyncRenderOwnership = nil
        }
        return true
    }

    /// Resolves terminal text acknowledgements for one exact session incarnation. Any native
    /// correction invalidations are applied before barriers are removed and waiters are signaled.
    @discardableResult
    func releaseTextAcknowledgements(
        through revision: UInt64,
        binding: EventOutboxConnectionBinding,
        sessionIncarnation: EventOutboxSessionIncarnation,
        onResolved: @escaping TextAcknowledgementResolutionHandler
    ) async -> Bool {
        guard activeConnectionBinding == binding,
              activeSessionIncarnation == sessionIncarnation else {
            return false
        }
        let ready = textAcknowledgementBarriers.values
            .filter { $0.revisionAfterEffect <= revision }
            .sorted { $0.nodeId.value < $1.nodeId.value }
        guard !ready.isEmpty else {
            return activeConnectionBinding == binding
                && activeSessionIncarnation == sessionIncarnation
        }

        if let nativeTextLifecycleWillHopForTesting {
            await nativeTextLifecycleWillHopForTesting()
        }
        guard activeConnectionBinding == binding,
              activeSessionIncarnation == sessionIncarnation else {
            return false
        }
        let resolved: Bool? = await textLifecycleFence.performIfActive(
            sessionIncarnation: sessionIncarnation,
            handler: {
                onResolved(ready)
                return true
            }
        )
        guard resolved == true else { return false }
        guard activeConnectionBinding == binding,
              activeSessionIncarnation == sessionIncarnation else {
            return false
        }

        var removedBarrier = false
        for barrier in ready where textAcknowledgementBarriers[barrier.nodeId] == barrier {
            textAcknowledgementBarriers.removeValue(forKey: barrier.nodeId)
            removedBarrier = true
        }
        if removedBarrier {
            signalTextLaneStateChange()
        }
        return activeConnectionBinding == binding
            && activeSessionIncarnation == sessionIncarnation
    }

    private static func identityKey(_ ref: PendingTextEditDescriptor) -> String {
        "\(ref.eventId.toHex()):\(ref.eventSeq):\(ref.nodeId.value):\(ref.editSeq.rawValue)"
    }

    private func assignedTextEvent(for nodeId: NodeId) -> Event? {
        pendingEvents.values.first {
            $0.eventType == .EVENT_TEXT_EDIT && $0.nodeId == nodeId
        }
    }

    /// Replays every unacknowledged event in original send order with its original identity.
    public func resendPendingEvents(
        binding: EventOutboxConnectionBinding,
        sessionIncarnation suppliedIncarnation: EventOutboxSessionIncarnation? = nil,
        via transport: any Transport
    ) async throws {
        let sessionIncarnation = try resolveSessionIncarnation(
            suppliedIncarnation,
            binding: binding
        )
        let replay = pendingOrder.compactMap { pendingEvents[$0] }
        let send = enqueueSend { [weak self] in
            for event in replay {
                try Task.checkCancellation()
                guard let self else {
                    throw EventOutboxError.resumeNotConfirmed
                }
                _ = try await self.sendReplayEventIfActive(
                    event,
                    binding: binding,
                    sessionIncarnation: sessionIncarnation,
                    via: transport
                )
            }
        }
        try await withTaskCancellationHandler {
            try await send.value
        } onCancel: {
            send.cancel()
        }
    }

    /// Selectively acknowledges one event. A later sequence does not cross an earlier gap.
    ///
    /// Private: settling an intent mutates state whose ownership depends on wire identity, so
    /// `settleAcknowledgement` is the only way in from the wire (§18.2).
    private func acknowledgeEvent(id: EventId) {
        preparedTextEditAuthorizationFence.retire(id)
        guard let event = pendingEvents.removeValue(forKey: id) else { return }
        pendingOrder.removeAll { $0 == id }
        recordSelectiveAcknowledgement(event.eventSeq)
        signalTextLaneStateChange()
        cancelReplayRetryLoopIfSettled()
    }

    func isActiveConnectionBinding(_ binding: EventOutboxConnectionBinding) -> Bool {
        activeConnectionBinding == binding
    }

    func isActiveSessionIncarnation(
        binding: EventOutboxConnectionBinding,
        sessionIncarnation: EventOutboxSessionIncarnation
    ) -> Bool {
        activeConnectionBinding == binding
            && activeSessionIncarnation == sessionIncarnation
    }

    func sessionIncarnation(
        binding: EventOutboxConnectionBinding
    ) -> EventOutboxSessionIncarnation? {
        guard activeConnectionBinding == binding else { return nil }
        return activeSessionIncarnation
    }

    private func resolveSessionIncarnation(
        _ supplied: EventOutboxSessionIncarnation?,
        binding: EventOutboxConnectionBinding
    ) throws -> EventOutboxSessionIncarnation {
        guard activeConnectionBinding == binding,
              let expected = activeSessionIncarnation,
              supplied == nil || supplied == expected else {
            throw EventOutboxError.resumeNotConfirmed
        }
        return expected
    }

    private func advanceSessionIncarnation(
        binding: EventOutboxConnectionBinding
    ) -> EventOutboxSessionIncarnation? {
        guard activeConnectionBinding == binding,
              let current = activeSessionIncarnation,
              current.binding == binding else {
            return nil
        }
        precondition(
            current.sequence < UInt64.max,
            "EventOutbox session incarnation exhausted"
        )
        let next = EventOutboxSessionIncarnation(
            binding: binding,
            sequence: current.sequence + 1
        )
        activeSessionIncarnation = next
        textLifecycleFence.activate(next)
        return next
    }

    var activeConnectionBindingForTesting: EventOutboxConnectionBinding? {
        activeConnectionBinding
    }

    var preparedTextEditSendCountForTesting: Int {
        preparedTextEditSends.count
    }

    var retainedPreparedTextEditRevocationCountForTesting: Int {
        preparedTextEditAuthorizationFence.retainedRevocationCount
    }

    func setNativeTextLifecycleWillHopForTesting(
        _ interceptor: (@Sendable () async -> Void)?
    ) {
        nativeTextLifecycleWillHopForTesting = interceptor
    }

    func withActiveConnectionBinding<Value: Sendable>(
        _ binding: EventOutboxConnectionBinding,
        _ body: @Sendable () -> Value
    ) -> Value? {
        guard activeConnectionBinding == binding else { return nil }
        return body()
    }

    func withActiveSessionIncarnation<Value: Sendable>(
        binding: EventOutboxConnectionBinding,
        sessionIncarnation: EventOutboxSessionIncarnation,
        _ body: @Sendable () -> Value
    ) -> Value? {
        guard activeConnectionBinding == binding,
              activeSessionIncarnation == sessionIncarnation else {
            return nil
        }
        return body()
    }

    /// Runs a synchronous MainActor lifecycle mutation only for the exact active incarnation.
    ///
    /// The nonisolated fence closes the actor-to-MainActor reentrancy window: a newer binding
    /// invalidates the incarnation before its actor work can proceed, and waits for any already
    /// authorized native mutation to finish.
    func performNativeLifecycleIfActive<Value: Sendable>(
        binding: EventOutboxConnectionBinding,
        sessionIncarnation: EventOutboxSessionIncarnation,
        _ handler: @escaping @MainActor @Sendable () -> Value
    ) async -> Value? {
        guard activeConnectionBinding == binding,
              activeSessionIncarnation == sessionIncarnation else {
            return nil
        }
        if let nativeTextLifecycleWillHopForTesting {
            await nativeTextLifecycleWillHopForTesting()
        }
        guard activeConnectionBinding == binding,
              activeSessionIncarnation == sessionIncarnation else {
            return nil
        }
        return await textLifecycleFence.performIfActive(
            sessionIncarnation: sessionIncarnation,
            handler: handler
        )
    }

    /// Acquires exclusive ownership for one controller/transport attempt before its handshake.
    ///
    /// Acquiring a newer binding immediately suspends allocation and supersedes every older
    /// binding, even when the replacement connection resumes the same session. Call
    /// `confirmFreshSession(id:binding:)` after a fresh WELCOME snapshot is committed; resume
    /// handshakes use the resume lifecycle APIs. Every send must carry the returned opaque binding.
    public func beginConnectionBinding() async -> EventOutboxConnectionBinding {
        await beginConnectionBinding(onActivated: { _, _ in })
    }

    /// Cancellation-aware acquisition for connection orchestrators.
    ///
    /// The check executes on the outbox actor immediately before ownership changes, so a canceled
    /// task that was queued behind a newer click cannot wake later and steal its binding.
    func beginConnectionBindingUnlessCancelled(
        onActivated: @Sendable (
            EventOutboxConnectionBinding,
            EventOutboxSessionIncarnation
        ) async -> Void
    ) async throws -> EventOutboxConnectionBinding {
        try Task.checkCancellation()
        return await beginConnectionBinding(onActivated: onActivated)
    }

    private func beginConnectionBinding(
        onActivated: @Sendable (
            EventOutboxConnectionBinding,
            EventOutboxSessionIncarnation
        ) async -> Void
    ) async -> EventOutboxConnectionBinding {
        let binding = EventOutboxConnectionBinding(
            epoch: EventOutboxConnectionBindingEpochAllocator.shared.next()
        )
        let transitionEpoch = beginResyncBoundaryTransition()
        let sessionIncarnation = EventOutboxSessionIncarnation(
            binding: binding,
            sequence: 0
        )

        // Retire old assignment/lifecycle callbacks immediately, but leave correction
        // invalidations authorized until every synchronous native render has exited.
        textLifecycleFence.beginActivation(sessionIncarnation)
        activeConnectionBinding = binding
        activeSessionIncarnation = sessionIncarnation
        await onActivated(binding, sessionIncarnation)
        // onActivated may suspend while a later B/C acquisition publishes a newer owner. The
        // earlier continuation must not cancel that owner's replay, prepared writes, or render
        // state when it resumes.
        guard resyncBoundaryTransitionEpoch == transitionEpoch,
              activeConnectionBinding == binding,
              activeSessionIncarnation == sessionIncarnation else {
            return binding
        }
        activeResumeGeneration = nil
        pendingResumeFinalizationGeneration = nil
        acceptsNewEvents = false
        signalTextLaneStateChange()
        cancelReplayRetryLoop()
        cancelPendingWrites(retainUnauthorizedPreparedTextEdits: false)

        let boundaryCleanup = adoptOrBeginResyncBoundaryCleanup()
        let liveInvalidation = adoptOrBeginLiveRenderInvalidation()
        let recoveryInvalidation = adoptOrBeginResumeRecoveryRenderInvalidation()
        if let boundaryCleanup {
            let renderedBoundaryEpoch = await boundaryCleanup.invalidation.value
            _ = finishResyncBoundaryCleanup(
                boundaryCleanup,
                renderedBoundaryEpoch: renderedBoundaryEpoch,
                transitionEpoch: transitionEpoch
            )
        }
        if let liveInvalidation {
            _ = await liveInvalidation.invalidation.value
            _ = finishLiveRenderInvalidation(
                liveInvalidation,
                transitionEpoch: transitionEpoch
            )
        }
        if let recoveryInvalidation {
            _ = await recoveryInvalidation.invalidation.value
            _ = finishResumeRecoveryRenderInvalidation(
                recoveryInvalidation,
                transitionEpoch: transitionEpoch
            )
        }

        guard resyncBoundaryTransitionEpoch == transitionEpoch,
              activeConnectionBinding == binding,
              activeSessionIncarnation == sessionIncarnation,
              textLifecycleFence.completeActivation(sessionIncarnation) else {
            return binding
        }
        return binding
    }
    /// allocating new events until that generation receives an authoritative decision.
    ///
    /// Issuing a generation immediately supersedes every older attempt, so a delayed response
    /// from an abandoned connection is discarded rather than replayed (§18).
    func beginResumeAttempt(binding: EventOutboxConnectionBinding) async -> UInt64? {
        guard activeConnectionBinding == binding else { return nil }
        let transitionEpoch = beginResyncBoundaryTransition()
        cancelReplayRetryLoop()
        cancelPendingWrites()
        pendingResumeFinalizationGeneration = nil
        lastIssuedResumeGeneration += 1
        let generation = lastIssuedResumeGeneration
        activeResumeGeneration = generation
        acceptsNewEvents = false
        signalTextLaneStateChange()

        let boundaryCleanup = adoptOrBeginResyncBoundaryCleanup()
        let liveInvalidation = adoptOrBeginLiveRenderInvalidation()
        let recoveryInvalidation = adoptOrBeginResumeRecoveryRenderInvalidation()
        if let boundaryCleanup {
            let renderedBoundaryEpoch = await boundaryCleanup.invalidation.value
            _ = finishResyncBoundaryCleanup(
                boundaryCleanup,
                renderedBoundaryEpoch: renderedBoundaryEpoch,
                transitionEpoch: transitionEpoch
            )
        }
        if let liveInvalidation {
            _ = await liveInvalidation.invalidation.value
            _ = finishLiveRenderInvalidation(
                liveInvalidation,
                transitionEpoch: transitionEpoch
            )
        }
        if let recoveryInvalidation {
            _ = await recoveryInvalidation.invalidation.value
            _ = finishResumeRecoveryRenderInvalidation(
                recoveryInvalidation,
                transitionEpoch: transitionEpoch
            )
        }
        guard activeConnectionBinding == binding,
              activeResumeGeneration == generation,
              resyncBoundaryTransitionEpoch == transitionEpoch else {
            return nil
        }
        return generation
    }

    /// Whether `generation` still owns the reconnect decision (§18).
    func isActiveResumeGeneration(_ generation: UInt64) -> Bool {
        activeResumeGeneration == generation
    }

    /// Whether this outbox ever minted `generation` (§18).
    ///
    /// Generations are strictly increasing, so a value above the last issued one cannot be a
    /// stale decision from an abandoned attempt — it was never issued at all, which means the
    /// caller is bound to a different outbox than the one that minted it (§4 inv. 13).
    func hasIssuedResumeGeneration(_ generation: UInt64) -> Bool {
        generation >= 1 && generation <= lastIssuedResumeGeneration
    }

    /// Publishes a resync snapshot while the reconnect latch still blocks event allocation (§18).
    func commitResyncSnapshot<T: Sendable>(
        generation: UInt64?,
        binding: EventOutboxConnectionBinding,
        sessionIncarnation suppliedIncarnation: EventOutboxSessionIncarnation? = nil,
        onSessionIncarnationAdvanced: ReplacementTextEditingResetHandler? = nil,
        publish: @Sendable () -> T,
        committed: @Sendable (T) -> Bool
    ) async -> ResyncSnapshotCommit<T>? {
        await commitValidatedResyncSnapshot(
            generation: generation,
            binding: binding,
            sessionIncarnation: suppliedIncarnation,
            onSessionIncarnationAdvanced: onSessionIncarnationAdvanced,
            preflight: {},
            publish: publish,
            committed: committed
        )
    }

    /// Validates and publishes a resync snapshot while reconnect blocks event allocation (§18).
    ///
    /// The supersession check and publish share this actor-isolated boundary. An asynchronous
    /// preflight is re-checked for ownership after its suspension before state can be published.
    /// The controller releases the latch only after the snapshot has also mounted and the text
    /// resync boundary has reached this actor, preventing stale drafts from escaping in between.
    ///
    /// `publish` must be the swap alone — build the snapshot with
    /// `TransactionApplier.prepareResyncSnapshot(record:)` before calling, so the rebuild never
    /// occupies this actor while acknowledgements and sends wait behind it.
    ///
    /// A `nil` generation means "this controller has no attempt outstanding", which is the
    /// live-resync case: it matches only while no other controller holds the latch either.
    /// Returns `nil` when a newer attempt owns the latch and nothing was published.
    func commitValidatedResyncSnapshot<T: Sendable>(
        generation: UInt64?,
        binding: EventOutboxConnectionBinding,
        sessionIncarnation suppliedIncarnation: EventOutboxSessionIncarnation? = nil,
        onSessionIncarnationAdvanced: ReplacementTextEditingResetHandler? = nil,
        preflight: @Sendable () async throws -> Void,
        publish: @Sendable () -> T,
        committed: @Sendable (T) -> Bool
    ) async rethrows -> ResyncSnapshotCommit<T>? {
        guard ownsResyncScope(generation: generation, binding: binding),
              let currentIncarnation = activeSessionIncarnation,
              suppliedIncarnation == nil || suppliedIncarnation == currentIncarnation else {
            return nil
        }

        // Renderer preflight may hop to MainActor, making this actor reentrant. A newer reconnect
        // can take ownership during that suspension, so both success and failure paths re-check
        // the complete resync scope before publishing state or reporting a current-session error.
        do {
            try await preflight()
        } catch {
            guard ownsResyncScope(generation: generation, binding: binding),
                  activeSessionIncarnation == currentIncarnation else {
                return nil
            }
            throw error
        }
        guard ownsResyncScope(generation: generation, binding: binding),
              activeSessionIncarnation == currentIncarnation else {
            return nil
        }

        let result = publish()
        guard committed(result) else {
            return ResyncSnapshotCommit(
                result: result,
                renderToken: nil,
                sessionIncarnation: nil
            )
        }

        // A hard snapshot is an interaction boundary even when the server session ID is unchanged.
        // Advance before remount so pre-snapshot actions cannot enter after dispatch reopens.
        guard let snapshotIncarnation = advanceSessionIncarnation(binding: binding) else {
            return nil
        }
        if let onSessionIncarnationAdvanced {
            guard await performNativeTextLifecycle(
                binding: binding,
                sessionIncarnation: snapshotIncarnation,
                resumeGeneration: generation,
                handler: { onSessionIncarnationAdvanced(snapshotIncarnation) }
            ) else {
                return nil
            }
        }

        let renderToken = UUID()
        resyncRenderOwnership = ResyncRenderOwnership(
            generation: generation,
            binding: binding,
            token: renderToken
        )
        await resyncRenderFence.activate(renderToken)
        guard ownsResyncScope(generation: generation, binding: binding),
              activeSessionIncarnation == snapshotIncarnation,
              resyncRenderOwnership?.generation == generation,
              resyncRenderOwnership?.binding == binding,
              resyncRenderOwnership?.token == renderToken else {
            await resyncRenderFence.invalidate(renderToken)
            return nil
        }
        return ResyncSnapshotCommit(
            result: result,
            renderToken: renderToken,
            sessionIncarnation: snapshotIncarnation
        )
    }

    /// Publishes a live transaction and reserves its synchronous native render against rebinding.
    func commitLiveRender<T: Sendable>(
        binding: EventOutboxConnectionBinding,
        sessionIncarnation suppliedIncarnation: EventOutboxSessionIncarnation? = nil,
        publish: @Sendable () -> T,
        committed: @Sendable (T) -> Bool
    ) async -> LiveRenderCommit<T>? {
        guard activeConnectionBinding == binding,
              let sessionIncarnation = activeSessionIncarnation,
              suppliedIncarnation == nil || suppliedIncarnation == sessionIncarnation,
              activeResumeGeneration == nil,
              acceptsNewEvents,
              resyncRenderOwnership == nil,
              pendingResyncBoundaryCleanup == nil,
              liveRenderOwnership == nil,
              pendingLiveRenderInvalidation == nil,
              resumeRecoveryRenderOwnership == nil,
              pendingResumeRecoveryRenderInvalidation == nil else {
            return nil
        }

        let result = publish()
        guard committed(result) else {
            return LiveRenderCommit(result: result, renderToken: nil)
        }

        let renderToken = UUID()
        liveRenderOwnership = LiveRenderOwnership(
            binding: binding,
            sessionIncarnation: sessionIncarnation,
            token: renderToken
        )
        await resyncRenderFence.activate(renderToken)
        guard activeConnectionBinding == binding,
              activeSessionIncarnation == sessionIncarnation,
              activeResumeGeneration == nil,
              liveRenderOwnership?.binding == binding,
              liveRenderOwnership?.sessionIncarnation == sessionIncarnation,
              liveRenderOwnership?.token == renderToken else {
            await resyncRenderFence.invalidate(renderToken)
            return nil
        }
        return LiveRenderCommit(result: result, renderToken: renderToken)
    }

    @discardableResult
    func completeLiveRender(
        binding: EventOutboxConnectionBinding,
        sessionIncarnation: EventOutboxSessionIncarnation,
        renderToken: UUID
    ) async -> Bool {
        guard activeConnectionBinding == binding,
              activeSessionIncarnation == sessionIncarnation,
              liveRenderOwnership?.binding == binding,
              liveRenderOwnership?.sessionIncarnation == sessionIncarnation,
              liveRenderOwnership?.token == renderToken else {
            return false
        }
        guard await resyncRenderFence.consumeIfActive(renderToken),
              activeConnectionBinding == binding,
              activeSessionIncarnation == sessionIncarnation,
              liveRenderOwnership?.binding == binding,
              liveRenderOwnership?.sessionIncarnation == sessionIncarnation,
              liveRenderOwnership?.token == renderToken else {
            return false
        }
        liveRenderOwnership = nil
        return true
    }

    func beginResumeRecoveryRender(
        binding: EventOutboxConnectionBinding,
        generation: UInt64
    ) async -> UUID? {
        guard activeConnectionBinding == binding,
              activeResumeGeneration == generation,
              !acceptsNewEvents,
              resyncRenderOwnership == nil,
              pendingResyncBoundaryCleanup == nil,
              liveRenderOwnership == nil,
              pendingLiveRenderInvalidation == nil,
              resumeRecoveryRenderOwnership == nil,
              pendingResumeRecoveryRenderInvalidation == nil else {
            return nil
        }

        let renderToken = UUID()
        resumeRecoveryRenderOwnership = ResumeRecoveryRenderOwnership(
            generation: generation,
            binding: binding,
            token: renderToken
        )
        await resyncRenderFence.activate(renderToken)
        guard ownsResumeRecoveryRender(
            binding: binding,
            generation: generation,
            renderToken: renderToken
        ) else {
            await resyncRenderFence.invalidate(renderToken)
            return nil
        }
        return renderToken
    }

    @discardableResult
    func completeResumeRecoveryRender(
        binding: EventOutboxConnectionBinding,
        generation: UInt64,
        renderToken: UUID
    ) async -> Bool {
        guard ownsResumeRecoveryRender(
            binding: binding,
            generation: generation,
            renderToken: renderToken
        ) else {
            return false
        }
        guard await resyncRenderFence.consumeIfActive(renderToken),
              ownsResumeRecoveryRender(
                binding: binding,
                generation: generation,
                renderToken: renderToken
              ) else {
            return false
        }
        resumeRecoveryRenderOwnership = nil
        return true
    }

    @discardableResult
    func abortResumeRecoveryRender(
        binding: EventOutboxConnectionBinding,
        generation: UInt64,
        renderToken: UUID
    ) async -> Bool {
        guard ownsResumeRecoveryRender(
            binding: binding,
            generation: generation,
            renderToken: renderToken
        ) else {
            return false
        }
        acceptsNewEvents = false
        signalTextLaneStateChange()
        _ = await resyncRenderFence.invalidate(renderToken)
        guard ownsResumeRecoveryRender(
            binding: binding,
            generation: generation,
            renderToken: renderToken
        ) else {
            return false
        }
        resumeRecoveryRenderOwnership = nil
        return true
    }

    /// Applies a same-session frontier and returns the assigned identities MainActor must adopt
    /// before replay can open. The reconnect generation remains latched across that actor hop.
    ///
    /// When `discardedTextEdits` is non-`nil`, assigned `TEXT_EDIT` cancellation shares this
    /// generation check so a stale resync cannot drain a newer attempt's pending set (§18.3).
    func prepareSameSessionResume(
        id: String,
        lastProcessedEventSeq: UInt64,
        generation: UInt64,
        binding: EventOutboxConnectionBinding,
        discardedTextEdits: [SRUIPendingTextEditRef]? = nil,
        requireExactTextMatch: Bool = true,
        onTextEditsCanceled: TextEditCancellationHandler? = nil
    ) async throws -> SameSessionResumePreparation? {
        guard activeConnectionBinding == binding,
              activeResumeGeneration == generation,
              let sessionIncarnation = activeSessionIncarnation else {
            return nil
        }
        if let discardedTextEdits {
            let confirming = discardedTextEdits.compactMap(PendingTextEditDescriptor.init(wire:))
            if requireExactTextMatch, confirming.count != discardedTextEdits.count {
                throw EventOutboxError.textEditDiscardMismatch
            }
            // Validate before the destructive MainActor callback. The mutation revalidates after
            // the hop so a superseding lifecycle still fails closed.
            let canceled = try textEditsToCancel(
                confirming: confirming,
                requireExactMatch: requireExactTextMatch
            )
            if let onTextEditsCanceled {
                guard await performNativeTextLifecycle(
                    binding: binding,
                    sessionIncarnation: sessionIncarnation,
                    resumeGeneration: generation,
                    handler: { onTextEditsCanceled(canceled) }
                ) else {
                    return nil
                }
            }
            try cancelAssignedTextEdits(
                confirming: confirming,
                requireExactMatch: requireExactTextMatch
            )
        }
        activeSessionId = id
        let frontierSettledTextEdits = acknowledgeEvents(
            throughSeq: lastProcessedEventSeq
        ).filter { $0.eventType == .EVENT_TEXT_EDIT }
        // Once a resume frontier is accepted, an undecided old transport slot is replayable and
        // can no longer be rolled back by a delayed pre-resume controller task.
        lifecycleAuthorizedPreparedTextEdits.merge(
            lifecycleSuspendedPreparedTextEdits,
            uniquingKeysWith: { current, _ in current }
        )
        lifecycleSuspendedPreparedTextEdits.removeAll(keepingCapacity: true)
        return SameSessionResumePreparation(
            sessionId: id,
            generation: generation,
            binding: binding,
            sessionIncarnation: sessionIncarnation,
            frontierSettledTextEdits: frontierSettledTextEdits,
            assignedTextEdits: assignedTextEditEvents()
        )
    }

    /// Revalidates a prepared same-session transition after MainActor adopted its assignments,
    /// then replays retained identities in allocation order.
    func completeSameSessionResume(
        _ preparation: SameSessionResumePreparation,
        via transport: any Transport,
        enableNewEventsAfterReplay: Bool,
        onReplayFailure: (@Sendable (String) async -> Void)? = nil
    ) async throws -> Bool {
        guard activeConnectionBinding == preparation.binding,
              activeSessionIncarnation == preparation.sessionIncarnation,
              activeResumeGeneration == preparation.generation,
              activeSessionId == preparation.sessionId else {
            return false
        }
        try await resendPendingEvents(
            binding: preparation.binding,
            sessionIncarnation: preparation.sessionIncarnation,
            via: transport
        )
        guard activeConnectionBinding == preparation.binding,
              activeSessionIncarnation == preparation.sessionIncarnation,
              activeResumeGeneration == preparation.generation,
              activeSessionId == preparation.sessionId else {
            return false
        }
        acceptsNewEvents = enableNewEventsAfterReplay
        signalTextLaneStateChange()
        startReplayRetryLoop(
            generation: preparation.generation,
            binding: preparation.binding,
            sessionIncarnation: preparation.sessionIncarnation,
            via: transport,
            onFailure: onReplayFailure
        )
        if enableNewEventsAfterReplay {
            activeResumeGeneration = nil
            pendingResumeFinalizationGeneration = preparation.generation
        }
        return true
    }

    /// Activates a freshly bootstrapped session for the current connection binding.
    ///
    /// Returns `false` when the binding was superseded while the handshake or snapshot commit
    /// was in flight. On success, new events may be allocated only by callers presenting this
    /// binding; stale handles remain permanently rejected.
    @discardableResult
    public func confirmFreshSession(
        id: String,
        binding: EventOutboxConnectionBinding
    ) async -> Bool {
        guard activeConnectionBinding == binding, activeResumeGeneration == nil else {
            return false
        }
        if let ownership = resyncRenderOwnership {
            await resyncRenderFence.invalidate(ownership.token)
            guard activeConnectionBinding == binding, activeResumeGeneration == nil,
                  resyncRenderOwnership?.binding == ownership.binding,
                  resyncRenderOwnership?.token == ownership.token else {
                return false
            }
            resyncRenderOwnership = nil
            applyFullResyncTextBoundaryState()
        }

        // The current HELLO binding owns this decision: retire the prior transport lease while
        // retaining unsettled intents for explicit acknowledgement or a later resume.
        cancelReplayRetryLoop()
        pendingResumeFinalizationGeneration = nil
        activeSessionId = id
        acceptsNewEvents = true
        signalTextLaneStateChange()
        return true
    }

    /// Blocks new event allocation until a bootstrap or resync snapshot commits (§15, §18).
    @discardableResult
    func suspendNewEvents(binding: EventOutboxConnectionBinding) -> Bool {
        guard activeConnectionBinding == binding else { return false }
        acceptsNewEvents = false
        signalTextLaneStateChange()
        return true
    }

    /// Terminal suspension also cancels the current transport writer. Retained event identities
    /// and text drafts remain replayable, while interaction callbacks are free to stage newer
    /// drafts without waiting for an unresponsive transport.
    func suspendForTeardown(binding: EventOutboxConnectionBinding) -> Bool {
        guard suspendNewEvents(binding: binding) else { return false }
        cancelReplayRetryLoop()
        cancelPendingWrites()
        return true
    }
    /// Applies the event frontier for a live-session resync without an outstanding resume attempt.
    ///
    /// Does not cancel the replay retry loop: unsettled events keep retrying until settlement
    /// advances the frontier (§18.2).
    @discardableResult
    func applyLiveResyncFrontier(
        lastProcessedEventSeq: UInt64,
        binding: EventOutboxConnectionBinding
    ) async -> Bool {
        guard activeConnectionBinding == binding, activeResumeGeneration == nil else {
            return false
        }
        acceptsNewEvents = false
        signalTextLaneStateChange()
        if let ownership = resyncRenderOwnership {
            await resyncRenderFence.invalidate(ownership.token)
            guard activeConnectionBinding == binding, activeResumeGeneration == nil,
                  resyncRenderOwnership?.binding == ownership.binding,
                  resyncRenderOwnership?.token == ownership.token else {
                return false
            }
            resyncRenderOwnership = nil
        }
        acknowledgeEvents(throughSeq: lastProcessedEventSeq)
        return true
    }

    /// Applies a live same-session frontier only when dropping assigned text edits cannot create a
    /// hole in the server receive window. `discarded_text_edits` belongs to CLIENT_RESUME and is
    /// intentionally absent here: the cumulative frontier is the live proof of delivery.
    ///
    /// An edit above the frontier keeps its retained identity and forces the normal resume
    /// cancellation handshake, where the server atomically settles and echoes that identity
    /// (§18.2, §18.3). A reconnect generation that already superseded this controller refuses the
    /// whole transition without mutating the newer attempt.
    @discardableResult
    func applyLiveSameSessionResync(
        lastProcessedEventSeq: UInt64,
        binding: EventOutboxConnectionBinding,
        onTextEditsCanceled: TextEditCancellationHandler? = nil
    ) async throws -> LiveSameSessionResyncDecision {
        guard activeConnectionBinding == binding,
              activeResumeGeneration == nil,
              let sessionIncarnation = activeSessionIncarnation else {
            return .superseded
        }

        // Close allocation before any MainActor hop. Drafts may still coalesce while suspended;
        // no event can allocate and invalidate the frontier proof below.
        acceptsNewEvents = false
        signalTextLaneStateChange()
        var assigned = assignedTextEditDescriptors()
        guard assigned.allSatisfy({ $0.eventSeq <= lastProcessedEventSeq }) else {
            return .resumeRequired
        }

        if let ownership = resyncRenderOwnership {
            await resyncRenderFence.invalidate(ownership.token)
            guard activeConnectionBinding == binding,
                  activeSessionIncarnation == sessionIncarnation,
                  activeResumeGeneration == nil,
                  resyncRenderOwnership?.generation == ownership.generation,
                  resyncRenderOwnership?.binding == binding,
                  resyncRenderOwnership?.token == ownership.token else {
                return .superseded
            }
            resyncRenderOwnership = nil

            // Actor reentrancy cannot allocate while suspended, but re-read the owned set after the
            // hop so exact cancellation is based on current state rather than a stale capture.
            assigned = assignedTextEditDescriptors()
            guard assigned.allSatisfy({ $0.eventSeq <= lastProcessedEventSeq }) else {
                return .resumeRequired
            }
        }

        // Native cleanup linearizes before actor mutation. A newer binding that wins the
        // MainActor hop preserves both its editor state and the retained outbox identities.
        if let onTextEditsCanceled {
            guard await performNativeTextLifecycle(
                binding: binding,
                sessionIncarnation: sessionIncarnation,
                resumeGeneration: nil,
                handler: { onTextEditsCanceled(assigned) }
            ) else {
                return .superseded
            }
        }
        // Every assigned sequence is covered by the server frontier, so exact local cancellation
        // cannot manufacture a receive-window acknowledgement for an unprocessed event.
        try cancelAssignedTextEdits(confirming: assigned, requireExactMatch: true)
        acknowledgeEvents(throughSeq: lastProcessedEventSeq)
        return .applied(canceledTextEdits: assigned.map { $0.toWire() })
    }

    /// Abandons pending intents and binds a replacement incarnation without a resume attempt (§18).
    @discardableResult
    func applyReplacementFrontier(
        id: String,
        lastProcessedEventSeq: UInt64,
        binding: EventOutboxConnectionBinding,
        onTextEditingReset: ReplacementTextEditingResetHandler? = nil
    ) async -> Bool {
        guard activeConnectionBinding == binding, activeResumeGeneration == nil else {
            return false
        }
        acceptsNewEvents = false
        signalTextLaneStateChange()
        if let ownership = resyncRenderOwnership {
            await resyncRenderFence.invalidate(ownership.token)
            guard activeConnectionBinding == binding, activeResumeGeneration == nil,
                  resyncRenderOwnership?.binding == ownership.binding,
                  resyncRenderOwnership?.token == ownership.token else {
                return false
            }
            resyncRenderOwnership = nil
        }
        guard let replacementIncarnation = advanceSessionIncarnation(
            binding: binding
        ) else {
            return false
        }
        if let onTextEditingReset {
            guard await performNativeTextLifecycle(
                binding: binding,
                sessionIncarnation: replacementIncarnation,
                resumeGeneration: nil,
                handler: { onTextEditingReset(replacementIncarnation) }
            ) else {
                return false
            }
        }
        applyReplacementFrontierState(
            id: id,
            lastProcessedEventSeq: lastProcessedEventSeq,
            sessionIncarnation: replacementIncarnation
        )
        return activeConnectionBinding == binding && activeResumeGeneration == nil
    }
    /// Applies one selective acknowledgement plus the server's contiguous cumulative frontier.
    ///
    /// Both wire identities are checked here rather than by the caller: the ack settles state this
    /// actor owns, so a draining connection, an expired incarnation, or another client instance
    /// must be refused inside the same isolated step that would otherwise mutate it (§18, §18.2).
    /// Returns false without any mutation when the identity does not bind.
    @discardableResult
    public func settleAcknowledgement(
        binding: EventOutboxConnectionBinding,
        sessionIncarnation suppliedIncarnation: EventOutboxSessionIncarnation? = nil,
        clientInstanceId ackClientInstanceId: ClientInstanceId,
        eventId: EventId,
        settledEventSeq: UInt64? = nil,
        throughSeq seq: UInt64,
        sessionId: String,
        revisionAfterEffect: UInt64? = nil,
        textEditRejected: Bool = false
    ) -> EventAcknowledgementSettlement {
        guard activeConnectionBinding == binding,
              let currentIncarnation = activeSessionIncarnation,
              suppliedIncarnation == nil || suppliedIncarnation == currentIncarnation else {
            return .staleConnection
        }
        guard ackClientInstanceId == clientInstanceId else { return .unbound }
        // `session_id` is required on every ack (§18.2): an empty one proves nothing about which
        // incarnation settled the event, so it can never retire an intent.
        guard !sessionId.isEmpty,
              let activeSessionId,
              sessionId == activeSessionId else {
            return .unbound
        }
        if let settledEventSeq, settledEventSeq > currentEventSeq {
            return .unbound
        }
        let eventById = pendingEvents[eventId]
        let eventBySequence = settledEventSeq.flatMap { settledSeq in
            pendingEvents.values.first { $0.eventSeq == settledSeq }
        }
        let event: Event?
        if let settledEventSeq {
            if textEditRejected {
                // The bounded marker for an invalid ID is not an identity key and may equal some
                // other valid event ID. The explicit sequence is authoritative for rejections.
                event = eventBySequence
            } else if let eventById {
                guard eventById.eventSeq == settledEventSeq else { return .unbound }
                event = eventById
            } else {
                // A normal ack may already name an event retired by an earlier cumulative ack, but
                // it must not use a sequence to retire a different still-pending identifier.
                guard eventBySequence == nil else { return .unbound }
                event = nil
            }
        } else {
            event = eventById
        }
        // With an explicit `settled_event_seq` the sequence — not the echoed `event_id` — names the
        // slot to retire (§18.2). A rejection's `event_id` may be a bounded marker that collides
        // with an unrelated valid identifier, so an unresolved sequence means that slot was already
        // settled and nothing may be retired by identity.
        let settledIdentity: EventId? = settledEventSeq == nil ? eventId : event?.eventId
        // The cumulative frontier is normative settlement (§18.2). Keeping covered text edits in
        // the retry set can outlive the server's bounded result record and turn replay into a
        // permanent OutsideReceiveWindow reconnect loop.
        var settledEvents = acknowledgeEvents(throughSeq: seq)
        if let event, !settledEvents.contains(where: { $0.eventId == event.eventId }) {
            settledEvents.append(event)
            settledEvents.sort { $0.eventSeq < $1.eventSeq }
        }

        // The named edit has an exact outcome. Earlier edits settled only by the cumulative
        // frontier are handled conservatively as rejected: after this revision renders, native
        // state converges to the last authoritative value unless a newer local draft exists.
        let effectRevision = revisionAfterEffect ?? 0
        for settledEvent in settledEvents where settledEvent.eventType == .EVENT_TEXT_EDIT {
            let barrier = TextEditAcknowledgementBarrier(
                nodeId: settledEvent.nodeId,
                eventId: settledEvent.eventId,
                revisionAfterEffect: effectRevision,
                rejected: settledEvent.eventId == settledIdentity ? textEditRejected : true
            )
            if let current = textAcknowledgementBarriers[settledEvent.nodeId] {
                if current.revisionAfterEffect <= effectRevision {
                    textAcknowledgementBarriers[settledEvent.nodeId] = barrier
                }
            } else {
                textAcknowledgementBarriers[settledEvent.nodeId] = barrier
            }
        }

        if let settledIdentity {
            acknowledgeEvent(id: settledIdentity)
        }
        return EventAcknowledgementSettlement(
            connectionBound: true,
            bound: true,
            event: event,
            settledEvents: settledEvents
        )
    }

    /// Acknowledges every event through the server's highest contiguous settled sequence.
    ///
    /// Private for the same reason as `acknowledgeEvent(id:)`: reachable from the wire only
    /// through the identity-checked `settleAcknowledgement`, and internally only from a resume or
    /// resync decision the generation latch already bound to this outbox (§18, §18.2).
    @discardableResult
    private func acknowledgeEvents(throughSeq seq: UInt64) -> [Event] {
        guard seq > _lastAckedEventSeq, seq <= currentEventSeq else { return [] }

        _lastAckedEventSeq = seq
        acknowledgedOutOfOrder = Set(acknowledgedOutOfOrder.filter { $0 > seq })
        let settled = pendingEvents.values
            .filter { $0.eventSeq <= seq }
            .sorted { $0.eventSeq < $1.eventSeq }
        for event in settled {
            preparedTextEditAuthorizationFence.retire(event.eventId)
            pendingEvents.removeValue(forKey: event.eventId)
        }
        pendingOrder.removeAll { pendingEvents[$0] == nil }
        advanceContiguousAcknowledgement()
        if !settled.isEmpty {
            signalTextLaneStateChange()
        }
        cancelReplayRetryLoopIfSettled()
        return settled
    }

    /// Abandons every intent from an expired session and aligns sequencing with the
    /// authoritative replacement session's receive frontier (§18).
    func prepareReplacedSession(
        id: String,
        lastProcessedEventSeq: UInt64,
        generation: UInt64,
        binding: EventOutboxConnectionBinding,
        onTextEditingReset: ReplacementTextEditingResetHandler? = nil
    ) async -> Bool {
        guard activeConnectionBinding == binding,
              activeResumeGeneration == generation else {
            return false
        }
        if let ownership = resyncRenderOwnership {
            await resyncRenderFence.invalidate(ownership.token)
            guard activeConnectionBinding == binding,
                  activeResumeGeneration == generation,
                  resyncRenderOwnership?.generation == generation,
                  resyncRenderOwnership?.binding == binding,
                  resyncRenderOwnership?.token == ownership.token else {
                return false
            }
            resyncRenderOwnership = nil
        }
        guard let replacementIncarnation = advanceSessionIncarnation(
            binding: binding
        ) else {
            return false
        }
        if let onTextEditingReset {
            guard await performNativeTextLifecycle(
                binding: binding,
                sessionIncarnation: replacementIncarnation,
                resumeGeneration: generation,
                handler: { onTextEditingReset(replacementIncarnation) }
            ) else {
                return false
            }
        }
        applyReplacementFrontierState(
            id: id,
            lastProcessedEventSeq: lastProcessedEventSeq,
            sessionIncarnation: replacementIncarnation
        )
        return activeConnectionBinding == binding && activeResumeGeneration == generation
    }

    /// Re-enables allocation after a HELLO catch-up snapshot with no resume attempt (§15, §18).
    ///
    /// Refuses while any controller holds an outstanding reconnect generation: that latch exists
    /// precisely to keep new events from being allocated before the continuity decision arrives,
    /// and re-opening it from an unrelated catch-up would bypass it (§18).
    @discardableResult
    func allowNewEvents(binding: EventOutboxConnectionBinding) -> Bool {
        guard activeConnectionBinding == binding,
              activeResumeGeneration == nil,
              resyncRenderOwnership == nil,
              pendingResyncBoundaryCleanup == nil,
              liveRenderOwnership == nil,
              pendingLiveRenderInvalidation == nil,
              resumeRecoveryRenderOwnership == nil,
              pendingResumeRecoveryRenderInvalidation == nil else {
            return false
        }
        acceptsNewEvents = true
        signalTextLaneStateChange()
        return true
    }

    /// Enables new events only after the snapshot for the current reconnect generation commits.
    func finishResync(generation: UInt64) -> Bool {
        guard activeResumeGeneration == generation,
              resyncRenderOwnership == nil,
              pendingResyncBoundaryCleanup == nil,
              liveRenderOwnership == nil,
              pendingLiveRenderInvalidation == nil,
              resumeRecoveryRenderOwnership == nil,
              pendingResumeRecoveryRenderInvalidation == nil else {
            return false
        }
        acceptsNewEvents = true
        signalTextLaneStateChange()
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
    func stopResumeWork(generation: UInt64) async {
        guard ownsResumeWork(generation: generation) else { return }
        let transitionEpoch = beginResyncBoundaryTransition()
        let ownsRender = resyncRenderOwnership?.generation == generation
        let boundaryCleanup: PendingResyncBoundaryCleanup?
        if pendingResyncBoundaryCleanup != nil || ownsRender {
            boundaryCleanup = adoptOrBeginResyncBoundaryCleanup()
        } else {
            boundaryCleanup = nil
        }
        let recoveryInvalidation: PendingResumeRecoveryRenderInvalidation?
        if pendingResumeRecoveryRenderInvalidation?.generation == generation
            || resumeRecoveryRenderOwnership?.generation == generation {
            recoveryInvalidation = adoptOrBeginResumeRecoveryRenderInvalidation()
        } else {
            recoveryInvalidation = nil
        }

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
        signalTextLaneStateChange()

        if let boundaryCleanup {
            let renderedBoundaryEpoch = await boundaryCleanup.invalidation.value
            _ = finishResyncBoundaryCleanup(
                boundaryCleanup,
                renderedBoundaryEpoch: renderedBoundaryEpoch,
                transitionEpoch: transitionEpoch
            )
        }
        if let recoveryInvalidation {
            _ = await recoveryInvalidation.invalidation.value
            _ = finishResumeRecoveryRenderInvalidation(
                recoveryInvalidation,
                transitionEpoch: transitionEpoch
            )
        }
    }

    private func ownsResumeWork(generation: UInt64) -> Bool {
        activeResumeGeneration == generation
            || pendingResumeFinalizationGeneration == generation
            || replayLease?.resumeScope == generation
    }

    private func ownsResyncScope(
        generation: UInt64?,
        binding: EventOutboxConnectionBinding
    ) -> Bool {
        activeConnectionBinding == binding
            && activeResumeGeneration == generation
            && pendingResyncBoundaryCleanup == nil
            && liveRenderOwnership == nil
            && pendingLiveRenderInvalidation == nil
            && resumeRecoveryRenderOwnership == nil
            && pendingResumeRecoveryRenderInvalidation == nil
    }

    private func beginResyncBoundaryTransition() -> UInt64 {
        resyncBoundaryTransitionEpoch &+= 1
        return resyncBoundaryTransitionEpoch
    }

    private func adoptOrBeginResyncBoundaryCleanup() -> PendingResyncBoundaryCleanup? {
        if let pendingResyncBoundaryCleanup {
            return pendingResyncBoundaryCleanup
        }
        guard let ownership = resyncRenderOwnership else { return nil }

        resyncRenderOwnership = nil
        let fence = resyncRenderFence
        let invalidation = Task { @MainActor in
            fence.invalidate(ownership.token)
        }
        let cleanup = PendingResyncBoundaryCleanup(
            token: ownership.token,
            invalidation: invalidation,
            acknowledgementBarriersAtStart: textAcknowledgementBarriers
        )
        pendingResyncBoundaryCleanup = cleanup
        return cleanup
    }

    @discardableResult
    private func finishResyncBoundaryCleanup(
        _ cleanup: PendingResyncBoundaryCleanup,
        renderedBoundaryEpoch: UInt64?,
        transitionEpoch: UInt64
    ) -> Bool {
        guard resyncBoundaryTransitionEpoch == transitionEpoch,
              pendingResyncBoundaryCleanup?.token == cleanup.token else {
            return false
        }

        if renderedBoundaryEpoch != nil {
            applyFullResyncTextBoundaryState()
        } else {
            for (nodeId, barrier) in cleanup.acknowledgementBarriersAtStart
            where textAcknowledgementBarriers[nodeId] == barrier {
                textAcknowledgementBarriers.removeValue(forKey: nodeId)
            }
            signalTextLaneStateChange()
        }
        pendingResyncBoundaryCleanup = nil
        return true
    }

    private func adoptOrBeginLiveRenderInvalidation() -> PendingLiveRenderInvalidation? {
        if let pendingLiveRenderInvalidation {
            return pendingLiveRenderInvalidation
        }
        guard let ownership = liveRenderOwnership else { return nil }

        liveRenderOwnership = nil
        let fence = resyncRenderFence
        let invalidation = Task { @MainActor in
            fence.invalidate(ownership.token)
        }
        let cleanup = PendingLiveRenderInvalidation(
            token: ownership.token,
            invalidation: invalidation
        )
        pendingLiveRenderInvalidation = cleanup
        return cleanup
    }

    @discardableResult
    private func finishLiveRenderInvalidation(
        _ cleanup: PendingLiveRenderInvalidation,
        transitionEpoch: UInt64
    ) -> Bool {
        guard resyncBoundaryTransitionEpoch == transitionEpoch,
              pendingLiveRenderInvalidation?.token == cleanup.token else {
            return false
        }
        pendingLiveRenderInvalidation = nil
        return true
    }

    private func adoptOrBeginResumeRecoveryRenderInvalidation()
        -> PendingResumeRecoveryRenderInvalidation? {
        if let pendingResumeRecoveryRenderInvalidation {
            return pendingResumeRecoveryRenderInvalidation
        }
        guard let ownership = resumeRecoveryRenderOwnership else { return nil }

        resumeRecoveryRenderOwnership = nil
        let fence = resyncRenderFence
        let invalidation = Task { @MainActor in
            fence.invalidate(ownership.token)
        }
        let cleanup = PendingResumeRecoveryRenderInvalidation(
            generation: ownership.generation,
            token: ownership.token,
            invalidation: invalidation
        )
        pendingResumeRecoveryRenderInvalidation = cleanup
        return cleanup
    }

    @discardableResult
    private func finishResumeRecoveryRenderInvalidation(
        _ cleanup: PendingResumeRecoveryRenderInvalidation,
        transitionEpoch: UInt64
    ) -> Bool {
        guard resyncBoundaryTransitionEpoch == transitionEpoch,
              pendingResumeRecoveryRenderInvalidation?.token == cleanup.token else {
            return false
        }
        pendingResumeRecoveryRenderInvalidation = nil
        return true
    }

    private func ownsResumeRecoveryRender(
        binding: EventOutboxConnectionBinding,
        generation: UInt64,
        renderToken: UUID
    ) -> Bool {
        activeConnectionBinding == binding
            && activeResumeGeneration == generation
            && resumeRecoveryRenderOwnership?.binding == binding
            && resumeRecoveryRenderOwnership?.generation == generation
            && resumeRecoveryRenderOwnership?.token == renderToken
            && pendingResumeRecoveryRenderInvalidation == nil
    }

    /// Retires every acknowledgement barrier at the native full-resync boundary.
    ///
    /// The lane-epoch floor that fences callbacks queued by the old native controls is owned by
    /// `TextEditingSession.discardUnresolvedEditsForResync()`, not by this outbox (§18.3).
    private func applyFullResyncTextBoundaryState() {
        textAcknowledgementBarriers.removeAll(keepingCapacity: true)
        signalTextLaneStateChange()
    }

    private func applyReplacementFrontierState(
        id: String,
        lastProcessedEventSeq: UInt64,
        sessionIncarnation: EventOutboxSessionIncarnation
    ) {
        precondition(
            activeSessionIncarnation == sessionIncarnation,
            "replacement frontier lost its session incarnation"
        )
        activeSessionIncarnation = sessionIncarnation
        cancelReplayRetryLoop()
        pendingResumeFinalizationGeneration = nil
        activeSessionId = id
        acceptsNewEvents = false
        currentEventSeq = lastProcessedEventSeq
        _lastAckedEventSeq = lastProcessedEventSeq
        pendingEvents.removeAll(keepingCapacity: true)
        pendingOrder.removeAll(keepingCapacity: true)
        acknowledgedOutOfOrder.removeAll(keepingCapacity: true)
        textAcknowledgementBarriers.removeAll(keepingCapacity: true)
        signalTextLaneStateChange()
        cancelPendingWrites()
        lifecycleSuspendedPreparedTextEdits.removeAll(keepingCapacity: true)
        lifecycleAuthorizedPreparedTextEdits.removeAll(keepingCapacity: true)
        preparedTextEditAuthorizationFence.reset()
    }

    private func startReplayRetryLoop(
        generation: UInt64,
        binding: EventOutboxConnectionBinding,
        sessionIncarnation: EventOutboxSessionIncarnation,
        via transport: any Transport,
        onFailure: (@Sendable (String) async -> Void)?
    ) {
        cancelReplayRetryLoop()
        guard activeConnectionBinding == binding,
              activeResumeGeneration == generation,
              pendingEvents.isEmpty == false else {
            return
        }

        // The transport is valid for this lease's lifetime. SessionController invalidates resume
        // work before replacing or closing the transport; the replay loop never owns teardown.
        replayLease = pendingEventReplayLoop.start(
            resumeScope: generation,
            replay: { [weak self] lease in
                guard let self else { return false }
                return try await self.replayPendingEvents(
                    for: lease,
                    binding: binding,
                    sessionIncarnation: sessionIncarnation,
                    via: transport
                )
            },
            onFailure: onFailure,
            onFinish: { [weak self] lease in
                await self?.finishReplayRetryLoop(lease)
            }
        )
    }

    private func replayPendingEvents(
        for lease: PendingEventReplayLoop.Lease,
        binding: EventOutboxConnectionBinding,
        sessionIncarnation: EventOutboxSessionIncarnation,
        via transport: any Transport
    ) async throws -> Bool {
        // Lease identity *is* resume ownership here: `beginResumeAttempt()` and
        // `stopResumeWork(generation:)` cancel the lease of the generation they supersede, which
        // clears `replayLease` before any newer attempt can run. Re-deriving ownership from the
        // latch instead would stop the loop after `commitResumeWork(generation:)` releases it,
        // stranding events that are still unsettled (§18, §18.2).
        guard activeConnectionBinding == binding,
              activeSessionIncarnation == sessionIncarnation,
              replayLease == lease,
              pendingEventReplayLoop.isActive(lease),
              pendingEvents.isEmpty == false else {
            return false
        }
        try Task.checkCancellation()
        try await resendPendingEvents(
            binding: binding,
            sessionIncarnation: sessionIncarnation,
            via: transport
        )
        // Re-checked after the suspension: a reconnect that superseded this generation while the
        // replay was in flight owns the retry set now, and this lease must not touch it (§18).
        // `Lease` equality covers `resumeScope`, so still being the current lease is itself proof
        // that this generation still owns the retry set.
        guard activeConnectionBinding == binding,
              activeSessionIncarnation == sessionIncarnation,
              replayLease == lease,
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
              outstandingSpan < UInt64(maxPendingEvents),
              pendingEvents.count < maxPendingEvents else {
            throw EventOutboxError.sequenceWindowExhausted(limit: maxPendingEvents)
        }
    }

    private func waitForPreparedTextEditResolution(
        binding: EventOutboxConnectionBinding,
        sessionIncarnation: EventOutboxSessionIncarnation
    ) async throws {
        while !preparedTextEditSends.isEmpty {
            guard activeConnectionBinding == binding,
                  activeSessionIncarnation == sessionIncarnation,
                  acceptsNewEvents else {
                throw EventOutboxError.resumeNotConfirmed
            }
            let version = textLaneStateVersion
            try await waitForTextLaneStateChange(binding: binding, after: version)
        }
    }

    private func waitForTextLaneStateChange(
        binding: EventOutboxConnectionBinding,
        after version: UInt64
    ) async throws {
        let waiterId = UUID()
        try Task.checkCancellation()
        await withTaskCancellationHandler {
            await withCheckedContinuation { continuation in
                guard !Task.isCancelled,
                      activeConnectionBinding == binding,
                      acceptsNewEvents,
                      textLaneStateVersion == version else {
                    continuation.resume()
                    return
                }
                textLaneWaiters[waiterId] = continuation
            }
        } onCancel: {
            Task { await self.cancelTextLaneWaiter(waiterId) }
        }
        try Task.checkCancellation()
    }

    private func cancelTextLaneWaiter(_ waiterId: UUID) {
        textLaneWaiters.removeValue(forKey: waiterId)?.resume()
    }

    private func signalTextLaneStateChange() {
        textLaneStateVersion &+= 1
        let waiters = textLaneWaiters.values
        textLaneWaiters.removeAll(keepingCapacity: true)
        for waiter in waiters {
            waiter.resume()
        }
    }

    private func performNativeTextLifecycle(
        binding: EventOutboxConnectionBinding,
        sessionIncarnation: EventOutboxSessionIncarnation,
        resumeGeneration: UInt64?,
        handler: @escaping @MainActor @Sendable () -> Void
    ) async -> Bool {
        if let nativeTextLifecycleWillHopForTesting {
            await nativeTextLifecycleWillHopForTesting()
        }
        guard activeConnectionBinding == binding,
              activeSessionIncarnation == sessionIncarnation,
              activeResumeGeneration == resumeGeneration else {
            return false
        }
        let performed: Bool? = await textLifecycleFence.performIfActive(
            sessionIncarnation: sessionIncarnation,
            handler: {
                handler()
                return true
            }
        )
        guard performed == true else {
            return false
        }
        return activeConnectionBinding == binding
            && activeSessionIncarnation == sessionIncarnation
            && activeResumeGeneration == resumeGeneration
    }

    private func mayTransmitRetainedEvent(
        _ event: Event,
        binding: EventOutboxConnectionBinding,
        sessionIncarnation: EventOutboxSessionIncarnation,
        resumeEpoch: UInt64,
        sessionId: String?
    ) -> Bool {
        activeConnectionBinding == binding
            && activeSessionIncarnation == sessionIncarnation
            && pendingEvents[event.eventId] == event
            && lastIssuedResumeGeneration == resumeEpoch
            && activeSessionId == sessionId
            && acceptsNewEvents
    }

    /// Validation and transport call initiation share one actor turn. Replacement therefore
    /// linearizes either before the old write is refused or after that write has begun.
    private func sendRetainedEventIfActive(
        _ event: Event,
        framedBytes: Data,
        binding: EventOutboxConnectionBinding,
        sessionIncarnation: EventOutboxSessionIncarnation,
        resumeEpoch: UInt64,
        sessionId: String?,
        via transport: any Transport
    ) async throws -> Bool {
        guard mayTransmitRetainedEvent(
            event,
            binding: binding,
            sessionIncarnation: sessionIncarnation,
            resumeEpoch: resumeEpoch,
            sessionId: sessionId
        ) else {
            return false
        }
        try Task.checkCancellation()
        try await transport.send(data: framedBytes, logicalClass: .input)
        return true
    }

    private func sendReplayEventIfActive(
        _ event: Event,
        binding: EventOutboxConnectionBinding,
        sessionIncarnation: EventOutboxSessionIncarnation,
        via transport: any Transport
    ) async throws -> Bool {
        guard activeConnectionBinding == binding,
              activeSessionIncarnation == sessionIncarnation,
              pendingEvents[event.eventId] == event else {
            return false
        }
        var message = SRUIMessage()
        message.event = event.toWire()
        let framedBytes = try SRUIFraming.encodeFramed(message)
        try Task.checkCancellation()
        try await transport.send(data: framedBytes, logicalClass: .input)
        return true
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
    private func cancelPendingWrites(
        retainUnauthorizedPreparedTextEdits: Bool = true
    ) {
        let hadPreparedSlots = !preparedTextEditSends.isEmpty
            || !authorizedPreparedTextEditSends.isEmpty
            || !lifecycleSuspendedPreparedTextEdits.isEmpty
        var unauthorizedEventsToRollback = [Event]()
        for (token, retained) in preparedTextEditSends {
            _ = retained.gate.cancelIfUnresolved()
            retained.task.cancel()
            guard pendingEvents[retained.event.eventId] == retained.event else { continue }
            if retainUnauthorizedPreparedTextEdits {
                lifecycleSuspendedPreparedTextEdits[token] = retained.event
            } else if preparedTextEditAuthorizationFence.consumeAssignedForLifecycle(
                retained.event.eventId
            ) {
                lifecycleAuthorizedPreparedTextEdits[token] = retained.event
            } else {
                unauthorizedEventsToRollback.append(retained.event)
            }
        }
        preparedTextEditSends.removeAll(keepingCapacity: true)

        if !retainUnauthorizedPreparedTextEdits {
            for (token, event) in lifecycleSuspendedPreparedTextEdits {
                if preparedTextEditAuthorizationFence.consumeAssignedForLifecycle(event.eventId) {
                    lifecycleAuthorizedPreparedTextEdits[token] = event
                } else {
                    unauthorizedEventsToRollback.append(event)
                }
            }
            lifecycleSuspendedPreparedTextEdits.removeAll(keepingCapacity: true)
            for event in unauthorizedEventsToRollback.sorted(
                by: { $0.eventSeq > $1.eventSeq }
            ) {
                guard pendingOrder.last == event.eventId,
                      pendingEvents[event.eventId] == event,
                      currentEventSeq == event.eventSeq else {
                    continue
                }
                pendingEvents.removeValue(forKey: event.eventId)
                pendingOrder.removeLast()
                currentEventSeq -= 1
            }
        }

        for (token, retained) in authorizedPreparedTextEditSends {
            retained.task.cancel()
            if pendingEvents[retained.event.eventId] == retained.event {
                lifecycleAuthorizedPreparedTextEdits[token] = retained.event
            }
        }
        authorizedPreparedTextEditSends.removeAll(keepingCapacity: true)
        if hadPreparedSlots {
            signalTextLaneStateChange()
        }
        sendTail?.cancel()
        sendTail = nil
    }

    /// Retains a new event without evicting an earlier unacknowledged sequence.
    private func retainPending(_ event: Event) throws {
        if let retained = pendingEvents[event.eventId] {
            // A retry is byte-identical by definition (§18.2). Overwriting the entry instead would
            // let a second, semantically different intent inherit the first one's idempotency key,
            // and the server would answer it from the result cache without ever running it.
            guard retained == event else {
                throw EventOutboxError.pendingEventIdentityConflict(eventId: event.eventId)
            }
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
