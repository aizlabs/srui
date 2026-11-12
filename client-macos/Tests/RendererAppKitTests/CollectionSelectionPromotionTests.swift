//
// CollectionSelectionPromotionTests.swift
// RendererAppKitTests
//
// Native AppKit evidence for a server-promoted collection selection (§6.3, §7.6, §8, §22.5,
// §22.7, §22.8). Each test mounts a real NSTableView/NSOutlineView in an offscreen surface
// window through LayoutRenderer and drives it with committed transactions.
//

import AppKit
import SemanticModel
import Testing
@testable import RendererAppKit
@testable import Collections

@MainActor
struct CollectionSelectionPromotionTests {
    private static let surfaceID = NodeId(1)
    private static let tableID = NodeId(2)
    private static let modelID = ModelId(7)

    /// Mounted renderer plus the bookkeeping each test inspects.
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
        var adapter: TableCollectionAdapter { handle.modelAdapter as! TableCollectionAdapter }
        var visibleRows: Range<Int> {
            let rows = tableView.rows(in: tableView.visibleRect)
            return rows.location..<(rows.location + rows.length)
        }

        /// Re-binds the notification sink after a remount replaced the adapter.
        func observeNotifications() {
            guard let adapter = handle.modelAdapter as? TableCollectionAdapter else { return }
            adapter.postAccessibilityNotification = { [weak self] _, name in
                self?.notifications.append(name)
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

        func tearDown() {
            renderer.registry.handle(for: surfaceID)?.window?.close()
        }
    }

    private static func row(_ i: UInt64) -> ModelItem {
        ModelItem(itemID: ItemId(i + 1), value: .list([.string("\(i)"), .string("process \(i)")]))
    }

    /// A surface with a 10,000-row model-backed table. `cached` limits the replica's cache.
    private static func store(
        rows: UInt64 = 10_000,
        cached: Range<UInt64>? = nil,
        mode: Value = .enumToken(.selectionModeSingle),
        nodeType: TypeRef = .table,
        promoted: ItemId? = nil
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
            Property(property: .selectionMode, value: mode),
        ]
        if nodeType == .table {
            properties.append(Property(property: .columns, value: .list([.string("PID"), .string("Command")])))
        }
        if let promoted {
            properties.append(Property(property: .value, value: .itemID(promoted)))
        }
        try store.createNode(id: tableID, nodeType: nodeType, parentID: surfaceID, properties: properties)
        return store
    }

    /// What the SDK's `promote_selection` records: the item itself, then the promotion.
    private static func promotion(of index: UInt64, total: UInt64 = 10_000) -> [SemanticModel.Operation] {
        [
            .modelResetRange(id: modelID, startIndex: index, items: [row(index)], totalCount: total),
            .setProperty(id: tableID, property: .value, value: .itemID(ItemId(index + 1))),
        ]
    }

    @Test
    func promotionBeyondTheViewportSelectsAndRevealsTheNativeRow() throws {
        let harness = try Harness(store: Self.store())
        defer { harness.tearDown() }
        #expect(harness.tableView.numberOfRows == 10_000)
        #expect(harness.visibleRows.contains(9_000) == false)

        try harness.commit(Self.promotion(of: 9_000))

        #expect(harness.tableView.selectedRowIndexes == IndexSet(integer: 9_000))
        #expect(harness.visibleRows.contains(9_000))
        #expect(harness.adapter.promotedItemID == ItemId(9_001))
        #expect(harness.interactions.isEmpty, "a promotion is not echoed as SELECTION_CHANGED")
        #expect(harness.notifications == [.selectedRowsChanged])
        let rowView = try #require(harness.tableView.rowView(atRow: 9_000, makeIfNecessary: true))
        #expect(rowView.isAccessibilitySelected())
    }

    @Test
    func unknownItemIsNeverMappedToARowIndex() throws {
        let harness = try Harness(store: Self.store(rows: 100))
        defer { harness.tearDown() }
        harness.click(row: 3)
        // ItemId(500) is absent; row 499 does not exist and row 4 holds ItemId(5).
        try harness.commit([.setProperty(id: Self.tableID, property: .value, value: .itemID(ItemId(500)))])

        #expect(harness.tableView.selectedRowIndexes.isEmpty, "the promotion replaced the local row")
        #expect(harness.adapter.pendingPromotion == CollectionPromotion(itemID: ItemId(500), reveal: true))
        #expect(harness.adapter.row(of: ItemId(500)) == nil)
    }

    @Test
    func targetOutsideTheCachedRangeArrivesThroughRangeMachineryAndIsRevealed() throws {
        // The replica caches rows 0..<50 of 10,000.
        let harness = try Harness(store: Self.store(cached: 0..<50))
        defer { harness.tearDown() }
        #expect(harness.adapter.row(of: ItemId(8_001)) == nil)
        harness.rangeRequests.removeAll()

        // A bare promotion of an uncached item waits; nothing is guessed.
        try harness.commit([.setProperty(id: Self.tableID, property: .value, value: .itemID(ItemId(8_001)))])
        #expect(harness.tableView.selectedRowIndexes.isEmpty)
        #expect(harness.adapter.pendingPromotion?.itemID == ItemId(8_001))

        // The item reaches the replica as an authoritative MODEL_RESET_RANGE (the SDK sends it
        // with the promotion; a range fulfilment is the same commit type) and is then revealed.
        try harness.commit([
            .modelResetRange(id: Self.modelID, startIndex: 8_000, items: [Self.row(8_000)], totalCount: 10_000),
        ])
        #expect(harness.tableView.selectedRowIndexes == IndexSet(integer: 8_000))
        #expect(harness.visibleRows.contains(8_000))
        #expect(harness.adapter.pendingPromotion == nil)

        // The revealed viewport's neighbours are requested through ordinary range requests. The
        // reveal scrolls the clip view, whose bounds-change notification is delivered
        // synchronously to the adapter's viewport observer; force layout so the visible rows are
        // final instead of servicing the main run loop (scripts/check-test-main-runloop-nesting.sh).
        harness.tableView.window?.layoutIfNeeded()
        let covered = harness.rangeRequests.contains { request in
            request.nodeID == Self.tableID && request.modelID == Self.modelID
                && (request.startIndex..<(request.startIndex + request.count)).overlaps(
                    UInt64(harness.visibleRows.lowerBound)..<UInt64(harness.visibleRows.upperBound)
                )
        }
        #expect(covered, "requests: \(harness.rangeRequests)")
    }

    @Test
    func sdkShapedPromotionOfAnUncachedItemRevealsInOneCommit() throws {
        let harness = try Harness(store: Self.store(cached: 0..<50))
        defer { harness.tearDown() }
        try harness.commit(Self.promotion(of: 7_500))
        #expect(harness.tableView.selectedRowIndexes == IndexSet(integer: 7_500))
        #expect(harness.visibleRows.contains(7_500))
    }

    @Test
    func localClickAfterPromotionStaysLocalAcrossUnrelatedCommits() throws {
        let harness = try Harness(store: Self.store())
        defer { harness.tearDown() }
        try harness.commit(Self.promotion(of: 9_000))
        harness.notifications.removeAll()

        harness.click(row: 9_002)
        #expect(harness.interactions == [.selectionChanged(nodeID: Self.tableID, itemID: ItemId(9_003))])
        let scrolledTo = harness.tableView.visibleRect

        // Unrelated commits: a row content update and a label change.
        try harness.commit([
            .modelUpdate(id: Self.modelID, index: 9_001, items: [
                ModelItem(itemID: ItemId(9_002), value: .list([.string("9001"), .string("renamed")])),
            ]),
            .setProperty(id: Self.tableID, property: .label, value: .string("All processes")),
        ])
        #expect(harness.tableView.selectedRowIndexes == IndexSet(integer: 9_002))
        #expect(harness.tableView.visibleRect == scrolledTo, "no reveal without a promotion change")
        #expect(harness.notifications.isEmpty)

        // A structural commit remounts the table; the local choice survives it too.
        try harness.commit([.create(id: NodeId(3), nodeType: .text, parentID: Self.surfaceID)])
        #expect(harness.tableView.selectedRowIndexes == IndexSet(integer: 9_002))
        #expect(harness.interactions.count == 1)
    }

    @Test
    func laterPromotionReplacesLocalSelectionAndRepromotionRevealsAgain() throws {
        let harness = try Harness(store: Self.store())
        defer { harness.tearDown() }
        try harness.commit(Self.promotion(of: 9_000))
        harness.click(row: 2)
        harness.tableView.scrollRowToVisible(0)
        #expect(harness.visibleRows.contains(9_000) == false)

        // Promoting the same ItemId again is a new promotion.
        try harness.commit(Self.promotion(of: 9_000))
        #expect(harness.tableView.selectedRowIndexes == IndexSet(integer: 9_000))
        #expect(harness.visibleRows.contains(9_000))

        try harness.commit(Self.promotion(of: 4_000))
        #expect(harness.tableView.selectedRowIndexes == IndexSet(integer: 4_000))
        #expect(harness.visibleRows.contains(4_000))
        #expect(harness.interactions == [.selectionChanged(nodeID: Self.tableID, itemID: ItemId(3))])
    }

    @Test
    func movingThePromotedItemUnderSortingKeepsItsItemIdAndDeletionClearsIt() throws {
        let harness = try Harness(store: Self.store(rows: 100))
        defer { harness.tearDown() }
        try harness.commit(Self.promotion(of: 40, total: 100))
        #expect(harness.tableView.selectedRowIndexes == IndexSet(integer: 40))

        // Re-sort: ItemId(41) moves to the top.
        try harness.commit([
            .modelDeleteItems(id: Self.modelID, itemIds: [ItemId(41)]),
            .modelInsert(id: Self.modelID, index: 0, items: [Self.row(40)]),
        ])
        #expect(harness.tableView.selectedRowIndexes == IndexSet(integer: 0))
        #expect(harness.adapter.itemID(at: 0) == ItemId(41))

        // Deletion, with the clear the SDK settles into the same commit.
        try harness.commit([
            .modelDeleteItems(id: Self.modelID, itemIds: [ItemId(41)]),
            .clearProperty(id: Self.tableID, property: .value),
        ])
        #expect(harness.tableView.selectedRowIndexes.isEmpty)
        #expect(harness.adapter.promotedItemID == nil)
    }

    @Test
    func selectionModeNoneIgnoresAPromotionAtTheAdapter() throws {
        var store = try Self.store(rows: 10)
        let model = try #require(store.getModel(Self.modelID))
        let adapter = TableCollectionAdapter(nodeID: Self.tableID, model: model, modelID: Self.modelID, selectionMode: .none)
        let tableView = NSTableView()
        tableView.dataSource = adapter
        tableView.delegate = adapter
        adapter.applyPromotion(ItemId(3), reveal: true, tableView: tableView)
        #expect(tableView.selectedRowIndexes.isEmpty)
        #expect(adapter.pendingPromotion == nil)

        // The replica refuses the semantic state outright.
        try store.setProperty(nodeID: Self.tableID, property: .selectionMode, value: .enumToken(.selectionModeNone))
        #expect(throws: StoreError.self) {
            try store.setProperty(nodeID: Self.tableID, property: .value, value: .itemID(ItemId(3)))
        }
    }

    @Test
    func resyncRestoresThePromotedSelectionFromSemanticState() throws {
        let promoted = try Self.store(promoted: ItemId(6_001))

        // A fresh renderer (reconnect) restores and reveals the promotion.
        let fresh = try Harness(store: promoted)
        defer { fresh.tearDown() }
        #expect(fresh.tableView.selectedRowIndexes == IndexSet(integer: 6_000))
        #expect(fresh.visibleRows.contains(6_000))

        // The same renderer remounting the same state (resync) restores without re-revealing.
        fresh.tableView.scrollRowToVisible(0)
        try fresh.renderer.mount(store: promoted)
        #expect(fresh.tableView.selectedRowIndexes == IndexSet(integer: 6_000))
        #expect(fresh.visibleRows.contains(6_000) == false, "an unchanged promotion is not revealed again")

        // A resync whose snapshot carries a changed promotion reveals the new one.
        var changed = promoted
        try changed.setProperty(nodeID: Self.tableID, property: .value, value: .itemID(ItemId(3_001)))
        try fresh.renderer.mount(store: changed)
        #expect(fresh.tableView.selectedRowIndexes == IndexSet(integer: 3_000))
        #expect(fresh.visibleRows.contains(3_000))
    }

    @Test
    func treePromotionSelectsAndRevealsTheOutlineRow() throws {
        let harness = try Harness(store: Self.store(rows: 2_000, nodeType: .tree))
        defer { harness.tearDown() }
        let outline = try #require((harness.handle.view as? NSScrollView)?.documentView as? NSOutlineView)
        let adapter = try #require(harness.handle.modelAdapter as? OutlineCollectionAdapter)
        var posted: [NSAccessibility.Notification] = []
        adapter.postAccessibilityNotification = { _, name in posted.append(name) }

        try harness.commit(Self.promotion(of: 1_500, total: 2_000))
        #expect(outline.selectedRowIndexes == IndexSet(integer: 1_500))
        let visible = outline.rows(in: outline.visibleRect)
        #expect((visible.location..<(visible.location + visible.length)).contains(1_500))
        #expect(posted == [.selectedRowsChanged])
        #expect(harness.interactions.isEmpty)
    }

    // MARK: - Round 2: withdrawal keeps a later local selection (§6.3, §22.5)

    private static let withdrawalKinds: [TypeRef] = [.table, .tree]

    /// Promotes ItemId 41 (row 40) in a 100-row Table or Tree.
    private static func promotedHarness(_ nodeType: TypeRef) throws -> Harness {
        let harness = try Harness(store: Self.store(rows: 100, nodeType: nodeType))
        try harness.commit(Self.promotion(of: 40, total: 100))
        #expect(harness.tableView.selectedRowIndexes == IndexSet(integer: 40))
        return harness
    }

    @Test
    func withdrawalByDeletionKeepsALaterLocalSelection() throws {
        for nodeType in Self.withdrawalKinds {
            let harness = try Self.promotedHarness(nodeType)
            defer { harness.tearDown() }
            harness.click(row: 42)
            #expect(harness.interactions == [.selectionChanged(nodeID: Self.tableID, itemID: ItemId(43))])

            // What the SDK commits when the promoted item is deleted.
            try harness.commit([
                .modelDeleteItems(id: Self.modelID, itemIds: [ItemId(41)]),
                .clearProperty(id: Self.tableID, property: .value),
            ])
            // ItemId 43 moved up to row 41 and is still the selection.
            #expect(harness.tableView.selectedRowIndexes == IndexSet(integer: 41), "\(nodeType)")
            #expect(harness.interactions.count == 1, "no SELECTION_CHANGED for the withdrawal")

            // The local choice also survives a later structural remount.
            try harness.commit([.create(id: NodeId(3), nodeType: .text, parentID: Self.surfaceID)])
            #expect(harness.tableView.selectedRowIndexes == IndexSet(integer: 41), "\(nodeType)")
        }
    }

    @Test
    func explicitWithdrawalKeepsALaterLocalSelection() throws {
        for nodeType in Self.withdrawalKinds {
            let harness = try Self.promotedHarness(nodeType)
            defer { harness.tearDown() }
            harness.click(row: 42)
            harness.notifications.removeAll()
            try harness.commit([.clearProperty(id: Self.tableID, property: .value)])
            #expect(harness.tableView.selectedRowIndexes == IndexSet(integer: 42), "\(nodeType)")
            #expect(harness.notifications.isEmpty, "the local selection is never touched")
            #expect(harness.interactions == [.selectionChanged(nodeID: Self.tableID, itemID: ItemId(43))])

            // Clicking back onto the promoted row is still a local choice and is kept too.
            let again = try Self.promotedHarness(nodeType)
            defer { again.tearDown() }
            again.click(row: 42)
            again.click(row: 40)
            try again.commit([.clearProperty(id: Self.tableID, property: .value)])
            #expect(again.tableView.selectedRowIndexes == IndexSet(integer: 40), "\(nodeType)")
        }
    }

    @Test
    func withdrawalWithoutALocalChangeDeselectsOnlyThePromotedRow() throws {
        for nodeType in Self.withdrawalKinds {
            // Explicit clear: the untouched promoted row is deselected.
            let cleared = try Self.promotedHarness(nodeType)
            defer { cleared.tearDown() }
            try cleared.commit([.clearProperty(id: Self.tableID, property: .value)])
            #expect(cleared.tableView.selectedRowIndexes.isEmpty, "\(nodeType)")
            #expect(cleared.interactions.isEmpty, "a withdrawal emits no SELECTION_CHANGED")

            // Deletion: the row vanishes with its item; nothing else becomes selected.
            let deleted = try Self.promotedHarness(nodeType)
            defer { deleted.tearDown() }
            try deleted.commit([
                .modelDeleteItems(id: Self.modelID, itemIds: [ItemId(41)]),
                .clearProperty(id: Self.tableID, property: .value),
            ])
            #expect(deleted.tableView.selectedRowIndexes.isEmpty, "\(nodeType)")
            #expect(deleted.interactions.isEmpty)
        }
    }

    @Test
    func adaptersKeepALaterLocalSelectionOnWithdrawal() throws {
        let store = try Self.store(rows: 10)
        let model = try #require(store.getModel(Self.modelID))

        let table = TableCollectionAdapter(nodeID: Self.tableID, model: model, modelID: Self.modelID, selectionMode: .single)
        let tableView = NSTableView()
        TableCollectionAdapter.reconcileColumns(in: tableView, columns: ["PID"], fallbackTitle: "Table")
        tableView.dataSource = table
        tableView.delegate = table
        tableView.reloadData()

        let outline = OutlineCollectionAdapter(nodeID: Self.tableID, model: model, modelID: Self.modelID, selectionMode: .single)
        let outlineView = NSOutlineView()
        outlineView.addTableColumn(NSTableColumn(identifier: NSUserInterfaceItemIdentifier("tree")))
        outlineView.dataSource = outline
        outlineView.delegate = outline
        outlineView.reloadData()

        // Promote ItemId 4 (row 3), the user picks row 5, then the promotion is withdrawn.
        table.applyPromotion(ItemId(4), reveal: false, tableView: tableView)
        tableView.selectRowIndexes(IndexSet(integer: 5), byExtendingSelection: false)
        #expect(table.localSelectionSincePromotion)
        table.applyPromotion(nil, reveal: false, tableView: tableView)
        #expect(tableView.selectedRowIndexes == IndexSet(integer: 5))

        outline.applyPromotion(ItemId(4), reveal: false, outlineView: outlineView)
        outlineView.selectRowIndexes(IndexSet(integer: 5), byExtendingSelection: false)
        #expect(outline.localSelectionSincePromotion)
        outline.applyPromotion(nil, reveal: false, outlineView: outlineView)
        #expect(outlineView.selectedRowIndexes == IndexSet(integer: 5))

        // Without a local change, only the untouched promoted row is deselected.
        table.applyPromotion(ItemId(4), reveal: false, tableView: tableView)
        #expect(table.localSelectionSincePromotion == false)
        table.applyPromotion(nil, reveal: false, tableView: tableView)
        #expect(tableView.selectedRowIndexes.isEmpty)
        outline.applyPromotion(ItemId(4), reveal: false, outlineView: outlineView)
        outline.applyPromotion(nil, reveal: false, outlineView: outlineView)
        #expect(outlineView.selectedRowIndexes.isEmpty)
    }
}
