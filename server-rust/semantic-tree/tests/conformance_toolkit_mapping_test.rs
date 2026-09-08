//! SRUI Toolkit Mapping Conformance Suite (§32 item 12).
//!
//! Implements: §22.4 (native mappings are informative), §32.12.
//!
//! This suite is deliberately **informative**. §22.4 says a renderer may change how it realises a
//! semantic node without a protocol version change, so nothing here asserts that a particular
//! `NSView` subclass was used. What conformance does require is that the shared mapping fixture
//! is *complete* (every standard node type is accounted for) and *honest* (its `implemented`
//! flags match the tier partition the renderer actually enforces), so a second renderer can use
//! it as a starting point without inheriting stale claims.

mod common;

use serde::Deserialize;
use srui_semantic_tree::*;

#[derive(Debug, Deserialize)]
struct ToolkitMappings {
    normative: bool,
    mappings: Vec<MappingRow>,
}

#[derive(Debug, Deserialize)]
struct MappingRow {
    node: String,
    tier: String,
    category: String,
    appkit_view: String,
    appkit_strategy: String,
    implemented: bool,
}

fn load_mappings() -> ToolkitMappings {
    let path = common::suite_generated(12, "mappings.generated.json");
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Failed to read toolkit mappings {:?}: {}", path, e));
    serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("Failed to parse toolkit mappings {:?}: {}", path, e))
}

/// §22.4: the fixture must declare itself non-normative so no implementation treats an AppKit
/// class name as a protocol requirement.
#[test]
fn test_toolkit_mapping_fixture_is_informative() {
    assert!(
        !load_mappings().normative,
        "Toolkit mappings must be marked non-normative (§22.4)"
    );
}

/// Completeness is the real conformance property: a node type with no mapping entry is a node
/// type a new renderer has no guidance for.
#[test]
fn test_every_standard_node_type_has_a_mapping() {
    let mappings = load_mappings();

    assert_eq!(
        mappings.mappings.len(),
        STANDARD_NODE_TYPES.len(),
        "Toolkit mapping covers {} node types, registry has {}",
        mappings.mappings.len(),
        STANDARD_NODE_TYPES.len()
    );

    for (_, name) in STANDARD_NODE_TYPES {
        let row = mappings
            .mappings
            .iter()
            .find(|row| row.node == *name)
            .unwrap_or_else(|| panic!("Standard node type '{}' has no toolkit mapping", name));
        assert!(
            !row.appkit_view.trim().is_empty(),
            "Node type '{}' has an empty AppKit view mapping",
            name
        );
        assert!(
            !row.appkit_strategy.trim().is_empty(),
            "Node type '{}' has an empty AppKit strategy description",
            name
        );
        assert!(
            !row.category.trim().is_empty(),
            "Node type '{}' has an empty category",
            name
        );
    }
}

/// Honesty: `implemented` must track the required tier, which is what the renderer actually
/// constructs. A mapping that claims coverage the renderer does not have would mislead exactly
/// the second-renderer author this fixture exists to help.
#[test]
fn test_implementation_claims_match_the_tier_partition() {
    for row in &load_mappings().mappings {
        assert_eq!(
            row.implemented,
            row.tier == "required",
            "Node type '{}' (tier '{}') claims implemented={}; only required-tier node types \
             are implemented by the AppKit renderer today",
            row.node,
            row.tier,
            row.implemented
        );
    }
}

/// Mappings must be distinct per node, but two node types may legitimately share one AppKit
/// class (List and Table are both `NSTableView`); the strategy text is what disambiguates them.
#[test]
fn test_shared_appkit_classes_are_disambiguated_by_strategy() {
    let mappings = load_mappings();

    for row in &mappings.mappings {
        let siblings: Vec<&MappingRow> = mappings
            .mappings
            .iter()
            .filter(|other| other.appkit_view == row.appkit_view)
            .collect();
        if siblings.len() > 1 {
            let strategies: std::collections::BTreeSet<&str> = siblings
                .iter()
                .map(|s| s.appkit_strategy.as_str())
                .collect();
            assert_eq!(
                strategies.len(),
                siblings.len(),
                "Node types {:?} all map to '{}' but do not have distinct strategies",
                siblings.iter().map(|s| &s.node).collect::<Vec<_>>(),
                row.appkit_view
            );
        }
    }
}
