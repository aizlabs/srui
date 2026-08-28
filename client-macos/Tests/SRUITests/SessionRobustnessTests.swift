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

    private static func framedResyncRequired(revision: UInt64) throws -> Data {
        var resync = SRUIServerResyncRequired()
        resync.sessionID = "test-session"
        resync.snapshotRevision = revision
        resync.reason = "journal evicted"
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

    // MARK: - §18.2: the pending-event cache is bounded

    @Test("Outbox bounds its pending (unacknowledged) event cache")
    func outboxBoundsPendingEventCache() async throws {
        let (clientTransport, serverTransport) = await PipeTransport.createPair()
        let outbox = EventOutbox()

        for _ in 0..<600 {
            try await outbox.sendActivate(
                nodeId: NodeId(7),
                observedRevision: Revision(1),
                via: clientTransport
            )
        }

        // §18.2 / App. B: the dedupe + retry cache is explicitly a *bounded* cache.
        let pending = await outbox.pendingCount
        #expect(pending <= 256, "pending event cache grew unbounded: \(pending)")

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
