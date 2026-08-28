//! Transaction error types (§12.1, §26).

use super::operation::Revision;
use crate::store::error::StoreError;
use std::fmt;

/// Errors returned when validating or applying a semantic transaction (§12.1, §26).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TxnError {
    /// Base revision does not match the store's current committed revision.
    StaleBaseRevision {
        expected: Revision,
        actual: Revision,
    },
    /// New revision is not strictly monotonic (`expected != actual`).
    InvalidNewRevision {
        expected: Revision,
        actual: Revision,
    },
    /// Transaction exceeds the configured maximum operations limit (§26).
    MaxOperationsExceeded {
        limit: usize,
        actual: usize,
    },
    /// An operation within the transaction failed during application.
    OpFailed {
        op_index: usize,
        source: StoreError,
    },
    /// Wire transaction payload decoding or conversion failed.
    WireError(String),
}

impl fmt::Display for TxnError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StaleBaseRevision { expected, actual } => write!(
                f,
                "stale base revision: store committed revision is {}, transaction base is {}",
                expected, actual
            ),
            Self::InvalidNewRevision { expected, actual } => write!(
                f,
                "invalid new revision: expected {} (base + 1), but got {}",
                expected, actual
            ),
            Self::MaxOperationsExceeded { limit, actual } => write!(
                f,
                "transaction operations limit exceeded: {} ops exceeds max limit of {} (§26)",
                actual, limit
            ),
            Self::OpFailed { op_index, source } => {
                write!(f, "operation at index {} failed: {}", op_index, source)
            }
            Self::WireError(msg) => write!(f, "wire transaction error: {}", msg),
        }
    }
}

impl std::error::Error for TxnError {}

impl TxnError {
    /// Returns the canonical conformance error code for this transaction error, if applicable (§32).
    pub fn conformance_code(&self) -> Option<&'static str> {
        match self {
            Self::StaleBaseRevision { .. } => Some("stale_base_revision"),
            Self::InvalidNewRevision { .. } => Some("invalid_new_revision"),
            Self::MaxOperationsExceeded { .. } => Some("max_operations_exceeded"),
            Self::OpFailed { source, .. } => Some(source.conformance_code()),
            Self::WireError(_) => None,
        }
    }
}
