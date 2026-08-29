//! Initial semantic graph coverage (§7.2, §7.3, §7.7, §8, §12.1).

mod common;

use common::{base_fixture, MIB, UID};
use srui_example_process_monitor::testing::{record, snapshot};
use srui_example_process_monitor::*;
use srui_sdk::*;

#[test]
fn initial_graph_has_the_required_hierarchy_and_node_types() {
    let fixture = base_fixture();

    fixture.session.with_store(|store| {
        let surface = store.get_node(SURFACE_ID).expect("surface");
        assert_eq!(surface.node_type, TypeRef::SURFACE);
        assert_eq!(surface.parent_id, None);
        assert_eq!(surface.ordered_children, vec![COLUMN_ID]);
        assert_eq!(
            surface.get_property(LABEL),
            Some(&Value::String("System Monitor".to_string()))
        );

        let column = store.get_node(COLUMN_ID).expect("column");
        assert_eq!(column.node_type, TypeRef::COLUMN);
        assert_eq!(column.parent_id, Some(SURFACE_ID));
        assert_eq!(
            column.ordered_children,
            vec![STATS_ROW_ID, SHOW_ALL_ID, PROCESS_TABLE_ID, ACTIONS_ROW_ID]
        );

        let stats_row = store.get_node(STATS_ROW_ID).expect("stats row");
        assert_eq!(stats_row.node_type, TypeRef::ROW);
        assert_eq!(stats_row.parent_id, Some(COLUMN_ID));
        assert_eq!(
            stats_row.ordered_children,
            vec![HEADING_ID, CPU_PROGRESS_ID, MEM_PROGRESS_ID]
        );

        let actions_row = store.get_node(ACTIONS_ROW_ID).expect("actions row");
        assert_eq!(actions_row.node_type, TypeRef::ROW);
        assert_eq!(actions_row.ordered_children, vec![KILL_BUTTON_ID]);

        let heading = Text::from_store(store, HEADING_ID).expect("heading");
        assert_eq!(heading.text(store), Some("System Monitor"));
        assert_eq!(heading.role(store), Some(TextRole::Heading));
    });
}

#[test]
fn initial_graph_has_two_determinate_progress_nodes() {
    let fixture = base_fixture();

    fixture.session.with_store(|store| {
        for id in [CPU_PROGRESS_ID, MEM_PROGRESS_ID] {
            let node = store.get_node(id).expect("progress node");
            assert_eq!(node.node_type, TypeRef::PROGRESS);
            assert_eq!(node.parent_id, Some(STATS_ROW_ID));
            let value = Progress::new(id).value(store).expect("determinate value");
            assert!(
                (0.0..=1.0).contains(&value),
                "progress {id:?} must be normalized, got {value}"
            );
        }

        assert_eq!(Progress::new(CPU_PROGRESS_ID).label(store), Some("CPU"));
        assert_eq!(Progress::new(MEM_PROGRESS_ID).label(store), Some("Memory"));
        // 25% CPU and 8 GiB of 16 GiB.
        assert_eq!(Progress::new(CPU_PROGRESS_ID).value(store), Some(0.25));
        assert_eq!(Progress::new(MEM_PROGRESS_ID).value(store), Some(0.5));
    });
}

#[test]
fn toggle_starts_false_with_switch_presentation_hint_and_opaque_action_key() {
    let fixture = base_fixture();

    fixture.session.with_store(|store| {
        let toggle = Toggle::from_store(store, SHOW_ALL_ID).expect("toggle");
        assert_eq!(toggle.label(store), Some("Show all processes"));
        assert_eq!(toggle.value(store), Some(false));
        assert_eq!(
            toggle.presentation_hint(store),
            Some(TogglePresentationHint::Switch)
        );
        // §7.7: the action key is inert metadata on the node, nothing more.
        assert_eq!(toggle.action_key(store), Some(SHOW_ALL_ACTION_KEY));
    });
}

#[test]
fn table_is_model_backed_with_four_columns_and_single_selection() {
    let fixture = base_fixture();

    fixture.session.with_store(|store| {
        let table = Table::from_store(store, PROCESS_TABLE_ID).expect("table");
        assert_eq!(table.model_ref(store), Some(PROCESS_MODEL_ID));
        assert_eq!(table.selection_mode(store), Some(SelectionMode::Single));
        assert_eq!(table.label(store), Some("Running processes"));
        assert!(table.grow(store).is_some_and(|grow| grow > 0.0));

        let columns: Vec<&str> = table
            .columns(store)
            .expect("columns")
            .iter()
            .map(|value| value.as_string().expect("column title"))
            .collect();
        assert_eq!(columns, vec!["PID", "Name", "CPU %", "Memory MiB"]);
    });
}

#[test]
fn kill_button_is_destructive_with_an_opaque_action_key() {
    let fixture = base_fixture();

    fixture.session.with_store(|store| {
        let button = Button::from_store(store, KILL_BUTTON_ID).expect("button");
        assert_eq!(button.label(store), Some("Kill Selected"));
        assert_eq!(button.role(store), Some(ActionRole::Destructive));
        assert_eq!(button.action_key(store), Some(KILL_ACTION_KEY));
    });
}

#[test]
fn initial_model_item_count_matches_the_cached_visible_rows() {
    let fixture = base_fixture();
    // Three same-user processes are visible; the root-owned one is filtered out.
    let visible = fixture.monitor.with_state(|state| state.visible().to_vec());
    assert_eq!(visible.len(), 3);

    fixture.session.with_store(|store| {
        let model = store.get_model(PROCESS_MODEL_ID).expect("process model");
        assert_eq!(model.item_count(), 3);
        assert_eq!(model.cached_item_count(), 3);

        for (index, row) in visible.iter().enumerate() {
            let item = model
                .get_item_by_index(index as u64)
                .expect("cached item at index");
            assert_eq!(item.item_id, row.item_id);
            assert_eq!(item.value, row.values.to_value());
        }
    });
}

#[test]
fn model_rows_are_ordered_semantic_value_lists() {
    let fixture = fixture_with_single_process();

    fixture.session.with_store(|store| {
        let model = store.get_model(PROCESS_MODEL_ID).expect("process model");
        let item = model.get_item_by_index(0).expect("single row");
        assert_eq!(
            item.value,
            Value::List(vec![
                Value::UnsignedInt(77),
                Value::String("solo".to_string()),
                Value::Float64(12.3),
                Value::UnsignedInt(64),
            ])
        );
    });
}

fn fixture_with_single_process() -> common::Fixture {
    common::fixture(snapshot(
        10.0,
        vec![record(77, 5, "solo", 12.34, 64 * MIB, Some(UID))],
    ))
}

#[test]
fn initial_transaction_is_a_single_atomic_commit() {
    let fixture = base_fixture();
    assert_eq!(fixture.session.current_revision(), 1);
}
