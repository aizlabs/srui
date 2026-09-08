//! SRUI Semantic-Input Conformance Suite (§32 item 5).
//!
//! Implements: §7.6 (semantic events), §7.7 (coordinate events reserved for subscribed scenes),
//! §18.3 (text edit sequencing), §27 (authorization), §32.5.
//!
//! Ordinary controls emit semantic actions and state changes, never pointer coordinates.
//! Coordinate streams are accepted only for explicitly subscribed custom scene nodes.
//!
//! Every assertion runs a real `Event` through `Event::validate` against a real store. Event
//! identity comes from `STANDARD_EVENTS`, the table `semantic-tree/build.rs` already generates
//! from `protocol/registry.yaml`; this suite deliberately introduces no second copy of the
//! registry to check the first one against.
//!
//! ## Documented gap
//!
//! §32.5's rule that coordinates are refused outside a subscribed scene is **unenforced**:
//! `Event::validate` never compares event kind against target node type, so a `POINTER_DOWN`
//! aimed at a Button validates today. The rule is not invented here to make the suite green —
//! `test_coordinate_events_are_refused_for_unsubscribed_standard_nodes` pins the defect, and the
//! manifest records it with a probe that fails the runner if it is closed silently.

use srui_semantic_tree::*;

/// The §7.7 coordinate family. Event *kind* is registry metadata with no Rust-visible form, so
/// the partition is named here and cross-checked against `registry.yaml` by
/// `protocol/tests/test_conformance_manifest.py`.
const COORDINATE_EVENTS: &[&str] = &[
    "POINTER_DOWN",
    "POINTER_UP",
    "POINTER_MOVE",
    "POINTER_CANCEL",
    "POINTER_SCROLL",
];

fn store_with(
    node_type: TypeRef,
    properties: Vec<(PropertyRef, Value)>,
) -> (SemanticStore, NodeId) {
    let mut store = SemanticStore::new();
    let target = NodeId::new(2);
    store
        .create_node(NodeId::new(1), TypeRef::SURFACE, None, None, [])
        .expect("root surface");
    store
        .create_node(target, node_type, Some(NodeId::new(1)), None, properties)
        .expect("target node");
    (store, target)
}

fn event_of(store: &SemanticStore, target: NodeId, name: &str) -> Event {
    let event_type = resolve_standard_event(name).expect("standard event name must resolve");
    Event::new(
        None,
        1,
        EventId::from_string("conformance"),
        store.revision(),
        target,
        event_type,
        [],
    )
}

/// §7.6/§7.7: the registered event table partitions cleanly, and every coordinate event is a
/// `POINTER_*`. Asserted against the generated table, not a restated list.
#[test]
fn test_coordinate_events_are_exactly_the_pointer_family() {
    let registered: Vec<&str> = STANDARD_EVENTS.iter().map(|(_, name)| *name).collect();

    for coordinate in COORDINATE_EVENTS {
        assert!(
            registered.contains(coordinate),
            "coordinate event '{coordinate}' is not registered"
        );
    }
    for name in &registered {
        assert_eq!(
            name.starts_with("POINTER_"),
            COORDINATE_EVENTS.contains(name),
            "event '{name}' disagrees with the §7.7 coordinate family"
        );
    }
    assert_eq!(
        registered.len() - COORDINATE_EVENTS.len(),
        6,
        "§7.6 defines six semantic events"
    );
}

/// §7.6: the semantic events required-tier widgets originate must validate against a live node of
/// the declaring type, through the same path the server uses.
#[test]
fn test_required_widget_events_validate_against_live_nodes() {
    let cases: &[(TypeRef, &str)] = &[
        (TypeRef::BUTTON, "ACTIVATE"),
        (TypeRef::TOGGLE, "VALUE_CHANGED"),
        (TypeRef::LIST, "SELECTION_CHANGED"),
        (TypeRef::TABLE, "SELECTION_CHANGED"),
        (TypeRef::TREE, "SELECTION_CHANGED"),
        (TypeRef::TREE, "EXPANSION_CHANGED"),
        (TypeRef::SURFACE, "VIEWPORT_CHANGED"),
    ];

    for (node_type, event_name) in cases {
        let (store, target) = store_with(*node_type, vec![]);
        let event = event_of(&store, target, event_name);
        assert!(
            event.validate(&store).is_ok(),
            "{event_name} must validate against a live node of type {node_type:?}"
        );
    }
}

/// §18.3: a `TEXT_EDIT` from a text control carries a positive `edit_seq` and validates.
#[test]
fn test_text_edit_validates_against_text_controls() {
    for node_type in [TypeRef::TEXT_INPUT, TypeRef::TEXT_AREA] {
        let (store, target) = store_with(node_type, vec![]);
        let edit_seq = EditSeq::new(1).expect("edit_seq 1 is positive");
        let event = Event::text_edit(1, "edit", store.revision(), target, "hello", edit_seq);
        assert!(
            event.validate(&store).is_ok(),
            "TEXT_EDIT must validate against {node_type:?}"
        );
        assert_eq!(event.edit_seq, Some(edit_seq));
    }
}

/// §7.4 / §27: a disabled node refuses interaction. Authorization is part of the event contract,
/// not a transport detail.
#[test]
fn test_disabled_nodes_refuse_semantic_events() {
    let (store, target) = store_with(
        TypeRef::BUTTON,
        vec![(PropertyRef::ENABLED, Value::Bool(false))],
    );

    assert_eq!(
        Event::activate(1, "disabled", store.revision(), target).validate(&store),
        Err(EventValidationError::NodeDisabled(target)),
        "a disabled node must refuse ACTIVATE (§7.4, §27)"
    );
}

/// §7.7 / §12.1: an event observing a revision the server has not reached is refused, so a client
/// cannot act on state that does not exist yet.
#[test]
fn test_events_from_the_future_are_refused() {
    let (store, target) = store_with(TypeRef::BUTTON, vec![]);
    let future = Revision::new(store.revision().get() + 1);

    assert!(
        matches!(
            Event::activate(1, "future", future, target).validate(&store),
            Err(EventValidationError::FutureRevision { .. })
        ),
        "an event observing a future revision must be refused (§7.7, §12.1)"
    );
}

/// §7.7: an event targeting a node that does not exist is refused rather than ignored.
#[test]
fn test_events_targeting_missing_nodes_are_refused() {
    let (store, _) = store_with(TypeRef::BUTTON, vec![]);
    let missing = NodeId::new(9_999);

    assert_eq!(
        Event::activate(1, "missing", store.revision(), missing).validate(&store),
        Err(EventValidationError::NodeNotFound(missing)),
        "an event targeting a missing node must be refused (§7.7)"
    );
}

/// The §32.5 rule this suite exists to enforce: coordinates are accepted **only** for explicitly
/// subscribed custom scene nodes, so an ordinary Standard Widget node must refuse them.
///
/// ## This currently fails against the implementation, and the manifest says so
///
/// `Event::validate` checks observed revision, node existence and enabled state — never the
/// event kind against the target node type. A `POINTER_DOWN` aimed at a Button therefore
/// validates successfully today.
///
/// Per Task 33's "don't invent protocol behavior to pass a suite", the rule is not implemented
/// here and the assertion is not weakened to match the defect. When the rule lands, this
/// `#[should_panic]` inverts and the manifest's probe on `event.rs` fails the runner if the gap
/// is closed without the manifest being updated.
/// One test per coordinate event, not a loop: `#[should_panic]` stops at the first failing
/// assertion, so a loop would only ever exercise `POINTER_DOWN` — and worse, would keep
/// reporting green after a partial fix, because the second iteration panics with the same
/// message. Split this way, each event inverts independently when it is fixed.
macro_rules! coordinate_event_is_refused {
    ($name:ident, $event:literal) => {
        #[test]
        #[should_panic(expected = "ordinary Standard Widget node must refuse coordinate events")]
        fn $name() {
            let (store, target) = store_with(TypeRef::BUTTON, vec![]);
            let event = event_of(&store, target, $event);
            assert!(
                event.validate(&store).is_err(),
                "an ordinary Standard Widget node must refuse coordinate events, but '{}' was \
                 accepted against a Button (§7.7, §32.5)",
                $event
            );
        }
    };
}

coordinate_event_is_refused!(
    test_pointer_down_is_refused_for_unsubscribed_standard_nodes,
    "POINTER_DOWN"
);
coordinate_event_is_refused!(
    test_pointer_up_is_refused_for_unsubscribed_standard_nodes,
    "POINTER_UP"
);
coordinate_event_is_refused!(
    test_pointer_move_is_refused_for_unsubscribed_standard_nodes,
    "POINTER_MOVE"
);
coordinate_event_is_refused!(
    test_pointer_cancel_is_refused_for_unsubscribed_standard_nodes,
    "POINTER_CANCEL"
);
coordinate_event_is_refused!(
    test_pointer_scroll_is_refused_for_unsubscribed_standard_nodes,
    "POINTER_SCROLL"
);

/// The macro invocations above are hand-written, so a sixth coordinate event would otherwise
/// gain no pinning test. This fails the moment the family changes.
#[test]
fn test_every_coordinate_event_has_a_pinning_test() {
    assert_eq!(
        COORDINATE_EVENTS.len(),
        5,
        "COORDINATE_EVENTS changed; add or remove a `coordinate_event_is_refused!` invocation \
         so every §7.7 event keeps its own pinning test"
    );
}

/// §7.6 gives each node type a defined set of events it can originate. A node with no interactive
/// semantics — `Text`, `Progress`, `Image`, `Separator` — has no meaning for `ACTIVATE`.
///
/// ## This currently fails against the implementation, and the manifest says so
///
/// Same root cause as the coordinate gap above: `Event::validate` never compares event type
/// against target node type, so `ACTIVATE` aimed at a `Text` node validates today. Pinned rather
/// than weakened; the suite 5 gap probe on `event.rs` covers both.
/// Split per node type for the same reason as the coordinate tests above: under
/// `#[should_panic]` a loop would only ever reach `Text`, and would stay green if `Text` alone
/// were fixed.
macro_rules! activate_is_refused_by {
    ($name:ident, $node_type:expr) => {
        #[test]
        #[should_panic(expected = "non-interactive node must refuse ACTIVATE")]
        fn $name() {
            let node_type = $node_type;
            let (store, target) = store_with(node_type, vec![]);
            let event = Event::activate(1, "wrong-node", store.revision(), target);
            assert!(
                event.validate(&store).is_err(),
                "a non-interactive node must refuse ACTIVATE, but {node_type:?} accepted it (§7.6)"
            );
        }
    };
}

activate_is_refused_by!(test_text_refuses_activate, TypeRef::TEXT);
activate_is_refused_by!(test_progress_refuses_activate, TypeRef::PROGRESS);
activate_is_refused_by!(test_image_refuses_activate, TypeRef::IMAGE);
activate_is_refused_by!(test_separator_refuses_activate, TypeRef::SEPARATOR);
