import AppKit
import SemanticModel

/// How a collection adapter applied the last row mutation. Exposed for tests.
enum TableRowUpdate: Equatable {
    case none
    case fullReload
    case contentReload(IndexSet)
}

/// Shared scrolling chrome for list/table/tree when they sit inside an ancestor `.scroll` node.
@MainActor
enum CollectionScrollEmbedding {
    static func apply(
        nested: Bool,
        scrollView: NSScrollView,
        minHeightConstraint: NSLayoutConstraint?,
        fitHeightConstraint: inout NSLayoutConstraint?,
        contentHeight: CGFloat
    ) {
        scrollView.hasVerticalScroller = !nested
        scrollView.hasHorizontalScroller = false
        scrollView.verticalScrollElasticity = nested ? .none : .automatic
        scrollView.horizontalScrollElasticity = .none
        scrollView.borderType = nested ? .noBorder : .bezelBorder

        if nested {
            minHeightConstraint?.isActive = false
            let height = max(contentHeight, 1)
            if let existing = fitHeightConstraint {
                existing.constant = height
                existing.isActive = true
            } else {
                let constraint = scrollView.heightAnchor.constraint(equalToConstant: height)
                constraint.priority = .defaultHigh
                constraint.isActive = true
                fitHeightConstraint = constraint
            }
        } else {
            fitHeightConstraint?.isActive = false
            minHeightConstraint?.isActive = true
        }
    }

    static func tableContentHeight(_ tableView: NSTableView) -> CGFloat {
        let headerHeight = tableView.headerView?.frame.height ?? 0
        let rowHeight = tableView.rowHeight > 0 ? tableView.rowHeight : 17
        let spacing = tableView.intercellSpacing.height
        let rowCount = max(tableView.numberOfRows, 1)
        return headerHeight + CGFloat(rowCount) * (rowHeight + spacing)
    }
}

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

    public static let primaryColumnIdentifier = NSUserInterfaceItemIdentifier("srui.column.primary")
    public static let rowIdentifier = NSUserInterfaceItemIdentifier("srui.row")

    public let nodeID: NodeId
    public private(set) var rows: [TableRow]
    public var hasExplicitColumns: Bool = false
    public var selectionMode: StandardSelectionMode = .none
    public var onInteraction: (@MainActor (SemanticInteraction) -> Void)?

    var minHeightConstraint: NSLayoutConstraint?
    var fitHeightConstraint: NSLayoutConstraint?
    private(set) var isNestedInScroll = false
    private(set) var lastRowUpdate: TableRowUpdate = .none

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

    public static func columnIdentifier(for title: String, occurrence: Int = 1) -> NSUserInterfaceItemIdentifier {
        let key = title.isEmpty ? "column" : title
        if occurrence <= 1 {
            return NSUserInterfaceItemIdentifier("srui.column.\(key)")
        }
        return NSUserInterfaceItemIdentifier("srui.column.\(key).\(occurrence)")
    }

    public static func columnIdentifiers(for titles: [String]) -> [NSUserInterfaceItemIdentifier] {
        var seen: [String: Int] = [:]
        return titles.map { title in
            let key = title.isEmpty ? "column" : title
            let occurrence = (seen[key] ?? 0) + 1
            seen[key] = occurrence
            return columnIdentifier(for: title, occurrence: occurrence)
        }
    }

    public static func reconcileColumns(
        in tableView: NSTableView,
        columns: [String]?,
        fallbackTitle: String
    ) {
        if let columns, !columns.isEmpty {
            let identifiers = columnIdentifiers(for: columns)
            var unused = tableView.tableColumns
            var ordered: [NSTableColumn] = []

            for (title, identifier) in zip(columns, identifiers) {
                if let matchIndex = unused.firstIndex(where: { $0.identifier == identifier }) {
                    let column = unused.remove(at: matchIndex)
                    column.title = title
                    ordered.append(column)
                } else if !unused.isEmpty {
                    let column = unused.removeFirst()
                    column.identifier = identifier
                    column.title = title
                    ordered.append(column)
                } else {
                    let column = NSTableColumn(identifier: identifier)
                    column.title = title
                    column.width = max(80, 360 / CGFloat(columns.count))
                    ordered.append(column)
                }
            }

            for leftover in unused {
                tableView.removeTableColumn(leftover)
            }
            for column in ordered where tableView.tableColumns.contains(column) == false {
                tableView.addTableColumn(column)
            }
            for (target, column) in ordered.enumerated() {
                if let current = tableView.tableColumns.firstIndex(of: column), current != target {
                    tableView.moveColumn(current, toColumn: target)
                }
            }
            return
        }

        if tableView.tableColumns.isEmpty {
            let column = NSTableColumn(identifier: primaryColumnIdentifier)
            column.title = fallbackTitle
            column.width = 360
            tableView.addTableColumn(column)
            return
        }

        let column = tableView.tableColumns[0]
        column.identifier = primaryColumnIdentifier
        column.title = fallbackTitle
        while tableView.tableColumns.count > 1 {
            guard let last = tableView.tableColumns.last else { break }
            tableView.removeTableColumn(last)
        }
    }

    public static func applySelectionMode(_ mode: StandardSelectionMode, to tableView: NSTableView) {
        switch mode {
        case .none:
            tableView.allowsEmptySelection = true
            tableView.allowsMultipleSelection = false
            tableView.selectionHighlightStyle = .none
            tableView.deselectAll(nil)
        case .single:
            tableView.allowsEmptySelection = true
            tableView.allowsMultipleSelection = false
            tableView.selectionHighlightStyle = .regular
        case .multiple:
            tableView.allowsEmptySelection = true
            tableView.allowsMultipleSelection = true
            tableView.selectionHighlightStyle = .regular
        }
    }

    public static func applyChrome(isList: Bool, to tableView: NSTableView) {
        if isList {
            tableView.headerView = nil
            tableView.usesAlternatingRowBackgroundColors = false
            tableView.style = .plain
        } else {
            if tableView.headerView == nil {
                tableView.headerView = NSTableHeaderView()
            }
            tableView.usesAlternatingRowBackgroundColors = true
            tableView.style = .fullWidth
        }
    }

    func setNestedInScroll(_ nested: Bool, scrollView: NSScrollView, tableView: NSTableView) {
        isNestedInScroll = nested
        CollectionScrollEmbedding.apply(
            nested: nested,
            scrollView: scrollView,
            minHeightConstraint: minHeightConstraint,
            fitHeightConstraint: &fitHeightConstraint,
            contentHeight: CollectionScrollEmbedding.tableContentHeight(tableView)
        )
    }

    public func update(rows: [TableRow], tableView: NSTableView) {
        let selectedRowIndexes = tableView.selectedRowIndexes
        let selectedItemIDs: [ItemId] = selectedRowIndexes.compactMap { idx in
            guard idx >= 0 && idx < self.rows.count else { return nil }
            return self.rows[idx].itemID
        }

        isSuppressingSelectionEvents = true
        defer { isSuppressingSelectionEvents = false }

        let oldRows = self.rows
        self.rows = rows
        applyRowUpdate(from: oldRows, to: rows, tableView: tableView)

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

        refreshRowAccessibility(in: tableView)
        if isNestedInScroll, let scrollView = tableView.enclosingScrollView {
            setNestedInScroll(true, scrollView: scrollView, tableView: tableView)
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
        let identifier = tableColumn?.identifier ?? Self.primaryColumnIdentifier
        let field: NSTextField
        if let reused = tableView.makeView(withIdentifier: identifier, owner: self) as? NSTextField {
            field = reused
        } else {
            field = NSTextField(labelWithString: "")
            field.identifier = identifier
            field.lineBreakMode = .byTruncatingTail
            field.maximumNumberOfLines = 1
        }
        field.stringValue = cellText
        field.setAccessibilityElement(false)
        return field
    }

    public func tableView(_ tableView: NSTableView, rowViewForRow row: Int) -> NSTableRowView? {
        let rowView = tableView.makeView(withIdentifier: Self.rowIdentifier, owner: self) as? NSTableRowView
            ?? NSTableRowView()
        rowView.identifier = Self.rowIdentifier
        return rowView
    }

    public func tableView(_ tableView: NSTableView, didAdd rowView: NSTableRowView, forRow row: Int) {
        applyAccessibility(to: rowView, row: row, selected: tableView.selectedRowIndexes.contains(row))
    }

    public func tableView(_ tableView: NSTableView, shouldSelectRow row: Int) -> Bool {
        selectionMode != .none
    }

    public func tableViewSelectionDidChange(_ notification: Notification) {
        guard !isSuppressingSelectionEvents else { return }
        guard selectionMode != .none else { return }
        guard let tableView = notification.object as? NSTableView else { return }
        refreshRowAccessibility(in: tableView)

        let selectedIDs: [ItemId] = tableView.selectedRowIndexes.compactMap { index in
            guard index >= 0 && index < rows.count else { return nil }
            return rows[index].itemID
        }
        guard selectedIDs.isEmpty == false else { return }

        if selectionMode == .single {
            guard selectedIDs.count == 1, let itemID = selectedIDs.first else { return }
            onInteraction?(.selectionChanged(nodeID: nodeID, itemID: itemID))
            return
        }

        for itemID in selectedIDs {
            onInteraction?(.selectionChanged(nodeID: nodeID, itemID: itemID))
        }
    }

    private func applyRowUpdate(from oldRows: [TableRow], to newRows: [TableRow], tableView: NSTableView) {
        let columnIndexes = IndexSet(integersIn: 0..<tableView.numberOfColumns)
        if oldRows.map(\.itemID) == newRows.map(\.itemID), oldRows.count == newRows.count {
            let changed = IndexSet(oldRows.indices.filter { oldRows[$0].cells != newRows[$0].cells })
            if changed.isEmpty || columnIndexes.isEmpty {
                lastRowUpdate = .none
                return
            }
            tableView.reloadData(forRowIndexes: changed, columnIndexes: columnIndexes)
            lastRowUpdate = .contentReload(changed)
            return
        }

        tableView.reloadData()
        lastRowUpdate = .fullReload
    }

    private func refreshRowAccessibility(in tableView: NSTableView) {
        let selected = tableView.selectedRowIndexes
        tableView.enumerateAvailableRowViews { rowView, row in
            applyAccessibility(to: rowView, row: row, selected: selected.contains(row))
        }
    }

    private func applyAccessibility(to rowView: NSTableRowView, row: Int, selected: Bool) {
        guard row >= 0 && row < rows.count else { return }
        let label = rows[row].cells.filter { $0.isEmpty == false }.joined(separator: ", ")
        rowView.setAccessibilityRole(.row)
        rowView.setAccessibilityLabel(label.isEmpty ? nil : label)
        rowView.setAccessibilitySelected(selected)
    }
}

@MainActor
public final class OutlineCollectionAdapter: NSObject, NSOutlineViewDataSource, NSOutlineViewDelegate {
    public static let cellIdentifier = NSUserInterfaceItemIdentifier("srui.outline.cell")

    public final class Item: NSObject {
        public var title: String

        public init(title: String) {
            self.title = title
        }
    }

    public private(set) var items: [Item]
    var minHeightConstraint: NSLayoutConstraint?
    var fitHeightConstraint: NSLayoutConstraint?
    private(set) var isNestedInScroll = false

    public init(rows: [String]) {
        self.items = rows.map(Item.init(title:))
    }

    public func update(rows: [String], outlineView: NSOutlineView) {
        items = reusedItems(matching: rows)
        outlineView.reloadData()
        if isNestedInScroll, let scrollView = outlineView.enclosingScrollView {
            setNestedInScroll(true, scrollView: scrollView, outlineView: outlineView)
        }
    }

    func setNestedInScroll(_ nested: Bool, scrollView: NSScrollView, outlineView: NSOutlineView) {
        isNestedInScroll = nested
        CollectionScrollEmbedding.apply(
            nested: nested,
            scrollView: scrollView,
            minHeightConstraint: minHeightConstraint,
            fitHeightConstraint: &fitHeightConstraint,
            contentHeight: CollectionScrollEmbedding.tableContentHeight(outlineView)
        )
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
        let field: NSTextField
        if let reused = outlineView.makeView(withIdentifier: Self.cellIdentifier, owner: self) as? NSTextField {
            field = reused
        } else {
            field = NSTextField(labelWithString: "")
            field.identifier = Self.cellIdentifier
            field.lineBreakMode = .byTruncatingTail
            field.maximumNumberOfLines = 1
        }
        field.stringValue = item.title
        field.setAccessibilityElement(false)
        return field
    }

    private func reusedItems(matching titles: [String]) -> [Item] {
        var unused = items
        return titles.map { title in
            if let index = unused.firstIndex(where: { $0.title == title }) {
                return unused.remove(at: index)
            }
            return Item(title: title)
        }
    }
}
