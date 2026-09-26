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
@testable import Session
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

    @Test("Replacement resync installs fresh extension negotiation before Terminal remount")
    @MainActor
    func replacementResyncInstallsTerminalMappingBeforeRemount() async throws {
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

        var replacementMapping = Srui_Protocol_ExtensionNamespaceMapping()
        replacementMapping.extensionUri = terminalProfileURI
        replacementMapping.namespaceID = 4
        var resync = SRUIServerResyncRequired()
        resync.sessionID = "terminal-after-replacement"
        resync.snapshotRevision = 2
        resync.reason = "replacement"
        resync.continuity = .replaced
        resync.requiredProfiles = ["org.srui.standard-widgets/1", terminalProfileURI]
        resync.extensionNamespaces = [replacementMapping]
        var resyncMessage = SRUIMessage()
        resyncMessage.serverResyncRequired = resync
        await controller.handleIncomingMessage(resyncMessage)

        let replacementTerminalType = TypeRef(namespaceID: 4, localID: 1)
        #expect(controller.negotiatedCapabilities?.contains(.terminalV1) == true)
        #expect(renderer.controlFactory.extensionKind(for: terminalType) == nil)
        #expect(renderer.controlFactory.extensionKind(for: replacementTerminalType) == .terminal)

        let replacement = Transaction(
            baseRevision: .initial,
            newRevision: Revision(2),
            operations: [
                .createNode(id: NodeId(1), nodeType: .surface),
                .createNode(
                    id: NodeId(30),
                    nodeType: replacementTerminalType,
                    parentID: NodeId(1)
                ),
            ]
        )
        var replacementMessage = SRUIMessage()
        replacementMessage.transaction = replacement.toWire()
        await controller.handleIncomingMessage(replacementMessage)

        #expect(controller.sessionId == "terminal-after-replacement")
        #expect(controller.isHandshakeComplete)
        #expect(applier.lastAppliedRevision == Revision(2))
        #expect(renderer.registry.view(for: NodeId(30)) is TerminalView)
        #expect(renderer.registry.view(for: NodeId(30)) !== mountedTerminal)

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
        #expect(message.contains("live replacement resync omitted negotiation metadata"))
        #expect(controller.sessionId == "required-terminal")
        #expect(controller.isDiverged)

        await controller.stop()
        await serverTransport.close()
    }

    @Test("Cold resume rejects optional Terminal without its namespace mapping")
    @MainActor
    func coldResumeRejectsOptionalTerminalWithoutMapping() async throws {
        let (clientTransport, serverTransport) = await PipeTransport.createPair()
        let controller = SessionController(
            transport: clientTransport,
            sessionId: "optional-terminal",
            clientCapabilities: [.standardWidgetsV1, .terminalV1]
        )
        try await controller.start()

        var resume = SRUIServerResumeOk()
        resume.sessionID = "optional-terminal"
        resume.requiredProfiles = ["org.srui.standard-widgets/1"]
        resume.optionalProfiles = [terminalProfileURI]
        var message = SRUIMessage()
        message.serverResumeOk = resume
        await controller.handleIncomingMessage(message)

        #expect(controller.isDiverged)
        #expect(!controller.isHandshakeComplete)

        await controller.stop()
        await serverTransport.close()
    }

    @Test("A recreated process cold-resumes Terminal after authoritative negotiation")
    @MainActor
    func recreatedProcessColdResumesTerminalWithServerMapping() async throws {
        let (clientTransport, serverTransport) = await PipeTransport.createPair()
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
        let controller = SessionController(
            transport: clientTransport,
            applier: applier,
            outbox: EventOutbox(),
            renderer: renderer,
            sessionId: "recreated-terminal-controller",
            requiredServerProfiles: [.terminalV1],
            continuityContext: SessionContinuityContext()
        )
        controller.attachRenderer(renderer)
        try await controller.start()

        let serverStream = serverTransport.receiveStream()
        var streamDecoder = SRUIMessageStreamDecoder()
        var resumeOffer: SRUIClientResume?
        for try await chunk in serverStream {
            for message in try streamDecoder.appendAndExtract(incoming: chunk) {
                if case .clientResume(let resume) = message.msg {
                    resumeOffer = resume
                }
            }
            if resumeOffer != nil { break }
        }

        let offer = try #require(resumeOffer)
        #expect(offer.sessionID == "recreated-terminal-controller")
        #expect(offer.lastAppliedRevision == 0)
        #expect(offer.coreVersion == SRUICoreVersion)
        #expect(offer.profiles.contains("org.srui.standard-widgets/1"))
        #expect(offer.profiles.contains(terminalProfileURI))

        var mapping = Srui_Protocol_ExtensionNamespaceMapping()
        mapping.extensionUri = terminalProfileURI
        mapping.namespaceID = 3
        var resume = SRUIServerResumeOk()
        resume.sessionID = "recreated-terminal-controller"
        resume.requiredProfiles = ["org.srui.standard-widgets/1", terminalProfileURI]
        resume.extensionNamespaces = [mapping]
        var resumeMessage = SRUIMessage()
        resumeMessage.serverResumeOk = resume
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(resumeMessage))

        let terminalType = TypeRef(namespaceID: 3, localID: 1)
        let replay = Transaction(
            baseRevision: .initial,
            newRevision: Revision(1),
            operations: [
                .createNode(id: NodeId(1), nodeType: .surface),
                .createNode(id: NodeId(30), nodeType: terminalType, parentID: NodeId(1)),
            ]
        )
        var replayMessage = SRUIMessage()
        replayMessage.transaction = replay.toWire()
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(replayMessage))

        try await AsyncTestSupport.eventually(description: "cold Terminal replay mounted") {
            applier.lastAppliedRevision == Revision(1)
                && renderer.registry.view(for: NodeId(30)) is TerminalView
        }
        #expect(controller.isHandshakeComplete)
        #expect(renderer.controlFactory.extensionKind(for: terminalType) == .terminal)

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

    /// A fresh WELCOME abandons both the semantic replica and the independent Terminal island.
    /// A positive initial revision waits for a snapshot, but stale state must already be gone.
    @Test(
        "Fresh WELCOME clears abandoned semantic and Terminal state",
        arguments: [UInt64(0), UInt64(2)]
    )
    @MainActor
    func freshWelcomeDiscardsAbandonedState(initialRevision: UInt64) async throws {
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
        let outbox = EventOutbox()
        let continuityContext = SessionContinuityContext()

        let (seedClient, seedServer) = await PipeTransport.createPair()
        let seedController = SessionController(
            transport: seedClient,
            applier: applier,
            outbox: outbox,
            renderer: renderer,
            continuityContext: continuityContext
        )
        seedController.attachRenderer(renderer)
        try await seedController.start()

        var mapping = Srui_Protocol_ExtensionNamespaceMapping()
        mapping.extensionUri = terminalProfileURI
        mapping.namespaceID = 3
        var seedWelcome = SRUIServerWelcome()
        seedWelcome.coreVersion = SRUICoreVersion
        seedWelcome.sessionID = "seeded-session"
        seedWelcome.requiredProfiles = [
            "org.srui.standard-widgets/1",
            terminalProfileURI,
        ]
        seedWelcome.extensionNamespaces = [mapping]
        var seedWelcomeMessage = SRUIMessage()
        seedWelcomeMessage.serverWelcome = seedWelcome
        await seedController.handleIncomingMessage(seedWelcomeMessage)

        let seeded = Transaction(
            baseRevision: .initial,
            newRevision: Revision(1),
            operations: [
                .createNode(id: NodeId(1), nodeType: .surface),
                .createNode(id: NodeId(2), nodeType: .button, parentID: NodeId(1)),
            ]
        )
        var seededMessage = SRUIMessage()
        seededMessage.transaction = seeded.toWire()
        await seedController.handleIncomingMessage(seededMessage)
        var terminalData = SRUITerminalData()
        terminalData.streamID = 30
        terminalData.byteOffset = 0
        terminalData.data = Data([0x78])
        var terminalMessage = SRUIMessage()
        terminalMessage.terminalData = terminalData
        await seedController.handleIncomingMessage(terminalMessage)

        #expect(applier.lastAppliedRevision == Revision(1))
        #expect(renderer.registry.handle(for: NodeId(2)) != nil)
        #expect(await renderer.terminalSession.snapshot(for: NodeId(30))?.nextOffset == 1)
        await seedController.stop()
        await seedServer.close()

        let (freshClient, freshServer) = await PipeTransport.createPair()
        let freshController = SessionController(
            transport: freshClient,
            applier: applier,
            outbox: outbox,
            renderer: renderer,
            clientCapabilities: [.standardWidgetsV1],
            continuityContext: continuityContext
        )
        freshController.attachRenderer(renderer)
        try await freshController.start()
        #expect(freshController.sessionId == nil)
        await freshController.handleIncomingMessage(
            welcomeMessage(
                sessionID: "replacement-session",
                initialRevision: initialRevision
            )
        )

        #expect(applier.lastAppliedRevision == .initial)
        #expect(applier.currentSnapshot.store.rootIDs.isEmpty)
        #expect(renderer.registry.count == 0)
        #expect(await renderer.terminalSession.snapshot(for: NodeId(30)) == nil)

        await freshController.stop()
        await freshServer.close()
    }

    /// A catch-up snapshot advances the session incarnation before it mounts. The native callbacks
    /// are reinstalled at the mount, so the tree the snapshot publishes can queue Terminal input
    /// immediately — and `TerminalCommandPump.drain()` *discards* an item its sender refuses, so a
    /// pump still bound to the previous incarnation loses those keystrokes instead of retrying them
    /// (§21, §22.2).
    @Test("Terminal input typed while a catch-up snapshot mounts reaches the wire")
    @MainActor
    func terminalInputDuringSnapshotMountIsSent() async throws {
        let (clientPipe, serverTransport) = await PipeTransport.createPair()
        let recording = TerminalRecordingTransport(inner: clientPipe)
        let applier = TransactionApplier()
        let renderer = AppKitRenderer()
        let controller = SessionController(
            transport: recording,
            applier: applier,
            renderer: renderer,
            clientCapabilities: [Profile.standardWidgetsV1, Profile.terminalV1]
        )
        controller.attachRenderer(renderer)

        let terminalID = NodeId(30)
        let typed = ManagedAtomic<Bool>(false)
        // Fires inside the snapshot's own renderer update: the window between the native mount and
        // `completeSnapshotCatchUp()`'s reinstall, i.e. exactly when a user can type into the
        // freshly shown window.
        controller.rendererDidRenderInterceptorForTesting = { [renderer] in
            guard typed.load() == false else { return }
            await MainActor.run {
                guard renderer.registry.view(for: terminalID) is TerminalView else { return }
                typed.store(true)
                renderer.onTerminalInput?(terminalID, Data("ls\n".utf8))
            }
        }

        try await controller.start()

        var mapping = Srui_Protocol_ExtensionNamespaceMapping()
        mapping.extensionUri = terminalProfileURI
        mapping.namespaceID = 3
        var welcome = SRUIServerWelcome()
        welcome.coreVersion = SRUICoreVersion
        welcome.sessionID = "terminal-catch-up"
        welcome.requiredProfiles = ["org.srui.standard-widgets/1", terminalProfileURI]
        welcome.extensionNamespaces = [mapping]
        // A positive initial revision makes the next transaction the catch-up snapshot (§18).
        welcome.initialRevision = 1
        var welcomeMessage = SRUIMessage()
        welcomeMessage.serverWelcome = welcome
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(welcomeMessage))

        let terminalType = TypeRef(namespaceID: 3, localID: 1)
        let snapshot = Transaction(
            baseRevision: .initial,
            newRevision: Revision(1),
            operations: [
                .createNode(id: surfaceID, nodeType: .surface),
                .createNode(id: terminalID, nodeType: terminalType, parentID: surfaceID),
            ]
        )
        var snapshotMessage = SRUIMessage()
        snapshotMessage.transaction = snapshot.toWire()
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(snapshotMessage))

        try await AsyncTestSupport.eventuallyAsync(
            timeout: .seconds(5),
            description: "input typed while the snapshot mounted"
        ) {
            typed.load()
        }
        try await AsyncTestSupport.eventuallyAsync(
            timeout: .seconds(5),
            description: "TERMINAL_INPUT reached the transport"
        ) {
            await recording.terminalFrameCount >= 1
        }

        let framed = try #require(await recording.terminalFrames.first)
        let decoded = try SRUIFraming.decodeFramed(SRUIMessage.self, from: framed)
        guard case .terminalInput(let input)? = decoded.msg else {
            Issue.record("expected TerminalInput, got \(String(describing: decoded.msg))")
            return
        }
        #expect(input.streamID == terminalID.value)
        #expect(input.data == Data("ls\n".utf8))

        await controller.stop()
        await serverTransport.close()
    }

    private func welcomeMessage(
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
}

/// Records `.terminalHigh` frames without gating them.
private actor TerminalRecordingTransport: Transport {
    private let inner: PipeTransport
    private let stream: AsyncThrowingStream<Data, Error>
    private(set) var terminalFrames: [Data] = []

    init(inner: PipeTransport) {
        self.inner = inner
        self.stream = inner.receiveStream()
    }

    var terminalFrameCount: Int { terminalFrames.count }

    func send(data: Data, logicalClass: LogicalChannelClass) async throws {
        if logicalClass == .terminalHigh {
            terminalFrames.append(data)
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
