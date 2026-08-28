//
// CounterLiveIntegrationTests.swift
// SRUITests
//
// End-to-end integration test executing 3+ consecutive click-and-observe cycles (§7.7, §12.1, §22, §29).
//

import Testing
import Foundation
import AppKit
import SemanticModel
import Protocol
import Session
import TransportSSH
import RendererAppKit

@Suite("Counter Live Integration Tests")
struct CounterLiveIntegrationTests {

    @Test("Execute 3 consecutive click-and-observe cycles over transport")
    @MainActor
    func threeConsecutiveClickAndObserveCycles() async throws {
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

        try await controller.start()

        let surfaceID = NodeId(1)
        let textID = NodeId(2)
        let progressID = NodeId(3)
        let buttonID = NodeId(4)

        // 1. Server sends initial UI tree (Revision 0 -> 1)
        let initialTx = Transaction(
            baseRevision: .initial,
            newRevision: Revision(1),
            operations: [
                .createNode(id: surfaceID, nodeType: .surface, properties: [
                    Property(property: .label, value: .string("Counter Application"))
                ]),
                .createNode(id: textID, nodeType: .text, parentID: surfaceID, properties: [
                    Property(property: .text, value: .string("Count: 0")),
                    Property(property: .role, value: .enumToken(.textRoleHeading))
                ]),
                .createNode(id: progressID, nodeType: .progress, parentID: surfaceID, properties: [
                    Property(property: .value, value: .float64(0.0)),
                    Property(property: .valueDescription, value: .string("0 / 100"))
                ]),
                .createNode(id: buttonID, nodeType: .button, parentID: surfaceID, properties: [
                    Property(property: .label, value: .string("Increment")),
                    Property(property: .role, value: .enumToken(.actionRolePrimary))
                ]),
            ]
        )

        var initialMsg = SRUIMessage()
        initialMsg.transaction = initialTx.toWire()
        try await serverTransport.send(data: try SRUIFraming.encodeFramed(initialMsg))

        // Wait for initial render mount
        try await Task.sleep(nanoseconds: 50_000_000)

        #expect(applier.lastAppliedRevision == Revision(1))
        let textHandle = try #require(renderer.registry.handle(for: textID))
        let textField = try #require(textHandle.view as? NSTextField)
        let progressHandle = try #require(renderer.registry.handle(for: progressID))
        let progressIndicator = try #require(progressHandle.view as? NSProgressIndicator)
        let buttonHandle = try #require(renderer.registry.handle(for: buttonID))
        let buttonTrampoline = try #require(buttonHandle.actionTrampoline as? ActionTrampoline)

        #expect(textField.stringValue == "Count: 0")
        #expect(progressIndicator.doubleValue == 0.0)

        // Server simulated event handler loop
        let serverTask = Task.detached {
            let serverStream = serverTransport.receiveStream()
            var currentCount: UInt64 = 0
            var streamDecoder = SRUIMessageStreamDecoder()

            for try await chunk in serverStream {
                let messages = try streamDecoder.appendAndExtract(incoming: chunk)
                for msg in messages {
                    if case .event(let wireEvent) = msg.msg {
                        let event = try ProtocolDecoder().validateAndConvertEvent(wire: wireEvent)
                        if event.nodeId == buttonID && event.eventType == .EVENT_ACTIVATE {
                            currentCount += 1
                            let newRev = event.observedRevision.next
                            let serverTx = Transaction(
                                baseRevision: event.observedRevision,
                                newRevision: newRev,
                                operations: [
                                    .setProperty(id: textID, property: .text, value: .string("Count: \(currentCount)")),
                                    .setProperty(id: progressID, property: .value, value: .float64(Double(currentCount) / 100.0)),
                                    .setProperty(id: progressID, property: .valueDescription, value: .string("\(currentCount) / 100")),
                                ]
                            )
                            var responseMsg = SRUIMessage()
                            responseMsg.transaction = serverTx.toWire()
                            try await serverTransport.send(data: try SRUIFraming.encodeFramed(responseMsg))
                        }
                    }
                }
            }
        }

        // Execute 3 consecutive click-and-observe cycles (§7.7, §12.1)
        for cycle in 1...3 {
            // Click the button in the real AppKit control
            buttonTrampoline.performAction(buttonHandle.view)

            // Wait for roundtrip transaction update
            try await Task.sleep(nanoseconds: 80_000_000)

            #expect(applier.lastAppliedRevision == Revision(UInt64(cycle + 1)))
            #expect(textField.stringValue == "Count: \(cycle)")
            #expect(abs(progressIndicator.doubleValue - (Double(cycle) / 100.0)) < 0.0001)
        }

        serverTask.cancel()
        await controller.stop()
        await serverTransport.close()
    }
}
