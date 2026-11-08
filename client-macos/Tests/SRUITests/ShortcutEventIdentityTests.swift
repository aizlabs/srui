//
// ShortcutEventIdentityTests.swift
// SRUITests
//
// PX-016-G01 (§7.6, §7.7, §32 suite 5): a Button or Toggle shortcut produces exactly the
// `SemanticInteraction` a click produces, and the session builds the wire event from that
// interaction alone (SessionController.renderer.onInteraction), so the encoded events are
// byte-identical for the same sequence number, id and observed revision.
//

import AppKit
import Foundation
@testable import Protocol
@testable import RendererAppKit
import SemanticModel
import Testing

@Suite(.timeLimit(.minutes(1)))
@MainActor
struct ShortcutEventIdentityTests {
    private func wireBytes(for interaction: SemanticInteraction) throws -> Data {
        let event: Event
        switch interaction {
        case .activate(let nodeID):
            event = Event(eventSeq: 7, eventId: "shortcut-identity", observedRevision: Revision(3),
                          nodeId: nodeID, eventType: .EVENT_ACTIVATE)
        case .valueChanged(let nodeID, let value):
            event = Event(eventSeq: 7, eventId: "shortcut-identity", observedRevision: Revision(3),
                          nodeId: nodeID, eventType: .EVENT_VALUE_CHANGED, arguments: [.value: value])
        default:
            Issue.record("unexpected interaction \(interaction)")
            return Data()
        }
        return try encodeEvent(event)
    }

    @Test
    func shortcutEventsAreByteIdenticalToClickEvents() throws {
        var store = SemanticStore()
        try store.createNode(id: 1, nodeType: .surface)
        try store.createNode(id: 2, nodeType: .button, parentID: 1, properties: [(.shortcut, .string("primary+r"))])
        try store.createNode(id: 3, nodeType: .toggle, parentID: 1, properties: [(.shortcut, .string("primary+t"))])
        let renderer = AppKitRenderer()
        var interactions: [SemanticInteraction] = []
        renderer.onInteraction = { interactions.append($0) }
        try renderer.attach(store: store)
        renderer.layoutRenderer.shortcutRouter.isWindowKey = { _ in true }
        let window = try #require(renderer.registry.handle(for: 1)?.window)
        defer { window.orderOut(nil); window.close() }

        func press(_ character: String, _ code: UInt16) throws -> Bool {
            let event = try #require(NSEvent.keyEvent(
                with: .keyDown, location: .zero, modifierFlags: [.command], timestamp: 0,
                windowNumber: window.windowNumber, context: nil, characters: character,
                charactersIgnoringModifiers: character, isARepeat: false, keyCode: code
            ))
            return window.performKeyEquivalent(with: event)
        }

        // Button: a click, then the shortcut.
        let button = try #require(renderer.registry.view(for: 2) as? NSButton)
        #expect(button.sendAction(try #require(button.action), to: button.target))
        #expect(try press("r", 15))
        #expect(interactions.count == 2)
        #expect(interactions[0] == interactions[1])
        #expect(try wireBytes(for: interactions[0]) == wireBytes(for: interactions[1]))

        // Toggle: a click flips the checkbox and sends; the shortcut flips it back the same way.
        let toggle = try #require(renderer.registry.view(for: 3) as? NSButton)
        toggle.state = .on
        #expect(toggle.sendAction(try #require(toggle.action), to: toggle.target))
        #expect(try press("t", 17))
        #expect(toggle.state == .off)
        #expect(interactions.count == 4)
        #expect(interactions[2] == .valueChanged(nodeID: 3, value: .bool(true)))
        #expect(interactions[3] == .valueChanged(nodeID: 3, value: .bool(false)))
        let clickedOff = try wireBytes(for: .valueChanged(nodeID: 3, value: .bool(false)))
        #expect(try wireBytes(for: interactions[3]) == clickedOff)
    }
}
