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
// Every window of a session shows the connection status as client-owned chrome (PX-022-G01): stale
// while detached or catching up, rebuilding after a REPLACED decision, and cleared once ready. It is
// written only with the status, by the current attempt generation (§18), never from a server claim.
// A dropped session context closes its windows, so none is left on screen without that status.
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

enum ConnectionPrimaryAction: Equatable, Sendable {
    case connect
    case open
    case unavailable

    var title: String {
        switch self {
        case .connect, .unavailable:
            return "Connect"
        case .open:
            return "Open"
        }
    }

    var isEnabled: Bool {
        self != .unavailable
    }
}

public enum ConnectionStatus: Equatable, Sendable {
    case unknown
    case connecting
    case resynchronizing
    case connected
    case disconnected(resumeAvailable: Bool)

    var primaryAction: ConnectionPrimaryAction {
        switch self {
        case .unknown, .disconnected:
            return .connect
        case .connected:
            return .open
        case .connecting, .resynchronizing:
            return .unavailable
        }
    }

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

    /// Client-owned chrome every window of the session shows for this status, or `nil` for none
    /// (PX-022-G01). It is derived only from the client's own transport state, never from a server
    /// property. While the transport is not delivering live state (§17 DETACHED, or the catch-up
    /// that follows a §18 continuity decision) the window still shows the replica at
    /// `lastAppliedRevision`, so the text says that data is stale.
    func sessionWindowStatus(lastAppliedRevision: UInt64) -> String? {
        switch self {
        case .unknown, .connected:
            return nil
        case .connecting:
            return "connecting — showing data from revision \(lastAppliedRevision) (stale)"
        case .resynchronizing:
            return "resynchronizing — showing data from revision \(lastAppliedRevision) (stale)"
        case .disconnected:
            return "disconnected — showing data from revision \(lastAppliedRevision) (stale)"
        }
    }

    /// Window chrome from a REPLACED continuity decision until its snapshot is ready (§18): the old
    /// incarnation's replica is discarded, so there is no revision to call stale.
    static let sessionReplacedWindowStatus = "session replaced — rebuilding"
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

    private let beforeSave: (@Sendable ([SavedConnection]) async -> Void)?

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
        self.beforeSave = nil
    }

    init(
        url: URL,
        beforeSave: @escaping @Sendable ([SavedConnection]) async -> Void
    ) {
        self.url = url
        self.beforeSave = beforeSave
    }

    public func load() throws -> [SavedConnection] {
        guard FileManager.default.fileExists(atPath: url.path) else { return [] }
        let data = try Data(contentsOf: url)
        return try JSONDecoder().decode([SavedConnection].self, from: data)
    }

    public func save(_ connections: [SavedConnection]) async throws {
        if let beforeSave {
            await beforeSave(connections)
        }

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
    private var stopTask: Task<Void, Never>?

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

    /// Stops the controller once; every caller awaits that same stop to its end.
    ///
    /// `SessionController.stop()` returns at once while another stop is running, and `shutdown()`
    /// stops attempts concurrently with their own `runAttempt` tail. Without one shared stop, the
    /// tail could return from `stop()` while the controller still renders, and close a dropped
    /// context's windows before a render in flight reopens them (PX-022-G01).
    func stop() async {
        if stopTask == nil {
            stopTask = Task { await self.performStop() }
        }
        await stopTask?.value
    }

    private func performStop() async {
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
    @ObservationIgnored private var persistenceTail: Task<String?, Never>?
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
            presentAlert(ConnectionAlert(
                kind: .persistenceFailed,
                title: "Couldn’t Load Connections",
                message: String(describing: error)
            ))
        }
    }

    public func status(for connectionID: SavedConnection.ID) -> ConnectionStatus {
        statuses[connectionID] ?? .unknown
    }

    public func dismissAlert() {
        alert = nil
    }

    private func presentAlert(_ candidate: ConnectionAlert) {
        guard let current = alert, current.kind == .sessionReplaced else {
            alert = candidate
            return
        }

        let detail: String
        if candidate.kind == .connectionFailed,
           candidate.connectionID == current.connectionID {
            detail = "The replacement connection then failed: \(candidate.message)"
        } else {
            detail = "\(candidate.title): \(candidate.message)"
        }
        alert = ConnectionAlert(
            id: current.id,
            kind: .sessionReplaced,
            connectionID: current.connectionID,
            title: current.title,
            message: "\(current.message)\n\n\(detail)"
        )
    }

    @discardableResult
    public func connect(_ draft: ConnectDraft) -> SavedConnection.ID? {
        guard isShuttingDown == false else { return nil }
        guard let connection = draft.savedConnection else {
            presentAlert(ConnectionAlert(
                kind: .invalidInput,
                connectionID: draft.id,
                title: "Can’t Connect",
                message: draft.validationMessage ?? "The connection details are invalid."
            ))
            return nil
        }
        guard entries.contains(where: { $0.id == connection.id }) == false else {
            presentAlert(ConnectionAlert(
                kind: .invalidInput,
                connectionID: connection.id,
                title: "Can’t Connect",
                message: "A saved connection with this identifier already exists."
            ))
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
            presentAlert(ConnectionAlert(
                kind: .invalidInput,
                connectionID: connectionID,
                title: "Can’t Connect",
                message: "This saved connection no longer exists."
            ))
            return
        }
        beginAttempt(for: connection)
    }

    @discardableResult
    public func open(id connectionID: SavedConnection.ID) -> Bool {
        guard isShuttingDown == false,
              statuses[connectionID] == .connected,
              let context = contexts[connectionID] else {
            return false
        }

        context.renderer.showWindows()
        return true
    }

    public func remove(id connectionID: SavedConnection.ID) {
        guard isShuttingDown == false else { return }
        guard entries.contains(where: { $0.id == connectionID }) else { return }

        entries.removeAll(where: { $0.id == connectionID })
        statuses.removeValue(forKey: connectionID)
        pendingFirstSuccess.remove(connectionID)
        if activeTokens[connectionID] == nil {
            discardContext(for: connectionID)
        }

        enqueueSave(entries) { [weak self] errorMessage in
            guard let self, let errorMessage else { return }
            self.presentAlert(ConnectionAlert(
                kind: .persistenceFailed,
                connectionID: connectionID,
                title: "Couldn’t Delete Connection",
                message: errorMessage
            ))
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
            discardContext(for: connectionID)
        }

        for index in entries.indices {
            if let context = contexts[entries[index].id],
               let revision = context.persistableRevision(
                   for: entries[index].sessionID
               ) {
                entries[index].lastKnownRevision = revision
            }
            setStatus(
                .disconnected(resumeAvailable: entries[index].sessionID != nil),
                for: entries[index].id
            )
        }

        if let errorMessage = await save(entries) {
            presentAlert(ConnectionAlert(
                kind: .persistenceFailed,
                title: "Couldn’t Save Connections",
                message: errorMessage
            ))
        }
    }

    /// `configureController` lets tests install `SessionController` interceptors before start.
    static func makeSessionControllerAttempt(
        request: ConnectionAttemptRequest,
        transport: any Transport,
        configureController: ((SessionController) -> Void)? = nil
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
        configureController?(controller)
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
        setStatus(.connecting, for: connection.id)
        if alert?.kind != .sessionReplaced {
            alert = nil
        }

        let task = Task { [weak self] in
            guard let self else {
                await attempt.stop()
                return
            }
            await self.runAttempt(
                attempt,
                connectionID: connection.id,
                token: token,
                context: context
            )
        }
        tasksByToken[token] = task
    }

    private func runAttempt(
        _ attempt: any ConnectionAttempt,
        connectionID: SavedConnection.ID,
        token: UInt64,
        context: ConnectionSessionContext
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
                discardContext(for: connectionID)
            }
        }
        // Only now can this attempt's controller no longer render into `context` (`stop()` is
        // shared, so it returns only once the controller has finished stopping): a render already
        // in flight when the session failed may have reopened windows `discardContext` closed
        // (PX-022-G01). So a context the manager no longer owns closes its windows again here.
        if contexts[connectionID] !== context {
            closeWindows(of: context)
        }
    }

    private func handle(
        _ event: ConnectionAttemptEvent,
        connectionID: SavedConnection.ID,
        token: UInt64
    ) async -> Bool {
        guard activeTokens[connectionID] == token else { return true }
        guard let index = entries.firstIndex(where: { $0.id == connectionID }) else {
            // A removed entry's session keeps running until its transport ends, and its windows
            // stay open until then, so they keep following the transport (PX-022-G01). When it
            // ends, nothing will reconnect it: its windows close at once, not after the stop
            // (they would read as live meanwhile), and the `runAttempt` tail closes them again.
            switch event {
            case .failed, .stopped:
                discardContext(for: connectionID)
                return true
            case .connecting:
                showWindowStatus(.connecting, for: connectionID)
                return false
            case .resynchronizing:
                showWindowStatus(.resynchronizing, for: connectionID)
                return false
            case .replaced:
                showWindowStatus(.resynchronizing, for: connectionID, replaced: true)
                return false
            case .ready:
                showWindowStatus(.connected, for: connectionID)
                return false
            }
        }

        switch event {
        case .connecting:
            setStatus(.connecting, for: connectionID)
            return false

        case .resynchronizing:
            contexts[connectionID]?.invalidatePersistableRevision()
            setStatus(.resynchronizing, for: connectionID)
            return false

        case .replaced(let previousSessionID, let newSessionID):
            contexts[connectionID]?.invalidatePersistableRevision()
            entries[index].sessionID = newSessionID
            entries[index].lastKnownRevision = 0
            setStatus(.resynchronizing, for: connectionID, replaced: true)
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
            presentAlert(ConnectionAlert(
                kind: .sessionReplaced,
                connectionID: connectionID,
                title: "Session Replaced",
                message: message
            ))
            return false

        case .ready(let sessionID, let revision):
            entries[index].sessionID = sessionID
            entries[index].lastKnownRevision = revision
            contexts[connectionID]?.markPersistableRevisionReady(for: sessionID)
            pendingFirstSuccess.remove(connectionID)
            setStatus(.connected, for: connectionID)
            let persistenceError = await save(entries)
            guard activeTokens[connectionID] == token else { return true }
            if let persistenceError {
                presentAlert(ConnectionAlert(
                    kind: .persistenceFailed,
                    connectionID: connectionID,
                    title: "Couldn’t Save Connection",
                    message: persistenceError
                ))
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
        presentAlert(ConnectionAlert(
            kind: .connectionFailed,
            connectionID: connectionID,
            title: "Connection Failed",
            message: fullMessage
        ))
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
            presentAlert(ConnectionAlert(
                kind: .persistenceFailed,
                connectionID: connectionID,
                title: "Couldn’t Save Connection",
                message: persistenceError
            ))
        }
    }

    private func markDisconnected(
        connectionID: SavedConnection.ID,
        token: UInt64
    ) async -> String? {
        guard activeTokens[connectionID] == token else { return nil }
        guard let index = entries.firstIndex(where: { $0.id == connectionID }) else {
            // The entry was removed while its session ran: the session has now ended, so its
            // windows close at once (PX-022-G01).
            discardContext(for: connectionID)
            return nil
        }

        let wasPending = pendingFirstSuccess.remove(connectionID) != nil
        if wasPending {
            entries.remove(at: index)
            statuses.removeValue(forKey: connectionID)
            discardContext(for: connectionID)
        } else {
            if let context = contexts[connectionID],
               let revision = context.persistableRevision(
                   for: entries[index].sessionID
               ) {
                entries[index].lastKnownRevision = revision
            }
            setStatus(
                .disconnected(resumeAvailable: entries[index].sessionID != nil),
                for: connectionID
            )
        }
        return await save(entries)
    }

    /// Records a connection's status and puts the matching client-owned chrome on every window of
    /// its session (PX-022-G01). Callers have already established that they speak for the current
    /// attempt generation (the `activeTokens` checks), so a superseded attempt's late outcome never
    /// reaches either one (§18, generation-bound resume attempts).
    private func setStatus(
        _ status: ConnectionStatus,
        for connectionID: SavedConnection.ID,
        replaced: Bool = false
    ) {
        statuses[connectionID] = status
        showWindowStatus(status, for: connectionID, replaced: replaced)
    }

    /// Puts the chrome for `status` on every window of the connection's session, if it still has
    /// a context. Used directly only for a removed entry whose attempt is still running: it has no
    /// row status any more, but its windows are still on screen (PX-022-G01).
    ///
    /// N is the applier's `lastAppliedRevision` when the transition happens, not a rendered
    /// revision: it can briefly lead or trail what the window draws, but every non-live state says
    /// `(stale)`, so live data is never claimed.
    private func showWindowStatus(
        _ status: ConnectionStatus,
        for connectionID: SavedConnection.ID,
        replaced: Bool = false
    ) {
        guard let context = contexts[connectionID] else { return }
        context.renderer.setSessionWindowStatus(
            replaced
                ? ConnectionStatus.sessionReplacedWindowStatus
                : status.sessionWindowStatus(
                    lastAppliedRevision: context.applier.lastAppliedRevision.value
                )
        )
    }

    /// Drops a session context and closes its windows.
    ///
    /// A dropped context is never reconnected or reopened: `connect` mints a fresh one and `open`
    /// needs one, and removing an entry only forgets it locally (D2 T38; the remote session lives
    /// on, §17). Its windows must not outlive it. AppKit keeps a shown `NSWindow` alive after its
    /// renderer is released, and nothing else would close it, so the window would stay on screen
    /// frozen, and with its status cleared it would read as a live session. The status is cleared
    /// at once so the renderer holds no transport claim. Pending text in those windows belongs to
    /// the dropped outbox and is not sent (§18.3 covers resume only).
    ///
    /// It runs as soon as nothing can bring the session back for that entry: a failed first
    /// connection, the end of a removed entry's session (its `.failed` or `.stopped`, or any
    /// failure reaching `markDisconnected`), `remove(id:)` with no attempt running, and drafts
    /// that never connected, at shutdown. The windows close at once, so none stays on screen
    /// without its status while the attempt finishes its save and stop. A render already in flight may reopen them after that (a
    /// first connection that fails mid-snapshot), so the attempt's `runAttempt` tail closes the
    /// windows of every context the manager no longer owns again, after its shared `stop()` has
    /// finished and the controller can no longer render.
    private func discardContext(for connectionID: SavedConnection.ID) {
        guard let context = contexts.removeValue(forKey: connectionID) else { return }
        closeWindows(of: context)
    }

    /// Closes every Surface window of a context the manager no longer owns, by mounting an empty
    /// store (`LayoutRenderer.tearDown`), as a REPLACED reset does. Idempotent.
    private func closeWindows(of context: ConnectionSessionContext) {
        context.renderer.setSessionWindowStatus(nil)
        guard context.renderer.registry.isEmpty == false else { return }
        // Mounting a store with no roots registers nothing, so it cannot throw.
        try? context.renderer.attach(store: SemanticStore())
    }

    @discardableResult
    private func enqueueSave(
        _ snapshot: [SavedConnection],
        completion: (@MainActor @Sendable (String?) -> Void)? = nil
    ) -> Task<String?, Never> {
        let pendingConnectionIDs = pendingFirstSuccess
        let persistableSnapshot = snapshot.filter {
            pendingConnectionIDs.contains($0.id) == false
        }
        let predecessor = persistenceTail
        let store = self.store

        let task = Task { @MainActor in
            _ = await predecessor?.value

            let errorMessage: String?
            do {
                try await store.save(persistableSnapshot)
                errorMessage = nil
            } catch {
                errorMessage = String(describing: error)
            }
            completion?(errorMessage)
            return errorMessage
        }
        persistenceTail = task
        return task
    }

    private func save(_ snapshot: [SavedConnection]) async -> String? {
        await enqueueSave(snapshot).value
    }

    func waitForPersistenceForTesting() async {
        _ = await persistenceTail?.value
    }

    func hasActiveAttemptForTesting(_ connectionID: SavedConnection.ID) -> Bool {
        activeTokens[connectionID] != nil
    }

    var isShuttingDownForTesting: Bool {
        isShuttingDown
    }
}
