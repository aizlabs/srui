//
// CollectionKeepVisible.swift
// Collections
//
// Server-hinted keep-in-view for one collection (the standard `keep_visible` property; §4 inv. 13,
// §6.3, §8, §22.5, §22.7). The hint names an ItemId the renderer keeps in view. It is
// presentation guidance and never selection state: nothing here reads or writes a selection,
// posts an accessibility notification or emits an event.
//
// Rules, shared by TableCollectionAdapter and OutlineCollectionAdapter:
// - A commit that sets the hint (even to the same ItemId) reveals the item's row once: the
//   minimum scroll that makes it visible, as for a promotion but without selecting it.
// - Any later commit reveals it again only when the row index at which the item is shown
//   changed. A commit that leaves the row index unchanged never scrolls, so a local pan persists
//   until the item moves.
// - An item the replica does not cache has no row: nothing scrolls and nothing is requested.
//   It is revealed when it arrives through range data at a row other than the last one shown.
//   The same holds for any item the adapter cannot map to a displayed row (for example one
//   under a collapsed Tree ancestor): the hint never expands anything, it just does not scroll.
// - A local expand or collapse never scrolls, even when it expands the hinted item's own
//   ancestor or shifts the hinted row: it only re-baselines the last revealed row to the item's
//   new displayed row, or to "not shown" when the change hides it. The next commit that sets
//   the hint, or changes the displayed row relative to that baseline, reveals it as above.
// - Clearing the hint never scrolls. Local selection, scrolling and keyboard navigation never
//   clear or override it; only the server does.
//

import SemanticModel

/// The keep-in-view state of one collection adapter (§6.3, §22.7).
public struct CollectionKeepVisible: Equatable, Sendable {
    /// Hinted item identity; never a row index (§8).
    public private(set) var itemID: ItemId?
    /// The hinted item's displayed row as of the last reveal or re-baseline (a local disclosure
    /// change), or `nil` while the item is hidden or not yet revealed.
    public private(set) var shownRow: Int?

    public init() {}

    /// Applies the hint a commit leaves in semantic state and returns the row to reveal, if any.
    ///
    /// - Parameters:
    ///   - itemID: the collection's `keep_visible` after the commit; `nil` when absent.
    ///   - explicit: the commit carried an operation that set the property, so the row is
    ///     revealed again even if neither the item nor its row changed.
    ///   - rowOf: the replica's row for an item, `nil` when not cached. Never a guess.
    public mutating func apply(
        _ itemID: ItemId?,
        explicit: Bool,
        rowOf: (ItemId) -> Int?
    ) -> Int? {
        guard let itemID else {
            self = CollectionKeepVisible()
            return nil
        }
        if explicit || itemID != self.itemID {
            shownRow = nil
        }
        self.itemID = itemID
        guard let row = rowOf(itemID), row != shownRow else { return nil }
        shownRow = row
        return row
    }

    /// Re-baselines after a local disclosure change, which never scrolls (§22.5).
    ///
    /// The last revealed row becomes the item's current displayed row, or `nil` ("not shown")
    /// when it has none, so only a later commit that sets the hint or moves the item relative to
    /// this baseline reveals it.
    public mutating func rebaseline(rowOf: (ItemId) -> Int?) {
        guard let itemID else { return }
        shownRow = rowOf(itemID)
    }
}
