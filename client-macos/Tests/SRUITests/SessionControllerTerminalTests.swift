//
// SessionControllerTerminalTests.swift
// SRUITests
//
// Terminal island resync must not disturb semantic text-edit state (§18.3, §21, §21.2).
//

import AppKit
import Foundation
import Protocol
import RendererAppKit
import SemanticModel
import Session
import Testing
import Terminal
import Text
import TransportSSH

@Suite("SessionController terminal independence")
struct SessionControllerTerminalTests {
    private let surfaceID = NodeId(1)
    private let editorID = NodeId(12)

    @Test("TerminalResyncRequired leaves pending text, revision, and phase intact")
    @MainActor
    func terminalResyncDoesNotTouchSemanticSession() async throws {
        let (clientTransport, serverTransport) = await PipeTransport.createPair()
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
        renderer.textEditingSession.debounceNanoseconds = 1_000_000_000
        let controller = SessionController(
            transport: clientTransport,
            applier: applier,
            renderer: renderer,
            clientCapabilities: [Profile.standardWidgetsV1, Profile.terminalV1]
        )
        controller.attachRenderer(renderer)
        try await controller.start()

        var welcome = SRUIServerWelcome()
        welcome.coreVersion = SRUICoreVersion
        welcome.sessionID = "term-island"
        welcome.requiredProfiles = ["org.srui.standard-widgets/1"]
        welcome.optionalProfiles = ["org.srui.terminal/1"]
        welcome.initialRevision = 0
        var mapping = Srui_Protocol_ExtensionNamespaceMapping()
        mapping.extensionUri = terminalProfileURI
        mapping.namespaceID = 3
        welcome.extensionNamespaces = [mapping]
        var welcomeMsg = SRUIMessage()
        welcomeMsg.serverWelcome = welcome
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(welcomeMsg))
        try await AsyncTestSupport.eventually(description: "handshake") {
            controller.isHandshakeComplete
        }

        let initialTx = Transaction(
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
            ]
        )
        var txMsg = SRUIMessage()
        txMsg.transaction = initialTx.toWire()
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(txMsg))
        try await AsyncTestSupport.eventually(description: "text node mounted") {
            applier.lastAppliedRevision == Revision(1) && renderer.registry.handle(for: editorID) != nil
        }

        renderer.textEditingSession.noteLocalValue(
            "draft-survives",
            nodeID: editorID,
            composing: false,
            flushImmediately: false
        )
        #expect(renderer.textEditingSession.localValue(for: editorID) == "draft-survives")
        let revisionBefore = applier.lastAppliedRevision
        let dispatchBefore = controller.isEventDispatchEnabled
        #expect(controller.isHandshakeComplete)
        #expect(!controller.isDiverged)

        var resync = SRUITerminalResyncRequired()
        resync.streamID = 30
        resync.requestedOffset = 0
        resync.retainedFromOffset = 8
        resync.resumeAtOffset = 16
        resync.reason = .retentionLoss
        var resyncMsg = SRUIMessage()
        resyncMsg.terminalResyncRequired = resync
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(resyncMsg))
        try await AsyncTestSupport.eventuallyAsync(description: "terminal island resync applied") {
            await renderer.terminalSession.snapshot(for: NodeId(30))?.needsRedraw == true
        }

        #expect(controller.isHandshakeComplete)
        #expect(!controller.isDiverged)
        #expect(controller.isEventDispatchEnabled == dispatchBefore)
        #expect(applier.lastAppliedRevision == revisionBefore)
        #expect(renderer.textEditingSession.localValue(for: editorID) == "draft-survives")
        #expect(await renderer.terminalSession.streamOffsets()[30] == 16)

        await controller.stop()
        await serverTransport.close()
    }
}
