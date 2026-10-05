//
// SessionControllerSplitSnapshotTests.swift
// SRUITests
//
// SessionController handling of snapshots delivered in several envelopes and of a server's
// handshake refusal (§12.1, §18, §19.2, §26, §32.8).
//

import Foundation
import Protocol
import SemanticModel
import Testing

@testable import Session

@Suite("SessionController split snapshots and handshake refusal")
struct SessionControllerSplitSnapshotTests {
    private static func framed(_ message: SRUIMessage) throws -> Data {
        try SRUIFraming.encodeFramed(message)
    }

    private static func welcome(initialRevision: UInt64, snapshotParts: UInt32) -> SRUIMessage {
        var welcome = SRUIServerWelcome()
        welcome.coreVersion = SRUICoreVersion
        welcome.sessionID = "split-session"
        welcome.requiredProfiles = ["org.srui.standard-widgets/1"]
        welcome.initialRevision = initialRevision
        welcome.snapshotParts = snapshotParts
        var message = SRUIMessage()
        message.serverWelcome = welcome
        return message
    }

    private static func envelope(
        base: UInt64 = 0, revision: UInt64, _ operations: [SemanticModel.Operation]
    ) -> SRUIMessage {
        var message = SRUIMessage()
        message.transaction = Transaction(
            baseRevision: Revision(base),
            newRevision: Revision(revision),
            operations: operations
        ).toWire()
        return message
    }

    private static let surface: SemanticModel.Operation = .createNode(
        id: NodeId(1), nodeType: .surface)
    private static func text(_ id: UInt64, _ value: String) -> SemanticModel.Operation {
        .createNode(
            id: NodeId(id),
            nodeType: .text,
            parentID: NodeId(1),
            properties: [Property(property: .text, value: .string(value))]
        )
    }

    private static func waitUntil(
        timeout: TimeInterval = 5.0,
        _ condition: @Sendable () async -> Bool
    ) async -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if await condition() { return true }
            try? await Task.sleep(nanoseconds: 10_000_000)
        }
        return await condition()
    }

    @MainActor
    private static func makeController(
        limits: StoreLimits = StoreLimits()
    ) async throws -> (
        SessionController, TransactionApplier, PipeTransport, SplitFailureBox
    ) {
        let (clientTransport, serverTransport) = await PipeTransport.createPair()
        let applier = TransactionApplier(limits: limits)
        let controller = SessionController(transport: clientTransport, applier: applier)
        let failures = SplitFailureBox()
        controller.onFailure = { failure in
            Task { await failures.record(failure) }
        }
        try await controller.start()
        return (controller, applier, serverTransport, failures)
    }

    @Test("a catch-up snapshot announced in two envelopes is applied once, whole, after the last")
    @MainActor
    func splitCatchUpAppliesAtomically() async throws {
        let (controller, applier, server, failures) = try await Self.makeController()

        try await server.send(data: try Self.framed(Self.welcome(initialRevision: 3, snapshotParts: 2)))
        try await server.send(
            data: try Self.framed(Self.envelope(revision: 3, [Self.surface, Self.text(2, "first")])))
        try await server.send(data: try Self.framed(Self.envelope(revision: 3, [Self.text(3, "second")])))

        #expect(await Self.waitUntil { applier.lastAppliedRevision == Revision(3) })
        let store = applier.currentSnapshot.store
        // Had the first envelope been applied as the whole snapshot, node 3 would be missing and
        // the second envelope would have failed the session as an unclassifiable 0 -> 3 delivery.
        #expect(store.rootIDs == [NodeId(1)])
        #expect(store.getNode(NodeId(1))?.orderedChildren == [NodeId(2), NodeId(3)])
        #expect(store.getNode(NodeId(3))?.properties[.text] == .string("second"))
        #expect(await failures.count == 0)
        // The snapshot latch reopens event dispatch once the committed snapshot is finalized.
        #expect(await Self.waitUntil { controller.isEventDispatchEnabled })

        await controller.stop()
        await server.close()
    }

    @Test("a resync snapshot announced in two envelopes replaces the replica once, after the last")
    @MainActor
    func splitResyncAppliesAtomically() async throws {
        let (controller, applier, server, failures) = try await Self.makeController()

        try await server.send(data: try Self.framed(Self.welcome(initialRevision: 0, snapshotParts: 0)))
        try await server.send(
            data: try Self.framed(Self.envelope(revision: 1, [Self.surface, Self.text(2, "before")])))
        #expect(await Self.waitUntil { applier.lastAppliedRevision == Revision(1) })

        var resync = SRUIServerResyncRequired()
        resync.sessionID = "split-session"
        resync.snapshotRevision = 2
        resync.reason = "journal evicted"
        resync.continuity = .sameSession
        resync.snapshotParts = 2
        var resyncMessage = SRUIMessage()
        resyncMessage.serverResyncRequired = resync
        try await server.send(data: try Self.framed(resyncMessage))
        try await server.send(
            data: try Self.framed(Self.envelope(revision: 2, [Self.surface, Self.text(2, "after")])))
        try await server.send(data: try Self.framed(Self.envelope(revision: 2, [Self.text(3, "second")])))

        #expect(await Self.waitUntil { applier.lastAppliedRevision == Revision(2) })
        let store = applier.currentSnapshot.store
        // Had the first envelope replaced the replica on its own, node 3 would be missing and the
        // second envelope would have failed the session as an unclassifiable 0 -> 2 delivery.
        #expect(store.getNode(NodeId(1))?.orderedChildren == [NodeId(2), NodeId(3)])
        #expect(store.getNode(NodeId(2))?.properties[.text] == .string("after"))
        #expect(store.getNode(NodeId(3))?.properties[.text] == .string("second"))
        #expect(await failures.count == 0)

        await controller.stop()
        await server.close()
    }

    @Test("a refusal detail above max_string_length is discarded, not surfaced")
    @MainActor
    func oversizedRefusalDetailIsDiscarded() async throws {
        let (controller, _, server, failures) = try await Self.makeController(
            limits: StoreLimits(maxStringLength: 32))

        var refusal = SRUIServerHandshakeRefused()
        refusal.reason = .snapshotUndeliverable
        refusal.detail = String(repeating: "Z", count: 33)
        var message = SRUIMessage()
        message.serverHandshakeRefused = refusal
        try await server.send(data: try Self.framed(message))

        #expect(await Self.waitUntil { await failures.count >= 1 })
        guard case .handshakeRefused(let description)? = await failures.first else {
            Issue.record("expected .handshakeRefused, got \(String(describing: await failures.first))")
            return
        }
        #expect(description.contains("snapshot undeliverable"))
        #expect(description.contains("exceeds max_string_length 32"))
        #expect(!description.contains("ZZZZ"))

        await controller.stop()
        await server.close()
    }

    @Test("a live transaction inside a split snapshot fails loudly and nothing partial is applied")
    @MainActor
    func liveTransactionInsideSplitIsRejected() async throws {
        let (controller, applier, server, failures) = try await Self.makeController()

        try await server.send(data: try Self.framed(Self.welcome(initialRevision: 3, snapshotParts: 2)))
        try await server.send(
            data: try Self.framed(Self.envelope(revision: 3, [Self.surface, Self.text(2, "first")])))
        try await server.send(
            data: try Self.framed(Self.envelope(base: 3, revision: 4, [Self.text(3, "live")])))

        #expect(await Self.waitUntil { await failures.count == 1 })
        guard case .protocolViolation(let description)? = await failures.first else {
            Issue.record("expected a protocol violation, got \(String(describing: await failures.first))")
            return
        }
        #expect(description.contains("split snapshot rejected"))
        #expect(applier.lastAppliedRevision == Revision.initial)
        #expect(applier.currentSnapshot.store.rootIDs.isEmpty)

        await controller.stop()
        await server.close()
    }

    @Test("a decision announcing more envelopes than advertised fails before any is staged")
    @MainActor
    func announcementAboveAdvertisedPartsFails() async throws {
        let (controller, applier, server, failures) = try await Self.makeController()

        try await server.send(
            data: try Self.framed(
                Self.welcome(initialRevision: 3, snapshotParts: defaultMaxSnapshotParts + 1)))

        #expect(await Self.waitUntil { await failures.count == 1 })
        guard case .protocolViolation(let description)? = await failures.first else {
            Issue.record("expected a protocol violation, got \(String(describing: await failures.first))")
            return
        }
        #expect(description.contains("max_snapshot_parts"))
        #expect(applier.lastAppliedRevision == Revision.initial)

        await controller.stop()
        await server.close()
    }

    @Test("SERVER HANDSHAKE_REFUSED surfaces a diagnosable handshake failure")
    @MainActor
    func handshakeRefusalIsReported() async throws {
        let (controller, _, server, failures) = try await Self.makeController()

        var refusal = SRUIServerHandshakeRefused()
        refusal.reason = .snapshotUndeliverable
        refusal.detail = "snapshot needs 2 envelopes; client stages at most 1"
        var message = SRUIMessage()
        message.serverHandshakeRefused = refusal
        try await server.send(data: try Self.framed(message))

        #expect(await Self.waitUntil { await failures.count >= 1 })
        guard case .handshakeRefused(let description)? = await failures.first else {
            Issue.record("expected .handshakeRefused, got \(String(describing: await failures.first))")
            return
        }
        #expect(description.contains("snapshot undeliverable"))
        #expect(description.contains("client stages at most 1"))
        #expect(!controller.isHandshakeComplete)

        await controller.stop()
        await server.close()
    }

    @Test("the client advertises its snapshot staging bound in CLIENT HELLO")
    @MainActor
    func helloAdvertisesMaxSnapshotParts() async throws {
        let (controller, _, server, _) = try await Self.makeController()
        var decoder = SRUIMessageStreamDecoder()
        var hello: SRUIClientHello?
        for try await chunk in server.receiveStream() {
            for message in try decoder.appendAndExtract(incoming: chunk) {
                if case .clientHello(let decoded)? = message.msg { hello = decoded }
            }
            if hello != nil { break }
        }
        #expect(hello?.limits.maxSnapshotParts == defaultMaxSnapshotParts)
        await controller.stop()
        await server.close()
    }
}

/// Collects session failures reported from the receive loop's non-isolated callback.
private actor SplitFailureBox {
    private var failures: [SessionFailure] = []

    func record(_ failure: SessionFailure) {
        failures.append(failure)
    }

    var count: Int { failures.count }
    var first: SessionFailure? { failures.first }
}
