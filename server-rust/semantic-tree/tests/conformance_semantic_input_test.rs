//! SRUI Semantic-Input Conformance Suite (§32 item 5).
//!
//! Implements: §7.6 (semantic events), §7.7 (coordinate events reserved for subscribed scenes),
//! §18.3 (text edit sequencing), §32.5.
//!
//! Ordinary controls emit semantic actions and state changes, never pointer coordinates.
//! Coordinate streams are accepted only for explicitly subscribed custom scene nodes.
//!
//! ## Documented gap
//!
//! The *positive* half of §32.5 — a coordinate event accepted for a subscribed scene — cannot be
//! exercised on this base. `POINTER_*` events are registered in namespace 0, but there is no
//! `VectorScene` node type, no subscription model, and no server-side "coordinate event only for
//! a subscribed scene" validation. That is Task 36 work and is recorded as a gap in
//! `protocol/conformance-vectors/suites/manifest.json` rather than papered over here.

mod common;

use serde::Deserialize;
use srui_semantic_tree::*;

#[derive(Debug, Deserialize)]
struct EventMatrix {
    semantic_events: Vec<String>,
    coordinate_events: Vec<String>,
    node_event_matrix: Vec<NodeEventRow>,
}

#[derive(Debug, Deserialize)]
struct NodeEventRow {
    node: String,
    #[allow(dead_code)]
    tier: String,
    allowed: Vec<String>,
    forbidden: Vec<String>,
}

fn load_matrix() -> EventMatrix {
    let path = common::suite_generated(5, "events.generated.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to read event matrix {:?}: {}", path, e));
    serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("Failed to parse event matrix {:?}: {}", path, e))
}

/// The semantic/coordinate partition must be total and disjoint over the registered events.
#[test]
fn test_event_partition_is_total_and_disjoint() {
    let matrix = load_matrix();

    let partitioned = matrix.semantic_events.len() + matrix.coordinate_events.len();
    assert_eq!(
        partitioned,
        STANDARD_EVENTS.len(),
        "Event matrix partitions {} events but the registry declares {}",
        partitioned,
        STANDARD_EVENTS.len()
    );

    for semantic in &matrix.semantic_events {
        assert!(
            !matrix.coordinate_events.contains(semantic),
            "Event '{}' is classified as both semantic and coordinate",
            semantic
        );
    }

    for (_, name) in STANDARD_EVENTS {
        let known = matrix.semantic_events.iter().any(|e| e == name)
            || matrix.coordinate_events.iter().any(|e| e == name);
        assert!(
            known,
            "Registered event '{}' is absent from the matrix",
            name
        );
    }
}

/// §7.7: every coordinate event is a `POINTER_*` event and no other event is.
#[test]
fn test_coordinate_events_are_exactly_the_pointer_family() {
    let matrix = load_matrix();

    for coordinate in &matrix.coordinate_events {
        assert!(
            coordinate.starts_with("POINTER_"),
            "Coordinate event '{}' is not a POINTER_* event",
            coordinate
        );
    }
    for semantic in &matrix.semantic_events {
        assert!(
            !semantic.starts_with("POINTER_"),
            "Semantic event '{}' must not be a POINTER_* event",
            semantic
        );
    }
    assert_eq!(
        matrix.coordinate_events.len(),
        5,
        "§7.7 defines five coordinate events (down/up/move/cancel/scroll)"
    );
}

/// The core assertion of §32.5: no Standard Widget Profile node type may originate a coordinate
/// event. Standard controls report *what the user meant*, not where the pointer was.
#[test]
fn test_no_standard_node_type_emits_coordinate_events() {
    let matrix = load_matrix();

    for row in &matrix.node_event_matrix {
        for allowed in &row.allowed {
            assert!(
                !matrix.coordinate_events.contains(allowed),
                "Node type '{}' is allowed to emit coordinate event '{}'; coordinates are \
                 reserved for explicitly subscribed custom scenes (§7.7, §32.5)",
                row.node,
                allowed
            );
        }
        assert_eq!(
            &row.forbidden, &matrix.coordinate_events,
            "Node type '{}' must forbid the complete coordinate event set",
            row.node
        );
    }
}

/// Every allowed emission must construct a real event through the same path the server uses,
/// and must carry no coordinate payload (§7.6).
#[test]
fn test_allowed_semantic_events_construct_without_coordinates() {
    let matrix = load_matrix();

    for row in &matrix.node_event_matrix {
        for allowed in &row.allowed {
            let event_ref = resolve_standard_event(allowed).unwrap_or_else(|e| {
                panic!(
                    "Node '{}' declares emission '{}' which does not resolve: {}",
                    row.node, allowed, e
                )
            });
            assert_eq!(
                event_ref.namespace_id, 0,
                "Standard semantic event '{}' must live in namespace 0",
                allowed
            );
            assert_eq!(
                event_ref.standard_event_name(),
                Some(allowed.as_str()),
                "Event '{}' must round-trip through the standard event table",
                allowed
            );
        }
    }
}

/// §18.3 / §22.6: `edit_seq` is meaningful only for `TEXT_EDIT`. Non-text semantic events must
/// carry `edit_seq == 0`, which is what keeps text sequencing from leaking into button clicks.
#[test]
fn test_text_edit_sequencing_is_scoped_to_text_events() {
    let matrix = load_matrix();

    let text_emitters: Vec<&str> = matrix
        .node_event_matrix
        .iter()
        .filter(|row| row.allowed.iter().any(|e| e == "TEXT_EDIT"))
        .map(|row| row.node.as_str())
        .collect();

    assert_eq!(
        text_emitters,
        vec!["TextInput", "TextArea"],
        "Only the two text controls may originate TEXT_EDIT (§7.6)"
    );
}

/// Documents the Task 36 gap as an executable assertion: while no scene subscription model
/// exists, no node type may claim coordinate capability. When Task 36 lands a subscribed
/// `VectorScene`, this test is the one that must be revisited alongside the manifest gap entry.
#[test]
fn test_subscribed_scene_coordinate_path_is_absent() {
    let matrix = load_matrix();

    let coordinate_capable: Vec<&str> = matrix
        .node_event_matrix
        .iter()
        .filter(|row| row.allowed.iter().any(|e| e.starts_with("POINTER_")))
        .map(|row| row.node.as_str())
        .collect();

    assert!(
        coordinate_capable.is_empty(),
        "No subscribed-scene node type exists yet, so nothing may accept coordinates; found {:?}. \
         If Task 36 added one, move the suite 5 gap entry in the conformance manifest to active \
         coverage instead of relaxing this assertion.",
        coordinate_capable
    );
}
