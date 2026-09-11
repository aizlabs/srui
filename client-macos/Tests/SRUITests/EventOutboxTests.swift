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
import Text
@testable import Session
import TransportSSH

@Suite("EventOutbox Tests")
struct EventOutboxTests {

    @Test("Pending text-edit wire identities use the protocol-wide event-ID bound")
    func pendingTextEditWireIdentityBound() {
        var reference = SRUIPendingTextEditRef()
        reference.eventID = Data(repeating: 0x41, count: maxEventIDBytes + 1)
        reference.eventSeq = 1
        reference.nodeID = 2
        reference.editSeq = 1
        #expect(PendingTextEditDescriptor(wire: reference) == nil)

        reference.eventID = Data("bounded".utf8)
        #expect(PendingTextEditDescriptor(wire: reference)?.eventId == EventId(string: "bounded"))
    }

    @Test("EventOutbox allocates monotonically increasing sequence numbers")
    func monotonicSequenceNumbers() async throws {
        let (client, server) = await PipeTransport.createPair()
        let outbox = EventOutbox()
        let binding = await activeBinding(for: outbox)

        // Sending is the whole allocation surface: no caller can mint a sequence without also
        // retaining and transmitting it, so allocation and retention cannot diverge (§18.2).
        let seq1 = try await outbox.sendActivate(nodeId: NodeId(1), observedRevision: Revision(1), binding: binding, via: client).eventSeq
        let seq2 = try await outbox.sendActivate(nodeId: NodeId(2), observedRevision: Revision(1), binding: binding, via: client).eventSeq
        let seq3 = try await outbox.sendActivate(nodeId: NodeId(3), observedRevision: Revision(1), binding: binding, via: client).eventSeq

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
        let binding = await activeBinding(for: outbox)

        let id1 = try await outbox.sendActivate(nodeId: NodeId(1), observedRevision: Revision(1), binding: binding, via: client).eventId
        let id2 = try await outbox.sendActivate(nodeId: NodeId(1), observedRevision: Revision(1), binding: binding, via: client).eventId

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
        let binding = await activeBinding(for: outbox)

        let nodeId = NodeId(183)
        let observedRevision = Revision(104)

        let event = try await outbox.sendActivate(
            nodeId: nodeId,
            observedRevision: observedRevision,
            binding: binding,
            via: client
        )

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
        let binding = await activeBinding(for: outbox)

        let nodeId = NodeId(200)
        let observedRevision = Revision(50)
        let value = Value.bool(true)

        let event = try await outbox.sendValueChanged(
            nodeId: nodeId,
            observedRevision: observedRevision,
            value: value,
            binding: binding,
            via: client
        )

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
        let binding = await activeBinding(for: outbox)

        let nodeId = NodeId(300)
        let observedRevision = Revision(75)
        let itemId = ItemId(999)

        let event = try await outbox.sendSelectionChanged(
            nodeId: nodeId,
            observedRevision: observedRevision,
            itemId: itemId,
            binding: binding,
            via: client
        )

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
        let binding = await activeBinding(for: outbox)

        let ev1 = try await outbox.sendActivate(nodeId: NodeId(1), observedRevision: Revision(1), binding: binding, via: client)
        let ev2 = try await outbox.sendValueChanged(nodeId: NodeId(2), observedRevision: Revision(1), value: .bool(true), binding: binding, via: client)
        let ev3 = try await outbox.sendSelectionChanged(nodeId: NodeId(3), observedRevision: Revision(1), itemId: ItemId(42), binding: binding, via: client)

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
        let binding = await activeBinding(for: outbox)

        // 1. Fill capacity (2 events)
        _ = try await outbox.sendActivate(nodeId: NodeId(1), observedRevision: Revision(1), binding: binding, via: client)
        _ = try await outbox.sendValueChanged(nodeId: NodeId(2), observedRevision: Revision(1), value: .bool(true), binding: binding, via: client)

        // 3rd event should throw sequenceWindowExhausted
        await #expect(throws: EventOutboxError.sequenceWindowExhausted(limit: 2)) {
            try await outbox.sendSelectionChanged(nodeId: NodeId(3), observedRevision: Revision(1), itemId: ItemId(10), binding: binding, via: client)
        }

        // 2. Suspended outbox throws resumeNotConfirmed
        _ = await outbox.suspendNewEvents(binding: binding)
        await #expect(throws: EventOutboxError.resumeNotConfirmed) {
            try await outbox.sendActivate(nodeId: NodeId(1), observedRevision: Revision(1), binding: binding, via: client)
        }
        await #expect(throws: EventOutboxError.resumeNotConfirmed) {
            try await outbox.sendValueChanged(nodeId: NodeId(2), observedRevision: Revision(1), value: .bool(true), binding: binding, via: client)
        }
        await #expect(throws: EventOutboxError.resumeNotConfirmed) {
            try await outbox.sendSelectionChanged(nodeId: NodeId(3), observedRevision: Revision(1), itemId: ItemId(10), binding: binding, via: client)
        }

        await client.close()
        await server.close()
    }

    @Test("EventOutbox sendActivate transmits framed event over Transport")
    func sendActivateOverTransport() async throws {
        let (client, server) = await PipeTransport.createPair()
        let outbox = EventOutbox()
        let binding = await activeBinding(for: outbox)

        let serverStream = server.receiveStream()

        let sendTask = Task {
            try await outbox.sendActivate(
                nodeId: NodeId(7),
                observedRevision: Revision(10),
                binding: binding,
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
        let initialBinding = await activeBinding(for: outbox)

        let ev1 = try await outbox.sendActivate(nodeId: NodeId(1), observedRevision: Revision(1), binding: initialBinding, via: client)
        let ev2 = try await outbox.sendValueChanged(nodeId: NodeId(2), observedRevision: Revision(1), value: .bool(true), binding: initialBinding, via: client)
        let ev3 = try await outbox.sendSelectionChanged(nodeId: NodeId(3), observedRevision: Revision(1), itemId: ItemId(42), binding: initialBinding, via: client)

        #expect(await outbox.pendingCount == 3)

        // Settle ack through seq 1
        let binding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "session-live", binding: binding))
        _ = await outbox.settleAcknowledgement(
            binding: binding,
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

        let accepted = try await resumeSameSession(
            outbox,
            id: "session-123",
            lastProcessedEventSeq: 1,
            generation: generation,
            binding: resumedBinding,
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
        let ev4 = try await outbox.sendActivate(nodeId: NodeId(4), observedRevision: Revision(2), binding: resumedBinding, via: client2)
        #expect(ev4.eventSeq == 4)

        await outbox.stopResumeWork(generation: generation)
        await client2.close()
        await server2.close()
    }

    @Test("Same-session RESUME_OK clears the generation latch for a later fresh HELLO")
    func resumeOkClearsGenerationLatch() async throws {
        let (client, server) = await PipeTransport.createPair()
        let outbox = EventOutbox()
        let initialBinding = await activeBinding(for: outbox)
        _ = try await outbox.sendActivate(
            nodeId: NodeId(1),
            observedRevision: Revision(1),
            binding: initialBinding,
            via: client
        )

        let binding = await outbox.beginConnectionBinding()
        let generation = try #require(await outbox.beginResumeAttempt(binding: binding))
        let accepted = try await resumeSameSession(
            outbox,
            id: "session-123",
            lastProcessedEventSeq: 0,
            generation: generation,
            binding: binding,
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
        let initialBinding = await activeBinding(for: outbox)

        _ = try await outbox.sendActivate(nodeId: NodeId(1), observedRevision: Revision(1), binding: initialBinding, via: client)
        _ = try await outbox.sendValueChanged(nodeId: NodeId(2), observedRevision: Revision(1), value: .bool(false), binding: initialBinding, via: client)
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

        let freshEvent = try await outbox.sendActivate(nodeId: NodeId(10), observedRevision: Revision(1), binding: binding, via: client)
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

        let first = try await outbox.sendActivate(
            nodeId: NodeId(1), observedRevision: Revision(1), binding: binding, via: client
        )
        let second = try await outbox.sendActivate(
            nodeId: NodeId(2), observedRevision: Revision(1), binding: binding, via: client
        )

        await #expect(throws: EventOutboxError.sequenceWindowExhausted(limit: 2)) {
            try await outbox.sendActivate(
                nodeId: NodeId(3), observedRevision: Revision(1), binding: binding, via: client
            )
        }
        #expect(await outbox.eventSeq == 2)
        #expect(await outbox.pendingCount == 2)
        #expect(await outbox.lastAckedEventSeq == 0)

        #expect(await outbox.settleAcknowledgement(
            binding: binding,
            clientInstanceId: outbox.clientInstanceId,
            eventId: first.eventId,
            throughSeq: 0,
            sessionId: "session-window"
        ).bound)
        #expect(await outbox.settleAcknowledgement(
            binding: binding,
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

        let first = try await outbox.sendActivate(
            nodeId: NodeId(1), observedRevision: Revision(1), binding: binding, via: client
        )
        let second = try await outbox.sendActivate(
            nodeId: NodeId(2), observedRevision: Revision(1), binding: binding, via: client
        )

        #expect(await outbox.settleAcknowledgement(
            binding: binding,
            clientInstanceId: outbox.clientInstanceId,
            eventId: second.eventId,
            throughSeq: 0,
            sessionId: "session-gap"
        ).bound)
        #expect(await outbox.pendingCount == 1)
        #expect(await outbox.lastAckedEventSeq == 0)

        await #expect(throws: EventOutboxError.sequenceWindowExhausted(limit: 2)) {
            try await outbox.sendActivate(
                nodeId: NodeId(3), observedRevision: Revision(1), binding: binding, via: client
            )
        }
        #expect(await outbox.eventSeq == 2)

        #expect(await outbox.settleAcknowledgement(
            binding: binding,
            clientInstanceId: outbox.clientInstanceId,
            eventId: first.eventId,
            throughSeq: 0,
            sessionId: "session-gap"
        ).bound)
        #expect(await outbox.lastAckedEventSeq == 2)

        let third = try await outbox.sendActivate(
            nodeId: NodeId(3), observedRevision: Revision(1), binding: binding, via: client
        )
        #expect(third.eventSeq == 3)
        #expect(await outbox.pendingCount == 1)

        await client.close()
        await server.close()
    }
    @Test("An unresolved TEXT_EDIT assignment blocks later event allocation and transmission")
    func textEditAssignmentPreservesAllocationOrder() async throws {
        let transport = EventSequenceRecordingTransport()
        let outbox = EventOutbox()
        let binding = await activeBinding(for: outbox)
        let editSeq = try #require(EditSeq(1))
        let prepared = try #require(try await outbox.prepareTextEdit(
            nodeId: NodeId(12),
            text: "typed",
            editSeq: editSeq,
            observedRevision: Revision(1),
            binding: binding,
            via: transport
        ))

        let activateTask = Task {
            try await outbox.sendActivate(
                nodeId: NodeId(13),
                observedRevision: Revision(1),
                binding: binding,
                via: transport
            )
        }
        for _ in 0..<20 { await Task.yield() }

        #expect(await outbox.eventSeq == 1)
        #expect(await outbox.pendingCount == 1)
        #expect(await transport.sentEventSequences().isEmpty)
        #expect(await outbox.authorizePreparedTextEdit(prepared))
        let text = try #require(try await outbox.releasePreparedTextEdit(prepared))
        let activate = try await activateTask.value

        #expect(text.eventSeq == 1)
        #expect(activate.eventSeq == 2)
        #expect(await outbox.pendingCount == 2)
        #expect(await transport.sentEventSequences() == [1, 2])
        await transport.close()
    }

    @Test("Rejecting an unassigned prepared edit rolls back without a sequence hole")
    func rejectedPreparedTextEditReusesSequence() async throws {
        let transport = EventSequenceRecordingTransport()
        let outbox = EventOutbox()
        let binding = await activeBinding(for: outbox)
        let editSeq = try #require(EditSeq(1))
        let prepared = try #require(try await outbox.prepareTextEdit(
            nodeId: NodeId(12),
            text: "stale",
            editSeq: editSeq,
            observedRevision: Revision(1),
            binding: binding,
            via: transport
        ))

        #expect(await outbox.rejectPreparedTextEdit(prepared))
        #expect(await outbox.eventSeq == 0)
        #expect(await outbox.pendingCount == 0)
        #expect(await outbox.assignedTextEditDescriptors().isEmpty)
        #expect(await transport.sentEventSequences().isEmpty)

        let action = try await outbox.sendActivate(
            nodeId: NodeId(13), observedRevision: Revision(1), binding: binding, via: transport
        )
        #expect(action.eventSeq == 1)
        #expect(await outbox.pendingCount == 1)
        #expect(await transport.sentEventSequences() == [1])
        await transport.close()
    }

    @Test("A correction before authorization rolls back the prepared TEXT_EDIT")
    func correctionRevokesUnauthorizedPreparedTextEdit() async throws {
        let transport = EventSequenceRecordingTransport()
        let outbox = EventOutbox()
        let binding = await activeBinding(for: outbox)
        let textSession = await MainActor.run {
            TextEditingSession(debounceNanoseconds: 0)
        }
        let edit = try await MainActor.run {
            let editSeq = try #require(EditSeq(1))
            textSession.noteLocalValue(
                "stale",
                nodeID: NodeId(12),
                composing: false,
                flushImmediately: true
            )
            #expect(textSession.recordObservedRevision(
                nodeID: NodeId(12),
                text: "stale",
                editSeq: editSeq,
                laneEpoch: 1,
                observedRevision: Revision(1)
            ))
            return try #require(textSession.claimNextUnassignedEdit())
        }
        let prepared = try #require(try await outbox.prepareTextEdit(
            nodeId: edit.nodeId,
            text: edit.text,
            editSeq: edit.editSeq,
            observedRevision: edit.observedRevision,
            binding: binding,
            via: transport
        ))

        #expect(await MainActor.run {
            textSession.onAssignedIdentityRevoked = { eventId in
                outbox.revokeUnauthorizedPreparedTextEdit(eventId: eventId)
            }
            return textSession.noteAssigned(prepared.event, matching: edit)
        })
        #expect(await MainActor.run {
            textSession.applyPublishedValue(nodeID: NodeId(12), published: "corrected")
        } == .apply)
        #expect(await outbox.authorizePreparedTextEdit(prepared) == false)
        #expect(await outbox.eventSeq == 0)
        #expect(await outbox.pendingCount == 0)
        #expect(await transport.sentEventSequences().isEmpty)

        let action = try await outbox.sendActivate(
            nodeId: NodeId(13), observedRevision: Revision(1), binding: binding, via: transport
        )
        #expect(action.eventSeq == 1)
        #expect(await transport.sentEventSequences() == [1])
        await transport.close()
    }

    @Test("Authorization that already opened the send gate is not rolled back by a later revoke")
    func revokeAfterAuthorizationDoesNotPreventSend() async throws {
        let transport = EventSequenceRecordingTransport()
        let outbox = EventOutbox()
        let binding = await activeBinding(for: outbox)
        let editSeq = try #require(EditSeq(1))
        let prepared = try #require(try await outbox.prepareTextEdit(
            nodeId: NodeId(12),
            text: "typed",
            editSeq: editSeq,
            observedRevision: Revision(1),
            binding: binding,
            via: transport
        ))
        #expect(await outbox.authorizePreparedTextEdit(prepared))
        outbox.revokeUnauthorizedPreparedTextEdit(eventId: prepared.event.eventId)
        let sent = try #require(try await outbox.releasePreparedTextEdit(prepared))
        #expect(sent == prepared.event)
        #expect(await transport.sentEventSequences() == [1])
        #expect(await outbox.retainedPreparedTextEditRevocationCountForTesting == 0)
        await transport.close()
    }

    @Test("A prepared assignment survives teardown before its send gate opens")
    func nativeAssignmentSurvivesTeardownBeforeRelease() async throws {
        let transport = EventSequenceRecordingTransport()
        let outbox = EventOutbox()
        let binding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "session-authorize-race", binding: binding))
        let textSession = await MainActor.run {
            TextEditingSession(debounceNanoseconds: 0)
        }
        let edit = try await MainActor.run {
            let editSeq = try #require(EditSeq(1))
            textSession.noteLocalValue(
                "typed",
                nodeID: NodeId(12),
                composing: false,
                flushImmediately: true
            )
            #expect(textSession.recordObservedRevision(
                nodeID: NodeId(12),
                text: "typed",
                editSeq: editSeq,
                laneEpoch: 1,
                observedRevision: Revision(1)
            ))
            return try #require(textSession.claimNextUnassignedEdit())
        }
        let prepared = try #require(try await outbox.prepareTextEdit(
            nodeId: edit.nodeId,
            text: edit.text,
            editSeq: edit.editSeq,
            observedRevision: edit.observedRevision,
            binding: binding,
            via: transport
        ))

        #expect(await MainActor.run {
            textSession.noteAssigned(prepared.event, matching: edit)
        })
        #expect(await outbox.suspendForTeardown(binding: binding))
        #expect(await outbox.authorizePreparedTextEdit(prepared))
        let retained = try #require(try await outbox.releasePreparedTextEdit(prepared))
        #expect(retained == prepared.event)
        #expect(await outbox.assignedTextEditEvents() == [prepared.event])
        #expect(await transport.sentEventSequences().isEmpty)

        let resumedBinding = await outbox.beginConnectionBinding()
        let generation = try #require(await outbox.beginResumeAttempt(binding: resumedBinding))
        let preparation = try #require(try await outbox.prepareSameSessionResume(
            id: "session-authorize-race",
            lastProcessedEventSeq: 0,
            generation: generation,
            binding: resumedBinding
        ))
        #expect(preparation.assignedTextEdits == [prepared.event])
        #expect(try await outbox.completeSameSessionResume(
            preparation,
            via: transport,
            enableNewEventsAfterReplay: true
        ))
        #expect(await transport.sentEventSequences() == [1])

        let action = try await outbox.sendActivate(
            nodeId: NodeId(13),
            observedRevision: Revision(1),
            binding: resumedBinding,
            via: transport
        )
        #expect(action.eventSeq == 2)
        #expect(await transport.sentEventSequences() == [1, 2])
        await transport.close()
    }

    @Test("Rejecting a settled prepared edit always releases the outbound FIFO")
    func rejectSettledPreparedTextEditReleasesSendGate() async throws {
        let transport = EventSequenceRecordingTransport()
        let outbox = EventOutbox()
        let binding = await activeBinding(for: outbox)
        let editSeq = try #require(EditSeq(1))
        let prepared = try #require(try await outbox.prepareTextEdit(
            nodeId: NodeId(12),
            text: "typed",
            editSeq: editSeq,
            observedRevision: Revision(1),
            binding: binding,
            via: transport
        ))
        let descriptor = PendingTextEditDescriptor(
            eventId: prepared.event.eventId,
            eventSeq: prepared.event.eventSeq,
            nodeId: prepared.event.nodeId,
            editSeq: editSeq
        )
        try await outbox.cancelAssignedTextEdits(
            confirming: [descriptor],
            requireExactMatch: true
        )

        #expect(await outbox.rejectPreparedTextEdit(prepared) == false)
        #expect(await outbox.preparedTextEditSendCountForTesting == 0)
        #expect(await outbox.retainedPreparedTextEditRevocationCountForTesting == 0)

        let action = try await outbox.sendActivate(
            nodeId: NodeId(13),
            observedRevision: Revision(1),
            binding: binding,
            via: transport
        )
        #expect(action.eventSeq == 2)
        #expect(await transport.sentEventSequences() == [2])
        await transport.close()
    }

    @Test("A deliver-then-throw send remains assigned and replayable")
    func deliverThenThrowTextEditRemainsReplayable() async throws {
        let failingTransport = DeliverThenThrowTransport()
        let outbox = EventOutbox()
        let binding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "session-deliver-throw", binding: binding))
        let editSeq = try #require(EditSeq(1))
        let prepared = try #require(try await outbox.prepareTextEdit(
            nodeId: NodeId(12),
            text: "typed",
            editSeq: editSeq,
            observedRevision: Revision(1),
            binding: binding,
            via: failingTransport
        ))
        #expect(await outbox.authorizePreparedTextEdit(prepared))
        await #expect(throws: DeliverThenThrowError.delivered) {
            try await outbox.releasePreparedTextEdit(prepared)
        }
        #expect(await failingTransport.sentEvents() == [prepared.event])
        #expect(await outbox.assignedTextEditEvents() == [prepared.event])
        #expect(await outbox.pendingCount == 1)

        let replayTransport = EventSequenceRecordingTransport()
        let resumedBinding = await outbox.beginConnectionBinding()
        let generation = try #require(await outbox.beginResumeAttempt(binding: resumedBinding))
        let preparation = try #require(try await outbox.prepareSameSessionResume(
            id: "session-deliver-throw",
            lastProcessedEventSeq: 0,
            generation: generation,
            binding: resumedBinding
        ))
        #expect(preparation.assignedTextEdits == [prepared.event])
        #expect(try await outbox.completeSameSessionResume(
            preparation,
            via: replayTransport,
            enableNewEventsAfterReplay: true
        ))
        #expect(await replayTransport.sentEvents() == [prepared.event])
        #expect(await outbox.pendingCount == 1)
        await failingTransport.close()
        await replayTransport.close()
    }

    @Test("TEXT_EDIT serializes edit_seq and the whole-value TEXT argument")
    func textEditEventSerialization() async throws {
        let clientInstanceId = ClientInstanceId(string: "client-test-text")
        let outbox = EventOutbox(clientInstanceId: clientInstanceId)
        let (client, server) = await PipeTransport.createPair()
        let binding = await activeBinding(for: outbox)

        let nodeId = NodeId(12)
        let seq = try #require(EditSeq(7))
        let event = try await sendPreparedTextEdit(
            outbox,
            nodeId: nodeId,
            text: "whole-value",
            editSeq: seq,
            observedRevision: Revision(4),
            binding: binding,
            via: client
        )
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
    @Test("Skipped edit_seq values preserve contiguous event_seq allocation")
    func coalescedTextEditsSkipEditSeqButKeepEventSeqContiguous() async throws {
        let (client, server) = await PipeTransport.createPair()
        let outbox = EventOutbox()
        let binding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "session-coalesce", binding: binding))
        let incarnation = try #require(await outbox.sessionIncarnation(binding: binding))

        let first = try await sendPreparedTextEdit(
            outbox,
            nodeId: NodeId(12),
            text: "a",
            editSeq: try #require(EditSeq(1)),
            observedRevision: Revision(1),
            binding: binding,
            sessionIncarnation: incarnation,
            via: client
        )
        #expect(first.eventSeq == 1)

        #expect(await outbox.settleAcknowledgement(
            binding: binding,
            sessionIncarnation: incarnation,
            clientInstanceId: outbox.clientInstanceId,
            eventId: first.eventId,
            throughSeq: first.eventSeq,
            sessionId: "session-coalesce",
            revisionAfterEffect: 2
        ).bound)
        #expect(await outbox.releaseTextAcknowledgements(
            through: 2,
            binding: binding,
            sessionIncarnation: incarnation,
            onResolved: { _ in }
        ))

        let coalesced = try await sendPreparedTextEdit(
            outbox,
            nodeId: NodeId(12),
            text: "abc",
            editSeq: try #require(EditSeq(3)),
            observedRevision: Revision(2),
            binding: binding,
            sessionIncarnation: incarnation,
            via: client
        )
        #expect(coalesced.eventSeq == 2)
        #expect(coalesced.editSeq?.rawValue == 3)
        #expect(await outbox.eventSeq == 2)
        #expect(await outbox.assignedTextEditDescriptors().map(\.eventSeq) == [2])
        await client.close()
        await server.close()
    }
    @Test("A cumulative ack settles every covered text lane with conservative barriers")
    func cumulativeAckSettlesCoveredTextOutcomes() async throws {
        let (client, server) = await PipeTransport.createPair()
        let outbox = EventOutbox()
        let binding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "session-barriers", binding: binding))
        let incarnation = try #require(await outbox.sessionIncarnation(binding: binding))
        let editSeq = try #require(EditSeq(1))
        let recorder = await MainActor.run { TextAssignmentRecorder() }

        let first = try await sendPreparedTextEdit(
            outbox, nodeId: NodeId(12), text: "left", editSeq: editSeq,
            observedRevision: Revision(1), binding: binding,
            sessionIncarnation: incarnation, via: client
        )
        let second = try await sendPreparedTextEdit(
            outbox, nodeId: NodeId(13), text: "right", editSeq: editSeq,
            observedRevision: Revision(1), binding: binding,
            sessionIncarnation: incarnation, via: client
        )

        let settlement = await outbox.settleAcknowledgement(
            binding: binding,
            sessionIncarnation: incarnation,
            clientInstanceId: outbox.clientInstanceId,
            eventId: second.eventId,
            throughSeq: second.eventSeq,
            sessionId: "session-barriers",
            revisionAfterEffect: 5
        )
        #expect(settlement.settledEvents.map(\.eventId) == [first.eventId, second.eventId])
        #expect(await outbox.assignedTextEditDescriptors().isEmpty)
        #expect(await outbox.pendingCount == 0)

        #expect(await outbox.releaseTextAcknowledgements(
            through: 4,
            binding: binding,
            sessionIncarnation: incarnation,
            onResolved: { recorder.noteAcknowledgements($0) }
        ))
        #expect(await MainActor.run { recorder.acknowledgementCalls.isEmpty })

        #expect(await outbox.releaseTextAcknowledgements(
            through: 5,
            binding: binding,
            sessionIncarnation: incarnation,
            onResolved: { recorder.noteAcknowledgements($0) }
        ))
        #expect(await MainActor.run {
            let barriers = recorder.acknowledgementCalls.last ?? []
            let outcomes = Dictionary(uniqueKeysWithValues: barriers.map {
                ($0.eventId, $0.rejected)
            })
            return outcomes == [first.eventId: true, second.eventId: false]
        })
        await client.close()
        await server.close()
    }
    @Test("Selective TEXT_EDIT cancellation preserves ordinary pending events")
    func cancelAssignedTextEditsPreservesOrdinaryEvents() async throws {
        let (client, server) = await PipeTransport.createPair()
        let outbox = EventOutbox()
        let binding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "session-cancel", binding: binding))
        let textEvent = try await sendPreparedTextEdit(
            outbox,
            nodeId: NodeId(12),
            text: "typed",
            editSeq: try #require(EditSeq(1)),
            observedRevision: Revision(1),
            binding: binding,
            via: client
        )
        let activate = try await outbox.sendActivate(
            nodeId: NodeId(7), observedRevision: Revision(1), binding: binding, via: client
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
                        editSeq: try #require(textEvent.editSeq)
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
        let assigned = try await sendPreparedTextEdit(
            outbox,
            nodeId: NodeId(12),
            text: "typed",
            editSeq: try #require(EditSeq(1)),
            observedRevision: Revision(1),
            binding: binding,
            via: client
        )
        let discarded = [
            PendingTextEditDescriptor(
                eventId: assigned.eventId,
                eventSeq: assigned.eventSeq,
                nodeId: assigned.nodeId,
                editSeq: try #require(assigned.editSeq)
            ).toWire()
        ]

        let stale = try #require(await outbox.beginResumeAttempt(binding: binding))
        let current = try #require(await outbox.beginResumeAttempt(binding: binding))

        let skipped = try await outbox.cancelAssignedTextEdits(
            confirming: discarded, requireExactMatch: true, onlyIfResumeGeneration: stale
        )
        #expect(!skipped)
        #expect(await outbox.assignedTextEditDescriptors().count == 1)

        let liveSkipped = try await outbox.cancelAssignedTextEdits(
            confirming: discarded, requireExactMatch: true, onlyIfResumeGeneration: nil
        )
        #expect(!liveSkipped)
        #expect(await outbox.assignedTextEditDescriptors().count == 1)

        let canceled = try await outbox.cancelAssignedTextEdits(
            confirming: discarded, requireExactMatch: true, onlyIfResumeGeneration: current
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
        let assigned = try await sendPreparedTextEdit(
            outbox,
            nodeId: NodeId(12),
            text: "typed",
            editSeq: try #require(EditSeq(1)),
            observedRevision: Revision(1),
            binding: binding,
            via: seedClient
        )
        let discarded = [
            PendingTextEditDescriptor(
                eventId: assigned.eventId,
                eventSeq: assigned.eventSeq,
                nodeId: assigned.nodeId,
                editSeq: try #require(assigned.editSeq)
            ).toWire()
        ]

        let stale = try #require(await outbox.beginResumeAttempt(binding: binding))
        let current = try #require(await outbox.beginResumeAttempt(binding: binding))
        let (client, server) = await PipeTransport.createPair()
        let refused = try await resumeSameSession(
            outbox,
            id: "session-bind",
            lastProcessedEventSeq: 0,
            generation: stale,
            binding: binding,
            via: client,
            enableNewEventsAfterReplay: false,
            discardedTextEdits: discarded
        )
        #expect(!refused)
        #expect(await outbox.assignedTextEditDescriptors().count == 1)

        let accepted = try await resumeSameSession(
            outbox,
            id: "session-bind",
            lastProcessedEventSeq: 0,
            generation: current,
            binding: binding,
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
    @Test("Resume discard mismatch is rejected before native drafts are canceled")
    func resumeDiscardMismatchPreservesNativeAndOutboxState() async throws {
        let transport = EventSequenceRecordingTransport()
        let outbox = EventOutbox()
        let binding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "session-mismatch", binding: binding))
        let assigned = try await sendPreparedTextEdit(
            outbox,
            nodeId: NodeId(12),
            text: "typed",
            editSeq: try #require(EditSeq(1)),
            observedRevision: Revision(1),
            binding: binding,
            via: transport
        )
        let generation = try #require(await outbox.beginResumeAttempt(binding: binding))
        let recorder = await MainActor.run { TextAssignmentRecorder() }
        var mismatch = PendingTextEditDescriptor(
            eventId: assigned.eventId,
            eventSeq: assigned.eventSeq,
            nodeId: assigned.nodeId,
            editSeq: try #require(assigned.editSeq)
        ).toWire()
        mismatch.nodeID += 1

        await #expect(throws: EventOutboxError.textEditDiscardMismatch) {
            _ = try await outbox.prepareSameSessionResume(
                id: "session-mismatch",
                lastProcessedEventSeq: 0,
                generation: generation,
                binding: binding,
                discardedTextEdits: [mismatch],
                requireExactTextMatch: true,
                onTextEditsCanceled: { recorder.noteCancellation($0) }
            )
        }
        #expect(await MainActor.run { recorder.cancellationCalls.isEmpty })
        #expect(await outbox.assignedTextEditDescriptors().map(\.eventId) == [assigned.eventId])
        #expect(await outbox.pendingCount == 1)
        await transport.close()
    }

    @Test("Resume frontier settles covered text and never replays an evictable result")
    func resumeFrontierSettlesCoveredTextWithoutReplay() async throws {
        let transport = EventSequenceRecordingTransport()
        let outbox = EventOutbox()
        let binding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "session-outcome", binding: binding))
        let pending = try await sendPreparedTextEdit(
            outbox,
            nodeId: NodeId(12),
            text: "invalid",
            editSeq: try #require(EditSeq(1)),
            observedRevision: Revision(1),
            binding: binding,
            via: transport
        )
        #expect(await transport.sentEventSequences() == [1])

        let generation = try #require(await outbox.beginResumeAttempt(binding: binding))
        let preparation = try #require(try await outbox.prepareSameSessionResume(
            id: "session-outcome",
            lastProcessedEventSeq: pending.eventSeq,
            generation: generation,
            binding: binding
        ))
        #expect(preparation.frontierSettledTextEdits == [pending])
        #expect(preparation.assignedTextEdits.isEmpty)
        #expect(await outbox.lastAckedEventSeq == pending.eventSeq)
        #expect(await outbox.pendingCount == 0)
        #expect(await outbox.assignedTextEditDescriptors().isEmpty)

        #expect(try await outbox.completeSameSessionResume(
            preparation,
            via: transport,
            enableNewEventsAfterReplay: true
        ))
        #expect(await transport.sentEventSequences() == [1])

        let later = try await outbox.sendActivate(
            nodeId: NodeId(13),
            observedRevision: Revision(2),
            binding: binding,
            via: transport
        )
        #expect(later.eventSeq == 2)
        #expect(await transport.sentEventSequences() == [1, 2])
        await transport.close()
    }
    @Test("A reconnect generation makes live same-session resync an atomic no-op")
    func reconnectSupersedesLiveSameSessionResyncAtomically() async throws {
        let (client, server) = await PipeTransport.createPair()
        let outbox = EventOutbox()
        let binding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "session-live-race", binding: binding))
        let edit = try await sendPreparedTextEdit(
            outbox,
            nodeId: NodeId(12),
            text: "pending",
            editSeq: try #require(EditSeq(1)),
            observedRevision: Revision(1),
            binding: binding,
            via: client
        )
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
        let binding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "session-live-pending", binding: binding))
        let edit = try await sendPreparedTextEdit(
            outbox,
            nodeId: NodeId(12),
            text: "pending",
            editSeq: try #require(EditSeq(1)),
            observedRevision: Revision(1),
            binding: binding,
            via: transport
        )
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

        let resumedBinding = await outbox.beginConnectionBinding()
        let generation = try #require(await outbox.beginResumeAttempt(binding: resumedBinding))
        #expect(try await resumeSameSession(
            outbox,
            id: "session-live-pending",
            lastProcessedEventSeq: 0,
            generation: generation,
            binding: resumedBinding,
            via: transport,
            enableNewEventsAfterReplay: true,
            discardedTextEdits: [descriptor.toWire()]
        ))
        #expect(await outbox.pendingCount == 0)
        #expect(await outbox.lastAckedEventSeq == edit.eventSeq)

        let next = try await outbox.sendActivate(
            nodeId: NodeId(13),
            observedRevision: Revision(1),
            binding: resumedBinding,
            via: transport
        )
        #expect(next.eventSeq == 2)
        #expect(await transport.sentEventSequences() == [1, 2])
        await transport.close()
    }
    @Test("Live resync locally discards only text edits covered by the server frontier")
    func liveResyncDiscardsDeliveredTextEditWithoutBreakingSequence() async throws {
        let transport = EventSequenceRecordingTransport()
        let outbox = EventOutbox()
        let binding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "session-live-covered", binding: binding))
        let edit = try await sendPreparedTextEdit(
            outbox,
            nodeId: NodeId(12),
            text: "delivered",
            editSeq: try #require(EditSeq(1)),
            observedRevision: Revision(1),
            binding: binding,
            via: transport
        )
        let decision = try await outbox.applyLiveSameSessionResync(
            lastProcessedEventSeq: edit.eventSeq,
            binding: binding
        )
        guard case .applied(let canceled) = decision else {
            Issue.record("Expected the server frontier to authorize local cancellation")
            return
        }
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
        #expect(await MainActor.run {
            outbox.resyncRenderFence.performIfActive(renderToken) { true }
        } == true)
        #expect(await outbox.applyFullResyncTextBoundary(
            generation: nil,
            binding: binding,
            renderToken: renderToken
        ))
        #expect(await outbox.allowNewEvents(binding: binding))

        let next = try await outbox.sendActivate(
            nodeId: NodeId(13),
            observedRevision: Revision(2),
            binding: binding,
            via: transport
        )
        #expect(next.eventSeq == 2)
        #expect(await transport.sentEventSequences() == [1, 2])
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
        let edit = try await sendPreparedTextEdit(
            outbox,
            nodeId: NodeId(12),
            text: "new binding",
            editSeq: try #require(EditSeq(1)),
            observedRevision: Revision(1),
            binding: currentBinding,
            via: transport
        )

        guard case .superseded = try await outbox.applyLiveSameSessionResync(
            lastProcessedEventSeq: edit.eventSeq,
            binding: staleBinding
        ) else {
            Issue.record("Expected stale live resync to be superseded")
            return
        }
        #expect(await outbox.assignedTextEditDescriptors().map(\.eventId) == [edit.eventId])
        #expect(await outbox.lastAckedEventSeq == 0)
        #expect(await outbox.commitResyncSnapshot(
            generation: nil,
            binding: staleBinding,
            publish: { true },
            committed: { $0 }
        ) == nil)
        #expect(await outbox.applyFullResyncTextBoundary(
            generation: nil,
            binding: staleBinding,
            renderToken: staleRenderToken
        ) == false)
        #expect(await MainActor.run {
            outbox.resyncRenderFence.performIfActive(staleRenderToken) { true }
        } == nil)
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

        #expect(await MainActor.run {
            outbox.resyncRenderFence.performIfActive(renderToken) { true }
        } == nil)
        #expect(await outbox.applyFullResyncTextBoundary(
            generation: staleGeneration,
            binding: binding,
            renderToken: renderToken
        ) == false)
        #expect(await outbox.isActiveResumeGeneration(currentGeneration))
        await outbox.stopResumeWork(generation: currentGeneration)
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

    @Test("The newest lifecycle transition inherits an in-progress render invalidation")
    func newestLifecycleTransitionInheritsBoundary() async throws {
        let outbox = EventOutbox()
        let initialBinding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "same-session", binding: initialBinding))
        #expect(await outbox.suspendNewEvents(binding: initialBinding))
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

        blocker.releaseRender()
        _ = await render.value
        #expect(await firstRebind.value == intermediateBinding)
        #expect(await secondRebind.value == newestBinding)
        #expect(await outbox.confirmFreshSession(id: "same-session", binding: newestBinding))

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

    @Test("A superseded same-session cleanup preserves the newer binding's assigned envelope")
    func supersededSameSessionCleanupPreservesNewDraft() async throws {
        let outbox = EventOutbox()
        let transport = EventSequenceRecordingTransport()
        let staleBinding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "same-session", binding: staleBinding))
        let retained = try await sendPreparedTextEdit(
            outbox,
            nodeId: NodeId(40),
            text: "old",
            editSeq: try #require(EditSeq(1)),
            observedRevision: Revision(1),
            binding: staleBinding,
            via: transport
        )

        let gate = AssignmentSuspensionGate()
        await outbox.setNativeTextLifecycleWillHopForTesting {
            await gate.holdAssignment()
        }
        let recorder = await MainActor.run { TextAssignmentRecorder() }
        let staleCleanup = Task {
            try await outbox.applyLiveSameSessionResync(
                lastProcessedEventSeq: retained.eventSeq,
                binding: staleBinding,
                onTextEditsCanceled: { recorder.noteCancellation($0) }
            )
        }
        await gate.waitUntilEntered()

        let currentBinding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "same-session", binding: currentBinding))
        let current = try await sendPreparedTextEdit(
            outbox,
            nodeId: NodeId(41),
            text: "new",
            editSeq: try #require(EditSeq(1)),
            observedRevision: Revision(2),
            binding: currentBinding,
            via: transport
        )

        await gate.releaseAssignment()
        guard case .superseded = try await staleCleanup.value else {
            Issue.record("Stale same-session cleanup was not superseded")
            return
        }
        #expect(await MainActor.run { recorder.cancellationCalls.isEmpty })
        #expect(await outbox.pendingCount == 2)
        #expect(await outbox.assignedTextEditEvents().map(\.eventId) == [
            retained.eventId, current.eventId
        ])
        await transport.close()
    }
    @Test("A superseded live replacement preserves the newer binding's assigned envelope")
    func supersededLiveReplacementPreservesNewDraft() async throws {
        let outbox = EventOutbox()
        let transport = EventSequenceRecordingTransport()
        let staleBinding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "old-session", binding: staleBinding))
        let retained = try await sendPreparedTextEdit(
            outbox,
            nodeId: NodeId(41),
            text: "old",
            editSeq: try #require(EditSeq(1)),
            observedRevision: Revision(1),
            binding: staleBinding,
            via: transport
        )

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
        let current = try await sendPreparedTextEdit(
            outbox,
            nodeId: NodeId(42),
            text: "new",
            editSeq: try #require(EditSeq(1)),
            observedRevision: Revision(2),
            binding: currentBinding,
            via: transport
        )

        await gate.releaseAssignment()
        #expect(await staleReplacement.value == false)
        #expect(await MainActor.run { recorder.resetCount == 0 })
        #expect(await outbox.eventSeq == 2)
        #expect(await outbox.pendingCount == 2)
        #expect(await outbox.assignedTextEditEvents().map(\.eventId) == [
            retained.eventId, current.eventId
        ])
        await transport.close()
    }
    @Test("A superseded resumed replacement preserves the newer binding's assigned envelope")
    func supersededResumedReplacementPreservesNewDraft() async throws {
        let outbox = EventOutbox()
        let transport = EventSequenceRecordingTransport()
        let staleBinding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "old-session", binding: staleBinding))
        let retained = try await sendPreparedTextEdit(
            outbox,
            nodeId: NodeId(42),
            text: "old",
            editSeq: try #require(EditSeq(1)),
            observedRevision: Revision(1),
            binding: staleBinding,
            via: transport
        )
        let generation = try #require(await outbox.beginResumeAttempt(binding: staleBinding))

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
        let current = try await sendPreparedTextEdit(
            outbox,
            nodeId: NodeId(43),
            text: "new",
            editSeq: try #require(EditSeq(1)),
            observedRevision: Revision(2),
            binding: currentBinding,
            via: transport
        )

        await gate.releaseAssignment()
        #expect(await staleReplacement.value == false)
        #expect(await MainActor.run { recorder.resetCount == 0 })
        #expect(await outbox.eventSeq == 2)
        #expect(await outbox.pendingCount == 2)
        #expect(await outbox.assignedTextEditEvents().map(\.eventId) == [
            retained.eventId, current.eventId
        ])
        await transport.close()
    }
    @Test("An oversized text edit does not burn event_seq")
    func oversizedTextEditAllocationIsTransactional() async throws {
        let outbox = EventOutbox()
        let transport = EventSequenceRecordingTransport()
        let binding = await outbox.beginConnectionBinding()
        let incarnation = try #require(await outbox.sessionIncarnation(binding: binding))
        #expect(await outbox.confirmFreshSession(id: "same-session", binding: binding))

        do {
            _ = try await outbox.prepareTextEdit(
                nodeId: NodeId(46),
                text: String(repeating: "x", count: defaultMaxFrameSize),
                editSeq: try #require(EditSeq(1)),
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
        #expect(await outbox.assignedTextEditDescriptors().isEmpty)

        let next = try await sendPreparedTextEdit(
            outbox,
            nodeId: NodeId(46),
            text: "fits",
            editSeq: try #require(EditSeq(2)),
            observedRevision: Revision(1),
            binding: binding,
            sessionIncarnation: incarnation,
            via: transport
        )
        #expect(next.eventSeq == 1)
        #expect(await outbox.eventSeq == 1)
        #expect(await outbox.pendingCount == 1)
        #expect(await transport.sentEventSequences() == [1])
        await transport.close()
    }
    @Test("A rejected acknowledgement from an old incarnation cannot resolve replacement text")
    func staleRejectedAcknowledgementCannotResolveReplacementText() async throws {
        let outbox = EventOutbox()
        let transport = EventSequenceRecordingTransport()
        let binding = await outbox.beginConnectionBinding()
        let staleIncarnation = try #require(await outbox.sessionIncarnation(binding: binding))
        #expect(await outbox.confirmFreshSession(id: "old-session", binding: binding))
        let retained = try await sendPreparedTextEdit(
            outbox,
            nodeId: NodeId(44),
            text: "rejected",
            editSeq: try #require(EditSeq(1)),
            observedRevision: Revision(1),
            binding: binding,
            sessionIncarnation: staleIncarnation,
            via: transport
        )
        #expect(await outbox.settleAcknowledgement(
            binding: binding,
            sessionIncarnation: staleIncarnation,
            clientInstanceId: outbox.clientInstanceId,
            eventId: retained.eventId,
            throughSeq: retained.eventSeq,
            sessionId: "old-session",
            revisionAfterEffect: 5,
            textEditRejected: true
        ).bound)

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
                onResolved: { recorder.noteAcknowledgements($0) }
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
        let current = try await sendPreparedTextEdit(
            outbox,
            nodeId: NodeId(44),
            text: "current",
            editSeq: try #require(EditSeq(1)),
            observedRevision: Revision(1),
            binding: binding,
            sessionIncarnation: currentIncarnation,
            via: transport
        )

        await gate.releaseAssignment()
        #expect(await staleResolution.value == false)
        #expect(await MainActor.run { recorder.acknowledgementCalls.isEmpty })
        #expect(await outbox.assignedTextEditDescriptors().map(\.eventId) == [current.eventId])
        await transport.close()
    }
}

private func activeBinding(for outbox: EventOutbox) async -> EventOutboxConnectionBinding {
    let binding = await outbox.beginConnectionBinding()
    guard await outbox.allowNewEvents(binding: binding) else {
        Issue.record("Fresh test binding did not open event dispatch")
        return binding
    }
    return binding
}

private func sendPreparedTextEdit(
    _ outbox: EventOutbox,
    nodeId: NodeId,
    text: String,
    editSeq: EditSeq,
    observedRevision: Revision,
    binding: EventOutboxConnectionBinding,
    sessionIncarnation: EventOutboxSessionIncarnation? = nil,
    via transport: any Transport
) async throws -> Event {
    let prepared = try #require(try await outbox.prepareTextEdit(
        nodeId: nodeId,
        text: text,
        editSeq: editSeq,
        observedRevision: observedRevision,
        binding: binding,
        sessionIncarnation: sessionIncarnation,
        via: transport
    ))
    #expect(await outbox.authorizePreparedTextEdit(prepared))
    return try #require(try await outbox.releasePreparedTextEdit(prepared))
}

private func resumeSameSession(
    _ outbox: EventOutbox,
    id: String,
    lastProcessedEventSeq: UInt64,
    generation: UInt64,
    binding: EventOutboxConnectionBinding,
    via transport: any Transport,
    enableNewEventsAfterReplay: Bool,
    discardedTextEdits: [SRUIPendingTextEditRef]? = nil,
    requireExactTextMatch: Bool = true
) async throws -> Bool {
    guard let preparation = try await outbox.prepareSameSessionResume(
        id: id,
        lastProcessedEventSeq: lastProcessedEventSeq,
        generation: generation,
        binding: binding,
        discardedTextEdits: discardedTextEdits,
        requireExactTextMatch: requireExactTextMatch
    ) else {
        return false
    }
    return try await outbox.completeSameSessionResume(
        preparation,
        via: transport,
        enableNewEventsAfterReplay: enableNewEventsAfterReplay
    )
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
        entered = true
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
    private(set) var cancellationCalls: [[PendingTextEditDescriptor]] = []
    private(set) var acknowledgementCalls: [[TextEditAcknowledgementBarrier]] = []
    private(set) var resetCount = 0

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

private enum DeliverThenThrowError: Error, Equatable {
    case delivered
}

private actor DeliverThenThrowTransport: Transport {
    nonisolated let stream = AsyncThrowingStream<Data, Error> { continuation in
        continuation.finish()
    }
    private var events: [Event] = []

    func send(data: Data, logicalClass _: LogicalChannelClass) async throws {
        let message = try decodeFramedMessage(from: data)
        if case .event(let event) = message.msg {
            events.append(try ProtocolDecoder().validateAndConvertEvent(wire: event))
        }
        throw DeliverThenThrowError.delivered
    }

    nonisolated func receiveStream() -> AsyncThrowingStream<Data, Error> {
        stream
    }

    func close() async {}

    func sentEvents() -> [Event] {
        events
    }
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
