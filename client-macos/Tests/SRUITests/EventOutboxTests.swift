//
// EventOutboxTests.swift
// SRUITests
//
// Unit tests for EventOutbox semantic event queue and serialization (§7.7, §16, §18.2).
//

import Testing
import Foundation
import SemanticModel
import Protocol
@testable import Session
import TransportSSH

@Suite("EventOutbox Tests")
struct EventOutboxTests {

    @Test("EventOutbox allocates monotonically increasing sequence numbers")
    func monotonicSequenceNumbers() async throws {
        let (client, server) = await PipeTransport.createPair()
        let outbox = EventOutbox()

        // Sending is the whole allocation surface: no caller can mint a sequence without also
        // retaining and transmitting it, so allocation and retention cannot diverge (§18.2).
        let seq1 = try await outbox.sendActivate(nodeId: NodeId(1), observedRevision: Revision(1), via: client).eventSeq
        let seq2 = try await outbox.sendActivate(nodeId: NodeId(2), observedRevision: Revision(1), via: client).eventSeq
        let seq3 = try await outbox.sendActivate(nodeId: NodeId(3), observedRevision: Revision(1), via: client).eventSeq

        #expect(seq1 == 1)
        #expect(seq2 == 2)
        #expect(seq3 == 3)
        #expect(await outbox.eventSeq == 3)
        #expect(await outbox.pendingCount == 3)

        await client.close()
        await server.close()
    }

    @Test("EventOutbox generates unique retry-safe event IDs")
    func uniqueEventIds() async throws {
        let (client, server) = await PipeTransport.createPair()
        let outbox = EventOutbox()

        let id1 = try await outbox.sendActivate(nodeId: NodeId(1), observedRevision: Revision(1), via: client).eventId
        let id2 = try await outbox.sendActivate(nodeId: NodeId(1), observedRevision: Revision(1), via: client).eventId

        #expect(!id1.isEmpty)
        #expect(!id2.isEmpty)
        #expect(id1 != id2)

        await client.close()
        await server.close()
    }

    @Test("EventOutbox creates and serializes ACTIVATE event")
    func activateEventSerialization() async throws {
        let clientInstanceId = ClientInstanceId(string: "client-test-42")
        let outbox = EventOutbox(clientInstanceId: clientInstanceId)
        let (client, server) = await PipeTransport.createPair()

        let nodeId = NodeId(183)
        let observedRevision = Revision(104)

        let event = try await outbox.sendActivate(nodeId: nodeId, observedRevision: observedRevision, via: client)

        #expect(event.eventSeq == 1)
        #expect(event.nodeId == nodeId)
        #expect(event.observedRevision == observedRevision)
        #expect(event.eventType == .EVENT_ACTIVATE)
        #expect(event.clientInstanceId == clientInstanceId)

        // Verify wire encoding roundtrip
        var msg = SRUIMessage()
        msg.event = event.toWire()
        let framedBytes = try SRUIFraming.encodeFramed(msg)

        let decodedMsg = try decodeFramedMessage(from: framedBytes)
        guard case .event(let wireEvent) = decodedMsg.msg else {
            Issue.record("Expected event message payload")
            return
        }

        let decodedEvent = try ProtocolDecoder().validateAndConvertEvent(wire: wireEvent)
        #expect(decodedEvent.eventSeq == 1)
        #expect(decodedEvent.nodeId == nodeId)
        #expect(decodedEvent.observedRevision == observedRevision)
        #expect(decodedEvent.eventType == .EVENT_ACTIVATE)
        #expect(decodedEvent.clientInstanceId == clientInstanceId)

        await client.close()
        await server.close()
    }

    @Test("EventOutbox creates and serializes VALUE_CHANGED event")
    func valueChangedEventSerialization() async throws {
        let clientInstanceId = ClientInstanceId(string: "client-test-val")
        let outbox = EventOutbox(clientInstanceId: clientInstanceId)
        let (client, server) = await PipeTransport.createPair()

        let nodeId = NodeId(200)
        let observedRevision = Revision(50)
        let value = Value.bool(true)

        let event = try await outbox.sendValueChanged(nodeId: nodeId, observedRevision: observedRevision, value: value, via: client)

        #expect(event.eventSeq == 1)
        #expect(event.nodeId == nodeId)
        #expect(event.observedRevision == observedRevision)
        #expect(event.eventType == .EVENT_VALUE_CHANGED)
        #expect(event.boolArg == true)
        #expect(event.clientInstanceId == clientInstanceId)

        // Verify wire roundtrip
        var msg = SRUIMessage()
        msg.event = event.toWire()
        let framedBytes = try SRUIFraming.encodeFramed(msg)

        let decodedMsg = try decodeFramedMessage(from: framedBytes)
        guard case .event(let wireEvent) = decodedMsg.msg else {
            Issue.record("Expected event message payload")
            return
        }

        let decodedEvent = try ProtocolDecoder().validateAndConvertEvent(wire: wireEvent)
        #expect(decodedEvent.eventSeq == 1)
        #expect(decodedEvent.nodeId == nodeId)
        #expect(decodedEvent.eventType == .EVENT_VALUE_CHANGED)
        #expect(decodedEvent.boolArg == true)

        await client.close()
        await server.close()
    }

    @Test("EventOutbox creates and serializes SELECTION_CHANGED event")
    func selectionChangedEventSerialization() async throws {
        let clientInstanceId = ClientInstanceId(string: "client-test-sel")
        let outbox = EventOutbox(clientInstanceId: clientInstanceId)
        let (client, server) = await PipeTransport.createPair()

        let nodeId = NodeId(300)
        let observedRevision = Revision(75)
        let itemId = ItemId(999)

        let event = try await outbox.sendSelectionChanged(nodeId: nodeId, observedRevision: observedRevision, itemId: itemId, via: client)

        #expect(event.eventSeq == 1)
        #expect(event.nodeId == nodeId)
        #expect(event.observedRevision == observedRevision)
        #expect(event.eventType == .EVENT_SELECTION_CHANGED)
        #expect(event.itemIdArg == itemId)
        #expect(event.clientInstanceId == clientInstanceId)

        // Verify wire roundtrip
        var msg = SRUIMessage()
        msg.event = event.toWire()
        let framedBytes = try SRUIFraming.encodeFramed(msg)

        let decodedMsg = try decodeFramedMessage(from: framedBytes)
        guard case .event(let wireEvent) = decodedMsg.msg else {
            Issue.record("Expected event message payload")
            return
        }

        let decodedEvent = try ProtocolDecoder().validateAndConvertEvent(wire: wireEvent)
        #expect(decodedEvent.eventSeq == 1)
        #expect(decodedEvent.nodeId == nodeId)
        #expect(decodedEvent.eventType == .EVENT_SELECTION_CHANGED)
        #expect(decodedEvent.itemIdArg == itemId)

        await client.close()
        await server.close()
    }

    @Test("Mixed event types share contiguous monotonic sequences and are retained")
    func mixedEventTypesContiguousSequences() async throws {
        let (client, server) = await PipeTransport.createPair()
        let outbox = EventOutbox()

        let ev1 = try await outbox.sendActivate(nodeId: NodeId(1), observedRevision: Revision(1), via: client)
        let ev2 = try await outbox.sendValueChanged(nodeId: NodeId(2), observedRevision: Revision(1), value: .bool(true), via: client)
        let ev3 = try await outbox.sendSelectionChanged(nodeId: NodeId(3), observedRevision: Revision(1), itemId: ItemId(42), via: client)

        #expect(ev1.eventSeq == 1)
        #expect(ev2.eventSeq == 2)
        #expect(ev3.eventSeq == 3)

        #expect(await outbox.eventSeq == 3)
        #expect(await outbox.pendingCount == 3)

        await client.close()
        await server.close()
    }

    @Test("Permission and window capacity behavior across event send APIs")
    func permissionAndCapacityBehavior() async throws {
        let (client, server) = await PipeTransport.createPair()
        let outbox = EventOutbox(maxPendingEvents: 2)

        // 1. Fill capacity (2 events)
        _ = try await outbox.sendActivate(nodeId: NodeId(1), observedRevision: Revision(1), via: client)
        _ = try await outbox.sendValueChanged(nodeId: NodeId(2), observedRevision: Revision(1), value: .bool(true), via: client)

        // 3rd event should throw sequenceWindowExhausted
        await #expect(throws: EventOutboxError.sequenceWindowExhausted(limit: 2)) {
            try await outbox.sendSelectionChanged(nodeId: NodeId(3), observedRevision: Revision(1), itemId: ItemId(10), via: client)
        }

        // 2. Suspended outbox throws resumeNotConfirmed
        await outbox.suspendNewEvents()
        await #expect(throws: EventOutboxError.resumeNotConfirmed) {
            try await outbox.sendActivate(nodeId: NodeId(1), observedRevision: Revision(1), via: client)
        }
        await #expect(throws: EventOutboxError.resumeNotConfirmed) {
            try await outbox.sendValueChanged(nodeId: NodeId(2), observedRevision: Revision(1), value: .bool(true), via: client)
        }
        await #expect(throws: EventOutboxError.resumeNotConfirmed) {
            try await outbox.sendSelectionChanged(nodeId: NodeId(3), observedRevision: Revision(1), itemId: ItemId(10), via: client)
        }

        await client.close()
        await server.close()
    }

    @Test("EventOutbox sendActivate transmits framed event over Transport")
    func sendActivateOverTransport() async throws {
        let (client, server) = await PipeTransport.createPair()
        let outbox = EventOutbox()

        let serverStream = server.receiveStream()

        let sendTask = Task {
            try await outbox.sendActivate(
                nodeId: NodeId(7),
                observedRevision: Revision(10),
                via: client
            )
        }

        var receivedData: Data?
        for try await chunk in serverStream {
            receivedData = chunk
            break
        }

        let sentEvent = try await sendTask.value
        let nonNilData = try #require(receivedData)

        let decodedMsg = try decodeFramedMessage(from: nonNilData)
        guard case .event(let wireEvent) = decodedMsg.msg else {
            Issue.record("Expected event message payload")
            return
        }
        let decodedEvent = try ProtocolDecoder().validateAndConvertEvent(wire: wireEvent)

        #expect(decodedEvent.eventSeq == sentEvent.eventSeq)
        #expect(decodedEvent.eventId == sentEvent.eventId)
        #expect(decodedEvent.nodeId == NodeId(7))
        #expect(decodedEvent.observedRevision == Revision(10))

        await client.close()
        await server.close()
    }

    @Test("Same-session resume replays unacknowledged events in original order with original identity")
    func sameSessionResumeReplay() async throws {
        let (client, server) = await PipeTransport.createPair()
        let outbox = EventOutbox()

        let ev1 = try await outbox.sendActivate(nodeId: NodeId(1), observedRevision: Revision(1), via: client)
        let ev2 = try await outbox.sendValueChanged(nodeId: NodeId(2), observedRevision: Revision(1), value: .bool(true), via: client)
        let ev3 = try await outbox.sendSelectionChanged(nodeId: NodeId(3), observedRevision: Revision(1), itemId: ItemId(42), via: client)

        #expect(await outbox.pendingCount == 3)

        // Settle ack through seq 1
        #expect(await outbox.confirmFreshSession(id: "session-live"))
        _ = await outbox.settleAcknowledgement(
            clientInstanceId: outbox.clientInstanceId,
            eventId: ev1.eventId,
            throughSeq: 1,
            sessionId: "session-live"
        )
        #expect(await outbox.pendingCount == 2)

        await client.close()
        await server.close()

        // Reconnect on a fresh transport pair and complete same-session resume
        let (client2, server2) = await PipeTransport.createPair()
        let generation = await outbox.beginResumeAttempt()
        let serverStream2 = server2.receiveStream()

        let accepted = try await outbox.completeSameSessionResume(
            id: "session-123",
            lastProcessedEventSeq: 1,
            generation: generation,
            via: client2,
            enableNewEventsAfterReplay: true
        )
        #expect(accepted)

        // Read replayed messages from server2 (should be ev2 and ev3)
        var streamDecoder = SRUIMessageStreamDecoder()
        var replayedEvents: [Event] = []
        for try await chunk in serverStream2 {
            let messages = try streamDecoder.appendAndExtract(incoming: chunk)
            for msg in messages {
                if case .event(let wireEvent) = msg.msg {
                    let domainEvent = try ProtocolDecoder().validateAndConvertEvent(wire: wireEvent)
                    replayedEvents.append(domainEvent)
                }
            }
            if replayedEvents.count >= 2 {
                break
            }
        }

        #expect(replayedEvents.count == 2)
        #expect(replayedEvents[0].eventSeq == ev2.eventSeq)
        #expect(replayedEvents[0].eventId == ev2.eventId)
        #expect(replayedEvents[1].eventSeq == ev3.eventSeq)
        #expect(replayedEvents[1].eventId == ev3.eventId)

        // Next new event continues monotonically from seq 4
        let ev4 = try await outbox.sendActivate(nodeId: NodeId(4), observedRevision: Revision(2), via: client2)
        #expect(ev4.eventSeq == 4)

        await outbox.stopResumeWork(generation: generation)
        await client2.close()
        await server2.close()
    }

    @Test("Same-session RESUME_OK clears the generation latch for a later fresh HELLO")
    func resumeOkClearsGenerationLatch() async throws {
        let (client, server) = await PipeTransport.createPair()
        let outbox = EventOutbox()
        _ = try await outbox.sendActivate(
            nodeId: NodeId(1),
            observedRevision: Revision(1),
            via: client
        )

        let generation = await outbox.beginResumeAttempt()
        let accepted = try await outbox.completeSameSessionResume(
            id: "session-123",
            lastProcessedEventSeq: 0,
            generation: generation,
            via: client,
            enableNewEventsAfterReplay: true
        )
        #expect(accepted)
        #expect(await outbox.confirmFreshSession(id: "fresh-session"))

        await client.close()
        await server.close()
    }

    @Test("Finish resync clears the generation latch for a later fresh HELLO")
    func finishResyncClearsGenerationLatch() async {
        let outbox = EventOutbox()
        let generation = await outbox.beginResumeAttempt()
        #expect(await outbox.finishResync(generation: generation))
        #expect(await outbox.confirmFreshSession(id: "fresh-session"))
    }

    @Test("Stopping resume work clears the generation latch for a later fresh HELLO")
    func stopResumeWorkClearsGenerationLatch() async {
        let outbox = EventOutbox()
        let generation = await outbox.beginResumeAttempt()
        await outbox.stopResumeWork(generation: generation)
        #expect(await outbox.confirmFreshSession(id: "fresh-session"))
    }

    @Test("A superseded generation cannot release the latch held by a newer attempt")
    func supersededGenerationCannotReleaseNewerLatch() async {
        let outbox = EventOutbox()
        let superseded = await outbox.beginResumeAttempt()
        let newest = await outbox.beginResumeAttempt()

        await outbox.stopResumeWork(generation: superseded)

        // The newest attempt still owns the latch, so its own decision is still the only one
        // that can settle the outbox (§18).
        let freshAccepted = await outbox.confirmFreshSession(id: "fresh-session")
        #expect(freshAccepted == false)
        let newestAccepted = await outbox.finishResync(generation: newest)
        #expect(newestAccepted == true)
    }

    @Test("Catch-up cannot re-enable allocation while a reconnect generation is outstanding")
    func allowNewEventsRefusesDuringOutstandingResume() async {
        let outbox = EventOutbox()
        let idleAllowed = await outbox.allowNewEvents()
        #expect(idleAllowed == true)

        let generation = await outbox.beginResumeAttempt()
        let latchedAllowed = await outbox.allowNewEvents()
        #expect(latchedAllowed == false)

        let finished = await outbox.finishResync(generation: generation)
        #expect(finished == true)
        let releasedAllowed = await outbox.allowNewEvents()
        #expect(releasedAllowed == true)
    }

    @Test("Replaced session abandons pending events and resets sequence")
    func replacedSessionResetsSequence() async throws {
        let (client, server) = await PipeTransport.createPair()
        let outbox = EventOutbox()

        _ = try await outbox.sendActivate(nodeId: NodeId(1), observedRevision: Revision(1), via: client)
        _ = try await outbox.sendValueChanged(nodeId: NodeId(2), observedRevision: Revision(1), value: .bool(false), via: client)
        #expect(await outbox.pendingCount == 2)

        let generation = await outbox.beginResumeAttempt()
        let accepted = await outbox.prepareReplacedSession(
            id: "new-incarnation",
            lastProcessedEventSeq: 0,
            generation: generation
        )
        #expect(accepted)
        #expect(await outbox.pendingCount == 0)

        // The replacement's snapshot commits under the same reconnect generation, so allocation
        // reopens through `finishResync`; `allowNewEvents` covers only a catch-up with no
        // outstanding attempt and refuses while this generation still holds the latch (§18).
        let reopened = await outbox.finishResync(generation: generation)
        #expect(reopened == true)

        let freshEvent = try await outbox.sendActivate(nodeId: NodeId(10), observedRevision: Revision(1), via: client)
        #expect(freshEvent.eventSeq == 1)
        #expect(await outbox.pendingCount == 1)

        await client.close()
        await server.close()
    }

    @Test("Window exhaustion is refused before a sequence or id is minted (§18.2)")
    func exhaustionLeavesSequenceAndRetentionUntouched() async throws {
        let (client, server) = await PipeTransport.createPair()
        let outbox = EventOutbox(maxPendingEvents: 2)
        #expect(await outbox.confirmFreshSession(id: "session-window"))

        let first = try await outbox.sendActivate(nodeId: NodeId(1), observedRevision: Revision(1), via: client)
        let second = try await outbox.sendActivate(nodeId: NodeId(2), observedRevision: Revision(1), via: client)

        await #expect(throws: EventOutboxError.sequenceWindowExhausted(limit: 2)) {
            try await outbox.sendActivate(nodeId: NodeId(3), observedRevision: Revision(1), via: client)
        }

        // A sequence burned by a refused send would be a permanent hole the server never settles
        // past, so backpressure must precede allocation (§18.2).
        #expect(await outbox.eventSeq == 2)
        #expect(await outbox.pendingCount == 2)
        #expect(await outbox.lastAckedEventSeq == 0)

        // The two retained identities are still exactly the originals: acking them by id drains
        // the outbox and advances the contiguous frontier to the newest allocated sequence.
        #expect(await outbox.settleAcknowledgement(
            clientInstanceId: outbox.clientInstanceId,
            eventId: first.eventId,
            throughSeq: 0,
            sessionId: "session-window"
        ).bound)
        #expect(await outbox.settleAcknowledgement(
            clientInstanceId: outbox.clientInstanceId,
            eventId: second.eventId,
            throughSeq: 0,
            sessionId: "session-window"
        ).bound)
        #expect(await outbox.pendingCount == 0)
        #expect(await outbox.lastAckedEventSeq == 2)

        await client.close()
        await server.close()
    }

    @Test("A selective ack across a gap neither reopens the window nor skips a sequence (§18.2)")
    func selectiveAckAcrossGapKeepsWindowClosed() async throws {
        let (client, server) = await PipeTransport.createPair()
        let outbox = EventOutbox(maxPendingEvents: 2)
        #expect(await outbox.confirmFreshSession(id: "session-gap"))

        let first = try await outbox.sendActivate(nodeId: NodeId(1), observedRevision: Revision(1), via: client)
        let second = try await outbox.sendActivate(nodeId: NodeId(2), observedRevision: Revision(1), via: client)

        #expect(await outbox.settleAcknowledgement(
            clientInstanceId: outbox.clientInstanceId,
            eventId: second.eventId,
            throughSeq: 0,
            sessionId: "session-gap"
        ).bound)
        #expect(await outbox.pendingCount == 1)
        // Sequence 1 is still unsettled, so the cumulative frontier cannot cross it.
        #expect(await outbox.lastAckedEventSeq == 0)

        // The window is measured from that frontier, not from the selective set, so the slot the
        // later ack settled is not reusable while the gap remains.
        await #expect(throws: EventOutboxError.sequenceWindowExhausted(limit: 2)) {
            try await outbox.sendActivate(nodeId: NodeId(3), observedRevision: Revision(1), via: client)
        }
        #expect(await outbox.eventSeq == 2)

        #expect(await outbox.settleAcknowledgement(
            clientInstanceId: outbox.clientInstanceId,
            eventId: first.eventId,
            throughSeq: 0,
            sessionId: "session-gap"
        ).bound)
        #expect(await outbox.lastAckedEventSeq == 2)

        let third = try await outbox.sendActivate(nodeId: NodeId(3), observedRevision: Revision(1), via: client)
        #expect(third.eventSeq == 3)
        #expect(await outbox.pendingCount == 1)

        await client.close()
        await server.close()
    }

    @Test("TEXT_EDIT serializes edit_seq and the whole-value TEXT argument")
    func textEditEventSerialization() async throws {
        let clientInstanceId = ClientInstanceId(string: "client-test-text")
        let outbox = EventOutbox(clientInstanceId: clientInstanceId)
        let (client, server) = await PipeTransport.createPair()
        #expect(await outbox.confirmFreshSession(id: "session-text"))

        let nodeId = NodeId(12)
        let seq = try #require(EditSeq(7))
        let event = try #require(try await outbox.queueTextEdit(
            nodeId: nodeId,
            text: "whole-value",
            editSeq: seq,
            observedRevision: Revision(4),
            via: client
        ))
        #expect(event.eventType == .EVENT_TEXT_EDIT)
        #expect(event.editSeq == seq)
        #expect(event.textArg == "whole-value")
        #expect(event.eventSeq == 1)

        var msg = SRUIMessage()
        msg.event = event.toWire()
        let decodedMsg = try decodeFramedMessage(from: try SRUIFraming.encodeFramed(msg))
        guard case .event(let wireEvent) = decodedMsg.msg else {
            Issue.record("Expected event message payload")
            return
        }
        let decodedEvent = try ProtocolDecoder().validateAndConvertEvent(wire: wireEvent)
        #expect(decodedEvent.editSeq == seq)
        #expect(decodedEvent.textArg == "whole-value")
        #expect(decodedEvent.eventType == .EVENT_TEXT_EDIT)

        await client.close()
        await server.close()
    }

    @Test("Coalesced drafts keep event_seq contiguous while edit_seq may skip")
    func coalescedTextEditsSkipEditSeqButKeepEventSeqContiguous() async throws {
        let (client, server) = await PipeTransport.createPair()
        let outbox = EventOutbox()
        #expect(await outbox.confirmFreshSession(id: "session-coalesce"))
        let nodeId = NodeId(12)
        let seq1 = try #require(EditSeq(1))
        let first = try #require(try await outbox.queueTextEdit(
            nodeId: nodeId,
            text: "a",
            editSeq: seq1,
            observedRevision: Revision(1),
            via: client
        ))
        #expect(first.eventSeq == 1)

        let second = try await outbox.queueTextEdit(
            nodeId: nodeId,
            text: "ab",
            editSeq: try #require(EditSeq(2)),
            observedRevision: Revision(1),
            via: client
        )
        #expect(second == nil)
        let third = try await outbox.queueTextEdit(
            nodeId: nodeId,
            text: "abc",
            editSeq: try #require(EditSeq(3)),
            observedRevision: Revision(1),
            via: client
        )
        #expect(third == nil)
        #expect(await outbox.unsentTextDraftCount == 1)
        #expect(await outbox.eventSeq == 1)

        #expect(await outbox.settleAcknowledgement(
            clientInstanceId: outbox.clientInstanceId,
            eventId: first.eventId,
            throughSeq: 1,
            sessionId: "session-coalesce"
        ).bound)
        let promoted = try await outbox.promoteReadyTextDrafts(via: client)
        #expect(promoted.count == 1)
        #expect(promoted[0].textArg == "abc")
        #expect(promoted[0].eventSeq == 2)
        #expect(promoted[0].editSeq?.rawValue == 3)

        #expect(await outbox.eventSeq == 2)
        let assigned = await outbox.assignedTextEditDescriptors()
        #expect(assigned.count == 1)
        #expect(assigned[0].eventSeq == 2)
        #expect(assigned[0].editSeq.rawValue == 3)
        #expect(await outbox.unsentTextDraftCount == 0)

        await client.close()
        await server.close()
    }

    @Test("queueTextEdit retains a draft while dispatch is suspended")
    func queueTextEditWhileSuspendedKeepsNewestDraft() async throws {
        let (client, server) = await PipeTransport.createPair()
        let outbox = EventOutbox()
        #expect(await outbox.confirmFreshSession(id: "session-suspend"))
        let generation = await outbox.beginResumeAttempt()

        let promoted = try await outbox.queueTextEdit(
            nodeId: NodeId(12),
            text: "first",
            editSeq: try #require(EditSeq(1)),
            observedRevision: Revision(1),
            via: client
        )
        #expect(promoted == nil)
        _ = try await outbox.queueTextEdit(
            nodeId: NodeId(12),
            text: "newest",
            editSeq: try #require(EditSeq(3)),
            observedRevision: Revision(1),
            via: client
        )
        #expect(await outbox.unsentTextDraftCount == 1)
        #expect(await outbox.assignedTextEditDescriptors().isEmpty)
        #expect(await outbox.eventSeq == 0)

        let accepted = try await outbox.completeSameSessionResume(
            id: "session-suspend",
            lastProcessedEventSeq: 0,
            generation: generation,
            via: client,
            enableNewEventsAfterReplay: true
        )
        #expect(accepted)
        #expect(await outbox.eventSeq == 1)
        let assigned = await outbox.assignedTextEditDescriptors()
        #expect(assigned.count == 1)
        #expect(assigned[0].editSeq.rawValue == 3)

        await client.close()
        await server.close()
    }

    @Test("Selective TEXT_EDIT cancellation preserves ordinary pending events")
    func cancelAssignedTextEditsPreservesOrdinaryEvents() async throws {
        let (client, server) = await PipeTransport.createPair()
        let outbox = EventOutbox()
        #expect(await outbox.confirmFreshSession(id: "session-cancel"))
        let seq1 = try #require(EditSeq(1))
        let textEvent = try #require(try await outbox.queueTextEdit(
            nodeId: NodeId(12),
            text: "typed",
            editSeq: seq1,
            observedRevision: Revision(1),
            via: client
        ))
        let activate = try await outbox.sendActivate(
            nodeId: NodeId(7),
            observedRevision: Revision(1),
            via: client
        )
        #expect(await outbox.pendingCount == 2)

        try await outbox.cancelAssignedTextEdits(
            confirming: await outbox.assignedTextEditDescriptors(),
            requireExactMatch: true
        )
        #expect(await outbox.pendingCount == 1)
        #expect(await outbox.assignedTextEditDescriptors().isEmpty)
        #expect(activate.eventSeq == 2)
        #expect(textEvent.eventSeq == 1)

        await #expect(throws: EventOutboxError.textEditDiscardMismatch) {
        try await outbox.cancelAssignedTextEdits(
            confirming: [
                PendingTextEditDescriptor(
                    eventId: textEvent.eventId,
                    eventSeq: textEvent.eventSeq,
                    nodeId: textEvent.nodeId,
                    editSeq: textEvent.editSeq ?? EditSeq(1)!
                )
            ],
            requireExactMatch: true
        )
        }

        await client.close()
        await server.close()
    }
}
