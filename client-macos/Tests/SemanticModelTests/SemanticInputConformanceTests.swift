//
// SemanticInputConformanceTests.swift
// SemanticModelTests
//
// SRUI Semantic-Input Conformance Suite (§32 item 5) — client side.
//
// Implements: §7.6 (semantic events), §7.7 (coordinate events reserved for subscribed scenes),
// §18.3 (text edit sequencing), §32.5.
//
// Drives protocol/conformance-vectors/suites/05-semantic-input/events.generated.json, generated
// from protocol/registry.yaml, so the Rust and Swift halves of this suite are checked against the
// same oracle rather than two hand-maintained lists that can disagree.
//
// SemanticModel must never import AppKit or Cocoa.
//
// ## Documented gap
//
// The positive half of §32.5 — coordinates ACCEPTED for an explicitly subscribed custom scene —
// has no implementation on this base (no VectorScene node type, no subscription model). That is
// Task 36 and is recorded in the suite 5 manifest entry.
//

import XCTest
import Foundation
@testable import SemanticModel

struct EventMatrix: Decodable {
    let semanticEvents: [String]
    let coordinateEvents: [String]
    let nodeEventMatrix: [NodeEventRow]

    enum CodingKeys: String, CodingKey {
        case semanticEvents = "semantic_events"
        case coordinateEvents = "coordinate_events"
        case nodeEventMatrix = "node_event_matrix"
    }

    struct NodeEventRow: Decodable {
        let node: String
        let tier: String
        let allowed: [String]
        let forbidden: [String]
    }
}

final class SemanticInputConformanceTests: XCTestCase {

    private func loadMatrix() throws -> EventMatrix {
        let url = try ConformanceVectors.generated(
            forSuite: 5, named: "events.generated.json")
        return try JSONDecoder().decode(EventMatrix.self, from: Data(contentsOf: url))
    }

    /// The semantic/coordinate partition must be total and disjoint over the registered events.
    func testEventPartitionIsTotalAndDisjoint() throws {
        let matrix = try loadMatrix()

        XCTAssertEqual(
            matrix.semanticEvents.count + matrix.coordinateEvents.count,
            standardEventsTable.count,
            "Event matrix partitions a different number of events than the registry declares")

        for semantic in matrix.semanticEvents {
            XCTAssertFalse(
                matrix.coordinateEvents.contains(semantic),
                "Event '\(semantic)' is classified as both semantic and coordinate")
        }

        for entry in standardEventsTable {
            XCTAssertTrue(
                matrix.semanticEvents.contains(entry.name)
                    || matrix.coordinateEvents.contains(entry.name),
                "Registered event '\(entry.name)' is absent from the matrix")
        }
    }

    /// Every event in the matrix must resolve through the client's own registry tables, so the
    /// two implementations agree on identity, not just on spelling.
    func testEveryMatrixEventResolvesThroughClientRegistry() throws {
        let matrix = try loadMatrix()

        for name in matrix.semanticEvents + matrix.coordinateEvents {
            let id = lookupStandardEvent(name)
            XCTAssertNotNil(id, "Event '\(name)' does not resolve in the client registry table")
            if let id {
                XCTAssertEqual(
                    lookupStandardEventName(id), name,
                    "Event '\(name)' does not round-trip through the client registry table")
            }
        }
    }

    /// §7.7: coordinate events are exactly the POINTER_* family.
    func testCoordinateEventsAreExactlyThePointerFamily() throws {
        let matrix = try loadMatrix()

        for coordinate in matrix.coordinateEvents {
            XCTAssertTrue(
                coordinate.hasPrefix("POINTER_"),
                "Coordinate event '\(coordinate)' is not a POINTER_* event")
        }
        for semantic in matrix.semanticEvents {
            XCTAssertFalse(
                semantic.hasPrefix("POINTER_"),
                "Semantic event '\(semantic)' must not be a POINTER_* event")
        }
        XCTAssertEqual(
            matrix.coordinateEvents.count, 5,
            "§7.7 defines five coordinate events (down/up/move/cancel/scroll)")
    }

    /// The core §32.5 assertion: no Standard Widget Profile node type may originate a coordinate
    /// event. Ordinary controls report what the user meant, not where the pointer was.
    func testNoStandardNodeTypeEmitsCoordinateEvents() throws {
        let matrix = try loadMatrix()

        for row in matrix.nodeEventMatrix {
            for allowed in row.allowed {
                XCTAssertFalse(
                    matrix.coordinateEvents.contains(allowed),
                    "Node type '\(row.node)' is allowed to emit coordinate event '\(allowed)'; coordinates are reserved for explicitly subscribed custom scenes (§7.7, §32.5)"
                )
            }
            XCTAssertEqual(
                row.forbidden, matrix.coordinateEvents,
                "Node type '\(row.node)' must forbid the complete coordinate event set")
        }
    }

    /// §7.6: the interactive required-tier widgets carry the semantics §32.5 names explicitly —
    /// activation, boolean change, collection selection and text editing, all coordinate-free.
    func testRequiredInteractiveWidgetsEmitCoordinateFreeSemantics() throws {
        let matrix = try loadMatrix()
        func allowed(_ node: String) throws -> [String] {
            let row = try XCTUnwrap(
                matrix.nodeEventMatrix.first { $0.node == node },
                "Event matrix is missing node type '\(node)'")
            return row.allowed
        }

        XCTAssertEqual(try allowed("Button"), ["ACTIVATE"])
        XCTAssertEqual(try allowed("Toggle"), ["VALUE_CHANGED"])
        XCTAssertEqual(try allowed("List"), ["SELECTION_CHANGED"])
        XCTAssertEqual(try allowed("Table"), ["SELECTION_CHANGED"])
        XCTAssertEqual(try allowed("Tree"), ["SELECTION_CHANGED", "EXPANSION_CHANGED"])
        XCTAssertEqual(try allowed("TextInput"), ["TEXT_EDIT"])
        XCTAssertEqual(try allowed("TextArea"), ["TEXT_EDIT"])
    }

    /// §18.3 / §22.6: only the two text controls originate TEXT_EDIT, which is what keeps
    /// `edit_seq` sequencing scoped to text rather than leaking into every control.
    func testTextEditIsScopedToTextControls() throws {
        let matrix = try loadMatrix()

        let textEmitters = matrix.nodeEventMatrix
            .filter { $0.allowed.contains("TEXT_EDIT") }
            .map(\.node)
        XCTAssertEqual(
            textEmitters, ["TextInput", "TextArea"],
            "Only the two text controls may originate TEXT_EDIT (§7.6)")
    }

    /// Executable form of the Task 36 gap: while no scene subscription model exists, nothing may
    /// claim coordinate capability. If this ever fails, the manifest gap entry must be updated
    /// rather than the assertion relaxed.
    func testSubscribedSceneCoordinatePathIsAbsent() throws {
        let matrix = try loadMatrix()

        let coordinateCapable = matrix.nodeEventMatrix
            .filter { $0.allowed.contains { $0.hasPrefix("POINTER_") } }
            .map(\.node)
        XCTAssertTrue(
            coordinateCapable.isEmpty,
            "No subscribed-scene node type exists yet, so nothing may accept coordinates; found \(coordinateCapable). If Task 36 added one, move the suite 5 gap entry in the conformance manifest to active coverage."
        )
    }
}
