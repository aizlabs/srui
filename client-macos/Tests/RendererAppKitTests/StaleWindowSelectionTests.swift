//
// StaleWindowSelectionTests.swift
// RendererAppKitTests
//
// A session window that carries a client-owned status (PX-022-G01) takes no user selection change
// (PX-022-G02; §18, §22.5): while the session cannot report a selection, a click or an arrow key
// leaves the selection where it is, so no local selection outlives the reconnect unreported.
// Server-driven selection still applies, and the gate follows the status through remounts.
//

import AppKit
import SemanticModel
import Testing
@testable import RendererAppKit
@testable import Collections

@MainActor
@Suite("Stale session windows take no user selection change (PX-022-G02; §18, §22.5)")
struct StaleWindowSelectionTests {
    private static let surfaceID = NodeId(1)
    private static let collectionID = NodeId(2)
    private static let modelID = ModelId(7)
    private static let stale = "disconnected — showing data from revision 4 (stale)"

    @MainActor
    final class Harness {
        let renderer = LayoutRenderer()
        var store: SemanticStore
        var interactions: [SemanticInteraction] = []

        init(_ nodeType: TypeRef) throws {
            store = try StaleWindowSelectionTests.store(nodeType)
            renderer.controlFactory.onInteraction = { [weak self] in self?.interactions.append($0) }
            try renderer.mount(store: store)
        }

        var handle: RenderHandle { renderer.registry.handle(for: StaleWindowSelectionTests.collectionID)! }
        var collection: NSTableView { (handle.view as! NSScrollView).documentView as! NSTableView }
        var window: NSWindow { renderer.registry.handle(for: StaleWindowSelectionTests.surfaceID)!.window! }

        func commit(_ operations: [SemanticModel.Operation]) throws {
            var next = store
            for operation in operations {
                try operation.apply(to: &next)
            }
            try renderer.apply(
                transaction: Transaction(baseRevision: store.revision, operations: operations),
                newStore: next
            )
            store = next
        }

        /// The user's Down Arrow, delivered to the collection as a real key-down event.
        func pressDownArrow() throws {
            window.makeFirstResponder(collection)
            let down = String(Character(UnicodeScalar(UInt16(NSDownArrowFunctionKey))!))
            let event = try #require(NSEvent.keyEvent(
                with: .keyDown, location: .zero, modifierFlags: [.numericPad, .function], timestamp: 0,
                windowNumber: window.windowNumber, context: nil, characters: down,
                charactersIgnoringModifiers: down, isARepeat: false, keyCode: 125
            ))
            collection.keyDown(with: event)
        }

        /// Whether AppKit may apply a user's click, deselection or arrow key now.
        var acceptsUserSelectionChange: Bool {
            if let outline = collection as? NSOutlineView {
                return (handle.modelAdapter as! OutlineCollectionAdapter).selectionShouldChange(in: outline)
            }
            return (handle.modelAdapter as! TableCollectionAdapter).selectionShouldChange(in: collection)
        }

        func tearDown() {
            window.close()
        }
    }

    private static func row(_ i: UInt64) -> ModelItem {
        ModelItem(itemID: ItemId(i + 1), value: .list([.string("\(4100 + i)"), .string("process \(i)")]))
    }

    private static func store(_ nodeType: TypeRef) throws -> SemanticStore {
        var store = SemanticStore()
        try store.createNode(id: surfaceID, nodeType: .surface,
                             properties: [Property(property: .label, value: .string("Processes"))])
        try store.createModel(id: modelID, modelType: nodeType, itemCount: 0)
        try store.modelInsert(id: modelID, index: 0, items: (0..<10).map(row))
        var properties = [
            Property(property: .modelRef, value: .unsignedInt(modelID.value)),
            Property(property: .selectionMode, value: .enumToken(.selectionModeSingle)),
        ]
        if nodeType == .table {
            properties.append(Property(property: .columns, value: .list([.string("PID"), .string("Name")])))
        }
        try store.createNode(id: collectionID, nodeType: nodeType, parentID: surfaceID, properties: properties)
        return store
    }

    @Test("a stale Table or Tree keeps its selection against the user's arrow key, and moves once live",
          arguments: [TypeRef.table, TypeRef.tree])
    func staleCollectionsRefuseUserSelectionChanges(_ nodeType: TypeRef) throws {
        let harness = try Harness(nodeType)
        defer { harness.tearDown() }
        harness.collection.selectRowIndexes(IndexSet(integer: 2), byExtendingSelection: false)
        #expect(harness.interactions == [.selectionChanged(nodeID: Self.collectionID, itemID: ItemId(3))])
        harness.interactions.removeAll()
        #expect(harness.acceptsUserSelectionChange)

        harness.renderer.setSessionWindowStatus(Self.stale)
        #expect(!harness.acceptsUserSelectionChange, "no click, deselection or arrow key is applied")
        try harness.pressDownArrow()
        #expect(harness.collection.selectedRowIndexes == IndexSet(integer: 2), "the selection stays")
        #expect(harness.interactions.isEmpty, "nothing is reported, and nothing is kept unreported")

        harness.renderer.setSessionWindowStatus(nil)
        #expect(harness.acceptsUserSelectionChange)
        try harness.pressDownArrow()
        #expect(harness.collection.selectedRowIndexes == IndexSet(integer: 3))
        #expect(harness.interactions == [.selectionChanged(nodeID: Self.collectionID, itemID: ItemId(4))])
    }

    @Test("a server promotion still selects in a stale window", arguments: [TypeRef.table, TypeRef.tree])
    func promotionsStillApplyToAStaleWindow(_ nodeType: TypeRef) throws {
        let harness = try Harness(nodeType)
        defer { harness.tearDown() }
        harness.renderer.setSessionWindowStatus(Self.stale)
        try harness.commit([.setProperty(id: Self.collectionID, property: .value, value: .itemID(ItemId(6)))])
        #expect(harness.collection.selectedRowIndexes == IndexSet(integer: 5))
        #expect(harness.interactions.isEmpty, "a promotion is not a user change")
    }

    // AppKit's `deselectAll(nil)` asks `selectionShouldChange(in:)` even when called by code, so a
    // server-driven clear must still pass the gate (PX-022-G02 round 2).

    @Test("a promotion's withdrawal still clears in a stale window", arguments: [TypeRef.table, TypeRef.tree])
    func withdrawalsStillClearInAStaleWindow(_ nodeType: TypeRef) throws {
        let harness = try Harness(nodeType)
        defer { harness.tearDown() }
        try harness.commit([.setProperty(id: Self.collectionID, property: .value, value: .itemID(ItemId(3)))])
        #expect(harness.collection.selectedRowIndexes == IndexSet(integer: 2))
        harness.renderer.setSessionWindowStatus(Self.stale)
        try harness.commit([.clearProperty(id: Self.collectionID, property: .value)])
        #expect(harness.collection.selectedRowIndexes.isEmpty, "the withdrawn row is deselected")
        #expect(harness.interactions.isEmpty, "a withdrawal is not a user change")
    }

    @Test("a promotion to an item absent from the replica still clears in a stale window",
          arguments: [TypeRef.table, TypeRef.tree])
    func absentPromotionsStillClearInAStaleWindow(_ nodeType: TypeRef) throws {
        let harness = try Harness(nodeType)
        defer { harness.tearDown() }
        try harness.commit([.setProperty(id: Self.collectionID, property: .value, value: .itemID(ItemId(3)))])
        harness.renderer.setSessionWindowStatus(Self.stale)
        try harness.commit([.setProperty(id: Self.collectionID, property: .value, value: .itemID(ItemId(999)))])
        #expect(harness.collection.selectedRowIndexes.isEmpty, "no row stands in for an absent item (§8)")
        #expect(harness.interactions.isEmpty)
    }

    @Test("a selection_mode of none still clears a stale window's selection, and the user's deselect stays refused",
          arguments: [TypeRef.table, TypeRef.tree])
    func selectionModeNoneStillClearsAStaleWindow(_ nodeType: TypeRef) throws {
        let harness = try Harness(nodeType)
        defer { harness.tearDown() }
        harness.collection.selectRowIndexes(IndexSet(integer: 2), byExtendingSelection: false)
        harness.interactions.removeAll()
        harness.renderer.setSessionWindowStatus(Self.stale)
        harness.collection.deselectAll(harness.collection)
        #expect(harness.collection.selectedRowIndexes == IndexSet(integer: 2), "the user's deselect action is refused")
        try harness.commit([
            .setProperty(id: Self.collectionID, property: .selectionMode, value: .enumToken(.selectionModeNone)),
        ])
        #expect(harness.collection.selectedRowIndexes.isEmpty, "a collection that selects nothing shows nothing selected")
        #expect(harness.interactions.isEmpty)
    }

    @Test("the gate follows the status through a remount", arguments: [TypeRef.table, TypeRef.tree])
    func remountedCollectionsKeepTheGate(_ nodeType: TypeRef) throws {
        let harness = try Harness(nodeType)
        defer { harness.tearDown() }
        let before = harness.handle.modelAdapter
        harness.renderer.setSessionWindowStatus(Self.stale)
        try harness.renderer.mount(store: harness.store)
        #expect(harness.handle.modelAdapter !== before, "a remount builds a new adapter")
        #expect(!harness.acceptsUserSelectionChange)
        harness.renderer.setSessionWindowStatus("")
        #expect(harness.acceptsUserSelectionChange, "an empty status clears the gate as it clears the line")
    }
}
