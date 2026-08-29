import AppKit
import SemanticModel
import Testing
@testable import RendererAppKit

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
        var interactions: [SemanticInteraction] = []
        let adapter = TableCollectionAdapter(
            nodeID: 30,
            rows: [
                TableCollectionAdapter.TableRow(itemID: ItemId(100), cells: ["Item 100"]),
                TableCollectionAdapter.TableRow(itemID: ItemId(200), cells: ["Item 200"]),
                TableCollectionAdapter.TableRow(itemID: nil, cells: ["Inline Item (No ID)"]),
            ],
            selectionMode: .none,
            onInteraction: { interactions.append($0) }
        )

        let tableView = NSTableView()
        tableView.dataSource = adapter
        tableView.delegate = adapter

        // 1. None selection mode
        #expect(adapter.tableView(tableView, shouldSelectRow: 0) == false)
        #expect(adapter.tableView(tableView, shouldSelectRow: 1) == false)
        adapter.tableViewSelectionDidChange(Notification(name: NSTableView.selectionDidChangeNotification, object: tableView))
        #expect(interactions.isEmpty)

        // 2. Single selection mode
        adapter.selectionMode = .single
        #expect(adapter.tableView(tableView, shouldSelectRow: 0) == true)

        tableView.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
        #expect(interactions == [.selectionChanged(nodeID: 30, itemID: ItemId(100))])
        interactions.removeAll()

        tableView.selectRowIndexes(IndexSet(integer: 1), byExtendingSelection: false)
        #expect(interactions == [.selectionChanged(nodeID: 30, itemID: ItemId(200))])
        interactions.removeAll()

        // Selecting row with nil itemID (inline item) emits no interaction
        tableView.selectRowIndexes(IndexSet(integer: 2), byExtendingSelection: false)
        #expect(interactions.isEmpty)

        // Deselection (no row selected) emits no interaction
        tableView.deselectAll(nil)
        #expect(interactions.isEmpty)

        // 3. Multiple selection mode emits one event per selected item ID
        adapter.selectionMode = .multiple
        tableView.selectRowIndexes(IndexSet([0, 1]), byExtendingSelection: false)
        #expect(interactions == [
            .selectionChanged(nodeID: 30, itemID: ItemId(100)),
            .selectionChanged(nodeID: 30, itemID: ItemId(200)),
        ])
    }

    @Test
    func tableAdapterPreservesSelectionAcrossComplexUpdates() throws {
        var interactions: [SemanticInteraction] = []
        let adapter = TableCollectionAdapter(
            nodeID: 40,
            rows: [
                TableCollectionAdapter.TableRow(itemID: ItemId(1), cells: ["Row 1"]),
                TableCollectionAdapter.TableRow(itemID: ItemId(2), cells: ["Row 2"]),
                TableCollectionAdapter.TableRow(itemID: ItemId(3), cells: ["Row 3"]),
                TableCollectionAdapter.TableRow(itemID: ItemId(4), cells: ["Row 4"]),
            ],
            selectionMode: .single,
            onInteraction: { interactions.append($0) }
        )

        let tableView = NSTableView()
        tableView.dataSource = adapter
        tableView.delegate = adapter

        // Select row index 2 (ItemId 3)
        tableView.selectRowIndexes(IndexSet(integer: 2), byExtendingSelection: false)
        #expect(tableView.selectedRow == 2)
        #expect(interactions == [.selectionChanged(nodeID: 40, itemID: ItemId(3))])
        interactions.removeAll()

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
        #expect(interactions.isEmpty)

        // Update rows where the selected item (ItemId 3) is removed
        let withoutItem3 = [
            TableCollectionAdapter.TableRow(itemID: ItemId(5), cells: ["Row 5"]),
            TableCollectionAdapter.TableRow(itemID: ItemId(1), cells: ["Row 1"]),
            TableCollectionAdapter.TableRow(itemID: ItemId(4), cells: ["Row 4"]),
        ]
        adapter.update(rows: withoutItem3, tableView: tableView)

        // Selection should be empty
        #expect(tableView.selectedRow == -1)
        #expect(interactions.isEmpty)
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
}
