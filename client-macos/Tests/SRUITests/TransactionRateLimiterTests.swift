//
// TransactionRateLimiterTests.swift
// SRUITests
//

import Foundation
import Testing
import SemanticModel
import Protocol
@testable import Session
import TransportSSH

private actor TransactionRateFailureLog {
    private var messages: [String] = []

    func record(_ failure: SessionFailure) {
        messages.append(failure.description)
    }

    func contains(_ fragment: String) -> Bool {
        messages.contains { $0.contains(fragment) }
    }
}

@Suite("Transaction rate security limits")
struct TransactionRateLimiterTests {
    @Test("Default bucket admits 240 transaction burst and rejects the next")
    func defaultBurstBoundary() {
        var limiter = TransactionRateLimiter()
        for _ in 0..<TransactionRateLimits.defaultBurstCapacity {
            let admitted = limiter.admit(atUptimeNanoseconds: 10)
            #expect(admitted)
        }
        let overBurst = limiter.admit(atUptimeNanoseconds: 10)
        #expect(!overBurst)
    }

    @Test("Default bucket refills at 120 transactions per second and stays bounded")
    func sustainedRefillAndCapacity() {
        var limiter = TransactionRateLimiter()
        for _ in 0..<TransactionRateLimits.defaultBurstCapacity {
            let admitted = limiter.admit(atUptimeNanoseconds: 0)
            #expect(admitted)
        }

        for _ in 0..<TransactionRateLimits.defaultSustainedTransactionsPerSecond {
            let admitted = limiter.admit(atUptimeNanoseconds: 1_000_000_000)
            #expect(admitted)
        }
        let overSustained = limiter.admit(atUptimeNanoseconds: 1_000_000_000)
        #expect(!overSustained)

        // A very long idle interval saturates at burst capacity without arithmetic overflow.
        for _ in 0..<TransactionRateLimits.defaultBurstCapacity {
            let admitted = limiter.admit(atUptimeNanoseconds: UInt64.max)
            #expect(admitted)
        }
        let overRefilledBurst = limiter.admit(atUptimeNanoseconds: UInt64.max)
        #expect(!overRefilledBurst)
    }

    @Test("Invalid local rate configurations fail closed")
    func invalidConfiguration() {
        #expect(TransactionRateLimits(
            sustainedTransactionsPerSecond: 0,
            burstCapacity: 1
        ) == nil)
        #expect(TransactionRateLimits(
            sustainedTransactionsPerSecond: 1,
            burstCapacity: 0
        ) == nil)
        #expect(TransactionRateLimits(
            sustainedTransactionsPerSecond: UInt64.max,
            burstCapacity: 1
        ) == nil)
    }

    @Test("Controller reports update-rate exhaustion and leaves the excess transaction unapplied")
    func controllerFailsExplicitly() async throws {
        let (clientTransport, serverTransport) = await PipeTransport.createPair()
        let configuredLimits = try #require(TransactionRateLimits(
            sustainedTransactionsPerSecond: 1,
            burstCapacity: 1
        ))
        let applier = TransactionApplier()
        let controller = SessionController(
            transport: clientTransport,
            applier: applier,
            transactionRateLimits: configuredLimits
        )
        let failures = TransactionRateFailureLog()
        controller.onFailure = { failure in
            Task { await failures.record(failure) }
        }

        try await controller.start()

        var welcome = SRUIServerWelcome()
        welcome.coreVersion = SRUICoreVersion
        welcome.sessionID = "rate-limit-test"
        welcome.requiredProfiles = ["org.srui.standard-widgets/1"]
        var welcomeEnvelope = SRUIMessage()
        welcomeEnvelope.serverWelcome = welcome
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(welcomeEnvelope))

        for _ in 0..<200 where !controller.isHandshakeComplete {
            try await Task.sleep(for: .milliseconds(10))
        }
        #expect(controller.isHandshakeComplete)

        let first = Transaction(
            baseRevision: .initial,
            newRevision: Revision(1),
            operations: [.createNode(id: NodeId(1), nodeType: .surface)]
        )
        let excess = Transaction(
            baseRevision: Revision(1),
            newRevision: Revision(2),
            operations: [
                .setProperty(
                    id: NodeId(1),
                    property: .label,
                    value: .string("must not apply")
                ),
            ]
        )
        var firstEnvelope = SRUIMessage()
        firstEnvelope.transaction = first.toWire()
        var excessEnvelope = SRUIMessage()
        excessEnvelope.transaction = excess.toWire()
        var combined = try SRUIFraming.encodeFramed(firstEnvelope)
        combined.append(try SRUIFraming.encodeFramed(excessEnvelope))
        try await serverTransport.send(data: combined)

        for _ in 0..<200 {
            if await failures.contains("Maximum semantic transaction update rate exceeded") {
                break
            }
            try await Task.sleep(for: .milliseconds(10))
        }

        #expect(await failures.contains("1/s sustained, 1 burst"))
        #expect(applier.lastAppliedRevision == Revision(1))
        #expect(applier.store.node(for: NodeId(1))?.getProperty(.label) == nil)

        await controller.stop()
        await serverTransport.close()
    }
}
