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
// PX-023: the Connect form's endpoint is validated by `ConnectionEndpoint` before it becomes `ssh`
// argv, and a saved entry is validated again before every attempt. Host-key verification stays
// OpenSSH's (`StrictHostKeyChecking=yes`, `BatchMode=yes`, the user's own known_hosts, §19.1, §25),
// and every failure is a blocking "Connection Failed" alert with no trust-anyway path.
//
// PX-023 round 5: no alert content is derived from `ssh`'s output, which a peer can shape. When
// `ssh` exits 255 before any SRUI byte (`SSHTransport.localFailureBeforeStream()`), the alert
// quotes its stderr verbatim under a label saying it can include the server's words, and adds
// only a `KnownHostsHint` computed from the local known_hosts files. The app writes no host-key
// verdict and no trust-changing advice.
//
// PX-024: the saved-connections file (versioned; an unreadable one is moved aside, never written
// over) holds only the endpoint, its local label, the last session ID (validated on load and
// before any server-reported ID is stored) and that revision as presentation metadata. Nothing
// in it is continuity state: a relaunched manager builds a fresh `ConnectionSessionContext` (new
// outbox, so a new `client_instance_id`; an empty replica). Until a session has been ready in a
// context, it attaches with `CLIENT_HELLO`, so ready follows the WELCOME catch-up snapshot and
// the window shows the complete current state; a WELCOME naming another session is reported as
// the saved session's replacement. Removing an entry forgets it locally and sends nothing.
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

    /// The Connections row's caption for a remembered session (PX-024). `lastKnownRevision` is
    /// presentation metadata only (§18): after a relaunch no committed replica stands behind it,
    /// so the caption says when the session was last seen and never offers it as a resume point.
    var sessionCaption: String? {
        sessionID.map { "Session \($0) · last seen at revision \(lastKnownRevision)" }
    }

    /// The most Unicode scalars a session ID may have to be stored, shown or compared (PX-024
    /// round 2). `session_id` is an opaque token (§17); `srui-sessiond` mints 32 lowercase hex
    /// characters (`mint_session_id`). The bound leaves four times that for another server's
    /// format (a 36-character UUID string, a prefixed token) and refuses anything longer.
    static let maximumSessionIDScalars = 128

    /// `id` if it may be stored, shown in the caption and sent back as a session ID, else `nil`:
    /// non-empty, at most `maximumSessionIDScalars` scalars, and free of every control or
    /// invisible scalar `ConnectionEndpoint` refuses in a field (NUL, newline, ESC, format,
    /// separator, surrogate, private-use, unassigned). Applied to the saved file on load and to
    /// every session ID the server reports before it is stored or shown.
    static func validSessionID(_ id: String) -> String? {
        guard !id.isEmpty,
              id.utf8.count <= ConnectionEndpoint.maximumRawFieldBytes,
              id.unicodeScalars.count <= maximumSessionIDScalars,
              ConnectionEndpoint.firstDisallowedScalar(in: id) == nil else {
            return nil
        }
        return id
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

    /// The endpoint the fields name, or why they name none (PX-023).
    public var endpoint: Result<ConnectionEndpoint, ConnectionEndpoint.ParseError> {
        ConnectionEndpoint.parse(host: host, user: user, port: port)
    }

    public var validationMessage: String? {
        switch endpoint {
        case .success:
            return ConnectionEndpoint.validate(label: label)?.description
        case .failure(let error):
            return error.description
        }
    }

    /// The saved entry for a valid draft. An empty name defaults to the endpoint itself.
    var savedConnection: SavedConnection? {
        guard validationMessage == nil, case .success(let endpoint) = endpoint else { return nil }
        return SavedConnection(
            id: id,
            label: normalizedLabel.isEmpty ? endpoint.displayString : normalizedLabel,
            host: endpoint.host,
            port: endpoint.port,
            user: endpoint.user
        )
    }

    private var normalizedLabel: String {
        ConnectionEndpoint.trimmed(label)
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

    /// The file this build writes (PX-024 round 2): `{"connections": [...], "version": 1}`. A
    /// file that is a bare JSON array is the pre-versioned form of version 1 and still loads.
    public static let currentVersion = 1

    /// Why `load()` refused the file. Nothing is ever written over a file `load()` refused: it is
    /// first moved aside to `backupURL`, replacing any earlier backup (one file, never more).
    public enum LoadError: Error, CustomStringConvertible, Equatable, Sendable {
        /// The file was moved to `backup`; the next save writes a fresh file in its place.
        case movedAside(reason: String, backup: URL)
        /// The move itself failed, so the file is still at `file` and this store refuses to save.
        case leftInPlace(reason: String, file: URL)

        public var description: String {
            switch self {
            case .movedAside(let reason, let backup):
                return "The saved connections file could not be read (\(reason)). It was moved to "
                    + "\(backup.path) and SRUI will not write over it; SRUI starts with no saved connections."
            case .leftInPlace(let reason, let file):
                return "The saved connections file could not be read (\(reason)) and could not be moved "
                    + "aside, so SRUI will not save connections over \(file.path). Move it away and relaunch."
            }
        }
    }

    /// Refused by `save` while an unreadable file could not be moved aside.
    public struct UnreadableFileInPlace: Error, CustomStringConvertible, Sendable {
        public let file: URL
        public var description: String {
            "\(file.path) could not be read or moved aside; SRUI will not write over it."
        }
    }

    /// The single fixed backup name next to the file (`<name>.unreadable`).
    public nonisolated var backupURL: URL {
        url.appendingPathExtension("unreadable")
    }

    private var unreadableFileInPlace = false

    private struct VersionedFile: Codable {
        var version: Int
        var connections: [SavedConnection]
    }

    private struct VersionHeader: Decodable {
        var version: Int
    }

    private struct FormatError: Error, CustomStringConvertible {
        let description: String
    }

    /// The saved entries, normalised (`normalized(_:)`). A file that cannot be read as saved
    /// connections (truncated, another shape, an out-of-range number, an unknown version) is
    /// moved aside before this throws, so no later save can destroy it.
    public func load() throws -> [SavedConnection] {
        guard FileManager.default.fileExists(atPath: url.path) else { return [] }
        let loaded: [SavedConnection]
        do {
            loaded = try Self.decode(Data(contentsOf: url))
        } catch {
            let reason = String(String(describing: error).prefix(400))
            if Self.moveAside(url, to: backupURL) {
                unreadableFileInPlace = false
                throw LoadError.movedAside(reason: reason, backup: backupURL)
            }
            unreadableFileInPlace = true
            throw LoadError.leftInPlace(reason: reason, file: url)
        }
        unreadableFileInPlace = false
        return Self.normalized(loaded)
    }

    /// One `rename(2)`: atomic, and it replaces an earlier backup, so at most one is kept.
    private static func moveAside(_ file: URL, to backup: URL) -> Bool {
        rename(file.path, backup.path) == 0
    }

    static func decode(_ data: Data) throws -> [SavedConnection] {
        let isJSONWhitespace: (UInt8) -> Bool = { $0 == 0x20 || $0 == 0x09 || $0 == 0x0A || $0 == 0x0D }
        let decoder = JSONDecoder()
        switch data.first(where: { !isJSONWhitespace($0) }) {
        case UInt8(ascii: "["):
            return try decoder.decode([SavedConnection].self, from: data)
        case UInt8(ascii: "{"):
            let version = try decoder.decode(VersionHeader.self, from: data).version
            guard version == currentVersion else {
                throw FormatError(description: "version \(version) is not one this build reads (\(currentVersion))")
            }
            return try decoder.decode(VersionedFile.self, from: data).connections
        default:
            throw FormatError(description: "not a saved-connections file")
        }
    }

    /// Loaded entries as the manager may use them: the first entry of each ID is kept and later
    /// duplicates are dropped (two rows must never share one session context or one removal),
    /// and a session ID `SavedConnection.validSessionID` refuses is dropped with its revision,
    /// so the entry keeps its endpoint and label and has no remembered session.
    static func normalized(_ loaded: [SavedConnection]) -> [SavedConnection] {
        var seen = Set<SavedConnection.ID>()
        var result: [SavedConnection] = []
        for var entry in loaded {
            guard seen.insert(entry.id).inserted else { continue }
            if let sessionID = entry.sessionID, SavedConnection.validSessionID(sessionID) == nil {
                entry.sessionID = nil
                entry.lastKnownRevision = 0
            }
            result.append(entry)
        }
        return result
    }

    public func save(_ connections: [SavedConnection]) async throws {
        if let beforeSave {
            await beforeSave(connections)
        }
        guard unreadableFileInPlace == false else {
            throw UnreadableFileInPlace(file: url)
        }

        let directory = url.deletingLastPathComponent()
        try FileManager.default.createDirectory(
            at: directory,
            withIntermediateDirectories: true
        )
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        let data = try encoder.encode(VersionedFile(version: Self.currentVersion, connections: connections))
        try data.write(to: url, options: .atomic)
    }
}

enum ConnectionAttemptEvent: Equatable, Sendable {
    case connecting
    case resynchronizing
    case replaced(previousSessionID: String?, newSessionID: String)
    case ready(sessionID: String, revision: UInt64)
    case failed(String)
    /// `ssh` exited 255 before any SRUI byte (PX-023 round 5). `stderr` is quoted verbatim and
    /// never interpreted; `hint` comes only from the local known_hosts files, or is `nil`.
    case sshFailedBeforeStream(stderr: String, hint: KnownHostsHint?)
    case stopped
}

/// Thrown by an attempt's `start()` when `ssh` exited 255 before the handshake could begin.
struct SSHFailedBeforeStream: Error, Equatable, Sendable {
    let stderr: String
    let hint: KnownHostsHint?

    /// `nil` unless `transport` is `ssh` and its child exited 255 before any SRUI byte. Only the
    /// exit itself is the trigger; the hint is computed from local files by `lookup`.
    static func check(_ transport: SSHTransport?, lookup: any KnownHostsLookup) async -> SSHFailedBeforeStream? {
        guard let transport, let local = await transport.localFailureBeforeStream() else {
            return nil
        }
        let hint = await lookup.hint(for: transport.configuration)
        return SSHFailedBeforeStream(stderr: local.stderr, hint: hint)
    }
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
    /// Whether a session has ever been ready in this context (PX-024 round 2). Only then are its
    /// replica and outbox this process's own continuity checkpoint (§18, warm reconnect), so only
    /// then does a reconnect send `CLIENT_RESUME`. A context without one (a cold relaunch, or
    /// attempts that never became ready) attaches with `CLIENT_HELLO`, whose WELCOME catch-up
    /// snapshot is applied before ready, as for any fresh attach.
    private(set) var hasBeenReady = false

    private var revisionReadySessionID: String?

    func invalidatePersistableRevision() {
        revisionReadySessionID = nil
    }

    func markPersistableRevisionReady(for sessionID: String) {
        revisionReadySessionID = sessionID
        hasBeenReady = true
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

    /// The saved session this attempt reattaches to, if any.
    var requestedSessionID: String? {
        connection.sessionID
    }

    /// The session ID the controller resumes (`CLIENT_RESUME`), or `nil` for `CLIENT_HELLO`.
    ///
    /// PX-024 round 2: a saved session is resumed only from a context that has been ready, where
    /// a real committed replica and its outbox exist. Without one, `RESUME_OK` would replay from
    /// revision 1 after a `ready` emitted at revision 0, so the window would read as live over an
    /// empty, then historical, replica; `ServerResumeOk` carries no replay target to wait for.
    /// The documented attach path is used instead (§18 allows, not requires, the cold resume):
    /// HELLO with the fresh identity, ready only after the WELCOME snapshot. A different session
    /// in the WELCOME is reported as a replacement of the saved one.
    var resumeSessionID: String? {
        context.hasBeenReady ? connection.sessionID : nil
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
    /// The transport when it is `ssh`: its exit before the stream triggers the known_hosts hint.
    private let sshTransport: SSHTransport?
    private let knownHostsLookup: any KnownHostsLookup
    /// The saved session a `CLIENT_HELLO` attach stands in for (PX-024 round 2), or `nil`.
    private let rememberedSessionID: String?
    private var forwardingTask: Task<Void, Never>?
    private var stopTask: Task<Void, Never>?

    init(
        controller: SessionController,
        sshTransport: SSHTransport?,
        knownHostsLookup: any KnownHostsLookup,
        rememberedSessionID: String? = nil
    ) {
        self.rememberedSessionID = rememberedSessionID
        let pair = AsyncStream.makeStream(
            of: ConnectionAttemptEvent.self,
            bufferingPolicy: .bufferingNewest(16)
        )
        self.events = pair.stream
        self.continuation = pair.continuation
        self.controller = controller
        self.sshTransport = sshTransport
        self.knownHostsLookup = knownHostsLookup
    }

    func start() async throws {
        try Task.checkCancellation()
        guard forwardingTask == nil else { return }

        let lifecycleEvents = controller.lifecycleEvents
        let sshTransport = self.sshTransport
        let knownHostsLookup = self.knownHostsLookup
        let continuation = self.continuation
        let remembered = self.rememberedSessionID
        forwardingTask = Task {
            // A HELLO attach of a remembered session: the server attaches this client to the one
            // session it serves. A different ID is a replacement of the saved one, reported as
            // such before its snapshot is ready, exactly as a REPLACED resume decision is (§18).
            var attached = false
            func attach(_ sessionID: String) -> Bool {
                defer { attached = true }
                guard !attached, let remembered, remembered != sessionID else { return false }
                continuation.yield(.replaced(previousSessionID: remembered, newSessionID: sessionID))
                return true
            }
            lifecycleLoop: for await event in lifecycleEvents {
                switch event {
                case .connecting:
                    continuation.yield(.connecting)
                case .resynchronizing(let resynchronization):
                    switch resynchronization {
                    case .fresh(let sessionID):
                        if !attach(sessionID) {
                            continuation.yield(.resynchronizing)
                        }
                    case .sameSession(_):
                        continuation.yield(.resynchronizing)
                    case .replaced(let previousSessionID, let newSessionID):
                        attached = true
                        continuation.yield(.replaced(
                            previousSessionID: previousSessionID,
                            newSessionID: newSessionID
                        ))
                    }
                case .ready(let sessionID, let revision):
                    _ = attach(sessionID)
                    continuation.yield(.ready(sessionID: sessionID, revision: revision))
                case .failed(let failure):
                    if let local = await SSHFailedBeforeStream.check(sshTransport, lookup: knownHostsLookup) {
                        continuation.yield(.sshFailedBeforeStream(stderr: local.stderr, hint: local.hint))
                    } else {
                        continuation.yield(.failed(failure.description))
                    }
                case .stopped:
                    continuation.yield(.stopped)
                    break lifecycleLoop
                }
            }
            continuation.finish()
        }

        try Task.checkCancellation()
        do {
            try await controller.start()
        } catch is CancellationError {
            throw CancellationError()
        } catch {
            if let local = await SSHFailedBeforeStream.check(sshTransport, lookup: knownHostsLookup) {
                throw local
            }
            throw error
        }
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
        // The saved-connections file is local input that a cold launch connects from without the
        // form, so it passes the same endpoint validation before it can become `ssh` argv (PX-023).
        if let error = ConnectionEndpoint.validate(
            savedHost: connection.host,
            user: connection.user,
            port: connection.port
        ) {
            presentAlert(ConnectionAlert(
                kind: .invalidInput,
                connectionID: connectionID,
                title: "Can’t Connect",
                message: "This saved connection’s endpoint is invalid: \(error.description) Remove it and add it again."
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
        knownHostsLookup: any KnownHostsLookup = SSHKnownHostsLookup(),
        configureController: ((SessionController) -> Void)? = nil
    ) -> any ConnectionAttempt {
        let controller = SessionController(
            transport: transport,
            applier: request.context.applier,
            outbox: request.context.outbox,
            renderer: request.context.renderer,
            resourceCache: request.context.resourceCache,
            sessionId: request.resumeSessionID,
            continuityContext: request.context.continuityContext,
            transactionIngressGate: request.context.transactionIngressGate
        )
        controller.attachRenderer(request.context.renderer)
        configureController?(controller)
        return SessionControllerConnectionAttempt(
            controller: controller,
            sshTransport: transport as? SSHTransport,
            knownHostsLookup: knownHostsLookup,
            rememberedSessionID: request.resumeSessionID == nil ? request.requestedSessionID : nil
        )
    }

    /// The one SSH configuration every connection uses, whatever application serves the remote
    /// `srui` subsystem (PX-023: counter and srtop alike). The remote app never chooses any of it.
    /// Identity, agent and known_hosts are the user's own OpenSSH setup (§19.1); host keys are
    /// checked strictly and never prompted for, so an unknown or changed key fails the attempt.
    nonisolated static func sshConfiguration(for connection: SavedConnection) -> SSHConfiguration {
        SSHConfiguration(
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

        let configuration = Self.sshConfiguration(for: connection)
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
        } catch let local as SSHFailedBeforeStream {
            handledTerminalEvent = await recordFailure(
                local.stderr,
                connectionID: connectionID,
                token: token,
                sshHint: .some(local.hint)
            )
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
            case .failed, .sshFailedBeforeStream, .stopped:
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
            // A server-reported ID is stored and shown only if it passes the saved file's rule.
            let replacement = SavedConnection.validSessionID(newSessionID)
            entries[index].sessionID = replacement
            entries[index].lastKnownRevision = 0
            setStatus(.resynchronizing, for: connectionID, replaced: true)
            let persistenceError = await save(entries)
            guard activeTokens[connectionID] == token else { return true }

            let newName = replacement ?? "a session with an invalid ID"
            var message: String
            if let previous = previousSessionID.flatMap(SavedConnection.validSessionID) {
                message = "Remote session \(previous) was replaced by \(newName). "
            } else {
                message = "The saved remote session was replaced by \(newName). "
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
            let stored = SavedConnection.validSessionID(sessionID)
            entries[index].sessionID = stored
            entries[index].lastKnownRevision = stored == nil ? 0 : revision
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

        case .sshFailedBeforeStream(let stderr, let hint):
            return await recordFailure(
                stderr,
                connectionID: connectionID,
                token: token,
                sshHint: .some(hint)
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
        token: UInt64,
        sshHint: KnownHostsHint?? = nil
) async -> Bool {
        guard activeTokens[connectionID] == token else { return true }

        // Read before `markDisconnected`, which forgets a draft that never connected. For an
        // `ssh` exit before the stream (`sshHint` set, its inner value possibly nil), `message` is
        // `ssh`'s stderr, quoted verbatim; nothing in it decides anything in the alert.
        let sshMessage = sshHint.map { hint in
            Self.sshFailureMessage(
                endpoint: entries.first(where: { $0.id == connectionID }).map {
                    ConnectionEndpoint.displayString(host: $0.host, port: $0.port, user: $0.user)
                },
                hint: hint,
                stderr: message
            )
        }

        let persistenceError = await markDisconnected(
            connectionID: connectionID,
            token: token
        )
        guard activeTokens[connectionID] == token else { return true }

        var fullMessage = sshMessage ?? message
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

    /// The label above `ssh`'s quoted stderr: what follows is not SRUI's and may be the server's.
    nonisolated static let sshOutputLabel = "ssh reported (this text can include messages from the server):"

    /// The alert text for an `ssh` exit before the stream. Everything SRUI writes depends only on
    /// the endpoint and the local known_hosts hint; `stderr` is appended verbatim after the label.
    nonisolated static func sshFailureMessage(endpoint: String?, hint: KnownHostsHint?, stderr: String) -> String {
        var text = endpoint.map { "SRUI could not connect to \($0)." } ?? "SRUI could not connect."
        if let hint {
            text += "\n\n" + hint.text
        }
        return text + "\n\n" + sshOutputLabel + "\n" + stderr
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
