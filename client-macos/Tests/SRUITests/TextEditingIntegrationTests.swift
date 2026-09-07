//
// TextEditingIntegrationTests.swift
// SRUITests
//
// Session-level native text editing: local-first feedback on a delayed/gated
// input lane, rejection convergence, and resume/resync coordination (§18.3, §22.6).
//

import AppKit
import Foundation
import Protocol
import RendererAppKit
import SemanticModel
@testable import Session
import Testing
import Text
import TransportSSH

@Suite("Native text editing integration")
struct TextEditingIntegrationTests {
    private let surfaceID = NodeId(1)
    private let editorID = NodeId(12)

    @Test("Teardown end-editing cannot consume a structural-remount draft")
    @MainActor
    func teardownEndEditingPreservesRemountDraft() {
        let session = TextEditingSession(debounceNanoseconds: 0)
        var committedValues: [String] = []
        session.onCommit = { _, value, _, _ in
            committedValues.append(value)
        }

        #expect(session.applyPublishedValue(nodeID: editorID, published: "server") == .apply)
        let resolution = session.withPreservedLocalText {
            session.noteLocalValue(
                "local",
                nodeID: editorID,
                composing: false,
                flushImmediately: false
            )
            session.endEditing(nodeID: editorID)
            return session.applyPublishedValue(nodeID: editorID, published: "server")
        }

        #expect(resolution == .keepLocal)
        #expect(session.localValue(for: editorID) == "local")
        #expect(committedValues.isEmpty)
    }

    @Test("flushAllPending commits each editor once")
    @MainActor
    func flushAllPendingCommitsEveryEditor() {
        let session = TextEditingSession(debounceNanoseconds: 60_000_000_000)
        let secondEditorID = NodeId(13)
        var commits: [(NodeId, String)] = []
        session.onCommit = { nodeID, value, _, _ in
            commits.append((nodeID, value))
        }

        session.noteLocalValue(
            "first",
            nodeID: editorID,
            composing: false,
            flushImmediately: false
        )
        session.noteLocalValue(
            "second",
            nodeID: secondEditorID,
            composing: false,
            flushImmediately: false
        )

        session.flushAllPending()
        session.flushAllPending()

        #expect(commits.count == 2)
        #expect(Dictionary(uniqueKeysWithValues: commits)[editorID] == "first")
        #expect(Dictionary(uniqueKeysWithValues: commits)[secondEditorID] == "second")
    }

    @Test("Rendered acknowledgement retires its echo without losing an unassigned successor")
    @MainActor
    func renderedAcknowledgementRetiresSettledEchoIdentity() throws {
        let session = TextEditingSession(debounceNanoseconds: 0)
        var commits: [(value: String, editSeq: EditSeq)] = []
        session.onCommit = { _, value, editSeq, _ in
            commits.append((value, editSeq))
        }

        #expect(session.applyPublishedValue(nodeID: editorID, published: "seed") == .apply)
        session.noteLocalValue(
            "foo",
            nodeID: editorID,
            composing: false,
            flushImmediately: true
        )
        let submitted = try #require(commits.first)
        let event = Event.textEdit(
            eventSeq: 1,
            eventId: EventId(string: "settled-foo"),
            observedRevision: Revision(1),
            nodeId: editorID,
            text: submitted.value,
            editSeq: submitted.editSeq
        )
        session.noteAssigned(event)
        #expect(session.applyPublishedValue(nodeID: editorID, published: "foo") == .keepLocal)
        session.noteAcknowledged(event)

        // This flush has no event identity yet. It must survive the acknowledgement bookkeeping,
        // but a later live authoritative re-publication of the settled value is a correction.
        session.noteLocalValue(
            "food",
            nodeID: editorID,
            composing: false,
            flushImmediately: true
        )
        #expect(session.hasUnsentSuccessorDraft(for: editorID))
        #expect(session.localValue(for: editorID) == "food")

        #expect(session.applyPublishedValue(nodeID: editorID, published: "foo") == .apply)
        #expect(session.localValue(for: editorID) == "foo")
        #expect(session.hasUnsentSuccessorDraft(for: editorID) == false)
    }

    @Test("Delayed input lane: native text changes before any network delivery")
    @MainActor
    func delayedTransportKeepsEditingLocal() async throws {
        let (clientPipe, serverTransport) = await PipeTransport.createPair()
        let delayed = DelayedInputTransport(inner: clientPipe, delayNanoseconds: 500_000_000)
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
        renderer.textEditingSession.debounceNanoseconds = 0
        let controller = SessionController(
            transport: delayed,
            applier: applier,
            renderer: renderer
        )
        controller.attachRenderer(renderer)
        try await controller.start()
        try await handshakeAndMount(controller: controller, server: serverTransport, applier: applier, renderer: renderer)

        let handle = try #require(renderer.registry.handle(for: editorID))
        let adapter = try #require(handle.textAdapter)
        let field = try #require(handle.view as? NSTextField)
        let collector = EventCollector()
        await collector.start(draining: serverTransport)

        let window = try #require(field.window)
        #expect(window.makeFirstResponder(field))
        let editor = try #require(field.currentEditor() as? NSTextView)

        // Glyphs, caret movement, and selection are all mutations of AppKit's live field editor.
        editor.string = "glyphs"
        #expect(editor.string == "glyphs")
        editor.setSelectedRange(NSRange(location: 2, length: 0))
        #expect(editor.selectedRange == NSRange(location: 2, length: 0))
        editor.setSelectedRange(NSRange(location: 2, length: 2))
        #expect(editor.selectedRange == NSRange(location: 2, length: 2))

        // Exercise NSTextInputClient's real marked range rather than the adapter test override.
        editor.setMarkedText(
            "かな",
            selectedRange: NSRange(location: 2, length: 0),
            replacementRange: editor.selectedRange
        )
        #expect(editor.hasMarkedText())
        let markedRange = editor.markedRange()
        #expect(markedRange.location != NSNotFound)
        #expect(editor.selectedRange.location == NSMaxRange(markedRange))
        #expect(adapter.isComposing)
        adapter.notifyTextDidChangeForTests()
        #expect(await collector.eventCount() == 0)

        editor.unmarkText()
        #expect(editor.hasMarkedText() == false)
        adapter.notifyTextDidChangeForTests()
        _ = window.makeFirstResponder(nil)
        adapter.notifyEndEditingForTests()
        let committed = field.stringValue
        #expect(committed.isEmpty == false)
        #expect(await collector.eventCount() == 0)

        await delayed.waitUntilInputSendStarted()
        #expect(await collector.eventCount() == 0)

        let delivered = try await waitForTextEvent(collector)
        #expect(delivered.eventType == .EVENT_TEXT_EDIT)
        #expect(delivered.textArg == committed)
        #expect(delivered.editSeq?.rawValue == 1)
        #expect(await collector.eventCount() == 1)

        await controller.stop()
        await collector.stop()
        await delayed.close()
        await serverTransport.close()
    }

    @Test("Type, type, then activate sends the latest text before the action")
    @MainActor
    func latestTextPrecedesNonTextAction() async throws {
        let (clientTransport, serverTransport) = await PipeTransport.createPair()
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
        renderer.textEditingSession.debounceNanoseconds = 60_000_000_000
        let controller = SessionController(
            transport: clientTransport,
            applier: applier,
            renderer: renderer
        )
        controller.attachRenderer(renderer)
        let collector = EventCollector()

        do {
            try await controller.start()
            try await handshakeAndMount(
                controller: controller,
                server: serverTransport,
                applier: applier,
                renderer: renderer,
                sessionId: "text-before-action"
            )
            await collector.start(draining: serverTransport)

            renderer.textEditingSession.noteLocalValue(
                "first",
                nodeID: editorID,
                composing: false,
                flushImmediately: true
            )
            let first = try await waitForTextEvent(collector)
            renderer.textEditingSession.noteLocalValue(
                "second",
                nodeID: editorID,
                composing: false,
                flushImmediately: false
            )
            // The activate callback synchronously flushes and queues the successor before it
            // returns. E1 still owns this editor lane, so neither E2 nor the action may allocate.
            renderer.onInteraction?(.activate(nodeID: NodeId(99)))
            #expect(renderer.textEditingSession.hasUnsentSuccessorDraft(for: editorID))
            #expect(await controller.outbox.eventSeq == 1)
            #expect(
                await collector.eventCount() == 1,
                "the action must remain unallocated while the successor text draft is blocked"
            )

            var acknowledgement = SRUIServerEventAck()
            acknowledgement.clientInstanceID = try #require(first.clientInstanceId).bytes
            acknowledgement.eventID = first.eventId.bytes
            acknowledgement.lastProcessedEventSeq = first.eventSeq
            acknowledgement.status = .processed
            acknowledgement.revisionAfterEffect = 1
            acknowledgement.sessionID = "text-before-action"
            var acknowledgementMessage = SRUIMessage()
            acknowledgementMessage.serverEventAck = acknowledgement
            await controller.handleIncomingMessage(acknowledgementMessage)

            try await waitUntil(description: "successor text and action delivered") {
                await collector.eventCount() == 3
            }
            let delivered = await collector.events()
            #expect(delivered.map(\.eventType) == [
                .EVENT_TEXT_EDIT,
                .EVENT_TEXT_EDIT,
                .EVENT_ACTIVATE,
            ])
            #expect(delivered.map(\.eventSeq) == [1, 2, 3])
            #expect(delivered[1].textArg == "second")
        } catch {
            // A failed assertion must not strand the AppKit fixture window above the desktop.
            await controller.stop()
            await collector.stop()
            await serverTransport.close()
            throw error
        }

        await controller.stop()
        await collector.stop()
        await serverTransport.close()
    }

    @Test("Typing during an editor-lane wait stays behind the already-queued action")
    @MainActor
    func typingDuringLaneWaitFollowsNonTextAction() async throws {
        let (clientTransport, serverTransport) = await PipeTransport.createPair()
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
        renderer.textEditingSession.debounceNanoseconds = 60_000_000_000
        let controller = SessionController(
            transport: clientTransport,
            applier: applier,
            renderer: renderer
        )
        controller.attachRenderer(renderer)
        let collector = EventCollector()

        do {
            try await controller.start()
            try await handshakeAndMount(
                controller: controller,
                server: serverTransport,
                applier: applier,
                renderer: renderer,
                sessionId: "text-after-action"
            )
            await collector.start(draining: serverTransport)

            renderer.textEditingSession.noteLocalValue(
                "first",
                nodeID: editorID,
                composing: false,
                flushImmediately: true
            )
            let first = try await waitForTextEvent(collector)
            renderer.textEditingSession.noteLocalValue(
                "second",
                nodeID: editorID,
                composing: false,
                flushImmediately: false
            )
            renderer.onInteraction?(.activate(nodeID: NodeId(99)))
            renderer.textEditingSession.noteLocalValue(
                "third",
                nodeID: editorID,
                composing: false,
                flushImmediately: true
            )
            #expect(await controller.outbox.eventSeq == 1)
            #expect(
                await collector.eventCount() == 1,
                "later typing must not allocate ahead of the queued action"
            )

            var acknowledgement = SRUIServerEventAck()
            acknowledgement.clientInstanceID = try #require(first.clientInstanceId).bytes
            acknowledgement.eventID = first.eventId.bytes
            acknowledgement.lastProcessedEventSeq = first.eventSeq
            acknowledgement.status = .processed
            acknowledgement.revisionAfterEffect = 1
            acknowledgement.sessionID = "text-after-action"
            var acknowledgementMessage = SRUIMessage()
            acknowledgementMessage.serverEventAck = acknowledgement
            await controller.handleIncomingMessage(acknowledgementMessage)

            try await waitUntil(description: "E2 and the queued action delivered") {
                await collector.eventCount() == 3
            }
            let beforeSecondAck = await collector.events()
            #expect(beforeSecondAck.map(\.eventType) == [
                .EVENT_TEXT_EDIT,
                .EVENT_TEXT_EDIT,
                .EVENT_ACTIVATE,
            ])
            #expect(beforeSecondAck.map(\.eventSeq) == [1, 2, 3])
            #expect(beforeSecondAck[1].textArg == "second")

            var secondAcknowledgement = SRUIServerEventAck()
            secondAcknowledgement.clientInstanceID = try #require(
                beforeSecondAck[1].clientInstanceId
            ).bytes
            secondAcknowledgement.eventID = beforeSecondAck[1].eventId.bytes
            secondAcknowledgement.lastProcessedEventSeq = beforeSecondAck[1].eventSeq
            secondAcknowledgement.status = .processed
            secondAcknowledgement.revisionAfterEffect = 1
            secondAcknowledgement.sessionID = "text-after-action"
            var secondAcknowledgementMessage = SRUIMessage()
            secondAcknowledgementMessage.serverEventAck = secondAcknowledgement
            await controller.handleIncomingMessage(secondAcknowledgementMessage)

            try await waitUntil(description: "later typing delivered after E2 settles") {
                await collector.eventCount() == 4
            }
            let delivered = await collector.events()
            #expect(delivered.map(\.eventType) == [
                .EVENT_TEXT_EDIT,
                .EVENT_TEXT_EDIT,
                .EVENT_ACTIVATE,
                .EVENT_TEXT_EDIT,
            ])
            #expect(delivered.map(\.eventSeq) == [1, 2, 3, 4])
            #expect(delivered[3].textArg == "third")
        } catch {
            await controller.stop()
            await collector.stop()
            await serverTransport.close()
            throw error
        }

        await controller.stop()
        await collector.stop()
        await serverTransport.close()
    }

    @Test("A correction between native assignment and authorization does not send the stale edit")
    @MainActor
    func correctionBeforeAuthorizationDropsStaleTextEdit() async throws {
        let (clientTransport, serverTransport) = await PipeTransport.createPair()
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
        renderer.textEditingSession.debounceNanoseconds = 0
        let controller = SessionController(
            transport: clientTransport,
            applier: applier,
            renderer: renderer
        )
        controller.attachRenderer(renderer)
        let collector = EventCollector()

        do {
            try await controller.start()
            try await handshakeAndMount(
                controller: controller,
                server: serverTransport,
                applier: applier,
                renderer: renderer,
                sessionId: "text-correction-before-authorize"
            )
            await collector.start(draining: serverTransport)

            let authorizeGate = TextLifecycleGate()
            controller.textEditWillAuthorizeForTesting = {
                await authorizeGate.pause()
            }

            renderer.textEditingSession.noteLocalValue(
                "stale",
                nodeID: editorID,
                composing: false,
                flushImmediately: true
            )
            await authorizeGate.waitUntilPaused()
            #expect(await collector.events().isEmpty)

            var correction = SRUIMessage()
            correction.transaction = Transaction(
                baseRevision: Revision(1),
                newRevision: Revision(2),
                operations: [
                    .setProperty(id: editorID, property: .value, value: .string("corrected")),
                ]
            ).toWire()
            try await serverTransport.send(data: try SRUIFraming.encodeFramed(correction))
            try await waitUntil(description: "correction rendered") {
                (renderer.registry.handle(for: editorID)?.view as? NSTextField)?.stringValue
                    == "corrected"
            }

            controller.textEditWillAuthorizeForTesting = nil
            await authorizeGate.release()

            try await waitUntil(description: "stale envelope rolled back") {
                let pending = await controller.outbox.pendingCount
                let eventSeq = await controller.outbox.eventSeq
                return pending == 0 && eventSeq == 0
            }
            #expect(await collector.events().filter { $0.eventType == .EVENT_TEXT_EDIT }.isEmpty)
            #expect(
                (renderer.registry.handle(for: editorID)?.view as? NSTextField)?.stringValue
                    == "corrected"
            )
        } catch {
            await controller.stop()
            await collector.stop()
            await serverTransport.close()
            throw error
        }

        await controller.stop()
        await collector.stop()
        await serverTransport.close()
    }

    @Test("Stopping wakes an action blocked behind a text draft without allocating it")
    @MainActor
    func stopCancelsActionWaitingForTextDraft() async throws {
        let (clientTransport, serverTransport) = await PipeTransport.createPair()
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
        renderer.textEditingSession.debounceNanoseconds = 60_000_000_000
        let outbox = EventOutbox()
        let controller = SessionController(
            transport: clientTransport,
            applier: applier,
            outbox: outbox,
            renderer: renderer
        )
        controller.attachRenderer(renderer)
        try await controller.start()
        try await handshakeAndMount(
            controller: controller,
            server: serverTransport,
            applier: applier,
            renderer: renderer,
            sessionId: "stop-text-before-action"
        )

        let collector = EventCollector()
        await collector.start(draining: serverTransport)
        renderer.textEditingSession.noteLocalValue(
            "first",
            nodeID: editorID,
            composing: false,
            flushImmediately: true
        )
        _ = try await waitForTextEvent(collector)
        renderer.textEditingSession.noteLocalValue(
            "second",
            nodeID: editorID,
            composing: false,
            flushImmediately: false
        )
        renderer.onInteraction?(.activate(nodeID: NodeId(99)))
        #expect(await outbox.eventSeq == 1)
        #expect(await collector.eventCount() == 1)

        // This callback is serialized behind the blocked successor and action. Terminal suspension
        // must cancel that tail without losing the newest MainActor-owned draft.
        renderer.textEditingSession.noteLocalValue(
            "third",
            nodeID: editorID,
            composing: false,
            flushImmediately: true
        )
        await controller.stop()
        #expect(
            await collector.eventCount() == 1,
            "terminal suspension must not let the blocked action allocate on a closed transport"
        )
        #expect(renderer.textEditingSession.hasUnsentSuccessorDraft(for: editorID))
        #expect(renderer.textEditingSession.localValue(for: editorID) == "third")
        #expect(renderer.textEditingSession.nextEditSeqValue(for: editorID) == 4)

        await collector.stop()
        await serverTransport.close()
    }

    @Test("Stopping cancels a blocked writer and retains the newer buffered text callback")
    @MainActor
    func stopCancelsBlockedWriterAndRetainsBufferedText() async throws {
        let (clientPipe, serverTransport) = await PipeTransport.createPair()
        let delayed = DelayedInputTransport(suspendingInputTo: clientPipe)
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
        renderer.textEditingSession.debounceNanoseconds = 0
        let outbox = EventOutbox()
        let controller = SessionController(
            transport: delayed,
            applier: applier,
            outbox: outbox,
            renderer: renderer
        )
        controller.attachRenderer(renderer)
        try await controller.start()
        try await handshakeAndMount(
            controller: controller,
            server: serverTransport,
            applier: applier,
            renderer: renderer,
            sessionId: "blocked-writer-stop"
        )

        renderer.textEditingSession.noteLocalValue(
            "first",
            nodeID: editorID,
            composing: false,
            flushImmediately: true
        )
        try await waitUntil(description: "first edit retained before blocked send") {
            await outbox.assignedTextEditDescriptors().count == 1
        }
        await delayed.waitUntilInputSendStarted()
        renderer.textEditingSession.noteLocalValue(
            "second",
            nodeID: editorID,
            composing: false,
            flushImmediately: true
        )

        let clock = ContinuousClock()
        let started = clock.now
        await controller.stop()
        #expect(started.duration(to: clock.now) < .seconds(2))
        #expect(renderer.textEditingSession.hasUnsentSuccessorDraft(for: editorID))
        #expect(renderer.textEditingSession.localValue(for: editorID) == "second")
        #expect(renderer.textEditingSession.nextEditSeqValue(for: editorID) == 3)

        await serverTransport.close()
    }

    @Test("Stopping flushes a native debounce draft for same-session resume")
    @MainActor
    func stopFlushesDebouncedDraftForResume() async throws {
        let outbox = EventOutbox()
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
        renderer.textEditingSession.debounceNanoseconds = 60_000_000_000
        let (client, server) = await PipeTransport.createPair()
        let controller = SessionController(
            transport: client,
            applier: applier,
            outbox: outbox,
            renderer: renderer
        )
        controller.attachRenderer(renderer)
        try await controller.start()
        try await handshakeAndMount(
            controller: controller,
            server: server,
            applier: applier,
            renderer: renderer,
            sessionId: "debounce-stop-resume"
        )

        let handle = try #require(renderer.registry.handle(for: editorID))
        let adapter = try #require(handle.textAdapter)
        let field = try #require(handle.view as? NSTextField)
        field.stringValue = "offline draft"
        adapter.notifyTextDidChangeForTests()
        await controller.stop()
        #expect(renderer.textEditingSession.hasUnsentSuccessorDraft(for: editorID))

        let (resumeClient, resumeServer) = await PipeTransport.createPair()
        let resumed = SessionController(
            transport: resumeClient,
            applier: applier,
            outbox: outbox,
            renderer: renderer,
            sessionId: "debounce-stop-resume"
        )
        resumed.attachRenderer(renderer)
        let collector = EventCollector()
        await collector.start(draining: resumeServer)
        try await resumed.start()

        var resumeOK = SRUIServerResumeOk()
        resumeOK.sessionID = "debounce-stop-resume"
        var resumeMessage = SRUIMessage()
        resumeMessage.serverResumeOk = resumeOK
        await resumed.handleIncomingMessage(resumeMessage)

        let edit = try await waitForTextEvent(
            collector,
            matching: { $0.textArg == "offline draft" }
        )
        #expect(edit.editSeq?.rawValue == 1)
        #expect(renderer.textEditingSession.hasUnsentSuccessorDraft(for: editorID) == false)

        await resumed.stop()
        await collector.stop()
        await server.close()
        await resumeServer.close()
    }

    @Test("An edit committed while disconnected replays when the same session resumes")
    @MainActor
    func disconnectedEditReplaysAfterResume() async throws {
        let outbox = EventOutbox()
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
        renderer.textEditingSession.debounceNanoseconds = 60_000_000_000
        let (client, server) = await PipeTransport.createPair()
        let controller = SessionController(
            transport: client,
            applier: applier,
            outbox: outbox,
            renderer: renderer
        )
        controller.attachRenderer(renderer)
        try await controller.start()
        try await handshakeAndMount(
            controller: controller,
            server: server,
            applier: applier,
            renderer: renderer,
            sessionId: "offline-edit-resume"
        )

        let handle = try #require(renderer.registry.handle(for: editorID))
        let adapter = try #require(handle.textAdapter)
        let field = try #require(handle.view as? NSTextField)
        await controller.stop()

        field.stringValue = "typed while disconnected"
        adapter.notifyTextDidChangeForTests()
        adapter.notifyEndEditingForTests()
        #expect(renderer.textEditingSession.hasUnsentSuccessorDraft(for: editorID))
        #expect(renderer.textEditingSession.localValue(for: editorID) == "typed while disconnected")

        let (resumeClient, resumeServer) = await PipeTransport.createPair()
        let resumed = SessionController(
            transport: resumeClient,
            applier: applier,
            outbox: outbox,
            renderer: renderer,
            sessionId: "offline-edit-resume"
        )
        resumed.attachRenderer(renderer)
        let collector = EventCollector()
        await collector.start(draining: resumeServer)

        do {
            try await resumed.start()
            var resumeOK = SRUIServerResumeOk()
            resumeOK.sessionID = "offline-edit-resume"
            var resumeMessage = SRUIMessage()
            resumeMessage.serverResumeOk = resumeOK
            await resumed.handleIncomingMessage(resumeMessage)

            let edit = try await waitForTextEvent(
                collector,
                matching: { $0.textArg == "typed while disconnected" }
            )
            #expect(edit.editSeq?.rawValue == 1)
            #expect(edit.observedRevision == Revision(1))
            #expect(renderer.textEditingSession.hasUnsentSuccessorDraft(for: editorID) == false)
            #expect(field.stringValue == "typed while disconnected")
        } catch {
            await resumed.stop()
            await collector.stop()
            await server.close()
            await resumeServer.close()
            throw error
        }

        await resumed.stop()
        await collector.stop()
        await server.close()
        await resumeServer.close()
    }

    @Test("A replacement snapshot discards an edit committed while disconnected")
    @MainActor
    func replacementDiscardsDisconnectedDraft() async throws {
        let outbox = EventOutbox()
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
        renderer.textEditingSession.debounceNanoseconds = 60_000_000_000
        let (client, server) = await PipeTransport.createPair()
        let controller = SessionController(
            transport: client,
            applier: applier,
            outbox: outbox,
            renderer: renderer
        )
        controller.attachRenderer(renderer)
        try await controller.start()
        try await handshakeAndMount(
            controller: controller,
            server: server,
            applier: applier,
            renderer: renderer,
            sessionId: "debounce-stop-replaced"
        )

        let handle = try #require(renderer.registry.handle(for: editorID))
        let adapter = try #require(handle.textAdapter)
        let field = try #require(handle.view as? NSTextField)
        await controller.stop()
        field.stringValue = "discard me"
        adapter.notifyTextDidChangeForTests()
        adapter.notifyEndEditingForTests()
        #expect(renderer.textEditingSession.hasUnsentSuccessorDraft(for: editorID))

        let (resumeClient, resumeServer) = await PipeTransport.createPair()
        let resumed = SessionController(
            transport: resumeClient,
            applier: applier,
            outbox: outbox,
            renderer: renderer,
            sessionId: "debounce-stop-replaced"
        )
        resumed.attachRenderer(renderer)
        let collector = EventCollector()
        await collector.start(draining: resumeServer)
        try await resumed.start()

        var resync = SRUIServerResyncRequired()
        resync.sessionID = "debounce-replacement"
        resync.snapshotRevision = 2
        resync.reason = "replacement"
        resync.continuity = .replaced
        var resyncMessage = SRUIMessage()
        resyncMessage.serverResyncRequired = resync
        await resumed.handleIncomingMessage(resyncMessage)

        let snapshot = Transaction(
            baseRevision: .initial,
            newRevision: Revision(2),
            operations: [
                .createNode(id: surfaceID, nodeType: .surface),
                .createNode(
                    id: editorID,
                    nodeType: .textInput,
                    parentID: surfaceID,
                    properties: [
                        Property(property: .value, value: .string("authoritative")),
                    ]
                ),
            ]
        )
        var snapshotMessage = SRUIMessage()
        snapshotMessage.transaction = snapshot.toWire()
        await resumed.handleIncomingMessage(snapshotMessage)

        try await waitUntil(description: "replacement text mounted") {
            guard let replacement = renderer.registry.handle(for: editorID)?.view as? NSTextField else {
                return false
            }
            return replacement.stringValue == "authoritative"
        }
        #expect(renderer.textEditingSession.hasUnsentSuccessorDraft(for: editorID) == false)
        #expect(await collector.eventCount() == 0)

        await resumed.stop()
        await collector.stop()
        await server.close()
        await resumeServer.close()
    }

    @Test("Assignment is recorded before an ambiguous input send failure")
    @MainActor
    func ambiguousSendCannotMisclassifyEchoAsCorrection() async throws {
        let (clientPipe, serverTransport) = await PipeTransport.createPair()
        let ambiguous = DeliverThenThrowInputTransport(inner: clientPipe)
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
        renderer.textEditingSession.debounceNanoseconds = 0
        let outbox = EventOutbox()
        let controller = SessionController(
            transport: ambiguous,
            applier: applier,
            outbox: outbox,
            renderer: renderer
        )
        controller.attachRenderer(renderer)
        try await controller.start()
        try await handshakeAndMount(
            controller: controller,
            server: serverTransport,
            applier: applier,
            renderer: renderer,
            sessionId: "text-ambiguous-send"
        )

        let handle = try #require(renderer.registry.handle(for: editorID))
        let adapter = try #require(handle.textAdapter)
        let field = try #require(handle.view as? NSTextField)
        let collector = EventCollector()
        await collector.start(draining: serverTransport)

        field.stringValue = "foo"
        adapter.notifyTextDidChangeForTests()
        adapter.notifyEndEditingForTests()
        let first = try await waitForTextEvent(collector)

        field.stringValue = "food"
        adapter.notifyTextDidChangeForTests()
        adapter.notifyEndEditingForTests()
        try await waitUntil(description: "successor retained after ambiguous send") {
            renderer.textEditingSession.hasUnsentSuccessorDraft(for: editorID)
        }

        var echo = SRUIMessage()
        echo.transaction = Transaction(
            baseRevision: Revision(1),
            newRevision: Revision(2),
            operations: [
                .setProperty(id: editorID, property: .value, value: .string("foo")),
            ]
        ).toWire()
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(echo))
        try await waitUntil(description: "echo renders without replacing successor") {
            applier.lastAppliedRevision == Revision(2)
        }
        #expect(field.stringValue == "food")
        #expect(await collector.eventCount() == 1)

        var ack = SRUIServerEventAck()
        ack.clientInstanceID = outbox.clientInstanceId.bytes
        ack.eventID = first.eventId.bytes
        ack.lastProcessedEventSeq = first.eventSeq
        ack.status = .processed
        ack.revisionAfterEffect = 2
        ack.sessionID = "text-ambiguous-send"
        var ackMessage = SRUIMessage()
        ackMessage.serverEventAck = ack
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(ackMessage))

        let successor = try await waitForTextEvent(
            collector,
            matching: { $0.eventId != first.eventId }
        )
        #expect(successor.textArg == "food")

        await controller.stop()
        await collector.stop()
        await ambiguous.close()
        await serverTransport.close()
    }

    @Test("A correction during marked text retires its coalesced successor immediately")
    @MainActor
    func correctionDuringCompositionRetiresSuccessorBeforeAck() async throws {
        let (clientPipe, serverTransport) = await PipeTransport.createPair()
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
        renderer.textEditingSession.debounceNanoseconds = 0
        let outbox = EventOutbox()
        let controller = SessionController(
            transport: clientPipe,
            applier: applier,
            outbox: outbox,
            renderer: renderer
        )
        controller.attachRenderer(renderer)
        try await controller.start()
        try await handshakeAndMount(
            controller: controller,
            server: serverTransport,
            applier: applier,
            renderer: renderer,
            sessionId: "composition-correction"
        )

        let handle = try #require(renderer.registry.handle(for: editorID))
        let adapter = try #require(handle.textAdapter)
        let field = try #require(handle.view as? NSTextField)
        let collector = EventCollector()
        await collector.start(draining: serverTransport)

        field.stringValue = "E1"
        adapter.notifyTextDidChangeForTests()
        adapter.notifyEndEditingForTests()
        let first = try await waitForTextEvent(collector)
        #expect(first.textArg == "E1")

        // E2 is flushed into the coalescing slot behind assigned E1, so it has no event identity.
        field.stringValue = "E2 coalesced"
        adapter.notifyTextDidChangeForTests()
        adapter.notifyEndEditingForTests()
        try await waitUntil(description: "E2 retained in the coalescing slot") {
            renderer.textEditingSession.hasUnsentSuccessorDraft(for: editorID)
        }
        #expect(renderer.textEditingSession.hasUnsentSuccessorDraft(for: editorID))

        // E3 is visible marked text. It must remain local while the authoritative correction is
        // classified, and E2 must be retired before E1's ACK is allowed to promote anything.
        adapter.compositionOverride = true
        field.stringValue = "E3 marked"
        adapter.notifyTextDidChangeForTests()
        #expect(adapter.isComposing)
        #expect(field.stringValue == "E3 marked")

        var correction = SRUIMessage()
        correction.transaction = Transaction(
            baseRevision: Revision(1),
            newRevision: Revision(2),
            operations: [
                .setProperty(
                    id: editorID,
                    property: .value,
                    value: .string("corrected")
                ),
            ]
        ).toWire()
        await controller.handleIncomingMessage(correction)

        #expect(applier.lastAppliedRevision == Revision(2))
        #expect(field.stringValue == "E3 marked")
        #expect(renderer.textEditingSession.localValue(for: editorID) == "E3 marked")
        #expect(renderer.textEditingSession.lastKnownAuthoritative(for: editorID) == "corrected")
        #expect(
            renderer.textEditingSession.hasUnsentSuccessorDraft(for: editorID) == false
        )
        try await waitUntil(description: "correction invalidated E2 before acknowledgement") {
            renderer.textEditingSession.hasUnsentSuccessorDraft(for: editorID) == false
        }

        var acknowledgement = SRUIServerEventAck()
        acknowledgement.clientInstanceID = outbox.clientInstanceId.bytes
        acknowledgement.eventID = first.eventId.bytes
        acknowledgement.lastProcessedEventSeq = first.eventSeq
        acknowledgement.status = .rejected
        acknowledgement.revisionAfterEffect = 2
        acknowledgement.rejectReason = "normalized"
        acknowledgement.sessionID = "composition-correction"
        var acknowledgementMessage = SRUIMessage()
        acknowledgementMessage.serverEventAck = acknowledgement
        await controller.handleIncomingMessage(acknowledgementMessage)

        #expect(field.stringValue == "E3 marked")
        #expect(renderer.textEditingSession.lastKnownAuthoritative(for: editorID) == "corrected")
        #expect(await outbox.pendingCount == 0)
        #expect(renderer.textEditingSession.hasUnsentSuccessorDraft(for: editorID) == false)
        #expect(await collector.events().filter { $0.eventType == .EVENT_TEXT_EDIT }.count == 1)

        adapter.compositionOverride = false
        adapter.notifyTextDidChangeForTests()

        #expect(field.stringValue == "corrected")
        #expect(renderer.textEditingSession.localValue(for: editorID) == "corrected")
        #expect(await collector.events().filter { $0.eventType == .EVENT_TEXT_EDIT }.count == 1)

        await controller.stop()
        await collector.stop()
        await clientPipe.close()
        await serverTransport.close()
    }

    @Test("An accepted echo during marked text cannot overwrite the committed IME result")
    @MainActor
    func acceptedEchoDuringCompositionDoesNotBecomeDeferredCorrection() async throws {
        let (clientPipe, serverTransport) = await PipeTransport.createPair()
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
        renderer.textEditingSession.debounceNanoseconds = 0
        let outbox = EventOutbox()
        let controller = SessionController(
            transport: clientPipe,
            applier: applier,
            outbox: outbox,
            renderer: renderer
        )
        controller.attachRenderer(renderer)
        try await controller.start()
        try await handshakeAndMount(
            controller: controller,
            server: serverTransport,
            applier: applier,
            renderer: renderer,
            sessionId: "composition-echo"
        )

        let handle = try #require(renderer.registry.handle(for: editorID))
        let adapter = try #require(handle.textAdapter)
        let field = try #require(handle.view as? NSTextField)
        let collector = EventCollector()
        await collector.start(draining: serverTransport)

        field.stringValue = "E1"
        adapter.notifyTextDidChangeForTests()
        adapter.notifyEndEditingForTests()
        let first = try await waitForTextEvent(collector)
        #expect(first.textArg == "E1")

        adapter.compositionOverride = true
        field.stringValue = "E3 marked"
        adapter.notifyTextDidChangeForTests()
        #expect(adapter.isComposing)
        #expect(field.stringValue == "E3 marked")

        var echo = SRUIMessage()
        echo.transaction = Transaction(
            baseRevision: Revision(1),
            newRevision: Revision(2),
            operations: [
                .setProperty(id: editorID, property: .value, value: .string("E1")),
            ]
        ).toWire()
        await controller.handleIncomingMessage(echo)

        #expect(applier.lastAppliedRevision == Revision(2))
        #expect(field.stringValue == "E3 marked")
        #expect(renderer.textEditingSession.lastKnownAuthoritative(for: editorID) == "E1")
        #expect(await collector.events().filter { $0.eventType == .EVENT_TEXT_EDIT }.count == 1)

        var acknowledgement = SRUIServerEventAck()
        acknowledgement.clientInstanceID = outbox.clientInstanceId.bytes
        acknowledgement.eventID = first.eventId.bytes
        acknowledgement.lastProcessedEventSeq = first.eventSeq
        acknowledgement.status = .processed
        acknowledgement.revisionAfterEffect = 2
        acknowledgement.sessionID = "composition-echo"
        var acknowledgementMessage = SRUIMessage()
        acknowledgementMessage.serverEventAck = acknowledgement
        await controller.handleIncomingMessage(acknowledgementMessage)

        #expect(field.stringValue == "E3 marked")
        #expect(await outbox.pendingCount == 0)

        adapter.compositionOverride = false
        adapter.notifyTextDidChangeForTests()

        let committedIME = try await waitForTextEvent(
            collector,
            matching: { $0.eventId != first.eventId }
        )
        #expect(committedIME.textArg == "E3 marked")
        #expect(committedIME.editSeq?.rawValue == 2)
        #expect(field.stringValue == "E3 marked")
        #expect(
            await collector.events().filter { $0.eventType == .EVENT_TEXT_EDIT }.map(\.textArg)
                == ["E1", "E3 marked"]
        )

        await controller.stop()
        await collector.stop()
        await clientPipe.close()
        await serverTransport.close()
    }

    @Test("Structural remount restores deferred correction instead of stale marked text")
    @MainActor
    func structuralRemountUsesDeferredCorrectionPrecedence() async throws {
        let (clientPipe, serverTransport) = await PipeTransport.createPair()
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
        renderer.textEditingSession.debounceNanoseconds = 0
        let outbox = EventOutbox()
        let controller = SessionController(
            transport: clientPipe,
            applier: applier,
            outbox: outbox,
            renderer: renderer
        )
        controller.attachRenderer(renderer)
        try await controller.start()
        try await handshakeAndMount(
            controller: controller,
            server: serverTransport,
            applier: applier,
            renderer: renderer,
            sessionId: "composition-structural-remount"
        )

        let originalHandle = try #require(renderer.registry.handle(for: editorID))
        let originalAdapter = try #require(originalHandle.textAdapter)
        let originalField = try #require(originalHandle.view as? NSTextField)
        let collector = EventCollector()
        await collector.start(draining: serverTransport)

        originalField.stringValue = "E1 submitted"
        originalAdapter.notifyTextDidChangeForTests()
        originalAdapter.notifyEndEditingForTests()
        let first = try await waitForTextEvent(collector)
        #expect(first.textArg == "E1 submitted")

        originalField.stringValue = "E2 coalesced"
        originalAdapter.notifyTextDidChangeForTests()
        originalAdapter.notifyEndEditingForTests()
        try await waitUntil(description: "E2 retained behind assigned E1") {
            renderer.textEditingSession.hasUnsentSuccessorDraft(for: editorID)
        }

        originalAdapter.compositionOverride = true
        originalField.stringValue = "E3 marked"
        originalAdapter.notifyTextDidChangeForTests()
        #expect(renderer.textEditingSession.isComposing(for: editorID))

        // With no deferred correction yet, destroying the field editor must restore the newest
        // flushed-but-unassigned value, not the marked preedit or the older assigned submit.
        var firstStructural = SRUIMessage()
        firstStructural.transaction = Transaction(
            baseRevision: Revision(1),
            newRevision: Revision(2),
            operations: [
                .createNode(
                    id: NodeId(20),
                    nodeType: .text,
                    parentID: surfaceID,
                    properties: [
                        Property(property: .text, value: .string("first remount")),
                    ]
                ),
            ]
        ).toWire()
        await controller.handleIncomingMessage(firstStructural)

        let firstRemountedHandle = try #require(renderer.registry.handle(for: editorID))
        let firstRemountedAdapter = try #require(firstRemountedHandle.textAdapter)
        let firstRemountedField = try #require(firstRemountedHandle.view as? NSTextField)
        #expect(ObjectIdentifier(firstRemountedField) != ObjectIdentifier(originalField))
        #expect(firstRemountedField.stringValue == "E2 coalesced")
        #expect(renderer.textEditingSession.localValue(for: editorID) == "E2 coalesced")
        #expect(renderer.textEditingSession.isComposing(for: editorID) == false)
        #expect(renderer.textEditingSession.hasUnsentSuccessorDraft(for: editorID))
        #expect(await collector.events().filter { $0.eventType == .EVENT_TEXT_EDIT }.count == 1)

        // A real correction is classified while the replacement field has active marked text.
        // Only its native assignment is deferred.
        firstRemountedAdapter.compositionOverride = true
        firstRemountedField.stringValue = "E4 marked"
        firstRemountedAdapter.notifyTextDidChangeForTests()

        var correction = SRUIMessage()
        correction.transaction = Transaction(
            baseRevision: Revision(2),
            newRevision: Revision(3),
            operations: [
                .setProperty(
                    id: editorID,
                    property: .value,
                    value: .string("corrected")
                ),
            ]
        ).toWire()
        await controller.handleIncomingMessage(correction)

        #expect(firstRemountedField.stringValue == "E4 marked")
        #expect(renderer.textEditingSession.localValue(for: editorID) == "E4 marked")
        #expect(renderer.textEditingSession.lastKnownAuthoritative(for: editorID) == "corrected")
        try await waitUntil(description: "correction retired E2 before structural remount") {
            renderer.textEditingSession.hasUnsentSuccessorDraft(for: editorID) == false
        }

        var secondStructural = SRUIMessage()
        secondStructural.transaction = Transaction(
            baseRevision: Revision(3),
            newRevision: Revision(4),
            operations: [
                .createNode(
                    id: NodeId(21),
                    nodeType: .text,
                    parentID: surfaceID,
                    properties: [
                        Property(property: .text, value: .string("second remount")),
                    ]
                ),
            ]
        ).toWire()
        await controller.handleIncomingMessage(secondStructural)

        let correctedHandle = try #require(renderer.registry.handle(for: editorID))
        let correctedField = try #require(correctedHandle.view as? NSTextField)
        #expect(ObjectIdentifier(correctedField) != ObjectIdentifier(firstRemountedField))
        #expect(correctedField.stringValue == "corrected")
        #expect(renderer.textEditingSession.localValue(for: editorID) == "corrected")
        #expect(renderer.textEditingSession.lastKnownAuthoritative(for: editorID) == "corrected")
        #expect(renderer.textEditingSession.isComposing(for: editorID) == false)

        // Retiring E1 after the remount must use the already-updated corrected baseline.
        var acknowledgement = SRUIServerEventAck()
        acknowledgement.clientInstanceID = outbox.clientInstanceId.bytes
        acknowledgement.eventID = first.eventId.bytes
        acknowledgement.lastProcessedEventSeq = first.eventSeq
        acknowledgement.status = .rejected
        acknowledgement.revisionAfterEffect = 4
        acknowledgement.rejectReason = "normalized"
        acknowledgement.sessionID = "composition-structural-remount"
        var acknowledgementMessage = SRUIMessage()
        acknowledgementMessage.serverEventAck = acknowledgement
        await controller.handleIncomingMessage(acknowledgementMessage)

        #expect(correctedField.stringValue == "corrected")
        #expect(await outbox.pendingCount == 0)
        #expect(renderer.textEditingSession.hasUnsentSuccessorDraft(for: editorID) == false)
        #expect(await collector.events().filter { $0.eventType == .EVENT_TEXT_EDIT }.count == 1)

        await controller.stop()
        await collector.stop()
        await clientPipe.close()
        await serverTransport.close()
    }

    @Test("Rejected input converges to the published value without a feedback event")
    @MainActor
    func rejectedAckConvergesWithoutFeedbackEvent() async throws {
        let (clientPipe, serverTransport) = await PipeTransport.createPair()
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
        renderer.textEditingSession.debounceNanoseconds = 0
        let outbox = EventOutbox()
        let controller = SessionController(
            transport: clientPipe,
            applier: applier,
            outbox: outbox,
            renderer: renderer
        )
        controller.attachRenderer(renderer)
        try await controller.start()
        try await handshakeAndMount(
            controller: controller,
            server: serverTransport,
            applier: applier,
            renderer: renderer,
            sessionId: "text-reject"
        )

        let handle = try #require(renderer.registry.handle(for: editorID))
        let adapter = try #require(handle.textAdapter)
        let field = try #require(handle.view as? NSTextField)
        let collector = EventCollector()
        await collector.start(draining: serverTransport)

        field.stringValue = "nope"
        adapter.notifyTextDidChangeForTests()
        adapter.notifyEndEditingForTests()

        let submitted = try await waitForTextEvent(collector)
        #expect(submitted.textArg == "nope")

        var correction = SRUIMessage()
        correction.transaction = Transaction(
            baseRevision: Revision(1),
            newRevision: Revision(2),
            operations: [
                .setProperty(id: editorID, property: .value, value: .string("corrected")),
                .setProperty(
                    id: editorID,
                    property: .validationState,
                    value: .enumToken(StandardValidationState.error.enumToken)
                ),
            ]
        ).toWire()
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(correction))
        try await AsyncTestSupport.eventually(description: "correction applied") {
            field.stringValue == "corrected"
        }
        #expect(adapter.validationState == .error)

        var ack = SRUIServerEventAck()
        ack.clientInstanceID = outbox.clientInstanceId.bytes
        ack.eventID = submitted.eventId.bytes
        ack.lastProcessedEventSeq = submitted.eventSeq
        ack.status = .rejected
        ack.revisionAfterEffect = 2
        ack.rejectReason = "not allowed"
        ack.sessionID = "text-reject"
        var ackMessage = SRUIMessage()
        ackMessage.serverEventAck = ack
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(ackMessage))

        try await waitUntil(description: "rejected acknowledgement retired") {
            await outbox.pendingCount == 0
        }
        #expect(field.stringValue == "corrected")
        #expect(await collector.events().filter { $0.eventType == .EVENT_TEXT_EDIT }.count == 1)

        await controller.stop()
        await collector.stop()
        await clientPipe.close()
        await serverTransport.close()
    }

    @Test("Rejected ack without a transaction reverts native text when there is no successor")
    @MainActor
    func rejectedAckWithoutTransactionRevertsNative() async throws {
        let (clientPipe, serverTransport) = await PipeTransport.createPair()
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
        renderer.textEditingSession.debounceNanoseconds = 0
        let outbox = EventOutbox()
        let controller = SessionController(
            transport: clientPipe,
            applier: applier,
            outbox: outbox,
            renderer: renderer
        )
        controller.attachRenderer(renderer)
        try await controller.start()
        try await handshakeAndMount(
            controller: controller,
            server: serverTransport,
            applier: applier,
            renderer: renderer,
            sessionId: "text-reject-no-tx"
        )

        let handle = try #require(renderer.registry.handle(for: editorID))
        let adapter = try #require(handle.textAdapter)
        let field = try #require(handle.view as? NSTextField)
        let collector = EventCollector()
        await collector.start(draining: serverTransport)

        field.stringValue = "nope"
        adapter.notifyTextDidChangeForTests()
        adapter.notifyEndEditingForTests()

        let submitted = try await waitForTextEvent(collector)
        #expect(submitted.textArg == "nope")

        var ack = SRUIServerEventAck()
        ack.clientInstanceID = outbox.clientInstanceId.bytes
        ack.eventID = submitted.eventId.bytes
        ack.lastProcessedEventSeq = submitted.eventSeq
        ack.status = .rejected
        ack.revisionAfterEffect = 1
        ack.rejectReason = "not allowed"
        ack.sessionID = "text-reject-no-tx"
        var ackMessage = SRUIMessage()
        ackMessage.serverEventAck = ack
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(ackMessage))

        try await AsyncTestSupport.eventually(description: "native reverts without a transaction") {
            field.stringValue == ""
        }
        #expect(await collector.events().filter { $0.eventType == .EVENT_TEXT_EDIT }.count == 1)

        await controller.stop()
        await collector.stop()
        await clientPipe.close()
        await serverTransport.close()
    }

    @Test("A stale rejected ack cannot revert a replacement binding's assigned value")
    @MainActor
    func staleRejectedAckCannotRevertReplacementAssignment() async throws {
        let sessionID = "stale-rejected-ack"
        let (oldClient, oldServer) = await PipeTransport.createPair()
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
        renderer.textEditingSession.debounceNanoseconds = 0
        let outbox = EventOutbox()
        let old = SessionController(
            transport: oldClient,
            applier: applier,
            outbox: outbox,
            renderer: renderer
        )
        old.attachRenderer(renderer)
        try await old.start()
        try await handshakeAndMount(
            controller: old,
            server: oldServer,
            applier: applier,
            renderer: renderer,
            sessionId: sessionID
        )

        let oldCollector = EventCollector()
        await oldCollector.start(draining: oldServer)
        let oldHandle = try #require(renderer.registry.handle(for: editorID))
        let oldAdapter = try #require(oldHandle.textAdapter)
        let oldField = try #require(oldHandle.view as? NSTextField)
        oldField.stringValue = "rejected old value"
        oldAdapter.notifyTextDidChangeForTests()
        oldAdapter.notifyEndEditingForTests()
        let rejectedEdit = try await waitForTextEvent(oldCollector)

        let lifecycleGate = TextLifecycleGate()
        await outbox.setNativeTextLifecycleWillHopForTesting {
            await lifecycleGate.pause()
        }
        var ack = SRUIServerEventAck()
        ack.clientInstanceID = outbox.clientInstanceId.bytes
        ack.eventID = rejectedEdit.eventId.bytes
        ack.lastProcessedEventSeq = rejectedEdit.eventSeq
        ack.status = .rejected
        ack.revisionAfterEffect = 1
        ack.rejectReason = "old connection rejects"
        ack.sessionID = sessionID
        var ackMessage = SRUIMessage()
        ackMessage.serverEventAck = ack
        let staleAcknowledgement = Task {
            await old.handleIncomingMessage(ackMessage)
        }
        await lifecycleGate.waitUntilPaused()

        let (replacementClient, replacementServer) = await PipeTransport.createPair()
        let replacementCollector = EventCollector()
        await replacementCollector.start(draining: replacementServer)
        let replacement = SessionController(
            transport: replacementClient,
            applier: applier,
            outbox: outbox,
            renderer: renderer,
            sessionId: sessionID
        )
        replacement.attachRenderer(renderer)
        try await replacement.start()

        // The new binding may resolve the retained barrier; only the paused old binding must not
        // apply its rejected-ACK callback after the new native assignment exists.
        await outbox.setNativeTextLifecycleWillHopForTesting(nil)
        var resumeOK = SRUIServerResumeOk()
        resumeOK.sessionID = sessionID
        resumeOK.lastProcessedEventSeq = rejectedEdit.eventSeq
        var resumeOKMessage = SRUIMessage()
        resumeOKMessage.serverResumeOk = resumeOK
        await replacement.handleIncomingMessage(resumeOKMessage)
        #expect(replacement.isEventDispatchEnabled)

        let replacementHandle = try #require(renderer.registry.handle(for: editorID))
        let replacementAdapter = try #require(replacementHandle.textAdapter)
        let replacementField = try #require(replacementHandle.view as? NSTextField)
        replacementField.stringValue = "new assigned value"
        replacementAdapter.notifyTextDidChangeForTests()
        replacementAdapter.notifyEndEditingForTests()
        let replacementEdit = try await waitForTextEvent(replacementCollector)
        #expect(replacementEdit.textArg == "new assigned value")

        await lifecycleGate.release()
        await staleAcknowledgement.value

        #expect(replacementField.stringValue == "new assigned value")
        let assigned = await outbox.assignedTextEditEvents()
        #expect(assigned.contains { $0.eventId == replacementEdit.eventId })

        await old.stop()
        await replacement.stop()
        await oldCollector.stop()
        await replacementCollector.stop()
        await oldServer.close()
        await replacementServer.close()
    }

    @Test("Rejected ack without a transaction keeps a successor draft and promotes it")
    @MainActor
    func rejectedAckWithoutTransactionPromotesSuccessor() async throws {
        let (clientPipe, serverTransport) = await PipeTransport.createPair()
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
        renderer.textEditingSession.debounceNanoseconds = 0
        let outbox = EventOutbox()
        let controller = SessionController(
            transport: clientPipe,
            applier: applier,
            outbox: outbox,
            renderer: renderer
        )
        controller.attachRenderer(renderer)
        try await controller.start()
        try await handshakeAndMount(
            controller: controller,
            server: serverTransport,
            applier: applier,
            renderer: renderer,
            sessionId: "text-reject-promote"
        )

        let handle = try #require(renderer.registry.handle(for: editorID))
        let adapter = try #require(handle.textAdapter)
        let field = try #require(handle.view as? NSTextField)
        let collector = EventCollector()
        await collector.start(draining: serverTransport)

        field.stringValue = "foo"
        adapter.notifyTextDidChangeForTests()
        adapter.notifyEndEditingForTests()
        let first = try await waitForTextEvent(collector)
        #expect(first.textArg == "foo")

        field.stringValue = "food"
        adapter.notifyTextDidChangeForTests()
        adapter.notifyEndEditingForTests()
        try await waitUntil(description: "successor draft queued") {
            renderer.textEditingSession.hasUnsentSuccessorDraft(for: editorID)
        }

        var ack = SRUIServerEventAck()
        ack.clientInstanceID = outbox.clientInstanceId.bytes
        ack.eventID = first.eventId.bytes
        ack.lastProcessedEventSeq = first.eventSeq
        ack.status = .rejected
        ack.revisionAfterEffect = 1
        ack.rejectReason = "not allowed"
        ack.sessionID = "text-reject-promote"
        var ackMessage = SRUIMessage()
        ackMessage.serverEventAck = ack
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(ackMessage))

        let second = try await waitForTextEvent(collector, matching: { $0.eventId != first.eventId })
        #expect(second.textArg == "food")
        #expect(field.stringValue == "food")

        await controller.stop()
        await collector.stop()
        await clientPipe.close()
        await serverTransport.close()
    }

    @Test("A rendered rejection invalidates its successor before acknowledgement promotion")
    @MainActor
    func correctionInvalidationPrecedesAcknowledgementPromotion() async throws {
        let (clientPipe, serverTransport) = await PipeTransport.createPair()
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
        renderer.textEditingSession.debounceNanoseconds = 0
        let outbox = EventOutbox()
        let controller = SessionController(
            transport: clientPipe,
            applier: applier,
            outbox: outbox,
            renderer: renderer
        )
        controller.attachRenderer(renderer)
        try await controller.start()
        try await handshakeAndMount(
            controller: controller,
            server: serverTransport,
            applier: applier,
            renderer: renderer,
            sessionId: "text-correction-invalidation"
        )

        let handle = try #require(renderer.registry.handle(for: editorID))
        let adapter = try #require(handle.textAdapter)
        let field = try #require(handle.view as? NSTextField)
        let collector = EventCollector()
        await collector.start(draining: serverTransport)

        field.stringValue = "first"
        adapter.notifyTextDidChangeForTests()
        adapter.notifyEndEditingForTests()
        let first = try await waitForTextEvent(collector)

        field.stringValue = "stale-successor"
        adapter.notifyTextDidChangeForTests()
        adapter.notifyEndEditingForTests()
        try await waitUntil(description: "successor draft queued behind first edit") {
            renderer.textEditingSession.hasUnsentSuccessorDraft(for: editorID)
        }

        var acknowledgement = SRUIServerEventAck()
        acknowledgement.clientInstanceID = outbox.clientInstanceId.bytes
        acknowledgement.eventID = first.eventId.bytes
        acknowledgement.lastProcessedEventSeq = first.eventSeq
        acknowledgement.status = .rejected
        acknowledgement.revisionAfterEffect = 2
        acknowledgement.rejectReason = "corrected"
        acknowledgement.sessionID = "text-correction-invalidation"
        var acknowledgementMessage = SRUIMessage()
        acknowledgementMessage.serverEventAck = acknowledgement
        await controller.handleIncomingMessage(acknowledgementMessage)


        var correction = SRUIMessage()
        correction.transaction = Transaction(
            baseRevision: Revision(1),
            newRevision: Revision(2),
            operations: [
                .setProperty(
                    id: editorID,
                    property: .value,
                    value: .string("corrected")
                ),
            ]
        ).toWire()
        await controller.handleIncomingMessage(correction)


        try await waitUntil(description: "correction converged without successor promotion") {
            let hasDraft = renderer.textEditingSession.hasUnsentSuccessorDraft(for: editorID)
            let pendingCount = await outbox.pendingCount
            return field.stringValue == "corrected"
                && hasDraft == false
                && pendingCount == 0
        }
        #expect(
            await collector.events().filter { $0.eventType == .EVENT_TEXT_EDIT }.count == 1
        )
        #expect(field.stringValue == "corrected")

        await controller.stop()
        await collector.stop()
        await clientPipe.close()
        await serverTransport.close()
    }

    @Test("A correction invalidation remains authoritative across connection rebinding")
    @MainActor
    func correctionInvalidationSurvivesRebind() async throws {
        let (clientPipe, serverTransport) = await PipeTransport.createPair()
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
        renderer.textEditingSession.debounceNanoseconds = 0
        let outbox = EventOutbox()
        let controller = SessionController(
            transport: clientPipe,
            applier: applier,
            outbox: outbox,
            renderer: renderer
        )
        controller.attachRenderer(renderer)
        try await controller.start()
        try await handshakeAndMount(
            controller: controller,
            server: serverTransport,
            applier: applier,
            renderer: renderer,
            sessionId: "correction-rebind"
        )

        let handle = try #require(renderer.registry.handle(for: editorID))
        let adapter = try #require(handle.textAdapter)
        let field = try #require(handle.view as? NSTextField)
        let collector = EventCollector()
        await collector.start(draining: serverTransport)

        field.stringValue = "first"
        adapter.notifyTextDidChangeForTests()
        adapter.notifyEndEditingForTests()
        let first = try await waitForTextEvent(collector)

        field.stringValue = "must-not-replay"
        adapter.notifyTextDidChangeForTests()
        adapter.notifyEndEditingForTests()
        try await waitUntil(description: "successor draft queued before correction") {
            renderer.textEditingSession.hasUnsentSuccessorDraft(for: editorID)
        }

        var acknowledgement = SRUIServerEventAck()
        acknowledgement.clientInstanceID = outbox.clientInstanceId.bytes
        acknowledgement.eventID = first.eventId.bytes
        acknowledgement.lastProcessedEventSeq = first.eventSeq
        acknowledgement.status = .rejected
        acknowledgement.revisionAfterEffect = 2
        acknowledgement.sessionID = "correction-rebind"
        var acknowledgementMessage = SRUIMessage()
        acknowledgementMessage.serverEventAck = acknowledgement
        await controller.handleIncomingMessage(acknowledgementMessage)

        let renderGate = TextLifecycleGate()
        controller.rendererDidRenderInterceptorForTesting = {
            await renderGate.pause()
        }
        var correction = SRUIMessage()
        correction.transaction = Transaction(
            baseRevision: Revision(1),
            newRevision: Revision(2),
            operations: [
                .setProperty(
                    id: editorID,
                    property: .value,
                    value: .string("corrected")
                ),
            ]
        ).toWire()
        let correctionTask = Task {
            await controller.handleIncomingMessage(correction)
        }
        await renderGate.waitUntilPaused()
        #expect(field.stringValue == "corrected")

        let (replacementClient, replacementServer) = await PipeTransport.createPair()
        let replacement = SessionController(
            transport: replacementClient,
            applier: applier,
            outbox: outbox,
            sessionId: "correction-rebind"
        )
        try await replacement.start()
        try await waitUntil(description: "rebinding consumed buffered correction invalidation") {
            renderer.textEditingSession.hasUnsentSuccessorDraft(for: editorID) == false
        }

        controller.rendererDidRenderInterceptorForTesting = nil
        await renderGate.release()
        await correctionTask.value

        #expect(renderer.textEditingSession.hasUnsentSuccessorDraft(for: editorID) == false)
        #expect(
            await collector.events().filter { $0.eventType == .EVENT_TEXT_EDIT }.count == 1
        )

        await controller.stop()
        await replacement.stop()
        await collector.stop()
        await clientPipe.close()
        await serverTransport.close()
        await replacementClient.close()
        await replacementServer.close()
    }

    @Test("Processed ack waits through intervening transactions before promoting a successor")
    @MainActor
    func processedAckWaitsForEffectRevisionBeforePromotingSuccessor() async throws {
        let (clientPipe, serverTransport) = await PipeTransport.createPair()
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
        renderer.textEditingSession.debounceNanoseconds = 0
        let outbox = EventOutbox()
        let controller = SessionController(
            transport: clientPipe,
            applier: applier,
            outbox: outbox,
            renderer: renderer
        )
        controller.attachRenderer(renderer)
        try await controller.start()
        try await handshakeAndMount(
            controller: controller,
            server: serverTransport,
            applier: applier,
            renderer: renderer,
            sessionId: "text-ack-wait"
        )

        let handle = try #require(renderer.registry.handle(for: editorID))
        let adapter = try #require(handle.textAdapter)
        let field = try #require(handle.view as? NSTextField)
        let collector = EventCollector()
        await collector.start(draining: serverTransport)

        field.stringValue = "foo"
        adapter.notifyTextDidChangeForTests()
        adapter.notifyEndEditingForTests()
        let first = try await waitForTextEvent(collector)
        #expect(first.textArg == "foo")

        field.stringValue = "food"
        adapter.notifyTextDidChangeForTests()
        adapter.notifyEndEditingForTests()
        try await waitUntil(description: "successor draft queued") {
            renderer.textEditingSession.hasUnsentSuccessorDraft(for: editorID)
        }

        var ack = SRUIServerEventAck()
        ack.clientInstanceID = outbox.clientInstanceId.bytes
        ack.eventID = first.eventId.bytes
        ack.lastProcessedEventSeq = first.eventSeq
        ack.status = .processed
        ack.revisionAfterEffect = 3
        ack.sessionID = "text-ack-wait"
        var ackMessage = SRUIMessage()
        ackMessage.serverEventAck = ack
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(ackMessage))

        try await waitUntil(description: "processed acknowledgement installed its revision barrier") {
            await outbox.pendingCount == 0
        }
        #expect(await collector.events().filter { $0.eventType == .EVENT_TEXT_EDIT }.count == 1)
        #expect(field.stringValue == "food")

        var intervening = SRUIMessage()
        intervening.transaction = Transaction(
            baseRevision: Revision(1),
            newRevision: Revision(2),
            operations: [
                .createNode(
                    id: NodeId(99),
                    nodeType: .text,
                    parentID: surfaceID,
                    properties: [Property(property: .text, value: .string("structural"))]
                ),
            ]
        ).toWire()
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(intervening))
        try await AsyncTestSupport.eventually(description: "intervening structural revision remounted") {
            applier.lastAppliedRevision == Revision(2)
                && renderer.registry.handle(for: NodeId(99)) != nil
        }
        let remountedHandle = try #require(renderer.registry.handle(for: editorID))
        let remountedField = try #require(remountedHandle.view as? NSTextField)
        #expect(remountedField !== field)
        #expect(await collector.events().filter { $0.eventType == .EVENT_TEXT_EDIT }.count == 1)
        #expect(remountedField.stringValue == "food")

        var echo = SRUIMessage()
        echo.transaction = Transaction(
            baseRevision: Revision(2),
            newRevision: Revision(3),
            operations: [
                .setProperty(id: editorID, property: .value, value: .string("foo")),
            ]
        ).toWire()
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(echo))

        let second = try await waitForTextEvent(collector, matching: { $0.eventId != first.eventId })
        #expect(second.textArg == "food")
        #expect(remountedField.stringValue == "food")

        await controller.stop()
        await collector.stop()
        await clientPipe.close()
        await serverTransport.close()
    }

    @Test("Forced same-session resync discards pre-snapshot drafts without resetting edit_seq")
    @MainActor
    func forcedResyncDiscardsOnlyPreSnapshotDrafts() async throws {
        let (clientPipe, serverTransport) = await PipeTransport.createPair()
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
        renderer.textEditingSession.debounceNanoseconds = 0
        let outbox = EventOutbox()
        let controller = SessionController(
            transport: clientPipe,
            applier: applier,
            outbox: outbox,
            renderer: renderer
        )
        controller.attachRenderer(renderer)
        try await controller.start()
        try await handshakeAndMount(
            controller: controller,
            server: serverTransport,
            applier: applier,
            renderer: renderer,
            sessionId: "text-resync"
        )

        let oldHandle = try #require(renderer.registry.handle(for: editorID))
        let oldAdapter = try #require(oldHandle.textAdapter)
        let oldField = try #require(oldHandle.view as? NSTextField)
        let collector = EventCollector()
        await collector.start(draining: serverTransport)

        var resync = SRUIServerResyncRequired()
        resync.sessionID = "text-resync"
        resync.snapshotRevision = 2
        resync.reason = "forced test resync"
        resync.continuity = .sameSession
        resync.lastProcessedEventSeq = 0
        var resyncMessage = SRUIMessage()
        resyncMessage.serverResyncRequired = resync
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(resyncMessage))

        try await waitUntil(description: "dispatch suspended for snapshot") {
            !controller.isEventDispatchEnabled
        }

        oldField.stringValue = "stale local"
        oldAdapter.notifyTextDidChangeForTests()
        oldAdapter.notifyEndEditingForTests()
        try await waitUntil(description: "pre-snapshot draft queued") {
            renderer.textEditingSession.hasUnsentSuccessorDraft(for: editorID)
        }
        #expect(renderer.textEditingSession.nextEditSeqValue(for: editorID) == 2)

        let actionGate = TextLifecycleGate()
        controller.interactionWillEnterOutboxForTesting = {
            await actionGate.pause()
        }
        renderer.onInteraction?(.activate(nodeID: NodeId(99)))
        await actionGate.waitUntilPaused()
        controller.interactionWillEnterOutboxForTesting = nil

        var snapshotMessage = SRUIMessage()
        snapshotMessage.transaction = Transaction(
            baseRevision: .initial,
            newRevision: Revision(2),
            operations: [
                .createNode(id: surfaceID, nodeType: .surface),
                .createNode(
                    id: editorID,
                    nodeType: .textInput,
                    parentID: surfaceID,
                    properties: [
                        Property(property: .value, value: .string("authoritative")),
                    ]
                ),
            ]
        ).toWire()
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(snapshotMessage))

        try await waitUntil(description: "authoritative snapshot mounted") {
            guard controller.isEventDispatchEnabled,
                  let handle = renderer.registry.handle(for: editorID),
                  let field = handle.view as? NSTextField else {
                return false
            }
            return field.stringValue == "authoritative"
        }
        await actionGate.release()
        renderer.onInteraction?(.activate(nodeID: NodeId(100)))
        try await waitUntil(description: "post-resync sentinel followed the stale dispatch tail") {
            await collector.events().contains {
                $0.eventType == .EVENT_ACTIVATE && $0.nodeId == NodeId(100)
            }
        }
        #expect(renderer.textEditingSession.hasUnsentSuccessorDraft(for: editorID) == false)
        #expect(await collector.events().filter { $0.eventType == .EVENT_TEXT_EDIT }.isEmpty)
        #expect(
            await collector.events().filter { $0.eventType == .EVENT_ACTIVATE }.map(\.nodeId)
                == [NodeId(100)]
        )

        let newHandle = try #require(renderer.registry.handle(for: editorID))
        let newAdapter = try #require(newHandle.textAdapter)
        let newField = try #require(newHandle.view as? NSTextField)
        newField.stringValue = "fresh local"
        newAdapter.notifyTextDidChangeForTests()
        newAdapter.notifyEndEditingForTests()

        let fresh = try await waitForTextEvent(collector)
        #expect(fresh.textArg == "fresh local")
        #expect(fresh.editSeq?.rawValue == 2)

        await controller.stop()
        await collector.stop()
        await clientPipe.close()
        await serverTransport.close()
    }

    @Test("Replacement rejects an interaction paused before outbox admission")
    @MainActor
    func replacementRejectsInteractionPausedBeforeOutboxAdmission() async throws {
        let (clientTransport, serverTransport) = await PipeTransport.createPair()
        let renderer = AppKitRenderer()
        renderer.textEditingSession.debounceNanoseconds = 0
        let applier = TransactionApplier()
        let outbox = EventOutbox()
        let controller = SessionController(
            transport: clientTransport,
            applier: applier,
            outbox: outbox,
            renderer: renderer
        )
        controller.attachRenderer(renderer)
        try await controller.start()
        try await handshakeAndMount(
            controller: controller,
            server: serverTransport,
            applier: applier,
            renderer: renderer,
            sessionId: "session-old"
        )

        let collector = EventCollector()
        await collector.start(draining: serverTransport)
        let admissionGate = TextLifecycleGate()
        controller.interactionWillEnterOutboxForTesting = {
            await admissionGate.pause()
        }

        renderer.textEditingSession.noteLocalValue(
            "expired local",
            nodeID: editorID,
            composing: false,
            flushImmediately: true
        )
        await admissionGate.waitUntilPaused()
        #expect(renderer.textEditingSession.hasUnsentSuccessorDraft(for: editorID))

        let manualGate = TextLifecycleGate()
        controller.interactionWillEnterOutboxForTesting = {
            await manualGate.pause()
        }
        let manualAction = Task {
            do {
                _ = try await controller.sendActivate(nodeId: NodeId(99))
                return false
            } catch {
                return true
            }
        }
        await manualGate.waitUntilPaused()

        var resync = SRUIServerResyncRequired()
        resync.sessionID = "session-new"
        resync.snapshotRevision = 1
        resync.reason = "replaced"
        resync.continuity = .replaced
        resync.lastProcessedEventSeq = 0
        var resyncMessage = SRUIMessage()
        resyncMessage.serverResyncRequired = resync
        await controller.handleIncomingMessage(resyncMessage)
        #expect(renderer.textEditingSession.hasUnsentSuccessorDraft(for: editorID) == false)

        var snapshotMessage = SRUIMessage()
        snapshotMessage.transaction = Transaction(
            baseRevision: .initial,
            newRevision: Revision(1),
            operations: [
                .createNode(id: surfaceID, nodeType: .surface),
                .createNode(
                    id: editorID,
                    nodeType: .textInput,
                    parentID: surfaceID,
                    properties: [
                        Property(property: .value, value: .string("authoritative")),
                    ]
                ),
            ]
        ).toWire()
        await controller.handleIncomingMessage(snapshotMessage)
        try await waitUntil(description: "replacement snapshot mounted") {
            guard controller.isEventDispatchEnabled,
                  let field = renderer.registry.handle(for: editorID)?.view as? NSTextField else {
                return false
            }
            return field.stringValue == "authoritative"
        }

        controller.interactionWillEnterOutboxForTesting = nil
        await admissionGate.release()
        await manualGate.release()
        #expect(await manualAction.value)
        renderer.onInteraction?(.activate(nodeID: NodeId(100)))
        try await waitUntil(description: "replacement sentinel followed the stale dispatch tail") {
            await collector.events().contains {
                $0.eventType == .EVENT_ACTIVATE && $0.nodeId == NodeId(100)
            }
        }

        #expect(renderer.textEditingSession.hasUnsentSuccessorDraft(for: editorID) == false)
        #expect(await outbox.assignedTextEditDescriptors().isEmpty)
        #expect(await collector.events().filter { $0.eventType == .EVENT_TEXT_EDIT }.isEmpty)
        #expect(
            await collector.events().filter { $0.eventType == .EVENT_ACTIVATE }.map(\.nodeId)
                == [NodeId(100)]
        )
        #expect(
            (renderer.registry.handle(for: editorID)?.view as? NSTextField)?.stringValue
                == "authoritative"
        )

        await controller.stop()
        await collector.stop()
        await clientTransport.close()
        await serverTransport.close()
    }

    @Test("Replacement resync resets the text-editing session sequence space")
    @MainActor
    func replacementResyncResetsTextSession() async throws {
        let (clientPipe, serverTransport) = await PipeTransport.createPair()
        let delayed = DelayedInputTransport(suspendingInputTo: clientPipe)
        let renderer = AppKitRenderer()
        renderer.textEditingSession.debounceNanoseconds = 0
        let applier = TransactionApplier()
        let outbox = EventOutbox()
        let controller = SessionController(
            transport: delayed,
            applier: applier,
            outbox: outbox,
            renderer: renderer
        )
        controller.attachRenderer(renderer)
        try await controller.start()
        try await handshakeAndMount(
            controller: controller,
            server: serverTransport,
            applier: applier,
            renderer: renderer,
            sessionId: "session-old"
        )

        renderer.textEditingSession.noteLocalValue(
            "old",
            nodeID: editorID,
            composing: false,
            flushImmediately: true
        )
        try await waitUntil(description: "old edit retained by its blocked writer") {
            await outbox.assignedTextEditDescriptors().count == 1
        }
        await delayed.waitUntilInputSendStarted()
        #expect(renderer.textEditingSession.nextEditSeqValue(for: editorID) == 2)

        var resync = SRUIServerResyncRequired()
        resync.sessionID = "session-new"
        resync.snapshotRevision = 1
        resync.reason = "replaced"
        resync.continuity = .replaced
        resync.lastProcessedEventSeq = 0
        var message = SRUIMessage()
        message.serverResyncRequired = resync
        await controller.handleIncomingMessage(message)

        #expect(renderer.textEditingSession.nextEditSeqValue(for: editorID) == 1)
        #expect(renderer.textEditingSession.localValue(for: editorID) == nil)
        #expect(renderer.textEditingSession.hasUnsentSuccessorDraft(for: editorID) == false)

        renderer.textEditingSession.noteLocalValue(
            "new incarnation",
            nodeID: editorID,
            composing: false,
            flushImmediately: true
        )
        try await waitUntil(description: "new incarnation edit accepted at reset sequence") {
            renderer.textEditingSession.hasUnsentSuccessorDraft(for: editorID)
                && renderer.textEditingSession.localValue(for: editorID) == "new incarnation"
                && renderer.textEditingSession.nextEditSeqValue(for: editorID) == 2
        }

        await controller.stop()
        await clientPipe.close()
        await serverTransport.close()
    }

    private func handshakeAndMount(
        controller: SessionController,
        server: any Transport,
        applier: TransactionApplier,
        renderer: AppKitRenderer,
        sessionId: String = "text-session"
    ) async throws {
        var welcome = SRUIServerWelcome()
        welcome.coreVersion = SRUICoreVersion
        welcome.sessionID = sessionId
        welcome.requiredProfiles = ["org.srui.standard-widgets/1"]
        var welcomeMessage = SRUIMessage()
        welcomeMessage.serverWelcome = welcome
        try await server.send(data: try SRUIFraming.encodeFramed(welcomeMessage))

        let mountTx = Transaction(
            baseRevision: .initial,
            newRevision: Revision(1),
            operations: [
                .createNode(id: surfaceID, nodeType: .surface),
                .createNode(
                    id: editorID,
                    nodeType: .textInput,
                    parentID: surfaceID,
                    properties: [
                        Property(property: .value, value: .string("")),
                    ]
                ),
            ]
        )
        var mountMsg = SRUIMessage()
        mountMsg.transaction = mountTx.toWire()
        try await server.send(data: try SRUIFraming.encodeFramed(mountMsg))

        try await AsyncTestSupport.eventually(description: "text input mounted") {
            applier.lastAppliedRevision == Revision(1)
                && renderer.registry.handle(for: editorID) != nil
        }
        #expect(controller.isEventDispatchEnabled)
    }

    @MainActor
    private func waitUntil(
        timeout: Double = 2.0,
        description: String,
        condition: @MainActor () async -> Bool
    ) async throws {
        try await AsyncTestSupport.eventuallyAsync(
            timeout: .seconds(timeout),
            description: description,
            condition: condition
        )
    }

    private func waitForTextEvent(
        _ collector: EventCollector,
        timeout: Double = 2.0,
        matching predicate: @Sendable (Event) -> Bool = { $0.eventType == .EVENT_TEXT_EDIT }
    ) async throws -> Event {
        try await AsyncTestSupport.eventuallyAsync(
            timeout: .seconds(timeout),
            description: "TEXT_EDIT"
        ) {
            await collector.events().contains {
                $0.eventType == .EVENT_TEXT_EDIT && predicate($0)
            }
        }
        if let event = await collector.events().first(where: {
            $0.eventType == .EVENT_TEXT_EDIT && predicate($0)
        }) {
            return event
        }
        throw AsyncTestTimeout(description: "Timed out waiting for TEXT_EDIT")
    }
}

private actor DelayedInputTransport: Transport {
    let inner: PipeTransport
    private let delayNanoseconds: UInt64?
    private let inputSuspensionStream: AsyncStream<Void>?
    private let inputSuspensionContinuation: AsyncStream<Void>.Continuation?
    private let stream: AsyncThrowingStream<Data, Error>
    private var inputSendStarted = false
    private var inputSendStartWaiters: [CheckedContinuation<Void, Never>] = []

    init(inner: PipeTransport, delayNanoseconds: UInt64) {
        self.inner = inner
        self.delayNanoseconds = delayNanoseconds
        self.inputSuspensionStream = nil
        self.inputSuspensionContinuation = nil
        self.stream = inner.receiveStream()
    }

    init(suspendingInputTo inner: PipeTransport) {
        let (suspensionStream, suspensionContinuation) = AsyncStream<Void>.makeStream()
        self.inner = inner
        self.delayNanoseconds = nil
        self.inputSuspensionStream = suspensionStream
        self.inputSuspensionContinuation = suspensionContinuation
        self.stream = inner.receiveStream()
    }

    func waitUntilInputSendStarted() async {
        guard inputSendStarted == false else { return }
        await withCheckedContinuation { continuation in
            if inputSendStarted {
                continuation.resume()
            } else {
                inputSendStartWaiters.append(continuation)
            }
        }
    }

    func send(data: Data, logicalClass: LogicalChannelClass) async throws {
        if logicalClass == .input {
            noteInputSendStarted()
            if let delayNanoseconds {
                try await Task.sleep(nanoseconds: delayNanoseconds)
            } else if let inputSuspensionStream {
                var iterator = inputSuspensionStream.makeAsyncIterator()
                _ = await iterator.next()
                try Task.checkCancellation()
            }
        }
        try await inner.send(data: data, logicalClass: logicalClass)
    }

    nonisolated func receiveStream() -> AsyncThrowingStream<Data, Error> {
        stream
    }

    func close() async {
        inputSuspensionContinuation?.finish()
        await inner.close()
    }

    private func noteInputSendStarted() {
        guard inputSendStarted == false else { return }
        inputSendStarted = true
        let waiters = inputSendStartWaiters
        inputSendStartWaiters.removeAll()
        for waiter in waiters {
            waiter.resume()
        }
    }
}

private actor DeliverThenThrowInputTransport: Transport {
    let inner: PipeTransport
    private let stream: AsyncThrowingStream<Data, Error>
    private var shouldThrow = true

    init(inner: PipeTransport) {
        self.inner = inner
        self.stream = inner.receiveStream()
    }

    func send(data: Data, logicalClass: LogicalChannelClass) async throws {
        try await inner.send(data: data, logicalClass: logicalClass)
        if logicalClass == .input, shouldThrow {
            shouldThrow = false
            throw TransportError.ioError("ambiguous send after delivery")
        }
    }

    nonisolated func receiveStream() -> AsyncThrowingStream<Data, Error> {
        stream
    }

    func close() async {
        await inner.close()
    }
}
