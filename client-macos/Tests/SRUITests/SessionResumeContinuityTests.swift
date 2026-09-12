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

typealias ResumeWireCollector = ResyncWireCollector

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
        lastProcessedEventSeq: UInt64 = 0,
        discardedTextEdits: [SRUIPendingTextEditRef] = []
    ) -> SRUIMessage {
        var resync = SRUIServerResyncRequired()
        resync.sessionID = sessionId
        resync.snapshotRevision = snapshotRevision
        resync.reason = "test"
        resync.continuity = continuity
        resync.lastProcessedEventSeq = lastProcessedEventSeq
        resync.discardedTextEdits = discardedTextEdits
        resync.requiredProfiles = ["org.srui.standard-widgets/1"]
        var message = SRUIMessage()
        message.serverResyncRequired = resync
        return message
    }

    private func activeBinding(
        for outbox: EventOutbox,
        sessionId: String? = nil
    ) async -> EventOutboxConnectionBinding {
        let binding = await outbox.beginConnectionBinding()
        if let sessionId {
            #expect(await outbox.confirmFreshSession(id: sessionId, binding: binding))
        } else {
            #expect(await outbox.allowNewEvents(binding: binding))
        }
        #expect(await outbox.isActiveConnectionBinding(binding))
        #expect(await outbox.sessionIncarnation(binding: binding) != nil)
        return binding
    }

    private func sendPreparedTextEdit(
        _ outbox: EventOutbox,
        nodeId: NodeId,
        text: String,
        editSeq: EditSeq,
        observedRevision: Revision,
        binding: EventOutboxConnectionBinding,
        via transport: any Transport
    ) async throws -> Event {
        let prepared = try #require(try await outbox.prepareTextEdit(
            nodeId: nodeId,
            text: text,
            editSeq: editSeq,
            observedRevision: observedRevision,
            binding: binding,
            via: transport
        ))
        #expect(await outbox.authorizePreparedTextEdit(prepared))
        return try #require(try await outbox.releasePreparedTextEdit(prepared))
    }

    @Test("SAME_SESSION resync replays pending events, then the snapshot re-enables dispatch")
    func sameSessionResyncReplaysPendingThenAppliesSnapshot() async throws {
        let (seedClient, seedServer) = await PipeTransport.createPair()
        let outbox = EventOutbox()
        let seedBinding = await activeBinding(for: outbox)
        let pending = try await outbox.sendActivate(
            nodeId: NodeId(7),
            observedRevision: Revision(3),
            binding: seedBinding,
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
        let seedBinding = await activeBinding(for: outbox)
        _ = try await outbox.sendActivate(
            nodeId: NodeId(7),
            observedRevision: Revision(3),
            binding: seedBinding,
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
        let messages = await collector.wait(forAtLeast: 1)
        #expect(try events(in: messages).isEmpty)
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
        let seedBinding = await activeBinding(for: outbox)
        _ = try await outbox.sendActivate(
            nodeId: NodeId(7),
            observedRevision: Revision(3),
            binding: seedBinding,
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
        let messages = await collector.wait(forAtLeast: 1)
        #expect(try events(in: messages).isEmpty)
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
        let seedBinding = await activeBinding(for: outbox)
        let pending = try await outbox.sendActivate(
            nodeId: NodeId(7),
            observedRevision: Revision(3),
            binding: seedBinding,
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
        #expect(try events(in: await secondCollector.wait(forAtLeast: 1)).isEmpty)

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
        let seedBinding = await activeBinding(for: outbox)
        let pending = try await outbox.sendActivate(
            nodeId: NodeId(7),
            observedRevision: Revision(3),
            binding: seedBinding,
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
        #expect(try events(in: await firstCollector.wait(forAtLeast: 1)).isEmpty)
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

    @Test("A replay cancelled by a newer attempt reports supersession, not a transport failure")
    func supersededReplayReportsSupersessionNotTransportFailure() async throws {
        let (seedClient, seedServer) = await PipeTransport.createPair()
        let outbox = EventOutbox()
        let seedBinding = await activeBinding(for: outbox)
        // Two pending events: the cancellation the newer attempt raises is observed between
        // frames, so a single-frame replay would finish before it is ever checked.
        _ = try await outbox.sendActivate(
            nodeId: NodeId(7),
            observedRevision: Revision(3),
            binding: seedBinding,
            via: seedClient
        )
        _ = try await outbox.sendActivate(
            nodeId: NodeId(8),
            observedRevision: Revision(3),
            binding: seedBinding,
            via: seedClient
        )

        let gated = GatedSendTransport()
        let failures = FailureRecorder()
        let firstController = SessionController(
            transport: gated,
            outbox: outbox,
            sessionId: "session-old"
        )
        firstController.onFailure = { failure in
            Task { await failures.record(failure) }
        }

        // `start()` blocks on the gated CLIENT RESUME write until it is released.
        let startTask = Task { try await firstController.start() }
        await gated.waitForSendCount(1)
        await gated.releaseNextSend()
        try await startTask.value

        var resumeOk = SRUIServerResumeOk()
        resumeOk.sessionID = "session-old"
        resumeOk.replayFromRevision = 1
        resumeOk.lastProcessedEventSeq = 0
        var response = SRUIMessage()
        response.serverResumeOk = resumeOk

        // Hold the first replay frame in flight.
        let deliverTask = Task { await firstController.handleIncomingMessage(response) }
        await gated.waitForSendCount(2)

        // A newer attempt cancels the superseded write chain mid-replay (§18).
        let (secondClient, secondServer) = await PipeTransport.createPair()
        let secondController = SessionController(
            transport: secondClient,
            outbox: outbox,
            sessionId: "session-old"
        )
        try await secondController.start()

        await gated.releaseNextSend()
        await deliverTask.value

        // The cancellation surfaces as `CancellationError`, but this controller did not lose its
        // transport — it lost the race, and the owner must not reconnect it (§18).
        let failure = try #require(await failures.wait())
        guard case .superseded = failure else {
            Issue.record("Expected a superseded failure, got \(failure)")
            return
        }
        #expect(firstController.isEventDispatchEnabled == false)

        await firstController.stop()
        await secondController.stop()
        await gated.close()
        await seedClient.close()
        await seedServer.close()
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

    @Test("stop() applies a snapshot buffered on the transport instead of reporting supersession")
    func stopAppliesSnapshotBufferedDuringDrain() async throws {
        let transport = DrainOnCloseTransport()
        let outbox = EventOutbox()
        let applier = TransactionApplier()
        let controller = SessionController(
            transport: transport,
            applier: applier,
            outbox: outbox,
            sessionId: "session-buffered"
        )
        let failures = FailureRecorder()
        controller.onFailure = { failure in
            Task { await failures.record(failure) }
        }
        try await controller.start()

        // Both frames were already in the socket when the owner called `stop()`. The drain exists
        // to consume exactly these, so the resume latch must outlive it: releasing the latch first
        // refuses the resume decision, drops the snapshot, and reports supersession on a
        // deliberate stop that nothing superseded (§18).
        try await transport.buffer(
            resyncMessage(sessionId: "session-buffered", continuity: .sameSession)
        )
        try await transport.buffer(snapshot(revision: 4, text: "buffered"))

        await controller.stop()

        #expect(applier.lastAppliedRevision == Revision(4))
        #expect(await failures.wait(timeout: 0.2) == nil)
    }

    @Test("An EVENT_ACK without session_id fails the session instead of stranding the event")
    func ackWithoutSessionIdFailsTheSession() async throws {
        let (client, server) = await PipeTransport.createPair()
        let outbox = EventOutbox()
        let controller = SessionController(transport: client, outbox: outbox)
        let failures = FailureRecorder()
        controller.onFailure = { failure in
            Task { await failures.record(failure) }
        }
        try await controller.start()
        await controller.handleIncomingMessage(
            HandshakeFixtures.welcomeMessage(sessionId: "session-ack")
        )
        #expect(controller.isEventDispatchEnabled)

        let event = try await controller.sendActivate(nodeId: NodeId(4))

        var ack = SRUIServerEventAck()
        ack.clientInstanceID = outbox.clientInstanceId.bytes
        ack.eventID = event.eventId.bytes
        ack.lastProcessedEventSeq = event.eventSeq
        ack.status = .processed
        ack.revisionAfterEffect = 1
        var ackMessage = SRUIMessage()
        ackMessage.serverEventAck = ack
        await controller.handleIncomingMessage(ackMessage)

        // Settling on an ack that names no incarnation is what the identity check forbids, so the
        // intent stays pending — and the session fails instead of replaying it until the sequence
        // window is exhausted (§18.2, §4 inv. 13).
        #expect(await outbox.pendingCount == 1)
        let failure = try #require(await failures.wait())
        guard case .protocolViolation = failure else {
            Issue.record("Expected a protocol violation, got \(failure)")
            return
        }

        await controller.stop()
        await server.close()
    }

    @Test("RESUME_OK replays an assigned TEXT_EDIT byte-for-byte")
    func resumeOkReplaysAssignedTextEditUnchanged() async throws {
        let (seedClient, seedServer) = await PipeTransport.createPair()
        let outbox = EventOutbox()
        let seedBinding = await activeBinding(for: outbox, sessionId: "session-live")
        let seq = try #require(EditSeq(4))
        let pending = try await sendPreparedTextEdit(
            outbox,
            nodeId: NodeId(12),
            text: "typed",
            editSeq: seq,
            observedRevision: Revision(3),
            binding: seedBinding,
            via: seedClient
        )
        #expect(await outbox.assignedTextEditEvents() == [pending])

        let (client, server) = await PipeTransport.createPair()
        let collector = ResumeWireCollector()
        await collector.start(draining: server)
        let controller = SessionController(
            transport: client,
            outbox: outbox,
            sessionId: "session-live"
        )
        try await controller.start()

        let resumeMessages = await collector.wait(forAtLeast: 1)
        let resume = try #require(resumeMessages.compactMap { message -> SRUIClientResume? in
            if case .clientResume(let resume) = message.msg { return resume }
            return nil
        }.first)
        #expect(resume.pendingTextEdits.count == 1)
        #expect(resume.pendingTextEdits[0].eventID == pending.eventId.bytes)
        #expect(resume.pendingTextEdits[0].eventSeq == pending.eventSeq)
        #expect(resume.pendingTextEdits[0].editSeq == seq.rawValue)

        var resumeOk = SRUIServerResumeOk()
        resumeOk.sessionID = "session-live"
        resumeOk.lastProcessedEventSeq = 0
        var response = SRUIMessage()
        response.serverResumeOk = resumeOk
        await controller.handleIncomingMessage(response)

        let after = await collector.wait(forAtLeast: 2)
        let replayed = try #require(try events(in: after).last)
        #expect(replayed.eventId == pending.eventId)
        #expect(replayed.eventSeq == pending.eventSeq)
        #expect(replayed.editSeq == seq)
        #expect(replayed.textArg == "typed")
        #expect(replayed.eventType == .EVENT_TEXT_EDIT)

        await controller.stop()
        await collector.stop()
        await seedClient.close()
        await seedServer.close()
        await server.close()
    }

    @Test("Same-session resync cancels TEXT_EDIT and replays an interleaved ordinary event")
    func sameSessionResyncCancelsTextAndReplaysOrdinaryEvent() async throws {
        let (seedClient, seedServer) = await PipeTransport.createPair()
        let outbox = EventOutbox()
        let seedBinding = await activeBinding(for: outbox, sessionId: "session-live")
        let seq = try #require(EditSeq(2))
        let textEvent = try await sendPreparedTextEdit(
            outbox,
            nodeId: NodeId(12),
            text: "draft",
            editSeq: seq,
            observedRevision: Revision(3),
            binding: seedBinding,
            via: seedClient
        )
        #expect(await outbox.assignedTextEditEvents() == [textEvent])
        let activate = try await outbox.sendActivate(
            nodeId: NodeId(7),
            observedRevision: Revision(3),
            binding: seedBinding,
            via: seedClient
        )
        #expect(textEvent.eventSeq == 1)
        #expect(activate.eventSeq == 2)

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
        _ = await collector.wait(forAtLeast: 1)

        let discarded = await outbox.assignedTextEditDescriptors().map { $0.toWire() }
        #expect(discarded.count == 1)
        await controller.handleIncomingMessage(
            resyncMessage(
                sessionId: "session-live",
                continuity: .sameSession,
                lastProcessedEventSeq: 1,
                discardedTextEdits: discarded
            )
        )

        let afterDecision = await collector.wait(forAtLeast: 2)
        let replayed = try events(in: afterDecision)
        #expect(replayed.map(\.eventId) == [activate.eventId])
        #expect(await outbox.pendingCount == 1)

        await controller.handleIncomingMessage(snapshot(revision: 5, text: "snapshot-wins"))
        #expect(applier.lastAppliedRevision == Revision(5))
        #expect(controller.isEventDispatchEnabled)

        await controller.stop()
        await collector.stop()
        await seedClient.close()
        await seedServer.close()
        await server.close()
    }

    @Test("Replacement resync discards assigned TEXT_EDIT state from the old incarnation")
    func replacementResyncDiscardsTextEditState() async throws {
        let (seedClient, seedServer) = await PipeTransport.createPair()
        let outbox = EventOutbox()
        let seedBinding = await activeBinding(for: outbox, sessionId: "session-old")
        let assigned = try await sendPreparedTextEdit(
            outbox,
            nodeId: NodeId(12),
            text: "old",
            editSeq: try #require(EditSeq(9)),
            observedRevision: Revision(3),
            binding: seedBinding,
            via: seedClient
        )
        #expect(await outbox.assignedTextEditEvents() == [assigned])
        #expect(await outbox.pendingCount == 1)

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
            resyncMessage(sessionId: "session-new", continuity: .replaced)
        )

        let messages = await collector.wait(forAtLeast: 1)
        #expect(try events(in: messages).isEmpty)
        #expect(await outbox.pendingCount == 0)
        #expect(await outbox.eventSeq == 0)
        #expect(await outbox.assignedTextEditDescriptors().isEmpty)

        await controller.stop()
        await collector.stop()
        await seedClient.close()
        await seedServer.close()
        await server.close()
    }

    @Test("A superseded same-session resync does not cancel a newer attempt's TEXT_EDITs")
    func supersededSameSessionResyncDoesNotCancelTextEdits() async throws {
        let (seedClient, seedServer) = await PipeTransport.createPair()
        let outbox = EventOutbox()
        let seedBinding = await activeBinding(for: outbox, sessionId: "session-live")
        let editSeq = try #require(EditSeq(1))
        let textEvent = try await sendPreparedTextEdit(
            outbox,
            nodeId: NodeId(12),
            text: "typed",
            editSeq: editSeq,
            observedRevision: Revision(3),
            binding: seedBinding,
            via: seedClient
        )
        #expect(await outbox.assignedTextEditEvents() == [textEvent])
        let (firstClient, firstServer) = await PipeTransport.createPair()
        let firstCollector = ResumeWireCollector()
        await firstCollector.start(draining: firstServer)
        let firstController = SessionController(
            transport: firstClient,
            outbox: outbox,
            sessionId: "session-live"
        )
        try await firstController.start()
        _ = await firstCollector.wait(forAtLeast: 1)

        let (secondClient, secondServer) = await PipeTransport.createPair()
        let secondCollector = ResumeWireCollector()
        await secondCollector.start(draining: secondServer)
        let secondController = SessionController(
            transport: secondClient,
            outbox: outbox,
            sessionId: "session-live"
        )
        try await secondController.start()
        _ = await secondCollector.wait(forAtLeast: 1)

        let discarded = [
            PendingTextEditDescriptor(
                eventId: textEvent.eventId,
                eventSeq: textEvent.eventSeq,
                nodeId: textEvent.nodeId,
                editSeq: try #require(textEvent.editSeq)
            ).toWire()
        ]
        await firstController.handleIncomingMessage(
            resyncMessage(
                sessionId: "session-live",
                continuity: .sameSession,
                lastProcessedEventSeq: 0,
                discardedTextEdits: discarded
            )
        )

        #expect(await outbox.assignedTextEditDescriptors().count == 1)
        #expect(await outbox.pendingCount == 1)
        #expect(try events(in: await firstCollector.wait(forAtLeast: 1)).isEmpty)

        await firstController.stop()
        await secondController.stop()
        await firstCollector.stop()
        await secondCollector.stop()
        await seedClient.close()
        await seedServer.close()
        await firstServer.close()
        await secondServer.close()
    }

    @Test("A mismatched discarded_text_edits confirmation fails closed")
    func mismatchedTextDiscardConfirmationFailsClosed() async throws {
        let (seedClient, seedServer) = await PipeTransport.createPair()
        let outbox = EventOutbox()
        let seedBinding = await activeBinding(for: outbox, sessionId: "session-live")
        let assigned = try await sendPreparedTextEdit(
            outbox,
            nodeId: NodeId(12),
            text: "typed",
            editSeq: try #require(EditSeq(1)),
            observedRevision: Revision(3),
            binding: seedBinding,
            via: seedClient
        )
        #expect(await outbox.assignedTextEditEvents() == [assigned])

        let (client, server) = await PipeTransport.createPair()
        let collector = ResumeWireCollector()
        await collector.start(draining: server)
        let controller = SessionController(
            transport: client,
            outbox: outbox,
            sessionId: "session-live"
        )
        let failures = FailureRecorder()
        controller.onFailure = { failure in
            Task { await failures.record(failure) }
        }
        try await controller.start()
        _ = await collector.wait(forAtLeast: 1)

        var bogus = SRUIPendingTextEditRef()
        bogus.eventID = Data("not-the-assigned-id".utf8)
        bogus.eventSeq = 99
        bogus.nodeID = 12
        bogus.editSeq = 1
        await controller.handleIncomingMessage(
            resyncMessage(
                sessionId: "session-live",
                continuity: .sameSession,
                discardedTextEdits: [bogus]
            )
        )

        let failure = try #require(await failures.wait())
        guard case .protocolViolation = failure else {
            Issue.record("Expected a protocol violation, got \(failure)")
            return
        }
        #expect(await outbox.pendingCount == 1)
        let messages = await collector.wait(forAtLeast: 1)
        #expect(try events(in: messages).isEmpty)

        await controller.stop()
        await collector.stop()
        await seedClient.close()
        await seedServer.close()
        await server.close()
    }

    /// A failed transport or protocol exchange before the server answers CLIENT_RESUME is not an
    /// authoritative continuity decision. Keep the checkpoint so the next transport can retry and
    /// let RESUME_OK or RESYNC_REQUIRED decide whether the session survived (§18).
    @Test("A failure with CLIENT_RESUME unanswered preserves the resume identity")
    func unansweredResumeFailurePreservesResumeIdentity() async throws {
        let (clientTransport, serverTransport) = await PipeTransport.createPair()
        let controller = SessionController(
            transport: clientTransport,
            sessionId: "replaced-incarnation",
            clientCapabilities: [Profile.standardWidgetsV1]
        )
        try await controller.start()
        #expect(controller.sessionId == "replaced-incarnation")

        // Any failure before RESUME_OK or RESYNC_REQUIRED answers the attempt is equivalent here.
        var hello = SRUIClientHello()
        hello.coreVersion = SRUICoreVersion
        var helloMessage = SRUIMessage()
        helloMessage.clientHello = hello
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(helloMessage))

        try await AsyncTestSupport.eventually(description: "unanswered resume failure") {
            controller.isDiverged
        }
        #expect(controller.sessionId == "replaced-incarnation")

        await controller.stop()
        await serverTransport.close()
    }
}

/// Transport whose `send` records the frame and then suspends until the test releases it, so a
/// replay can be held mid-flight while another controller supersedes it.
private actor GatedSendTransport: Transport {
    private let stream: AsyncThrowingStream<Data, Error>
    private let streamContinuation: AsyncThrowingStream<Data, Error>.Continuation
    private let releaseStream: AsyncStream<Void>
    private let releaseContinuation: AsyncStream<Void>.Continuation
    private var sentFrameCount = 0

    init() {
        let (stream, continuation) = AsyncThrowingStream<Data, Error>.makeStream()
        self.stream = stream
        self.streamContinuation = continuation
        let (releaseStream, releaseContinuation) = AsyncStream<Void>.makeStream()
        self.releaseStream = releaseStream
        self.releaseContinuation = releaseContinuation
    }

    func waitForSendCount(_ count: Int, timeout: Double = 2.0) async {
        let deadline = Date().addingTimeInterval(timeout)
        while sentFrameCount < count && Date() < deadline {
            await Task.yield()
        }
    }

    func releaseNextSend() {
        releaseContinuation.yield()
    }

    func send(data: Data, logicalClass _: LogicalChannelClass) async throws {
        sentFrameCount += 1
        var releases = releaseStream.makeAsyncIterator()
        _ = await releases.next()
    }

    nonisolated func receiveStream() -> AsyncThrowingStream<Data, Error> {
        stream
    }

    func close() async {
        releaseContinuation.finish()
        streamContinuation.finish()
    }
}

/// Transport that holds queued frames until `close()`, modelling bytes already buffered in the
/// socket when `stop()` closes it: the receive loop drains them inside the stop grace period.
private actor DrainOnCloseTransport: Transport {
    private let stream: AsyncThrowingStream<Data, Error>
    private let streamContinuation: AsyncThrowingStream<Data, Error>.Continuation
    private var buffered: [Data] = []

    init() {
        let (stream, continuation) = AsyncThrowingStream<Data, Error>.makeStream()
        self.stream = stream
        self.streamContinuation = continuation
    }

    func buffer(_ message: SRUIMessage) throws {
        buffered.append(try SRUIFraming.encodeFramed(message))
    }

    func send(data: Data, logicalClass _: LogicalChannelClass) async throws {}

    nonisolated func receiveStream() -> AsyncThrowingStream<Data, Error> {
        stream
    }

    func close() async {
        for frame in buffered {
            streamContinuation.yield(frame)
        }
        buffered.removeAll()
        streamContinuation.finish()
    }
}

/// Collects reported session failures for assertions.
typealias FailureRecorder = SessionFailureRecorder
