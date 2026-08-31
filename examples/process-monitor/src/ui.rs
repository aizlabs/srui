//! Initial semantic graph construction (§7.2, §7.3).

use srui_sdk::*;
use srui_semantic_tree::ModelItem;
use srui_sessiond::{Session, SessionError};

use crate::domain::{
    VisibleRow, ACTIONS_ROW_ID, COLUMN_ID, COLUMN_TITLES, CPU_PROGRESS_ID, HEADING_ID,
    KILL_ACTION_KEY, KILL_BUTTON_ID, MAX_ITEMS_PER_MODEL_OP, MEM_PROGRESS_ID, PROCESS_MODEL_ID,
    PROCESS_TABLE_ID, SHOW_ALL_ACTION_KEY, SHOW_ALL_ID, STATS_ROW_ID, SURFACE_ID,
};
use crate::state::MonitorState;

/// Creates the model, its initial items, and the complete node tree in one transaction (§12.1).
pub fn build_initial_ui(session: &Session, state: &MonitorState) -> Result<(), SessionError> {
    let items: Vec<ModelItem> = state
        .visible()
        .iter()
        .map(VisibleRow::to_model_item)
        .collect();
    let cpu = state.cpu().clone();
    let mem = state.mem().clone();
    let show_all = state.show_all();

    session.transaction(move |ui| {
        ui.apply_op(&Operation::create_model(
            PROCESS_MODEL_ID,
            TypeRef::TABLE,
            0,
        ))?;
        let mut offset: u64 = 0;
        for chunk in items.chunks(MAX_ITEMS_PER_MODEL_OP) {
            let chunk_len = chunk.len() as u64;
            ui.apply_op(&Operation::model_insert(
                PROCESS_MODEL_ID,
                offset,
                chunk.to_vec(),
            ))?;
            offset += chunk_len;
        }

        Surface::builder(SURFACE_ID)
            .label("System Monitor")
            .create(ui)?;

        Column::builder(COLUMN_ID)
            .parent(SURFACE_ID)
            .spacing_role(SpacingRole::Normal)
            .padding_role(PaddingRole::Normal)
            .grow(1.0)
            .create(ui)?;

        Row::builder(STATS_ROW_ID)
            .parent(COLUMN_ID)
            .spacing_role(SpacingRole::Normal)
            .create(ui)?;

        Text::builder(HEADING_ID)
            .parent(STATS_ROW_ID)
            .text("System Monitor")
            .role(TextRole::Heading)
            .create(ui)?;

        Progress::builder(CPU_PROGRESS_ID)
            .parent(STATS_ROW_ID)
            .label("CPU")
            .accessible_description("Global CPU utilization")
            .value(cpu.value)
            .value_description(cpu.description.clone())
            .grow(1.0)
            .create(ui)?;

        Progress::builder(MEM_PROGRESS_ID)
            .parent(STATS_ROW_ID)
            .label("Memory")
            .accessible_description("Physical memory in use")
            .value(mem.value)
            .value_description(mem.description.clone())
            .grow(1.0)
            .create(ui)?;

        Toggle::switch(SHOW_ALL_ID)
            .parent(COLUMN_ID)
            .label("Show all processes")
            .value(show_all)
            .action_key(SHOW_ALL_ACTION_KEY)
            .create(ui)?;

        Table::builder(PROCESS_TABLE_ID)
            .parent(COLUMN_ID)
            .model_ref(PROCESS_MODEL_ID)
            .columns(
                COLUMN_TITLES
                    .iter()
                    .map(|title| Value::String((*title).to_string())),
            )
            .selection_mode(SelectionMode::Single)
            .label("Running processes")
            .accessible_description("PID, name, CPU percentage and resident memory per process")
            .grow(1.0)
            .create(ui)?;

        Row::builder(ACTIONS_ROW_ID)
            .parent(COLUMN_ID)
            .spacing_role(SpacingRole::Normal)
            .create(ui)?;

        Button::builder(KILL_BUTTON_ID)
            .parent(ACTIONS_ROW_ID)
            .label("Kill Selected")
            .role(ActionRole::Destructive)
            .action_key(KILL_ACTION_KEY)
            .create(ui)?;

        Ok(())
    })?;

    Ok(())
}
