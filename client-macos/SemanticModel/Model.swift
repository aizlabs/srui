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

    /// Constructs a new empty collection model with specified item count (§8, §13).
    public init(id: ModelId, modelType: TypeRef, itemCount: UInt64) {
        self.id = id
        self.modelType = modelType
        self.itemCount = itemCount
        self.items = [:]
        self.idToIndex = [:]
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
