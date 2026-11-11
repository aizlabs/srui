//
// HierarchicalTreeTests.swift
// RendererAppKitTests
//
// PX-018-G01: native AppKit evidence for a hierarchical, model-backed Tree with local disclosure
// (§7.6, §8, §22.5, §22.7, §22.8). The model's index space is the depth-first preorder of the
// forest; item i's subtree is i+1 ... i+descendant_count. Disclosure is local: collapsing hides
// that span without a round trip, EXPANSION_CHANGED reports it, and uncached rows that stay
// visible keep loading through the ordinary asynchronous range requests.
//

import AppKit
import SemanticModel
import Testing
@testable import RendererAppKit
@testable import Collections

@MainActor
struct HierarchicalTreeTests {
    private static let modelID = ModelId(18)
    private static let treeID = NodeId(4)

    private struct Expansion: Equatable {
        let itemID: ItemId
        let expanded: Bool
    }

    /// Records what an adapter reports.
    @MainActor
    private final class Recorder {
        var expansions: [Expansion] = []
        var requests: [CollectionRangeRequest] = []
        var collapsed: [Set<ItemId>] = []

        func attach(to adapter: OutlineCollectionAdapter) {
            adapter.onExpansionChanged = { [weak self] _, itemID, expanded in
                self?.expansions.append(Expansion(itemID: itemID, expanded: expanded))
            }
            adapter.onRangeRequest = { [weak self] in self?.requests.append($0) }
            adapter.onCollapsedItemsChanged = { [weak self] _, _, set in self?.collapsed.append(set) }
        }
    }

    private static func item(
        _ id: UInt64,
        depth: UInt64,
        descendants: UInt64,
        value: Value? = nil
    ) -> ModelItem {
        ModelItem(
            itemID: ItemId(id),
            value: value ?? .string("item \(id)"),
            properties: [
                .treeDepth: .unsignedInt(depth),
                .descendantCount: .unsignedInt(descendants),
            ]
        )
    }

    // A ItemId 1 (index 0): roots A and F; A holds B (with C, D) and E; F holds G.
    //   0 A d0 dc4 · 1 B d1 dc2 · 2 C d2 · 3 D d2 · 4 E d1 · 5 F d0 dc1 · 6 G d1
    private static let a = ItemId(1), b = ItemId(2), c = ItemId(3), d = ItemId(4)
    private static let e = ItemId(5), f = ItemId(6), g = ItemId(7)
    private static var forest: [ModelItem] {
        [
            item(1, depth: 0, descendants: 4),
            item(2, depth: 1, descendants: 2),
            item(3, depth: 2, descendants: 0),
            item(4, depth: 2, descendants: 0),
            item(5, depth: 1, descendants: 0),
            item(6, depth: 0, descendants: 1),
            item(7, depth: 1, descendants: 0),
        ]
    }

    /// A store whose model went through the shape validation every transaction gets.
    private static func store(
        _ items: [ModelItem],
        itemCount: UInt64? = nil,
        startIndex: UInt64 = 0,
        limits: StoreLimits = StoreLimits()
    ) throws -> SemanticStore {
        var store = SemanticStore(limits: limits)
        try store.apply([
            .createModel(id: modelID, modelType: .tree, itemCount: 0),
            .modelResetRange(
                id: modelID,
                startIndex: startIndex,
                items: items,
                totalCount: itemCount ?? (startIndex + UInt64(items.count))
            ),
        ])
        return store
    }

    private static func outline(
        for store: SemanticStore,
        selectionMode: StandardSelectionMode = .none,
        recorder: Recorder
    ) throws -> (OutlineCollectionAdapter, NSOutlineView) {
        let model = try #require(store.getModel(modelID))
        let adapter = OutlineCollectionAdapter(
            nodeID: treeID,
            model: model,
            modelID: modelID,
            selectionMode: selectionMode
        )
        recorder.attach(to: adapter)
        let outlineView = NSOutlineView()
        TableCollectionAdapter.applySelectionMode(selectionMode, to: outlineView)
        adapter.setColumns(nil, outlineView: outlineView)
        outlineView.dataSource = adapter
        outlineView.delegate = adapter
        outlineView.reloadData()
        return (adapter, outlineView)
    }

    private static func rowIndices(_ adapter: OutlineCollectionAdapter, _ outlineView: NSOutlineView) -> [UInt64] {
        let count = adapter.outlineView(outlineView, numberOfChildrenOfItem: nil)
        return (0..<count).map { adapter.modelIndex(forRow: $0) }
    }

    // MARK: - Local disclosure

    @Test
    func localCollapseAndExpandSendNoRangeRequestAndReportExpansionOnce() throws {
        let recorder = Recorder()
        let (adapter, outlineView) = try Self.outline(for: Self.store(Self.forest), recorder: recorder)
        adapter.noteVisibleRange(start: 0, count: 7)
        #expect(recorder.requests.isEmpty, "a fully cached tree needs no range")
        #expect(adapter.isHierarchical)
        #expect(Self.rowIndices(adapter, outlineView) == [0, 1, 2, 3, 4, 5, 6], "items start expanded")
        #expect(adapter.outlineView(outlineView, isItemExpandable: adapter.outlineView(outlineView, child: 0, ofItem: nil)) == false)

        #expect(adapter.setExpanded(false, itemID: Self.a, outlineView: outlineView))
        #expect(Self.rowIndices(adapter, outlineView) == [0, 5, 6])
        #expect(adapter.hiddenIntervals == [1..<5])
        #expect(recorder.expansions == [Expansion(itemID: Self.a, expanded: false)])
        #expect(recorder.requests.isEmpty, "collapse is local")

        // Already collapsed: no second report.
        #expect(adapter.setExpanded(false, itemID: Self.a, outlineView: outlineView) == false)
        #expect(recorder.expansions.count == 1)

        #expect(adapter.setExpanded(true, itemID: Self.a, outlineView: outlineView))
        #expect(Self.rowIndices(adapter, outlineView) == [0, 1, 2, 3, 4, 5, 6])
        #expect(recorder.expansions == [
            Expansion(itemID: Self.a, expanded: false),
            Expansion(itemID: Self.a, expanded: true),
        ])
        #expect(recorder.requests.isEmpty, "expand is local")

        // A leaf has nothing to disclose and reports nothing.
        #expect(adapter.setExpanded(false, itemID: Self.c, outlineView: outlineView) == false)

        // Nested collapse survives its ancestor's expand; intervals merge.
        #expect(adapter.setExpanded(false, itemID: Self.b, outlineView: outlineView))
        #expect(adapter.setExpanded(false, itemID: Self.a, outlineView: outlineView))
        #expect(adapter.hiddenIntervals == [1..<5])
        #expect(Self.rowIndices(adapter, outlineView) == [0, 5, 6])
        #expect(adapter.setExpanded(true, itemID: Self.a, outlineView: outlineView))
        #expect(Self.rowIndices(adapter, outlineView) == [0, 1, 4, 5, 6])
        #expect(adapter.row(forIndex: 2) == nil)
        #expect(adapter.row(forIndex: 4) == 2)
        #expect(adapter.collapsedItemIDs == [Self.b])
        #expect(recorder.expansions.count == 5)
        #expect(recorder.requests.isEmpty)
    }

    @Test
    func disclosureButtonInTheReusedCellTogglesAndReportsOnce() throws {
        let recorder = Recorder()
        let (adapter, outlineView) = try Self.outline(for: Self.store(Self.forest), recorder: recorder)
        let column = try #require(outlineView.outlineTableColumn)
        let rowA = adapter.outlineView(outlineView, child: 0, ofItem: nil)
        let cell = try #require(
            adapter.outlineView(outlineView, viewFor: column, item: rowA) as? OutlineCollectionAdapter.TreeCellView
        )
        #expect(cell.disclosureButton.isHidden == false, "a cached item with descendants shows a disclosure")
        #expect(cell.disclosureButton.bezelStyle == .disclosure)
        #expect(cell.disclosureButton.state == .on)
        #expect(cell.label.stringValue == "item 1")

        let leaf = adapter.outlineView(outlineView, child: 2, ofItem: nil)
        let leafCell = try #require(
            adapter.outlineView(outlineView, viewFor: column, item: leaf) as? OutlineCollectionAdapter.TreeCellView
        )
        #expect(leafCell.disclosureButton.isHidden, "a leaf has no disclosure")

        // A click flips an on/off button's state before it sends the action. `sendAction` routes
        // through NSApp, which is nil until something creates the shared application; create it
        // here so the test does not depend on which test ran first (no run loop is serviced).
        _ = NSApplication.shared
        let button = cell.disclosureButton
        button.state = .off
        let action = try #require(button.action)
        #expect(action == #selector(OutlineCollectionAdapter.disclosureButtonClicked(_:)))
        #expect(button.target === adapter)
        #expect(button.sendAction(action, to: button.target))
        #expect(adapter.collapsedItemIDs == [Self.a])
        #expect(recorder.expansions == [Expansion(itemID: Self.a, expanded: false)])
        #expect(recorder.requests.isEmpty)
    }

    @Test
    func treeRendererOriginatesExpansionChangedInteraction() throws {
        let factory = ControlFactory()
        var interactions: [SemanticInteraction] = []
        factory.onInteraction = { interactions.append($0) }
        let store = try Self.store(Self.forest)
        let handle = try factory.makeHandle(
            for: Node(
                id: Self.treeID,
                nodeType: .tree,
                properties: [.modelRef: .unsignedInt(Self.modelID.value)]
            ),
            store: store
        )
        let adapter = try #require(handle.modelAdapter as? OutlineCollectionAdapter)
        let outlineView = try #require((handle.view as? NSScrollView)?.documentView as? NSOutlineView)
        #expect(adapter.setExpanded(false, itemID: Self.f, outlineView: outlineView))
        #expect(adapter.setExpanded(true, itemID: Self.f, outlineView: outlineView))
        #expect(interactions == [
            .expansionChanged(nodeID: Self.treeID, itemID: Self.f, expanded: false),
            .expansionChanged(nodeID: Self.treeID, itemID: Self.f, expanded: true),
        ])
    }

    // MARK: - Virtualization (§22.7)

    @Test
    func collapsedSubtreeKeepsLoadingVisibleUncachedRowsAsynchronously() throws {
        // R (index 0) spans 1 ... 99_000; only R and nine children are cached.
        let total: UInt64 = 100_000
        var cached = [Self.item(1, depth: 0, descendants: 99_000)]
        cached += (1...9).map { Self.item(UInt64(1 + $0), depth: 1, descendants: 0) }
        var store = try Self.store(cached, itemCount: total)
        let recorder = Recorder()
        let (adapter, outlineView) = try Self.outline(for: store, recorder: recorder)

        adapter.noteVisibleRange(start: 0, count: 20)
        #expect(recorder.requests.count == 1)
        #expect(recorder.requests.first?.startIndex == 10)

        #expect(adapter.setExpanded(false, itemID: ItemId(1), outlineView: outlineView))
        #expect(adapter.outlineView(outlineView, numberOfChildrenOfItem: nil) == 1_000)
        #expect(adapter.modelIndex(forRow: 1) == 99_001)

        // The rows now visible after the span are uncached: loading rows, not selectable, and one
        // request for their flat indices only.
        let placeholder = try #require(
            adapter.outlineView(outlineView, child: 1, ofItem: nil) as? OutlineCollectionAdapter.Item
        )
        #expect(placeholder.index == 99_001)
        #expect(placeholder.title == CollectionCells.loadingPlaceholder)
        #expect(adapter.outlineLevel(forRow: 1) == 0)
        #expect(recorder.requests.count == 2)
        let request = try #require(recorder.requests.last)
        #expect(request.startIndex == 99_001)
        #expect(request.startIndex + request.count <= total)
        for request in recorder.requests.dropFirst() {
            let range = request.startIndex..<(request.startIndex + request.count)
            #expect(range.overlaps(1..<99_001) == false, "no request names a hidden index")
        }

        // Asking again for the same window is covered by the in-flight request.
        adapter.noteVisibleRange(start: 0, count: 20)
        #expect(recorder.requests.count == 2)

        // The range arrives: the rows paint in place, and the collapse holds.
        try store.apply([
            .modelResetRange(
                id: Self.modelID,
                startIndex: 99_001,
                items: (0..<4).map { Self.item(1_000 + $0, depth: 0, descendants: 0) },
                totalCount: total
            ),
        ])
        adapter.update(model: store.getModel(Self.modelID), modelID: Self.modelID, outlineView: outlineView)
        #expect(adapter.collapsedItemIDs == [ItemId(1)])
        let arrived = try #require(
            adapter.outlineView(outlineView, child: 1, ofItem: nil) as? OutlineCollectionAdapter.Item
        )
        #expect(arrived.itemID == ItemId(1_000))
        #expect(recorder.expansions == [Expansion(itemID: ItemId(1), expanded: false)])
    }

    // MARK: - Promotion (§6.3)

    @Test
    func promotionRevealsThroughOnlyTheCollapsedAncestorsHidingIt() throws {
        let recorder = Recorder()
        let (adapter, outlineView) = try Self.outline(
            for: Self.store(Self.forest),
            selectionMode: .single,
            recorder: recorder
        )
        var notifications: [NSAccessibility.Notification] = []
        adapter.postAccessibilityNotification = { _, name in notifications.append(name) }
        for itemID in [Self.b, Self.a, Self.f] {
            #expect(adapter.setExpanded(false, itemID: itemID, outlineView: outlineView))
        }
        #expect(Self.rowIndices(adapter, outlineView) == [0, 5])
        recorder.expansions.removeAll()

        // A restore without reveal never undoes the user's collapse.
        adapter.applyPromotion(Self.d, reveal: false, outlineView: outlineView)
        #expect(adapter.collapsedItemIDs == [Self.a, Self.b, Self.f])
        #expect(outlineView.selectedRowIndexes.isEmpty)
        #expect(recorder.expansions.isEmpty)

        // A revealing promotion of D (index 3) opens A and B, whose spans contain it, not F.
        adapter.applyPromotion(Self.d, reveal: true, outlineView: outlineView)
        #expect(adapter.collapsedItemIDs == [Self.f])
        #expect(recorder.expansions == [
            Expansion(itemID: Self.a, expanded: true),
            Expansion(itemID: Self.b, expanded: true),
        ])
        #expect(Self.rowIndices(adapter, outlineView) == [0, 1, 2, 3, 4, 5])
        #expect(adapter.row(of: Self.d) == 3)
        #expect(outlineView.selectedRowIndexes == IndexSet(integer: 3))
        #expect(notifications == [.selectedRowsChanged])
        #expect(recorder.requests.isEmpty)
    }

    // MARK: - Model updates keep disclosure by ItemId (§22.5)

    @Test
    func modelUpdatePreservesCollapsedItemsByIdentityAndDropsLeavers() throws {
        var store = try Self.store(Self.forest)
        let recorder = Recorder()
        let (adapter, outlineView) = try Self.outline(for: store, recorder: recorder)
        #expect(adapter.setExpanded(false, itemID: Self.b, outlineView: outlineView))
        #expect(adapter.hiddenIntervals == [2..<4])

        // A new root before A shifts B to index 2: B stays collapsed and its span moves with it.
        try store.apply([.modelInsert(id: Self.modelID, index: 0, items: [Self.item(50, depth: 0, descendants: 0)])])
        adapter.update(model: store.getModel(Self.modelID), modelID: Self.modelID, outlineView: outlineView)
        #expect(adapter.collapsedItemIDs == [Self.b])
        #expect(adapter.hiddenIntervals == [3..<5])
        #expect(Self.rowIndices(adapter, outlineView) == [0, 1, 2, 5, 6, 7])
        #expect(adapter.row(of: Self.b) == 2)
        #expect(adapter.row(of: Self.c) == nil)

        // Deleting B with its subtree (and shrinking A) drops the entry.
        try store.apply([
            .modelDeleteItems(id: Self.modelID, itemIds: [Self.b, Self.c, Self.d]),
            .modelUpdate(id: Self.modelID, index: nil, items: [Self.item(1, depth: 0, descendants: 1)]),
        ])
        adapter.update(model: store.getModel(Self.modelID), modelID: Self.modelID, outlineView: outlineView)
        #expect(adapter.collapsedItemIDs.isEmpty)
        #expect(adapter.hiddenIntervals.isEmpty)
        #expect(Self.rowIndices(adapter, outlineView) == [0, 1, 2, 3, 4])
        #expect(recorder.collapsed.last == [])
        #expect(recorder.expansions == [Expansion(itemID: Self.b, expanded: false)], "dropping is not a user toggle")
    }

    @Test
    func evictionOfACollapsedItemDropsItsEntry() throws {
        // R (0) holds 1...5; S (6) is a leaf root; T (7) spans 8 ... 999.
        var items = [Self.item(1, depth: 0, descendants: 5)]
        items += (1...5).map { Self.item(UInt64(1 + $0), depth: 1, descendants: 0) }
        items += [Self.item(7, depth: 0, descendants: 0), Self.item(8, depth: 0, descendants: 992)]
        var store = try Self.store(
            items,
            itemCount: 1_000,
            limits: StoreLimits(maxCachedItemsPerModel: 8)
        )
        let recorder = Recorder()
        let (adapter, outlineView) = try Self.outline(for: store, recorder: recorder)
        #expect(adapter.setExpanded(false, itemID: ItemId(1), outlineView: outlineView))
        #expect(adapter.outlineView(outlineView, numberOfChildrenOfItem: nil) == 995)

        // A range far away evicts R; its collapse cannot outlive it.
        try store.apply([
            .modelResetRange(
                id: Self.modelID,
                startIndex: 900,
                items: (0..<8).map { Self.item(500 + $0, depth: 1, descendants: 0) },
                totalCount: 1_000
            ),
        ])
        let model = try #require(store.getModel(Self.modelID))
        #expect(model.containsItem(ItemId(1)) == false)
        adapter.update(model: model, modelID: Self.modelID, outlineView: outlineView)
        #expect(adapter.collapsedItemIDs.isEmpty)
        #expect(adapter.outlineView(outlineView, numberOfChildrenOfItem: nil) == 1_000)
    }

    @Test
    func collapsedItemsSurviveARemount() throws {
        let renderer = LayoutRenderer()
        var store = SemanticStore()
        try store.apply([
            .createNode(id: NodeId(1), nodeType: .surface),
            .createModel(id: Self.modelID, modelType: .tree, itemCount: 0),
            .modelInsert(id: Self.modelID, index: 0, items: Self.forest),
            .createNode(
                id: Self.treeID,
                nodeType: .tree,
                parentID: NodeId(1),
                properties: [Property(property: .modelRef, value: .unsignedInt(Self.modelID.value))]
            ),
        ])
        try renderer.mount(store: store)
        defer { renderer.registry.handle(for: NodeId(1))?.window?.close() }
        func outline() throws -> (OutlineCollectionAdapter, NSOutlineView) {
            let handle = try #require(renderer.registry.handle(for: Self.treeID))
            return (
                try #require(handle.modelAdapter as? OutlineCollectionAdapter),
                try #require((handle.view as? NSScrollView)?.documentView as? NSOutlineView)
            )
        }
        let (before, beforeView) = try outline()
        #expect(before.setExpanded(false, itemID: Self.f, outlineView: beforeView))

        // A structural commit elsewhere remounts every view.
        let operations: [SemanticModel.Operation] = [
            .createNode(id: NodeId(9), nodeType: .text, parentID: NodeId(1)),
        ]
        var next = store
        try next.apply(operations)
        try renderer.apply(transaction: Transaction(baseRevision: store.revision, operations: operations), newStore: next)
        store = next
        let (after, afterView) = try outline()
        #expect(after !== before, "the commit remounted the Tree")
        #expect(after.collapsedItemIDs == [Self.f])
        #expect(Self.rowIndices(after, afterView) == [0, 1, 2, 3, 4, 5])
    }

    // MARK: - Depth limit (§26)

    @Test
    func deepTreeAtTheLimitRendersOutlineLevelsAndPastTheLimitIsRefused() throws {
        let limit = defaultMaxModelTreeDepth
        #expect(limit == 256)
        func chain(_ count: Int) -> [ModelItem] {
            (0..<count).map { (level: Int) -> ModelItem in
                let depth = UInt64(level)
                let descendants = UInt64(count - 1 - level)
                return Self.item(depth + 1, depth: depth, descendants: descendants)
            }
        }
        let applier = TransactionApplier()
        let accepted = applier.apply(
            baseRevision: .initial,
            operations: [
                .createModel(id: Self.modelID, modelType: .tree, itemCount: 0),
                .modelInsert(id: Self.modelID, index: 0, items: chain(limit)),
            ]
        )
        guard case .success = accepted else {
            Issue.record("depth \(limit - 1) is inside the default limit: \(accepted)")
            return
        }
        let store = applier.currentSnapshot.store
        let recorder = Recorder()
        let (adapter, outlineView) = try Self.outline(for: store, recorder: recorder)
        #expect(adapter.outlineView(outlineView, numberOfChildrenOfItem: nil) == limit)
        #expect(adapter.outlineLevel(forRow: 0) == 0)
        #expect(adapter.outlineLevel(forRow: limit - 1) == limit - 1)

        let column = try #require(outlineView.outlineTableColumn)
        let deepest = adapter.outlineView(outlineView, child: limit - 1, ofItem: nil)
        let cell = try #require(
            adapter.outlineView(outlineView, viewFor: column, item: deepest) as? OutlineCollectionAdapter.TreeCellView
        )
        #expect(cell.level == limit - 1)
        #expect(cell.indentation == CGFloat(limit - 1) * outlineView.indentationPerLevel)
        #expect(cell.disclosureButton.isHidden)
        let root = adapter.outlineView(outlineView, child: 0, ofItem: nil)
        let rootCell = try #require(
            adapter.outlineView(outlineView, viewFor: column, item: root) as? OutlineCollectionAdapter.TreeCellView
        )
        #expect(rootCell.indentation == 0)
        #expect(rootCell.disclosureButton.isHidden == false)

        // One level deeper is refused by the store: nothing commits, the renderer keeps its rows.
        let refused = applier.apply(
            baseRevision: applier.currentSnapshot.revision,
            operations: [
                .modelResetRange(id: Self.modelID, startIndex: 0, items: chain(limit + 1), totalCount: UInt64(limit + 1)),
            ]
        )
        guard case .failure = refused else {
            Issue.record("depth \(limit) must exceed the default limit")
            return
        }
        #expect(applier.currentSnapshot.store.getModel(Self.modelID)?.itemCount == UInt64(limit))
        #expect(adapter.outlineView(outlineView, numberOfChildrenOfItem: nil) == limit)
        #expect(adapter.outlineLevel(forRow: limit - 1) == limit - 1)
    }

    // MARK: - Columns (§8)

    @Test
    func treeColumnsRenderWithHeaderOutlineColumnFirstAndTableCellProjection() throws {
        let factory = ControlFactory()
        let columns = ["Name", "PID", "CPU"]
        let rows: [Value] = [
            .list([.string("launchd"), .string("1"), .string("0.1")]),
            .list([.string("syslogd"), .string("80"), .string("0.0")]),
        ]
        let store = try Self.store([
            Self.item(1, depth: 0, descendants: 1, value: rows[0]),
            Self.item(2, depth: 1, descendants: 0, value: rows[1]),
        ])
        let handle = try factory.makeHandle(
            for: Node(
                id: Self.treeID,
                nodeType: .tree,
                properties: [
                    .modelRef: .unsignedInt(Self.modelID.value),
                    .columns: .list(columns.map(Value.string)),
                ]
            ),
            store: store
        )
        let adapter = try #require(handle.modelAdapter as? OutlineCollectionAdapter)
        let outlineView = try #require((handle.view as? NSScrollView)?.documentView as? NSOutlineView)
        #expect(outlineView.headerView != nil)
        #expect(outlineView.tableColumns.map(\.title) == columns)
        #expect(outlineView.tableColumns.map(\.identifier) == TableCollectionAdapter.columnIdentifiers(for: columns))
        #expect(outlineView.outlineTableColumn === outlineView.tableColumns.first)

        for (row, value) in rows.enumerated() {
            let item = adapter.outlineView(outlineView, child: row, ofItem: nil)
            let projected = CollectionCells.cells(from: value)
            let outlineCell = try #require(
                adapter.outlineView(outlineView, viewFor: outlineView.tableColumns[0], item: item)
                    as? OutlineCollectionAdapter.TreeCellView
            )
            #expect(outlineCell.label.stringValue == projected[0])
            #expect(outlineCell.level == row)
            for column in 1..<columns.count {
                let field = try #require(
                    adapter.outlineView(outlineView, viewFor: outlineView.tableColumns[column], item: item)
                        as? NSTextField
                )
                #expect(field.stringValue == projected[column])
            }
        }

        // `columns` changes in place; dropping them returns to the single header-less column.
        factory.apply(property: .columns, value: .list([.string("Name"), .string("CPU")]), to: handle, store: store)
        #expect(outlineView.tableColumns.map(\.title) == ["Name", "CPU"])
        let item = adapter.outlineView(outlineView, child: 0, ofItem: nil)
        let cpu = try #require(
            adapter.outlineView(outlineView, viewFor: outlineView.tableColumns[1], item: item) as? NSTextField
        )
        #expect(cpu.stringValue == "1", "cells project by declared position, like Table")
        factory.apply(property: .columns, value: nil, to: handle, store: store)
        #expect(outlineView.headerView == nil)
        #expect(outlineView.tableColumns.map(\.identifier) == [OutlineCollectionAdapter.treeColumnIdentifier])
        #expect(outlineView.outlineTableColumn === outlineView.tableColumns.first)
    }

    @Test
    func treeWithoutColumnsKeepsOneHeaderlessColumn() throws {
        let factory = ControlFactory()
        let handle = try factory.makeHandle(
            for: Node(
                id: Self.treeID,
                nodeType: .tree,
                properties: [.items: .list([.string("Branch 1"), .string("Branch 2")])]
            ),
            store: SemanticStore()
        )
        let outlineView = try #require((handle.view as? NSScrollView)?.documentView as? NSOutlineView)
        #expect(outlineView.headerView == nil)
        #expect(outlineView.tableColumns.count == 1)
        #expect(outlineView.outlineTableColumn?.identifier == OutlineCollectionAdapter.treeColumnIdentifier)
        #expect(outlineView.numberOfRows == 2)
    }

    // MARK: - Flat models are unchanged

    @Test
    func flatModelWithoutTreePropertiesStillRendersFlat() throws {
        var store = SemanticStore()
        try store.apply([
            .createModel(id: Self.modelID, modelType: .tree, itemCount: 0),
            .modelInsert(
                id: Self.modelID,
                index: 0,
                items: (0..<5).map { ModelItem(itemID: ItemId($0 + 1), value: .string("row \($0)")) }
            ),
        ])
        let recorder = Recorder()
        let (adapter, outlineView) = try Self.outline(for: store, recorder: recorder)
        #expect(adapter.isHierarchical == false)
        #expect(adapter.outlineView(outlineView, numberOfChildrenOfItem: nil) == 5)
        #expect(Self.rowIndices(adapter, outlineView) == [0, 1, 2, 3, 4])
        #expect(adapter.setExpanded(false, itemID: ItemId(1), outlineView: outlineView) == false)
        #expect(recorder.expansions.isEmpty)
        let column = try #require(outlineView.outlineTableColumn)
        for row in 0..<5 {
            #expect(adapter.outlineLevel(forRow: row) == 0)
            let item = adapter.outlineView(outlineView, child: row, ofItem: nil)
            let field = try #require(adapter.outlineView(outlineView, viewFor: column, item: item) as? NSTextField)
            #expect(field.stringValue == "row \(row)")
            #expect(field.identifier == OutlineCollectionAdapter.cellIdentifier)
        }
    }
}
