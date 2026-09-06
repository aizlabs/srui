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
        let binding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "session-live", binding: binding))
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
        let resumedBinding = await outbox.beginConnectionBinding()
        let generation = try #require(
            await outbox.beginResumeAttempt(binding: resumedBinding)
        )
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

        let binding = await outbox.beginConnectionBinding()
        let generation = try #require(await outbox.beginResumeAttempt(binding: binding))
        let accepted = try await outbox.completeSameSessionResume(
            id: "session-123",
            lastProcessedEventSeq: 0,
            generation: generation,
            via: client,
            enableNewEventsAfterReplay: true
        )
        #expect(accepted)
        #expect(await outbox.confirmFreshSession(id: "fresh-session", binding: binding))

        await client.close()
        await server.close()
    }

    @Test("Finish resync clears the generation latch for a later fresh HELLO")
    func finishResyncClearsGenerationLatch() async throws {
        let outbox = EventOutbox()
        let binding = await outbox.beginConnectionBinding()
        let generation = try #require(await outbox.beginResumeAttempt(binding: binding))
        #expect(await outbox.finishResync(generation: generation))
        #expect(await outbox.confirmFreshSession(id: "fresh-session", binding: binding))
    }

    @Test("Stopping resume work clears the generation latch for a later fresh HELLO")
    func stopResumeWorkClearsGenerationLatch() async throws {
        let outbox = EventOutbox()
        let binding = await outbox.beginConnectionBinding()
        let generation = try #require(await outbox.beginResumeAttempt(binding: binding))
        await outbox.stopResumeWork(generation: generation)
        #expect(await outbox.confirmFreshSession(id: "fresh-session", binding: binding))
    }

    @Test("A superseded generation cannot release the latch held by a newer attempt")
    func supersededGenerationCannotReleaseNewerLatch() async throws {
        let outbox = EventOutbox()
        let binding = await outbox.beginConnectionBinding()
        let superseded = try #require(await outbox.beginResumeAttempt(binding: binding))
        let newest = try #require(await outbox.beginResumeAttempt(binding: binding))

        await outbox.stopResumeWork(generation: superseded)

        // The newest attempt still owns the latch, so its own decision is still the only one
        // that can settle the outbox (§18).
        let freshAccepted = await outbox.confirmFreshSession(
            id: "fresh-session",
            binding: binding
        )
        #expect(freshAccepted == false)
        let newestAccepted = await outbox.finishResync(generation: newest)
        #expect(newestAccepted == true)
    }

    @Test("Catch-up cannot re-enable allocation while a reconnect generation is outstanding")
    func allowNewEventsRefusesDuringOutstandingResume() async throws {
        let outbox = EventOutbox()
        let binding = await outbox.beginConnectionBinding()
        let idleAllowed = await outbox.allowNewEvents(binding: binding)
        #expect(idleAllowed == true)

        let generation = try #require(await outbox.beginResumeAttempt(binding: binding))
        let latchedAllowed = await outbox.allowNewEvents(binding: binding)
        #expect(latchedAllowed == false)

        let finished = await outbox.finishResync(generation: generation)
        #expect(finished == true)
        let releasedAllowed = await outbox.allowNewEvents(binding: binding)
        #expect(releasedAllowed == true)
    }

    @Test("Replaced session abandons pending events and resets sequence")
    func replacedSessionResetsSequence() async throws {
        let (client, server) = await PipeTransport.createPair()
        let outbox = EventOutbox()

        _ = try await outbox.sendActivate(nodeId: NodeId(1), observedRevision: Revision(1), via: client)
        _ = try await outbox.sendValueChanged(nodeId: NodeId(2), observedRevision: Revision(1), value: .bool(false), via: client)
        #expect(await outbox.pendingCount == 2)

        let binding = await outbox.beginConnectionBinding()
        let generation = try #require(await outbox.beginResumeAttempt(binding: binding))
        let accepted = await outbox.prepareReplacedSession(
            id: "new-incarnation",
            lastProcessedEventSeq: 0,
            generation: generation,
            binding: binding
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
        let binding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "session-window", binding: binding))

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
        let binding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "session-gap", binding: binding))

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

    @Test("A suspended TEXT_EDIT assignment cannot let a later event overtake it")
    func textEditAssignmentPreservesAllocationOrder() async throws {
        let transport = EventSequenceRecordingTransport()
        let outbox = EventOutbox()
        let gate = MainActorRenderBlocker()
        let editSeq = try #require(EditSeq(1))
        let binding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(
            id: "session-assignment-order",
            binding: binding
        ))

        let textTask = Task {
            try await outbox.queueTextEdit(
                nodeId: NodeId(12),
                text: "typed",
                editSeq: editSeq,
                observedRevision: Revision(1),
                via: transport,
                onAssigned: { _ in gate.block() }
            )
        }
        await gate.waitUntilEntered()

        let activateTask = Task {
            try await outbox.sendActivate(
                nodeId: NodeId(13),
                observedRevision: Revision(1),
                via: transport
            )
        }
        while await outbox.eventSeq < 2 {
            await Task.yield()
        }

        // Both identities have been allocated, but event 2 remains chained behind event 1 while
        // its assignment callback is suspended.
        #expect(await transport.sentEventSequences().isEmpty)
        gate.releaseRender()

        let text = try #require(try await textTask.value)
        let activate = try await activateTask.value
        #expect(text.eventSeq == 1)
        #expect(activate.eventSeq == 2)
        #expect(await transport.sentEventSequences() == [1, 2])

        await transport.close()
    }

    @Test("TEXT_EDIT serializes edit_seq and the whole-value TEXT argument")
    func textEditEventSerialization() async throws {
        let clientInstanceId = ClientInstanceId(string: "client-test-text")
        let outbox = EventOutbox(clientInstanceId: clientInstanceId)
        let (client, server) = await PipeTransport.createPair()
        let binding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "session-text", binding: binding))

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
        let binding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "session-coalesce", binding: binding))
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

    @Test("A cumulative ack retains earlier text lanes until each outcome is known")
    func cumulativeAckRetainsEarlierTextOutcomes() async throws {
        let (client, server) = await PipeTransport.createPair()
        let outbox = EventOutbox()
        let binding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "session-barriers", binding: binding))
        let edit1 = try #require(EditSeq(1))
        let edit2 = try #require(EditSeq(2))

        let first = try #require(try await outbox.queueTextEdit(
            nodeId: NodeId(12),
            text: "left-1",
            editSeq: edit1,
            observedRevision: Revision(1),
            via: client
        ))
        let second = try #require(try await outbox.queueTextEdit(
            nodeId: NodeId(13),
            text: "right-1",
            editSeq: edit1,
            observedRevision: Revision(1),
            via: client
        ))
        _ = try await outbox.queueTextEdit(
            nodeId: NodeId(12),
            text: "left-2",
            editSeq: edit2,
            observedRevision: Revision(1),
            via: client
        )
        _ = try await outbox.queueTextEdit(
            nodeId: NodeId(13),
            text: "right-2",
            editSeq: edit2,
            observedRevision: Revision(1),
            via: client
        )

        let secondSettlement = await outbox.settleAcknowledgement(
            clientInstanceId: outbox.clientInstanceId,
            eventId: second.eventId,
            throughSeq: second.eventSeq,
            sessionId: "session-barriers",
            revisionAfterEffect: 5
        )
        #expect(secondSettlement.settledEvents.map(\.eventId) == [second.eventId])
        #expect(await outbox.assignedTextEditDescriptors().map(\.eventId) == [first.eventId])
        #expect(try await outbox.promoteReadyTextDrafts(via: client).isEmpty)
        #expect(await outbox.releaseTextAcknowledgements(through: 4).isEmpty)

        let secondBarrier = await outbox.releaseTextAcknowledgements(through: 5)
        #expect(secondBarrier.map(\.eventId) == [second.eventId])
        let rightPromoted = try await outbox.promoteReadyTextDrafts(via: client)
        #expect(rightPromoted.compactMap(\.textArg) == ["right-2"])

        let firstSettlement = await outbox.settleAcknowledgement(
            clientInstanceId: outbox.clientInstanceId,
            eventId: first.eventId,
            throughSeq: second.eventSeq,
            sessionId: "session-barriers",
            revisionAfterEffect: 4,
            textEditRejected: true
        )
        #expect(firstSettlement.settledEvents.map(\.eventId) == [first.eventId])
        let firstBarrier = await outbox.releaseTextAcknowledgements(through: 5)
        #expect(firstBarrier.map(\.eventId) == [first.eventId])
        #expect(firstBarrier.first?.rejected == true)
        let leftPromoted = try await outbox.promoteReadyTextDrafts(via: client)
        #expect(leftPromoted.compactMap(\.textArg) == ["left-2"])

        await client.close()
        await server.close()
    }

    @Test("queueTextEdit retains a draft while dispatch is suspended")
    func queueTextEditWhileSuspendedKeepsNewestDraft() async throws {
        let (client, server) = await PipeTransport.createPair()
        let outbox = EventOutbox()
        let binding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "session-suspend", binding: binding))
        let generation = try #require(await outbox.beginResumeAttempt(binding: binding))

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
        let binding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "session-cancel", binding: binding))
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

    @Test("TEXT_EDIT cancel is a no-op when the resume generation no longer owns the latch")
    func cancelAssignedTextEditsRespectsResumeGeneration() async throws {
        let (client, server) = await PipeTransport.createPair()
        let outbox = EventOutbox()
        let binding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "session-gen", binding: binding))
        let editSeq = try #require(EditSeq(1))
        let assigned = try #require(try await outbox.queueTextEdit(
            nodeId: NodeId(12),
            text: "typed",
            editSeq: editSeq,
            observedRevision: Revision(1),
            via: client
        ))
        let discarded = [assigned].compactMap { event -> SRUIPendingTextEditRef? in
            guard let seq = event.editSeq else { return nil }
            return PendingTextEditDescriptor(
                eventId: event.eventId,
                eventSeq: event.eventSeq,
                nodeId: event.nodeId,
                editSeq: seq
            ).toWire()
        }

        let stale = await outbox.beginResumeAttempt()
        let current = await outbox.beginResumeAttempt()

        let skipped = try await outbox.cancelAssignedTextEdits(
            confirming: discarded,
            requireExactMatch: true,
            onlyIfResumeGeneration: stale
        )
        #expect(!skipped)
        #expect(await outbox.assignedTextEditDescriptors().count == 1)

        let liveSkipped = try await outbox.cancelAssignedTextEdits(
            confirming: discarded,
            requireExactMatch: true,
            onlyIfResumeGeneration: nil
        )
        #expect(!liveSkipped)
        #expect(await outbox.assignedTextEditDescriptors().count == 1)

        let canceled = try await outbox.cancelAssignedTextEdits(
            confirming: discarded,
            requireExactMatch: true,
            onlyIfResumeGeneration: current
        )
        #expect(canceled)
        #expect(await outbox.assignedTextEditDescriptors().isEmpty)

        await client.close()
        await server.close()
    }

    @Test("completeSameSessionResume cancels TEXT_EDITs only for the owning generation")
    func completeSameSessionResumeBindsTextCancelToGeneration() async throws {
        let (seedClient, seedServer) = await PipeTransport.createPair()
        let outbox = EventOutbox()
        let binding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "session-bind", binding: binding))
        let editSeq = try #require(EditSeq(1))
        let assigned = try #require(try await outbox.queueTextEdit(
            nodeId: NodeId(12),
            text: "typed",
            editSeq: editSeq,
            observedRevision: Revision(1),
            via: seedClient
        ))
        let discarded = [
            PendingTextEditDescriptor(
                eventId: assigned.eventId,
                eventSeq: assigned.eventSeq,
                nodeId: assigned.nodeId,
                editSeq: try #require(assigned.editSeq)
            ).toWire()
        ]

        let stale = await outbox.beginResumeAttempt()
        let current = await outbox.beginResumeAttempt()

        let (client, server) = await PipeTransport.createPair()
        let refused = try await outbox.completeSameSessionResume(
            id: "session-bind",
            lastProcessedEventSeq: 0,
            generation: stale,
            via: client,
            enableNewEventsAfterReplay: false,
            discardedTextEdits: discarded
        )
        #expect(!refused)
        #expect(await outbox.assignedTextEditDescriptors().count == 1)

        let accepted = try await outbox.completeSameSessionResume(
            id: "session-bind",
            lastProcessedEventSeq: 0,
            generation: current,
            via: client,
            enableNewEventsAfterReplay: false,
            discardedTextEdits: discarded
        )
        #expect(accepted)
        #expect(await outbox.assignedTextEditDescriptors().isEmpty)

        await client.close()
        await server.close()
        await seedClient.close()
        await seedServer.close()
    }

    @Test("Older text callbacks cannot replace a newer coalesced draft")
    func olderTextCallbackCannotReplaceNewerDraft() async throws {
        let (client, server) = await PipeTransport.createPair()
        let outbox = EventOutbox()
        let binding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "session-order", binding: binding))

        let firstEditSeq = try #require(EditSeq(1))
        let staleEditSeq = try #require(EditSeq(2))
        let newestEditSeq = try #require(EditSeq(3))
        let first = try #require(try await outbox.queueTextEdit(
            nodeId: NodeId(12),
            text: "a",
            editSeq: firstEditSeq,
            observedRevision: Revision(1),
            via: client,
            laneEpoch: 1
        ))
        _ = try await outbox.queueTextEdit(
            nodeId: NodeId(12),
            text: "newest",
            editSeq: newestEditSeq,
            observedRevision: Revision(1),
            via: client,
            laneEpoch: 3
        )
        _ = try await outbox.queueTextEdit(
            nodeId: NodeId(12),
            text: "stale",
            editSeq: staleEditSeq,
            observedRevision: Revision(1),
            via: client,
            laneEpoch: 3
        )

        #expect(await outbox.settleAcknowledgement(
            clientInstanceId: outbox.clientInstanceId,
            eventId: first.eventId,
            throughSeq: first.eventSeq,
            sessionId: "session-order"
        ).bound)
        let promoted = try await outbox.promoteReadyTextDrafts(via: client)
        #expect(promoted.count == 1)
        #expect(promoted[0].textArg == "newest")
        #expect(promoted[0].editSeq?.rawValue == 3)

        await client.close()
        await server.close()
    }

    @Test("An older invalidation cannot discard a newer text draft")
    func olderInvalidationCannotDiscardNewerDraft() async throws {
        let (client, server) = await PipeTransport.createPair()
        let outbox = EventOutbox()
        let binding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "session-invalidate", binding: binding))
        let generation = try #require(await outbox.beginResumeAttempt(binding: binding))

        let newestEditSeq = try #require(EditSeq(3))
        _ = try await outbox.queueTextEdit(
            nodeId: NodeId(12),
            text: "newest",
            editSeq: newestEditSeq,
            observedRevision: Revision(1),
            via: client,
            laneEpoch: 3
        )
        await outbox.invalidateTextDraft(nodeId: NodeId(12), laneEpoch: 2)
        #expect(await outbox.unsentTextDraftCount == 1)

        let resumed = try await outbox.completeSameSessionResume(
            id: "session-invalidate",
            lastProcessedEventSeq: 0,
            generation: generation,
            via: client,
            enableNewEventsAfterReplay: true
        )
        #expect(resumed)
        let assigned = await outbox.assignedTextEditDescriptors()
        #expect(assigned.count == 1)
        #expect(assigned[0].editSeq.rawValue == 3)

        await client.close()
        await server.close()
    }

    @Test("Resume frontier replays text until its cached outcome is acknowledged")
    func resumeFrontierRetainsTextUntilOutcomeAck() async throws {
        let (seedClient, seedServer) = await PipeTransport.createPair()
        let outbox = EventOutbox()
        let binding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "session-outcome", binding: binding))
        let editSeq = try #require(EditSeq(1))
        let pending = try #require(try await outbox.queueTextEdit(
            nodeId: NodeId(12),
            text: "invalid",
            editSeq: editSeq,
            observedRevision: Revision(1),
            via: seedClient
        ))

        let generation = try #require(await outbox.beginResumeAttempt(binding: binding))
        let (client, server) = await PipeTransport.createPair()
        let serverStream = server.receiveStream()
        let resumed = try await outbox.completeSameSessionResume(
            id: "session-outcome",
            lastProcessedEventSeq: pending.eventSeq,
            generation: generation,
            via: client,
            enableNewEventsAfterReplay: true
        )
        #expect(resumed)

        #expect(await outbox.lastAckedEventSeq == pending.eventSeq)
        #expect(await outbox.pendingCount == 1)
        #expect(await outbox.assignedTextEditDescriptors().map(\.eventId) == [pending.eventId])

        var streamDecoder = SRUIMessageStreamDecoder()
        var replayedEvent: Event?
        for try await chunk in serverStream {
            for message in try streamDecoder.appendAndExtract(incoming: chunk) {
                if case .event(let wireEvent) = message.msg {
                    replayedEvent = try ProtocolDecoder().validateAndConvertEvent(wire: wireEvent)
                    break
                }
            }
            if replayedEvent != nil {
                break
            }
        }
        #expect(replayedEvent?.eventId == pending.eventId)
        #expect(replayedEvent?.eventSeq == pending.eventSeq)
        #expect(replayedEvent?.editSeq == pending.editSeq)

        let later = try await outbox.sendActivate(
            nodeId: NodeId(13),
            observedRevision: Revision(2),
            via: client
        )
        let laterSettlement = await outbox.settleAcknowledgement(
            clientInstanceId: outbox.clientInstanceId,
            eventId: later.eventId,
            throughSeq: later.eventSeq,
            sessionId: "session-outcome",
            revisionAfterEffect: 2
        )
        #expect(laterSettlement.bound)
        #expect(await outbox.pendingCount == 1)
        #expect(await outbox.assignedTextEditDescriptors().map(\.eventId) == [pending.eventId])

        let settlement = await outbox.settleAcknowledgement(
            clientInstanceId: outbox.clientInstanceId,
            eventId: pending.eventId,
            throughSeq: later.eventSeq,
            sessionId: "session-outcome",
            revisionAfterEffect: 2,
            textEditRejected: true
        )
        #expect(settlement.bound)
        #expect(settlement.event?.eventId == pending.eventId)
        #expect(settlement.settledEvents.map(\.eventId) == [pending.eventId])
        #expect(await outbox.pendingCount == 0)

        await client.close()
        await server.close()
        await seedClient.close()
        await seedServer.close()
    }

    @Test("A reconnect generation makes live same-session resync an atomic no-op")
    func reconnectSupersedesLiveSameSessionResyncAtomically() async throws {
        let (client, server) = await PipeTransport.createPair()
        let outbox = EventOutbox()
        let binding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "session-live-race", binding: binding))
        let editSeq = try #require(EditSeq(1))
        let edit = try #require(try await outbox.queueTextEdit(
            nodeId: NodeId(12),
            text: "pending",
            editSeq: editSeq,
            observedRevision: Revision(1),
            via: client
        ))
        let generation = try #require(await outbox.beginResumeAttempt(binding: binding))

        let decision = try await outbox.applyLiveSameSessionResync(
            lastProcessedEventSeq: edit.eventSeq,
            binding: binding
        )
        guard case .superseded = decision else {
            Issue.record("Expected the reconnect generation to supersede live resync")
            return
        }
        #expect(await outbox.assignedTextEditDescriptors().map(\.eventId) == [edit.eventId])
        #expect(await outbox.lastAckedEventSeq == 0)

        await outbox.stopResumeWork(generation: generation)
        await client.close()
        await server.close()
    }

    @Test("Live resync defers an edit above the server frontier to resume cancellation")
    func liveResyncPreservesUndeliveredTextEditForResume() async throws {
        let transport = EventSequenceRecordingTransport()
        let outbox = EventOutbox()
        let gate = MainActorRenderBlocker()
        let editSeq = try #require(EditSeq(1))
        let binding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "session-live-pending", binding: binding))

        let textTask = Task {
            try await outbox.queueTextEdit(
                nodeId: NodeId(12),
                text: "not delivered",
                editSeq: editSeq,
                observedRevision: Revision(1),
                via: transport,
                onAssigned: { _ in gate.block() }
            )
        }
        await gate.waitUntilEntered()
        let descriptor = try #require(await outbox.assignedTextEditDescriptors().first)

        let decision = try await outbox.applyLiveSameSessionResync(
            lastProcessedEventSeq: 0,
            binding: binding
        )
        guard case .resumeRequired = decision else {
            Issue.record("Expected resume cancellation for an edit above the server frontier")
            return
        }
        #expect(await outbox.assignedTextEditDescriptors() == [descriptor])
        #expect(await outbox.lastAckedEventSeq == 0)

        gate.releaseRender()
        _ = try #require(try await textTask.value)
        #expect(await transport.sentEventSequences().isEmpty)

        let resumedBinding = await outbox.beginConnectionBinding()
        let generation = try #require(
            await outbox.beginResumeAttempt(binding: resumedBinding)
        )
        let resumed = try await outbox.completeSameSessionResume(
            id: "session-live-pending",
            lastProcessedEventSeq: 0,
            generation: generation,
            via: transport,
            enableNewEventsAfterReplay: true,
            discardedTextEdits: [descriptor.toWire()]
        )
        #expect(resumed)
        #expect(await outbox.pendingCount == 0)
        #expect(await outbox.lastAckedEventSeq == 1)

        let next = try await outbox.sendActivate(
            nodeId: NodeId(13),
            observedRevision: Revision(1),
            via: transport
        )
        #expect(next.eventSeq == 2)
        #expect(await transport.sentEventSequences() == [2])

        await transport.close()
    }

    @Test("Live resync locally discards only text edits covered by the server frontier")
    func liveResyncDiscardsDeliveredTextEditWithoutBreakingSequence() async throws {
        let transport = EventSequenceRecordingTransport()
        let outbox = EventOutbox()
        let binding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "session-live-covered", binding: binding))
        let editSeq = try #require(EditSeq(1))

        let edit = try #require(try await outbox.queueTextEdit(
            nodeId: NodeId(12),
            text: "delivered",
            editSeq: editSeq,
            observedRevision: Revision(1),
            via: transport
        ))
        let decision = try await outbox.applyLiveSameSessionResync(
            lastProcessedEventSeq: edit.eventSeq,
            binding: binding
        )
        let canceled: [SRUIPendingTextEditRef]
        guard case .applied(let refs) = decision else {
            Issue.record("Expected the server frontier to authorize local cancellation")
            return
        }
        canceled = refs
        #expect(canceled.map(\.eventID) == [edit.eventId.bytes])
        #expect(await outbox.pendingCount == 0)
        #expect(await outbox.lastAckedEventSeq == 1)

        let committed = try #require(await outbox.commitResyncSnapshot(
            generation: nil,
            binding: binding,
            publish: { true },
            committed: { $0 }
        ))
        let renderToken = try #require(committed.renderToken)
        let rendered = await MainActor.run {
            outbox.resyncRenderFence.performIfActive(renderToken) { true }
        }
        #expect(rendered == true)
        #expect(await outbox.applyFullResyncTextBoundary(
            laneEpoch: nil,
            generation: nil,
            binding: binding,
            renderToken: renderToken
        ))
        #expect(await outbox.allowNewEvents(binding: binding))

        let next = try await outbox.sendActivate(
            nodeId: NodeId(13),
            observedRevision: Revision(2),
            via: transport
        )
        #expect(next.eventSeq == 2)
        #expect(await transport.sentEventSequences() == [1, 2])

        await transport.close()
    }

    @Test("A failed snapshot render discards queued drafts before reconnect")
    func abortFailedSnapshotRenderAppliesTextBoundary() async throws {
        let outbox = EventOutbox()
        let binding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "session-render-failure", binding: binding))
        let generation = try #require(await outbox.beginResumeAttempt(binding: binding))
        let transport = EventSequenceRecordingTransport()
        _ = try await outbox.queueTextEdit(
            nodeId: NodeId(12),
            text: "pre-snapshot draft",
            editSeq: try #require(EditSeq(1)),
            observedRevision: Revision(1),
            via: transport,
            laneEpoch: 1
        )
        #expect(await outbox.unsentTextDraftCount == 1)

        let committed = try #require(await outbox.commitResyncSnapshot(
            generation: generation,
            binding: binding,
            publish: { true },
            committed: { $0 }
        ))
        let renderToken = try #require(committed.renderToken)
        #expect(await outbox.abortResyncRender(
            laneEpoch: 2,
            generation: generation,
            binding: binding,
            renderToken: renderToken
        ))
        #expect(await outbox.unsentTextDraftCount == 0)
        let rendered = await MainActor.run {
            outbox.resyncRenderFence.performIfActive(renderToken) { true }
        }
        #expect(rendered == nil)

        let resumedBinding = await outbox.beginConnectionBinding()
        let resumedGeneration = try #require(
            await outbox.beginResumeAttempt(binding: resumedBinding)
        )
        #expect(try await outbox.completeSameSessionResume(
            id: "session-render-failure",
            lastProcessedEventSeq: 0,
            generation: resumedGeneration,
            via: transport,
            enableNewEventsAfterReplay: true
        ))
        #expect(await outbox.unsentTextDraftCount == 0)
        #expect(await outbox.eventSeq == 0)
        #expect(await transport.sentEventSequences().isEmpty)

        await transport.close()
    }

    @Test("A stale connection binding cannot mutate a newly bound same-session controller")
    func staleConnectionBindingCannotMutateNewSession() async throws {
        let outbox = EventOutbox()
        let staleBinding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "same-session", binding: staleBinding))
        #expect(await outbox.suspendNewEvents(binding: staleBinding))
        let staleCommit = try #require(await outbox.commitResyncSnapshot(
            generation: nil,
            binding: staleBinding,
            publish: { true },
            committed: { $0 }
        ))
        let staleRenderToken = try #require(staleCommit.renderToken)

        let currentBinding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "same-session", binding: currentBinding))
        let transport = EventSequenceRecordingTransport()
        let editSeq = try #require(EditSeq(1))
        let edit = try #require(try await outbox.queueTextEdit(
            nodeId: NodeId(12),
            text: "new binding",
            editSeq: editSeq,
            observedRevision: Revision(1),
            via: transport
        ))

        let staleDecision = try await outbox.applyLiveSameSessionResync(
            lastProcessedEventSeq: edit.eventSeq,
            binding: staleBinding
        )
        guard case .superseded = staleDecision else {
            Issue.record("Expected stale live resync to be superseded")
            return
        }
        #expect(await outbox.assignedTextEditDescriptors().map(\.eventId) == [edit.eventId])
        #expect(await outbox.lastAckedEventSeq == 0)

        let stalePublished = await outbox.commitResyncSnapshot(
            generation: nil,
            binding: staleBinding,
            publish: { true },
            committed: { $0 }
        )
        #expect(stalePublished == nil)
        #expect(await outbox.applyFullResyncTextBoundary(
            laneEpoch: nil,
            generation: nil,
            binding: staleBinding,
            renderToken: staleRenderToken
        ) == false)
        let staleRendered = await MainActor.run {
            outbox.resyncRenderFence.performIfActive(staleRenderToken) { true }
        }
        #expect(staleRendered == nil)

        await transport.close()
    }

    @Test("Rebinding after a completed snapshot render preserves only post-snapshot drafts")
    func rebindingPreservesPostSnapshotDrafts() async throws {
        let outbox = EventOutbox()
        let staleBinding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "same-session", binding: staleBinding))
        #expect(await outbox.suspendNewEvents(binding: staleBinding))
        let transport = EventSequenceRecordingTransport()
        let editSeq = try #require(EditSeq(1))

        _ = try await outbox.queueTextEdit(
            nodeId: NodeId(12),
            text: "pre-snapshot",
            editSeq: editSeq,
            observedRevision: Revision(1),
            via: transport,
            laneEpoch: 1
        )
        let committed = try #require(await outbox.commitResyncSnapshot(
            generation: nil,
            binding: staleBinding,
            publish: { true },
            committed: { $0 }
        ))
        let renderToken = try #require(committed.renderToken)
        let renderedBoundary = await MainActor.run {
            outbox.resyncRenderFence.performIfActive(
                renderToken,
                boundaryEpoch: { $0 }
            ) {
                UInt64(2)
            }
        }
        #expect(renderedBoundary == 2)

        _ = try await outbox.queueTextEdit(
            nodeId: NodeId(13),
            text: "post-snapshot",
            editSeq: editSeq,
            observedRevision: Revision(2),
            via: transport,
            laneEpoch: 3
        )
        #expect(await outbox.unsentTextDraftCount == 2)

        let currentBinding = await outbox.beginConnectionBinding()
        #expect(await outbox.unsentTextDraftCount == 1)
        #expect(await outbox.confirmFreshSession(id: "same-session", binding: currentBinding))
        let promoted = try await outbox.promoteReadyTextDrafts(via: transport)
        #expect(promoted.map(\.nodeId) == [NodeId(13)])
        #expect(promoted.map(\.textArg) == ["post-snapshot"])

        await transport.close()
    }

    @Test("A newer resume generation invalidates a stale snapshot render and boundary")
    func newerResumeInvalidatesStaleSnapshotRender() async throws {
        let outbox = EventOutbox()
        let binding = await outbox.beginConnectionBinding()
        let staleGeneration = try #require(await outbox.beginResumeAttempt(binding: binding))
        let committed = try #require(await outbox.commitResyncSnapshot(
            generation: staleGeneration,
            binding: binding,
            publish: { true },
            committed: { $0 }
        ))
        let renderToken = try #require(committed.renderToken)

        let currentGeneration = try #require(await outbox.beginResumeAttempt(binding: binding))
        let (client, server) = await PipeTransport.createPair()
        let editSeq = try #require(EditSeq(1))
        _ = try await outbox.queueTextEdit(
            nodeId: NodeId(12),
            text: "new generation",
            editSeq: editSeq,
            observedRevision: Revision(1),
            via: client,
            laneEpoch: 2
        )

        let rendered = await MainActor.run {
            outbox.resyncRenderFence.performIfActive(renderToken) { true }
        }
        #expect(rendered == nil)
        let applied = await outbox.applyFullResyncTextBoundary(
            laneEpoch: 2,
            generation: staleGeneration,
            binding: binding,
            renderToken: renderToken
        )
        #expect(!applied)
        #expect(await outbox.unsentTextDraftCount == 1)

        await outbox.stopResumeWork(generation: currentGeneration)
        await client.close()
        await server.close()
    }

    @Test("Stale bindings cannot send or settle current connection state")
    func staleBindingGuardsDataPlaneAndAcknowledgement() async throws {
        let outbox = EventOutbox()
        let staleTransport = EventSequenceRecordingTransport()
        let currentTransport = EventSequenceRecordingTransport()
        let staleBinding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "same-session", binding: staleBinding))
        let retained = try await outbox.sendActivate(
            nodeId: NodeId(1),
            observedRevision: Revision(1),
            binding: staleBinding,
            via: staleTransport
        )

        let currentBinding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "same-session", binding: currentBinding))
        do {
            _ = try await outbox.sendActivate(
                nodeId: NodeId(2),
                observedRevision: Revision(1),
                binding: staleBinding,
                via: staleTransport
            )
            Issue.record("Expected the stale binding to reject ACTIVATE")
        } catch let error as EventOutboxError {
            #expect(error == .resumeNotConfirmed)
        }

        let staleSettlement = await outbox.settleAcknowledgement(
            binding: staleBinding,
            clientInstanceId: outbox.clientInstanceId,
            eventId: retained.eventId,
            throughSeq: retained.eventSeq,
            sessionId: "same-session"
        )
        #expect(!staleSettlement.connectionBound)
        #expect(await outbox.pendingCount == 1)

        let currentSettlement = await outbox.settleAcknowledgement(
            binding: currentBinding,
            clientInstanceId: outbox.clientInstanceId,
            eventId: retained.eventId,
            throughSeq: retained.eventSeq,
            sessionId: "same-session"
        )
        #expect(currentSettlement.connectionBound)
        #expect(currentSettlement.bound)
        let current = try await outbox.sendActivate(
            nodeId: NodeId(3),
            observedRevision: Revision(1),
            binding: currentBinding,
            via: currentTransport
        )
        #expect(current.eventSeq == 2)
        #expect(await staleTransport.sentEventSequences() == [1])
        #expect(await currentTransport.sentEventSequences() == [2])

        await staleTransport.close()
        await currentTransport.close()
    }

    @Test("A queued successor text edit is sequenced before a following action")
    func textDraftCausalBarrierPrecedesAction() async throws {
        let outbox = EventOutbox()
        let transport = EventSequenceRecordingTransport()
        let binding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "session-order", binding: binding))
        let firstEditSeq = try #require(EditSeq(1))
        let secondEditSeq = try #require(EditSeq(2))

        let first = try #require(try await outbox.queueTextEdit(
            nodeId: NodeId(10),
            text: "first",
            editSeq: firstEditSeq,
            observedRevision: Revision(1),
            binding: binding,
            via: transport
        ))
        #expect(try await outbox.queueTextEdit(
            nodeId: NodeId(10),
            text: "latest",
            editSeq: secondEditSeq,
            observedRevision: Revision(1),
            binding: binding,
            via: transport
        ) == nil)

        let action = Task {
            try await outbox.sendActivate(
                nodeId: NodeId(11),
                observedRevision: Revision(1),
                binding: binding,
                via: transport
            )
        }
        #expect(await waitForTextDraftWaiter(outbox))
        #expect(await outbox.eventSeq == 1)

        let settlement = await outbox.settleAcknowledgement(
            binding: binding,
            clientInstanceId: outbox.clientInstanceId,
            eventId: first.eventId,
            throughSeq: first.eventSeq,
            sessionId: "session-order",
            revisionAfterEffect: 2
        )
        #expect(settlement.bound)
        _ = await outbox.releaseTextAcknowledgements(through: 2)

        let activation = try await action.value
        #expect(activation.eventSeq == 3)
        let events = await transport.sentEvents()
        #expect(events.map(\.eventSeq) == [1, 2, 3])
        #expect(events.map(\.eventType) == [
            .EVENT_TEXT_EDIT,
            .EVENT_TEXT_EDIT,
            .EVENT_ACTIVATE
        ])
        #expect(events[1].textArg == "latest")

        await transport.close()
    }

    @Test("The newest lifecycle transition inherits an in-progress snapshot invalidation")
    func newestLifecycleTransitionInheritsBoundary() async throws {
        let outbox = EventOutbox()
        let initialBinding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "same-session", binding: initialBinding))
        #expect(await outbox.suspendNewEvents(binding: initialBinding))
        let transport = EventSequenceRecordingTransport()
        let editSeq = try #require(EditSeq(1))
        _ = try await outbox.queueTextEdit(
            nodeId: NodeId(12),
            text: "pre-snapshot",
            editSeq: editSeq,
            observedRevision: Revision(1),
            binding: initialBinding,
            via: transport,
            laneEpoch: 1
        )
        let committed = try #require(await outbox.commitResyncSnapshot(
            generation: nil,
            binding: initialBinding,
            publish: { true },
            committed: { $0 }
        ))
        let renderToken = try #require(committed.renderToken)
        let blocker = MainActorRenderBlocker()
        let render = Task { @MainActor in
            outbox.resyncRenderFence.performIfActive(renderToken) {
                blocker.block()
                return true
            }
        }
        await blocker.waitUntilEntered()
        defer { blocker.releaseRender() }

        let firstRebind = Task { await outbox.beginConnectionBinding() }
        let intermediateBinding = try #require(
            await waitForActiveBinding(outbox, differentFrom: initialBinding)
        )
        let secondRebind = Task { await outbox.beginConnectionBinding() }
        let newestBinding = try #require(
            await waitForActiveBinding(outbox, differentFrom: intermediateBinding)
        )
        _ = try await outbox.queueTextEdit(
            nodeId: NodeId(13),
            text: "post-snapshot",
            editSeq: editSeq,
            observedRevision: Revision(2),
            binding: newestBinding,
            via: transport,
            laneEpoch: 2
        )

        blocker.releaseRender()
        _ = await render.value
        #expect(await firstRebind.value == intermediateBinding)
        #expect(await secondRebind.value == newestBinding)
        #expect(await outbox.unsentTextDraftCount == 1)
        #expect(await outbox.confirmFreshSession(id: "same-session", binding: newestBinding))
        let promoted = try await outbox.promoteReadyTextDrafts(
            binding: newestBinding,
            via: transport
        )
        #expect(promoted.map(\.nodeId) == [NodeId(13)])

        let staleGeneration = try #require(await outbox.beginResumeAttempt(binding: newestBinding))
        let nextCommit = try #require(await outbox.commitResyncSnapshot(
            generation: staleGeneration,
            binding: newestBinding,
            publish: { true },
            committed: { $0 }
        ))
        let nextToken = try #require(nextCommit.renderToken)
        let resumeBlocker = MainActorRenderBlocker()
        let resumeRender = Task { @MainActor in
            outbox.resyncRenderFence.performIfActive(nextToken) {
                resumeBlocker.block()
                return true
            }
        }
        await resumeBlocker.waitUntilEntered()
        defer { resumeBlocker.releaseRender() }

        let middleAttempt = Task {
            await outbox.beginResumeAttempt(binding: newestBinding)
        }
        #expect(await waitForResumeGeneration(outbox, staleGeneration + 1))
        let newestAttempt = Task {
            await outbox.beginResumeAttempt(binding: newestBinding)
        }
        #expect(await waitForResumeGeneration(outbox, staleGeneration + 2))
        resumeBlocker.releaseRender()
        _ = await resumeRender.value
        #expect(await middleAttempt.value == nil)
        #expect(await newestAttempt.value == staleGeneration + 2)

        await outbox.stopResumeWork(generation: staleGeneration + 2)
        await transport.close()
    }

    @Test("Stopping resume work applies the completed snapshot boundary")
    func stopResumeWorkAppliesCompletedBoundary() async throws {
        let outbox = EventOutbox()
        let binding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "same-session", binding: binding))
        let generation = try #require(await outbox.beginResumeAttempt(binding: binding))
        let transport = EventSequenceRecordingTransport()
        let editSeq = try #require(EditSeq(1))
        _ = try await outbox.queueTextEdit(
            nodeId: NodeId(12),
            text: "pre-snapshot",
            editSeq: editSeq,
            observedRevision: Revision(1),
            binding: binding,
            via: transport,
            laneEpoch: 1
        )
        let committed = try #require(await outbox.commitResyncSnapshot(
            generation: generation,
            binding: binding,
            publish: { true },
            committed: { $0 }
        ))
        let renderToken = try #require(committed.renderToken)
        #expect(await MainActor.run {
            outbox.resyncRenderFence.performIfActive(
                renderToken,
                boundaryEpoch: { $0 }
            ) {
                UInt64(2)
            }
        } == 2)
        _ = try await outbox.queueTextEdit(
            nodeId: NodeId(13),
            text: "post-snapshot",
            editSeq: editSeq,
            observedRevision: Revision(2),
            binding: binding,
            via: transport,
            laneEpoch: 3
        )

        await outbox.stopResumeWork(generation: generation)
        #expect(await outbox.unsentTextDraftCount == 1)
        let currentBinding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "same-session", binding: currentBinding))
        let promoted = try await outbox.promoteReadyTextDrafts(
            binding: currentBinding,
            via: transport
        )
        #expect(promoted.map(\.nodeId) == [NodeId(13)])

        await transport.close()
    }

    @Test("A live render token is invalidated before a newer binding starts")
    func liveRenderTokenCannotOutliveBinding() async throws {
        let outbox = EventOutbox()
        let staleBinding = await outbox.beginConnectionBinding()
        let staleIncarnation = try #require(await outbox.sessionIncarnation(binding: staleBinding))
        #expect(await outbox.confirmFreshSession(id: "same-session", binding: staleBinding))
        let committed = try #require(await outbox.commitLiveRender(
            binding: staleBinding,
            publish: { true },
            committed: { $0 }
        ))
        let renderToken = try #require(committed.renderToken)
        let blocker = MainActorRenderBlocker()
        let render = Task { @MainActor in
            outbox.resyncRenderFence.performIfActive(renderToken) {
                blocker.block()
                return true
            }
        }
        await blocker.waitUntilEntered()
        defer { blocker.releaseRender() }

        let rebind = Task { await outbox.beginConnectionBinding() }
        let currentBinding = try #require(
            await waitForActiveBinding(outbox, differentFrom: staleBinding)
        )
        blocker.releaseRender()
        #expect(await render.value == true)
        #expect(await rebind.value == currentBinding)
        let staleCompletion = await outbox.completeLiveRender(
            binding: staleBinding,
            sessionIncarnation: staleIncarnation,
            renderToken: renderToken
        )
        #expect(!staleCompletion)
        #expect(await outbox.commitLiveRender(
            binding: staleBinding,
            publish: { true },
            committed: { $0 }
        ) == nil)
    }

    @Test("Resume recovery ownership is generation scoped")
    func resumeRecoveryOwnershipIsGenerationScoped() async throws {
        let outbox = EventOutbox()
        let binding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "same-session", binding: binding))
        let generation = try #require(await outbox.beginResumeAttempt(binding: binding))
        let token = try #require(await outbox.beginResumeRecoveryRender(
            binding: binding,
            generation: generation
        ))
        let blocker = MainActorRenderBlocker()
        let render = Task { @MainActor in
            outbox.resyncRenderFence.performIfActive(token) {
                blocker.block()
                return true
            }
        }
        await blocker.waitUntilEntered()
        defer { blocker.releaseRender() }

        let newerAttempt = Task {
            await outbox.beginResumeAttempt(binding: binding)
        }
        #expect(await waitForResumeGeneration(outbox, generation + 1))
        blocker.releaseRender()
        #expect(await render.value == true)
        #expect(await newerAttempt.value == generation + 1)
        let staleCompletion = await outbox.completeResumeRecoveryRender(
            binding: binding,
            generation: generation,
            renderToken: token
        )
        #expect(!staleCompletion)

        let currentToken = try #require(await outbox.beginResumeRecoveryRender(
            binding: binding,
            generation: generation + 1
        ))
        #expect(await MainActor.run {
            outbox.resyncRenderFence.performIfActive(currentToken) { true }
        } == true)
        #expect(await outbox.completeResumeRecoveryRender(
            binding: binding,
            generation: generation + 1,
            renderToken: currentToken
        ))
        await outbox.stopResumeWork(generation: generation + 1)
    }

    @Test("Resume recovery ownership is connection scoped")
    func resumeRecoveryOwnershipIsConnectionScoped() async throws {
        let outbox = EventOutbox()
        let staleBinding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "same-session", binding: staleBinding))
        let generation = try #require(await outbox.beginResumeAttempt(binding: staleBinding))
        let token = try #require(await outbox.beginResumeRecoveryRender(
            binding: staleBinding,
            generation: generation
        ))
        let blocker = MainActorRenderBlocker()
        let render = Task { @MainActor in
            outbox.resyncRenderFence.performIfActive(token) {
                blocker.block()
                return true
            }
        }
        await blocker.waitUntilEntered()
        defer { blocker.releaseRender() }

        let rebind = Task { await outbox.beginConnectionBinding() }
        let currentBinding = try #require(
            await waitForActiveBinding(outbox, differentFrom: staleBinding)
        )
        blocker.releaseRender()
        #expect(await render.value == true)
        #expect(await rebind.value == currentBinding)
        let staleCompletion = await outbox.completeResumeRecoveryRender(
            binding: staleBinding,
            generation: generation,
            renderToken: token
        )
        #expect(!staleCompletion)
        #expect(await outbox.confirmFreshSession(id: "same-session", binding: currentBinding))
    }

    @Test("Recovery abort clears ownership without reopening dispatch")
    func recoveryAbortLeavesDispatchClosed() async throws {
        let outbox = EventOutbox()
        let transport = EventSequenceRecordingTransport()
        let binding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "same-session", binding: binding))
        let generation = try #require(await outbox.beginResumeAttempt(binding: binding))
        let token = try #require(await outbox.beginResumeRecoveryRender(
            binding: binding,
            generation: generation
        ))
        #expect(await outbox.abortResumeRecoveryRender(
            binding: binding,
            generation: generation,
            renderToken: token
        ))

        await #expect(throws: EventOutboxError.resumeNotConfirmed) {
            try await outbox.sendActivate(
                nodeId: NodeId(1),
                observedRevision: Revision(1),
                binding: binding,
                via: transport
            )
        }
        let replacementToken = try #require(await outbox.beginResumeRecoveryRender(
            binding: binding,
            generation: generation
        ))
        #expect(await outbox.abortResumeRecoveryRender(
            binding: binding,
            generation: generation,
            renderToken: replacementToken
        ))
        await outbox.stopResumeWork(generation: generation)
        await transport.close()
    }

    @Test("A canceled queued text edit never reaches the native assignment callback")
    func canceledQueuedTextEditSkipsAssignmentCallback() async throws {
        let gate = AssignmentSuspensionGate()
        let transport = FirstSendBlockingTransport(gate: gate)
        let outbox = EventOutbox()
        let binding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "old-session", binding: binding))
        let recorder = await MainActor.run { TextAssignmentRecorder() }

        let predecessor = Task {
            try await outbox.sendActivate(
                nodeId: NodeId(1),
                observedRevision: Revision(1),
                binding: binding,
                via: transport
            )
        }
        await gate.waitUntilEntered()
        let editSeq = try #require(EditSeq(1))
        let queued = Task {
            try await outbox.queueTextEdit(
                nodeId: NodeId(2),
                text: "stale",
                editSeq: editSeq,
                observedRevision: Revision(1),
                binding: binding,
                via: transport,
                onAssigned: { event in recorder.note(event) }
            )
        }
        while await outbox.eventSeq < 2 {
            await Task.yield()
        }

        _ = await outbox.beginConnectionBinding()
        await gate.releaseAssignment()
        _ = try? await predecessor.value
        _ = try? await queued.value
        #expect(await MainActor.run { recorder.events.isEmpty })
    }

    @Test("A superseded same-session cleanup preserves the newer binding's draft")
    func supersededSameSessionCleanupPreservesNewDraft() async throws {
        let outbox = EventOutbox()
        let transport = EventSequenceRecordingTransport()
        let staleBinding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "same-session", binding: staleBinding))
        let editSeq1 = try #require(EditSeq(1))
        let retained = try await outbox.queueTextEdit(
            nodeId: NodeId(40),
            text: "old",
            editSeq: editSeq1,
            observedRevision: Revision(1),
            binding: staleBinding,
            via: transport
        )
        _ = try #require(retained)

        let gate = AssignmentSuspensionGate()
        await outbox.setNativeTextLifecycleWillHopForTesting {
            await gate.holdAssignment()
        }
        let recorder = await MainActor.run { TextAssignmentRecorder() }
        let staleCleanup = Task {
            try await outbox.applyLiveSameSessionResync(
                lastProcessedEventSeq: 1,
                binding: staleBinding,
                onTextEditsCanceled: { descriptors in
                    recorder.noteCancellation(descriptors)
                }
            )
        }
        await gate.waitUntilEntered()

        let currentBinding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "same-session", binding: currentBinding))
        let editSeq2 = try #require(EditSeq(2))
        let successor = try await outbox.queueTextEdit(
            nodeId: NodeId(40),
            text: "new",
            editSeq: editSeq2,
            observedRevision: Revision(2),
            binding: currentBinding,
            via: transport
        )
        #expect(successor == nil)

        await gate.releaseAssignment()
        let decision = try await staleCleanup.value
        if case .superseded = decision {
            // Expected: the newer binding won before native cleanup.
        } else {
            Issue.record("Stale same-session cleanup was not superseded")
        }
        #expect(await MainActor.run { recorder.cancellationCalls.isEmpty })
        #expect(await outbox.pendingCount == 1)
        #expect(await outbox.unsentTextDraftCount == 1)
        await transport.close()
    }

    @Test("A superseded live replacement preserves the newer binding's draft")
    func supersededLiveReplacementPreservesNewDraft() async throws {
        let outbox = EventOutbox()
        let transport = EventSequenceRecordingTransport()
        let staleBinding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "old-session", binding: staleBinding))
        let editSeq1 = try #require(EditSeq(1))
        let retained = try await outbox.queueTextEdit(
            nodeId: NodeId(41),
            text: "old",
            editSeq: editSeq1,
            observedRevision: Revision(1),
            binding: staleBinding,
            via: transport
        )
        _ = try #require(retained)

        let gate = AssignmentSuspensionGate()
        await outbox.setNativeTextLifecycleWillHopForTesting {
            await gate.holdAssignment()
        }
        let recorder = await MainActor.run { TextAssignmentRecorder() }
        let staleReplacement = Task {
            await outbox.applyReplacementFrontier(
                id: "replacement",
                lastProcessedEventSeq: 10,
                binding: staleBinding,
                onTextEditingReset: { _ in recorder.noteReset() }
            )
        }
        await gate.waitUntilEntered()

        let currentBinding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "current-session", binding: currentBinding))
        let editSeq2 = try #require(EditSeq(2))
        let successor = try await outbox.queueTextEdit(
            nodeId: NodeId(41),
            text: "new",
            editSeq: editSeq2,
            observedRevision: Revision(2),
            binding: currentBinding,
            via: transport
        )
        #expect(successor == nil)

        await gate.releaseAssignment()
        let applied = await staleReplacement.value
        #expect(!applied)
        #expect(await MainActor.run { recorder.resetCount == 0 })
        #expect(await outbox.eventSeq == 1)
        #expect(await outbox.pendingCount == 1)
        #expect(await outbox.unsentTextDraftCount == 1)
        await transport.close()
    }

    @Test("A superseded resumed replacement preserves the newer binding's draft")
    func supersededResumedReplacementPreservesNewDraft() async throws {
        let outbox = EventOutbox()
        let transport = EventSequenceRecordingTransport()
        let staleBinding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "old-session", binding: staleBinding))
        let editSeq1 = try #require(EditSeq(1))
        let retained = try await outbox.queueTextEdit(
            nodeId: NodeId(42),
            text: "old",
            editSeq: editSeq1,
            observedRevision: Revision(1),
            binding: staleBinding,
            via: transport
        )
        _ = try #require(retained)
        let generationValue = await outbox.beginResumeAttempt(binding: staleBinding)
        let generation = try #require(generationValue)

        let gate = AssignmentSuspensionGate()
        await outbox.setNativeTextLifecycleWillHopForTesting {
            await gate.holdAssignment()
        }
        let recorder = await MainActor.run { TextAssignmentRecorder() }
        let staleReplacement = Task {
            await outbox.prepareReplacedSession(
                id: "replacement",
                lastProcessedEventSeq: 10,
                generation: generation,
                binding: staleBinding,
                onTextEditingReset: { _ in recorder.noteReset() }
            )
        }
        await gate.waitUntilEntered()

        let currentBinding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "current-session", binding: currentBinding))
        let editSeq2 = try #require(EditSeq(2))
        let successor = try await outbox.queueTextEdit(
            nodeId: NodeId(42),
            text: "new",
            editSeq: editSeq2,
            observedRevision: Revision(2),
            binding: currentBinding,
            via: transport
        )
        #expect(successor == nil)

        await gate.releaseAssignment()
        let applied = await staleReplacement.value
        #expect(!applied)
        #expect(await MainActor.run { recorder.resetCount == 0 })
        #expect(await outbox.eventSeq == 1)
        #expect(await outbox.pendingCount == 1)
        #expect(await outbox.unsentTextDraftCount == 1)
        await transport.close()
    }

    @Test("An assignment callback from an old incarnation cannot enter replacement native state")
    func staleAssignmentCannotEnterReplacementNativeState() async throws {
        let outbox = EventOutbox()
        let transport = EventSequenceRecordingTransport()
        let binding = await outbox.beginConnectionBinding()
        let staleIncarnation = try #require(await outbox.sessionIncarnation(binding: binding))
        #expect(await outbox.confirmFreshSession(id: "old-session", binding: binding))
        let recorder = await MainActor.run { TextAssignmentRecorder() }
        let gate = AssignmentSuspensionGate()
        await outbox.setNativeTextAssignmentWillHopForTesting {
            await gate.holdAssignment()
        }
        let editSeq1 = try #require(EditSeq(1))
        let staleQueue = Task {
            try await outbox.queueTextEdit(
                nodeId: NodeId(45),
                text: "stale",
                editSeq: editSeq1,
                observedRevision: Revision(1),
                binding: binding,
                sessionIncarnation: staleIncarnation,
                via: transport,
                onAssigned: { event in recorder.note(event) }
            )
        }
        await gate.waitUntilEntered()
        await outbox.setNativeTextAssignmentWillHopForTesting(nil)

        #expect(await outbox.applyReplacementFrontier(
            id: "replacement-session",
            lastProcessedEventSeq: 0,
            binding: binding
        ))
        #expect(await outbox.confirmFreshSession(id: "replacement-session", binding: binding))
        let currentIncarnation = try #require(await outbox.sessionIncarnation(binding: binding))
        let currentQueue = Task {
            try await outbox.queueTextEdit(
                nodeId: NodeId(45),
                text: "current",
                editSeq: editSeq1,
                observedRevision: Revision(1),
                binding: binding,
                sessionIncarnation: currentIncarnation,
                via: transport,
                onAssigned: { event in recorder.note(event) }
            )
        }

        await gate.releaseAssignment()
        _ = try? await staleQueue.value
        let current = try #require(try await currentQueue.value)
        #expect(await MainActor.run { recorder.events.map(\.eventId) } == [current.eventId])
        #expect(await outbox.assignedTextEditDescriptors().map(\.eventId) == [current.eventId])
        await transport.close()
    }

    @Test("An oversized text edit preserves its draft and does not burn event_seq")
    func oversizedTextEditAllocationIsTransactional() async throws {
        let outbox = EventOutbox()
        let transport = EventSequenceRecordingTransport()
        let binding = await outbox.beginConnectionBinding()
        let incarnation = try #require(await outbox.sessionIncarnation(binding: binding))
        #expect(await outbox.confirmFreshSession(id: "same-session", binding: binding))
        let editSeq1 = try #require(EditSeq(1))

        do {
            _ = try await outbox.queueTextEdit(
                nodeId: NodeId(46),
                text: String(repeating: "x", count: defaultMaxFrameSize),
                editSeq: editSeq1,
                observedRevision: Revision(1),
                binding: binding,
                sessionIncarnation: incarnation,
                via: transport
            )
            Issue.record("Oversized TEXT_EDIT unexpectedly framed")
        } catch let error as SRUIFramingError {
            guard case .frameSizeLimitExceeded = error else {
                Issue.record("Unexpected framing error: \(error)")
                return
            }
        }

        #expect(await outbox.eventSeq == 0)
        #expect(await outbox.lastAckedEventSeq == 0)
        #expect(await outbox.pendingCount == 0)
        #expect(await outbox.unsentTextDraftCount == 1)

        let editSeq2 = try #require(EditSeq(2))
        let next = try #require(try await outbox.queueTextEdit(
            nodeId: NodeId(46),
            text: "fits",
            editSeq: editSeq2,
            observedRevision: Revision(1),
            binding: binding,
            sessionIncarnation: incarnation,
            via: transport
        ))
        #expect(next.eventSeq == 1)
        #expect(await outbox.eventSeq == 1)
        #expect(await outbox.pendingCount == 1)
        #expect(await outbox.unsentTextDraftCount == 0)
        #expect(await transport.sentEventSequences() == [1])
        await transport.close()
    }

    @Test("An old-incarnation adapter invalidation cannot clear a replacement draft")
    func staleIncarnationCannotInvalidateReplacementDraft() async throws {
        let outbox = EventOutbox()
        let transport = EventSequenceRecordingTransport()
        let binding = await outbox.beginConnectionBinding()
        let staleIncarnation = try #require(await outbox.sessionIncarnation(binding: binding))
        #expect(await outbox.confirmFreshSession(id: "old-session", binding: binding))
        let editSeq1 = try #require(EditSeq(1))
        _ = try #require(try await outbox.queueTextEdit(
            nodeId: NodeId(43),
            text: "old-assigned",
            editSeq: editSeq1,
            observedRevision: Revision(1),
            binding: binding,
            sessionIncarnation: staleIncarnation,
            via: transport
        ))

        #expect(await outbox.applyReplacementFrontier(
            id: "replacement-session",
            lastProcessedEventSeq: 0,
            binding: binding
        ))
        #expect(await outbox.confirmFreshSession(id: "replacement-session", binding: binding))
        let currentIncarnation = try #require(await outbox.sessionIncarnation(binding: binding))
        #expect(currentIncarnation != staleIncarnation)

        _ = try #require(try await outbox.queueTextEdit(
            nodeId: NodeId(43),
            text: "replacement-assigned",
            editSeq: editSeq1,
            observedRevision: Revision(1),
            binding: binding,
            sessionIncarnation: currentIncarnation,
            via: transport,
            laneEpoch: 1
        ))
        let editSeq2 = try #require(EditSeq(2))
        let successor = try await outbox.queueTextEdit(
            nodeId: NodeId(43),
            text: "replacement-draft",
            editSeq: editSeq2,
            observedRevision: Revision(1),
            binding: binding,
            sessionIncarnation: currentIncarnation,
            via: transport,
            laneEpoch: 2
        )
        #expect(successor == nil)

        let invalidated = await outbox.invalidateTextDraft(
            nodeId: NodeId(43),
            laneEpoch: 2,
            binding: binding,
            sessionIncarnation: staleIncarnation
        )
        #expect(!invalidated)
        #expect(await outbox.unsentTextDraftCount == 1)
        await transport.close()
    }

    @Test("A rejected acknowledgement from an old incarnation cannot resolve replacement text")
    func staleRejectedAcknowledgementCannotResolveReplacementText() async throws {
        let outbox = EventOutbox()
        let transport = EventSequenceRecordingTransport()
        let binding = await outbox.beginConnectionBinding()
        let staleIncarnation = try #require(await outbox.sessionIncarnation(binding: binding))
        #expect(await outbox.confirmFreshSession(id: "old-session", binding: binding))
        let editSeq1 = try #require(EditSeq(1))
        let retained = try #require(try await outbox.queueTextEdit(
            nodeId: NodeId(44),
            text: "rejected",
            editSeq: editSeq1,
            observedRevision: Revision(1),
            binding: binding,
            sessionIncarnation: staleIncarnation,
            via: transport
        ))
        let settlement = await outbox.settleAcknowledgement(
            binding: binding,
            sessionIncarnation: staleIncarnation,
            clientInstanceId: outbox.clientInstanceId,
            eventId: retained.eventId,
            throughSeq: retained.eventSeq,
            sessionId: "old-session",
            revisionAfterEffect: 5,
            textEditRejected: true
        )
        #expect(settlement.bound)

        let gate = AssignmentSuspensionGate()
        await outbox.setNativeTextLifecycleWillHopForTesting {
            await gate.holdAssignment()
        }
        let recorder = await MainActor.run { TextAssignmentRecorder() }
        let staleResolution = Task {
            await outbox.releaseTextAcknowledgements(
                through: 5,
                binding: binding,
                sessionIncarnation: staleIncarnation,
                onResolved: { barriers in
                    recorder.noteAcknowledgements(barriers)
                    return []
                }
            )
        }
        await gate.waitUntilEntered()
        await outbox.setNativeTextLifecycleWillHopForTesting(nil)

        #expect(await outbox.applyReplacementFrontier(
            id: "replacement-session",
            lastProcessedEventSeq: 0,
            binding: binding
        ))
        #expect(await outbox.confirmFreshSession(id: "replacement-session", binding: binding))
        let currentIncarnation = try #require(await outbox.sessionIncarnation(binding: binding))
        let current = try #require(try await outbox.queueTextEdit(
            nodeId: NodeId(44),
            text: "current",
            editSeq: editSeq1,
            observedRevision: Revision(1),
            binding: binding,
            sessionIncarnation: currentIncarnation,
            via: transport
        ))

        await gate.releaseAssignment()
        let stillOwned = await staleResolution.value
        #expect(!stillOwned)
        #expect(await MainActor.run { recorder.acknowledgementCalls.isEmpty })
        #expect(await outbox.assignedTextEditDescriptors().map(\.eventId) == [current.eventId])
        await transport.close()
    }

    @Test("Rebinding waits for an active correction render before changing invalidation ownership")
    func rebindRetainsCorrectionPublishedInsideActiveRender() async throws {
        let outbox = EventOutbox()
        let transport = EventSequenceRecordingTransport()
        let binding = await outbox.beginConnectionBinding()
        let incarnation = try #require(await outbox.sessionIncarnation(binding: binding))
        #expect(await outbox.confirmFreshSession(id: "same-session", binding: binding))

        let editSeq1 = try #require(EditSeq(1))
        _ = try #require(try await outbox.queueTextEdit(
            nodeId: NodeId(47),
            text: "assigned",
            editSeq: editSeq1,
            observedRevision: Revision(1),
            binding: binding,
            sessionIncarnation: incarnation,
            via: transport,
            laneEpoch: 1
        ))
        let editSeq2 = try #require(EditSeq(2))
        let successor = try await outbox.queueTextEdit(
            nodeId: NodeId(47),
            text: "successor",
            editSeq: editSeq2,
            observedRevision: Revision(1),
            binding: binding,
            sessionIncarnation: incarnation,
            via: transport,
            laneEpoch: 2
        )
        #expect(successor == nil)
        #expect(await outbox.unsentTextDraftCount == 1)

        let commit = try #require(await outbox.commitLiveRender(
            binding: binding,
            sessionIncarnation: incarnation,
            publish: { true },
            committed: { $0 }
        ))
        let renderToken = try #require(commit.renderToken)
        let blocker = MainActorRenderBlocker()
        let oldRender = Task { @MainActor in
            let staged = outbox.resyncRenderFence.performIfActive(renderToken) {
                blocker.block()
                return outbox.stageTextDraftInvalidation(
                    nodeId: NodeId(47),
                    laneEpoch: 2,
                    sessionIncarnation: incarnation
                )
            }
            return staged == true
        }
        await blocker.waitUntilEntered()

        let replacement = Task {
            await outbox.beginConnectionBinding()
        }
        let observedReplacement = try #require(
            await waitForActiveBinding(outbox, differentFrom: binding)
        )

        blocker.releaseRender()
        let invalidationPublished = await oldRender.value
        #expect(invalidationPublished)
        let replacementBinding = await replacement.value
        #expect(replacementBinding == observedReplacement)
        #expect(await outbox.unsentTextDraftCount == 0)
        await transport.close()
    }
}

private func waitForActiveBinding(
    _ outbox: EventOutbox,
    differentFrom binding: EventOutboxConnectionBinding
) async -> EventOutboxConnectionBinding? {
    for _ in 0..<10_000 {
        if let current = await outbox.activeConnectionBindingForTesting,
           current != binding {
            return current
        }
        await Task.yield()
    }
    return nil
}

private func waitForResumeGeneration(
    _ outbox: EventOutbox,
    _ generation: UInt64
) async -> Bool {
    for _ in 0..<10_000 {
        if await outbox.isActiveResumeGeneration(generation) {
            return true
        }
        await Task.yield()
    }
    return false
}

private func waitForTextDraftWaiter(_ outbox: EventOutbox) async -> Bool {
    for _ in 0..<10_000 {
        if await outbox.textDraftWaiterCountForTesting > 0 {
            return true
        }
        await Task.yield()
    }
    return false
}

private final class MainActorRenderBlocker: @unchecked Sendable {
    private let entered = DispatchSemaphore(value: 0)
    private let release = DispatchSemaphore(value: 0)

    @MainActor
    func block() {
        entered.signal()
        release.wait()
    }

    func waitUntilEntered() async {
        await Task.detached { [self] in
            waitForEntry()
        }.value
    }

    private func waitForEntry() {
        entered.wait()
    }

    func releaseRender() {
        release.signal()
    }
}

private actor AssignmentSuspensionGate {
    private var entered = false
    private var released = false
    private var enteredWaiters: [CheckedContinuation<Void, Never>] = []
    private var releaseWaiters: [CheckedContinuation<Void, Never>] = []

    func holdAssignment() async {
        let waiters = enteredWaiters
        enteredWaiters.removeAll(keepingCapacity: true)
        for waiter in waiters {
            waiter.resume()
        }
        guard !released else { return }
        await withCheckedContinuation { continuation in
            if released {
                continuation.resume()
            } else {
                releaseWaiters.append(continuation)
            }
        }
    }

    func waitUntilEntered() async {
        guard !entered else { return }
        await withCheckedContinuation { continuation in
            if entered {
                continuation.resume()
            } else {
                enteredWaiters.append(continuation)
            }
        }
    }

    func releaseAssignment() {
        released = true
        let waiters = releaseWaiters
        releaseWaiters.removeAll(keepingCapacity: true)
        for waiter in waiters {
            waiter.resume()
        }
    }
}

@MainActor
private final class TextAssignmentRecorder {
    private(set) var events: [Event] = []
    private(set) var cancellationCalls: [[PendingTextEditDescriptor]] = []
    private(set) var acknowledgementCalls: [[TextEditAcknowledgementBarrier]] = []
    private(set) var resetCount = 0

    func note(_ event: Event) {
        events.append(event)
    }

    func noteCancellation(_ descriptors: [PendingTextEditDescriptor]) {
        cancellationCalls.append(descriptors)
    }

    func noteAcknowledgements(_ barriers: [TextEditAcknowledgementBarrier]) {
        acknowledgementCalls.append(barriers)
    }

    func noteReset() {
        resetCount += 1
    }
}

private actor FirstSendBlockingTransport: Transport {
    nonisolated let stream = AsyncThrowingStream<Data, Error> { continuation in
        continuation.finish()
    }
    private let gate: AssignmentSuspensionGate
    private var sendCount = 0

    init(gate: AssignmentSuspensionGate) {
        self.gate = gate
    }

    func send(data _: Data, logicalClass _: LogicalChannelClass) async throws {
        sendCount += 1
        if sendCount == 1 {
            await gate.holdAssignment()
        }
    }

    nonisolated func receiveStream() -> AsyncThrowingStream<Data, Error> {
        stream
    }

    func close() async {}
}

private actor EventSequenceRecordingTransport: Transport {
    nonisolated let stream = AsyncThrowingStream<Data, Error> { continuation in
        continuation.finish()
    }
    private var eventSequences: [UInt64] = []
    private var events: [Event] = []

    func send(data: Data, logicalClass _: LogicalChannelClass) async throws {
        let message = try decodeFramedMessage(from: data)
        guard case .event(let event) = message.msg else { return }
        eventSequences.append(event.eventSeq)
        events.append(try ProtocolDecoder().validateAndConvertEvent(wire: event))
    }

    nonisolated func receiveStream() -> AsyncThrowingStream<Data, Error> {
        stream
    }

    func close() async {}

    func sentEventSequences() -> [UInt64] {
        eventSequences
    }

    func sentEvents() -> [Event] {
        events
    }
}
