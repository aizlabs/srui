//
// SessionController.swift
// Session
//
// Central client session coordinator wiring Transport, ProtocolDecoder, TransactionApplier,
// EventOutbox, and RendererAppKit (§22, §22.2).
//

import Foundation
import SemanticModel
import Protocol
import TransportSSH
import RendererAppKit

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
            if typeRef == .EVENT_ACTIVATE || typeRef == TypeRef.standard(1) {
                Task {
                    do {
                        let observedRev = self.applier.currentSnapshot.revision
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

        await MainActor.run {
            ensureActionHandlerWired()
        }

        // 1. Send Handshake (§15, §18)
        let clientInstanceId = outbox.clientInstanceId
        var resume = SRUIClientResume()
        resume.sessionID = withStateLock { currentSessionId } ?? "default"
        resume.clientInstanceID = clientInstanceId.bytes
        resume.lastAppliedRevision = applier.lastAppliedRevision.value

        var envelope = SRUIMessage()
        envelope.clientResume = resume
        let framedHandshake = try SRUIFraming.encodeFramed(envelope)
        try await transport.send(data: framedHandshake)

        // 2. Launch background message processing loop (§22.2)
        let task = Task.detached { [weak self] in
            guard let self else { return }
            await self.runReceiveLoop()
        }

        withStateLock {
            self.receiveTask = task
        }
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
                    SessionDiagnostics.error("Frame decode failed: \(error)")
                    break
                }

                for msg in messages {
                    await handleIncomingMessage(msg)
                }
            }
        } catch {
            SessionDiagnostics.error("Transport receive stream ended: \(error)")
        }
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
            SessionDiagnostics.log("Server resync required at revision \(resync.snapshotRevision): \(resync.reason)")

        case .transaction(let wireTx):
            let domainTx: Transaction
            do {
                domainTx = try decoder.validateAndConvertTransaction(wire: wireTx)
            } catch {
                SessionDiagnostics.error("Transaction decode failed: \(error)")
                return
            }

            let shouldApplySnapshot = withStateLock {
                pendingResync
                    || (domainTx.baseRevision == .initial && applier.lastAppliedRevision > .initial)
            }

            let applyResult: Result<Revision, TxnError>
            if shouldApplySnapshot {
                applyResult = applier.applySnapshot(record: domainTx)
                withStateLock {
                    self.pendingResync = false
                }
            } else {
                applyResult = applier.apply(record: domainTx)
            }

            switch applyResult {
            case .success:
                let snapshot = applier.currentSnapshot
                await updateRenderer(
                    transaction: shouldApplySnapshot ? nil : domainTx,
                    snapshot: snapshot,
                    forceRemount: shouldApplySnapshot
                )

            case .failure(let err):
                SessionDiagnostics.error("Transaction rejected (no side effects): \(err)")
            }

        default:
            break
        }
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
                if forceRemount || !self.hasMountedInitialTree {
                    self.hasMountedInitialTree = false
                }
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
            return t
        }

        task?.cancel()
        await transport.close()
    }
}
