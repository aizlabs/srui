//! SRUI Semantic-Not-Paint Conformance Suite (§32 item 3).
//!
//! Implements: §4.7 (no paint commands), §4.16 (no frame cadence), §4.17 (no mandatory pixel
//! geometry), §7.1 (widget semantics), §10 (local layout), §32.3.
//!
//! Two halves, both independently runnable:
//!
//! 1. A registry/schema audit asserting the Protocol Core and Standard Widget Profile carry no
//!    frame, paint, or absolute-pixel vocabulary at all.
//! 2. The declarative vectors in
//!    `protocol/conformance-vectors/suites/03-semantic-not-paint/vectors/*.json`, which show a
//!    concrete widget tree built from semantic intent alone.
//!
//! Split out of `state_machine_conformance_test.rs` so §32 item 3 is a named suite rather than a
//! stowaway in item 1.

mod common;

use srui_semantic_tree::*;

/// Replays suite 3's vectors through the same engine suite 1 uses.
///
/// The fixture moved here from the state-machine directory, so it must still be *executed*:
/// parsing it and checking that `expected_outcome` exists would let the relocated vector rot
/// while both suites reported green.
#[test]
fn test_semantic_not_paint_vectors_replay() {
    let vectors = common::suite_vectors(3);
    assert!(!vectors.is_empty(), "suite 3 declares no vectors");
    for path in &vectors {
        println!("  -> Replaying fixture: {:?}", path.file_name().unwrap());
        common::fixture_replay::replay_fixture(path);
    }
}

#[test]
fn test_semantic_not_paint_architectural_invariants() {
    // §32 item 3 & §4 Invariants:
    // Assert that the Protocol Core and Standard Widget Profile contain zero notions of:
    // 1. Frame cadence (no start_frame, end_frame, frame_id, vsync)
    // 2. Paint / drawing commands (no draw_rect, fill_path, set_pixel, draw_text)
    // 3. Mandatory absolute pixel geometry (no pixel_x, pixel_y, width_px, height_px)

    // 1. Verify Standard Node Types
    for &(id, name) in STANDARD_NODE_TYPES {
        assert!(
            !name.to_lowercase().contains("frame"),
            "Node type {} ({}) must not contain frame concepts (§4.16)",
            id,
            name
        );
        assert!(
            !name.to_lowercase().contains("paint"),
            "Node type {} ({}) must not contain paint concepts (§4.7)",
            id,
            name
        );
        assert!(
            !name.to_lowercase().contains("raster"),
            "Node type {} ({}) must not contain raster concepts (§4.7)",
            id,
            name
        );
    }

    // 2. Verify Standard Properties
    for &(id, name) in STANDARD_PROPERTIES {
        assert!(
            !name.contains("pixel_x") && !name.contains("pixel_y") && !name.contains("px"),
            "Standard property {} ({}) must not require absolute pixel coordinates (§4.17)",
            id,
            name
        );
        assert!(
            !name.contains("color_hex")
                && !name.contains("background_color")
                && !name.contains("brush"),
            "Standard property {} ({}) must not prescribe direct painting brushes (§4.7)",
            id,
            name
        );
        assert!(
            !name.contains("font_family") && !name.contains("font_size"),
            "Standard property {} ({}) must not dictate renderer fonts in v0.1 (§3.2, §4.7)",
            id,
            name
        );
    }

    // 3. Verify Standard Operations
    for &(id, name) in STANDARD_OPERATIONS {
        let name_lower = name.to_lowercase();
        assert!(
            !name_lower.contains("frame"),
            "Operation {} ({}) must not introduce display frame cadence (§4.16, §12.2)",
            id,
            name
        );
        assert!(
            !name_lower.contains("paint")
                && !name_lower.contains("draw")
                && !name_lower.contains("render"),
            "Operation {} ({}) must not be a paint or drawing command (§4.7, §7.1)",
            id,
            name
        );
    }

    // 4. Verify Standard Events
    for &(id, name) in STANDARD_EVENTS {
        assert!(
            !name.to_lowercase().contains("mouse_move") && !name.to_lowercase().contains("raw_pointer"),
            "Standard event {} ({}) must route semantic intent rather than raw pointer streams (§7.6, §7.7)",
            id,
            name
        );
    }
}
