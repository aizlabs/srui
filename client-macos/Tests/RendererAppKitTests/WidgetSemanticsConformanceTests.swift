//
// WidgetSemanticsConformanceTests.swift
// RendererAppKitTests
//
// SRUI Widget Semantics Conformance Suite (§32 item 2) — renderer side.
//
// Implements: §7.2 (standard node types), §7.3 (implementation tiers), §7.6 (semantic events),
// §4 inv. 13 (unknown required semantics fail explicitly), §32.2.
//
// Drives protocol/conformance-vectors/suites/02-widget-semantics/widgets.generated.json, which is
// generated from protocol/registry.yaml. `ControlFactoryTests` keeps its own hand-written tier
// lists for readability; this suite is what binds those lists to the registry, so promoting a
// widget from `should` to `required` fails here until the renderer actually implements it.
//

import AppKit
import Foundation
import SemanticModel
import Testing

@testable import RendererAppKit

@MainActor
struct WidgetSemanticsConformanceTests {

    @Test
    func widgetMatrixCoversEveryStandardNodeType() throws {
        let matrix = try ConformanceFixtures.widgetMatrix()

        #expect(matrix.nodeTypes.count == standardNodeTypesTable.count)

        for row in matrix.nodeTypes {
            let known = standardNodeTypesTable.first { $0.id == UInt32(row.id) }
            let entry = try #require(
                known, "Widget matrix lists unknown node type id \(row.id)")
            #expect(
                entry.name == row.name,
                "Node type id \(row.id) is '\(entry.name)' in the client registry but '\(row.name)' in the matrix"
            )
        }
    }

    /// §7.3 + §4 inv. 13: every required-tier node type must construct, and every node type
    /// outside the required tier must be refused outright rather than silently approximated.
    @Test
    func rendererImplementsExactlyTheRequiredTier() throws {
        let matrix = try ConformanceFixtures.widgetMatrix()
        let factory = ControlFactory()

        for row in matrix.nodeTypes {
            let nodeType = TypeRef.standard(UInt32(row.id))
            let node = Node(id: 1, nodeType: nodeType)

            if row.expectConstructible {
                let handle = try factory.makeHandle(for: node)
                #expect(
                    handle.nodeType == nodeType,
                    "Required-tier '\(row.name)' produced a handle for the wrong node type")
            } else {
                #expect(throws: ControlFactoryError.self) {
                    _ = try factory.makeHandle(for: node)
                }
            }
        }
    }

    /// The tier partition itself: 18 required-tier node types, and `expect_constructible` tracks
    /// exactly that set.
    @Test
    func tierPartitionMatchesTheRegistry() throws {
        let matrix = try ConformanceFixtures.widgetMatrix()

        let required = matrix.nodeTypes.filter { $0.tier == "required" }
        #expect(required.count == 18, "§7.3 defines 18 required-tier node types")

        for row in matrix.nodeTypes {
            #expect(
                row.expectConstructible == (row.tier == "required"),
                "Node type '\(row.name)' (tier '\(row.tier)') has expect_constructible=\(row.expectConstructible)"
            )
        }
    }

    /// §7.6: declared emissions must be registered semantic events, never coordinate events.
    @Test
    func widgetEmissionsAreRegisteredSemanticEvents() throws {
        let matrix = try ConformanceFixtures.widgetMatrix()

        for row in matrix.nodeTypes {
            for event in row.emits {
                #expect(
                    lookupStandardEvent(event) != nil,
                    "Node type '\(row.name)' declares emission '\(event)', which is not a registered standard event"
                )
                #expect(
                    !event.hasPrefix("POINTER_"),
                    "Node type '\(row.name)' must not emit coordinate event '\(event)' (§7.7, §32.5)"
                )
            }
        }
    }
}
