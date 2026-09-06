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

/// A whole-value local edit ready to acquire its durable transport identity.
public struct LocalTextEdit: Equatable, Sendable {
    public var nodeId: NodeId
    public var text: String
    public var editSeq: EditSeq
    public var observedRevision: Revision
    public var laneEpoch: UInt64

    public init(
        nodeId: NodeId,
        text: String,
        editSeq: EditSeq,
        observedRevision: Revision,
        laneEpoch: UInt64
    ) {
        self.nodeId = nodeId
        self.text = text
        self.editSeq = editSeq
        self.observedRevision = observedRevision
        self.laneEpoch = laneEpoch
    }
}

/// Shared coordinator for native text editors in one semantic session (§18.3, §22.6).
@MainActor
public final class TextEditingSession {
    typealias DebounceSleep = @Sendable (UInt64) async throws -> Void

    public var debounceNanoseconds: UInt64
    public var onCommit: (@MainActor (NodeId, String, EditSeq, UInt64) -> Void)?
    /// Surfaced when `edit_seq` cannot increment; the pending value is kept (§18.3).
    public var onEditSeqOverflow: (@MainActor (NodeId) -> Void)?

    private let debounceSleep: DebounceSleep

    private struct NodeState {
        var nextEditSeq: UInt64 = 1
        var pendingValue: String?
        var debounceTask: Task<Void, Never>?
        var lastFlushedValue: String?
        /// Identity of the latest flushed callback which has not acquired a transport event ID.
        ///
        /// This is independent from `lastSubmittedValue`: retiring a rendered acknowledgement
        /// must stop treating its value as an in-flight echo without hiding a newer flushed draft.
        var unassignedFlushedEditSeq: EditSeq?
        var unassignedFlushedValue: String?
        var unassignedObservedRevision: Revision?
        var unassignedLaneEpoch: UInt64?
        var unassignedClaimed = false
        var lastSubmittedValue: String?
        /// Last string known to be the store's `.value` (echo or applied correction).
        var lastKnownAuthoritative: String?
        var assignedEventId: EventId?
        var localValue: String = ""
        var deferredAuthoritative: String?
        var composing: Bool = false
    }

    private var nodes: [NodeId: NodeState] = [:]
    private var unassignedNodeOrder: [NodeId] = []
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
        debounceSleep = { delay in
            try await Task<Never, Never>.sleep(nanoseconds: delay)
        }
    }

    init(
        debounceNanoseconds: UInt64,
        sleep: @escaping DebounceSleep
    ) {
        self.debounceNanoseconds = debounceNanoseconds
        debounceSleep = sleep
    }

    public func resetForReplacementSession() {
        for state in nodes.values {
            state.debounceTask?.cancel()
        }
        nodes.removeAll()
        unassignedNodeOrder.removeAll(keepingCapacity: false)
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
        unassignedNodeOrder.removeAll { !present.contains($0) }
    }

    public func noteAssigned(_ event: Event) {
        guard event.eventType == .EVENT_TEXT_EDIT else { return }
        var state = nodes[event.nodeId] ?? NodeState()
        state.lastSubmittedValue = event.textArg
        state.assignedEventId = event.eventId
        if state.unassignedFlushedEditSeq == event.editSeq {
            clearUnassignedEdit(&state, nodeID: event.nodeId)
        }
        nodes[event.nodeId] = state
    }

    public func noteAcknowledged(_ event: Event) {
        guard event.eventType == .EVENT_TEXT_EDIT else { return }
        noteAcknowledged(nodeID: event.nodeId, eventId: event.eventId)
    }

    /// Retires both native assignment and echo identity after that event's authoritative effect
    /// has rendered. A newer flushed-but-unassigned edit is tracked independently.
    public func noteAcknowledged(nodeID: NodeId, eventId: EventId) {
        guard var state = nodes[nodeID] else { return }
        if state.assignedEventId == eventId {
            state.assignedEventId = nil
            state.lastSubmittedValue = nil
        }
        nodes[nodeID] = state
    }

    /// Drops in-flight identity after a same-session forced resync cancelled the assigned edit.
    /// Only the matching assigned event is cleared; a mismatched id leaves the node untouched.
    public func noteCanceled(nodeID: NodeId, eventId: EventId) {
        guard var state = nodes[nodeID] else { return }
        guard state.assignedEventId == eventId else { return }
        state.assignedEventId = nil
        state.lastSubmittedValue = nil
        state.pendingValue = nil
        clearUnassignedEdit(&state, nodeID: nodeID)
        state.debounceTask?.cancel()
        state.debounceTask = nil
        _ = bumpLaneEpoch(nodeID: nodeID)
        nodes[nodeID] = state
    }

    /// Structural remounts re-apply the current store string for every editor. That is not a
    /// correction: keep local typing while the published value is still the last known store value.
    ///
    /// IME composition cannot survive field-editor teardown, so every in-flight composition is
    /// abandoned here (without flushing) before `body` runs.
    public func withPreservedLocalText<T>(_ body: () throws -> T) rethrows -> T {
        preservingLocalTextAcrossRemount = true
        abandonAllCompositions()
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
        clearUnassignedEdit(&state, nodeID: nodeID)
        state.debounceTask?.cancel()
        state.debounceTask = nil
        nodes[nodeID] = state
        _ = bumpLaneEpoch(nodeID: nodeID)
    }

    /// Records a committed local string. While composition is active, remote emission is suppressed.
    public func noteLocalValue(
        _ value: String,
        nodeID: NodeId,
        composing: Bool,
        flushImmediately: Bool
    ) {
        guard !suppressingLocalEditsForResync else { return }
        if preservingLocalTextAcrossRemount {
            var state = seededState(for: nodeID)
            // Marked text dies with the field editor. Do not re-enter composing or promote
            // an intermediate preedit into a pending draft (§22.6).
            if composing {
                state.composing = false
                state.deferredAuthoritative = nil
                nodes[nodeID] = state
                return
            }
            state.localValue = value
            state.composing = false
            state.deferredAuthoritative = nil
            if state.pendingValue != nil || value != state.lastKnownAuthoritative {
                state.pendingValue = value
            }
            nodes[nodeID] = state
            return
        }
        var state = seededState(for: nodeID)
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
        let sleep = debounceSleep
        state.debounceTask = Task { [weak self] in
            do {
                try await sleep(delay)
            } catch {
                return
            }
            guard !Task.isCancelled else { return }
            self?.flushPending(nodeID: nodeID)
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
        state.unassignedFlushedEditSeq = seq
        state.unassignedFlushedValue = value
        state.unassignedObservedRevision = nil
        let epoch = bumpLaneEpoch(nodeID: nodeID)
        state.unassignedLaneEpoch = epoch
        state.unassignedClaimed = false
        state.localValue = value
        nodes[nodeID] = state
        unassignedNodeOrder.removeAll { $0 == nodeID }
        unassignedNodeOrder.append(nodeID)
        onCommit?(nodeID, value, seq, epoch)
    }

    /// Flushes every committed, non-composing native value before a non-text interaction is
    /// admitted. Snapshotting the keys makes recursive commit callbacks safe.
    public func flushAllPending() {
        for nodeID in Array(nodes.keys) {
            flushPending(nodeID: nodeID)
        }
    }

    /// Records the revision visible when this already-flushed edit entered interaction dispatch.
    @discardableResult
    public func recordObservedRevision(
        nodeID: NodeId,
        text: String,
        editSeq: EditSeq,
        laneEpoch: UInt64,
        observedRevision: Revision
    ) -> Bool {
        guard var state = nodes[nodeID],
              state.unassignedFlushedEditSeq == editSeq,
              state.unassignedFlushedValue == text,
              state.unassignedLaneEpoch == laneEpoch else {
            return false
        }
        state.unassignedObservedRevision = observedRevision
        nodes[nodeID] = state
        return true
    }

    /// Node whose oldest unassigned edit must acquire lane availability next.
    public func nextUnassignedEditNode() -> NodeId? {
        firstUnassignedNode()
    }

    /// Claims the oldest unassigned edit after its assigned-event lane is available.
    public func claimNextUnassignedEdit() -> LocalTextEdit? {
        guard let nodeID = firstUnassignedNode(),
              var state = nodes[nodeID],
              !state.unassignedClaimed,
              let text = state.unassignedFlushedValue,
              let editSeq = state.unassignedFlushedEditSeq,
              let observedRevision = state.unassignedObservedRevision,
              let laneEpoch = state.unassignedLaneEpoch else {
            return nil
        }
        state.unassignedClaimed = true
        nodes[nodeID] = state
        return LocalTextEdit(
            nodeId: nodeID,
            text: text,
            editSeq: editSeq,
            observedRevision: observedRevision,
            laneEpoch: laneEpoch
        )
    }

    public func releaseClaim(_ edit: LocalTextEdit) {
        guard var state = nodes[edit.nodeId],
              state.unassignedFlushedEditSeq == edit.editSeq,
              state.unassignedFlushedValue == edit.text,
              state.unassignedLaneEpoch == edit.laneEpoch else {
            return
        }
        state.unassignedClaimed = false
        nodes[edit.nodeId] = state
    }

    @discardableResult
    public func noteAssigned(_ event: Event, matching edit: LocalTextEdit) -> Bool {
        guard event.eventType == .EVENT_TEXT_EDIT,
              event.nodeId == edit.nodeId,
              event.editSeq == edit.editSeq,
              event.textArg == edit.text,
              var state = nodes[edit.nodeId],
              state.unassignedClaimed,
              state.unassignedFlushedEditSeq == edit.editSeq,
              state.unassignedFlushedValue == edit.text,
              state.unassignedLaneEpoch == edit.laneEpoch else {
            return false
        }
        state.lastSubmittedValue = event.textArg
        state.assignedEventId = event.eventId
        clearUnassignedEdit(&state, nodeID: edit.nodeId)
        nodes[edit.nodeId] = state
        return true
    }

    /// Restores a native assignment when its still-closed transport slot was rolled back by a
    /// lifecycle transition. A correction or replacement that already retired the assignment wins.
    public func restoreUnassigned(_ edit: LocalTextEdit, from event: Event) {
        guard event.eventType == .EVENT_TEXT_EDIT,
              event.nodeId == edit.nodeId,
              event.editSeq == edit.editSeq,
              var state = nodes[edit.nodeId],
              state.assignedEventId == event.eventId,
              state.lastSubmittedValue == event.textArg else {
            return
        }
        state.assignedEventId = nil
        state.lastSubmittedValue = nil
        state.unassignedFlushedEditSeq = edit.editSeq
        state.unassignedFlushedValue = edit.text
        state.unassignedObservedRevision = edit.observedRevision
        state.unassignedLaneEpoch = edit.laneEpoch
        state.unassignedClaimed = false
        nodes[edit.nodeId] = state
        unassignedNodeOrder.removeAll { $0 == edit.nodeId }
        unassignedNodeOrder.insert(edit.nodeId, at: 0)
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

        // Classify while composition is still active. An accepted echo is authoritative metadata,
        // not a native replacement: recording it now prevents a later ACK from turning that same
        // publication into a deferred correction that overwrites newer marked text.
        if let submitted = state.lastSubmittedValue, submitted == published {
            state.lastKnownAuthoritative = published
            nodes[nodeID] = state
            return .keepLocal
        }

        let hasLocalWork = state.pendingValue != nil
            || state.unassignedFlushedEditSeq != nil
            || state.assignedEventId != nil
        if preservingLocalTextAcrossRemount, hasLocalWork, state.lastKnownAuthoritative == published {
            nodes[nodeID] = state
            return .keepLocal
        }

        // A non-echo publication is authoritative immediately even when marked text prevents its
        // native assignment. Retire protocol-visible successors now so the acknowledgement for
        // the corrected submit cannot promote stale E2 while E3 is still composing.
        let hadDraft = state.pendingValue != nil
            || state.unassignedFlushedEditSeq != nil
            || state.lastSubmittedValue != nil
        let deferNativeReplacement = state.composing
        state.pendingValue = nil
        clearUnassignedEdit(&state, nodeID: nodeID)
        state.debounceTask?.cancel()
        state.debounceTask = nil
        state.lastSubmittedValue = nil
        state.lastKnownAuthoritative = published
        state.lastFlushedValue = published
        if deferNativeReplacement {
            state.deferredAuthoritative = published
        } else {
            state.deferredAuthoritative = nil
            state.localValue = published
        }
        nodes[nodeID] = state
        if hadDraft {
            _ = bumpLaneEpoch(nodeID: nodeID)
        }
        return deferNativeReplacement ? .deferred : .apply
    }

    public func localValue(for nodeID: NodeId) -> String? {
        nodes[nodeID]?.localValue
    }

    /// True while this editor has marked text that has not yet been committed locally.
    public func isComposing(for nodeID: NodeId) -> Bool {
        nodes[nodeID]?.composing ?? false
    }

    /// Drops IME bookkeeping for a destroyed field editor without flushing a `TEXT_EDIT`.
    ///
    /// A deferred authoritative correction wins because the marked field editor is being
    /// destroyed. Otherwise the newest committed local draft or in-flight submit is restored as
    /// `localValue`, so a remount never paints an intermediate marked string.
    public func abandonComposition(nodeID: NodeId) {
        guard var state = nodes[nodeID] else { return }
        let deferred = state.deferredAuthoritative
        state.composing = false
        state.deferredAuthoritative = nil
        if let deferred = deferred {
            state.localValue = deferred
        } else if let pending = state.pendingValue {
            state.localValue = pending
        } else if let unassigned = state.unassignedFlushedValue {
            state.localValue = unassigned
        } else if let submitted = state.lastSubmittedValue {
            state.localValue = submitted
        }
        nodes[nodeID] = state
    }

    /// Last store string applied or echoed for this editor.
    ///
    /// `nil` only when this node has no session state. An editor that never received
    /// `.value`/`.text` is seeded to `""` on mount or first local edit.
    public func lastKnownAuthoritative(for nodeID: NodeId) -> String? {
        nodes[nodeID]?.lastKnownAuthoritative
    }

    /// True when a coalesced draft newer than the in-flight submit is waiting locally.
    ///
    /// A rejected ack that will not be followed by a transaction must keep this typing
    /// and promote it; only a submit with no successor reverts to the last store string.
    public func hasUnsentSuccessorDraft(for nodeID: NodeId) -> Bool {
        guard let state = nodes[nodeID] else { return false }
        return state.pendingValue != nil || state.unassignedFlushedEditSeq != nil
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
        unassignedNodeOrder.removeAll(keepingCapacity: false)

        for nodeID in Array(nodes.keys) {
            guard var state = nodes[nodeID] else { continue }
            state.debounceTask?.cancel()
            state.debounceTask = nil
            state.pendingValue = nil
            state.lastFlushedValue = nil
            clearUnassignedEdit(&state, nodeID: nodeID)
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
        if preservingLocalTextAcrossRemount {
            abandonComposition(nodeID: nodeID)
            return nil
        }
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

    private func abandonAllCompositions() {
        for nodeID in Array(nodes.keys) {
            abandonComposition(nodeID: nodeID)
        }
    }

    /// First local edit on an unpublished editor treats the store baseline as empty.
    private func seededState(for nodeID: NodeId) -> NodeState {
        var state = nodes[nodeID] ?? NodeState()
        if state.lastKnownAuthoritative == nil {
            state.lastKnownAuthoritative = ""
        }
        return state
    }

    private func firstUnassignedNode() -> NodeId? {
        while let nodeID = unassignedNodeOrder.first {
            guard let state = nodes[nodeID],
                  state.unassignedFlushedEditSeq != nil,
                  state.unassignedFlushedValue != nil,
                  state.unassignedObservedRevision != nil,
                  state.unassignedLaneEpoch != nil else {
                unassignedNodeOrder.removeFirst()
                continue
            }
            return nodeID
        }
        return nil
    }

    private func clearUnassignedEdit(_ state: inout NodeState, nodeID: NodeId) {
        state.unassignedFlushedEditSeq = nil
        state.unassignedFlushedValue = nil
        state.unassignedObservedRevision = nil
        state.unassignedLaneEpoch = nil
        state.unassignedClaimed = false
        unassignedNodeOrder.removeAll { $0 == nodeID }
    }

    @discardableResult
    private func bumpLaneEpoch(nodeID: NodeId) -> UInt64 {
        let current = max(resyncLaneEpoch, laneEpoch[nodeID] ?? 0)
        let epoch = current == .max ? .max : current + 1
        laneEpoch[nodeID] = epoch
        return epoch
    }
}
