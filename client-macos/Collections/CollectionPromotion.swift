//
// CollectionPromotion.swift
// Collections
//
// A server-promoted selection waiting to be applied by a collection adapter (§6.3, §8, §22.7).
// Scrolling stays local (§22.5); the only server-driven scroll is the one reveal per promotion.
//

import AppKit
import SemanticModel

/// One promotion an adapter has not applied yet because its item is not in the replica.
public struct CollectionPromotion: Equatable, Sendable {
    /// Promoted item identity; never a row index (§8).
    public let itemID: ItemId
    /// Whether applying it scrolls the row into view.
    public let reveal: Bool

    public init(itemID: ItemId, reveal: Bool) {
        self.itemID = itemID
        self.reveal = reveal
    }

    /// Scrolls `row` into view after the window has laid out the collection.
    @MainActor
    static func reveal(row: Int, in tableView: NSTableView) {
        tableView.window?.layoutIfNeeded()
        tableView.scrollRowToVisible(row)
    }
}
