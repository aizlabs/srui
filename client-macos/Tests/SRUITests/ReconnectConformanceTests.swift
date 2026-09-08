//
// ReconnectConformanceTests.swift
// SRUITests
//
// SRUI Reconnect Conformance Suite (§32 item 8) — client side.
//
// Implements: §18 (reconnect and continuity), §18.2 (event settlement, dedupe, frontier),
// §18.3 (pending text edits), §32.8.
//
// The server half is server-rust/sessiond/tests/conformance_reconnect_test.rs, which walks the
// full fourteen-scenario checklist against `Session`. This file is the client's counterpart: the
// same settlement rules seen from `EventOutbox`, which is where a client can independently get
// them wrong — by retiring an intent an acknowledgement did not cover, by letting a foreign
// session settle its outbox, or by advancing its frontier across a gap.
//
// `SessionResumeContinuityTests` remains the detailed continuity coverage (SAME_SESSION vs
// REPLACED, supersession, hard-failure on unknown continuity); this suite deliberately does not
// restate it, and the manifest lists both under suite 8.
//

import Foundation
import Protocol
import SemanticModel
import Testing

@testable import Session
import TransportSSH

@Suite
struct ReconnectConformanceTests {

    /// §18.2: the cumulative frontier is normative settlement. An acknowledgement that covers
    /// sequence 1 retires it, and the outbox reports the frontier it actually reached.
    @Test("A covering acknowledgement retires the intent and advances the frontier")
    func coveringAckAdvancesFrontier() async throws {
        let (client, server) = await PipeTransport.createPair()
        let outbox = EventOutbox(maxPendingEvents: 4)
        let binding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "conformance-frontier", binding: binding))

        let first = try await outbox.sendActivate(
            nodeId: NodeId(1), observedRevision: Revision(1), binding: binding, via: client)

        let settlement = await outbox.settleAcknowledgement(
            binding: binding,
            clientInstanceId: outbox.clientInstanceId,
            eventId: first.eventId,
            throughSeq: 1,
            sessionId: "conformance-frontier")

        #expect(settlement.bound)
        #expect(await outbox.lastAckedEventSeq == 1)
        #expect(await outbox.pendingCount == 0)

        await client.close()
        await server.close()
    }

    /// §18.2: the frontier is the highest *contiguous* settled sequence. Settling sequence 2
    /// while sequence 1 is outstanding must not advance it past the gap, and must not free the
    /// window slot sequence 1 still occupies.
    @Test("Out-of-order settlement does not advance the frontier across a gap")
    func outOfOrderSettlementHoldsAtTheGap() async throws {
        let (client, server) = await PipeTransport.createPair()
        let outbox = EventOutbox(maxPendingEvents: 4)
        let binding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "conformance-gap", binding: binding))

        let first = try await outbox.sendActivate(
            nodeId: NodeId(1), observedRevision: Revision(1), binding: binding, via: client)
        let second = try await outbox.sendActivate(
            nodeId: NodeId(2), observedRevision: Revision(1), binding: binding, via: client)

        // Sequence 2 settles first; the frontier cannot cross the missing sequence 1.
        #expect(
            await outbox.settleAcknowledgement(
                binding: binding,
                clientInstanceId: outbox.clientInstanceId,
                eventId: second.eventId,
                throughSeq: 0,
                sessionId: "conformance-gap"
            ).bound)
        #expect(await outbox.lastAckedEventSeq == 0)
        #expect(await outbox.pendingCount == 1)

        // Closing the gap advances the frontier straight through the already-settled sequence 2.
        #expect(
            await outbox.settleAcknowledgement(
                binding: binding,
                clientInstanceId: outbox.clientInstanceId,
                eventId: first.eventId,
                throughSeq: 2,
                sessionId: "conformance-gap"
            ).bound)
        #expect(await outbox.lastAckedEventSeq == 2)
        #expect(await outbox.pendingCount == 0)

        await client.close()
        await server.close()
    }

    /// §18.2: `session_id` is required on every acknowledgement. An ack from another incarnation
    /// — or with no incarnation at all — proves nothing about what settled, so it can never
    /// retire an intent.
    @Test("A foreign or empty session id cannot settle the active outbox")
    func foreignSessionCannotSettleOutbox() async throws {
        let (client, server) = await PipeTransport.createPair()
        let outbox = EventOutbox(maxPendingEvents: 4)
        let binding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "conformance-session", binding: binding))

        let event = try await outbox.sendActivate(
            nodeId: NodeId(1), observedRevision: Revision(1), binding: binding, via: client)

        for foreignSessionId in ["", "some-other-incarnation"] {
            let settlement = await outbox.settleAcknowledgement(
                binding: binding,
                clientInstanceId: outbox.clientInstanceId,
                eventId: event.eventId,
                throughSeq: 1,
                sessionId: foreignSessionId)
            #expect(
                !settlement.bound,
                "session id '\(foreignSessionId)' must not settle the active outbox (§18.2)")
        }

        #expect(await outbox.lastAckedEventSeq == 0)
        #expect(await outbox.pendingCount == 1)

        await client.close()
        await server.close()
    }

    /// §18.2: dedupe windows are per client instance. An acknowledgement addressed to a different
    /// `client_instance_id` must not settle this client's outbox.
    @Test("Another client instance cannot settle this outbox")
    func foreignClientInstanceCannotSettleOutbox() async throws {
        let (client, server) = await PipeTransport.createPair()
        let outbox = EventOutbox(maxPendingEvents: 4)
        let binding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "conformance-client", binding: binding))

        let event = try await outbox.sendActivate(
            nodeId: NodeId(1), observedRevision: Revision(1), binding: binding, via: client)

        let settlement = await outbox.settleAcknowledgement(
            binding: binding,
            clientInstanceId: ClientInstanceId(Data([9, 9, 9, 9])),
            eventId: event.eventId,
            throughSeq: 1,
            sessionId: "conformance-client")

        #expect(!settlement.bound, "an ack for another client instance must not settle (§18.2)")
        #expect(await outbox.lastAckedEventSeq == 0)
        #expect(await outbox.pendingCount == 1)

        await client.close()
        await server.close()
    }

    /// §18.2: a stale connection binding cannot settle intents bound to the live one. This is the
    /// client mirror of the server's supersession rule — a delayed ack from an abandoned
    /// connection is inert.
    @Test("A stale connection binding cannot settle intents on the live binding")
    func staleBindingCannotSettle() async throws {
        let (client, server) = await PipeTransport.createPair()
        let outbox = EventOutbox(maxPendingEvents: 4)

        let staleBinding = await outbox.beginConnectionBinding()
        let liveBinding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "conformance-stale", binding: liveBinding))

        let event = try await outbox.sendActivate(
            nodeId: NodeId(1), observedRevision: Revision(1), binding: liveBinding, via: client)

        let settlement = await outbox.settleAcknowledgement(
            binding: staleBinding,
            clientInstanceId: outbox.clientInstanceId,
            eventId: event.eventId,
            throughSeq: 1,
            sessionId: "conformance-stale")

        #expect(
            settlement == .staleConnection,
            "an ack arriving on a superseded connection binding is inert (§18, §18.2)")
        #expect(await outbox.lastAckedEventSeq == 0)
        #expect(await outbox.pendingCount == 1)

        await client.close()
        await server.close()
    }

    /// §18.2: acknowledgements are cumulative, so a re-delivered ack for an already-settled
    /// event must be inert — it may not rewind the frontier, resurrect a retired intent, or
    /// double-count. A lost-ack retry is the normal way this happens.
    ///
    /// Replay *identity* across a resume is covered in depth by `EventOutboxTests`; the manifest
    /// lists both files under suite 8 rather than restating that flow here.
    @Test("Re-delivering a settled acknowledgement is inert")
    func repeatedAcknowledgementIsIdempotent() async throws {
        let (client, server) = await PipeTransport.createPair()
        let outbox = EventOutbox(maxPendingEvents: 4)
        let binding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "conformance-idempotent", binding: binding))

        let first = try await outbox.sendActivate(
            nodeId: NodeId(1), observedRevision: Revision(1), binding: binding, via: client)
        let second = try await outbox.sendActivate(
            nodeId: NodeId(2), observedRevision: Revision(1), binding: binding, via: client)

        #expect(
            await outbox.settleAcknowledgement(
                binding: binding,
                clientInstanceId: outbox.clientInstanceId,
                eventId: first.eventId,
                throughSeq: 1,
                sessionId: "conformance-idempotent"
            ).bound)
        #expect(await outbox.lastAckedEventSeq == 1)
        #expect(await outbox.pendingCount == 1)

        // The same ack arrives again after a retry: nothing may move.
        _ = await outbox.settleAcknowledgement(
            binding: binding,
            clientInstanceId: outbox.clientInstanceId,
            eventId: first.eventId,
            throughSeq: 1,
            sessionId: "conformance-idempotent")

        #expect(
            await outbox.lastAckedEventSeq == 1,
            "a repeated acknowledgement must not move the frontier")
        #expect(
            await outbox.pendingCount == 1,
            "sequence 2 is still unsettled and must remain pending")
        #expect(await outbox.eventSeq == 2, "no sequence may be minted or reused by a repeat ack")

        // Sequence 2 still settles normally afterwards.
        #expect(
            await outbox.settleAcknowledgement(
                binding: binding,
                clientInstanceId: outbox.clientInstanceId,
                eventId: second.eventId,
                throughSeq: 2,
                sessionId: "conformance-idempotent"
            ).bound)
        #expect(await outbox.lastAckedEventSeq == 2)
        #expect(await outbox.pendingCount == 0)

        await client.close()
        await server.close()
    }
}
