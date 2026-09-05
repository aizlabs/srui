//
// Text.swift
// Text
//
// Per-node editor sequence, debounce/coalescing, and authoritative-resolution
// bookkeeping for native `TEXT_EDIT` (§7.6, §18.3, §22.6).
// Spellchecking stays entirely local; this module adds no spellcheck protocol.
//

import Foundation
import SemanticModel

/// Default quiet-period before a coalesced whole-value `TEXT_EDIT` is committed (§22.6).
public let defaultTextEditDebounceNanoseconds: UInt64 = 75_000_000

/// How a published authoritative string should land in the native editor (§22.6).
public enum AuthoritativeResolution: Equatable, Sendable {
    /// Replace the native string with the published value.
    case apply
    /// Keep (or restore) local typing; the published value is an echo of an in-flight submit.
    case keepLocal
    /// Marked text is active; apply the published value after composition ends.
    case deferred
}

/// Shared coordinator for native text editors in one semantic session (§18.3, §22.6).
@MainActor
public final class TextEditingSession {
    public struct OverflowError: Error, Equatable, Sendable {
        public let nodeID: NodeId
    }

    public var debounceNanoseconds: UInt64
    public var onCommit: (@MainActor (NodeId, String, EditSeq) -> Void)?
    public var onInvalidateOutboxDraft: (@MainActor (NodeId) -> Void)?

    private struct NodeState {
        var nextEditSeq: UInt64 = 1
        var pendingValue: String?
        var debounceTask: Task<Void, Never>?
        var lastFlushedValue: String?
        var lastSubmittedValue: String?
        /// Last string known to be the store's `.value` (echo or applied correction).
        /// Reapplying this same string — a structural remount — must not clobber local typing.
        var lastKnownAuthoritative: String?
        var assignedEditSeq: EditSeq?
        var assignedEventId: EventId?
        var localValue: String = ""
        var deferredAuthoritative: String?
        var composing: Bool = false
    }

    private var nodes: [NodeId: NodeState] = [:]

    public init(debounceNanoseconds: UInt64 = defaultTextEditDebounceNanoseconds) {
        self.debounceNanoseconds = debounceNanoseconds
    }

    public func resetForReplacementSession() {
        for state in nodes.values {
            state.debounceTask?.cancel()
        }
        nodes.removeAll()
    }

    /// Drops sequence state for nodes that no longer exist after a committed delete.
    /// Same-session remounts keep sequence state for still-present node IDs (§18.3).
    public func syncPresentNodes(_ present: Set<NodeId>) {
        let stale = nodes.keys.filter { !present.contains($0) }
        for nodeID in stale {
            nodes[nodeID]?.debounceTask?.cancel()
            nodes.removeValue(forKey: nodeID)
        }
    }

    public func noteAssigned(_ event: Event) {
        guard event.eventType == .EVENT_TEXT_EDIT else { return }
        var state = nodes[event.nodeId] ?? NodeState()
        state.lastSubmittedValue = event.textArg
        state.assignedEditSeq = event.editSeq
        state.assignedEventId = event.eventId
        nodes[event.nodeId] = state
    }

    public func noteAcknowledged(_ event: Event) {
        guard event.eventType == .EVENT_TEXT_EDIT else { return }
        guard var state = nodes[event.nodeId] else { return }
        if state.assignedEventId == event.eventId || state.assignedEditSeq == event.editSeq {
            state.assignedEditSeq = nil
            state.assignedEventId = nil
        }
        nodes[event.nodeId] = state
    }

    /// Drops in-flight identity after a same-session forced resync cancelled the assigned edit.
    public func noteCanceled(nodeID: NodeId, eventId: EventId) {
        guard var state = nodes[nodeID] else { return }
        if state.assignedEventId == eventId || state.assignedEventId != nil {
            state.assignedEditSeq = nil
            state.assignedEventId = nil
        }
        state.lastSubmittedValue = nil
        state.pendingValue = nil
        state.debounceTask?.cancel()
        state.debounceTask = nil
        nodes[nodeID] = state
    }

    public func invalidateDraft(for nodeID: NodeId) {
        guard var state = nodes[nodeID] else { return }
        state.pendingValue = nil
        state.debounceTask?.cancel()
        state.debounceTask = nil
        nodes[nodeID] = state
        onInvalidateOutboxDraft?(nodeID)
    }

    /// Records a committed local string. While composition is active, remote emission is suppressed.
    public func noteLocalValue(_ value: String, nodeID: NodeId, composing: Bool, flushImmediately: Bool) {
        var state = nodes[nodeID] ?? NodeState()
        state.localValue = value
        state.composing = composing
        if composing {
            nodes[nodeID] = state
            return
        }
        if let deferred = state.deferredAuthoritative {
            state.deferredAuthoritative = nil
            nodes[nodeID] = state
            _ = applyPublishedValue(nodeID: nodeID, published: deferred)
            return
        }
        if state.lastFlushedValue == value, state.pendingValue == nil {
            state.localValue = value
            nodes[nodeID] = state
            return
        }
        state.pendingValue = value
        state.debounceTask?.cancel()
        if flushImmediately || debounceNanoseconds == 0 {
            state.debounceTask = nil
            nodes[nodeID] = state
            flushPending(nodeID: nodeID)
            return
        }
        let delay = debounceNanoseconds
        state.debounceTask = Task { [weak self] in
            try? await Task.sleep(nanoseconds: delay)
            guard !Task.isCancelled else { return }
            await self?.flushPending(nodeID: nodeID)
        }
        nodes[nodeID] = state
    }

    public func endEditing(nodeID: NodeId) {
        nodes[nodeID]?.debounceTask?.cancel()
        nodes[nodeID]?.debounceTask = nil
        flushPending(nodeID: nodeID)
    }

    public func flushPending(nodeID: NodeId) {
        guard var state = nodes[nodeID], let value = state.pendingValue else { return }
        state.pendingValue = nil
        state.debounceTask?.cancel()
        state.debounceTask = nil
        guard !state.composing else {
            nodes[nodeID] = state
            return
        }
        let seqValue = state.nextEditSeq
        guard seqValue > 0, let seq = EditSeq(seqValue) else { return }
        let next = seqValue &+ 1
        guard next > seqValue else { return }
        state.nextEditSeq = next
        state.lastFlushedValue = value
        state.localValue = value
        nodes[nodeID] = state
        onCommit?(nodeID, value, seq)
    }

    /// Echo of a submitted value must not overwrite newer local typing; any other *new*
    /// published string is a normalization/correction and replaces native text (§22.6).
    /// Reapplying the last known store value (structural remount) keeps local drafts.
    @discardableResult
    public func applyPublishedValue(nodeID: NodeId, published: String) -> AuthoritativeResolution {
        var state = nodes[nodeID] ?? NodeState()
        if state.composing {
            state.deferredAuthoritative = published
            nodes[nodeID] = state
            return .deferred
        }
        if let submitted = state.lastSubmittedValue, submitted == published {
            state.lastKnownAuthoritative = published
            nodes[nodeID] = state
            return .keepLocal
        }
        if state.lastKnownAuthoritative == published {
            nodes[nodeID] = state
            return .keepLocal
        }
        let hadDraft = state.pendingValue != nil || state.lastSubmittedValue != nil
        state.pendingValue = nil
        state.debounceTask?.cancel()
        state.debounceTask = nil
        state.lastSubmittedValue = nil
        state.lastKnownAuthoritative = published
        state.localValue = published
        nodes[nodeID] = state
        if hadDraft {
            onInvalidateOutboxDraft?(nodeID)
        }
        return .apply
    }

    public func localValue(for nodeID: NodeId) -> String? {
        nodes[nodeID]?.localValue
    }

    public func nextEditSeqValue(for nodeID: NodeId) -> UInt64 {
        nodes[nodeID]?.nextEditSeq ?? 1
    }

    /// Updates composition state. Returns a deferred authoritative string that the adapter
    /// must apply now that marked text has ended.
    ///
    /// Does not flush on composition end: the adapter must pass the control's committed
    /// string through `noteLocalValue` so an intermediate marked value is not emitted (§22.6).
    @discardableResult
    public func setComposing(_ composing: Bool, nodeID: NodeId) -> String? {
        var state = nodes[nodeID] ?? NodeState()
        let ending = state.composing && !composing
        state.composing = composing
        nodes[nodeID] = state
        if ending, let deferred = state.deferredAuthoritative {
            state.deferredAuthoritative = nil
            nodes[nodeID] = state
            if applyPublishedValue(nodeID: nodeID, published: deferred) == .apply {
                return deferred
            }
        }
        return nil
    }
}
