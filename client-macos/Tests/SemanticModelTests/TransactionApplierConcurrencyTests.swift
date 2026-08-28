import SemanticModel
import Testing

struct TransactionApplierConcurrencyTests {
    @Test
    func snapshotReturnsMatchingStoreAndRevision() {
        let applier = TransactionApplier()
        let result = applier.apply(
            baseRevision: .initial,
            operations: [
                .createNode(id: 1, nodeType: .surface)
            ]
        )
        #expect(result == .success(Revision(1)))

        let snapshot = applier.currentSnapshot
        #expect(snapshot.store.revision == snapshot.revision)
        #expect(snapshot.revision == Revision(1))
        #expect(applier.store == snapshot.store)
        #expect(applier.lastAppliedRevision == snapshot.revision)
    }

    @Test
    func snapshotsStayConsistentDuringConcurrentReadsAndWrites() async throws {
        let applier = TransactionApplier()
        let initialResult = applier.apply(
            baseRevision: .initial,
            operations: [
                .createNode(
                    id: 1,
                    nodeType: .text,
                    properties: [
                        Property(property: .value, value: .signedInt(0))
                    ]
                )
            ]
        )
        try #require(initialResult == .success(Revision(1)))

        let allOperationsSucceeded = await withTaskGroup(of: Bool.self) { group in
            group.addTask {
                for value in 1...250 {
                    let snapshot = applier.currentSnapshot
                    let transaction = Transaction(
                        baseRevision: snapshot.revision,
                        operations: [
                            .setProperty(
                                id: 1,
                                property: .value,
                                value: .signedInt(Int64(value))
                            )
                        ]
                    )
                    guard case .success = applier.apply(record: transaction) else {
                        return false
                    }
                    await Task.yield()
                }
                return true
            }

            for _ in 0..<4 {
                group.addTask {
                    for _ in 0..<1_000 {
                        let independentlyReadStore = applier.store
                        let independentlyReadRevision = applier.lastAppliedRevision
                        let snapshot = applier.currentSnapshot

                        guard snapshot.store.revision == snapshot.revision,
                              independentlyReadStore.revision <= snapshot.revision,
                              independentlyReadRevision <= snapshot.revision else {
                            return false
                        }
                        await Task.yield()
                    }
                    return true
                }
            }

            var succeeded = true
            for await result in group {
                if result == false {
                    succeeded = false
                }
            }
            return succeeded
        }

        #expect(allOperationsSucceeded)

        let finalSnapshot = applier.currentSnapshot
        #expect(finalSnapshot.store.revision == Revision(251))
        #expect(finalSnapshot.revision == Revision(251))
        let node = try #require(finalSnapshot.store.getNode(1))
        #expect(node.getProperty(.value) == .signedInt(250))
    }
}
