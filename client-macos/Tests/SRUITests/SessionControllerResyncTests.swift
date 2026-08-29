//
// SessionControllerResyncTests.swift
// SRUITests
//
// Resync snapshot handling and receive-loop resilience tests (§12.1, §20.2, §22).
//

import Testing
import Foundation
import AppKit
import SemanticModel
import Protocol
import Session
import TransportSSH
import RendererAppKit

@Suite("SessionController Resync & Resilience Tests")
struct SessionControllerResyncTests {

    @Test("ServerResyncRequired followed by snapshot replaces local state")
    @MainActor
    func resyncSnapshotReplacesLocalState() async throws {
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

        let surfaceID = NodeId(1)
        let textID = NodeId(2)

        let initialTx = Transaction(
            baseRevision: .initial,
            newRevision: Revision(1),
            operations: [
                .createNode(id: surfaceID, nodeType: .surface),
                .createNode(
                    id: textID,
                    nodeType: .text,
                    parentID: surfaceID,
                    properties: [Property(property: .text, value: .string("Before resync"))]
                ),
            ]
        )

        var initialMsg = SRUIMessage()
        initialMsg.transaction = initialTx.toWire()
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(initialMsg))
        try await Task.sleep(nanoseconds: 50_000_000)

        #expect(applier.lastAppliedRevision == Revision(1))

        var resyncMsg = SRUIMessage()
        var resync = SRUIServerResyncRequired()
        resync.sessionID = "test-session"
        resync.snapshotRevision = 2
        resync.reason = "session replaced"
        resync.continuity = .replaced
        resyncMsg.serverResyncRequired = resync
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(resyncMsg))

        let snapshotTx = Transaction(
            baseRevision: .initial,
            newRevision: Revision(2),
            operations: [
                .createNode(id: surfaceID, nodeType: .surface),
                .createNode(
                    id: textID,
                    nodeType: .text,
                    parentID: surfaceID,
                    properties: [Property(property: .text, value: .string("After resync"))]
                ),
            ]
        )

        var snapshotMsg = SRUIMessage()
        snapshotMsg.transaction = snapshotTx.toWire()
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(snapshotMsg))
        try await Task.sleep(nanoseconds: 50_000_000)

        #expect(applier.lastAppliedRevision == Revision(2))
        let textHandle = try #require(renderer.registry.handle(for: textID))
        let textField = try #require(textHandle.view as? NSTextField)
        #expect(textField.stringValue == "After resync")

        await controller.stop()
        await serverTransport.close()
    }

    @Test("Rejected transaction does not stop processing subsequent transactions")
    @MainActor
    func rejectedTransactionDoesNotStopReceiveLoop() async throws {
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

        let surfaceID = NodeId(1)
        let textID = NodeId(2)

        let initialTx = Transaction(
            baseRevision: .initial,
            newRevision: Revision(1),
            operations: [
                .createNode(id: surfaceID, nodeType: .surface),
                .createNode(
                    id: textID,
                    nodeType: .text,
                    parentID: surfaceID,
                    properties: [Property(property: .text, value: .string("Count: 0"))]
                ),
            ]
        )

        var initialMsg = SRUIMessage()
        initialMsg.transaction = initialTx.toWire()
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(initialMsg))
        try await Task.sleep(nanoseconds: 50_000_000)
        #expect(applier.lastAppliedRevision == Revision(1))

        let staleTx = Transaction(
            baseRevision: .initial,
            newRevision: Revision(1),
            operations: [
                .setProperty(id: textID, property: .text, value: .string("Should not apply")),
            ]
        )

        var staleMsg = SRUIMessage()
        staleMsg.transaction = staleTx.toWire()
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(staleMsg))
        try await Task.sleep(nanoseconds: 30_000_000)
        #expect(applier.lastAppliedRevision == Revision(1))

        let validTx = Transaction(
            baseRevision: Revision(1),
            newRevision: Revision(2),
            operations: [
                .setProperty(id: textID, property: .text, value: .string("Count: 1")),
            ]
        )

        var validMsg = SRUIMessage()
        validMsg.transaction = validTx.toWire()
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(validMsg))
        try await Task.sleep(nanoseconds: 50_000_000)

        #expect(applier.lastAppliedRevision == Revision(2))
        let textHandle = try #require(renderer.registry.handle(for: textID))
        let textField = try #require(textHandle.view as? NSTextField)
        #expect(textField.stringValue == "Count: 1")

        await controller.stop()
        await serverTransport.close()
    }

    // NOTE: this scenario never reaches the renderer at all — the store rejects `badTx` because of
    // its dangling parent, so no mount is ever attempted. What it actually pins down is that a
    // rejected transaction leaves no side effects and that a store-level rejection is treated as
    // replica divergence rather than being silently skipped (§12.1, §4 inv. 13). Recovery of the
    // mount flag after a genuine *renderer* failure is covered by
    // `SessionRobustnessTests.rendererFailureForcesFullReattach`.
    @Test("A rejected transaction leaves no side effects and ends the session")
    @MainActor
    func rejectedTransactionLeavesNoSideEffects() async throws {
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
        let controller = SessionController(
            transport: await PipeTransport(),
            applier: applier,
            renderer: renderer
        )
        controller.attachRenderer(renderer)

        let surfaceID = NodeId(1)
        let badTx = Transaction(
            baseRevision: .initial,
            newRevision: Revision(1),
            operations: [
                .createNode(id: surfaceID, nodeType: .surface),
                .createNode(id: NodeId(2), nodeType: .text, parentID: NodeId(999)),
            ]
        )

        await controller.handleIncomingMessage({
            var msg = SRUIMessage()
            msg.transaction = badTx.toWire()
            return msg
        }())

        #expect(applier.lastAppliedRevision == .initial)
        #expect(renderer.registry.count == 0)

        // The server committed this revision even though we could not, so the replica is now behind
        // and can only recover by resuming on a fresh transport (§18).
        #expect(controller.isDiverged)

        let goodTx = Transaction(
            baseRevision: .initial,
            newRevision: Revision(1),
            operations: [
                .createNode(id: surfaceID, nodeType: .surface),
                .createNode(
                    id: NodeId(2),
                    nodeType: .text,
                    parentID: surfaceID,
                    properties: [Property(property: .text, value: .string("Mounted"))]
                ),
            ]
        )

        await controller.handleIncomingMessage({
            var msg = SRUIMessage()
            msg.transaction = goodTx.toWire()
            return msg
        }())

        // A diverged session must not keep applying the stream as though nothing happened.
        #expect(applier.lastAppliedRevision == .initial)
        #expect(renderer.registry.count == 0)
    }
}
