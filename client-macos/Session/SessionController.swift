//
// SessionController.swift
// Session
//
// Central client session coordinator wiring Transport, ProtocolDecoder, TransactionApplier,
// EventOutbox, ResourceCache, and RendererAppKit (§22, §22.2).
//
// Spec sections implemented:
// - §12.1 Revisions and transactions: transactions are applied atomically and the renderer never
//   observes a half-committed transaction.
// - §14 Resource model: resource metadata/chunks are assembled in ResourceCache; only verified
//   decoded images are committed to the renderer. Failures log/drop and keep placeholders.
// - §15 Capability negotiation: `CLIENT HELLO` / `SERVER WELCOME` establish the session; resume
//   reuses the retained negotiated set instead of re-parsing profiles from `RESUME_OK`.
// - §18 Reconnect and resynchronization: `CLIENT RESUME` carries `last_applied_revision` and
//   `last_acked_event_seq`. `SERVER RESYNC_REQUIRED` and a HELLO catch-up snapshot (when
//   `WELCOME.initial_revision > 0`) replace the replica; incremental transactions never do.
// - §18.2 Event settlement: server event frontiers raise `last_acked_event_seq`, while
//   per-event acknowledgements selectively drain the outbox's retry set.
// - §18.3 / §22.6 Native text editing: local drafts are claimed on MainActor, assigned a
//   monotonic `edit_seq`, and reconciled against `RESUME_OK` / `RESYNC_REQUIRED` discard lists
//   before any native mutation runs.
// - §22.2 Threading: network IO and protobuf decoding run off the main actor; AppKit mutations
//   are dispatched to `MainActor`.
// - §8 / §22.7 Sparse collections: `ClientModelRangeRequest` is sent on the `.ui` lane and is
//   not an Event. A copy arriving from the server is a protocol violation.
// - §4 inv. 13: unrecoverable divergence fails explicitly instead of degrading silently.
// - §21 Terminal compatibility: `TerminalData` / `TerminalResyncRequired` update the local
//   VT actor only. They never enter the semantic `ServerResyncRequired` path, discard
//   pending text edits, mutate EventOutbox, reset revision, disable dispatch, or mark
//   the session diverged. `ClientResume.terminal_stream_offsets` is independent of
//   `pending_text_edits`.
//

import Foundation
import SemanticModel
import Protocol
import TransportSSH
import RendererAppKit
import Resources
import Collections
import Text
import Terminal

/// Reason a session stopped tracking the authoritative semantic stream (§4 inv. 13, §18).
public enum SessionFailure: Error, Sendable, CustomStringConvertible {
    /// The local replica can no longer be advanced by the incoming transaction stream.
    /// Recovery is a fresh transport plus `CLIENT RESUME`, which the server answers with
    /// replay or `RESYNC_REQUIRED` (§18).
    case replicaDiverged(TxnError)
    /// A wire frame or transaction payload could not be decoded (§16, §26).
    case decodeFailed(String)
    /// A required handshake invariant was violated.
    case protocolViolation(String)
    /// The committed semantic state could not be presented by the native renderer.
    case rendererFailed(String)
    /// The transport stream ended or errored.
    case transportEnded(String)
    /// A newer reconnect attempt on the same outbox superseded this controller. Its handshake can
    /// never complete, so the owner discards this controller instead of reconnecting it (§18).
    case superseded(String)

    public var description: String {
        switch self {
        case .replicaDiverged(let err): return "local replica diverged: \(err)"
        case .decodeFailed(let msg): return "decode failed: \(msg)"
        case .protocolViolation(let msg): return "protocol violation: \(msg)"
        case .rendererFailed(let msg): return "renderer failed: \(msg)"
        case .transportEnded(let msg): return "transport ended: \(msg)"
        case .superseded(let msg): return "superseded by a newer reconnect attempt: \(msg)"
        }
    }
}

/// Event dispatch is disabled until the server proves session identity continuity (§18).
public enum SessionDispatchError: Error, Equatable, Sendable {
    case resumeNotConfirmed
}

/// Core protocol version this build speaks (§15).
public let SRUICoreVersion = "0.5.0"

/// Whether `advertised` names a core version this build can talk to (§15, §4 inv. 13).
///
/// Compatibility is decided on `major.minor`; the patch level is free. An absent field decodes to
/// the proto3 default `""`, which is indistinguishable from "omitted" on the wire, so it is
/// refused rather than read as "unspecified, therefore fine": a default must never be the thing
/// that authorizes a session.
func sruiCoreVersionIsCompatible(_ advertised: String) -> Bool {
    func majorMinor(_ version: String) -> (Substring, Substring)? {
        let parts = version.split(separator: ".", omittingEmptySubsequences: false)
        guard parts.count >= 2 else { return nil }
        let major = parts[0]
        let minor = parts[1]
        guard !major.isEmpty, !minor.isEmpty,
              major.allSatisfy(\.isNumber), minor.allSatisfy(\.isNumber) else { return nil }
        return (major, minor)
    }

    guard let advertised = majorMinor(advertised),
          let supported = majorMinor(SRUICoreVersion) else { return false }
    return advertised == supported
}

/// Connection handshake / data-plane phase (§15, §18).
///
/// Illegal `(phase, payload)` pairs fail in one place instead of combining `isRunning`,
/// `_isHandshakeComplete`, and nil resume-attempt flags.
private enum ProtocolPhase: Equatable {
    case idle
    case awaitingWelcome
    case awaitingResume(sessionId: String, generation: UInt64)
    case active(negotiated: CapabilitySet)
    case awaitingSnapshot(negotiated: CapabilitySet)
    case failed
}

private struct RendererUpdateResult: Sendable {
    var didRender: Bool
    var resyncLaneEpoch: UInt64?
    var failureDescription: String? = nil
    var wasSuperseded: Bool = false
}

private struct PendingReplayFailureOwnership: Sendable {
    var lifecycleGeneration: UInt64
    var binding: EventOutboxConnectionBinding
    var sessionIncarnation: EventOutboxSessionIncarnation
    var resumeGeneration: UInt64
}

private struct SessionFailureTeardownState: Sendable {
    var handler: @Sendable (SessionFailure) -> Void
    var replayGeneration: UInt64?
    var connectionBinding: EventOutboxConnectionBinding?
    var lifecycleGeneration: UInt64
}

private actor ReceiveLoopStartGate {
    private var isReleased = false
    private var waiter: CheckedContinuation<Void, Never>?

    func wait() async {
        guard !isReleased else { return }
        await withCheckedContinuation { continuation in
            if isReleased {
                continuation.resume()
            } else {
                waiter = continuation
            }
        }
    }

    func release() {
        isReleased = true
        waiter?.resume()
        waiter = nil
    }
}

private struct HandshakeSendOwnership {
    var token: UUID
    var lifecycleGeneration: UInt64
    var task: Task<Void, Error>
}

/// Central coordinator managing client session lifecycle, message decoding, store application,
/// outbox event dispatch, resource assembly, and UI rendering (§22, §22.2).
public final class SessionController: @unchecked Sendable {
    public let transport: any Transport
    public let applier: TransactionApplier
    public let outbox: EventOutbox
    public let decoder: ProtocolDecoder
    public let renderer: AppKitRenderer?
    public let resourceCache: ResourceCache
    public let clientCapabilities: CapabilitySet
    public let requiredServerProfiles: CapabilitySet

    private let lock = NSLock()
    private var streamDecoder = SRUIMessageStreamDecoder()
    private var receiveTask: Task<Void, Never>?
    /// The handshake send is published before it can enter Transport so stop() can cancel,
    /// close, and await it before admitting a restarted lifecycle.
    private var handshakeSendOwnership: HandshakeSendOwnership?
    private var isRunning = false
    /// Keeps restart inadmissible until an in-progress stop has closed and drained the transport.
    private var isStopping = false
    /// Monotonic ownership for async start/stop/failure continuations.
    private var lifecycleGeneration: UInt64 = 0
    /// Allows only the synchronous native flush staged by terminal suspension.
    private var isFlushingTextForDisconnect = false
    private var hasMountedInitialTree = false
    private var pendingResync = false
    /// Highest revision whose committed value has finished applying to the native renderer.
    private var lastRenderedRevision: UInt64 = 0
    private var currentSessionId: String?
    /// Identifies this controller/transport attempt inside the shared outbox.
    private var outboxConnectionBinding: EventOutboxConnectionBinding?
    private var actionHandlerWired = false
    /// Serializes native callbacks so a synchronous text flush is admitted before the action
    /// which caused editing to end (§18.2, §22.6).
    @MainActor private var interactionDispatchTail: Task<Void, Never>?
    /// Invalidates callbacks queued by an older authoritative text incarnation even when a
    /// replacement session keeps the same outbox connection binding.
    @MainActor private var interactionIncarnation: UInt64 = 0
    /// Actor-validated session-incarnation authority carried by every interaction admission.
    /// Protected by `lock` so public async send APIs can capture it before their first await.
    private var outboxSessionIncarnation: EventOutboxSessionIncarnation?
    /// Deterministic internal fault seams for renderer lifecycle regression tests.
    @MainActor var rendererUpdateInterceptorForTesting: (() throws -> Void)?
    private var _liveTransactionPublishedInterceptorForTesting: (@Sendable () async -> Void)?
    var liveTransactionPublishedInterceptorForTesting: (@Sendable () async -> Void)? {
        get { withStateLock { _liveTransactionPublishedInterceptorForTesting } }
        set { withStateLock { _liveTransactionPublishedInterceptorForTesting = newValue } }
    }
    private var _resumeRecoverySnapshotLoadedInterceptorForTesting: (@Sendable () async -> Void)?
    var resumeRecoverySnapshotLoadedInterceptorForTesting: (@Sendable () async -> Void)? {
        get { withStateLock { _resumeRecoverySnapshotLoadedInterceptorForTesting } }
        set { withStateLock { _resumeRecoverySnapshotLoadedInterceptorForTesting = newValue } }
    }
    private var _rendererResourcesPreloadedInterceptorForTesting: (@Sendable () async -> Void)?
    var rendererResourcesPreloadedInterceptorForTesting: (@Sendable () async -> Void)? {
        get { withStateLock { _rendererResourcesPreloadedInterceptorForTesting } }
        set { withStateLock { _rendererResourcesPreloadedInterceptorForTesting = newValue } }
    }
    private var _rendererDidRenderInterceptorForTesting: (@Sendable () async -> Void)?
    var rendererDidRenderInterceptorForTesting: (@Sendable () async -> Void)? {
        get { withStateLock { _rendererDidRenderInterceptorForTesting } }
        set { withStateLock { _rendererDidRenderInterceptorForTesting = newValue } }
    }
    private var _interactionWillEnterOutboxForTesting: (@Sendable () async -> Void)?
    var interactionWillEnterOutboxForTesting: (@Sendable () async -> Void)? {
        get { withStateLock { _interactionWillEnterOutboxForTesting } }
        set { withStateLock { _interactionWillEnterOutboxForTesting = newValue } }
    }
    private var _textEditWillAuthorizeForTesting: (@Sendable () async -> Void)?
    var textEditWillAuthorizeForTesting: (@Sendable () async -> Void)? {
        get { withStateLock { _textEditWillAuthorizeForTesting } }
        set { withStateLock { _textEditWillAuthorizeForTesting = newValue } }
    }
    private var _receiveLoopWillAdoptForTesting: (@Sendable () async -> Void)?
    var receiveLoopWillAdoptForTesting: (@Sendable () async -> Void)? {
        get { withStateLock { _receiveLoopWillAdoptForTesting } }
        set { withStateLock { _receiveLoopWillAdoptForTesting = newValue } }
    }
    private var _handshakeSendDidFinishForTesting: (@Sendable () async -> Void)?
    var handshakeSendDidFinishForTesting: (@Sendable () async -> Void)? {
        get { withStateLock { _handshakeSendDidFinishForTesting } }
        set { withStateLock { _handshakeSendDidFinishForTesting = newValue } }
    }
    private var _pendingReplayFailureWillReportForTesting: (@Sendable () async -> Void)?
    var pendingReplayFailureWillReportForTesting: (@Sendable () async -> Void)? {
        get { withStateLock { _pendingReplayFailureWillReportForTesting } }
        set { withStateLock { _pendingReplayFailureWillReportForTesting = newValue } }
    }
    private var _stopWillRetireMountForTesting: (@Sendable () async -> Void)?
    var stopWillRetireMountForTesting: (@Sendable () async -> Void)? {
        get { withStateLock { _stopWillRetireMountForTesting } }
        set { withStateLock { _stopWillRetireMountForTesting = newValue } }
    }
    private var _resourceReferencesSynchronizedInterceptorForTesting:
        (@Sendable () async -> Void)?
    var resourceReferencesSynchronizedInterceptorForTesting: (@Sendable () async -> Void)? {
        get { withStateLock { _resourceReferencesSynchronizedInterceptorForTesting } }
        set { withStateLock { _resourceReferencesSynchronizedInterceptorForTesting = newValue } }
    }
    private var _resourceCommitReadyInterceptorForTesting: (@Sendable () async -> Void)?
    var resourceCommitReadyInterceptorForTesting: (@Sendable () async -> Void)? {
        get { withStateLock { _resourceCommitReadyInterceptorForTesting } }
        set { withStateLock { _resourceCommitReadyInterceptorForTesting = newValue } }
    }
    private var _isDiverged = false
    private var _onFailure: (@Sendable (SessionFailure) -> Void)?
    private var requestedSessionId: String?
    /// Generation of this controller's outstanding resume attempt (§18). Strictly increasing per
    /// outbox: a decision carrying an older generation is discarded outright.
    private var resumeGeneration: UInt64?
    private var activeReplayRetryGeneration: UInt64?
    private var eventDispatchEnabled = false
    private var phase: ProtocolPhase = .idle
    /// Negotiated set from the last successful `SERVER WELCOME`, retained across `stop()` so a
    /// later `CLIENT RESUME` can restore it (§15, §18).
    private var retainedCapabilities: CapabilitySet?
    /// Hashes whose transfer was already rejected; suppress per-chunk log spam (§14, §26).
    private var rejectedResourceHashes: Set<ResourceHash> = []
    private var rangeRequestContinuation: AsyncStream<CollectionRangeRequest>.Continuation?
    private var rangeRequestTask: Task<Void, Never>?
    private let terminalPump = TerminalCommandPump()

    public init(
        transport: any Transport,
        applier: TransactionApplier = TransactionApplier(),
        outbox: EventOutbox = EventOutbox(),
        decoder: ProtocolDecoder = ProtocolDecoder(),
        renderer: AppKitRenderer? = nil,
        /// Inject a shared cache across reconnecting controller instances so committed resources
        /// survive replacement; the default constructs a fresh CAS per controller (§14, §18).
        resourceCache: ResourceCache = ResourceCache(),
        sessionId: String? = nil,
        clientCapabilities: CapabilitySet = [Profile.standardWidgetsV1, Profile.terminalV1],
        requiredServerProfiles: CapabilitySet = []
    ) {
        self.transport = transport
        self.applier = applier
        self.outbox = outbox
        self.decoder = decoder
        self.renderer = renderer
        self.resourceCache = resourceCache
        self.currentSessionId = sessionId
        self.clientCapabilities = clientCapabilities
        self.requiredServerProfiles = requiredServerProfiles
    }

    private func withStateLock<T>(_ body: () -> T) -> T {
        lock.lock()
        defer { lock.unlock() }
        return body()
    }

    /// Whether user events may be sent. False during handshake and while a catch-up or
    /// resync snapshot is still outstanding (§15, §18).
    public var isEventDispatchEnabled: Bool {
        withStateLock { eventDispatchEnabled }
    }

    /// Whether the handshake has completed successfully (§15).
    public var isHandshakeComplete: Bool {
        withStateLock {
            switch phase {
            case .active, .awaitingSnapshot:
                return true
            case .idle, .awaitingWelcome, .awaitingResume, .failed:
                return false
            }
        }
    }

    /// Active negotiated capability set with the remote server (§15).
    public var negotiatedCapabilities: CapabilitySet? {
        withStateLock {
            switch phase {
            case .active(let negotiated), .awaitingSnapshot(let negotiated):
                return negotiated
            case .idle, .awaitingWelcome, .awaitingResume, .failed:
                return nil
            }
        }
    }

    /// Whether the local replica has stopped tracking the authoritative stream (§18).
    ///
    /// Once true the session no longer applies transactions; the caller must establish a new
    /// transport and resume, which the server answers with replay or a resync snapshot.
    public var isDiverged: Bool {
        withStateLock { _isDiverged }
    }

    /// Current active session ID assigned by the server or requested during resume (§15, §18).
    public var sessionId: String? {
        withStateLock { currentSessionId }
    }

    /// Invoked when the session stops tracking the authoritative stream. Always also reported to
    /// stderr, so a session can never fail completely silently (§4 inv. 13).
    public var onFailure: (@Sendable (SessionFailure) -> Void)? {
        get { withStateLock { _onFailure } }
        set { withStateLock { _onFailure = newValue } }
    }

    /// Attaches the renderer action trampoline to forward UI events to the outbox (§7.7, §22).
    @MainActor
    public func attachRenderer(_ renderer: AppKitRenderer) {
        wireActionHandler(for: renderer)
    }

    @MainActor
    private func wireActionHandler(for renderer: AppKitRenderer) {
        guard !actionHandlerWired else { return }
        actionHandlerWired = true

        renderer.textEditingSession.onAssignedIdentityRevoked = { [weak self] eventId in
            self?.outbox.revokeUnauthorizedPreparedTextEdit(eventId: eventId)
        }

        renderer.onInteraction = { [weak self, weak renderer] interaction in
            guard let self else { return }

            // §7.7: observed_revision is the revision the user was actually looking at when the
            // control was interacted with. Record it synchronously even while disconnected so a
            // locally committed text edit can be replayed if this same session resumes (§18.3).
            // Dispatch remains gated below; a replacement/full resync discards the recorded draft.
            let observedRev = self.applier.currentSnapshot.revision
            if case .textEdit(let nodeID, let text, let editSeq, let laneEpoch) = interaction {
                guard renderer?.textEditingSession.recordObservedRevision(
                    nodeID: nodeID,
                    text: text,
                    editSeq: editSeq,
                    laneEpoch: laneEpoch,
                    observedRevision: observedRev
                ) == true else {
                    return
                }
            }

            let acceptsInteraction = self.withStateLock {
                (self.isRunning && !self.isStopping && !self._isDiverged)
                    || self.isFlushingTextForDisconnect
            }
            guard acceptsInteraction else { return }

            // AppKit can report the button/menu action before the editor's debounce fires. Flush
            // every committed, non-composing native value synchronously; each recursive text-edit
            // callback is appended to the same dispatch tail before this action is appended.
            switch interaction {
            case .textEdit:
                break
            case .activate, .valueChanged, .selectionChanged:
                renderer?.textEditingSession.flushAllPending()
            }

            let drainCutoff: UInt64
            switch interaction {
            case .textEdit(let nodeID, _, _, _):
                drainCutoff = renderer?.textEditingSession.unassignedFlushGeneration(for: nodeID)
                    ?? renderer?.textEditingSession.currentFlushGeneration ?? 0
            case .activate, .valueChanged, .selectionChanged:
                drainCutoff = renderer?.textEditingSession.currentFlushGeneration ?? 0
            }

            guard let ownership = self.withStateLock({ () -> (
                EventOutboxConnectionBinding,
                EventOutboxSessionIncarnation
            )? in
                guard let binding = self.outboxConnectionBinding,
                      let sessionIncarnation = self.outboxSessionIncarnation else {
                    return nil
                }
                return (binding, sessionIncarnation)
            }) else {
                SessionDiagnostics.error("Interaction dispatch skipped without outbox session ownership")
                return
            }

            // A text callback is the ordering boundary itself. Claim its exact snapshot before
            // yielding MainActor; otherwise later typing can replace the coalesced slot before the
            // queued dispatch task starts.
            let initialTextEdit: LocalTextEdit?
            if case .textEdit(let nodeID, let text, let editSeq, let laneEpoch) = interaction {
                guard let claimed = renderer?.textEditingSession.claimUnassignedEdit(
                    nodeID: nodeID,
                    text: text,
                    editSeq: editSeq,
                    laneEpoch: laneEpoch
                ) else {
                    return
                }
                initialTextEdit = claimed
            } else {
                initialTextEdit = nil
            }

            let (binding, sessionIncarnation) = ownership
            let incarnation = self.interactionIncarnation
            let predecessor = self.interactionDispatchTail
            let dispatch = Task { [weak self] in
                _ = await predecessor?.result
                guard let self else { return }
                guard !Task.isCancelled,
                      self.interactionIncarnation == incarnation else {
                    if let initialTextEdit {
                        self.renderer?.textEditingSession.releaseClaim(initialTextEdit)
                    }
                    return
                }
                if let interceptor = self.interactionWillEnterOutboxForTesting {
                    await interceptor()
                }
                do {
                    switch interaction {
                    case .activate(let nodeID):
                        try await self.sendActivate(
                            nodeId: nodeID,
                            observedRevision: observedRev,
                            binding: binding,
                            sessionIncarnation: sessionIncarnation,
                            maxFlushGeneration: drainCutoff
                        )
                    case .valueChanged(let nodeID, let value):
                        try await self.sendValueChanged(
                            nodeId: nodeID,
                            observedRevision: observedRev,
                            value: value,
                            binding: binding,
                            sessionIncarnation: sessionIncarnation,
                            maxFlushGeneration: drainCutoff
                        )
                    case .selectionChanged(let nodeID, let itemID):
                        try await self.sendSelectionChanged(
                            nodeId: nodeID,
                            observedRevision: observedRev,
                            itemId: itemID,
                            binding: binding,
                            sessionIncarnation: sessionIncarnation,
                            maxFlushGeneration: drainCutoff
                        )
                    case .textEdit:
                        try await self.dispatchUnassignedTextEdits(
                            binding: binding,
                            sessionIncarnation: sessionIncarnation,
                            interactionIncarnation: incarnation,
                            maxFlushGeneration: drainCutoff,
                            initialEdit: initialTextEdit
                        )
                    }
                } catch {
                    // Before-admission failures leave the synchronous bridge intact for terminal
                    // handoff. A post-retention send failure is harmlessly re-staged by identity.
                    SessionDiagnostics.error("Interaction dispatch failed: \(error)")
                }
            }
            self.interactionDispatchTail = dispatch
        }

        renderer.onTerminalInput = { [weak self] nodeID, data in
            guard let self else { return }
            let predecessor = self.interactionDispatchTail
            let dispatch = Task { [weak self] in
                _ = await predecessor?.result
                guard let self, !Task.isCancelled else { return }
                await self.terminalPump.enqueueInput(streamID: nodeID, data: data)
            }
            self.interactionDispatchTail = dispatch
        }
        renderer.onTerminalResize = { [weak self] nodeID, cols, rows, width, height in
            guard let self else { return }
            let predecessor = self.interactionDispatchTail
            let dispatch = Task { [weak self] in
                _ = await predecessor?.result
                guard let self, !Task.isCancelled else { return }
                await self.terminalPump.enqueueResize(streamID: nodeID, columns: cols, rows: rows, pixelWidth: width, pixelHeight: height)
            }
            self.interactionDispatchTail = dispatch
        }

        renderer.onCollectionRangeRequest = { [weak self, weak renderer] request in
            guard let self else { return }
            // A request emitted while the pump is torn down (between sessions) would
            // otherwise stay marked in-flight in the adapter's tracker and suppress the
            // re-request after reconnect. Give the coverage straight back (§8, §22.7).
            guard let continuation = self.rangeRequestContinuation else {
                renderer?.noteDroppedCollectionRange(request)
                return
            }
            if case .terminated = continuation.yield(request) {
                renderer?.noteDroppedCollectionRange(request)
            }
        }
    }

    /// Drains TextEditingSession-owned drafts through retain, native authorization, then send.
    ///
    /// The callback's initial edit is snapshotted synchronously on MainActor. Later typing may
    /// coalesce into the live slot, but this drain still sends the claimed snapshot and a non-text
    /// interaction only admits generations `<= maxFlushGeneration`.
    private func dispatchUnassignedTextEdits(
        binding: EventOutboxConnectionBinding,
        sessionIncarnation: EventOutboxSessionIncarnation,
        interactionIncarnation: UInt64,
        maxFlushGeneration: UInt64,
        initialEdit: LocalTextEdit? = nil
    ) async throws {
        var queuedEdit = initialEdit
        while true {
            let edit: LocalTextEdit
            if let initial = queuedEdit {
                edit = initial
                queuedEdit = nil
            } else {
                guard withStateLock({
                    outboxConnectionBinding == binding
                        && outboxSessionIncarnation == sessionIncarnation
                }) else {
                    return
                }
                guard let claimed = await MainActor.run(body: { () -> LocalTextEdit? in
                    guard self.interactionIncarnation == interactionIncarnation else { return nil }
                    return self.renderer?.textEditingSession.claimNextUnassignedEdit(
                        maxFlushGeneration: maxFlushGeneration
                    )
                }) else {
                    return
                }
                edit = claimed
            }

            var prepared: PreparedTextEdit?
            do {
                try Task.checkCancellation()
                guard withStateLock({
                    outboxConnectionBinding == binding
                        && outboxSessionIncarnation == sessionIncarnation
                }) else {
                    await MainActor.run {
                        self.renderer?.textEditingSession.releaseClaim(edit)
                    }
                    return
                }
                try await outbox.waitUntilTextEditLaneIsAvailable(
                    nodeId: edit.nodeId,
                    binding: binding,
                    sessionIncarnation: sessionIncarnation
                )
                let snapshotStillValid = await MainActor.run { () -> Bool in
                    guard self.interactionIncarnation == interactionIncarnation else { return false }
                    return self.renderer?.textEditingSession.isSnapshotStillValid(edit) == true
                }
                if !snapshotStillValid {
                    await MainActor.run {
                        self.renderer?.textEditingSession.releaseClaim(edit)
                    }
                    continue
                }

                guard let retained = try await outbox.prepareTextEdit(
                    nodeId: edit.nodeId,
                    text: edit.text,
                    editSeq: edit.editSeq,
                    observedRevision: edit.observedRevision,
                    binding: binding,
                    sessionIncarnation: sessionIncarnation,
                    via: transport
                ) else {
                    await MainActor.run {
                        self.renderer?.textEditingSession.releaseClaim(edit)
                    }
                    continue
                }
                prepared = retained
                let assigned = await MainActor.run { () -> Bool in
                    guard self.interactionIncarnation == interactionIncarnation,
                          self.withStateLock({
                              self.outboxConnectionBinding == binding
                                  && self.outboxSessionIncarnation == sessionIncarnation
                          }) else {
                        return false
                    }
                    return self.renderer?.textEditingSession.noteAssigned(
                        retained.event,
                        matching: edit
                    ) == true
                }
                if !assigned {
                    let rejected = await outbox.rejectPreparedTextEdit(retained)
                    await MainActor.run {
                        self.renderer?.textEditingSession.releaseClaim(edit)
                    }
                    if rejected {
                        continue
                    }
                    return
                }
                if let interceptor = textEditWillAuthorizeForTesting {
                    await interceptor()
                }
                guard await outbox.authorizePreparedTextEdit(retained) else {
                    await MainActor.run {
                        self.renderer?.textEditingSession.restoreUnassigned(
                            edit,
                            from: retained.event
                        )
                    }
                    if await outbox.assignedTextEditEvents().contains(where: {
                        $0.eventId == retained.event.eventId
                    }) {
                        return
                    }
                    continue
                }
                guard try await outbox.releasePreparedTextEdit(retained) != nil else {
                    await MainActor.run {
                        self.renderer?.textEditingSession.restoreUnassigned(
                            edit,
                            from: retained.event
                        )
                    }
                    return
                }
            } catch {
                if prepared == nil {
                    await MainActor.run {
                        self.renderer?.textEditingSession.releaseClaim(edit)
                    }
                }
                throw error
            }
        }
    }

    @MainActor
    private func advanceInteractionIncarnation() {
        precondition(
            interactionIncarnation < UInt64.max,
            "SessionController interaction incarnation exhausted"
        )
        interactionIncarnation += 1
        interactionDispatchTail?.cancel()
        interactionDispatchTail = nil
    }

    @MainActor
    private func ensureActionHandlerWired() {
        guard let renderer else { return }
        wireActionHandler(for: renderer)
    }

    private func startRangeRequestPump() {
        stopRangeRequestPump()
        let (stream, continuation) = AsyncStream.makeStream(
            of: CollectionRangeRequest.self
        )
        rangeRequestContinuation = continuation
        rangeRequestTask = Task { [weak self] in
            for await request in stream {
                guard let self, !Task.isCancelled else { break }
                await self.sendCollectionRangeRequest(request)
            }
        }
    }

    private func stopRangeRequestPump() {
        rangeRequestContinuation?.finish()
        rangeRequestContinuation = nil
        rangeRequestTask?.cancel()
        rangeRequestTask = nil
    }

    private func sendCollectionRangeRequest(_ request: CollectionRangeRequest) async {
        let allowed = withStateLock { allowsDataPlane(phase) && isRunning && !_isDiverged }
        guard allowed else {
            await noteDroppedCollectionRange(request)
            return
        }

        var envelope = SRUIClientModelRangeRequest()
        envelope.nodeID = request.nodeID.value
        envelope.modelID = request.modelID.value
        envelope.startIndex = request.startIndex
        envelope.count = request.count
        envelope.observedRevision = applier.lastAppliedRevision.value

        var message = SRUIMessage()
        message.clientModelRangeRequest = envelope
        do {
            try await transport.send(
                data: try SRUIFraming.encodeFramed(message),
                logicalClass: .ui
            )
        } catch {
            await noteDroppedCollectionRange(request)
            SessionDiagnostics.error("Collection range request send failed: \(error)")
        }
    }

    private func noteDroppedCollectionRange(_ request: CollectionRangeRequest) async {
        await MainActor.run {
            self.renderer?.noteDroppedCollectionRange(request)
        }
    }

    /// Re-request each collection's last viewport once the `.ui` lane can send.
    /// Must not run from `stop()` (the pump is already gone) or from a failed
    /// send (that storms every adapter). Handshake start is still
    /// `.awaitingWelcome`, so this waits until the session becomes `.active`.
    private func reissueCollectionRangeRequestsIfAllowed() async {
        let allowed = withStateLock { allowsDataPlane(phase) && isRunning && !_isDiverged }
        guard allowed else { return }
        await MainActor.run {
            self.renderer?.reissueCollectionRangeRequests()
        }
    }

    private func sendHandshakeEnvelope(
        _ envelope: SRUIMessage,
        lifecycleGeneration: UInt64
    ) async throws {
        let framed = try SRUIFraming.encodeFramed(envelope)
        let startGate = ReceiveLoopStartGate()
        let token = UUID()
        let transport = self.transport
        let task = Task<Void, Error> {
            await startGate.wait()
            try Task.checkCancellation()
            try await transport.send(data: framed, logicalClass: .control)
        }
        let adopted = withStateLock { () -> Bool in
            guard self.lifecycleGeneration == lifecycleGeneration,
                  isRunning,
                  !isStopping,
                  handshakeSendOwnership == nil else {
                return false
            }
            handshakeSendOwnership = HandshakeSendOwnership(
                token: token,
                lifecycleGeneration: lifecycleGeneration,
                task: task
            )
            return true
        }
        guard adopted else {
            task.cancel()
            await startGate.release()
            _ = try? await task.value
            throw SessionFailure.superseded(
                "start lost lifecycle ownership before handshake send adoption"
            )
        }

        defer {
            withStateLock {
                guard handshakeSendOwnership?.token == token,
                      handshakeSendOwnership?.lifecycleGeneration == lifecycleGeneration else {
                    return
                }
                handshakeSendOwnership = nil
            }
        }
        await startGate.release()
        do {
            try await withTaskCancellationHandler {
                try await task.value
            } onCancel: {
                task.cancel()
            }
            if let interceptor = handshakeSendDidFinishForTesting {
                await interceptor()
            }
        } catch {
            if let interceptor = handshakeSendDidFinishForTesting {
                await interceptor()
            }
            throw error
        }
    }

    /// Starts the session by sending a handshake request and launching the receive loop (§15, §18, §22.2).
    ///
    /// Fresh connections send `CLIENT_HELLO` with client capabilities (§15). Reconnections send `CLIENT_RESUME` (§18).
    /// The handshake must complete before any transaction or event traffic is permitted (§4 inv. 13, §15).
    public func start() async throws {
        let lifecycleGeneration = withStateLock { () -> UInt64? in
            guard !isRunning, !isStopping else { return nil }
            precondition(
                self.lifecycleGeneration < UInt64.max,
                "SessionController lifecycle generation exhausted"
            )
            self.lifecycleGeneration += 1
            isRunning = true
            eventDispatchEnabled = false
            phase = .idle
            return self.lifecycleGeneration
        }
        guard let lifecycleGeneration else { return }
        var activatedResourceOwnerEpoch: UInt64?

        startRangeRequestPump()

        do {
            let connectionBinding = await outbox.beginConnectionBinding()
            guard ownsRunningLifecycle(lifecycleGeneration) else {
                throw SessionFailure.superseded("start lost lifecycle ownership during binding")
            }
            let currentResourceReferences = await currentLiveResourceReferences()
            guard await resourceCache.activateReferenceOwner(
                epoch: connectionBinding.resourceOwnershipEpoch,
                liveReferences: currentResourceReferences
            ) else {
                throw SessionFailure.superseded(
                    "connection lost resource-cache ownership during binding"
                )
            }
            activatedResourceOwnerEpoch = connectionBinding.resourceOwnershipEpoch
            guard ownsRunningLifecycle(lifecycleGeneration),
                  let sessionIncarnation = await outbox.sessionIncarnation(
                    binding: connectionBinding
                  ) else {
                throw SessionFailure.superseded("start lost lifecycle ownership during binding")
            }
            let adoptedBinding = withStateLock { () -> Bool in
                guard self.lifecycleGeneration == lifecycleGeneration,
                      isRunning,
                      !isStopping else {
                    return false
                }
                outboxConnectionBinding = connectionBinding
                outboxSessionIncarnation = sessionIncarnation
                return true
            }
            guard adoptedBinding else {
                throw SessionFailure.superseded("start lost lifecycle ownership during binding")
            }

            await MainActor.run {
                ensureActionHandlerWired()
            }
            guard ownsRunningLifecycle(lifecycleGeneration) else {
                throw SessionFailure.superseded("start lost lifecycle ownership before handshake")
            }

            let clientInstanceId = outbox.clientInstanceId
            let requestedId = withStateLock {
                currentSessionId ?? requestedSessionId
            }
            let limits = makeClientLimits()
            let knownResourceHashes = await resourceCache.knownHashes().map(\.bytes)
            guard ownsRunningLifecycle(lifecycleGeneration) else {
                throw SessionFailure.superseded("start lost lifecycle ownership before handshake")
            }

            if let requestedId {
                guard let resumeGeneration = await outbox.beginResumeAttempt(
                    binding: connectionBinding
                ) else {
                    throw SessionFailure.superseded(
                        "resume start lost its outbox connection binding"
                    )
                }
                let adoptedResume = withStateLock { () -> Bool in
                    guard self.lifecycleGeneration == lifecycleGeneration,
                          isRunning,
                          !isStopping else {
                        return false
                    }
                    self.requestedSessionId = requestedId
                    self.resumeGeneration = resumeGeneration
                    return true
                }
                guard adoptedResume else {
                    await outbox.stopResumeWork(generation: resumeGeneration)
                    throw SessionFailure.superseded(
                        "resume start lost lifecycle ownership"
                    )
                }
                var resume = SRUIClientResume()
                resume.sessionID = requestedId
                resume.clientInstanceID = clientInstanceId.bytes
                resume.lastAppliedRevision = applier.lastAppliedRevision.value
                resume.lastAckedEventSeq = await outbox.lastAckedEventSeq
                resume.limits = limits
                resume.knownResourceHashes = knownResourceHashes
                resume.pendingTextEdits = await outbox.assignedTextEditDescriptors().map {
                    $0.toWire()
                }
                if let renderer {
                    resume.terminalStreamOffsets = await renderer.terminalSession.streamOffsets()
                }

                var envelope = SRUIMessage()
                envelope.clientResume = resume
                guard ownsRunningLifecycle(lifecycleGeneration) else {
                    await outbox.stopResumeWork(generation: resumeGeneration)
                    throw SessionFailure.superseded(
                        "resume start lost lifecycle ownership before send"
                    )
                }
                try await sendHandshakeEnvelope(
                    envelope,
                    lifecycleGeneration: lifecycleGeneration
                )
                let adoptedPhase = withStateLock { () -> Bool in
                    guard self.lifecycleGeneration == lifecycleGeneration,
                          isRunning,
                          !isStopping else {
                        return false
                    }
                    phase = .awaitingResume(
                        sessionId: requestedId,
                        generation: resumeGeneration
                    )
                    return true
                }
                guard adoptedPhase else {
                    await outbox.stopResumeWork(generation: resumeGeneration)
                    throw SessionFailure.superseded(
                        "resume start lost lifecycle ownership after send"
                    )
                }
            } else {
                var hello = SRUIClientHello()
                hello.coreVersion = SRUICoreVersion
                hello.profiles = clientCapabilities.toStringArray()
                hello.clientInstanceID = clientInstanceId.bytes
                hello.limits = limits
                hello.knownResourceHashes = knownResourceHashes

                var envelope = SRUIMessage()
                envelope.clientHello = hello
                guard ownsRunningLifecycle(lifecycleGeneration) else {
                    throw SessionFailure.superseded(
                        "fresh start lost lifecycle ownership before send"
                    )
                }
                try await sendHandshakeEnvelope(
                    envelope,
                    lifecycleGeneration: lifecycleGeneration
                )
                let adoptedPhase = withStateLock { () -> Bool in
                    guard self.lifecycleGeneration == lifecycleGeneration,
                          isRunning,
                          !isStopping else {
                        return false
                    }
                    phase = .awaitingWelcome
                    return true
                }
                guard adoptedPhase else {
                    throw SessionFailure.superseded(
                        "fresh start lost lifecycle ownership after send"
                    )
                }
            }

            guard ownsRunningLifecycle(lifecycleGeneration) else {
                throw SessionFailure.superseded(
                    "start lost lifecycle ownership before receive loop"
                )
            }

            await terminalPump.attach(transport: transport)

            // The detached loop waits behind this gate until its task is published under the
            // lifecycle lock. A concurrent stop therefore either owns and awaits the task, or
            // prevents it from ever entering message dispatch.
            let receiveStartGate = ReceiveLoopStartGate()
            let task = Task.detached { [weak self] in
                await receiveStartGate.wait()
                guard let self, !Task.isCancelled else { return }
                await self.runReceiveLoop()
            }
            if let interceptor = receiveLoopWillAdoptForTesting {
                await interceptor()
            }

            let adopted = withStateLock { () -> Bool in
                guard self.lifecycleGeneration == lifecycleGeneration,
                      isRunning,
                      !isStopping else {
                    return false
                }
                receiveTask = task
                return true
            }
            guard adopted else {
                task.cancel()
                await receiveStartGate.release()
                await task.value
                throw SessionFailure.superseded(
                    "start lost lifecycle ownership before receive-loop adoption"
                )
            }
            await receiveStartGate.release()
        } catch {
            await cleanUpFailedStart(generation: lifecycleGeneration)
            if let activatedResourceOwnerEpoch {
                _ = await resourceCache.deactivateReferenceOwner(
                    epoch: activatedResourceOwnerEpoch
                )
            }
            throw error
        }
    }

    private func ownsRunningLifecycle(_ generation: UInt64) -> Bool {
        withStateLock {
            lifecycleGeneration == generation && isRunning && !isStopping
        }
    }

    private func cleanUpFailedStart(generation: UInt64) async {
        let cleanup: (
            resumeGeneration: UInt64?,
            connectionBinding: EventOutboxConnectionBinding?
        )? = withStateLock {
            guard lifecycleGeneration == generation, !isStopping else { return nil }
            isStopping = true
            eventDispatchEnabled = false
            return (
                resumeGeneration ?? activeReplayRetryGeneration,
                outboxConnectionBinding
            )
        }
        guard let cleanup else { return }

        await terminalPump.disconnect()
        stopRangeRequestPump()
        if let resumeGeneration = cleanup.resumeGeneration {
            await outbox.stopResumeWork(generation: resumeGeneration)
        }
        await transport.close()
        if let connectionBinding = cleanup.connectionBinding {
            _ = await resourceCache.clearPartials(
                ownerEpoch: connectionBinding.resourceOwnershipEpoch
            )
        }
        withStateLock {
            guard lifecycleGeneration == generation else { return }
            handshakeSendOwnership = nil
            receiveTask = nil
            requestedSessionId = nil
            resumeGeneration = nil
            activeReplayRetryGeneration = nil
            outboxConnectionBinding = nil
            outboxSessionIncarnation = nil
            isFlushingTextForDisconnect = false
            eventDispatchEnabled = false
            phase = .idle
            isRunning = false
            isStopping = false
        }
    }
    private func makeClientLimits() -> Srui_Protocol_ClientLimits {
        var limits = Srui_Protocol_ClientLimits()
        limits.maxFrameSize = UInt32(clamping: defaultMaxFrameSize)
        limits.maxTransactionOperations = UInt32(
            clamping: applier.currentSnapshot.store.limits.maxTransactionOperations
        )
        limits.maxTreeDepth = UInt32(clamping: applier.currentSnapshot.store.limits.maxTreeDepth)
        limits.maxNodeCount = UInt32(clamping: applier.currentSnapshot.store.limits.maxNodeCount)
        limits.maxStringLength = UInt32(
            clamping: applier.currentSnapshot.store.limits.maxStringLength
        )
        limits.maxResourceSize = UInt32(clamping: resourceCache.limits.maxEncodedBytes)
        return limits
    }

    /// Flushes every committed, non-composing native value and waits for the resulting text-edit
    /// Tasks so a manual activate/value/selection cannot overtake a still-debounced draft.
    ///
    /// `flushAllPending()` commits through `onCommit`, which claims each draft onto
    /// `interactionDispatchTail`. Capturing that tail on the same MainActor hop is required:
    /// the later inline drain skips claimed identities, so returning without waiting would
    /// allocate the non-text event first.
    private func flushPendingTextForManualNonTextInteraction() async -> UInt64 {
        let (drainCutoff, predecessor) = await MainActor.run {
            () -> (UInt64, Task<Void, Never>?) in
            self.renderer?.textEditingSession.flushAllPending()
            return (
                self.renderer?.textEditingSession.currentFlushGeneration ?? 0,
                self.interactionDispatchTail
            )
        }
        _ = await predecessor?.result
        return drainCutoff
    }

    /// Dispatches a manual activation event for the given node ID (§7.7).
    @discardableResult
    public func sendActivate(nodeId: NodeId) async throws -> Event {
        guard let ownership = withStateLock({ () -> (
            EventOutboxConnectionBinding,
            EventOutboxSessionIncarnation
        )? in
            guard let binding = outboxConnectionBinding,
                  let sessionIncarnation = outboxSessionIncarnation else {
                return nil
            }
            return (binding, sessionIncarnation)
        }) else {
            throw SessionDispatchError.resumeNotConfirmed
        }
        let snapshot = applier.currentSnapshot
        if let interceptor = interactionWillEnterOutboxForTesting {
            await interceptor()
        }
        let drainCutoff = await flushPendingTextForManualNonTextInteraction()
        return try await sendActivate(
            nodeId: nodeId,
            observedRevision: snapshot.revision,
            binding: ownership.0,
            sessionIncarnation: ownership.1,
            maxFlushGeneration: drainCutoff
        )
    }

    @discardableResult
    private func sendActivate(
        nodeId: NodeId,
        observedRevision: Revision,
        binding: EventOutboxConnectionBinding,
        sessionIncarnation: EventOutboxSessionIncarnation,
        maxFlushGeneration: UInt64
    ) async throws -> Event {
        let incarnation = await MainActor.run { self.interactionIncarnation }
        try await dispatchUnassignedTextEdits(
            binding: binding,
            sessionIncarnation: sessionIncarnation,
            interactionIncarnation: incarnation,
            maxFlushGeneration: maxFlushGeneration
        )
        guard withStateLock({
            guard eventDispatchEnabled,
                  outboxConnectionBinding == binding,
                  outboxSessionIncarnation == sessionIncarnation else {
                return false
            }
            if case .active = phase { return true }
            return false
        }) else {
            throw SessionDispatchError.resumeNotConfirmed
        }
        return try await outbox.sendActivate(
            nodeId: nodeId,
            observedRevision: observedRevision,
            binding: binding,
            sessionIncarnation: sessionIncarnation,
            via: transport
        )
    }

    /// Dispatches a manual value change event for the given node ID (§7.6).
    @discardableResult
    public func sendValueChanged(nodeId: NodeId, value: Value) async throws -> Event {
        guard let ownership = withStateLock({ () -> (
            EventOutboxConnectionBinding,
            EventOutboxSessionIncarnation
        )? in
            guard let binding = outboxConnectionBinding,
                  let sessionIncarnation = outboxSessionIncarnation else {
                return nil
            }
            return (binding, sessionIncarnation)
        }) else {
            throw SessionDispatchError.resumeNotConfirmed
        }
        let snapshot = applier.currentSnapshot
        if let interceptor = interactionWillEnterOutboxForTesting {
            await interceptor()
        }
        let drainCutoff = await flushPendingTextForManualNonTextInteraction()
        return try await sendValueChanged(
            nodeId: nodeId,
            observedRevision: snapshot.revision,
            value: value,
            binding: ownership.0,
            sessionIncarnation: ownership.1,
            maxFlushGeneration: drainCutoff
        )
    }

    @discardableResult
    private func sendValueChanged(
        nodeId: NodeId,
        observedRevision: Revision,
        value: Value,
        binding: EventOutboxConnectionBinding,
        sessionIncarnation: EventOutboxSessionIncarnation,
        maxFlushGeneration: UInt64
    ) async throws -> Event {
        let incarnation = await MainActor.run { self.interactionIncarnation }
        try await dispatchUnassignedTextEdits(
            binding: binding,
            sessionIncarnation: sessionIncarnation,
            interactionIncarnation: incarnation,
            maxFlushGeneration: maxFlushGeneration
        )
        guard withStateLock({
            guard eventDispatchEnabled,
                  outboxConnectionBinding == binding,
                  outboxSessionIncarnation == sessionIncarnation else {
                return false
            }
            if case .active = phase { return true }
            return false
        }) else {
            throw SessionDispatchError.resumeNotConfirmed
        }
        return try await outbox.sendValueChanged(
            nodeId: nodeId,
            observedRevision: observedRevision,
            value: value,
            binding: binding,
            sessionIncarnation: sessionIncarnation,
            via: transport
        )
    }

    /// Dispatches a manual selection change event for the given node ID (§7.6).
    @discardableResult
    public func sendSelectionChanged(nodeId: NodeId, itemId: ItemId) async throws -> Event {
        guard let ownership = withStateLock({ () -> (
            EventOutboxConnectionBinding,
            EventOutboxSessionIncarnation
        )? in
            guard let binding = outboxConnectionBinding,
                  let sessionIncarnation = outboxSessionIncarnation else {
                return nil
            }
            return (binding, sessionIncarnation)
        }) else {
            throw SessionDispatchError.resumeNotConfirmed
        }
        let snapshot = applier.currentSnapshot
        if let interceptor = interactionWillEnterOutboxForTesting {
            await interceptor()
        }
        let drainCutoff = await flushPendingTextForManualNonTextInteraction()
        return try await sendSelectionChanged(
            nodeId: nodeId,
            observedRevision: snapshot.revision,
            itemId: itemId,
            binding: ownership.0,
            sessionIncarnation: ownership.1,
            maxFlushGeneration: drainCutoff
        )
    }

    @discardableResult
    private func sendSelectionChanged(
        nodeId: NodeId,
        observedRevision: Revision,
        itemId: ItemId,
        binding: EventOutboxConnectionBinding,
        sessionIncarnation: EventOutboxSessionIncarnation,
        maxFlushGeneration: UInt64
    ) async throws -> Event {
        let incarnation = await MainActor.run { self.interactionIncarnation }
        try await dispatchUnassignedTextEdits(
            binding: binding,
            sessionIncarnation: sessionIncarnation,
            interactionIncarnation: incarnation,
            maxFlushGeneration: maxFlushGeneration
        )
        guard withStateLock({
            guard eventDispatchEnabled,
                  outboxConnectionBinding == binding,
                  outboxSessionIncarnation == sessionIncarnation else {
                return false
            }
            if case .active = phase { return true }
            return false
        }) else {
            throw SessionDispatchError.resumeNotConfirmed
        }
        return try await outbox.sendSelectionChanged(
            nodeId: nodeId,
            observedRevision: observedRevision,
            itemId: itemId,
            binding: binding,
            sessionIncarnation: sessionIncarnation,
            via: transport
        )
    }

    /// Processes the incoming transport stream off the main actor (§22.2).
    private func runReceiveLoop() async {
        let stream = transport.receiveStream()

        do {
            for try await chunk in stream {
                guard !Task.isCancelled else { break }
                let messages: [SRUIMessage]
                do {
                    messages = try streamDecoder.appendAndExtract(incoming: chunk)
                } catch {
                    guard !Task.isCancelled else { return }
                    await reportFailure(.decodeFailed("frame decode failed: \(error)"))
                    return
                }

                for msg in messages {
                    guard !Task.isCancelled else { break }
                    await handleIncomingMessage(msg)
                }

                // Release half of inbound backpressure (§26): acknowledged only after the chunk
                // has been decoded *and* applied, so a slow renderer throttles the socket instead
                // of letting the transport buffer committed transactions without bound. Reporting
                // it earlier would make the bound meaningless, since rendering is the slow step.
                await transport.acknowledgeReceived(byteCount: chunk.count)
            }
        } catch {
            guard !Task.isCancelled else { return }
            await reportFailure(.transportEnded("\(error)"))
            return
        }

        // A peer that closes cleanly finishes the stream *without* throwing (socket EOF calls
        // `continuation.finish()`), so falling out of the loop here is the common disconnect, not a
        // normal shutdown. Unless `stop()` asked for the teardown, this is terminal for the replica
        // and must be reported so the caller reconnects and resumes rather than sitting on a live
        // session with no reader (§18, §4 inv. 13).
        let stoppedIntentionally = Task.isCancelled || withStateLock { isStopping || !isRunning }
        guard !stoppedIntentionally else { return }
        await reportFailure(.transportEnded("receive stream closed by peer"))
    }

    /// Processes a single wire envelope, dispatching on handshake phase (§12.1, §15, §22.2).
    public func handleIncomingMessage(_ message: SRUIMessage) async {
        guard let payload = message.msg else { return }

        let phase = withStateLock { self.phase }

        switch payload {
        case .serverWelcome(let welcome):
            switch phase {
            case .idle, .awaitingWelcome:
                await handleWelcome(welcome)
            case .awaitingResume, .active, .awaitingSnapshot, .failed:
                await reportFailure(.protocolViolation(
                    "Unexpected SERVER WELCOME during active session after handshake completed"
                ))
            }

        case .serverResumeOk(let resumeOk):
            switch phase {
            case .awaitingResume:
                await handleResumeOk(resumeOk)
            case .idle, .awaitingWelcome, .active, .awaitingSnapshot, .failed:
                await reportFailure(.protocolViolation(
                    "Unexpected SERVER RESUME_OK without an outstanding resume"
                ))
            }

        case .serverResyncRequired(let resync):
            switch phase {
            case .awaitingResume, .active, .awaitingSnapshot:
                await handleResyncRequired(resync, phase: phase)
            case .idle, .awaitingWelcome, .failed:
                await reportFailure(.protocolViolation(
                    "Received SERVER RESYNC_REQUIRED before handshake completed"
                ))
            }

        case .transaction(let wireTx):
            guard allowsDataPlane(phase) else {
                await reportFailure(.protocolViolation(
                    "Received Transaction before handshake completed"
                ))
                return
            }
            await handleTransaction(wireTx)

        case .event:
            guard allowsDataPlane(phase) else {
                await reportFailure(.protocolViolation(
                    "Received Event before handshake completed"
                ))
                return
            }

        case .serverEventAck(let ack):
            guard allowsDataPlane(phase) else {
                await reportFailure(.protocolViolation(
                    "Received ServerEventAck before handshake completed"
                ))
                return
            }
            await handleEventAck(ack)

        case .resourceMetadata(let metadata):
            guard allowsDataPlane(phase) else {
                await reportFailure(.protocolViolation(
                    "Received ResourceMetadata before handshake completed"
                ))
                return
            }
            await handleResourceMetadata(metadata)

        case .resourceChunk(let chunk):
            guard allowsDataPlane(phase) else {
                await reportFailure(.protocolViolation(
                    "Received ResourceChunk before handshake completed"
                ))
                return
            }
            await handleResourceChunk(chunk)

        case .clientModelRangeRequest:
            await reportFailure(.protocolViolation(
                "Received client-originated model range request from server"
            ))

        case .clientHello, .clientResume:
            await reportFailure(.protocolViolation(
                "Received client-originated handshake message from server"
            ))

        case .terminalInput, .terminalResize:
            await reportFailure(.protocolViolation(
                "Received client-originated terminal command from server"
            ))

        case .terminalData(let data):
            switch phase {
            case .active, .awaitingSnapshot, .awaitingResume:
                await handleTerminalData(data)
            case .idle, .awaitingWelcome, .failed:
                await reportFailure(.protocolViolation(
                    "Received TerminalData before handshake completed"
                ))
            }

        case .terminalResyncRequired(let resync):
            switch phase {
            case .active, .awaitingSnapshot, .awaitingResume:
                await handleTerminalResync(resync)
            case .idle, .awaitingWelcome, .failed:
                await reportFailure(.protocolViolation(
                    "Received TerminalResyncRequired before handshake completed"
                ))
            }
        }
    }

    private func allowsDataPlane(_ phase: ProtocolPhase) -> Bool {
        switch phase {
        case .active, .awaitingSnapshot:
            return true
        case .idle, .awaitingWelcome, .awaitingResume, .failed:
            return false
        }
    }

    private func handleWelcome(_ welcome: SRUIServerWelcome) async {
        // §15: `core_version` is part of the handshake, not decoration. Accepting an unknown core
        // version would let two peers that disagree about required semantics reach the data plane
        // (§4 inv. 13).
        guard sruiCoreVersionIsCompatible(welcome.coreVersion) else {
            await reportFailure(.protocolViolation(
                "SERVER WELCOME core_version \(welcome.coreVersion.isEmpty ? "<absent>" : welcome.coreVersion) "
                + "is not compatible with \(SRUICoreVersion)"
            ))
            return
        }

        let serverRequired: CapabilitySet
        do {
            serverRequired = try CapabilitySet.fromStrings(welcome.requiredProfiles)
        } catch {
            await reportFailure(.protocolViolation(
                "SERVER WELCOME required_profiles could not be parsed: \(error)"
            ))
            return
        }
        let serverOptional = CapabilitySet.fromValidStrings(welcome.optionalProfiles)

        let negotiated: CapabilitySet
        do {
            negotiated = try CapabilitySet.negotiate(
                clientOffered: clientCapabilities,
                serverRequired: serverRequired,
                serverOptional: serverOptional
            )
        } catch {
            await reportFailure(.protocolViolation(
                "Capability negotiation failed: \(error)"
            ))
            return
        }

        if !requiredServerProfiles.isEmpty && !negotiated.isSuperset(of: requiredServerProfiles) {
            let missing = requiredServerProfiles.subtracting(negotiated)
            await reportFailure(.protocolViolation(
                "Server does not satisfy client required profiles: \(missing)"
            ))
            return
        }

        guard let connectionBinding = await connectionBindingForWelcome() else {
            await reportFailure(.superseded(
                "SERVER WELCOME lost resource-cache ownership"
            ))
            return
        }
        let accepted = await outbox.confirmFreshSession(
            id: welcome.sessionID,
            binding: connectionBinding
        )
        guard accepted else {
            await reportFailure(.protocolViolation(
                "SERVER WELCOME cannot replace an outstanding resume decision"
            ))
            return
        }

        withStateLock {
            self.currentSessionId = welcome.sessionID
            self.requestedSessionId = nil
            self.resumeGeneration = nil
            self.retainedCapabilities = negotiated
            if welcome.initialRevision > 0 {
                self.pendingResync = true
                self.eventDispatchEnabled = false
                self.phase = .awaitingSnapshot(negotiated: negotiated)
            } else {
                self.phase = .active(negotiated: negotiated)
                self.eventDispatchEnabled = self.isRunning && !self._isDiverged
            }
        }
        do {
            try await applyExtensionNamespaces(
                welcome.extensionNamespaces,
                negotiated: negotiated,
                terminalRequired: serverRequired.contains(.terminalV1)
            )
        } catch {
            await reportFailure(.protocolViolation("\(error)"))
            return
        }
        if welcome.initialRevision > 0 {
            guard await outbox.suspendNewEvents(binding: connectionBinding) else {
                await reportFailure(.superseded(
                    "WELCOME snapshot catch-up lost its outbox connection binding"
                ))
                return
            }
        } else {
            await reissueCollectionRangeRequestsIfAllowed()
        }
        SessionDiagnostics.log(
            "Handshake completed successfully with session \(welcome.sessionID), negotiated: \(negotiated)"
        )
    }

    /// Supplies a binding for direct message-injection tests that bypass `start()`. Production
    /// connections always bind before sending their handshake.
    private func connectionBindingForWelcome() async -> EventOutboxConnectionBinding? {
        if let binding = withStateLock({ outboxConnectionBinding }) {
            guard await resourceCache.isReferenceOwnerActive(
                ownerEpoch: binding.resourceOwnershipEpoch
            ),
            let sessionIncarnation = await outbox.sessionIncarnation(binding: binding) else {
                return nil
            }
            let adopted = withStateLock { () -> Bool in
                guard outboxConnectionBinding == binding else { return false }
                outboxSessionIncarnation = sessionIncarnation
                return true
            }
            return adopted ? binding : nil
        }
        let binding = await outbox.beginConnectionBinding()
        let currentResourceReferences = await currentLiveResourceReferences()
        guard await resourceCache.activateReferenceOwner(
            epoch: binding.resourceOwnershipEpoch,
            liveReferences: currentResourceReferences
        ) else {
            return nil
        }
        guard let sessionIncarnation = await outbox.sessionIncarnation(binding: binding) else {
            return nil
        }
        withStateLock {
            outboxConnectionBinding = binding
            outboxSessionIncarnation = sessionIncarnation
        }
        return binding
    }

    private func handleResumeOk(_ resumeOk: SRUIServerResumeOk) async {
        let (
            requested,
            generation,
            connectionBinding,
            sessionIncarnation,
            lifecycleGeneration
        ) = withStateLock {
            (
                requestedSessionId,
                resumeGeneration,
                outboxConnectionBinding,
                outboxSessionIncarnation,
                self.lifecycleGeneration
            )
        }
        guard let requested, let generation, let connectionBinding,
              let sessionIncarnation else {
            await reportFailure(.protocolViolation(
                "Unexpected SERVER RESUME_OK without an outstanding resume"
            ))
            return
        }
        guard requested == resumeOk.sessionID else {
            await reportFailure(.protocolViolation(
                "SERVER RESUME_OK session_id \(resumeOk.sessionID) does not match requested \(requested)"
            ))
            return
        }

        do {
            guard let preparation = try await outbox.prepareSameSessionResume(
                id: resumeOk.sessionID,
                lastProcessedEventSeq: resumeOk.lastProcessedEventSeq,
                generation: generation,
                binding: connectionBinding
            ) else {
                await failRefusedResumeDecision(
                    generation,
                    "SERVER RESUME_OK answered a superseded resume attempt"
                )
                return
            }
            let adoptedAssignments = await MainActor.run { () -> Bool in
                guard self.withStateLock({
                    self.outboxConnectionBinding == preparation.binding
                        && self.outboxSessionIncarnation == preparation.sessionIncarnation
                        && self.resumeGeneration == preparation.generation
                }) else {
                    return false
                }
                self.reconcileFrontierSettledTextEdits(
                    preparation.frontierSettledTextEdits
                )
                self.noteAssignedTextEdits(preparation.assignedTextEdits)
                return true
            }
            guard adoptedAssignments else {
                await failRefusedResumeDecision(
                    generation,
                    "SERVER RESUME_OK lost native text assignment authority"
                )
                return
            }
            let accepted = try await outbox.completeSameSessionResume(
                preparation,
                via: transport,
                enableNewEventsAfterReplay: false,
                onReplayFailure: { [weak self] error in
                    await self?.handlePendingEventReplayFailure(
                        error,
                        ownership: PendingReplayFailureOwnership(
                            lifecycleGeneration: lifecycleGeneration,
                            binding: connectionBinding,
                            sessionIncarnation: sessionIncarnation,
                            resumeGeneration: generation
                        )
                    )
                }
            )
            guard accepted else {
                await failRefusedResumeDecision(
                    generation,
                    "SERVER RESUME_OK answered a superseded resume attempt"
                )
                return
            }
        } catch {
            await failReplayError(generation, error)
            return
        }
        guard await renderCommittedSnapshotBeforeResumeIfNeeded(
            generation: generation,
            binding: connectionBinding,
            sessionIncarnation: sessionIncarnation
        ) else {
            return
        }
        guard await outbox.finishResync(generation: generation) else {
            await failRefusedResumeDecision(
                generation,
                "SERVER RESUME_OK was superseded before renderer recovery completed"
            )
            return
        }
        await finalizeResumeAttempt(generation) {
            let negotiated = self.retainedCapabilities ?? self.clientCapabilities
            self.currentSessionId = resumeOk.sessionID
            self.requestedSessionId = nil
            self.resumeGeneration = nil
            self.retainedCapabilities = negotiated
            self.phase = .active(negotiated: negotiated)
            self.eventDispatchEnabled = !self._isDiverged
        }
        await reissueCollectionRangeRequestsIfAllowed()
        guard let resumedIncarnation = withStateLock({
            outboxSessionIncarnation
        }) else {
            await reportFailure(.superseded(
                "SERVER RESUME_OK lost its session-incarnation authority"
            ))
            return
        }
        await dispatchReadyTextEdits(
            binding: connectionBinding,
            sessionIncarnation: resumedIncarnation
        )
    }

    /// A committed resync snapshot can outlive the connection whose AppKit mount failed. A
    /// same-session RESUME_OK carries no replacement transaction, so remount that committed state
    /// before either controller or outbox event dispatch is reopened.
    private func renderCommittedSnapshotBeforeResumeIfNeeded(
        generation: UInt64,
        binding: EventOutboxConnectionBinding,
        sessionIncarnation: EventOutboxSessionIncarnation
    ) async -> Bool {
        let renderedRevision = withStateLock { lastRenderedRevision }
        let ownsMountedTree = await MainActor.run { self.hasMountedInitialTree }
        guard applier.lastAppliedRevision.value > renderedRevision || !ownsMountedTree else {
            return true
        }

        // Reserve both the reconnect generation and AppKit render lane before loading the shared
        // snapshot. A newer controller invalidates this lease before it can preload or remount.
        guard let renderToken = await outbox.beginResumeRecoveryRender(
            binding: binding,
            generation: generation
        ) else {
            await failRefusedResumeDecision(
                generation,
                "resume recovery render was superseded before snapshot preload"
            )
            return false
        }

        let snapshot = applier.currentSnapshot
        if let interceptor = resumeRecoverySnapshotLoadedInterceptorForTesting {
            await interceptor()
        }

        // A concurrently completed render may have caught this controller up while it acquired
        // ownership. Equal revision is still remounted when stop() retired native mount ownership.
        let stillOwnsMountedTree = await MainActor.run { self.hasMountedInitialTree }
        guard snapshot.revision.value > withStateLock({ lastRenderedRevision })
                || !stillOwnsMountedTree else {
            guard await outbox.completeResumeRecoveryRender(
                binding: binding,
                generation: generation,
                renderToken: renderToken
            ) else {
                await failRefusedResumeDecision(
                    generation,
                    "resume recovery render was superseded before no-op completion"
                )
                return false
            }
            return true
        }

        let rendererUpdate = await updateRenderer(
            transaction: nil,
            snapshot: snapshot,
            forceRemount: true,
            preserveLocalTextForRemount: true,
            binding: binding,
            renderToken: renderToken
        )
        guard rendererUpdate.didRender else {
            if rendererUpdate.wasSuperseded {
                await failRefusedResumeDecision(
                    generation,
                    "resume recovery render was superseded by a newer reconnect attempt"
                )
                return false
            }
            guard await outbox.abortResumeRecoveryRender(
                binding: binding,
                generation: generation,
                renderToken: renderToken
            ) else {
                await failRefusedResumeDecision(
                    generation,
                    "failed resume recovery render lost renderer ownership"
                )
                return false
            }
            await reportFailure(.rendererFailed(
                "resume recovery mount failed: \(rendererUpdate.failureDescription ?? "unknown renderer error")"
            ))
            return false
        }

        guard await outbox.completeResumeRecoveryRender(
            binding: binding,
            generation: generation,
            renderToken: renderToken
        ) else {
            await failRefusedResumeDecision(
                generation,
                "resume recovery render was superseded after mounting"
            )
            return false
        }
        withStateLock { lastRenderedRevision = snapshot.revision.value }
        guard await resolveRenderedTextAcknowledgements(
            through: snapshot.revision.value,
            binding: binding,
            sessionIncarnation: sessionIncarnation
        ) else {
            await failRefusedResumeDecision(
                generation,
                "resume recovery acknowledgement resolution was superseded"
            )
            return false
        }
        return true
    }

    private func handleResyncRequired(_ resync: SRUIServerResyncRequired, phase: ProtocolPhase) async {
        let negotiated: CapabilitySet
        switch phase {
        case .active(let caps), .awaitingSnapshot(let caps):
            negotiated = caps
        case .awaitingResume:
            negotiated = withStateLock { retainedCapabilities ?? clientCapabilities }
        case .idle, .awaitingWelcome, .failed:
            await reportFailure(.protocolViolation(
                "Received SERVER RESYNC_REQUIRED before handshake completed"
            ))
            return
        }

        let (
            requested,
            generation,
            connectionBinding,
            sessionIncarnation,
            lifecycleGeneration
        ) = withStateLock {
            (
                requestedSessionId,
                resumeGeneration,
                outboxConnectionBinding,
                outboxSessionIncarnation,
                self.lifecycleGeneration
            )
        }
        guard let connectionBinding, let sessionIncarnation else {
            await reportFailure(.protocolViolation(
                "SERVER RESYNC_REQUIRED arrived without an outbox connection binding"
            ))
            return
        }

        if let requested, let generation {
            switch resync.continuity {
            case .sameSession:
                guard resync.sessionID == requested else {
                    await reportFailure(.protocolViolation(
                        "same-session resync changed session_id from \(requested) to \(resync.sessionID)"
                    ))
                    return
                }
                do {
                    guard let preparation = try await outbox.prepareSameSessionResume(
                        id: resync.sessionID,
                        lastProcessedEventSeq: resync.lastProcessedEventSeq,
                        generation: generation,
                        binding: connectionBinding,
                        discardedTextEdits: resync.discardedTextEdits,
                        requireExactTextMatch: true,
                        onTextEditsCanceled: { [weak self] descriptors in
                            self?.noteCanceledTextEdits(descriptors)
                        }
                    ) else {
                        await failRefusedResumeDecision(
                            generation,
                            "same-session resync answered a superseded resume attempt"
                        )
                        return
                    }
                    let adoptedAssignments = await MainActor.run { () -> Bool in
                        guard self.withStateLock({
                            self.outboxConnectionBinding == preparation.binding
                                && self.outboxSessionIncarnation == preparation.sessionIncarnation
                                && self.resumeGeneration == preparation.generation
                        }) else {
                            return false
                        }
                        self.noteAssignedTextEdits(preparation.assignedTextEdits)
                        return true
                    }
                    guard adoptedAssignments else {
                        await failRefusedResumeDecision(
                            generation,
                            "same-session resync lost native text assignment authority"
                        )
                        return
                    }
                    let accepted = try await outbox.completeSameSessionResume(
                        preparation,
                        via: transport,
                        enableNewEventsAfterReplay: false,
                        onReplayFailure: { [weak self] error in
                            await self?.handlePendingEventReplayFailure(
                                error,
                                ownership: PendingReplayFailureOwnership(
                                    lifecycleGeneration: lifecycleGeneration,
                                    binding: connectionBinding,
                                    sessionIncarnation: sessionIncarnation,
                                    resumeGeneration: generation
                                )
                            )
                        }
                    )
                    guard accepted else {
                        await failRefusedResumeDecision(
                            generation,
                            "same-session resync answered a superseded resume attempt"
                        )
                        return
                    }
                } catch let error as EventOutboxError where error == .textEditDiscardMismatch {
                    await reportFailure(.protocolViolation(
                        "same-session resync discarded_text_edits did not match assigned TEXT_EDIT identities"
                    ))
                    return
                } catch {
                    await failReplayError(generation, error)
                    return
                }
                enterAwaitingSnapshot(sessionId: resync.sessionID, negotiated: negotiated)

            case .replaced:
                guard resync.sessionID != requested else {
                    await reportFailure(.protocolViolation(
                        "replacement resync reused expired session_id \(requested)"
                    ))
                    return
                }
                let accepted = await outbox.prepareReplacedSession(
                    id: resync.sessionID,
                    lastProcessedEventSeq: resync.lastProcessedEventSeq,
                    generation: generation,
                    binding: connectionBinding,
                    onTextEditingReset: { [weak self] sessionIncarnation in
                        self?.resetTextEditingForReplacement(
                            sessionIncarnation: sessionIncarnation
                        )
                    }
                )
                guard accepted else {
                    await failRefusedResumeDecision(
                        generation,
                        "replacement resync answered a superseded resume attempt"
                    )
                    return
                }
                // Replacement tears down in-flight resource assemblies; committed CAS is retained (§14, §18).
                if let renderer {
                    await renderer.terminalSession.resetForReplacementSession()
                    await MainActor.run { renderer.resetExtensionRegistry() }
                }
                await terminalPump.prune(retainedStreamIDs: [])
                guard await resourceCache.clearPartials(
                    ownerEpoch: connectionBinding.resourceOwnershipEpoch
                ) else {
                    await failRefusedResumeDecision(
                        generation,
                        "replacement resync lost resource-cache ownership"
                    )
                    return
                }
                enterAwaitingSnapshot(sessionId: resync.sessionID, negotiated: negotiated)

            case .unspecified:
                await reportFailure(.protocolViolation(
                    "SERVER RESYNC_REQUIRED received with unspecified continuity"
                ))
                return
            case .UNRECOGNIZED:
                await reportFailure(.protocolViolation(
                    "SERVER RESYNC_REQUIRED omitted a recognized session continuity"
                ))
                return
            }
        } else {
            switch resync.continuity {
            case .sameSession:
                let currentId = withStateLock { currentSessionId }
                guard resync.sessionID == currentId else {
                    await reportFailure(.protocolViolation(
                        "same-session resync changed session_id from \(currentId ?? "<none>") to \(resync.sessionID)"
                    ))
                    return
                }
                do {
                    switch try await outbox.applyLiveSameSessionResync(
                        lastProcessedEventSeq: resync.lastProcessedEventSeq,
                        binding: connectionBinding,
                        onTextEditsCanceled: { [weak self] descriptors in
                            self?.noteCanceledTextEdits(descriptors)
                        }
                    ) {
                    case .applied:
                        enterAwaitingSnapshot(sessionId: resync.sessionID, negotiated: negotiated)

                    case .resumeRequired:
                        await reportFailure(.transportEnded(
                            "live same-session resync requires reconnect to settle pending TEXT_EDIT identities"
                        ))
                        return

                    case .superseded:
                        await reportFailure(.superseded(
                            "live same-session resync lost its outbox connection binding"
                        ))
                        return
                    }
                } catch {
                    await reportFailure(.protocolViolation(
                        "same-session live resync text-edit cancellation failed: \(error)"
                    ))
                    return
                }

            case .replaced:
                let currentId = withStateLock { currentSessionId }
                guard resync.sessionID != currentId else {
                    await reportFailure(.protocolViolation(
                        "replacement resync reused expired session_id \(currentId ?? "<none>")"
                    ))
                    return
                }
                guard await outbox.applyReplacementFrontier(
                    id: resync.sessionID,
                    lastProcessedEventSeq: resync.lastProcessedEventSeq,
                    binding: connectionBinding,
                    onTextEditingReset: { [weak self] sessionIncarnation in
                        self?.resetTextEditingForReplacement(
                            sessionIncarnation: sessionIncarnation
                        )
                    }
                ) else {
                    await reportFailure(.superseded(
                        "live replacement resync lost its outbox connection binding"
                    ))
                    return
                }
                if let renderer {
                    await renderer.terminalSession.resetForReplacementSession()
                    await MainActor.run { renderer.resetExtensionRegistry() }
                }
                guard await resourceCache.clearPartials(
                    ownerEpoch: connectionBinding.resourceOwnershipEpoch
                ) else {
                    await reportFailure(.superseded(
                        "live replacement resync lost resource-cache ownership"
                    ))
                    return
                }
                enterAwaitingSnapshot(sessionId: resync.sessionID, negotiated: negotiated)

            case .unspecified:
                await reportFailure(.protocolViolation(
                    "SERVER RESYNC_REQUIRED received with unspecified continuity"
                ))
                return
            case .UNRECOGNIZED:
                await reportFailure(.protocolViolation(
                    "SERVER RESYNC_REQUIRED omitted a recognized session continuity"
                ))
                return
            }
        }

        SessionDiagnostics.log(
            "Server resync required at revision \(resync.snapshotRevision): \(resync.reason)"
        )
    }

    private func enterAwaitingSnapshot(sessionId: String, negotiated: CapabilitySet) {
        withStateLock {
            self.currentSessionId = sessionId
            self.requestedSessionId = nil
            self.pendingResync = true
            self.eventDispatchEnabled = false
            self.retainedCapabilities = negotiated
            self.phase = .awaitingSnapshot(negotiated: negotiated)
        }
    }

    private func applyExtensionNamespaces(
        _ mappings: [Srui_Protocol_ExtensionNamespaceMapping],
        negotiated: CapabilitySet,
        terminalRequired: Bool
    ) async throws {
        var seenIDs = Set<UInt32>()
        var seenURIs = Set<String>()
        for mapping in mappings {
            if !seenIDs.insert(mapping.namespaceID).inserted {
                throw SessionFailure.protocolViolation(
                    "duplicate extension namespace_id \(mapping.namespaceID)"
                )
            }
            if !seenURIs.insert(mapping.extensionUri).inserted {
                throw SessionFailure.protocolViolation(
                    "duplicate extension_uri \(mapping.extensionUri)"
                )
            }
        }
        let mapping = mappings.first(where: { $0.extensionUri == terminalProfileURI })
        if terminalRequired && mapping == nil {
            throw SessionFailure.protocolViolation(
                "required \(terminalProfileURI) but ServerWelcome omitted its namespace mapping"
            )
        }
        guard negotiated.contains(.terminalV1), let mapping else { return }
        guard mapping.namespaceID != 0 else {
            throw SessionFailure.protocolViolation(
                "\(terminalProfileURI) must use a nonzero session-assigned namespace"
            )
        }
        if let renderer {
            try await MainActor.run {
                try renderer.registerTerminalType(terminalTypeRef(namespaceID: mapping.namespaceID))
            }
        }
    }

    @discardableResult
    private func registerTerminalTypes(from store: SemanticStore) async -> Set<NodeId> {
        let negotiated = withStateLock { () -> CapabilitySet? in
            switch phase {
            case .active(let caps), .awaitingSnapshot(let caps):
                return caps
            default:
                return retainedCapabilities
            }
        }
        guard negotiated?.contains(.terminalV1) == true, let renderer else { return [] }
        var types = Set<TypeRef>()
        var terminalNodeIDs = Set<NodeId>()
        var pending = store.rootIDs
        var seen = Set<NodeId>()
        while let id = pending.popLast() {
            guard seen.insert(id).inserted, let node = store.getNode(id) else { continue }
            if !node.nodeType.isStandard, node.nodeType.localID == terminalLocalTypeID {
                types.insert(node.nodeType)
                terminalNodeIDs.insert(id)
            }
            pending.append(contentsOf: node.orderedChildren)
        }
        await MainActor.run {
            for typeRef in types {
                try? renderer.registerTerminalType(typeRef)
            }
        }
        return terminalNodeIDs
    }

    /// Local terminal apply only. Never touches semantic phase, revision, outbox, or text drafts.
    private func handleTerminalData(_ data: SRUITerminalData) async {
        guard let renderer else { return }
        do {
            _ = try await renderer.terminalSession.applyData(
                streamID: NodeId(data.streamID),
                byteOffset: data.byteOffset,
                data: data.data
            )
        } catch {
            SessionDiagnostics.error("TerminalData rejected: \(error)")
        }
    }

    /// Island resync. Must not enter `ServerResyncRequired` handling.
    private func handleTerminalResync(_ resync: SRUITerminalResyncRequired) async {
        guard let renderer else { return }
        let cause: TerminalResyncCause
        switch resync.reason {
        case .retentionLoss: cause = .retentionLoss
        case .offsetAhead: cause = .offsetAhead
        case .subscriberFallbehind: cause = .subscriberFallbehind
        case .unspecified: cause = .unspecified
        case .UNRECOGNIZED: cause = .unspecified
        }
        _ = await renderer.terminalSession.applyResync(
            streamID: NodeId(resync.streamID),
            requestedOffset: resync.requestedOffset,
            retainedFromOffset: resync.retainedFromOffset,
            resumeAtOffset: resync.resumeAtOffset,
            cause: cause
        )
    }

    /// Assembles resource metadata into the shared cache. Failures log/drop; the semantic store
    /// is never corrupted by a bad resource (§14, §26).
    private func handleResourceMetadata(_ wire: SRUIResourceMetadata) async {
        let input: ResourceMetadataInput
        do {
            input = try Self.mapResourceMetadata(wire)
        } catch {
            SessionDiagnostics.error("Ignoring malformed ResourceMetadata: \(error)")
            return
        }
        guard let binding = withStateLock({ outboxConnectionBinding }) else { return }
        guard await syncLiveResourceReferences(binding: binding) else { return }

        do {
            if let commit = try await resourceCache.ingestMetadata(
                input,
                ownerEpoch: binding.resourceOwnershipEpoch
            ) {
                if let interceptor = resourceCommitReadyInterceptorForTesting {
                    await interceptor()
                }
                await dispatchResourceCommit(commit, binding: binding)
            }
        } catch {
            rejectedResourceHashes.insert(input.resourceHash)
            SessionDiagnostics.error(
                "Resource metadata rejected for \(input.resourceHash): \(error); keeping placeholder (§14)"
            )
        }
    }

    /// Assembles one resource chunk. Bad resources log/drop and leave placeholders in place (§14).
    private func handleResourceChunk(_ wire: SRUIResourceChunk) async {
        let input: ResourceChunkInput
        do {
            input = try Self.mapResourceChunk(wire)
        } catch {
            SessionDiagnostics.error("Ignoring malformed ResourceChunk: \(error)")
            return
        }

        if rejectedResourceHashes.contains(input.resourceHash) {
            // Already rejected (e.g. oversized metadata); do not flood diagnostics per chunk.
            return
        }
        guard let binding = withStateLock({ outboxConnectionBinding }) else { return }
        guard await syncLiveResourceReferences(binding: binding) else { return }
        if let interceptor = resourceReferencesSynchronizedInterceptorForTesting {
            await interceptor()
        }
        do {
            if let commit = try await resourceCache.ingestChunk(
                input,
                ownerEpoch: binding.resourceOwnershipEpoch
            ) {
                if let interceptor = resourceCommitReadyInterceptorForTesting {
                    await interceptor()
                }
                await dispatchResourceCommit(commit, binding: binding)
            }
        } catch {
            rejectedResourceHashes.insert(input.resourceHash)
            SessionDiagnostics.error(
                "Resource chunk rejected for \(input.resourceHash): \(error); keeping placeholder (§14)"
            )
        }
    }

    /// Pins hashes currently shown by Image nodes or referenced by the replica store so
    /// committed-CAS eviction cannot drop still-needed content (§26).
    private func currentLiveResourceReferences() async -> Set<ResourceHash> {
        await MainActor.run {
            var hashes = self.renderer?.liveResourceHashes() ?? []
            hashes.formUnion(self.applier.currentSnapshot.store.referencedResourceHashes())
            return hashes
        }
    }

    private func syncLiveResourceReferences(
        binding: EventOutboxConnectionBinding
    ) async -> Bool {
        guard withStateLock({ outboxConnectionBinding == binding }) else { return false }
        let live = await currentLiveResourceReferences()
        return await resourceCache.setLiveReferences(
            live,
            ownerEpoch: binding.resourceOwnershipEpoch
        )
    }

    /// Pushes a newly committed decoded image onto AppKit on the main actor (§14, §22.2).
    ///
    /// Reconfirmed commits (`newlyCommitted == false`) still hydrate a replacement renderer that
    /// shares the cache but does not yet hold the `NSImage`.
    private func dispatchResourceCommit(
        _ commit: ResourceCommit,
        binding: EventOutboxConnectionBinding
    ) async {
        let dispatched = await resourceCache.performIfReferenceOwnerActive(
            ownerEpoch: binding.resourceOwnershipEpoch
        ) { [weak self] in
            guard let self else { return }
            if !commit.evictedHashes.isEmpty {
                self.renderer?.evictResourceImages(commit.evictedHashes)
            }
            let alreadyInstalled = self.renderer?.resolveResourceImage(commit.image.hash) != nil
            if commit.newlyCommitted || !alreadyInstalled {
                self.renderer?.commitResourceImage(commit.image)
            }
        }
        guard dispatched else { return }
        rejectedResourceHashes.remove(commit.image.hash)
    }

    private static func mapResourceMetadata(_ wire: SRUIResourceMetadata) throws -> ResourceMetadataInput {
        let hash = try ResourceHash(bytes: wire.resourceHash)
        let priority: ResourceTransferPriority
        switch wire.priority {
        case .unspecified, .UNRECOGNIZED:
            priority = .unspecified
        case .normal:
            priority = .normal
        case .low:
            priority = .low
        }
        return ResourceMetadataInput(
            resourceHash: hash,
            mediaType: wire.mediaType,
            encodedLength: wire.encodedLength,
            decodedWidth: wire.decodedWidth,
            decodedHeight: wire.decodedHeight,
            priority: priority
        )
    }

    private static func mapResourceChunk(_ wire: SRUIResourceChunk) throws -> ResourceChunkInput {
        let hash = try ResourceHash(bytes: wire.resourceHash)
        return ResourceChunkInput(
            resourceHash: hash,
            byteOffset: wire.byteOffset,
            data: wire.data
        )
    }

    /// Settles one outbound event against the server's acknowledgement (§18, §18.2).
    ///
    /// Every status is terminal for that `event_id` — including `rejected`. An event the server
    /// refuses must be dropped here: leaving it pending would replay it on the next resume, which
    /// the server would refuse again, forever.
    private func handleEventAck(_ ack: SRUIServerEventAck) async {
        let eventId = EventId(ack.eventID)
        guard let ownership = withStateLock({ () -> (
            EventOutboxConnectionBinding,
            EventOutboxSessionIncarnation
        )? in
            guard let binding = outboxConnectionBinding,
                  let sessionIncarnation = outboxSessionIncarnation else {
                return nil
            }
            return (binding, sessionIncarnation)
        }) else {
            await reportFailure(.protocolViolation(
                "SERVER EVENT_ACK arrived without an outbox connection binding"
            ))
            return
        }
        let (connectionBinding, sessionIncarnation) = ownership
        // `session_id` is required on every ack (§18.2): it is the only proof of which incarnation
        // settled the event. Accepting an ack without it and logging would leave the event pending
        // forever — replayed on every retry, answered `duplicate`, never settled — until the
        // sequence window is exhausted, so a missing required semantic fails here (§4 inv. 13).
        guard !ack.sessionID.isEmpty else {
            await reportFailure(.protocolViolation(
                "SERVER EVENT_ACK for event \(eventId) omitted the required session_id"
            ))
            return
        }

        // The full wire identity is handed to the outbox so the identity check and the mutation it
        // guards share one actor-isolated step (§18.2).
        let settlement = await outbox.settleAcknowledgement(
            binding: connectionBinding,
            sessionIncarnation: sessionIncarnation,
            clientInstanceId: ClientInstanceId(ack.clientInstanceID),
            eventId: eventId,
            throughSeq: ack.lastProcessedEventSeq,
            sessionId: ack.sessionID,
            revisionAfterEffect: ack.revisionAfterEffect,
            textEditRejected: ack.status == .rejected
        )
        guard settlement.connectionBound else {
            await reportFailure(.superseded(
                "SERVER EVENT_ACK arrived after a newer connection binding"
            ))
            return
        }
        guard settlement.bound else {
            SessionDiagnostics.error(
                "Ignoring event acknowledgement with unbound identity (session \(ack.sessionID))"
            )
            return
        }

        switch ack.status {
        case .rejected:
            SessionDiagnostics.error(
                "Server rejected event \(eventId) at revision \(ack.revisionAfterEffect): \(ack.rejectReason)"
            )
        case .processed, .duplicate:
            SessionDiagnostics.log(
                "Event \(eventId) settled as \(ack.status) at revision \(ack.revisionAfterEffect)"
            )
        case .unspecified, .UNRECOGNIZED:
            // An ack is optional-to-consume: §4 inv. 13 requires unknown *required* semantics to
            // fail closed, and a settlement status this build does not know is not one. Settle by
            // id and keep going rather than stranding the event in the retry set forever.
            SessionDiagnostics.log(
                "Event \(eventId) settled with unrecognized ack status \(ack.status)"
            )
        }

        // Native assignment identity is cleared by `resolveRenderedTextAcknowledgements`,
        // after the named edit's authoritative revision has rendered. Clearing it here would let
        // an intervening structural transaction remount the stale store value over local text.

        // Control-lane acknowledgements can overtake multiple UI-lane transactions. The outbox
        // keeps each editor blocked until the exact-or-later authoritative revision has rendered,
        // so an unrelated intervening transaction cannot promote the successor (§22.6).
        let renderedRevision = withStateLock { lastRenderedRevision }
        guard await resolveRenderedTextAcknowledgements(
            through: renderedRevision,
            binding: connectionBinding,
            sessionIncarnation: sessionIncarnation
        ) else {
            await reportFailure(.superseded(
                "SERVER EVENT_ACK native resolution lost its connection binding"
            ))
            return
        }
        await dispatchReadyTextEdits(
            binding: connectionBinding,
            sessionIncarnation: sessionIncarnation
        )
    }

    /// Resolves native assignment identity and any rejection revert in the same MainActor
    /// lifecycle fence that owns the outbox barriers. A replacement binding can therefore win
    /// before the callback or after the complete native transition, never between its two halves.
    private func resolveRenderedTextAcknowledgements(
        through revision: UInt64,
        binding: EventOutboxConnectionBinding,
        sessionIncarnation: EventOutboxSessionIncarnation
    ) async -> Bool {
        return await outbox.releaseTextAcknowledgements(
            through: revision,
            binding: binding,
            sessionIncarnation: sessionIncarnation,
            onResolved: { [weak self] acknowledgements in
                guard let self, let renderer = self.renderer else { return }
                let session = renderer.textEditingSession
                for acknowledgement in acknowledgements {
                    session.noteAcknowledged(
                        nodeID: acknowledgement.nodeId,
                        eventId: acknowledgement.eventId
                    )
                    guard acknowledgement.rejected,
                          !session.hasUnsentSuccessorDraft(for: acknowledgement.nodeId) else {
                        continue
                    }
                    // Editors mounted without `.value`/`.text` never published a string; the
                    // store baseline is the empty default, not "unknown".
                    let published = session.lastKnownAuthoritative(
                        for: acknowledgement.nodeId
                    ) ?? ""
                    if let adapter = renderer.registry.handle(
                        for: acknowledgement.nodeId
                    )?.textAdapter {
                        adapter.applyAuthoritativeString(published)
                    } else {
                        _ = session.applyPublishedValue(
                            nodeID: acknowledgement.nodeId,
                            published: published
                        )
                    }
                }
            }
        )
    }

    @discardableResult
    private func applyResyncTextCancellation(
        _ resync: SRUIServerResyncRequired,
        requireExactMatch: Bool,
        onlyIfResumeGeneration generation: UInt64?
    ) async throws -> Bool {
        try await outbox.cancelAssignedTextEdits(
            confirming: resync.discardedTextEdits,
            requireExactMatch: requireExactMatch,
            onlyIfResumeGeneration: generation
        )
    }

    @MainActor
    private func reconcileFrontierSettledTextEdits(_ events: [Event]) {
        guard let renderer else { return }
        let session = renderer.textEditingSession
        for event in events where event.eventType == .EVENT_TEXT_EDIT {
            session.noteAcknowledged(nodeID: event.nodeId, eventId: event.eventId)
            guard !session.hasUnsentSuccessorDraft(for: event.nodeId) else {
                continue
            }
            // A cumulative frontier proves settlement but carries no individual rejection bit.
            // Reapply the last published value conservatively; an accepted edit's transaction
            // will publish its value, while a rejected edit cannot remain visible.
            let published = session.lastKnownAuthoritative(for: event.nodeId) ?? ""
            if let adapter = renderer.registry.handle(for: event.nodeId)?.textAdapter {
                adapter.applyAuthoritativeString(published)
            } else {
                _ = session.applyPublishedValue(nodeID: event.nodeId, published: published)
            }
        }
    }

    @MainActor
    private func noteCanceledTextEdits(_ descriptors: [PendingTextEditDescriptor]) {
        for descriptor in descriptors {
            renderer?.textEditingSession.noteCanceled(
                nodeID: descriptor.nodeId,
                eventId: descriptor.eventId
            )
        }
    }

    @MainActor
    private func resetTextEditingForReplacement(
        sessionIncarnation: EventOutboxSessionIncarnation
    ) {
        withStateLock {
            outboxSessionIncarnation = sessionIncarnation
        }
        advanceInteractionIncarnation()
        renderer?.textEditingSession.resetForReplacementSession()
    }

    @MainActor
    private func adoptFullResyncInteractionBoundary(
        sessionIncarnation: EventOutboxSessionIncarnation
    ) {
        withStateLock {
            outboxSessionIncarnation = sessionIncarnation
        }
        advanceInteractionIncarnation()
    }

    @MainActor
    private func noteAssignedTextEdits(_ events: [Event]) {
        for event in events {
            renderer?.textEditingSession.noteAssigned(event)
        }
    }

    /// Schedules a drain behind native interaction callbacks without blocking the receive loop.
    ///
    /// Acknowledgements can release an existing drain while this method runs. Appending to the
    /// same tail prevents two drain loops from claiming one MainActor-owned draft concurrently,
    /// and lets the receive loop continue delivering the transaction named by an effect barrier.
    private func dispatchReadyTextEdits(
        binding: EventOutboxConnectionBinding,
        sessionIncarnation: EventOutboxSessionIncarnation
    ) async {
        await enqueueReadyTextEditDispatch(
            binding: binding,
            sessionIncarnation: sessionIncarnation
        )
    }

    @MainActor
    private func enqueueReadyTextEditDispatch(
        binding: EventOutboxConnectionBinding,
        sessionIncarnation: EventOutboxSessionIncarnation
    ) {
        guard withStateLock({
            outboxConnectionBinding == binding
                && outboxSessionIncarnation == sessionIncarnation
        }) else {
            return
        }

        let incarnation = interactionIncarnation
        let drainCutoff = renderer?.textEditingSession.currentFlushGeneration ?? 0
        let predecessor = interactionDispatchTail
        let dispatch = Task { [weak self] in
            _ = await predecessor?.result
            guard let self,
                  !Task.isCancelled,
                  self.interactionIncarnation == incarnation else {
                return
            }
            do {
                try await self.dispatchUnassignedTextEdits(
                    binding: binding,
                    sessionIncarnation: sessionIncarnation,
                    interactionIncarnation: incarnation,
                    maxFlushGeneration: drainCutoff
                )
            } catch {
                SessionDiagnostics.error("Failed to dispatch local text edits: \(error)")
            }
        }
        interactionDispatchTail = dispatch
    }

    private func handleTransaction(_ wireTx: SRUITransaction) async {
        // A diverged replica cannot meaningfully apply anything until it resumes (§18).
        guard !isDiverged else { return }
        guard let ownership = withStateLock({ () -> (
            EventOutboxConnectionBinding,
            EventOutboxSessionIncarnation
        )? in
            guard let binding = outboxConnectionBinding,
                  let sessionIncarnation = outboxSessionIncarnation else {
                return nil
            }
            return (binding, sessionIncarnation)
        }) else {
            await reportFailure(.protocolViolation(
                "Transaction arrived without outbox session ownership"
            ))
            return
        }
        let (connectionBinding, deliveredIncarnation) = ownership

        let domainTx: Transaction
        do {
            domainTx = try decoder.validateAndConvertTransaction(wire: wireTx)
        } catch {
            await reportFailure(.decodeFailed("transaction decode failed: \(error)"))
            return
        }

        // §18: a snapshot replaces authoritative state only when `pendingResync` is set —
        // `SERVER RESYNC_REQUIRED` or a HELLO catch-up after `WELCOME.initial_revision > 0`.
        // `base_revision == 0` is not by itself evidence of a snapshot: a stale copy of the
        // initial transaction carries it too, and treating that as a snapshot would wipe a live
        // replica (§12.1).
        let (isResyncSnapshot, outstandingGeneration) = withStateLock {
            (pendingResync, resumeGeneration)
        }

        // Apply and capture the committed snapshot in the outbox's connection-ownership section so
        // an older transport cannot mutate the shared replica after a replacement binds (§22.2).
        let applyResult: Result<TransactionSnapshot, TxnError>
        let renderToken: UUID?
        let renderSessionIncarnation: EventOutboxSessionIncarnation
        if isResyncSnapshot {
            // A snapshot replaces the entire replica, so the supersession check and publish run
            // inside one outbox critical section. The latch remains closed until the snapshot has
            // also mounted and the text boundary has discarded every pre-snapshot draft (§18.3).
            // Rebuilding happens before that section so it never occupies the outbox actor while
            // acknowledgements wait (§22.2).
            let prepared: PreparedResyncSnapshot
            switch applier.prepareResyncSnapshot(record: domainTx) {
            case .success(let snapshot):
                prepared = snapshot
            case .failure(let error):
                await handleTransactionRejection(error, isResyncSnapshot: true)
                return
            }
            guard let published = await outbox.commitResyncSnapshot(
                generation: outstandingGeneration,
                binding: connectionBinding,
                sessionIncarnation: deliveredIncarnation,
                onSessionIncarnationAdvanced: { [weak self] sessionIncarnation in
                    self?.adoptFullResyncInteractionBoundary(
                        sessionIncarnation: sessionIncarnation
                    )
                },
                publish: { self.applier.publishResyncSnapshot(prepared) },
                committed: { result in
                    guard case .success = result else { return false }
                    return true
                }
            ) else {
                let context = "resync snapshot arrived after a newer reconnect attempt"
                if let outstandingGeneration {
                    await failRefusedResumeDecision(outstandingGeneration, context)
                } else {
                    await reportFailure(.superseded(context))
                }
                return
            }
            guard let snapshotIncarnation = published.sessionIncarnation else {
                await reportFailure(.protocolViolation(
                    "committed resync snapshot did not advance interaction ownership"
                ))
                return
            }
            applyResult = published.result
            renderToken = published.renderToken
            renderSessionIncarnation = snapshotIncarnation
        } else {
            // Live deliveries use the same atomic bind check and reserve a MainActor render fence.
            // Rebinding either happens after this render or invalidates it before AppKit mutation.
            guard let published = await outbox.commitLiveRender(
                binding: connectionBinding,
                sessionIncarnation: deliveredIncarnation,
                publish: { self.applier.applyDelivered(record: domainTx) },
                committed: { result in
                    guard case .success = result else { return false }
                    return true
                }
            ) else {
                await reportFailure(.superseded(
                    "live transaction arrived after a newer connection binding"
                ))
                return
            }
            applyResult = published.result
            renderToken = published.renderToken
            renderSessionIncarnation = deliveredIncarnation
        }

        switch applyResult {
        case .success(let snapshot):
            guard let renderToken else {
                await reportFailure(.protocolViolation(
                    "committed transaction did not produce a renderer ownership token"
                ))
                return
            }

            if !isResyncSnapshot,
               let interceptor = liveTransactionPublishedInterceptorForTesting {
                await interceptor()
            }
            let terminalNodeIDs = await registerTerminalTypes(from: snapshot.store)
            await terminalPump.prune(retainedStreamIDs: terminalNodeIDs)
            let rendererUpdate = await updateRenderer(
                transaction: isResyncSnapshot ? nil : domainTx,
                snapshot: snapshot,
                forceRemount: isResyncSnapshot,
                discardTextEditsForResync: isResyncSnapshot,
                binding: connectionBinding,
                renderToken: renderToken
            )

            if !isResyncSnapshot {
                guard await outbox.completeLiveRender(
                    binding: connectionBinding,
                    sessionIncarnation: renderSessionIncarnation,
                    renderToken: renderToken
                ) else {
                    await reportFailure(.superseded(
                        "live transaction render was superseded by a newer connection binding"
                    ))
                    return
                }
            }

            guard rendererUpdate.didRender else {
                if rendererUpdate.wasSuperseded {
                    let context = "resync snapshot render was superseded by a newer reconnect attempt"
                    if let outstandingGeneration {
                        await failRefusedResumeDecision(outstandingGeneration, context)
                    } else {
                        await reportFailure(.superseded(context))
                    }
                } else if isResyncSnapshot {
                    guard await outbox.abortResyncRender(
                        generation: outstandingGeneration,
                        binding: connectionBinding,
                        renderToken: renderToken
                    ) else {
                        let context = "failed resync snapshot render lost renderer ownership"
                        if let outstandingGeneration {
                            await failRefusedResumeDecision(outstandingGeneration, context)
                        } else {
                            await reportFailure(.superseded(context))
                        }
                        return
                    }
                    withStateLock { self.pendingResync = false }
                    await reportFailure(.rendererFailed(
                        "resync snapshot update failed: \(rendererUpdate.failureDescription ?? "unknown renderer error")"
                    ))
                }
                return
            }

            if isResyncSnapshot {
                // This actor hop occurs after the native remount. Old callbacks carry a lower
                // epoch and are discarded; genuinely post-mount edits carry the new epoch and
                // remain queued while dispatch is still closed.
                guard await outbox.applyFullResyncTextBoundary(
                    generation: outstandingGeneration,
                    binding: connectionBinding,
                    renderToken: renderToken
                ) else {
                    let context = "resync snapshot boundary was superseded after rendering"
                    if let outstandingGeneration {
                        await failRefusedResumeDecision(outstandingGeneration, context)
                    } else {
                        await reportFailure(.superseded(context))
                    }
                    return
                }
            }

            withStateLock { lastRenderedRevision = snapshot.revision.value }
            guard await resolveRenderedTextAcknowledgements(
                through: snapshot.revision.value,
                binding: connectionBinding,
                sessionIncarnation: renderSessionIncarnation
            ) else {
                let context = "transaction acknowledgement resolution lost its connection binding"
                if let outstandingGeneration {
                    await failRefusedResumeDecision(outstandingGeneration, context)
                } else {
                    await reportFailure(.superseded(context))
                }
                return
            }


            if isResyncSnapshot {
                // Only now may the outbox release the reconnect latch and promote post-snapshot
                // intent. No unresolved pre-snapshot edit is silently merged (§18.3).
                await completeSnapshotCatchUp()
            } else {
                await dispatchReadyTextEdits(
                    binding: connectionBinding,
                    sessionIncarnation: renderSessionIncarnation
                )
            }

        case .failure(let err):
            await handleTransactionRejection(err, isResyncSnapshot: isResyncSnapshot)
        }
    }

    private func handlePendingEventReplayFailure(
        _ error: String,
        ownership: PendingReplayFailureOwnership
    ) async {
        if let interceptor = pendingReplayFailureWillReportForTesting {
            await interceptor()
        }
        let failure = SessionFailure.transportEnded(
            "pending event replay retry failed: \(error)"
        )
        guard let authorized = await outbox.withActiveSessionIncarnation(
            binding: ownership.binding,
            sessionIncarnation: ownership.sessionIncarnation,
            { [weak self] in
                self?.markFailure(requiring: ownership)
            }
        ), let failureState = authorized else {
            return
        }
        await finishFailureReport(failure, state: failureState)
    }

    /// Commits controller state after resume replay and abandons background retries when teardown raced completion.
    private func finalizeResumeAttempt(_ generation: UInt64, mutate: () -> Void) async {
        let didCommitControllerState = withStateLock { () -> Bool in
            guard isRunning, !_isDiverged else { return false }
            mutate()
            activeReplayRetryGeneration = generation
            return true
        }
        if didCommitControllerState {
            await outbox.commitResumeWork(generation: generation)
        } else {
            await outbox.stopResumeWork(generation: generation)
        }
    }

    /// Reports the terminal failure for a resume decision the outbox refused (§18).
    ///
    /// A generation the outbox never issued cannot be explained by a lost race: it means this
    /// controller is bound to a different outbox than the one that minted it, which is a
    /// required-semantics failure rather than a benign supersession (§4 inv. 13).
    private func failRefusedResumeDecision(_ generation: UInt64, _ context: String) async {
        if await outbox.hasIssuedResumeGeneration(generation) {
            await reportFailure(.superseded(context))
        } else {
            await reportFailure(.protocolViolation(
                "\(context): resume generation \(generation) was never issued by this outbox"
            ))
        }
    }

    /// Classifies a throw out of pending-event replay (§18).
    ///
    /// A newer `beginResumeAttempt()` cancels the write chain of the attempt it supersedes, so a
    /// superseded replay throws `CancellationError` from mid-loop instead of returning `false`
    /// from the decision guard. Reporting that as a transport failure would tell the owner to
    /// reconnect a controller that merely lost the race, so classify by latch ownership rather
    /// than by the error that surfaced.
    private func failReplayError(_ generation: UInt64, _ error: any Error) async {
        guard await outbox.isActiveResumeGeneration(generation) else {
            await failRefusedResumeDecision(
                generation,
                "pending event replay was cancelled by a newer reconnect attempt"
            )
            return
        }
        await reportFailure(.transportEnded("pending event replay failed: \(error)"))
    }

    /// Releases the snapshot latch only after authoritative state has rendered and the full-resync
    /// text boundary has reached the outbox.
    private func completeSnapshotCatchUp() async {
        let (generation, connectionBinding, sessionIncarnation) = withStateLock {
            (
                self.resumeGeneration,
                self.outboxConnectionBinding,
                self.outboxSessionIncarnation
            )
        }
        guard let connectionBinding, let sessionIncarnation else {
            await reportFailure(.protocolViolation(
                "resync snapshot finalization lost its outbox connection binding"
            ))
            return
        }
        let released: Bool
        if let generation {
            released = await outbox.finishResync(generation: generation)
        } else {
            released = await outbox.allowNewEvents(binding: connectionBinding)
        }

        guard released else {
            let context = "resync snapshot was superseded before renderer finalization"
            if let generation {
                await failRefusedResumeDecision(generation, context)
            } else {
                await reportFailure(.superseded(context))
            }
            return
        }

        withStateLock { self.pendingResync = false }
        if let generation {
            await finalizeResumeAttempt(generation) {
                self.resumeGeneration = nil
                self.eventDispatchEnabled = true
                if case .awaitingSnapshot(let negotiated) = self.phase {
                    self.phase = .active(negotiated: negotiated)
                }
            }
        } else {
            withStateLock {
                self.eventDispatchEnabled = self.isRunning && !self._isDiverged
                if case .awaitingSnapshot(let negotiated) = self.phase {
                    self.phase = .active(negotiated: negotiated)
                }
            }
        }
        await reissueCollectionRangeRequestsIfAllowed()
        await dispatchReadyTextEdits(
            binding: connectionBinding,
            sessionIncarnation: sessionIncarnation
        )
    }

    /// Classifies a rejected transaction as a benign duplicate or as replica divergence (§12.1, §18).
    private func handleTransactionRejection(_ error: TxnError, isResyncSnapshot: Bool) async {
        // A rejected snapshot is recoverable: the resync latch is still set, so the server can
        // simply send another snapshot. Tearing the session down here would throw away the one
        // channel on which recovery can still arrive (§18).
        if isResyncSnapshot {
            SessionDiagnostics.error(
                "Resync snapshot rejected (\(error)); still awaiting a usable snapshot"
            )
            return
        }

        // A transaction whose base is *older* than our committed revision is a re-delivery of work
        // we already have (§18.1 journal replay overlap). Discarding it is correct and the stream
        // stays healthy.
        if case .staleBaseRevision(let expected, let actual) = error, actual < expected {
            SessionDiagnostics.log(
                "Ignoring already-applied transaction base=\(actual) committed=\(expected)"
            )
            return
        }

        await reportFailure(.replicaDiverged(error))
    }

    private func markFailure(
        requiring ownership: PendingReplayFailureOwnership? = nil
    ) -> SessionFailureTeardownState? {
        withStateLock {
            if let ownership {
                guard lifecycleGeneration == ownership.lifecycleGeneration,
                      isRunning,
                      !isStopping,
                      outboxConnectionBinding == ownership.binding,
                      outboxSessionIncarnation == ownership.sessionIncarnation,
                      resumeGeneration == ownership.resumeGeneration
                        || activeReplayRetryGeneration == ownership.resumeGeneration else {
                    return nil
                }
            }
            guard !_isDiverged else { return nil }
            _isDiverged = true
            eventDispatchEnabled = false
            phase = .failed
            let replayGeneration = resumeGeneration ?? activeReplayRetryGeneration
            activeReplayRetryGeneration = nil
            return SessionFailureTeardownState(
                handler: _onFailure ?? { _ in },
                replayGeneration: replayGeneration,
                connectionBinding: outboxConnectionBinding,
                lifecycleGeneration: lifecycleGeneration
            )
        }
    }

    private func reportFailure(_ failure: SessionFailure) async {
        guard let state = markFailure() else { return }
        await finishFailureReport(failure, state: state)
    }

    private func finishFailureReport(
        _ failure: SessionFailure,
        state: SessionFailureTeardownState
    ) async {
        await retainNativeTextBeforeDisconnect(
            binding: state.connectionBinding
        )
        guard ownsRunningLifecycle(state.lifecycleGeneration) else { return }
        if let replayGeneration = state.replayGeneration {
            await outbox.stopResumeWork(generation: replayGeneration)
        }
        guard ownsRunningLifecycle(state.lifecycleGeneration) else { return }
        await MainActor.run {
            guard self.ownsRunningLifecycle(state.lifecycleGeneration) else {
                return
            }
            self.interactionDispatchTail?.cancel()
            self.interactionDispatchTail = nil
        }
        guard ownsRunningLifecycle(state.lifecycleGeneration) else { return }
        SessionDiagnostics.error("Session failed: \(failure). Reconnect and resume to recover (§18).")
        state.handler(failure)
        guard ownsRunningLifecycle(state.lifecycleGeneration) else { return }
        await transport.close()
    }
    /// Dispatches committed store state to AppKit on the main actor (§22.2).
    private func updateRenderer(
        transaction: Transaction?,
        snapshot: TransactionSnapshot,
        forceRemount: Bool,
        discardTextEditsForResync: Bool = false,
        preserveLocalTextForRemount: Bool = false,
        binding: EventOutboxConnectionBinding,
        renderToken: UUID
    ) async -> RendererUpdateResult {
        guard let cachedResources = await resourceCache.beginRenderReferenceLease(
            ownerEpoch: binding.resourceOwnershipEpoch,
            renderToken: renderToken,
            hashes: snapshot.store.referencedResourceHashes()
        ) else {
            return RendererUpdateResult(
                didRender: false,
                resyncLaneEpoch: nil,
                wasSuperseded: true
            )
        }
        if let interceptor = rendererResourcesPreloadedInterceptorForTesting {
            await interceptor()
        }
        let guardedResult = await resourceCache.performIfRenderReferenceOwnerActive(
            ownerEpoch: binding.resourceOwnershipEpoch,
            renderToken: renderToken
        ) { () -> RendererUpdateResult in
            let update: () -> RendererUpdateResult = {
                guard let renderer = self.renderer else {
                    return RendererUpdateResult(didRender: true, resyncLaneEpoch: nil)
                }
                for image in cachedResources
                where renderer.resolveResourceImage(image.hash) == nil {
                    renderer.commitResourceImage(image)
                }
                let epoch = discardTextEditsForResync
                    ? renderer.textEditingSession.discardUnresolvedEditsForResync()
                    : nil
                if epoch != nil {
                    self.advanceInteractionIncarnation()
                }
                defer {
                    if discardTextEditsForResync {
                        renderer.textEditingSession.finishResyncTextBoundary()
                    }
                }
                do {
                    try self.rendererUpdateInterceptorForTesting?()
                    if forceRemount || !self.hasMountedInitialTree {
                        if preserveLocalTextForRemount {
                            try renderer.layoutRenderer.mount(
                                store: snapshot.store,
                                preserveLocalText: true
                            )
                        } else {
                            try renderer.attach(store: snapshot.store)
                        }
                        renderer.showWindows()
                        self.hasMountedInitialTree = true
                    } else if let transaction {
                        try renderer.apply(transaction: transaction, newStore: snapshot.store)
                    }
                    return RendererUpdateResult(didRender: true, resyncLaneEpoch: epoch)
                } catch {
                    SessionDiagnostics.error("Renderer update failed: \(error)")
                    // Any renderer failure may have left a partially torn-down view tree: the
                    // incremental path remounts internally for structural transactions, so a throw
                    // can mean every surface window was closed. Force a full re-attach from the
                    // committed store on the next transaction rather than mutating a tree we no
                    // longer trust.
                    self.hasMountedInitialTree = false
                    return RendererUpdateResult(
                        didRender: false,
                        resyncLaneEpoch: epoch,
                        failureDescription: String(describing: error)
                    )
                }
            }

            return self.outbox.resyncRenderFence.performIfActive(
                renderToken,
                boundaryEpoch: { $0.resyncLaneEpoch },
                update
            )
                ?? RendererUpdateResult(
                    didRender: false,
                    resyncLaneEpoch: nil,
                    wasSuperseded: true
                )
        }
        guard let result = guardedResult else {
            return RendererUpdateResult(
                didRender: false,
                resyncLaneEpoch: nil,
                wasSuperseded: true
            )
        }
        if let interceptor = rendererDidRenderInterceptorForTesting {
            await interceptor()
        }
        let liveReferences = await currentLiveResourceReferences()
        guard await resourceCache.finishRenderReferenceLease(
            ownerEpoch: binding.resourceOwnershipEpoch,
            renderToken: renderToken,
            liveReferences: liveReferences
        ) else {
            return RendererUpdateResult(
                didRender: false,
                resyncLaneEpoch: result.resyncLaneEpoch,
                wasSuperseded: true
            )
        }
        return result
    }

    /// Suspends allocation, then leaves every flushed edit in TextEditingSession for same-session
    /// resume. Assigned envelopes remain in EventOutbox; a forced resync clears native drafts.
    private func retainNativeTextBeforeDisconnect(
        binding: EventOutboxConnectionBinding?
    ) async {
        guard let binding,
              await outbox.suspendForTeardown(binding: binding) else {
            return
        }
        withStateLock { isFlushingTextForDisconnect = true }
        await MainActor.run {
            self.renderer?.textEditingSession.flushAllPending()
            self.withStateLock { self.isFlushingTextForDisconnect = false }
            self.advanceInteractionIncarnation()
        }
    }

    /// How long `stop()` lets the receive loop drain closed-transport frames before cancelling it.
    private static let receiveDrainGraceNanoseconds: UInt64 = 2_000_000_000

    /// Stops the session coordinator and closes the underlying transport.
    public func stop() async {
        let stoppedState: (
            receiveTask: Task<Void, Never>?,
            handshakeSendTask: Task<Void, Error>?,
            connectionBinding: EventOutboxConnectionBinding?,
            sessionIncarnation: EventOutboxSessionIncarnation?,
            lifecycleGeneration: UInt64,
            shouldStop: Bool
        ) = withStateLock {
            guard isRunning, !isStopping else {
                return (nil, nil, nil, nil, lifecycleGeneration, false)
            }
            precondition(
                lifecycleGeneration < UInt64.max,
                "SessionController lifecycle generation exhausted"
            )
            lifecycleGeneration += 1
            isStopping = true
            eventDispatchEnabled = false
            return (
                receiveTask,
                handshakeSendOwnership?.task,
                outboxConnectionBinding,
                outboxSessionIncarnation,
                lifecycleGeneration,
                true
            )
        }

        guard stoppedState.shouldStop else { return }
        stoppedState.handshakeSendTask?.cancel()
        await retainNativeTextBeforeDisconnect(
            binding: stoppedState.connectionBinding
        )
        await terminalPump.disconnect()
        stopRangeRequestPump()
        await MainActor.run {
            self.interactionDispatchTail?.cancel()
            self.interactionDispatchTail = nil
            self.renderer?.clearCollectionRangeTrackers()
        }

        // Restart stays inadmissible until this close and receive drain complete, so the old
        // teardown can never close a newly started attempt on the same Transport instance.
        await transport.close()

        if let handshakeSendTask = stoppedState.handshakeSendTask {
            _ = try? await handshakeSendTask.value
        }
        if let receiveTask = stoppedState.receiveTask {
            // Bound the drain. `Transport` is a public protocol: a conformer whose `close()` never
            // finishes its stream continuation would otherwise hang `stop()` forever, with no
            // cancellation to break it.
            await withTaskGroup(of: Void.self) { group in
                group.addTask { await receiveTask.value }
                group.addTask {
                    try? await Task.sleep(nanoseconds: Self.receiveDrainGraceNanoseconds)
                    receiveTask.cancel()
                }
                await group.next()
                group.cancelAll()
            }
            receiveTask.cancel()
            await receiveTask.value
        }

        let replayGeneration = withStateLock {
            resumeGeneration ?? activeReplayRetryGeneration
        }
        if let replayGeneration {
            await outbox.stopResumeWork(generation: replayGeneration)
        }

        // An active disconnect drops only its own in-flight assemblies. A stale controller
        // cannot clear transfers already started by a replacement cache owner.
        if let connectionBinding = stoppedState.connectionBinding {
            _ = await resourceCache.clearPartials(
                ownerEpoch: connectionBinding.resourceOwnershipEpoch
            )
        }
        rejectedResourceHashes.removeAll(keepingCapacity: false)

        if let interceptor = stopWillRetireMountForTesting {
            await interceptor()
        }
        let mayRetireMount = withStateLock {
            lifecycleGeneration == stoppedState.lifecycleGeneration && isStopping
        }
        if mayRetireMount {
            await MainActor.run {
                guard self.withStateLock({
                    self.lifecycleGeneration == stoppedState.lifecycleGeneration
                        && self.isStopping
                }) else {
                    return
                }
                self.hasMountedInitialTree = false
            }
        }
        if let connectionBinding = stoppedState.connectionBinding {
            _ = await resourceCache.deactivateReferenceOwner(
                epoch: connectionBinding.resourceOwnershipEpoch
            )
        }
        _ = clearSessionStateAfterStop(
            generation: stoppedState.lifecycleGeneration
        )
    }

    @discardableResult
    private func clearSessionStateAfterStop(generation: UInt64) -> Bool {
        withStateLock {
            guard lifecycleGeneration == generation, isStopping else {
                return false
            }
            handshakeSendOwnership = nil
            receiveTask = nil
            // A restarted session re-handshakes and re-mounts from scratch, so no partial frame or
            // mount state may survive.
            streamDecoder = SRUIMessageStreamDecoder()
            // The server answers the next `CLIENT RESUME` with replay or a fresh snapshot, so the
            // divergence latch and the resync latch must not outlive this session either: a
            // surviving `_isDiverged` makes `handleTransaction` silently discard every transaction
            // of the next one, and a surviving `pendingResync` would treat its first transaction as
            // a snapshot (§18, §4 inv. 13).
            _isDiverged = false
            pendingResync = false
            requestedSessionId = nil
            resumeGeneration = nil
            activeReplayRetryGeneration = nil
            outboxConnectionBinding = nil
            outboxSessionIncarnation = nil
            isFlushingTextForDisconnect = false
            eventDispatchEnabled = false
            phase = .idle
            isRunning = false
            isStopping = false
            return true
        }
    }
}
