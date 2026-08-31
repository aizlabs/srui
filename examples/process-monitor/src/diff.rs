//! Minimal collection model diffing algorithm (§8, §13, §23).

use std::collections::{HashMap, HashSet};

use srui_sdk::*;
use srui_semantic_tree::ModelItem;

use crate::domain::{RowValues, VisibleRow, MAX_ITEMS_PER_MODEL_OP};

/// Computes the minimal `MODEL_*` operation list transforming `prev` into `next`.
///
/// Deletions are emitted first so insertion indices are the item's final index in `next`.
/// Retained items are only re-sent when their displayed values changed. A retained item that
/// somehow reordered relative to its peers is deleted and reinserted explicitly rather than left
/// in an inconsistent position.
pub fn diff_visible_rows(
    model: ModelId,
    prev: &[VisibleRow],
    next: &[VisibleRow],
) -> Vec<Operation> {
    let next_positions: HashMap<ItemId, usize> = next
        .iter()
        .enumerate()
        .map(|(index, row)| (row.item_id, index))
        .collect();

    let mut deleted: Vec<ItemId> = Vec::new();
    let mut retained: HashSet<ItemId> = HashSet::new();
    let mut highest_kept: Option<usize> = None;
    for row in prev {
        match next_positions.get(&row.item_id) {
            None => deleted.push(row.item_id),
            Some(&position) => {
                if highest_kept.is_some_and(|kept| position < kept) {
                    deleted.push(row.item_id);
                } else {
                    highest_kept = Some(position);
                    retained.insert(row.item_id);
                }
            }
        }
    }

    let mut operations = Vec::new();
    for chunk in deleted.chunks(MAX_ITEMS_PER_MODEL_OP) {
        operations.push(Operation::model_delete_items(model, chunk.iter().copied()));
    }

    let mut run_start: u64 = 0;
    let mut run: Vec<ModelItem> = Vec::new();
    for (index, row) in next.iter().enumerate() {
        if retained.contains(&row.item_id) {
            flush_insert(&mut operations, model, run_start, &mut run);
            continue;
        }
        if run.is_empty() {
            run_start = index as u64;
        }
        run.push(row.to_model_item());
        if run.len() == MAX_ITEMS_PER_MODEL_OP {
            flush_insert(&mut operations, model, run_start, &mut run);
        }
    }
    flush_insert(&mut operations, model, run_start, &mut run);

    let previous_values: HashMap<ItemId, &RowValues> =
        prev.iter().map(|row| (row.item_id, &row.values)).collect();
    let updates: Vec<ModelItem> = next
        .iter()
        .filter(|row| retained.contains(&row.item_id))
        .filter(|row| {
            previous_values
                .get(&row.item_id)
                .is_some_and(|old| **old != row.values)
        })
        .map(VisibleRow::to_model_item)
        .collect();
    for chunk in updates.chunks(MAX_ITEMS_PER_MODEL_OP) {
        operations.push(Operation::model_update(model, None, chunk.to_vec()));
    }

    operations
}

fn flush_insert(
    operations: &mut Vec<Operation>,
    model: ModelId,
    start: u64,
    items: &mut Vec<ModelItem>,
) {
    if items.is_empty() {
        return;
    }
    operations.push(Operation::model_insert(model, start, std::mem::take(items)));
}
