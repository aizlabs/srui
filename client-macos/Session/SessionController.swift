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
    private var currentSessionId: String?

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

        if let renderer {
            MainActor.assumeIsolated {
                self.wireActionHandler(for: renderer)
            }
        }
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
        renderer.onAction = { [weak self] nodeID, typeRef in
            guard let self else { return }
            if typeRef == .EVENT_ACTIVATE || typeRef == TypeRef.standard(1) {
                let observedRev = self.applier.currentSnapshot.revision
                Task {
                    do {
                        try await self.outbox.sendActivate(
                            nodeId: nodeID,
                            observedRevision: observedRev,
                            via: self.transport
                        )
                    } catch {
                        // Log or handle dispatch failure
                    }
                }
            }
        }
    }

    /// Starts the session by sending initial handshake and launching the background receive loop (§18, §22.2).
    public func start() async throws {
        let shouldStart = withStateLock {
            if isRunning { return false }
            isRunning = true
            return true
        }
        guard shouldStart else { return }

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
                    break
                }

                for msg in messages {
                    try await handleIncomingMessage(msg)
                }
            }
        } catch {
            // Stream terminated
        }
    }

    /// Processes a single wire envelope, deserializing and applying transactions serially (§12.1, §22.2).
    public func handleIncomingMessage(_ message: SRUIMessage) async throws {
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

        case .transaction(let wireTx):
            // 1. Off-main Protobuf validation and conversion (§16, §22.2)
            let domainTx = try decoder.validateAndConvertTransaction(wire: wireTx)

            // 2. Serialized store application and revision advancement (§12.1, §14)
            let applyResult = applier.apply(record: domainTx)
            switch applyResult {
            case .success:
                let snapshot = applier.currentSnapshot

                // 3. Dispatch to MainActor for AppKit view mutation (§22.2)
                await MainActor.run {
                    guard let renderer = self.renderer else { return }
                    do {
                        if !self.hasMountedInitialTree {
                            self.hasMountedInitialTree = true
                            try renderer.attach(store: snapshot.store)
                            renderer.showWindows()
                        } else {
                            try renderer.apply(transaction: domainTx, newStore: snapshot.store)
                        }
                    } catch {
                        // View apply error
                    }
                }

            case .failure(let err):
                // Transaction rejected without side-effects (§12.1)
                throw err
            }

        default:
            break
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
