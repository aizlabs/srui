//
// SessionResumeContinuityTests.swift
// SRUITests
//
// Continuity-bound reconnect decisions: SAME_SESSION replay, REPLACED abandonment, fail-closed
// handling of an unknown continuity, and generation-bound resume supersession (§18, §4 inv. 13).
//

import Testing
import Foundation
import SemanticModel
import Protocol
@testable import Session
import TransportSSH

/// Drains one side of a transport and decodes the framed messages it carries.
private actor ResumeWireCollector {
    private var messages: [SRUIMessage] = []
    private var task: Task<Void, Never>?

    func start(draining transport: any Transport) {
        guard task == nil else { return }
        let stream = transport.receiveStream()
        task = Task { [weak self] in
            var decoder = SRUIMessageStreamDecoder()
            do {
                for try await chunk in stream {
                    for message in try decoder.appendAndExtract(incoming: chunk) {
                        await self?.append(message)
                    }
                }
            } catch {
                // Stream teardown at end of test; whatever was collected stays valid.
            }
        }
    }

    private func append(_ message: SRUIMessage) {
        messages.append(message)
    }

    func stop() {
        task?.cancel()
        task = nil
    }

    func collected() -> [SRUIMessage] {
        messages
    }

    func wait(forAtLeast count: Int, timeout: Double = 2.0) async -> [SRUIMessage] {
        let deadline = Date().addingTimeInterval(timeout)
        while messages.count < count && Date() < deadline {
            try? await Task.sleep(nanoseconds: 5_000_000)
        }
        return messages
    }
}

@Suite("Resume continuity decisions (§18)")
struct SessionResumeContinuityTests {

    private func events(in messages: [SRUIMessage]) throws -> [Event] {
        let decoder = ProtocolDecoder()
        return try messages.compactMap { message -> Event? in
            guard case .event(let wire) = message.msg else { return nil }
            return try decoder.validateAndConvertEvent(wire: wire)
        }
    }

    /// Snapshot transaction the server sends after `RESYNC_REQUIRED`.
    private func snapshot(revision: UInt64, text: String) -> SRUIMessage {
        let surfaceID = NodeId(1)
        let textID = NodeId(2)
        let tx = Transaction(
            baseRevision: .initial,
            newRevision: Revision(revision),
            operations: [
                .createNode(id: surfaceID, nodeType: .surface),
                .createNode(
                    id: textID,
                    nodeType: .text,
                    parentID: surfaceID,
                    properties: [Property(property: .text, value: .string(text))]
                ),
            ]
        )
        var message = SRUIMessage()
        message.transaction = tx.toWire()
        return message
    }

    private func resyncMessage(
        sessionId: String,
        continuity: Srui_Protocol_SessionContinuity,
        snapshotRevision: UInt64 = 5,
        lastProcessedEventSeq: UInt64 = 0
    ) -> SRUIMessage {
        var resync = SRUIServerResyncRequired()
        resync.sessionID = sessionId
        resync.snapshotRevision = snapshotRevision
        resync.reason = "test"
        resync.continuity = continuity
        resync.lastProcessedEventSeq = lastProcessedEventSeq
        var message = SRUIMessage()
        message.serverResyncRequired = resync
        return message
    }

    @Test("SAME_SESSION resync replays pending events, then the snapshot re-enables dispatch")
    func sameSessionResyncReplaysPendingThenAppliesSnapshot() async throws {
        let (seedClient, seedServer) = await PipeTransport.createPair()
        let outbox = EventOutbox()
        let pending = try await outbox.sendActivate(
            nodeId: NodeId(7),
            observedRevision: Revision(3),
            via: seedClient
        )

        let (client, server) = await PipeTransport.createPair()
        let collector = ResumeWireCollector()
        await collector.start(draining: server)
        let applier = TransactionApplier()
        let controller = SessionController(
            transport: client,
            applier: applier,
            outbox: outbox,
            sessionId: "session-live"
        )
        try await controller.start()

        // CLIENT RESUME only: nothing is replayed before the continuity decision arrives (§18).
        let beforeDecision = await collector.wait(forAtLeast: 1)
        #expect(try events(in: beforeDecision).isEmpty)
        #expect(controller.isEventDispatchEnabled == false)

        await controller.handleIncomingMessage(
            resyncMessage(sessionId: "session-live", continuity: .sameSession)
        )

        // The same incarnation survived, so the retained intent is replayed with its original
        // identity even though a snapshot is still outstanding.
        let afterDecision = await collector.wait(forAtLeast: 2)
        let replayed = try #require(try events(in: afterDecision).last)
        #expect(replayed.eventId == pending.eventId)
        #expect(replayed.eventSeq == pending.eventSeq)

        // New user events stay disabled until the snapshot commits.
        #expect(controller.isEventDispatchEnabled == false)
        await #expect(throws: SessionDispatchError.self) {
            try await controller.sendActivate(nodeId: NodeId(9))
        }

        await controller.handleIncomingMessage(snapshot(revision: 5, text: "after resync"))

        #expect(applier.lastAppliedRevision == Revision(5))
        #expect(controller.isEventDispatchEnabled)

        await controller.stop()
        await collector.stop()
        await seedClient.close()
        await seedServer.close()
        await server.close()
    }

    @Test("REPLACED resync abandons pending intents without sending a single replay frame")
    func replacedResyncSendsNoEventFrames() async throws {
        let (seedClient, seedServer) = await PipeTransport.createPair()
        let outbox = EventOutbox()
        _ = try await outbox.sendActivate(
            nodeId: NodeId(7),
            observedRevision: Revision(3),
            via: seedClient
        )

        let (client, server) = await PipeTransport.createPair()
        let collector = ResumeWireCollector()
        await collector.start(draining: server)
        let controller = SessionController(
            transport: client,
            outbox: outbox,
            sessionId: "session-old"
        )
        try await controller.start()
        _ = await collector.wait(forAtLeast: 1)

        await controller.handleIncomingMessage(
            resyncMessage(
                sessionId: "session-new",
                continuity: .replaced,
                lastProcessedEventSeq: 4
            )
        )

        // §18: an expired incarnation's intents are abandoned, never replayed against the
        // replacement's unrelated state.
        try? await Task.sleep(nanoseconds: 100_000_000)
        #expect(try events(in: await collector.collected()).isEmpty)
        #expect(await outbox.pendingCount == 0)
        #expect(await outbox.lastAckedEventSeq == 4)
        #expect(await outbox.eventSeq == 4)
        #expect(controller.sessionId == "session-new")
        #expect(controller.isEventDispatchEnabled == false)

        await controller.stop()
        await collector.stop()
        await seedClient.close()
        await seedServer.close()
        await server.close()
    }

    @Test("An omitted continuity is a required-semantics failure, not an implicit SAME_SESSION")
    func omittedContinuityIsHardFailure() async throws {
        try await expectContinuityFailure(continuity: .unspecified)
    }

    @Test("An unrecognized continuity is a required-semantics failure (§4 inv. 13)")
    func unrecognizedContinuityIsHardFailure() async throws {
        try await expectContinuityFailure(continuity: .UNRECOGNIZED(99))
    }

    private func expectContinuityFailure(
        continuity: Srui_Protocol_SessionContinuity
    ) async throws {
        let (seedClient, seedServer) = await PipeTransport.createPair()
        let outbox = EventOutbox()
        _ = try await outbox.sendActivate(
            nodeId: NodeId(7),
            observedRevision: Revision(3),
            via: seedClient
        )

        let (client, server) = await PipeTransport.createPair()
        let collector = ResumeWireCollector()
        await collector.start(draining: server)
        let controller = SessionController(
            transport: client,
            outbox: outbox,
            sessionId: "session-old"
        )

        let failures = FailureRecorder()
        controller.onFailure = { failure in
            Task { await failures.record(failure) }
        }
        try await controller.start()
        _ = await collector.wait(forAtLeast: 1)

        await controller.handleIncomingMessage(
            resyncMessage(sessionId: "session-old", continuity: continuity)
        )

        let failure = try #require(await failures.wait())
        guard case .protocolViolation = failure else {
            Issue.record("Expected a protocol violation, got \(failure)")
            return
        }

        // Fail closed: no replay, no rebind, no new-event allocation.
        #expect(try events(in: await collector.collected()).isEmpty)
        #expect(await outbox.pendingCount == 1)
        #expect(controller.isEventDispatchEnabled == false)
        await #expect(throws: SessionDispatchError.self) {
            try await controller.sendActivate(nodeId: NodeId(9))
        }

        await controller.stop()
        await collector.stop()
        await seedClient.close()
        await seedServer.close()
        await server.close()
    }

    @Test("A superseded REPLACED response cannot abandon intents bound to a newer attempt")
    func supersededReplacementResyncIsDiscarded() async throws {
        let (seedClient, seedServer) = await PipeTransport.createPair()
        let outbox = EventOutbox()
        let pending = try await outbox.sendActivate(
            nodeId: NodeId(7),
            observedRevision: Revision(3),
            via: seedClient
        )

        let (firstClient, firstServer) = await PipeTransport.createPair()
        let firstCollector = ResumeWireCollector()
        await firstCollector.start(draining: firstServer)
        let firstController = SessionController(
            transport: firstClient,
            outbox: outbox,
            sessionId: "session-old"
        )
        try await firstController.start()
        _ = await firstCollector.wait(forAtLeast: 1)

        // The user reconnects again before the first attempt is answered: generation 2 supersedes
        // generation 1 (§18).
        let (secondClient, secondServer) = await PipeTransport.createPair()
        let secondCollector = ResumeWireCollector()
        await secondCollector.start(draining: secondServer)
        let secondController = SessionController(
            transport: secondClient,
            outbox: outbox,
            sessionId: "session-old"
        )
        try await secondController.start()
        _ = await secondCollector.wait(forAtLeast: 1)

        // Late REPLACED answer for the abandoned connection: it must not reset the outbox.
        await firstController.handleIncomingMessage(
            resyncMessage(
                sessionId: "session-new",
                continuity: .replaced,
                lastProcessedEventSeq: 9
            )
        )
        #expect(await outbox.pendingCount == 1)
        #expect(await outbox.eventSeq == pending.eventSeq)
        #expect(await outbox.lastAckedEventSeq == 0)
        #expect(firstController.isEventDispatchEnabled == false)

        // The newest attempt's answer is honored.
        await secondController.handleIncomingMessage(
            resyncMessage(
                sessionId: "session-new",
                continuity: .replaced,
                lastProcessedEventSeq: 9
            )
        )
        #expect(await outbox.pendingCount == 0)
        #expect(await outbox.eventSeq == 9)
        #expect(try events(in: await secondCollector.collected()).isEmpty)

        await firstController.stop()
        await secondController.stop()
        await firstCollector.stop()
        await secondCollector.stop()
        await seedClient.close()
        await seedServer.close()
        await firstServer.close()
        await secondServer.close()
    }

    @Test("A superseded RESUME_OK never replays, rebinds, or re-enables event allocation")
    func supersededResumeOkCannotUnblockOlderAttempt() async throws {
        let (seedClient, seedServer) = await PipeTransport.createPair()
        let outbox = EventOutbox()
        let pending = try await outbox.sendActivate(
            nodeId: NodeId(7),
            observedRevision: Revision(3),
            via: seedClient
        )

        let (firstClient, firstServer) = await PipeTransport.createPair()
        let firstCollector = ResumeWireCollector()
        await firstCollector.start(draining: firstServer)
        let firstController = SessionController(
            transport: firstClient,
            outbox: outbox,
            sessionId: "session-old"
        )
        try await firstController.start()
        _ = await firstCollector.wait(forAtLeast: 1)

        let (secondClient, secondServer) = await PipeTransport.createPair()
        let secondCollector = ResumeWireCollector()
        await secondCollector.start(draining: secondServer)
        let secondController = SessionController(
            transport: secondClient,
            outbox: outbox,
            sessionId: "session-old"
        )
        try await secondController.start()
        _ = await secondCollector.wait(forAtLeast: 1)

        var resumeOk = SRUIServerResumeOk()
        resumeOk.sessionID = "session-old"
        resumeOk.replayFromRevision = 1
        resumeOk.lastProcessedEventSeq = 0
        var response = SRUIMessage()
        response.serverResumeOk = resumeOk

        await firstController.handleIncomingMessage(response)
        try? await Task.sleep(nanoseconds: 100_000_000)
        #expect(try events(in: await firstCollector.collected()).isEmpty)
        #expect(firstController.isEventDispatchEnabled == false)
        await #expect(throws: SessionDispatchError.self) {
            try await firstController.sendActivate(nodeId: NodeId(9))
        }
        #expect(await outbox.pendingCount == 1)

        await secondController.handleIncomingMessage(response)
        let secondMessages = await secondCollector.wait(forAtLeast: 2)
        let replayed = try #require(try events(in: secondMessages).last)
        #expect(replayed.eventId == pending.eventId)
        #expect(replayed.eventSeq == pending.eventSeq)
        #expect(secondController.isEventDispatchEnabled)

        await firstController.stop()
        await secondController.stop()
        await firstCollector.stop()
        await secondCollector.stop()
        await seedClient.close()
        await seedServer.close()
        await firstServer.close()
        await secondServer.close()
    }

    @Test("stop() during resync clears the outbox latch so a shared outbox can fresh HELLO")
    func stopDuringResyncAllowsFreshHelloWithSharedOutbox() async throws {
        let outbox = EventOutbox()
        let (client, server) = await PipeTransport.createPair()
        let controller = SessionController(
            transport: client,
            outbox: outbox,
            sessionId: "session-live"
        )
        try await controller.start()

        await controller.handleIncomingMessage(
            resyncMessage(sessionId: "session-live", continuity: .sameSession)
        )
        #expect(controller.isEventDispatchEnabled == false)

        await controller.stop()

        let (freshClient, freshServer) = await PipeTransport.createPair()
        let freshController = SessionController(transport: freshClient, outbox: outbox)
        try await freshController.start()

        var welcome = HandshakeFixtures.welcomeMessage(sessionId: "fresh-session")
        welcome.serverWelcome.initialRevision = 0
        try await freshServer.send(data: SRUIFraming.encodeFramed(welcome))

        try await AsyncTestSupport.eventually(description: "fresh HELLO handshake") {
            freshController.sessionId == "fresh-session"
        }

        await freshController.stop()
        await freshServer.close()
        await server.close()
    }

    @Test("A superseded controller ignores a late resync snapshot")
    func supersededControllerIgnoresLateSnapshot() async throws {
        let outbox = EventOutbox()
        let applier = TransactionApplier()

        let (firstClient, firstServer) = await PipeTransport.createPair()
        let firstController = SessionController(
            transport: firstClient,
            applier: applier,
            outbox: outbox,
            sessionId: "session-old"
        )
        let failures = FailureRecorder()
        firstController.onFailure = { failure in
            Task { await failures.record(failure) }
        }
        try await firstController.start()

        await firstController.handleIncomingMessage(
            resyncMessage(sessionId: "session-old", continuity: .sameSession)
        )

        let (secondClient, secondServer) = await PipeTransport.createPair()
        let secondController = SessionController(
            transport: secondClient,
            outbox: outbox,
            sessionId: "session-old"
        )
        try await secondController.start()

        await firstController.handleIncomingMessage(snapshot(revision: 5, text: "stale"))

        #expect(applier.lastAppliedRevision == .initial)

        // A superseded controller can never finish its handshake, so it reports instead of
        // holding an open transport that silently drops every frame (§18).
        let failure = try #require(await failures.wait())
        guard case .superseded = failure else {
            Issue.record("Expected a superseded failure, got \(failure)")
            return
        }

        await firstController.stop()
        await secondController.stop()
        await firstServer.close()
        await secondServer.close()
    }

    @Test("An older controller's stop() cannot strand a newer resume attempt")
    func stoppingSupersededControllerLeavesNewerAttemptUsable() async throws {
        let outbox = EventOutbox()

        let (firstClient, firstServer) = await PipeTransport.createPair()
        let firstController = SessionController(
            transport: firstClient,
            outbox: outbox,
            sessionId: "session-old"
        )
        try await firstController.start()

        let (secondClient, secondServer) = await PipeTransport.createPair()
        let secondCollector = ResumeWireCollector()
        await secondCollector.start(draining: secondServer)
        let secondController = SessionController(
            transport: secondClient,
            outbox: outbox,
            sessionId: "session-old"
        )
        try await secondController.start()
        _ = await secondCollector.wait(forAtLeast: 1)

        // Generation 1 is already superseded: releasing it on stop() must leave generation 2's
        // latch intact, or the newer attempt could never be answered at all (§18).
        await firstController.stop()

        var resumeOk = SRUIServerResumeOk()
        resumeOk.sessionID = "session-old"
        resumeOk.replayFromRevision = 1
        resumeOk.lastProcessedEventSeq = 0
        var response = SRUIMessage()
        response.serverResumeOk = resumeOk
        await secondController.handleIncomingMessage(response)

        #expect(secondController.sessionId == "session-old")
        #expect(secondController.isEventDispatchEnabled)

        await secondController.stop()
        await secondCollector.stop()
        await firstServer.close()
        await secondServer.close()
    }
}

/// Collects reported session failures for assertions.
private actor FailureRecorder {
    private var failures: [SessionFailure] = []

    func record(_ failure: SessionFailure) {
        failures.append(failure)
    }

    func wait(timeout: Double = 2.0) async -> SessionFailure? {
        let deadline = Date().addingTimeInterval(timeout)
        while failures.isEmpty && Date() < deadline {
            try? await Task.sleep(nanoseconds: 5_000_000)
        }
        return failures.first
    }
}
