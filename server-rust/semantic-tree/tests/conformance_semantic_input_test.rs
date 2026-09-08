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

/// Every declared emission must survive real validation against a real store, on a real node of
/// the declaring type — not merely resolve as a registry name.
#[test]
fn test_declared_emissions_validate_against_a_live_node() {
    let matrix = load_matrix();

    for row in &matrix.node_event_matrix {
        if row.allowed.is_empty() {
            continue;
        }
        let node_type = resolve_standard_node_type(&row.node)
            .unwrap_or_else(|e| panic!("node type '{}' must resolve: {}", row.node, e));

        let mut store = SemanticStore::new();
        store
            .create_node(NodeId::new(1), TypeRef::SURFACE, None, None, [])
            .expect("root surface");
        store
            .create_node(NodeId::new(2), node_type, Some(NodeId::new(1)), None, [])
            .expect("target node");

        for allowed in &row.allowed {
            let event_type = resolve_standard_event(allowed)
                .unwrap_or_else(|e| panic!("event '{}' must resolve: {}", allowed, e));
            assert_eq!(event_type.namespace_id, 0);

            let event = Event::new(
                None,
                1,
                EventId::from_string("conformance"),
                store.revision(),
                NodeId::new(2),
                event_type,
                [],
            );
            assert!(
                event.validate(&store).is_ok(),
                "node '{}' declares emission '{}' but the server refuses it against a live \
                 node of that type",
                row.node,
                allowed
            );
        }
    }
}

/// §7.4 / §27: a disabled node refuses interaction whatever the event type. This is the
/// authorization edge every semantic event shares.
#[test]
fn test_disabled_nodes_refuse_declared_semantic_events() {
    let mut store = SemanticStore::new();
    store
        .create_node(NodeId::new(1), TypeRef::SURFACE, None, None, [])
        .expect("root surface");
    store
        .create_node(
            NodeId::new(2),
            TypeRef::BUTTON,
            Some(NodeId::new(1)),
            None,
            [(PropertyRef::ENABLED, Value::Bool(false))],
        )
        .expect("disabled button");

    let event = Event::activate(1, "disabled", store.revision(), NodeId::new(2));
    assert_eq!(
        event.validate(&store),
        Err(EventValidationError::NodeDisabled(NodeId::new(2))),
        "a disabled node must refuse ACTIVATE (§7.4, §27)"
    );
}

/// The §32.5 rule this suite exists to enforce: coordinates are accepted **only** for explicitly
/// subscribed custom scene nodes, so an ordinary Standard Widget node must refuse them.
///
/// ## This currently fails against the implementation, and the manifest says so
///
/// `Event::validate` checks observed revision, node existence, enabled/read-only state and
/// `TEXT_EDIT` sequencing — it never compares the event kind against the target node type. A
/// `POINTER_DOWN` aimed at a Button therefore validates successfully today.
///
/// Per Task 33's "don't invent protocol behavior to pass a suite", the rule is not implemented
/// here and the assertion is not weakened to match the defect. Instead the exact defect is
/// pinned below, and suite 5 reports GAP with the scenario recorded in the manifest. When the
/// rule lands, this test's `#[should_panic]` inverts and the manifest gap closes — the gap probe
/// on `event.rs` fails the runner if that happens without the manifest being updated.
#[test]
#[should_panic(expected = "ordinary Standard Widget node must refuse coordinate events")]
fn test_coordinate_events_are_refused_for_unsubscribed_standard_nodes() {
    let matrix = load_matrix();

    let mut store = SemanticStore::new();
    store
        .create_node(NodeId::new(1), TypeRef::SURFACE, None, None, [])
        .expect("root surface");
    store
        .create_node(
            NodeId::new(2),
            TypeRef::BUTTON,
            Some(NodeId::new(1)),
            None,
            [],
        )
        .expect("button");

    for coordinate in &matrix.coordinate_events {
        let event_type = resolve_standard_event(coordinate)
            .unwrap_or_else(|e| panic!("event '{}' must resolve: {}", coordinate, e));
        let event = Event::new(
            None,
            1,
            EventId::from_string("coord"),
            store.revision(),
            NodeId::new(2),
            event_type,
            [],
        );
        assert!(
            event.validate(&store).is_err(),
            "an ordinary Standard Widget node must refuse coordinate events, but '{}' was \
             accepted against a Button (§7.7, §32.5)",
            coordinate
        );
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
