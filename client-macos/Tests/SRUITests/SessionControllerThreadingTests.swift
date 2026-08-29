//
// SessionControllerThreadingTests.swift
// SRUITests
//
// Concurrency, threading split, and transaction consistency tests (§12.1, §22, §22.2).
//

import Testing
import Foundation
import AppKit
import SemanticModel
import Protocol
import Session
import TransportSSH
import RendererAppKit

@Suite("SessionController Threading & Consistency Tests")
struct SessionControllerThreadingTests {

    @Test("Threading pipeline applies sequential transactions and mounts AppKit tree")
    @MainActor
    func threadingPipelineAppliesTransactions() async throws {
        let (clientTransport, serverTransport) = await PipeTransport.createPair()
        let applier = TransactionApplier()
        let outbox = EventOutbox()
        let renderer = AppKitRenderer()

        let controller = SessionController(
            transport: clientTransport,
            applier: applier,
            outbox: outbox,
            renderer: renderer
        )
        controller.attachRenderer(renderer)

        try await controller.start()

        var welcome = SRUIServerWelcome()
        welcome.sessionID = "test-session"
        welcome.requiredProfiles = ["org.srui.standard-widgets/1"]
        var welcomeMsg = SRUIMessage()
        welcomeMsg.serverWelcome = welcome
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(welcomeMsg))

        // 1. Server sends initial transaction (Revision 0 -> 1)
        let surfaceID = NodeId(1)
        let textID = NodeId(2)
        let progressID = NodeId(3)
        let buttonID = NodeId(4)

        let initialTx = Transaction(
            baseRevision: .initial,
            newRevision: Revision(1),
            operations: [
                .createNode(
                    id: surfaceID,
                    nodeType: .surface,
                    properties: [Property(property: .label, value: .string("Test Window"))]
                ),
                .createNode(
                    id: textID,
                    nodeType: .text,
                    parentID: surfaceID,
                    properties: [Property(property: .text, value: .string("Count: 0"))]
                ),
                .createNode(
                    id: progressID,
                    nodeType: .progress,
                    parentID: surfaceID,
                    properties: [Property(property: .value, value: .float64(0.0))]
                ),
                .createNode(
                    id: buttonID,
                    nodeType: .button,
                    parentID: surfaceID,
                    properties: [Property(property: .label, value: .string("Increment"))]
                ),
            ]
        )

        var msg1 = SRUIMessage()
        msg1.transaction = initialTx.toWire()
        let bytes1 = try SRUIFraming.encodeFramed(msg1)
        try await serverTransport.send(data: bytes1)

        // Allow background loop to decode and MainActor to apply
        try await Task.sleep(nanoseconds: 50_000_000)

        // Verify initial mount
        #expect(applier.lastAppliedRevision == Revision(1))
        #expect(renderer.registry.count == 4)
        #expect(renderer.registry.handle(for: textID) != nil)
        #expect(renderer.registry.handle(for: buttonID) != nil)

        let textHandle = try #require(renderer.registry.handle(for: textID))
        let buttonHandle = try #require(renderer.registry.handle(for: buttonID))
        let textIdentity = ObjectIdentifier(textHandle.view)
        let buttonIdentity = ObjectIdentifier(buttonHandle.view)

        // 2. Server sends scalar update transaction (Revision 1 -> 2)
        let updateTx = Transaction(
            baseRevision: Revision(1),
            newRevision: Revision(2),
            operations: [
                .setProperty(id: textID, property: .text, value: .string("Count: 1")),
                .setProperty(id: progressID, property: .value, value: .float64(0.01)),
            ]
        )

        var msg2 = SRUIMessage()
        msg2.transaction = updateTx.toWire()
        let bytes2 = try SRUIFraming.encodeFramed(msg2)
        try await serverTransport.send(data: bytes2)

        try await Task.sleep(nanoseconds: 50_000_000)

        // Verify scalar update in place
        #expect(applier.lastAppliedRevision == Revision(2))
        let updatedTextHandle = try #require(renderer.registry.handle(for: textID))
        #expect(ObjectIdentifier(updatedTextHandle.view) == textIdentity)
        #expect(ObjectIdentifier(buttonHandle.view) == buttonIdentity)

        let textField = updatedTextHandle.view as? NSTextField
        #expect(textField?.stringValue == "Count: 1")

        await controller.stop()
        await serverTransport.close()
    }

    @Test("Renderer never observes half-committed transactions during network processing")
    func atomicityGuaranteedUnderConcurrency() async throws {
        let applier = TransactionApplier()
        let initialStore = applier.currentSnapshot.store

        // Create a transaction with an intentional invalid operation at index 2
        let badTx = Transaction(
            baseRevision: .initial,
            newRevision: Revision(1),
            operations: [
                .createNode(id: NodeId(1), nodeType: .surface),
                .createNode(id: NodeId(2), nodeType: .text, parentID: NodeId(1)),
                .createNode(id: NodeId(3), nodeType: .text, parentID: NodeId(999)), // Nonexistent parent!
            ]
        )

        let result = applier.apply(record: badTx)
        #expect(result.isFailure)

        // Verify store remains in exact initial state
        let currentSnapshot = applier.currentSnapshot
        #expect(currentSnapshot.revision == .initial)
        #expect(currentSnapshot.store.nodeCount == 0)
        #expect(currentSnapshot.store == initialStore)
    }

    @Test("Button action invokes outbox and transmits ACTIVATE event over transport")
    @MainActor
    func buttonActionDispatchesActivateEvent() async throws {
        let (clientTransport, serverTransport) = await PipeTransport.createPair()
        let applier = TransactionApplier()
        let outbox = EventOutbox()
        let renderer = AppKitRenderer()

        let controller = SessionController(
            transport: clientTransport,
            applier: applier,
            outbox: outbox,
            renderer: renderer
        )
        controller.attachRenderer(renderer)

        try await controller.start()

        var welcome = SRUIServerWelcome()
        welcome.sessionID = "default"
        welcome.requiredProfiles = ["org.srui.standard-widgets/1"]
        var welcomeMessage = SRUIMessage()
        welcomeMessage.serverWelcome = welcome
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(welcomeMessage))

        // Mount a button
        let buttonID = NodeId(42)
        let mountTx = Transaction(
            baseRevision: .initial,
            newRevision: Revision(1),
            operations: [
                .createNode(id: NodeId(1), nodeType: .surface),
                .createNode(id: buttonID, nodeType: .button, parentID: NodeId(1), properties: [
                    Property(property: .label, value: .string("Click Me"))
                ]),
            ]
        )

        var mountMsg = SRUIMessage()
        mountMsg.transaction = mountTx.toWire()
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(mountMsg))

        try await Task.sleep(nanoseconds: 50_000_000)

        let buttonHandle = try #require(renderer.registry.handle(for: buttonID))
        let button = try #require(buttonHandle.view as? NSButton)

        // Trigger button action via target-action trampoline
        let serverStream = serverTransport.receiveStream()

        let trampoline = try #require(buttonHandle.actionTrampoline as? ActionTrampoline)
        trampoline.performAction(button)

        // Read outgoing event on server
        var receivedEventData: Data?
        for try await chunk in serverStream {
            // Note: the handshake ClientResume was sent first, so decode messages
            var streamDecoder = SRUIMessageStreamDecoder()
            let messages = try streamDecoder.appendAndExtract(incoming: chunk)
            if let eventMsg = messages.first(where: {
                if case .event = $0.msg { return true }
                return false
            }) {
                receivedEventData = try SRUIFraming.encodeFramed(eventMsg)
                break
            }
        }

        let nonNilData = try #require(receivedEventData)
        let decodedMsg = try decodeFramedMessage(from: nonNilData)
        guard case .event(let wireEvent) = decodedMsg.msg else {
            Issue.record("Expected event message payload")
            return
        }
        let event = try ProtocolDecoder().validateAndConvertEvent(wire: wireEvent)

        #expect(event.nodeId == buttonID)
        #expect(event.eventType == .EVENT_ACTIVATE)
        #expect(event.observedRevision == Revision(1))

        await controller.stop()
        await serverTransport.close()
    }
}

private extension Result {
    var isFailure: Bool {
        if case .failure = self { return true }
        return false
    }
}
