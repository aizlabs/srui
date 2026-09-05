import AppKit
import SemanticModel
import Testing
@testable import RendererAppKit
@testable import Collections

@MainActor
struct CollectionAdaptersTests {

    @Test
    func tableAdapterInitialStateAndRowCount() {
        let adapter = TableCollectionAdapter(
            nodeID: 1,
            rows: [
                TableCollectionAdapter.TableRow(itemID: ItemId(1), cells: ["A", "B"]),
                TableCollectionAdapter.TableRow(itemID: ItemId(2), cells: ["C", "D"]),
            ]
        )

        let tableView = NSTableView()
        tableView.dataSource = adapter
        tableView.delegate = adapter

        #expect(adapter.numberOfRows(in: tableView) == 2)
        #expect(adapter.rows.count == 2)
    }

    @Test
    func tableAdapterResolvesCellsByColumnIdentifier() throws {
        let adapter = TableCollectionAdapter(
            nodeID: 10,
            rows: [
                TableCollectionAdapter.TableRow(itemID: ItemId(1), cells: ["Col0-Val", "Col1-Val", "Col2-Val"]),
            ],
            hasExplicitColumns: true
        )

        let tableView = NSTableView()
        let col0 = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("srui.column.0"))
        let col1 = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("srui.column.1"))
        let col2 = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("srui.column.2"))
        let colOut = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("srui.column.99"))
        let nonSruiCol = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("custom.column"))

        tableView.addTableColumn(col0)
        tableView.addTableColumn(col1)
        tableView.addTableColumn(col2)
        tableView.addTableColumn(colOut)
        tableView.addTableColumn(nonSruiCol)
        tableView.dataSource = adapter
        tableView.delegate = adapter

        let cell0 = try #require(adapter.tableView(tableView, viewFor: col0, row: 0) as? NSTextField)
        #expect(cell0.stringValue == "Col0-Val")
        #expect(cell0.isEditable == false)
        #expect(cell0.isBezeled == false)
        #expect(cell0.drawsBackground == false)

        let cell1 = try #require(adapter.tableView(tableView, viewFor: col1, row: 0) as? NSTextField)
        #expect(cell1.stringValue == "Col1-Val")

        let cell2 = try #require(adapter.tableView(tableView, viewFor: col2, row: 0) as? NSTextField)
        #expect(cell2.stringValue == "Col2-Val")

        // Out of bounds column index returns empty string
        let cellOut = try #require(adapter.tableView(tableView, viewFor: colOut, row: 0) as? NSTextField)
        #expect(cellOut.stringValue == "")

        // Non-srui column at index 4 exceeds cells.count so returns empty string
        let nonSruiCell = try #require(adapter.tableView(tableView, viewFor: nonSruiCol, row: 0) as? NSTextField)
        #expect(nonSruiCell.stringValue == "")
    }

    @Test
    func tableAdapterFallbackColumnRendersPrimaryCellWhenNotExplicit() throws {
        let adapter = TableCollectionAdapter(
            nodeID: 20,
            rows: [
                TableCollectionAdapter.TableRow(itemID: ItemId(1), cells: ["Alpha", "Beta", "Gamma"]),
            ],
            hasExplicitColumns: false
        )

        let tableView = NSTableView()
        let fallbackCol = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("fallback"))
        tableView.addTableColumn(fallbackCol)
        tableView.dataSource = adapter
        tableView.delegate = adapter

        let cell = try #require(adapter.tableView(tableView, viewFor: fallbackCol, row: 0) as? NSTextField)
        #expect(cell.stringValue == "Alpha")
    }

    @Test
    func tableAdapterSelectionModesBehavior() throws {
        var selections: [(NodeId, ItemId)] = []
        let adapter = TableCollectionAdapter(
            nodeID: 30,
            rows: [
                TableCollectionAdapter.TableRow(itemID: ItemId(100), cells: ["Item 100"]),
                TableCollectionAdapter.TableRow(itemID: ItemId(200), cells: ["Item 200"]),
                TableCollectionAdapter.TableRow(itemID: nil, cells: ["Inline Item (No ID)"]),
            ],
            selectionMode: .none,
            onSelectionChanged: { nodeID, itemID in selections.append((nodeID, itemID)) }
        )

        let tableView = NSTableView()
        tableView.dataSource = adapter
        tableView.delegate = adapter

        // 1. None selection mode
        #expect(adapter.tableView(tableView, shouldSelectRow: 0) == false)
        #expect(adapter.tableView(tableView, shouldSelectRow: 1) == false)
        adapter.tableViewSelectionDidChange(Notification(name: NSTableView.selectionDidChangeNotification, object: tableView))
        #expect(selections.isEmpty)

        // 2. Single selection mode
        adapter.selectionMode = .single
        #expect(adapter.tableView(tableView, shouldSelectRow: 0) == true)

        tableView.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
        #expect(selections.map(\.1) == [ItemId(100)])
        selections.removeAll()

        tableView.selectRowIndexes(IndexSet(integer: 1), byExtendingSelection: false)
        #expect(selections.map(\.1) == [ItemId(200)])
        selections.removeAll()

        // Selecting row with nil itemID (inline item) emits no interaction
        tableView.selectRowIndexes(IndexSet(integer: 2), byExtendingSelection: false)
        #expect(selections.isEmpty)

        // Deselection (no row selected) emits no interaction
        tableView.deselectAll(nil)
        #expect(selections.isEmpty)

        // 3. Multiple selection mode emits one event per selected item ID
        adapter.selectionMode = .multiple
        tableView.selectRowIndexes(IndexSet([0, 1]), byExtendingSelection: false)
        #expect(selections.map(\.1) == [ItemId(100), ItemId(200)])
    }

    @Test
    func tableAdapterPreservesSelectionAcrossComplexUpdates() throws {
        var selections: [(NodeId, ItemId)] = []
        let adapter = TableCollectionAdapter(
            nodeID: 40,
            rows: [
                TableCollectionAdapter.TableRow(itemID: ItemId(1), cells: ["Row 1"]),
                TableCollectionAdapter.TableRow(itemID: ItemId(2), cells: ["Row 2"]),
                TableCollectionAdapter.TableRow(itemID: ItemId(3), cells: ["Row 3"]),
                TableCollectionAdapter.TableRow(itemID: ItemId(4), cells: ["Row 4"]),
            ],
            selectionMode: .single,
            onSelectionChanged: { nodeID, itemID in selections.append((nodeID, itemID)) }
        )

        let tableView = NSTableView()
        tableView.dataSource = adapter
        tableView.delegate = adapter

        // Select row index 2 (ItemId 3)
        tableView.selectRowIndexes(IndexSet(integer: 2), byExtendingSelection: false)
        #expect(tableView.selectedRow == 2)
        #expect(selections.map(\.1) == [ItemId(3)])
        selections.removeAll()

        // Update rows: ItemId 3 moved to index 0, ItemId 2 deleted, ItemId 5 inserted
        let updatedRows = [
            TableCollectionAdapter.TableRow(itemID: ItemId(3), cells: ["Row 3 (Moved to Top)"]),
            TableCollectionAdapter.TableRow(itemID: ItemId(5), cells: ["Row 5 (New)"]),
            TableCollectionAdapter.TableRow(itemID: ItemId(1), cells: ["Row 1"]),
            TableCollectionAdapter.TableRow(itemID: ItemId(4), cells: ["Row 4"]),
        ]
        adapter.update(rows: updatedRows, tableView: tableView)

        // Selection should follow ItemId 3 to index 0
        #expect(tableView.selectedRow == 0)
        // No spurious interaction event fired during reload / selection restore
        #expect(selections.isEmpty)

        // Update rows where the selected item (ItemId 3) is removed
        let withoutItem3 = [
            TableCollectionAdapter.TableRow(itemID: ItemId(5), cells: ["Row 5"]),
            TableCollectionAdapter.TableRow(itemID: ItemId(1), cells: ["Row 1"]),
            TableCollectionAdapter.TableRow(itemID: ItemId(4), cells: ["Row 4"]),
        ]
        adapter.update(rows: withoutItem3, tableView: tableView)

        // Selection should be empty
        #expect(tableView.selectedRow == -1)
        #expect(selections.isEmpty)
    }

    @Test
    func outlineAdapterTreeHierarchyAndValues() throws {
        let adapter = OutlineCollectionAdapter(rows: ["Branch 1", "Branch 2", "Branch 3"])
        let outlineView = NSOutlineView()
        outlineView.dataSource = adapter
        outlineView.delegate = adapter

        #expect(adapter.outlineView(outlineView, numberOfChildrenOfItem: nil) == 3)
        #expect(adapter.outlineView(outlineView, isItemExpandable: "Branch 1") == false)

        let child0 = adapter.outlineView(outlineView, child: 0, ofItem: nil) as? OutlineCollectionAdapter.Item
        #expect(child0?.title == "Branch 1")

        let child2 = adapter.outlineView(outlineView, child: 2, ofItem: nil) as? OutlineCollectionAdapter.Item
        #expect(child2?.title == "Branch 3")

        let col = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("tree"))
        outlineView.addTableColumn(col)
        outlineView.outlineTableColumn = col

        let item1 = adapter.items[0]
        let cell = try #require(adapter.outlineView(outlineView, viewFor: col, item: item1) as? NSTextField)
        #expect(cell.stringValue == "Branch 1")
        #expect(cell.isEditable == false)

        // Update outline rows
        adapter.update(rows: ["Single Root"], outlineView: outlineView)
        #expect(adapter.items.map(\.title) == ["Single Root"])
        #expect(adapter.outlineView(outlineView, numberOfChildrenOfItem: nil) == 1)
    }

    @Test
    func tableAdapterReusesCellIdentifierFromColumn() throws {
        let adapter = TableCollectionAdapter(
            nodeID: 50,
            rows: [TableCollectionAdapter.TableRow(itemID: ItemId(1), cells: ["Alpha", "Beta"])]
        )
        let tableView = NSTableView()
        let col0 = NSTableColumn(identifier: TableCollectionAdapter.columnIdentifier(for: "Service"))
        col0.title = "Service"
        tableView.addTableColumn(col0)
        tableView.dataSource = adapter
        tableView.delegate = adapter

        let cell = try #require(adapter.tableView(tableView, viewFor: col0, row: 0) as? NSTextField)
        #expect(cell.identifier == col0.identifier)
        #expect(cell.stringValue == "Alpha")
        #expect(cell.isAccessibilityElement() == false)
    }

    @Test
    func tableAdapterReloadsOnlyChangedRowsWhenIdentitiesMatch() {
        let adapter = TableCollectionAdapter(
            nodeID: 51,
            rows: [
                TableCollectionAdapter.TableRow(itemID: ItemId(1), cells: ["A"]),
                TableCollectionAdapter.TableRow(itemID: ItemId(2), cells: ["B"]),
            ]
        )
        let tableView = NSTableView()
        tableView.addTableColumn(NSTableColumn(identifier: TableCollectionAdapter.primaryColumnIdentifier))
        tableView.dataSource = adapter
        tableView.delegate = adapter

        adapter.update(
            rows: [
                TableCollectionAdapter.TableRow(itemID: ItemId(1), cells: ["A"]),
                TableCollectionAdapter.TableRow(itemID: ItemId(2), cells: ["B-updated"]),
            ],
            tableView: tableView
        )
        #expect(adapter.lastRowUpdate == .contentReload(IndexSet(integer: 1)))
        #expect(adapter.rows[1].cells == ["B-updated"])

        adapter.update(
            rows: [
                TableCollectionAdapter.TableRow(itemID: ItemId(2), cells: ["B-updated"]),
                TableCollectionAdapter.TableRow(itemID: ItemId(3), cells: ["C"]),
            ],
            tableView: tableView
        )
        #expect(adapter.lastRowUpdate == .fullReload)
    }

    @Test
    func tableAdapterExposesRowAccessibilityLabelAndSelectedState() throws {
        let adapter = TableCollectionAdapter(
            nodeID: 52,
            rows: [
                TableCollectionAdapter.TableRow(itemID: ItemId(1), cells: ["Alpha", "100", "Active"]),
            ],
            selectionMode: .single
        )
        let tableView = NSTableView()
        tableView.dataSource = adapter
        tableView.delegate = adapter

        let rowView = try #require(adapter.tableView(tableView, rowViewForRow: 0))
        adapter.tableView(tableView, didAdd: rowView, forRow: 0)
        #expect(rowView.accessibilityLabel() == "Alpha, 100, Active")
        #expect(rowView.isAccessibilitySelected() == false)

        tableView.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
        adapter.tableView(tableView, didAdd: rowView, forRow: 0)
        #expect(rowView.isAccessibilitySelected() == true)
    }

    @Test
    func outlineAdapterReusesItemIdentityWhenTitlesMatch() {
        let adapter = OutlineCollectionAdapter(rows: ["Branch 1", "Branch 2"])
        let outlineView = NSOutlineView()
        outlineView.dataSource = adapter
        outlineView.delegate = adapter
        let first = adapter.items[0]
        let second = adapter.items[1]

        adapter.update(rows: ["Branch 2", "Branch 1", "Branch 3"], outlineView: outlineView)

        #expect(adapter.items.map(\.title) == ["Branch 2", "Branch 1", "Branch 3"])
        #expect(adapter.items[0] === second)
        #expect(adapter.items[1] === first)
        #expect(adapter.items[2] !== first)
        #expect(adapter.items[2] !== second)
    }

    @Test
    func sparseModelReportsLogicalRowCountWithoutDensifyingCache() throws {
        var store = SemanticStore()
        let modelID = ModelId(70)
        try store.createModel(id: modelID, modelType: .table, itemCount: 500_000)
        try store.modelResetRange(
            id: modelID,
            startIndex: 0,
            items: [
                ModelItem(itemID: ItemId(1), value: .list([.string("0"), .string("Row 0")])),
            ],
            totalCount: 500_000
        )
        let model = try #require(store.getModel(modelID))
        #expect(model.itemCount == 500_000)
        #expect(model.iterCachedItems().count == 1)

        var requests: [CollectionRangeRequest] = []
        let adapter = TableCollectionAdapter(
            nodeID: NodeId(2),
            model: model,
            modelID: modelID,
            onRangeRequest: { requests.append($0) }
        )
        let tableView = NSTableView()
        TableCollectionAdapter.reconcileColumns(in: tableView, columns: ["Index", "Label"], fallbackTitle: "Table")
        tableView.dataSource = adapter
        tableView.delegate = adapter

        #expect(adapter.numberOfRows(in: tableView) == 500_000)
        #expect(adapter.rows.count == 1)
        #expect(adapter.rowContent(at: 0)?.cells.first == "0")

        let loading = try #require(
            adapter.tableView(tableView, viewFor: tableView.tableColumns[0], row: 250_000) as? NSTextField
        )
        #expect(loading.stringValue == CollectionCells.loadingPlaceholder)
        #expect(adapter.tableView(tableView, shouldSelectRow: 250_000) == false)

        adapter.noteVisibleRange(start: 250_000, count: 20)
        adapter.noteVisibleRange(start: 250_000, count: 20)
        #expect(requests.count == 1)
        #expect(requests[0].nodeID == NodeId(2))
        #expect(requests[0].modelID == modelID)
        #expect(requests[0].count > 0)
        #expect(requests[0].startIndex % CollectionRangeTracker.pageSize == 0)

        adapter.noteDropped(start: requests[0].startIndex, count: requests[0].count)
        adapter.noteVisibleRange(start: 250_000, count: 20)
        #expect(requests.count == 2)
        #expect(requests[1].startIndex == requests[0].startIndex)
        #expect(requests[1].count == requests[0].count)
    }

    @Test
    func modelRangeArrivalPaintsWithoutReplacingTable() throws {
        var store = SemanticStore()
        let modelID = ModelId(71)
        try store.createModel(id: modelID, modelType: .table, itemCount: 500_000)
        let model = try #require(store.getModel(modelID))
        let adapter = TableCollectionAdapter(nodeID: NodeId(3), model: model, modelID: modelID)
        let tableView = NSTableView()
        TableCollectionAdapter.reconcileColumns(in: tableView, columns: ["Label"], fallbackTitle: "List")
        tableView.dataSource = adapter
        tableView.delegate = adapter
        let tableIdentity = ObjectIdentifier(tableView)

        try store.modelResetRange(
            id: modelID,
            startIndex: 10,
            items: [ModelItem(itemID: ItemId(11), value: .string("hydrated"))],
            totalCount: 500_000
        )
        adapter.update(model: store.getModel(modelID), modelID: modelID, tableView: tableView)
        #expect(ObjectIdentifier(tableView) == tableIdentity)
        #expect(adapter.rowContent(at: 10)?.cells == ["hydrated"])
        #expect(adapter.rowContent(at: 10)?.itemID == ItemId(11))
        #expect(adapter.numberOfRows(in: tableView) == 500_000)
    }

    @Test
    func modelBackedOutlineIsFlatRootLeaves() throws {
        var store = SemanticStore()
        let modelID = ModelId(72)
        try store.createModel(id: modelID, modelType: .tree, itemCount: 500_000)
        let model = try #require(store.getModel(modelID))
        var requests: [CollectionRangeRequest] = []
        let adapter = OutlineCollectionAdapter(
            nodeID: NodeId(4),
            model: model,
            modelID: modelID,
            onRangeRequest: { requests.append($0) }
        )
        let outlineView = NSOutlineView()
        outlineView.dataSource = adapter
        outlineView.delegate = adapter
        #expect(adapter.outlineView(outlineView, numberOfChildrenOfItem: nil) == 500_000)
        let leaf = adapter.outlineView(outlineView, child: 99, ofItem: nil) as? OutlineCollectionAdapter.Item
        #expect(leaf?.title == CollectionCells.loadingPlaceholder)
        #expect(adapter.outlineView(outlineView, isItemExpandable: leaf as Any) == false)
        adapter.noteVisibleRange(start: 0, count: 8)
        adapter.noteVisibleRange(start: 0, count: 8)
        #expect(requests.count == 1)

        adapter.resetRangeTracker()
        adapter.reissueVisibleRangeRequests()
        #expect(requests.count == 2)
        #expect(requests[1].startIndex == requests[0].startIndex)
    }

    @Test
    func listAndTableUseExactNSTableViewTreeUsesExactNSOutlineView() throws {
        let factory = ControlFactory()
        var store = SemanticStore()
        let modelID = ModelId(73)
        try store.createModel(id: modelID, modelType: .table, itemCount: 500_000)

        let list = try factory.makeHandle(
            for: Node(
                id: 1,
                nodeType: .list,
                properties: [.modelRef: .unsignedInt(modelID.value)]
            ),
            store: store
        )
        let table = try factory.makeHandle(
            for: Node(
                id: 2,
                nodeType: .table,
                properties: [.modelRef: .unsignedInt(modelID.value)]
            ),
            store: store
        )
        let tree = try factory.makeHandle(
            for: Node(
                id: 3,
                nodeType: .tree,
                properties: [.modelRef: .unsignedInt(modelID.value)]
            ),
            store: store
        )

        let listTable = try #require((list.view as? NSScrollView)?.documentView as? NSTableView)
        let tableTable = try #require((table.view as? NSScrollView)?.documentView as? NSTableView)
        let treeOutline = try #require((tree.view as? NSScrollView)?.documentView as? NSOutlineView)

        #expect(type(of: listTable) == NSTableView.self)
        #expect(type(of: tableTable) == NSTableView.self)
        #expect(type(of: treeOutline) == NSOutlineView.self)
        #expect(exactViewCount(NSTableView.self, in: list.view) == 1)
        #expect(exactViewCount(NSOutlineView.self, in: list.view) == 0)
        #expect(exactViewCount(NSTableView.self, in: table.view) == 1)
        #expect(exactViewCount(NSOutlineView.self, in: table.view) == 0)
        #expect(exactViewCount(NSOutlineView.self, in: tree.view) == 1)
        #expect(exactViewCount(NSTableView.self, in: tree.view) == 0)
        #expect(listTable.numberOfRows == 500_000)
        #expect(tableTable.numberOfRows == 500_000)
        #expect(treeOutline.numberOfRows == 500_000)
        #expect(listTable.tableColumns.count == 1)
        #expect(listTable.headerView == nil)
        #expect(tableTable.headerView != nil)
    }
}

private func exactViewCount<T: NSView>(_ type: T.Type, in root: NSView) -> Int {
    var count = 0
    if ObjectIdentifier(Swift.type(of: root)) == ObjectIdentifier(type) {
        count += 1
    }
    for child in root.subviews {
        count += exactViewCount(type, in: child)
    }
    return count
}
