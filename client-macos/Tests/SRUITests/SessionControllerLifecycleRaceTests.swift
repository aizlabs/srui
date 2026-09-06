//
// SessionControllerLifecycleRaceTests.swift
// SRUITests
//
// Deterministic lifecycle interleavings for controller admission, receive-loop ownership,
// replay-failure teardown, and AppKit mount retirement.
//

import AppKit
import Foundation
import Protocol
import RendererAppKit
import SemanticModel
@testable import Session
import Testing
import TransportSSH

private final class LifecycleRaceCounter: @unchecked Sendable {
    private let lock = NSLock()
    private var storedValue = 0

    var value: Int {
        lock.lock()
        defer { lock.unlock() }
        return storedValue
    }

    func increment() {
        lock.lock()
        defer { lock.unlock() }
        storedValue += 1
    }
}

/// A restartable in-memory transport whose send outcomes and close suspension are deterministic.
///
/// close() finishes only the stream owned by the current lifecycle, then installs a fresh stream
/// for a later SessionController.start(). This models a reconnectable test transport without
/// letting an old close terminate the new receive loop.
private final class LifecycleRaceTransport: @unchecked Sendable, Transport {
    enum SendVerdict: Sendable {
        case succeed
        case fail(String)
    }

    private struct Inbound {
        let id: UUID
        let stream: AsyncThrowingStream<Data, Error>
        let continuation: AsyncThrowingStream<Data, Error>.Continuation
    }

    private let lock = NSLock()
    private let sendScript: [SendVerdict]
    private let sendGate: LifecycleRaceGate?
    private let closeGate: LifecycleRaceGate?
    private var inbound: Inbound
    private var sentFrames: [Data] = []
    private var closeCalls = 0
    private var acknowledgedBytes = 0

    init(
        sendScript: [SendVerdict] = [],
        sendGate: LifecycleRaceGate? = nil,
        closeGate: LifecycleRaceGate? = nil
    ) {
        self.sendScript = sendScript
        self.sendGate = sendGate
        self.closeGate = closeGate
        self.inbound = Self.makeInbound()
    }

    var sentFrameCount: Int {
        withLock { sentFrames.count }
    }

    var closeCallCount: Int {
        withLock { closeCalls }
    }

    var acknowledgedByteCount: Int {
        withLock { acknowledgedBytes }
    }

    func frame(at index: Int) -> Data? {
        withLock {
            guard sentFrames.indices.contains(index) else { return nil }
            return sentFrames[index]
        }
    }

    func deliver(_ message: SRUIMessage) throws {
        let data = try SRUIFraming.encodeFramed(message)
        let continuation = withLock { inbound.continuation }
        continuation.yield(data)
    }

    func send(data: Data, logicalClass _: LogicalChannelClass) async throws {
        if let sendGate {
            await sendGate.pause()
        }
        try Task.checkCancellation()
        let verdict = recordSend(data)
        if case .fail(let message) = verdict {
            throw TransportError.ioError(message)
        }
    }

    func receiveStream() -> AsyncThrowingStream<Data, Error> {
        withLock { inbound.stream }
    }

    func acknowledgeReceived(byteCount: Int) async {
        recordAcknowledgement(byteCount)
    }

    func close() async {
        let closing = beginClose()
        if let closeGate {
            await closeGate.pause()
        }
        closing.continuation.finish()
        rotateInbound(afterClosing: closing.id)
    }

    private static func makeInbound() -> Inbound {
        let pair = AsyncThrowingStream<Data, Error>.makeStream()
        return Inbound(
            id: UUID(),
            stream: pair.stream,
            continuation: pair.continuation
        )
    }

    private func recordSend(_ data: Data) -> SendVerdict {
        withLock {
            let index = sentFrames.count
            sentFrames.append(data)
            return index < sendScript.count ? sendScript[index] : .succeed
        }
    }

    private func beginClose() -> Inbound {
        withLock {
            closeCalls += 1
            return inbound
        }
    }

    private func rotateInbound(afterClosing id: UUID) {
        withLock {
            guard inbound.id == id else { return }
            inbound = Self.makeInbound()
        }
    }

    private func recordAcknowledgement(_ byteCount: Int) {
        withLock {
            acknowledgedBytes += byteCount
        }
    }

    private func withLock<T>(_ body: () -> T) -> T {
        lock.lock()
        defer { lock.unlock() }
        return body()
    }
}

@Suite("SessionController Lifecycle Race Tests")
struct SessionControllerLifecycleRaceTests {
    private let surfaceID = NodeId(1)
    private let textID = NodeId(2)

    @Test(
        "stop quiesces an in-flight handshake send before restart",
        .bug("https://github.com/aizlabs/srui/pull/39"),
        .timeLimit(.minutes(1))
    )
    func stopQuiescesHandshakeSendBeforeRestart() async throws {
        let sendGate = LifecycleRaceGate()
        let completionGate = LifecycleRaceGate()
        let transport = LifecycleRaceTransport(sendGate: sendGate)
        let controller = SessionController(transport: transport)
        controller.handshakeSendDidFinishForTesting = {
            await completionGate.pause()
        }

        let firstStart = Task { () -> Bool in
            do {
                try await controller.start()
                return false
            } catch {
                return true
            }
        }
        await sendGate.waitUntilPaused()
        #expect(transport.sentFrameCount == 0)

        let stopFinished = LifecycleRaceCounter()
        let stopTask = Task {
            await controller.stop()
            stopFinished.increment()
        }
        try await Self.waitUntil("stop closed the transport while handshake remained paused") {
            transport.closeCallCount == 1
        }

        // A restart attempted during teardown remains inadmissible, and stop cannot finish until
        // the exact old handshake send has unwound.
        try await controller.start()
        #expect(transport.sentFrameCount == 0)
        #expect(stopFinished.value == 0)
        await sendGate.release()
        await completionGate.waitUntilPaused()
        await stopTask.value
        #expect(stopFinished.value == 1)
        #expect(transport.sentFrameCount == 0)

        // stop() clears the exact completed send ownership before reopening lifecycle admission;
        // the still-paused old start must not reject or later clear this replacement attempt.
        controller.handshakeSendDidFinishForTesting = nil
        try await controller.start()
        #expect(transport.sentFrameCount == 1)
        let handshake = try #require(transport.frame(at: 0))
        guard case .clientHello = try decodeFramedMessage(from: handshake).msg else {
            Issue.record("restart did not send CLIENT HELLO")
            return
        }
        await completionGate.release()
        #expect(await firstStart.value)
        try transport.deliver(Self.welcome(sessionID: "handshake-restart"))
        try await Self.waitUntil("replacement handshake remained active") {
            controller.isEventDispatchEnabled
        }
        await controller.stop()
        #expect(transport.closeCallCount == 2)
    }

    @Test(
        "stop closes native interaction admission before transport teardown",
        .bug("https://github.com/aizlabs/srui/pull/39"),
        .timeLimit(.minutes(1))
    )
    @MainActor
    func stopRejectsLateNativeInteractionAdmission() async throws {
        let closeGate = LifecycleRaceGate()
        let transport = LifecycleRaceTransport(closeGate: closeGate)
        let outbox = EventOutbox()
        let renderer = AppKitRenderer()
        renderer.textEditingSession.debounceNanoseconds = 0
        let controller = SessionController(
            transport: transport,
            outbox: outbox,
            renderer: renderer
        )
        controller.attachRenderer(renderer)

        try await controller.start()
        try transport.deliver(Self.welcome(sessionID: "admission"))
        try await AsyncTestSupport.eventually(description: "fresh session event admission") {
            controller.isEventDispatchEnabled
        }

        let admissionCalls = LifecycleRaceCounter()
        controller.interactionWillEnterOutboxForTesting = {
            admissionCalls.increment()
        }

        let stopTask = Task {
            await controller.stop()
        }
        await closeGate.waitUntilPaused()

        // Both callbacks arrive after stop has synchronously closed admission but while transport
        // teardown is still suspended. Neither may allocate a dispatch-tail task.
        renderer.onInteraction?(.activate(nodeID: NodeId(90)))
        renderer.textEditingSession.noteLocalValue(
            "late text",
            nodeID: NodeId(91),
            composing: false,
            flushImmediately: true
        )

        #expect(admissionCalls.value == 0)
        #expect(renderer.textEditingSession.hasUnsentSuccessorDraft(for: NodeId(91)))
        #expect(await outbox.pendingCount == 0)
        #expect(await outbox.assignedTextEditDescriptors().isEmpty)

        await closeGate.release()
        await stopTask.value

        #expect(admissionCalls.value == 0)
        #expect(transport.sentFrameCount == 1)
        #expect(await outbox.pendingCount == 0)
        #expect(await outbox.assignedTextEditDescriptors().isEmpty)
    }

    @Test(
        "stop quiesces a receive loop paused before task adoption",
        .bug("https://github.com/aizlabs/srui/pull/39"),
        .timeLimit(.minutes(1))
    )
    func stopSupersedesReceiveLoopBeforeAdoption() async throws {
        let transport = LifecycleRaceTransport()
        let applier = TransactionApplier()
        let controller = SessionController(
            transport: transport,
            applier: applier
        )
        let adoptionGate = LifecycleRaceGate()
        let failures = LifecycleRaceCounter()
        controller.receiveLoopWillAdoptForTesting = {
            await adoptionGate.pause()
        }
        controller.onFailure = { _ in
            failures.increment()
        }

        let startTask = Task { () -> Bool in
            do {
                try await controller.start()
                return false
            } catch {
                return true
            }
        }
        await adoptionGate.waitUntilPaused()

        // stop() owns no published receive task yet, but it must still supersede and quiesce the
        // gated task. Put frames on the replacement stream so an orphan that starts late would
        // visibly mutate the replica.
        await controller.stop()
        try transport.deliver(Self.welcome(sessionID: "stale-start"))
        try transport.deliver(Self.mountTransaction(text: "must not apply"))
        controller.receiveLoopWillAdoptForTesting = nil
        await adoptionGate.release()

        #expect(await startTask.value)
        #expect(applier.lastAppliedRevision == .initial)
        #expect(applier.store.nodeCount == 0)
        #expect(controller.sessionId == nil)
        #expect(transport.acknowledgedByteCount == 0)
        #expect(failures.value == 0)

        await transport.close()
    }

    @Test(
        "a stale replay failure cannot fail or close a restarted lifecycle",
        .bug("https://github.com/aizlabs/srui/pull/39"),
        .timeLimit(.minutes(1))
    )
    func staleReplayFailureCannotTearDownRestartedLifecycle() async throws {
        let (seedClient, seedServer) = await PipeTransport.createPair()
        let outbox = EventOutbox(
            replayRetryInitialDelay: .zero,
            replayRetryMaximumDelay: .zero
        )
        let seedBinding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "replay-session", binding: seedBinding))
        let pending = try await outbox.sendActivate(
            nodeId: NodeId(7),
            observedRevision: Revision(1),
            binding: seedBinding,
            via: seedClient
        )
        await seedClient.close()
        await seedServer.close()

        // Resume, initial replay, and its background retry are independently scripted. Only the
        // old lifecycle's retry fails; all traffic for the restarted lifecycle succeeds.
        let transport = LifecycleRaceTransport(sendScript: [
            .succeed,
            .succeed,
            .fail("stale replay retry"),
            .succeed,
            .succeed,
        ])
        let controller = SessionController(
            transport: transport,
            outbox: outbox,
            sessionId: "replay-session"
        )
        let replayFailureGate = LifecycleRaceGate()
        let replayHookReturns = LifecycleRaceCounter()
        let failures = LifecycleRaceCounter()
        controller.pendingReplayFailureWillReportForTesting = {
            await replayFailureGate.pause()
            replayHookReturns.increment()
        }
        controller.onFailure = { _ in
            failures.increment()
        }

        try await controller.start()
        try transport.deliver(Self.resumeOK(
            sessionID: "replay-session",
            lastProcessedEventSeq: 0
        ))
        await replayFailureGate.waitUntilPaused()
        #expect(transport.sentFrameCount == 3)

        // Invalidate every ownership component captured by the paused failure callback.
        controller.pendingReplayFailureWillReportForTesting = nil
        await controller.stop()
        #expect(transport.closeCallCount == 1)
        #expect(controller.isDiverged == false)

        try await controller.start()
        #expect(transport.sentFrameCount == 4)
        let secondHandshake = try #require(transport.frame(at: 3))
        guard case .clientResume(let resume) = try decodeFramedMessage(from: secondHandshake).msg else {
            Issue.record("restart did not send CLIENT RESUME")
            return
        }
        #expect(resume.sessionID == "replay-session")

        try transport.deliver(Self.resumeOK(
            sessionID: "replay-session",
            lastProcessedEventSeq: pending.eventSeq
        ))
        try await Self.waitUntil("restarted resume enabled event dispatch") {
            controller.isEventDispatchEnabled
        }

        await replayFailureGate.release()
        try await Self.waitUntil("stale replay callback left its test seam") {
            replayHookReturns.value == 1
        }
        let fresh = try await controller.sendActivate(nodeId: NodeId(8))
        #expect(fresh.eventSeq == pending.eventSeq + 1)
        #expect(failures.value == 0)
        #expect(controller.isDiverged == false)
        #expect(controller.isEventDispatchEnabled)
        #expect(transport.closeCallCount == 1)
        #expect(transport.sentFrameCount == 5)

        await controller.stop()
    }

    @Test(
        "stop retires mount ownership before allowing an equal-revision restart",
        .bug("https://github.com/aizlabs/srui/pull/39"),
        .timeLimit(.minutes(1))
    )
    @MainActor
    func stopRetiresMountBeforeRestartAdmission() async throws {
        let transport = LifecycleRaceTransport()
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
        let controller = SessionController(
            transport: transport,
            applier: applier,
            renderer: renderer
        )
        controller.attachRenderer(renderer)

        try await controller.start()
        try transport.deliver(Self.welcome(sessionID: "mount-session"))
        try transport.deliver(Self.mountTransaction(text: "before stop"))
        try await AsyncTestSupport.eventually(description: "initial tree mounted") {
            applier.lastAppliedRevision == Revision(1)
                && renderer.registry.handle(for: textID) != nil
        }
        let initialHandle = try #require(renderer.registry.handle(for: textID))
        let initialIdentity = ObjectIdentifier(initialHandle.view)

        let retirementGate = LifecycleRaceGate()
        controller.stopWillRetireMountForTesting = {
            await retirementGate.pause()
        }
        let stopTask = Task {
            await controller.stop()
        }
        await retirementGate.waitUntilPaused()

        // Admission must remain closed until the old lifecycle has retired its MainActor mount.
        // A start attempted here is a no-op and cannot publish a new receive task or handshake.
        let sendsBeforePrematureStart = transport.sentFrameCount
        try await controller.start()
        #expect(transport.sentFrameCount == sendsBeforePrematureStart)

        controller.stopWillRetireMountForTesting = nil
        await retirementGate.release()
        await stopTask.value

        try await controller.start()
        #expect(transport.sentFrameCount == sendsBeforePrematureStart + 1)
        let resumeFrame = try #require(transport.frame(at: sendsBeforePrematureStart))
        guard case .clientResume(let resume) = try decodeFramedMessage(from: resumeFrame).msg else {
            Issue.record("equal-revision restart did not send CLIENT RESUME")
            return
        }
        #expect(resume.sessionID == "mount-session")

        try transport.deliver(Self.resumeOK(
            sessionID: "mount-session",
            lastProcessedEventSeq: 0
        ))
        try await AsyncTestSupport.eventually(description: "equal-revision tree remounted") {
            guard controller.isEventDispatchEnabled,
                  let handle = renderer.registry.handle(for: textID) else {
                return false
            }
            return ObjectIdentifier(handle.view) != initialIdentity
        }
        let resumedHandle = try #require(renderer.registry.handle(for: textID))
        let resumedIdentity = ObjectIdentifier(resumedHandle.view)

        try transport.deliver(Self.updateTextTransaction(text: "after restart"))
        try await AsyncTestSupport.eventually(description: "post-restart delta rendered incrementally") {
            guard applier.lastAppliedRevision == Revision(2),
                  let handle = renderer.registry.handle(for: textID),
                  let field = handle.view as? NSTextField else {
                return false
            }
            return field.stringValue == "after restart"
        }

        let updatedHandle = try #require(renderer.registry.handle(for: textID))
        #expect(ObjectIdentifier(updatedHandle.view) == resumedIdentity)
        #expect(controller.isDiverged == false)

        await controller.stop()
    }

    private static func welcome(sessionID: String) -> SRUIMessage {
        HandshakeFixtures.welcomeMessage(sessionId: sessionID)
    }

    private static func resumeOK(
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

    private static func mountTransaction(text: String) -> SRUIMessage {
        var message = SRUIMessage()
        message.transaction = Transaction(
            baseRevision: .initial,
            newRevision: Revision(1),
            operations: [
                .createNode(id: NodeId(1), nodeType: .surface),
                .createNode(
                    id: NodeId(2),
                    nodeType: .text,
                    parentID: NodeId(1),
                    properties: [
                        Property(property: .text, value: .string(text)),
                    ]
                ),
            ]
        ).toWire()
        return message
    }

    private static func updateTextTransaction(text: String) -> SRUIMessage {
        var message = SRUIMessage()
        message.transaction = Transaction(
            baseRevision: Revision(1),
            newRevision: Revision(2),
            operations: [
                .setProperty(
                    id: NodeId(2),
                    property: .text,
                    value: .string(text)
                ),
            ]
        ).toWire()
        return message
    }

    private static func waitUntil(
        _ description: String,
        timeout: Duration = .seconds(2),
        condition: @escaping @Sendable () async -> Bool
    ) async throws {
        try await AsyncTestSupport.eventuallyAsync(
            timeout: timeout,
            description: description,
            condition: condition
        )
    }
}
