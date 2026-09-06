//
// EventOutboxPublicAPITests.swift
// SRUITests
//
// Compile-time coverage of the external EventOutbox connection lifecycle.
//

import Testing
import SemanticModel
import Session
import TransportSSH

@Suite("EventOutbox Public API Tests")
struct EventOutboxPublicAPITests {
    @Test("External callers acquire, activate, and use an opaque connection binding")
    func acquireActivateAndSend() async throws {
        let (client, server) = await PipeTransport.createPair()
        let outbox = EventOutbox()

        let binding = await outbox.beginConnectionBinding()
        let activated = await outbox.confirmFreshSession(
            id: "public-session",
            binding: binding
        )
        #expect(activated)

        let event = try await outbox.sendActivate(
            nodeId: NodeId(1),
            observedRevision: Revision(1),
            binding: binding,
            via: client
        )
        #expect(event.eventSeq == 1)

        await client.close()
        await server.close()
    }
}
