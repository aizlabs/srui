import AppKit
import SemanticModel

@MainActor
public final class TableCollectionAdapter: NSObject, NSTableViewDataSource, NSTableViewDelegate {
    public struct TableRow: Equatable, Sendable {
        public let itemID: ItemId?
        public let cells: [String]

        public init(itemID: ItemId? = nil, cells: [String]) {
            self.itemID = itemID
            self.cells = cells
        }
    }

    public let nodeID: NodeId
    public private(set) var rows: [TableRow]
    public var hasExplicitColumns: Bool = false
    public var selectionMode: StandardSelectionMode = .none
    public var onInteraction: (@MainActor (SemanticInteraction) -> Void)?

    private var isSuppressingSelectionEvents = false

    public init(
        nodeID: NodeId,
        rows: [TableRow] = [],
        hasExplicitColumns: Bool = false,
        selectionMode: StandardSelectionMode = .none,
        onInteraction: (@MainActor (SemanticInteraction) -> Void)? = nil
    ) {
        self.nodeID = nodeID
        self.rows = rows
        self.hasExplicitColumns = hasExplicitColumns
        self.selectionMode = selectionMode
        self.onInteraction = onInteraction
    }

    public func update(rows: [TableRow], tableView: NSTableView) {
        let selectedRowIndexes = tableView.selectedRowIndexes
        let selectedItemIDs: [ItemId] = selectedRowIndexes.compactMap { idx in
            guard idx >= 0 && idx < self.rows.count else { return nil }
            return self.rows[idx].itemID
        }

        isSuppressingSelectionEvents = true
        defer { isSuppressingSelectionEvents = false }

        self.rows = rows
        tableView.reloadData()

        if !selectedItemIDs.isEmpty {
            var newIndices = IndexSet()
            for id in selectedItemIDs {
                if let newIdx = self.rows.firstIndex(where: { $0.itemID == id }) {
                    newIndices.insert(newIdx)
                }
            }
            tableView.selectRowIndexes(newIndices, byExtendingSelection: false)
        } else {
            tableView.deselectAll(nil)
        }
    }

    public func numberOfRows(in tableView: NSTableView) -> Int {
        rows.count
    }

    public func tableView(
        _ tableView: NSTableView,
        viewFor tableColumn: NSTableColumn?,
        row: Int
    ) -> NSView? {
        guard row >= 0 && row < rows.count else { return nil }
        let tableRow = rows[row]
        let columnIndex: Int
        if let tableColumn, let idx = tableView.tableColumns.firstIndex(of: tableColumn) {
            columnIndex = idx
        } else {
            columnIndex = 0
        }

        let cellText = columnIndex < tableRow.cells.count ? tableRow.cells[columnIndex] : ""
        return NSTextField(labelWithString: cellText)
    }

    public func tableView(_ tableView: NSTableView, shouldSelectRow row: Int) -> Bool {
        selectionMode != .none
    }

    public func tableViewSelectionDidChange(_ notification: Notification) {
        guard !isSuppressingSelectionEvents else { return }
        guard let tableView = notification.object as? NSTableView else { return }
        let selectedIndexes = tableView.selectedRowIndexes
        guard selectedIndexes.count == 1, let selectedRow = selectedIndexes.first else {
            return
        }
        guard selectedRow >= 0 && selectedRow < rows.count else { return }
        guard let itemID = rows[selectedRow].itemID else {
            return
        }
        onInteraction?(.selectionChanged(nodeID: nodeID, itemID: itemID))
    }
}

@MainActor
public final class OutlineCollectionAdapter: NSObject, NSOutlineViewDataSource, NSOutlineViewDelegate {
    public final class Item: NSObject {
        public let title: String

        public init(title: String) {
            self.title = title
        }
    }

    public private(set) var items: [Item]

    public init(rows: [String]) {
        self.items = rows.map(Item.init(title:))
    }

    public func update(rows: [String], outlineView: NSOutlineView) {
        items = rows.map(Item.init(title:))
        outlineView.reloadData()
    }

    public func outlineView(
        _ outlineView: NSOutlineView,
        numberOfChildrenOfItem item: Any?
    ) -> Int {
        item == nil ? items.count : 0
    }

    public func outlineView(
        _ outlineView: NSOutlineView,
        child index: Int,
        ofItem item: Any?
    ) -> Any {
        items[index]
    }

    public func outlineView(_ outlineView: NSOutlineView, isItemExpandable item: Any) -> Bool {
        false
    }

    public func outlineView(
        _ outlineView: NSOutlineView,
        viewFor tableColumn: NSTableColumn?,
        item: Any
    ) -> NSView? {
        guard let item = item as? Item else { return nil }
        return NSTextField(labelWithString: item.title)
    }
}
