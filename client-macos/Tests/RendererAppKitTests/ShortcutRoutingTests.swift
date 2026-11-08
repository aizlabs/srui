//
// ShortcutRoutingTests.swift
// RendererAppKitTests
//
// Native AppKit behaviour of the standard `shortcut` property (PX-016-G01), driven with real
// `NSEvent` key-down events through `NSWindow.performKeyEquivalent(with:)`.
//
// Implements: §4 inv. 3 (focus and IME stay local), §7.6/§7.7 (a Button/Toggle shortcut is the
// click's target/action), §22.5 (focus-only targets send nothing), §22.6 (no dispatch during
// marked text; typed letters never trigger a shortcut), §22.8 (tooltip and accessibility help),
// §23 (SET/CLEAR are in-place updates). §32 suites 5, 6, 10 and 12 run this file.
//
// Dispatch never calls `performClick(_:)`: it uses the control's own `sendAction(_:to:)`, the
// event-loop-free path NativeActivationTestSupport.swift documents.
//

import AppKit
import Foundation
import SemanticModel
import Testing

@testable import RendererAppKit

@Suite(.timeLimit(.minutes(1)))
@MainActor
struct ShortcutRoutingTests {

    // MARK: - Fixture

    private static let base: [SemanticModel.Operation] = [
        .createNode(id: 1, nodeType: .surface, properties: [(.label, .string("Shortcuts"))]),
        .createNode(id: 2, nodeType: .button, parentID: 1, properties: [
            (.label, .string("Refresh")), (.shortcut, .string("primary+r")),
        ]),
        .createNode(id: 3, nodeType: .toggle, parentID: 1, properties: [
            (.label, .string("Follow")), (.shortcut, .string("primary+shift+t")),
        ]),
        .createNode(id: 4, nodeType: .textInput, parentID: 1, properties: [
            (.label, .string("Filter")), (.shortcut, .string("primary+l")),
        ]),
        .createNode(id: 5, nodeType: .table, parentID: 1, properties: [
            (.label, .string("Processes")), (.shortcut, .string("primary+2")),
        ]),
        .createNode(id: 6, nodeType: .textArea, parentID: 1, properties: [
            (.shortcut, .string("primary+alt+n")),
        ]),
        .createNode(id: 7, nodeType: .richText, parentID: 1, properties: [
            (.text, .string("Details")), (.shortcut, .string("f6")),
        ]),
        .createNode(id: 8, nodeType: .button, parentID: 1, properties: [
            (.label, .string("Copy row")), (.shortcut, .string("primary+c")),
        ]),
    ]

    @MainActor
    private struct Harness {
        let renderer: AppKitRenderer
        let window: NSWindow
        var store: SemanticStore
        var interactions: [SemanticInteraction] { recorder.values }
        let recorder: Recorder

        func close() {
            window.orderOut(nil)
            window.close()
        }
    }

    @MainActor
    final class Recorder {
        var values: [SemanticInteraction] = []
    }

    private func makeStore(_ operations: [SemanticModel.Operation]) throws -> SemanticStore {
        var store = SemanticStore()
        for operation in operations {
            try operation.apply(to: &store)
        }
        return store
    }

    private func mount(_ operations: [SemanticModel.Operation] = base, key: Bool = true) throws -> Harness {
        let store = try makeStore(operations)
        let renderer = AppKitRenderer()
        let recorder = Recorder()
        renderer.onInteraction = { recorder.values.append($0) }
        try renderer.attach(store: store)
        renderer.layoutRenderer.shortcutRouter.isWindowKey = { _ in key }
        let window = try #require(renderer.registry.handle(for: 1)?.window)
        return Harness(renderer: renderer, window: window, store: store, recorder: recorder)
    }

    private func apply(_ operations: [SemanticModel.Operation], to harness: inout Harness) throws
        -> [DirtyClassification]
    {
        var next = harness.store
        for operation in operations {
            try operation.apply(to: &next)
        }
        let classifications = try harness.renderer.apply(
            transaction: Transaction(baseRevision: harness.store.revision, operations: operations),
            newStore: next
        )
        harness.store = next
        return classifications
    }

    /// A real key-down event. `keyCode` matters only for named keys, which match by key code.
    private func key(
        _ characters: String,
        _ modifiers: NSEvent.ModifierFlags = [],
        keyCode: UInt16 = 0,
        in window: NSWindow
    ) throws -> NSEvent {
        try #require(NSEvent.keyEvent(
            with: .keyDown,
            location: .zero,
            modifierFlags: modifiers,
            timestamp: 0,
            windowNumber: window.windowNumber,
            context: nil,
            characters: characters,
            charactersIgnoringModifiers: characters,
            isARepeat: false,
            keyCode: keyCode
        ))
    }

    // MARK: - Suite 12: toolkit mapping

    @Test
    func shortcutsMapToKeyEquivalentAndModifierMask() throws {
        let cases: [(String, String, NSEvent.ModifierFlags, UInt16?, String)] = [
            ("primary+r", "r", [.command], nil, "⌘R"),
            ("primary+shift+t", "t", [.command, .shift], nil, "⇧⌘T"),
            ("primary+alt+n", "n", [.command, .option], nil, "⌥⌘N"),
            ("primary+shift+alt+1", "1", [.command, .shift, .option], nil, "⌥⇧⌘1"),
            ("primary+return", "\r", [.command], 36, "⌘↩"),
            ("primary+escape", "\u{1b}", [.command], 53, "⌘⎋"),
            ("primary+delete", "\u{08}", [.command], 51, "⌘⌫"),
            ("primary+up", String(Character(UnicodeScalar(UInt32(NSUpArrowFunctionKey))!)), [.command], 126, "⌘↑"),
            ("primary+page_down", String(Character(UnicodeScalar(UInt32(NSPageDownFunctionKey))!)), [.command], 121, "⌘⇟"),
            ("f6", String(Character(UnicodeScalar(UInt32(NSF6FunctionKey))!)), [], 97, "F6"),
            ("shift+f12", String(Character(UnicodeScalar(UInt32(NSF12FunctionKey))!)), [.shift], 111, "⇧F12"),
        ]
        for (value, keyEquivalent, mask, keyCode, display) in cases {
            let binding = AppKitShortcutBinding(try Shortcut(parsing: value))
            #expect(binding.keyEquivalent == keyEquivalent, "\(value)")
            #expect(binding.modifierMask == mask, "\(value)")
            #expect(binding.keyCode == keyCode, "\(value)")
            #expect(binding.displayString == display, "\(value)")
        }
    }

    @Test
    func mountedHandlesCarryTheirBindingAndAReservedComboStaysUnboundWithADiagnostic() throws {
        let harness = try mount()
        defer { harness.close() }
        let registry = harness.renderer.registry
        #expect(registry.handle(for: 2)?.shortcutBinding?.keyEquivalent == "r")
        #expect(registry.handle(for: 2)?.shortcutBinding?.modifierMask == [.command])
        #expect(registry.handle(for: 3)?.shortcutBinding?.modifierMask == [.command, .shift])

        let reserved = try #require(registry.handle(for: 8))
        #expect(reserved.shortcutBinding == nil)
        #expect(
            reserved.shortcutDiagnostic
                == .reserved(nodeID: 8, shortcut: "primary+c", reservedName: "Cmd-C")
        )
        #expect(reserved.view.toolTip == nil)
        // Cmd-C is left to the Edit menu and the focused editor; the Surface never claims it.
        let copy = try key("c", [.command], keyCode: 8, in: harness.window)
        #expect(harness.window.performKeyEquivalent(with: copy) == false)
        #expect(harness.interactions.isEmpty)

        // Every expressible macOS reserved entry stays unbound.
        for entry in Shortcut.macOSReserved {
            guard let canonical = entry.canonical else { continue }
            let shortcut = try Shortcut(parsing: canonical)
            #expect(shortcut.reservedEntry(on: .macOS) == entry)
        }
        #expect(try Shortcut(parsing: "primary+shift+h").reservedEntry(on: .macOS) == nil)
        #expect(try Shortcut(parsing: "primary+q").reservedEntry(on: .other) == nil)
    }

    // MARK: - Suite 5: semantic input

    @Test
    func aCommandShortcutActivatesTheButtonExactlyAsAClickDoes() throws {
        let harness = try mount()
        defer { harness.close() }
        let button = try #require(harness.renderer.registry.view(for: 2) as? NSButton)
        let action = try #require(button.action)
        #expect(button.sendAction(action, to: button.target))
        let click = harness.interactions
        #expect(click == [.activate(nodeID: 2)])

        let event = try key("r", [.command], keyCode: 15, in: harness.window)
        #expect(harness.window.performKeyEquivalent(with: event))
        #expect(harness.interactions == click + click)
        #expect(harness.renderer.layoutRenderer.shortcutRouter.lastRefusal == nil)

        // A different modifier set is a different shortcut and dispatches nothing.
        let shifted = try key("r", [.command, .shift], keyCode: 15, in: harness.window)
        #expect(harness.window.performKeyEquivalent(with: shifted) == false)
        #expect(harness.interactions.count == 2)
    }

    @Test
    func aToggleShortcutChangesItsValueThroughTheClickPath() throws {
        let harness = try mount()
        defer { harness.close() }
        let toggle = try #require(harness.renderer.registry.view(for: 3) as? NSButton)
        let event = try key("t", [.command, .shift], keyCode: 17, in: harness.window)
        #expect(harness.window.performKeyEquivalent(with: event))
        #expect(toggle.state == .on)
        #expect(harness.window.performKeyEquivalent(with: event))
        #expect(toggle.state == .off)
        #expect(harness.interactions == [
            .valueChanged(nodeID: 3, value: .bool(true)),
            .valueChanged(nodeID: 3, value: .bool(false)),
        ])
    }

    @Test
    func aFocusShortcutMovesLocalFocusAndEmitsNoEvent() throws {
        let harness = try mount()
        defer { harness.close() }
        let registry = harness.renderer.registry
        let field = try #require(registry.view(for: 4) as? NSTextField)
        #expect(harness.window.performKeyEquivalent(with: try key("l", [.command], keyCode: 37, in: harness.window)))
        #expect(field.currentEditor() != nil)
        #expect((harness.window.firstResponder as? NSTextView)?.delegate === field)

        let table = try #require((registry.view(for: 5) as? NSScrollView)?.documentView as? NSTableView)
        #expect(harness.window.performKeyEquivalent(with: try key("2", [.command], keyCode: 19, in: harness.window)))
        #expect(harness.window.firstResponder === table)

        let area = try #require((registry.view(for: 6) as? NSScrollView)?.documentView as? NSTextView)
        #expect(harness.window.performKeyEquivalent(with: try key("n", [.command, .option], keyCode: 45, in: harness.window)))
        #expect(harness.window.firstResponder === area)

        let rich = try #require((registry.view(for: 7) as? NSScrollView)?.documentView as? NSTextView)
        let f6 = String(Character(UnicodeScalar(UInt32(NSF6FunctionKey))!))
        #expect(harness.window.performKeyEquivalent(with: try key(f6, [.function], keyCode: 97, in: harness.window)))
        #expect(harness.window.firstResponder === rich)

        #expect(harness.interactions.isEmpty)
    }

    @Test
    func aDisabledHiddenOrUnfocusedWindowNodeDispatchesNothing() throws {
        var harness = try mount()
        defer { harness.close() }
        let router = harness.renderer.layoutRenderer.shortcutRouter
        let event = try key("r", [.command], keyCode: 15, in: harness.window)

        _ = try apply([.setProperty(id: 2, property: .enabled, value: .bool(false))], to: &harness)
        #expect(harness.window.performKeyEquivalent(with: event) == false)
        #expect(router.lastRefusal == .disabled)

        _ = try apply([
            .setProperty(id: 2, property: .enabled, value: .bool(true)),
            .setProperty(id: 2, property: .visibility, value: .enumToken(.visibilityCollapsed)),
        ], to: &harness)
        #expect(harness.window.performKeyEquivalent(with: event) == false)
        #expect(router.lastRefusal == .hidden)

        _ = try apply([
            .setProperty(id: 2, property: .visibility, value: .enumToken(.visibilityVisible)),
            .setProperty(id: 1, property: .visibility, value: .enumToken(.visibilityHidden)),
        ], to: &harness)
        #expect(harness.window.performKeyEquivalent(with: event) == false)
        #expect(router.lastRefusal == .hidden)

        _ = try apply([.clearProperty(id: 1, property: .visibility)], to: &harness)
        router.isWindowKey = { _ in false }
        #expect(harness.window.performKeyEquivalent(with: event) == false)
        #expect(router.lastRefusal == .windowNotKey)
        #expect(harness.interactions.isEmpty)

        router.isWindowKey = { _ in true }
        #expect(harness.window.performKeyEquivalent(with: event))
        #expect(harness.interactions == [.activate(nodeID: 2)])
    }

    // MARK: - Suite 6: local text interaction

    @Test
    func noShortcutDispatchesWhileTheFirstResponderHasMarkedText() throws {
        let harness = try mount()
        defer { harness.close() }
        let field = try #require(harness.renderer.registry.view(for: 4) as? NSTextField)
        #expect(harness.window.makeFirstResponder(field))
        let editor = try #require(field.currentEditor() as? NSTextView)
        editor.setMarkedText(
            "かな",
            selectedRange: NSRange(location: 2, length: 0),
            replacementRange: editor.selectedRange
        )
        #expect(editor.hasMarkedText())

        let event = try key("r", [.command], keyCode: 15, in: harness.window)
        #expect(harness.window.performKeyEquivalent(with: event) == false)
        #expect(harness.renderer.layoutRenderer.shortcutRouter.lastRefusal == .markedText)
        #expect(harness.interactions.isEmpty)
        #expect(editor.hasMarkedText(), "the composition is untouched")

        editor.unmarkText()
        #expect(harness.window.performKeyEquivalent(with: event))
        #expect(harness.interactions == [.activate(nodeID: 2)])
    }

    @Test
    func lettersTypedIntoATextInputNeverTriggerAShortcut() throws {
        let harness = try mount()
        defer { harness.close() }
        let field = try #require(harness.renderer.registry.view(for: 4) as? NSTextField)
        #expect(harness.window.makeFirstResponder(field))
        let editor = try #require(field.currentEditor() as? NSTextView)

        for (character, code) in [("r", UInt16(15)), ("l", 37), ("t", 17), ("2", 19), ("n", 45)] {
            for modifiers: NSEvent.ModifierFlags in [[], [.shift], [.option]] {
                let typed = try key(character, modifiers, keyCode: code, in: harness.window)
                #expect(harness.window.performKeyEquivalent(with: typed) == false)
            }
            editor.insertText(character, replacementRange: editor.selectedRange)
        }
        #expect(editor.string == "rlt2n")
        #expect(harness.interactions.isEmpty)
        #expect((harness.window.firstResponder as? NSTextView)?.delegate === field)
    }

    // MARK: - Suites 10 and 12: renderer semantics

    @Test
    func setAndClearUpdateTheBindingAndDiscoverabilityInPlace() throws {
        var harness = try mount()
        defer { harness.close() }
        let before = try #require(harness.renderer.registry.view(for: 2) as? NSButton)
        #expect(before.toolTip == "Refresh (⌘R)")
        #expect(before.accessibilityHelp() == "Keyboard shortcut ⌘R")

        let set = try apply([
            .setProperty(id: 2, property: .shortcut, value: .string("primary+shift+e")),
            .setProperty(id: 2, property: .accessibleDescription, value: .string("Reload the table")),
        ], to: &harness)
        #expect(!set.contains(where: { $0.isStructureAffecting }))
        #expect(set.first == .accessibilityOnly(nodeID: 2, property: .shortcut))
        let after = try #require(harness.renderer.registry.view(for: 2) as? NSButton)
        #expect(after === before)
        #expect(harness.renderer.registry.handle(for: 2)?.shortcutBinding?.displayString == "⇧⌘E")
        #expect(after.toolTip == "Refresh (⇧⌘E)")
        #expect(after.accessibilityHelp() == "Reload the table. Keyboard shortcut ⇧⌘E")
        let old = try key("r", [.command], keyCode: 15, in: harness.window)
        #expect(harness.window.performKeyEquivalent(with: old) == false)
        let new = try key("e", [.command, .shift], keyCode: 14, in: harness.window)
        #expect(harness.window.performKeyEquivalent(with: new))

        let clear = try apply([.clearProperty(id: 2, property: .shortcut)], to: &harness)
        #expect(!clear.contains(where: { $0.isStructureAffecting }))
        #expect(harness.renderer.registry.view(for: 2) === before)
        #expect(harness.renderer.registry.handle(for: 2)?.shortcutBinding == nil)
        #expect(after.toolTip == nil)
        #expect(after.accessibilityHelp() == "Reload the table")
        #expect(harness.window.performKeyEquivalent(with: new) == false)
        #expect(harness.interactions == [.activate(nodeID: 2)])
    }

    @Test
    func aDuplicateWithinTheSurfaceIsRefusedBeforeItReachesTheRenderer() throws {
        var harness = try mount()
        defer { harness.close() }
        #expect(throws: StoreError.duplicateShortcut(nodeID: 3, existing: 2, shortcut: "primary+r")) {
            _ = try apply([.setProperty(id: 3, property: .shortcut, value: .string("primary+r"))], to: &harness)
        }
        #expect(harness.renderer.registry.handle(for: 3)?.shortcutBinding?.displayString == "⇧⌘T")
    }

    @Test
    func tabOrderSurvivesARemount() throws {
        var harness = try mount([
            .createNode(id: 1, nodeType: .surface),
            .createNode(id: 2, nodeType: .column, parentID: 1),
            .createNode(id: 3, nodeType: .textInput, parentID: 2, properties: [(.label, .string("First"))]),
            .createNode(id: 4, nodeType: .textInput, parentID: 2, properties: [(.label, .string("Second"))]),
        ])
        defer { harness.close() }
        let structural = try apply([
            .createNode(id: 5, nodeType: .text, parentID: 2, properties: [(.text, .string("Status"))]),
        ], to: &harness)
        #expect(structural.contains(where: { $0.isStructureAffecting }))
        let window = try #require(harness.renderer.registry.handle(for: 1)?.window)
        defer { window.orderOut(nil); window.close() }
        #expect(window.autorecalculatesKeyViewLoop)
        window.contentView?.layoutSubtreeIfNeeded()

        let first = try #require(harness.renderer.registry.view(for: 3) as? NSTextField)
        let second = try #require(harness.renderer.registry.view(for: 4) as? NSTextField)
        #expect(window.makeFirstResponder(first))
        let tab = try key("\t", [], keyCode: 48, in: window)
        window.sendEvent(tab)
        #expect((window.firstResponder as? NSTextView)?.delegate === second)
        #expect(harness.interactions.allSatisfy {
            if case .textEdit = $0 { return true }
            return false
        })
    }
}
