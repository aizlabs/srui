//
// ConnectionManager.swift
// ConnectionManager
//
// Client-owned connection chrome and warm reconnect orchestration (§6.3, §17, §18, §19.1).
//
// A persisted revision is presentation metadata only. After a cold launch, a saved session ID is
// resumed with a fresh client identity and an empty revision-zero replica. Warm reconnects reuse
// the in-memory applier, outbox, resource cache, ingress gate, and renderer.
//

import Foundation
import Observation
import RendererAppKit
import Resources
import SemanticModel
import Session
import TransportSSH

public struct SavedConnection: Identifiable, Codable, Hashable, Sendable {
    public typealias ID = UUID

    public var id: ID
    public var label: String
    public var host: String
    public var port: UInt16?
    public var user: String
    public var sessionID: String?
    public var lastKnownRevision: UInt64

    public init(
        id: ID = UUID(),
        label: String,
        host: String,
        port: UInt16? = nil,
        user: String,
        sessionID: String? = nil,
        lastKnownRevision: UInt64 = 0
    ) {
        self.id = id
        self.label = label
        self.host = host
        self.port = port
        self.user = user
        self.sessionID = sessionID
        self.lastKnownRevision = lastKnownRevision
    }
}

public struct ConnectDraft: Identifiable, Equatable, Sendable {
    public var id: UUID
    public var label: String
    public var host: String
    public var port: String
    public var user: String

    public init(
        id: UUID = UUID(),
        label: String = "",
        host: String = "",
        port: String = "",
        user: String = ""
    ) {
        self.id = id
        self.label = label
        self.host = host
        self.port = port
        self.user = user
    }

    public var validationMessage: String? {
        if normalizedLabel.isEmpty {
            return "Enter a label."
        }
        if normalizedHost.isEmpty {
            return "Enter a host."
        }
        if normalizedUser.isEmpty {
            return "Enter a remote user."
        }
        if normalizedPort.isEmpty == false,
           (UInt16(normalizedPort) == nil || UInt16(normalizedPort) == 0) {
            return "Port must be a number from 1 through 65535."
        }
        return nil
    }

    fileprivate var savedConnection: SavedConnection? {
        guard validationMessage == nil else { return nil }
        return SavedConnection(
            id: id,
            label: normalizedLabel,
            host: normalizedHost,
            port: normalizedPort.isEmpty ? nil : UInt16(normalizedPort),
            user: normalizedUser
        )
    }

    private var normalizedLabel: String {
        label.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var normalizedHost: String {
        host.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var normalizedPort: String {
        port.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var normalizedUser: String {
        user.trimmingCharacters(in: .whitespacesAndNewlines)
    }
}

public enum ConnectionStatus: Equatable, Sendable {
    case unknown
    case connecting
    case resynchronizing
    case connected
    case disconnected(resumeAvailable: Bool)

    public var displayText: String {
        switch self {
        case .unknown:
            return "unknown"
        case .connecting:
            return "connecting"
        case .resynchronizing:
            return "resynchronizing"
        case .connected:
            return "connected"
        case .disconnected(resumeAvailable: true):
            return "disconnected — resume will be attempted"
        case .disconnected(resumeAvailable: false):
            return "disconnected"
        }
    }
}

public struct ConnectionAlert: Identifiable, Equatable, Sendable {
    public enum Kind: Equatable, Sendable {
        case invalidInput
        case connectionFailed
        case sessionReplaced
        case persistenceFailed
    }

    public let id: UUID
    public let kind: Kind
    public let connectionID: SavedConnection.ID?
    public let title: String
    public let message: String

    public init(
        id: UUID = UUID(),
        kind: Kind,
        connectionID: SavedConnection.ID? = nil,
        title: String,
        message: String
    ) {
        self.id = id
        self.kind = kind
        self.connectionID = connectionID
        self.title = title
        self.message = message
    }
}

public actor SavedConnectionStore {
    public nonisolated let url: URL

    public static var defaultURL: URL {
        let applicationSupport = FileManager.default.urls(
            for: .applicationSupportDirectory,
            in: .userDomainMask
        ).first ?? FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/Application Support", isDirectory: true)
        return applicationSupport
            .appendingPathComponent("SRUI", isDirectory: true)
            .appendingPathComponent("saved-connections.json", isDirectory: false)
    }

    public init(url: URL = SavedConnectionStore.defaultURL) {
        self.url = url
    }

    public func load() throws -> [SavedConnection] {
        guard FileManager.default.fileExists(atPath: url.path) else { return [] }
        let data = try Data(contentsOf: url)
        return try JSONDecoder().decode([SavedConnection].self, from: data)
    }

    public func save(_ connections: [SavedConnection]) throws {
        let directory = url.deletingLastPathComponent()
        try FileManager.default.createDirectory(
            at: directory,
            withIntermediateDirectories: true
        )
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        let data = try encoder.encode(connections)
        try data.write(to: url, options: .atomic)
    }
}

enum ConnectionAttemptEvent: Equatable, Sendable {
    case connecting
    case resynchronizing
    case replaced(previousSessionID: String?, newSessionID: String)
    case ready(sessionID: String, revision: UInt64)
    case failed(String)
    case stopped
}

protocol ConnectionAttempt: Sendable {
    var events: AsyncStream<ConnectionAttemptEvent> { get }
    func start() async throws
    func stop() async
}

@MainActor
final class ConnectionSessionContext {
    let applier = TransactionApplier()
    let outbox = EventOutbox()
    let resourceCache = ResourceCache()
    let transactionIngressGate = TransactionIngressGate()
    let continuityContext = SessionContinuityContext()
    let renderer = AppKitRenderer()
    var hasStartedAttempt = false

    private var revisionReadySessionID: String?

    func invalidatePersistableRevision() {
        revisionReadySessionID = nil
    }

    func markPersistableRevisionReady(for sessionID: String) {
        revisionReadySessionID = sessionID
    }

    func persistableRevision(for sessionID: String?) -> UInt64? {
        guard let sessionID, revisionReadySessionID == sessionID else {
            return nil
        }
        return applier.lastAppliedRevision.value
    }
}

@MainActor
struct ConnectionAttemptRequest {
    let connection: SavedConnection
    let configuration: SSHConfiguration
    let context: ConnectionSessionContext
    let isWarmReconnect: Bool

    var requestedSessionID: String? {
        connection.sessionID
    }

    var resumeRevision: UInt64 {
        context.applier.lastAppliedRevision.value
    }
}

typealias ConnectionAttemptFactory = @MainActor (ConnectionAttemptRequest) -> any ConnectionAttempt

private actor SessionControllerConnectionAttempt: ConnectionAttempt {
    nonisolated let events: AsyncStream<ConnectionAttemptEvent>

    private let continuation: AsyncStream<ConnectionAttemptEvent>.Continuation
    private let controller: SessionController
    private var forwardingTask: Task<Void, Never>?

    init(controller: SessionController) {
        let pair = AsyncStream.makeStream(
            of: ConnectionAttemptEvent.self,
            bufferingPolicy: .bufferingNewest(16)
        )
        self.events = pair.stream
        self.continuation = pair.continuation
        self.controller = controller
    }

    func start() async throws {
        try Task.checkCancellation()
        guard forwardingTask == nil else { return }

        let lifecycleEvents = controller.lifecycleEvents
        let continuation = self.continuation
        forwardingTask = Task {
            lifecycleLoop: for await event in lifecycleEvents {
                switch event {
                case .connecting:
                    continuation.yield(.connecting)
                case .resynchronizing(let resynchronization):
                    switch resynchronization {
                    case .fresh(_), .sameSession(_):
                        continuation.yield(.resynchronizing)
                    case .replaced(let previousSessionID, let newSessionID):
                        continuation.yield(.replaced(
                            previousSessionID: previousSessionID,
                            newSessionID: newSessionID
                        ))
                    }
                case .ready(let sessionID, let revision):
                    continuation.yield(.ready(sessionID: sessionID, revision: revision))
                case .failed(let failure):
                    continuation.yield(.failed(failure.description))
                case .stopped:
                    continuation.yield(.stopped)
                    break lifecycleLoop
                }
            }
            continuation.finish()
        }

        try Task.checkCancellation()
        try await controller.start()
    }

    func stop() async {
        await controller.stop()
        let forwardingTask = self.forwardingTask
        forwardingTask?.cancel()
        continuation.finish()
        await forwardingTask?.value
        self.forwardingTask = nil
    }
}

@MainActor
@Observable
public final class ConnectionManager {
    public private(set) var entries: [SavedConnection] = []
    public private(set) var statuses: [SavedConnection.ID: ConnectionStatus] = [:]
    public private(set) var alert: ConnectionAlert?

    @ObservationIgnored private let store: SavedConnectionStore
    @ObservationIgnored private let attemptFactory: ConnectionAttemptFactory
    @ObservationIgnored private var contexts: [SavedConnection.ID: ConnectionSessionContext] = [:]
    @ObservationIgnored private var activeTokens: [SavedConnection.ID: UInt64] = [:]
    @ObservationIgnored private var attemptsByToken: [UInt64: any ConnectionAttempt] = [:]
    @ObservationIgnored private var tasksByToken: [UInt64: Task<Void, Never>] = [:]
    @ObservationIgnored private var pendingFirstSuccess: Set<SavedConnection.ID> = []
    @ObservationIgnored private var nextAttemptToken: UInt64 = 0
    @ObservationIgnored private var isShuttingDown = false

    public init(store: SavedConnectionStore = SavedConnectionStore()) {
        self.store = store
        self.attemptFactory = { request in
            Self.makeSessionControllerAttempt(
                request: request,
                transport: SSHTransport(configuration: request.configuration)
            )
        }
    }

    init(
        store: SavedConnectionStore,
        attemptFactory: @escaping ConnectionAttemptFactory
    ) {
        self.store = store
        self.attemptFactory = attemptFactory
    }

    public func load() async {
        guard isShuttingDown == false else { return }
        do {
            let loaded = try await store.load()
            entries = loaded
            var initialStatuses: [SavedConnection.ID: ConnectionStatus] = [:]
            for entry in loaded {
                initialStatuses[entry.id] = .unknown
            }
            statuses = initialStatuses
            alert = nil
        } catch {
            entries = []
            statuses = [:]
            alert = ConnectionAlert(
                kind: .persistenceFailed,
                title: "Couldn’t Load Connections",
                message: String(describing: error)
            )
        }
    }

    public func status(for connectionID: SavedConnection.ID) -> ConnectionStatus {
        statuses[connectionID] ?? .unknown
    }

    public func dismissAlert() {
        alert = nil
    }

    @discardableResult
    public func connect(_ draft: ConnectDraft) -> SavedConnection.ID? {
        guard isShuttingDown == false else { return nil }
        guard let connection = draft.savedConnection else {
            alert = ConnectionAlert(
                kind: .invalidInput,
                connectionID: draft.id,
                title: "Can’t Connect",
                message: draft.validationMessage ?? "The connection details are invalid."
            )
            return nil
        }
        guard entries.contains(where: { $0.id == connection.id }) == false else {
            alert = ConnectionAlert(
                kind: .invalidInput,
                connectionID: connection.id,
                title: "Can’t Connect",
                message: "A saved connection with this identifier already exists."
            )
            return nil
        }

        entries.append(connection)
        statuses[connection.id] = .unknown
        pendingFirstSuccess.insert(connection.id)
        beginAttempt(for: connection)
        return connection.id
    }

    public func connect(id connectionID: SavedConnection.ID) {
        guard isShuttingDown == false else { return }
        guard let connection = entries.first(where: { $0.id == connectionID }) else {
            alert = ConnectionAlert(
                kind: .invalidInput,
                connectionID: connectionID,
                title: "Can’t Connect",
                message: "This saved connection no longer exists."
            )
            return
        }
        beginAttempt(for: connection)
    }

    public func remove(id connectionID: SavedConnection.ID) async {
        guard isShuttingDown == false else { return }
        guard entries.contains(where: { $0.id == connectionID }) else { return }

        entries.removeAll(where: { $0.id == connectionID })
        statuses.removeValue(forKey: connectionID)
        pendingFirstSuccess.remove(connectionID)

        if let errorMessage = await save(entries) {
            alert = ConnectionAlert(
                kind: .persistenceFailed,
                connectionID: connectionID,
                title: "Couldn’t Delete Connection",
                message: errorMessage
            )
        }
    }

    public func shutdown() async {
        guard isShuttingDown == false else { return }
        isShuttingDown = true

        let tasks = Array(tasksByToken.values)
        let attempts = Array(attemptsByToken.values)
        for task in tasks {
            task.cancel()
        }

        await withTaskGroup(of: Void.self) { group in
            for attempt in attempts {
                group.addTask {
                    await attempt.stop()
                }
            }
        }
        for task in tasks {
            await task.value
        }

        tasksByToken.removeAll()
        attemptsByToken.removeAll()
        activeTokens.removeAll()

        let neverConnected = pendingFirstSuccess
        pendingFirstSuccess.removeAll()
        entries.removeAll { neverConnected.contains($0.id) }
        for connectionID in neverConnected {
            statuses.removeValue(forKey: connectionID)
            contexts.removeValue(forKey: connectionID)
        }

        for index in entries.indices {
            if let context = contexts[entries[index].id],
               let revision = context.persistableRevision(
                   for: entries[index].sessionID
               ) {
                entries[index].lastKnownRevision = revision
            }
            statuses[entries[index].id] = .disconnected(
                resumeAvailable: entries[index].sessionID != nil
            )
        }

        if let errorMessage = await save(entries) {
            alert = ConnectionAlert(
                kind: .persistenceFailed,
                title: "Couldn’t Save Connections",
                message: errorMessage
            )
        }
    }

    static func makeSessionControllerAttempt(
        request: ConnectionAttemptRequest,
        transport: any Transport
    ) -> any ConnectionAttempt {
        let controller = SessionController(
            transport: transport,
            applier: request.context.applier,
            outbox: request.context.outbox,
            renderer: request.context.renderer,
            resourceCache: request.context.resourceCache,
            sessionId: request.requestedSessionID,
            continuityContext: request.context.continuityContext,
            transactionIngressGate: request.context.transactionIngressGate
        )
        controller.attachRenderer(request.context.renderer)
        return SessionControllerConnectionAttempt(controller: controller)
    }

    private func beginAttempt(for connection: SavedConnection) {
        precondition(nextAttemptToken < UInt64.max, "ConnectionManager attempt token exhausted")
        nextAttemptToken += 1
        let token = nextAttemptToken
        let previousToken = activeTokens[connection.id]

        let context: ConnectionSessionContext
        if let existing = contexts[connection.id] {
            context = existing
        } else {
            let created = ConnectionSessionContext()
            contexts[connection.id] = created
            context = created
        }

        let configuration = SSHConfiguration(
            host: connection.host,
            port: connection.port,
            user: connection.user,
            subsystem: "srui",
            identityFile: nil,
            knownHostsFile: nil,
            strictHostKeyChecking: .yes,
            batchMode: true,
            connectTimeout: 30,
            extraOptions: [:],
            sshBinaryPath: "/usr/bin/ssh"
        )
        let request = ConnectionAttemptRequest(
            connection: connection,
            configuration: configuration,
            context: context,
            isWarmReconnect: context.hasStartedAttempt
        )
        context.hasStartedAttempt = true

        let attempt = attemptFactory(request)
        activeTokens[connection.id] = token
        if let previousToken {
            tasksByToken[previousToken]?.cancel()
        }
        attemptsByToken[token] = attempt
        statuses[connection.id] = .connecting
        alert = nil

        let task = Task { [weak self] in
            guard let self else {
                await attempt.stop()
                return
            }
            await self.runAttempt(
                attempt,
                connectionID: connection.id,
                token: token
            )
        }
        tasksByToken[token] = task
    }

    private func runAttempt(
        _ attempt: any ConnectionAttempt,
        connectionID: SavedConnection.ID,
        token: UInt64
    ) async {
        var handledTerminalEvent = false

        do {
            guard activeTokens[connectionID] == token else {
                throw CancellationError()
            }
            try Task.checkCancellation()
            try await attempt.start()
            try Task.checkCancellation()

            for await event in attempt.events {
                try Task.checkCancellation()
                if await handle(
                    event,
                    connectionID: connectionID,
                    token: token
                ) {
                    handledTerminalEvent = true
                    break
                }
            }
        } catch is CancellationError {
            // Supersession and application shutdown are ordinary ownership transitions.
        } catch {
            handledTerminalEvent = await recordFailure(
                String(describing: error),
                connectionID: connectionID,
                token: token
            )
        }

        if handledTerminalEvent == false,
           Task.isCancelled == false,
           activeTokens[connectionID] == token {
            await recordStopped(connectionID: connectionID, token: token)
        }

        await attempt.stop()
        attemptsByToken.removeValue(forKey: token)
        tasksByToken.removeValue(forKey: token)
        if activeTokens[connectionID] == token {
            activeTokens.removeValue(forKey: connectionID)
            if entries.contains(where: { $0.id == connectionID }) == false {
                contexts.removeValue(forKey: connectionID)
            }
        }
    }

    private func handle(
        _ event: ConnectionAttemptEvent,
        connectionID: SavedConnection.ID,
        token: UInt64
    ) async -> Bool {
        guard activeTokens[connectionID] == token else { return true }
        guard let index = entries.firstIndex(where: { $0.id == connectionID }) else {
            switch event {
            case .failed, .stopped:
                return true
            case .connecting, .resynchronizing, .replaced, .ready:
                return false
            }
        }

        switch event {
        case .connecting:
            statuses[connectionID] = .connecting
            return false

        case .resynchronizing:
            contexts[connectionID]?.invalidatePersistableRevision()
            statuses[connectionID] = .resynchronizing
            return false

        case .replaced(let previousSessionID, let newSessionID):
            contexts[connectionID]?.invalidatePersistableRevision()
            entries[index].sessionID = newSessionID
            entries[index].lastKnownRevision = 0
            statuses[connectionID] = .resynchronizing
            let persistenceError = await save(entries)
            guard activeTokens[connectionID] == token else { return true }

            var message: String
            if let previousSessionID {
                message = "Remote session \(previousSessionID) was replaced by \(newSessionID). "
            } else {
                message = "The saved remote session was replaced by \(newSessionID). "
            }
            message += "SRUI is rebuilding from authoritative state."
            if let persistenceError {
                message += "\n\nThe replacement could not be saved: \(persistenceError)"
            }
            alert = ConnectionAlert(
                kind: .sessionReplaced,
                connectionID: connectionID,
                title: "Session Replaced",
                message: message
            )
            return false

        case .ready(let sessionID, let revision):
            entries[index].sessionID = sessionID
            entries[index].lastKnownRevision = revision
            contexts[connectionID]?.markPersistableRevisionReady(for: sessionID)
            pendingFirstSuccess.remove(connectionID)
            statuses[connectionID] = .connected
            let persistenceError = await save(entries)
            guard activeTokens[connectionID] == token else { return true }
            if let persistenceError {
                alert = ConnectionAlert(
                    kind: .persistenceFailed,
                    connectionID: connectionID,
                    title: "Couldn’t Save Connection",
                    message: persistenceError
                )
            }
            return false

        case .failed(let message):
            return await recordFailure(
                message,
                connectionID: connectionID,
                token: token
            )

        case .stopped:
            await recordStopped(connectionID: connectionID, token: token)
            return true
        }
    }

    @discardableResult
    private func recordFailure(
        _ message: String,
        connectionID: SavedConnection.ID,
        token: UInt64
    ) async -> Bool {
        guard activeTokens[connectionID] == token else { return true }

        let persistenceError = await markDisconnected(
            connectionID: connectionID,
            token: token
        )
        guard activeTokens[connectionID] == token else { return true }

        var fullMessage = message
        if let persistenceError {
            fullMessage += "\n\nThe latest connection metadata could not be saved: \(persistenceError)"
        }
        alert = ConnectionAlert(
            kind: .connectionFailed,
            connectionID: connectionID,
            title: "Connection Failed",
            message: fullMessage
        )
        return true
    }

    private func recordStopped(
        connectionID: SavedConnection.ID,
        token: UInt64
    ) async {
        guard activeTokens[connectionID] == token else { return }
        if let persistenceError = await markDisconnected(
            connectionID: connectionID,
            token: token
        ), activeTokens[connectionID] == token {
            alert = ConnectionAlert(
                kind: .persistenceFailed,
                connectionID: connectionID,
                title: "Couldn’t Save Connection",
                message: persistenceError
            )
        }
    }

    private func markDisconnected(
        connectionID: SavedConnection.ID,
        token: UInt64
    ) async -> String? {
        guard activeTokens[connectionID] == token else { return nil }
        guard let index = entries.firstIndex(where: { $0.id == connectionID }) else {
            return nil
        }

        let wasPending = pendingFirstSuccess.remove(connectionID) != nil
        if wasPending {
            entries.remove(at: index)
            statuses.removeValue(forKey: connectionID)
            contexts.removeValue(forKey: connectionID)
        } else {
            if let context = contexts[connectionID],
               let revision = context.persistableRevision(
                   for: entries[index].sessionID
               ) {
                entries[index].lastKnownRevision = revision
            }
            statuses[connectionID] = .disconnected(
                resumeAvailable: entries[index].sessionID != nil
            )
        }
        return await save(entries)
    }

    private func save(_ snapshot: [SavedConnection]) async -> String? {
        do {
            try await store.save(snapshot)
            return nil
        } catch {
            return String(describing: error)
        }
    }
}
