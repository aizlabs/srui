//
// SessionLifecycleEventTests.swift
// SRUITests
//
// Deterministic coverage for the low-volume SessionController lifecycle observation stream.
//

import Collections
import Foundation
import Protocol
import RendererAppKit
import SemanticModel
@testable import Session
import Testing
import Terminal
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
    private var sentFrames: [Data] = []
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

    func send(data: Data, logicalClass _: LogicalChannelClass) async throws {
        let sendIndex = withLock { () -> Int in
            let sendIndex = sentFrames.count
            sentFrames.append(data)
            return sendIndex
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

    func sentFrame(at index: Int) -> Data? {
        withLock {
            guard sentFrames.indices.contains(index) else { return nil }
            return sentFrames[index]
        }
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

private final class LifecycleCompletionSignal: @unchecked Sendable {
    private let lock = NSLock()
    private var isSignaled = false
    private var waiter: CheckedContinuation<Void, Never>?

    func signal() {
        let waiter = withLock { () -> CheckedContinuation<Void, Never>? in
            isSignaled = true
            defer { self.waiter = nil }
            return self.waiter
        }
        waiter?.resume()
    }

    func wait() async {
        await withCheckedContinuation { continuation in
            lock.lock()
            if isSignaled {
                lock.unlock()
                continuation.resume()
            } else {
                waiter = continuation
                lock.unlock()
            }
        }
    }

    private func withLock<T>(_ body: () -> T) -> T {
        lock.lock()
        defer { lock.unlock() }
        return body()
    }
}

private final class LifecycleMutationCounter: @unchecked Sendable {
    private let lock = NSLock()
    private var storedValue = 0

    var value: Int {
        lock.lock()
        defer { lock.unlock() }
        return storedValue
    }

    func increment() {
        lock.lock()
        storedValue += 1
        lock.unlock()
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

    @Test(
        "unanswered Terminal resume preserves the checkpoint for a third controller",
        .timeLimit(.minutes(1))
    )
    @MainActor
    func unansweredTerminalResumePreservesCheckpoint() async throws {
        let continuityContext = SessionContinuityContext()
        let outbox = EventOutbox()
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
        let firstTransport = LifecycleEventTransport()
        let firstController = SessionController(
            transport: firstTransport,
            applier: applier,
            outbox: outbox,
            renderer: renderer,
            requiredServerProfiles: [.terminalV1],
            continuityContext: continuityContext
        )
        firstController.attachRenderer(renderer)
        try await firstController.start()

        var mapping = Srui_Protocol_ExtensionNamespaceMapping()
        mapping.extensionUri = terminalProfileURI
        mapping.namespaceID = 3
        var welcome = SRUIServerWelcome()
        welcome.coreVersion = SRUICoreVersion
        welcome.sessionID = "shared-terminal"
        welcome.requiredProfiles = [
            "org.srui.standard-widgets/1",
            terminalProfileURI,
        ]
        welcome.extensionNamespaces = [mapping]
        var welcomeMessage = SRUIMessage()
        welcomeMessage.serverWelcome = welcome
        await firstController.handleIncomingMessage(welcomeMessage)
        await firstController.handleIncomingMessage(
            terminalData(streamID: 9, byteOffset: 0, bytes: [0x78])
        )
        #expect(firstController.isHandshakeComplete)
        #expect(renderer.controlFactory.extensionKind(
            for: TypeRef(namespaceID: 3, localID: 1)
        ) == .terminal)
        #expect(await renderer.terminalSession.snapshot(for: NodeId(9))?.nextOffset == 1)

        await firstController.stop()

        let secondTransport = LifecycleEventTransport()
        let secondController = SessionController(
            transport: secondTransport,
            applier: applier,
            outbox: outbox,
            renderer: renderer,
            sessionId: "shared-terminal",
            requiredServerProfiles: [.terminalV1],
            continuityContext: continuityContext
        )
        secondController.attachRenderer(renderer)
        let failureSignal = LifecycleFailureSignal()
        secondController.onFailure = { failure in
            failureSignal.record(failure)
        }
        try await secondController.start()

        let secondFrame = try #require(secondTransport.sentFrame(at: 0))
        var secondDecoder = SRUIMessageStreamDecoder()
        let secondHandshake = try #require(
            try secondDecoder.appendAndExtract(incoming: secondFrame).first
        )
        guard case .clientResume(let secondResume) = secondHandshake.msg else {
            Issue.record("second controller abandoned Terminal continuity")
            await secondController.stop()
            return
        }
        #expect(secondResume.sessionID == "shared-terminal")
        #expect(secondResume.coreVersion == SRUICoreVersion)
        #expect(secondResume.profiles.contains("org.srui.standard-widgets/1"))
        #expect(secondResume.profiles.contains(terminalProfileURI))
        #expect(secondResume.terminalStreamOffsets[9] == 1)

        secondTransport.finishPeer()
        _ = await failureSignal.next()
        await secondTransport.waitUntilClosed()
        await secondController.stop()

        let thirdTransport = LifecycleEventTransport()
        let thirdController = SessionController(
            transport: thirdTransport,
            applier: applier,
            outbox: outbox,
            renderer: renderer,
            sessionId: "shared-terminal",
            requiredServerProfiles: [.terminalV1],
            continuityContext: continuityContext
        )
        thirdController.attachRenderer(renderer)
        try await thirdController.start()

        let thirdFrame = try #require(thirdTransport.sentFrame(at: 0))
        var thirdDecoder = SRUIMessageStreamDecoder()
        let thirdHandshake = try #require(
            try thirdDecoder.appendAndExtract(incoming: thirdFrame).first
        )
        guard case .clientResume(let thirdResume) = thirdHandshake.msg else {
            Issue.record("third controller sent CLIENT_HELLO after unanswered resume")
            await thirdController.stop()
            return
        }
        #expect(thirdResume.sessionID == "shared-terminal")
        #expect(thirdResume.coreVersion == SRUICoreVersion)
        #expect(thirdResume.profiles.contains(terminalProfileURI))
        #expect(thirdResume.terminalStreamOffsets[9] == 1)
        #expect(renderer.controlFactory.extensionKind(
            for: TypeRef(namespaceID: 3, localID: 1)
        ) == .terminal)

        await thirdController.stop()
    }

    @Test(
        "superseded stop cannot mutate a shared renderer",
        .timeLimit(.minutes(1))
    )
    @MainActor
    func supersededStopCannotMutateSharedRenderer() async throws {
        let continuityContext = SessionContinuityContext()
        let outbox = EventOutbox()
        let renderer = AppKitRenderer()
        let oldController = SessionController(
            transport: LifecycleEventTransport(),
            outbox: outbox,
            renderer: renderer,
            continuityContext: continuityContext
        )
        oldController.attachRenderer(renderer)
        try await oldController.start()
        await oldController.handleIncomingMessage(welcome(sessionID: "shared-renderer"))

        let lifecycleHopGate = AsyncGate()
        await outbox.setNativeTextLifecycleWillHopForTesting {
            await lifecycleHopGate.pause()
        }
        let mutationCounter = LifecycleMutationCounter()
        oldController.nativeDisconnectMutationForTesting = {
            mutationCounter.increment()
        }
        let oldStop = Task {
            await oldController.stop()
        }
        await lifecycleHopGate.waitUntilPaused()

        let newController = SessionController(
            transport: LifecycleEventTransport(),
            outbox: outbox,
            renderer: renderer,
            sessionId: "shared-renderer",
            continuityContext: continuityContext
        )
        newController.attachRenderer(renderer)
        try await newController.start()

        await lifecycleHopGate.release()
        await oldStop.value
        #expect(mutationCounter.value == 0)

        await outbox.setNativeTextLifecycleWillHopForTesting(nil)
        await newController.stop()
    }


    @Test(
        "terminal data paused before authorization cannot mutate after supersession",
        .timeLimit(.minutes(1))
    )
    @MainActor
    func staleTerminalDataCannotMutateSharedSession() async throws {
        let continuityContext = SessionContinuityContext()
        let outbox = EventOutbox()
        let renderer = AppKitRenderer()
        let oldTransport = LifecycleEventTransport()
        let oldController = SessionController(
            transport: oldTransport,
            outbox: outbox,
            renderer: renderer,
            requiredServerProfiles: [.terminalV1],
            continuityContext: continuityContext
        )
        oldController.attachRenderer(renderer)
        try await oldController.start()

        var mapping = Srui_Protocol_ExtensionNamespaceMapping()
        mapping.extensionUri = terminalProfileURI
        mapping.namespaceID = 3
        var welcome = SRUIServerWelcome()
        welcome.coreVersion = SRUICoreVersion
        welcome.sessionID = "terminal-race"
        welcome.requiredProfiles = [
            "org.srui.standard-widgets/1",
            terminalProfileURI,
        ]
        welcome.extensionNamespaces = [mapping]
        var welcomeMessage = SRUIMessage()
        welcomeMessage.serverWelcome = welcome
        await oldController.handleIncomingMessage(welcomeMessage)

        let authorizationGate = AsyncGate()
        oldController.sharedMutationWillAuthorizeForTesting = {
            await authorizationGate.pause()
        }
        let oldDelivery = Task {
            await oldController.handleIncomingMessage(
                terminalData(streamID: 41, byteOffset: 0, bytes: [0x78])
            )
        }
        await authorizationGate.waitUntilPaused()

        let newController = SessionController(
            transport: LifecycleEventTransport(),
            outbox: outbox,
            renderer: renderer,
            sessionId: "terminal-race",
            requiredServerProfiles: [.terminalV1],
            continuityContext: continuityContext
        )
        newController.attachRenderer(renderer)
        try await newController.start()

        await authorizationGate.release()
        await oldDelivery.value
        #expect(await renderer.terminalSession.snapshot(for: NodeId(41)) == nil)

        await oldController.stop()
        await newController.stop()
    }

    @Test(
        "replacement reset paused before authorization cannot clear newer shared state",
        .timeLimit(.minutes(1))
    )
    @MainActor
    func staleReplacementResetCannotClearSharedTerminal() async throws {
        let continuityContext = SessionContinuityContext()
        let outbox = EventOutbox()
        let renderer = AppKitRenderer()
        let oldTransport = LifecycleEventTransport()
        let oldController = SessionController(
            transport: oldTransport,
            outbox: outbox,
            renderer: renderer,
            requiredServerProfiles: [.terminalV1],
            continuityContext: continuityContext
        )
        oldController.attachRenderer(renderer)
        try await oldController.start()

        var mapping = Srui_Protocol_ExtensionNamespaceMapping()
        mapping.extensionUri = terminalProfileURI
        mapping.namespaceID = 3
        var welcome = SRUIServerWelcome()
        welcome.coreVersion = SRUICoreVersion
        welcome.sessionID = "replacement-old"
        welcome.requiredProfiles = [
            "org.srui.standard-widgets/1",
            terminalProfileURI,
        ]
        welcome.extensionNamespaces = [mapping]
        var welcomeMessage = SRUIMessage()
        welcomeMessage.serverWelcome = welcome
        await oldController.handleIncomingMessage(welcomeMessage)
        await oldController.handleIncomingMessage(
            terminalData(streamID: 51, byteOffset: 0, bytes: [0x78])
        )
        #expect(await renderer.terminalSession.snapshot(for: NodeId(51))?.nextOffset == 1)

        let authorizationGate = AsyncGate()
        oldController.sharedMutationWillAuthorizeForTesting = {
            await authorizationGate.pause()
        }
        let oldReplacement = Task {
            await oldController.handleIncomingMessage(
                resync(
                    sessionID: "replacement-new",
                    continuity: .replaced,
                    snapshotRevision: 1,
                    requiredProfiles: [
                        "org.srui.standard-widgets/1",
                        terminalProfileURI,
                    ],
                    extensionNamespaces: [mapping]
                )
            )
        }
        await authorizationGate.waitUntilPaused()

        let newController = SessionController(
            transport: LifecycleEventTransport(),
            outbox: outbox,
            renderer: renderer,
            sessionId: "replacement-old",
            continuityContext: continuityContext
        )
        newController.attachRenderer(renderer)
        try await newController.start()

        await authorizationGate.release()
        await oldReplacement.value
        #expect(await renderer.terminalSession.snapshot(for: NodeId(51))?.nextOffset == 1)

        await oldController.stop()
        await newController.stop()
    }

    @Test(
        "superseded manual action cannot flush the replacement controller's draft",
        .timeLimit(.minutes(1))
    )
    @MainActor
    func supersededManualActionCannotFlushReplacementDraft() async throws {
        let continuityContext = SessionContinuityContext()
        let outbox = EventOutbox()
        let renderer = AppKitRenderer()
        renderer.textEditingSession.debounceNanoseconds = 10_000_000_000
        let oldController = SessionController(
            transport: LifecycleEventTransport(),
            outbox: outbox,
            renderer: renderer,
            continuityContext: continuityContext
        )
        oldController.attachRenderer(renderer)
        try await oldController.start()
        await oldController.handleIncomingMessage(welcome(sessionID: "manual-flush-race"))

        let actionGate = AsyncGate()
        oldController.interactionWillEnterOutboxForTesting = {
            await actionGate.pause()
        }
        let staleAction = Task {
            do {
                _ = try await oldController.sendActivate(nodeId: NodeId(1))
                return false
            } catch {
                return error as? SessionDispatchError == .resumeNotConfirmed
            }
        }
        await actionGate.waitUntilPaused()

        let newController = SessionController(
            transport: LifecycleEventTransport(),
            outbox: outbox,
            renderer: renderer,
            sessionId: "manual-flush-race",
            continuityContext: continuityContext
        )
        newController.attachRenderer(renderer)
        try await newController.start()
        renderer.textEditingSession.noteLocalValue(
            "replacement draft",
            nodeID: NodeId(12),
            composing: false,
            flushImmediately: false
        )

        await actionGate.release()
        #expect(await staleAction.value)
        #expect(renderer.textEditingSession.localValue(for: NodeId(12)) == "replacement draft")
        #expect(renderer.textEditingSession.nextEditSeqValue(for: NodeId(12)) == 1)

        await oldController.stop()
        await newController.stop()
    }

    @Test(
        "superseded queued transaction cannot consume the shared ingress budget",
        .timeLimit(.minutes(1))
    )
    func supersededTransactionCannotConsumeReplacementBudget() async throws {
        let continuityContext = SessionContinuityContext()
        let outbox = EventOutbox()
        let ingressGate = TransactionIngressGate()
        let oldController = SessionController(
            transport: LifecycleEventTransport(),
            outbox: outbox,
            continuityContext: continuityContext,
            transactionIngressGate: ingressGate
        )
        try await oldController.start()
        await oldController.handleIncomingMessage(welcome(sessionID: "ingress-race"))

        let authorizationGate = AsyncGate()
        oldController.sharedMutationWillAuthorizeForTesting = {
            await authorizationGate.pause()
        }
        let staleTransaction = Task {
            await oldController.handleIncomingMessage(
                snapshot(revision: 1, text: "stale")
            )
        }
        await authorizationGate.waitUntilPaused()

        let newController = SessionController(
            transport: LifecycleEventTransport(),
            outbox: outbox,
            sessionId: "ingress-race",
            continuityContext: continuityContext,
            transactionIngressGate: ingressGate
        )
        try await newController.start()

        await authorizationGate.release()
        await staleTransaction.value
        #expect(await newController.availableTransactionIngressCredit == 240)

        oldController.sharedMutationWillAuthorizeForTesting = nil
        await oldController.stop()
        await newController.stop()
    }

    @Test(
        "replacement clears queued Terminal commands before stale authorization resumes",
        .timeLimit(.minutes(1))
    )
    @MainActor
    func replacementClearsStaleTerminalCommands() async throws {
        let continuityContext = SessionContinuityContext()
        let transport = LifecycleEventTransport()
        let renderer = AppKitRenderer()
        let controller = SessionController(
            transport: transport,
            renderer: renderer,
            continuityContext: continuityContext
        )
        controller.attachRenderer(renderer)
        try await controller.start()

        var mapping = Srui_Protocol_ExtensionNamespaceMapping()
        mapping.extensionUri = terminalProfileURI
        mapping.namespaceID = 3
        var terminalWelcome = SRUIServerWelcome()
        terminalWelcome.coreVersion = SRUICoreVersion
        terminalWelcome.sessionID = "terminal-command-old"
        terminalWelcome.requiredProfiles = [
            "org.srui.standard-widgets/1",
            terminalProfileURI,
        ]
        terminalWelcome.extensionNamespaces = [mapping]
        var terminalWelcomeMessage = SRUIMessage()
        terminalWelcomeMessage.serverWelcome = terminalWelcome
        await controller.handleIncomingMessage(terminalWelcomeMessage)

        let sendGate = AsyncGate()
        controller.terminalCommandWillAuthorizeForTesting = {
            await sendGate.pause()
        }
        renderer.onTerminalResize?(NodeId(30), 100, 40, 800, 600)
        await sendGate.waitUntilPaused()

        var replacementMapping = Srui_Protocol_ExtensionNamespaceMapping()
        replacementMapping.extensionUri = terminalProfileURI
        replacementMapping.namespaceID = 4
        await controller.handleIncomingMessage(
            resync(
                sessionID: "terminal-command-new",
                continuity: .replaced,
                snapshotRevision: 1,
                requiredProfiles: [
                    "org.srui.standard-widgets/1",
                    terminalProfileURI,
                ],
                extensionNamespaces: [replacementMapping]
            )
        )
        #expect(await controller.retainedTerminalResizeCountForTesting == 0)

        await sendGate.release()
        await controller.waitForTerminalCommandDrainForTesting()
        #expect(transport.sentFrame(at: 1) == nil)

        controller.terminalCommandWillAuthorizeForTesting = nil
        await controller.stop()
    }

    @Test(
        "queued collection request cannot send through a superseded controller",
        .timeLimit(.minutes(1))
    )
    @MainActor
    func staleCollectionRequestCannotSendAfterSupersession() async throws {
        let continuityContext = SessionContinuityContext()
        let outbox = EventOutbox()
        let renderer = AppKitRenderer()
        let oldTransport = LifecycleEventTransport()
        let oldController = SessionController(
            transport: oldTransport,
            outbox: outbox,
            renderer: renderer,
            continuityContext: continuityContext
        )
        oldController.attachRenderer(renderer)
        try await oldController.start()
        await oldController.handleIncomingMessage(welcome(sessionID: "collection-race"))
        let staleCallback = try #require(renderer.onCollectionRangeRequest)

        let authorizationGate = AsyncGate()
        let completion = LifecycleCompletionSignal()
        oldController.sharedMutationWillAuthorizeForTesting = {
            await authorizationGate.pause()
        }
        oldController.collectionRangeDispatchDidFinishForTesting = {
            completion.signal()
        }
        staleCallback(
            CollectionRangeRequest(
                nodeID: NodeId(20),
                modelID: ModelId(7),
                startIndex: 0,
                count: 10
            )
        )
        await authorizationGate.waitUntilPaused()

        let newController = SessionController(
            transport: LifecycleEventTransport(),
            outbox: outbox,
            renderer: renderer,
            sessionId: "collection-race",
            continuityContext: continuityContext
        )
        newController.attachRenderer(renderer)
        try await newController.start()

        await authorizationGate.release()
        await completion.wait()
        #expect(oldTransport.sentFrame(at: 1) == nil)

        oldController.sharedMutationWillAuthorizeForTesting = nil
        await oldController.stop()
        await newController.stop()
    }

    @Test(
        "binding replacement rolls back an unauthorized prepared text identity",
        .timeLimit(.minutes(1))
    )
    func bindingReplacementRollsBackUnauthorizedPreparedTextEdit() async throws {
        let transport = LifecycleEventTransport()
        let outbox = EventOutbox()
        let oldBinding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: "text-old", binding: oldBinding))
        let editSeq = try #require(EditSeq(1))
        let prepared = try #require(try await outbox.prepareTextEdit(
            nodeId: NodeId(12),
            text: "prepared",
            editSeq: editSeq,
            observedRevision: Revision(1),
            binding: oldBinding,
            via: transport
        ))
        #expect(prepared.event.eventSeq == 1)
        #expect(await outbox.assignedTextEditDescriptors().count == 1)

        let newBinding = await outbox.beginConnectionBinding()
        #expect(await outbox.assignedTextEditDescriptors().isEmpty)
        #expect(await outbox.confirmFreshSession(id: "text-new", binding: newBinding))
        let next = try await outbox.sendActivate(
            nodeId: NodeId(13),
            observedRevision: Revision(1),
            binding: newBinding,
            via: transport
        )
        #expect(next.eventSeq == 1)
    }

    @Test(
        "canceled queued binding acquisition cannot supersede the active binding",
        .timeLimit(.minutes(1))
    )
    func canceledBindingAcquisitionDoesNotStealOwnership() async throws {
        let outbox = EventOutbox()
        let activeBinding = await outbox.beginConnectionBinding()
        let entryGate = AsyncGate()
        let activationCounter = LifecycleMutationCounter()
        let canceledAcquisition = Task {
            await entryGate.pause()
            return try await outbox.beginConnectionBindingUnlessCancelled { _, _ in
                activationCounter.increment()
            }
        }
        await entryGate.waitUntilPaused()
        canceledAcquisition.cancel()
        await entryGate.release()

        await #expect(throws: CancellationError.self) {
            try await canceledAcquisition.value
        }
        #expect(activationCounter.value == 0)
        #expect(await outbox.activeConnectionBindingForTesting == activeBinding)
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
        snapshotRevision: UInt64,
        requiredProfiles: [String] = ["org.srui.standard-widgets/1"],
        extensionNamespaces: [Srui_Protocol_ExtensionNamespaceMapping] = []
    ) -> SRUIMessage {
        var resync = SRUIServerResyncRequired()
        resync.sessionID = sessionID
        resync.snapshotRevision = snapshotRevision
        resync.reason = "lifecycle test"
        resync.continuity = continuity
        resync.requiredProfiles = requiredProfiles
        resync.extensionNamespaces = extensionNamespaces
        var message = SRUIMessage()
        message.serverResyncRequired = resync
        return message
    }

    private func terminalData(
        streamID: UInt64,
        byteOffset: UInt64,
        bytes: [UInt8]
    ) -> SRUIMessage {
        var data = SRUITerminalData()
        data.streamID = streamID
        data.byteOffset = byteOffset
        data.data = Data(bytes)
        var message = SRUIMessage()
        message.terminalData = data
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
