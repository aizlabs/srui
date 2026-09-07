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

    /// A server that never negotiated `org.srui.terminal/1` must not be able to push terminal
    /// frames, nor to allocate per-stream client state by naming stream IDs (§11.1, §21).
    @Test("TerminalData without a negotiated terminal profile fails the session")
    @MainActor
    func unnegotiatedTerminalDataIsAProtocolViolation() async throws {
        let (clientTransport, serverTransport) = await PipeTransport.createPair()
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
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
        welcome.sessionID = "no-terminal"
        welcome.requiredProfiles = ["org.srui.standard-widgets/1"]
        welcome.optionalProfiles = []
        welcome.initialRevision = 0
        var welcomeMsg = SRUIMessage()
        welcomeMsg.serverWelcome = welcome
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(welcomeMsg))
        try await AsyncTestSupport.eventually(description: "handshake") {
            controller.isHandshakeComplete
        }

        var data = SRUITerminalData()
        data.streamID = 30
        data.byteOffset = 0
        data.data = Data("hello".utf8)
        var dataMsg = SRUIMessage()
        dataMsg.terminalData = data
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(dataMsg))

        try await AsyncTestSupport.eventually(description: "unnegotiated terminal frame rejected") {
            controller.isDiverged
        }
        #expect(await renderer.terminalSession.snapshot(for: NodeId(30)) == nil)

        await controller.stop()
        await serverTransport.close()
    }

    @Test("Only the negotiated terminal namespace is registered when local IDs collide")
    @MainActor
    func exactTerminalNamespaceWinsLocalIDCollision() async throws {
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

        var terminalMapping = Srui_Protocol_ExtensionNamespaceMapping()
        terminalMapping.extensionUri = terminalProfileURI
        terminalMapping.namespaceID = 3
        var diffMapping = Srui_Protocol_ExtensionNamespaceMapping()
        diffMapping.extensionUri = "org.example.diff/1"
        diffMapping.namespaceID = 4
        var welcome = SRUIServerWelcome()
        welcome.coreVersion = SRUICoreVersion
        welcome.sessionID = "terminal-namespace-collision"
        welcome.requiredProfiles = ["org.srui.standard-widgets/1", terminalProfileURI]
        welcome.optionalProfiles = ["org.example.diff/1"]
        welcome.extensionNamespaces = [terminalMapping, diffMapping]
        var welcomeMessage = SRUIMessage()
        welcomeMessage.serverWelcome = welcome
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(welcomeMessage))
        try await AsyncTestSupport.eventually(description: "collision handshake") {
            controller.isHandshakeComplete
        }

        let terminalType = TypeRef(namespaceID: 3, localID: 1)
        let diffType = TypeRef(namespaceID: 4, localID: 1)
        let transaction = Transaction(
            baseRevision: .initial,
            newRevision: Revision(1),
            operations: [
                .createNode(id: 1, nodeType: .surface),
                .createNode(id: 2, nodeType: diffType, parentID: 1),
                .createNode(id: 3, nodeType: .column, parentID: 2),
                .createNode(id: 4, nodeType: .text, parentID: 3),
                .createNode(id: 30, nodeType: terminalType, parentID: 1),
            ]
        )
        var transactionMessage = SRUIMessage()
        transactionMessage.transaction = transaction.toWire()
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(transactionMessage))
        try await AsyncTestSupport.eventually(description: "collision snapshot rendered") {
            applier.lastAppliedRevision == Revision(1)
                && renderer.registry.handle(for: NodeId(30)) != nil
        }

        #expect(renderer.registry.view(for: NodeId(2)) is NSStackView)
        #expect(renderer.registry.view(for: NodeId(30)) is TerminalView)
        #expect(renderer.controlFactory.extensionKind(for: diffType) == nil)
        #expect(renderer.controlFactory.extensionKind(for: terminalType) == .terminal)

        await controller.stop()
        await serverTransport.close()
    }
}
