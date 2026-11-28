//
// SplitRendering.swift
// RendererAppKit
//
// The standard Split container rendered as `NSSplitView` (PX-070-G02; §7.3, §7.6, §10, §22.4,
// §22.5, §22.8).
//
// - `side_by_side` (the default) maps to a vertical-divider split view (`isVertical == true`),
//   `stacked` to a horizontal-divider one. Changing `split_axis` is structure-affecting
//   (`DirtyClassifier`), so it rebuilds the panes.
// - Each child is one pane, in child order, held by a `SplitPaneView`. The pane decouples the
//   divider from the child's own layout hints: the child keeps its `minimum_size` (required, so
//   a divider stops there; the pane repeats it), `maximum_size` and `preferred_size`
//   constraints, and is pinned to the pane's leading and top edges with its trailing and bottom
//   edges filling the pane (249.5) and staying inside it (249.8), both just below NSSplitView's
//   default holding priority (250) that keeps the divider positions. Flexible content (hugging
//   249; a Row/Column pane's stack hugging is lowered to 249) fills its pane, rigid content
//   (`grow == 0`, hugging 499) keeps its natural size, content wider than a narrow pane is
//   clipped, and no hint pulls a divider: a hint never becomes a pixel mandate (§10).
//   The holding priority is left at AppKit's default on purpose: above 250 it outranks the
//   enclosing NSStackView's own 250-priority edge constraints, and every divider move then lands
//   short by the stack's edge inset (measured: 20 points inside a Surface).
// - Until the user moves a divider, pane lengths come from the hints at every length of the
//   split view: a pane's `preferred_size` along the axis claims that length, and the other
//   panes share the rest in proportion to their `grow` (absent or zero counts as 1). Once the
//   user moves a divider, the resulting proportions are kept: a window resize scales every pane
//   by the same factor (minimum sizes still bound them), and only another divider move changes
//   them.
// - A child whose `visibility` is `collapsed` collapses its pane: the pane leaves the split
//   view's arranged subviews and the remaining panes share the length. `hidden` keeps the space.
// - Divider positions are local interaction state (§7.6 "resize locally", §22.5). Neither a
//   drag nor an accessibility adjustment originates a `SemanticInteraction`, so no event and no
//   frame is sent; `Split`'s optional `VALUE_CHANGED` is not implemented. The proportions are
//   remembered per Split node in `ControlFactory.splitProportionMemory` and restored when a
//   remount rebuilds the view: unchanged panes get their proportions back, and when panes were
//   added or removed the remaining panes keep their proportions relative to each other while a
//   new pane takes an equal share.
// - Accessibility: the split view is an `AXSplitGroup`. AppKit's own splitter elements expose a
//   value but implement no increment/decrement (their `accessibilityPerformIncrement` reaches an
//   unimplemented `accessibilityPerformAction:`), so the split view replaces them with
//   `SplitDividerAccessibilityElement`s that carry the divider position as their value and
//   move it by a step on increment/decrement.
//

import AppKit
import SemanticModel

/// One pane of a `SemanticSplitView`: holds exactly one child's view.
///
/// The child is pinned to the pane's leading and top edges. Its trailing and bottom edges fill
/// the pane at `fillPriority` and stay inside it at `containPriority`, both below NSSplitView's
/// default holding priority (250), so neither the child's hugging nor its compression resistance
/// moves a divider; content larger than a narrow pane is clipped. The child's `minimum_size` is repeated
/// on the pane as a required constraint, so a divider stops there.
@MainActor
public final class SplitPaneView: NSView {
    /// The child node this pane holds.
    public let nodeID: NodeId
    /// The child's `preferred_size`, a hint for the pane's first length along the axis only.
    /// A change re-applies the hints; proportions the user set by moving a divider still win.
    var preferredSize: Size? {
        didSet { if preferredSize != oldValue { owner?.setNeedsProportionRestore() } }
    }
    /// The child's `grow`, its weight when panes without a preferred length share the rest.
    var growWeight: Double? {
        didSet { if growWeight != oldValue { owner?.setNeedsProportionRestore() } }
    }
    /// Whether the child's `visibility` is `collapsed`, which collapses the pane.
    var isCollapsedPane = false
    weak var owner: SemanticSplitView?

    /// The child's `minimum_size`, which the pane itself never goes below.
    var minimumSize: Size? {
        didSet { updateMinimumConstraints() }
    }
    private var minimumConstraints: [NSLayoutConstraint] = []

    /// Below NSSplitView's default holding priority (250), above flexible content hugging (249):
    /// flexible content fills its pane, rigid content keeps its natural size, neither moves a
    /// divider.
    static let fillPriority = NSLayoutConstraint.Priority(rawValue: 249.5)
    /// Keeps the child inside the pane when it fits, without letting its compression resistance
    /// widen the pane.
    static let containPriority = NSLayoutConstraint.Priority(rawValue: 249.8)

    init(nodeID: NodeId, content: NSView) {
        self.nodeID = nodeID
        super.init(frame: .zero)
        translatesAutoresizingMaskIntoConstraints = false
        clipsToBounds = true
        content.translatesAutoresizingMaskIntoConstraints = false
        addSubview(content)
        func constraint(_ c: NSLayoutConstraint, _ priority: NSLayoutConstraint.Priority) -> NSLayoutConstraint {
            c.priority = priority
            return c
        }
        NSLayoutConstraint.activate([
            content.leadingAnchor.constraint(equalTo: leadingAnchor),
            content.topAnchor.constraint(equalTo: topAnchor),
            constraint(content.trailingAnchor.constraint(lessThanOrEqualTo: trailingAnchor), Self.containPriority),
            constraint(content.bottomAnchor.constraint(lessThanOrEqualTo: bottomAnchor), Self.containPriority),
            constraint(content.trailingAnchor.constraint(equalTo: trailingAnchor), Self.fillPriority),
            constraint(content.bottomAnchor.constraint(equalTo: bottomAnchor), Self.fillPriority),
        ])
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) is not supported")
    }

    private func updateMinimumConstraints() {
        NSLayoutConstraint.deactivate(minimumConstraints)
        minimumConstraints = []
        guard let minimumSize else { return }
        minimumConstraints = [
            widthAnchor.constraint(greaterThanOrEqualToConstant: max(0, minimumSize.width)),
            heightAnchor.constraint(greaterThanOrEqualToConstant: max(0, minimumSize.height)),
        ]
        NSLayoutConstraint.activate(minimumConstraints)
    }
}

/// The native view of a standard `Split` node (§22.4).
@MainActor
public final class SemanticSplitView: NSSplitView, NSSplitViewDelegate {
    /// The Split node this view renders.
    public let nodeID: NodeId

    /// Reads the remembered pane proportions of this Split (pane id → fraction of the laid-out
    /// panes' total length). Supplied by `ControlFactory`, which keeps them across remounts.
    var loadProportions: () -> [NodeId: Double] = { [:] }
    /// Replaces the remembered pane proportions of this Split.
    var storeProportions: ([NodeId: Double]) -> Void = { _ in }

    /// Every pane in child order, collapsed ones included.
    public private(set) var panes: [SplitPaneView] = []
    /// Whether the next layout pass must recompute the proportions (panes or axis changed).
    private var needsProportionRestore = true
    /// Split view length the proportions were last applied at; a different length is a resize.
    private var appliedLength: CGFloat?
    private var isApplyingProportions = false
    private var dividerElements: [SplitDividerAccessibilityElement] = []

    init(nodeID: NodeId) {
        self.nodeID = nodeID
        super.init(frame: .zero)
        dividerStyle = .thin
        isVertical = true
        delegate = self
        translatesAutoresizingMaskIntoConstraints = false
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) {
        fatalError("init(coder:) is not supported")
    }

    // MARK: - Axis and panes

    /// The axis the panes are laid out on (`split_axis`, absent means `side_by_side`).
    public var axis: StandardSplitAxis {
        isVertical ? .sideBySide : .stacked
    }

    func apply(axis: StandardSplitAxis) {
        let vertical = axis == .sideBySide
        guard isVertical != vertical else { return }
        isVertical = vertical
        setNeedsProportionRestore()
    }

    /// Ids of the panes currently laid out (collapsed panes excluded), in pane order.
    public var visiblePaneIDs: [NodeId] {
        arrangedPanes.map(\.nodeID)
    }

    /// Ids of every pane, in child order.
    public var paneIDs: [NodeId] {
        panes.map(\.nodeID)
    }

    public func pane(for nodeID: NodeId) -> SplitPaneView? {
        panes.first { $0.nodeID == nodeID }
    }

    private var arrangedPanes: [SplitPaneView] {
        arrangedSubviews.compactMap { $0 as? SplitPaneView }
    }

    @discardableResult
    func addPane(content: NSView, nodeID: NodeId) -> SplitPaneView {
        let pane = SplitPaneView(nodeID: nodeID, content: content)
        pane.owner = self
        panes.append(pane)
        addArrangedSubview(pane)
        setNeedsProportionRestore()
        return pane
    }

    /// Collapses or restores `pane` (its child's `visibility` is or is no longer `collapsed`).
    func setPane(_ pane: SplitPaneView, collapsed: Bool) {
        guard pane.owner === self, pane.isCollapsedPane != collapsed else { return }
        pane.isCollapsedPane = collapsed
        pane.isHidden = collapsed
        if collapsed {
            removeArrangedSubview(pane)
            pane.removeFromSuperview()
        } else {
            let index = panes.prefix { $0 !== pane }.filter { !$0.isCollapsedPane }.count
            insertArrangedSubview(pane, at: index)
        }
        setNeedsProportionRestore()
    }

    // MARK: - Proportions (local interaction state, §22.5)

    private var axisLength: CGFloat {
        isVertical ? bounds.width : bounds.height
    }

    private func length(of view: NSView) -> CGFloat {
        isVertical ? view.frame.width : view.frame.height
    }

    /// Asks the next layout pass to recompute and apply the proportions.
    func setNeedsProportionRestore() {
        needsProportionRestore = true
        needsLayout = true
    }

    /// The current proportions of the laid-out panes (fractions of their total length).
    public var currentProportions: [NodeId: Double] {
        let visible = arrangedPanes
        let total = visible.reduce(CGFloat(0)) { $0 + length(of: $1) }
        guard total > 0 else { return [:] }
        return Dictionary(uniqueKeysWithValues: visible.map {
            ($0.nodeID, Double(length(of: $0) / total))
        })
    }

    public override func layout() {
        super.layout()
        applyProportionsIfNeeded()
    }

    /// A resize must reach `layout()`, where the proportions are re-applied at the new length.
    public override func setFrameSize(_ newSize: NSSize) {
        super.setFrameSize(newSize)
        if let appliedLength, abs(appliedLength - axisLength) > 0.5 {
            needsLayout = true
        }
    }

    /// Applies the proportions when the panes changed or the split view was resized.
    private func applyProportionsIfNeeded() {
        let visible = arrangedPanes
        guard visible.count > 1 else {
            needsProportionRestore = false
            appliedLength = nil
            return
        }
        let resized = appliedLength.map { abs($0 - axisLength) > 0.5 } ?? true
        guard needsProportionRestore || resized else { return }
        let available = axisLength - dividerThickness * CGFloat(visible.count - 1)
        // Wait for a real size: a zero-sized split view has no proportions to apply.
        guard available > CGFloat(visible.count) else { return }
        let fractions = targetFractions(for: visible, available: available)
        needsProportionRestore = false
        appliedLength = axisLength
        isApplyingProportions = true
        var position: CGFloat = 0
        for index in 0..<(visible.count - 1) {
            position += available * CGFloat(fractions[index])
            setPosition(position, ofDividerAt: index)
            position += dividerThickness
        }
        super.layout()
        isApplyingProportions = false
        // Nothing is remembered here: only a user's divider move is local state worth keeping.
        // Until then the hints are re-applied at every length, so a preferred length stays that
        // length however the window is sized.
    }

    /// Fractions for `visible`, summing to 1.
    ///
    /// Panes with a remembered proportion keep their proportions relative to each other; a pane
    /// without one takes an equal share. With nothing remembered, a pane's `preferred_size`
    /// along the axis claims that length and the others share the rest by `grow`. Panes without a
    /// preferred size always keep at least half an equal share between them: preferred lengths
    /// that would consume the axis are scaled down rather than squeezing those panes to nothing.
    private func targetFractions(for visible: [SplitPaneView], available: CGFloat) -> [Double] {
        let remembered = loadProportions()
        let known = visible.compactMap { remembered[$0.nodeID] }.filter { $0 > 0 }
        let count = Double(visible.count)
        guard known.isEmpty else {
            let newCount = Double(visible.filter { (remembered[$0.nodeID] ?? 0) <= 0 }.count)
            let knownShare = 1 - newCount / count
            let knownTotal = known.reduce(0, +)
            return visible.map { pane in
                if let fraction = remembered[pane.nodeID], fraction > 0 {
                    return fraction / knownTotal * knownShare
                }
                return 1 / count
            }
        }
        let preferred: [Double?] = visible.map { pane in
            guard let size = pane.preferredSize else { return nil }
            let length = Double(isVertical ? size.width : size.height)
            return length > 0 ? min(length / Double(available), 0.9) : nil
        }
        let claimed = preferred.compactMap { $0 }.reduce(0, +)
        let weights = visible.map { pane in (pane.growWeight ?? 0) > 0 ? pane.growWeight! : 1 }
        let openWeight = zip(preferred, weights).filter { $0.0 == nil }.map(\.1).reduce(0, +)
        guard openWeight > 0 else {
            let total = claimed > 0 ? claimed : 1
            return preferred.map { ($0 ?? 0) / total }
        }
        let openCount = Double(preferred.filter { $0 == nil }.count)
        let openShare = max(1 - claimed, openCount / count / 2)
        let scale = claimed > 0 ? min(1, (1 - openShare) / claimed) : 1
        return zip(preferred, weights).map { preferred, weight in
            preferred.map { $0 * scale } ?? openShare * weight / openWeight
        }
    }

    /// Merges `proportions` into the remembered ones; collapsed panes keep theirs, panes that are
    /// gone are dropped, so at most one entry per pane (at most `maxSplitPanes`) is kept.
    private func remember(_ proportions: [NodeId: Double]) {
        var merged = loadProportions().filter { id, _ in panes.contains { $0.nodeID == id } }
        merged.merge(proportions) { _, new in new }
        storeProportions(merged)
    }

    /// AppKit marks a divider move (a drag, or `setPosition(_:ofDividerAt:)` as used by the
    /// accessibility splitters) with this userInfo key; resizes from layout carry no such key.
    static let userResizeKey = "NSSplitViewUserResizeKey"

    /// A divider moved: the user dragged it or adjusted it through accessibility. Its result
    /// becomes the remembered proportions. Proportions this view applies itself are not recorded.
    public func splitViewDidResizeSubviews(_ notification: Notification) {
        guard !isApplyingProportions, window != nil,
              notification.userInfo?[Self.userResizeKey] != nil else { return }
        let current = currentProportions
        guard current.count == arrangedPanes.count, current.count > 1 else { return }
        remember(current)
    }

    /// Moves divider `index` to `position` (points from the leading or top edge), as a drag or an
    /// accessibility adjustment does. Local only: nothing is sent.
    public func moveDivider(at index: Int, to position: CGFloat) {
        guard index >= 0, index < arrangedPanes.count - 1 else { return }
        let clamped = min(
            max(position, minPossiblePositionOfDivider(at: index)),
            maxPossiblePositionOfDivider(at: index)
        )
        setPosition(clamped, ofDividerAt: index)
        layoutSubtreeIfNeeded()
    }

    /// Position of divider `index` (points from the leading or top edge).
    public func positionOfDivider(at index: Int) -> CGFloat? {
        let visible = arrangedPanes
        guard index >= 0, index < visible.count - 1 else { return nil }
        let pane = visible[index]
        return isVertical ? pane.frame.maxX : pane.frame.maxY
    }

    // MARK: - Accessibility (§22.8)

    public override func accessibilityChildren() -> [Any]? {
        let native = (super.accessibilityChildren() ?? []).filter { child in
            (child as? NSAccessibilityElement)?.accessibilityRole() != .splitter
        }
        let count = max(0, arrangedPanes.count - 1)
        if dividerElements.count != count {
            dividerElements = (0..<count).map { SplitDividerAccessibilityElement(splitView: self, index: $0) }
        }
        return native + dividerElements
    }
}

/// An adjustable accessibility splitter for one divider of a `SemanticSplitView` (§22.8).
///
/// Its value is the divider position in points from the leading or top edge, like AppKit's own
/// splitter; increment and decrement move it by 5% of the split view's length. Neither sends
/// anything: the position is local interaction state (§22.5).
///
/// `NSAccessibilityElement` is not main-actor isolated, but AppKit calls accessibility on the main
/// thread, so each override asserts that and reads the main-actor split view there.
public final class SplitDividerAccessibilityElement: NSAccessibilityElement {
    private weak var splitView: SemanticSplitView?
    public let index: Int

    @MainActor
    init(splitView: SemanticSplitView, index: Int) {
        self.splitView = splitView
        self.index = index
        super.init()
    }

    /// Runs `body` with the split view on the main actor (where AppKit calls accessibility).
    private func withSplitView<T: Sendable>(
        _ fallback: T,
        _ body: @MainActor (SemanticSplitView, Int) -> T
    ) -> T {
        // Read outside the closure so it captures the (main-actor, hence Sendable) view, not self.
        guard let split = splitView else { return fallback }
        let index = index
        return MainActor.assumeIsolated { body(split, index) }
    }

    public override func isAccessibilityElement() -> Bool { true }
    public override func accessibilityRole() -> NSAccessibility.Role? { .splitter }

    public override func accessibilityParent() -> Any? {
        splitView
    }

    public override func accessibilityOrientation() -> NSAccessibilityOrientation {
        withSplitView(.vertical) { split, _ in split.isVertical ? .vertical : .horizontal }
    }

    public override func accessibilityFrame() -> NSRect {
        withSplitView(.zero) { split, index in
            guard let position = split.positionOfDivider(at: index) else { return .zero }
            let thickness = split.dividerThickness
            let rect = split.isVertical
                ? NSRect(x: position, y: 0, width: thickness, height: split.bounds.height)
                : NSRect(x: 0, y: position, width: split.bounds.width, height: thickness)
            return NSAccessibility.screenRect(fromView: split, rect: rect)
        }
    }

    public override func accessibilityValue() -> Any? {
        withSplitView(nil) { split, index in split.positionOfDivider(at: index).map { Double($0) } }
            .map { NSNumber(value: $0) }
    }

    public override func accessibilityMinValue() -> Any? {
        withSplitView(nil) { split, index in Double(split.minPossiblePositionOfDivider(at: index)) }
            .map { NSNumber(value: $0) }
    }

    public override func accessibilityMaxValue() -> Any? {
        withSplitView(nil) { split, index in Double(split.maxPossiblePositionOfDivider(at: index)) }
            .map { NSNumber(value: $0) }
    }

    public override func setAccessibilityValue(_ accessibilityValue: Any?) {
        guard let position = (accessibilityValue as? NSNumber)?.doubleValue else { return }
        withSplitView(()) { split, index in split.moveDivider(at: index, to: CGFloat(position)) }
    }

    public override func accessibilityPerformIncrement() -> Bool {
        adjust(by: 1)
    }

    public override func accessibilityPerformDecrement() -> Bool {
        adjust(by: -1)
    }

    private func adjust(by direction: CGFloat) -> Bool {
        withSplitView(false) { split, index in
            guard let position = split.positionOfDivider(at: index) else { return false }
            let length = split.isVertical ? split.bounds.width : split.bounds.height
            let step = max(1, (length * 0.05).rounded())
            split.moveDivider(at: index, to: position + direction * step)
            return true
        }
    }
}
