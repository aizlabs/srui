//
// TransactionApplierAtomicityTests.swift
// SemanticModelTests
//
// The renderer must never be handed a store from a different revision than the transaction it is
// applying (§12.1, §22.2).
//

import Testing
import SemanticModel

@Suite("TransactionApplier Commit Atomicity Tests")
struct TransactionApplierAtomicityTests {
    private static let surfaceID = NodeId(1)
    private static let textID = NodeId(2)

    private func snapshot(newRevision: UInt64, text: String) -> Transaction {
        Transaction(
            baseRevision: .initial,
            newRevision: Revision(newRevision),
            operations: [
                .createNode(id: Self.surfaceID, nodeType: .surface),
                .createNode(
                    id: Self.textID,
                    nodeType: .text,
                    parentID: Self.surfaceID,
                    properties: [Property(property: .text, value: .string(text))]
                ),
            ]
        )
    }

    @Test("applyCommitted returns the store produced by that very transaction (§22.2)")
    func commitPairsTransactionWithItsOwnStore() async throws {
        let applier = TransactionApplier()
        _ = try applier.applyResyncSnapshot(record: snapshot(newRevision: 1, text: "seed")).get()

        let mismatches = await withTaskGroup(of: Int.self) { group in
            for worker in 0..<8 {
                group.addTask {
                    var mismatched = 0
                    for iteration in 0..<50 {
                        let text = "w\(worker)-\(iteration)"
                        let record = Transaction(
                            baseRevision: applier.lastAppliedRevision,
                            operations: [
                                .setProperty(id: Self.textID, property: .text, value: .string(text)),
                            ]
                        )
                        guard case .success(let committed) = applier.applyCommitted(record: record) else {
                            continue
                        }
                        if committed.store.revision != committed.revision {
                            mismatched += 1
                        }
                        if committed.store.getNode(Self.textID)?.getProperty(.text) != .string(text) {
                            mismatched += 1
                        }
                    }
                    return mismatched
                }
            }
            var total = 0
            for await count in group {
                total += count
            }
            return total
        }

        #expect(mismatches == 0)
    }

    @Test("applyResyncSnapshot returns the rebuilt store atomically (§22.2)")
    func resyncSnapshotReturnsRebuiltStore() throws {
        let applier = TransactionApplier()
        let committed = try applier.applyResyncSnapshot(
            record: snapshot(newRevision: 4, text: "fresh")
        ).get()

        #expect(committed.revision == Revision(4))
        #expect(committed.store.revision == Revision(4))
        #expect(committed.store.getNode(Self.textID)?.getProperty(.text) == .string("fresh"))
    }
}
