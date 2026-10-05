//
// SnapshotAssembler.swift
// Protocol
//
// Replica-side staging of one snapshot delivered in several envelopes: the PX-004-G01 protocol
// extension documented in protocol/README.md (grounded in §12.1 incomplete-transaction discard and
// §26 limits; the v0.6 design's §18 defines only the single-envelope snapshot form).
//
// A snapshot is ONE transaction (`base_revision = 0`, `new_revision = snapshot_revision`) applied
// wholesale. When its single envelope would exceed the §26 frame limit, the continuity decision
// (`SERVER WELCOME` / `SERVER RESYNC_REQUIRED`) announces `snapshot_parts`, the number of
// consecutive `Transaction` envelopes that carry it; the client bounds that number with
// `ClientLimits.max_snapshot_parts`. The assembler stages the envelopes off to the side and yields
// the reassembled snapshot only after the last one, so no partially applied snapshot is ever
// visible. Dropping it early (connection loss, a rejected envelope) discards the partial snapshot
// without touching the replica, as §12.1 requires of an incomplete transaction.
//
// Mirrors `server-rust/protocol/src/snapshot_framing.rs::SnapshotAssembler`; the shared suite-8
// vectors (`protocol/conformance-vectors/suites/08-reconnect/vectors`) hold both to one oracle.
//

import Foundation

/// `ClientLimits.max_snapshot_parts` the reference client advertises (§26).
///
/// Sixteen envelopes of at most `defaultMaxFrameSize` bound one snapshot's staged wire bytes at
/// 256 MiB; the decoded replica stays bounded by its own store limits.
public let defaultMaxSnapshotParts: UInt32 = 16

/// Why a replica refused a snapshot envelope sequence (PX-004-G01 extension; §12.1, §26).
public enum SnapshotAssemblyError: Error, Equatable, Sendable, CustomStringConvertible {
    /// The decision announced more envelopes than this client advertised it would stage.
    case partsExceedLimit(announced: UInt32, maxParts: UInt32)
    /// An envelope is not part of the announced snapshot: wrong base or revision.
    case notASnapshotPart(
        partIndex: UInt32, baseRevision: UInt64, newRevision: UInt64, snapshotRevision: UInt64)
    /// A split envelope carried no operations; a conforming server never sends one.
    case emptyPart(partIndex: UInt32)
    /// The staged snapshot would exceed `max_transaction_operations`.
    case operationLimitExceeded(partIndex: UInt32, maxOperations: Int, staged: Int)
    /// Every announced envelope already arrived.
    case alreadyComplete

    /// Stable identifier shared with the Rust assembler and the suite-8 vectors.
    public var code: String {
        switch self {
        case .partsExceedLimit: return "parts_exceed_limit"
        case .notASnapshotPart: return "not_a_snapshot_part"
        case .emptyPart: return "empty_part"
        case .operationLimitExceeded: return "operation_limit_exceeded"
        case .alreadyComplete: return "already_complete"
        }
    }

    public var description: String {
        switch self {
        case .partsExceedLimit(let announced, let maxParts):
            return "snapshot announced in \(announced) envelopes, above the advertised "
                + "max_snapshot_parts \(maxParts) (§26)"
        case .notASnapshotPart(let index, let base, let new, let snapshot):
            return "snapshot envelope \(index) spans \(base) -> \(new), expected 0 -> \(snapshot) (§18)"
        case .emptyPart(let index):
            return "split snapshot envelope \(index) carries no operations"
        case .operationLimitExceeded(let index, let maxOperations, let staged):
            return "snapshot envelope \(index) brings the staged snapshot to \(staged) operations, "
                + "above max_transaction_operations \(maxOperations) (§26)"
        case .alreadyComplete:
            return "snapshot already complete"
        }
    }
}

/// Stages the envelopes of one announced snapshot (PX-004-G01 extension; §12.1).
///
/// After any thrown error the assembler must be discarded.
public struct SnapshotAssembler: Sendable {
    public let snapshotRevision: UInt64
    public let expectedParts: UInt32
    public private(set) var receivedParts: UInt32 = 0
    private let maxOperations: Int
    private var priority: UInt32 = 0
    private var operations: [SRUIOperation] = []

    /// Prepares to stage the snapshot a continuity decision announced.
    ///
    /// `announcedParts` and `maxParts` of zero both mean one envelope.
    public init(
        snapshotRevision: UInt64,
        announcedParts: UInt32,
        maxParts: UInt32,
        maxOperations: Int
    ) throws {
        let expected = max(announcedParts, 1)
        let bound = max(maxParts, 1)
        guard expected <= bound else {
            throw SnapshotAssemblyError.partsExceedLimit(announced: expected, maxParts: bound)
        }
        self.snapshotRevision = snapshotRevision
        self.expectedParts = expected
        self.maxOperations = maxOperations
    }

    /// Stages one envelope; returns the whole snapshot once the last one arrives.
    public mutating func accept(_ part: SRUITransaction) throws -> SRUITransaction? {
        guard receivedParts < expectedParts else {
            throw SnapshotAssemblyError.alreadyComplete
        }
        let index = receivedParts
        guard part.baseRevision == 0, part.newRevision == snapshotRevision else {
            throw SnapshotAssemblyError.notASnapshotPart(
                partIndex: index,
                baseRevision: part.baseRevision,
                newRevision: part.newRevision,
                snapshotRevision: snapshotRevision
            )
        }
        if expectedParts > 1, part.operations.isEmpty {
            throw SnapshotAssemblyError.emptyPart(partIndex: index)
        }
        let staged = operations.count + part.operations.count
        guard staged <= maxOperations else {
            throw SnapshotAssemblyError.operationLimitExceeded(
                partIndex: index, maxOperations: maxOperations, staged: staged)
        }

        receivedParts += 1
        if index == 0 {
            priority = part.priority
        }
        if expectedParts == 1 {
            return part
        }
        operations.append(contentsOf: part.operations)
        guard receivedParts == expectedParts else {
            return nil
        }
        var snapshot = SRUITransaction()
        snapshot.baseRevision = 0
        snapshot.newRevision = snapshotRevision
        snapshot.priority = priority
        snapshot.operations = operations
        operations = []
        return snapshot
    }
}
