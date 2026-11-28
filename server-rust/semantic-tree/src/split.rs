//! The standard `Split` container (§4 inv. 13, §7.3, §7.6, §10, §22.4, §22.5, §26).
//!
//! A `Split` (registry node type 25) divides its area into user-resizable panes: each child is
//! one pane, in child order. The standard property 35 `split_axis` (enum 13 `SplitAxis`) names
//! the axis: `side_by_side` (the default when absent) or `stacked`. It is admitted only on a
//! `Split`, and a value outside `SplitAxis` is refused at decode and at apply, identically by
//! the Rust store and the Swift replica (`client-macos/SemanticModel/Split.swift`).
//!
//! A `Split` holds at most [`MAX_SPLIT_PANES`] children; a `CREATE_NODE` or `MOVE_NODE` that
//! would give it a ninth fails the whole transaction (§12.1). Nesting counts toward the
//! existing `max_tree_depth` limit like any other container.
//!
//! Divider positions are local interaction state (§7.6 "resize locally", §22.5): nothing here
//! carries a position, and no client reports one. The optional `VALUE_CHANGED` the registry
//! declares for `Split` is not originated by any client in this repository.

use crate::ids::{StandardSplitAxis, TypeRef};
use crate::value::Value;
use std::fmt;

/// Most panes (children) one `Split` holds (§26).
pub const MAX_SPLIT_PANES: usize = 8;

/// Registry id of the `SplitAxis` enum (protocol/registry.yaml `enums[].id`).
pub const SPLIT_AXIS_ENUM_ID: u32 = 13;

/// Why a `split_axis` value is refused (§4 inv. 13).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitAxisError {
    /// The value is not an enum token at all.
    NotAnEnumValue,
    /// The value is an enum token that is not `SplitAxis` `side_by_side` or `stacked`.
    UnknownValue { enum_id: u32, value_id: u32 },
}

impl fmt::Display for SplitAxisError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The Swift replica formats the same text, byte for byte (malformed vectors compare it).
        let expected = "split_axis must be SplitAxis side_by_side (13:1) or stacked (13:2)";
        match self {
            Self::NotAnEnumValue => write!(f, "{expected}, got a non-enum value"),
            Self::UnknownValue { enum_id, value_id } => {
                write!(f, "{expected}, got enum {enum_id} value {value_id}")
            }
        }
    }
}

impl std::error::Error for SplitAxisError {}

/// Returns `true` for the standard `Split` node type.
pub fn is_split(node_type: TypeRef) -> bool {
    node_type == TypeRef::SPLIT
}

/// Parses a `split_axis` value: only `SplitAxis` `side_by_side` or `stacked` is accepted.
pub fn parse_split_axis(value: &Value) -> Result<StandardSplitAxis, SplitAxisError> {
    let Value::EnumToken(token) = value else {
        return Err(SplitAxisError::NotAnEnumValue);
    };
    StandardSplitAxis::try_from(*token).map_err(|()| SplitAxisError::UnknownValue {
        enum_id: token.enum_id,
        value_id: token.value_id,
    })
}

/// The axis a `Split` lays its panes out on: its `split_axis`, or `side_by_side` when absent.
///
/// Returns `None` for a value the store would refuse.
pub fn effective_split_axis(value: Option<&Value>) -> Option<StandardSplitAxis> {
    match value {
        None => Some(StandardSplitAxis::SideBySide),
        Some(value) => parse_split_axis(value).ok(),
    }
}
