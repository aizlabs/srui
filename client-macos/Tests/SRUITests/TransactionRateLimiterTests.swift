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

    /// §26 bounds the update rate per session, so continuing the same session must not hand back a
    /// full burst — otherwise a server could disconnect and resume repeatedly to replay one burst
    /// per connection. Only adopting a *different* session refills the budget.
    @Test("The ingress budget follows the session, not the connection")
    func ingressBudgetIsScopedToTheSession() async throws {
        // Sustained credit is deliberately slow (1/s) and the bucket small, so a single admission
        // leaves the budget measurably below capacity. Reading credit never refills it, so every
        // assertion below is deterministic rather than wall-clock dependent.
        let capacity: UInt64 = 3
        let limits = try #require(TransactionRateLimits(
            sustainedTransactionsPerSecond: 1,
            burstCapacity: capacity
        ))
        let (clientTransport, serverTransport) = await PipeTransport.createPair()
        let controller = SessionController(
            transport: clientTransport,
            transactionRateLimits: limits
        )
        try await controller.start()

        func deliverTransaction(newRevision: UInt64) async {
            var envelope = SRUIMessage()
            envelope.transaction = Transaction(
                baseRevision: Revision(newRevision - 1),
                newRevision: Revision(newRevision),
                operations: [.createNode(id: NodeId(newRevision), nodeType: .surface)]
            ).toWire()
            await controller.handleIncomingMessage(envelope)
        }

        func deliverResync(sessionId: String, continuity: Srui_Protocol_SessionContinuity) async {
            var resync = SRUIServerResyncRequired()
            resync.sessionID = sessionId
            resync.snapshotRevision = 1
            resync.reason = "budget probe"
            resync.continuity = continuity
            var envelope = SRUIMessage()
            envelope.serverResyncRequired = resync
            await controller.handleIncomingMessage(envelope)
        }

        // A fresh SERVER WELCOME session is adopted with a full budget.
        await controller.handleIncomingMessage(
            HandshakeFixtures.welcomeMessage(sessionId: "budget-session")
        )
        #expect(await controller.availableTransactionIngressCredit == capacity)

        // Admitting one transaction spends one token.
        await deliverTransaction(newRevision: 1)
        let spent = await controller.availableTransactionIngressCredit
        #expect(spent < capacity)

        // Same-session resync continues the same session, so the spent budget is retained.
        await deliverResync(sessionId: "budget-session", continuity: .sameSession)
        #expect(await controller.availableTransactionIngressCredit == spent)

        // A replacement is a different session and legitimately starts a new budget.
        await deliverResync(sessionId: "budget-session-2", continuity: .replaced)
        #expect(await controller.availableTransactionIngressCredit == capacity)

        // Restarting the connection is not adopting a session, so it must not refill either. The
        // restart cannot complete over a closed pipe, but `start()` reaches its own setup before
        // the handshake send, which is where a per-connection reset would live.
        await deliverTransaction(newRevision: 2)
        let beforeRestart = await controller.availableTransactionIngressCredit
        #expect(beforeRestart < capacity)
        await controller.stop()
        try? await controller.start()
        #expect(await controller.availableTransactionIngressCredit == beforeRestart)

        await controller.stop()
        await serverTransport.close()
    }

    /// Control traffic may overtake the throttled transaction lane, but a message that *replaces*
    /// the replica may not. `SERVER RESUME_OK` and `SERVER RESYNC_REQUIRED` must observe every
    /// transaction the server sent before them, or a queued transaction would be evaluated against
    /// a store the server never based it on (§18).
    ///
    /// The probe is a `RESUME_OK` with no outstanding resume: it is an unconditional protocol
    /// violation, and the teardown it triggers cancels the queued lane. If the resume were
    /// dispatched ahead of the parked transaction, that transaction would be cancelled and the
    /// replica would stop at revision 1.
    @Test("A replica-replacing control message waits for transactions the server sent before it")
    func replicaReplacingControlMessageWaitsForQueuedTransactions() async throws {
        let limits = try #require(TransactionRateLimits(
            sustainedTransactionsPerSecond: 1,
            burstCapacity: 1
        ))
        let (clientTransport, serverTransport) = await PipeTransport.createPair()
        let applier = TransactionApplier()
        let controller = SessionController(
            transport: clientTransport,
            applier: applier,
            transactionRateLimits: limits
        )
        let failures = TransactionRateFailureLog()
        controller.onFailure = { failure in
            Task { await failures.record(failure) }
        }

        try await controller.start()
        try await serverTransport.send(
            data: try SRUIFraming.encodeFramed(
                HandshakeFixtures.welcomeMessage(sessionId: "resume-ordering")
            )
        )
        try await AsyncTestSupport.eventually(description: "welcome completes") {
            controller.isHandshakeComplete
        }

        // One chunk: two transactions and the replica-replacing message. Burst capacity is 1, so
        // the second transaction is still parked on rate credit when the resume is dispatched.
        var combined = Data()
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
                    value: .string("sent-before-resume")
                ),
            ]
        )
        for transaction in [first, second] {
            var envelope = SRUIMessage()
            envelope.transaction = transaction.toWire()
            combined.append(try SRUIFraming.encodeFramed(envelope))
        }
        var resumeOK = SRUIServerResumeOk()
        resumeOK.sessionID = "resume-ordering"
        var resumeEnvelope = SRUIMessage()
        resumeEnvelope.serverResumeOk = resumeOK
        combined.append(try SRUIFraming.encodeFramed(resumeEnvelope))
        try await serverTransport.send(data: combined)

        try await AsyncTestSupport.eventuallyAsync(
            timeout: .seconds(5),
            description: "the out-of-phase resume is refused"
        ) {
            await failures.contains("Unexpected SERVER RESUME_OK")
        }

        // Both transactions were sent before the resume, so both must already be applied.
        #expect(applier.lastAppliedRevision == Revision(2))
        #expect(
            applier.store.node(for: NodeId(1))?.getProperty(.label)
                == .string("sent-before-resume")
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
