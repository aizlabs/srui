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

        field.stringValue = "glyphs"
        field.currentEditor()?.selectedRange = NSRange(location: 2, length: 2)
        adapter.compositionOverride = true
        adapter.notifyTextDidChangeForTests()
        #expect(field.stringValue == "glyphs")
        #expect(adapter.isComposing)
        #expect(await collector.eventCount() == 0)

        adapter.compositionOverride = false
        adapter.notifyEndEditingForTests()
        #expect(field.stringValue == "glyphs")
        #expect(await collector.eventCount() == 0)

        try await Task.sleep(nanoseconds: 80_000_000)
        #expect(await collector.eventCount() == 0)

        try await Task.sleep(nanoseconds: 500_000_000)
        let events = await collector.events()
        #expect(events.count == 1)
        #expect(events[0].eventType == .EVENT_TEXT_EDIT)
        #expect(events[0].textArg == "glyphs")
        #expect(events[0].editSeq?.rawValue == 1)

        await controller.stop()
        await collector.stop()
        await delayed.close()
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

    private func waitForTextEvent(_ collector: EventCollector, timeout: Double = 2.0) async throws -> Event {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if let event = await collector.events().first(where: { $0.eventType == .EVENT_TEXT_EDIT }) {
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
