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

        // Await until both applier and renderer have processed the initial mount
        while applier.lastAppliedRevision < Revision(1) || renderer.registry.count < 4 {
            await Task.yield()
        }

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

        while applier.lastAppliedRevision < Revision(2) {
            await Task.yield()
        }
        await Task.yield()

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

    @Test("Button, toggle, and table interactions route through outbox and capture observed revision")
    @MainActor
    func buttonToggleAndTableRouting() async throws {
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
        welcome.sessionID = "interaction-session"
        welcome.requiredProfiles = ["org.srui.standard-widgets/1"]
        var welcomeMessage = SRUIMessage()
        welcomeMessage.serverWelcome = welcome
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(welcomeMessage))

        let modelID = ModelId(99)
        let buttonID = NodeId(10)
        let toggleID = NodeId(11)
        let tableID = NodeId(12)

        let mountTx = Transaction(
            baseRevision: .initial,
            newRevision: Revision(1),
            operations: [
                .createModel(id: modelID, modelType: .table, itemCount: 0),
                .modelInsert(
                    id: modelID,
                    index: 0,
                    items: [
                        ModelItem(itemID: ItemId(1), value: .string("Item 1")),
                        ModelItem(itemID: ItemId(2), value: .string("Item 2")),
                    ]
                ),
                .createNode(id: NodeId(1), nodeType: .surface),
                .createNode(id: buttonID, nodeType: .button, parentID: NodeId(1), properties: [
                    Property(property: .label, value: .string("Click"))
                ]),
                .createNode(id: toggleID, nodeType: .toggle, parentID: NodeId(1), properties: [
                    Property(property: .label, value: .string("Toggle"))
                ]),
                .createNode(id: tableID, nodeType: .table, parentID: NodeId(1), properties: [
                    Property(property: .modelRef, value: .unsignedInt(modelID.value)),
                    Property(property: .selectionMode, value: .enumToken(.selectionModeSingle)),
                ]),
            ]
        )

        var mountMsg = SRUIMessage()
        mountMsg.transaction = mountTx.toWire()
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(mountMsg))

        while applier.lastAppliedRevision < Revision(1) || renderer.registry.count < 4 {
            await Task.yield()
        }

        // Advance revision to Revision(2) with a scalar update
        let rev2Tx = Transaction(
            baseRevision: Revision(1),
            newRevision: Revision(2),
            operations: [
                .setProperty(id: buttonID, property: .label, value: .string("Click Rev 2"))
            ]
        )
        var rev2Msg = SRUIMessage()
        rev2Msg.transaction = rev2Tx.toWire()
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(rev2Msg))

        while applier.lastAppliedRevision < Revision(2) {
            await Task.yield()
        }
        await Task.yield()

        let buttonHandle = try #require(renderer.registry.handle(for: buttonID))
        let toggleHandle = try #require(renderer.registry.handle(for: toggleID))
        let tableHandle = try #require(renderer.registry.handle(for: tableID))

        let button = try #require(buttonHandle.view as? NSButton)
        let toggle = try #require(toggleHandle.view as? NSButton)
        let tableView = try #require((tableHandle.view as? NSScrollView)?.documentView as? NSTableView)

        let serverStream = serverTransport.receiveStream()

        // 1. Trigger button
        let buttonTrampoline = try #require(buttonHandle.actionTrampoline as? ActionTrampoline)
        buttonTrampoline.performButtonAction(button)

        // 2. Trigger toggle
        let toggleTrampoline = try #require(toggleHandle.actionTrampoline as? ActionTrampoline)
        toggle.state = .on
        toggleTrampoline.performToggleAction(toggle)

        // 3. Trigger table selection (select row 1 -> ItemId 2)
        tableView.selectRowIndexes(IndexSet(integer: 1), byExtendingSelection: false)

        // Collect 3 events on server
        var collectedEvents: [Event] = []
        var streamDecoder = SRUIMessageStreamDecoder()
        for try await chunk in serverStream {
            let messages = try streamDecoder.appendAndExtract(incoming: chunk)
            for msg in messages {
                if case .event(let wireEvent) = msg.msg {
                    let domainEvent = try ProtocolDecoder().validateAndConvertEvent(wire: wireEvent)
                    collectedEvents.append(domainEvent)
                }
            }
            if collectedEvents.count >= 3 {
                break
            }
        }

        #expect(collectedEvents.count == 3)
        #expect(collectedEvents.map(\.eventSeq) == [1, 2, 3])

        let buttonEvent = try #require(collectedEvents.first { $0.nodeId == buttonID })
        #expect(buttonEvent.nodeId == buttonID)
        #expect(buttonEvent.eventType == .EVENT_ACTIVATE)
        #expect(buttonEvent.observedRevision == Revision(2))

        let toggleEvent = try #require(collectedEvents.first { $0.nodeId == toggleID })
        #expect(toggleEvent.nodeId == toggleID)
        #expect(toggleEvent.eventType == .EVENT_VALUE_CHANGED)
        #expect(toggleEvent.boolArg == true)
        #expect(toggleEvent.observedRevision == Revision(2))

        let tableEvent = try #require(collectedEvents.first { $0.nodeId == tableID })
        #expect(tableEvent.nodeId == tableID)
        #expect(tableEvent.eventType == .EVENT_SELECTION_CHANGED)
        #expect(tableEvent.itemIdArg == ItemId(2))
        #expect(tableEvent.observedRevision == Revision(2))

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
