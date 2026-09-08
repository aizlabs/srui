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

    var isEmpty: Bool {
        messages.isEmpty
    }

    func contains(_ fragment: String) -> Bool {
        messages.contains { $0.contains(fragment) }
    }
}

@Suite("Transaction rate security limits")
struct TransactionRateLimiterTests {
    @Test("Default bucket admits 240 transaction burst then requests backpressure")
    func defaultBurstBoundary() {
        var limiter = TransactionRateLimiter()
        for _ in 0..<TransactionRateLimits.defaultBurstCapacity {
            #expect(limiter.admission(atUptimeNanoseconds: 10) == .admitted)
        }
        let overBurst = limiter.admission(atUptimeNanoseconds: 10)
        guard case .wait(let nanoseconds) = overBurst else {
            Issue.record("Expected ingress delay after burst exhaustion")
            return
        }
        #expect(nanoseconds > 0)
    }

    @Test("Default bucket refills at 120 transactions per second and stays bounded")
    func sustainedRefillAndCapacity() {
        var limiter = TransactionRateLimiter()
        for _ in 0..<TransactionRateLimits.defaultBurstCapacity {
            #expect(limiter.admission(atUptimeNanoseconds: 0) == .admitted)
        }

        for _ in 0..<TransactionRateLimits.defaultSustainedTransactionsPerSecond {
            #expect(limiter.admission(atUptimeNanoseconds: 1_000_000_000) == .admitted)
        }
        guard case .wait = limiter.admission(atUptimeNanoseconds: 1_000_000_000) else {
            Issue.record("Expected ingress delay after sustained credit was consumed")
            return
        }

        // A very long idle interval saturates at burst capacity without arithmetic overflow.
        for _ in 0..<TransactionRateLimits.defaultBurstCapacity {
            #expect(limiter.admission(atUptimeNanoseconds: UInt64.max) == .admitted)
        }
        guard case .wait = limiter.admission(atUptimeNanoseconds: UInt64.max) else {
            Issue.record("Expected a bounded bucket after long-idle refill")
            return
        }
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

    @Test("A 241-transaction same-session replay is backpressured, not failed")
    func journalReplayBeyondBurstCompletes() async throws {
        let outbox = EventOutbox()
        let seedBinding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "rate-limit-resume", binding: seedBinding))

        let (clientTransport, serverTransport) = await PipeTransport.createPair()
        let applier = TransactionApplier()
        let controller = SessionController(
            transport: clientTransport,
            applier: applier,
            outbox: outbox,
            sessionId: "rate-limit-resume"
        )
        let failures = TransactionRateFailureLog()
        controller.onFailure = { failure in
            Task { await failures.record(failure) }
        }

        try await controller.start()

        var resumeOK = SRUIServerResumeOk()
        resumeOK.sessionID = "rate-limit-resume"
        resumeOK.replayFromRevision = 0
        var resumeEnvelope = SRUIMessage()
        resumeEnvelope.serverResumeOk = resumeOK
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(resumeEnvelope))

        try await AsyncTestSupport.eventually(description: "resume handshake completes") {
            controller.isHandshakeComplete
        }

        var replay = Data()
        for revision in 1...241 {
            let transaction: Transaction
            if revision == 1 {
                transaction = Transaction(
                    baseRevision: .initial,
                    newRevision: Revision(1),
                    operations: [.createNode(id: NodeId(1), nodeType: .surface)]
                )
            } else {
                transaction = Transaction(
                    baseRevision: Revision(UInt64(revision - 1)),
                    newRevision: Revision(UInt64(revision)),
                    operations: [
                        .setProperty(
                            id: NodeId(1),
                            property: .label,
                            value: .string("revision-\(revision)")
                        ),
                    ]
                )
            }
            var envelope = SRUIMessage()
            envelope.transaction = transaction.toWire()
            replay.append(try SRUIFraming.encodeFramed(envelope))
        }
        try await serverTransport.send(data: replay)

        try await AsyncTestSupport.eventually(
            timeout: .seconds(3),
            description: "all 241 replay transactions apply through ingress backpressure"
        ) {
            applier.lastAppliedRevision == Revision(241)
        }
        #expect(await failures.isEmpty)
        #expect(
            applier.store.node(for: NodeId(1))?.getProperty(.label)
                == .string("revision-241")
        )

        await controller.stop()
        await serverTransport.close()
    }

    @Test("Control messages bypass a throttled transaction lane and reject oversized ACK IDs")
    func controlMessagesRemainResponsiveDuringThrottle() async throws {
        let limits = try #require(TransactionRateLimits(
            sustainedTransactionsPerSecond: 1,
            burstCapacity: 1
        ))
        let (clientTransport, serverTransport) = await PipeTransport.createPair()
        let controller = SessionController(
            transport: clientTransport,
            transactionRateLimits: limits
        )
        let failures = TransactionRateFailureLog()
        controller.onFailure = { failure in
            Task { await failures.record(failure) }
        }

        try await controller.start()
        try await serverTransport.send(
            data: try SRUIFraming.encodeFramed(
                HandshakeFixtures.welcomeMessage(sessionId: "throttled-control")
            )
        )
        try await AsyncTestSupport.eventually(description: "welcome completes") {
            controller.isHandshakeComplete
        }

        let first = Transaction(
            baseRevision: .initial,
            newRevision: Revision(1),
            operations: [.createNode(id: NodeId(1), nodeType: .surface)]
        )
        let second = Transaction(
            baseRevision: Revision(1),
            newRevision: Revision(2),
            operations: [
                .setProperty(
                    id: NodeId(1),
                    property: .label,
                    value: .string("queued")
                ),
            ]
        )
        var firstEnvelope = SRUIMessage()
        firstEnvelope.transaction = first.toWire()
        var secondEnvelope = SRUIMessage()
        secondEnvelope.transaction = second.toWire()
        var invalidAck = SRUIServerEventAck()
        invalidAck.eventID = Data(repeating: 0x41, count: maxEventIDBytes + 1)
        invalidAck.sessionID = "throttled-control"
        var ackEnvelope = SRUIMessage()
        ackEnvelope.serverEventAck = invalidAck

        var combined = Data()
        combined.append(try SRUIFraming.encodeFramed(firstEnvelope))
        combined.append(try SRUIFraming.encodeFramed(secondEnvelope))
        combined.append(try SRUIFraming.encodeFramed(ackEnvelope))
        try await serverTransport.send(data: combined)

        try await AsyncTestSupport.eventuallyAsync(
            timeout: .milliseconds(300),
            description: "oversized control ACK rejected without waiting one second for rate credit"
        ) {
            await failures.contains("invalid event_id")
        }

        await controller.stop()
        await serverTransport.close()
    }
}
