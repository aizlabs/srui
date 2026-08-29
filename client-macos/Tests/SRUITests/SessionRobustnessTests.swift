//
// SessionRobustnessTests.swift
// SRUITests
//
// Regression tests for session/transport robustness defects (§7.7, §12.1, §18, §18.2, §22.2).
//
// Each test here encodes one spec invariant that the implementation violated:
// - §12.1 committed revisions are monotonic and a duplicate transaction never rewrites state.
// - §18 snapshot replacement is driven only by an explicit SERVER RESYNC_REQUIRED.
// - §18.2 the outbound event cache is bounded.
// - §7.7 `observed_revision` is the revision the user saw when the control was activated.
// - §4 inv. 13 unrecoverable divergence fails explicitly instead of degrading silently.
//

import Testing
import Foundation
import AppKit
import SemanticModel
import Protocol
import Session
import TransportSSH
import RendererAppKit

@Suite("Session Robustness Tests")
struct SessionRobustnessTests {

    // MARK: - Helpers

    private static func framed(_ tx: Transaction) throws -> Data {
        var msg = SRUIMessage()
        msg.transaction = tx.toWire()
        return try SRUIFraming.encodeFramed(msg)
    }

    private static func framedWelcome(sessionId: String = "test-session") throws -> Data {
        var welcome = SRUIServerWelcome()
        welcome.sessionID = sessionId
        welcome.requiredProfiles = ["org.srui.standard-widgets/1"]
        var msg = SRUIMessage()
        msg.serverWelcome = welcome
        return try SRUIFraming.encodeFramed(msg)
    }

    private static func framedResyncRequired(revision: UInt64) throws -> Data {
        var resync = SRUIServerResyncRequired()
        resync.sessionID = "test-session"
        resync.snapshotRevision = revision
        resync.reason = "journal evicted"
        resync.continuity = .sameSession
        var msg = SRUIMessage()
        msg.serverResyncRequired = resync
        return try SRUIFraming.encodeFramed(msg)
    }

    private static func waitUntil(
        timeout: TimeInterval = 2.0,
        _ condition: @Sendable () async -> Bool
    ) async -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if await condition() { return true }
            try? await Task.sleep(nanoseconds: 10_000_000)
        }
        return await condition()
    }

    private static func surfaceAndText(_ text: String) -> [SemanticModel.Operation] {
        [
            .createNode(id: NodeId(1), nodeType: .surface),
            .createNode(
                id: NodeId(2),
                nodeType: .text,
                parentID: NodeId(1),
                properties: [Property(property: .text, value: .string(text))]
            ),
        ]
    }

    // MARK: - §12.1 / §18: a duplicated transaction must never replace a live replica

    @Test("Duplicate initial transaction does not wipe the replica or regress the revision")
    @MainActor
    func duplicateInitialTransactionDoesNotWipeLiveReplica() async throws {
        let (clientTransport, serverTransport) = await PipeTransport.createPair()
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
        let controller = SessionController(
            transport: clientTransport,
            applier: applier,
            renderer: renderer
        )
        controller.attachRenderer(renderer)
        try await controller.start()
        try await serverTransport.send(data: try Self.framedWelcome())

        let initialTx = Transaction(
            baseRevision: .initial,
            newRevision: Revision(1),
            operations: Self.surfaceAndText("Count: 0")
        )
        try await serverTransport.send(data: try Self.framed(initialTx))
        #expect(await Self.waitUntil { applier.lastAppliedRevision == Revision(1) })

        // Advance to revision 5.
        for rev in 1..<5 {
            let tx = Transaction(
                baseRevision: Revision(UInt64(rev)),
                newRevision: Revision(UInt64(rev + 1)),
                operations: [
                    .setProperty(id: NodeId(2), property: .text, value: .string("Count: \(rev)")),
                ]
            )
            try await serverTransport.send(data: try Self.framed(tx))
        }
        #expect(await Self.waitUntil { applier.lastAppliedRevision == Revision(5) })

        // A re-delivery of the very first transaction. It carries base_revision == 0, but it is a
        // duplicate, NOT a resync snapshot: no SERVER RESYNC_REQUIRED preceded it (§18).
        try await serverTransport.send(data: try Self.framed(initialTx))
        try await Task.sleep(nanoseconds: 150_000_000)

        // §12.1: revisions are monotonic and never repeat downwards.
        #expect(applier.lastAppliedRevision == Revision(5))
        #expect(applier.store.node(for: NodeId(2))?.getProperty(.text) == .string("Count: 4"))

        await controller.stop()
        await serverTransport.close()
    }

    // MARK: - §12.1: applySnapshot must not regress the revision

    @Test("Resync snapshot cannot regress the committed revision")
    func applySnapshotRejectsRevisionRegression() {
        let applier = TransactionApplier()
        _ = applier.apply(baseRevision: .initial, operations: Self.surfaceAndText("a"))
        _ = applier.apply(
            baseRevision: Revision(1),
            operations: [.setProperty(id: NodeId(2), property: .text, value: .string("b"))]
        )
        _ = applier.apply(
            baseRevision: Revision(2),
            operations: [.setProperty(id: NodeId(2), property: .text, value: .string("c"))]
        )
        #expect(applier.lastAppliedRevision == Revision(3))

        let regressing = Transaction(
            baseRevision: .initial,
            newRevision: Revision(2),
            operations: Self.surfaceAndText("stale")
        )
        let result = applier.applySnapshot(record: regressing)

        guard case .failure = result else {
            Issue.record("snapshot at revision 2 must be rejected while committed at revision 3")
            return
        }
        #expect(applier.lastAppliedRevision == Revision(3))
        #expect(applier.store.node(for: NodeId(2))?.getProperty(.text) == .string("c"))
    }

    // MARK: - §7.7: observed_revision is sampled at activation time

    @Test("ACTIVATE reports the revision observed at click time, not a later one")
    @MainActor
    func activateReportsRevisionObservedAtClickTime() async throws {
        let (clientTransport, serverTransport) = await PipeTransport.createPair()
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
        let controller = SessionController(
            transport: clientTransport,
            applier: applier,
            renderer: renderer
        )
        controller.attachRenderer(renderer)
        let serverStream = serverTransport.receiveStream()
        try await controller.start()

        var welcome = SRUIServerWelcome()
        welcome.sessionID = "default"
        welcome.requiredProfiles = ["org.srui.standard-widgets/1"]
        var welcomeMessage = SRUIMessage()
        welcomeMessage.serverWelcome = welcome
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(welcomeMessage))

        let buttonID = NodeId(4)
        let mountTx = Transaction(
            baseRevision: .initial,
            newRevision: Revision(1),
            operations: [
                .createNode(id: NodeId(1), nodeType: .surface),
                .createNode(
                    id: buttonID,
                    nodeType: .button,
                    parentID: NodeId(1),
                    properties: [Property(property: .label, value: .string("Increment"))]
                ),
            ]
        )
        try await serverTransport.send(data: try Self.framed(mountTx))
        #expect(await Self.waitUntil { applier.lastAppliedRevision == Revision(1) })

        let buttonHandle = try #require(renderer.registry.handle(for: buttonID))
        let trampoline = try #require(buttonHandle.actionTrampoline as? ActionTrampoline)

        // The user clicks while looking at revision 1.
        trampoline.performAction(nil)

        // Before the outbound dispatch gets a chance to run, revision 2 commits. This is exactly
        // the race §7.7 cares about: the server validates the action "at the event's observed
        // revision", so the event must still say 1 — the state the user actually acted on.
        _ = applier.apply(
            baseRevision: Revision(1),
            operations: [.setProperty(id: buttonID, property: .enabled, value: .bool(false))]
        )
        #expect(applier.lastAppliedRevision == Revision(2))

        var streamDecoder = SRUIMessageStreamDecoder()
        var observed: Revision?
        for try await chunk in serverStream {
            for msg in try streamDecoder.appendAndExtract(incoming: chunk) {
                if case .event(let wireEvent) = msg.msg {
                    observed = try ProtocolDecoder()
                        .validateAndConvertEvent(wire: wireEvent).observedRevision
                }
            }
            if observed != nil { break }
        }

        #expect(observed == Revision(1))

        await controller.stop()
        await serverTransport.close()
    }
    // MARK: - §18.2: the pending-event sequence window is bounded

    @Test("Outbox applies backpressure without discarding an unacknowledged sequence")
    func outboxBoundsPendingEventCache() async throws {
        let (clientTransport, serverTransport) = await PipeTransport.createPair()
        let outbox = EventOutbox()

        for _ in 0..<EventOutbox.defaultMaxPendingEvents {
            try await outbox.sendActivate(
                nodeId: NodeId(7),
                observedRevision: Revision(1),
                via: clientTransport
            )
        }

        await #expect(throws: EventOutboxError.self) {
            try await outbox.sendActivate(
                nodeId: NodeId(7),
                observedRevision: Revision(1),
                via: clientTransport
            )
        }
        #expect(await outbox.pendingCount == EventOutbox.defaultMaxPendingEvents)
        #expect(await outbox.eventSeq == UInt64(EventOutbox.defaultMaxPendingEvents))

        await clientTransport.close()
        await serverTransport.close()
    }

    // MARK: - §4 inv. 13: divergence is explicit, not silent

    @Test("A gap in the transaction stream ends the session instead of degrading silently")
    @MainActor
    func missedTransactionEndsSessionExplicitly() async throws {
        let (clientTransport, serverTransport) = await PipeTransport.createPair()
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
        let controller = SessionController(
            transport: clientTransport,
            applier: applier,
            renderer: renderer
        )
        controller.attachRenderer(renderer)
        try await controller.start()
        try await serverTransport.send(data: try Self.framedWelcome())

        try await serverTransport.send(
            data: try Self.framed(
                Transaction(
                    baseRevision: .initial,
                    newRevision: Revision(1),
                    operations: Self.surfaceAndText("Count: 0")
                )
            )
        )
        #expect(await Self.waitUntil { applier.lastAppliedRevision == Revision(1) })

        // We are at revision 1 and the server jumps to 6: revisions 2...5 were lost. The replica can
        // never catch up by listening, so the session must fail loudly and force a resume (§18).
        try await serverTransport.send(
            data: try Self.framed(
                Transaction(
                    baseRevision: Revision(5),
                    newRevision: Revision(6),
                    operations: [
                        .setProperty(id: NodeId(2), property: .text, value: .string("Count: 5")),
                    ]
                )
            )
        )

        let torndown = await Self.waitUntil {
            do {
                try await serverTransport.send(data: Data([0x00]))
                return false
            } catch {
                return true
            }
        }
        #expect(torndown, "diverged session must drop the transport so the client can resume (§18)")
        #expect(applier.lastAppliedRevision == Revision(1))

        await controller.stop()
        await serverTransport.close()
    }

    // MARK: - §18: a failed snapshot must not consume the pending-resync latch

    @Test("A rejected resync snapshot leaves the resync pending for the next snapshot")
    @MainActor
    func failedResyncSnapshotKeepsResyncPending() async throws {
        let (clientTransport, serverTransport) = await PipeTransport.createPair()
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
        let controller = SessionController(
            transport: clientTransport,
            applier: applier,
            renderer: renderer
        )
        controller.attachRenderer(renderer)
        try await controller.start()
        try await serverTransport.send(data: try Self.framedWelcome())

        // Server evicted the journal before we ever applied anything.
        try await serverTransport.send(data: try Self.framedResyncRequired(revision: 3))

        // First snapshot attempt is malformed and must be rejected without side effects (§12.1).
        try await serverTransport.send(
            data: try Self.framed(
                Transaction(
                    baseRevision: .initial,
                    newRevision: Revision(3),
                    operations: [
                        .setProperty(id: NodeId(2), property: .text, value: .string("orphan")),
                    ]
                )
            )
        )
        try await Task.sleep(nanoseconds: 100_000_000)
        #expect(applier.lastAppliedRevision == .initial)

        // The server retries with a good snapshot at the same revision. The resync is still
        // outstanding, so this must be applied as a snapshot (its new_revision is 3, not base + 1).
        try await serverTransport.send(
            data: try Self.framed(
                Transaction(
                    baseRevision: .initial,
                    newRevision: Revision(3),
                    operations: Self.surfaceAndText("After resync")
                )
            )
        )

        #expect(await Self.waitUntil { applier.lastAppliedRevision == Revision(3) })
        #expect(applier.store.node(for: NodeId(2))?.getProperty(.text) == .string("After resync"))

        await controller.stop()
        await serverTransport.close()
    }

    // MARK: - §22.2: a renderer failure forces a full re-attach

    @Test("Renderer failure forces a full re-attach on the next transaction")
    @MainActor
    func rendererFailureForcesFullReattach() async throws {
        let (clientTransport, serverTransport) = await PipeTransport.createPair()
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
        let controller = SessionController(
            transport: clientTransport,
            applier: applier,
            renderer: renderer
        )
        controller.attachRenderer(renderer)
        try await controller.start()
        try await serverTransport.send(data: try Self.framedWelcome())

        try await serverTransport.send(
            data: try Self.framed(
                Transaction(
                    baseRevision: .initial,
                    newRevision: Revision(1),
                    operations: Self.surfaceAndText("Count: 0")
                )
            )
        )
        #expect(await Self.waitUntil { applier.lastAppliedRevision == Revision(1) })
        #expect(renderer.registry.count == 2)

        // Force the view tree out of sync with the committed store, so the next incremental apply
        // throws the way a partially-failed structural remount would leave it.
        _ = renderer.registry.remove(NodeId(2))
        #expect(renderer.registry.count == 1)

        try await serverTransport.send(
            data: try Self.framed(
                Transaction(
                    baseRevision: Revision(1),
                    newRevision: Revision(2),
                    operations: [
                        .setProperty(id: NodeId(2), property: .text, value: .string("Count: 1")),
                    ]
                )
            )
        )
        #expect(await Self.waitUntil { applier.lastAppliedRevision == Revision(2) })

        // The next transaction must rebuild the whole tree from the committed store rather than
        // keep mutating a tree the renderer already failed on.
        try await serverTransport.send(
            data: try Self.framed(
                Transaction(
                    baseRevision: Revision(2),
                    newRevision: Revision(3),
                    operations: [
                        .setProperty(id: NodeId(2), property: .text, value: .string("Count: 2")),
                    ]
                )
            )
        )
        #expect(await Self.waitUntil { applier.lastAppliedRevision == Revision(3) })

        #expect(await Self.waitUntil { await MainActor.run { renderer.registry.count == 2 } })
        let textHandle = try #require(renderer.registry.handle(for: NodeId(2)))
        let textField = try #require(textHandle.view as? NSTextField)
        #expect(textField.stringValue == "Count: 2")

        await controller.stop()
        await serverTransport.close()
    }

    // MARK: - §18 / §4 inv. 13: a clean peer EOF is a failure, not a normal shutdown

    @Test("A peer that closes the stream cleanly reports a transport failure")
    @MainActor
    func cleanPeerEOFReportsTransportFailure() async throws {
        let (clientTransport, serverTransport) = await PipeTransport.createPair()
        let applier = TransactionApplier()
        let controller = SessionController(transport: clientTransport, applier: applier)

        let failures = FailureBox()
        controller.onFailure = { failure in
            Task { await failures.record(failure) }
        }
        try await controller.start()
        try await serverTransport.send(data: try Self.framedWelcome())

        try await serverTransport.send(
            data: try Self.framed(
                Transaction(
                    baseRevision: .initial,
                    newRevision: Revision(1),
                    operations: Self.surfaceAndText("Count: 0")
                )
            )
        )
        #expect(await Self.waitUntil { applier.lastAppliedRevision == Revision(1) })

        // A socket EOF finishes the receive stream *without* throwing. The session must still fail
        // loudly so the caller reconnects and resumes (§18, §4 inv. 13).
        await serverTransport.close()

        #expect(await Self.waitUntil { await failures.count == 1 })
        let reported = await failures.first
        guard case .transportEnded = reported else {
            Issue.record("clean EOF must report .transportEnded, got \(String(describing: reported))")
            return
        }
        #expect(controller.isDiverged, "a session with no reader must not look healthy")

        await controller.stop()
    }

    @Test("stop() does not report a failure when the stream finishes")
    @MainActor
    func intentionalStopReportsNoFailure() async throws {
        let (clientTransport, serverTransport) = await PipeTransport.createPair()
        let controller = SessionController(transport: clientTransport)

        let failures = FailureBox()
        controller.onFailure = { failure in
            Task { await failures.record(failure) }
        }
        try await controller.start()

        await controller.stop()
        try? await Task.sleep(nanoseconds: 150_000_000)

        #expect(await failures.count == 0, "an intentional stop is not a session failure")
        #expect(!controller.isDiverged)

        await serverTransport.close()
    }

    // MARK: - §18: a stopped session must be restartable

    @Test("stop() clears the divergence and resync latches so the next session tracks again")
    @MainActor
    func stopClearsDivergenceLatch() async throws {
        let (clientTransport, serverTransport) = await PipeTransport.createPair()
        let applier = TransactionApplier()
        let controller = SessionController(transport: clientTransport, applier: applier)
        try await controller.start()
        try await serverTransport.send(data: try Self.framedWelcome())

        try await serverTransport.send(
            data: try Self.framed(
                Transaction(
                    baseRevision: .initial,
                    newRevision: Revision(1),
                    operations: Self.surfaceAndText("Count: 0")
                )
            )
        )
        #expect(await Self.waitUntil { applier.lastAppliedRevision == Revision(1) })

        // Diverge: revisions 2...5 were never delivered.
        try await serverTransport.send(
            data: try Self.framed(
                Transaction(
                    baseRevision: Revision(5),
                    newRevision: Revision(6),
                    operations: [
                        .setProperty(id: NodeId(2), property: .text, value: .string("Count: 5")),
                    ]
                )
            )
        )
        #expect(await Self.waitUntil { controller.isDiverged })

        // A restart re-handshakes and the server answers with replay or a snapshot, so the latch
        // must not survive: otherwise every transaction of the next session is silently dropped.
        await controller.stop()
        #expect(!controller.isDiverged)

        var welcomeMsg = SRUIMessage()
        var welcome = SRUIServerWelcome()
        welcome.sessionID = "test-session"
        welcome.requiredProfiles = ["org.srui.standard-widgets/1"]
        welcomeMsg.serverWelcome = welcome
        await controller.handleIncomingMessage(welcomeMsg)

        var msg = SRUIMessage()
        msg.transaction = Transaction(
            baseRevision: Revision(1),
            newRevision: Revision(2),
            operations: [
                .setProperty(id: NodeId(2), property: .text, value: .string("Count: 1")),
            ]
        ).toWire()
        await controller.handleIncomingMessage(msg)

        #expect(applier.lastAppliedRevision == Revision(2))
        #expect(applier.store.node(for: NodeId(2))?.getProperty(.text) == .string("Count: 1"))

        await serverTransport.close()
    }

    @Test("A handshake that fails to send leaves the controller startable")
    @MainActor
    func failedHandshakeDoesNotWedgeController() async throws {
        let transport = FlakyTransport(failFirstSend: true)
        let applier = TransactionApplier()
        let controller = SessionController(transport: transport, applier: applier)

        await #expect(throws: TransportError.self) {
            try await controller.start()
        }
        #expect(await transport.sentFrameCount == 0)

        // The failed attempt started nothing, so the second attempt must actually run — not return
        // early on a stale `isRunning` latch.
        try await controller.start()
        #expect(await transport.sentFrameCount == 1, "restart must re-send CLIENT RESUME (§18)")

        await controller.stop()
    }

    // MARK: - Transport lifecycle

    @Test("A closed socket transport refuses to silently reconnect")
    func closedTransportRefusesToReconnect() async throws {
        let transport = UnixSocketTransport(
            socketPath: "/tmp/srui-never-bound-\(UUID().uuidString).sock"
        )
        await transport.close()

        var caught: TransportError?
        do {
            try await transport.send(data: Data([0x01]))
        } catch let error as TransportError {
            caught = error
        }

        guard let caught else {
            Issue.record("send() on a closed transport must throw")
            return
        }
        guard case .closed = caught else {
            Issue.record("closed transport must stay closed, but it tried to reconnect: \(caught)")
            return
        }
    }

    @Test("Releasing one pipe end gives the survivor an EOF instead of hanging it")
    func releasedPipePeerFinishesSurvivorStream() async throws {
        var pair: (client: PipeTransport, server: PipeTransport)? = await PipeTransport.createPair()
        let survivor = pair!.client
        let stream = survivor.receiveStream()

        let ended = EndedFlag()
        let consumer = Task {
            for try await _ in stream {}
            await ended.markEnded()
        }

        // Only the peer feeds `survivor`'s stream, so dropping it must terminate that stream.
        pair = nil

        #expect(await Self.waitUntil { await ended.isEnded })
        consumer.cancel()
    }

    @Test("A connected pipe pair is released once callers drop it")
    func pipeTransportPairDoesNotLeak() async throws {
        weak var weakClient: PipeTransport?
        weak var weakServer: PipeTransport?

        var pair: (client: PipeTransport, server: PipeTransport)? = await PipeTransport.createPair()
        weakClient = pair?.client
        weakServer = pair?.server
        #expect(weakClient != nil)
        #expect(weakServer != nil)
        pair = nil

        #expect(weakClient == nil, "PipeTransport pair retains itself through a peer cycle")
        #expect(weakServer == nil, "PipeTransport pair retains itself through a peer cycle")
    }
}

/// Records that an `AsyncThrowingStream` consumer observed end-of-stream.
private actor EndedFlag {
    private var ended = false
    func markEnded() { ended = true }
    var isEnded: Bool { ended }
}

/// Transport whose first `send` fails, modelling a handshake that never reaches the server.
private actor FlakyTransport: Transport {
    private var failNextSend: Bool
    private var sentFrames: [Data] = []
    private let stream: AsyncThrowingStream<Data, Error>
    private let continuation: AsyncThrowingStream<Data, Error>.Continuation

    init(failFirstSend: Bool) {
        self.failNextSend = failFirstSend
        let (stream, continuation) = AsyncThrowingStream<Data, Error>.makeStream()
        self.stream = stream
        self.continuation = continuation
    }

    var sentFrameCount: Int { sentFrames.count }

    func send(data: Data) async throws {
        if failNextSend {
            failNextSend = false
            throw TransportError.closed
        }
        sentFrames.append(data)
    }

    nonisolated func receiveStream() -> AsyncThrowingStream<Data, Error> {
        stream
    }

    func close() async {
        continuation.finish()
    }
}

/// Collects session failures reported from the receive loop's non-isolated callback.
private actor FailureBox {
    private var failures: [SessionFailure] = []

    func record(_ failure: SessionFailure) {
        failures.append(failure)
    }

    var count: Int { failures.count }
    var first: SessionFailure? { failures.first }
}
