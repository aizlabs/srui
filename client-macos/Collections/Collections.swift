//
// Collections.swift
// Collections
//
// Sparse AppKit collection adapters for List, Table, and Tree (§8, §22.7).
//
// Spec sections implemented:
// - §8 Collections and Model Data: logical `itemCount` may be hundreds of thousands while
//   the native control virtualizes a viewport-sized set of cells.
// - §12.1 Transactions: range arrival is an authoritative `MODEL_RESET_RANGE`; adapters
//   refresh in place without replacing the native table or outline.
// - §22.7 Viewport: clip-view observation emits bounded, page-aligned cache-miss requests.
//
// This target may import AppKit. It must not import Session or SemanticInteraction.
//

import AppKit
import SemanticModel

/// Shared cell formatting for collection adapters (§8).
public enum CollectionCells {
    public static let loadingPlaceholder = "Loading…"

    public static func cellString(from value: Value) -> String {
        if let str = value.asString { return str }
        if value == .null { return "" }
        return value.description
    }

    public static func cells(from value: Value) -> [String] {
        if case .list(let items) = value {
            return items.map(cellString(from:))
        }
        return [cellString(from: value)]
    }

    public static func clampedRowCount(_ count: UInt64) -> Int {
        if count > UInt64(Int.max) {
            return Int.max
        }
        return Int(count)
    }
}
