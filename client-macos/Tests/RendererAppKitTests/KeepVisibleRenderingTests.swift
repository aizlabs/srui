//
// KeepVisibleRenderingTests.swift
// RendererAppKitTests
//
// Native AppKit evidence for the server-hinted keep-in-view item, the standard
// `keep_visible` property (§4 inv. 13, §6.3, §8, §22.5, §22.7). Each test mounts a real NSTableView or
// NSOutlineView in an offscreen surface window through LayoutRenderer and drives it with
// committed transactions. The hint reveals rows; it must never select, deselect, notify or emit.
//

import AppKit
import SemanticModel
import Testing
@testable import RendererAppKit
@testable import Collections

@MainActor
struct KeepVisibleRenderingTests {
    private static let surfaceID = NodeId(1)
    private static let tableID = NodeId(2)
    private static let modelID = ModelId(7)

    @MainActor
    final class Harness {
        let renderer = LayoutRenderer()
        var store: SemanticStore
        var interactions: [SemanticInteraction] = []
        var rangeRequests: [CollectionRangeRequest] = []
        var notifications: [NSAccessibility.Notification] = []

        init(store: SemanticStore) throws {
            self.store = store
            renderer.controlFactory.onInteraction = { [weak self] in self?.interactions.append($0) }
            renderer.controlFactory.onCollectionRangeRequest = { [weak self] in
                self?.rangeRequests.append($0)
            }
            try renderer.mount(store: store)
            observeNotifications()
        }

        var handle: RenderHandle { renderer.registry.handle(for: tableID)! }
        var tableView: NSTableView { (handle.view as! NSScrollView).documentView as! NSTableView }
        var tableAdapter: TableCollectionAdapter { handle.modelAdapter as! TableCollectionAdapter }
        var visibleRows: Range<Int> {
            let rows = tableView.rows(in: tableView.visibleRect)
            return rows.location..<(rows.location + rows.length)
        }
        var selectedItems: [ItemId] {
            tableView.selectedRowIndexes.compactMap { row in
                if let adapter = handle.modelAdapter as? TableCollectionAdapter {
                    return adapter.itemID(at: row)
                }
                return ((tableView as? NSOutlineView)?.item(atRow: row) as? OutlineCollectionAdapter.Item)?.itemID
            }
        }

        func observeNotifications() {
            let sink: @MainActor (NSTableView, NSAccessibility.Notification) -> Void = { [weak self] _, name in
                self?.notifications.append(name)
            }
            if let adapter = handle.modelAdapter as? TableCollectionAdapter {
                adapter.postAccessibilityNotification = sink
            } else if let adapter = handle.modelAdapter as? OutlineCollectionAdapter {
                adapter.postAccessibilityNotification = sink
            }
        }

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
            observeNotifications()
        }

        /// A user click: AppKit delivers the selection change to the delegate.
        func click(row: Int) {
            tableView.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
        }

        /// A user's Down Arrow press delivered to the table as a real key-down event (§22.5).
        func pressDownArrow() throws {
            let window = try #require(tableView.window)
            window.makeFirstResponder(tableView)
            let down = String(Character(UnicodeScalar(UInt16(NSDownArrowFunctionKey))!))
            let event = try #require(NSEvent.keyEvent(
                with: .keyDown,
                location: .zero,
                modifierFlags: [.numericPad, .function],
                timestamp: 0,
                windowNumber: window.windowNumber,
                context: nil,
                characters: down,
                charactersIgnoringModifiers: down,
                isARepeat: false,
                keyCode: 125
            ))
            tableView.keyDown(with: event)
        }

        /// A local pan: the user scrolls `row` into view.
        func pan(to row: Int) {
            tableView.scrollRowToVisible(row)
        }

        func tearDown() {
            renderer.registry.handle(for: surfaceID)?.window?.close()
        }
    }

    /// Row `i` holds `ItemId(i + 1)`.
    private static func row(_ i: UInt64) -> ModelItem {
        ModelItem(itemID: ItemId(i + 1), value: .list([.string("\(i)"), .string("process \(i)")]))
    }

    private static func item(_ id: UInt64) -> ModelItem {
        ModelItem(itemID: ItemId(id), value: .list([.string("\(id)"), .string("moved \(id)")]))
    }

    /// A surface with a 10,000-row model-backed collection. `cached` limits the replica's cache.
    private static func store(
        rows: UInt64 = 10_000,
        cached: Range<UInt64>? = nil,
        nodeType: TypeRef = .table,
        keepVisible: ItemId? = nil
    ) throws -> SemanticStore {
        var store = SemanticStore()
        try store.createNode(
            id: surfaceID,
            nodeType: .surface,
            properties: [Property(property: .label, value: .string("Processes"))]
        )
        try store.createModel(id: modelID, modelType: nodeType, itemCount: 0)
        if let cached {
            try store.modelResetRange(
                id: modelID,
                startIndex: cached.lowerBound,
                items: cached.map(row),
                totalCount: rows
            )
        } else {
            try store.modelInsert(id: modelID, index: 0, items: (0..<rows).map(row))
        }
        var properties = [
            Property(property: .modelRef, value: .unsignedInt(modelID.value)),
            Property(property: .selectionMode, value: .enumToken(.selectionModeSingle)),
        ]
        if nodeType == .table {
            properties.append(Property(property: .columns, value: .list([.string("PID"), .string("Command")])))
        }
        if let keepVisible {
            properties.append(Property(property: .keepVisible, value: .itemID(keepVisible)))
        }
        try store.createNode(id: tableID, nodeType: nodeType, parentID: surfaceID, properties: properties)
        return store
    }

    /// What the SDK's `set_keep_visible` records: the item itself, then the hint.
    private static func hint(row index: UInt64, total: UInt64 = 10_000) -> [SemanticModel.Operation] {
        [
            .modelResetRange(id: modelID, startIndex: index, items: [row(index)], totalCount: total),
            .setProperty(id: tableID, property: .keepVisible, value: .itemID(ItemId(index + 1))),
        ]
    }

    /// A re-sort expressed as delete + insert of the same ItemId (§8).
    private static func move(_ id: UInt64, to index: UInt64) -> [SemanticModel.Operation] {
        [
            .modelDelete(id: modelID, index: nil, count: nil, itemIds: [ItemId(id)]),
            .modelInsert(id: modelID, index: index, items: [item(id)]),
        ]
    }

    // MARK: - Reveal without selection

    @Test
    func settingTheHintRevealsTheRowWithoutTouchingALocalSelection() throws {
        let harness = try Harness(store: Self.store())
        defer { harness.tearDown() }
        harness.click(row: 3)
        #expect(harness.interactions == [.selectionChanged(nodeID: Self.tableID, itemID: ItemId(4))])
        #expect(harness.visibleRows.contains(9_000) == false)
        let memory = harness.renderer.controlFactory.collectionSelectionMemory[Self.tableID]

        try harness.commit(Self.hint(row: 9_000))

        #expect(harness.visibleRows.contains(9_000))
        #expect(harness.tableView.selectedRowIndexes == IndexSet(integer: 3), "the local selection stays")
        #expect(harness.interactions.count == 1, "no SELECTION_CHANGED from the hint")
        #expect(harness.notifications.isEmpty, "no selection notification")
        #expect(harness.tableAdapter.promotedItemID == nil)
        #expect(harness.tableAdapter.pendingPromotion == nil)
        #expect(harness.tableAdapter.localSelectionSincePromotion, "promotion bookkeeping untouched")
        #expect(harness.renderer.controlFactory.collectionSelectionMemory[Self.tableID] == memory)
        #expect(harness.tableAdapter.keepVisible.shownRow == 9_000)
    }

    @Test
    func aCommitThatMovesTheItemRevealsItAgain() throws {
        let harness = try Harness(store: Self.store())
        defer { harness.tearDown() }
        try harness.commit(Self.hint(row: 9_000))
        harness.click(row: 9_001)
        harness.pan(to: 0)
        #expect(harness.visibleRows.contains(9_000) == false)

        // ItemId 9,001 moves to row 5,000 under a re-sort.
        try harness.commit(Self.move(9_001, to: 5_000))

        #expect(harness.visibleRows.contains(5_000))
        #expect(harness.selectedItems == [ItemId(9_002)], "the selection keeps its ItemId")
        #expect(harness.interactions.count == 1)
        #expect(harness.notifications.isEmpty)
        #expect(harness.tableAdapter.keepVisible.shownRow == 5_000)
    }

    @Test
    func aCommitThatDoesNotMoveTheItemLeavesALocalPanUntouched() throws {
        let harness = try Harness(store: Self.store())
        defer { harness.tearDown() }
        try harness.commit(Self.hint(row: 9_000))
        harness.pan(to: 200)
        let panned = harness.tableView.visibleRect
        #expect(harness.visibleRows.contains(9_000) == false)

        // Content of the hinted row and of others changes, a row after it is deleted, the
        // label changes: the hinted item stays at row 9,000.
        try harness.commit([
            .modelUpdate(id: Self.modelID, index: 9_000, items: [Self.item(9_001)]),
            .modelUpdate(id: Self.modelID, index: 10, items: [Self.item(11)]),
            .modelDelete(id: Self.modelID, index: nil, count: nil, itemIds: [ItemId(9_501)]),
            .setProperty(id: Self.tableID, property: .label, value: .string("All processes")),
        ])
        #expect(harness.tableView.visibleRect == panned, "no scroll without a move or a set")

        // A range reset that re-sends the item at the same row index is not a move either.
        try harness.commit([
            .modelResetRange(id: Self.modelID, startIndex: 9_000, items: [Self.item(9_001)], totalCount: 9_999),
        ])
        #expect(harness.tableView.visibleRect == panned)
        #expect(harness.tableView.selectedRowIndexes.isEmpty)
        #expect(harness.interactions.isEmpty)
    }

    @Test
    func settingTheSameItemAgainRevealsAgain() throws {
        let harness = try Harness(store: Self.store())
        defer { harness.tearDown() }
        try harness.commit(Self.hint(row: 9_000))
        harness.pan(to: 0)
        try harness.commit([
            .setProperty(id: Self.tableID, property: .keepVisible, value: .itemID(ItemId(9_001))),
        ])
        #expect(harness.visibleRows.contains(9_000))
        #expect(harness.tableView.selectedRowIndexes.isEmpty)
    }

    // MARK: - Rapid local selections (the PX-017 race this replaces)

    @Test
    func twentyRapidLocalSelectionsWhileTheItemKeepsMovingEndOnTheLastSelection() throws {
        let harness = try Harness(store: Self.store())
        defer { harness.tearDown() }
        // Follow-style start: the user's row 30 is also the hinted item, ItemId 31.
        try harness.commit(Self.hint(row: 30))
        harness.click(row: 30)
        harness.interactions.removeAll()

        var pressed: [ItemId] = []
        var changedByCommit = 0
        var backwardJumps = 0
        var reveals = 0
        for press in 0..<20 {
            try harness.pressDownArrow()
            let local = harness.selectedItems
            pressed.append(try #require(local.first))
            // Every press races a sample that moves the hinted item across the user's rows:
            // alternately to the top and well below the user's position.
            let target: UInt64 = press.isMultiple(of: 2) ? 0 : 120
            let before = harness.tableAdapter.keepVisible.shownRow
            try harness.commit(Self.move(31, to: target))
            if harness.tableAdapter.keepVisible.shownRow != before { reveals += 1 }
            let after = harness.selectedItems
            if after != local { changedByCommit += 1 }
            if let now = after.first, let index = pressed.firstIndex(of: now), index < pressed.count - 1 {
                backwardJumps += 1
            }
        }
        let final = harness.selectedItems
        print("PX-017-G01 rapid-selection measurement: presses=20 distinct=\(Set(pressed).count) "
            + "final=\(final.map(\.value)) last=\(pressed.last!.value) changedByCommit=\(changedByCommit) "
            + "backwardJumps=\(backwardJumps) reveals=\(reveals) notifications=\(harness.notifications.count) "
            + "selectionEvents=\(harness.interactions.count)")

        #expect(Set(pressed).count == 20, "each press selected a new item: \(pressed)")
        #expect(final == [pressed.last!], "ends on the last local selection")
        #expect(changedByCommit == 0)
        #expect(backwardJumps == 0)
        #expect(reveals == 20, "every move of the hinted item revealed it")
        #expect(harness.notifications.isEmpty)
        #expect(harness.interactions == pressed.map { .selectionChanged(nodeID: Self.tableID, itemID: $0) },
                "only the user's selections are reported")
        #expect(harness.store.getNode(Self.tableID)?.keepVisible == ItemId(31), "local input never clears it")
    }

    // MARK: - Uncached item, clearing, remount, Tree

    @Test
    func anUncachedItemIsRevealedWhenItArrivesAndNothingIsRequestedForIt() throws {
        let harness = try Harness(store: Self.store(cached: 0..<50))
        defer { harness.tearDown() }
        let origin = harness.tableView.visibleRect
        harness.rangeRequests.removeAll()

        try harness.commit([
            .setProperty(id: Self.tableID, property: .keepVisible, value: .itemID(ItemId(8_001))),
        ])
        #expect(harness.tableView.visibleRect == origin, "no row is guessed for an uncached item")
        #expect(harness.rangeRequests.isEmpty, "the hint requests nothing")
        #expect(harness.tableAdapter.keepVisible.itemID == ItemId(8_001))
        #expect(harness.tableAdapter.keepVisible.shownRow == nil)

        // The item arrives through range data and is revealed then.
        try harness.commit([
            .modelResetRange(id: Self.modelID, startIndex: 8_000, items: [Self.row(8_000)], totalCount: 10_000),
        ])
        #expect(harness.visibleRows.contains(8_000))
        #expect(harness.tableView.selectedRowIndexes.isEmpty)
        #expect(harness.notifications.isEmpty)
        #expect(harness.interactions.isEmpty)
    }

    @Test
    func clearingTheHintNeverMovesTheViewportOrTheSelection() throws {
        let harness = try Harness(store: Self.store())
        defer { harness.tearDown() }
        try harness.commit(Self.hint(row: 9_000))
        harness.click(row: 9_002)
        harness.pan(to: 100)
        let panned = harness.tableView.visibleRect

        try harness.commit([.clearProperty(id: Self.tableID, property: .keepVisible)])
        #expect(harness.tableView.visibleRect == panned)
        #expect(harness.selectedItems == [ItemId(9_003)])

        // Once cleared, moving the former item never scrolls.
        try harness.commit(Self.move(9_001, to: 5_000))
        #expect(harness.tableView.visibleRect == panned)
        #expect(harness.selectedItems == [ItemId(9_003)])
        #expect(harness.interactions.count == 1)
        #expect(harness.notifications.isEmpty)
        #expect(harness.tableAdapter.keepVisible == CollectionKeepVisible())
    }

    @Test
    func theHintNeverTouchesAPromotion() throws {
        let harness = try Harness(store: Self.store())
        defer { harness.tearDown() }
        try harness.commit([
            .modelResetRange(id: Self.modelID, startIndex: 40, items: [Self.row(40)], totalCount: 10_000),
            .setProperty(id: Self.tableID, property: .value, value: .itemID(ItemId(41))),
        ])
        #expect(harness.tableView.selectedRowIndexes == IndexSet(integer: 40))
        harness.notifications.removeAll()

        try harness.commit(Self.hint(row: 9_000))
        try harness.commit([.clearProperty(id: Self.tableID, property: .keepVisible)])
        #expect(harness.tableView.selectedRowIndexes == IndexSet(integer: 40))
        #expect(harness.tableAdapter.promotedItemID == ItemId(41))
        #expect(harness.tableAdapter.localSelectionSincePromotion == false)
        #expect(harness.notifications.isEmpty)
        #expect(harness.interactions.isEmpty)
    }

    @Test
    func aRemountRevealsTheHintedRowInTheFreshViewAndKeepsTheLocalSelection() throws {
        let harness = try Harness(store: Self.store())
        defer { harness.tearDown() }
        try harness.commit(Self.hint(row: 6_000))
        harness.click(row: 6_003)

        // A structural commit rebuilds the table; its viewport restarts at the origin.
        try harness.commit([.create(id: NodeId(3), nodeType: .text, parentID: Self.surfaceID)])
        #expect(harness.visibleRows.contains(6_000))
        #expect(harness.selectedItems == [ItemId(6_004)])
        #expect(harness.interactions.count == 1)
    }

    @Test
    func aFreshRendererRevealsAHintHeldInSemanticState() throws {
        let harness = try Harness(store: Self.store(keepVisible: ItemId(7_001)))
        defer { harness.tearDown() }
        #expect(harness.visibleRows.contains(7_000))
        #expect(harness.tableView.selectedRowIndexes.isEmpty)
    }

    @Test
    func treeHintRevealsTheOutlineRowWithoutSelecting() throws {
        let harness = try Harness(store: Self.store(rows: 2_000, nodeType: .tree))
        defer { harness.tearDown() }
        let outline = try #require(harness.tableView as? NSOutlineView)
        outline.selectRowIndexes(IndexSet(integer: 2), byExtendingSelection: false)
        let selected = outline.selectedRowIndexes
        try harness.commit(Self.hint(row: 1_500, total: 2_000))
        #expect(harness.visibleRows.contains(1_500))
        #expect(outline.selectedRowIndexes == selected)
        #expect(harness.notifications.isEmpty)
        #expect(harness.interactions.count == 1)

        // A move reveals the new row; an unmoved commit leaves a pan alone.
        harness.pan(to: 0)
        try harness.commit(Self.move(1_501, to: 900))
        #expect(harness.visibleRows.contains(900))
        harness.pan(to: 0)
        let panned = outline.visibleRect
        try harness.commit([.modelUpdate(id: Self.modelID, index: 900, items: [Self.item(1_501)])])
        #expect(outline.visibleRect == panned)
        #expect(outline.selectedRowIndexes == selected)
    }
}
