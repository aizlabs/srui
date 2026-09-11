//
// SessionLifecycleEventTests.swift
// SRUITests
//
// Deterministic coverage for the low-volume SessionController lifecycle observation stream.
//

import Foundation
import Protocol
import SemanticModel
@testable import Session
import Testing
import TransportSSH

private final class LifecycleEventTransport: @unchecked Sendable, Transport {
    private struct Inbound {
        let id: UUID
        let stream: AsyncThrowingStream<Data, Error>
        let continuation: AsyncThrowingStream<Data, Error>.Continuation
    }

    private let lock = NSLock()
    private let gatedSendIndex: Int?
    private let sendGate: AsyncGate?
    private let closeGate: AsyncGate?
    private var inbound = LifecycleEventTransport.makeInbound()
    private var sentFrameCount = 0
    private var closeCount = 0
    private var closeWaiters: [CheckedContinuation<Void, Never>] = []

    init(
        gatedSendIndex: Int? = nil,
        sendGate: AsyncGate? = nil,
        closeGate: AsyncGate? = nil
    ) {
        self.gatedSendIndex = gatedSendIndex
        self.sendGate = sendGate
        self.closeGate = closeGate
    }

    func send(data _: Data, logicalClass _: LogicalChannelClass) async throws {
        let sendIndex = withLock { () -> Int in
            defer { sentFrameCount += 1 }
            return sentFrameCount
        }
        if sendIndex == gatedSendIndex, let sendGate {
            await sendGate.pause()
        }
        try Task.checkCancellation()
    }

    func receiveStream() -> AsyncThrowingStream<Data, Error> {
        withLock { inbound.stream }
    }

    func close() async {
        if let closeGate {
            await closeGate.pause()
        }
        let (closing, waiters) = withLock {
            let closing = inbound
            inbound = Self.makeInbound()
            closeCount += 1
            let waiters = closeWaiters
            closeWaiters.removeAll(keepingCapacity: false)
            return (closing, waiters)
        }
        closing.continuation.finish()
        for waiter in waiters {
            waiter.resume()
        }
    }

    func finishPeer() {
        let continuation = withLock { inbound.continuation }
        continuation.finish()
    }

    func waitUntilClosed() async {
        await withCheckedContinuation { continuation in
            lock.lock()
            if closeCount > 0 {
                lock.unlock()
                continuation.resume()
            } else {
                closeWaiters.append(continuation)
                lock.unlock()
            }
        }
    }

    private static func makeInbound() -> Inbound {
        let pair = AsyncThrowingStream<Data, Error>.makeStream()
        return Inbound(
            id: UUID(),
            stream: pair.stream,
            continuation: pair.continuation
        )
    }

    private func withLock<T>(_ body: () -> T) -> T {
        lock.lock()
        defer { lock.unlock() }
        return body()
    }
}

private final class LifecycleFailureSignal: @unchecked Sendable {
    private let lock = NSLock()
    private var failure: SessionFailure?
    private var waiter: CheckedContinuation<SessionFailure, Never>?

    func record(_ failure: SessionFailure) {
        lock.lock()
        guard self.failure == nil else {
            lock.unlock()
            return
        }
        self.failure = failure
        let waiter = self.waiter
        self.waiter = nil
        lock.unlock()
        waiter?.resume(returning: failure)
    }

    func next() async -> SessionFailure {
        await withCheckedContinuation { continuation in
            lock.lock()
            if let failure {
                lock.unlock()
                continuation.resume(returning: failure)
            } else {
                precondition(waiter == nil, "LifecycleFailureSignal supports one waiter")
                waiter = continuation
                lock.unlock()
            }
        }
    }
}

@Suite("Session lifecycle events")
struct SessionLifecycleEventTests {
    @Test(
        "fresh handshake catch-up becomes ready only after its snapshot commits",
        .timeLimit(.minutes(1))
    )
    func freshHandshakeSnapshotLifecycle() async throws {
        let transport = LifecycleEventTransport()
        let controller = SessionController(transport: transport)
        var events = controller.lifecycleEvents.makeAsyncIterator()

        try await controller.start()

        if let event = await events.next() {
            guard case .connecting(.fresh) = event else {
                Issue.record("expected a fresh connecting event")
                await controller.stop()
                return
            }
        } else {
            Issue.record("lifecycle stream ended before fresh connection")
        }

        await controller.handleIncomingMessage(
            welcome(sessionID: "fresh-session", initialRevision: 5)
        )

        if let event = await events.next() {
            guard case .resynchronizing(.fresh(let sessionID)) = event else {
                Issue.record("expected fresh snapshot resynchronization")
                await controller.stop()
                return
            }
            #expect(sessionID == "fresh-session")
        } else {
            Issue.record("lifecycle stream ended before fresh resynchronization")
        }
        #expect(controller.isEventDispatchEnabled == false)

        await controller.handleIncomingMessage(snapshot(revision: 5, text: "fresh"))

        if let event = await events.next() {
            guard case .ready(let sessionID, let revision) = event else {
                Issue.record("expected ready after fresh snapshot")
                await controller.stop()
                return
            }
            #expect(sessionID == "fresh-session")
            #expect(revision == 5)
        } else {
            Issue.record("lifecycle stream ended before fresh ready")
        }
        #expect(controller.isEventDispatchEnabled)

        await controller.stop()
        if let event = await events.next() {
            guard case .stopped = event else {
                Issue.record("expected stopped after explicit stop")
                return
            }
        } else {
            Issue.record("lifecycle stream ended instead of reporting stop")
        }
    }

    @Test(
        "resume ready waits for pending-event replay",
        .timeLimit(.minutes(1))
    )
    func resumeReadyWaitsForReplay() async throws {
        let (seedClient, seedServer) = await PipeTransport.createPair()
        let outbox = EventOutbox()
        let seedBinding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(
            id: "resume-session",
            binding: seedBinding
        ))
        _ = try await outbox.sendActivate(
            nodeId: NodeId(9),
            observedRevision: .initial,
            binding: seedBinding,
            via: seedClient
        )

        let replayGate = AsyncGate()
        let transport = LifecycleEventTransport(
            gatedSendIndex: 1,
            sendGate: replayGate
        )
        let controller = SessionController(
            transport: transport,
            outbox: outbox,
            sessionId: "resume-session"
        )
        var events = controller.lifecycleEvents.makeAsyncIterator()

        try await controller.start()
        if let event = await events.next() {
            guard case .connecting(.resume(let sessionID, let revision)) = event else {
                Issue.record("expected a resume connecting event")
                await controller.stop()
                await seedClient.close()
                await seedServer.close()
                return
            }
            #expect(sessionID == "resume-session")
            #expect(revision == 0)
        } else {
            Issue.record("lifecycle stream ended before resume connection")
        }

        let resumeTask = Task {
            await controller.handleIncomingMessage(
                resumeOK(sessionID: "resume-session", lastProcessedEventSeq: 0)
            )
        }
        await replayGate.waitUntilPaused()
        #expect(controller.isEventDispatchEnabled == false)

        await replayGate.release()
        await resumeTask.value

        if let event = await events.next() {
            guard case .ready(let sessionID, let revision) = event else {
                Issue.record("expected ready after replay completed")
                await controller.stop()
                await seedClient.close()
                await seedServer.close()
                return
            }
            #expect(sessionID == "resume-session")
            #expect(revision == 0)
        } else {
            Issue.record("lifecycle stream ended before resumed ready")
        }
        #expect(controller.isEventDispatchEnabled)

        await controller.stop()
        await seedClient.close()
        await seedServer.close()
    }

    @Test(
        "same-session and replacement resync expose continuity before snapshot ready",
        .timeLimit(.minutes(1))
    )
    func resyncContinuityLifecycle() async throws {
        let transport = LifecycleEventTransport()
        let controller = SessionController(transport: transport)
        var events = controller.lifecycleEvents.makeAsyncIterator()

        try await controller.start()
        _ = await events.next()
        await controller.handleIncomingMessage(welcome(sessionID: "session-old"))
        _ = await events.next()

        await controller.handleIncomingMessage(
            resync(
                sessionID: "session-old",
                continuity: .sameSession,
                snapshotRevision: 5
            )
        )
        if let event = await events.next() {
            guard case .resynchronizing(.sameSession(let sessionID)) = event else {
                Issue.record("expected SAME_SESSION resynchronization")
                await controller.stop()
                return
            }
            #expect(sessionID == "session-old")
        } else {
            Issue.record("lifecycle stream ended before SAME_SESSION resynchronization")
        }
        #expect(controller.isEventDispatchEnabled == false)

        await controller.handleIncomingMessage(snapshot(revision: 5, text: "same"))
        if let event = await events.next() {
            guard case .ready(let sessionID, let revision) = event else {
                Issue.record("expected ready after SAME_SESSION snapshot")
                await controller.stop()
                return
            }
            #expect(sessionID == "session-old")
            #expect(revision == 5)
        } else {
            Issue.record("lifecycle stream ended before SAME_SESSION ready")
        }

        await controller.handleIncomingMessage(
            resync(
                sessionID: "session-new",
                continuity: .replaced,
                snapshotRevision: 9
            )
        )
        if let event = await events.next() {
            guard case .resynchronizing(.replaced(
                let previousSessionID,
                let newSessionID
            )) = event else {
                Issue.record("expected REPLACED resynchronization")
                await controller.stop()
                return
            }
            #expect(previousSessionID == "session-old")
            #expect(newSessionID == "session-new")
        } else {
            Issue.record("lifecycle stream ended before REPLACED resynchronization")
        }
        #expect(controller.sessionId == "session-new")
        #expect(controller.isEventDispatchEnabled == false)

        await controller.handleIncomingMessage(snapshot(revision: 9, text: "replacement"))
        if let event = await events.next() {
            guard case .ready(let sessionID, let revision) = event else {
                Issue.record("expected ready after REPLACED snapshot")
                await controller.stop()
                return
            }
            #expect(sessionID == "session-new")
            #expect(revision == 9)
        } else {
            Issue.record("lifecycle stream ended before REPLACED ready")
        }

        await controller.stop()
    }

    @Test(
        "clean peer disconnect emits transport failure and preserves onFailure",
        .timeLimit(.minutes(1))
    )
    func peerDisconnectLifecycle() async throws {
        let transport = LifecycleEventTransport()
        let controller = SessionController(transport: transport)
        let failureSignal = LifecycleFailureSignal()
        controller.onFailure = { failure in
            failureSignal.record(failure)
        }
        var events = controller.lifecycleEvents.makeAsyncIterator()

        try await controller.start()
        _ = await events.next()
        transport.finishPeer()

        if let event = await events.next() {
            guard case .failed(.transportEnded(let description)) = event else {
                Issue.record("expected transport-ended lifecycle failure")
                return
            }
            #expect(description == "receive stream closed by peer")
        } else {
            Issue.record("lifecycle stream ended before disconnect failure")
        }

        let callbackFailure = await failureSignal.next()
        guard case .transportEnded(let callbackDescription) = callbackFailure else {
            Issue.record("onFailure did not receive transportEnded")
            return
        }
        #expect(callbackDescription == "receive stream closed by peer")

        await transport.waitUntilClosed()
        await controller.stop()
    }

    @Test(
        "stopped generation suppresses its cancelled start failure",
        .timeLimit(.minutes(1))
    )
    func stoppedGenerationSuppressesLateStartFailure() async throws {
        let sendGate = AsyncGate()
        let closeGate = AsyncGate()
        let transport = LifecycleEventTransport(
            gatedSendIndex: 0,
            sendGate: sendGate,
            closeGate: closeGate
        )
        let controller = SessionController(transport: transport)
        var events = controller.lifecycleEvents.makeAsyncIterator()

        let firstStart = Task { () -> Bool in
            do {
                try await controller.start()
                return false
            } catch {
                return true
            }
        }
        await sendGate.waitUntilPaused()

        if let event = await events.next() {
            guard case .connecting(.fresh) = event else {
                Issue.record("expected initial fresh connection")
                return
            }
        } else {
            Issue.record("lifecycle stream ended during gated start")
        }

        let stopTask = Task {
            await controller.stop()
        }
        await closeGate.waitUntilPaused()
        await sendGate.release()
        await closeGate.release()

        #expect(await firstStart.value)
        await stopTask.value

        if let event = await events.next() {
            guard case .stopped = event else {
                Issue.record("cancelled old start leaked a failure past stop")
                return
            }
        } else {
            Issue.record("lifecycle stream ended before stopped transition")
        }

        try await controller.start()
        if let event = await events.next() {
            guard case .connecting(.fresh) = event else {
                Issue.record("stale lifecycle event preceded restarted connection")
                await controller.stop()
                return
            }
        } else {
            Issue.record("lifecycle stream ended before restarted connection")
        }

        await controller.stop()
    }

    @Test("releasing the controller finishes the lifecycle stream")
    func controllerReleaseFinishesStream() async {
        var controller: SessionController? = SessionController(
            transport: LifecycleEventTransport()
        )
        guard let stream = controller?.lifecycleEvents else {
            Issue.record("controller did not expose lifecycle stream")
            return
        }
        var events = stream.makeAsyncIterator()

        controller = nil

        switch await events.next() {
        case nil:
            break
        case .some:
            Issue.record("lifecycle stream stayed open after controller release")
        }
    }

    private func welcome(
        sessionID: String,
        initialRevision: UInt64 = 0
    ) -> SRUIMessage {
        var welcome = SRUIServerWelcome()
        welcome.coreVersion = SRUICoreVersion
        welcome.sessionID = sessionID
        welcome.requiredProfiles = ["org.srui.standard-widgets/1"]
        welcome.initialRevision = initialRevision
        var message = SRUIMessage()
        message.serverWelcome = welcome
        return message
    }

    private func resumeOK(
        sessionID: String,
        lastProcessedEventSeq: UInt64
    ) -> SRUIMessage {
        var resume = SRUIServerResumeOk()
        resume.sessionID = sessionID
        resume.lastProcessedEventSeq = lastProcessedEventSeq
        var message = SRUIMessage()
        message.serverResumeOk = resume
        return message
    }

    private func resync(
        sessionID: String,
        continuity: Srui_Protocol_SessionContinuity,
        snapshotRevision: UInt64
    ) -> SRUIMessage {
        var resync = SRUIServerResyncRequired()
        resync.sessionID = sessionID
        resync.snapshotRevision = snapshotRevision
        resync.reason = "lifecycle test"
        resync.continuity = continuity
        var message = SRUIMessage()
        message.serverResyncRequired = resync
        return message
    }

    private func snapshot(revision: UInt64, text: String) -> SRUIMessage {
        let surfaceID = NodeId(1)
        let textID = NodeId(2)
        let transaction = Transaction(
            baseRevision: .initial,
            newRevision: Revision(revision),
            operations: [
                .createNode(id: surfaceID, nodeType: .surface),
                .createNode(
                    id: textID,
                    nodeType: .text,
                    parentID: surfaceID,
                    properties: [
                        Property(property: .text, value: .string(text)),
                    ]
                ),
            ]
        )
        var message = SRUIMessage()
        message.transaction = transaction.toWire()
        return message
    }
}
