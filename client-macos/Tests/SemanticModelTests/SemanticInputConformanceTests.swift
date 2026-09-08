//
// SemanticInputConformanceTests.swift
// SemanticModelTests
//
// SRUI Semantic-Input Conformance Suite (§32 item 5) — protocol-model half.
//
// Implements: §7.6 (semantic events), §7.7 (coordinate events reserved for subscribed scenes),
// §32.5.
//
// Asserts the client's own generated registry table (`standardEventsTable`, produced by
// protocol/generate_swift_registry.py) agrees with the §7.7 coordinate family. That is a genuine
// cross-language check: it fails if the Swift table drifts from the Rust one. No intermediate
// fixture is introduced — the renderer-side behaviour half lives in
// RendererAppKitTests/WidgetSemanticsConformanceTests.swift, which needs AppKit.
//
// SemanticModel must never import AppKit or Cocoa.
//

import XCTest
import Foundation
@testable import SemanticModel

final class SemanticInputConformanceTests: XCTestCase {

    /// The §7.7 coordinate family. Cross-checked against `registry.yaml` by
    /// `protocol/tests/test_conformance_manifest.py`, and against the Rust table by the
    /// equivalent constant in `conformance_semantic_input_test.rs`.
    private static let coordinateEvents: Set<String> = [
        "POINTER_DOWN", "POINTER_UP", "POINTER_MOVE", "POINTER_CANCEL", "POINTER_SCROLL",
    ]

    /// §7.6/§7.7: the registered table partitions cleanly into semantic and coordinate events.
    func testEventTablePartitionsIntoSemanticAndCoordinateFamilies() {
        let registered = Set(standardEventsTable.map(\.name))

        for coordinate in Self.coordinateEvents {
            XCTAssertTrue(
                registered.contains(coordinate),
                "coordinate event '\(coordinate)' is not registered in the client table")
        }

        for entry in standardEventsTable {
            XCTAssertEqual(
                entry.name.hasPrefix("POINTER_"),
                Self.coordinateEvents.contains(entry.name),
                "event '\(entry.name)' disagrees with the §7.7 coordinate family")
        }

        XCTAssertEqual(
            registered.count - Self.coordinateEvents.count, 6,
            "§7.6 defines six semantic events")
    }

    /// Every registered event must round-trip through the client's lookup helpers, so an event
    /// the server can send is one the client can name.
    func testEveryRegisteredEventRoundTrips() {
        for entry in standardEventsTable {
            XCTAssertEqual(
                lookupStandardEvent(entry.name), entry.id,
                "event '\(entry.name)' does not resolve to its registered id")
            XCTAssertEqual(
                lookupStandardEventName(entry.id), entry.name,
                "event id \(entry.id) does not resolve back to '\(entry.name)'")
        }
    }
}
