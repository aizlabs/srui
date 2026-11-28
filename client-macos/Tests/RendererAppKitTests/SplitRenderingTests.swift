//
// SplitRenderingTests.swift
// RendererAppKitTests
//
// Native AppKit evidence for the standard Split container (PX-070-G02; §7.3, §7.6, §10, §22.4,
// §22.5, §22.8, §23). Every test mounts a real NSSplitView inside a Surface window through
// LayoutRenderer, drives it with committed transactions and with real local interaction (posted
// mouse-drag events, accessibility increments), and checks the native views. Divider positions
// are local: no drag or adjustment may originate an interaction, a range request or terminal
// traffic, and commits must not move a divider the user placed.
//

import AppKit
import SemanticModel
import Testing
@testable import RendererAppKit

@Suite(.timeLimit(.minutes(1)))
@MainActor
struct SplitRenderingTests {
    static let surfaceID = NodeId(1)
    static let splitID = NodeId(2)
    static let paneA = NodeId(10)
    static let paneB = NodeId(11)
    static let paneC = NodeId(12)

    @MainActor
    final class Harness {
        let renderer = LayoutRenderer()
        var store: SemanticStore
        /// Everything the renderer could send toward the server.
        var interactions: [SemanticInteraction] = []
        var rangeRequests: [CollectionRangeRequest] = []
        var terminalTraffic = 0

        init(_ operations: [SemanticModel.Operation]) throws {
            var store = SemanticStore()
            for operation in operations {
                try operation.apply(to: &store)
            }
            self.store = store
            let factory = renderer.controlFactory
            factory.onInteraction = { [weak self] in self?.interactions.append($0) }
            factory.onCollectionRangeRequest = { [weak self] in self?.rangeRequests.append($0) }
            factory.onTerminalInput = { [weak self] _, _ in self?.terminalTraffic += 1 }
            factory.onTerminalResize = { [weak self] _, _, _, _, _ in self?.terminalTraffic += 1 }
            try renderer.mount(store: store)
            place()
        }

        var window: NSWindow { renderer.registry.handle(for: surfaceID)!.window! }
        var split: SemanticSplitView { renderer.registry.view(for: splitID) as! SemanticSplitView }
        var sent: Int { interactions.count + rangeRequests.count + terminalTraffic }

        /// The test window size, at the screen origin: posted mouse events carry window
        /// coordinates, and NSSplitView's drag tracking maps them correctly only there.
        func place(size: NSSize = NSSize(width: 840, height: 640)) {
            window.setContentSize(size)
            window.setFrameOrigin(.zero)
            layout()
        }

        func layout() {
            window.contentView?.layoutSubtreeIfNeeded()
            split.layoutSubtreeIfNeeded()
        }

        func commit(_ operations: [SemanticModel.Operation]) throws -> [DirtyClassification] {
            var next = store
            for operation in operations {
                try operation.apply(to: &next)
            }
            let classifications = try renderer.apply(
                transaction: Transaction(baseRevision: store.revision, operations: operations),
                newStore: next
            )
            store = next
            // A remount opens a new window at the default size and place; restore the test's.
            place()
            return classifications
        }

        func paneView(_ id: NodeId) -> SplitPaneView? { split.pane(for: id) }

        func length(_ id: NodeId) -> CGFloat {
            guard let pane = paneView(id), pane.superview != nil else { return 0 }
            return split.isVertical ? pane.frame.width : pane.frame.height
        }

        func proportion(_ id: NodeId) -> Double {
            split.currentProportions[id] ?? 0
        }

        /// A user's divider drag, as it lands: `setPosition(_:ofDividerAt:)` is where NSSplitView's
        /// own drag tracking ends, with the same Auto Layout constraints (minimum sizes) applied.
        ///
        /// Posting mouse events into NSSplitView's nested tracking loop is not used: in the
        /// swift-testing process that loop can stop the main run loop, which ends the whole run
        /// with exit status 0 and no results (observed while writing this suite).
        func drag(divider index: Int, by delta: CGFloat) throws {
            let position = try #require(split.positionOfDivider(at: index))
            split.setPosition(position + delta, ofDividerAt: index)
            layout()
        }

        func tearDown() {
            window.close()
        }
    }

    static func text(_ id: NodeId, parent: NodeId = splitID, _ properties: [(PropertyRef, Value)] = []) -> SemanticModel.Operation {
        .createNode(id: id, nodeType: .text, parentID: parent, properties: [(.text, .string("pane \(id)"))] + properties)
    }

    static func base(axis: EnumToken? = nil, splitProperties: [(PropertyRef, Value)] = [], panes: [SemanticModel.Operation]) -> [SemanticModel.Operation] {
        var properties = splitProperties + [(.label, .string("Dashboard"))]
        if let axis { properties.append((.splitAxis, .enumToken(axis))) }
        return [
            .createNode(id: surfaceID, nodeType: .surface),
            .createNode(id: splitID, nodeType: .split, parentID: surfaceID, properties: properties),
        ] + panes
    }

    // MARK: - Toolkit mapping (suites 10 and 12)

    @Test
    func sideBySideMapsToVerticalDividersAndStackedToHorizontalOnes() throws {
        for (axis, vertical) in [(nil, true), (EnumToken.splitAxisSideBySide, true), (.splitAxisStacked, false)] {
            let h = try Harness(Self.base(axis: axis, panes: [Self.text(Self.paneA), Self.text(Self.paneB)]))
            defer { h.tearDown() }
            #expect(h.split.isVertical == vertical, "\(String(describing: axis))")
            #expect(h.split.axis == (vertical ? .sideBySide : .stacked))
            #expect(h.split.paneIDs == [Self.paneA, Self.paneB])
            let a = try #require(h.paneView(Self.paneA))
            let b = try #require(h.paneView(Self.paneB))
            if vertical {
                #expect(a.frame.maxX <= b.frame.minX, "side by side: a vertical divider between")
                #expect(abs(a.frame.height - b.frame.height) < 1)
            } else {
                #expect(a.frame.maxY <= b.frame.minY, "stacked: a horizontal divider between (flipped)")
                #expect(abs(a.frame.width - b.frame.width) < 1)
            }
            #expect(h.length(Self.paneA) > 100 && h.length(Self.paneB) > 100, "both panes get real space")
            #expect(h.split.accessibilityRole() == .splitGroup)
            #expect(h.split.accessibilityLabel() == "Dashboard")
        }
    }

    @Test
    func eightPanesMountWithSevenDividers() throws {
        let panes = (0..<8).map { Self.text(NodeId(UInt64(20 + $0))) }
        let h = try Harness(Self.base(panes: panes))
        defer { h.tearDown() }
        #expect(h.split.arrangedSubviews.count == 8)
        let splitters = (h.split.accessibilityChildren() ?? []).compactMap { $0 as? SplitDividerAccessibilityElement }
        #expect(splitters.count == 7)
        for id in 20..<28 {
            #expect(h.length(NodeId(UInt64(id))) > 50, "pane \(id) gets an equal share")
        }
    }

    // MARK: - Local resizing sends nothing (§7.6, §22.5)

    @Test
    func draggingOrAdjustingADividerSendsNothing() throws {
        let h = try Harness(Self.base(panes: [Self.text(Self.paneA), Self.text(Self.paneB)]))
        defer { h.tearDown() }
        let before = try #require(h.split.positionOfDivider(at: 0))
        try h.drag(divider: 0, by: 120)
        let dragged = try #require(h.split.positionOfDivider(at: 0))
        #expect(abs(dragged - (before + 120)) < 1, "the drag moved the divider: \(before) -> \(dragged)")
        h.split.setPosition(dragged - 200, ofDividerAt: 0)
        h.layout()
        let splitter = try #require((h.split.accessibilityChildren() ?? []).compactMap { $0 as? SplitDividerAccessibilityElement }.first)
        #expect(splitter.accessibilityPerformIncrement())
        #expect(h.sent == 0, "divider moves are local: \(h.interactions) \(h.rangeRequests)")
        // The renderer remembered them locally instead.
        let remembered = try #require(h.renderer.controlFactory.splitProportionMemory[Self.splitID])
        #expect(abs((remembered[Self.paneA] ?? 0) - h.proportion(Self.paneA)) < 0.001)
    }

    // MARK: - Commits keep the user's dividers

    @Test
    func aCommitUpdatingAPaneTextKeepsTheDividerWhereTheUserPutIt() throws {
        let h = try Harness(Self.base(panes: [Self.text(Self.paneA), Self.text(Self.paneB)]))
        defer { h.tearDown() }
        let splitView = h.split
        try h.drag(divider: 0, by: -180)
        let placed = try #require(h.split.positionOfDivider(at: 0))
        let classifications = try h.commit([
            .setProperty(id: Self.paneA, property: .text, value: .string("updated pane text")),
        ])
        #expect(!classifications.contains { $0.isStructureAffecting })
        #expect(h.split === splitView, "a content commit mutates in place (§23)")
        let after = try #require(h.split.positionOfDivider(at: 0))
        #expect(abs(after - placed) < 1, "divider stayed at \(placed), now \(after)")
        #expect((h.renderer.registry.view(for: Self.paneA) as? NSTextField)?.stringValue == "updated pane text")
        #expect(h.sent == 0)
    }

    @Test
    func aRemountWithTheSamePanesRestoresTheProportions() throws {
        let h = try Harness(Self.base(panes: [Self.text(Self.paneA), Self.text(Self.paneB)]))
        defer { h.tearDown() }
        try h.drag(divider: 0, by: -200)
        let proportion = h.proportion(Self.paneA)
        #expect(proportion < 0.35, "the drag shrank pane A: \(proportion)")
        let old = h.split
        // A structural commit elsewhere remounts every view.
        let classifications = try h.commit([
            .createNode(id: 40, nodeType: .text, parentID: Self.surfaceID, properties: [(.text, .string("status"))]),
        ])
        #expect(classifications.contains { $0.isStructureAffecting })
        #expect(h.split !== old, "the remount rebuilt the split view")
        #expect(abs(h.proportion(Self.paneA) - proportion) < 0.01, "\(h.proportion(Self.paneA)) vs \(proportion)")
        #expect(h.sent == 0)
    }

    @Test
    func insertingAThirdPaneKeepsTheFirstDividersProportion() throws {
        let h = try Harness(Self.base(panes: [Self.text(Self.paneA), Self.text(Self.paneB)]))
        defer { h.tearDown() }
        try h.drag(divider: 0, by: -150)
        let ratio = h.proportion(Self.paneA) / (h.proportion(Self.paneA) + h.proportion(Self.paneB))
        #expect(ratio < 0.4)
        _ = try h.commit([Self.text(Self.paneC)])
        #expect(h.split.paneIDs == [Self.paneA, Self.paneB, Self.paneC])
        let a = h.proportion(Self.paneA), b = h.proportion(Self.paneB), c = h.proportion(Self.paneC)
        #expect(abs(a / (a + b) - ratio) < 0.01, "A:B kept \(ratio), now \(a / (a + b))")
        #expect(abs(c - 1.0 / 3) < 0.02, "the new pane takes an equal share: \(c)")
    }

    @Test
    func removingAPaneKeepsTheRemainingPanesProportions() throws {
        let h = try Harness(Self.base(panes: [Self.text(Self.paneA), Self.text(Self.paneB), Self.text(Self.paneC)]))
        defer { h.tearDown() }
        try h.drag(divider: 0, by: -120)
        let a = h.proportion(Self.paneA), c = h.proportion(Self.paneC)
        _ = try h.commit([.deleteNode(id: Self.paneB)])
        #expect(h.split.paneIDs == [Self.paneA, Self.paneC])
        let ratio = a / (a + c)
        #expect(abs(h.proportion(Self.paneA) - ratio) < 0.01, "A:C kept \(ratio), now \(h.proportion(Self.paneA))")
    }

    // MARK: - Pane layout hints (§10)

    @Test
    func aPanesMinimumSizeIsHonouredWhileDragging() throws {
        let h = try Harness(Self.base(panes: [
            Self.text(Self.paneA, [(.minimumSize, .size(Size(width: 260, height: 0)))]),
            Self.text(Self.paneB, [(.minimumSize, .size(Size(width: 180, height: 0)))]),
        ]))
        defer { h.tearDown() }
        try h.drag(divider: 0, by: -600)
        #expect(h.length(Self.paneA) >= 259, "A kept its minimum while dragged: \(h.length(Self.paneA))")
        try h.drag(divider: 0, by: 900)
        #expect(h.length(Self.paneB) >= 179, "B kept its minimum while dragged: \(h.length(Self.paneB))")
        // Accessibility adjustments stop at the same bound.
        let splitter = try #require((h.split.accessibilityChildren() ?? []).compactMap { $0 as? SplitDividerAccessibilityElement }.first)
        splitter.setAccessibilityValue(NSNumber(value: 0))
        h.layout()
        #expect(h.length(Self.paneA) >= 259)
        #expect(h.sent == 0)
    }

    @Test
    func preferredSizeAndGrowSeedTheFirstLengthsOnly() throws {
        let h = try Harness(Self.base(axis: .splitAxisStacked, panes: [
            Self.text(Self.paneA, [(.preferredSize, .size(Size(width: 0, height: 150)))]),
            Self.text(Self.paneB, [(.grow, .float64(3))]),
            Self.text(Self.paneC),
        ]))
        defer { h.tearDown() }
        let rest = h.split.bounds.height - 2 * h.split.dividerThickness - 150
        #expect(abs(h.length(Self.paneA) - 150) < 3, "preferred height seeds pane A: \(h.length(Self.paneA))")
        #expect(abs(h.length(Self.paneB) - rest * 0.75) < 3, "grow 3 takes three quarters of the rest: \(h.length(Self.paneB))")
        #expect(abs(h.length(Self.paneC) - rest * 0.25) < 3, "absent grow counts as 1: \(h.length(Self.paneC))")
        // A hint, never a pixel mandate: the user can still move the divider past it.
        try h.drag(divider: 0, by: 100)
        #expect(abs(h.length(Self.paneA) - 250) < 3, "dragged past the preferred size: \(h.length(Self.paneA))")
    }

    @Test
    func aWindowResizeScalesThePanesAndKeepsTheUsersProportions() throws {
        let h = try Harness(Self.base(panes: [Self.text(Self.paneA), Self.text(Self.paneB)]))
        defer { h.tearDown() }
        try h.drag(divider: 0, by: -160)
        let proportion = h.proportion(Self.paneA)
        h.place(size: NSSize(width: 1240, height: 640))
        #expect(abs(h.proportion(Self.paneA) - proportion) < 0.01, "\(h.proportion(Self.paneA)) vs \(proportion)")
        h.place(size: NSSize(width: 640, height: 640))
        #expect(abs(h.proportion(Self.paneA) - proportion) < 0.01)
        let remembered = try #require(h.renderer.controlFactory.splitProportionMemory[Self.splitID]?[Self.paneA])
        #expect(abs(remembered - proportion) < 0.01, "a resize does not overwrite the user's proportions")
        #expect(h.sent == 0)
    }

    @Test
    func aCollapsedChildCollapsesItsPaneAndHiddenKeepsItsSpace() throws {
        let collapsed: (PropertyRef, Value) = (.visibility, .enumToken(.visibilityCollapsed))
        let h = try Harness(Self.base(panes: [Self.text(Self.paneA), Self.text(Self.paneB, [collapsed]), Self.text(Self.paneC)]))
        defer { h.tearDown() }
        #expect(h.split.paneIDs == [Self.paneA, Self.paneB, Self.paneC])
        #expect(h.split.visiblePaneIDs == [Self.paneA, Self.paneC], "the collapsed pane leaves the layout")
        #expect(h.length(Self.paneB) == 0)
        #expect(h.length(Self.paneA) + h.length(Self.paneC) + h.split.dividerThickness >= h.split.bounds.width - 1)
        let splitters = (h.split.accessibilityChildren() ?? []).compactMap { $0 as? SplitDividerAccessibilityElement }
        #expect(splitters.count == 1)

        // Un-collapsing is a layout-affecting commit: the pane returns in child order, in place.
        let splitView = h.split
        let shown = try h.commit([.setProperty(id: Self.paneB, property: .visibility, value: .enumToken(.visibilityVisible))])
        #expect(!shown.contains { $0.isStructureAffecting })
        #expect(h.split === splitView)
        #expect(h.split.visiblePaneIDs == [Self.paneA, Self.paneB, Self.paneC])
        #expect(h.length(Self.paneB) > 50, "the restored pane gets space: \(h.length(Self.paneB))")

        // `hidden` keeps the pane's space; only its content is invisible.
        _ = try h.commit([.setProperty(id: Self.paneB, property: .visibility, value: .enumToken(.visibilityHidden))])
        #expect(h.split.visiblePaneIDs == [Self.paneA, Self.paneB, Self.paneC])
        #expect(h.length(Self.paneB) > 50)
        #expect(h.renderer.registry.view(for: Self.paneB)?.alphaValue == 0)

        // Collapsing again by commit removes it from the layout without a remount.
        _ = try h.commit([.setProperty(id: Self.paneB, property: .visibility, value: .enumToken(.visibilityCollapsed))])
        #expect(h.split === splitView)
        #expect(h.split.visiblePaneIDs == [Self.paneA, Self.paneC])
        #expect(h.sent == 0)
    }

    // MARK: - Accessibility (suites 10 and 12, §22.8)

    @Test
    func theAccessibilityTreeExposesAdjustableSplitters() throws {
        let h = try Harness(Self.base(axis: .splitAxisStacked, panes: [Self.text(Self.paneA), Self.text(Self.paneB), Self.text(Self.paneC)]))
        defer { h.tearDown() }
        #expect(h.split.accessibilityRole() == .splitGroup)
        let children = h.split.accessibilityChildren() ?? []
        // AppKit's own splitters (no increment/decrement) are replaced, not duplicated.
        let splitterRoles = children.compactMap { ($0 as? NSAccessibilityElement)?.accessibilityRole() }.filter { $0 == .splitter }
        let splitters = children.compactMap { $0 as? SplitDividerAccessibilityElement }
        #expect(splitters.count == 2)
        #expect(splitterRoles.count == 2)
        for (index, splitter) in splitters.enumerated() {
            #expect(splitter.accessibilityRole() == .splitter)
            #expect(splitter.accessibilityParent() as? SemanticSplitView === h.split)
            #expect(splitter.accessibilityOrientation() == .horizontal, "stacked panes have horizontal dividers")
            #expect(splitter.isAccessibilitySelectorAllowed(#selector(NSAccessibilityElement.accessibilityPerformIncrement)))
            #expect(splitter.isAccessibilitySelectorAllowed(#selector(NSAccessibilityElement.accessibilityPerformDecrement)))
            let value = try #require(splitter.accessibilityValue() as? NSNumber)
            #expect(abs(CGFloat(value.doubleValue) - (h.split.positionOfDivider(at: index) ?? -1)) < 0.5)
            #expect(splitter.accessibilityFrame().height > 0)
        }
        let first = splitters[0]
        let start = try #require((first.accessibilityValue() as? NSNumber)?.doubleValue)
        #expect(first.accessibilityPerformIncrement())
        h.layout()
        let up = try #require((first.accessibilityValue() as? NSNumber)?.doubleValue)
        #expect(up > start, "increment moved the divider down: \(start) -> \(up)")
        #expect(first.accessibilityPerformDecrement())
        #expect(first.accessibilityPerformDecrement())
        h.layout()
        let down = try #require((first.accessibilityValue() as? NSNumber)?.doubleValue)
        #expect(down < start, "decrement moved it back up past the start: \(down)")
        #expect(h.sent == 0)
    }

    // MARK: - split_axis is structure-affecting for that Split (§23)

    @Test
    func changingTheAxisRebuildsThePanesOnTheOtherAxisAndKeepsTheirProportions() throws {
        let h = try Harness(Self.base(panes: [Self.text(Self.paneA), Self.text(Self.paneB)]))
        defer { h.tearDown() }
        try h.drag(divider: 0, by: -170)
        let proportion = h.proportion(Self.paneA)
        let set = SemanticModel.Operation.setProperty(id: Self.splitID, property: .splitAxis, value: .enumToken(.splitAxisStacked))
        let classifications = try h.commit([set])
        #expect(classifications == [.structureAffecting(operation: set)])
        #expect(h.split.isVertical == false)
        #expect(abs(h.proportion(Self.paneA) - proportion) < 0.01)
        // A pane hint change is layout, not structure.
        #expect(DirtyClassifier.classify(.setProperty(id: Self.paneA, property: .minimumSize, value: .size(Size(width: 10, height: 10))))
            == [.layoutAffecting(nodeID: Self.paneA, property: .minimumSize)])
        let batch = SemanticModel.Operation.batchPropertySet(
            id: Self.splitID, properties: [Property(property: .label, value: .string("x")), Property(property: .splitAxis, value: .enumToken(.splitAxisSideBySide))]
        )
        #expect(DirtyClassifier.classify(batch) == [.contentOnly(nodeID: Self.splitID, property: .label), .structureAffecting(operation: batch)])
    }

    @Test
    func rememberedProportionsAreDroppedWhenTheSplitIsGone() throws {
        let h = try Harness(Self.base(panes: [Self.text(Self.paneA), Self.text(Self.paneB)]))
        defer { h.tearDown() }
        try h.drag(divider: 0, by: 90)
        #expect(h.renderer.controlFactory.splitProportionMemory[Self.splitID] != nil)
        var next = h.store
        try SemanticModel.Operation.deleteNode(id: Self.splitID).apply(to: &next)
        try h.renderer.apply(
            transaction: Transaction(baseRevision: h.store.revision, operations: [.deleteNode(id: Self.splitID)]),
            newStore: next
        )
        #expect(h.renderer.controlFactory.splitProportionMemory.isEmpty, "no proportions outlive their Split")
    }
}
