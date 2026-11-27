//
// ConnectionManagerTests.swift
// SRUITests
//
// Deterministic coverage for client-owned saved connections and reconnect orchestration.
//

import AppKit
import Foundation
@testable import ConnectionManager
import Protocol
@testable import RendererAppKit
import SemanticModel
@testable import Session
import Testing
import TransportSSH

private enum ConnectionManagerTestError: Error, CustomStringConvertible, Sendable {
    case startFailed(String)

    var description: String {
        switch self {
        case .startFailed(let message):
            return message
        }
    }
}

private actor TestConnectionAttempt: ConnectionAttempt {
    nonisolated let events: AsyncStream<ConnectionAttemptEvent>

    private let continuation: AsyncStream<ConnectionAttemptEvent>.Continuation
    private let startGate: AsyncGate?
    private let startError: ConnectionManagerTestError?
    private let stopGate: AsyncGate?
    private var starts = 0
    private var stops = 0

    init(
        startGate: AsyncGate? = nil,
        startError: ConnectionManagerTestError? = nil,
        stopGate: AsyncGate? = nil
    ) {
        let pair = AsyncStream.makeStream(
            of: ConnectionAttemptEvent.self,
            bufferingPolicy: .bufferingNewest(16)
        )
        self.events = pair.stream
        self.continuation = pair.continuation
        self.startGate = startGate
        self.startError = startError
        self.stopGate = stopGate
    }

    func start() async throws {
        starts += 1
        if let startGate {
            await startGate.pause()
        }
        if let startError {
            throw startError
        }
    }

    func stop() async {
        stops += 1
        if let stopGate {
            await stopGate.pause()
        }
        continuation.finish()
    }

    func emit(_ event: ConnectionAttemptEvent) {
        continuation.yield(event)
    }

    func startCount() -> Int {
        starts
    }

    func stopCount() -> Int {
        stops
    }
}

@MainActor
private final class ConnectionAttemptHarness {
    struct StartPlan {
        var gate: AsyncGate?
        var error: ConnectionManagerTestError?
        var stopGate: AsyncGate?
    }

    private(set) var requests: [ConnectionAttemptRequest] = []
    private(set) var attempts: [TestConnectionAttempt] = []
    var startPlans: [StartPlan] = []

    func makeAttempt(_ request: ConnectionAttemptRequest) -> any ConnectionAttempt {
        let plan = startPlans.isEmpty ? StartPlan() : startPlans.removeFirst()
        let attempt = TestConnectionAttempt(
            startGate: plan.gate,
            startError: plan.error,
            stopGate: plan.stopGate
        )
        requests.append(request)
        attempts.append(attempt)
        return attempt
    }

    func discardRequests() {
        requests.removeAll()
    }
}

@MainActor
private final class WeakConnectionSessionContext {
    weak var value: ConnectionSessionContext?

    init(_ value: ConnectionSessionContext) {
        self.value = value
    }
}

private actor ConnectionManagerCaptureTransport: Transport {
    nonisolated let stream: AsyncThrowingStream<Data, Error>

    private let continuation: AsyncThrowingStream<Data, Error>.Continuation
    private var frames: [Data] = []

    init() {
        let pair = AsyncThrowingStream<Data, Error>.makeStream()
        self.stream = pair.stream
        self.continuation = pair.continuation
    }

    func send(data: Data, logicalClass _: LogicalChannelClass) async throws {
        frames.append(data)
    }

    nonisolated func receiveStream() -> AsyncThrowingStream<Data, Error> {
        stream
    }

    func close() async {
        continuation.finish()
    }

    /// Ends the inbound stream with an error, as a dropped connection does.
    func fail() {
        continuation.finish(throwing: ConnectionManagerTestError.startFailed("connection dropped"))
    }

    func frame(at index: Int) -> Data? {
        guard frames.indices.contains(index) else { return nil }
        return frames[index]
    }

    func receive(_ message: SRUIMessage) throws {
        continuation.yield(try SRUIFraming.encodeFramed(message))
    }
}

@MainActor
private final class TransportBox {
    var values: [ConnectionManagerCaptureTransport] = []
    var contexts: [ConnectionSessionContext] = []
}

@MainActor
private final class ConnectionControllerHarness {
    private(set) var requests: [ConnectionAttemptRequest] = []
    private(set) var transports: [ConnectionManagerCaptureTransport] = []

    func makeAttempt(_ request: ConnectionAttemptRequest) -> any ConnectionAttempt {
        let transport = ConnectionManagerCaptureTransport()
        requests.append(request)
        transports.append(transport)
        return ConnectionManager.makeSessionControllerAttempt(request: request, transport: transport)
    }
}

private struct TemporaryConnectionStore {
    let directory: URL
    let file: URL
    let store: SavedConnectionStore

    init() {
        directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("srui-connection-manager-\(UUID().uuidString)", isDirectory: true)
        file = directory.appendingPathComponent("connections.json", isDirectory: false)
        store = SavedConnectionStore(url: file)
    }

    func cleanUp() {
        try? FileManager.default.removeItem(at: directory)
    }
}

private actor PersistenceSaveProbe {
    private let firstSaveGate: AsyncGate
    private var recordedSnapshots: [[SavedConnection]] = []

    init(firstSaveGate: AsyncGate) {
        self.firstSaveGate = firstSaveGate
    }

    func observe(_ snapshot: [SavedConnection]) async {
        recordedSnapshots.append(snapshot)
        if recordedSnapshots.count == 1 {
            await firstSaveGate.pause()
        }
    }

    func snapshots() -> [[SavedConnection]] {
        recordedSnapshots
    }
}

@MainActor
// A parked test must fail, not wedge the run: `AsyncTestSupport.observed` waits have no deadline of
// their own, so this ceiling is what bounds a genuine hang.
@Suite("Connection Manager Tests (§6.3, §17, §18, §19.1)", .timeLimit(.minutes(1)))
struct ConnectionManagerTests {
    private func enterReplacementCatchUp(
        manager: ConnectionManager,
        harness: ConnectionAttemptHarness,
        saved: SavedConnection,
        replacementSessionID: String
    ) async throws -> TestConnectionAttempt {
        let originalSessionID = try #require(saved.sessionID)
        manager.connect(id: saved.id)
        let attempt = try #require(harness.attempts.first)
        let request = try #require(harness.requests.first)
        let appliedRevision = try request.context.applier.apply(
            baseRevision: .initial,
            operations: []
        ).get()
        #expect(appliedRevision == Revision(1))

        await attempt.emit(.ready(
            sessionID: originalSessionID,
            revision: appliedRevision.value
        ))
        try await AsyncTestSupport.observed("original session ready") {
            manager.status(for: saved.id) == .connected
                && manager.entries.first?.lastKnownRevision == appliedRevision.value
        }

        await attempt.emit(.replaced(
            previousSessionID: originalSessionID,
            newSessionID: replacementSessionID
        ))
        try await AsyncTestSupport.observed("replacement catch-up") {
            manager.status(for: saved.id) == .resynchronizing
                && manager.entries.first?.sessionID == replacementSessionID
                && manager.entries.first?.lastKnownRevision == 0
                && manager.alert?.kind == .sessionReplaced
        }
        return attempt
    }

    @Test("connection status selects the correct primary action")
    func connectionStatusSelectsPrimaryAction() {
        #expect(ConnectionStatus.unknown.primaryAction == .connect)
        #expect(ConnectionStatus.connecting.primaryAction == .unavailable)
        #expect(ConnectionStatus.resynchronizing.primaryAction == .unavailable)
        #expect(ConnectionStatus.connected.primaryAction == .open)
        #expect(ConnectionStatus.disconnected(resumeAvailable: true).primaryAction == .connect)
        #expect(ConnectionStatus.disconnected(resumeAvailable: false).primaryAction == .connect)
    }

    @Test("opening a connected renderer does not start another attempt")
    func openingConnectedRendererDoesNotReconnect() async throws {
        let temporary = TemporaryConnectionStore()
        defer { temporary.cleanUp() }
        let saved = SavedConnection(
            label: "Open Existing",
            host: "open.example",
            user: "alice"
        )
        try await temporary.store.save([saved])

        let harness = ConnectionAttemptHarness()
        let manager = ConnectionManager(
            store: temporary.store,
            attemptFactory: { harness.makeAttempt($0) }
        )
        await manager.load()

        #expect(manager.open(id: saved.id) == false)
        manager.connect(id: saved.id)
        let attempt = try #require(harness.attempts.first)
        await attempt.emit(.ready(sessionID: "open-session", revision: 1))
        try await AsyncTestSupport.observed("connection ready for open") {
            manager.status(for: saved.id) == .connected
        }

        #expect(manager.open(id: saved.id))
        #expect(harness.attempts.count == 1)
        await manager.shutdown()
    }

    @Test("saved connections replace the JSON file atomically")
    func savedConnectionsRoundTripAtomically() async throws {
        let temporary = TemporaryConnectionStore()
        defer { temporary.cleanUp() }

        let expected = [
            SavedConnection(
                label: "Production",
                host: "example.com",
                port: 2222,
                user: "alice",
                sessionID: "opaque-session",
                lastKnownRevision: 41
            ),
            SavedConnection(
                label: "Lab",
                host: "lab.internal",
                user: "bob"
            ),
        ]

        try await temporary.store.save(expected)
        #expect(try await temporary.store.load() == expected)
        let contents = try FileManager.default.contentsOfDirectory(
            at: temporary.directory,
            includingPropertiesForKeys: nil
        )
        #expect(contents.map(\.lastPathComponent) == ["connections.json"])

        let replacement = [expected[1]]
        try await temporary.store.save(replacement)
        #expect(try await temporary.store.load() == replacement)
        let replacedContents = try FileManager.default.contentsOfDirectory(
            at: temporary.directory,
            includingPropertiesForKeys: nil
        )
        #expect(replacedContents.map(\.lastPathComponent) == ["connections.json"])
    }

    /// PX-024 round 2: a context in which no session has been ready holds no committed replica, so
    /// a saved session is attached with CLIENT_HELLO and the fresh identity (§18's documented
    /// attach path), never a revision-0 CLIENT_RESUME whose RESUME_OK would be ready before replay.
    @Test("cold relaunch attaches with CLIENT_HELLO, never a revision-zero resume")
    func coldRelaunchAttachesWithHello() async throws {
        let connection = SavedConnection(
            label: "Saved",
            host: "server.example",
            user: "alice",
            sessionID: "saved-session",
            lastKnownRevision: 900
        )
        let context = ConnectionSessionContext()
        let configuration = SSHConfiguration(
            host: connection.host,
            user: connection.user,
            subsystem: "srui",
            strictHostKeyChecking: .yes,
            batchMode: true
        )
        let request = ConnectionAttemptRequest(
            connection: connection,
            configuration: configuration,
            context: context,
            isWarmReconnect: false
        )
        #expect(request.requestedSessionID == "saved-session")
        #expect(request.resumeSessionID == nil)
        let transport = ConnectionManagerCaptureTransport()
        let attempt = ConnectionManager.makeSessionControllerAttempt(
            request: request,
            transport: transport
        )

        try await attempt.start()
        let frame = try #require(await transport.frame(at: 0))
        let message = try decodeFramedMessage(from: frame)
        guard case .clientHello(let hello) = message.msg else {
            Issue.record("a cold saved connection did not attach with CLIENT_HELLO: \(String(describing: message.msg))")
            await attempt.stop()
            return
        }
        #expect(hello.clientInstanceID == context.outbox.clientInstanceId.bytes)
        #expect(request.resumeRevision == 0)

        await attempt.stop()
    }

    /// PX-024: two manager instances over one saved-connections file, as two launches of the app.
    /// Each attaches through the production `beginAttempt` path with CLIENT_HELLO and its own new
    /// client identity; the remembered revision is shown, never sent (§18: a persisted last-known
    /// revision is presentation metadata, not resume authority).
    @Test("a relaunched manager attaches the saved session with a fresh client identity")
    func relaunchedManagerAttachesWithFreshIdentity() async throws {
        let temporary = TemporaryConnectionStore()
        defer { temporary.cleanUp() }
        let saved = SavedConnection(
            label: "Remembered",
            host: "remembered.example",
            user: "alice",
            sessionID: "saved-session",
            lastKnownRevision: 900
        )
        try await temporary.store.save([saved])

        func launch() async throws -> (hello: SRUIClientHello, identity: ClientInstanceId) {
            let harness = ConnectionControllerHarness()
            let manager = ConnectionManager(
                store: temporary.store,
                attemptFactory: { harness.makeAttempt($0) }
            )
            await manager.load()
            #expect(manager.entries == [saved])
            #expect(manager.status(for: saved.id) == .unknown)
            #expect(saved.sessionCaption == "Session saved-session · last seen at revision 900")
            manager.connect(id: saved.id)
            let request = try #require(harness.requests.first)
            #expect(request.isWarmReconnect == false)
            let message = try await waitForFrame(0, from: try #require(harness.transports.first))
            await manager.shutdown()
            guard case .clientHello(let hello) = message.msg else {
                Issue.record("a relaunch did not attach with CLIENT_HELLO: \(String(describing: message.msg))")
                throw CancellationError()
            }
            return (hello, request.context.outbox.clientInstanceId)
        }

        let first = try await launch()
        let second = try await launch()
        for (hello, identity) in [first, second] {
            #expect(hello.clientInstanceID == identity.bytes)
        }
        #expect(first.identity != second.identity, "a relaunch never reuses a client identity")
        // Never ready, so the shutdowns kept the remembered metadata as it was.
        #expect(try await temporary.store.load() == [saved])
    }

    private func welcome(sessionID: String, initialRevision: UInt64 = 1) -> SRUIMessage {
        var welcome = SRUIServerWelcome()
        welcome.coreVersion = SRUICoreVersion
        welcome.sessionID = sessionID
        welcome.requiredProfiles = ["org.srui.standard-widgets/1"]
        welcome.initialRevision = initialRevision
        var message = SRUIMessage()
        message.serverWelcome = welcome
        return message
    }

    /// PX-024 round 2: the server attaches a HELLO to the one session it serves. The saved ID is
    /// kept, silently, when that is the saved session; any other session is the saved one's
    /// replacement, saved and alerted before its snapshot is shown, as a REPLACED decision is.
    @Test("a cold attach keeps the saved session silently, or reports and saves its replacement", arguments: [false, true])
    func coldAttachDetectsReplacement(replaced: Bool) async throws {
        let temporary = TemporaryConnectionStore()
        defer { temporary.cleanUp() }
        let saved = SavedConnection(label: "Saved", host: "saved.example", user: "alice",
                                    sessionID: "saved-session", lastKnownRevision: 40)
        try await temporary.store.save([saved])
        let harness = ConnectionControllerHarness()
        let manager = ConnectionManager(store: temporary.store, attemptFactory: { harness.makeAttempt($0) })
        await manager.load()
        manager.connect(id: saved.id)
        let transport = try #require(harness.transports.first)
        guard case .clientHello = try await waitForFrame(0, from: transport).msg else {
            Issue.record("a cold attach did not send CLIENT_HELLO")
            await manager.shutdown()
            return
        }
        let attached = replaced ? "replacement-session" : "saved-session"
        try await transport.receive(welcome(sessionID: attached))
        try await AsyncTestSupport.eventually(timeout: .roundTrip, description: "the WELCOME decision") {
            manager.status(for: saved.id) == .resynchronizing
        }
        let context = try #require(harness.requests.first?.context)
        if replaced {
            // Saved and alerted before the snapshot is applied or shown.
            try await AsyncTestSupport.eventuallyAsync(timeout: .roundTrip, description: "the replacement saved and alerted") {
                (try? await temporary.store.load().first?.sessionID) == "replacement-session"
                    && manager.alert?.kind == .sessionReplaced
            }
            #expect(manager.entries.first?.sessionID == "replacement-session")
            #expect(manager.entries.first?.lastKnownRevision == 0)
            let alert = try #require(manager.alert)
            #expect(alert.kind == .sessionReplaced)
            #expect(alert.message.contains("saved-session") && alert.message.contains("replacement-session"))
            #expect(context.renderer.sessionWindowStatus == ConnectionStatus.sessionReplacedWindowStatus)
        }
        #expect(context.applier.currentSnapshot.store.rootIDs.isEmpty, "nothing shown before the snapshot")

        try await transport.receive(textSnapshot(value: "current"))
        try await AsyncTestSupport.eventually(timeout: .roundTrip, description: "ready after the snapshot") {
            manager.status(for: saved.id) == .connected
                && context.renderer.textEditingSession.lastKnownAuthoritative(for: NodeId(2)) == "current"
        }
        #expect(manager.entries.first?.sessionID == attached)
        #expect(manager.entries.first?.lastKnownRevision == 1)
        #expect((manager.alert?.kind == .sessionReplaced) == replaced)
        await manager.shutdown()
        #expect(try await temporary.store.load().first?.sessionID == attached)
    }

    /// PX-024 round 2: once a session has been ready in a context, its replica and outbox are this
    /// process's continuity checkpoint, and a reconnect resumes them exactly as before (§18 warm).
    @Test("a context that has been ready resumes with CLIENT_RESUME at its committed revision")
    func readyContextResumes() async throws {
        let temporary = TemporaryConnectionStore()
        defer { temporary.cleanUp() }
        let saved = SavedConnection(label: "Warm", host: "warm.example", user: "alice",
                                    sessionID: "original-session", lastKnownRevision: 7)
        try await temporary.store.save([saved])
        let harness = ConnectionControllerHarness()
        let manager = ConnectionManager(store: temporary.store, attemptFactory: { harness.makeAttempt($0) })
        await manager.load()
        try await mountOriginalSession(manager: manager, harness: harness, connectionID: saved.id)
        let first = try #require(harness.transports.first)
        guard case .clientHello = try await waitForFrame(0, from: first).msg else {
            Issue.record("the cold attach did not send CLIENT_HELLO")
            await manager.shutdown()
            return
        }
        await first.close()
        try await AsyncTestSupport.eventually(timeout: .roundTrip, description: "the first attempt released") {
            !manager.hasActiveAttemptForTesting(saved.id)
        }
        manager.connect(id: saved.id)
        let request = try #require(harness.requests.last)
        #expect(request.resumeSessionID == "original-session")
        guard case .clientResume(let resume) = try await waitForFrame(0, from: try #require(harness.transports.last)).msg else {
            Issue.record("a warm reconnect did not resume")
            await manager.shutdown()
            return
        }
        #expect(resume.sessionID == "original-session")
        #expect(resume.lastAppliedRevision == 1)
        #expect(resume.clientInstanceID == request.context.outbox.clientInstanceId.bytes)
        await manager.shutdown()
    }

    /// PX-024 round 2 (F2): a superseded attempt whose replaced and ready events arrive only after
    /// the newer attempt has saved its session can change neither the row nor the file.
    @Test("a superseded attempt's late replaced and ready never reach the row or the file")
    func supersededAttemptCannotWriteTheStore() async throws {
        let temporary = TemporaryConnectionStore()
        defer { temporary.cleanUp() }
        let saved = SavedConnection(label: "Race", host: "race.example", user: "alice", sessionID: "old-session")
        try await temporary.store.save([saved])
        let firstGate = AsyncGate()
        let harness = ConnectionAttemptHarness()
        harness.startPlans = [.init(gate: firstGate), .init()]
        let manager = ConnectionManager(store: temporary.store, attemptFactory: { harness.makeAttempt($0) })
        await manager.load()

        manager.connect(id: saved.id)
        await firstGate.waitUntilPaused()
        manager.connect(id: saved.id)
        let first = try #require(harness.attempts.first)
        let second = try #require(harness.attempts.last)
        await second.emit(.ready(sessionID: "new-session", revision: 4))
        try await AsyncTestSupport.eventuallyAsync(timeout: .roundTrip, description: "the newer attempt saved") {
            let stored = try? await temporary.store.load().first
            return stored?.sessionID == "new-session" && stored?.lastKnownRevision == 4
        }

        await first.emit(.replaced(previousSessionID: "old-session", newSessionID: "stale-session"))
        await first.emit(.ready(sessionID: "stale-session", revision: 9))
        await firstGate.release()
        try await AsyncTestSupport.eventuallyAsync(timeout: .roundTrip, description: "the superseded attempt stopped") {
            await first.stopCount() > 0
        }
        try await Task.sleep(for: .milliseconds(200))
        await manager.waitForPersistenceForTesting()
        #expect(manager.status(for: saved.id) == .connected)
        #expect(manager.entries.first?.sessionID == "new-session")
        #expect(manager.entries.first?.lastKnownRevision == 4)
        #expect(manager.alert == nil)
        let stored = try #require(try await temporary.store.load().first)
        #expect(stored.sessionID == "new-session" && stored.lastKnownRevision == 4)
        await manager.shutdown()
    }

    /// PX-024 round 2 (F2): a forgotten entry stays forgotten even when its still-running session
    /// later becomes ready; nothing brings the row or its file record back.
    @Test("a forgotten entry is not brought back when its running session becomes ready")
    func forgottenEntryStaysForgotten() async throws {
        let temporary = TemporaryConnectionStore()
        defer { temporary.cleanUp() }
        let saved = SavedConnection(label: "Forget", host: "forget.example", user: "alice", sessionID: "kept-session")
        try await temporary.store.save([saved])
        let harness = ConnectionAttemptHarness()
        let manager = ConnectionManager(store: temporary.store, attemptFactory: { harness.makeAttempt($0) })
        await manager.load()
        manager.connect(id: saved.id)
        let attempt = try #require(harness.attempts.first)
        let context = try #require(harness.requests.first?.context)
        try await AsyncTestSupport.eventuallyAsync(timeout: .roundTrip, description: "attempt start") {
            await attempt.startCount() == 1
        }
        manager.remove(id: saved.id)
        await manager.waitForPersistenceForTesting()
        #expect(try await temporary.store.load().isEmpty)

        await attempt.emit(.connecting)
        try await AsyncTestSupport.eventually(timeout: .roundTrip, description: "the removed entry's window connecting") {
            context.renderer.sessionWindowStatus != nil
        }
        await attempt.emit(.ready(sessionID: "kept-session", revision: 3))
        try await AsyncTestSupport.eventually(timeout: .roundTrip, description: "the removed entry's window live") {
            context.renderer.sessionWindowStatus == nil
        }
        await manager.waitForPersistenceForTesting()
        #expect(manager.entries.isEmpty)
        #expect(manager.status(for: saved.id) == .unknown)
        #expect(try await temporary.store.load().isEmpty)
        await manager.shutdown()
        #expect(try await temporary.store.load().isEmpty)
    }

    // MARK: - The saved-connections file (PX-024 round 2, F3 and F4)

    enum UnreadableFile: String, CaseIterable, Sendable {
        case truncated, wrongShape, revisionPastUInt64, negativeRevision, unknownVersion, empty

        func contents(id: UUID) -> String {
            let entry = { (revision: String) in
                "{\"id\":\"\(id.uuidString)\",\"label\":\"L\",\"host\":\"h.example\",\"user\":\"u\",\"lastKnownRevision\":\(revision)}"
            }
            switch self {
            case .truncated: return "[\(entry("3").dropLast(5))"
            case .wrongShape: return "{\"schemaVersion\":2,\"entries\":[\(entry("3"))]}"
            case .revisionPastUInt64: return "[\(entry("18446744073709551616"))]"
            case .negativeRevision: return "[\(entry("-1"))]"
            case .unknownVersion: return "{\"version\":2,\"connections\":[\(entry("3"))]}"
            case .empty: return ""
            }
        }
    }

    @Test("an unreadable saved file is moved aside, named in the alert, and never written over",
          arguments: UnreadableFile.allCases)
    func unreadableFileIsMovedAsideNotDestroyed(_ kind: UnreadableFile) async throws {
        let temporary = TemporaryConnectionStore()
        defer { temporary.cleanUp() }
        try FileManager.default.createDirectory(at: temporary.directory, withIntermediateDirectories: true)
        let original = Data(kind.contents(id: UUID()).utf8)
        try original.write(to: temporary.file)
        let backup = temporary.store.backupURL
        #expect(backup.lastPathComponent == "connections.json.unreadable")

        let harness = ConnectionAttemptHarness()
        let manager = ConnectionManager(store: temporary.store, attemptFactory: { harness.makeAttempt($0) })
        await manager.load()
        #expect(manager.entries.isEmpty)
        let alert = try #require(manager.alert)
        #expect(alert.kind == .persistenceFailed && alert.title == "Couldn’t Load Connections")
        #expect(alert.message.contains(backup.path), "\(alert.message)")
        #expect(try Data(contentsOf: backup) == original)
        #expect(!FileManager.default.fileExists(atPath: temporary.file.path))

        // Launch, connect something new, quit: a fresh file is written beside the kept backup.
        let id = try #require(manager.connect(ConnectDraft(label: "New", host: "new.example", user: "bob")))
        await harness.attempts.first?.emit(.ready(sessionID: "new-session", revision: 2))
        try await AsyncTestSupport.observed("the new connection ready") { manager.status(for: id) == .connected }
        await manager.shutdown()
        #expect(try Data(contentsOf: backup) == original, "the unreadable file survives every later save")
        #expect(try await SavedConnectionStore(url: temporary.file).load().map(\.id) == [id])
        let written = try String(contentsOf: temporary.file, encoding: .utf8)
        #expect(written.contains("\"version\" : 1"), "\(written)")
    }

    @Test("a later unreadable file replaces the one backup; nothing accumulates")
    func laterUnreadableFileReplacesTheBackup() async throws {
        let temporary = TemporaryConnectionStore()
        defer { temporary.cleanUp() }
        try FileManager.default.createDirectory(at: temporary.directory, withIntermediateDirectories: true)
        for contents in ["[{\"first\"", "{\"version\":99,\"connections\":[]}"] {
            try Data(contents.utf8).write(to: temporary.file)
            await #expect(throws: SavedConnectionStore.LoadError.self) { try await temporary.store.load() }
            #expect(try String(contentsOf: temporary.store.backupURL, encoding: .utf8) == contents)
        }
        let names = try FileManager.default.contentsOfDirectory(atPath: temporary.directory.path)
        #expect(names == ["connections.json.unreadable"])
    }

    @Test("an unreadable file that cannot be moved aside is left in place and never saved over")
    func unmovableUnreadableFileBlocksSaves() async throws {
        let temporary = TemporaryConnectionStore()
        defer {
            chmod(temporary.directory.path, 0o700)
            temporary.cleanUp()
        }
        try FileManager.default.createDirectory(at: temporary.directory, withIntermediateDirectories: true)
        let original = Data("[{\"truncated\"".utf8)
        try original.write(to: temporary.file)
        #expect(chmod(temporary.directory.path, 0o500) == 0)
        do {
            _ = try await temporary.store.load()
            Issue.record("an unreadable file loaded")
        } catch let error as SavedConnectionStore.LoadError {
            guard case .leftInPlace(_, let file) = error else {
                Issue.record("expected leftInPlace, got \(error)")
                return
            }
            #expect(file == temporary.file)
            #expect(error.description.contains(temporary.file.path))
        }
        await #expect(throws: SavedConnectionStore.UnreadableFileInPlace.self) {
            try await temporary.store.save([SavedConnection(label: "X", host: "x.example", user: "u")])
        }
        #expect(try Data(contentsOf: temporary.file) == original)
    }

    @Test("a pre-versioned array file loads as version 1 and is rewritten versioned")
    func legacyArrayFileLoads() async throws {
        let temporary = TemporaryConnectionStore()
        defer { temporary.cleanUp() }
        try FileManager.default.createDirectory(at: temporary.directory, withIntermediateDirectories: true)
        let entries = [SavedConnection(label: "Old", host: "old.example", port: 2222, user: "alice",
                                       sessionID: "0123456789abcdef0123456789abcdef", lastKnownRevision: 12)]
        try JSONEncoder().encode(entries).write(to: temporary.file)
        #expect(try await temporary.store.load() == entries)
        try await temporary.store.save(entries)
        #expect(try String(contentsOf: temporary.file, encoding: .utf8).contains("\"connections\""))
        #expect(try await temporary.store.load() == entries)
        #expect(!FileManager.default.fileExists(atPath: temporary.store.backupURL.path))
    }

    /// F4: the bound is `SavedConnection.maximumSessionIDScalars` (128; sessiond mints 32 hex).
    @Test("a saved session ID with a control or invisible scalar, or past the bound, is dropped, not the entry")
    func invalidSavedSessionIDsAreDropped() async throws {
        let temporary = TemporaryConnectionStore()
        defer { temporary.cleanUp() }
        let bound = SavedConnection.maximumSessionIDScalars
        #expect(bound == 128)
        let cases: [(String, Bool)] = [
            ("0123456789abcdef0123456789abcdef", true),
            ("evil\u{0}x", false),
            ("line\nbreak", false),
            ("esc\u{1B}[31mSRUI verified this host", false),
            ("zero\u{200B}width", false),
            ("", false),
            (String(repeating: "a", count: 2_000_000), false),
            (String(repeating: "b", count: bound), true),
            (String(repeating: "c", count: bound + 1), false),
            (String(repeating: "é", count: bound), true),
            (String(repeating: "é", count: bound + 1), false),
        ]
        let entries = cases.enumerated().map { index, item in
            SavedConnection(label: "Entry \(index)", host: "h\(index).example", user: "u", sessionID: item.0, lastKnownRevision: 9)
        }
        try await temporary.store.save(entries)
        let loaded = try await temporary.store.load()
        #expect(loaded.count == entries.count)
        for (index, (sessionID, kept)) in cases.enumerated() {
            #expect(loaded[index].label == entries[index].label && loaded[index].host == entries[index].host)
            #expect(loaded[index].sessionID == (kept ? sessionID : nil), "case \(index)")
            #expect(loaded[index].lastKnownRevision == (kept ? 9 : 0), "case \(index)")
            #expect((loaded[index].sessionCaption != nil) == kept, "case \(index)")
        }
    }

    @Test("a server-reported session ID that fails the rule is neither stored nor shown")
    func invalidServerSessionIDsAreNotStored() async throws {
        let temporary = TemporaryConnectionStore()
        defer { temporary.cleanUp() }
        let saved = SavedConnection(label: "Server", host: "server.example", user: "alice", sessionID: "good-session")
        try await temporary.store.save([saved])
        let harness = ConnectionAttemptHarness()
        let manager = ConnectionManager(store: temporary.store, attemptFactory: { harness.makeAttempt($0) })
        await manager.load()
        manager.connect(id: saved.id)
        let attempt = try #require(harness.attempts.first)
        let hostile = "evil\u{0}\n\u{1B}[31mtrusted"
        await attempt.emit(.replaced(previousSessionID: "good-session", newSessionID: hostile))
        try await AsyncTestSupport.observed("the replacement alert") { manager.alert?.kind == .sessionReplaced }
        #expect(manager.entries.first?.sessionID == nil)
        #expect(manager.alert?.message.contains("\u{1B}") == false && manager.alert?.message.contains("\u{0}") == false)
        await attempt.emit(.ready(sessionID: String(repeating: "x", count: SavedConnection.maximumSessionIDScalars + 1), revision: 5))
        try await AsyncTestSupport.observed("ready") { manager.status(for: saved.id) == .connected }
        #expect(manager.entries.first?.sessionID == nil && manager.entries.first?.lastKnownRevision == 0)
        await manager.waitForPersistenceForTesting()
        #expect(try await temporary.store.load().first?.sessionID == nil)
        await manager.shutdown()
    }

    @Test("duplicate entry IDs load as one row, and one remove deletes exactly one row")
    func duplicateEntryIDsAreDropped() async throws {
        let temporary = TemporaryConnectionStore()
        defer { temporary.cleanUp() }
        let shared = UUID()
        let first = SavedConnection(id: shared, label: "First", host: "first.example", user: "alice")
        let duplicate = SavedConnection(id: shared, label: "Duplicate", host: "dup.example", user: "mallory")
        let other = SavedConnection(label: "Other", host: "other.example", user: "bob")
        try await temporary.store.save([first, duplicate, other])
        let harness = ConnectionAttemptHarness()
        let manager = ConnectionManager(store: temporary.store, attemptFactory: { harness.makeAttempt($0) })
        await manager.load()
        #expect(manager.entries == [first, other])
        manager.remove(id: shared)
        #expect(manager.entries == [other])
        await manager.waitForPersistenceForTesting()
        #expect(try await temporary.store.load() == [other])
        await manager.shutdown()
    }

    private func mountOriginalSession(
        manager: ConnectionManager,
        harness: ConnectionControllerHarness,
        connectionID: SavedConnection.ID
    ) async throws {
        manager.connect(id: connectionID)
        let transport = try #require(harness.transports.last)
        _ = try await waitForFrame(0, from: transport)

        var welcome = SRUIServerWelcome()
        welcome.coreVersion = SRUICoreVersion
        welcome.sessionID = "original-session"
        welcome.requiredProfiles = ["org.srui.standard-widgets/1"]
        welcome.initialRevision = 1
        var message = SRUIMessage()
        message.serverWelcome = welcome
        try await transport.receive(message)
        try await transport.receive(textSnapshot(value: "original value"))
        try await AsyncTestSupport.eventually(timeout: .roundTrip, description: "original session mounted") {
            manager.status(for: connectionID) == .connected
                && harness.requests.last?.context.renderer.textEditingSession
                    .lastKnownAuthoritative(for: NodeId(2)) == "original value"
        }
    }

    private func textSnapshot(value: String) -> SRUIMessage {
        var message = SRUIMessage()
        message.transaction = Transaction(
            baseRevision: .initial,
            newRevision: Revision(1),
            operations: [
                .createNode(id: NodeId(1), nodeType: .surface),
                .createNode(
                    id: NodeId(2),
                    nodeType: .textInput,
                    parentID: NodeId(1),
                    properties: [Property(property: .value, value: .string(value))]
                ),
            ]
        ).toWire()
        return message
    }

    private func waitForFrame(
        _ index: Int,
        from transport: ConnectionManagerCaptureTransport
    ) async throws -> SRUIMessage {
        try await AsyncTestSupport.eventuallyAsync(timeout: .roundTrip, description: "outbound frame \(index)") {
            await transport.frame(at: index) != nil
        }
        return try decodeFramedMessage(from: #require(await transport.frame(at: index)))
    }

    @Test(
        "interrupted replacement reattaches with HELLO from a fresh context, ready only with the snapshot",
        arguments: [false, true]
    )
    func interruptedReplacementReattachesWithHello(duringResume: Bool) async throws {
        let temporary = TemporaryConnectionStore()
        defer { temporary.cleanUp() }
        let saved = SavedConnection(label: "Replacement", host: "replacement.example", user: "alice")
        try await temporary.store.save([saved])
        let harness = ConnectionControllerHarness()
        let manager = ConnectionManager(
            store: temporary.store,
            attemptFactory: { harness.makeAttempt($0) }
        )
        await manager.load()
        try await mountOriginalSession(manager: manager, harness: harness, connectionID: saved.id)
        let context = try #require(harness.requests.first?.context)

        if duringResume {
            await harness.transports.last?.close()
            try await AsyncTestSupport.eventually(timeout: .roundTrip, description: "original attempt released") {
                !manager.hasActiveAttemptForTesting(saved.id)
            }
            manager.connect(id: saved.id)
            _ = try await waitForFrame(0, from: #require(harness.transports.last))
        }

        let replacedTransport = try #require(harness.transports.last)
        var replacement = SRUIServerResyncRequired()
        replacement.sessionID = "replacement-session"
        replacement.continuity = .replaced
        replacement.snapshotRevision = 1
        replacement.requiredProfiles = ["org.srui.standard-widgets/1"]
        var message = SRUIMessage()
        message.serverResyncRequired = replacement
        try await replacedTransport.receive(message)
        try await AsyncTestSupport.observed("replacement accepted before snapshot") {
            manager.entries.first?.sessionID == "replacement-session"
                && manager.status(for: saved.id) == .resynchronizing
        }
        #expect(context.applier.lastAppliedRevision == .initial)
        #expect(context.applier.currentSnapshot.store.rootIDs.isEmpty)
        #expect(context.renderer.registry.handle(for: NodeId(2)) == nil)

        // Lose the transport after REPLACED, before the replacement snapshot can arrive.
        await replacedTransport.close()
        try await AsyncTestSupport.eventually(timeout: .roundTrip, description: "replacement attempt released") {
            !manager.hasActiveAttemptForTesting(saved.id)
        }
        // PX-024 round 3 (R2-1): the context has been ready, but its replica is now empty, so the
        // reconnect starts from a fresh context and attaches with CLIENT_HELLO: a revision-0
        // CLIENT_RESUME would be RESUME_OK'd and declared ready before its replay (an empty window
        // reading live). The server below answers whichever handshake arrives as srtop would.
        manager.connect(id: saved.id)
        let reconnect = try #require(harness.requests.last)
        let resumedTransport = try #require(harness.transports.last)
        let handshake = try await waitForFrame(0, from: resumedTransport)
        switch handshake.msg {
        case .clientHello(let hello):
            #expect(hello.clientInstanceID == reconnect.context.outbox.clientInstanceId.bytes)
            try await resumedTransport.receive(welcome(sessionID: "replacement-session"))
            try await resumedTransport.receive(textSnapshot(value: "replacement value"))
        case .clientResume:
            var resumed = SRUIServerResumeOk()
            resumed.sessionID = "replacement-session"
            resumed.replayFromRevision = 0
            resumed.requiredProfiles = ["org.srui.standard-widgets/1"]
            message.serverResumeOk = resumed
            try await resumedTransport.receive(message)
        default:
            Issue.record("unexpected handshake \(String(describing: handshake.msg))")
        }
        // The moment the reconnect is ready, its replica already holds the current state.
        try await AsyncTestSupport.observed("the reconnect ready") { manager.status(for: saved.id) == .connected }
        let readyContext = reconnect.context
        #expect(!readyContext.applier.currentSnapshot.store.rootIDs.isEmpty, "ready over an empty replica")
        #expect(readyContext.applier.lastAppliedRevision == Revision(1))
        guard case .clientHello = handshake.msg else {
            Issue.record("an empty-replica reconnect sent \(String(describing: handshake.msg)), not CLIENT_HELLO")
            try await resumedTransport.receive(textSnapshot(value: "replacement value"))
            await manager.shutdown()
            return
        }
        #expect(readyContext !== context, "an emptied context is replaced by a fresh one")
        #expect(readyContext.outbox.clientInstanceId != context.outbox.clientInstanceId,
                "a fresh identity, as for a cold relaunch")
        try await AsyncTestSupport.eventually(timeout: .roundTrip, description: "replacement snapshot mounted") {
            readyContext.renderer.textEditingSession.lastKnownAuthoritative(for: NodeId(2)) == "replacement value"
        }
        #expect(manager.entries.first?.sessionID == "replacement-session")
        await manager.shutdown()
    }

    @Test("offline text is sent after the manager releases the stopped attempt")
    func offlineTextSurvivesReleasedAttempt() async throws {
        let temporary = TemporaryConnectionStore()
        defer { temporary.cleanUp() }
        let saved = SavedConnection(label: "Offline", host: "offline.example", user: "alice")
        try await temporary.store.save([saved])
        let harness = ConnectionControllerHarness()
        let manager = ConnectionManager(
            store: temporary.store,
            attemptFactory: { harness.makeAttempt($0) }
        )
        await manager.load()
        try await mountOriginalSession(manager: manager, harness: harness, connectionID: saved.id)
        let context = try #require(harness.requests.first?.context)
        await harness.transports.last?.close()
        try await AsyncTestSupport.eventually(timeout: .roundTrip, description: "stopped attempt released") {
            !manager.hasActiveAttemptForTesting(saved.id)
        }

        context.renderer.textEditingSession.noteLocalValue(
            "offline edit",
            nodeID: NodeId(2),
            composing: false,
            flushImmediately: true
        )
        #expect(context.renderer.textEditingSession.nextUnassignedEditNode() == NodeId(2))
        #expect(await context.outbox.pendingCount == 0)

        manager.connect(id: saved.id)
        let transport = try #require(harness.transports.last)
        let resumeMessage = try await waitForFrame(0, from: transport)
        #expect(resumeMessage.clientResume.lastAppliedRevision == 1)
        #expect(await transport.frame(at: 1) == nil)

        var resumed = SRUIServerResumeOk()
        resumed.sessionID = "original-session"
        resumed.replayFromRevision = 2
        resumed.requiredProfiles = ["org.srui.standard-widgets/1"]
        var message = SRUIMessage()
        message.serverResumeOk = resumed
        try await transport.receive(message)
        let eventMessage = try await waitForFrame(1, from: transport)
        let event = try ProtocolDecoder().validateAndConvertEvent(wire: eventMessage.event)
        #expect(event.eventType == .EVENT_TEXT_EDIT)
        #expect(event.nodeId == NodeId(2))
        #expect(event.textArg == "offline edit")
        #expect(event.observedRevision == Revision(1))
        #expect(event.eventSeq == 1)
        #expect(context.renderer.textEditingSession.localValue(for: NodeId(2)) == "offline edit")
        await manager.shutdown()
    }

    @Test("cold failure preserves last-known revision metadata until ready")
    func coldFailurePreservesSavedRevision() async throws {
        let temporary = TemporaryConnectionStore()
        defer { temporary.cleanUp() }
        let saved = SavedConnection(
            label: "Cold Failure",
            host: "cold-failure.example",
            user: "alice",
            sessionID: "saved-session",
            lastKnownRevision: 900
        )
        try await temporary.store.save([saved])

        let harness = ConnectionAttemptHarness()
        let manager = ConnectionManager(
            store: temporary.store,
            attemptFactory: { harness.makeAttempt($0) }
        )
        await manager.load()
        manager.connect(id: saved.id)
        let attempt = try #require(harness.attempts.first)

        await attempt.emit(.failed("cold resume failed"))
        try await AsyncTestSupport.observed("cold failure recorded") {
            manager.status(for: saved.id) == .disconnected(resumeAvailable: true)
                && manager.entries.first?.lastKnownRevision == 900
                && manager.alert?.kind == .connectionFailed
        }
        #expect(try await temporary.store.load().first?.lastKnownRevision == 900)

        await manager.shutdown()
        #expect(try await temporary.store.load().first?.lastKnownRevision == 900)
    }

    @Test("first ready event saves the entry and authoritative revision")
    func successfulConnectionIsSaved() async throws {
        let temporary = TemporaryConnectionStore()
        defer { temporary.cleanUp() }
        let harness = ConnectionAttemptHarness()
        let manager = ConnectionManager(
            store: temporary.store,
            attemptFactory: { harness.makeAttempt($0) }
        )

        let connectionID = try #require(manager.connect(ConnectDraft(
            label: "Work",
            host: "work.example",
            port: "2200",
            user: "zoe"
        )))
        let attempt = try #require(harness.attempts.first)
        await attempt.emit(.ready(sessionID: "session-one", revision: 12))

        try await AsyncTestSupport.observed("connection ready state") {
            manager.status(for: connectionID) == .connected
        }
        try await AsyncTestSupport.eventuallyAsync(
            timeout: .roundTrip,
            description: "successful connection persistence"
        ) {
            let stored = try? await temporary.store.load()
            return stored?.first?.sessionID == "session-one"
                && stored?.first?.lastKnownRevision == 12
        }

        let stored = try #require(try await temporary.store.load().first)
        #expect(stored.label == "Work")
        #expect(stored.host == "work.example")
        #expect(stored.port == 2200)
        #expect(stored.user == "zoe")
        await manager.shutdown()
    }

    @Test("replacement is blocking and remains resynchronizing until ready")
    func replacementUpdatesIdentityAndNotifies() async throws {
        let temporary = TemporaryConnectionStore()
        defer { temporary.cleanUp() }
        let saved = SavedConnection(
            label: "Old",
            host: "old.example",
            user: "alice",
            sessionID: "old-session",
            lastKnownRevision: 77
        )
        try await temporary.store.save([saved])

        let harness = ConnectionAttemptHarness()
        let manager = ConnectionManager(
            store: temporary.store,
            attemptFactory: { harness.makeAttempt($0) }
        )
        await manager.load()
        manager.connect(id: saved.id)
        let attempt = try #require(harness.attempts.first)

        await attempt.emit(.replaced(
            previousSessionID: "old-session",
            newSessionID: "new-session"
        ))
        try await AsyncTestSupport.observed("replacement notification") {
            manager.status(for: saved.id) == .resynchronizing
                && manager.alert?.kind == .sessionReplaced
        }
        #expect(manager.entries.first?.sessionID == "new-session")
        #expect(manager.entries.first?.lastKnownRevision == 0)
        #expect(try await temporary.store.load().first?.sessionID == "new-session")

        await attempt.emit(.ready(sessionID: "new-session", revision: 5))
        try await AsyncTestSupport.observed("replacement snapshot ready") {
            manager.status(for: saved.id) == .connected
        }
        #expect(manager.entries.first?.lastKnownRevision == 5)
        await manager.shutdown()
    }

    @Test("disconnect during replacement catch-up keeps revision zero")
    func replacementDisconnectKeepsRevisionZero() async throws {
        let temporary = TemporaryConnectionStore()
        defer { temporary.cleanUp() }
        let saved = SavedConnection(
            label: "Replacement Disconnect",
            host: "replacement-disconnect.example",
            user: "alice",
            sessionID: "old-session",
            lastKnownRevision: 40
        )
        try await temporary.store.save([saved])

        let harness = ConnectionAttemptHarness()
        let manager = ConnectionManager(
            store: temporary.store,
            attemptFactory: { harness.makeAttempt($0) }
        )
        await manager.load()
        let attempt = try await enterReplacementCatchUp(
            manager: manager,
            harness: harness,
            saved: saved,
            replacementSessionID: "new-session"
        )

        await attempt.emit(.stopped)
        try await AsyncTestSupport.eventuallyAsync(
            timeout: .roundTrip,
            description: "replacement attempt stopped"
        ) {
            await attempt.stopCount() > 0
        }

        #expect(manager.status(for: saved.id) == .disconnected(resumeAvailable: true))
        #expect(manager.entries.first?.sessionID == "new-session")
        #expect(manager.entries.first?.lastKnownRevision == 0)
        let stored = try #require(try await temporary.store.load().first)
        #expect(stored.sessionID == "new-session")
        #expect(stored.lastKnownRevision == 0)
        await manager.shutdown()
    }

    @Test("shutdown during replacement catch-up keeps revision zero")
    func replacementShutdownKeepsRevisionZero() async throws {
        let temporary = TemporaryConnectionStore()
        defer { temporary.cleanUp() }
        let saved = SavedConnection(
            label: "Replacement Shutdown",
            host: "replacement-shutdown.example",
            user: "alice",
            sessionID: "old-session",
            lastKnownRevision: 40
        )
        try await temporary.store.save([saved])

        let harness = ConnectionAttemptHarness()
        let manager = ConnectionManager(
            store: temporary.store,
            attemptFactory: { harness.makeAttempt($0) }
        )
        await manager.load()
        _ = try await enterReplacementCatchUp(
            manager: manager,
            harness: harness,
            saved: saved,
            replacementSessionID: "new-session"
        )

        await manager.shutdown()

        #expect(manager.entries.first?.sessionID == "new-session")
        #expect(manager.entries.first?.lastKnownRevision == 0)
        let stored = try #require(try await temporary.store.load().first)
        #expect(stored.sessionID == "new-session")
        #expect(stored.lastKnownRevision == 0)
    }

    @Test("back-to-back connects never start the superseded attempt")
    func supersededAttemptIsCancelledBeforeStart() async throws {
        let temporary = TemporaryConnectionStore()
        defer { temporary.cleanUp() }
        let saved = SavedConnection(
            label: "Queued",
            host: "queued.example",
            user: "alice",
            sessionID: "queued-session"
        )
        try await temporary.store.save([saved])

        let harness = ConnectionAttemptHarness()
        let manager = ConnectionManager(
            store: temporary.store,
            attemptFactory: { harness.makeAttempt($0) }
        )
        await manager.load()

        manager.connect(id: saved.id)
        manager.connect(id: saved.id)
        let firstAttempt = try #require(harness.attempts.first)
        let secondAttempt = try #require(harness.attempts.last)

        try await AsyncTestSupport.eventuallyAsync(
            timeout: .roundTrip,
            description: "superseded attempt stopped without starting"
        ) {
            let firstStops = await firstAttempt.stopCount()
            let secondStarts = await secondAttempt.startCount()
            return firstStops > 0 && secondStarts == 1
        }
        #expect(await firstAttempt.startCount() == 0)
        await manager.shutdown()
    }

    @Test("a late failed start cannot overwrite a newer attempt")
    func staleAttemptOutcomeIsIgnored() async throws {
        let temporary = TemporaryConnectionStore()
        defer { temporary.cleanUp() }
        let saved = SavedConnection(
            label: "Race",
            host: "race.example",
            user: "alice",
            sessionID: "old-session"
        )
        try await temporary.store.save([saved])

        let firstStartGate = AsyncGate()
        let harness = ConnectionAttemptHarness()
        harness.startPlans = [
            .init(gate: firstStartGate, error: .startFailed("stale failure")),
            .init(),
        ]
        let manager = ConnectionManager(
            store: temporary.store,
            attemptFactory: { harness.makeAttempt($0) }
        )
        await manager.load()

        manager.connect(id: saved.id)
        await firstStartGate.waitUntilPaused()
        manager.connect(id: saved.id)
        let secondAttempt = try #require(harness.attempts.last)
        await secondAttempt.emit(.ready(sessionID: "new-session", revision: 4))
        try await AsyncTestSupport.observed("newer attempt ready") {
            manager.status(for: saved.id) == .connected
                && manager.entries.first?.sessionID == "new-session"
        }

        await firstStartGate.release()
        let firstAttempt = try #require(harness.attempts.first)
        try await AsyncTestSupport.eventuallyAsync(timeout: .roundTrip, description: "stale attempt teardown") {
            await firstAttempt.stopCount() > 0
        }
        #expect(manager.status(for: saved.id) == .connected)
        #expect(manager.entries.first?.sessionID == "new-session")
        #expect(manager.alert == nil)
        await manager.shutdown()
    }

    @Test("a failed first connection discards its session context")
    func failedFirstConnectionDiscardsContext() async throws {
        let temporary = TemporaryConnectionStore()
        defer { temporary.cleanUp() }
        let draft = ConnectDraft(
            id: UUID(),
            label: "Retry",
            host: "retry.example",
            user: "alice"
        )
        let harness = ConnectionAttemptHarness()
        let manager = ConnectionManager(
            store: temporary.store,
            attemptFactory: { harness.makeAttempt($0) }
        )

        let connectionID = try #require(manager.connect(draft))
        let firstAttempt = try #require(harness.attempts.first)
        await firstAttempt.emit(.failed("initial failure"))

        try await AsyncTestSupport.observed("failed draft removed") {
            manager.entries.isEmpty && manager.alert?.kind == .connectionFailed
        }

        #expect(manager.connect(draft) == connectionID)
        let firstRequest = try #require(harness.requests.first)
        let secondRequest = try #require(harness.requests.last)
        #expect(firstRequest.context !== secondRequest.context)
        #expect(secondRequest.isWarmReconnect == false)
        await manager.shutdown()
    }

    @Test("warm reconnect reuses one renderer and all continuity state")
    func warmReconnectReusesSessionContext() async throws {
        let temporary = TemporaryConnectionStore()
        defer { temporary.cleanUp() }
        let saved = SavedConnection(
            label: "Warm",
            host: "warm.example",
            user: "alice",
            sessionID: "warm-session",
            lastKnownRevision: 55
        )
        try await temporary.store.save([saved])

        let harness = ConnectionAttemptHarness()
        let manager = ConnectionManager(
            store: temporary.store,
            attemptFactory: { harness.makeAttempt($0) }
        )
        await manager.load()
        manager.connect(id: saved.id)
        manager.connect(id: saved.id)

        let first = try #require(harness.requests.first)
        let second = try #require(harness.requests.last)
        #expect(first.isWarmReconnect == false)
        #expect(second.isWarmReconnect)
        #expect(first.resumeRevision == 0)
        #expect(first.context === second.context)
        #expect(first.context.applier === second.context.applier)
        #expect(first.context.outbox === second.context.outbox)
        #expect(first.context.resourceCache === second.context.resourceCache)
        #expect(first.context.transactionIngressGate === second.context.transactionIngressGate)
        #expect(first.context.continuityContext === second.context.continuityContext)
        #expect(first.context.renderer === second.context.renderer)

        #expect(first.configuration.strictHostKeyChecking == .yes)
        #expect(first.configuration.batchMode)
        #expect(first.configuration.subsystem == "srui")
        #expect(first.configuration.identityFile == nil)
        #expect(first.configuration.knownHostsFile == nil)
        await manager.shutdown()
    }

    @Test("SSH failures surface a blocking alert without a bypass")
    func sshFailureIsBlocking() async throws {
        let temporary = TemporaryConnectionStore()
        defer { temporary.cleanUp() }
        let saved = SavedConnection(
            label: "Secure",
            host: "secure.example",
            user: "alice",
            sessionID: "secure-session"
        )
        try await temporary.store.save([saved])

        let harness = ConnectionAttemptHarness()
        let manager = ConnectionManager(
            store: temporary.store,
            attemptFactory: { harness.makeAttempt($0) }
        )
        await manager.load()
        manager.connect(id: saved.id)
        let attempt = try #require(harness.attempts.first)
        let hint = KnownHostsHint(name: "secure.example", presence: .absent(checkedFiles: ["/k"]))
        await attempt.emit(.sshFailedBeforeStream(stderr: "Host key verification failed.", hint: hint))

        // PX-023 round 5: an ssh exit before the stream is a plain blocking "Connection Failed".
        // The app writes only the endpoint and the local known_hosts hint; ssh's stderr follows
        // verbatim under its label. The alert's only action is OK (ConnectionManagerView).
        try await AsyncTestSupport.observed("blocking SSH failure alert") {
            manager.alert?.kind == .connectionFailed
        }
        #expect(manager.alert?.title == "Connection Failed")
        #expect(manager.alert?.message == "SRUI could not connect to alice@secure.example.\n\n" + hint.text
            + "\n\nssh reported (this text can include messages from the server):\nHost key verification failed.")
        #expect(manager.status(for: saved.id) == .disconnected(resumeAvailable: true))
        #expect(harness.attempts.count == 1)

        // A failed lookup leaves no hint, only the generic text.
        manager.dismissAlert()
        manager.connect(id: saved.id)
        let second = try #require(harness.attempts.last)
        await second.emit(.sshFailedBeforeStream(stderr: "", hint: nil))
        try await AsyncTestSupport.observed("alert without hint") { manager.alert != nil }
        #expect(manager.alert?.message == "SRUI could not connect to alice@secure.example.\n\nssh reported (this text can include messages from the server):\n")

        // Any other failure keeps its message, with the same title.
        manager.dismissAlert()
        manager.connect(id: saved.id)
        let third = try #require(harness.attempts.last)
        await third.emit(.failed("Host key verification failed."))
        try await AsyncTestSupport.observed("blocking non-ssh failure alert") { manager.alert != nil }
        #expect(manager.alert?.kind == .connectionFailed)
        #expect(manager.alert?.title == "Connection Failed")
        #expect(manager.alert?.message == "Host key verification failed.")
        await manager.shutdown()
    }

    @Test("pending first-success rows are filtered from unrelated saves")
    func pendingFirstSuccessIsNeverPersistedIncidentally() async throws {
        let temporary = TemporaryConnectionStore()
        defer { temporary.cleanUp() }
        let saved = SavedConnection(
            label: "Existing",
            host: "existing.example",
            user: "alice",
            sessionID: "existing-session"
        )
        try await temporary.store.save([saved])

        let harness = ConnectionAttemptHarness()
        let manager = ConnectionManager(
            store: temporary.store,
            attemptFactory: { harness.makeAttempt($0) }
        )
        await manager.load()

        let pendingID = try #require(manager.connect(ConnectDraft(
            label: "Pending",
            host: "pending.example",
            user: "bob"
        )))
        manager.remove(id: saved.id)

        #expect(manager.entries.map(\.id) == [pendingID])
        await manager.waitForPersistenceForTesting()
        #expect(try await temporary.store.load().isEmpty)
        await manager.shutdown()
    }

    @Test("removing a disconnected connection releases its session context")
    func removalReleasesDisconnectedContext() async throws {
        let temporary = TemporaryConnectionStore()
        defer { temporary.cleanUp() }
        let saved = SavedConnection(
            label: "Disconnected",
            host: "disconnected.example",
            user: "alice",
            sessionID: "disconnected-session"
        )
        try await temporary.store.save([saved])

        let harness = ConnectionAttemptHarness()
        let manager = ConnectionManager(
            store: temporary.store,
            attemptFactory: { harness.makeAttempt($0) }
        )
        await manager.load()
        manager.connect(id: saved.id)

        let attempt = try #require(harness.attempts.first)
        let context = WeakConnectionSessionContext(
            try #require(harness.requests.first).context
        )
        harness.discardRequests()
        await attempt.emit(.stopped)
        try await AsyncTestSupport.eventually(timeout: .roundTrip, description: "attempt ownership released") {
            manager.status(for: saved.id) == .disconnected(resumeAvailable: true)
                && manager.hasActiveAttemptForTesting(saved.id) == false
        }

        #expect(context.value != nil)
        manager.remove(id: saved.id)
        #expect(manager.entries.isEmpty)
        #expect(context.value == nil)
        await manager.waitForPersistenceForTesting()
        await manager.shutdown()
    }

    @Test("removal saves remain ordered and shutdown waits for them")
    func removalPersistenceIsOrderedThroughShutdown() async throws {
        let temporary = TemporaryConnectionStore()
        defer { temporary.cleanUp() }
        let first = SavedConnection(
            label: "First",
            host: "first.example",
            user: "alice",
            sessionID: "first-session"
        )
        let second = SavedConnection(
            label: "Second",
            host: "second.example",
            user: "bob",
            sessionID: "second-session"
        )
        try await temporary.store.save([first, second])

        let firstSaveGate = AsyncGate()
        let probe = PersistenceSaveProbe(firstSaveGate: firstSaveGate)
        let orderedStore = SavedConnectionStore(
            url: temporary.file,
            beforeSave: { snapshot in
                await probe.observe(snapshot)
            }
        )
        let manager = ConnectionManager(
            store: orderedStore,
            attemptFactory: { _ in TestConnectionAttempt() }
        )
        await manager.load()

        manager.remove(id: first.id)
        #expect(manager.entries == [second])
        await firstSaveGate.waitUntilPaused()

        manager.remove(id: second.id)
        #expect(manager.entries.isEmpty)
        let shutdownTask = Task { @MainActor in
            await manager.shutdown()
        }
        try await AsyncTestSupport.eventually(timeout: .roundTrip, description: "shutdown entered") {
            manager.isShuttingDownForTesting
        }

        await firstSaveGate.release()
        await shutdownTask.value

        let snapshots = await probe.snapshots()
        #expect(snapshots == [[second], [], []])
        #expect(try await temporary.store.load().isEmpty)
    }

    @Test("replacement disclosure survives an immediate failure until dismissal")
    func replacementDisclosureSurvivesImmediateFailure() async throws {
        let temporary = TemporaryConnectionStore()
        defer { temporary.cleanUp() }
        let saved = SavedConnection(
            label: "Replacement Failure",
            host: "replacement-failure.example",
            user: "alice",
            sessionID: "old-session",
            lastKnownRevision: 9
        )
        try await temporary.store.save([saved])

        let harness = ConnectionAttemptHarness()
        let manager = ConnectionManager(
            store: temporary.store,
            attemptFactory: { harness.makeAttempt($0) }
        )
        await manager.load()
        manager.connect(id: saved.id)
        let attempt = try #require(harness.attempts.first)

        await attempt.emit(.replaced(
            previousSessionID: "old-session",
            newSessionID: "new-session"
        ))
        await attempt.emit(.failed("catch-up failed"))
        try await AsyncTestSupport.observed("replacement failure disclosed") {
            manager.status(for: saved.id) == .disconnected(resumeAvailable: true)
                && manager.alert?.kind == .sessionReplaced
                && manager.alert?.message.contains("new-session") == true
                && manager.alert?.message.contains(
                    "The replacement connection then failed: catch-up failed"
                ) == true
        }

        manager.dismissAlert()
        #expect(manager.alert == nil)
        await manager.shutdown()
    }

    @Test("shutdown does not persist a connection that never became ready")
    func shutdownBeforeReadyDoesNotPersistDraft() async throws {
        let temporary = TemporaryConnectionStore()
        defer { temporary.cleanUp() }
        let harness = ConnectionAttemptHarness()
        let manager = ConnectionManager(
            store: temporary.store,
            attemptFactory: { harness.makeAttempt($0) }
        )

        let connectionID = try #require(manager.connect(ConnectDraft(
            label: "Pending",
            host: "pending.example",
            user: "alice"
        )))
        let attempt = try #require(harness.attempts.first)

        await manager.shutdown()

        #expect(manager.entries.isEmpty)
        #expect(manager.status(for: connectionID) == .unknown)
        #expect(try await temporary.store.load().isEmpty)
        #expect(await attempt.stopCount() > 0)
    }

    @Test("deleting a row is local bookkeeping and does not stop its session")
    func deletionIsLocalOnly() async throws {
        let temporary = TemporaryConnectionStore()
        defer { temporary.cleanUp() }
        let saved = SavedConnection(
            label: "Keep Remote",
            host: "remote.example",
            user: "alice",
            sessionID: "remote-session"
        )
        try await temporary.store.save([saved])

        let harness = ConnectionAttemptHarness()
        let manager = ConnectionManager(
            store: temporary.store,
            attemptFactory: { harness.makeAttempt($0) }
        )
        await manager.load()
        manager.connect(id: saved.id)
        let attempt = try #require(harness.attempts.first)
        let context = WeakConnectionSessionContext(
            try #require(harness.requests.first).context
        )
        harness.discardRequests()
        try await AsyncTestSupport.eventuallyAsync(timeout: .roundTrip, description: "attempt start") {
            await attempt.startCount() == 1
        }

        manager.remove(id: saved.id)
        #expect(manager.entries.isEmpty)
        await manager.waitForPersistenceForTesting()
        #expect(try await temporary.store.load().isEmpty)
        #expect(await attempt.stopCount() == 0)
        #expect(context.value != nil)

        await attempt.emit(.stopped)
        try await AsyncTestSupport.eventually(timeout: .roundTrip, description: "removed connection context release") {
            context.value == nil
        }

        await manager.shutdown()
        await manager.shutdown()
    }

    // MARK: - Client-owned session-window status (PX-022-G01; §17, §18)

    private func stale(_ state: String, _ revision: UInt64) -> String {
        "\(state) — showing data from revision \(revision) (stale)"
    }

    /// Commits revision 1 with one titled Surface to `context`'s replica and mounts it, so the
    /// status can be read off a real `NSWindow`. `shown` orders it in, as the session controller
    /// does after its first mount (concealed in the test host).
    private func mountTitledSurface(
        in context: ConnectionSessionContext,
        shown: Bool = false
    ) throws -> NSWindow {
        let revision = try context.applier.apply(
            baseRevision: .initial,
            operations: [
                .createNode(
                    id: NodeId(1),
                    nodeType: .surface,
                    properties: [Property(property: .label, value: .string("Server Title"))]
                ),
            ]
        ).get()
        #expect(revision == Revision(1))
        try context.renderer.attach(store: context.applier.store)
        if shown {
            context.renderer.showWindows()
        }
        let window = try #require(context.renderer.registry.handle(for: NodeId(1))?.window)
        #expect(window.isVisible == shown)
        return window
    }

    /// A dropped context's windows are closed, not left on screen without their status.
    private func expectClosed(_ window: NSWindow, of context: ConnectionSessionContext) {
        #expect(window.isVisible == false)
        #expect(NSApplication.shared.windows.contains { $0 === window && $0.isVisible } == false)
        #expect(context.renderer.registry.surfaceHandles.isEmpty)
        #expect(context.renderer.sessionWindowStatus == nil)
    }

    @Test("every connection status maps to its session-window chrome")
    func connectionStatusMapsToSessionWindowStatus() {
        #expect(ConnectionStatus.unknown.sessionWindowStatus(lastAppliedRevision: 9) == nil)
        #expect(ConnectionStatus.connected.sessionWindowStatus(lastAppliedRevision: 9) == nil)
        #expect(
            ConnectionStatus.connecting.sessionWindowStatus(lastAppliedRevision: 9)
                == stale("connecting", 9)
        )
        #expect(
            ConnectionStatus.resynchronizing.sessionWindowStatus(lastAppliedRevision: 9)
                == stale("resynchronizing", 9)
        )
        #expect(
            ConnectionStatus.disconnected(resumeAvailable: true)
                .sessionWindowStatus(lastAppliedRevision: 9) == stale("disconnected", 9)
        )
        #expect(
            ConnectionStatus.disconnected(resumeAvailable: false)
                .sessionWindowStatus(lastAppliedRevision: 0) == stale("disconnected", 0)
        )
        #expect(ConnectionStatus.sessionReplacedWindowStatus == "session replaced — rebuilding")
    }

    @Test("session windows follow every connection status transition")
    func sessionWindowsFollowConnectionStatus() async throws {
        let temporary = TemporaryConnectionStore()
        defer { temporary.cleanUp() }
        let saved = SavedConnection(
            label: "Window Status",
            host: "window-status.example",
            user: "alice",
            sessionID: "window-session"
        )
        try await temporary.store.save([saved])

        let harness = ConnectionAttemptHarness()
        let manager = ConnectionManager(
            store: temporary.store,
            attemptFactory: { harness.makeAttempt($0) }
        )
        await manager.load()

        manager.connect(id: saved.id)
        let first = try #require(harness.attempts.first)
        let context = try #require(harness.requests.first).context
        #expect(manager.status(for: saved.id) == .connecting)
        #expect(context.renderer.sessionWindowStatus == stale("connecting", 0))
        // A window mounted while the status stands gets it.
        let window = try mountTitledSurface(in: context)
        #expect(window.subtitle == stale("connecting", 0))

        await first.emit(.ready(sessionID: "window-session", revision: 1))
        try await AsyncTestSupport.observed("first attempt ready") {
            manager.status(for: saved.id) == .connected
        }
        #expect(context.renderer.sessionWindowStatus == nil)
        #expect(window.subtitle.isEmpty)
        #expect(window.title == "Server Title")

        await first.emit(.stopped)
        try await AsyncTestSupport.observed("transport lost") {
            manager.status(for: saved.id) == .disconnected(resumeAvailable: true)
        }
        #expect(window.subtitle == stale("disconnected", 1))
        #expect(window.accessibilityHelp() == stale("disconnected", 1))
        #expect(window.title == "Server Title")

        manager.connect(id: saved.id)
        let second = try #require(harness.attempts.last)
        #expect(second !== first)
        #expect(harness.requests.last?.context === context)
        #expect(window.subtitle == stale("connecting", 1))

        await second.emit(.resynchronizing)
        try await AsyncTestSupport.observed("catch-up") {
            manager.status(for: saved.id) == .resynchronizing
        }
        #expect(window.subtitle == stale("resynchronizing", 1))

        await second.emit(.ready(sessionID: "window-session", revision: 1))
        try await AsyncTestSupport.observed("catch-up ready") {
            manager.status(for: saved.id) == .connected
        }
        #expect(window.subtitle.isEmpty)
        #expect(window.accessibilityHelp() == nil)

        await manager.shutdown()
        #expect(manager.status(for: saved.id) == .disconnected(resumeAvailable: true))
        #expect(window.subtitle == stale("disconnected", 1))
        #expect(window.title == "Server Title")
    }

    @Test("REPLACED says the session is rebuilding until ready clears it")
    func replacedThenReadyClearsSessionWindowStatus() async throws {
        let temporary = TemporaryConnectionStore()
        defer { temporary.cleanUp() }
        let saved = SavedConnection(
            label: "Replaced Window",
            host: "replaced-window.example",
            user: "alice",
            sessionID: "old-session",
            lastKnownRevision: 30
        )
        try await temporary.store.save([saved])

        let harness = ConnectionAttemptHarness()
        let manager = ConnectionManager(
            store: temporary.store,
            attemptFactory: { harness.makeAttempt($0) }
        )
        await manager.load()
        manager.connect(id: saved.id)
        let attempt = try #require(harness.attempts.first)
        let context = try #require(harness.requests.first).context

        await attempt.emit(.replaced(previousSessionID: "old-session", newSessionID: "new-session"))
        try await AsyncTestSupport.observed("replacement decision") {
            manager.status(for: saved.id) == .resynchronizing
                && manager.alert?.kind == .sessionReplaced
        }
        #expect(context.renderer.sessionWindowStatus == ConnectionStatus.sessionReplacedWindowStatus)
        // The replacement's snapshot mounts while the decision stands.
        let window = try mountTitledSurface(in: context)
        #expect(window.subtitle == "session replaced — rebuilding")
        #expect(window.title == "Server Title")

        await attempt.emit(.ready(sessionID: "new-session", revision: 1))
        try await AsyncTestSupport.observed("replacement ready") {
            manager.status(for: saved.id) == .connected
        }
        #expect(context.renderer.sessionWindowStatus == nil)
        #expect(window.subtitle.isEmpty)
        #expect(window.title == "Server Title")
        await manager.shutdown()
    }

    @Test("a superseded attempt's late failure cannot overwrite the newer window status")
    func supersededAttemptCannotOverwriteSessionWindowStatus() async throws {
        let temporary = TemporaryConnectionStore()
        defer { temporary.cleanUp() }
        let saved = SavedConnection(
            label: "Window Race",
            host: "window-race.example",
            user: "alice",
            sessionID: "race-session"
        )
        try await temporary.store.save([saved])

        let firstStartGate = AsyncGate()
        let harness = ConnectionAttemptHarness()
        harness.startPlans = [
            .init(gate: firstStartGate, error: .startFailed("stale failure")),
            .init(),
        ]
        let manager = ConnectionManager(
            store: temporary.store,
            attemptFactory: { harness.makeAttempt($0) }
        )
        await manager.load()

        manager.connect(id: saved.id)
        let context = try #require(harness.requests.first).context
        let window = try mountTitledSurface(in: context)
        await firstStartGate.waitUntilPaused()
        manager.connect(id: saved.id)
        let secondAttempt = try #require(harness.attempts.last)
        await secondAttempt.emit(.resynchronizing)
        try await AsyncTestSupport.observed("newer attempt catching up") {
            manager.status(for: saved.id) == .resynchronizing
        }
        #expect(window.subtitle == stale("resynchronizing", 1))

        await firstStartGate.release()
        let firstAttempt = try #require(harness.attempts.first)
        try await AsyncTestSupport.eventuallyAsync(timeout: .roundTrip, description: "stale attempt teardown") {
            await firstAttempt.stopCount() > 0
        }
        #expect(manager.status(for: saved.id) == .resynchronizing)
        #expect(context.renderer.sessionWindowStatus == stale("resynchronizing", 1))
        #expect(window.subtitle == stale("resynchronizing", 1))

        await secondAttempt.emit(.ready(sessionID: "race-session", revision: 1))
        try await AsyncTestSupport.observed("newer attempt ready") {
            manager.status(for: saved.id) == .connected
        }
        #expect(window.subtitle.isEmpty)
        await manager.shutdown()
    }

    @Test("a failed first connection closes the windows it had mounted")
    func failedFirstConnectionClosesItsWindows() async throws {
        let temporary = TemporaryConnectionStore()
        defer { temporary.cleanUp() }
        let saveGate = AsyncGate()
        // Holds the failed attempt in its failure save, before its save and stop finish.
        let gatedStore = SavedConnectionStore(url: temporary.file, beforeSave: { _ in
            await saveGate.pause()
        })
        let harness = ConnectionAttemptHarness()
        let manager = ConnectionManager(
            store: gatedStore,
            attemptFactory: { harness.makeAttempt($0) }
        )

        let draft = ConnectDraft(id: UUID(), label: "Never", host: "never.example", user: "alice")
        _ = try #require(manager.connect(draft))
        let context = try #require(harness.requests.first).context
        let window = try mountTitledSurface(in: context, shown: true)
        #expect(window.subtitle == stale("connecting", 0))
        let attempt = try #require(harness.attempts.first)
        await attempt.emit(.failed("initial failure"))
        try await AsyncTestSupport.observed("failed draft removed") {
            manager.entries.isEmpty
        }
        // Closed at once: no window waits out the save and stop without its status.
        #expect(manager.hasActiveAttemptForTesting(draft.id))
        expectClosed(window, of: context)

        await saveGate.release()
        try await AsyncTestSupport.eventually(timeout: .roundTrip, description: "failed attempt released") {
            manager.hasActiveAttemptForTesting(draft.id) == false
        }
        expectClosed(window, of: context)
        await manager.shutdown()
    }

    /// A real `SessionController` attempt whose snapshot render pauses on `renderGate` and whose
    /// stop pauses on `stopGate` before it retires the mount and resource owner. Both hooks are
    /// per controller, so nothing leaks into other tests.
    private func gatedControllerManager(
        store: SavedConnectionStore,
        transports: TransportBox,
        renderGate: AsyncGate,
        stopGate: AsyncGate
    ) -> ConnectionManager {
        ConnectionManager(store: store, attemptFactory: { request in
            let transport = ConnectionManagerCaptureTransport()
            transports.values.append(transport)
            transports.contexts.append(request.context)
            return ConnectionManager.makeSessionControllerAttempt(
                request: request,
                transport: transport,
                configureController: { controller in
                    controller.rendererResourcesPreloadedInterceptorForTesting = {
                        await renderGate.pause()
                    }
                    controller.stopWillRetireMountForTesting = {
                        await stopGate.pause()
                    }
                }
            )
        })
    }

    /// WELCOME plus a one-Surface snapshot, whose render then pauses on the render gate.
    private func sendWelcomeAndSnapshot(to transport: ConnectionManagerCaptureTransport) async throws {
        _ = try await waitForFrame(0, from: transport)
        var welcome = SRUIServerWelcome()
        welcome.coreVersion = SRUICoreVersion
        welcome.sessionID = "gated-session"
        welcome.requiredProfiles = ["org.srui.standard-widgets/1"]
        welcome.initialRevision = 1
        var message = SRUIMessage()
        message.serverWelcome = welcome
        try await transport.receive(message)
        try await transport.receive(textSnapshot(value: "gated value"))
    }

    private func visibleWindows(of context: ConnectionSessionContext) -> [NSWindow] {
        context.renderer.registry.surfaceHandles.compactMap(\.window).filter(\.isVisible)
    }

    @Test("a render that lands while the failed attempt is stopping is closed after the stop")
    func renderDuringStopIsClosedAfterStop() async throws {
        let temporary = TemporaryConnectionStore()
        defer { temporary.cleanUp() }
        let renderGate = AsyncGate()
        let stopGate = AsyncGate()
        let transports = TransportBox()
        let manager = gatedControllerManager(
            store: temporary.store,
            transports: transports,
            renderGate: renderGate,
            stopGate: stopGate
        )
        let draft = ConnectDraft(id: UUID(), label: "Stopping", host: "stopping.example", user: "alice")
        _ = try #require(manager.connect(draft))
        let transport = try #require(transports.values.first)
        let context = try #require(transports.contexts.first)
        try await sendWelcomeAndSnapshot(to: transport)
        await renderGate.waitUntilPaused()

        await transport.fail()
        try await AsyncTestSupport.observed("failed draft removed") {
            manager.entries.isEmpty
        }
        // The failed attempt's own stop is running, paused before it retires the mount.
        await stopGate.waitUntilPaused()
        await renderGate.release()
        try await AsyncTestSupport.eventually(timeout: .roundTrip, description: "render landed during stop") {
            self.visibleWindows(of: context).isEmpty == false
        }
        #expect(manager.hasActiveAttemptForTesting(draft.id))

        await stopGate.release()
        try await AsyncTestSupport.eventually(timeout: .roundTrip, description: "failed attempt released") {
            manager.hasActiveAttemptForTesting(draft.id) == false
        }
        #expect(visibleWindows(of: context).isEmpty)
        #expect(context.renderer.registry.surfaceHandles.isEmpty)
        #expect(context.renderer.sessionWindowStatus == nil)
        await manager.shutdown()
    }

    @Test("shutdown's stop and the failed attempt's own stop share one stop before the close")
    func shutdownStopAndTailStopShareOneStop() async throws {
        let temporary = TemporaryConnectionStore()
        defer { temporary.cleanUp() }
        let renderGate = AsyncGate()
        let saveGate = AsyncGate()
        let stopGate = AsyncGate()
        let transports = TransportBox()
        let gatedStore = SavedConnectionStore(url: temporary.file, beforeSave: { _ in
            await saveGate.pause()
        })
        let manager = gatedControllerManager(
            store: gatedStore,
            transports: transports,
            renderGate: renderGate,
            stopGate: stopGate
        )
        let draft = ConnectDraft(id: UUID(), label: "Quit", host: "quit.example", user: "alice")
        _ = try #require(manager.connect(draft))
        let transport = try #require(transports.values.first)
        let context = try #require(transports.contexts.first)
        try await sendWelcomeAndSnapshot(to: transport)
        await renderGate.waitUntilPaused()

        await transport.fail()
        try await AsyncTestSupport.observed("failed draft removed") {
            manager.entries.isEmpty
        }
        // The app quits while the failed attempt is still in its failure save.
        let shutdown = Task { @MainActor in await manager.shutdown() }
        await stopGate.waitUntilPaused()
        await saveGate.release()
        try await AsyncTestSupport.observed("failure reported, the attempt's tail reaches stop()") {
            manager.alert?.kind == .connectionFailed
        }
        // The tail must not finish while shutdown's stop is still running. This bound only
        // limits how long a regression takes to show; a correct stop never ends inside it.
        let tailEndedDuringStop = (try? await AsyncTestSupport.eventually(
            timeout: .milliseconds(500),
            description: "tail finished during the stop"
        ) {
            manager.hasActiveAttemptForTesting(draft.id) == false
        }) != nil
        #expect(tailEndedDuringStop == false)

        await renderGate.release()
        try await AsyncTestSupport.eventually(timeout: .roundTrip, description: "render landed during stop") {
            self.visibleWindows(of: context).isEmpty == false
        }
        await stopGate.release()
        await shutdown.value
        #expect(visibleWindows(of: context).isEmpty, "a window survived shutdown with no status")
        #expect(context.renderer.registry.surfaceHandles.isEmpty)
        #expect(context.renderer.sessionWindowStatus == nil)
    }

    @Test("a failed draft's late render is closed even when the same draft is retried meanwhile")
    func lateRenderOfFailedDraftClosedAfterSameDraftRetry() async throws {
        let temporary = TemporaryConnectionStore()
        defer { temporary.cleanUp() }
        let renderGate = AsyncGate()
        let saveGate = AsyncGate()
        let stopGate = AsyncGate()
        await stopGate.release()
        let transports = TransportBox()
        let gatedStore = SavedConnectionStore(url: temporary.file, beforeSave: { _ in
            await saveGate.pause()
        })
        let manager = gatedControllerManager(
            store: gatedStore,
            transports: transports,
            renderGate: renderGate,
            stopGate: stopGate
        )
        let id = UUID()
        _ = try #require(manager.connect(ConnectDraft(id: id, label: "Retry", host: "retry.example", user: "alice")))
        let transport = try #require(transports.values.first)
        let failed = try #require(transports.contexts.first)
        try await sendWelcomeAndSnapshot(to: transport)
        await renderGate.waitUntilPaused()

        await transport.fail()
        try await AsyncTestSupport.observed("failed draft removed") {
            manager.entries.isEmpty
        }
        // Retry the same draft while the failed attempt is still in its failure save: the entry
        // with that ID is back, owned by a fresh context.
        _ = try #require(manager.connect(ConnectDraft(id: id, label: "Retry", host: "retry.example", user: "alice")))
        let retried = try #require(transports.contexts.last)
        #expect(retried !== failed)
        #expect(manager.entries.contains { $0.id == id })

        // The failed attempt's paused render lands in its dropped context after the retry.
        await renderGate.release()
        try await AsyncTestSupport.eventually(timeout: .roundTrip, description: "late render landed") {
            self.visibleWindows(of: failed).isEmpty == false
        }
        await saveGate.release()
        // Its tail closes it: the context is no longer owned, whatever the entries say.
        try await AsyncTestSupport.eventually(timeout: .roundTrip, description: "dropped context closed") {
            self.visibleWindows(of: failed).isEmpty && failed.renderer.registry.surfaceHandles.isEmpty
        }
        #expect(failed.renderer.sessionWindowStatus == nil)
        #expect(retried.renderer.sessionWindowStatus == stale("connecting", 0))
        await manager.shutdown()
    }

    @Test("a removed live entry's windows close as soon as its session fails, before the stop")
    func removedLiveEntryClosesOnFailureBeforeStop() async throws {
        let temporary = TemporaryConnectionStore()
        defer { temporary.cleanUp() }
        let saved = SavedConnection(label: "Live", host: "live.example", user: "alice")
        try await temporary.store.save([saved])
        let renderGate = AsyncGate()
        let stopGate = AsyncGate()
        await renderGate.release()
        let transports = TransportBox()
        let manager = gatedControllerManager(
            store: temporary.store,
            transports: transports,
            renderGate: renderGate,
            stopGate: stopGate
        )
        await manager.load()
        manager.connect(id: saved.id)
        let transport = try #require(transports.values.first)
        let context = try #require(transports.contexts.first)
        try await sendWelcomeAndSnapshot(to: transport)
        try await AsyncTestSupport.eventually(timeout: .roundTrip, description: "live window") {
            manager.status(for: saved.id) == .connected && self.visibleWindows(of: context).isEmpty == false
        }
        #expect(context.renderer.sessionWindowStatus == nil)

        // The row is deleted; the session keeps running and its window stays, live.
        manager.remove(id: saved.id)
        #expect(visibleWindows(of: context).isEmpty == false)

        // The transport fails; the attempt's stop is paused before it retires the mount.
        await transport.fail()
        await stopGate.waitUntilPaused()
        // Nothing will reconnect a deleted row: its window must not stay up reading as live.
        #expect(visibleWindows(of: context).isEmpty)
        #expect(context.renderer.registry.surfaceHandles.isEmpty)
        #expect(context.renderer.sessionWindowStatus == nil)

        await stopGate.release()
        try await AsyncTestSupport.eventually(timeout: .roundTrip, description: "attempt released") {
            manager.hasActiveAttemptForTesting(saved.id) == false
        }
        #expect(visibleWindows(of: context).isEmpty)
        await manager.shutdown()
    }

    @Test("a removed entry whose attempt fails to start closes its windows before the stop")
    func removedEntryFailedStartClosesBeforeStop() async throws {
        let temporary = TemporaryConnectionStore()
        defer { temporary.cleanUp() }
        let saved = SavedConnection(
            label: "Removed Start",
            host: "removed-start.example",
            user: "alice",
            sessionID: "removed-start-session"
        )
        try await temporary.store.save([saved])
        let startGate = AsyncGate()
        let stopGate = AsyncGate()
        let harness = ConnectionAttemptHarness()
        harness.startPlans = [
            .init(gate: startGate, error: .startFailed("start failed"), stopGate: stopGate),
        ]
        let manager = ConnectionManager(
            store: temporary.store,
            attemptFactory: { harness.makeAttempt($0) }
        )
        await manager.load()
        manager.connect(id: saved.id)
        let context = try #require(harness.requests.first).context
        let window = try mountTitledSurface(in: context, shown: true)
        await startGate.waitUntilPaused()

        // Deleting the row keeps the starting attempt and its window.
        manager.remove(id: saved.id)
        #expect(window.isVisible)
        await startGate.release()
        // The start failure reaches `markDisconnected` for a missing entry; the attempt's stop is
        // paused, so its tail has not closed anything yet.
        await stopGate.waitUntilPaused()
        #expect(manager.hasActiveAttemptForTesting(saved.id))
        expectClosed(window, of: context)

        await stopGate.release()
        try await AsyncTestSupport.eventually(timeout: .roundTrip, description: "attempt released") {
            manager.hasActiveAttemptForTesting(saved.id) == false
        }
        expectClosed(window, of: context)
        await manager.shutdown()
    }

    @Test("a render in flight when a first connection fails cannot leave a window open")
    func inFlightRenderAfterFailedFirstConnectionIsClosed() async throws {
        let temporary = TemporaryConnectionStore()
        defer { temporary.cleanUp() }
        let renderGate = AsyncGate()
        let saveGate = AsyncGate()
        let transports = TransportBox()
        // Holds the manager in the failure's save: after it dropped the context, before
        // `runAttempt` reaches `attempt.stop()`.
        let gatedStore = SavedConnectionStore(url: temporary.file, beforeSave: { _ in
            await saveGate.pause()
        })
        let manager = ConnectionManager(store: gatedStore, attemptFactory: { request in
            let transport = ConnectionManagerCaptureTransport()
            transports.values.append(transport)
            transports.contexts.append(request.context)
            return ConnectionManager.makeSessionControllerAttempt(
                request: request,
                transport: transport,
                configureController: { controller in
                    controller.rendererResourcesPreloadedInterceptorForTesting = {
                        await renderGate.pause()
                    }
                }
            )
        })

        let draft = ConnectDraft(id: UUID(), label: "Race", host: "race.example", user: "alice")
        _ = try #require(manager.connect(draft))
        let transport = try #require(transports.values.first)
        let context = try #require(transports.contexts.first)
        _ = try await waitForFrame(0, from: transport)
        var welcome = SRUIServerWelcome()
        welcome.coreVersion = SRUICoreVersion
        welcome.sessionID = "race-session"
        welcome.requiredProfiles = ["org.srui.standard-widgets/1"]
        welcome.initialRevision = 1
        var message = SRUIMessage()
        message.serverWelcome = welcome
        try await transport.receive(message)
        try await transport.receive(textSnapshot(value: "race value"))
        // The snapshot's render is in flight, paused before it mounts.
        await renderGate.waitUntilPaused()
        #expect(context.renderer.registry.isEmpty)

        await transport.fail()
        try await AsyncTestSupport.observed("failed draft removed") {
            manager.entries.isEmpty
        }
        #expect(manager.hasActiveAttemptForTesting(draft.id))
        await renderGate.release()
        // The late render mounts the snapshot and shows its window after the context was dropped.
        try await AsyncTestSupport.eventually(timeout: .roundTrip, description: "late render mounted") {
            context.renderer.registry.surfaceHandles.contains { $0.window?.isVisible == true }
        }
        let window = try #require(context.renderer.registry.surfaceHandles.first?.window)
        #expect(context.renderer.sessionWindowStatus == nil)

        await saveGate.release()
        try await AsyncTestSupport.eventually(timeout: .roundTrip, description: "failed attempt released") {
            manager.hasActiveAttemptForTesting(draft.id) == false
        }
        expectClosed(window, of: context)
        await manager.shutdown()
    }

    @Test("removing a disconnected entry closes its windows")
    func removingDisconnectedEntryClosesItsWindows() async throws {
        let temporary = TemporaryConnectionStore()
        defer { temporary.cleanUp() }
        let saved = SavedConnection(
            label: "Removed",
            host: "removed.example",
            user: "alice",
            sessionID: "removed-session"
        )
        try await temporary.store.save([saved])
        let harness = ConnectionAttemptHarness()
        let manager = ConnectionManager(
            store: temporary.store,
            attemptFactory: { harness.makeAttempt($0) }
        )
        await manager.load()
        manager.connect(id: saved.id)
        let attempt = try #require(harness.attempts.last)
        let context = try #require(harness.requests.last).context
        let window = try mountTitledSurface(in: context, shown: true)
        await attempt.emit(.stopped)
        try await AsyncTestSupport.eventually(timeout: .roundTrip, description: "attempt ownership released") {
            manager.status(for: saved.id) == .disconnected(resumeAvailable: true)
                && manager.hasActiveAttemptForTesting(saved.id) == false
        }
        #expect(window.isVisible)
        #expect(window.subtitle == stale("disconnected", 1))

        manager.remove(id: saved.id)
        expectClosed(window, of: context)
        await manager.waitForPersistenceForTesting()
        await manager.shutdown()
    }

    @Test("a removed entry's running session keeps its window status, then closes when it ends")
    func removedEntryWithRunningAttemptFollowsThenCloses() async throws {
        let temporary = TemporaryConnectionStore()
        defer { temporary.cleanUp() }
        let saved = SavedConnection(
            label: "Removed While Connecting",
            host: "removed-connecting.example",
            user: "alice",
            sessionID: "removed-connecting-session"
        )
        try await temporary.store.save([saved])
        let harness = ConnectionAttemptHarness()
        let manager = ConnectionManager(
            store: temporary.store,
            attemptFactory: { harness.makeAttempt($0) }
        )
        await manager.load()
        manager.connect(id: saved.id)
        let attempt = try #require(harness.attempts.last)
        let context = try #require(harness.requests.last).context
        let window = try mountTitledSurface(in: context, shown: true)
        try await AsyncTestSupport.eventuallyAsync(timeout: .roundTrip, description: "attempt start") {
            await attempt.startCount() == 1
        }

        // Removing the row is local bookkeeping: the session keeps running, and its window stays.
        manager.remove(id: saved.id)
        #expect(manager.entries.isEmpty)
        #expect(window.isVisible)
        #expect(window.subtitle == stale("connecting", 0))

        await attempt.emit(.connecting)
        try await AsyncTestSupport.eventually(timeout: .roundTrip, description: "removed entry reconnecting") {
            context.renderer.sessionWindowStatus == stale("connecting", 1)
        }
        await attempt.emit(.replaced(
            previousSessionID: "removed-connecting-session",
            newSessionID: "removed-connecting-replacement"
        ))
        try await AsyncTestSupport.eventually(timeout: .roundTrip, description: "removed entry replaced") {
            context.renderer.sessionWindowStatus == ConnectionStatus.sessionReplacedWindowStatus
        }
        #expect(window.subtitle == "session replaced — rebuilding")
        // A removed row raises no replacement alert.
        #expect(manager.alert == nil)

        await attempt.emit(.resynchronizing)
        try await AsyncTestSupport.eventually(timeout: .roundTrip, description: "removed entry catching up") {
            context.renderer.sessionWindowStatus == stale("resynchronizing", 1)
        }
        await attempt.emit(.ready(sessionID: "removed-connecting-session", revision: 1))
        try await AsyncTestSupport.eventually(timeout: .roundTrip, description: "removed entry live") {
            context.renderer.sessionWindowStatus == nil
        }
        #expect(window.isVisible)
        #expect(window.subtitle.isEmpty)

        await attempt.emit(.stopped)
        try await AsyncTestSupport.eventually(timeout: .roundTrip, description: "removed session ended") {
            manager.hasActiveAttemptForTesting(saved.id) == false
                && context.renderer.registry.surfaceHandles.isEmpty
        }
        expectClosed(window, of: context)
        await manager.shutdown()
    }

    // MARK: - PX-095: several isolated sessions

    /// Two saved hosts with the same local name, as two machines serving the same application
    /// often are: the account is the same, the hosts and ports differ.
    private func twoHosts() -> (a: SavedConnection, b: SavedConnection) {
        (
            SavedConnection(label: "srtop", host: "host-a.example", port: 2228, user: "alice", sessionID: "session-a"),
            SavedConnection(label: "srtop", host: "host-b.example", port: 2229, user: "alice", sessionID: "session-b")
        )
    }

    private func connectTwoHosts(
        _ temporary: TemporaryConnectionStore
    ) async throws -> (ConnectionManager, ConnectionAttemptHarness, SavedConnection, SavedConnection) {
        let (a, b) = twoHosts()
        try await temporary.store.save([a, b])
        let harness = ConnectionAttemptHarness()
        let manager = ConnectionManager(store: temporary.store, attemptFactory: { harness.makeAttempt($0) })
        await manager.load()
        manager.connect(id: a.id)
        manager.connect(id: b.id)
        #expect(harness.requests.map(\.connection.id) == [a.id, b.id])
        return (manager, harness, a, b)
    }

    @Test("two concurrent connections run in session contexts that share nothing")
    func concurrentConnectionsShareNoSessionState() async throws {
        let temporary = TemporaryConnectionStore()
        defer { temporary.cleanUp() }
        let (manager, harness, a, b) = try await connectTwoHosts(temporary)
        let contextA = harness.requests[0].context, contextB = harness.requests[1].context
        #expect(contextA !== contextB)
        #expect(contextA.applier !== contextB.applier, "replicas, and so cached rows and item identities")
        #expect(contextA.outbox !== contextB.outbox, "outboxes, and so event_seq and pending events")
        #expect(contextA.outbox.clientInstanceId != contextB.outbox.clientInstanceId)
        #expect(contextA.resourceCache !== contextB.resourceCache)
        #expect(contextA.transactionIngressGate !== contextB.transactionIngressGate)
        #expect(contextA.continuityContext !== contextB.continuityContext, "reconnect generations")
        #expect(contextA.renderer !== contextB.renderer, "render objects, selection memory and collection caches")
        #expect(contextA.renderer.controlFactory !== contextB.renderer.controlFactory)
        #expect(manager.hasActiveAttemptForTesting(a.id) && manager.hasActiveAttemptForTesting(b.id))
        #expect(harness.requests[0].resumeSessionID == nil && harness.requests[1].resumeSessionID == nil)

        // A selection made in one session's table is remembered by that renderer alone.
        let item = ItemId(7)
        contextA.renderer.controlFactory.collectionSelectionMemory[NodeId(5)] =
            CollectionSelectionMemory(promoted: nil, local: [item])
        #expect(contextB.renderer.controlFactory.collectionSelectionMemory[NodeId(5)] == nil)
        await manager.shutdown()
    }

    @Test("every window names its host, account and incarnation, and follows only its own session")
    func windowIdentityNamesHostAccountAndIncarnation() async throws {
        let temporary = TemporaryConnectionStore()
        defer { temporary.cleanUp() }
        let (manager, harness, a, b) = try await connectTwoHosts(temporary)
        let contextA = harness.requests[0].context, contextB = harness.requests[1].context
        let windowA = try mountTitledSurface(in: contextA)
        let windowB = try mountTitledSurface(in: contextB)
        #expect(windowA.title == windowB.title, "the server's titles cannot tell the hosts apart")
        #expect(windowA.sessionIdentityText == "alice@host-a.example:2228 · no session attached yet · saved as “srtop”")
        #expect(windowB.sessionIdentityText == "alice@host-b.example:2229 · no session attached yet · saved as “srtop”")
        #expect(windowA.tabbingIdentifier == "org.srui.session.\(a.id.uuidString)")
        #expect(windowB.tabbingIdentifier == "org.srui.session.\(b.id.uuidString)")

        await harness.attempts[0].emit(.ready(sessionID: "session-a", revision: 1))
        await harness.attempts[1].emit(.ready(sessionID: "session-b", revision: 1))
        try await AsyncTestSupport.observed("both ready") {
            manager.status(for: a.id) == .connected && manager.status(for: b.id) == .connected
        }
        #expect(windowA.sessionIdentityText == "alice@host-a.example:2228 · session session-a · saved as “srtop”")
        #expect(windowB.sessionIdentityText == "alice@host-b.example:2229 · session session-b · saved as “srtop”")

        // A's server is replaced: A's windows name the new incarnation; B's are untouched.
        await harness.attempts[0].emit(.replaced(previousSessionID: "session-a", newSessionID: "session-a2"))
        try await AsyncTestSupport.observed("A replaced") {
            manager.status(for: a.id) == .resynchronizing && manager.alert?.kind == .sessionReplaced
        }
        #expect(contextA.renderer.sessionWindowIdentity?.text == "alice@host-a.example:2228 · session session-a2 · saved as “srtop”")
        #expect(windowB.sessionIdentityText == "alice@host-b.example:2229 · session session-b · saved as “srtop”")
        #expect(manager.alert?.connectionID == a.id)
        #expect(manager.alert?.message.contains("Remote session session-a on alice@host-a.example:2228 was replaced by session-a2") == true,
                "\(manager.alert?.message ?? "no alert")")

        // A server-reported ID that fails the saved file's rule is never shown.
        await harness.attempts[1].emit(.ready(sessionID: "bad\u{1B}id", revision: 1))
        try await AsyncTestSupport.eventually(timeout: .roundTrip, description: "B's invalid ID") {
            contextB.renderer.sessionWindowIdentity?.text.contains("with an invalid ID") == true
        }
        #expect(windowB.sessionIdentityText?.contains("\u{1B}") == false)

        // A local name equal to the endpoint is not repeated.
        #expect(ConnectionManager.sessionWindowIdentity(
            for: SavedConnection(label: "bob@[fe80::1]", host: "fe80::1", user: "bob"), incarnation: "s"
        ).text == "bob@[fe80::1] · session s")
        await manager.shutdown()
    }

    @Test("replacing, reconnecting, failing or removing one connection leaves the other session intact")
    func oneConnectionsLifecycleLeavesTheOtherIntact() async throws {
        let temporary = TemporaryConnectionStore()
        defer { temporary.cleanUp() }
        let (manager, harness, a, b) = try await connectTwoHosts(temporary)
        let contextA = harness.requests[0].context, contextB = harness.requests[1].context
        let windowA = try mountTitledSurface(in: contextA)
        let windowB = try mountTitledSurface(in: contextB, shown: true)
        defer { windowB.orderOut(nil); windowB.close() }
        #expect(windowA !== windowB)
        await harness.attempts[0].emit(.ready(sessionID: "session-a", revision: 1))
        await harness.attempts[1].emit(.ready(sessionID: "session-b", revision: 1))
        try await AsyncTestSupport.observed("both ready") {
            manager.status(for: a.id) == .connected && manager.status(for: b.id) == .connected
        }
        let attemptB = harness.attempts[1]
        func bIsIntact(_ step: String) async {
            #expect(manager.status(for: b.id) == .connected, "\(step): B's status")
            #expect(manager.hasActiveAttemptForTesting(b.id), "\(step): B's attempt")
            #expect(await attemptB.stopCount() == 0, "\(step): B's attempt was never stopped")
            #expect(contextB.applier.lastAppliedRevision == Revision(1), "\(step): B's replica")
            #expect(contextB.renderer.registry.handle(for: NodeId(1))?.window === windowB, "\(step): B's window")
            #expect(windowB.isVisible && windowB.subtitle.isEmpty, "\(step): B's window chrome")
            #expect(windowB.sessionIdentityText?.contains("session session-b") == true, "\(step): B's identity")
            #expect(manager.entries.first { $0.id == b.id }?.sessionID == "session-b", "\(step): B's saved session")
        }

        await harness.attempts[0].emit(.replaced(previousSessionID: "session-a", newSessionID: "session-a2"))
        await harness.attempts[0].emit(.ready(sessionID: "session-a2", revision: 1))
        try await AsyncTestSupport.observed("A rebuilt") {
            manager.status(for: a.id) == .connected && manager.entries.first?.sessionID == "session-a2"
        }
        await bIsIntact("A replaced")

        // A new attempt for A supersedes A's own, never B's.
        manager.connect(id: a.id)
        #expect(harness.requests.count == 3 && harness.requests[2].connection.id == a.id)
        #expect(harness.requests[2].context === contextA)
        try await AsyncTestSupport.eventuallyAsync(timeout: .roundTrip, description: "A's first attempt superseded") {
            await harness.attempts[0].stopCount() == 1
        }
        await bIsIntact("A reconnected")

        await harness.attempts[2].emit(.failed("connection dropped"))
        try await AsyncTestSupport.observed("A failed") {
            manager.status(for: a.id) == .disconnected(resumeAvailable: true)
        }
        await bIsIntact("A failed")

        manager.remove(id: a.id)
        try await AsyncTestSupport.observed("A removed") { manager.entries.map(\.id) == [b.id] }
        await bIsIntact("A removed")
        await manager.shutdown()
    }

    @Test("one connection's attempt never clears another's alert, and a folded-in alert names its host")
    func alertsStayBoundToTheirConnection() async throws {
        let temporary = TemporaryConnectionStore()
        defer { temporary.cleanUp() }
        let (manager, harness, a, b) = try await connectTwoHosts(temporary)
        await harness.attempts[0].emit(.ready(sessionID: "session-a", revision: 1))
        await harness.attempts[1].emit(.ready(sessionID: "session-b", revision: 1))
        try await AsyncTestSupport.observed("both ready") {
            manager.status(for: a.id) == .connected && manager.status(for: b.id) == .connected
        }

        await harness.attempts[0].emit(.failed("A's link dropped"))
        try await AsyncTestSupport.observed("A's failure") { manager.alert?.connectionID == a.id }
        manager.connect(id: b.id)
        #expect(manager.alert?.connectionID == a.id && manager.alert?.message == "A's link dropped",
                "B's reconnect does not dismiss A's alert")
        manager.connect(id: a.id)
        #expect(manager.alert == nil, "A's own reconnect does")
        #expect(harness.requests.map(\.connection.id) == [a.id, b.id, b.id, a.id])

        await harness.attempts[3].emit(.ready(sessionID: "session-a", revision: 1))
        await harness.attempts[3].emit(.replaced(previousSessionID: "session-a", newSessionID: "session-a2"))
        try await AsyncTestSupport.observed("A replaced") { manager.alert?.kind == .sessionReplaced }
        await harness.attempts[2].emit(.failed("B's link dropped"))
        try await AsyncTestSupport.observed("B's failure folded in") {
            manager.alert?.message.contains("Connection Failed (alice@host-b.example:2229): B's link dropped") == true
        }
        #expect(manager.alert?.kind == .sessionReplaced && manager.alert?.connectionID == a.id)
        await manager.shutdown()
    }
}
