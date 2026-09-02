//! Transaction execution engine on SemanticStore (§12.1, §26).

use super::error::TxnError;
use super::operation::{Operation, Revision};
use super::record::Transaction;
use crate::store::SemanticStore;

impl SemanticStore {
    fn apply_staged(
        &mut self,
        ops: &[Operation],
        new_revision: Revision,
    ) -> Result<Revision, TxnError> {
        let max_ops = self.limits().max_transaction_operations;
        if ops.len() > max_ops {
            return Err(TxnError::MaxOperationsExceeded {
                limit: max_ops,
                actual: ops.len(),
            });
        }

        let mut staged = self.clone_staging();
        for (idx, op) in ops.iter().enumerate() {
            if let Err(source) = op.apply(&mut staged) {
                return Err(TxnError::OpFailed {
                    op_index: idx,
                    source,
                });
            }
        }

        self.commit_staging(staged, new_revision);
        Ok(new_revision)
    }

    fn apply_staged_owned(
        &mut self,
        ops: Vec<Operation>,
        new_revision: Revision,
    ) -> Result<Revision, TxnError> {
        let max_ops = self.limits().max_transaction_operations;
        if ops.len() > max_ops {
            return Err(TxnError::MaxOperationsExceeded {
                limit: max_ops,
                actual: ops.len(),
            });
        }

        let mut staged = self.clone_staging();
        for (idx, op) in ops.into_iter().enumerate() {
            if let Err(source) = op.apply_owned(&mut staged) {
                return Err(TxnError::OpFailed {
                    op_index: idx,
                    source,
                });
            }
        }

        self.commit_staging(staged, new_revision);
        Ok(new_revision)
    }

    /// Applies a sequence of mutation operations as an atomic transaction advancing from `base_revision` to `base_revision + 1` (§12.1).
    ///
    /// Semantics required by §12.1 and §26:
    /// 1. Rejects if `base_revision` does not match the store's current committed revision (`TxnError::StaleBaseRevision`).
    /// 2. Enforces the §26 `max_transaction_operations` limit as a pre-check before applying any operations (`TxnError::MaxOperationsExceeded`).
    /// 3. Applies all operations speculatively to a private staging copy of the store.
    /// 4. If any operation fails, the entire transaction is discarded with zero visible side-effects or partial changes on the store (`TxnError::OpFailed`).
    /// 5. On full success, atomically commits the staged mutations and advances the store's revision to `new_revision = base_revision + 1`.
    ///
    /// Note (§12.2): Commits establish state-consistency boundaries, not render frames or pacing cues.
    ///
    /// # Performance & Staging Complexity
    ///
    /// Speculative execution is currently implemented via [`SemanticStore::clone_staging`], which
    /// deep-clones the authoritative node graph and model caches ($O(\text{store size})$ per transaction).
    /// Before production integration at large scale (`DEFAULT_MAX_NODE_COUNT = 100_000`), this will be
    /// migrated to structural Copy-on-Write (`im::HashMap`) or in-place transaction execution with
    /// an undo-journal rollback log.
    pub fn apply_transaction(
        &mut self,
        base_revision: impl Into<Revision>,
        ops: Vec<Operation>,
    ) -> Result<Revision, TxnError> {
        let base_rev = base_revision.into();
        let current_rev = self.revision();
        if base_rev != current_rev {
            return Err(TxnError::StaleBaseRevision {
                expected: current_rev,
                actual: base_rev,
            });
        }

        self.apply_staged_owned(ops, base_rev.next())
    }

    /// Applies a structured [`Transaction`] record, validating base revision, forward revision advancement,
    /// and operational limits (§12.1, §20.2).
    ///
    /// Single-step increments (`new_revision == base_revision + 1`) accept arbitrary operations.
    /// Multi-revision forward spans (`new_revision > base_revision + 1`) are legal exclusively for
    /// coalesced scalar updates (`txn.is_coalesceable()`).
    pub fn apply_transaction_record(&mut self, txn: &Transaction) -> Result<Revision, TxnError> {
        let current_rev = self.revision();
        if txn.base_revision != current_rev {
            return Err(TxnError::StaleBaseRevision {
                expected: current_rev,
                actual: txn.base_revision,
            });
        }

        if txn.new_revision <= txn.base_revision {
            return Err(TxnError::InvalidNewRevision {
                expected: txn.base_revision.next(),
                actual: txn.new_revision,
            });
        }

        if txn.new_revision > txn.base_revision.next() && !txn.is_coalesceable() {
            return Err(TxnError::InvalidNewRevision {
                expected: txn.base_revision.next(),
                actual: txn.new_revision,
            });
        }

        self.apply_staged(&txn.operations, txn.new_revision)
    }

    /// Decodes and applies a protobuf wire `srui_protocol::Transaction` atomically (§12.1, §16, §20.2).
    pub fn apply_wire_transaction(
        &mut self,
        wire_txn: srui_protocol::Transaction,
    ) -> Result<Revision, TxnError> {
        let txn = Transaction::try_from(wire_txn)?;
        let current_rev = self.revision();
        if txn.base_revision != current_rev {
            return Err(TxnError::StaleBaseRevision {
                expected: current_rev,
                actual: txn.base_revision,
            });
        }

        if txn.new_revision <= txn.base_revision {
            return Err(TxnError::InvalidNewRevision {
                expected: txn.base_revision.next(),
                actual: txn.new_revision,
            });
        }

        if txn.new_revision > txn.base_revision.next() && !txn.is_coalesceable() {
            return Err(TxnError::InvalidNewRevision {
                expected: txn.base_revision.next(),
                actual: txn.new_revision,
            });
        }

        self.apply_staged_owned(txn.operations, txn.new_revision)
    }
}
