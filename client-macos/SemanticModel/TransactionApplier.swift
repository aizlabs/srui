//
// TransactionApplier.swift
// SemanticModel
//
// Atomic transaction execution engine and monotonic revision tracking (§12, §12.1, §12.2, §18, §26).
// Platform-neutral: NEVER import AppKit or Cocoa in this file.
//
// Architectural Invariants (§4):
// - §12 Persistent Object Graph: The semantic UI graph persists across transactions; mutations
//   are incremental rather than full-document resends.
// - §12.1 Revisions and Transactions: Transactions are all-or-nothing atomic units advancing
//   monotonically from base_revision to new_revision = base_revision + 1. If any operation
//   within a transaction fails, the entire transaction is discarded and the store is left in its
//   exact pre-transaction state without observable side effects.
// - §12.2 Commits Are Not Frames: Commits establish semantic state-consistency boundaries, not
//   display render cues or repaint pacing. The client/renderer independently schedules drawing.
// - §18 Reconnect & Tracking: The client tracks `lastAppliedRevision` across applied commits.
// - §26 Operational Limits: Transactions pre-check and enforce safety limits, including
//   max_transaction_operations, prior to applying mutations.
//

import Foundation

// MARK: - Revision (§12.1)

/// Monotonically increasing committed semantic state revision (§12.1).
///
/// A revision counter is scoped to a session and never decreases or repeats.
public struct Revision: Hashable, Equatable, Comparable, Sendable, CustomStringConvertible, ExpressibleByIntegerLiteral {
    /// Initial session revision (0).
    public static let initial = Revision(0)
    public static let INITIAL = Revision(0)

    /// Raw 64-bit unsigned revision counter.
    public let value: UInt64

    public init(_ value: UInt64) {
        self.value = value
    }

    public init(integerLiteral value: UInt64) {
        self.value = value
    }

    /// Returns the next monotonically increasing revision (`self.value + 1`).
    public var next: Revision {
        Revision(value + 1)
    }

    public var description: String {
        "Revision(\(value))"
    }

    public static func < (lhs: Revision, rhs: Revision) -> Bool {
        lhs.value < rhs.value
    }
}

// MARK: - Transaction Error (§12.1, §26)

/// Errors returned when validating or applying a semantic transaction (§12.1, §26).
public enum TxnError: Error, Equatable, Sendable, CustomStringConvertible {
    /// Base revision does not match the store's current committed revision.
    case staleBaseRevision(expected: Revision, actual: Revision)
    /// New revision is not strictly monotonic (`expected != actual`).
    case invalidNewRevision(expected: Revision, actual: Revision)
    /// Transaction exceeds the configured maximum operations limit (§26).
    case maxOperationsExceeded(limit: Int, actual: Int)
    /// An operation within the transaction failed during application.
    case opFailed(opIndex: Int, source: StoreError)
    /// Wire transaction payload decoding or conversion failed.
    case wireError(String)

    public var description: String {
        switch self {
        case .staleBaseRevision(let expected, let actual):
            return "stale base revision: store committed revision is \(expected), transaction base is \(actual)"
        case .invalidNewRevision(let expected, let actual):
            return "invalid new revision: expected \(expected) (base + 1), but got \(actual)"
        case .maxOperationsExceeded(let limit, let actual):
            return "transaction operations limit exceeded: \(actual) ops exceeds max limit of \(limit) (§26)"
        case .opFailed(let opIndex, let source):
            return "operation at index \(opIndex) failed: \(source)"
        case .wireError(let msg):
            return "wire transaction error: \(msg)"
        }
    }

    /// Returns the canonical conformance error code for this transaction error, if applicable (§32).
    public var conformanceCode: String? {
        switch self {
        case .staleBaseRevision: return "stale_base_revision"
        case .invalidNewRevision: return "invalid_new_revision"
        case .maxOperationsExceeded: return "max_operations_exceeded"
        case .opFailed(_, let source): return source.conformanceCode
        case .wireError: return nil
        }
    }
}

// MARK: - Transaction Record (§12.1, §16)

/// An atomic transaction envelope advancing the store from `baseRevision` to `newRevision` (§12.1, §16).
public struct Transaction: Equatable, Sendable {
    /// Committed revision on which this transaction is based.
    public var baseRevision: Revision
    /// Target revision produced upon successful commit (`baseRevision + 1`).
    public var newRevision: Revision
    /// Ordered list of mutation operations to apply atomically.
    public var operations: [Operation]
    /// Optional scheduling and transport priority class (§16).
    public var priority: UInt32

    /// Constructs a standard transaction advancing from `baseRevision` to `baseRevision + 1`.
    public init(
        baseRevision: Revision,
        operations: [Operation]
    ) {
        self.baseRevision = baseRevision
        self.newRevision = baseRevision.next
        self.operations = operations
        self.priority = 0
    }

    /// Constructs a transaction with explicit target revision and priority.
    public init(
        baseRevision: Revision,
        newRevision: Revision,
        operations: [Operation],
        priority: UInt32 = 0
    ) {
        self.baseRevision = baseRevision
        self.newRevision = newRevision
        self.operations = operations
        self.priority = priority
    }
}

// MARK: - Transaction Applier (§12.1, §18, §22)

/// The authoritative transactional mutation gateway for `SemanticStore` (§12.1, §18, §22).
///
/// `TransactionApplier` enforces all-or-nothing atomic execution, revision advancement,
/// and tracks `lastAppliedRevision` (§18).
/// Atomically captured semantic state and its corresponding applied revision.
public struct TransactionSnapshot: Equatable, Sendable {
    public let store: SemanticStore
    public let revision: Revision

    public init(store: SemanticStore, revision: Revision) {
        self.store = store
        self.revision = revision
    }
}

/// Synchronous transaction gateway whose mutable state is protected by `lock`.
///
/// The `@unchecked Sendable` conformance is justified by locking every read and write of
/// `_store` and `_lastAppliedRevision`. New mutable state must use the same lock.
public final class TransactionApplier: @unchecked Sendable {
    private let lock = NSLock()
    private var _store: SemanticStore
    private var _lastAppliedRevision: Revision

    /// A thread-safe copy of the underlying semantic store replica.
    public var store: SemanticStore {
        lock.lock()
        defer { lock.unlock() }
        return _store
    }

    /// A thread-safe copy of the latest committed revision.
    public var lastAppliedRevision: Revision {
        lock.lock()
        defer { lock.unlock() }
        return _lastAppliedRevision
    }

    /// Atomically returns the store and revision from the same committed state.
    public var currentSnapshot: TransactionSnapshot {
        lock.lock()
        defer { lock.unlock() }
        return TransactionSnapshot(
            store: _store,
            revision: _lastAppliedRevision
        )
    }

    /// Constructs a `TransactionApplier` wrapping an existing `SemanticStore`.
    public init(store: SemanticStore = SemanticStore()) {
        self._store = store
        self._lastAppliedRevision = store.revision
    }

    /// Constructs a `TransactionApplier` with specified limits and initial revision.
    public init(limits: StoreLimits, initialRevision: Revision = .initial) {
        self._store = SemanticStore(limits: limits, revision: initialRevision)
        self._lastAppliedRevision = initialRevision
    }

    /// Executes operations speculatively against a staged clone of the store.
    ///
    /// The caller must hold `lock`; this method accesses backing storage directly to avoid
    /// recursively acquiring the non-recursive `NSLock`.
    private func applyStaged(
        operations: [Operation],
        newRevision: Revision
    ) -> Result<Revision, TxnError> {
        let maxOps = _store.limits.maxTransactionOperations
        if operations.count > maxOps {
            return .failure(.maxOperationsExceeded(limit: maxOps, actual: operations.count))
        }

        var staged = _store.cloneStaging()
        for (idx, op) in operations.enumerated() {
            do {
                try op.apply(to: &staged)
            } catch let error as StoreError {
                return .failure(.opFailed(opIndex: idx, source: error))
            } catch {
                return .failure(.opFailed(opIndex: idx, source: .operationError(error.localizedDescription)))
            }
        }

        _store.commitStaging(staged, newRevision: newRevision)
        _lastAppliedRevision = newRevision
        return .success(newRevision)
    }

    /// Applies a sequence of mutation operations as an atomic transaction advancing from `baseRevision` to `baseRevision + 1` (§12.1).
    ///
    /// Semantics (§12.1, §26):
    /// 1. Rejects if `baseRevision` does not match the store's current committed revision (`TxnError.staleBaseRevision`).
    /// 2. Enforces `maxTransactionOperations` limit as a pre-check (`TxnError.maxOperationsExceeded`).
    /// 3. Applies all operations speculatively to a private staging copy of the store.
    /// 4. If any operation fails, the entire transaction is discarded with zero visible side-effects or partial changes on the store.
    /// 5. On full success, atomically commits staged mutations and advances the revision.
    public func apply(
        baseRevision: Revision,
        operations: [Operation]
    ) -> Result<Revision, TxnError> {
        lock.lock()
        defer { lock.unlock() }

        let currentRevision = _store.revision
        if baseRevision != currentRevision {
            return .failure(
                .staleBaseRevision(expected: currentRevision, actual: baseRevision)
            )
        }

        return applyStaged(
            operations: operations,
            newRevision: baseRevision.next
        )
    }

    /// Applies a structured `Transaction` record, validating its base and target revisions.
    public func apply(record: Transaction) -> Result<Revision, TxnError> {
        lock.lock()
        defer { lock.unlock() }

        let currentRevision = _store.revision
        if record.baseRevision != currentRevision {
            return .failure(
                .staleBaseRevision(
                    expected: currentRevision,
                    actual: record.baseRevision
                )
            )
        }

        let expectedNewRevision = record.baseRevision.next
        if record.newRevision != expectedNewRevision {
            return .failure(
                .invalidNewRevision(
                    expected: expectedNewRevision,
                    actual: record.newRevision
                )
            )
        }

        return applyStaged(
            operations: record.operations,
            newRevision: record.newRevision
        )
    }
}
