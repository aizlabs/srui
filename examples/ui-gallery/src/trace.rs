//! Live protocol inspector: renders the semantic traffic in both directions (§7.6, §12.1, §13).
//!
//! # Why this cannot loop
//!
//! Appending an inspector row is itself a mutation, so a naive "log every transaction" design
//! would log its own log forever. The inspector avoids that by being **self-describing inside a
//! single transaction**: [`TraceLog::record`] runs after the caller has staged its operations but
//! before the commit, reads the operations already on [`UiTransaction`], and appends the rows
//! describing them to the same transaction. The row and the change it describes therefore reach
//! the client atomically at one revision (§12.1), and no second transaction exists to recurse on.
//!
//! The inspector's own `MODEL_INSERT`/`MODEL_DELETE` operations and the telemetry `SET_PROPERTY`
//! operations from [`crate::stats`] are deliberately not traced — they are bookkeeping about the
//! traffic, not the traffic itself, and tracing them would drown the interesting rows.
//!
//! # What this shows, and what it cannot
//!
//! - **server → client**: every [`Operation`] the application commits.
//! - **client → server**: every [`srui_protocol::Event`] that reached a handler.
//!
//! It cannot show framing, the handshake, `EVENT_ACK`, or `RESOURCE_*` chunk frames: those are
//! produced by `handle_connection` below the `Session` API and are never visible to an application.

use srui_sdk::{ItemId, NodeId, Operation, StoreError, TypeRef, UiTransaction, Value};
use srui_semantic_tree::ModelItem;

use crate::ids;

/// Rows retained in the inspector model. Oldest rows are dropped from the bottom (§8, §26).
pub const MAX_TRACE_ROWS: usize = 40;
/// Operation rows emitted for a single transaction before the remainder is summarised.
pub const MAX_ROWS_PER_TRANSACTION: usize = 12;

/// Column titles of the inspector table.
pub const TRACE_COLUMNS: [&str; 4] = ["#", "Dir", "Message", "Detail"];

const SERVER_TO_CLIENT: &str = "S\u{2192}C";
const CLIENT_TO_SERVER: &str = "C\u{2192}S";

/// What caused the transaction being traced.
#[derive(Debug, Clone)]
pub enum Trigger {
    /// Server-initiated: startup, an autoplay tick, or a programmatic scene change.
    Server { label: String },
    /// A client event that reached [`srui_sessiond::Session::process_event`] (§7.6, §7.7).
    Event {
        node: NodeId,
        event_type: TypeRef,
        event_seq: u64,
        observed_revision: u64,
        detail: String,
    },
}

impl Trigger {
    /// Convenience constructor for a server-initiated trigger.
    pub fn server(label: impl Into<String>) -> Self {
        Self::Server {
            label: label.into(),
        }
    }

    fn row(&self) -> (&'static str, String, String) {
        match self {
            Self::Server { label } => (SERVER_TO_CLIENT, "TRANSACTION".to_string(), label.clone()),
            Self::Event {
                node,
                event_type,
                event_seq,
                observed_revision,
                detail,
            } => (
                CLIENT_TO_SERVER,
                event_type
                    .standard_event_name()
                    .unwrap_or("EVENT")
                    .to_string(),
                format!(
                    "node {} \u{b7} seq {event_seq} \u{b7} observed rev {observed_revision}{}",
                    node.get(),
                    if detail.is_empty() {
                        String::new()
                    } else {
                        format!(" \u{b7} {detail}")
                    }
                ),
            ),
        }
    }
}

/// Bounded, newest-first log of semantic traffic backed by [`ids::TRACE_MODEL`].
#[derive(Debug)]
pub struct TraceLog {
    next_seq: u64,
    next_item: u64,
    len: usize,
}

impl Default for TraceLog {
    fn default() -> Self {
        Self {
            next_seq: 1,
            next_item: ids::TRACE_ITEM_BASE,
            len: 0,
        }
    }
}

impl TraceLog {
    /// Number of rows currently retained in the model.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the inspector has recorded anything yet.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Sequence number the next recorded row will receive.
    pub fn next_seq(&self) -> u64 {
        self.next_seq
    }

    /// Appends the trigger and the operations it produced to the inspector model.
    ///
    /// `operations` must be the slice staged on the transaction *before* this call, so the
    /// inspector never describes its own rows.
    pub fn record(
        &mut self,
        ui: &mut UiTransaction,
        trigger: &Trigger,
        operations: &[Operation],
    ) -> Result<(), StoreError> {
        let mut chronological: Vec<(&'static str, String, String)> = Vec::new();

        let (direction, kind, detail) = trigger.row();
        chronological.push((direction, kind, detail));

        let shown = operations.len().min(MAX_ROWS_PER_TRANSACTION);
        for operation in &operations[..shown] {
            let (kind, detail) = describe(operation);
            chronological.push((SERVER_TO_CLIENT, kind, detail));
        }
        if operations.len() > shown {
            chronological.push((
                SERVER_TO_CLIENT,
                "\u{2026}".to_string(),
                format!(
                    "{} further operations in this transaction",
                    operations.len() - shown
                ),
            ));
        }

        // Newest row sits at index 0, so the batch is inserted in reverse chronological order.
        let items: Vec<ModelItem> = chronological
            .into_iter()
            .map(|(direction, kind, detail)| {
                let seq = self.next_seq;
                self.next_seq += 1;
                let item_id = ItemId::new(self.next_item);
                self.next_item += 1;
                ModelItem::new(
                    item_id,
                    Value::List(vec![
                        Value::String(seq.to_string()),
                        Value::String(direction.to_string()),
                        Value::String(kind),
                        Value::String(detail),
                    ]),
                    [],
                )
            })
            .rev()
            .collect();

        let inserted = items.len();
        ui.apply_op(&Operation::model_insert(ids::TRACE_MODEL, 0, items))?;
        self.len += inserted;

        if self.len > MAX_TRACE_ROWS {
            let excess = self.len - MAX_TRACE_ROWS;
            ui.apply_op(&Operation::model_delete_range(
                ids::TRACE_MODEL,
                MAX_TRACE_ROWS as u64,
                excess as u64,
            ))?;
            self.len = MAX_TRACE_ROWS;
        }

        Ok(())
    }
}

/// Renders one operation as `(wire operation name, human detail)`.
///
/// Names come from the generated registry tables rather than a local copy, so a registry rename
/// shows up here instead of silently drifting (§6.4).
pub fn describe(operation: &Operation) -> (String, String) {
    match operation {
        Operation::CreateNode {
            id,
            node_type,
            parent_id,
            properties,
            ..
        } => (
            "CREATE_NODE".to_string(),
            format!(
                "{} node {} under {} \u{b7} {} propert{}",
                type_name(*node_type),
                id.get(),
                parent_id.map_or("root".to_string(), |p| p.get().to_string()),
                properties.len(),
                if properties.len() == 1 { "y" } else { "ies" }
            ),
        ),
        Operation::DeleteNode { id } => ("DELETE_NODE".to_string(), format!("node {}", id.get())),
        Operation::SetProperty {
            id,
            property,
            value,
        } => (
            "SET_PROPERTY".to_string(),
            format!(
                "node {} \u{b7} {} = {}",
                id.get(),
                property_name(*property),
                render_value(value)
            ),
        ),
        Operation::ClearProperty { id, property } => (
            "CLEAR_PROPERTY".to_string(),
            format!("node {} \u{b7} {}", id.get(), property_name(*property)),
        ),
        Operation::MoveNode {
            id,
            new_parent_id,
            new_child_index,
        } => (
            "MOVE_NODE".to_string(),
            format!(
                "node {} \u{2192} parent {} index {}",
                id.get(),
                new_parent_id.map_or("root".to_string(), |p| p.get().to_string()),
                new_child_index.map_or("end".to_string(), |i| i.to_string())
            ),
        ),
        Operation::ReorderChildren {
            parent_id,
            new_order,
        } => (
            "REORDER_CHILDREN".to_string(),
            format!(
                "parent {} \u{b7} [{}]",
                parent_id.get(),
                new_order
                    .iter()
                    .map(|id| id.get().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ),
        Operation::BatchPropertySet { id, properties } => (
            "BATCH_PROPERTY_SET".to_string(),
            format!("node {} \u{b7} {} properties", id.get(), properties.len()),
        ),
        Operation::CreateModel {
            id,
            model_type,
            item_count,
        } => (
            "CREATE_MODEL".to_string(),
            format!(
                "model {} \u{b7} {} \u{b7} {item_count} items",
                id.get(),
                type_name(*model_type)
            ),
        ),
        Operation::ModelInsert { id, index, items } => (
            "MODEL_INSERT".to_string(),
            format!("model {} @ {index} \u{b7} {} items", id.get(), items.len()),
        ),
        Operation::ModelDelete {
            id,
            index,
            count,
            item_ids,
        } => (
            "MODEL_DELETE".to_string(),
            match (index, count) {
                (Some(index), Some(count)) => {
                    format!("model {} @ {index} \u{b7} {count} items", id.get())
                }
                _ => format!(
                    "model {} \u{b7} items [{}]",
                    id.get(),
                    item_ids
                        .iter()
                        .map(|item| item.get().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            },
        ),
        Operation::ModelUpdate { id, items, .. } => (
            "MODEL_UPDATE".to_string(),
            format!("model {} \u{b7} {} items", id.get(), items.len()),
        ),
        Operation::ModelResetRange {
            id,
            start_index,
            items,
            ..
        } => (
            "MODEL_RESET_RANGE".to_string(),
            format!(
                "model {} @ {start_index} \u{b7} {} items",
                id.get(),
                items.len()
            ),
        ),
    }
}

fn type_name(type_ref: TypeRef) -> String {
    type_ref
        .standard_name()
        .map(str::to_string)
        .unwrap_or_else(|| format!("type {}:{}", type_ref.namespace_id, type_ref.local_id))
}

fn property_name(property: srui_sdk::PropertyRef) -> String {
    property
        .standard_name()
        .map(str::to_string)
        .unwrap_or_else(|| format!("property {}:{}", property.namespace_id, property.local_id))
}

/// Renders a value compactly, truncating long strings so one row stays readable.
fn render_value(value: &Value) -> String {
    const MAX_LEN: usize = 48;
    match value {
        Value::String(text) if text.chars().count() > MAX_LEN => {
            let truncated: String = text.chars().take(MAX_LEN).collect();
            format!("\"{truncated}\u{2026}\"")
        }
        Value::String(text) => format!("{text:?}"),
        Value::ResourceHash(hash) => {
            format!("resource {}\u{2026}", &hash.to_hex()[..8])
        }
        Value::EnumToken(token) => format!("enum {}:{}", token.enum_id, token.value_id),
        Value::List(items) => format!("list[{}]", items.len()),
        other => format!("{other}"),
    }
}
