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
    public var debounceNanoseconds: UInt64
    public var onCommit: (@MainActor (NodeId, String, EditSeq, UInt64) -> Void)?
    public var onInvalidateOutboxDraft: (@MainActor (NodeId, UInt64) -> Void)?
    /// Surfaced when `edit_seq` cannot increment; the pending value is kept (§18.3).
    public var onEditSeqOverflow: (@MainActor (NodeId) -> Void)?

    private struct NodeState {
        var nextEditSeq: UInt64 = 1
        var pendingValue: String?
        var debounceTask: Task<Void, Never>?
        var lastFlushedValue: String?
        var lastSubmittedValue: String?
        /// Last string known to be the store's `.value` (echo or applied correction).
        var lastKnownAuthoritative: String?
        var assignedEventId: EventId?
        var localValue: String = ""
        var deferredAuthoritative: String?
        var composing: Bool = false
    }

    private var nodes: [NodeId: NodeState] = [:]
    /// Structural remounts reapply unchanged store strings; live corrections must not.
    private var preservingLocalTextAcrossRemount = false
    /// Bumped when a correction/cancel invalidates drafts so unordered outbox Tasks cannot
    /// re-queue a rejected string after `invalidateTextDraft` (§22.6).
    private var laneEpoch: [NodeId: UInt64] = [:]
    /// Session-wide floor used to fence delayed local callbacks across an authoritative full
    /// resync. Per-node edit sequences intentionally survive a same-session resync (§18.3).
    private var resyncLaneEpoch: UInt64 = 0
    /// True only while old native controls are torn down and snapshot controls are mounted.
    /// AppKit may emit end-editing notifications during teardown; those are pre-snapshot intent.
    private var suppressingLocalEditsForResync = false

    public init(debounceNanoseconds: UInt64 = defaultTextEditDebounceNanoseconds) {
        self.debounceNanoseconds = debounceNanoseconds
    }

    public func resetForReplacementSession() {
        for state in nodes.values {
            state.debounceTask?.cancel()
        }
        nodes.removeAll()
        laneEpoch.removeAll()
        resyncLaneEpoch = 0
        suppressingLocalEditsForResync = false
    }

    /// Drops sequence state for nodes that no longer exist after a committed delete.
    /// Same-session remounts keep sequence state for still-present node IDs (§18.3).
    public func syncPresentNodes(_ present: Set<NodeId>) {
        let stale = nodes.keys.filter { !present.contains($0) }
        for nodeID in stale {
            nodes[nodeID]?.debounceTask?.cancel()
            nodes.removeValue(forKey: nodeID)
            laneEpoch.removeValue(forKey: nodeID)
        }
    }

    public func noteAssigned(_ event: Event) {
        guard event.eventType == .EVENT_TEXT_EDIT else { return }
        var state = nodes[event.nodeId] ?? NodeState()
        state.lastSubmittedValue = event.textArg
        state.assignedEventId = event.eventId
        nodes[event.nodeId] = state
    }

    public func noteAcknowledged(_ event: Event) {
        guard event.eventType == .EVENT_TEXT_EDIT else { return }
        guard var state = nodes[event.nodeId] else { return }
        if state.assignedEventId == event.eventId {
            state.assignedEventId = nil
        }
        nodes[event.nodeId] = state
    }

    /// Drops in-flight identity after a same-session forced resync cancelled the assigned edit.
    /// Only the matching assigned event is cleared; a mismatched id leaves the node untouched.
    public func noteCanceled(nodeID: NodeId, eventId: EventId) {
        guard var state = nodes[nodeID] else { return }
        guard state.assignedEventId == eventId else { return }
        state.assignedEventId = nil
        state.lastSubmittedValue = nil
        state.pendingValue = nil
        state.debounceTask?.cancel()
        state.debounceTask = nil
        let epoch = bumpLaneEpoch(nodeID: nodeID)
        nodes[nodeID] = state
        onInvalidateOutboxDraft?(nodeID, epoch)
    }

    /// Structural remounts re-apply the current store string for every editor. That is not a
    /// correction: keep local typing while the published value is still the last known store value.
    public func withPreservedLocalText<T>(_ body: () throws -> T) rethrows -> T {
        preservingLocalTextAcrossRemount = true
        defer { preservingLocalTextAcrossRemount = false }
        return try body()
    }

    /// True while `LayoutRenderer` is tearing down and rebuilding under `withPreservedLocalText`.
    /// AppKit end-editing callbacks must not flush in this window.
    public var isPreservingLocalTextAcrossRemount: Bool {
        preservingLocalTextAcrossRemount
    }

    public func invalidateDraft(for nodeID: NodeId) {
        guard var state = nodes[nodeID] else { return }
        state.pendingValue = nil
        state.debounceTask?.cancel()
        state.debounceTask = nil
        nodes[nodeID] = state
        onInvalidateOutboxDraft?(nodeID, bumpLaneEpoch(nodeID: nodeID))
    }

    /// Records a committed local string. While composition is active, remote emission is suppressed.
    public func noteLocalValue(_ value: String, nodeID: NodeId, composing: Bool, flushImmediately: Bool) {
        guard !suppressingLocalEditsForResync else { return }
        if preservingLocalTextAcrossRemount {
            var state = nodes[nodeID] ?? NodeState()
            state.localValue = value
            state.composing = composing
            if !composing, state.pendingValue != nil || value != state.lastKnownAuthoritative {
                state.pendingValue = value
            }
            nodes[nodeID] = state
            return
        }
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
        if !preservingLocalTextAcrossRemount,
           flushImmediately || debounceNanoseconds == 0 {
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
        guard !suppressingLocalEditsForResync,
              !preservingLocalTextAcrossRemount else {
            return
        }
        nodes[nodeID]?.debounceTask?.cancel()
        nodes[nodeID]?.debounceTask = nil
        flushPending(nodeID: nodeID)
    }

    public func flushPending(nodeID: NodeId) {
        guard !preservingLocalTextAcrossRemount, !suppressingLocalEditsForResync else { return }
        guard var state = nodes[nodeID], let value = state.pendingValue else { return }
        state.pendingValue = nil
        state.debounceTask?.cancel()
        state.debounceTask = nil
        guard !state.composing else {
            nodes[nodeID] = state
            return
        }
        let seqValue = state.nextEditSeq
        guard seqValue > 0, let seq = EditSeq(seqValue), seqValue < .max else {
            // Exhausted `edit_seq` must not wrap; restore the pending value so it is not dropped (§18.3).
            state.pendingValue = value
            nodes[nodeID] = state
            onEditSeqOverflow?(nodeID)
            return
        }
        let next = seqValue + 1
        state.nextEditSeq = next
        state.lastFlushedValue = value
        state.localValue = value
        nodes[nodeID] = state
        onCommit?(nodeID, value, seq, bumpLaneEpoch(nodeID: nodeID))
    }

    /// Echo of a submitted value must not overwrite newer local typing; any other published
    /// string is a normalization/correction and replaces native text (§22.6).
    ///
    /// An unchanged store string is kept only during an explicit structural remount
    /// (`withPreservedLocalText`) that still has a debounce draft or assigned in-flight
    /// edit. A live transaction that republishes the previous authoritative value —
    /// reject/revert or trim — must still apply. A remount with no local work also applies.
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
        let hasLocalWork = state.pendingValue != nil || state.assignedEventId != nil
        if preservingLocalTextAcrossRemount, hasLocalWork, state.lastKnownAuthoritative == published {
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
        // Authoritative replace is a new baseline: reset `lastFlushedValue` so retyping the
        // previous submit is not treated as a no-op echo of the old flush (§22.6).
        state.lastFlushedValue = published
        nodes[nodeID] = state
        if hadDraft {
            onInvalidateOutboxDraft?(nodeID, bumpLaneEpoch(nodeID: nodeID))
        }
        return .apply
    }

    public func localValue(for nodeID: NodeId) -> String? {
        nodes[nodeID]?.localValue
    }

    /// Last store string applied or echoed for this editor; `nil` if none has been published.
    public func lastKnownAuthoritative(for nodeID: NodeId) -> String? {
        nodes[nodeID]?.lastKnownAuthoritative
    }

    /// True when a coalesced draft newer than the in-flight submit is waiting locally.
    ///
    /// A rejected ack that will not be followed by a transaction must keep this typing
    /// and promote it; only a submit with no successor reverts to the last store string.
    public func hasUnsentSuccessorDraft(for nodeID: NodeId) -> Bool {
        guard let state = nodes[nodeID] else { return false }
        if state.pendingValue != nil {
            return true
        }
        guard let submitted = state.lastSubmittedValue else { return false }
        if let flushed = state.lastFlushedValue, flushed != submitted {
            return true
        }
        return false
    }

    public func nextEditSeqValue(for nodeID: NodeId) -> UInt64 {
        nodes[nodeID]?.nextEditSeq ?? 1
    }

    /// Establishes the authoritative full-resync boundary without resetting edit_seq.
    ///
    /// The returned epoch fences callbacks already queued by the old native controls. State
    /// created by controls mounted from the snapshot inherits the same floor, so edits genuinely
    /// made after the snapshot remain eligible for synchronization (§18.3).
    @discardableResult
    public func discardUnresolvedEditsForResync() -> UInt64 {
        suppressingLocalEditsForResync = true
        let highest = max(resyncLaneEpoch, laneEpoch.values.max() ?? 0)
        resyncLaneEpoch = highest == .max ? .max : highest + 1

        for nodeID in Array(nodes.keys) {
            guard var state = nodes[nodeID] else { continue }
            state.debounceTask?.cancel()
            state.debounceTask = nil
            state.pendingValue = nil
            state.lastFlushedValue = nil
            state.lastSubmittedValue = nil
            state.assignedEventId = nil
            state.deferredAuthoritative = nil
            state.composing = false
            state.localValue = state.lastKnownAuthoritative ?? ""
            nodes[nodeID] = state
            laneEpoch[nodeID] = resyncLaneEpoch
        }
        return resyncLaneEpoch
    }

    /// Ends the synchronous native remount begun by discardUnresolvedEditsForResync.
    public func finishResyncTextBoundary() {
        suppressingLocalEditsForResync = false
    }

    /// Updates composition state. Returns a deferred authoritative string that the adapter
    /// must apply now that marked text has ended.
    ///
    /// Does not flush on composition end: the adapter must pass the control's committed
    /// string through `noteLocalValue` so an intermediate marked value is not emitted (§22.6).
    @discardableResult
    public func setComposing(_ composing: Bool, nodeID: NodeId) -> String? {
        guard !suppressingLocalEditsForResync else { return nil }
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

    @discardableResult
    private func bumpLaneEpoch(nodeID: NodeId) -> UInt64 {
        let current = max(resyncLaneEpoch, laneEpoch[nodeID] ?? 0)
        let epoch = current == .max ? .max : current + 1
        laneEpoch[nodeID] = epoch
        return epoch
    }
}
