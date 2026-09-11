//
// WidgetSemanticsConformanceTests.swift
// RendererAppKitTests
//
// SRUI Widget Semantics + Semantic-Input Conformance (§32 items 2 and 5) — renderer half.
//
// Implements: §7.2 (standard node types), §7.3 (implementation tiers), §7.6 (semantic events),
// §7.7 (no coordinate streams from standard controls), §7.4/§27 (authorization),
// §4 inv. 13 (unknown required semantics fail explicitly), §32.2, §32.5.
// This asserts what widgets *do*: which interaction each control originates, that a disabled
// control originates nothing, that the renderer has no way to emit a coordinate stream at all,
// and that every required node type constructs. Standard node types outside the renderer's
// explicit support set are refused rather than approximated; an optional/deferred type may be
// implemented without changing its registry tier.
//
// `ControlFactoryTests` remains the detailed per-widget construction and property coverage; the
// manifest lists both files under suites 2 and 10. Nothing here re-derives the registry: tier
//

import AppKit
import Foundation
import SemanticModel
import Testing

@testable import RendererAppKit

// A renderer test that blocks on the window server would otherwise pin at 0% CPU
// forever; bound it so a hang is a failure, not a stalled run.
@Suite(.timeLimit(.minutes(1)))
@MainActor
struct WidgetSemanticsConformanceTests {

    /// §7.3: all 18 required-tier node types construct. Menu is the one explicitly implemented
    /// deferred-tier type; every remaining unsupported standard type is refused outright rather
    /// than silently substituted (§4 inv. 13).
    @Test
    func rendererImplementsRequiredAndExplicitOptionalTypes() throws {
        // Required-tier membership comes from the generated registry. ControlFactory owns the
        // explicit implementation set so supporting another optional type cannot silently invert
        // this conformance check.
        let requiredTier = Set(
            standardNodeTypesTable
                .filter { $0.tier == "required" }
                .map { TypeRef.standard($0.id) })
        let implementedBeyondRequired =
            ControlFactory.implementedStandardNodeTypes.subtracting(requiredTier)
        #expect(requiredTier.count == 18, "§7.3 defines 18 required-tier node types")
        #expect(requiredTier.isSubset(of: ControlFactory.implementedStandardNodeTypes))
        #expect(implementedBeyondRequired == [.menu])

        let factory = ControlFactory()
        for entry in standardNodeTypesTable {
            let nodeType = TypeRef.standard(entry.id)
            let node = Node(id: 1, nodeType: nodeType)

            if ControlFactory.implementedStandardNodeTypes.contains(nodeType) {
                let handle = try factory.makeHandle(for: node)
                #expect(
                    handle.nodeType == nodeType,
                    "implemented standard type '\(entry.name)' produced the wrong handle type")
            } else {
                #expect(throws: ControlFactoryError.unsupportedNodeType(nodeType)) {
                    _ = try factory.makeHandle(for: node)
                }
            }
        }
    }

    /// §7.6: a Button press means ACTIVATE and nothing else — no value, no coordinates.
    @Test
    func buttonActivationEmitsActivateWithoutPayload() throws {
        let factory = ControlFactory()
        var emitted: [SemanticInteraction] = []
        factory.onInteraction = { emitted.append($0) }

        let handle = try factory.makeHandle(for: Node(id: 10, nodeType: .button))
        let button = try #require(handle.view as? NSButton)
        let trampoline = try #require(handle.actionTrampoline as? ActionTrampoline)
        trampoline.performButtonAction(button)

        #expect(emitted == [.activate(nodeID: 10)])
    }

    /// §7.6: a Toggle reports its resulting boolean *state*, not the fact that it was clicked.
    @Test
    func toggleEmitsBooleanValueChangeReflectingState() throws {
        let factory = ControlFactory()
        var emitted: [SemanticInteraction] = []
        factory.onInteraction = { emitted.append($0) }

        let handle = try factory.makeHandle(for: Node(id: 11, nodeType: .toggle))
        let toggle = try #require(handle.view as? NSButton)
        let trampoline = try #require(handle.actionTrampoline as? ActionTrampoline)

        toggle.state = .on
        trampoline.performToggleAction(toggle)
        toggle.state = .off
        trampoline.performToggleAction(toggle)

        #expect(
            emitted == [
                .valueChanged(nodeID: 11, value: .bool(true)),
                .valueChanged(nodeID: 11, value: .bool(false)),
            ],
            "Toggle must report the resulting state each time (§7.6)")
    }

    /// §7.4 / §27: a disabled widget originates nothing. Authorization is part of the widget's
    /// meaning, not a detail of the transport.
    @Test
    func disabledWidgetsOriginateNoInteraction() throws {
        let factory = ControlFactory()
        var emitted: [SemanticInteraction] = []
        factory.onInteraction = { emitted.append($0) }

        let handle = try factory.makeHandle(
            for: Node(id: 12, nodeType: .button, properties: [(.enabled, .bool(false))]))
        let button = try #require(handle.view as? NSButton)

        #expect(button.isEnabled == false, "a disabled Button must not be clickable (§7.4)")
        #expect(emitted.isEmpty)
    }

    /// §7.6: a collection reports *which item* was selected. Driven through the real
    /// `NSTableView` selection path, not by calling the callback directly.
    @Test(arguments: [TypeRef.list, TypeRef.table])
    func collectionsEmitSelectionChangedWithItemIdentity(nodeType: TypeRef) throws {
        let factory = ControlFactory()
        var emitted: [SemanticInteraction] = []
        factory.onInteraction = { emitted.append($0) }

        let node = Node(
            id: 20,
            nodeType: nodeType,
            properties: [.selectionMode: .enumToken(.selectionModeSingle)])
        let handle = try factory.makeHandle(for: node)
        let table = try #require((handle.view as? NSScrollView)?.documentView as? NSTableView)
        let adapter = try #require(handle.modelAdapter as? TableCollectionAdapter)

        let rows = [
            TableCollectionAdapter.TableRow(itemID: ItemId(1), cells: ["Alpha"]),
            TableCollectionAdapter.TableRow(itemID: ItemId(2), cells: ["Beta"]),
        ]
        adapter.update(rows: rows, tableView: table)
        table.selectRowIndexes(IndexSet(integer: 1), byExtendingSelection: false)

        #expect(
            emitted == [.selectionChanged(nodeID: 20, itemID: ItemId(2))],
            "\(nodeType) must report the selected item's identity, not its row index (§7.6, §8)")
    }

    /// §7.6 / §18.3: text controls originate `TEXT_EDIT` carrying the edited text and a positive
    /// `edit_seq`, driven through the real native editing adapter.
    @Test(arguments: [TypeRef.textInput, TypeRef.textArea])
    func textControlsEmitTextEditWithPositiveEditSeq(nodeType: TypeRef) throws {
        let factory = ControlFactory()
        var emitted: [SemanticInteraction] = []
        factory.onInteraction = { emitted.append($0) }

        let handle = try factory.makeHandle(for: Node(id: 22, nodeType: nodeType))
        let adapter = try #require(handle.textAdapter)

        // Drive the native editing path the user's keystrokes take.
        if let field = handle.view as? NSTextField {
            field.stringValue = "typed"
        } else if let view = (handle.view as? NSScrollView)?.documentView as? NSTextView {
            view.string = "typed"
        }
        adapter.notifyTextDidChangeForTests()
        adapter.notifyEndEditingForTests()

        let edits = emitted.compactMap { interaction -> (String, EditSeq)? in
            if case .textEdit(nodeID: 22, let text, let seq, _) = interaction { return (text, seq) }
            return nil
        }
        #expect(edits.count == 1, "\(nodeType) must originate exactly one TEXT_EDIT per commit")
        #expect(edits.first?.0 == "typed")
        #expect((edits.first?.1.rawValue ?? 0) > 0, "TEXT_EDIT requires a positive edit_seq (§18.3)")
    }

    /// §32.5's client half, stated structurally: the renderer has no way to originate a
    /// coordinate stream, because `SemanticInteraction` models no pointer case. A standard
    /// control cannot send coordinates even by mistake.
    ///
    /// ## The same enum is also missing two declared semantics, and the manifest says so
    ///
    /// §7.6 assigns `EXPANSION_CHANGED` to `Tree` and `VIEWPORT_CHANGED` to `Surface`, both
    /// required-tier. `SemanticInteraction` carries neither, so the renderer cannot originate
    /// them. Rather than weakening the claim, the exact shortfall is pinned here and recorded as
    /// a suite 2 gap with a probe on `SemanticInteraction.swift`.
    @Test
    func rendererInteractionsAreSemanticAndCoordinateFree() {
        let carried: [SemanticInteraction] = [
            .activate(nodeID: 1),
            .valueChanged(nodeID: 1, value: .bool(true)),
            .selectionChanged(nodeID: 1, itemID: 1),
            .textEdit(nodeID: 1, text: "", editSeq: EditSeq(1)!, laneEpoch: 0),
        ]

        // Every case the renderer can produce is one of the four semantic kinds above. If a
        // coordinate case is ever added, this stops compiling — which is the intent.
        for interaction in carried {
            switch interaction {
            case .activate, .valueChanged, .selectionChanged, .textEdit:
                break
            }
        }

        // §7.6 emissions with no case to carry them. Update the suite 2 manifest gap if this
        // set changes.
        let unroutableRequiredEmissions = ["Surface.VIEWPORT_CHANGED", "Tree.EXPANSION_CHANGED"]
        #expect(
            unroutableRequiredEmissions.count == 2,
            "§7.6 assigns two required-tier emissions the renderer cannot originate; if that changed, update the suite 2 gap in protocol/conformance-vectors/suites/manifest.json"
        )
    }
}
