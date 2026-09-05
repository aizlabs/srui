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
import Session
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
        #expect(!editor.hasMarkedText())
        adapter.notifyTextDidChangeForTests()
        _ = window.makeFirstResponder(nil)
        adapter.notifyEndEditingForTests()
        let committed = field.stringValue
        #expect(!committed.isEmpty)
        #expect(await collector.eventCount() == 0)

        try await Task.sleep(nanoseconds: 80_000_000)
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
            await outbox.unsentTextDraftCount == 1
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

        try await Task.sleep(nanoseconds: 50_000_000)
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
            await outbox.unsentTextDraftCount == 1
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
            await outbox.unsentTextDraftCount == 1
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

        try await Task.sleep(nanoseconds: 50_000_000)
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
            await outbox.unsentTextDraftCount == 1
        }
        #expect(renderer.textEditingSession.nextEditSeqValue(for: editorID) == 2)

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
        try await Task.sleep(nanoseconds: 100_000_000)
        #expect(await outbox.unsentTextDraftCount == 0)
        #expect(await collector.events().filter { $0.eventType == .EVENT_TEXT_EDIT }.isEmpty)

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

    @Test("Replacement resync resets the text-editing session sequence space")
    @MainActor
    func replacementResyncResetsTextSession() async throws {
        let (clientPipe, serverTransport) = await PipeTransport.createPair()
        let renderer = AppKitRenderer()
        renderer.textEditingSession.debounceNanoseconds = 0
        let applier = TransactionApplier()
        let controller = SessionController(
            transport: clientPipe,
            applier: applier,
            renderer: renderer,
            sessionId: "session-old"
        )
        controller.attachRenderer(renderer)
        try await controller.start()

        renderer.textEditingSession.noteLocalValue(
            "old",
            nodeID: editorID,
            composing: false,
            flushImmediately: true
        )
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
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if await condition() { return }
            try await Task.sleep(nanoseconds: 10_000_000)
        }
        throw AsyncTestTimeout(description: "timed out waiting for \(description)")
    }

    private func waitForTextEvent(
        _ collector: EventCollector,
        timeout: Double = 2.0,
        matching predicate: @Sendable (Event) -> Bool = { $0.eventType == .EVENT_TEXT_EDIT }
    ) async throws -> Event {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if let event = await collector.events().first(where: { $0.eventType == .EVENT_TEXT_EDIT && predicate($0) }) {
                return event
            }
            try await Task.sleep(nanoseconds: 10_000_000)
        }
        throw AsyncTestTimeout(description: "timed out waiting for TEXT_EDIT")
    }
}

private actor DelayedInputTransport: Transport {
    let inner: PipeTransport
    let delayNanoseconds: UInt64
    private let stream: AsyncThrowingStream<Data, Error>

    init(inner: PipeTransport, delayNanoseconds: UInt64) {
        self.inner = inner
        self.delayNanoseconds = delayNanoseconds
        self.stream = inner.receiveStream()
    }

    func send(data: Data, logicalClass: LogicalChannelClass) async throws {
        if logicalClass == .input {
            try await Task.sleep(nanoseconds: delayNanoseconds)
        }
        try await inner.send(data: data, logicalClass: logicalClass)
    }

    nonisolated func receiveStream() -> AsyncThrowingStream<Data, Error> {
        stream
    }

    func close() async {
        await inner.close()
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

private actor EventCollector {
    private var decoded: [Event] = []
    private var task: Task<Void, Never>?

    func start(draining transport: any Transport) {
        guard task == nil else { return }
        let stream = transport.receiveStream()
        task = Task { [weak self] in
            var decoder = SRUIMessageStreamDecoder()
            let protocolDecoder = ProtocolDecoder()
            do {
                for try await chunk in stream {
                    for message in try decoder.appendAndExtract(incoming: chunk) {
                        if case .event(let wire) = message.msg,
                           let event = try? protocolDecoder.validateAndConvertEvent(wire: wire) {
                            await self?.append(event)
                        }
                    }
                }
            } catch {}
        }
    }

    private func append(_ event: Event) {
        decoded.append(event)
    }

    func stop() {
        task?.cancel()
        task = nil
    }

    func events() -> [Event] {
        decoded
    }

    func eventCount() -> Int {
        decoded.count
    }
}
