//
// CollectionKeepVisibleRendering.swift
// RendererAppKit
//
// Generic rendering of the server-hinted keep-in-view item, the standard `keep_visible` property
// (§4 inv. 13, §6.3, §8, §22.5, §22.7), next to the promoted selection
// (`CollectionSelectionPromotion.swift`) and deliberately independent of it: this path never
// reads or writes the selection, the promotion memory or the accessibility selection state, and
// emits no event. The per-collection rules live in `Collections/CollectionKeepVisible.swift`.
//
// A remount builds fresh adapters whose viewport starts at the origin, so there is no local pan
// left to preserve: a mounted collection holding a hint reveals it once. No application-specific
// path: every List, Table and Tree is handled the same way.
//

import AppKit
import SemanticModel
import Collections

extension ControlFactory {
    /// Applies a collection's `keep_visible` from `node` after a commit's model refresh.
    ///
    /// - Parameter explicit: `true` when the committed transaction carried an operation that set
    ///   or cleared the property; setting it again reveals again.
    public func applyKeepVisible(in handle: RenderHandle, for node: Node, explicit: Bool) {
        guard isSelectableCollection(node.nodeType) else { return }
        let itemID = node.keepVisible
        if let adapter = handle.modelAdapter as? TableCollectionAdapter,
           let tableView = tableView(in: handle) {
            adapter.applyKeepVisible(itemID, explicit: explicit, tableView: tableView)
        } else if let adapter = handle.modelAdapter as? OutlineCollectionAdapter,
                  let outlineView = outlineView(in: handle) {
            adapter.applyKeepVisible(itemID, explicit: explicit, outlineView: outlineView)
        }
    }
}
