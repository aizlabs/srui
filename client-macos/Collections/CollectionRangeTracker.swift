//
// CollectionRangeTracker.swift
// Collections
//
// Page-aligns the visible/prefetch window, subtracts cached and in-flight coverage,
// and emits contiguous missing segments only (§8, §22.7).
//

import Dispatch
import SemanticModel

/// Tracks in-flight cache-miss requests for one collection adapter.
public struct CollectionRangeTracker: Equatable, Sendable {
    public static let pageSize: UInt64 = 128
    public static let prefetchMargin: UInt64 = 32
    /// How long an in-flight window stays pending without a matching
    /// `MODEL_RESET_RANGE` before a later viewport update may re-request it.
    /// Covers server decline paths that send no envelope (`NoProvider`,
    /// `Stale`, `CountExceedsLimit`) and a dropped send while the data plane
    /// is closed.
    public static let pendingTTLNanoseconds: UInt64 = 2_000_000_000

    private var pending: [PendingWindow] = []

    public init() {}

    public static func == (lhs: Self, rhs: Self) -> Bool {
        lhs.pending.map(\.range) == rhs.pending.map(\.range)
    }

    /// Drops every in-flight segment. Call on model replacement or reconnect.
    public mutating func reset() {
        pending.removeAll(keepingCapacity: false)
    }

    /// Clears pending coverage that overlaps `[start, start + count)`.
    public mutating func noteArrived(start: UInt64, count: UInt64) {
        guard let arrived = exclusiveRange(start: start, count: count) else { return }
        subtractPending(arrived)
    }

    /// Forget an in-flight window that never left the client (data plane
    /// closed, send failed). The next viewport update re-emits it.
    public mutating func noteDropped(start: UInt64, count: UInt64) {
        guard let dropped = exclusiveRange(start: start, count: count) else { return }
        subtractPending(dropped)
    }

    /// Returns contiguous missing segments inside the aligned visible/prefetch window.
    public mutating func requests(
        visibleStart: UInt64,
        visibleCount: UInt64,
        itemCount: UInt64,
        model: Model,
        nodeID: NodeId,
        modelID: ModelId,
        nowNanos: UInt64 = DispatchTime.now().uptimeNanoseconds
    ) -> [CollectionRangeRequest] {
        expireStalePending(nowNanos: nowNanos)
        guard itemCount > 0, visibleCount > 0 else {
            pending.removeAll(keepingCapacity: false)
            return []
        }
        guard let window = alignedWindow(
            visibleStart: visibleStart,
            visibleCount: visibleCount,
            itemCount: itemCount
        ) else {
            pending.removeAll(keepingCapacity: false)
            return []
        }

        pending = pending.filter { $0.range.overlaps(window) }

        var covered: [Range<UInt64>] = pending.map(\.range)
        for cached in model.cachedRanges() {
            if let range = exclusiveRange(start: cached.start, count: cached.length) {
                covered.append(range)
            }
        }

        let holes = subtract(covers: covered, from: [window])
        guard let first = holes.first, let last = holes.last else {
            return []
        }
        let merged = first.lowerBound..<last.upperBound
        pending.append(PendingWindow(range: merged, issuedAtNanos: nowNanos))
        let count = merged.upperBound - merged.lowerBound
        guard count > 0 else { return [] }
        return [
            CollectionRangeRequest(
                nodeID: nodeID,
                modelID: modelID,
                startIndex: merged.lowerBound,
                count: count
            )
        ]
    }

    public static func alignedWindow(
        visibleStart: UInt64,
        visibleCount: UInt64,
        itemCount: UInt64
    ) -> Range<UInt64>? {
        guard itemCount > 0, visibleCount > 0 else { return nil }
        let visibleEnd = min(itemCount, saturatingAdd(visibleStart, visibleCount))
        let prefetchStart = visibleStart > prefetchMargin ? visibleStart - prefetchMargin : 0
        let prefetchEnd = min(itemCount, saturatingAdd(visibleEnd, prefetchMargin))
        let start = alignDown(prefetchStart, page: pageSize)
        var end = alignUp(prefetchEnd, page: pageSize)
        if end > itemCount {
            end = itemCount
        }
        if start >= end {
            return nil
        }
        return start..<end
    }

    private func alignedWindow(
        visibleStart: UInt64,
        visibleCount: UInt64,
        itemCount: UInt64
    ) -> Range<UInt64>? {
        Self.alignedWindow(
            visibleStart: visibleStart,
            visibleCount: visibleCount,
            itemCount: itemCount
        )
    }

    private mutating func expireStalePending(nowNanos: UInt64) {
        pending.removeAll { nowNanos &- $0.issuedAtNanos >= Self.pendingTTLNanoseconds }
    }

    private mutating func subtractPending(_ cover: Range<UInt64>) {
        var next: [PendingWindow] = []
        next.reserveCapacity(pending.count)
        for window in pending {
            for leftover in subtract(cover: cover, from: [window.range]) {
                next.append(PendingWindow(range: leftover, issuedAtNanos: window.issuedAtNanos))
            }
        }
        pending = next
    }
}

private struct PendingWindow: Sendable {
    var range: Range<UInt64>
    var issuedAtNanos: UInt64
}

private func alignDown(_ value: UInt64, page: UInt64) -> UInt64 {
    guard page > 0 else { return value }
    return (value / page) * page
}

private func alignUp(_ value: UInt64, page: UInt64) -> UInt64 {
    guard page > 0 else { return value }
    let remainder = value % page
    if remainder == 0 {
        return value
    }
    return saturatingAdd(value, page - remainder)
}

private func saturatingAdd(_ lhs: UInt64, _ rhs: UInt64) -> UInt64 {
    let (result, overflow) = lhs.addingReportingOverflow(rhs)
    return overflow ? UInt64.max : result
}

private func exclusiveRange(start: UInt64, count: UInt64) -> Range<UInt64>? {
    guard count > 0 else { return nil }
    let (end, overflow) = start.addingReportingOverflow(count)
    if overflow {
        return nil
    }
    return start..<end
}

private func subtract(cover: Range<UInt64>, from holes: [Range<UInt64>]) -> [Range<UInt64>] {
    subtract(covers: [cover], from: holes)
}

private func subtract(covers: [Range<UInt64>], from holes: [Range<UInt64>]) -> [Range<UInt64>] {
    var remaining = holes
    for cover in covers.sorted(by: { $0.lowerBound < $1.lowerBound }) {
        var next: [Range<UInt64>] = []
        next.reserveCapacity(remaining.count)
        for hole in remaining {
            if cover.upperBound <= hole.lowerBound || cover.lowerBound >= hole.upperBound {
                next.append(hole)
                continue
            }
            if hole.lowerBound < cover.lowerBound {
                next.append(hole.lowerBound..<cover.lowerBound)
            }
            if cover.upperBound < hole.upperBound {
                next.append(cover.upperBound..<hole.upperBound)
            }
        }
        remaining = next
    }
    return remaining.filter { $0.lowerBound < $0.upperBound }
}
