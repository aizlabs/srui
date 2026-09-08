//! SRUI Widget Semantics Conformance Suite (§32 item 2).
//!
//! Implements: §7.2 (standard node types), §7.3 (implementation tiers), §7.6 (semantic events),
//! §32.2.
//!
//! Drives `protocol/conformance-vectors/suites/02-widget-semantics/widgets.generated.json`, which
//! is generated from `protocol/registry.yaml` by `protocol/generate_conformance_matrix.py`. The
//! fixture is never hand-edited, so tier and emission facts cannot drift away from the registry
//! that the wire format is built from.

mod common;

use serde::Deserialize;
use srui_semantic_tree::*;

#[derive(Debug, Deserialize)]
struct WidgetMatrix {
    node_types: Vec<WidgetRow>,
}

#[derive(Debug, Deserialize)]
struct WidgetRow {
    id: u32,
    name: String,
    tier: String,
    category: String,
    emits: Vec<String>,
    expect_constructible: bool,
}

fn load_matrix() -> WidgetMatrix {
    let path = common::suite_generated(2, "widgets.generated.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to read widget matrix {:?}: {}", path, e));
    serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("Failed to parse widget matrix {:?}: {}", path, e))
}

/// The generated matrix must describe every standard node type the core knows about, with the
/// same ids. A mismatch means the fixture was generated from a different registry revision.
#[test]
fn test_widget_matrix_covers_every_standard_node_type() {
    let matrix = load_matrix();

    assert_eq!(
        matrix.node_types.len(),
        STANDARD_NODE_TYPES.len(),
        "Widget matrix covers {} node types but the core registry table has {}. \
         Regenerate with ./protocol/generate_proto.sh.",
        matrix.node_types.len(),
        STANDARD_NODE_TYPES.len()
    );

    for row in &matrix.node_types {
        let found = STANDARD_NODE_TYPES
            .iter()
            .find(|(id, _)| *id == row.id)
            .unwrap_or_else(|| panic!("Widget matrix lists unknown node type id {}", row.id));
        assert_eq!(
            found.1, row.name,
            "Node type id {} is '{}' in the core registry table but '{}' in the widget matrix",
            row.id, found.1, row.name
        );
    }
}

/// Every node type must resolve by name through the same path applications use (§7.2).
#[test]
fn test_every_matrix_node_type_resolves() {
    for row in &load_matrix().node_types {
        let type_ref = resolve_standard_node_type(&row.name)
            .unwrap_or_else(|e| panic!("Standard node type '{}' must resolve: {}", row.name, e));
        assert_eq!(
            type_ref,
            TypeRef::standard(row.id),
            "Node type '{}' resolved to {:?}, expected standard({})",
            row.name,
            type_ref,
            row.id
        );
    }
}

/// §7.6: every declared emission must be a registered standard event, and coordinate events
/// (§7.7) may never appear on a Standard Widget Profile node.
#[test]
fn test_widget_emissions_are_registered_semantic_events() {
    for row in &load_matrix().node_types {
        for event_name in &row.emits {
            let found = STANDARD_EVENTS.iter().any(|(_, name)| name == event_name);
            assert!(
                found,
                "Node type '{}' declares emission '{}', which is not a registered standard event",
                row.name, event_name
            );
            assert!(
                !event_name.starts_with("POINTER_"),
                "Node type '{}' must not emit coordinate event '{}' (§7.7, §32.5)",
                row.name,
                event_name
            );
        }
    }
}

/// §7.3: the tier partition is what decides whether a renderer must implement a node type, so
/// the matrix must agree with the tiers the registry declares.
#[test]
fn test_tier_partition_matches_expected_implementation_status() {
    let matrix = load_matrix();

    let required: Vec<&WidgetRow> = matrix
        .node_types
        .iter()
        .filter(|row| row.tier == "required")
        .collect();
    assert_eq!(
        required.len(),
        18,
        "§7.3 defines 18 required-tier node types, matrix has {}",
        required.len()
    );

    for row in &matrix.node_types {
        assert_eq!(
            row.expect_constructible,
            row.tier == "required",
            "Node type '{}' (tier '{}') has expect_constructible={}; only required-tier node \
             types are expected to construct today",
            row.name,
            row.tier,
            row.expect_constructible
        );
        assert!(
            !row.category.is_empty(),
            "Node type '{}' must declare a category",
            row.name
        );
    }
}

/// Collections carry selection semantics; text controls carry edit semantics. This pins the
/// meaning of the required-tier interactive widgets rather than merely their existence (§32.2).
#[test]
fn test_required_interactive_widgets_carry_expected_semantics() {
    let matrix = load_matrix();
    let emits_of = |name: &str| -> Vec<String> {
        matrix
            .node_types
            .iter()
            .find(|row| row.name == name)
            .unwrap_or_else(|| panic!("Widget matrix is missing node type '{}'", name))
            .emits
            .clone()
    };

    assert_eq!(emits_of("Button"), vec!["ACTIVATE".to_string()]);
    assert_eq!(emits_of("Toggle"), vec!["VALUE_CHANGED".to_string()]);
    assert_eq!(emits_of("TextInput"), vec!["TEXT_EDIT".to_string()]);
    assert_eq!(emits_of("TextArea"), vec!["TEXT_EDIT".to_string()]);
    assert_eq!(emits_of("List"), vec!["SELECTION_CHANGED".to_string()]);
    assert_eq!(emits_of("Table"), vec!["SELECTION_CHANGED".to_string()]);
    assert_eq!(
        emits_of("Tree"),
        vec![
            "SELECTION_CHANGED".to_string(),
            "EXPANSION_CHANGED".to_string()
        ],
        "Tree must carry both selection and disclosure semantics (§7.6)"
    );

    // Static content originates no events at all: it has state, not interaction.
    for inert in ["Text", "Progress", "Image", "Separator", "Spacer"] {
        assert!(
            emits_of(inert).is_empty(),
            "Node type '{}' is non-interactive and must emit no semantic events",
            inert
        );
    }
}
