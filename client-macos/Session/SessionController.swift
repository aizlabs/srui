//
// SessionController.swift
// Session
//
// Central client session coordinator wiring Transport, ProtocolDecoder, TransactionApplier,
// EventOutbox, and RendererAppKit (§22, §22.2).
//
// Spec sections implemented:
// - §12.1 Revisions and transactions: transactions are applied atomically and the renderer never
//   observes a half-committed transaction.
// - §15 Capability negotiation: `CLIENT HELLO` / `SERVER WELCOME` establish the session; resume
//   reuses the retained negotiated set instead of re-parsing profiles from `RESUME_OK`.
// - §18 Reconnect and resynchronization: `CLIENT RESUME` carries `last_applied_revision` and
//   `last_acked_event_seq`. `SERVER RESYNC_REQUIRED` and a HELLO catch-up snapshot (when
//   `WELCOME.initial_revision > 0`) replace the replica; incremental transactions never do.
// - §18.2 Event settlement: server event frontiers raise `last_acked_event_seq`, while
//   per-event acknowledgements selectively drain the outbox's retry set.
// - §22.2 Threading: network IO and protobuf decoding run off the main actor; AppKit mutations
//   are dispatched to `MainActor`.
// - §4 inv. 13: unrecoverable divergence fails explicitly instead of degrading silently.
//

import Foundation
import SemanticModel
import Protocol
import TransportSSH
import RendererAppKit

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
    /// The transport stream ended or errored.
    case transportEnded(String)

    public var description: String {
        switch self {
        case .replicaDiverged(let err): return "local replica diverged: \(err)"
        case .decodeFailed(let msg): return "decode failed: \(msg)"
        case .protocolViolation(let msg): return "protocol violation: \(msg)"
        case .transportEnded(let msg): return "transport ended: \(msg)"
        }
    }
}

/// Event dispatch is disabled until the server proves session identity continuity (§18).
public enum SessionDispatchError: Error, Equatable, Sendable {
    case resumeNotConfirmed
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

/// Central coordinator managing client session lifecycle, message decoding, store application,
/// outbox event dispatch, and UI rendering (§22, §22.2).
public final class SessionController: @unchecked Sendable {
    public let transport: any Transport
    public let applier: TransactionApplier
    public let outbox: EventOutbox
    public let decoder: ProtocolDecoder
    public let renderer: AppKitRenderer?
    public let clientCapabilities: CapabilitySet
    public let requiredServerProfiles: CapabilitySet

    private let lock = NSLock()
    private var streamDecoder = SRUIMessageStreamDecoder()
    private var receiveTask: Task<Void, Never>?
    private var isRunning = false
    private var hasMountedInitialTree = false
    private var pendingResync = false
    private var currentSessionId: String?
    private var actionHandlerWired = false
    private var _isDiverged = false
    private var _onFailure: (@Sendable (SessionFailure) -> Void)?
    private var requestedSessionId: String?
    /// Generation of this controller's outstanding resume attempt (§18). Strictly increasing per
    /// outbox: a response carrying an older generation is discarded outright.
    private var resumeGeneration: UInt64?
    private var eventDispatchEnabled = false
    private var phase: ProtocolPhase = .idle
    /// Negotiated set from the last successful `SERVER WELCOME`, retained across `stop()` so a
    /// later `CLIENT RESUME` can restore it (§15, §18).
    private var retainedCapabilities: CapabilitySet?

    public init(
        transport: any Transport,
        applier: TransactionApplier = TransactionApplier(),
        outbox: EventOutbox = EventOutbox(),
        decoder: ProtocolDecoder = ProtocolDecoder(),
        renderer: AppKitRenderer? = nil,
        sessionId: String? = nil,
        clientCapabilities: CapabilitySet = [Profile.standardWidgetsV1],
        requiredServerProfiles: CapabilitySet = []
    ) {
        self.transport = transport
        self.applier = applier
        self.outbox = outbox
        self.decoder = decoder
        self.renderer = renderer
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

        renderer.onInteraction = { [weak self] interaction in
            guard let self else { return }

            // §7.7: `observed_revision` is the revision the user was actually looking at when the
            // control was interacted with, and the server validates that the action is still enabled
            // and permitted at that revision. It must therefore be sampled synchronously here on the
            // MainActor — reading it after a suspension point would report a revision the user
            // never saw and defeat that staleness check.
            let observedRev = self.applier.currentSnapshot.revision

            Task {
                do {
                    switch interaction {
                    case .activate(let nodeID):
                        try await self.sendActivate(
                            nodeId: nodeID,
                            observedRevision: observedRev
                        )
                    case .valueChanged(let nodeID, let value):
                        try await self.sendValueChanged(
                            nodeId: nodeID,
                            observedRevision: observedRev,
                            value: value
                        )
                    case .selectionChanged(let nodeID, let itemID):
                        try await self.sendSelectionChanged(
                            nodeId: nodeID,
                            observedRevision: observedRev,
                            itemId: itemID
                        )
                    }
                } catch {
                    SessionDiagnostics.error("Interaction dispatch failed: \(error)")
                }
            }
        }
    }

    @MainActor
    private func ensureActionHandlerWired() {
        guard let renderer else { return }
        wireActionHandler(for: renderer)
    }

    /// Starts the session by sending a handshake request and launching the receive loop (§15, §18, §22.2).
    ///
    /// Fresh connections send `CLIENT_HELLO` with client capabilities (§15). Reconnections send `CLIENT_RESUME` (§18).
    /// The handshake must complete before any transaction or event traffic is permitted (§4 inv. 13, §15).
    public func start() async throws {
        let shouldStart = withStateLock {
            if isRunning { return false }
            isRunning = true
            eventDispatchEnabled = false
            phase = .idle
            return true
        }
        guard shouldStart else { return }

        var didStart = false
        defer {
            if !didStart {
                withStateLock {
                    self.isRunning = false
                    self.requestedSessionId = nil
                    self.resumeGeneration = nil
                    self.eventDispatchEnabled = false
                    self.phase = .idle
                }
            }
        }

        await MainActor.run {
            ensureActionHandlerWired()
        }

        let clientInstanceId = outbox.clientInstanceId
        let requestedId = withStateLock {
            currentSessionId ?? requestedSessionId
        }

        if let requestedId {
            let resumeGeneration = await outbox.beginResumeAttempt()
            withStateLock {
                self.requestedSessionId = requestedId
                self.resumeGeneration = resumeGeneration
            }
            var resume = SRUIClientResume()
            resume.sessionID = requestedId
            resume.clientInstanceID = clientInstanceId.bytes
            resume.lastAppliedRevision = applier.lastAppliedRevision.value
            resume.lastAckedEventSeq = await outbox.lastAckedEventSeq

            var envelope = SRUIMessage()
            envelope.clientResume = resume
            guard withStateLock({ isRunning }) else { return }
            try await transport.send(data: SRUIFraming.encodeFramed(envelope))
            withStateLock {
                self.phase = .awaitingResume(sessionId: requestedId, generation: resumeGeneration)
            }
        } else {
            var hello = SRUIClientHello()
            hello.coreVersion = "0.4.0"
            hello.profiles = clientCapabilities.toStringArray()
            hello.clientInstanceID = clientInstanceId.bytes

            var envelope = SRUIMessage()
            envelope.clientHello = hello
            guard withStateLock({ isRunning }) else { return }
            try await transport.send(data: SRUIFraming.encodeFramed(envelope))
            withStateLock {
                self.phase = .awaitingWelcome
            }
        }

        guard withStateLock({ isRunning }) else { return }

        // Start receiving the handshake response before processing data.
        let task = Task.detached { [weak self] in
            guard let self else { return }
            await self.runReceiveLoop()
        }

        let adopted = withStateLock { () -> Bool in
            guard isRunning else { return false }
            self.receiveTask = task
            return true
        }
        guard adopted else {
            task.cancel()
            return
        }
        didStart = true
    }

    /// Dispatches a manual activation event for the given node ID (§7.7).
    @discardableResult
    public func sendActivate(nodeId: NodeId) async throws -> Event {
        let snapshot = applier.currentSnapshot
        return try await sendActivate(nodeId: nodeId, observedRevision: snapshot.revision)
    }

    @discardableResult
    private func sendActivate(nodeId: NodeId, observedRevision: Revision) async throws -> Event {
        guard withStateLock({
            guard eventDispatchEnabled else { return false }
            if case .active = phase { return true }
            return false
        }) else {
            throw SessionDispatchError.resumeNotConfirmed
        }
        return try await outbox.sendActivate(
            nodeId: nodeId,
            observedRevision: observedRevision,
            via: transport
        )
    }

    /// Dispatches a manual value change event for the given node ID (§7.6).
    @discardableResult
    public func sendValueChanged(nodeId: NodeId, value: Value) async throws -> Event {
        let snapshot = applier.currentSnapshot
        return try await sendValueChanged(nodeId: nodeId, observedRevision: snapshot.revision, value: value)
    }

    @discardableResult
    private func sendValueChanged(nodeId: NodeId, observedRevision: Revision, value: Value) async throws -> Event {
        guard withStateLock({
            guard eventDispatchEnabled else { return false }
            if case .active = phase { return true }
            return false
        }) else {
            throw SessionDispatchError.resumeNotConfirmed
        }
        return try await outbox.sendValueChanged(
            nodeId: nodeId,
            observedRevision: observedRevision,
            value: value,
            via: transport
        )
    }

    /// Dispatches a manual selection change event for the given node ID (§7.6).
    @discardableResult
    public func sendSelectionChanged(nodeId: NodeId, itemId: ItemId) async throws -> Event {
        let snapshot = applier.currentSnapshot
        return try await sendSelectionChanged(nodeId: nodeId, observedRevision: snapshot.revision, itemId: itemId)
    }

    @discardableResult
    private func sendSelectionChanged(nodeId: NodeId, observedRevision: Revision, itemId: ItemId) async throws -> Event {
        guard withStateLock({
            guard eventDispatchEnabled else { return false }
            if case .active = phase { return true }
            return false
        }) else {
            throw SessionDispatchError.resumeNotConfirmed
        }
        return try await outbox.sendSelectionChanged(
            nodeId: nodeId,
            observedRevision: observedRevision,
            itemId: itemId,
            via: transport
        )
    }

    /// Processes the incoming transport stream off the main actor (§22.2).
    private func runReceiveLoop() async {
        let stream = transport.receiveStream()

        do {
            for try await chunk in stream {
                let messages: [SRUIMessage]
                do {
                    messages = try streamDecoder.appendAndExtract(incoming: chunk)
                } catch {
                    await reportFailure(.decodeFailed("frame decode failed: \(error)"))
                    return
                }

                for msg in messages {
                    await handleIncomingMessage(msg)
                }
            }
        } catch {
            await reportFailure(.transportEnded("\(error)"))
            return
        }

        // A peer that closes cleanly finishes the stream *without* throwing (socket EOF calls
        // `continuation.finish()`), so falling out of the loop here is the common disconnect, not a
        // normal shutdown. Unless `stop()` asked for the teardown, this is terminal for the replica
        // and must be reported so the caller reconnects and resumes rather than sitting on a live
        // session with no reader (§18, §4 inv. 13).
        let stoppedIntentionally = Task.isCancelled || withStateLock { !isRunning }
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

        case .clientHello, .clientResume:
            await reportFailure(.protocolViolation(
                "Received client-originated handshake message from server"
            ))
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

        let accepted = await outbox.confirmFreshSession(id: welcome.sessionID)
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
        if welcome.initialRevision > 0 {
            await outbox.suspendNewEvents()
        }
        SessionDiagnostics.log(
            "Handshake completed successfully with session \(welcome.sessionID), negotiated: \(negotiated)"
        )
    }

    private func handleResumeOk(_ resumeOk: SRUIServerResumeOk) async {
        let (requested, generation) = withStateLock {
            (requestedSessionId, resumeGeneration)
        }
        guard let requested, let generation else {
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
            let accepted = try await outbox.completeSameSessionResume(
                id: resumeOk.sessionID,
                lastProcessedEventSeq: resumeOk.lastProcessedEventSeq,
                generation: generation,
                via: transport,
                enableNewEventsAfterReplay: true
            )
            guard accepted else {
                SessionDiagnostics.log("Ignoring superseded SERVER RESUME_OK")
                return
            }
        } catch {
            await reportFailure(.transportEnded("pending event replay failed: \(error)"))
            return
        }
        withStateLock {
            let negotiated = self.retainedCapabilities ?? self.clientCapabilities
            self.currentSessionId = resumeOk.sessionID
            self.requestedSessionId = nil
            self.resumeGeneration = nil
            self.retainedCapabilities = negotiated
            self.phase = .active(negotiated: negotiated)
            self.eventDispatchEnabled = self.isRunning && !self._isDiverged
        }
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

        let (requested, generation) = withStateLock {
            (requestedSessionId, resumeGeneration)
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
                    let accepted = try await outbox.completeSameSessionResume(
                        id: resync.sessionID,
                        lastProcessedEventSeq: resync.lastProcessedEventSeq,
                        generation: generation,
                        via: transport,
                        enableNewEventsAfterReplay: false
                    )
                    guard accepted else {
                        SessionDiagnostics.log("Ignoring superseded same-session resync")
                        return
                    }
                } catch {
                    await reportFailure(.transportEnded("pending event replay failed: \(error)"))
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
                    generation: generation
                )
                guard accepted else {
                    SessionDiagnostics.log("Ignoring superseded replacement resync")
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
                await outbox.applyLiveResyncFrontier(
                    lastProcessedEventSeq: resync.lastProcessedEventSeq
                )
                enterAwaitingSnapshot(sessionId: resync.sessionID, negotiated: negotiated)

            case .replaced:
                let currentId = withStateLock { currentSessionId }
                guard resync.sessionID != currentId else {
                    await reportFailure(.protocolViolation(
                        "replacement resync reused expired session_id \(currentId ?? "<none>")"
                    ))
                    return
                }
                await outbox.applyReplacementFrontier(
                    id: resync.sessionID,
                    lastProcessedEventSeq: resync.lastProcessedEventSeq
                )
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

    /// Settles one outbound event against the server's acknowledgement (§18, §18.2).
    ///
    /// Every status is terminal for that `event_id` — including `rejected`. An event the server
    /// refuses must be dropped here: leaving it pending would replay it on the next resume, which
    /// the server would refuse again, forever.
    private func handleEventAck(_ ack: SRUIServerEventAck) async {
        guard ClientInstanceId(ack.clientInstanceID) == outbox.clientInstanceId else {
            SessionDiagnostics.error(
                "Ignoring event acknowledgement for a different client instance"
            )
            return
        }

        let eventId = EventId(ack.eventID)
        let ackSessionId = ack.sessionID.isEmpty ? nil : ack.sessionID
        let belongsToActiveSession = await outbox.settleAcknowledgement(
            eventId: eventId,
            throughSeq: ack.lastProcessedEventSeq,
            sessionId: ackSessionId
        )
        guard belongsToActiveSession else {
            SessionDiagnostics.error(
                "Ignoring event acknowledgement from expired session \(ack.sessionID)"
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

    }

    private func handleTransaction(_ wireTx: SRUITransaction) async {
        // A diverged replica cannot meaningfully apply anything until it resumes (§18).
        guard !isDiverged else { return }

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

        if isResyncSnapshot, let outstandingGeneration {
            guard await outbox.matchesActiveResumeGeneration(outstandingGeneration) else {
                SessionDiagnostics.log(
                    "Ignoring resync snapshot for superseded resume generation"
                )
                return
            }
        }

        // Apply and capture the committed snapshot in a single critical section so the renderer is
        // handed exactly the store produced by this transaction (§22.2).
        let applyResult: Result<TransactionSnapshot, TxnError> = isResyncSnapshot
            ? applier.applyResyncSnapshot(record: domainTx)
            : applier.applyCommitted(record: domainTx)

        switch applyResult {
        case .success(let snapshot):
            if isResyncSnapshot {
                // Enable dispatch before the renderer mounts so the first click after catch-up
                // is accepted. Clearing the latch only after a successful apply keeps a rejected
                // snapshot from stranding the client with no path back to a usable tree (§18).
                await completeSnapshotCatchUp()
            }
            await updateRenderer(
                transaction: isResyncSnapshot ? nil : domainTx,
                snapshot: snapshot,
                forceRemount: isResyncSnapshot
            )

        case .failure(let err):
            await handleTransactionRejection(err, isResyncSnapshot: isResyncSnapshot)
        }
    }

    /// Re-enables the outbox and data-plane after a committed catch-up or resync snapshot.
    private func completeSnapshotCatchUp() async {
        let generation = withStateLock { self.resumeGeneration }
        if let generation {
            let accepted = await outbox.finishResync(generation: generation)
            withStateLock {
                guard accepted else { return }
                self.pendingResync = false
                self.resumeGeneration = nil
                self.eventDispatchEnabled = self.isRunning && !self._isDiverged
                if case .awaitingSnapshot(let negotiated) = self.phase {
                    self.phase = .active(negotiated: negotiated)
                }
            }
        } else {
            withStateLock { self.pendingResync = false }
            await outbox.allowNewEvents()
            withStateLock {
                self.eventDispatchEnabled = self.isRunning && !self._isDiverged
                if case .awaitingSnapshot(let negotiated) = self.phase {
                    self.phase = .active(negotiated: negotiated)
                }
            }
        }
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

        // Anything else means the next transaction will fail for the same reason forever: we have
        // missed committed state and cannot resynchronize by continuing to listen (§4 inv. 13).
        await reportFailure(.replicaDiverged(error))
    }

    /// Reports a terminal session failure exactly once and tears the transport down so the caller
    /// can reconnect and resume (§18, §4 inv. 13).
    private func reportFailure(_ failure: SessionFailure) async {
        let handler: (@Sendable (SessionFailure) -> Void)? = withStateLock {
            if _isDiverged { return nil }
            _isDiverged = true
            eventDispatchEnabled = false
            phase = .failed
            return _onFailure ?? { _ in }
        }
        guard let handler else { return }

        SessionDiagnostics.error("Session failed: \(failure). Reconnect and resume to recover (§18).")
        handler(failure)
        await transport.close()
    }

    /// Dispatches committed store state to AppKit on the main actor (§22.2).
    private func updateRenderer(
        transaction: Transaction?,
        snapshot: TransactionSnapshot,
        forceRemount: Bool
    ) async {
        await MainActor.run {
            guard let renderer = self.renderer else { return }
            do {
                if forceRemount || !self.hasMountedInitialTree {
                    try renderer.attach(store: snapshot.store)
                    renderer.showWindows()
                    self.hasMountedInitialTree = true
                } else if let transaction {
                    try renderer.apply(transaction: transaction, newStore: snapshot.store)
                }
            } catch {
                SessionDiagnostics.error("Renderer update failed: \(error)")
                // Any renderer failure may have left a partially torn-down view tree: the
                // incremental path remounts internally for structural transactions, so a throw can
                // mean every surface window was closed. Force a full re-attach from the committed
                // store on the next transaction rather than mutating a tree we no longer trust.
                self.hasMountedInitialTree = false
            }
        }
    }

    /// Stops the session coordinator and closes the underlying transport.
    public func stop() async {
        let task = withStateLock { () -> (Task<Void, Never>?, Bool) in
            guard isRunning else { return (nil, false) }
            isRunning = false
            eventDispatchEnabled = false
            return (receiveTask, true)
        }

        guard task.1 else { return }

        // Close the transport first so the receive loop drains any buffered catch-up frames
        // (welcome snapshot, replay) while handshake phase is still valid. Resetting `phase` or
        // cancelling the task before that completes rejects in-flight transactions as protocol
        // violations even though the server sent them in order (§15, §18).
        // When `stop()` races `start()` before `receiveTask` is assigned, closing the transport
        // still tears down an in-progress handshake send (§22.2).
        await transport.close()

        if let receiveTask = task.0 {
            await receiveTask.value
        }

        await outbox.abandonResumeHandshake()
        clearSessionStateAfterStop()

        await MainActor.run {
            self.hasMountedInitialTree = false
        }
    }

    private func clearSessionStateAfterStop() {
        withStateLock {
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
            phase = .idle
        }
    }
}
