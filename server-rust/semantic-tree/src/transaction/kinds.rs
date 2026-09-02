//! Validated transaction delivery forms (§12.1, §18, §20.4).
//!
//! One wire envelope carries three distinct forms, and the form decides which revision spans are
//! legal, who may produce it, and who may accept it. Each form is a newtype whose only constructors
//! validate the shape invariant, so a value of the type cannot exist without it:
//!
//! | Form | Span | Operations | Journalled |
//! |---|---|---|---|
//! | [`AuthoritativeCommit`] | `base -> base + 1` | any valid operations | yes |
//! | [`CoalescedScalarDelta`] | `base -> M`, `M > base` | scalar `SetProperty` only, non-empty | never |
//! | [`ResyncSnapshot`] | `0 -> N` | full-tree reconstruction | never |
//!
//! The invariants here are *shape* invariants, which is all a value can carry on its own. The
//! state-relative preconditions — does this base revision match the store, does this base revision
//! continue the journal — belong to the store and the journal and are checked there.

use super::error::TxnError;
use super::operation::{Operation, Revision};
use super::record::Transaction;

/// A transaction that advances authoritative state by exactly one revision (§12.1).
///
/// This is the only form that may enter a session's store *and* journal. Constructing one is the
/// single point at which `new_revision == base_revision + 1` is enforced, so a journal cannot be
/// handed a coalesced span even by mistake.
#[derive(Debug, Clone, PartialEq)]
pub struct AuthoritativeCommit(Transaction);

impl AuthoritativeCommit {
    /// Builds a commit advancing `base_revision -> base_revision + 1`.
    ///
    /// Infallible: the span is constructed rather than checked, for callers such as a session
    /// applying its own staged operations.
    pub fn new(base_revision: Revision, operations: impl IntoIterator<Item = Operation>) -> Self {
        Self(Transaction::new(base_revision, operations))
    }

    /// The revision this commit applies to.
    #[must_use]
    pub fn base_revision(&self) -> Revision {
        self.0.base_revision
    }

    /// The revision this commit produces, always `base_revision + 1`.
    #[must_use]
    pub fn new_revision(&self) -> Revision {
        self.0.new_revision
    }

    /// Borrows the validated transaction.
    #[must_use]
    pub fn as_transaction(&self) -> &Transaction {
        &self.0
    }

    /// Consumes the wrapper, yielding the validated transaction.
    #[must_use]
    pub fn into_transaction(self) -> Transaction {
        self.0
    }
}

impl AuthoritativeCommit {
    /// Checks the single-step span invariant without consuming or cloning the transaction.
    ///
    /// Shares one implementation with [`TryFrom<Transaction>`] so a store applying a borrowed
    /// record and a journal accepting an owned commit cannot drift apart.
    pub fn validate(txn: &Transaction) -> Result<(), TxnError> {
        let expected = txn.base_revision.next();
        if txn.new_revision != expected {
            return Err(TxnError::InvalidNewRevision {
                expected,
                actual: txn.new_revision,
            });
        }
        Ok(())
    }
}

impl TryFrom<Transaction> for AuthoritativeCommit {
    type Error = TxnError;

    fn try_from(txn: Transaction) -> Result<Self, Self::Error> {
        Self::validate(&txn)?;
        Ok(Self(txn))
    }
}

impl TryFrom<srui_protocol::Transaction> for AuthoritativeCommit {
    type Error = TxnError;

    fn try_from(wire: srui_protocol::Transaction) -> Result<Self, Self::Error> {
        Self::try_from(Transaction::try_from(wire)?)
    }
}

/// A run of committed scalar updates collapsed into one delivery envelope (§12.1, §20.2, §20.4).
///
/// Spans `base -> M` with `M > base`, carrying only the latest value of each `(node, property)`
/// pair. It is *derived* from transactions the session already committed: it never advances
/// authoritative state, is produced only by an outbound delivery queue, and is accepted only by a
/// replica. It must never reach a journal.
#[derive(Debug, Clone, PartialEq)]
pub struct CoalescedScalarDelta(Transaction);

impl CoalescedScalarDelta {
    /// The revision this delta applies to.
    #[must_use]
    pub fn base_revision(&self) -> Revision {
        self.0.base_revision
    }

    /// The revision this delta produces, strictly greater than `base_revision`.
    #[must_use]
    pub fn new_revision(&self) -> Revision {
        self.0.new_revision
    }

    /// Borrows the validated transaction.
    #[must_use]
    pub fn as_transaction(&self) -> &Transaction {
        &self.0
    }

    /// Consumes the wrapper, yielding the validated transaction.
    #[must_use]
    pub fn into_transaction(self) -> Transaction {
        self.0
    }
}

impl CoalescedScalarDelta {
    /// Checks the delta shape invariant without consuming or cloning the transaction.
    pub fn validate(txn: &Transaction) -> Result<(), TxnError> {
        // A delta must move forward, and only scalar SetProperty operations may be collapsed:
        // any structural operation is a barrier that must keep its own revision boundary (§20.4).
        if txn.new_revision <= txn.base_revision || !txn.is_coalesceable() {
            return Err(TxnError::InvalidNewRevision {
                expected: txn.base_revision.next(),
                actual: txn.new_revision,
            });
        }
        Ok(())
    }
}

impl TryFrom<Transaction> for CoalescedScalarDelta {
    type Error = TxnError;

    fn try_from(txn: Transaction) -> Result<Self, Self::Error> {
        Self::validate(&txn)?;
        Ok(Self(txn))
    }
}

impl TryFrom<srui_protocol::Transaction> for CoalescedScalarDelta {
    type Error = TxnError;

    fn try_from(wire: srui_protocol::Transaction) -> Result<Self, Self::Error> {
        Self::try_from(Transaction::try_from(wire)?)
    }
}

/// A full-tree reconstruction replacing a replica's state at a declared revision (§18).
///
/// Carries `base_revision == 0` and `new_revision == <snapshot revision>`. Accepting one is a
/// decision made from protocol context — an outstanding `RESYNC_REQUIRED`, or the catch-up that
/// follows `SERVER WELCOME` — never from the shape alone: a replayed first transaction has the same
/// shape and would otherwise wipe a live replica (§18).
#[derive(Debug, Clone, PartialEq)]
pub struct ResyncSnapshot(Transaction);

impl ResyncSnapshot {
    /// The revision the reconstructed tree represents.
    #[must_use]
    pub fn new_revision(&self) -> Revision {
        self.0.new_revision
    }

    /// Borrows the validated transaction.
    #[must_use]
    pub fn as_transaction(&self) -> &Transaction {
        &self.0
    }

    /// Consumes the wrapper, yielding the validated transaction.
    #[must_use]
    pub fn into_transaction(self) -> Transaction {
        self.0
    }
}

impl TryFrom<Transaction> for ResyncSnapshot {
    type Error = TxnError;

    fn try_from(txn: Transaction) -> Result<Self, Self::Error> {
        if txn.base_revision != Revision::INITIAL {
            return Err(TxnError::StaleBaseRevision {
                expected: Revision::INITIAL,
                actual: txn.base_revision,
            });
        }
        Ok(Self(txn))
    }
}

impl TryFrom<srui_protocol::Transaction> for ResyncSnapshot {
    type Error = TxnError;

    fn try_from(wire: srui_protocol::Transaction) -> Result<Self, Self::Error> {
        Self::try_from(Transaction::try_from(wire)?)
    }
}

/// A transaction a replica may accept on the live stream (§12.1).
///
/// Both forms are legal for a replica and neither is journalable, which is exactly why they share
/// one type: this is the boundary where "which form is this?" is answered from the shape, and the
/// answer cannot leak into an authoritative path because [`AuthoritativeCommit`] is the only thing
/// a store commit or journal append accepts.
#[derive(Debug, Clone, PartialEq)]
pub enum DeliveredTransaction {
    /// A single-step commit, identical to what the authoritative store applied.
    Commit(AuthoritativeCommit),
    /// A coalesced run of scalar updates spanning several committed revisions.
    Delta(CoalescedScalarDelta),
}

impl DeliveredTransaction {
    /// The revision this transaction applies to.
    #[must_use]
    pub fn base_revision(&self) -> Revision {
        match self {
            Self::Commit(commit) => commit.base_revision(),
            Self::Delta(delta) => delta.base_revision(),
        }
    }

    /// The revision this transaction produces.
    #[must_use]
    pub fn new_revision(&self) -> Revision {
        match self {
            Self::Commit(commit) => commit.new_revision(),
            Self::Delta(delta) => delta.new_revision(),
        }
    }

    /// Borrows the validated transaction.
    #[must_use]
    pub fn as_transaction(&self) -> &Transaction {
        match self {
            Self::Commit(commit) => commit.as_transaction(),
            Self::Delta(delta) => delta.as_transaction(),
        }
    }
}

impl TryFrom<Transaction> for DeliveredTransaction {
    type Error = TxnError;

    fn try_from(txn: Transaction) -> Result<Self, Self::Error> {
        if txn.new_revision == txn.base_revision.next() {
            return Ok(Self::Commit(AuthoritativeCommit(txn)));
        }
        // Anything else is only deliverable as a coalesced delta; a span this rejects is a span
        // no replica may apply (§12.1).
        CoalescedScalarDelta::try_from(txn).map(Self::Delta)
    }
}

impl TryFrom<srui_protocol::Transaction> for DeliveredTransaction {
    type Error = TxnError;

    fn try_from(wire: srui_protocol::Transaction) -> Result<Self, Self::Error> {
        Self::try_from(Transaction::try_from(wire)?)
    }
}
