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
        guard let seq = EditSeq(wire.editSeq), !wire.eventID.isEmpty, wire.eventSeq > 0 else {
            return nil
        }
        self.init(
            eventId: EventId(wire.eventID),
            eventSeq: wire.eventSeq,
            nodeId: NodeId(wire.nodeID),
            editSeq: seq
        )
    }
}

/// Outcome of one identity-checked acknowledgement (§18.2, §22.6).
public struct EventAcknowledgementSettlement: Equatable, Sendable {
    public var bound: Bool
    /// Event named by event_id, when it was still retained.
    public var event: Event?
    /// Every event retired by the cumulative frontier or the selective event_id.
    public var settledEvents: [Event]

    public static let unbound = EventAcknowledgementSettlement(
        bound: false,
        event: nil,
        settledEvents: []
    )
}

/// A terminal text acknowledgement whose authoritative revision has not necessarily rendered yet.
struct TextEditAcknowledgementBarrier: Equatable, Sendable {
    var nodeId: NodeId
    var revisionAfterEffect: UInt64
    var rejected: Bool
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
    /// it instead of merely dropping the reference (§18). Scheduler FIFO inside the input lane
    /// does not replace this: increasing `event_seq` must reach the transport in allocation order
    /// (§18.2).
    private var sendTail: Task<Void, any Error>?
    private var textDrafts: [NodeId: TextEditDraft] = [:]
    private var assignedTextByNode: [NodeId: EventId] = [:]
    private var assignedTextByEventId: [EventId: NodeId] = [:]
    /// Highest correction/cancel epoch observed per node. Stale `queueTextEdit` Tasks
    /// with a lower epoch are dropped so unordered hops cannot resurrect a rejected draft.
    private var textLaneEpoch: [NodeId: UInt64] = [:]
    /// Session-wide floor advanced by an authoritative full resync. It also fences edits from
    /// newly mounted nodes whose IDs did not exist before the snapshot.
    private var textLaneEpochFloor: UInt64 = 0
    /// A successor for a node cannot be promoted until this acknowledgement's authoritative
    /// revision has reached the rendered replica.
    private var textAcknowledgementBarriers: [NodeId: TextEditAcknowledgementBarrier] = [:]

    private struct TextEditDraft: Equatable, Sendable {
        var nodeId: NodeId
        var text: String
        var editSeq: EditSeq
        var observedRevision: Revision
        var laneEpoch: UInt64
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
        via transport: any Transport,
        _ makeEvent: (UInt64, EventId) -> Event
    ) async throws -> Event {
        guard acceptsNewEvents else {
            throw EventOutboxError.resumeNotConfirmed
        }
        try ensureSequenceWindowCapacity()
        currentEventSeq += 1
        let event = makeEvent(currentEventSeq, generateEventId())
            .withClientInstanceId(clientInstanceId)
        try await sendEvent(event, via: transport)
        return event
    }

    /// Serializes and sends an event over the given transport (§16, §22).
    ///
    /// Internal because it accepts an already-allocated identity: only `allocateAndSend` may mint
    /// one, so allocation, retention, and transmission stay a single actor-isolated step.
    func sendEvent(_ event: Event, via transport: any Transport) async throws {
        var msg = SRUIMessage()
        msg.event = event.toWire()
        let framedBytes = try SRUIFraming.encodeFramed(msg)

        // Retain before the first suspension: a fast acknowledgement may arrive while send is
        // awaiting transport completion and must be able to remove this entry exactly once.
        try retainPending(event)
        let send = enqueueSend {
            try await transport.send(data: framedBytes, logicalClass: .input)
        }
        try await send.value
    }

    /// Constructs and sends an `ACTIVATE` event without creating a sequence beyond the window.
    @discardableResult
    public func sendActivate(nodeId: NodeId, observedRevision: Revision, via transport: any Transport) async throws -> Event {
        try await allocateAndSend(via: transport) { eventSeq, eventId in
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
    public func sendValueChanged(nodeId: NodeId, observedRevision: Revision, value: Value, via transport: any Transport) async throws -> Event {
        try await allocateAndSend(via: transport) { eventSeq, eventId in
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
    public func sendSelectionChanged(nodeId: NodeId, observedRevision: Revision, itemId: ItemId, via transport: any Transport) async throws -> Event {
        try await allocateAndSend(via: transport) { eventSeq, eventId in
            Event.selectionChanged(
                eventSeq: eventSeq,
                eventId: eventId,
                observedRevision: observedRevision,
                nodeId: nodeId,
                itemId: itemId
            )
        }
    }

    /// Queues a whole-value `TEXT_EDIT`. Drafts are retained while dispatch is suspended or
    /// another edit for the same node is already assigned (§18.3, §22.6).
    @discardableResult
    public func queueTextEdit(
        nodeId: NodeId,
        text: String,
        editSeq: EditSeq,
        observedRevision: Revision,
        via transport: any Transport,
        laneEpoch: UInt64 = 0
    ) async throws -> Event? {
        let minimumEpoch = max(textLaneEpochFloor, textLaneEpoch[nodeId] ?? 0)
        if laneEpoch < minimumEpoch {
            return nil
        }
        textDrafts[nodeId] = TextEditDraft(
            nodeId: nodeId,
            text: text,
            editSeq: editSeq,
            observedRevision: observedRevision,
            laneEpoch: laneEpoch
        )
        return try await promoteTextDraft(nodeId: nodeId, via: transport)
    }

    /// Promotes every coalesced draft that can enter the contiguous send window.
    /// Returns each newly allocated `TEXT_EDIT` so the text coordinator can record it as assigned.
    @discardableResult
    public func promoteReadyTextDrafts(via transport: any Transport) async throws -> [Event] {
        var promoted: [Event] = []
        for nodeId in Array(textDrafts.keys) {
            if let event = try await promoteTextDraft(nodeId: nodeId, via: transport) {
                promoted.append(event)
            }
        }
        return promoted
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

    public func invalidateTextDraft(nodeId: NodeId, laneEpoch: UInt64 = 0) {
        if laneEpoch >= (textLaneEpoch[nodeId] ?? 0) {
            textLaneEpoch[nodeId] = laneEpoch
        }
        textDrafts.removeValue(forKey: nodeId)
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

    /// Count of coalesced whole-value drafts that have not yet been allocated an `event_seq`.
    public var unsentTextDraftCount: Int {
        textDrafts.count
    }

    /// Selectively cancels assigned `TEXT_EDIT` events. A required exact match fails closed (§18.3).
    public func cancelAssignedTextEdits(
        confirming refs: [PendingTextEditDescriptor],
        requireExactMatch: Bool
    ) throws {
        let assigned = assignedTextEditDescriptors()
        if requireExactMatch {
            let assignedKeys = Set(assigned.map(Self.identityKey))
            let echoKeys = Set(refs.map(Self.identityKey))
            guard assignedKeys == echoKeys else {
                throw EventOutboxError.textEditDiscardMismatch
            }
        }
        let condemned = requireExactMatch ? refs : assigned
        textDrafts.removeAll(keepingCapacity: true)
        for ref in condemned {
            forgetAssignedText(eventId: ref.eventId)
            if let event = pendingEvents.removeValue(forKey: ref.eventId) {
                pendingOrder.removeAll { $0 == ref.eventId }
                recordSelectiveAcknowledgement(event.eventSeq)
            } else {
                recordSelectiveAcknowledgement(ref.eventSeq)
            }
        }
        cancelReplayRetryLoopIfSettled()
    }

    /// Cancels assigned `TEXT_EDIT` events only while `generation` still owns the reconnect latch.
    ///
    /// `nil` is the live-resync generation: it matches only when no controller holds the latch.
    /// A stale `SERVER RESYNC_REQUIRED` whose echoed descriptors still match the shared pending
    /// set must not cancel a newer attempt's in-flight edits (§18, §18.3). Returns `false`
    /// without mutation when the latch does not match, including before decoding the refs so a
    /// superseded attempt cannot fail the session closed on a discard mismatch.
    @discardableResult
    public func cancelAssignedTextEdits(
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

    public func discardUnsentTextDrafts() {
        textDrafts.removeAll(keepingCapacity: true)
    }

    /// Applies a hard full-resync boundary. Drafts from native controls that existed before the
    /// snapshot are discarded; callbacks from newly mounted controls carry laneEpoch (or later)
    /// and survive this actor hop. A snapshot is authoritative for every outstanding ack.
    func applyFullResyncTextBoundary(laneEpoch: UInt64?) {
        textAcknowledgementBarriers.removeAll(keepingCapacity: true)
        guard let laneEpoch else {
            textDrafts.removeAll(keepingCapacity: true)
            return
        }
        textLaneEpochFloor = max(textLaneEpochFloor, laneEpoch)
        for nodeID in Array(textDrafts.keys) {
            if let draft = textDrafts[nodeID], draft.laneEpoch < textLaneEpochFloor {
                textDrafts.removeValue(forKey: nodeID)
            }
        }
    }

    /// Releases acknowledgements only after their authoritative revision has rendered.
    func releaseTextAcknowledgements(through revision: UInt64) -> [TextEditAcknowledgementBarrier] {
        let ready = textAcknowledgementBarriers.values
            .filter { $0.revisionAfterEffect <= revision }
            .sorted { $0.nodeId.value < $1.nodeId.value }
        for barrier in ready {
            textAcknowledgementBarriers.removeValue(forKey: barrier.nodeId)
        }
        return ready
    }

    private static func identityKey(_ ref: PendingTextEditDescriptor) -> String {
        "\(ref.eventId.toHex()):\(ref.eventSeq):\(ref.nodeId.value):\(ref.editSeq.rawValue)"
    }

    private func promoteTextDraft(nodeId: NodeId, via transport: any Transport) async throws -> Event? {
        guard acceptsNewEvents else { return nil }
        guard assignedTextByNode[nodeId] == nil else { return nil }
        guard textAcknowledgementBarriers[nodeId] == nil else { return nil }
        guard let draft = textDrafts[nodeId] else { return nil }
        if draft.laneEpoch < max(textLaneEpochFloor, textLaneEpoch[nodeId] ?? 0) {
            textDrafts.removeValue(forKey: nodeId)
            return nil
        }
        return try await allocateAndSend(via: transport) { eventSeq, eventId in
            let event = Event.textEdit(
                eventSeq: eventSeq,
                eventId: eventId,
                observedRevision: draft.observedRevision,
                nodeId: draft.nodeId,
                text: draft.text,
                editSeq: draft.editSeq
            )
            self.textDrafts.removeValue(forKey: nodeId)
            self.assignedTextByNode[nodeId] = event.eventId
            self.assignedTextByEventId[event.eventId] = nodeId
            return event
        }
    }

    private func forgetAssignedText(eventId: EventId) {
        if let nodeId = assignedTextByEventId.removeValue(forKey: eventId) {
            assignedTextByNode.removeValue(forKey: nodeId)
        }
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
                try await transport.send(data: try SRUIFraming.encodeFramed(msg), logicalClass: .input)
            }
        }
        try await withTaskCancellationHandler {
            try await send.value
        } onCancel: {
            send.cancel()
        }
    }

    /// Selectively acknowledges one event ID. A later sequence does not cross an earlier gap.
    ///
    /// Private: settling an intent mutates state whose ownership depends on wire identity, so
    /// `settleAcknowledgement` is the only way in from the wire (§18.2).
    private func acknowledgeEvent(id: EventId) {
        guard let event = pendingEvents.removeValue(forKey: id) else { return }
        pendingOrder.removeAll { $0 == id }
        forgetAssignedText(eventId: id)
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
    ///
    /// The supersession check and publish share this single actor-isolated critical section.
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
    func commitResyncSnapshot<T: Sendable>(
        generation: UInt64?,
        publish: @Sendable () -> T,
        committed: @Sendable (T) -> Bool
    ) -> T? {
        guard activeResumeGeneration == generation else { return nil }
        let result = publish()
        // A rejected snapshot keeps the latch so the server can send another one. A committed
        // snapshot also keeps it until AppKit has mounted the authoritative state.
        guard committed(result) else { return result }
        return result
    }

    /// Completes a same-session decision only if no newer controller superseded this attempt.
    ///
    /// When `discardedTextEdits` is non-`nil`, assigned `TEXT_EDIT` cancellation shares this
    /// generation check so a stale resync cannot drain a newer attempt's pending set (§18.3).
    func completeSameSessionResume(
        id: String,
        lastProcessedEventSeq: UInt64,
        generation: UInt64,
        via transport: any Transport,
        enableNewEventsAfterReplay: Bool,
        discardedTextEdits: [SRUIPendingTextEditRef]? = nil,
        requireExactTextMatch: Bool = true,
        onReplayFailure: (@Sendable (String) async -> Void)? = nil
    ) async throws -> Bool {
        guard activeResumeGeneration == generation else { return false }
        if let discardedTextEdits {
            let confirming = discardedTextEdits.compactMap(PendingTextEditDescriptor.init(wire:))
            if requireExactTextMatch, confirming.count != discardedTextEdits.count {
                throw EventOutboxError.textEditDiscardMismatch
            }
            try cancelAssignedTextEdits(
                confirming: confirming,
                requireExactMatch: requireExactTextMatch
            )
        }
        activeSessionId = id
        acknowledgeEvents(throughSeq: lastProcessedEventSeq)
        try await resendPendingEvents(via: transport)
        guard activeResumeGeneration == generation else { return false }
        acceptsNewEvents = enableNewEventsAfterReplay
        if enableNewEventsAfterReplay {
            try await promoteReadyTextDrafts(via: transport)
        }
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
        textDrafts.removeAll(keepingCapacity: true)
        assignedTextByNode.removeAll(keepingCapacity: true)
        assignedTextByEventId.removeAll(keepingCapacity: true)
        textLaneEpoch.removeAll(keepingCapacity: true)
        textLaneEpochFloor = 0
        textAcknowledgementBarriers.removeAll(keepingCapacity: true)
        cancelPendingWrites()
    }

    /// Applies one selective acknowledgement plus the server's contiguous cumulative frontier.
    ///
    /// Both wire identities are checked here rather than by the caller: the ack settles state this
    /// actor owns, so a draining connection, an expired incarnation, or another client instance
    /// must be refused inside the same isolated step that would otherwise mutate it (§18, §18.2).
    /// Returns false without any mutation when the identity does not bind.
    @discardableResult
    public func settleAcknowledgement(
        clientInstanceId ackClientInstanceId: ClientInstanceId,
        eventId: EventId,
        throughSeq seq: UInt64,
        sessionId: String,
        revisionAfterEffect: UInt64? = nil,
        textEditRejected: Bool = false
    ) -> EventAcknowledgementSettlement {
        guard ackClientInstanceId == clientInstanceId else { return .unbound }
        // `session_id` is required on every ack (§18.2): an empty one proves nothing about which
        // incarnation settled the event, so it can never retire an intent.
        guard !sessionId.isEmpty,
              let activeSessionId,
              sessionId == activeSessionId else {
            return .unbound
        }
        let event = pendingEvents[eventId]
        var settledEvents = acknowledgeEvents(throughSeq: seq)
        if let event, !settledEvents.contains(where: { $0.eventId == event.eventId }) {
            settledEvents.append(event)
            settledEvents.sort { $0.eventSeq < $1.eventSeq }
        }

        if let revisionAfterEffect {
            for settled in settledEvents where settled.eventType == .EVENT_TEXT_EDIT {
                let barrier = TextEditAcknowledgementBarrier(
                    nodeId: settled.nodeId,
                    revisionAfterEffect: revisionAfterEffect,
                    rejected: textEditRejected && settled.eventId == eventId
                )
                if let current = textAcknowledgementBarriers[settled.nodeId] {
                    if current.revisionAfterEffect <= revisionAfterEffect {
                        textAcknowledgementBarriers[settled.nodeId] = barrier
                    }
                } else {
                    textAcknowledgementBarriers[settled.nodeId] = barrier
                }
            }
        }

        acknowledgeEvent(id: eventId)
        return EventAcknowledgementSettlement(
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
            pendingEvents.removeValue(forKey: event.eventId)
            forgetAssignedText(eventId: event.eventId)
        }
        pendingOrder.removeAll { pendingEvents[$0] == nil }
        advanceContiguousAcknowledgement()
        cancelReplayRetryLoopIfSettled()
        return settled
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
        // Lease identity *is* resume ownership here: `beginResumeAttempt()` and
        // `stopResumeWork(generation:)` cancel the lease of the generation they supersede, which
        // clears `replayLease` before any newer attempt can run. Re-deriving ownership from the
        // latch instead would stop the loop after `commitResumeWork(generation:)` releases it,
        // stranding events that are still unsettled (§18, §18.2).
        guard replayLease == lease,
              pendingEventReplayLoop.isActive(lease),
              pendingEvents.isEmpty == false else {
            return false
        }
        try Task.checkCancellation()
        try await resendPendingEvents(via: transport)
        // Re-checked after the suspension: a reconnect that superseded this generation while the
        // replay was in flight owns the retry set now, and this lease must not touch it (§18).
        // `Lease` equality covers `resumeScope`, so still being the current lease is itself proof
        // that this generation still owns the retry set.
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
