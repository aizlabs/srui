//
// CollectionSelectionPromotion.swift
// RendererAppKit
//
// Generic rendering of a server-promoted collection selection (§6.3, §7.6, §8, §22.5, §22.7,
// §22.8). A List/Table/Tree `value` holding an ItemId is the promoted selection. Rules:
//
// - A committed operation that sets or clears that `value` is a promotion. It replaces any local
//   selection and reveals the row once; a withdrawal clears the native selection.
// - A commit without such an operation never selects or scrolls: the user's local selection
//   survives unrelated commits, including structural remounts.
// - A remount (structural commit, resync, reconnect) restores the selection from semantic state
//   and reveals only when the promotion differs from the one this renderer last applied.
// - No application-specific path: every collection is handled the same way.
//

import AppKit
import SemanticModel
import Collections

/// What the renderer last knew about one collection's selection.
struct CollectionSelectionMemory: Equatable {
    /// Promotion last applied by this renderer.
    var promoted: ItemId?
    /// User selection made after that promotion, if any (§22.5).
    var local: [ItemId]?
}

extension ControlFactory {
    /// Applies a collection's promoted selection from `node` (§6.3).
    ///
    /// - Parameter explicit: `true` when the committed transaction carried an operation on the
    ///   collection's `value`. Promoting the same ItemId again is still a new promotion.
    public func applyPromotedSelection(in handle: RenderHandle, for node: Node, explicit: Bool) {
        guard isSelectableCollection(node.nodeType) else { return }
        let promoted = node.promotedSelection
        var memory = collectionSelectionMemory[node.id] ?? CollectionSelectionMemory()
        let changed = explicit || memory.promoted != promoted

        if changed {
            memory = CollectionSelectionMemory(promoted: promoted, local: nil)
            apply(promotion: promoted, reveal: promoted != nil, to: handle)
        } else if let local = memory.local {
            restore(local: local, to: handle)
        } else if let promoted {
            apply(promotion: promoted, reveal: false, to: handle)
        }

        if memory == CollectionSelectionMemory() {
            collectionSelectionMemory.removeValue(forKey: node.id)
        } else {
            collectionSelectionMemory[node.id] = memory
        }
    }

    /// Drops selection memory for collections no longer rendered.
    public func pruneCollectionSelectionMemory(keeping present: Set<NodeId>) {
        collectionSelectionMemory = collectionSelectionMemory.filter { present.contains($0.key) }
    }

    func noteLocalSelection(nodeID: NodeId, itemIDs: [ItemId]) {
        collectionSelectionMemory[nodeID, default: CollectionSelectionMemory()].local = itemIDs
    }

    private func apply(promotion: ItemId?, reveal: Bool, to handle: RenderHandle) {
        if let adapter = handle.modelAdapter as? TableCollectionAdapter,
           let tableView = tableView(in: handle) {
            adapter.applyPromotion(promotion, reveal: reveal, tableView: tableView)
        } else if let adapter = handle.modelAdapter as? OutlineCollectionAdapter,
                  let outlineView = outlineView(in: handle) {
            adapter.applyPromotion(promotion, reveal: reveal, outlineView: outlineView)
        }
    }

    private func restore(local: [ItemId], to handle: RenderHandle) {
        if let adapter = handle.modelAdapter as? TableCollectionAdapter,
           let tableView = tableView(in: handle) {
            adapter.restoreLocalSelection(local, tableView: tableView)
        } else if let adapter = handle.modelAdapter as? OutlineCollectionAdapter,
                  let outlineView = outlineView(in: handle) {
            adapter.restoreLocalSelection(local, outlineView: outlineView)
        }
    }
}
