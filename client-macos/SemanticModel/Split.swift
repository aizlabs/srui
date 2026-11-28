//
// Split.swift
// SemanticModel
//
// The standard Split container (§4 inv. 13, §7.3, §7.6, §10, §22.4, §22.5, §26).
// Platform-neutral: NEVER import AppKit or Cocoa in this file.
//
// A Split (registry node type 25) divides its area into user-resizable panes: each child is one
// pane, in child order, at most `maxSplitPanes`. The standard property 35 `split_axis` (enum 13
// `SplitAxis`) names the axis, `side_by_side` when absent. It is admitted only on a Split, and a
// value outside `SplitAxis` is refused at decode and at apply, with the same rules, order, codes
// and decode messages as the Rust store (`server-rust/semantic-tree/src/split.rs`).
//
// Divider positions are local interaction state (§7.6 "resize locally", §22.5): nothing in the
// semantic model carries one.
//

import Foundation

/// Most panes (children) one Split holds (§26), matching Rust `MAX_SPLIT_PANES`.
public let maxSplitPanes = 8

/// Registry id of the `SplitAxis` enum.
public let splitAxisEnumID: UInt32 = StandardSplitAxis.standardEnumID

/// Why a `split_axis` value is refused (§4 inv. 13).
public enum SplitAxisError: Error, Equatable, Sendable, CustomStringConvertible {
    /// The value is not an enum token at all.
    case notAnEnumValue
    /// The value is an enum token that is not `SplitAxis` `side_by_side` or `stacked`.
    case unknownValue(enumID: UInt32, valueID: UInt32)

    /// Same text as the Rust `SplitAxisError`, byte for byte (the malformed vectors compare it).
    public var description: String {
        let expected = "split_axis must be SplitAxis side_by_side (13:1) or stacked (13:2)"
        switch self {
        case .notAnEnumValue:
            return "\(expected), got a non-enum value"
        case .unknownValue(let enumID, let valueID):
            return "\(expected), got enum \(enumID) value \(valueID)"
        }
    }
}

/// Returns `true` for the standard Split node type.
public func isSplit(_ nodeType: TypeRef) -> Bool {
    nodeType == .split
}

/// Parses a `split_axis` value: only `SplitAxis` `side_by_side` or `stacked` is accepted.
public func parseSplitAxis(_ value: Value) -> Result<StandardSplitAxis, SplitAxisError> {
    guard case .enumToken(let token) = value else {
        return .failure(.notAnEnumValue)
    }
    guard let axis = StandardSplitAxis(enumToken: token) else {
        return .failure(.unknownValue(enumID: token.enumID, valueID: token.valueID))
    }
    return .success(axis)
}

/// The axis a Split lays its panes out on: its `split_axis`, or `side_by_side` when absent.
/// Returns `nil` for a value the store would refuse.
public func effectiveSplitAxis(_ value: Value?) -> StandardSplitAxis? {
    guard let value else { return .sideBySide }
    return try? parseSplitAxis(value).get()
}
