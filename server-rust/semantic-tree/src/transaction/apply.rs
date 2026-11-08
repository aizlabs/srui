//! Transaction execution engine on SemanticStore (§12.1, §18, §20.4, §26).
//!
//! Three delivery forms, three entry points (§12.1):
//!
//! - [`SemanticStore::apply_transaction`], [`SemanticStore::apply_transaction_record`] and
//!   [`SemanticStore::apply_wire_transaction`] apply an **authoritative commit** and accept only
//!   `new_revision == base_revision + 1`;
//! - [`SemanticStore::apply_delivered_transaction`] and [`SemanticStore::apply_coalesced_delta`]
//!   apply what a **replica** may receive on the live stream, which additionally includes a
//!   coalesced scalar span (§20.4);
//! - [`SemanticStore::replace_from_snapshot`] replaces the whole replica from a resync snapshot
//!   (§18).
//!
//! Only the authoritative form is journalable, and it is the only one a session may commit. The
//! split exists so that a transport-derived revision span cannot reach an authoritative path by
//! reusing the same method.
//!
//! Every form checks each touched collection model's hierarchical shape (§8, §26) right after the
//! last operation in the transaction that touches it ([`ModelTreeCheckpoints`]), so an edit inside
//! a subtree is judged as a whole and a failure is reported at that operation's index.

use std::collections::HashMap;

use super::error::TxnError;
use super::kinds::{
    AuthoritativeCommit, CoalescedScalarDelta, DeliveredTransaction, ResyncSnapshot,
};
use super::operation::{Operation, Revision};
use super::record::Transaction;
use crate::ids::ModelId;
use crate::store::SemanticStore;

/// Maps the index of the last operation touching each collection model to that model (§8, §12.1).
///
/// A model's `tree_depth`/`descendant_count` shape can only be judged once the transaction has
/// finished editing it: inserting a child needs the `MODEL_INSERT` and the ancestors'
/// `MODEL_UPDATE` together. Checking after that model's last operation, rather than once after
/// all operations, keeps a deterministic `op_index` that the Swift replica reports identically.
struct ModelTreeCheckpoints(HashMap<usize, ModelId>);

impl ModelTreeCheckpoints {
    fn new<'a>(ops: impl IntoIterator<Item = &'a Operation>) -> Self {
        let mut last: HashMap<ModelId, usize> = HashMap::new();
        for (idx, op) in ops.into_iter().enumerate() {
            if let Some(model) = op.model_id() {
                last.insert(model, idx);
            }
        }
        Self(last.into_iter().map(|(model, idx)| (idx, model)).collect())
    }

    /// Validates the model whose last touch is operation `idx`, if any.
    fn check(&self, staged: &SemanticStore, idx: usize) -> Result<(), TxnError> {
        match self.0.get(&idx) {
            Some(&model) => {
                staged
                    .validate_model_tree(model)
                    .map_err(|source| TxnError::OpFailed {
                        op_index: idx,
                        source,
                    })
            }
            None => Ok(()),
        }
    }
}

/// A transaction applied to a private staging copy, ready for an infallible commit (§12.1).
///
/// Produced by [`SemanticStore::prepare_commit`] and consumed by
/// [`SemanticStore::commit_prepared`]. Splitting the fallible work from the mutation lets a caller
/// that must also update a second structure — a journal, say — validate everything first and then
/// perform both mutations with no failure path in between.
#[derive(Debug)]
pub struct StagedCommit {
    staged: SemanticStore,
    base_revision: Revision,
    new_revision: Revision,
}

impl StagedCommit {
    /// The revision this staged commit was prepared against.
    #[must_use]
    pub fn base_revision(&self) -> Revision {
        self.base_revision
    }

    /// The revision this staged commit produces.
    #[must_use]
    pub fn new_revision(&self) -> Revision {
        self.new_revision
    }

    /// The store this commit would produce, for authoritative post-conditions checked before it
    /// is committed (e.g. promoted selections, §6.3).
    #[must_use]
    pub fn store(&self) -> &SemanticStore {
        &self.staged
    }
}

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

        let checkpoints = ModelTreeCheckpoints::new(ops);
        let mut staged = self.clone_staging();
        for (idx, op) in ops.iter().enumerate() {
            if let Err(source) = op.apply(&mut staged) {
                return Err(TxnError::OpFailed {
                    op_index: idx,
                    source,
                });
            }
            checkpoints.check(&staged, idx)?;
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

        let checkpoints = ModelTreeCheckpoints::new(&ops);
        let mut staged = self.clone_staging();
        for (idx, op) in ops.into_iter().enumerate() {
            if let Err(source) = op.apply_owned(&mut staged) {
                return Err(TxnError::OpFailed {
                    op_index: idx,
                    source,
                });
            }
            checkpoints.check(&staged, idx)?;
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
        // A replica's revision can be set from a snapshot, so the store may legitimately sit at a
        // revision with no successor; refuse rather than wrap (§12.1).
        let new_rev = base_rev
            .checked_next()
            .ok_or(TxnError::RevisionExhausted { base: base_rev })?;

        self.apply_staged_owned(ops, new_rev)
    }

    /// Checks that `base_revision` is the store's committed revision (§12.1).
    fn check_base_revision(&self, base_revision: Revision) -> Result<(), TxnError> {
        let current_rev = self.revision();
        if base_revision != current_rev {
            return Err(TxnError::StaleBaseRevision {
                expected: current_rev,
                actual: base_revision,
            });
        }
        Ok(())
    }

    /// Applies a structured [`Transaction`] record as an authoritative commit (§12.1).
    ///
    /// Accepts only `new_revision == base_revision + 1`. A coalesced delivery span belongs to
    /// [`Self::apply_coalesced_delta`] and a resync snapshot to [`Self::replace_from_snapshot`];
    /// neither may advance authoritative state through this method.
    pub fn apply_transaction_record(&mut self, txn: &Transaction) -> Result<Revision, TxnError> {
        AuthoritativeCommit::validate(txn)?;
        self.check_base_revision(txn.base_revision)?;
        self.apply_staged(&txn.operations, txn.new_revision)
    }

    /// Decodes and applies a protobuf wire `srui_protocol::Transaction` as an authoritative commit
    /// (§12.1, §16).
    pub fn apply_wire_transaction(
        &mut self,
        wire_txn: srui_protocol::Transaction,
    ) -> Result<Revision, TxnError> {
        let txn = Transaction::try_from(wire_txn)?;
        AuthoritativeCommit::validate(&txn)?;
        self.check_base_revision(txn.base_revision)?;
        self.apply_staged_owned(txn.operations, txn.new_revision)
    }

    /// Applies a validated authoritative commit (§12.1).
    pub fn apply_authoritative(
        &mut self,
        commit: &AuthoritativeCommit,
    ) -> Result<Revision, TxnError> {
        let txn = commit.as_transaction();
        self.check_base_revision(txn.base_revision)?;
        self.apply_staged(&txn.operations, txn.new_revision)
    }

    /// Applies a coalesced scalar delta to a **replica**, advancing several revisions at once
    /// (§12.1, §20.4).
    ///
    /// The delta is derived from transactions the session already committed, so applying it must
    /// leave the replica in the state those transactions produced, at `delta.new_revision()`. It is
    /// never journalable and must never be applied to authoritative state.
    pub fn apply_coalesced_delta(
        &mut self,
        delta: &CoalescedScalarDelta,
    ) -> Result<Revision, TxnError> {
        let txn = delta.as_transaction();
        self.check_base_revision(txn.base_revision)?;
        self.apply_staged(&txn.operations, txn.new_revision)
    }

    /// Applies one frame of the live stream to a **replica** (§12.1, §20.4).
    ///
    /// This is the single entry point a replica uses for streamed transactions: it classifies the
    /// frame as an authoritative commit or a coalesced scalar delta *by shape* and dispatches.
    /// Both are legal for a replica and neither is journalable, which is why they share an entry
    /// point — and why an authoritative session must not use this one. A span that is neither form
    /// is rejected rather than guessed at.
    ///
    /// A resync snapshot is deliberately **not** accepted here: its shape is indistinguishable from
    /// a replayed first transaction, so it may only be applied from explicit protocol context via
    /// [`Self::replace_from_snapshot`] (§18).
    pub fn apply_delivered_transaction(
        &mut self,
        wire_txn: srui_protocol::Transaction,
    ) -> Result<Revision, TxnError> {
        match DeliveredTransaction::try_from(wire_txn)? {
            DeliveredTransaction::Commit(commit) => self.apply_authoritative(&commit),
            DeliveredTransaction::Delta(delta) => self.apply_coalesced_delta(&delta),
        }
    }

    /// Replaces the entire replica from a resync snapshot (§18, §26).
    ///
    /// The snapshot reconstructs the tree at its declared revision rather than describing a
    /// difference against current state, so it is applied to a fresh store and swapped in whole.
    /// The caller must have an outstanding resync/catch-up decision: this method trusts that
    /// context and does not — cannot — infer it from the transaction (§18).
    ///
    /// Revisions stay monotonic: a snapshot may restate the revision already held but never regress
    /// it. The §26 `max_transaction_operations` bound is enforced here as a pre-check, exactly as
    /// for a commit.
    pub fn replace_from_snapshot(
        &mut self,
        snapshot: &ResyncSnapshot,
    ) -> Result<Revision, TxnError> {
        let txn = snapshot.as_transaction();
        let current_rev = self.revision();
        if txn.new_revision < current_rev {
            return Err(TxnError::InvalidNewRevision {
                expected: current_rev,
                actual: txn.new_revision,
            });
        }

        let max_ops = self.limits().max_transaction_operations;
        if txn.operations.len() > max_ops {
            return Err(TxnError::MaxOperationsExceeded {
                limit: max_ops,
                actual: txn.operations.len(),
            });
        }

        let limits = self.limits().clone();
        let checkpoints = ModelTreeCheckpoints::new(&txn.operations);
        let mut staged = SemanticStore::with_limits_and_revision(limits.clone(), Revision::INITIAL);
        for (idx, op) in txn.operations.iter().enumerate() {
            op.apply(&mut staged).map_err(|source| TxnError::OpFailed {
                op_index: idx,
                source,
            })?;
            checkpoints.check(&staged, idx)?;
        }

        // Swapped in whole rather than merged: a snapshot also resets identity bookkeeping to the
        // tree it declares, which `commit_staging` on the live store would not do (§6.2).
        let mut replaced = SemanticStore::with_limits_and_revision(limits, Revision::INITIAL);
        replaced.commit_staging(staged, txn.new_revision);
        *self = replaced;
        Ok(txn.new_revision)
    }

    /// Applies an authoritative commit to a private staging copy without mutating the store
    /// (§12.1).
    ///
    /// Every failure a commit can produce — stale base revision, §26 operation limit, a failing
    /// operation — surfaces here, so [`Self::commit_prepared`] cannot fail. A caller that must keep
    /// a second structure in lockstep with the store validates both first and then performs both
    /// mutations with no failure path in between.
    pub fn prepare_commit(&self, commit: &AuthoritativeCommit) -> Result<StagedCommit, TxnError> {
        let txn = commit.as_transaction();
        self.check_base_revision(txn.base_revision)?;

        let max_ops = self.limits().max_transaction_operations;
        if txn.operations.len() > max_ops {
            return Err(TxnError::MaxOperationsExceeded {
                limit: max_ops,
                actual: txn.operations.len(),
            });
        }

        let checkpoints = ModelTreeCheckpoints::new(&txn.operations);
        let mut staged = self.clone_staging();
        for (idx, op) in txn.operations.iter().enumerate() {
            if let Err(source) = op.apply(&mut staged) {
                return Err(TxnError::OpFailed {
                    op_index: idx,
                    source,
                });
            }
            checkpoints.check(&staged, idx)?;
        }

        Ok(StagedCommit {
            staged,
            base_revision: txn.base_revision,
            new_revision: txn.new_revision,
        })
    }

    /// Commits a [`StagedCommit`] prepared from this store (§12.1).
    ///
    /// Infallible by construction. `staged` must come from [`Self::prepare_commit`] on this store
    /// while it was still at `staged.base_revision()`; committing one prepared against a different
    /// revision would publish state built on a base the store has already left.
    pub fn commit_prepared(&mut self, staged: StagedCommit) {
        debug_assert_eq!(
            self.revision(),
            staged.base_revision,
            "staged commit was prepared against a different revision"
        );
        self.commit_staging(staged.staged, staged.new_revision);
    }
}
