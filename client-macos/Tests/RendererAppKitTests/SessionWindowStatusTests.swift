//
// SessionWindowStatusTests.swift
// RendererAppKitTests
//
// The client-owned status line on a session's Surface windows (PX-022-G01; §17, §18, §22.3,
// §22.8): it reaches every window, never touches the server-owned title, and survives remounts.
//

import AppKit
import SemanticModel
import Testing
@testable import RendererAppKit

@MainActor
@Suite("Session window status (PX-022-G01; §17, §18, §22.8)")
struct SessionWindowStatusTests {
    private let stale = "disconnected — showing data from revision 7 (stale)"

    @Test("setting, changing and clearing the status updates every Surface window")
    func statusReachesEverySurfaceWindow() throws {
        let store = try makeStore(twoSurfaces)
        let renderer = AppKitRenderer()
        try renderer.attach(store: store)
        let windows = try surfaceWindows(of: renderer, count: 2)
        #expect(windows.allSatisfy { $0.subtitle.isEmpty && $0.accessibilityHelp() == nil })

        renderer.setSessionWindowStatus(stale)
        #expect(renderer.sessionWindowStatus == stale)
        #expect(windows.allSatisfy { $0.subtitle == stale && $0.accessibilityHelp() == stale })

        renderer.setSessionWindowStatus("resynchronizing")
        #expect(windows.allSatisfy { $0.subtitle == "resynchronizing" })
        #expect(windows.allSatisfy { $0.accessibilityHelp() == "resynchronizing" })

        renderer.setSessionWindowStatus(nil)
        #expect(renderer.sessionWindowStatus == nil)
        #expect(windows.allSatisfy { $0.subtitle.isEmpty && $0.accessibilityHelp() == nil })

        renderer.setSessionWindowStatus(stale)
        renderer.setSessionWindowStatus("")
        #expect(renderer.sessionWindowStatus == nil)
        #expect(windows.allSatisfy { $0.subtitle.isEmpty && $0.accessibilityHelp() == nil })
    }

    @Test("the server title and the client status never overwrite each other")
    func serverTitleAndClientStatusAreIndependent() throws {
        let base: [SemanticModel.Operation] = [
            .createNode(id: 1, nodeType: .surface, properties: [(.label, .string("Server Title"))]),
            .createNode(id: 2, nodeType: .text, parentID: 1, properties: [(.text, .string("row"))]),
        ]
        let store = try makeStore(base)
        let renderer = AppKitRenderer()
        try renderer.attach(store: store)
        let window = try #require(renderer.registry.handle(for: 1)?.window)
        #expect(window.title == "Server Title")

        renderer.setSessionWindowStatus(stale)
        #expect(window.title == "Server Title")
        #expect(window.subtitle == stale)

        let rename = SemanticModel.Operation.setProperty(
            id: 1,
            property: .label,
            value: .string("Renamed by server")
        )
        let renamedStore = try makeStore(base + [rename])
        try renderer.apply(
            transaction: Transaction(baseRevision: store.revision, operations: [rename]),
            newStore: renamedStore
        )
        // A label change is applied in place, so this is the same window.
        #expect(renderer.registry.handle(for: 1)?.window === window)
        #expect(window.title == "Renamed by server")
        #expect(window.subtitle == stale)
        #expect(window.accessibilityHelp() == stale)

        let clear = SemanticModel.Operation.clearProperty(id: 1, property: .label)
        let clearedStore = try makeStore(base + [rename, clear])
        try renderer.apply(
            transaction: Transaction(baseRevision: renamedStore.revision, operations: [clear]),
            newStore: clearedStore
        )
        #expect(window.title == "SRUI")
        #expect(window.subtitle == stale)

        renderer.setSessionWindowStatus(nil)
        #expect(window.title == "SRUI")
        #expect(window.subtitle.isEmpty)
    }

    @Test("a status set before the first mount reaches the first windows")
    func statusSetBeforeMountReachesFirstWindows() throws {
        let renderer = AppKitRenderer()
        renderer.setSessionWindowStatus(stale)
        try renderer.attach(store: makeStore(twoSurfaces))
        let windows = try surfaceWindows(of: renderer, count: 2)
        #expect(windows.allSatisfy { $0.subtitle == stale && $0.accessibilityHelp() == stale })
    }

    @Test("a window a later commit creates, and every window a remount rebuilds, keep the status")
    func laterAndRemountedWindowsKeepTheStatus() throws {
        let base: [SemanticModel.Operation] = [
            .createNode(id: 1, nodeType: .surface, properties: [(.label, .string("Main"))]),
            .createNode(id: 2, nodeType: .text, parentID: 1),
        ]
        let store = try makeStore(base)
        let renderer = AppKitRenderer()
        try renderer.attach(store: store)
        let original = try #require(renderer.registry.handle(for: 1)?.window)
        renderer.setSessionWindowStatus(stale)

        // A structural commit remounts: every window is closed and rebuilt.
        let addSurface = SemanticModel.Operation.createNode(
            id: 3,
            nodeType: .surface,
            properties: [(.label, .string("Second"))]
        )
        let grownStore = try makeStore(base + [addSurface])
        let classifications = try renderer.apply(
            transaction: Transaction(baseRevision: store.revision, operations: [addSurface]),
            newStore: grownStore
        )
        let remounted = classifications.contains { $0.isStructureAffecting }
        #expect(remounted)
        let rebuilt = try #require(renderer.registry.handle(for: 1)?.window)
        let added = try #require(renderer.registry.handle(for: 3)?.window)
        #expect(rebuilt !== original, "a structural commit must rebuild the window")
        #expect(rebuilt.title == "Main" && rebuilt.subtitle == stale)
        #expect(added.title == "Second" && added.subtitle == stale)
        #expect(added.accessibilityHelp() == stale)

        // A remount of the whole tree (as on a catch-up snapshot) keeps it too.
        try renderer.attach(store: grownStore)
        let rebuiltAgain = try surfaceWindows(of: renderer, count: 2)
        #expect(rebuiltAgain.allSatisfy { $0 !== rebuilt && $0 !== added })
        #expect(rebuiltAgain.allSatisfy { $0.subtitle == stale && $0.accessibilityHelp() == stale })

        // Clearing reaches the rebuilt windows, and a later remount stays clear.
        renderer.setSessionWindowStatus(nil)
        #expect(rebuiltAgain.allSatisfy { $0.subtitle.isEmpty })
        try renderer.attach(store: grownStore)
        #expect(try surfaceWindows(of: renderer, count: 2).allSatisfy { $0.subtitle.isEmpty })
    }

    @Test("a real NSWindow shows the status beside the server title")
    func nativeWindowShowsTheStatus() throws {
        let store = try makeStore([
            .createNode(id: 1, nodeType: .surface, properties: [(.label, .string("Server Title"))]),
            .createNode(id: 2, nodeType: .text, parentID: 1, properties: [(.text, .string("row"))]),
        ])
        let renderer = AppKitRenderer()
        try renderer.attach(store: store)
        renderer.showWindows()
        let window = try #require(renderer.registry.handle(for: 1)?.window)
        defer { window.close() }
        renderer.setSessionWindowStatus(stale)
        window.displayIfNeeded()

        #expect(window.isVisible)
        #expect(window.title == "Server Title")
        #expect(window.subtitle == stale)
        #expect(window.accessibilityHelp() == stale)
        let axTitle = window.accessibilityTitle() ?? "<nil>"
        let os = ProcessInfo.processInfo.operatingSystemVersionString
        FileHandle.standardError.write(Data(
            "PX-022-G01 native evidence: NSWindow title=\"\(window.title)\" subtitle=\"\(window.subtitle)\" accessibilityHelp=\"\(window.accessibilityHelp() ?? "<nil>")\" accessibilityTitle=\"\(axTitle)\" visible=\(window.isVisible) macOS=\(os)\n".utf8
        ))
    }

    private var twoSurfaces: [SemanticModel.Operation] {
        [
            .createNode(id: 1, nodeType: .surface, properties: [(.label, .string("One"))]),
            .createNode(id: 2, nodeType: .text, parentID: 1),
            .createNode(id: 3, nodeType: .surface, properties: [(.label, .string("Two"))]),
        ]
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
