//
// OutlineCollectionAdapter.swift
// Collections
//
// NSOutlineView data source for Tree. Inline trees keep existing item objects; model-backed
// trees are a flat root-level outline generated on demand from sparse indices (§8).
//

import AppKit
import SemanticModel

@MainActor
public final class OutlineCollectionAdapter: NSObject, NSOutlineViewDataSource, NSOutlineViewDelegate {
    public static let cellIdentifier = NSUserInterfaceItemIdentifier("srui.outline.cell")

    public final class Item: NSObject {
        public var title: String
        public let index: Int
        public var itemID: ItemId?

        public init(title: String, index: Int = 0, itemID: ItemId? = nil) {
            self.title = title
            self.index = index
            self.itemID = itemID
        }
    }

    public let nodeID: NodeId
    public private(set) var items: [Item]
    public private(set) var model: Model?
    public private(set) var modelID: ModelId?
    public var selectionMode: StandardSelectionMode = .none
    public var onSelectionChanged: (@MainActor (NodeId, ItemId) -> Void)?
    public var onRangeRequest: (@MainActor (CollectionRangeRequest) -> Void)?

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
    private var lastVisibleStart: UInt64 = 0
    private var lastVisibleCount: UInt64 = 0

    public var isModelBacked: Bool { model != nil }

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

    public func update(rows: [String], outlineView: NSOutlineView) {
        model = nil
        modelID = nil
        rangeTracker.reset()
        outlineLeaves.removeAll(keepingCapacity: false)
        items = reusedItems(matching: rows)
        outlineView.reloadData()
        refreshNestedChrome(outlineView: outlineView)
    }

    public func update(model: Model?, modelID: ModelId?, outlineView: NSOutlineView) {
        let selectedItemIDs = outlineView.selectedRowIndexes.compactMap { row -> ItemId? in
            (outlineView.item(atRow: row) as? Item)?.itemID
        }
        isSuppressingSelectionEvents = true
        defer { isSuppressingSelectionEvents = false }

        if let model {
            for range in model.cachedRanges() {
                rangeTracker.noteArrived(start: range.start, count: range.length)
            }
        }
        let replaced = self.modelID != (modelID ?? model?.id)
        if replaced {
            rangeTracker.reset()
            outlineLeaves.removeAll(keepingCapacity: false)
        }
        self.model = model
        self.modelID = modelID ?? model?.id
        items = []
        refreshLeafTitles()
        outlineView.reloadData()
        restoreSelection(selectedItemIDs, in: outlineView)
        refreshNestedChrome(outlineView: outlineView)
        emitVisibleRangeRequests(from: outlineView)
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

    public func outlineView(
        _ outlineView: NSOutlineView,
        numberOfChildrenOfItem item: Any?
    ) -> Int {
        guard item == nil else { return 0 }
        if let model {
            return CollectionCells.clampedRowCount(model.itemCount)
        }
        return items.count
    }

    public func outlineView(
        _ outlineView: NSOutlineView,
        child index: Int,
        ofItem item: Any?
    ) -> Any {
        if model != nil {
            return leaf(at: index)
        }
        return items[index]
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
        if let model, let cached = model.getItemByIndex(UInt64(item.index)) {
            item.title = CollectionCells.cellString(from: cached.value)
            item.itemID = cached.itemID
        } else if model != nil {
            item.title = CollectionCells.loadingPlaceholder
            item.itemID = nil
        }
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
        for itemID in selected {
            onSelectionChanged?(nodeID, itemID)
        }
    }

    private func leaf(at index: Int) -> Item {
        if let existing = outlineLeaves[index] {
            return existing
        }
        let title: String
        let itemID: ItemId?
        if let model, let cached = model.getItemByIndex(UInt64(index)) {
            title = CollectionCells.cellString(from: cached.value)
            itemID = cached.itemID
        } else {
            title = CollectionCells.loadingPlaceholder
            itemID = nil
        }
        let item = Item(title: title, index: index, itemID: itemID)
        outlineLeaves[index] = item
        return item
    }

    private func refreshLeafTitles() {
        guard let model else { return }
        for (index, item) in outlineLeaves {
            if let cached = model.getItemByIndex(UInt64(index)) {
                item.title = CollectionCells.cellString(from: cached.value)
                item.itemID = cached.itemID
            } else {
                item.title = CollectionCells.loadingPlaceholder
                item.itemID = nil
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

    private func emitRequests(visibleStart: UInt64, visibleCount: UInt64) {
        lastVisibleStart = visibleStart
        lastVisibleCount = visibleCount
        guard let model, let modelID, let onRangeRequest else { return }
        if let window = CollectionRangeTracker.alignedWindow(
            visibleStart: visibleStart,
            visibleCount: visibleCount,
            itemCount: model.itemCount
        ) {
            pruneLeaves(keeping: window)
        }
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

    private func pruneLeaves(keeping window: Range<UInt64>) {
        outlineLeaves = outlineLeaves.filter { window.contains(UInt64($0.key)) }
    }

    private func restoreSelection(_ selectedItemIDs: [ItemId], in outlineView: NSOutlineView) {
        if selectedItemIDs.isEmpty {
            outlineView.deselectAll(nil)
            return
        }
        var newIndices = IndexSet()
        if let model {
            for (index, item) in model.iterCachedItems() {
                if selectedItemIDs.contains(item.itemID), index <= UInt64(Int.max) {
                    newIndices.insert(Int(index))
                }
            }
        }
        outlineView.selectRowIndexes(newIndices, byExtendingSelection: false)
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
