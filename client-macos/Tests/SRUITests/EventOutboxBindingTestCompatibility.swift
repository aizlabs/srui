//
// EventOutboxBindingTestCompatibility.swift
// SRUITests
//
// Test-only source compatibility for fixtures that exercise EventOutbox directly instead of
// through a SessionController-owned connection binding.
//

import Foundation
import Testing
import SemanticModel
import Protocol
@testable import Session
import TransportSSH

private actor EventOutboxTestBindingStore {
    private final class Entry {
        weak var outbox: EventOutbox?
        let binding: EventOutboxConnectionBinding

        init(outbox: EventOutbox, binding: EventOutboxConnectionBinding) {
            self.outbox = outbox
            self.binding = binding
        }
    }

    static let shared = EventOutboxTestBindingStore()

    private var bindings: [ObjectIdentifier: Entry] = [:]

    func binding(for outbox: EventOutbox) async -> EventOutboxConnectionBinding {
        let key = ObjectIdentifier(outbox)
        if let entry = bindings[key],
           entry.outbox === outbox,
           await outbox.isActiveConnectionBinding(entry.binding) {
            return entry.binding
        }

        bindings.removeValue(forKey: key)
        if let binding = await outbox.activeConnectionBindingForTesting {
            bindings[key] = Entry(outbox: outbox, binding: binding)
            return binding
        }

        let binding = await outbox.beginConnectionBinding()
        _ = await outbox.allowNewEvents(binding: binding)
        bindings[key] = Entry(outbox: outbox, binding: binding)
        return binding
    }
}

@MainActor
private final class TextAcknowledgementCapture {
    private(set) var barriers: [TextEditAcknowledgementBarrier] = []

    func store(_ barriers: [TextEditAcknowledgementBarrier]) {
        self.barriers = barriers
    }
}

extension EventOutbox {
    private func testConnectionBinding() async -> EventOutboxConnectionBinding {
        await EventOutboxTestBindingStore.shared.binding(for: self)
    }

    @discardableResult
    func invalidateTextDraft(nodeId: NodeId, laneEpoch: UInt64 = 0) async -> Bool {
        let binding = await testConnectionBinding()
        guard let sessionIncarnation = await sessionIncarnation(binding: binding) else {
            return false
        }
        return invalidateTextDraft(
            nodeId: nodeId,
            laneEpoch: laneEpoch,
            binding: binding,
            sessionIncarnation: sessionIncarnation
        )
    }

    @discardableResult
    func sendActivate(
        nodeId: NodeId,
        observedRevision: Revision,
        via transport: any Transport,
        onTextEditAssigned: TextEditAssignmentHandler? = nil
    ) async throws -> Event {
        let binding = await testConnectionBinding()
        return try await sendActivate(
            nodeId: nodeId,
            observedRevision: observedRevision,
            binding: binding,
            via: transport,
            onTextEditAssigned: onTextEditAssigned
        )
    }

    @discardableResult
    func sendValueChanged(
        nodeId: NodeId,
        observedRevision: Revision,
        value: Value,
        via transport: any Transport,
        onTextEditAssigned: TextEditAssignmentHandler? = nil
    ) async throws -> Event {
        let binding = await testConnectionBinding()
        return try await sendValueChanged(
            nodeId: nodeId,
            observedRevision: observedRevision,
            value: value,
            binding: binding,
            via: transport,
            onTextEditAssigned: onTextEditAssigned
        )
    }

    @discardableResult
    func sendSelectionChanged(
        nodeId: NodeId,
        observedRevision: Revision,
        itemId: ItemId,
        via transport: any Transport,
        onTextEditAssigned: TextEditAssignmentHandler? = nil
    ) async throws -> Event {
        let binding = await testConnectionBinding()
        return try await sendSelectionChanged(
            nodeId: nodeId,
            observedRevision: observedRevision,
            itemId: itemId,
            binding: binding,
            via: transport,
            onTextEditAssigned: onTextEditAssigned
        )
    }

    @discardableResult
    func queueTextEdit(
        nodeId: NodeId,
        text: String,
        editSeq: EditSeq,
        observedRevision: Revision,
        via transport: any Transport,
        laneEpoch: UInt64 = 0,
        onAssigned: TextEditAssignmentHandler? = nil
    ) async throws -> Event? {
        let binding = await testConnectionBinding()
        return try await queueTextEdit(
            nodeId: nodeId,
            text: text,
            editSeq: editSeq,
            observedRevision: observedRevision,
            binding: binding,
            via: transport,
            laneEpoch: laneEpoch,
            onAssigned: onAssigned
        )
    }

    @discardableResult
    func promoteReadyTextDrafts(
        via transport: any Transport,
        onAssigned: TextEditAssignmentHandler? = nil
    ) async throws -> [Event] {
        let binding = await testConnectionBinding()
        return try await promoteReadyTextDrafts(
            binding: binding,
            via: transport,
            onAssigned: onAssigned
        )
    }

    func sendEvent(
        _ event: Event,
        via transport: any Transport,
        onRetained: TextEditAssignmentHandler? = nil
    ) async throws {
        let binding = await testConnectionBinding()
        try await sendEvent(
            event,
            binding: binding,
            via: transport,
            onRetained: onRetained
        )
    }

    func resendPendingEvents(via transport: any Transport) async throws {
        let binding = await testConnectionBinding()
        try await resendPendingEvents(binding: binding, via: transport)
    }

    func settleAcknowledgement(
        clientInstanceId: ClientInstanceId,
        eventId: EventId,
        throughSeq seq: UInt64,
        sessionId: String,
        revisionAfterEffect: UInt64? = nil,
        textEditRejected: Bool = false
    ) async -> EventAcknowledgementSettlement {
        let binding = await testConnectionBinding()
        return settleAcknowledgement(
            binding: binding,
            clientInstanceId: clientInstanceId,
            eventId: eventId,
            throughSeq: seq,
            sessionId: sessionId,
            revisionAfterEffect: revisionAfterEffect,
            textEditRejected: textEditRejected
        )
    }

    func releaseTextAcknowledgements(
        through revision: UInt64
    ) async -> [TextEditAcknowledgementBarrier] {
        let binding = await testConnectionBinding()
        guard let sessionIncarnation = await sessionIncarnation(binding: binding) else {
            return []
        }
        let capture = await MainActor.run { TextAcknowledgementCapture() }
        _ = await releaseTextAcknowledgements(
            through: revision,
            binding: binding,
            sessionIncarnation: sessionIncarnation,
            onResolved: { barriers in
                capture.store(barriers)
                return []
            }
        )
        return await MainActor.run { capture.barriers }
    }
    func completeSameSessionResume(
        id: String,
        lastProcessedEventSeq: UInt64,
        generation: UInt64,
        via transport: any Transport,
        enableNewEventsAfterReplay: Bool,
        discardedTextEdits: [SRUIPendingTextEditRef]? = nil,
        requireExactTextMatch: Bool = true,
        onTextEditAssigned: TextEditAssignmentHandler? = nil,
        onReplayFailure: (@Sendable (String) async -> Void)? = nil
    ) async throws -> Bool {
        let binding = await testConnectionBinding()
        return try await completeSameSessionResume(
            id: id,
            lastProcessedEventSeq: lastProcessedEventSeq,
            generation: generation,
            binding: binding,
            via: transport,
            enableNewEventsAfterReplay: enableNewEventsAfterReplay,
            discardedTextEdits: discardedTextEdits,
            requireExactTextMatch: requireExactTextMatch,
            onTextEditAssigned: onTextEditAssigned,
            onReplayFailure: onReplayFailure
        )
    }

    func confirmFreshSession(id: String) async -> Bool {
        let binding = await testConnectionBinding()
        return await confirmFreshSession(id: id, binding: binding)
    }

    func beginResumeAttempt() async -> UInt64 {
        let binding = await testConnectionBinding()
        guard let generation = await beginResumeAttempt(binding: binding) else {
            Issue.record("The test connection binding was superseded")
            return 0
        }
        return generation
    }

    @discardableResult
    func suspendNewEvents() async -> Bool {
        let binding = await testConnectionBinding()
        return suspendNewEvents(binding: binding)
    }

    @discardableResult
    func applyLiveResyncFrontier(lastProcessedEventSeq: UInt64) async -> Bool {
        let binding = await testConnectionBinding()
        return await applyLiveResyncFrontier(
            lastProcessedEventSeq: lastProcessedEventSeq,
            binding: binding
        )
    }

    @discardableResult
    func applyReplacementFrontier(
        id: String,
        lastProcessedEventSeq: UInt64
    ) async -> Bool {
        let binding = await testConnectionBinding()
        return await applyReplacementFrontier(
            id: id,
            lastProcessedEventSeq: lastProcessedEventSeq,
            binding: binding
        )
    }

    @discardableResult
    func allowNewEvents() async -> Bool {
        let binding = await testConnectionBinding()
        return allowNewEvents(binding: binding)
    }

    func commitResyncSnapshot<Value: Sendable>(
        generation: UInt64?,
        publish: @Sendable () -> Value,
        committed: @Sendable (Value) -> Bool
    ) async -> ResyncSnapshotCommit<Value>? {
        let binding = await testConnectionBinding()
        return await commitResyncSnapshot(
            generation: generation,
            binding: binding,
            publish: publish,
            committed: committed
        )
    }

    @discardableResult
    func applyFullResyncTextBoundary(
        laneEpoch: UInt64?,
        generation: UInt64?,
        renderToken: UUID
    ) async -> Bool {
        let binding = await testConnectionBinding()
        return await applyFullResyncTextBoundary(
            laneEpoch: laneEpoch,
            generation: generation,
            binding: binding,
            renderToken: renderToken
        )
    }

    @discardableResult
    func abortResyncRender(
        laneEpoch: UInt64?,
        generation: UInt64?,
        renderToken: UUID
    ) async -> Bool {
        let binding = await testConnectionBinding()
        return await abortResyncRender(
            laneEpoch: laneEpoch,
            generation: generation,
            binding: binding,
            renderToken: renderToken
        )
    }
}
