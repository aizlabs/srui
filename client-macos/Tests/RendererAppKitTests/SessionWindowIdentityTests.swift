//
// SessionWindowIdentityTests.swift
// RendererAppKitTests
//
// The client-owned identity line on a session's Surface windows (PX-095; §6.3, §17): it reaches
// every window, survives remounts, leaves the server's title and the status subtitle alone, and
// gives each session's windows a tab group of their own.
//

import AppKit
import SemanticModel
import Testing
@testable import RendererAppKit

@MainActor
@Suite("Session window identity (PX-095; §6.3, §17)")
struct SessionWindowIdentityTests {
    private let hostA = SessionWindowIdentity(
        text: "alice@host-a:2228 · session aaaa",
        groupIdentifier: "org.srui.session.A"
    )
    private let hostB = SessionWindowIdentity(
        text: "alice@host-b:2229 · session bbbb",
        groupIdentifier: "org.srui.session.B"
    )

    @Test("the identity reaches every window, now and after a remount, and can be changed and removed")
    func identityReachesEveryWindowAndSurvivesRemounts() throws {
        let base: [SemanticModel.Operation] = [
            .createNode(id: 1, nodeType: .surface, properties: [(.label, .string("Main"))]),
            .createNode(id: 2, nodeType: .text, parentID: 1),
        ]
        let store = try makeStore(base)
        let renderer = AppKitRenderer()
        try renderer.attach(store: store)
        let original = try #require(renderer.registry.handle(for: 1)?.window)
        #expect(original.sessionIdentityText == nil, "no identity until the manager sets one")

        renderer.setSessionWindowIdentity(hostA)
        #expect(renderer.sessionWindowIdentity == hostA)
        #expect(original.sessionIdentityText == hostA.text)
        #expect(original.tabbingIdentifier == hostA.groupIdentifier)
        #expect(identityFields(of: original).count == 1)

        // A structural commit rebuilds the window and adds another: both carry it.
        let addSurface = SemanticModel.Operation.createNode(
            id: 3, nodeType: .surface, properties: [(.label, .string("Dialog"))]
        )
        let grown = try makeStore(base + [addSurface])
        try renderer.apply(
            transaction: Transaction(baseRevision: store.revision, operations: [addSurface]),
            newStore: grown
        )
        let rebuilt = try #require(renderer.registry.handle(for: 1)?.window)
        let added = try #require(renderer.registry.handle(for: 3)?.window)
        #expect(rebuilt !== original)
        for window in [rebuilt, added] {
            #expect(window.sessionIdentityText == hostA.text)
            #expect(window.tabbingIdentifier == hostA.groupIdentifier)
            #expect(identityFields(of: window).count == 1, "one line, never stacked")
        }

        // A new incarnation replaces the text in place; nothing accumulates.
        let replaced = SessionWindowIdentity(text: "alice@host-a:2228 · session cccc", groupIdentifier: hostA.groupIdentifier)
        renderer.setSessionWindowIdentity(replaced)
        #expect([rebuilt, added].allSatisfy { $0.sessionIdentityText == replaced.text && identityFields(of: $0).count == 1 })
        try renderer.attach(store: grown)
        #expect(try surfaceWindows(of: renderer, count: 2).allSatisfy { $0.sessionIdentityText == replaced.text })

        renderer.setSessionWindowIdentity(nil)
        #expect(renderer.sessionWindowIdentity == nil)
        #expect(try surfaceWindows(of: renderer, count: 2).allSatisfy { $0.sessionIdentityText == nil })
        try renderer.attach(store: grown)
        #expect(try surfaceWindows(of: renderer, count: 2).allSatisfy { $0.sessionIdentityText == nil })
    }

    @Test("the identity, the server's title and the status never overwrite one another")
    func identityTitleAndStatusAreIndependent() throws {
        let store = try makeStore([
            .createNode(id: 1, nodeType: .surface, properties: [(.label, .string("Server Title"))]),
        ])
        let renderer = AppKitRenderer()
        try renderer.attach(store: store)
        let window = try #require(renderer.registry.handle(for: 1)?.window)
        renderer.setSessionWindowIdentity(hostA)
        renderer.setSessionWindowStatus("disconnected — showing data from revision 3 (stale)")
        #expect(window.title == "Server Title")
        #expect(window.subtitle == "disconnected — showing data from revision 3 (stale)")
        #expect(window.sessionIdentityText == hostA.text)
        renderer.setSessionWindowStatus(nil)
        #expect(window.subtitle.isEmpty && window.sessionIdentityText == hostA.text)
        // A server relabel touches only the title.
        let relabel = SemanticModel.Operation.setProperty(id: 1, property: .label, value: .string("Renamed"))
        var relabeled = store
        try relabel.apply(to: &relabeled)
        try renderer.apply(transaction: Transaction(baseRevision: store.revision, operations: [relabel]), newStore: relabeled)
        #expect(window.title == "Renamed" && window.sessionIdentityText == hostA.text)
    }

    @Test("a server title or text that copies another host's identity line cannot replace or join it")
    func serverTextCannotStandInForTheIdentity() throws {
        let decoy = hostB.text
        let store = try makeStore([
            .createNode(id: 1, nodeType: .surface, properties: [(.label, .string(decoy))]),
            .createNode(id: 2, nodeType: .text, parentID: 1, properties: [(.text, .string(decoy))]),
        ])
        let renderer = AppKitRenderer()
        try renderer.attach(store: store)
        renderer.setSessionWindowIdentity(hostA)
        let window = try #require(renderer.registry.handle(for: 1)?.window)
        #expect(window.title == decoy, "the server owns the title, whatever it says")
        #expect(window.subtitle.isEmpty, "the subtitle is the client's status, never a server property")
        // The identity is the one line in the titlebar accessory, which no server node is mounted into.
        #expect(window.sessionIdentityText == hostA.text)
        #expect(identityFields(of: window).map(\.stringValue) == [hostA.text])
        let content = try #require(window.contentView)
        func identified(_ view: NSView) -> [NSView] {
            (view.identifier == SessionWindowIdentity.accessoryIdentifier ? [view] : []) + view.subviews.flatMap(identified)
        }
        #expect(identified(content).isEmpty, "nothing in the server's content carries the identity's identifier")
        let field = try #require(identityFields(of: window).first)
        #expect(!field.isDescendant(of: content), "the line is drawn outside the server's content view")
    }

    @Test("two renderers with the same node IDs and title keep their own identity and tab group")
    func twoSessionsWithIdenticalTreesAreToldApart() throws {
        let operations: [SemanticModel.Operation] = [
            .createNode(id: 1, nodeType: .surface, properties: [(.label, .string("Process Explorer"))]),
            .createNode(id: 2, nodeType: .text, parentID: 1, properties: [(.text, .string("PID 4101 · worker"))]),
        ]
        let a = AppKitRenderer(), b = AppKitRenderer()
        try a.attach(store: try makeStore(operations))
        try b.attach(store: try makeStore(operations))
        a.setSessionWindowIdentity(hostA)
        b.setSessionWindowIdentity(hostB)
        let windowA = try #require(a.registry.handle(for: 1)?.window)
        let windowB = try #require(b.registry.handle(for: 1)?.window)
        #expect(windowA !== windowB)
        #expect(windowA.title == windowB.title, "the server titles are the same")
        #expect(windowA.sessionIdentityText == hostA.text && windowB.sessionIdentityText == hostB.text)
        #expect(windowA.tabbingIdentifier != windowB.tabbingIdentifier)
        b.setSessionWindowIdentity(nil)
        #expect(windowA.sessionIdentityText == hostA.text, "one session's change leaves the other alone")
    }

    @Test("a real NSWindow shows the identity line below the server title")
    func nativeWindowShowsTheIdentity() throws {
        let store = try makeStore([
            .createNode(id: 1, nodeType: .surface, properties: [(.label, .string("Server Title"))]),
            .createNode(id: 2, nodeType: .text, parentID: 1, properties: [(.text, .string("row"))]),
        ])
        let renderer = AppKitRenderer()
        try renderer.attach(store: store)
        renderer.setSessionWindowIdentity(hostA)
        renderer.showWindows()
        let window = try #require(renderer.registry.handle(for: 1)?.window)
        defer { window.close() }
        window.displayIfNeeded()
        let field = try #require(identityFields(of: window).first)
        #expect(window.isVisible)
        #expect(field.stringValue == hostA.text && field.toolTip == hostA.text)
        #expect(field.window === window, "the line is in the window it names")
        #expect(field.isAccessibilityElement() && field.accessibilityValue() as? String == hostA.text)
        FileHandle.standardError.write(Data(
            "PX-095 native evidence: NSWindow title=\"\(window.title)\" identity=\"\(field.stringValue)\" tabbingIdentifier=\"\(window.tabbingIdentifier)\" visible=\(window.isVisible)\n".utf8
        ))
    }

    private func identityFields(of window: NSWindow) -> [NSTextField] {
        window.titlebarAccessoryViewControllers.flatMap { controller in
            controller.view.subviews.compactMap { $0 as? NSTextField }
                .filter { $0.identifier == SessionWindowIdentity.accessoryIdentifier }
        }
    }

    private func surfaceWindows(of renderer: AppKitRenderer, count: Int) throws -> [NSWindow] {
        let windows = renderer.registry.surfaceHandles.compactMap(\.window)
        try #require(windows.count == count)
        return windows
    }

    private func makeStore(_ operations: [SemanticModel.Operation]) throws -> SemanticStore {
        var store = SemanticStore()
        for operation in operations {
            try operation.apply(to: &store)
        }
        return store
    }
}
