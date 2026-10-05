//! Multi-envelope delivery of one snapshot within the §26 frame limit.
//!
//! This is the PX-004-G01 protocol extension documented in `protocol/README.md`. The v0.6 design's
//! §18 defines a snapshot as one `base_revision = 0` transaction in *the* transaction envelope; it
//! does not define splitting it. The extension is grounded in what the design does define:
//! §12.1 (atomic transactions; an incomplete transaction is discarded), §18 (the single-envelope
//! snapshot form it splits, and handshake evolution through additive protobuf fields) and §26
//! (maximum frame size, maximum transaction operations).
//!
//! A snapshot is still ONE transaction: `base_revision = 0`, `new_revision = snapshot_revision`,
//! applied wholesale and atomically. What changes is only how its operations reach the replica
//! when the single envelope would exceed the frame limit:
//!
//! - the client advertises how many envelopes it will stage for one snapshot in
//!   `ClientLimits.max_snapshot_parts` (zero/absent = 1, the legacy single-envelope form);
//! - the continuity decision that calls for the snapshot (`SERVER WELCOME` for a catch-up,
//!   `SERVER RESYNC_REQUIRED` for a resync) announces `snapshot_parts`, the number of consecutive
//!   `Transaction` envelopes that carry it (zero/absent = 1);
//! - every envelope carries `base_revision = 0` and `new_revision = snapshot_revision`; the replica
//!   concatenates their operations in arrival order and applies the result once, after the last
//!   envelope, so no partially applied snapshot is ever visible.
//!
//! [`plan_snapshot_frames`] is the server half: it measures the encoded snapshot *before* any frame
//! is written and either returns a plan whose every envelope fits the frame limit, or a
//! [`SnapshotFramingError`] the server reports at handshake. [`SnapshotAssembler`] is the replica
//! half. A snapshot that fits one frame is planned as one envelope and announced as
//! `snapshot_parts = 0`, so its bytes on the wire are unchanged from the single-envelope form.

use std::fmt;
use std::ops::Range;

use prost::encoding::encoded_len_varint;
use prost::Message;

use crate::{Operation, Transaction};

/// Default `ClientLimits.max_snapshot_parts` a reference client advertises (§26).
///
/// Sixteen envelopes of at most `DEFAULT_MAX_FRAME_SIZE` bound the staged wire bytes of one
/// snapshot at 256 MiB; the decoded replica remains bounded by its own store limits.
pub const DEFAULT_MAX_SNAPSHOT_PARTS: u32 = 16;

/// Envelope bound for an advertised `ClientLimits.max_snapshot_parts`: zero/absent means one.
pub fn effective_max_snapshot_parts(advertised: u32) -> u32 {
    advertised.max(1)
}

/// Envelope count for an announced `snapshot_parts`: zero/absent means one.
pub fn effective_snapshot_parts(announced: u32) -> u32 {
    announced.max(1)
}

/// Why a snapshot cannot be delivered within the client's limits (§18, §26).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotFramingError {
    /// One operation is larger than a whole frame, so no split can carry it.
    OperationExceedsFrame {
        operation_index: usize,
        framed_bytes: usize,
        max_frame_size: usize,
    },
    /// Delivering the snapshot needs more envelopes than the client will stage.
    TooManyParts {
        required_parts: usize,
        max_parts: u32,
        snapshot_bytes: usize,
        max_frame_size: usize,
    },
}

impl fmt::Display for SnapshotFramingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OperationExceedsFrame {
                operation_index,
                framed_bytes,
                max_frame_size,
            } => write!(
                f,
                "snapshot operation {operation_index} alone frames to {framed_bytes} bytes, above \
                 the {max_frame_size}-byte frame limit (§26)"
            ),
            Self::TooManyParts {
                required_parts,
                max_parts,
                snapshot_bytes,
                max_frame_size,
            } => write!(
                f,
                "snapshot of {snapshot_bytes} encoded bytes needs {required_parts} envelopes of at \
                 most {max_frame_size} bytes, but the client stages at most {max_parts} \
                 (ClientLimits.max_snapshot_parts, §18, §26)"
            ),
        }
    }
}

impl std::error::Error for SnapshotFramingError {}

/// How one snapshot's operations are divided among consecutive envelopes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotFramePlan {
    parts: Vec<Range<usize>>,
}

impl SnapshotFramePlan {
    /// A plan delivering all `operation_count` operations in one envelope.
    pub fn single(operation_count: usize) -> Self {
        Self {
            parts: std::iter::once(0..operation_count).collect(),
        }
    }

    /// Number of envelopes the plan writes (at least one).
    pub fn part_count(&self) -> u32 {
        u32::try_from(self.parts.len()).expect("part count is bounded by max_snapshot_parts")
    }

    /// Value for `ServerWelcome.snapshot_parts` / `ServerResyncRequired.snapshot_parts`.
    ///
    /// A single envelope is announced as zero so the decision message stays byte-identical to
    /// the legacy form.
    pub fn announced_parts(&self) -> u32 {
        match self.part_count() {
            1 => 0,
            n => n,
        }
    }

    /// Operation index ranges of each envelope, in delivery order.
    pub fn ranges(&self) -> &[Range<usize>] {
        &self.parts
    }

    /// Splits `snapshot` into the planned envelopes, in delivery order.
    ///
    /// A one-envelope plan returns `snapshot` unchanged.
    ///
    /// # Panics
    ///
    /// Panics if `snapshot` is not the transaction this plan was computed for.
    pub fn split(&self, snapshot: Transaction) -> Vec<Transaction> {
        let total = self.parts.last().map_or(0, |range| range.end);
        assert_eq!(
            snapshot.operations.len(),
            total,
            "snapshot frame plan applied to a different transaction"
        );
        if self.parts.len() == 1 {
            return vec![snapshot];
        }
        let Transaction {
            base_revision,
            new_revision,
            priority,
            operations,
        } = snapshot;
        let mut operations = operations.into_iter();
        self.parts
            .iter()
            .map(|range| Transaction {
                base_revision,
                new_revision,
                priority,
                operations: operations.by_ref().take(range.len()).collect(),
            })
            .collect()
    }
}

/// Encoded length of `op` as one `Transaction.operations` entry (key + length + body).
fn operation_field_len(op: &Operation) -> usize {
    let body = op.encoded_len();
    1 + encoded_len_varint(body as u64) + body
}

/// [`crate::framing::framed_payload_len`] of `SruiMessage { transaction }` for a snapshot envelope
/// whose operation fields total `operations_len` bytes.
///
/// Exact, not an estimate: proto3 omits `base_revision = 0` and zero scalars, every remaining field
/// key here is one byte, and the `transaction` oneof member is always written.
fn envelope_len(new_revision: u64, priority: u32, operations_len: usize) -> usize {
    let mut transaction = operations_len;
    if new_revision != 0 {
        transaction += 1 + encoded_len_varint(new_revision);
    }
    if priority != 0 {
        transaction += 1 + encoded_len_varint(u64::from(priority));
    }
    1 + encoded_len_varint(transaction as u64) + transaction
}

/// Measures `snapshot` and plans its delivery within `max_frame_size` per envelope (§18, §26).
///
/// The whole snapshot is one envelope whenever it fits. Otherwise operations are packed greedily,
/// in order, into the fewest envelopes whose framed payload each stays within `max_frame_size`.
///
/// # Errors
///
/// [`SnapshotFramingError::OperationExceedsFrame`] when one operation cannot fit any envelope, and
/// [`SnapshotFramingError::TooManyParts`] when the plan needs more than `max_parts` envelopes
/// (`max_parts` of zero is treated as one).
pub fn plan_snapshot_frames(
    snapshot: &Transaction,
    max_frame_size: usize,
    max_parts: u32,
) -> Result<SnapshotFramePlan, SnapshotFramingError> {
    debug_assert_eq!(snapshot.base_revision, 0, "only a snapshot may be split");
    let max_parts = effective_max_snapshot_parts(max_parts);
    let new_revision = snapshot.new_revision;
    let priority = snapshot.priority;

    let field_lens: Vec<usize> = snapshot
        .operations
        .iter()
        .map(operation_field_len)
        .collect();
    let total: usize = field_lens.iter().sum();
    let snapshot_bytes = envelope_len(new_revision, priority, total);
    if snapshot_bytes <= max_frame_size {
        return Ok(SnapshotFramePlan::single(field_lens.len()));
    }

    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut current = 0usize;
    for (index, &len) in field_lens.iter().enumerate() {
        let alone = envelope_len(new_revision, priority, len);
        if alone > max_frame_size {
            return Err(SnapshotFramingError::OperationExceedsFrame {
                operation_index: index,
                framed_bytes: alone,
                max_frame_size,
            });
        }
        if index > start && envelope_len(new_revision, priority, current + len) > max_frame_size {
            parts.push(start..index);
            start = index;
            current = 0;
        }
        current += len;
    }
    parts.push(start..field_lens.len());

    if parts.len() > max_parts as usize {
        return Err(SnapshotFramingError::TooManyParts {
            required_parts: parts.len(),
            max_parts,
            snapshot_bytes,
            max_frame_size,
        });
    }
    Ok(SnapshotFramePlan { parts })
}

/// Why a replica refused a snapshot envelope sequence (§12.1, §18, §26).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotAssemblyError {
    /// The decision announced more envelopes than this client advertised it would stage.
    PartsExceedLimit { announced: u32, max_parts: u32 },
    /// An envelope is not part of the announced snapshot: wrong base or revision.
    NotASnapshotPart {
        part_index: u32,
        base_revision: u64,
        new_revision: u64,
        snapshot_revision: u64,
    },
    /// A non-final split envelope carried no operations; a conforming plan never sends one.
    EmptyPart { part_index: u32 },
    /// The staged snapshot would exceed `max_transaction_operations`.
    OperationLimitExceeded {
        part_index: u32,
        max_operations: usize,
        staged: usize,
    },
    /// Every announced envelope already arrived.
    AlreadyComplete,
}

impl fmt::Display for SnapshotAssemblyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PartsExceedLimit {
                announced,
                max_parts,
            } => write!(
                f,
                "snapshot announced in {announced} envelopes, above the advertised \
                 max_snapshot_parts {max_parts} (§26)"
            ),
            Self::NotASnapshotPart {
                part_index,
                base_revision,
                new_revision,
                snapshot_revision,
            } => write!(
                f,
                "snapshot envelope {part_index} spans {base_revision} -> {new_revision}, \
                 expected 0 -> {snapshot_revision} (§18)"
            ),
            Self::EmptyPart { part_index } => {
                write!(
                    f,
                    "split snapshot envelope {part_index} carries no operations"
                )
            }
            Self::OperationLimitExceeded {
                part_index,
                max_operations,
                staged,
            } => write!(
                f,
                "snapshot envelope {part_index} brings the staged snapshot to {staged} \
                 operations, above max_transaction_operations {max_operations} (§26)"
            ),
            Self::AlreadyComplete => write!(f, "snapshot already complete"),
        }
    }
}

impl std::error::Error for SnapshotAssemblyError {}

/// Replica-side staging of one announced snapshot (§12.1, §18).
///
/// Envelopes are staged off to the side; [`Self::accept`] returns the reassembled snapshot only
/// when the last announced envelope arrives. Dropping the assembler early (connection loss, a
/// rejected envelope) discards the partial snapshot without touching the replica, exactly as
/// §12.1 requires of an incomplete transaction. After any error the assembler must be discarded.
#[derive(Debug)]
pub struct SnapshotAssembler {
    snapshot_revision: u64,
    expected_parts: u32,
    received_parts: u32,
    max_operations: usize,
    priority: u32,
    operations: Vec<Operation>,
}

impl SnapshotAssembler {
    /// Prepares to stage the snapshot announced by a continuity decision.
    ///
    /// # Errors
    ///
    /// [`SnapshotAssemblyError::PartsExceedLimit`] when `announced_parts` exceeds the client's
    /// advertised `max_parts` (zero for either means one).
    pub fn new(
        snapshot_revision: u64,
        announced_parts: u32,
        max_parts: u32,
        max_operations: usize,
    ) -> Result<Self, SnapshotAssemblyError> {
        let expected_parts = effective_snapshot_parts(announced_parts);
        let max_parts = effective_max_snapshot_parts(max_parts);
        if expected_parts > max_parts {
            return Err(SnapshotAssemblyError::PartsExceedLimit {
                announced: expected_parts,
                max_parts,
            });
        }
        Ok(Self {
            snapshot_revision,
            expected_parts,
            received_parts: 0,
            max_operations,
            priority: 0,
            operations: Vec::new(),
        })
    }

    /// Envelopes the decision announced.
    pub fn expected_parts(&self) -> u32 {
        self.expected_parts
    }

    /// Envelopes staged so far.
    pub fn received_parts(&self) -> u32 {
        self.received_parts
    }

    /// Stages one envelope; returns the whole snapshot once the last one arrives.
    ///
    /// # Errors
    ///
    /// Any [`SnapshotAssemblyError`] other than `PartsExceedLimit`.
    pub fn accept(
        &mut self,
        part: Transaction,
    ) -> Result<Option<Transaction>, SnapshotAssemblyError> {
        if self.received_parts >= self.expected_parts {
            return Err(SnapshotAssemblyError::AlreadyComplete);
        }
        let part_index = self.received_parts;
        if part.base_revision != 0 || part.new_revision != self.snapshot_revision {
            return Err(SnapshotAssemblyError::NotASnapshotPart {
                part_index,
                base_revision: part.base_revision,
                new_revision: part.new_revision,
                snapshot_revision: self.snapshot_revision,
            });
        }
        if self.expected_parts > 1 && part.operations.is_empty() {
            return Err(SnapshotAssemblyError::EmptyPart { part_index });
        }
        let staged = self.operations.len() + part.operations.len();
        if staged > self.max_operations {
            return Err(SnapshotAssemblyError::OperationLimitExceeded {
                part_index,
                max_operations: self.max_operations,
                staged,
            });
        }

        self.received_parts += 1;
        if part_index == 0 {
            self.priority = part.priority;
        }
        if self.expected_parts == 1 {
            return Ok(Some(part));
        }
        self.operations.extend(part.operations);
        if self.received_parts < self.expected_parts {
            return Ok(None);
        }
        Ok(Some(Transaction {
            base_revision: 0,
            new_revision: self.snapshot_revision,
            priority: self.priority,
            operations: std::mem::take(&mut self.operations),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framing::framed_payload_len;
    use crate::{operation, srui_message, CreateNodeOp, NodeRecord, Property, SruiMessage, Value};

    fn text_op(node_id: u64, text_len: usize) -> Operation {
        Operation {
            op: Some(operation::Op::CreateNode(CreateNodeOp {
                node: Some(NodeRecord {
                    node_id,
                    r#type: Some(crate::TypeRef {
                        namespace_id: 0,
                        local_id: 9,
                    }),
                    parent_id: 1,
                    child_index: 0,
                    properties: vec![Property {
                        property: Some(crate::PropertyRef {
                            namespace_id: 0,
                            local_id: 12,
                        }),
                        value: Some(Value {
                            value: Some(crate::value::Value::StringValue("x".repeat(text_len))),
                        }),
                    }],
                }),
            })),
        }
    }

    fn snapshot(revision: u64, ops: Vec<Operation>) -> Transaction {
        Transaction {
            base_revision: 0,
            new_revision: revision,
            priority: 0,
            operations: ops,
        }
    }

    fn framed(tx: &Transaction) -> usize {
        framed_payload_len(&SruiMessage {
            msg: Some(srui_message::Msg::Transaction(tx.clone())),
        })
    }

    #[test]
    fn envelope_len_is_exactly_framed_payload_len() {
        for revision in [1u64, 127, 128, 1 << 40] {
            for priority in [0u32, 1, 300] {
                for count in [0usize, 1, 7] {
                    let mut tx = snapshot(
                        revision,
                        (0..count as u64).map(|i| text_op(i + 2, 200)).collect(),
                    );
                    tx.priority = priority;
                    let ops: usize = tx.operations.iter().map(operation_field_len).sum();
                    assert_eq!(envelope_len(revision, priority, ops), framed(&tx));
                }
            }
        }
    }

    #[test]
    fn fitting_snapshot_is_one_unannounced_envelope_with_unchanged_bytes() {
        let tx = snapshot(5, vec![text_op(2, 10), text_op(3, 10)]);
        let plan = plan_snapshot_frames(&tx, 4096, 0).expect("fits");
        assert_eq!(plan.part_count(), 1);
        assert_eq!(plan.announced_parts(), 0);
        assert_eq!(plan.split(tx.clone()), vec![tx]);
    }

    #[test]
    fn oversized_snapshot_splits_under_the_limit_and_reassembles_exactly() {
        let ops: Vec<_> = (0..40).map(|i| text_op(i + 2, 1000)).collect();
        let tx = snapshot(9, ops);
        let limit = 8 * 1024;
        assert!(framed(&tx) > limit);
        let plan = plan_snapshot_frames(&tx, limit, 16).expect("plannable");
        assert!(plan.part_count() > 1);
        assert_eq!(plan.announced_parts(), plan.part_count());

        let parts = plan.split(tx.clone());
        assert_eq!(parts.len() as u32, plan.part_count());
        let mut assembler = SnapshotAssembler::new(9, plan.announced_parts(), 16, 10_000).unwrap();
        let mut assembled = None;
        for (i, part) in parts.into_iter().enumerate() {
            assert!(framed(&part) <= limit, "part {i} framed past the limit");
            assert!(assembled.is_none(), "assembled before the last part");
            assembled = assembler.accept(part).expect("accept");
        }
        assert_eq!(assembled, Some(tx));
    }

    #[test]
    fn size_bound_refuses_more_parts_than_the_client_stages() {
        let tx = snapshot(9, (0..40).map(|i| text_op(i + 2, 1000)).collect());
        match plan_snapshot_frames(&tx, 8 * 1024, 2) {
            Err(SnapshotFramingError::TooManyParts {
                required_parts,
                max_parts: 2,
                snapshot_bytes,
                max_frame_size: 8192,
            }) => {
                assert!(required_parts > 2);
                assert_eq!(snapshot_bytes, framed(&tx));
            }
            other => panic!("expected TooManyParts, got {other:?}"),
        }
        // A legacy client (zero = one envelope) is refused rather than sent a split it cannot read.
        assert!(matches!(
            plan_snapshot_frames(&tx, 8 * 1024, 0),
            Err(SnapshotFramingError::TooManyParts { max_parts: 1, .. })
        ));
    }

    #[test]
    fn an_operation_larger_than_a_frame_is_refused() {
        let tx = snapshot(3, vec![text_op(2, 10), text_op(3, 10_000)]);
        assert!(matches!(
            plan_snapshot_frames(&tx, 4096, 16),
            Err(SnapshotFramingError::OperationExceedsFrame {
                operation_index: 1,
                max_frame_size: 4096,
                ..
            })
        ));
    }

    #[test]
    fn assembler_refuses_out_of_bound_and_foreign_envelopes() {
        assert_eq!(
            SnapshotAssembler::new(4, 3, 2, 100).unwrap_err(),
            SnapshotAssemblyError::PartsExceedLimit {
                announced: 3,
                max_parts: 2
            }
        );

        let mut assembler = SnapshotAssembler::new(4, 2, 4, 100).unwrap();
        let mut live = snapshot(4, vec![text_op(2, 1)]);
        live.base_revision = 3;
        assert!(matches!(
            assembler.accept(live),
            Err(SnapshotAssemblyError::NotASnapshotPart { part_index: 0, .. })
        ));

        let mut assembler = SnapshotAssembler::new(4, 2, 4, 100).unwrap();
        assert_eq!(
            assembler.accept(snapshot(4, vec![])),
            Err(SnapshotAssemblyError::EmptyPart { part_index: 0 })
        );

        let mut assembler = SnapshotAssembler::new(4, 2, 4, 2).unwrap();
        assert_eq!(assembler.accept(snapshot(4, vec![text_op(2, 1)])), Ok(None));
        assert_eq!(
            assembler.accept(snapshot(4, vec![text_op(3, 1), text_op(4, 1)])),
            Err(SnapshotAssemblyError::OperationLimitExceeded {
                part_index: 1,
                max_operations: 2,
                staged: 3
            })
        );

        let mut assembler = SnapshotAssembler::new(4, 0, 0, 100).unwrap();
        assert!(assembler.accept(snapshot(4, vec![])).unwrap().is_some());
        assert_eq!(
            assembler.accept(snapshot(4, vec![])),
            Err(SnapshotAssemblyError::AlreadyComplete)
        );
    }
}
