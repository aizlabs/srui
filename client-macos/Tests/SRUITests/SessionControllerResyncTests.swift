//
// SessionControllerResyncTests.swift
// SRUITests
//
// Resync snapshot handling and receive-loop resilience tests (§12.1, §20.2, §22).
//

import Testing
import Foundation
import CryptoKit
import AppKit
import SemanticModel
import Protocol
@testable import Session
import TransportSSH
import RendererAppKit
import Resources

@Suite("SessionController Resync & Resilience Tests")
struct SessionControllerResyncTests {

    @Test("ServerResyncRequired followed by snapshot replaces local state")
    @MainActor
    func resyncSnapshotReplacesLocalState() async throws {
        let (clientTransport, serverTransport) = await PipeTransport.createPair()
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
        let controller = SessionController(
            transport: clientTransport,
            applier: applier,
            renderer: renderer
        )
        controller.attachRenderer(renderer)

        try await controller.start()

        var welcome = SRUIServerWelcome()
        welcome.coreVersion = SRUICoreVersion
        welcome.sessionID = "test-session"
        welcome.requiredProfiles = ["org.srui.standard-widgets/1"]
        var welcomeMsg = SRUIMessage()
        welcomeMsg.serverWelcome = welcome
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(welcomeMsg))

        let surfaceID = NodeId(1)
        let textID = NodeId(2)

        let initialTx = Transaction(
            baseRevision: .initial,
            newRevision: Revision(1),
            operations: [
                .createNode(id: surfaceID, nodeType: .surface),
                .createNode(
                    id: textID,
                    nodeType: .text,
                    parentID: surfaceID,
                    properties: [Property(property: .text, value: .string("Before resync"))]
                ),
            ]
        )

        var initialMsg = SRUIMessage()
        initialMsg.transaction = initialTx.toWire()
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(initialMsg))
        try await AsyncTestSupport.eventually(
            timeout: .seconds(5),
            description: "initial transaction applied"
        ) {
            applier.lastAppliedRevision == Revision(1)
        }

        #expect(applier.lastAppliedRevision == Revision(1))

        var resyncMsg = SRUIMessage()
        var resync = SRUIServerResyncRequired()
        resync.sessionID = "test-session"
        resync.snapshotRevision = 2
        resync.reason = "journal evicted"
        resync.continuity = .sameSession
        resyncMsg.serverResyncRequired = resync
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(resyncMsg))

        let snapshotTx = Transaction(
            baseRevision: .initial,
            newRevision: Revision(2),
            operations: [
                .createNode(id: surfaceID, nodeType: .surface),
                .createNode(
                    id: textID,
                    nodeType: .text,
                    parentID: surfaceID,
                    properties: [Property(property: .text, value: .string("After resync"))]
                ),
            ]
        )

        var snapshotMsg = SRUIMessage()
        snapshotMsg.transaction = snapshotTx.toWire()
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(snapshotMsg))
        // The applier commits ahead of the renderer: the mount is a separate main-actor
        // hop, so poll the painted value instead of sleeping a fixed interval.
        try await AsyncTestSupport.eventually(
            timeout: .seconds(5),
            description: "resync snapshot painted"
        ) {
            (renderer.registry.handle(for: textID)?.view as? NSTextField)?.stringValue
                == "After resync"
        }

        #expect(applier.lastAppliedRevision == Revision(2))
        let textHandle = try #require(renderer.registry.handle(for: textID))
        let textField = try #require(textHandle.view as? NSTextField)
        #expect(textField.stringValue == "After resync")

        await controller.stop()
        await serverTransport.close()
    }

    @Test("Invalid extension resync is rejected before replacing the replica")
    @MainActor
    func invalidExtensionResyncPreservesCommittedReplica() async throws {
        let (clientTransport, serverTransport) = await PipeTransport.createPair()
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
        let failures = ResyncFailureRecorder()
        let controller = SessionController(
            transport: clientTransport,
            applier: applier,
            renderer: renderer
        )
        controller.attachRenderer(renderer)
        controller.onFailure = { failure in
            Task { await failures.record(failure) }
        }

        try await controller.start()

        var welcome = SRUIServerWelcome()
        welcome.coreVersion = SRUICoreVersion
        welcome.sessionID = "invalid-extension-resync"
        welcome.requiredProfiles = ["org.srui.standard-widgets/1"]
        var welcomeMessage = SRUIMessage()
        welcomeMessage.serverWelcome = welcome
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(welcomeMessage))

        let surfaceID = NodeId(1)
        let textID = NodeId(2)
        let initial = Transaction(
            baseRevision: .initial,
            newRevision: Revision(1),
            operations: [
                .createNode(id: surfaceID, nodeType: .surface),
                .createNode(
                    id: textID,
                    nodeType: .text,
                    parentID: surfaceID,
                    properties: [(.text, .string("Retained state"))]
                ),
            ]
        )
        var initialMessage = SRUIMessage()
        initialMessage.transaction = initial.toWire()
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(initialMessage))
        try await AsyncTestSupport.eventually(
            timeout: .seconds(5),
            description: "initial replica committed"
        ) {
            applier.lastAppliedRevision == Revision(1)
        }

        var resync = SRUIServerResyncRequired()
        resync.sessionID = "invalid-extension-resync"
        resync.snapshotRevision = 2
        resync.reason = "force extension validation"
        resync.continuity = .sameSession
        var resyncMessage = SRUIMessage()
        resyncMessage.serverResyncRequired = resync
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(resyncMessage))

        let unsupportedType = TypeRef(namespaceID: 7, localID: 1)
        let invalidSnapshot = Transaction(
            baseRevision: .initial,
            newRevision: Revision(2),
            operations: [
                .createNode(id: NodeId(10), nodeType: .surface),
                .createNode(id: NodeId(11), nodeType: unsupportedType, parentID: NodeId(10)),
            ]
        )
        var snapshotMessage = SRUIMessage()
        snapshotMessage.transaction = invalidSnapshot.toWire()
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(snapshotMessage))

        let failure = await failures.wait()
        guard case .protocolViolation(let description) = failure else {
            Issue.record(
                "invalid extension snapshot must report .protocolViolation, got \(failure)"
            )
            await controller.stop()
            await serverTransport.close()
            return
        }
        #expect(description.contains("unsupported required extension semantics"))
        #expect(applier.lastAppliedRevision == Revision(1))
        let retainedStore = applier.currentSnapshot.store
        #expect(retainedStore.getNode(textID)?.getProperty(.text) == .string("Retained state"))
        #expect(retainedStore.getNode(NodeId(11)) == nil)
        #expect(controller.isDiverged)
        #expect(controller.isEventDispatchEnabled == false)

        await controller.stop()
        await serverTransport.close()
    }

    @Test("Renderer failure during resync fails the session and releases render ownership")
    @MainActor
    func resyncRendererFailureIsTerminal() async throws {
        let (clientTransport, serverTransport) = await PipeTransport.createPair()
        let applier = TransactionApplier()
        let outbox = EventOutbox()
        let renderer = AppKitRenderer()
        let failures = ResyncFailureRecorder()
        let controller = SessionController(
            transport: clientTransport,
            applier: applier,
            outbox: outbox,
            renderer: renderer
        )
        controller.attachRenderer(renderer)
        controller.onFailure = { failure in
            Task { await failures.record(failure) }
        }

        try await controller.start()

        var welcome = SRUIServerWelcome()
        welcome.coreVersion = SRUICoreVersion
        welcome.sessionID = "renderer-failure-session"
        welcome.requiredProfiles = ["org.srui.standard-widgets/1"]
        var welcomeMessage = SRUIMessage()
        welcomeMessage.serverWelcome = welcome
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(welcomeMessage))

        var resync = SRUIServerResyncRequired()
        resync.sessionID = "renderer-failure-session"
        resync.snapshotRevision = 1
        resync.reason = "force snapshot"
        resync.continuity = .sameSession
        var resyncMessage = SRUIMessage()
        resyncMessage.serverResyncRequired = resync
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(resyncMessage))

        let snapshot = Transaction(
            baseRevision: .initial,
            newRevision: Revision(1),
            operations: [
                .createNode(id: NodeId(1), nodeType: .surface),
                .createNode(
                    id: NodeId(2),
                    nodeType: TypeRef.standard(999_999),
                    parentID: NodeId(1)
                ),
            ]
        )
        var snapshotMessage = SRUIMessage()
        snapshotMessage.transaction = snapshot.toWire()
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(snapshotMessage))

        let failure = await failures.wait()
        guard case .rendererFailed(let description) = failure else {
            Issue.record("resync renderer throw must report .rendererFailed, got \(String(describing: failure))")
            await controller.stop()
            await serverTransport.close()
            return
        }
        #expect(description.contains("unsupportedNodeType"))
        #expect(controller.isDiverged)
        #expect(applier.lastAppliedRevision == Revision(1))
        #expect(
            !controller.isEventDispatchEnabled,
            "terminal renderer failure must leave event dispatch closed"
        )

        let followUp = Transaction(
            baseRevision: Revision(1),
            newRevision: Revision(2),
            operations: [
                .setProperty(id: NodeId(2), property: .text, value: .string("ignored")),
            ]
        )
        var followUpMessage = SRUIMessage()
        followUpMessage.transaction = followUp.toWire()
        await controller.handleIncomingMessage(followUpMessage)
        #expect(
            applier.lastAppliedRevision == Revision(1),
            "a renderer-failed session must not misclassify or apply later live deltas"
        )

        await controller.stop()
        await serverTransport.close()
    }

    @Test("RESUME_OK mounts a committed snapshot whose render failed, and repeated failure stays terminal")
    @MainActor
    func resumeOkRecoversCommittedButUnrenderedSnapshot() async throws {
        let applier = TransactionApplier()
        let outbox = EventOutbox()
        let renderer = AppKitRenderer()
        let injector = RendererFailureInjector(remainingFailures: 2)
        let sessionID = "renderer-recovery-session"

        let (firstClient, firstServer) = await PipeTransport.createPair()
        let firstFailures = ResyncFailureRecorder()
        let first = SessionController(
            transport: firstClient,
            applier: applier,
            outbox: outbox,
            renderer: renderer
        )
        first.rendererUpdateInterceptorForTesting = { try injector.failIfNeeded() }
        first.attachRenderer(renderer)
        first.onFailure = { failure in
            Task { await firstFailures.record(failure) }
        }
        try await first.start()

        var welcome = SRUIServerWelcome()
        welcome.coreVersion = SRUICoreVersion
        welcome.sessionID = sessionID
        welcome.requiredProfiles = ["org.srui.standard-widgets/1"]
        var welcomeMessage = SRUIMessage()
        welcomeMessage.serverWelcome = welcome
        try await firstServer.send(data: try SRUIFraming.encodeFramed(welcomeMessage))

        var resync = SRUIServerResyncRequired()
        resync.sessionID = sessionID
        resync.snapshotRevision = 1
        resync.reason = "force snapshot"
        resync.continuity = .sameSession
        var resyncMessage = SRUIMessage()
        resyncMessage.serverResyncRequired = resync
        try await firstServer.send(data: try SRUIFraming.encodeFramed(resyncMessage))

        let snapshot = Transaction(
            baseRevision: .initial,
            newRevision: Revision(1),
            operations: [
                .createNode(id: NodeId(1), nodeType: .surface),
                .createNode(
                    id: NodeId(2),
                    nodeType: .text,
                    parentID: NodeId(1),
                    properties: [Property(property: .text, value: .string("Recovered"))]
                ),
            ]
        )
        var snapshotMessage = SRUIMessage()
        snapshotMessage.transaction = snapshot.toWire()
        try await firstServer.send(data: try SRUIFraming.encodeFramed(snapshotMessage))

        let firstFailure = await firstFailures.wait()
        guard case .rendererFailed = firstFailure else {
            Issue.record("initial resync render must fail, got \(String(describing: firstFailure))")
            return
        }
        #expect(applier.lastAppliedRevision == Revision(1))
        #expect(renderer.registry.handle(for: NodeId(2)) == nil)
        await first.stop()
        await firstServer.close()

        let (retryClient, retryServer) = await PipeTransport.createPair()
        let retryFailures = ResyncFailureRecorder()
        let retry = SessionController(
            transport: retryClient,
            applier: applier,
            outbox: outbox,
            renderer: renderer,
            sessionId: sessionID
        )
        retry.rendererUpdateInterceptorForTesting = { try injector.failIfNeeded() }
        retry.attachRenderer(renderer)
        retry.onFailure = { failure in
            Task { await retryFailures.record(failure) }
        }
        try await retry.start()

        var retryOK = SRUIServerResumeOk()
        retryOK.sessionID = sessionID
        var retryOKMessage = SRUIMessage()
        retryOKMessage.serverResumeOk = retryOK
        try await retryServer.send(data: try SRUIFraming.encodeFramed(retryOKMessage))

        let retryFailure = await retryFailures.wait()
        guard case .rendererFailed(let retryDescription) = retryFailure else {
            Issue.record("repeated recovery render must remain terminal, got \(String(describing: retryFailure))")
            return
        }
        #expect(retryDescription.contains("resume recovery mount failed"))
        #expect(!retry.isEventDispatchEnabled)
        #expect(renderer.registry.handle(for: NodeId(2)) == nil)
        await retry.stop()
        await retryServer.close()

        let (recoveryClient, recoveryServer) = await PipeTransport.createPair()
        let recovery = SessionController(
            transport: recoveryClient,
            applier: applier,
            outbox: outbox,
            renderer: renderer,
            sessionId: sessionID
        )
        recovery.attachRenderer(renderer)
        try await recovery.start()

        var recoveryOK = SRUIServerResumeOk()
        recoveryOK.sessionID = sessionID
        var recoveryOKMessage = SRUIMessage()
        recoveryOKMessage.serverResumeOk = recoveryOK
        try await recoveryServer.send(data: try SRUIFraming.encodeFramed(recoveryOKMessage))

        try await AsyncTestSupport.eventually(
            timeout: .seconds(5),
            description: "committed snapshot mounted before resume dispatch"
        ) {
            recovery.isEventDispatchEnabled
                && (renderer.registry.handle(for: NodeId(2))?.view as? NSTextField)?.stringValue
                    == "Recovered"
        }
        #expect(applier.lastAppliedRevision == Revision(1))
        #expect(recovery.isEventDispatchEnabled)

        await recovery.stop()
        await recoveryServer.close()
    }

    @Test("RESUME_OK promotes a ready coalesced text draft after renderer recovery")
    @MainActor
    func resumeOkPromotesReadyDraftAfterRendererRecovery() async throws {
        let sessionID = "resume-draft-session"
        let nodeID = NodeId(2)
        let outbox = EventOutbox()
        let binding = await outbox.beginConnectionBinding()
        #expect(await outbox.confirmFreshSession(id: sessionID, binding: binding))
        let (seedClient, seedServer) = await PipeTransport.createPair()

        let renderer = AppKitRenderer()
        #expect(
            renderer.textEditingSession.applyPublishedValue(
                nodeID: nodeID,
                published: "authoritative"
            ) == .apply
        )
        var committedEdits: [(text: String, editSeq: EditSeq, laneEpoch: UInt64)] = []
        renderer.textEditingSession.onCommit = { _, text, editSeq, laneEpoch in
            committedEdits.append((text, editSeq, laneEpoch))
        }

        renderer.textEditingSession.noteLocalValue(
            "first",
            nodeID: nodeID,
            composing: false,
            flushImmediately: true
        )
        let firstCommit = try #require(committedEdits.last)
        #expect(renderer.textEditingSession.recordObservedRevision(
            nodeID: nodeID,
            text: firstCommit.text,
            editSeq: firstCommit.editSeq,
            laneEpoch: firstCommit.laneEpoch,
            observedRevision: Revision(1)
        ))
        let prepared = try #require(try await outbox.prepareTextEdit(
            nodeId: nodeID,
            text: firstCommit.text,
            editSeq: firstCommit.editSeq,
            observedRevision: Revision(1),
            binding: binding,
            via: seedClient
        ))
        renderer.textEditingSession.noteAssigned(prepared.event)
        #expect(await outbox.authorizePreparedTextEdit(prepared))
        let first = try #require(try await outbox.releasePreparedTextEdit(prepared))

        renderer.textEditingSession.noteLocalValue(
            "second",
            nodeID: nodeID,
            composing: false,
            flushImmediately: true
        )
        let secondCommit = try #require(committedEdits.last)
        let secondEditSeq = secondCommit.editSeq
        #expect(renderer.textEditingSession.recordObservedRevision(
            nodeID: nodeID,
            text: secondCommit.text,
            editSeq: secondCommit.editSeq,
            laneEpoch: secondCommit.laneEpoch,
            observedRevision: Revision(1)
        ))
        #expect(renderer.textEditingSession.hasUnsentSuccessorDraft(for: nodeID))

        let settlement = await outbox.settleAcknowledgement(
            binding: binding,
            clientInstanceId: outbox.clientInstanceId,
            eventId: first.eventId,
            throughSeq: first.eventSeq,
            sessionId: sessionID
        )
        #expect(settlement.bound)
        renderer.textEditingSession.noteAcknowledged(first)
        #expect(await outbox.pendingCount == 0)

        let applier = TransactionApplier()
        let committed = Transaction(
            baseRevision: .initial,
            newRevision: Revision(1),
            operations: [
                .createNode(id: NodeId(1), nodeType: .surface),
                .createNode(
                    id: nodeID,
                    nodeType: .textInput,
                    parentID: NodeId(1),
                    properties: [Property(property: .value, value: .string("authoritative"))]
                ),
            ]
        )
        guard case .success = applier.applyCommitted(record: committed) else {
            Issue.record("failed to seed committed snapshot")
            return
        }

        let (client, server) = await PipeTransport.createPair()
        let collector = ResyncWireCollector()
        await collector.start(draining: server)
        let controller = SessionController(
            transport: client,
            applier: applier,
            outbox: outbox,
            renderer: renderer,
            sessionId: sessionID
        )
        controller.attachRenderer(renderer)
        try await controller.start()
        _ = await collector.wait(forAtLeast: 1)
        #expect(renderer.registry.handle(for: nodeID) == nil)

        var resumeOK = SRUIServerResumeOk()
        resumeOK.sessionID = sessionID
        resumeOK.lastProcessedEventSeq = first.eventSeq
        var resumeOKMessage = SRUIMessage()
        resumeOKMessage.serverResumeOk = resumeOK
        await controller.handleIncomingMessage(resumeOKMessage)

        let messages = await collector.wait(forAtLeast: 2)
        let wireEvent = try #require(messages.compactMap { message -> SRUIEvent? in
            guard case .event(let event) = message.msg else { return nil }
            return event
        }.last)
        let promoted = try ProtocolDecoder().validateAndConvertEvent(wire: wireEvent)
        #expect(promoted.eventType == .EVENT_TEXT_EDIT)
        #expect(promoted.eventSeq == first.eventSeq + 1)
        #expect(promoted.editSeq == secondEditSeq)
        #expect(promoted.textArg == "second")
        #expect(renderer.textEditingSession.hasUnsentSuccessorDraft(for: nodeID) == false)
        #expect(controller.isEventDispatchEnabled)
        let field = try #require(renderer.registry.handle(for: nodeID)?.view as? NSTextField)
        #expect(field.stringValue == "second")

        await controller.stop()
        await collector.stop()
        await seedClient.close()
        await seedServer.close()
        await server.close()
    }

    @Test("RESUME_OK recovery preserves post-boundary native text until synchronization")
    @MainActor
    func resumeOkRecoveryPreservesPostBoundaryNativeText() async throws {
        let sessionID = "resume-local-text-session"
        let surfaceID = NodeId(1)
        let editorID = NodeId(2)
        let applier = TransactionApplier()
        let snapshot = Transaction(
            baseRevision: .initial,
            newRevision: Revision(1),
            operations: [
                .createNode(id: surfaceID, nodeType: .surface),
                .createNode(
                    id: editorID,
                    nodeType: .textInput,
                    parentID: surfaceID,
                    properties: [Property(property: .value, value: .string("server"))]
                ),
            ]
        )
        guard case .success = applier.applyCommitted(record: snapshot) else {
            Issue.record("failed to seed recovery snapshot")
            return
        }

        let renderer = AppKitRenderer()
        try renderer.attach(store: applier.currentSnapshot.store)
        renderer.showWindows()
        renderer.textEditingSession.debounceNanoseconds = 60_000_000_000

        let outbox = EventOutbox()
        let (client, server) = await PipeTransport.createPair()
        let collector = ResyncWireCollector()
        await collector.start(draining: server)
        let controller = SessionController(
            transport: client,
            applier: applier,
            outbox: outbox,
            renderer: renderer,
            sessionId: sessionID
        )
        controller.attachRenderer(renderer)
        try await controller.start()
        _ = await collector.wait(forAtLeast: 1)

        let initialHandle = try #require(renderer.registry.handle(for: editorID))
        let initialField = try #require(initialHandle.view as? NSTextField)
        let initialAdapter = try #require(initialHandle.textAdapter)
        initialField.stringValue = "post-boundary local"
        initialAdapter.notifyTextDidChangeForTests()
        #expect(initialField.stringValue == "post-boundary local")

        var resumeOK = SRUIServerResumeOk()
        resumeOK.sessionID = sessionID
        var resumeOKMessage = SRUIMessage()
        resumeOKMessage.serverResumeOk = resumeOK
        await controller.handleIncomingMessage(resumeOKMessage)

        let recoveredField = try #require(
            renderer.registry.handle(for: editorID)?.view as? NSTextField
        )
        #expect(recoveredField.stringValue == "post-boundary local")
        #expect(controller.isEventDispatchEnabled)
        #expect(
            applier.currentSnapshot.store.getNode(editorID)?.getProperty(.value)?.asString
                == "server"
        )

        renderer.textEditingSession.flushAllPending()
        let messages = await collector.wait(forAtLeast: 2)
        let wireEdit = try #require(messages.compactMap { message -> SRUIEvent? in
            guard case .event(let event) = message.msg else { return nil }
            return event
        }.last)
        let edit = try ProtocolDecoder().validateAndConvertEvent(wire: wireEdit)
        #expect(edit.eventType == .EVENT_TEXT_EDIT)
        #expect(edit.textArg == "post-boundary local")

        await controller.stop()
        await collector.stop()
        await server.close()
    }

    @Test("A rebind after recovery preload prevents the stale AppKit remount")
    @MainActor
    func rebindAfterRecoveryPreloadPreventsStaleRemount() async throws {
        let sessionID = "recovery-preload-binding-session"
        let surfaceID = NodeId(1)
        let editorID = NodeId(2)
        let applier = TransactionApplier()
        let staleSnapshot = Transaction(
            baseRevision: .initial,
            newRevision: Revision(1),
            operations: [
                .createNode(id: surfaceID, nodeType: .surface),
                .createNode(
                    id: editorID,
                    nodeType: .textInput,
                    parentID: surfaceID,
                    properties: [Property(property: .value, value: .string("stale"))]
                ),
            ]
        )
        guard case .success = applier.applyCommitted(record: staleSnapshot) else {
            Issue.record("failed to seed stale recovery snapshot")
            return
        }

        let renderer = AppKitRenderer()
        try renderer.attach(store: applier.currentSnapshot.store)
        renderer.showWindows()
        let initialField = try #require(
            renderer.registry.handle(for: editorID)?.view as? NSTextField
        )
        let window = try #require(initialField.window)
        #expect(window.makeFirstResponder(initialField))
        let fieldEditor = try #require(initialField.currentEditor() as? NSTextView)
        let originalSelection = NSRange(location: 1, length: 2)
        fieldEditor.setSelectedRange(originalSelection)

        let outbox = EventOutbox()
        let preloaded = LiveRenderGate()
        let oldFailures = ResyncFailureRecorder()
        let (oldClient, oldServer) = await PipeTransport.createPair()
        let old = SessionController(
            transport: oldClient,
            applier: applier,
            outbox: outbox,
            renderer: renderer,
            sessionId: sessionID
        )
        old.attachRenderer(renderer)
        old.rendererResourcesPreloadedInterceptorForTesting = {
            await preloaded.pauseAfterPublish()
        }
        old.onFailure = { failure in
            Task { await oldFailures.record(failure) }
        }
        try await old.start()

        var oldResumeOK = SRUIServerResumeOk()
        oldResumeOK.sessionID = sessionID
        var oldResumeOKMessage = SRUIMessage()
        oldResumeOKMessage.serverResumeOk = oldResumeOK
        let staleRecovery = Task {
            await old.handleIncomingMessage(oldResumeOKMessage)
        }
        await preloaded.waitUntilPaused()

        let (replacementClient, replacementServer) = await PipeTransport.createPair()
        let replacement = SessionController(
            transport: replacementClient,
            applier: applier,
            outbox: outbox,
            renderer: renderer,
            sessionId: sessionID
        )
        replacement.attachRenderer(renderer)
        try await replacement.start()

        // The old snapshot is already preloaded, but rebinding invalidates its render token before
        // the synchronous MainActor mount. The existing AppKit control and selection stay intact.
        #expect(renderer.registry.handle(for: editorID)?.view === initialField)
        #expect((initialField.currentEditor() as? NSTextView)?.selectedRange == originalSelection)

        var resync = SRUIServerResyncRequired()
        resync.sessionID = sessionID
        resync.snapshotRevision = 2
        resync.reason = "replacement owns recovery render"
        resync.continuity = .sameSession
        var resyncMessage = SRUIMessage()
        resyncMessage.serverResyncRequired = resync
        await replacement.handleIncomingMessage(resyncMessage)

        let replacementSnapshot = Transaction(
            baseRevision: .initial,
            newRevision: Revision(2),
            operations: [
                .createNode(id: surfaceID, nodeType: .surface),
                .createNode(
                    id: editorID,
                    nodeType: .textInput,
                    parentID: surfaceID,
                    properties: [Property(property: .value, value: .string("replacement"))]
                ),
            ]
        )
        var replacementSnapshotMessage = SRUIMessage()
        replacementSnapshotMessage.transaction = replacementSnapshot.toWire()
        await replacement.handleIncomingMessage(replacementSnapshotMessage)
        #expect(
            (renderer.registry.handle(for: editorID)?.view as? NSTextField)?.stringValue
                == "replacement"
        )

        await preloaded.release()
        await staleRecovery.value
        let failure = await oldFailures.wait()
        guard case .superseded = failure else {
            Issue.record("stale recovery render must report supersession, got \(String(describing: failure))")
            return
        }
        #expect(
            (renderer.registry.handle(for: editorID)?.view as? NSTextField)?.stringValue
                == "replacement"
        )

        await old.stop()
        await replacement.stop()
        await oldServer.close()
        await replacementServer.close()
    }

    @Test("A distinct outbox owner rejects a stale render after preload")
    @MainActor
    func distinctOutboxOwnerRejectsStaleRenderAfterPreload() async throws {
        let sessionID = "distinct-outbox-render-owner"
        let surfaceID = NodeId(1)
        let editorID = NodeId(2)
        let applier = TransactionApplier()
        let staleSnapshot = Transaction(
            baseRevision: .initial,
            newRevision: Revision(1),
            operations: [
                .createNode(id: surfaceID, nodeType: .surface),
                .createNode(
                    id: editorID,
                    nodeType: .textInput,
                    parentID: surfaceID,
                    properties: [Property(property: .value, value: .string("stale"))]
                ),
            ]
        )
        guard case .success = applier.applyCommitted(record: staleSnapshot) else {
            Issue.record("failed to seed stale recovery snapshot")
            return
        }

        let renderer = AppKitRenderer()
        try renderer.attach(store: applier.currentSnapshot.store)
        renderer.showWindows()
        let cache = ResourceCache()
        let preloaded = LiveRenderGate()
        let staleFailures = ResyncFailureRecorder()

        let staleOutbox = EventOutbox()
        let (staleClient, staleServer) = await PipeTransport.createPair()
        let staleController = SessionController(
            transport: staleClient,
            applier: applier,
            outbox: staleOutbox,
            renderer: renderer,
            resourceCache: cache,
            sessionId: sessionID
        )
        staleController.attachRenderer(renderer)
        staleController.rendererResourcesPreloadedInterceptorForTesting = {
            await preloaded.pauseAfterPublish()
        }
        staleController.onFailure = { failure in
            Task { await staleFailures.record(failure) }
        }
        try await staleController.start()

        var resumeOK = SRUIServerResumeOk()
        resumeOK.sessionID = sessionID
        var resumeOKMessage = SRUIMessage()
        resumeOKMessage.serverResumeOk = resumeOK
        let staleRecovery = Task {
            await staleController.handleIncomingMessage(resumeOKMessage)
        }
        await preloaded.waitUntilPaused()

        let replacementOutbox = EventOutbox()
        let (replacementClient, replacementServer) = await PipeTransport.createPair()
        let replacementController = SessionController(
            transport: replacementClient,
            applier: applier,
            outbox: replacementOutbox,
            renderer: renderer,
            resourceCache: cache,
            sessionId: sessionID
        )
        replacementController.attachRenderer(renderer)
        try await replacementController.start()

        var resync = SRUIServerResyncRequired()
        resync.sessionID = sessionID
        resync.snapshotRevision = 2
        resync.reason = "new resource owner"
        resync.continuity = .sameSession
        var resyncMessage = SRUIMessage()
        resyncMessage.serverResyncRequired = resync
        await replacementController.handleIncomingMessage(resyncMessage)

        let replacementSnapshot = Transaction(
            baseRevision: .initial,
            newRevision: Revision(2),
            operations: [
                .createNode(id: surfaceID, nodeType: .surface),
                .createNode(
                    id: editorID,
                    nodeType: .textInput,
                    parentID: surfaceID,
                    properties: [Property(property: .value, value: .string("replacement"))]
                ),
            ]
        )
        var replacementSnapshotMessage = SRUIMessage()
        replacementSnapshotMessage.transaction = replacementSnapshot.toWire()
        await replacementController.handleIncomingMessage(replacementSnapshotMessage)
        #expect(
            (renderer.registry.handle(for: editorID)?.view as? NSTextField)?.stringValue
                == "replacement"
        )

        await preloaded.release()
        await staleRecovery.value
        let failure = await staleFailures.wait()
        guard case .superseded = failure else {
            Issue.record(
                "stale distinct-outbox render must report supersession, got \(String(describing: failure))"
            )
            return
        }
        #expect(
            (renderer.registry.handle(for: editorID)?.view as? NSTextField)?.stringValue
                == "replacement"
        )

        await staleController.stop()
        await replacementController.stop()
        await staleServer.close()
        await replacementServer.close()
    }

    @Test("A stale recovery preload cannot remount or evict replacement resources after rebinding")
    @MainActor
    func staleRecoveryPreloadCannotOverwriteReplacementOrEvictItsResource() async throws {
        let limits = ResourceLimits(maxCommittedEntries: 2)
        let cache = ResourceCache(limits: limits)
        let staleHash = try await commitPNG(
            makeOnePixelPNG(NSColor(deviceRed: 1, green: 0, blue: 0, alpha: 1)),
            into: cache
        )
        let currentHash = try await commitPNG(
            makeOnePixelPNG(NSColor(deviceRed: 0, green: 1, blue: 0, alpha: 1)),
            into: cache
        )
        let pressureBytes = try makeOnePixelPNG(
            NSColor(deviceRed: 0, green: 0, blue: 1, alpha: 1)
        )

        let sessionID = "recovery-render-binding-session"
        let surfaceID = NodeId(1)
        let editorID = NodeId(2)
        let imageID = NodeId(3)
        let applier = TransactionApplier()
        let staleSnapshot = Transaction(
            baseRevision: .initial,
            newRevision: Revision(1),
            operations: [
                .createNode(id: surfaceID, nodeType: .surface),
                .createNode(
                    id: editorID,
                    nodeType: .textInput,
                    parentID: surfaceID,
                    properties: [Property(property: .value, value: .string("seed"))]
                ),
                .createNode(
                    id: imageID,
                    nodeType: .image,
                    parentID: surfaceID,
                    properties: [Property(property: .resource, value: .resourceHash(staleHash))]
                ),
            ]
        )
        guard case .success = applier.applyCommitted(record: staleSnapshot) else {
            Issue.record("failed to seed stale recovery snapshot")
            return
        }

        let renderer = AppKitRenderer()
        try renderer.attach(store: applier.currentSnapshot.store)
        renderer.showWindows()
        let initialField = try #require(
            renderer.registry.handle(for: editorID)?.view as? NSTextField
        )
        let window = try #require(initialField.window)
        #expect(window.makeFirstResponder(initialField))
        let fieldEditor = try #require(initialField.currentEditor() as? NSTextView)
        let originalSelection = NSRange(location: 1, length: 2)
        fieldEditor.setSelectedRange(originalSelection)

        let outbox = EventOutbox()
        let beforePreload = LiveRenderGate()
        let oldFailures = ResyncFailureRecorder()
        let (oldClient, oldServer) = await PipeTransport.createPair()
        let old = SessionController(
            transport: oldClient,
            applier: applier,
            outbox: outbox,
            renderer: renderer,
            resourceCache: cache,
            sessionId: sessionID
        )
        old.attachRenderer(renderer)
        old.resumeRecoverySnapshotLoadedInterceptorForTesting = {
            await beforePreload.pauseAfterPublish()
        }
        old.onFailure = { failure in
            Task { await oldFailures.record(failure) }
        }
        try await old.start()

        var oldResumeOK = SRUIServerResumeOk()
        oldResumeOK.sessionID = sessionID
        var oldResumeOKMessage = SRUIMessage()
        oldResumeOKMessage.serverResumeOk = oldResumeOK
        let staleRecovery = Task {
            await old.handleIncomingMessage(oldResumeOKMessage)
        }
        await beforePreload.waitUntilPaused()

        let (replacementClient, replacementServer) = await PipeTransport.createPair()
        let replacement = SessionController(
            transport: replacementClient,
            applier: applier,
            outbox: outbox,
            renderer: renderer,
            resourceCache: cache,
            sessionId: sessionID
        )
        replacement.attachRenderer(renderer)
        try await replacement.start()

        // Rebinding invalidates the recovery lease before any remount. The surviving native
        // control and its AppKit-owned selection therefore remain untouched at this point.
        #expect(renderer.registry.handle(for: editorID)?.view === initialField)
        #expect((initialField.currentEditor() as? NSTextView)?.selectedRange == originalSelection)

        var resync = SRUIServerResyncRequired()
        resync.sessionID = sessionID
        resync.snapshotRevision = 2
        resync.reason = "replacement owns recovery render"
        resync.continuity = .sameSession
        var resyncMessage = SRUIMessage()
        resyncMessage.serverResyncRequired = resync
        await replacement.handleIncomingMessage(resyncMessage)

        let replacementSnapshot = Transaction(
            baseRevision: .initial,
            newRevision: Revision(2),
            operations: [
                .createNode(id: surfaceID, nodeType: .surface),
                .createNode(
                    id: editorID,
                    nodeType: .textInput,
                    parentID: surfaceID,
                    properties: [Property(property: .value, value: .string("replacement"))]
                ),
                .createNode(
                    id: imageID,
                    nodeType: .image,
                    parentID: surfaceID,
                    properties: [Property(property: .resource, value: .resourceHash(currentHash))]
                ),
            ]
        )
        var replacementSnapshotMessage = SRUIMessage()
        replacementSnapshotMessage.transaction = replacementSnapshot.toWire()
        await replacement.handleIncomingMessage(replacementSnapshotMessage)
        #expect(
            (renderer.registry.handle(for: editorID)?.view as? NSTextField)?.stringValue
                == "replacement"
        )
        #expect(renderer.resolveResourceImage(currentHash) != nil)
        #expect(await cache.contains(currentHash))

        await beforePreload.release()
        await staleRecovery.value
        let failure = await oldFailures.wait()
        guard case .superseded = failure else {
            Issue.record("stale recovery render must report supersession, got \(String(describing: failure))")
            return
        }

        let pressureHash = try ResourceHash(
            rawBytes: Array(SHA256.hash(data: pressureBytes))
        )
        await replacement.handleIncomingMessage(
            makeResourceMetadataMessage(hash: pressureHash, bytes: pressureBytes)
        )
        await replacement.handleIncomingMessage(
            makeResourceChunkMessage(hash: pressureHash, bytes: pressureBytes)
        )
        let retainedCurrent = await cache.contains(currentHash)
        let retainedPressure = await cache.contains(pressureHash)
        let evictedStale = !(await cache.contains(staleHash))
        #expect(retainedCurrent)
        #expect(retainedPressure)
        #expect(
            evictedStale,
            "the replacement snapshot's live hash must remain pinned under capacity pressure"
        )

        #expect(
            (renderer.registry.handle(for: editorID)?.view as? NSTextField)?.stringValue
                == "replacement"
        )
        #expect(renderer.resolveResourceImage(currentHash) != nil)

        await old.stop()
        await replacement.stop()
        await oldServer.close()
        await replacementServer.close()
    }

    @Test("A render resource lease closes the old-pin to new-render eviction window")
    @MainActor
    func renderResourceLeaseProtectsJustRenderedHash() async throws {
        let cache = ResourceCache(limits: ResourceLimits(maxCommittedEntries: 3))
        let nextHash = try await commitPNG(
            makeOnePixelPNG(NSColor(deviceRed: 0, green: 1, blue: 0, alpha: 1)),
            into: cache
        )
        let initialHash = try await commitPNG(
            makeOnePixelPNG(NSColor(deviceRed: 1, green: 0, blue: 0, alpha: 1)),
            into: cache
        )
        let evictableHash = try await commitPNG(
            makeOnePixelPNG(NSColor(deviceRed: 0, green: 0, blue: 1, alpha: 1)),
            into: cache
        )
        let pressureBytes = try makeOnePixelPNG(
            NSColor(deviceRed: 1, green: 1, blue: 0, alpha: 1)
        )
        let pressureHash = try ResourceHash(
            rawBytes: Array(SHA256.hash(data: pressureBytes))
        )

        let (client, server) = await PipeTransport.createPair()
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
        let controller = SessionController(
            transport: client,
            applier: applier,
            renderer: renderer,
            resourceCache: cache
        )
        controller.attachRenderer(renderer)
        try await controller.start()

        var welcome = SRUIServerWelcome()
        welcome.coreVersion = SRUICoreVersion
        welcome.sessionID = "resource-render-lease-session"
        welcome.requiredProfiles = ["org.srui.standard-widgets/1"]
        var welcomeMessage = SRUIMessage()
        welcomeMessage.serverWelcome = welcome
        await controller.handleIncomingMessage(welcomeMessage)

        let surfaceID = NodeId(1)
        let imageID = NodeId(2)
        let initial = Transaction(
            baseRevision: .initial,
            newRevision: Revision(1),
            operations: [
                .createNode(id: surfaceID, nodeType: .surface),
                .createNode(
                    id: imageID,
                    nodeType: .image,
                    parentID: surfaceID,
                    properties: [
                        Property(property: .resource, value: .resourceHash(initialHash)),
                    ]
                ),
            ]
        )
        var initialMessage = SRUIMessage()
        initialMessage.transaction = initial.toWire()
        await controller.handleIncomingMessage(initialMessage)
        #expect(renderer.resolveResourceImage(initialHash) != nil)

        var metadata = SRUIResourceMetadata()
        metadata.resourceHash = pressureHash.bytes
        metadata.mediaType = "image/png"
        metadata.encodedLength = UInt64(pressureBytes.count)
        metadata.decodedWidth = 1
        metadata.decodedHeight = 1
        var metadataMessage = SRUIMessage()
        metadataMessage.resourceMetadata = metadata
        await controller.handleIncomingMessage(metadataMessage)

        let resourceGate = LiveRenderGate()
        controller.resourceReferencesSynchronizedInterceptorForTesting = {
            await resourceGate.pauseAfterPublish()
        }
        var chunk = SRUIResourceChunk()
        chunk.resourceHash = pressureHash.bytes
        chunk.byteOffset = 0
        chunk.data = pressureBytes
        var chunkMessage = SRUIMessage()
        chunkMessage.resourceChunk = chunk
        let pressureIngest = Task {
            await controller.handleIncomingMessage(chunkMessage)
        }
        await resourceGate.waitUntilPaused()

        let renderGate = LiveRenderGate()
        controller.rendererDidRenderInterceptorForTesting = {
            await renderGate.pauseAfterPublish()
        }
        let changeResource = Transaction(
            baseRevision: Revision(1),
            newRevision: Revision(2),
            operations: [
                .setProperty(
                    id: imageID,
                    property: .resource,
                    value: .resourceHash(nextHash)
                ),
            ]
        )
        var changeMessage = SRUIMessage()
        changeMessage.transaction = changeResource.toWire()
        let liveRender = Task {
            await controller.handleIncomingMessage(changeMessage)
        }
        await renderGate.waitUntilPaused()

        let imageView = try #require(
            renderer.registry.handle(for: imageID)?.view as? NSImageView
        )
        #expect(renderer.resolveResourceImage(nextHash) != nil)
        #expect(imageView.image != nil)

        // The resource handler captured the old baseline before revision 2 rendered. Ingestion now
        // runs while the exact revision-2 render token temporarily pins nextHash.
        await resourceGate.release()
        await pressureIngest.value

        let retainedNext = await cache.contains(nextHash)
        let retainedPressure = await cache.contains(pressureHash)
        let removedEvictable = !(await cache.contains(evictableHash))
        #expect(retainedNext)
        #expect(retainedPressure)
        #expect(removedEvictable)
        #expect(renderer.resolveResourceImage(nextHash) != nil)
        #expect(imageView.image != nil)

        await renderGate.release()
        await liveRender.value
        #expect(applier.lastAppliedRevision == Revision(2))
        #expect(renderer.resolveResourceImage(nextHash) != nil)

        await controller.stop()
        await server.close()
    }

    @Test("A stale resource chunk from a distinct outbox cannot discard the current assembly")
    @MainActor
    func distinctOutboxCannotDiscardCurrentResourceAssembly() async throws {
        let cache = ResourceCache()
        let bytes = try makeOnePixelPNG(
            NSColor(deviceRed: 0.2, green: 0.4, blue: 0.8, alpha: 1)
        )
        let hash = try ResourceHash(rawBytes: Array(SHA256.hash(data: bytes)))

        let (oldClient, oldServer) = await PipeTransport.createPair()
        let old = SessionController(
            transport: oldClient,
            outbox: EventOutbox(),
            resourceCache: cache
        )
        try await old.start()
        await old.handleIncomingMessage(makeWelcomeMessage(sessionID: "old-resource-owner"))

        let staleGate = LiveRenderGate()
        old.resourceReferencesSynchronizedInterceptorForTesting = {
            await staleGate.pauseAfterPublish()
        }
        let staleChunk = Task {
            await old.handleIncomingMessage(
                makeResourceChunkMessage(
                    hash: hash,
                    byteOffset: 1,
                    bytes: Data([0])
                )
            )
        }
        await staleGate.waitUntilPaused()

        let (currentClient, currentServer) = await PipeTransport.createPair()
        let current = SessionController(
            transport: currentClient,
            outbox: EventOutbox(),
            resourceCache: cache
        )
        try await current.start()
        await current.handleIncomingMessage(makeWelcomeMessage(sessionID: "current-resource-owner"))
        await current.handleIncomingMessage(
            makeResourceMetadataMessage(hash: hash, bytes: bytes)
        )

        // Stopping the stale owner must not clear the current owner's partial assembly.
        await old.stop()
        await staleGate.release()
        await staleChunk.value
        await current.handleIncomingMessage(
            makeResourceChunkMessage(hash: hash, bytes: bytes)
        )
        #expect(await cache.contains(hash))

        // Stopping the active owner does retire its own partial assembly.
        let pendingBytes = try makeOnePixelPNG(
            NSColor(deviceRed: 0.7, green: 0.3, blue: 0.6, alpha: 1)
        )
        let pendingHash = try ResourceHash(
            rawBytes: Array(SHA256.hash(data: pendingBytes))
        )
        let retainedBeforePartial = await cache.retainedBytes()
        await current.handleIncomingMessage(
            makeResourceMetadataMessage(hash: pendingHash, bytes: pendingBytes)
        )
        #expect(await cache.retainedBytes() > retainedBeforePartial)
        await current.stop()
        #expect(await cache.retainedBytes() == retainedBeforePartial)

        await oldServer.close()
        await currentServer.close()
    }

    @Test("A delayed resource commit from a distinct outbox cannot mutate the current renderer")
    @MainActor
    func distinctOutboxCannotDispatchStaleResourceCommit() async throws {
        let cache = ResourceCache(limits: ResourceLimits(maxCommittedEntries: 1))
        let victimBytes = try makeOnePixelPNG(
            NSColor(deviceRed: 0.9, green: 0.1, blue: 0.1, alpha: 1)
        )
        let victimHash = try await commitPNG(victimBytes, into: cache)
        let victimImage = try #require(await cache.lookup(victimHash))
        let pressureBytes = try makeOnePixelPNG(
            NSColor(deviceRed: 0.1, green: 0.9, blue: 0.1, alpha: 1)
        )
        let pressureHash = try ResourceHash(
            rawBytes: Array(SHA256.hash(data: pressureBytes))
        )
        let renderer = AppKitRenderer()
        renderer.commitResourceImage(victimImage)

        let (oldClient, oldServer) = await PipeTransport.createPair()
        let old = SessionController(
            transport: oldClient,
            outbox: EventOutbox(),
            renderer: renderer,
            resourceCache: cache
        )
        old.attachRenderer(renderer)
        try await old.start()
        await old.handleIncomingMessage(makeWelcomeMessage(sessionID: "old-resource-commit"))
        await old.handleIncomingMessage(
            makeResourceMetadataMessage(hash: pressureHash, bytes: pressureBytes)
        )

        let commitGate = LiveRenderGate()
        old.resourceCommitReadyInterceptorForTesting = {
            await commitGate.pauseAfterPublish()
        }
        let staleCommit = Task {
            await old.handleIncomingMessage(
                makeResourceChunkMessage(hash: pressureHash, bytes: pressureBytes)
            )
        }
        await commitGate.waitUntilPaused()

        let (currentClient, currentServer) = await PipeTransport.createPair()
        let current = SessionController(
            transport: currentClient,
            outbox: EventOutbox(),
            renderer: renderer,
            resourceCache: cache
        )
        current.attachRenderer(renderer)
        try await current.start()
        await current.handleIncomingMessage(makeWelcomeMessage(sessionID: "current-resource-commit"))
        await current.handleIncomingMessage(
            makeResourceMetadataMessage(hash: victimHash, bytes: victimBytes)
        )
        await current.handleIncomingMessage(
            makeResourceChunkMessage(hash: victimHash, bytes: victimBytes)
        )
        #expect(renderer.resolveResourceImage(victimHash) != nil)

        await commitGate.release()
        await staleCommit.value

        #expect(await cache.contains(victimHash))
        #expect(renderer.resolveResourceImage(victimHash) != nil)
        #expect(renderer.resolveResourceImage(pressureHash) == nil)

        await old.stop()
        await current.stop()
        await oldServer.close()
        await currentServer.close()
    }

    @Test("A stale connection cannot publish a live resync snapshot after rebinding")
    @MainActor
    func staleConnectionCannotCommitResyncSnapshot() async throws {
        let applier = TransactionApplier()
        let outbox = EventOutbox()
        let (oldClient, oldServer) = await PipeTransport.createPair()
        let oldFailures = ResyncFailureRecorder()
        let old = SessionController(
            transport: oldClient,
            applier: applier,
            outbox: outbox
        )
        old.onFailure = { failure in
            Task { await oldFailures.record(failure) }
        }
        try await old.start()

        var welcome = SRUIServerWelcome()
        welcome.coreVersion = SRUICoreVersion
        welcome.sessionID = "binding-session"
        welcome.requiredProfiles = ["org.srui.standard-widgets/1"]
        var welcomeMessage = SRUIMessage()
        welcomeMessage.serverWelcome = welcome
        try await oldServer.send(data: try SRUIFraming.encodeFramed(welcomeMessage))
        try await AsyncTestSupport.eventually(description: "old connection active") {
            old.isEventDispatchEnabled
        }

        var resync = SRUIServerResyncRequired()
        resync.sessionID = "binding-session"
        resync.snapshotRevision = 1
        resync.reason = "force snapshot"
        resync.continuity = .sameSession
        var resyncMessage = SRUIMessage()
        resyncMessage.serverResyncRequired = resync
        try await oldServer.send(data: try SRUIFraming.encodeFramed(resyncMessage))
        try await AsyncTestSupport.eventually(description: "old connection awaiting snapshot") {
            !old.isEventDispatchEnabled
        }

        let (newClient, newServer) = await PipeTransport.createPair()
        let replacement = SessionController(
            transport: newClient,
            applier: applier,
            outbox: outbox,
            sessionId: "binding-session"
        )
        try await replacement.start()

        let staleSnapshot = Transaction(
            baseRevision: .initial,
            newRevision: Revision(1),
            operations: [
                .createNode(id: NodeId(1), nodeType: .surface),
                .createNode(
                    id: NodeId(2),
                    nodeType: .text,
                    parentID: NodeId(1),
                    properties: [Property(property: .text, value: .string("stale"))]
                ),
            ]
        )
        var staleSnapshotMessage = SRUIMessage()
        staleSnapshotMessage.transaction = staleSnapshot.toWire()
        await old.handleIncomingMessage(staleSnapshotMessage)

        let failure = await oldFailures.wait()
        guard case .superseded = failure else {
            Issue.record("stale snapshot must report supersession, got \(String(describing: failure))")
            return
        }
        #expect(
            applier.lastAppliedRevision == .initial,
            "binding validation must run before publishing into the shared TransactionApplier"
        )

        await old.stop()
        await replacement.stop()
        await oldServer.close()
        await newServer.close()
    }

    @Test("A stale connection cannot apply a live delta after rebinding")
    @MainActor
    func staleConnectionCannotApplyLiveDelta() async throws {
        let applier = TransactionApplier()
        let outbox = EventOutbox()
        let (oldClient, oldServer) = await PipeTransport.createPair()
        let oldFailures = ResyncFailureRecorder()
        let old = SessionController(
            transport: oldClient,
            applier: applier,
            outbox: outbox
        )
        old.onFailure = { failure in
            Task { await oldFailures.record(failure) }
        }
        try await old.start()

        var welcome = SRUIServerWelcome()
        welcome.coreVersion = SRUICoreVersion
        welcome.sessionID = "live-binding-session"
        welcome.requiredProfiles = ["org.srui.standard-widgets/1"]
        var welcomeMessage = SRUIMessage()
        welcomeMessage.serverWelcome = welcome
        try await oldServer.send(data: try SRUIFraming.encodeFramed(welcomeMessage))
        try await AsyncTestSupport.eventually(description: "old live connection active") {
            old.isEventDispatchEnabled
        }

        let (newClient, newServer) = await PipeTransport.createPair()
        let replacement = SessionController(
            transport: newClient,
            applier: applier,
            outbox: outbox,
            sessionId: "live-binding-session"
        )
        try await replacement.start()

        let staleDelta = Transaction(
            baseRevision: .initial,
            newRevision: Revision(1),
            operations: [
                .createNode(id: NodeId(1), nodeType: .surface),
                .createNode(
                    id: NodeId(2),
                    nodeType: .text,
                    parentID: NodeId(1),
                    properties: [Property(property: .text, value: .string("stale live delta"))]
                ),
            ]
        )
        var staleDeltaMessage = SRUIMessage()
        staleDeltaMessage.transaction = staleDelta.toWire()
        await old.handleIncomingMessage(staleDeltaMessage)

        let failure = await oldFailures.wait()
        guard case .superseded = failure else {
            Issue.record("stale live delta must report supersession, got \(String(describing: failure))")
            return
        }
        #expect(
            applier.lastAppliedRevision == .initial,
            "live transaction ownership must be checked atomically with replica mutation"
        )

        await old.stop()
        await replacement.stop()
        await oldServer.close()
        await newServer.close()
    }

    @Test("A rebind between live publish and render cannot overwrite the replacement mount")
    @MainActor
    func staleLiveRenderCannotOverwriteReplacementMount() async throws {
        let applier = TransactionApplier()
        let outbox = EventOutbox()
        let renderer = AppKitRenderer()
        let gate = LiveRenderGate()
        let oldFailures = ResyncFailureRecorder()
        let sessionID = "live-render-binding-session"
        let surfaceID = NodeId(1)
        let textID = NodeId(2)

        let (oldClient, oldServer) = await PipeTransport.createPair()
        let old = SessionController(
            transport: oldClient,
            applier: applier,
            outbox: outbox,
            renderer: renderer
        )
        old.attachRenderer(renderer)
        old.onFailure = { failure in
            Task { await oldFailures.record(failure) }
        }
        try await old.start()

        var welcome = SRUIServerWelcome()
        welcome.coreVersion = SRUICoreVersion
        welcome.sessionID = sessionID
        welcome.requiredProfiles = ["org.srui.standard-widgets/1"]
        var welcomeMessage = SRUIMessage()
        welcomeMessage.serverWelcome = welcome
        await old.handleIncomingMessage(welcomeMessage)

        let initial = Transaction(
            baseRevision: .initial,
            newRevision: Revision(1),
            operations: [
                .createNode(id: surfaceID, nodeType: .surface),
                .createNode(
                    id: textID,
                    nodeType: .text,
                    parentID: surfaceID,
                    properties: [Property(property: .text, value: .string("initial"))]
                ),
            ]
        )
        var initialMessage = SRUIMessage()
        initialMessage.transaction = initial.toWire()
        await old.handleIncomingMessage(initialMessage)
        let initialField = try #require(renderer.registry.handle(for: textID)?.view as? NSTextField)
        #expect(initialField.stringValue == "initial")

        old.liveTransactionPublishedInterceptorForTesting = {
            await gate.pauseAfterPublish()
        }
        let staleDelta = Transaction(
            baseRevision: Revision(1),
            newRevision: Revision(2),
            operations: [
                .setProperty(id: textID, property: .text, value: .string("stale")),
            ]
        )
        var staleDeltaMessage = SRUIMessage()
        staleDeltaMessage.transaction = staleDelta.toWire()
        let staleDelivery = Task {
            await old.handleIncomingMessage(staleDeltaMessage)
        }
        await gate.waitUntilPaused()
        #expect(applier.lastAppliedRevision == Revision(2))

        let (replacementClient, replacementServer) = await PipeTransport.createPair()
        let replacement = SessionController(
            transport: replacementClient,
            applier: applier,
            outbox: outbox,
            renderer: renderer,
            sessionId: sessionID
        )
        replacement.attachRenderer(renderer)
        try await replacement.start()

        var resync = SRUIServerResyncRequired()
        resync.sessionID = sessionID
        resync.snapshotRevision = 3
        resync.reason = "replacement owns renderer"
        resync.continuity = .sameSession
        var resyncMessage = SRUIMessage()
        resyncMessage.serverResyncRequired = resync
        await replacement.handleIncomingMessage(resyncMessage)

        let replacementSnapshot = Transaction(
            baseRevision: .initial,
            newRevision: Revision(3),
            operations: [
                .createNode(id: surfaceID, nodeType: .surface),
                .createNode(
                    id: textID,
                    nodeType: .text,
                    parentID: surfaceID,
                    properties: [Property(property: .text, value: .string("replacement"))]
                ),
            ]
        )
        var replacementSnapshotMessage = SRUIMessage()
        replacementSnapshotMessage.transaction = replacementSnapshot.toWire()
        await replacement.handleIncomingMessage(replacementSnapshotMessage)
        let replacementField = try #require(
            renderer.registry.handle(for: textID)?.view as? NSTextField
        )
        #expect(replacementField.stringValue == "replacement")

        await gate.release()
        await staleDelivery.value
        let failure = await oldFailures.wait()
        guard case .superseded = failure else {
            Issue.record("stale live render must report supersession, got \(String(describing: failure))")
            return
        }
        #expect(
            (renderer.registry.handle(for: textID)?.view as? NSTextField)?.stringValue
                == "replacement"
        )

        await old.stop()
        await replacement.stop()
        await oldServer.close()
        await replacementServer.close()
    }

    @Test("Rejected transaction does not stop processing subsequent transactions")
    @MainActor
    func rejectedTransactionDoesNotStopReceiveLoop() async throws {
        let (clientTransport, serverTransport) = await PipeTransport.createPair()
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
        let controller = SessionController(
            transport: clientTransport,
            applier: applier,
            renderer: renderer
        )
        controller.attachRenderer(renderer)

        try await controller.start()

        var welcome = SRUIServerWelcome()
        welcome.coreVersion = SRUICoreVersion
        welcome.sessionID = "test-session"
        welcome.requiredProfiles = ["org.srui.standard-widgets/1"]
        var welcomeMsg = SRUIMessage()
        welcomeMsg.serverWelcome = welcome
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(welcomeMsg))

        let surfaceID = NodeId(1)
        let textID = NodeId(2)

        let initialTx = Transaction(
            baseRevision: .initial,
            newRevision: Revision(1),
            operations: [
                .createNode(id: surfaceID, nodeType: .surface),
                .createNode(
                    id: textID,
                    nodeType: .text,
                    parentID: surfaceID,
                    properties: [Property(property: .text, value: .string("Count: 0"))]
                ),
            ]
        )

        var initialMsg = SRUIMessage()
        initialMsg.transaction = initialTx.toWire()
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(initialMsg))
        try await AsyncTestSupport.eventually(
            timeout: .seconds(5),
            description: "initial transaction applied"
        ) {
            applier.lastAppliedRevision == Revision(1)
        }
        #expect(applier.lastAppliedRevision == Revision(1))

        let staleTx = Transaction(
            baseRevision: .initial,
            newRevision: Revision(1),
            operations: [
                .setProperty(id: textID, property: .text, value: .string("Should not apply")),
            ]
        )

        var staleMsg = SRUIMessage()
        staleMsg.transaction = staleTx.toWire()
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(staleMsg))
        #expect(applier.lastAppliedRevision == Revision(1))

        let validTx = Transaction(
            baseRevision: Revision(1),
            newRevision: Revision(2),
            operations: [
                .setProperty(id: textID, property: .text, value: .string("Count: 1")),
            ]
        )

        var validMsg = SRUIMessage()
        validMsg.transaction = validTx.toWire()
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(validMsg))
        try await AsyncTestSupport.eventually(
            timeout: .seconds(5),
            description: "follow-up transaction painted"
        ) {
            (renderer.registry.handle(for: textID)?.view as? NSTextField)?.stringValue
                == "Count: 1"
        }

        #expect(applier.lastAppliedRevision == Revision(2))
        let textHandle = try #require(renderer.registry.handle(for: textID))
        let textField = try #require(textHandle.view as? NSTextField)
        #expect(textField.stringValue == "Count: 1")

        await controller.stop()
        await serverTransport.close()
    }

    // NOTE: this scenario never reaches the renderer at all — the store rejects `badTx` because of
    // its dangling parent, so no mount is ever attempted. What it actually pins down is that a
    // rejected transaction leaves no side effects and that a store-level rejection is treated as
    // replica divergence rather than being silently skipped (§12.1, §4 inv. 13). Recovery of the
    // mount flag after a genuine *renderer* failure is covered by
    // `SessionRobustnessTests.rendererFailureForcesFullReattach`.
    @Test("A rejected transaction leaves no side effects and ends the session")
    @MainActor
    func rejectedTransactionLeavesNoSideEffects() async throws {
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
        let controller = SessionController(
            transport: PipeTransport(),
            applier: applier,
            renderer: renderer
        )
        controller.attachRenderer(renderer)

        let surfaceID = NodeId(1)

        await controller.handleIncomingMessage({
            var welcome = SRUIServerWelcome()
            welcome.coreVersion = SRUICoreVersion
            welcome.sessionID = "test-session"
            welcome.requiredProfiles = ["org.srui.standard-widgets/1"]
            var msg = SRUIMessage()
            msg.serverWelcome = welcome
            return msg
        }())

        let badTx = Transaction(
            baseRevision: .initial,
            newRevision: Revision(1),
            operations: [
                .createNode(id: surfaceID, nodeType: .surface),
                .createNode(id: NodeId(2), nodeType: .text, parentID: NodeId(999)),
            ]
        )

        await controller.handleIncomingMessage({
            var msg = SRUIMessage()
            msg.transaction = badTx.toWire()
            return msg
        }())

        #expect(applier.lastAppliedRevision == .initial)
        #expect(renderer.registry.count == 0)

        // The server committed this revision even though we could not, so the replica is now behind
        // and can only recover by resuming on a fresh transport (§18).
        #expect(controller.isDiverged)

        let goodTx = Transaction(
            baseRevision: .initial,
            newRevision: Revision(1),
            operations: [
                .createNode(id: surfaceID, nodeType: .surface),
                .createNode(
                    id: NodeId(2),
                    nodeType: .text,
                    parentID: surfaceID,
                    properties: [Property(property: .text, value: .string("Mounted"))]
                ),
            ]
        )

        await controller.handleIncomingMessage({
            var msg = SRUIMessage()
            msg.transaction = goodTx.toWire()
            return msg
        }())

        // A diverged session must not keep applying the stream as though nothing happened.
        #expect(applier.lastAppliedRevision == .initial)
        #expect(renderer.registry.count == 0)
    }

    @Test("A replacement renderer hydrates images from the shared cache")
    @MainActor
    func replacementRendererHydratesSharedCache() async throws {
        let bytes = Data([
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D,
            0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
            0x08, 0x02, 0x00, 0x00, 0x00, 0x90, 0x77, 0x53, 0xDE, 0x00, 0x00, 0x00,
            0x0C, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x60, 0x60, 0x60, 0x00,
            0x00, 0x00, 0x04, 0x00, 0x01, 0xF6, 0x17, 0x38, 0x55, 0x00, 0x00, 0x00,
            0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ])
        let hash = try ResourceHash(rawBytes: Array(SHA256.hash(data: bytes)))
        let cache = ResourceCache()
        _ = try await cache.ingestMetadata(
            ResourceMetadataInput(
                resourceHash: hash,
                mediaType: "image/png",
                encodedLength: UInt64(bytes.count),
                decodedWidth: 1,
                decodedHeight: 1
            )
        )
        _ = try #require(
            try await cache.ingestChunk(
                ResourceChunkInput(resourceHash: hash, byteOffset: 0, data: bytes)
            )
        )

        let (clientTransport, serverTransport) = await PipeTransport.createPair()
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
        let controller = SessionController(
            transport: clientTransport,
            applier: applier,
            renderer: renderer,
            resourceCache: cache
        )
        controller.attachRenderer(renderer)
        try await controller.start()

        let serverStream = serverTransport.receiveStream()
        var streamDecoder = SRUIMessageStreamDecoder()
        var receivedHello: SRUIClientHello?
        for try await chunk in serverStream {
            for message in try streamDecoder.appendAndExtract(incoming: chunk) {
                if case .clientHello(let hello) = message.msg {
                    receivedHello = hello
                    break
                }
            }
            if receivedHello != nil { break }
        }
        let hello = try #require(receivedHello)
        #expect(hello.knownResourceHashes == [hash.bytes])

        var welcome = SRUIServerWelcome()
        welcome.coreVersion = SRUICoreVersion
        welcome.sessionID = "cached-image-session"
        welcome.requiredProfiles = ["org.srui.standard-widgets/1"]
        welcome.initialRevision = 1
        var welcomeMessage = SRUIMessage()
        welcomeMessage.serverWelcome = welcome
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(welcomeMessage))

        let imageID = NodeId(2)
        let snapshot = Transaction(
            baseRevision: .initial,
            newRevision: Revision(1),
            operations: [
                .createNode(id: NodeId(1), nodeType: .surface),
                .createNode(
                    id: imageID,
                    nodeType: .image,
                    parentID: NodeId(1),
                    properties: [
                        Property(property: .resource, value: .resourceHash(hash)),
                    ]
                ),
            ]
        )
        var snapshotMessage = SRUIMessage()
        snapshotMessage.transaction = snapshot.toWire()
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(snapshotMessage))

        try await AsyncTestSupport.eventually(description: "cached image hydration") {
            applier.lastAppliedRevision == Revision(1)
                && renderer.resolveResourceImage(hash) != nil
                && renderer.registry.handle(for: imageID)?.view is NSImageView
        }
        let imageHandle = try #require(renderer.registry.handle(for: imageID))
        let imageView = try #require(imageHandle.view as? NSImageView)
        #expect(imageView.image != nil)

        await controller.stop()
        await serverTransport.close()
    }
}

private enum ResyncTestSupportError: Error {
    case pngEncodingFailed
    case resourceDidNotCommit
}

@MainActor
private func makeOnePixelPNG(_ color: NSColor) throws -> Data {
    guard let bitmap = NSBitmapImageRep(
        bitmapDataPlanes: nil,
        pixelsWide: 1,
        pixelsHigh: 1,
        bitsPerSample: 8,
        samplesPerPixel: 4,
        hasAlpha: true,
        isPlanar: false,
        colorSpaceName: .deviceRGB,
        bytesPerRow: 4,
        bitsPerPixel: 32
    ) else {
        throw ResyncTestSupportError.pngEncodingFailed
    }
    bitmap.setColor(color, atX: 0, y: 0)
    guard let bytes = bitmap.representation(using: .png, properties: [:]) else {
        throw ResyncTestSupportError.pngEncodingFailed
    }
    return bytes
}

private func makeWelcomeMessage(sessionID: String) -> SRUIMessage {
    HandshakeFixtures.welcomeMessage(sessionId: sessionID)
}

private func makeResourceMetadataMessage(
    hash: ResourceHash,
    bytes: Data
) -> SRUIMessage {
    var metadata = SRUIResourceMetadata()
    metadata.resourceHash = hash.bytes
    metadata.mediaType = "image/png"
    metadata.encodedLength = UInt64(bytes.count)
    metadata.decodedWidth = 1
    metadata.decodedHeight = 1
    var message = SRUIMessage()
    message.resourceMetadata = metadata
    return message
}

private func makeResourceChunkMessage(
    hash: ResourceHash,
    byteOffset: UInt64 = 0,
    bytes: Data
) -> SRUIMessage {
    var chunk = SRUIResourceChunk()
    chunk.resourceHash = hash.bytes
    chunk.byteOffset = byteOffset
    chunk.data = bytes
    var message = SRUIMessage()
    message.resourceChunk = chunk
    return message
}

private func commitPNG(_ bytes: Data, into cache: ResourceCache) async throws -> ResourceHash {
    let hash = try ResourceHash(rawBytes: Array(SHA256.hash(data: bytes)))
    _ = try await cache.ingestMetadata(
        ResourceMetadataInput(
            resourceHash: hash,
            mediaType: "image/png",
            encodedLength: UInt64(bytes.count),
            decodedWidth: 1,
            decodedHeight: 1
        )
    )
    let commit = try await cache.ingestChunk(
        ResourceChunkInput(resourceHash: hash, byteOffset: 0, data: bytes)
    )
    guard commit != nil else {
        throw ResyncTestSupportError.resourceDidNotCommit
    }
    return hash
}

private enum InjectedRendererFailure: Error {
    case requested
}

@MainActor
private final class RendererFailureInjector {
    private var remainingFailures: Int

    init(remainingFailures: Int) {
        self.remainingFailures = remainingFailures
    }

    func failIfNeeded() throws {
        guard remainingFailures > 0 else { return }
        remainingFailures -= 1
        throw InjectedRendererFailure.requested
    }
}
