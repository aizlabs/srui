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

    /// PX-017-G02: a user action that leaves nothing selected is reported exactly once, with no
    /// item (`SELECTION_CHANGED` with no `value`, spec/registries.md §4.4). A deselection the
    /// server causes — a commit removing the row, a promotion, its withdrawal, a restore — is
    /// never reported. A user deselect after a promotion still leaves the withdrawal a no-op.
    @Test
    func tableAdapterReportsAUserClearedSelectionOnceAndNoServerDeselection() throws {
        var selections: [ItemId] = []
        var cleared: [NodeId] = []
        let rows = (1...4).map {
            TableCollectionAdapter.TableRow(itemID: ItemId(UInt64($0)), cells: ["Row \($0)"])
        }
        let adapter = TableCollectionAdapter(
            nodeID: 41,
            rows: rows,
            selectionMode: .single,
            onSelectionChanged: { _, itemID in selections.append(itemID) }
        )
        adapter.onSelectionCleared = { cleared.append($0) }
        var local: [[ItemId]] = []
        adapter.onLocalSelection = { _, itemIDs in local.append(itemIDs) }
        let tableView = NSTableView()
        tableView.dataSource = adapter
        tableView.delegate = adapter

        // User selects, then deselects (the same delegate path as a click in empty space or
        // Command-click on the selected row): one clear, and the empty selection is kept locally.
        tableView.selectRowIndexes(IndexSet(integer: 1), byExtendingSelection: false)
        tableView.deselectAll(nil)
        #expect(selections == [ItemId(2)])
        #expect(cleared == [NodeId(41)])
        #expect(local.last == [])
        // Nothing to deselect: no change, nothing reported.
        tableView.deselectAll(nil)
        #expect(cleared == [NodeId(41)])

        // A commit removing the selected row deselects it without a report.
        tableView.selectRowIndexes(IndexSet(integer: 2), byExtendingSelection: false)
        adapter.update(rows: rows.filter { $0.itemID != ItemId(3) }, tableView: tableView)
        #expect(tableView.selectedRowIndexes.isEmpty)

        // A promotion and its withdrawal of the untouched promoted row: no report.
        adapter.applyPromotion(ItemId(1), reveal: false, tableView: tableView)
        #expect(tableView.selectedRowIndexes == IndexSet(integer: 0))
        adapter.applyPromotion(nil, reveal: false, tableView: tableView)
        #expect(tableView.selectedRowIndexes.isEmpty)

        // A promotion of an item not yet in the replica clears the selection while it waits.
        tableView.selectRowIndexes(IndexSet(integer: 1), byExtendingSelection: false)
        adapter.applyPromotion(ItemId(99), reveal: false, tableView: tableView)
        #expect(tableView.selectedRowIndexes.isEmpty)

        // Restoring an empty local selection on remount: no report.
        tableView.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
        adapter.restoreLocalSelection([], tableView: tableView)
        #expect(tableView.selectedRowIndexes.isEmpty)
        #expect(cleared == [NodeId(41)], "a server-driven deselection is never reported")

        // Promote, then the user deselects: reported once; the later withdrawal keeps the user's
        // (empty) selection and reports nothing (PX-011-G01).
        adapter.applyPromotion(ItemId(2), reveal: false, tableView: tableView)
        #expect(tableView.selectedRowIndexes == IndexSet(integer: 1))
        tableView.deselectAll(nil)
        #expect(cleared == [NodeId(41), NodeId(41)])
        #expect(adapter.localSelectionSincePromotion)
        adapter.applyPromotion(nil, reveal: false, tableView: tableView)
        #expect(tableView.selectedRowIndexes.isEmpty)
        #expect(cleared.count == 2)
        #expect(selections == [ItemId(2), ItemId(3), ItemId(2), ItemId(1)])

        // selection_mode none never reports anything.
        adapter.selectionMode = .none
        adapter.tableViewSelectionDidChange(
            Notification(name: NSTableView.selectionDidChangeNotification, object: tableView)
        )
        #expect(cleared.count == 2)
    }

    /// PX-017-G02: multiple selection stays one event per selected item; a change that leaves
    /// some items selected never reports which were deselected; only a change to nothing
    /// selected is reported as a clear.
    @Test
    func tableAdapterMultipleSelectionReportsOnlyAFullClear() throws {
        var selections: [ItemId] = []
        var cleared = 0
        let adapter = TableCollectionAdapter(
            nodeID: 42,
            rows: (1...3).map {
                TableCollectionAdapter.TableRow(itemID: ItemId(UInt64($0)), cells: ["Row \($0)"])
            },
            selectionMode: .multiple,
            onSelectionChanged: { _, itemID in selections.append(itemID) }
        )
        adapter.onSelectionCleared = { _ in cleared += 1 }
        let tableView = NSTableView()
        tableView.allowsMultipleSelection = true
        tableView.dataSource = adapter
        tableView.delegate = adapter

        tableView.selectRowIndexes(IndexSet([0, 2]), byExtendingSelection: false)
        #expect(selections == [ItemId(1), ItemId(3)])
        tableView.deselectRow(2)
        #expect(selections == [ItemId(1), ItemId(3), ItemId(1)])
        #expect(cleared == 0, "a partial deselection is not a clear")
        tableView.deselectRow(0)
        #expect(cleared == 1)
        #expect(selections.count == 3)
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

    /// A re-sort of a model-backed table keeps the row count, so the adapter reloads row content
    /// in place instead of calling `reloadData()`. AppKit can hold cell views for rows outside
    /// `visibleRect`: rows it prepared around the viewport, and any view realized on demand by
    /// `view(atColumn:row:makeIfNecessary:)` (accessibility, a test reading the table). Every such
    /// view must show the commit's item at its row, or scrolling it into view shows another
    /// item's cells, here on the very row the selection moved to by ItemId (§8, §22.5, §22.7).
    /// PX-017's native rapid-arrow test saw exactly this: the selected row showed the next PID.
    @Test
    func aResortReloadsEveryRealizedRowNotOnlyTheVisibleOnes() throws {
        func item(_ id: UInt64) -> ModelItem {
            ModelItem(itemID: ItemId(id), value: .list([.string("\(6000 + id)")]))
        }
        var store = SemanticStore()
        let modelID = ModelId(72)
        try store.createModel(id: modelID, modelType: .table, itemCount: 0)
        // Row r holds ItemId r + 1 ("600(r+1)"), with ItemId 100 inserted at row 55.
        var ids = Array(UInt64(1)...59)
        ids.insert(100, at: 55)
        try store.modelInsert(id: modelID, index: 0, items: ids.map(item))
        var selections: [ItemId] = []
        let adapter = TableCollectionAdapter(
            nodeID: NodeId(4),
            model: store.getModel(modelID),
            modelID: modelID,
            selectionMode: .single,
            onSelectionChanged: { selections.append($1) }
        )
        let scrollView = NSScrollView(frame: NSRect(x: 0, y: 0, width: 300, height: 90))
        let tableView = NSTableView(frame: .zero)
        TableCollectionAdapter.reconcileColumns(in: tableView, columns: ["PID"], fallbackTitle: "Table")
        tableView.dataSource = adapter
        tableView.delegate = adapter
        scrollView.documentView = tableView
        let window = NSWindow(
            contentRect: scrollView.frame,
            styleMask: [.titled],
            backing: .buffered,
            defer: false
        )
        window.isReleasedWhenClosed = false
        window.contentView = scrollView
        defer { window.close() }
        tableView.reloadData()

        // The user's selection is the bottom visible row, as after a run of Down Arrow presses.
        tableView.selectRowIndexes(IndexSet(integer: 45), byExtendingSelection: false)
        tableView.scrollRowToVisible(45)
        tableView.layoutSubtreeIfNeeded()
        #expect(selections == [ItemId(46)])
        let visible = tableView.rows(in: tableView.visibleRect)
        #expect(NSLocationInRange(46, visible) == false, "row 46 starts below the viewport")
        _ = tableView.view(atColumn: 0, row: 46, makeIfNecessary: true)
        var realized: [Int] = []
        tableView.enumerateAvailableRowViews { _, row in realized.append(row) }
        #expect(realized.contains(46), "AppKit holds row 46 outside the visible rows \(visible): \(realized)")

        // A sample moves ItemId 100 from row 55 to row 41: rows 41–54 shift down by one, and the
        // selected ItemId 46 moves from row 45 to row 46, below the viewport.
        try store.modelDelete(id: modelID, index: nil, count: nil, itemIds: [ItemId(100)])
        try store.modelInsert(id: modelID, index: 41, items: [item(100)])
        adapter.update(model: store.getModel(modelID), modelID: modelID, tableView: tableView)

        #expect(tableView.selectedRowIndexes == IndexSet(integer: 46), "the selection keeps its ItemId")
        #expect(adapter.itemID(at: 46) == ItemId(46))
        #expect(selections == [ItemId(46)], "a commit emits no SELECTION_CHANGED")
        func shown(_ row: Int) -> String? {
            (tableView.view(atColumn: 0, row: row, makeIfNecessary: false) as? NSTextField)?.stringValue
        }
        tableView.enumerateAvailableRowViews { _, row in
            #expect(shown(row) == adapter.rowContent(at: row)?.cells.first, "realized row \(row) shows its item")
        }
        // Scrolled into view, the selected row shows the selected item.
        tableView.scrollRowToVisible(46)
        tableView.layoutSubtreeIfNeeded()
        #expect(shown(46) == "6046")
        for row in 40...47 {
            tableView.scrollRowToVisible(row)
            tableView.layoutSubtreeIfNeeded()
            #expect(shown(row) == adapter.rowContent(at: row)?.cells.first, "row \(row) after scrolling to it")
        }
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

// `NSView.subviews` is main-actor isolated in the macOS 15+ SDKs, and every call
// site is inside the `@MainActor` suite above, so the helper is isolated too
// rather than reaching into AppKit from a nonisolated context.
@MainActor
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
