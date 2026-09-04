//
// CollectionScrollEmbedding.swift
// Collections
//
// Nested-scroll chrome for collection controls. Fit-to-content is only for small inline
// collections; model-backed collections keep a bounded viewport and scroller (§8, Task 16/28).
//

import AppKit

/// How a collection adapter applied the last row mutation. Exposed for tests.
enum TableRowUpdate: Equatable {
    case none
    case fullReload
    case contentReload(IndexSet)
}

/// Shared scrolling chrome for list/table/tree when they sit inside an ancestor `.scroll` node.
@MainActor
enum CollectionScrollEmbedding {
    static func apply(
        nested: Bool,
        modelBacked: Bool,
        scrollView: NSScrollView,
        minHeightConstraint: NSLayoutConstraint?,
        fitHeightConstraint: inout NSLayoutConstraint?,
        contentHeight: CGFloat
    ) {
        let fitToContent = nested && !modelBacked
        scrollView.hasVerticalScroller = !fitToContent
        scrollView.hasHorizontalScroller = false
        scrollView.verticalScrollElasticity = fitToContent ? .none : .automatic
        scrollView.horizontalScrollElasticity = .none
        scrollView.borderType = fitToContent ? .noBorder : .bezelBorder

        if fitToContent {
            minHeightConstraint?.isActive = false
            let height = max(contentHeight, 1)
            if let existing = fitHeightConstraint {
                existing.constant = height
                existing.isActive = true
            } else {
                let constraint = scrollView.heightAnchor.constraint(equalToConstant: height)
                constraint.priority = .defaultHigh
                constraint.isActive = true
                fitHeightConstraint = constraint
            }
        } else {
            fitHeightConstraint?.isActive = false
            minHeightConstraint?.isActive = true
        }
    }

    static func tableContentHeight(_ tableView: NSTableView) -> CGFloat {
        let headerHeight = tableView.headerView?.frame.height ?? 0
        let rowHeight = tableView.rowHeight > 0 ? tableView.rowHeight : 17
        let spacing = tableView.intercellSpacing.height
        let rowCount = max(tableView.numberOfRows, 1)
        return headerHeight + CGFloat(rowCount) * (rowHeight + spacing)
    }
}
