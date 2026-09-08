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

    @Test("Replacement resync requires fresh extension negotiation before Terminal remount")
    @MainActor
    func replacementResyncRejectsTerminalSnapshotBeforeRemount() async throws {
        let (clientTransport, serverTransport) = await PipeTransport.createPair()
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
        let failures = SessionFailureRecorder()
        let controller = SessionController(
            transport: clientTransport,
            applier: applier,
            renderer: renderer
        )
        controller.onFailure = { failure in
            Task { await failures.record(failure) }
        }
        controller.attachRenderer(renderer)
        try await controller.start()

        var mapping = Srui_Protocol_ExtensionNamespaceMapping()
        mapping.extensionUri = terminalProfileURI
        mapping.namespaceID = 3
        var welcome = SRUIServerWelcome()
        welcome.coreVersion = SRUICoreVersion
        welcome.sessionID = "terminal-before-replacement"
        welcome.requiredProfiles = ["org.srui.standard-widgets/1", terminalProfileURI]
        welcome.extensionNamespaces = [mapping]
        var welcomeMessage = SRUIMessage()
        welcomeMessage.serverWelcome = welcome
        await controller.handleIncomingMessage(welcomeMessage)

        let terminalType = TypeRef(namespaceID: 3, localID: 1)
        let initial = Transaction(
            baseRevision: .initial,
            newRevision: Revision(1),
            operations: [
                .createNode(id: NodeId(1), nodeType: .surface),
                .createNode(id: NodeId(30), nodeType: terminalType, parentID: NodeId(1)),
            ]
        )
        var initialMessage = SRUIMessage()
        initialMessage.transaction = initial.toWire()
        await controller.handleIncomingMessage(initialMessage)

        let mountedTerminal = try #require(renderer.registry.view(for: NodeId(30)))
        #expect(mountedTerminal is TerminalView)

        var resync = SRUIServerResyncRequired()
        resync.sessionID = "terminal-after-replacement"
        resync.snapshotRevision = 2
        resync.reason = "replacement"
        resync.continuity = .replaced
        var resyncMessage = SRUIMessage()
        resyncMessage.serverResyncRequired = resync
        await controller.handleIncomingMessage(resyncMessage)

        #expect(controller.negotiatedCapabilities == [Profile.standardWidgetsV1])
        #expect(renderer.controlFactory.extensionKind(for: terminalType) == nil)

        let replacement = Transaction(
            baseRevision: .initial,
            newRevision: Revision(2),
            operations: [
                .createNode(id: NodeId(1), nodeType: .surface),
                .createNode(id: NodeId(30), nodeType: terminalType, parentID: NodeId(1)),
            ]
        )
        var replacementMessage = SRUIMessage()
        replacementMessage.transaction = replacement.toWire()
        await controller.handleIncomingMessage(replacementMessage)

        let failure = try #require(await failures.wait())
        guard case .protocolViolation(let message) = failure else {
            Issue.record("Expected protocolViolation, got \(failure)")
            await controller.stop()
            await serverTransport.close()
            return
        }
        #expect(message.contains("capability renegotiation"))
        #expect(controller.sessionId == nil)
        #expect(renderer.registry.view(for: NodeId(30)) === mountedTerminal)

        await controller.stop()
        await serverTransport.close()
    }

    @Test("Replacement rejects loss of a client-required extension profile")
    @MainActor
    func replacementRejectsMissingClientRequiredProfile() async throws {
        let (clientTransport, serverTransport) = await PipeTransport.createPair()
        let renderer = AppKitRenderer()
        let failures = SessionFailureRecorder()
        let controller = SessionController(
            transport: clientTransport,
            renderer: renderer,
            requiredServerProfiles: [Profile.terminalV1]
        )
        controller.onFailure = { failure in
            Task { await failures.record(failure) }
        }
        controller.attachRenderer(renderer)
        try await controller.start()

        var mapping = Srui_Protocol_ExtensionNamespaceMapping()
        mapping.extensionUri = terminalProfileURI
        mapping.namespaceID = 3
        var welcome = SRUIServerWelcome()
        welcome.coreVersion = SRUICoreVersion
        welcome.sessionID = "required-terminal"
        welcome.requiredProfiles = ["org.srui.standard-widgets/1", terminalProfileURI]
        welcome.extensionNamespaces = [mapping]
        var welcomeMessage = SRUIMessage()
        welcomeMessage.serverWelcome = welcome
        await controller.handleIncomingMessage(welcomeMessage)
        #expect(controller.isHandshakeComplete)

        var resync = SRUIServerResyncRequired()
        resync.sessionID = "replacement-without-negotiation"
        resync.continuity = .replaced
        var resyncMessage = SRUIMessage()
        resyncMessage.serverResyncRequired = resync
        await controller.handleIncomingMessage(resyncMessage)

        let failure = try #require(await failures.wait())
        guard case .protocolViolation(let message) = failure else {
            Issue.record("Expected protocolViolation, got \(failure)")
            await controller.stop()
            await serverTransport.close()
            return
        }
        #expect(message.contains("client required profiles"))
        #expect(message.contains("CLIENT_HELLO/SERVER_WELCOME"))
        #expect(controller.sessionId == nil)
        #expect(controller.isDiverged)

        await controller.stop()
        await serverTransport.close()
    }

    @Test("A recreated Terminal renderer forces a fresh hello without namespace state")
    @MainActor
    func recreatedRendererWithoutNamespaceStateSendsFreshHello() async throws {
        let (firstClientTransport, firstServerTransport) = await PipeTransport.createPair()
        let applier = TransactionApplier()
        let outbox = EventOutbox()
        let firstRenderer = AppKitRenderer()
        let firstController = SessionController(
            transport: firstClientTransport,
            applier: applier,
            outbox: outbox,
            renderer: firstRenderer
        )
        firstController.attachRenderer(firstRenderer)
        try await firstController.start()

        var mapping = Srui_Protocol_ExtensionNamespaceMapping()
        mapping.extensionUri = terminalProfileURI
        mapping.namespaceID = 3
        var welcome = SRUIServerWelcome()
        welcome.coreVersion = SRUICoreVersion
        welcome.sessionID = "recreated-terminal-controller"
        welcome.requiredProfiles = ["org.srui.standard-widgets/1", terminalProfileURI]
        welcome.extensionNamespaces = [mapping]
        var welcomeMessage = SRUIMessage()
        welcomeMessage.serverWelcome = welcome
        await firstController.handleIncomingMessage(welcomeMessage)

        let transaction = Transaction(
            baseRevision: .initial,
            newRevision: Revision(1),
            operations: [
                .createNode(id: NodeId(1), nodeType: .surface),
                .createNode(
                    id: NodeId(30),
                    nodeType: TypeRef(namespaceID: 3, localID: 1),
                    parentID: NodeId(1)
                ),
            ]
        )
        var transactionMessage = SRUIMessage()
        transactionMessage.transaction = transaction.toWire()
        await firstController.handleIncomingMessage(transactionMessage)
        #expect(applier.lastAppliedRevision == Revision(1))
        await firstController.stop()
        await firstServerTransport.close()

        let (secondClientTransport, secondServerTransport) = await PipeTransport.createPair()
        let secondRenderer = AppKitRenderer()
        let secondController = SessionController(
            transport: secondClientTransport,
            applier: applier,
            outbox: outbox,
            renderer: secondRenderer,
            sessionId: "recreated-terminal-controller"
        )
        secondController.attachRenderer(secondRenderer)
        try await secondController.start()

        let serverStream = secondServerTransport.receiveStream()
        var streamDecoder = SRUIMessageStreamDecoder()
        var sentFreshHello = false
        for try await chunk in serverStream {
            for message in try streamDecoder.appendAndExtract(incoming: chunk) {
                switch message.msg {
                case .clientHello:
                    sentFreshHello = true
                case .clientResume:
                    Issue.record("Expected CLIENT_HELLO when Terminal namespace state is unavailable")
                default:
                    break
                }
            }
            if sentFreshHello { break }
        }

        #expect(sentFreshHello)
        #expect(secondController.sessionId == nil)
        // A revision-zero WELCOME carries no snapshot, so nothing after the hello would clear the
        // replica this controller inherited from the session it just abandoned: stale windows
        // would stay mounted and could emit events for nodes the new server never created (§18).
        #expect(applier.lastAppliedRevision == .initial)
        #expect(applier.currentSnapshot.store.rootIDs.isEmpty)
        #expect(secondRenderer.registry.count == 0)

        await secondController.stop()
        await secondServerTransport.close()
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
