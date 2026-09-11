//! Gallery graph, resource, event, mutation, and reset tests (§7.2, §7.3, §7.6, §8, §12.1, §14).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Barrier};
use std::time::Duration;

use srui_example_ui_gallery::{ids, scenes::Scene, stats::SIZE_BUCKETS, ui, GalleryApp, SCENES};
use srui_protocol::Event as WireEvent;
use srui_sdk::*;
use srui_semantic_tree::{Event as SemanticEvent, Node, SemanticStore};
use srui_sessiond::{EventOutcome, Session};

const CLIENT: &str = "ui-gallery-test";

// =============================================================================
// Harness
// =============================================================================

fn app() -> Arc<GalleryApp> {
    GalleryApp::start(Arc::new(Session::mint())).expect("gallery starts")
}

/// Emits the contiguous, uniquely identified event sequence `Session` deduplication requires
/// (§18.2): one client instance, `event_seq` starting at 1 and never repeating.
struct Client {
    seq: u64,
}

impl Client {
    fn new() -> Self {
        Self { seq: 0 }
    }

    fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }

    fn wire(&self, event: SemanticEvent) -> WireEvent {
        WireEvent::from(&event.with_client_instance_id(CLIENT))
    }

    fn activate(&mut self, session: &Session, node: NodeId) -> WireEvent {
        let seq = self.next_seq();
        self.wire(SemanticEvent::activate(
            seq,
            format!("activate-{seq}"),
            session.current_revision(),
            node,
        ))
    }

    fn value_changed(
        &mut self,
        session: &Session,
        node: NodeId,
        value: impl Into<Value>,
    ) -> WireEvent {
        let seq = self.next_seq();
        self.wire(SemanticEvent::value_changed(
            seq,
            format!("value-{seq}"),
            session.current_revision(),
            node,
            value,
        ))
    }

    fn selection(&mut self, session: &Session, node: NodeId, item: ItemId) -> WireEvent {
        let seq = self.next_seq();
        self.wire(SemanticEvent::selection_changed(
            seq,
            format!("selection-{seq}"),
            session.current_revision(),
            node,
            item,
        ))
    }
}

fn dispatch(session: &Session, event: &WireEvent) -> EventOutcome {
    session.process_event(event).expect("event is processable")
}

fn text_of(session: &Session, node: NodeId) -> String {
    session
        .get_node(node)
        .and_then(|node| Text::text_of(&node).map(str::to_string))
        .unwrap_or_default()
}

/// Depth-first walk of every node reachable from the roots.
fn walk(store: &SemanticStore) -> Vec<Node> {
    let mut out = Vec::new();
    let mut stack: Vec<NodeId> = store.root_ids().to_vec();
    stack.reverse();
    while let Some(id) = stack.pop() {
        let Some(node) = store.get_node(id) else {
            continue;
        };
        for child in node.ordered_children.iter().rev() {
            stack.push(*child);
        }
        out.push(node.clone());
    }
    out
}

fn all_nodes(session: &Session) -> Vec<Node> {
    session.with_store(walk)
}

/// Canonical description of everything the gallery owns and a scene may mutate.
///
/// Telemetry nodes (ids at or above [`ids::TELEMETRY_ID_FLOOR`]) and the inspector model are
/// excluded: their counters advance monotonically with traffic, so they are not part of the
/// baseline a reset restores.
fn fingerprint(session: &Session) -> String {
    session.with_store(|store| {
        let mut lines = Vec::new();
        for node in walk(store) {
            if ids::is_telemetry(node.id) {
                continue;
            }
            let mut properties: Vec<String> = node
                .properties
                .iter()
                .map(|(property, value)| {
                    format!("{}={value}", property.standard_name().unwrap_or("unknown"))
                })
                .collect();
            properties.sort();
            let children: Vec<String> = node
                .ordered_children
                .iter()
                .filter(|child| !ids::is_telemetry(**child))
                .map(|child| child.get().to_string())
                .collect();
            lines.push(format!(
                "node {} type {} parent {:?} children [{}] {{{}}}",
                node.id.get(),
                node.node_type.standard_name().unwrap_or("unknown"),
                node.parent_id.map(|parent| parent.get()),
                children.join(","),
                properties.join(", ")
            ));
        }

        for model_id in [ids::LIST_MODEL, ids::TABLE_MODEL] {
            let model = store.get_model(model_id).expect("gallery model exists");
            for index in 0..model.item_count() {
                let item = model.get_item_by_index(index).expect("cached item");
                lines.push(format!(
                    "model {} [{index}] item {} = {}",
                    model_id.get(),
                    item.item_id.get(),
                    item.value
                ));
            }
        }

        lines.join("\n")
    })
}

/// Compares two fingerprints, reporting only the first differing line.
///
/// A whole-fingerprint `assert_eq!` prints two 12 KB strings and hides the one line that matters.
fn assert_same_graph(actual: &str, expected: &str, context: &str) {
    if actual == expected {
        return;
    }
    let mismatch = actual
        .lines()
        .zip(expected.lines())
        .enumerate()
        .find(|(_, (a, b))| a != b);
    match mismatch {
        Some((index, (a, b))) => {
            panic!("{context}: graphs diverge at line {index}\n  actual:   {a}\n  expected: {b}")
        }
        None => panic!(
            "{context}: graphs have different lengths ({} vs {} lines)",
            actual.lines().count(),
            expected.lines().count()
        ),
    }
}

/// Short operation kind names, for asserting the operation mix of a transaction.
fn kinds(operations: &[Operation]) -> Vec<&'static str> {
    operations
        .iter()
        .map(|operation| match operation {
            Operation::CreateNode { .. } => "CREATE_NODE",
            Operation::DeleteNode { .. } => "DELETE_NODE",
            Operation::SetProperty { .. } => "SET_PROPERTY",
            Operation::ClearProperty { .. } => "CLEAR_PROPERTY",
            Operation::MoveNode { .. } => "MOVE_NODE",
            Operation::ReorderChildren { .. } => "REORDER_CHILDREN",
            Operation::BatchPropertySet { .. } => "BATCH_PROPERTY_SET",
            Operation::CreateModel { .. } => "CREATE_MODEL",
            Operation::ModelInsert { .. } => "MODEL_INSERT",
            Operation::ModelDelete { .. } => "MODEL_DELETE",
            Operation::ModelUpdate { .. } => "MODEL_UPDATE",
            Operation::ModelResetRange { .. } => "MODEL_RESET_RANGE",
        })
        .collect()
}

/// Operations touching gallery content, i.e. excluding inspector and telemetry bookkeeping.
fn content_operations(operations: &[Operation]) -> Vec<Operation> {
    operations
        .iter()
        .filter(|operation| match operation {
            Operation::CreateNode { id, .. }
            | Operation::DeleteNode { id }
            | Operation::SetProperty { id, .. }
            | Operation::ClearProperty { id, .. }
            | Operation::MoveNode { id, .. }
            | Operation::BatchPropertySet { id, .. } => !ids::is_telemetry(*id),
            Operation::ReorderChildren { parent_id, .. } => !ids::is_telemetry(*parent_id),
            Operation::CreateModel { id, .. }
            | Operation::ModelInsert { id, .. }
            | Operation::ModelDelete { id, .. }
            | Operation::ModelUpdate { id, .. }
            | Operation::ModelResetRange { id, .. } => *id != ids::TRACE_MODEL,
        })
        .cloned()
        .collect()
}

fn trace_rows(session: &Session) -> Vec<Vec<String>> {
    session.with_store(|store| {
        let model = store.get_model(ids::TRACE_MODEL).expect("trace model");
        (0..model.item_count())
            .map(|index| {
                let item = model.get_item_by_index(index).expect("cached trace row");
                match &item.value {
                    Value::List(cells) => cells
                        .iter()
                        .map(|cell| cell.as_string().unwrap_or_default().to_string())
                        .collect(),
                    other => vec![other.to_string()],
                }
            })
            .collect()
    })
}

// =============================================================================
// Graph coverage (§7.3)
// =============================================================================

#[test]
fn initial_graph_contains_every_supported_node_type() {
    let app = app();
    let present: BTreeSet<u32> = all_nodes(app.session())
        .iter()
        .map(|node| node.node_type.local_id)
        .collect();
    let expected: BTreeSet<u32> = ui::supported_node_types()
        .iter()
        .map(|type_ref| type_ref.local_id)
        .collect();

    assert_eq!(
        present, expected,
        "the gallery must instantiate exactly the renderable required-tier node types"
    );
    // Surface, Scroll, Column, Row, Grid, Spacer, Separator, Text, RichText, Image, Button,
    // Toggle, TextInput, TextArea, Progress, List, Table, Tree.
    assert_eq!(expected.len(), 18);
}

#[test]
fn unsupported_node_types_are_not_advertised() {
    let app = app();
    let present: BTreeSet<u32> = all_nodes(app.session())
        .iter()
        .map(|node| node.node_type.local_id)
        .collect();

    for name in ui::unsupported_node_type_names() {
        let type_ref = TypeRef::resolve_standard(name)
            .unwrap_or_else(|error| panic!("{name} must exist in the registry: {error}"));
        assert!(
            !present.contains(&type_ref.local_id),
            "{name} is not renderable by ControlFactory and must not appear in the gallery"
        );
    }
}

#[test]
fn every_section_and_collection_is_present() {
    let app = app();
    for node in [
        ids::SURFACE,
        ids::TOOLBAR,
        ids::ROOT_SCROLL,
        ids::HERO_COLUMN,
        ids::TYPO_COLUMN,
        ids::CTRL_COLUMN,
        ids::LAYOUT_COLUMN,
        ids::COLL_COLUMN,
        ids::INSPECT_COLUMN,
        ids::CONN_COLUMN,
    ] {
        assert!(
            app.session().contains_node(node),
            "section root {} must exist",
            node.get()
        );
    }

    app.session().with_store(|store| {
        assert_eq!(
            store
                .get_model(ids::LIST_MODEL)
                .expect("list model")
                .item_count(),
            ui::BASELINE_LIST_ROWS.len() as u64
        );
        assert_eq!(
            store
                .get_model(ids::TABLE_MODEL)
                .expect("table model")
                .item_count(),
            ui::BASELINE_TABLE_ROWS.len() as u64
        );
    });

    // All eight standard text roles are demonstrated, one node each.
    let roles: BTreeSet<String> = ids::TYPO_ROLE_NODES
        .iter()
        .map(|node| {
            let node = app.session().get_node(*node).expect("typography node");
            format!("{:?}", Text::role_of(&node).expect("role is set"))
        })
        .collect();
    assert_eq!(roles.len(), 8, "one node per standard text role");
}

// =============================================================================
// Resource delivery (§14, §19.2)
// =============================================================================

#[test]
fn gallery_image_is_published_and_referenced_by_the_image_node() {
    let app = app();
    let hash = app.image().expect("image published");

    let entry = app
        .session()
        .lookup_resource(&hash)
        .expect("published resource is retained");
    assert_eq!(
        entry.bytes.as_ref(),
        srui_example_ui_gallery::GALLERY_IMAGE,
        "the retained bytes must be the committed asset"
    );
    assert_eq!(
        entry.encoded_length as usize,
        srui_example_ui_gallery::GALLERY_IMAGE.len()
    );

    let node = app.session().get_node(ids::HERO_IMAGE).expect("image node");
    assert_eq!(
        Image::resource_of(&node),
        Some(hash),
        "the image node must reference the published hash, not inline bytes"
    );
}

// =============================================================================
// Events (§7.6, §7.7, §27)
// =============================================================================

#[test]
fn button_activation_commits_exactly_one_status_transaction() {
    let app = app();
    let mut client = Client::new();

    let before = app.session().current_revision();
    let event = client.activate(app.session(), ids::BTN_NORMAL);
    let outcome = dispatch(app.session(), &event);

    assert!(
        matches!(outcome, EventOutcome::Processed { .. }),
        "ACTIVATE on an enabled button must be accepted, got {outcome:?}"
    );
    assert_eq!(
        app.session().current_revision(),
        before + 1,
        "one accepted event produces exactly one transaction"
    );
    let status = text_of(app.session(), ids::CTRL_STATUS);
    assert!(
        status.contains("ACTIVATE") && status.contains("Normal"),
        "status line must report the activation, got {status:?}"
    );
}

#[test]
fn toggle_value_changed_echoes_the_authoritative_value() {
    let app = app();
    let mut client = Client::new();

    let event = client.value_changed(app.session(), ids::TOGGLE_SWITCH, true);
    dispatch(app.session(), &event);

    let node = app
        .session()
        .get_node(ids::TOGGLE_SWITCH)
        .expect("toggle node");
    assert_eq!(
        Toggle::value_of(&node),
        Some(true),
        "the server echoes the value back rather than trusting the client's local view"
    );
    assert!(text_of(app.session(), ids::CTRL_STATUS).contains("VALUE_CHANGED"));
}

#[test]
fn autoplay_toggle_drives_server_state() {
    let app = app();
    let mut client = Client::new();

    assert!(!app.autoplay());
    let on = client.value_changed(app.session(), ids::TOGGLE_AUTOPLAY, true);
    dispatch(app.session(), &on);
    assert!(app.autoplay());

    let off = client.value_changed(app.session(), ids::TOGGLE_AUTOPLAY, false);
    dispatch(app.session(), &off);
    assert!(!app.autoplay());
}

#[test]
fn scene_buttons_advance_and_reset_the_tour() {
    let app = app();
    let mut client = Client::new();

    let next = client.activate(app.session(), ids::BTN_NEXT);
    dispatch(app.session(), &next);
    assert_eq!(app.scene(), Scene::Content);

    let previous = client.activate(app.session(), ids::BTN_PREV);
    dispatch(app.session(), &previous);
    assert_eq!(app.scene(), Scene::Baseline);

    let next = client.activate(app.session(), ids::BTN_NEXT);
    dispatch(app.session(), &next);
    let reset = client.activate(app.session(), ids::BTN_RESET);
    dispatch(app.session(), &reset);
    assert_eq!(app.scene(), Scene::Baseline);
}

#[test]
fn collection_selection_is_resolved_against_authoritative_state() {
    let app = app();
    let mut client = Client::new();

    let known = client.selection(app.session(), ids::LIST, ids::LIST_ITEMS[2]);
    dispatch(app.session(), &known);
    assert_eq!(
        app.with_state(|state| state.list_selection),
        Some(ids::LIST_ITEMS[2])
    );
    assert!(text_of(app.session(), ids::COLL_SELECTION).contains("List selection"));

    let unknown = client.selection(app.session(), ids::TABLE, ItemId::new(9_999));
    dispatch(app.session(), &unknown);
    assert_eq!(
        app.with_state(|state| state.table_selection),
        None,
        "an item the server does not hold must never become a recorded selection"
    );
    assert!(text_of(app.session(), ids::COLL_SELECTION).contains("refused"));
}

// =============================================================================
// Scenes (§12.1, §13, §23)
// =============================================================================

#[test]
fn each_scene_exercises_its_intended_operation_kinds() {
    let expectations: [(Scene, &[&str]); 6] = [
        (Scene::Content, &["SET_PROPERTY"]),
        (Scene::State, &["SET_PROPERTY"]),
        (Scene::ImageOffline, &["CLEAR_PROPERTY", "SET_PROPERTY"]),
        (
            Scene::Models,
            &["MODEL_INSERT", "MODEL_UPDATE", "MODEL_DELETE"],
        ),
        (
            Scene::Structure,
            &["CREATE_NODE", "MOVE_NODE", "REORDER_CHILDREN"],
        ),
        (Scene::Layout, &["SET_PROPERTY"]),
    ];

    for (scene, required) in expectations {
        let app = app();
        let committed = app.goto_scene(scene).expect("scene applies");
        let observed = kinds(&content_operations(&committed));
        for kind in required {
            assert!(
                observed.contains(kind),
                "{scene:?} must emit {kind}, observed {observed:?}"
            );
        }
        assert_eq!(app.scene(), scene);
    }
}

#[test]
fn image_scene_clears_and_restores_the_cached_resource() {
    let app = app();
    let hash = app.image().expect("image published");

    app.goto_scene(Scene::ImageOffline).expect("scene applies");
    let node = app.session().get_node(ids::HERO_IMAGE).expect("image node");
    assert_eq!(Image::resource_of(&node), None);
    let placeholder = app
        .session()
        .get_node(ids::HERO_IMAGE_PLACEHOLDER)
        .expect("placeholder node");
    assert_eq!(
        Text::visibility_of(&placeholder),
        Some(Visibility::Visible),
        "the placeholder must become visible when the resource is cleared"
    );

    app.goto_scene(Scene::Models).expect("scene applies");
    let node = app.session().get_node(ids::HERO_IMAGE).expect("image node");
    assert_eq!(
        Image::resource_of(&node),
        Some(hash),
        "restoring must re-reference the same hash, not publish new bytes"
    );
    assert!(
        app.session().lookup_resource(&hash).is_some(),
        "the resource stays retained across the clear, so no re-transfer is needed"
    );
}

#[test]
fn model_scene_mutates_and_restores_both_collections() {
    let app = app();

    app.goto_scene(Scene::Models).expect("scene applies");
    let (list_ids, table_ids) = app.session().with_store(|store| {
        let list = store.get_model(ids::LIST_MODEL).expect("list model");
        let table = store.get_model(ids::TABLE_MODEL).expect("table model");
        let collect = |model: &srui_semantic_tree::Model| -> Vec<u64> {
            (0..model.item_count())
                .map(|index| model.get_item_by_index(index).expect("item").item_id.get())
                .collect()
        };
        (collect(list), collect(table))
    });

    assert_eq!(
        list_ids,
        vec![1, 90, 91, 2, 3, 5],
        "insert at 1, delete id 4"
    );
    assert_eq!(table_ids, vec![101, 102, 103, 190], "append, delete id 104");

    app.reset().expect("reset applies");
    let (list_ids, table_ids) = app.session().with_store(|store| {
        let list = store.get_model(ids::LIST_MODEL).expect("list model");
        let table = store.get_model(ids::TABLE_MODEL).expect("table model");
        let collect = |model: &srui_semantic_tree::Model| -> Vec<u64> {
            (0..model.item_count())
                .map(|index| model.get_item_by_index(index).expect("item").item_id.get())
                .collect()
        };
        (collect(list), collect(table))
    });
    assert_eq!(list_ids, vec![1, 2, 3, 4, 5]);
    assert_eq!(table_ids, vec![101, 102, 103, 104]);
}

#[test]
fn structure_scene_creates_moves_and_deletes_one_transient_node() {
    let app = app();

    app.goto_scene(Scene::Structure).expect("scene applies");
    let created = app.with_state(|state| state.scenes.transient().to_vec());
    assert_eq!(created.len(), 1, "the structure scene creates one node");
    let badge = app
        .session()
        .get_node(created[0])
        .expect("transient badge exists while the scene is applied");
    assert_eq!(
        badge.parent_id,
        Some(ids::CTRL_COLUMN),
        "MOVE_NODE reparents"
    );

    let row = app
        .session()
        .get_node(ids::CTRL_BUTTON_ROW)
        .expect("button row");
    let reversed: Vec<NodeId> = ids::CTRL_BUTTON_ORDER.iter().rev().copied().collect();
    assert_eq!(row.ordered_children, reversed, "REORDER_CHILDREN applied");

    app.reset().expect("reset applies");
    assert!(
        !app.session().contains_node(created[0]),
        "the transient node must be deleted on revert"
    );
    assert!(app.with_state(|state| state.scenes.transient().is_empty()));
    let row = app
        .session()
        .get_node(ids::CTRL_BUTTON_ROW)
        .expect("button row");
    assert_eq!(row.ordered_children, ids::CTRL_BUTTON_ORDER.to_vec());
}

#[test]
fn transient_node_ids_are_never_reused() {
    let app = app();
    let mut seen: BTreeSet<u64> = BTreeSet::new();

    for _ in 0..3 {
        app.goto_scene(Scene::Structure).expect("scene applies");
        let created = app.with_state(|state| state.scenes.transient().to_vec());
        for node in created {
            assert!(
                seen.insert(node.get()),
                "DELETE_NODE retires an id permanently: {} must not be reused",
                node.get()
            );
            assert!(node.get() >= ids::TRANSIENT_ID_BASE);
        }
        app.reset().expect("reset applies");
    }
    assert_eq!(seen.len(), 3);
}

#[test]
fn a_full_scene_cycle_and_reset_restore_the_baseline() {
    let app = app();
    let baseline = fingerprint(app.session());
    let baseline_nodes: BTreeSet<u64> = all_nodes(app.session())
        .iter()
        .map(|node| node.id.get())
        .collect();

    for _ in 0..SCENES.len() {
        app.next_scene().expect("scene advances");
    }
    assert_eq!(
        app.scene(),
        Scene::Baseline,
        "a full cycle returns to the baseline scene"
    );
    assert_same_graph(
        &fingerprint(app.session()),
        &baseline,
        "cycling through every scene must restore the baseline graph",
    );

    // Exercise every control value and gallery-owned selection before resetting from an
    // arbitrary mid-tour scene. Reset must restore state changed by events as well as scenes.
    let mut client = Client::new();
    for (node, value) in [
        (ids::TOGGLE_CHECKBOX, false),
        (ids::TOGGLE_SWITCH, true),
        (ids::TOGGLE_AUTOMATIC, true),
        (ids::TOGGLE_AUTOPLAY, true),
    ] {
        let event = client.value_changed(app.session(), node, value);
        dispatch(app.session(), &event);
    }
    let button = client.activate(app.session(), ids::BTN_DESTRUCTIVE);
    dispatch(app.session(), &button);
    let selection = client.selection(app.session(), ids::LIST, ids::LIST_ITEMS[1]);
    dispatch(app.session(), &selection);
    assert!(
        app.autoplay(),
        "the precondition must leave autoplay enabled"
    );

    app.goto_scene(Scene::Structure).expect("scene applies");
    app.reset().expect("reset applies");
    assert_same_graph(
        &fingerprint(app.session()),
        &baseline,
        "reset after control events and a scene must restore the baseline graph",
    );
    assert!(!app.autoplay(), "reset must stop autoplay");
    assert_eq!(
        app.with_state(|state| (state.list_selection, state.table_selection)),
        (None, None),
        "reset must clear gallery-owned selection state"
    );

    let after: BTreeSet<u64> = all_nodes(app.session())
        .iter()
        .map(|node| node.id.get())
        .collect();
    assert_eq!(
        after, baseline_nodes,
        "reset is a revert, not a rebuild: node identity survives"
    );
}

#[test]
fn failed_structure_commit_rolls_back_graph_and_all_gallery_state() {
    let app = app();
    let baseline = fingerprint(app.session());
    let revision_before = app.session().current_revision();
    let state_before = app.with_state(|state| {
        (
            state.scene,
            state.autoplay,
            state.scenes.transient().to_vec(),
            state.trace.len(),
            state.trace.next_seq(),
            state.metrics.transactions(),
            state.metrics.events(),
            state.metrics.bytes(),
        )
    });

    let failed = app.mutate("failing structure probe", |ui, state| {
        Scene::Structure.apply(ui, &mut state.scenes)?;
        state.scene = Scene::Structure;
        state.metrics.observe_event(Duration::from_micros(123), 7);
        ui.delete(ids::CONN_LAG)?;
        Ok(())
    });

    assert!(
        failed.is_err(),
        "the deleted telemetry node must reject render"
    );
    assert_eq!(
        app.session().current_revision(),
        revision_before,
        "a rejected transaction must not advance semantic state"
    );
    assert_same_graph(
        &fingerprint(app.session()),
        &baseline,
        "a rejected structure scene must leave the semantic graph unchanged",
    );
    assert_eq!(
        app.with_state(|state| {
            (
                state.scene,
                state.autoplay,
                state.scenes.transient().to_vec(),
                state.trace.len(),
                state.trace.next_seq(),
                state.metrics.transactions(),
                state.metrics.events(),
                state.metrics.bytes(),
            )
        }),
        state_before,
        "scene bookkeeping, trace allocators, metrics, and caches publish atomically"
    );

    app.goto_scene(Scene::Structure)
        .expect("a valid structure transition still succeeds after rollback");
    let created = app.with_state(|state| state.scenes.transient().to_vec());
    assert_eq!(created.len(), 1);
    assert_eq!(
        created[0].get(),
        ids::TRANSIENT_ID_BASE,
        "the failed allocation must not consume a transient id"
    );
    app.reset().expect("the successful structure scene reverts");
    assert_same_graph(
        &fingerprint(app.session()),
        &baseline,
        "a failed structure attempt must not wedge later transitions",
    );
}

#[test]
fn panicking_transaction_does_not_publish_gallery_state() {
    let app = app();
    let revision_before = app.session().current_revision();
    let failed = app.mutate("panic rollback probe", |_, state| {
        state.scene = Scene::Structure;
        state.autoplay = true;
        panic!("intentional gallery rollback probe");
    });

    assert!(
        failed.is_err(),
        "Session must convert the panic into an error"
    );
    assert_eq!(app.session().current_revision(), revision_before);
    assert_eq!(app.scene(), Scene::Baseline);
    assert!(!app.autoplay());
}

#[test]
fn reaching_a_scene_by_any_route_produces_the_same_graph() {
    let direct = app();
    direct.goto_scene(Scene::Layout).expect("scene applies");
    let expected = fingerprint(direct.session());

    let wandering = app();
    for scene in [
        Scene::Models,
        Scene::Structure,
        Scene::ImageOffline,
        Scene::Content,
        Scene::Layout,
    ] {
        wandering.goto_scene(scene).expect("scene applies");
    }

    assert_same_graph(
        &fingerprint(wandering.session()),
        &expected,
        "revert-then-apply must make the tour path-independent",
    );
}

// =============================================================================
// Protocol inspector
// =============================================================================

#[test]
fn inspector_status_tracks_each_trigger_and_its_committing_revision() {
    let app = app();
    assert!(
        text_of(app.session(), ids::INSPECT_LAST_EVENT)
            .contains("TRANSACTION · initial gallery graph"),
        "startup must replace the inspector placeholder with its actual trigger"
    );
    assert_eq!(
        text_of(app.session(), ids::INSPECT_REVISION),
        format!("Revision: {}", app.session().current_revision())
    );

    let mut client = Client::new();
    let event = client.value_changed(app.session(), ids::TOGGLE_SWITCH, true);
    dispatch(app.session(), &event);
    let last_event = text_of(app.session(), ids::INSPECT_LAST_EVENT);
    assert!(
        last_event.contains("VALUE_CHANGED")
            && last_event.contains(&format!("node {}", ids::TOGGLE_SWITCH.get()))
            && last_event.contains("seq 1"),
        "the inspector must identify the current client event, got {last_event:?}"
    );
    assert_eq!(
        text_of(app.session(), ids::INSPECT_REVISION),
        format!("Revision: {}", app.session().current_revision())
    );

    app.goto_scene(Scene::Content).expect("scene applies");
    assert!(
        text_of(app.session(), ids::INSPECT_LAST_EVENT).contains("TRANSACTION · scene"),
        "a programmatic scene change must replace the current trigger"
    );
    assert_eq!(
        text_of(app.session(), ids::INSPECT_REVISION),
        format!("Revision: {}", app.session().current_revision())
    );

    app.reset().expect("reset applies");
    assert!(
        text_of(app.session(), ids::INSPECT_LAST_EVENT).contains("TRANSACTION · reset to baseline"),
        "reset must publish its own trigger"
    );
    assert_eq!(
        text_of(app.session(), ids::INSPECT_REVISION),
        format!("Revision: {}", app.session().current_revision())
    );
}

#[test]
fn inspector_records_both_directions_and_stays_bounded() {
    let app = app();
    let mut client = Client::new();

    let event = client.activate(app.session(), ids::BTN_NORMAL);
    dispatch(app.session(), &event);

    let rows = trace_rows(app.session());
    assert!(
        !rows.is_empty(),
        "the inspector records the bootstrap graph"
    );
    assert!(
        rows.iter()
            .any(|row| row[1] == "C\u{2192}S" && row[2] == "ACTIVATE"),
        "an inbound ACTIVATE must be recorded, got {rows:?}"
    );
    assert!(
        rows.iter()
            .any(|row| row[1] == "S\u{2192}C" && row[2] == "SET_PROPERTY"),
        "the outbound operations it produced must be recorded"
    );

    // The row describing a change is committed with the change, never after it.
    let committed = app.next_scene().expect("scene advances");
    assert!(
        committed
            .iter()
            .any(|op| matches!(op, Operation::ModelInsert { id, .. } if *id == ids::TRACE_MODEL)),
        "inspector rows are inserted inside the transaction they describe"
    );

    for _ in 0..40 {
        app.next_scene().expect("scene advances");
    }
    assert_eq!(
        trace_rows(app.session()).len(),
        srui_example_ui_gallery::MAX_TRACE_ROWS,
        "the inspector model is bounded"
    );
}

#[test]
fn inspector_does_not_describe_its_own_bookkeeping() {
    let app = app();
    let committed = app.next_scene().expect("scene advances");
    let described: Vec<Vec<String>> = trace_rows(app.session());

    // No row may describe a MODEL_INSERT into the inspector model itself, which is what a
    // self-referential log would produce.
    let trace_model = format!("model {} ", ids::TRACE_MODEL.get());
    assert!(
        !described
            .iter()
            .any(|row| row[2] == "MODEL_INSERT" && row[3].starts_with(&trace_model)),
        "the inspector must not log its own rows"
    );
    assert!(
        committed
            .iter()
            .any(|op| matches!(op, Operation::ModelInsert { id, .. } if *id == ids::TRACE_MODEL)),
        "\u{2026} even though those operations are really committed"
    );
}

// =============================================================================
// Connection statistics
// =============================================================================

#[test]
fn connection_statistics_are_published_as_semantic_state() {
    let app = app();
    let mut client = Client::new();
    let event = client.activate(app.session(), ids::BTN_PRIMARY);
    dispatch(app.session(), &event);

    let throughput = text_of(app.session(), ids::CONN_THROUGHPUT);
    assert!(
        throughput.contains("txn") && throughput.contains("B framed"),
        "throughput must report real framed byte counts, got {throughput:?}"
    );

    let resource = text_of(app.session(), ids::CONN_RESOURCE);
    let expected_chunks = srui_example_ui_gallery::GALLERY_IMAGE
        .len()
        .div_ceil(srui_example_ui_gallery::stats::CHUNK_PAYLOAD_SIZE);
    assert!(
        resource.contains(&format!("{expected_chunks} chunks")),
        "resource line must report the exact chunk count, got {resource:?}"
    );

    let latency = text_of(app.session(), ids::CONN_LATENCY);
    assert!(
        latency.contains("p50") && latency.contains("µs"),
        "server-side latency percentiles must be published, got {latency:?}"
    );

    let revision = app.session().current_revision();
    assert_eq!(
        text_of(app.session(), ids::CONN_REVISION),
        format!("Revision: {revision} (journal head {revision})"),
        "the statistics carried by a transaction must name its committed revision"
    );

    let observed = app.with_state(|state| {
        (
            state.metrics.transactions(),
            state.metrics.bytes(),
            state.metrics.buckets(),
        )
    });
    assert!(observed.0 >= 2, "bootstrap plus the event transaction");
    assert!(observed.1 > 0, "framed bytes are measured, not guessed");
    assert_eq!(
        observed.2.iter().sum::<u64>(),
        observed.0,
        "every measured transaction lands in exactly one size bucket"
    );
    assert_eq!(observed.2.len(), SIZE_BUCKETS.len());
}

#[test]
fn concurrent_commits_capture_facts_after_gallery_state_serialization() {
    let app = app();
    let barrier = Arc::new(Barrier::new(3));

    let handles = app.with_state(|_| {
        let mut handles = Vec::new();
        for _ in 0..2 {
            let app = app.clone();
            let barrier = barrier.clone();
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                app.mutate("concurrent facts probe", |_, _| Ok(()))
            }));
        }

        barrier.wait();
        // Both workers have entered commit while this thread owns the state lock. With the old
        // facts-before-state ordering, both captured the same stale revision before blocking.
        std::thread::sleep(Duration::from_millis(100));
        handles
    });

    for handle in handles {
        handle
            .join()
            .expect("concurrent commit thread does not panic")
            .expect("concurrent commit succeeds");
    }

    let revision = app.session().current_revision();
    assert_eq!(
        text_of(app.session(), ids::CONN_REVISION),
        format!("Revision: {revision} (journal head {revision})"),
        "the last serialized commit must render facts for its own revision"
    );
    assert_eq!(
        text_of(app.session(), ids::INSPECT_REVISION),
        format!("Revision: {revision}")
    );
}

#[test]
fn telemetry_nodes_are_excluded_from_the_baseline_fingerprint() {
    let app = app();
    let baseline = fingerprint(app.session());

    // Committing a transaction that only moves telemetry forward must not change the fingerprint.
    app.mutate("no-op probe", |_, _| Ok(())).expect("commits");

    assert_same_graph(
        &fingerprint(app.session()),
        &baseline,
        "telemetry-only transactions leave the gallery graph untouched",
    );
    assert!(
        app.session().current_revision() > 1,
        "the probe really did commit"
    );
}

// =============================================================================
// Scene metadata
// =============================================================================

#[test]
fn scene_metadata_is_unique_and_ordered() {
    let mut names: BTreeMap<&str, usize> = BTreeMap::new();
    for (index, scene) in SCENES.iter().enumerate() {
        assert_eq!(scene.index(), index, "SCENES order defines Scene::index");
        assert_eq!(Scene::from_index(index), *scene);
        *names.entry(scene.name()).or_default() += 1;
        assert!(scene.label().contains(scene.name()));
    }
    assert_eq!(names.len(), SCENES.len(), "scene names are unique");
    assert_eq!(SCENES[0], Scene::Baseline);
    assert_eq!(Scene::Baseline.previous(), SCENES[SCENES.len() - 1]);
    assert_eq!(SCENES[SCENES.len() - 1].next(), Scene::Baseline);
}
