//
// TableCollectionAdapter.swift
// Collections
//
// NSTableView data source for List (one column) and Table (multi-column). Model-backed
// collections report logical `itemCount` and resolve rows with `getItemByIndex` (§8, §22.7).
//

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

    public static let primaryColumnIdentifier = NSUserInterfaceItemIdentifier("srui.column.primary")
    public static let rowIdentifier = NSUserInterfaceItemIdentifier("srui.row")

    public let nodeID: NodeId
    private var inlineRows: [TableRow]
    public private(set) var model: Model?
    public private(set) var modelID: ModelId?
    public var hasExplicitColumns: Bool = false
    public var selectionMode: StandardSelectionMode = .none
    public var onSelectionChanged: (@MainActor (NodeId, ItemId) -> Void)?
    public var onRangeRequest: (@MainActor (CollectionRangeRequest) -> Void)?

    /// Nested-scroll chrome. Public so `ControlFactory` (RendererAppKit) can
    /// wire collections that now live in a separate module.
    public var minHeightConstraint: NSLayoutConstraint?
    public var fitHeightConstraint: NSLayoutConstraint?
    public private(set) var isNestedInScroll = false
    private(set) var lastRowUpdate: TableRowUpdate = .none
    private(set) var rangeTracker = CollectionRangeTracker()

    private var isSuppressingSelectionEvents = false
    private weak var observedTableView: NSTableView?
    private weak var observedClipView: NSClipView?
    private var lastVisibleStart: UInt64 = 0
    private var lastVisibleCount: UInt64 = 0

    public var isModelBacked: Bool { model != nil }

    /// Inline rows, or cached model rows in index order. Does not densify uncached holes.
    public var materialisedRows: [TableRow] {
        if let model {
            return model.iterCachedItems().map { _, item in
                TableRow(itemID: item.itemID, cells: CollectionCells.cells(from: item.value))
            }
        }
        return inlineRows
    }

    /// Cached or inline rows only — never a dense `0..<itemCount` array (§8).
    public var rows: [TableRow] { materialisedRows }

    public init(
        nodeID: NodeId,
        rows: [TableRow] = [],
        model: Model? = nil,
        modelID: ModelId? = nil,
        hasExplicitColumns: Bool = false,
        selectionMode: StandardSelectionMode = .none,
        onSelectionChanged: (@MainActor (NodeId, ItemId) -> Void)? = nil,
        onRangeRequest: (@MainActor (CollectionRangeRequest) -> Void)? = nil
    ) {
        self.nodeID = nodeID
        self.inlineRows = rows
        self.model = model
        self.modelID = modelID ?? model?.id
        self.hasExplicitColumns = hasExplicitColumns
        self.selectionMode = selectionMode
        self.onSelectionChanged = onSelectionChanged
        self.onRangeRequest = onRangeRequest
    }

    deinit {
        NotificationCenter.default.removeObserver(self)
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

    public func setNestedInScroll(_ nested: Bool, scrollView: NSScrollView, tableView: NSTableView) {
        isNestedInScroll = nested
        CollectionScrollEmbedding.apply(
            nested: nested,
            modelBacked: isModelBacked,
            scrollView: scrollView,
            minHeightConstraint: minHeightConstraint,
            fitHeightConstraint: &fitHeightConstraint,
            contentHeight: CollectionScrollEmbedding.tableContentHeight(tableView)
        )
        attachViewportObservation(scrollView: scrollView, tableView: tableView)
    }

    public func attachViewportObservation(scrollView: NSScrollView, tableView: NSTableView) {
        observedTableView = tableView
        let clipView = scrollView.contentView
        clipView.postsBoundsChangedNotifications = true
        if let previous = observedClipView {
            NotificationCenter.default.removeObserver(
                self,
                name: NSView.boundsDidChangeNotification,
                object: previous
            )
        }
        observedClipView = clipView
        NotificationCenter.default.addObserver(
            self,
            selector: #selector(clipViewBoundsDidChange(_:)),
            name: NSView.boundsDidChangeNotification,
            object: clipView
        )
        emitVisibleRangeRequests(from: tableView)
    }

    @objc private func clipViewBoundsDidChange(_ notification: Notification) {
        emitVisibleRangeRequests()
    }

    public func resetRangeTracker() {
        rangeTracker.reset()
    }

    /// Forget an in-flight window that never reached the server so the next
    /// viewport update can re-emit it.
    public func noteDropped(start: UInt64, count: UInt64) {
        rangeTracker.noteDropped(start: start, count: count)
    }

    /// Re-emits cache-miss requests for the last known visible window (§8, §22.7).
    public func reissueVisibleRangeRequests() {
        if lastVisibleCount > 0 {
            emitRequests(visibleStart: lastVisibleStart, visibleCount: lastVisibleCount)
        } else {
            emitVisibleRangeRequests()
        }
    }

    public func update(rows: [TableRow], tableView: NSTableView) {
        model = nil
        modelID = nil
        rangeTracker.reset()
        let selectedItemIDs = selectedItemIDs(in: tableView)

        isSuppressingSelectionEvents = true
        defer { isSuppressingSelectionEvents = false }

        let oldRows = self.inlineRows
        self.inlineRows = rows
        applyRowUpdate(from: oldRows, to: rows, tableView: tableView)
        restoreSelection(selectedItemIDs, in: tableView)
        refreshRowAccessibility(in: tableView)
        refreshNestedChrome(tableView: tableView)
    }

    public func update(model: Model?, modelID: ModelId?, tableView: NSTableView) {
        let selectedItemIDs = selectedItemIDs(in: tableView)
        let previousID = self.modelID
        let previousCount = self.model?.itemCount

        isSuppressingSelectionEvents = true
        defer { isSuppressingSelectionEvents = false }

        if let model {
            for range in model.cachedRanges() {
                rangeTracker.noteArrived(start: range.start, count: range.length)
            }
        }

        let replaced = previousID != (modelID ?? model?.id) || (model == nil && self.model != nil)
        if replaced {
            rangeTracker.reset()
        }

        self.model = model
        self.modelID = modelID ?? model?.id
        self.inlineRows = []

        let countChanged = previousCount != model?.itemCount
        if countChanged || replaced {
            tableView.reloadData()
            lastRowUpdate = .fullReload
        } else {
            reloadVisibleRows(in: tableView)
            lastRowUpdate = .contentReload(visibleRowIndexes(in: tableView))
        }

        restoreSelection(selectedItemIDs, in: tableView)
        refreshRowAccessibility(in: tableView)
        refreshNestedChrome(tableView: tableView)
        emitVisibleRangeRequests(from: tableView)
    }

    /// Test helper: treat `start..<start+count` as the visible window without a live clip view.
    public func noteVisibleRange(start: UInt64, count: UInt64) {
        emitRequests(visibleStart: start, visibleCount: count)
    }

    public func numberOfRows(in tableView: NSTableView) -> Int {
        if let model {
            return CollectionCells.clampedRowCount(model.itemCount)
        }
        return inlineRows.count
    }

    public func tableView(
        _ tableView: NSTableView,
        viewFor tableColumn: NSTableColumn?,
        row: Int
    ) -> NSView? {
        guard let tableRow = rowContent(at: row) else { return nil }
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
        guard selectionMode != .none else { return false }
        return itemID(at: row) != nil
    }

    public func tableViewSelectionDidChange(_ notification: Notification) {
        guard !isSuppressingSelectionEvents else { return }
        guard selectionMode != .none else { return }
        guard let tableView = notification.object as? NSTableView else { return }
        refreshRowAccessibility(in: tableView)

        let selectedIDs: [ItemId] = tableView.selectedRowIndexes.compactMap { itemID(at: $0) }
        guard selectedIDs.isEmpty == false else { return }

        if selectionMode == .single {
            guard selectedIDs.count == 1, let itemID = selectedIDs.first else { return }
            onSelectionChanged?(nodeID, itemID)
            return
        }

        for itemID in selectedIDs {
            onSelectionChanged?(nodeID, itemID)
        }
    }

    public func rowContent(at row: Int) -> TableRow? {
        if let model {
            guard row >= 0 else { return nil }
            let index = UInt64(row)
            guard index < model.itemCount else { return nil }
            if let item = model.getItemByIndex(index) {
                return TableRow(itemID: item.itemID, cells: CollectionCells.cells(from: item.value))
            }
            let columns = max(1, observedTableView?.tableColumns.count ?? 1)
            return TableRow(
                itemID: nil,
                cells: Array(repeating: CollectionCells.loadingPlaceholder, count: columns)
            )
        }
        guard row >= 0 && row < inlineRows.count else { return nil }
        return inlineRows[row]
    }

    public func itemID(at row: Int) -> ItemId? {
        rowContent(at: row)?.itemID
    }

    private func emitVisibleRangeRequests(from tableView: NSTableView? = nil) {
        let tableView = tableView ?? observedTableView
        guard let tableView else { return }
        let rows = tableView.rows(in: tableView.visibleRect)
        guard rows.location != NSNotFound, rows.length > 0 else { return }
        emitRequests(visibleStart: UInt64(rows.location), visibleCount: UInt64(rows.length))
    }

    private func emitRequests(visibleStart: UInt64, visibleCount: UInt64) {
        lastVisibleStart = visibleStart
        lastVisibleCount = visibleCount
        guard let model, let modelID, let onRangeRequest else { return }
        let requests = rangeTracker.requests(
            visibleStart: visibleStart,
            visibleCount: visibleCount,
            itemCount: model.itemCount,
            model: model,
            nodeID: nodeID,
            modelID: modelID
        )
        for request in requests {
            onRangeRequest(request)
        }
    }

    private func selectedItemIDs(in tableView: NSTableView) -> [ItemId] {
        tableView.selectedRowIndexes.compactMap { itemID(at: $0) }
    }

    private func restoreSelection(_ selectedItemIDs: [ItemId], in tableView: NSTableView) {
        if selectedItemIDs.isEmpty {
            tableView.deselectAll(nil)
            return
        }
        var newIndices = IndexSet()
        if let model {
            for (index, item) in model.iterCachedItems() {
                if selectedItemIDs.contains(item.itemID), index <= UInt64(Int.max) {
                    newIndices.insert(Int(index))
                }
            }
        } else {
            for row in 0..<inlineRows.count {
                if let id = inlineRows[row].itemID, selectedItemIDs.contains(id) {
                    newIndices.insert(row)
                }
            }
        }
        tableView.selectRowIndexes(newIndices, byExtendingSelection: false)
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

    private func visibleRowIndexes(in tableView: NSTableView) -> IndexSet {
        let visible = tableView.rows(in: tableView.visibleRect)
        guard visible.location != NSNotFound, visible.length > 0 else { return IndexSet() }
        let start = visible.location
        let end = start + visible.length
        return IndexSet(integersIn: start..<end)
    }

    private func reloadVisibleRows(in tableView: NSTableView) {
        let indexes = visibleRowIndexes(in: tableView)
        let columnIndexes = IndexSet(integersIn: 0..<tableView.numberOfColumns)
        if !indexes.isEmpty, !columnIndexes.isEmpty {
            tableView.reloadData(forRowIndexes: indexes, columnIndexes: columnIndexes)
        } else {
            tableView.reloadData()
        }
    }

    private func refreshNestedChrome(tableView: NSTableView) {
        if isNestedInScroll, let scrollView = tableView.enclosingScrollView {
            setNestedInScroll(true, scrollView: scrollView, tableView: tableView)
        }
    }

    private func refreshRowAccessibility(in tableView: NSTableView) {
        let selected = tableView.selectedRowIndexes
        tableView.enumerateAvailableRowViews { rowView, row in
            applyAccessibility(to: rowView, row: row, selected: selected.contains(row))
        }
    }

    private func applyAccessibility(to rowView: NSTableRowView, row: Int, selected: Bool) {
        guard let tableRow = rowContent(at: row) else { return }
        let label = tableRow.cells.filter { $0.isEmpty == false }.joined(separator: ", ")
        rowView.setAccessibilityRole(.row)
        rowView.setAccessibilityLabel(label.isEmpty ? nil : label)
        rowView.setAccessibilitySelected(selected)
    }
}
