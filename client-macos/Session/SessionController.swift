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
// - §18 Reconnect and resynchronization: `CLIENT RESUME` carries `last_applied_revision` and
//   `last_acked_event_seq`; `SERVER RESYNC_REQUIRED` is the *only* trigger for snapshot replacement.
// - §18.2 Event settlement: `SERVER EVENT_ACK` is the only thing that raises
//   `last_acked_event_seq` and drains the outbox's retry set.
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
    /// The transport stream ended or errored.
    case transportEnded(String)

    public var description: String {
        switch self {
        case .replicaDiverged(let err): return "local replica diverged: \(err)"
        case .decodeFailed(let msg): return "decode failed: \(msg)"
        case .transportEnded(let msg): return "transport ended: \(msg)"
        }
    }
}

/// Central coordinator managing client session lifecycle, message decoding, store application,
/// outbox event dispatch, and UI rendering (§22, §22.2).
public final class SessionController: @unchecked Sendable {
    public let transport: any Transport
    public let applier: TransactionApplier
    public let outbox: EventOutbox
    public let decoder: ProtocolDecoder
    public let renderer: AppKitRenderer?

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

    public init(
        transport: any Transport,
        applier: TransactionApplier = TransactionApplier(),
        outbox: EventOutbox = EventOutbox(),
        decoder: ProtocolDecoder = ProtocolDecoder(),
        renderer: AppKitRenderer? = nil,
        sessionId: String? = nil
    ) {
        self.transport = transport
        self.applier = applier
        self.outbox = outbox
        self.decoder = decoder
        self.renderer = renderer
        self.currentSessionId = sessionId
    }

    private func withStateLock<T>(_ body: () -> T) -> T {
        lock.lock()
        defer { lock.unlock() }
        return body()
    }

    /// Whether the local replica has stopped tracking the authoritative stream (§18).
    ///
    /// Once true the session no longer applies transactions; the caller must establish a new
    /// transport and resume, which the server answers with replay or a resync snapshot.
    public var isDiverged: Bool {
        withStateLock { _isDiverged }
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

        renderer.onAction = { [weak self] nodeID, typeRef in
            guard let self else { return }
            guard typeRef == .EVENT_ACTIVATE || typeRef == TypeRef.standard(1) else { return }

            // §7.7: `observed_revision` is the revision the user was actually looking at when the
            // control was activated, and the server validates that the action is still enabled and
            // permitted at that revision. It must therefore be sampled synchronously here on the
            // MainActor — reading it after a suspension point would report a revision the user
            // never saw and defeat that staleness check.
            let observedRev = self.applier.currentSnapshot.revision

            Task {
                do {
                    try await self.outbox.sendActivate(
                        nodeId: nodeID,
                        observedRevision: observedRev,
                        via: self.transport
                    )
                } catch {
                    SessionDiagnostics.error("ACTIVATE dispatch failed: \(error)")
                }
            }
        }
    }

    @MainActor
    private func ensureActionHandlerWired() {
        guard let renderer else { return }
        wireActionHandler(for: renderer)
    }

    /// Starts the session by sending initial handshake and launching the background receive loop (§18, §22.2).
    public func start() async throws {
        let shouldStart = withStateLock {
            if isRunning { return false }
            isRunning = true
            return true
        }
        guard shouldStart else { return }

        // A handshake that never reaches the server leaves no receive loop behind, so the started
        // latch must not survive the throw: otherwise every later `start()` returns early at the
        // guard above and the controller is wedged with no reader and no diagnostics (§18).
        var didStart = false
        defer {
            if !didStart {
                withStateLock { self.isRunning = false }
            }
        }

        await MainActor.run {
            ensureActionHandlerWired()
        }

        // 1. Send Handshake (§15, §18)
        let clientInstanceId = outbox.clientInstanceId
        var resume = SRUIClientResume()
        resume.sessionID = withStateLock { currentSessionId } ?? "default"
        resume.clientInstanceID = clientInstanceId.bytes
        resume.lastAppliedRevision = applier.lastAppliedRevision.value
        // §18: the resume request carries the last acknowledged event sequence so the server can
        // bound its event-deduplication cache and replay acknowledgements (§18.2, App. B).
        resume.lastAckedEventSeq = await outbox.lastAckedEventSeq

        var envelope = SRUIMessage()
        envelope.clientResume = resume
        let framedHandshake = try SRUIFraming.encodeFramed(envelope)
        try await transport.send(data: framedHandshake)

        // 2. Replay events that were sent but never acknowledged, reusing their original
        //    `event_id` so the server's dedupe cache recognizes them as retries (§18.2).
        await outbox.resendPendingEvents(via: transport)

        // 3. Launch background message processing loop (§22.2)
        let task = Task.detached { [weak self] in
            guard let self else { return }
            await self.runReceiveLoop()
        }

        withStateLock {
            self.receiveTask = task
        }
        didStart = true
    }

    /// Dispatches a manual activation event for the given node ID (§7.7).
    @discardableResult
    public func sendActivate(nodeId: NodeId) async throws -> Event {
        let snapshot = applier.currentSnapshot
        return try await outbox.sendActivate(
            nodeId: nodeId,
            observedRevision: snapshot.revision,
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

    /// Processes a single wire envelope, deserializing and applying transactions serially (§12.1, §22.2).
    public func handleIncomingMessage(_ message: SRUIMessage) async {
        guard let payload = message.msg else { return }

        switch payload {
        case .serverResumeOk(let resumeOk):
            withStateLock {
                self.currentSessionId = resumeOk.sessionID
            }

        case .serverWelcome(let welcome):
            withStateLock {
                self.currentSessionId = welcome.sessionID
            }

        case .serverResyncRequired(let resync):
            withStateLock {
                self.pendingResync = true
            }
            SessionDiagnostics.log(
                "Server resync required at revision \(resync.snapshotRevision): \(resync.reason)"
            )

        case .transaction(let wireTx):
            await handleTransaction(wireTx)

        case .serverEventAck(let ack):
            await handleEventAck(ack)

        default:
            break
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

        await outbox.acknowledgeEvent(id: eventId)
        await outbox.acknowledgeEvents(throughSeq: ack.lastProcessedEventSeq)
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

        // §18: a snapshot replaces authoritative state, so it is driven *only* by an explicit
        // `SERVER RESYNC_REQUIRED`. `base_revision == 0` is not by itself evidence of a snapshot —
        // a stale or duplicated copy of the initial transaction carries it too, and treating that
        // as a snapshot would wipe a live replica and regress the revision (§12.1).
        let isResyncSnapshot = withStateLock { pendingResync }

        // Apply and capture the committed snapshot in a single critical section so the renderer is
        // handed exactly the store produced by this transaction (§22.2).
        let applyResult: Result<TransactionSnapshot, TxnError> = isResyncSnapshot
            ? applier.applyResyncSnapshot(record: domainTx)
            : applier.applyCommitted(record: domainTx)

        switch applyResult {
        case .success(let snapshot):
            if isResyncSnapshot {
                // Only consume the pending-resync latch once the snapshot actually committed;
                // clearing it on failure would strand the client with no path back to a resync.
                withStateLock { self.pendingResync = false }
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
        let task = withStateLock { () -> Task<Void, Never>? in
            guard isRunning else { return nil }
            isRunning = false
            let t = receiveTask
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
            return t
        }

        await MainActor.run {
            self.hasMountedInitialTree = false
        }

        task?.cancel()
        await transport.close()
    }
}
