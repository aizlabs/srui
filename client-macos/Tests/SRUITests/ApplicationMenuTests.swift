//
// ApplicationMenuTests.swift
// SRUITests
//

import AppKit
@testable import ConnectionManager
import RendererAppKit
import SemanticModel
import Testing

@Suite("Application Menu Tests")
struct ApplicationMenuTests {
    @Test("Application menu exposes the standard Command-Q quit action")
    @MainActor
    func commandQQuitAction() throws {
        let application = NSApplication.shared
        let menu = SRUIApplicationMenu.make(
            applicationName: "SRUI Test",
            target: application
        )

        let applicationMenu = try #require(menu.items.first?.submenu)
        let quitItem = try #require(applicationMenu.items.first)

        #expect(quitItem.title == "Quit SRUI Test")
        #expect(quitItem.action == #selector(NSApplication.terminate(_:)))
        #expect(quitItem.keyEquivalent == "q")
        #expect(quitItem.keyEquivalentModifierMask == [.command])
        #expect(quitItem.target === application)
    }

    /// PX-016-G01 (§22.5, §22.6): the generic shell's standard Edit menu; every item is sent to
    /// the first responder, so the focused local editor performs it.
    @Test("Application menu exposes the standard Edit menu")
    @MainActor
    func standardEditMenu() throws {
        let menu = SRUIApplicationMenu.make(applicationName: "SRUI Test", target: NSApplication.shared)
        let editItem = try #require(menu.items.first { $0.title == "Edit" })
        let edit = try #require(editItem.submenu)
        let items = edit.items.filter { !$0.isSeparatorItem }
        #expect(items.map(\.title) == ["Undo", "Redo", "Cut", "Copy", "Paste", "Select All"])
        #expect(items.map { $0.action.map(NSStringFromSelector) } == [
            "undo:", "redo:", "cut:", "copy:", "paste:", "selectAll:",
        ])
        #expect(items.map(\.keyEquivalent) == ["z", "z", "x", "c", "v", "a"])
        #expect(items.map(\.keyEquivalentModifierMask) == [
            [.command], [.command, .shift], [.command], [.command], [.command], [.command],
        ])
        #expect(items.allSatisfy { $0.target == nil })
    }

    /// PX-016-G01: Cmd-C in a TextInput is not claimed by the Surface's shortcut router (it is
    /// reserved, so a server `primary+c` stays unbound) and copies through the Edit menu's item.
    @Test("Cmd-C in a TextInput copies through the Edit menu and is not overridden")
    @MainActor
    func commandCCopiesThroughTheEditMenu() throws {
        var store = SemanticStore()
        try store.createNode(id: 1, nodeType: .surface)
        try store.createNode(id: 2, nodeType: .textInput, parentID: 1, properties: [(.value, .string("copied text"))])
        try store.createNode(id: 3, nodeType: .button, parentID: 1, properties: [
            (.label, .string("Copy row")), (.shortcut, .string("primary+c")),
        ])
        let renderer = AppKitRenderer()
        var interactions: [SemanticInteraction] = []
        renderer.onInteraction = { interactions.append($0) }
        try renderer.attach(store: store)
        renderer.layoutRenderer.shortcutRouter.isWindowKey = { _ in true }
        let window = try #require(renderer.registry.handle(for: 1)?.window)
        defer { window.orderOut(nil); window.close() }
        #expect(renderer.registry.handle(for: 3)?.shortcutBinding == nil)

        let field = try #require(renderer.registry.view(for: 2) as? NSTextField)
        #expect(window.makeFirstResponder(field))
        let editor = try #require(field.currentEditor() as? NSTextView)
        editor.selectAll(nil)

        let event = try #require(NSEvent.keyEvent(
            with: .keyDown, location: .zero, modifierFlags: [.command], timestamp: 0,
            windowNumber: window.windowNumber, context: nil, characters: "c",
            charactersIgnoringModifiers: "c", isARepeat: false, keyCode: 8
        ))
        // AppKit offers a Command key-down to the key window's views first: nothing claims it.
        #expect(window.performKeyEquivalent(with: event) == false)
        #expect(interactions.isEmpty)

        // Then to the main menu: the Edit menu's item for exactly this key equivalent is Copy,
        // targeted at the first responder, which the focused field editor implements.
        let menu = SRUIApplicationMenu.make(applicationName: "SRUI Test", target: NSApplication.shared)
        let edit = try #require(menu.items.first { $0.title == "Edit" }?.submenu)
        let item = try #require(edit.items.first {
            $0.keyEquivalent == event.charactersIgnoringModifiers
                && $0.keyEquivalentModifierMask
                    == event.modifierFlags.intersection(AppKitShortcutBinding.relevantModifiers)
        })
        #expect(item.title == "Copy")
        let action = try #require(item.action)
        let pasteboard = NSPasteboard.general
        let saved = pasteboard.string(forType: .string)
        defer {
            pasteboard.clearContents()
            if let saved { pasteboard.setString(saved, forType: .string) }
        }
        pasteboard.clearContents()
        #expect(NSApplication.shared.sendAction(action, to: window.firstResponder, from: item))
        #expect(pasteboard.string(forType: .string) == "copied text")
        #expect(interactions.isEmpty)
    }
}
