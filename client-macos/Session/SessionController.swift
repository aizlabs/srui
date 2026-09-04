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
// - §22.2 Threading: network IO and protobuf decoding run off the main actor; AppKit mutations
//   are dispatched to `MainActor`.
// - §8 / §22.7 Sparse collections: `ClientModelRangeRequest` is sent on the `.ui` lane and is
//   not an Event. A copy arriving from the server is a protocol violation.
// - §4 inv. 13: unrecoverable divergence fails explicitly instead of degrading silently.
//

import Foundation
import SemanticModel
import Protocol
import TransportSSH
import RendererAppKit
import Resources
import Collections

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
    /// A newer reconnect attempt on the same outbox superseded this controller. Its handshake can
    /// never complete, so the owner discards this controller instead of reconnecting it (§18).
    case superseded(String)

    public var description: String {
        switch self {
        case .replicaDiverged(let err): return "local replica diverged: \(err)"
        case .decodeFailed(let msg): return "decode failed: \(msg)"
        case .protocolViolation(let msg): return "protocol violation: \(msg)"
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
    private var isRunning = false
    private var hasMountedInitialTree = false
    private var pendingResync = false
    private var currentSessionId: String?
    private var actionHandlerWired = false
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
        clientCapabilities: CapabilitySet = [Profile.standardWidgetsV1],
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

        renderer.onCollectionRangeRequest = { [weak self] request in
            self?.rangeRequestContinuation?.yield(request)
        }
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
        guard allowed else { return }

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
            await MainActor.run {
                self.renderer?.resetCollectionRangeTrackers()
            }
            SessionDiagnostics.error("Collection range request send failed: \(error)")
        }
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

        startRangeRequestPump()

        var didStart = false
        defer {
            if !didStart {
                stopRangeRequestPump()
                withStateLock {
                    self.isRunning = false
                    self.requestedSessionId = nil
                    self.resumeGeneration = nil
                    self.eventDispatchEnabled = false
                    self.phase = .idle
                }
                Task { [transport] in
                    await transport.close()
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
        let limits = makeClientLimits()
        let knownResourceHashes = await resourceCache.knownHashes().map(\.bytes)

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
            resume.limits = limits
            resume.knownResourceHashes = knownResourceHashes

            var envelope = SRUIMessage()
            envelope.clientResume = resume
            guard withStateLock({ isRunning }) else { return }
            try await transport.send(data: SRUIFraming.encodeFramed(envelope), logicalClass: .control)
            withStateLock {
                self.phase = .awaitingResume(sessionId: requestedId, generation: resumeGeneration)
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
            guard withStateLock({ isRunning }) else { return }
            try await transport.send(data: SRUIFraming.encodeFramed(envelope), logicalClass: .control)
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
                enableNewEventsAfterReplay: true,
                onReplayFailure: { [weak self] error in
                    await self?.handlePendingEventReplayFailure(error)
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
        await finalizeResumeAttempt(generation) {
            let negotiated = self.retainedCapabilities ?? self.clientCapabilities
            self.currentSessionId = resumeOk.sessionID
            self.requestedSessionId = nil
            self.resumeGeneration = nil
            self.retainedCapabilities = negotiated
            self.phase = .active(negotiated: negotiated)
            self.eventDispatchEnabled = !self._isDiverged
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
                        enableNewEventsAfterReplay: false,
                        onReplayFailure: { [weak self] error in
                            await self?.handlePendingEventReplayFailure(error)
                        }
                    )
                    guard accepted else {
                        await failRefusedResumeDecision(
                            generation,
                            "same-session resync answered a superseded resume attempt"
                        )
                        return
                    }
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
                    generation: generation
                )
                guard accepted else {
                    await failRefusedResumeDecision(
                        generation,
                        "replacement resync answered a superseded resume attempt"
                    )
                    return
                }
                // Replacement tears down in-flight resource assemblies; committed CAS is retained (§14, §18).
                await resourceCache.clearPartials()
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
                await resourceCache.clearPartials()
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

        await syncLiveResourceReferences()
        do {
            if let commit = try await resourceCache.ingestMetadata(input) {
                await dispatchResourceCommit(commit)
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

        await syncLiveResourceReferences()
        do {
            if let commit = try await resourceCache.ingestChunk(input) {
                await dispatchResourceCommit(commit)
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
    private func syncLiveResourceReferences() async {
        let live = await MainActor.run { () -> Set<ResourceHash> in
            var hashes = self.renderer?.liveResourceHashes() ?? []
            hashes.formUnion(self.applier.currentSnapshot.store.referencedResourceHashes())
            return hashes
        }
        await resourceCache.setLiveReferences(live)
    }

    /// Pushes a newly committed decoded image onto AppKit on the main actor (§14, §22.2).
    ///
    /// Reconfirmed commits (`newlyCommitted == false`) still hydrate a replacement renderer that
    /// shares the cache but does not yet hold the `NSImage`.
    private func dispatchResourceCommit(_ commit: ResourceCommit) async {
        rejectedResourceHashes.remove(commit.image.hash)
        await MainActor.run {
            if !commit.evictedHashes.isEmpty {
                self.renderer?.evictResourceImages(commit.evictedHashes)
            }
            let alreadyInstalled = self.renderer?.resolveResourceImage(commit.image.hash) != nil
            if commit.newlyCommitted || !alreadyInstalled {
                self.renderer?.commitResourceImage(commit.image)
            }
        }
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
        let settled = await outbox.settleAcknowledgement(
            clientInstanceId: ClientInstanceId(ack.clientInstanceID),
            eventId: eventId,
            throughSeq: ack.lastProcessedEventSeq,
            sessionId: ack.sessionID
        )
        guard settled else {
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

        // Apply and capture the committed snapshot in a single critical section so the renderer is
        // handed exactly the store produced by this transaction (§22.2).
        let applyResult: Result<TransactionSnapshot, TxnError>
        if isResyncSnapshot {
            // A snapshot replaces the entire replica, so the supersession check, the publish, and
            // the latch release run inside one outbox critical section: checking here and
            // publishing after a suspension would let a newer attempt open in between and the
            // stale snapshot still land on the shared applier (§18). The rebuild happens before
            // that section so it never occupies the outbox actor while acknowledgements wait
            // (§22.2). A live resync carries no generation and is checked the same way — another
            // controller's outstanding attempt must block it too.
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
            applyResult = published
        } else {
            // Live-stream frames are deliveries: either a committed transaction verbatim, or a
            // coalesced scalar delta standing in for a run of them (§12.1, §20.4).
            applyResult = applier.applyDelivered(record: domainTx)
        }

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

    private func handlePendingEventReplayFailure(_ error: String) async {
        await reportFailure(.transportEnded("pending event replay retry failed: \(error)"))
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

    /// Commits controller state after a catch-up or resync snapshot committed under the latch.
    ///
    /// The outbox latch was already released inside the critical section that published the
    /// snapshot (`commitResyncSnapshot`), so there is no second supersession decision to lose
    /// here: an attempt that opens now supersedes this controller the ordinary way, and
    /// `finalizeResumeAttempt` hands the generation back if teardown or divergence raced it.
    private func completeSnapshotCatchUp() async {
        withStateLock { self.pendingResync = false }
        let generation = withStateLock { self.resumeGeneration }
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
        let failureState: (
            handler: (@Sendable (SessionFailure) -> Void)?,
            replayGeneration: UInt64?
        ) = withStateLock {
            if _isDiverged { return (nil, nil) }
            _isDiverged = true
            eventDispatchEnabled = false
            phase = .failed
            let replayGeneration = resumeGeneration ?? activeReplayRetryGeneration
            activeReplayRetryGeneration = nil
            return (_onFailure ?? { _ in }, replayGeneration)
        }
        guard let handler = failureState.handler else { return }

        if let replayGeneration = failureState.replayGeneration {
            await outbox.stopResumeWork(generation: replayGeneration)
        }
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
        await hydrateCachedResources(snapshot.store.referencedResourceHashes())
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
        await syncLiveResourceReferences()
    }

    /// Pins and installs verified shared-cache images before a renderer mounts a new snapshot.
    private func hydrateCachedResources(_ hashes: Set<ResourceHash>) async {
        let cached = await resourceCache.setLiveReferencesAndLookup(hashes)
        guard !cached.isEmpty else { return }
        await MainActor.run {
            for image in cached
            where self.renderer?.resolveResourceImage(image.hash) == nil {
                self.renderer?.commitResourceImage(image)
            }
        }
    }

    /// How long `stop()` lets the receive loop drain closed-transport frames before cancelling it.
    private static let receiveDrainGraceNanoseconds: UInt64 = 2_000_000_000

    /// Stops the session coordinator and closes the underlying transport.
    public func stop() async {
        let stoppedState: (
            receiveTask: Task<Void, Never>?,
            shouldStop: Bool
        ) = withStateLock {
            guard isRunning else { return (nil, false) }
            isRunning = false
            eventDispatchEnabled = false
            return (receiveTask, true)
        }

        guard stoppedState.shouldStop else { return }

        stopRangeRequestPump()
        await MainActor.run {
            self.renderer?.resetCollectionRangeTrackers()
        }

        // Close the transport first so the receive loop drains any buffered catch-up frames
        // (welcome snapshot, replay) while handshake phase is still valid. Resetting `phase` or
        // cancelling the task before that completes rejects in-flight transactions as protocol
        // violations even though the server sent them in order (§15, §18).
        // When `stop()` races `start()` before `receiveTask` is assigned, closing the transport
        // still tears down an in-progress handshake send (§22.2).
        await transport.close()

        if let receiveTask = stoppedState.receiveTask {
            // Bound the drain. `Transport` is a public protocol: a conformer whose `close()` never
            // finishes its stream continuation would otherwise hang `stop()` forever, with no
            // cancellation to break it. Wait for the drain, but cancel it once the grace period
            // elapses so teardown always completes (§22.2).
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

        // Release the resume latch only after the drain: the frames the drain exists to consume
        // are exactly the ones the latch guards — a buffered `SERVER RESUME_OK` would be refused
        // and reported as supersession on a deliberate stop, and a buffered catch-up snapshot
        // would be dropped instead of applied (§18). Allocation of new events is already blocked
        // by `eventDispatchEnabled == false` above, so holding the latch across the drain admits
        // nothing.
        let replayGeneration = withStateLock { resumeGeneration ?? activeReplayRetryGeneration }
        if let replayGeneration {
            await outbox.stopResumeWork(generation: replayGeneration)
        }

        // Disconnect drops in-flight assemblies. Committed CAS entries persist on this
        // `resourceCache` instance — inject the same cache into a replacement controller to
        // advertise verified hashes and hydrate its renderer without retransferring bytes (§14, §18).
        await resourceCache.clearPartials()
        rejectedResourceHashes.removeAll(keepingCapacity: false)

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
            activeReplayRetryGeneration = nil
            eventDispatchEnabled = false
            phase = .idle
        }
    }
}
