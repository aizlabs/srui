//
// OutlineCollectionAdapter.swift
// Collections
//
// NSOutlineView data source for Tree. Inline trees keep existing item objects; model-backed
// trees are generated on demand from sparse indices (§8, §22.7).
//
// Spec sections implemented:
// - §8 Collections and model data: a hierarchical model's index space is the depth-first
//   preorder of the whole forest. Item i carries `tree_depth` and `descendant_count`; its subtree
//   occupies indices i+1 ... i+descendant_count. Range requests, MODEL_* operations, promotion
//   and server-pushed ranges keep flat index semantics. A model whose items carry neither
//   property is a flat list of roots and renders exactly as before.
// - §22.5 Local interaction state: disclosure is local. Every item starts expanded; collapsing a
//   cached item with descendants hides its span. The collapsed set is keyed by ItemId, so a model
//   update moves it with its item, and an entry whose item leaves the cache is dropped. A toggle
//   reports EXPANSION_CHANGED (§7.6) through `onExpansionChanged` without awaiting a reply.
// - §22.7 Collections: the outline is one flat level of rows mapped to model indices through the
//   sorted, merged hidden intervals of collapsed items. Uncached indices keep a loading row and
//   asynchronous range requests for visible indices only (through `CollectionRangeTracker`).
//   Indentation is per row, from the cached `tree_depth` and the outline view's
//   `indentationPerLevel`, in one reused outline-column cell with its own disclosure button: no
//   native control per item. An uncached loading row sits at level 0.
// - §6.3 promotion: a server-promoted selection selects the promoted ItemId's row with the same
//   rules as TableCollectionAdapter: replaces local selection, reveals once, posts the
//   accessibility selection notification (§22.8), and never maps an absent ItemId to a row index.
//   A revealing promotion of a hidden item first expands exactly the collapsed items whose hidden
//   spans contain its index, reporting each with EXPANSION_CHANGED.
// - §22.8 Accessibility: each hierarchical row view exposes its disclosure level and state, and
//   the disclosure button is a standard AppKit disclosure control.
// - Columns: a Tree with `columns` uses the Table column identifiers and cell projection, with
//   the first declared column as the outline column and a visible header. Without `columns` it
//   keeps one header-less "tree" column.
//

import AppKit
import SemanticModel

@MainActor
public final class OutlineCollectionAdapter: NSObject, NSOutlineViewDataSource, NSOutlineViewDelegate {
    public static let cellIdentifier = NSUserInterfaceItemIdentifier("srui.outline.cell")
    /// Reuse identifier of the outline-column cell of a hierarchical model (§22.7).
    public static let treeCellIdentifier = NSUserInterfaceItemIdentifier("srui.outline.tree-cell")
    /// Identifier of the single column of a Tree without `columns`.
    public static let treeColumnIdentifier = NSUserInterfaceItemIdentifier("tree")

    public final class Item: NSObject {
        public var title: String
        public let index: Int
        public var itemID: ItemId?
        /// Per-column cells, projected like a Table row (§8). Inline rows default to `[title]`.
        public var cells: [String]

        public init(title: String, index: Int = 0, itemID: ItemId? = nil, cells: [String]? = nil) {
            self.title = title
            self.index = index
            self.itemID = itemID
            self.cells = cells ?? [title]
        }
    }

    /// Reused outline-column cell of a hierarchical row: indentation, disclosure button, label.
    ///
    /// One instance exists per visible row, reused by NSOutlineView; never one per item (§22.7).
    public final class TreeCellView: NSTableCellView {
        public let disclosureButton: NSButton
        public let label: NSTextField
        /// Item the cell currently shows; `nil` for a loading row.
        public internal(set) var itemID: ItemId?
        /// Outline level the cell is indented to.
        public internal(set) var level: Int = 0
        weak var outlineView: NSOutlineView?

        public init(identifier: NSUserInterfaceItemIdentifier) {
            disclosureButton = NSButton(frame: .zero)
            disclosureButton.bezelStyle = .disclosure
            disclosureButton.setButtonType(.pushOnPushOff)
            disclosureButton.title = ""
            disclosureButton.translatesAutoresizingMaskIntoConstraints = false
            label = NSTextField(labelWithString: "")
            label.lineBreakMode = .byTruncatingTail
            label.maximumNumberOfLines = 1
            label.translatesAutoresizingMaskIntoConstraints = false
            label.setAccessibilityElement(false)
            label.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
            super.init(frame: .zero)
            self.identifier = identifier
            addSubview(disclosureButton)
            addSubview(label)
            textField = label
            let indentation = disclosureButton.leadingAnchor.constraint(equalTo: leadingAnchor)
            indentationConstraintStorage = indentation
            NSLayoutConstraint.activate([
                indentation,
                disclosureButton.centerYAnchor.constraint(equalTo: centerYAnchor),
                label.leadingAnchor.constraint(equalTo: disclosureButton.trailingAnchor, constant: 2),
                label.trailingAnchor.constraint(lessThanOrEqualTo: trailingAnchor),
                label.centerYAnchor.constraint(equalTo: centerYAnchor),
            ])
        }

        private var indentationConstraintStorage: NSLayoutConstraint?

        /// Leading inset of the disclosure button: `level × indentationPerLevel`.
        public var indentation: CGFloat { indentationConstraintStorage?.constant ?? 0 }

        @available(*, unavailable)
        required init?(coder: NSCoder) {
            fatalError("init(coder:) is not supported")
        }

        func configure(
            text: String,
            level: Int,
            indentationPerLevel: CGFloat,
            expandable: Bool,
            expanded: Bool,
            itemID: ItemId?
        ) {
            label.stringValue = text
            self.level = level
            self.itemID = itemID
            indentationConstraintStorage?.constant = CGFloat(level) * indentationPerLevel
            // A hidden button still holds its place, so labels of one level stay aligned.
            disclosureButton.isHidden = !expandable
            disclosureButton.isEnabled = expandable
            disclosureButton.state = expanded ? .on : .off
            disclosureButton.setAccessibilityLabel(expandable ? text : nil)
        }
    }

    public let nodeID: NodeId
    public private(set) var items: [Item]
    public private(set) var model: Model?
    public private(set) var modelID: ModelId?
    public var selectionMode: StandardSelectionMode = .none
    public var onSelectionChanged: (@MainActor (NodeId, ItemId) -> Void)?
    public var onRangeRequest: (@MainActor (CollectionRangeRequest) -> Void)?
    /// User-originated selection changes, including an empty selection (§22.5).
    public var onLocalSelection: (@MainActor (NodeId, [ItemId]) -> Void)?
    /// Local disclosure change: `(node, item, expanded)`, reported as EXPANSION_CHANGED (§7.6).
    public var onExpansionChanged: (@MainActor (NodeId, ItemId, Bool) -> Void)?
    /// Every change of the collapsed set, with the model it belongs to, so the renderer can keep
    /// it across remounts (§22.5).
    public var onCollapsedItemsChanged: (@MainActor (NodeId, ModelId?, Set<ItemId>) -> Void)?
    /// Accessibility notification sink for promoted selections (§22.8). Replaceable for tests.
    public var postAccessibilityNotification: @MainActor (NSTableView, NSAccessibility.Notification) -> Void = {
        NSAccessibility.post(element: $0, notification: $1)
    }

    /// Server-promoted selection last applied to this adapter (§6.3).
    public private(set) var promotedItemID: ItemId?
    /// Promotion waiting for its item to reach the replica.
    public private(set) var pendingPromotion: CollectionPromotion?
    /// The user changed the selection after the last promotion (§22.5).
    public private(set) var localSelectionSincePromotion = false

    /// Collapsed items of a hierarchical model, by identity (§22.5). Only cached items.
    public private(set) var collapsedItemIDs: Set<ItemId> = []
    /// Sorted, merged index intervals hidden by collapsed items.
    public private(set) var hiddenIntervals: [Range<UInt64>] = []
    /// `hiddenBefore[j]` is the number of indices hidden by `hiddenIntervals[0..<j]`.
    private var hiddenBefore: [UInt64] = [0]
    /// Column identifiers in declared order when the Tree has `columns`; empty otherwise.
    public private(set) var columnIdentifiers: [NSUserInterfaceItemIdentifier] = []
    /// The Tree's `columns`, or `nil` when it declares none.
    public private(set) var declaredColumns: [String]?
    private var columnsConfigured = false

    /// Nested-scroll chrome. Public so `ControlFactory` (RendererAppKit) can
    /// wire collections that now live in a separate module.
    public var minHeightConstraint: NSLayoutConstraint?
    public var fitHeightConstraint: NSLayoutConstraint?
    public private(set) var isNestedInScroll = false
    private(set) var rangeTracker = CollectionRangeTracker()

    private var outlineLeaves: [Int: Item] = [:]
    private var isSuppressingSelectionEvents = false
    private weak var observedOutlineView: NSOutlineView?
    private weak var observedClipView: NSClipView?
    /// Last visible window, in rows.
    private var lastVisibleStart: UInt64 = 0
    private var lastVisibleCount: UInt64 = 0

    public var isModelBacked: Bool { model != nil }
    /// The model declares a hierarchy (`tree_depth`/`descendant_count`, §8).
    public var isHierarchical: Bool { model?.treeShaped == true }
    public var hasExplicitColumns: Bool { !columnIdentifiers.isEmpty }

    public init(
        nodeID: NodeId = NodeId(0),
        rows: [String] = [],
        model: Model? = nil,
        modelID: ModelId? = nil,
        selectionMode: StandardSelectionMode = .none,
        onSelectionChanged: (@MainActor (NodeId, ItemId) -> Void)? = nil,
        onRangeRequest: (@MainActor (CollectionRangeRequest) -> Void)? = nil
    ) {
        self.nodeID = nodeID
        self.items = rows.enumerated().map { Item(title: $0.element, index: $0.offset) }
        self.model = model
        self.modelID = modelID ?? model?.id
        self.selectionMode = selectionMode
        self.onSelectionChanged = onSelectionChanged
        self.onRangeRequest = onRangeRequest
    }

    public convenience init(rows: [String]) {
        self.init(nodeID: NodeId(0), rows: rows)
    }

    deinit {
        NotificationCenter.default.removeObserver(self)
    }

    // MARK: - Columns

    /// Reconciles a Tree's columns (§8) with the Table column identifiers and titles. With
    /// `columns`, the first declared column is the outline column and the header is shown.
    /// Without, one header-less "tree" column, as a Tree always had.
    ///
    /// NSOutlineView refuses `removeTableColumn(_:)` for its outline column, so that column object
    /// is kept and renamed in place; only the others are reused, added or removed.
    public static func reconcileColumns(in outlineView: NSOutlineView, columns: [String]?) {
        let outlineColumn: NSTableColumn
        if let current = outlineView.outlineTableColumn, outlineView.tableColumns.contains(current) {
            outlineColumn = current
        } else if let first = outlineView.tableColumns.first {
            outlineColumn = first
        } else {
            outlineColumn = NSTableColumn(identifier: treeColumnIdentifier)
            outlineColumn.width = 180
            outlineColumn.minWidth = 120
            outlineColumn.resizingMask = .autoresizingMask
            outlineView.addTableColumn(outlineColumn)
        }
        outlineView.outlineTableColumn = outlineColumn

        var others = outlineView.tableColumns.filter { $0 !== outlineColumn }
        var ordered = [outlineColumn]
        if let columns, !columns.isEmpty {
            let identifiers = TableCollectionAdapter.columnIdentifiers(for: columns)
            outlineColumn.identifier = identifiers[0]
            outlineColumn.title = columns[0]
            for (title, identifier) in zip(columns.dropFirst(), identifiers.dropFirst()) {
                let column: NSTableColumn
                if let match = others.firstIndex(where: { $0.identifier == identifier }) {
                    column = others.remove(at: match)
                } else if !others.isEmpty {
                    column = others.removeFirst()
                    column.identifier = identifier
                } else {
                    column = NSTableColumn(identifier: identifier)
                    column.width = max(80, 360 / CGFloat(columns.count))
                }
                column.title = title
                ordered.append(column)
            }
            if outlineView.headerView == nil {
                outlineView.headerView = NSTableHeaderView()
            }
        } else {
            outlineColumn.identifier = treeColumnIdentifier
            outlineColumn.title = "Tree"
            outlineView.headerView = nil
        }

        for leftover in others {
            outlineView.removeTableColumn(leftover)
        }
        for column in ordered where outlineView.tableColumns.contains(column) == false {
            outlineView.addTableColumn(column)
        }
        for (target, column) in ordered.enumerated() {
            if let current = outlineView.tableColumns.firstIndex(of: column), current != target {
                outlineView.moveColumn(current, toColumn: target)
            }
        }
    }

    /// Applies `columns` to the view and to the cell projection, then repaints. Unchanged
    /// columns are a no-op.
    public func setColumns(_ columns: [String]?, outlineView: NSOutlineView) {
        let declared = columns?.isEmpty == false ? columns : nil
        if columnsConfigured, declared == declaredColumns { return }
        columnsConfigured = true
        declaredColumns = declared
        Self.reconcileColumns(in: outlineView, columns: declared)
        columnIdentifiers = declared.map(TableCollectionAdapter.columnIdentifiers(for:)) ?? []
        outlineView.reloadData()
    }

    // MARK: - Updates

    public func update(rows: [String], outlineView: NSOutlineView) {
        update(rows: rows, cells: nil, outlineView: outlineView)
    }

    /// Inline rows; `cells` (one array per row) feed the columns when the Tree declares them.
    public func update(rows: [String], cells: [[String]]?, outlineView: NSOutlineView) {
        let hadCollapsed = !collapsedItemIDs.isEmpty
        model = nil
        modelID = nil
        rangeTracker.reset()
        outlineLeaves.removeAll(keepingCapacity: false)
        collapsedItemIDs.removeAll()
        rebuildHiddenIntervals()
        items = reusedItems(matching: rows)
        for (index, item) in items.enumerated() {
            item.cells = cells.flatMap { index < $0.count ? $0[index] : nil } ?? [item.title]
        }
        outlineView.reloadData()
        refreshNestedChrome(outlineView: outlineView)
        resolvePendingPromotion(in: outlineView)
        if hadCollapsed {
            onCollapsedItemsChanged?(nodeID, modelID, collapsedItemIDs)
        }
    }

    public func update(model: Model?, modelID: ModelId?, outlineView: NSOutlineView) {
        let selectedItemIDs = selectedItemIDs(in: outlineView)
        let wasSuppressing = isSuppressingSelectionEvents
        isSuppressingSelectionEvents = true
        defer { isSuppressingSelectionEvents = wasSuppressing }

        if let model {
            for range in model.cachedRanges() {
                rangeTracker.noteArrived(start: range.start, count: range.length)
            }
        }
        let collapsedBefore = collapsedItemIDs
        let replaced = self.modelID != (modelID ?? model?.id)
        if replaced {
            rangeTracker.reset()
            outlineLeaves.removeAll(keepingCapacity: false)
            collapsedItemIDs.removeAll()
        }
        self.model = model
        self.modelID = modelID ?? model?.id
        items = []
        pruneCollapsedItems()
        rebuildHiddenIntervals()
        refreshLeafTitles()
        outlineView.reloadData()
        restoreSelection(selectedItemIDs, in: outlineView)
        refreshNestedChrome(outlineView: outlineView)
        if collapsedBefore != collapsedItemIDs {
            onCollapsedItemsChanged?(nodeID, modelID, collapsedItemIDs)
        }
        resolvePendingPromotion(in: outlineView)
        emitVisibleRangeRequests(from: outlineView)
    }

    /// Seeds the collapsed set kept by the renderer across a remount (§22.5). The next model
    /// update drops entries whose items are not cached.
    public func seedCollapsedItems(_ itemIDs: Set<ItemId>) {
        collapsedItemIDs = itemIDs
        pruneCollapsedItems()
        rebuildHiddenIntervals()
    }

    // MARK: - Local disclosure (§7.6, §22.5)

    /// Whether the item is expanded locally. Every item starts expanded.
    public func isExpanded(_ itemID: ItemId) -> Bool {
        !collapsedItemIDs.contains(itemID)
    }

    /// Expands or collapses a cached item with descendants (§22.5).
    ///
    /// The change is local and immediate: rows are remapped in place, the visible window's
    /// uncached indices (only those) are requested, and EXPANSION_CHANGED is reported once
    /// through `onExpansionChanged` without awaiting any reply. Returns `false`, reporting nothing,
    /// when the item is not cached, has no descendants, or is already in that state.
    @discardableResult
    public func setExpanded(_ expanded: Bool, itemID: ItemId, outlineView: NSOutlineView) -> Bool {
        guard let model, model.treeShaped,
              let index = model.indexOf(itemID),
              let item = model.getItemByIndex(index),
              item.descendantCount > 0,
              collapsedItemIDs.contains(itemID) == expanded else {
            return false
        }
        if expanded {
            collapsedItemIDs.remove(itemID)
        } else {
            collapsedItemIDs.insert(itemID)
        }
        applyDisclosureChange(in: outlineView)
        onCollapsedItemsChanged?(nodeID, modelID, collapsedItemIDs)
        onExpansionChanged?(nodeID, itemID, expanded)
        resolvePendingPromotion(in: outlineView)
        emitRequestsAfterDisclosureChange(in: outlineView)
        return true
    }

    @objc func disclosureButtonClicked(_ sender: NSButton) {
        guard let cell = sender.superview as? TreeCellView,
              let itemID = cell.itemID,
              let outlineView = cell.outlineView ?? observedOutlineView else {
            return
        }
        if !setExpanded(sender.state == .on, itemID: itemID, outlineView: outlineView) {
            sender.state = isExpanded(itemID) ? .on : .off
        }
    }

    // MARK: - Row mapping (§8, §22.7)

    /// Rows the outline shows: `itemCount` less every hidden index.
    public var visibleRowCount: UInt64 {
        guard let model else { return UInt64(items.count) }
        let hidden = hiddenBefore.last ?? 0
        return model.itemCount > hidden ? model.itemCount - hidden : 0
    }

    /// Model index shown by `row` (identity when nothing is collapsed).
    public func modelIndex(forRow row: Int) -> UInt64 {
        modelIndex(forRow: UInt64(max(row, 0)))
    }

    /// Row showing model `index`, or `nil` when a collapsed item hides it or it is out of range.
    public func row(forIndex index: UInt64) -> Int? {
        guard let model, index < model.itemCount else { return nil }
        let preceding = intervalCount(startingAtOrBefore: index)
        if preceding > 0, index < hiddenIntervals[preceding - 1].upperBound {
            return nil
        }
        let row = index - hiddenBefore[preceding]
        guard row < UInt64(CollectionCells.clampedRowCount(visibleRowCount)) else { return nil }
        return Int(row)
    }

    /// Outline level of `row`: the cached `tree_depth`, 0 for a loading row or a flat model.
    public func outlineLevel(forRow row: Int) -> Int {
        guard let model, model.treeShaped,
              let item = model.getItemByIndex(modelIndex(forRow: row)) else { return 0 }
        return Int(clamping: item.treeDepth)
    }

    private func modelIndex(forRow row: UInt64) -> UInt64 {
        // Interval j begins after row `lower_j - hiddenBefore[j]`; those rows are strictly
        // increasing because merged intervals never touch.
        var low = 0
        var high = hiddenIntervals.count
        while low < high {
            let mid = (low + high) / 2
            if hiddenIntervals[mid].lowerBound - hiddenBefore[mid] <= row {
                low = mid + 1
            } else {
                high = mid
            }
        }
        return row + hiddenBefore[low]
    }

    /// Number of hidden intervals whose lower bound is at or before `index`.
    private func intervalCount(startingAtOrBefore index: UInt64) -> Int {
        var low = 0
        var high = hiddenIntervals.count
        while low < high {
            let mid = (low + high) / 2
            if hiddenIntervals[mid].lowerBound <= index {
                low = mid + 1
            } else {
                high = mid
            }
        }
        return low
    }

    /// Indices hidden by collapsing `itemID`: `index+1 ..< index+1+descendantCount`.
    private func hiddenSpan(of itemID: ItemId, in model: Model) -> Range<UInt64>? {
        guard let index = model.indexOf(itemID),
              let item = model.getItemByIndex(index),
              item.descendantCount > 0 else { return nil }
        let (start, startOverflow) = index.addingReportingOverflow(1)
        guard !startOverflow else { return nil }
        let (rawEnd, endOverflow) = start.addingReportingOverflow(item.descendantCount)
        let end = min(model.itemCount, endOverflow ? UInt64.max : rawEnd)
        guard start < end else { return nil }
        return start..<end
    }

    private func pruneCollapsedItems() {
        guard let model, model.treeShaped else {
            collapsedItemIDs.removeAll()
            return
        }
        collapsedItemIDs = collapsedItemIDs.filter { model.containsItem($0) }
    }

    /// lc-debt: O(k log k) in collapsed items per update or toggle; upgrade path is an interval
    /// tree updated incrementally per toggle.
    private func rebuildHiddenIntervals() {
        hiddenIntervals = []
        hiddenBefore = [0]
        guard let model, model.treeShaped, !collapsedItemIDs.isEmpty else { return }
        let spans = collapsedItemIDs
            .compactMap { hiddenSpan(of: $0, in: model) }
            .sorted { $0.lowerBound < $1.lowerBound }
        var merged: [Range<UInt64>] = []
        for span in spans {
            if let last = merged.last, span.lowerBound <= last.upperBound {
                merged[merged.count - 1] = last.lowerBound..<max(last.upperBound, span.upperBound)
            } else {
                merged.append(span)
            }
        }
        hiddenIntervals = merged
        var total: UInt64 = 0
        hiddenBefore.reserveCapacity(merged.count + 1)
        for span in merged {
            total += span.upperBound - span.lowerBound
            hiddenBefore.append(total)
        }
    }

    private func applyDisclosureChange(in outlineView: NSOutlineView) {
        let selected = selectedItemIDs(in: outlineView)
        let wasSuppressing = isSuppressingSelectionEvents
        isSuppressingSelectionEvents = true
        defer { isSuppressingSelectionEvents = wasSuppressing }
        rebuildHiddenIntervals()
        outlineView.reloadData()
        restoreSelection(selected, in: outlineView)
        refreshNestedChrome(outlineView: outlineView)
    }

    /// Expands exactly the collapsed items whose hidden spans contain `itemID`'s index (§6.3).
    private func expandCollapsedAncestors(of itemID: ItemId, in outlineView: NSOutlineView) {
        guard let model, let index = model.indexOf(itemID), !collapsedItemIDs.isEmpty else { return }
        let ancestors = collapsedItemIDs
            .filter { hiddenSpan(of: $0, in: model)?.contains(index) == true }
            .sorted { (model.indexOf($0) ?? 0) < (model.indexOf($1) ?? 0) }
        guard !ancestors.isEmpty else { return }
        collapsedItemIDs.subtract(ancestors)
        applyDisclosureChange(in: outlineView)
        onCollapsedItemsChanged?(nodeID, modelID, collapsedItemIDs)
        for ancestor in ancestors {
            onExpansionChanged?(nodeID, ancestor, true)
        }
    }

    // MARK: - Server-promoted selection (§6.3, §7.6, §8, §22.5, §22.7, §22.8)

    /// Applies a promotion or its withdrawal (`nil`) with the same rules as
    /// `TableCollectionAdapter.applyPromotion`: a withdrawal keeps a selection the user made after
    /// the promotion and deselects only an untouched promoted row, emitting no `SELECTION_CHANGED`.
    public func applyPromotion(_ itemID: ItemId?, reveal: Bool, outlineView: NSOutlineView) {
        let previous = promotedItemID
        let userChangedSelection = localSelectionSincePromotion
        promotedItemID = itemID
        pendingPromotion = nil
        localSelectionSincePromotion = false
        guard let itemID else {
            if !userChangedSelection, let previous, let row = row(of: previous),
               outlineView.selectedRowIndexes == IndexSet(integer: row) {
                clearSelectionForPromotion(in: outlineView)
            }
            return
        }
        guard selectionMode != .none else { return }
        pendingPromotion = CollectionPromotion(itemID: itemID, reveal: reveal)
        resolvePendingPromotion(in: outlineView)
    }

    /// Reselects a user's local selection by ItemId without revealing it (§22.5).
    public func restoreLocalSelection(_ itemIDs: [ItemId], outlineView: NSOutlineView) {
        pendingPromotion = nil
        isSuppressingSelectionEvents = true
        defer { isSuppressingSelectionEvents = false }
        restoreSelection(itemIDs, in: outlineView)
    }

    /// Row of `itemID` in a model-backed tree, or `nil` when it is not cached or a collapsed item
    /// hides it. Never a positional guess (§8).
    public func row(of itemID: ItemId) -> Int? {
        guard let model, let index = model.indexOf(itemID) else { return nil }
        return row(forIndex: index)
    }

    private func resolvePendingPromotion(in outlineView: NSOutlineView) {
        guard let pending = pendingPromotion else { return }
        if pending.reveal {
            // Only a revealing promotion opens collapsed ancestors; a restore on an unrelated
            // commit never undoes the user's collapse (§22.5).
            expandCollapsedAncestors(of: pending.itemID, in: outlineView)
        }
        guard let row = row(of: pending.itemID) else {
            clearSelectionForPromotion(in: outlineView)
            return
        }
        pendingPromotion = nil
        isSuppressingSelectionEvents = true
        outlineView.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
        isSuppressingSelectionEvents = false
        if pending.reveal {
            CollectionPromotion.reveal(row: row, in: outlineView)
        }
        postAccessibilityNotification(outlineView, .selectedRowsChanged)
    }

    private func clearSelectionForPromotion(in outlineView: NSOutlineView) {
        guard outlineView.selectedRowIndexes.isEmpty == false else { return }
        isSuppressingSelectionEvents = true
        defer { isSuppressingSelectionEvents = false }
        outlineView.deselectAll(nil)
        postAccessibilityNotification(outlineView, .selectedRowsChanged)
    }

    // MARK: - Viewport and range requests (§8, §22.7)

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

    /// Treats rows `start..<start+count` as the visible window (tests, and no live clip view).
    public func noteVisibleRange(start: UInt64, count: UInt64) {
        emitRequests(visibleStart: start, visibleCount: count)
    }

    public func setNestedInScroll(_ nested: Bool, scrollView: NSScrollView, outlineView: NSOutlineView) {
        isNestedInScroll = nested
        CollectionScrollEmbedding.apply(
            nested: nested,
            modelBacked: isModelBacked,
            scrollView: scrollView,
            minHeightConstraint: minHeightConstraint,
            fitHeightConstraint: &fitHeightConstraint,
            contentHeight: CollectionScrollEmbedding.tableContentHeight(outlineView)
        )
        attachViewportObservation(scrollView: scrollView, outlineView: outlineView)
    }

    public func attachViewportObservation(scrollView: NSScrollView, outlineView: NSOutlineView) {
        observedOutlineView = outlineView
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
        emitVisibleRangeRequests(from: outlineView)
    }

    @objc private func clipViewBoundsDidChange(_ notification: Notification) {
        emitVisibleRangeRequests()
    }

    // MARK: - NSOutlineViewDataSource / Delegate

    public func outlineView(
        _ outlineView: NSOutlineView,
        numberOfChildrenOfItem item: Any?
    ) -> Int {
        guard item == nil else { return 0 }
        if model != nil {
            return CollectionCells.clampedRowCount(visibleRowCount)
        }
        return items.count
    }

    public func outlineView(
        _ outlineView: NSOutlineView,
        child index: Int,
        ofItem item: Any?
    ) -> Any {
        if model != nil {
            return leaf(at: Int(clamping: modelIndex(forRow: index)))
        }
        return items[index]
    }

    /// Always `false`: rows are one native level, and disclosure is the adapter's own (§22.5).
    public func outlineView(_ outlineView: NSOutlineView, isItemExpandable item: Any) -> Bool {
        false
    }

    public func outlineView(
        _ outlineView: NSOutlineView,
        viewFor tableColumn: NSTableColumn?,
        item: Any
    ) -> NSView? {
        guard let item = item as? Item else { return nil }
        var cached: ModelItem?
        if let model, let hit = model.getItemByIndex(UInt64(item.index)) {
            cached = hit
            item.title = CollectionCells.cellString(from: hit.value)
            item.itemID = hit.itemID
            item.cells = CollectionCells.cells(from: hit.value)
        } else if model != nil {
            item.title = CollectionCells.loadingPlaceholder
            item.itemID = nil
            item.cells = Array(
                repeating: CollectionCells.loadingPlaceholder,
                count: max(1, columnIdentifiers.count)
            )
        }

        let isOutlineColumn = tableColumn == nil || tableColumn === outlineView.outlineTableColumn
        let text: String
        if hasExplicitColumns {
            let column = columnIndex(of: tableColumn, in: outlineView)
            text = column < item.cells.count ? item.cells[column] : ""
        } else {
            text = item.title
        }

        if isOutlineColumn, isHierarchical {
            let cell = (outlineView.makeView(withIdentifier: Self.treeCellIdentifier, owner: self) as? TreeCellView)
                ?? TreeCellView(identifier: Self.treeCellIdentifier)
            let expandable = (cached?.descendantCount ?? 0) > 0
            cell.outlineView = outlineView
            cell.configure(
                text: text,
                level: cached.map { Int(clamping: $0.treeDepth) } ?? 0,
                indentationPerLevel: outlineView.indentationPerLevel,
                expandable: expandable,
                expanded: item.itemID.map(isExpanded) ?? true,
                itemID: item.itemID
            )
            cell.disclosureButton.target = self
            cell.disclosureButton.action = #selector(disclosureButtonClicked(_:))
            return cell
        }

        let identifier = isOutlineColumn ? Self.cellIdentifier : (tableColumn?.identifier ?? Self.cellIdentifier)
        let field: NSTextField
        if let reused = outlineView.makeView(withIdentifier: identifier, owner: self) as? NSTextField {
            field = reused
        } else {
            field = NSTextField(labelWithString: "")
            field.identifier = identifier
            field.lineBreakMode = .byTruncatingTail
            field.maximumNumberOfLines = 1
        }
        field.stringValue = text
        field.setAccessibilityElement(false)
        return field
    }

    public func outlineView(_ outlineView: NSOutlineView, didAdd rowView: NSTableRowView, forRow row: Int) {
        guard isHierarchical, let model else { return }
        let cached = model.getItemByIndex(modelIndex(forRow: row))
        rowView.setAccessibilityDisclosureLevel(cached.map { Int(clamping: $0.treeDepth) } ?? 0)
        // A reused row view must not keep the previous row's state.
        let expandable = (cached?.descendantCount ?? 0) > 0
        rowView.setAccessibilityDisclosed(expandable && cached.map { isExpanded($0.itemID) } == true)
    }

    public func outlineView(_ outlineView: NSOutlineView, shouldSelectItem item: Any) -> Bool {
        guard selectionMode != .none else { return false }
        guard let item = item as? Item else { return false }
        return item.itemID != nil
    }

    public func outlineViewSelectionDidChange(_ notification: Notification) {
        guard !isSuppressingSelectionEvents else { return }
        guard selectionMode != .none else { return }
        guard let outlineView = notification.object as? NSOutlineView else { return }
        let selected = outlineView.selectedRowIndexes.compactMap { row -> ItemId? in
            guard row >= 0 else { return nil }
            let item = outlineView.item(atRow: row) as? Item
            return item?.itemID
        }
        pendingPromotion = nil
        localSelectionSincePromotion = true
        onLocalSelection?(nodeID, selected)
        for itemID in selected {
            onSelectionChanged?(nodeID, itemID)
        }
    }

    // MARK: - Private

    /// Position of `tableColumn` in the declared columns, by identifier, so a user's column drag
    /// never changes which cell a column shows.
    private func columnIndex(of tableColumn: NSTableColumn?, in outlineView: NSOutlineView) -> Int {
        guard let tableColumn else { return 0 }
        if let declared = columnIdentifiers.firstIndex(of: tableColumn.identifier) {
            return declared
        }
        return outlineView.tableColumns.firstIndex(of: tableColumn) ?? 0
    }

    private func leaf(at index: Int) -> Item {
        if let existing = outlineLeaves[index] {
            return existing
        }
        let title: String
        let itemID: ItemId?
        var cells: [String]?
        if let model, let cached = model.getItemByIndex(UInt64(index)) {
            title = CollectionCells.cellString(from: cached.value)
            itemID = cached.itemID
            cells = CollectionCells.cells(from: cached.value)
        } else {
            title = CollectionCells.loadingPlaceholder
            itemID = nil
        }
        let item = Item(title: title, index: index, itemID: itemID, cells: cells)
        outlineLeaves[index] = item
        return item
    }

    private func refreshLeafTitles() {
        guard let model else { return }
        for (index, item) in outlineLeaves {
            if let cached = model.getItemByIndex(UInt64(index)) {
                item.title = CollectionCells.cellString(from: cached.value)
                item.itemID = cached.itemID
                item.cells = CollectionCells.cells(from: cached.value)
            } else {
                item.title = CollectionCells.loadingPlaceholder
                item.itemID = nil
                item.cells = [CollectionCells.loadingPlaceholder]
            }
        }
    }

    private func emitVisibleRangeRequests(from outlineView: NSOutlineView? = nil) {
        let outlineView = outlineView ?? observedOutlineView
        guard let outlineView else { return }
        let rows = outlineView.rows(in: outlineView.visibleRect)
        guard rows.location != NSNotFound, rows.length > 0 else { return }
        emitRequests(visibleStart: UInt64(rows.location), visibleCount: UInt64(rows.length))
    }

    /// After a toggle the same rows show other indices; request only those still uncached.
    private func emitRequestsAfterDisclosureChange(in outlineView: NSOutlineView) {
        let rows = outlineView.rows(in: outlineView.visibleRect)
        if rows.location != NSNotFound, rows.length > 0 {
            emitRequests(visibleStart: UInt64(rows.location), visibleCount: UInt64(rows.length))
        } else if lastVisibleCount > 0 {
            emitRequests(visibleStart: lastVisibleStart, visibleCount: lastVisibleCount)
        }
    }

    /// `visibleStart`/`visibleCount` are rows. Without hidden intervals rows are indices and the
    /// single-window path runs unchanged.
    private func emitRequests(visibleStart: UInt64, visibleCount: UInt64) {
        lastVisibleStart = visibleStart
        lastVisibleCount = visibleCount
        guard let model, let modelID, let onRangeRequest else { return }
        let requests: [CollectionRangeRequest]
        if hiddenIntervals.isEmpty {
            if let window = CollectionRangeTracker.alignedWindow(
                visibleStart: visibleStart,
                visibleCount: visibleCount,
                itemCount: model.itemCount
            ) {
                pruneLeaves(keeping: [window])
            }
            requests = rangeTracker.requests(
                visibleStart: visibleStart,
                visibleCount: visibleCount,
                itemCount: model.itemCount,
                model: model,
                nodeID: nodeID,
                modelID: modelID
            )
        } else {
            let windows = visibleWindows(visibleStart: visibleStart, visibleCount: visibleCount, model: model)
            pruneLeaves(keeping: windows)
            requests = rangeTracker.requests(
                windows: windows,
                model: model,
                nodeID: nodeID,
                modelID: modelID
            )
        }
        for request in requests {
            onRangeRequest(request)
        }
    }

    /// Page-aligned windows around the visible rows' index runs, without hidden indices (§22.7).
    private func visibleWindows(visibleStart: UInt64, visibleCount: UInt64, model: Model) -> [Range<UInt64>] {
        let rowCount = visibleRowCount
        guard rowCount > 0, visibleCount > 0 else { return [] }
        let margin = CollectionRangeTracker.prefetchMargin
        let (rawEnd, overflow) = visibleStart.addingReportingOverflow(visibleCount)
        let visibleEnd = min(rowCount, overflow ? UInt64.max : rawEnd)
        let start = visibleStart > margin ? visibleStart - margin : 0
        let end = min(rowCount, visibleEnd > UInt64.max - margin ? UInt64.max : visibleEnd + margin)
        guard start < end else { return [] }

        var segments: [Range<UInt64>] = []
        var row = start
        while row < end {
            let first = modelIndex(forRow: row)
            let following = intervalCount(startingAtOrBefore: first)
            let runEnd = following < hiddenIntervals.count ? hiddenIntervals[following].lowerBound : model.itemCount
            let length = min(runEnd - first, end - row)
            guard length > 0 else { break }
            segments.append(first..<(first + length))
            row += length
        }
        return CollectionRangeTracker.alignedWindows(
            visibleSegments: segments,
            hidden: hiddenIntervals,
            itemCount: model.itemCount
        )
    }

    private func pruneLeaves(keeping windows: [Range<UInt64>]) {
        outlineLeaves = outlineLeaves.filter { entry in
            let index = UInt64(entry.key)
            return windows.contains { $0.contains(index) }
        }
    }

    private func selectedItemIDs(in outlineView: NSOutlineView) -> [ItemId] {
        outlineView.selectedRowIndexes.compactMap { row -> ItemId? in
            (outlineView.item(atRow: row) as? Item)?.itemID
        }
    }

    private func restoreSelection(_ selectedItemIDs: [ItemId], in outlineView: NSOutlineView) {
        if selectedItemIDs.isEmpty {
            outlineView.deselectAll(nil)
            return
        }
        var newRows = IndexSet()
        if model != nil {
            for itemID in selectedItemIDs {
                if let row = row(of: itemID) {
                    newRows.insert(row)
                }
            }
        }
        outlineView.selectRowIndexes(newRows, byExtendingSelection: false)
    }

    private func refreshNestedChrome(outlineView: NSOutlineView) {
        if isNestedInScroll, let scrollView = outlineView.enclosingScrollView {
            setNestedInScroll(true, scrollView: scrollView, outlineView: outlineView)
        }
    }

    private func reusedItems(matching titles: [String]) -> [Item] {
        var unused = items
        return titles.enumerated().map { index, title in
            if let match = unused.firstIndex(where: { $0.title == title }) {
                let item = unused.remove(at: match)
                return item
            }
            return Item(title: title, index: index)
        }
    }
}
