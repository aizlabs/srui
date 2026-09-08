//
// SemanticNotPaintConformanceTests.swift
// SemanticModelTests
//
// SRUI Semantic-Not-Paint Conformance Suite (§32 item 3).
//
// Implements: §4.7 (no paint commands), §4.16 (no frame cadence), §4.17 (no mandatory pixel
// geometry), §7.1, §10 (local layout), §32.3.
//
// Split out of StateMachineConformanceTests so §32 item 3 is a named, independently runnable
// suite rather than a stowaway in item 1. SemanticModel must never import AppKit or Cocoa.
//

import XCTest
import Foundation
@testable import SemanticModel

final class SemanticNotPaintConformanceTests: XCTestCase {

    /// Replays suite 3's vectors through the same engine suite 1 uses.
    ///
    /// The fixture moved here from the state-machine directory, so it must still be *executed*.
    /// Parsing it and checking that `expected_outcome` exists would let the relocated vector rot
    /// while both suites reported green.
    func testSemanticNotPaintVectorsReplay() throws {
        let vectors = try ConformanceVectors.vectors(forSuite: 3)
        XCTAssertFalse(vectors.isEmpty, "suite 3 declares no vectors")

        let replayer = StateMachineConformanceTests()
        for url in vectors {
            print("  Replaying \(url.lastPathComponent)...")
            try replayer.replayVectorFile(at: url)
        }
    }

    /// §4.16 / §12.2: the protocol has no frame cadence vocabulary. There is no START_FRAME,
    /// END_FRAME, frame sequence number, or server-driven refresh rate anywhere in the registry.
    func testNoFrameCadenceVocabulary() {
        let frameConcepts = ["frame", "vsync", "refresh_rate", "swap_chain", "present_"]

        for entry in standardNodeTypesTable + standardPropertiesTable + standardOperationsTable {
            let name = entry.name.lowercased()
            for forbidden in frameConcepts {
                XCTAssertFalse(
                    name.contains(forbidden),
                    "Registry entry '\(entry.name)' introduces display frame cadence '\(forbidden)'; commits are state-consistency boundaries, not render frames (§4.16, §12.2, §32.3)"
                )
            }
        }
    }

    /// §4.17 / §10: layout is computed locally from semantic relationships. Standard controls
    /// never require server-specified absolute pixel coordinates.
    func testNoMandatoryAbsolutePixelGeometry() {
        let pixelConcepts = ["pixel_x", "pixel_y", "width_px", "height_px", "abs_x", "abs_y"]

        for entry in standardPropertiesTable {
            let name = entry.name.lowercased()
            for forbidden in pixelConcepts {
                XCTAssertFalse(
                    name.contains(forbidden),
                    "Standard property '\(entry.name)' requires absolute pixel geometry '\(forbidden)'; layout is local (§4.17, §10, §32.3)"
                )
            }
        }
    }

    /// §7.6 / §7.7: standard events route semantic intent, not raw pointer streams.
    func testStandardEventsRouteSemanticIntentNotRawPointerStreams() {
        for entry in standardEventsTable {
            let name = entry.name.lowercased()
            XCTAssertFalse(
                name.contains("mouse_move") || name.contains("raw_pointer"),
                "Standard event '\(entry.name)' must route semantic intent rather than a raw pointer stream (§7.6, §7.7)"
            )
        }
    }

    /// Verifies the semantic-not-paint invariant: standard widget profile trees define portable meaning,
    /// roles, and layout intent without display frames, paint instructions, or mandatory absolute pixel geometry.
    func testSemanticNotPaintArchitecturalInvariants() {
        // Assert standard registry tables contain zero display frame, paint command, or absolute pixel coordinate concepts
        let forbiddenConcepts = [
            "paint", "draw_rect", "draw_line", "fill_path", "rasterize",
            "framebuffer", "pixel_buffer", "render_pass", "gpu_texture",
            "display_list", "paint_layer", "skia_canvas"
        ]

        for entry in standardNodeTypesTable {
            let name = entry.name.lowercased()
            for forbidden in forbiddenConcepts {
                XCTAssertFalse(
                    name.contains(forbidden),
                    "Standard node type '\(entry.name)' violates semantic-not-paint invariant by containing forbidden paint keyword '\(forbidden)' (§4.7, §32.3)"
                )
            }
        }

        for entry in standardPropertiesTable {
            let name = entry.name.lowercased()
            for forbidden in forbiddenConcepts {
                XCTAssertFalse(
                    name.contains(forbidden),
                    "Standard property '\(entry.name)' violates semantic-not-paint invariant by containing forbidden paint keyword '\(forbidden)' (§4.7, §32.3)"
                )
            }
        }
    }
}
