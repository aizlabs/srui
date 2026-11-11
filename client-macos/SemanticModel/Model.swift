//
// Model.swift
// SemanticModel
//
// Collection models and sparse cached items for virtualized data views (§8, §13).
// Platform-neutral: NEVER import AppKit or Cocoa in this file.
//
// Architectural Invariants (§4, §8, §13):
// - §8 Collections and Model Data: Large collection data is not represented as thousands
//   of individual child view nodes. Instead, a List, Table, or Tree node references a
//   Model containing stable ItemIds, total collection item_count (which may be millions),
//   and a sparse representation of currently cached item ranges.
// - §13 Core Mutation Operations: Collection data is mutated via five standard model operations:
//   CREATE_MODEL, MODEL_INSERT, MODEL_DELETE, MODEL_UPDATE, and MODEL_RESET_RANGE.
// - Stable Identity: Items are identified by stable ItemId values, supporting mutation
//   by item identity independently of display index or scroll position.
// - §8 Hierarchical models, §26 limits: a Tree model's index space is the depth-first preorder of
//   the whole forest. The standard item properties `tree_depth` and `descendant_count` carry its
//   shape; `validateTreeShape` checks the cached items against it and `maxModelTreeDepth` with the
//   same rules, order and conformance codes as the Rust `Model::validate_tree_shape`.
//

import Foundation

// MARK: - Model Item (§8, §13)

/// An item within a collection data model (§8, §13).
public struct ModelItem: Equatable, Sendable, CustomStringConvertible {
    /// Stable identifier for this item within the model lifetime (§8).
    public var itemID: ItemId

    /// Primary scalar or record value of the item (§6.5).
    public var value: Value

    /// Defined properties/attributes on this item (e.g. column values, metadata).
    public var properties: [PropertyRef: Value]

    /// Constructs a new `ModelItem` with the specified ID, value, and properties dictionary.
    public init(
        itemID: ItemId,
        value: Value,
        properties: [PropertyRef: Value] = [:]
    ) {
        self.itemID = itemID
        self.value = value
        self.properties = properties
    }

    /// Constructs a new `ModelItem` with property pairs.
    public init(
        itemID: ItemId,
        value: Value,
        properties: [(PropertyRef, Value)]
    ) {
        self.itemID = itemID
        self.value = value
        var dict: [PropertyRef: Value] = [:]
        dict.reserveCapacity(properties.count)
        for (k, v) in properties {
            dict[k] = v
        }
        self.properties = dict
    }

    /// Constructs a new `ModelItem` with an array of `Property` structs.
    public init(
        itemID: ItemId,
        value: Value,
        properties: [Property]
    ) {
        self.itemID = itemID
        self.value = value
        var dict: [PropertyRef: Value] = [:]
        dict.reserveCapacity(properties.count)
        for p in properties {
            dict[p.property] = p.value
        }
        self.properties = dict
    }

    /// Returns the property value if defined on this item.
    public func getProperty(_ prop: PropertyRef) -> Value? {
        properties[prop]
    }

    /// Returns `true` if the item has a defined value for the specified property.
    public func hasProperty(_ prop: PropertyRef) -> Bool {
        properties[prop] != nil
    }

    /// Sets a property on this item.
    public mutating func setProperty(_ prop: PropertyRef, value: Value) {
        properties[prop] = value
    }

    /// Clears a property from this item, returning the previous value if set.
    @discardableResult
    public mutating func clearProperty(_ prop: PropertyRef) -> Value? {
        properties.removeValue(forKey: prop)
    }

    public var description: String {
        "ModelItem(id: \(itemID), value: \(value), properties: \(properties.count))"
    }

    /// `true` when the item carries one of the hierarchical shape properties (§8).
    var carriesTreeShape: Bool {
        properties[.treeDepth] != nil || properties[.descendantCount] != nil
    }

    /// The item's `tree_depth` (0 for a root, and when absent), as declared by the server (§8).
    /// A malformed value reads as 0 here; the store refuses it before it is committed.
    public var treeDepth: UInt64 {
        (try? treeShapeValue(.treeDepth).get()) ?? 0
    }

    /// The item's `descendant_count` (0 when absent): its subtree occupies the next
    /// `descendantCount` indices of the preorder (§8).
    public var descendantCount: UInt64 {
        (try? treeShapeValue(.descendantCount).get()) ?? 0
    }

    /// Reads one hierarchical shape property; absent is 0, and only a non-negative integer is a
    /// shape (§8).
    func treeShapeValue(_ property: PropertyRef) -> Result<UInt64, ModelTreeViolation> {
        switch properties[property] {
        case nil:
            return .success(0)
        case .unsignedInt(let value)?:
            return .success(value)
        case .signedInt(let value)? where value >= 0:
            return .success(UInt64(value))
        default:
            return .failure(.invalidProperty(property: lookupStandardPropertyName(property.localID) ?? "tree property"))
        }
    }
}

// MARK: - Hierarchical Model Shape (§8, §26)

/// How a hierarchical model's cached items break the preorder shape that `tree_depth` and
/// `descendant_count` declare (§8, §26). Mirrors the Rust `ModelTreeViolation`, code for code.
public enum ModelTreeViolation: Error, Equatable, Sendable, CustomStringConvertible {
    /// `tree_depth` or `descendant_count` is not a non-negative integer.
    case invalidProperty(property: String)
    /// `index + descendant_count` reaches or passes `item_count`.
    case descendantCountOutOfRange(descendantCount: UInt64, itemCount: UInt64)
    /// No valid preorder reaches the item's depth through the preceding indices: depth grows by
    /// at most one level per index, index 0 is a root, and the position right after a cached
    /// item's span is no deeper than that item. `deepestReachable` is the deepest depth allowed.
    case depthJump(treeDepth: UInt64, deepestReachable: UInt64)
    /// The item lies inside a cached item's span without being deeper than it, its own span runs
    /// past that span, or it directly follows a span — including a leaf's empty span — while being
    /// deeper than the span's owner.
    case outsideParentSpan(parentIndex: UInt64)

    /// Canonical conformance code (§32), identical to the Rust store's.
    public var conformanceCode: String {
        switch self {
        case .invalidProperty: return "invalid_model_tree_property"
        case .descendantCountOutOfRange: return "model_tree_descendant_count_out_of_range"
        case .depthJump: return "model_tree_depth_jump"
        case .outsideParentSpan: return "model_tree_outside_parent_span"
        }
    }

    public var description: String {
        switch self {
        case .invalidProperty(let property):
            return "\(property) is not a non-negative integer"
        case .descendantCountOutOfRange(let descendantCount, let itemCount):
            return "descendant_count \(descendantCount) runs past item_count \(itemCount)"
        case .depthJump(let treeDepth, let deepestReachable):
            return "tree_depth \(treeDepth) is deeper than the preceding items allow (at most \(deepestReachable))"
        case .outsideParentSpan(let parentIndex):
            return "item breaks the subtree span of the item at index \(parentIndex)"
        }
    }
}

// MARK: - Collection Model (§8, §13)

/// A collection data model maintaining total logical count and sparse cached items (§8, §13).
public struct Model: Equatable, Sendable, CustomStringConvertible {
    /// Unique identifier for this model within the session (§6.2, §8).
    public let id: ModelId

    /// Semantic type reference for this model (e.g. List, Table, Tree, or custom).
    public let modelType: TypeRef

    /// Total logical item count in the collection (may far exceed cached item count, §8).
    public internal(set) var itemCount: UInt64

    /// Sparse map of cached items keyed by their 0-based collection index.
    public internal(set) var items: [UInt64: ModelItem]

    /// Reverse lookup mapping each cached item's stable `ItemId` to its index.
    public internal(set) var idToIndex: [ItemId: UInt64]

    /// Set once any item carrying `tree_depth` or `descendant_count` has been written, and kept
    /// for the model's lifetime: a model that never held one is a flat list of roots (§8).
    public internal(set) var treeShaped: Bool

    /// Constructs a new empty collection model with specified item count (§8, §13).
    public init(id: ModelId, modelType: TypeRef, itemCount: UInt64) {
        self.id = id
        self.modelType = modelType
        self.itemCount = itemCount
        self.items = [:]
        self.idToIndex = [:]
        self.treeShaped = false
    }

    private mutating func noteTreeShape(_ incoming: [ModelItem]) {
        if !treeShaped && incoming.contains(where: { $0.carriesTreeShape }) {
            treeShaped = true
        }
    }

    /// Checks the cached items against the preorder shape their `tree_depth` and
    /// `descendant_count` declare, and against `maxModelTreeDepth` (§8, §26).
    ///
    /// Exact for sparse caches, with the same rules, order and codes as the Rust
    /// `Model::validate_tree_shape`: it accepts precisely the caches that some valid forest of
    /// `itemCount` items agrees with on every cached item. Per item, in order:
    /// 1. both properties are non-negative integers (absent is 0);
    /// 2. `tree_depth < maxModelTreeDepth`, and an item with descendants leaves room for them;
    /// 3. `index + descendant_count < itemCount`;
    /// 4. the depth is reachable through the preceding indices, one level per index at most
    ///    (index 0 is a root), honouring every earlier span's after-span bound;
    /// 5. directly after a cached item's span (a leaf's span is empty) the item is no deeper than
    ///    that item; inside the nearest enclosing cached span it is deeper than its owner and its
    ///    own span ends inside.
    ///
    /// lc-debt: O(n log n) in cached items (sorted keys) per touched hierarchical model per
    /// transaction; upgrade path is a check windowed around the touched index ranges.
    public func validateTreeShape(maxModelTreeDepth: Int) throws {
        guard treeShaped else { return }
        func fail(_ index: UInt64, _ violation: ModelTreeViolation) -> StoreError {
            .invalidModelTree(modelID: id, index: index, violation: violation)
        }
        let limit = max(maxModelTreeDepth, 0)
        /// Deepest depth reachable at `target` walking one level per index from `reach` at `pos`
        /// (`nil` is the position before index 0), capped below the limit.
        func walk(_ reach: Int, from pos: UInt64?, to target: UInt64) -> Int {
            let steps = pos.map { target - $0 } ?? target &+ 1
            let capped = steps >= UInt64(limit) ? limit : Int(steps)
            return min(limit - 1, reach + capped)
        }
        // Spans of cached items as (index, depth, last index of the span); a leaf's span ends at
        // itself. Ends are non-increasing towards the top and depths strictly increasing.
        var open: [(index: UInt64, depth: UInt64, end: UInt64)] = []
        var pos: UInt64? = nil
        var reachAtPos = -1
        for index in items.keys.sorted() {
            guard let item = items[index] else { continue }
            let depth: UInt64
            let descendants: UInt64
            switch (item.treeShapeValue(.treeDepth), item.treeShapeValue(.descendantCount)) {
            case (.failure(let violation), _), (.success, .failure(let violation)):
                throw fail(index, violation)
            case (.success(let d), .success(let c)):
                depth = d
                descendants = c
            }
            if depth >= UInt64(limit) || (descendants > 0 && depth + 1 >= UInt64(limit)) {
                throw StoreError.maxModelTreeDepthExceeded(
                    modelID: id, index: index, limit: maxModelTreeDepth, treeDepth: depth
                )
            }
            let remaining = itemCount > index ? itemCount - index : 0
            if descendants >= remaining {
                throw fail(index, .descendantCountOutOfRange(
                    descendantCount: descendants, itemCount: itemCount
                ))
            }
            // Close every span that ends before `index`; the position right after each is no
            // deeper than its owner. At `index` itself that bound applies to this item.
            var boundHere: (index: UInt64, depth: UInt64)?
            while let top = open.last, top.end < index {
                let after = top.end + 1
                var bound: (index: UInt64, depth: UInt64)?
                while let owner = open.last, owner.end == top.end {
                    if bound == nil || owner.depth < bound!.depth {
                        bound = (index: owner.index, depth: owner.depth)
                    }
                    open.removeLast()
                }
                if after == index {
                    boundHere = bound
                } else if let bound {
                    reachAtPos = min(walk(reachAtPos, from: pos, to: after), Int(bound.depth))
                    pos = after
                }
            }
            let reachable = walk(reachAtPos, from: pos, to: index)
            if Int(depth) > reachable {
                throw fail(index, .depthJump(treeDepth: depth, deepestReachable: UInt64(max(reachable, 0))))
            }
            if let boundHere, depth > boundHere.depth {
                throw fail(index, .outsideParentSpan(parentIndex: boundHere.index))
            }
            // `descendants < itemCount - index`, so the span end cannot overflow.
            let end = index + descendants
            if let owner = open.last, depth <= owner.depth || end > owner.end {
                throw fail(index, .outsideParentSpan(parentIndex: owner.index))
            }
            open.append((index: index, depth: depth, end: end))
            pos = index
            reachAtPos = Int(depth)
        }
    }

    /// Constructs a model pre-populated with an initial contiguous range of items.
    public static func withItems(
        id: ModelId,
        modelType: TypeRef,
        itemCount: UInt64,
        startIndex: UInt64,
        items: [ModelItem]
    ) throws -> Model {
        var model = Model(id: id, modelType: modelType, itemCount: itemCount)
        try model.resetRange(startIndex: startIndex, items: items, totalCount: nil)
        return model
    }

    // MARK: - Inspection & Queries

    /// Returns the number of currently cached items in memory.
    public var cachedItemCount: Int {
        items.count
    }

    /// Returns `true` if the model has zero total items.
    public var isEmpty: Bool {
        itemCount == 0
    }

    /// Returns `true` if the given `ItemId` is present in the cache.
    public func containsItem(_ itemID: ItemId) -> Bool {
        idToIndex[itemID] != nil
    }

    /// Returns `true` if the given index is currently cached.
    public func containsIndex(_ index: UInt64) -> Bool {
        items[index] != nil
    }

    /// Drops up to `count` cached items whose indices lie outside `[keepStart, keepEnd)`.
    /// Farthest from the keep-window midpoint are removed first (§8, §26).
    mutating func evictFarthestOutside(keepStart: UInt64, keepEnd: UInt64, count: Int) {
        guard count > 0 else { return }
        let keepEnd = max(keepEnd, keepStart)
        let mid = keepStart + (keepEnd - keepStart) / 2
        let candidates = items.keys.filter { $0 < keepStart || $0 >= keepEnd }.sorted { a, b in
            let da = a > mid ? a - mid : mid - a
            let db = b > mid ? b - mid : mid - b
            if da != db { return da > db }
            return a < b
        }
        for index in candidates.prefix(count) {
            if let item = items.removeValue(forKey: index) {
                idToIndex.removeValue(forKey: item.itemID)
            }
        }
    }

    /// Returns the current index for a cached `ItemId`, if present.
    public func indexOf(_ itemID: ItemId) -> UInt64? {
        idToIndex[itemID]
    }

    /// Returns a cached item by its stable `ItemId`.
    public func getItemById(_ itemID: ItemId) -> ModelItem? {
        guard let idx = idToIndex[itemID] else { return nil }
        return items[idx]
    }

    /// Returns a cached item by its 0-based collection index.
    public func getItemByIndex(_ index: UInt64) -> ModelItem? {
        items[index]
    }

    /// Returns all currently cached (index, item) pairs in ascending index order.
    public func iterCachedItems() -> [(UInt64, ModelItem)] {
        items.keys.sorted().compactMap { idx in
            guard let item = items[idx] else { return nil }
            return (idx, item)
        }
    }

    /// Computes the list of contiguous index ranges currently cached in memory (§8).
    public func cachedRanges() -> [SemanticRange] {
        var ranges: [SemanticRange] = []
        var currentRange: SemanticRange? = nil

        for idx in items.keys.sorted() {
            if var r = currentRange {
                if r.start + r.length == idx {
                    r.length += 1
                    currentRange = r
                } else {
                    ranges.append(r)
                    currentRange = SemanticRange(start: idx, length: 1)
                }
            } else {
                currentRange = SemanticRange(start: idx, length: 1)
            }
        }

        if let r = currentRange {
            ranges.append(r)
        }

        return ranges
    }

    public var description: String {
        "Model(id: \(id), type: \(modelType), itemCount: \(itemCount), cached: \(cachedItemCount))"
    }

    // MARK: - Mutations (§8, §13)

    /// Shifts all cached item indices starting at `>= fromIndex` by signed `delta`.
    public mutating func shiftCachedIndices(fromIndex: UInt64, delta: Int64) {
        if delta == 0 { return }

        var toShift: [(UInt64, ModelItem)] = []
        for (idx, item) in items where idx >= fromIndex {
            toShift.append((idx, item))
        }

        for (oldIdx, item) in toShift {
            items.removeValue(forKey: oldIdx)
            idToIndex.removeValue(forKey: item.itemID)
        }

        for (oldIdx, item) in toShift {
            let newIdx: UInt64
            if delta > 0 {
                newIdx = oldIdx + UInt64(delta)
            } else {
                newIdx = oldIdx - UInt64(-delta)
            }
            idToIndex[item.itemID] = newIdx
            items[newIdx] = item
        }
    }

    /// Inserts items into the model at the specified index, shifting subsequent items (§13 MODEL_INSERT).
    public mutating func insertItems(index: UInt64, items: [ModelItem]) throws {
        if index > itemCount {
            throw StoreError.modelIndexOutOfBounds(index: index, count: itemCount)
        }

        // Validate uniqueness of ItemIds
        var incomingIDs = Set<ItemId>()
        incomingIDs.reserveCapacity(items.count)
        for item in items {
            if containsItem(item.itemID) || !incomingIDs.insert(item.itemID).inserted {
                throw StoreError.duplicateItemId(modelID: id, itemID: item.itemID)
            }
        }

        noteTreeShape(items)
        let n = UInt64(items.count)
        if n > 0 {
            shiftCachedIndices(fromIndex: index, delta: Int64(n))

            for (i, item) in items.enumerated() {
                let targetIdx = index + UInt64(i)
                idToIndex[item.itemID] = targetIdx
                self.items[targetIdx] = item
            }

            itemCount += n
        }
    }

    /// Deletes items by `itemIds` or by index range (§13 MODEL_DELETE).
    ///
    /// Per §8 and §13, a `MODEL_DELETE` operation must specify either discrete item identities
    /// (`itemIds`) or a contiguous index range (`index` + `count`), but not both.
    /// Attempting to combine both selectors returns `StoreError.invalidModelDelete` to prevent
    /// double-decrement corruption of `itemCount`.
    ///
    /// Note (§8 Sparse Collection Invariant): When deleting by `itemId`, if the item is not
    /// currently held in the local sparse cache (uncached), `itemCount` is still decremented
    /// to maintain consistency with the authoritative collection length.
    public mutating func deleteItems(
        index: UInt64?,
        count: UInt64?,
        itemIds: [ItemId]
    ) throws {
        let hasRange = count != nil && count! > 0
        let hasIDs = !itemIds.isEmpty

        if hasRange && hasIDs {
            throw StoreError.invalidModelDelete(
                "cannot combine item_ids and range deletion in ModelDelete (§8, §13)"
            )
        }

        if hasIDs {
            for itemID in itemIds {
                if let idx = idToIndex.removeValue(forKey: itemID) {
                    items.removeValue(forKey: idx)
                    itemCount = itemCount > 0 ? itemCount - 1 : 0
                    shiftCachedIndices(fromIndex: idx + 1, delta: -1)
                } else {
                    itemCount = itemCount > 0 ? itemCount - 1 : 0
                }
            }
            return
        }

        if hasRange {
            let idx = index ?? 0
            let cnt = count!

            let (rangeEnd, overflowed) = idx.addingReportingOverflow(cnt)
            if idx > itemCount || overflowed || rangeEnd > itemCount {
                throw StoreError.modelIndexOutOfBounds(
                    index: overflowed ? UInt64.max : rangeEnd,
                    count: itemCount
                )
            }
            var keysToRemove: [UInt64] = []
            for k in items.keys where k >= idx && k < rangeEnd {
                keysToRemove.append(k)
            }

            for k in keysToRemove {
                if let removed = items.removeValue(forKey: k) {
                    idToIndex.removeValue(forKey: removed.itemID)
                }
            }

            shiftCachedIndices(fromIndex: rangeEnd, delta: -Int64(cnt))
            itemCount = itemCount >= cnt ? itemCount - cnt : 0
            return
        }
    }

    /// Updates existing items by identity or index without shifting (§13 MODEL_UPDATE).
    ///
    /// - **Positional Update (`index: Some(base_idx)`)**: Items are written directly to indices
    ///   `base_idx + i`. If an incoming `item_id` was previously cached at a different index position,
    ///   its old index entry is removed and re-keyed to the new target index.
    /// - **Identity Update (`index: nil`)**: Items are matched by stable `item_id` in `idToIndex`.
    ///   If an item is not found in the sparse cache, `StoreError.itemNotFound` is returned.
    public mutating func updateItems(
        index: UInt64?,
        items: [ModelItem]
    ) throws {
        noteTreeShape(items)
        for (i, item) in items.enumerated() {
            if let baseIdx = index {
                let targetIdx = baseIdx + UInt64(i)
                if targetIdx >= itemCount {
                    throw StoreError.modelIndexOutOfBounds(
                        index: targetIdx,
                        count: itemCount
                    )
                }
                if let oldPos = idToIndex.removeValue(forKey: item.itemID) {
                    if oldPos != targetIdx {
                        self.items.removeValue(forKey: oldPos)
                    }
                }
                if let displaced = self.items.removeValue(forKey: targetIdx) {
                    idToIndex.removeValue(forKey: displaced.itemID)
                }
                idToIndex[item.itemID] = targetIdx
                self.items[targetIdx] = item
            } else if let idx = idToIndex[item.itemID] {
                self.items[idx] = item
            } else {
                throw StoreError.itemNotFound(item.itemID)
            }
        }
    }

    /// Resets/replaces a contiguous range of cached items (§13 MODEL_RESET_RANGE).
    public mutating func resetRange(
        startIndex: UInt64,
        items: [ModelItem],
        totalCount: UInt64?
    ) throws {
        if let tc = totalCount, tc > 0 {
            self.itemCount = tc
        }

        if startIndex > itemCount {
            throw StoreError.modelIndexOutOfBounds(
                index: startIndex,
                count: itemCount
            )
        }

        let n = UInt64(items.count)
        let endIndex = startIndex + n
        if endIndex > itemCount {
            throw StoreError.modelIndexOutOfBounds(
                index: endIndex,
                count: itemCount
            )
        }

        // Validate incoming ItemIds for duplicates within items and collisions outside range
        var incomingIDs = Set<ItemId>()
        incomingIDs.reserveCapacity(items.count)
        for item in items {
            if !incomingIDs.insert(item.itemID).inserted {
                throw StoreError.duplicateItemId(modelID: id, itemID: item.itemID)
            }
            if let existingIdx = idToIndex[item.itemID] {
                if existingIdx < startIndex || existingIdx >= endIndex {
                    throw StoreError.duplicateItemId(modelID: id, itemID: item.itemID)
                }
            }
        }

        noteTreeShape(items)
        // Remove old cached items in [startIndex, endIndex)
        for idx in startIndex..<endIndex {
            if let old = self.items.removeValue(forKey: idx) {
                idToIndex.removeValue(forKey: old.itemID)
            }
        }

        // Insert new items
        for (i, item) in items.enumerated() {
            let idx = startIndex + UInt64(i)
            idToIndex[item.itemID] = idx
            self.items[idx] = item
        }
    }
}
