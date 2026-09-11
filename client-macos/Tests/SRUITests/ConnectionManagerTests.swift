//
// ConnectionManagerTests.swift
// SRUITests
//
// Deterministic coverage for client-owned saved connections and reconnect orchestration.
//

import Foundation
@testable import ConnectionManager
import Protocol
import SemanticModel
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
    private var starts = 0
    private var stops = 0

    init(
        startGate: AsyncGate? = nil,
        startError: ConnectionManagerTestError? = nil
    ) {
        let pair = AsyncStream.makeStream(
            of: ConnectionAttemptEvent.self,
            bufferingPolicy: .bufferingNewest(16)
        )
        self.events = pair.stream
        self.continuation = pair.continuation
        self.startGate = startGate
        self.startError = startError
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
    }

    private(set) var requests: [ConnectionAttemptRequest] = []
    private(set) var attempts: [TestConnectionAttempt] = []
    var startPlans: [StartPlan] = []

    func makeAttempt(_ request: ConnectionAttemptRequest) -> any ConnectionAttempt {
        let plan = startPlans.isEmpty ? StartPlan() : startPlans.removeFirst()
        let attempt = TestConnectionAttempt(startGate: plan.gate, startError: plan.error)
        requests.append(request)
        attempts.append(attempt)
        return attempt
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

    func frame(at index: Int) -> Data? {
        guard frames.indices.contains(index) else { return nil }
        return frames[index]
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

@MainActor
@Suite("Connection Manager Tests (§6.3, §17, §18, §19.1)")
struct ConnectionManagerTests {
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

    @Test("cold relaunch emits CLIENT_RESUME from revision zero")
    func coldRelaunchResumesFromRevisionZero() async throws {
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
        let transport = ConnectionManagerCaptureTransport()
        let attempt = ConnectionManager.makeSessionControllerAttempt(
            request: request,
            transport: transport
        )

        try await attempt.start()
        let frame = try #require(await transport.frame(at: 0))
        let message = try decodeFramedMessage(from: frame)
        guard case .clientResume(let resume) = message.msg else {
            Issue.record("cold saved connection did not emit CLIENT_RESUME")
            await attempt.stop()
            return
        }
        #expect(resume.sessionID == "saved-session")
        #expect(resume.lastAppliedRevision == 0)
        #expect(request.resumeRevision == 0)

        await attempt.stop()
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

        try await AsyncTestSupport.eventually(description: "connection ready state") {
            manager.status(for: connectionID) == .connected
        }
        try await AsyncTestSupport.eventuallyAsync(
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
        try await AsyncTestSupport.eventually(description: "replacement notification") {
            manager.status(for: saved.id) == .resynchronizing
                && manager.alert?.kind == .sessionReplaced
        }
        #expect(manager.entries.first?.sessionID == "new-session")
        #expect(manager.entries.first?.lastKnownRevision == 0)
        #expect(try await temporary.store.load().first?.sessionID == "new-session")

        await attempt.emit(.ready(sessionID: "new-session", revision: 5))
        try await AsyncTestSupport.eventually(description: "replacement snapshot ready") {
            manager.status(for: saved.id) == .connected
        }
        #expect(manager.entries.first?.lastKnownRevision == 5)
        await manager.shutdown()
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
        try await AsyncTestSupport.eventually(description: "newer attempt ready") {
            manager.status(for: saved.id) == .connected
                && manager.entries.first?.sessionID == "new-session"
        }

        await firstStartGate.release()
        let firstAttempt = try #require(harness.attempts.first)
        try await AsyncTestSupport.eventuallyAsync(description: "stale attempt teardown") {
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

        try await AsyncTestSupport.eventually(description: "failed draft removed") {
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
        await attempt.emit(.failed("Host key verification failed."))

        try await AsyncTestSupport.eventually(description: "blocking SSH failure alert") {
            manager.alert?.kind == .connectionFailed
        }
        #expect(manager.alert?.title == "Connection Failed")
        #expect(manager.alert?.message == "Host key verification failed.")
        #expect(manager.status(for: saved.id) == .disconnected(resumeAvailable: true))
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
        try await AsyncTestSupport.eventuallyAsync(description: "attempt start") {
            await attempt.startCount() == 1
        }

        await manager.remove(id: saved.id)
        #expect(manager.entries.isEmpty)
        #expect(try await temporary.store.load().isEmpty)
        #expect(await attempt.stopCount() == 0)

        await manager.shutdown()
    }
}
