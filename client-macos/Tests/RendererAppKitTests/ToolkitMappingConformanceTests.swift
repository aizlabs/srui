//
// ToolkitMappingConformanceTests.swift
// RendererAppKitTests
//
// SRUI Toolkit Mapping Conformance Suite (§32 item 12) — renderer side.
//
// Implements: §22.4 (native mappings are informative), §32.12.
//
// §22.4 makes this suite deliberately informative: a renderer may change how it realises a
// semantic node without a protocol version change, so nothing here asserts a particular NSView
// subclass. What is asserted is that the shared mapping fixture is complete and honest about what
// the AppKit renderer implements, so a second renderer can adopt it without inheriting stale
// claims.
//

import AppKit
import Foundation
import SemanticModel
import Testing

@testable import RendererAppKit

@MainActor
struct ToolkitMappingConformanceTests {

    @Test
    func mappingFixtureIsInformative() throws {
        #expect(
            try ConformanceFixtures.toolkitMappings().normative == false,
            "Toolkit mappings must be marked non-normative (§22.4)")
    }

    @Test
    func everyStandardNodeTypeHasAMapping() throws {
        let mappings = try ConformanceFixtures.toolkitMappings()

        #expect(mappings.mappings.count == standardNodeTypesTable.count)

        for entry in standardNodeTypesTable {
            let row = try #require(
                mappings.mappings.first { $0.node == entry.name },
                "Standard node type '\(entry.name)' has no toolkit mapping")
            #expect(!row.appkitView.trimmingCharacters(in: .whitespaces).isEmpty)
            #expect(!row.appkitStrategy.trimmingCharacters(in: .whitespaces).isEmpty)
            #expect(!row.category.trimmingCharacters(in: .whitespaces).isEmpty)
        }
    }

    /// The honesty check, and the one that actually exercises the renderer: a mapping claiming
    /// `implemented` must correspond to a node type `ControlFactory` really constructs, and a
    /// mapping claiming otherwise must correspond to one it refuses.
    @Test
    func implementationClaimsMatchRendererBehaviour() throws {
        let mappings = try ConformanceFixtures.toolkitMappings()
        let factory = ControlFactory()

        for row in mappings.mappings {
            let id = try #require(
                lookupStandardNodeType(row.node), "Node type '\(row.node)' does not resolve")
            let node = Node(id: 1, nodeType: TypeRef.standard(id))

            if row.implemented {
                let handle = try factory.makeHandle(for: node)
                #expect(
                    handle.nodeID == 1,
                    "Mapping for '\(row.node)' claims implemented but produced no usable handle")
            } else {
                #expect(throws: ControlFactoryError.self) {
                    _ = try factory.makeHandle(for: node)
                }
            }
        }
    }

    /// Two node types may share an AppKit class (List and Table are both NSTableView); the
    /// strategy text is what has to disambiguate them for a reader of the fixture.
    @Test
    func sharedAppKitClassesAreDisambiguatedByStrategy() throws {
        let mappings = try ConformanceFixtures.toolkitMappings()

        let byView = Dictionary(grouping: mappings.mappings, by: \.appkitView)
        for (view, rows) in byView where rows.count > 1 {
            let strategies = Set(rows.map(\.appkitStrategy))
            #expect(
                strategies.count == rows.count,
                "Node types \(rows.map(\.node)) all map to '\(view)' but do not have distinct strategies"
            )
        }
    }
}
