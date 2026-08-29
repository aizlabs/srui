import SemanticModel
import Testing

@Suite("TransactionApplier Concurrency Tests")
struct TransactionApplierConcurrencyTests {
    private static let surfaceID: NodeId = 1
    private static let parentID: NodeId = 1
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
    func applySnapshotReplacesStateRegardlessOfCurrentRevision() {
        let applier = TransactionApplier()
        _ = applier.apply(
            baseRevision: .initial,
            operations: [
                .createNode(id: 1, nodeType: .surface),
                .createNode(
                    id: 2,
                    nodeType: .text,
                    parentID: 1,
                    properties: [Property(property: .text, value: .string("Before"))]
                ),
            ]
        )
        #expect(applier.lastAppliedRevision == Revision(1))

        let snapshot = Transaction(
            baseRevision: .initial,
            newRevision: Revision(5),
            operations: [
                .createNode(id: 1, nodeType: .surface),
                .createNode(
                    id: 2,
                    nodeType: .text,
                    parentID: 1,
                    properties: [Property(property: .text, value: .string("After"))]
                ),
            ]
        )

        let result = applier.applySnapshot(record: snapshot)
        #expect(result == .success(Revision(5)))
        #expect(applier.lastAppliedRevision == Revision(5))
        #expect(applier.store.node(for: 2)?.getProperty(.text) == .string("After"))
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

    @Test
    func multipleWritersSameRevisionProduceOneSuccessAndStaleFailures() async {
        let applier = TransactionApplier()
        let setup = applier.apply(
            baseRevision: .initial,
            operations: [
                .createNode(id: Self.surfaceID, nodeType: .surface)
            ]
        )
        #expect(setup == .success(Revision(1)))

        let sharedBase = Revision(1)
        let writerCount = 16

        let results = await withTaskGroup(of: Result<Revision, TxnError>.self) { group in
            for writer in 0..<writerCount {
                group.addTask {
                    applier.apply(
                        baseRevision: sharedBase,
                        operations: [
                            .createNode(
                                id: NodeId(UInt64(writer + 2)),
                                nodeType: .text,
                                parentID: Self.parentID
                            )
                        ]
                    )
                }
            }

            var collected: [Result<Revision, TxnError>] = []
            for await result in group {
                collected.append(result)
            }
            return collected
        }

        let successes = results.compactMap { result -> Revision? in
            guard case .success(let revision) = result else { return nil }
            return revision
        }
        let staleFailureCount = results.reduce(into: 0) { count, result in
            if case .failure(.staleBaseRevision) = result {
                count += 1
            }
        }

        #expect(successes.count == 1)
        #expect(successes[0] == Revision(2))
        #expect(staleFailureCount == writerCount - 1)
        #expect(applier.lastAppliedRevision == Revision(2))
        #expect(applier.store.nodeCount == 2)
    }

    @Test
    func freshSnapshotWritersProduceContiguousFinalRevisions() async {
        let applier = TransactionApplier()
        let setup = applier.apply(
            baseRevision: .initial,
            operations: [
                .createNode(
                    id: Self.surfaceID,
                    nodeType: .text,
                    properties: [
                        Property(property: .value, value: .signedInt(0))
                    ]
                )
            ]
        )
        #expect(setup == .success(Revision(1)))

        let writerCount = 4
        let commitsPerWriter = 25
        let expectedCommits = writerCount * commitsPerWriter

        let appliedRevisions = await withTaskGroup(of: [Revision].self) { group in
            for writer in 0..<writerCount {
                group.addTask {
                    var revisions: [Revision] = []
                    var remaining = commitsPerWriter
                    while remaining > 0 {
                        let snapshot = applier.currentSnapshot
                        let transaction = Transaction(
                            baseRevision: snapshot.revision,
                            operations: [
                                .setProperty(
                                    id: Self.surfaceID,
                                    property: .value,
                                    value: .signedInt(Int64(writer * commitsPerWriter + (commitsPerWriter - remaining + 1)))
                                )
                            ]
                        )
                        switch applier.apply(record: transaction) {
                        case .success(let revision):
                            revisions.append(revision)
                            remaining -= 1
                        case .failure(.staleBaseRevision):
                            continue
                        case .failure:
                            return []
                        }
                        await Task.yield()
                    }
                    return revisions
                }
            }

            var allRevisions: [Revision] = []
            for await revisions in group {
                allRevisions.append(contentsOf: revisions)
            }
            return allRevisions
        }

        #expect(appliedRevisions.count == expectedCommits)

        let sortedRevisions = appliedRevisions.map(\.value).sorted()
        #expect(sortedRevisions == Array(2...UInt64(expectedCommits + 1)))

        let finalSnapshot = applier.currentSnapshot
        #expect(finalSnapshot.revision == Revision(UInt64(expectedCommits + 1)))
        #expect(finalSnapshot.store.revision == finalSnapshot.revision)
    }

    @Test
    func rejectedWriteNeverMutatesSnapshot() async {
        let applier = TransactionApplier()
        let setup = applier.apply(
            baseRevision: .initial,
            operations: [
                .createNode(
                    id: Self.surfaceID,
                    nodeType: .text,
                    properties: [
                        Property(property: .text, value: .string("Committed"))
                    ]
                )
            ]
        )
        #expect(setup == .success(Revision(1)))

        let unchanged = applier.currentSnapshot

        let staleResult = applier.apply(
            baseRevision: .initial,
            operations: [
                .setProperty(id: Self.surfaceID, property: .text, value: .string("Stale"))
            ]
        )
        #expect(
            staleResult == .failure(.staleBaseRevision(expected: Revision(1), actual: .initial))
        )
        #expect(applier.currentSnapshot == unchanged)

        let invalidResult = applier.apply(
            baseRevision: Revision(1),
            operations: [
                .setProperty(id: Self.surfaceID, property: .text, value: .string("Partial")),
                .setProperty(id: NodeId(999), property: .text, value: .string("Missing node"))
            ]
        )
        #expect(
            invalidResult == .failure(.opFailed(opIndex: 1, source: .nodeNotFound(NodeId(999))))
        )
        #expect(applier.currentSnapshot == unchanged)

        let observedSnapshots = await withTaskGroup(of: Bool.self) { group in
            for _ in 0..<4 {
                group.addTask {
                    for _ in 0..<500 {
                        let before = applier.currentSnapshot
                        _ = applier.apply(
                            baseRevision: .initial,
                            operations: [
                                .setProperty(
                                    id: Self.surfaceID,
                                    property: .text,
                                    value: .string("Rejected")
                                )
                            ]
                        )
                        if applier.currentSnapshot != before {
                            return false
                        }
                        await Task.yield()
                    }
                    return true
                }
            }

            var allConsistent = true
            for await consistent in group {
                if consistent == false {
                    allConsistent = false
                }
            }
            return allConsistent
        }

        #expect(observedSnapshots)
        #expect(applier.currentSnapshot == unchanged)
        #expect(applier.store.getNode(Self.surfaceID)?.getProperty(.text) == .string("Committed"))
    }

    @Test
    func readersNeverSeeHalfAppliedNodeOrModelChanges() async throws {
        let applier = TransactionApplier()
        let setup = applier.apply(
            baseRevision: .initial,
            operations: [
                .createNode(id: Self.surfaceID, nodeType: .surface)
            ]
        )
        try #require(setup == .success(Revision(1)))

        let allChecksPassed = await withTaskGroup(of: Bool.self) { group in
            group.addTask {
                for index in 0..<100 {
                    let nodeID = NodeId(UInt64(index + 100))
                    let modelID = ModelId(UInt64(index + 200))
                    let itemID = ItemId(UInt64(index + 300))
                    let marker = "node-\(index)"

                    while true {
                        let snapshot = applier.currentSnapshot
                        let transaction = Transaction(
                            baseRevision: snapshot.revision,
                            operations: [
                                .createNode(
                                    id: nodeID,
                                    nodeType: .text,
                                    parentID: Self.parentID,
                                    properties: []
                                ),
                                .setProperty(
                                    id: nodeID,
                                    property: .text,
                                    value: .string(marker)
                                ),
                                .createModel(id: modelID, modelType: .table, itemCount: 1),
                                .modelInsert(
                                    id: modelID,
                                    index: 0,
                                    items: [
                                        ModelItem(itemID: itemID, value: .string(marker))
                                    ]
                                ),
                            ]
                        )
                        if case .success = applier.apply(record: transaction) {
                            break
                        }
                        await Task.yield()
                    }
                }
                return true
            }

            for _ in 0..<4 {
                group.addTask {
                    for _ in 0..<2_000 {
                        let snapshot = applier.currentSnapshot
                        guard snapshot.store.revision == snapshot.revision else {
                            return false
                        }

                        for nodeValue in 100..<200 {
                            let nodeID = NodeId(UInt64(nodeValue))
                            guard snapshot.store.containsNode(nodeID) else { continue }
                            let expected = "node-\(nodeValue - 100)"
                            let text = snapshot.store.getNode(nodeID)?.getProperty(.text)
                            if text != .string(expected) {
                                return false
                            }
                        }

                        for modelValue in 200..<300 {
                            let modelID = ModelId(UInt64(modelValue))
                            guard let model = snapshot.store.getModel(modelID) else { continue }
                            if model.cachedItemCount == 0 {
                                return false
                            }
                            let expected = "node-\(modelValue - 200)"
                            if model.getItemByIndex(0)?.value != .string(expected) {
                                return false
                            }
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

        #expect(allChecksPassed)
        #expect(applier.currentSnapshot.revision == Revision(101))
    }

    @Test
    func concurrentPropertyUpdatesOnDifferentNodes() async {
        let applier = TransactionApplier()
        let nodeCount = 8
        let updatesPerNode = 20

        var setupOps: [Operation] = [
            .createNode(id: Self.surfaceID, nodeType: .surface)
        ]
        for index in 0..<nodeCount {
            setupOps.append(
                .createNode(
                    id: NodeId(UInt64(index + 2)),
                    nodeType: .text,
                    parentID: Self.parentID,
                    properties: [
                        Property(property: .value, value: .signedInt(0))
                    ]
                )
            )
        }
        #expect(applier.apply(baseRevision: .initial, operations: setupOps) == .success(Revision(1)))

        let allSucceeded = await withTaskGroup(of: Bool.self) { group in
            for nodeIndex in 0..<nodeCount {
                group.addTask {
                    let nodeID = NodeId(UInt64(nodeIndex + 2))
                    var applied = 0
                    while applied < updatesPerNode {
                        let snapshot = applier.currentSnapshot
                        let transaction = Transaction(
                            baseRevision: snapshot.revision,
                            operations: [
                                .setProperty(
                                    id: nodeID,
                                    property: .value,
                                    value: .signedInt(Int64(applied + 1))
                                )
                            ]
                        )
                        switch applier.apply(record: transaction) {
                        case .success:
                            applied += 1
                        case .failure(.staleBaseRevision):
                            continue
                        case .failure:
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

        #expect(allSucceeded)

        let expectedRevision = Revision(UInt64(1 + nodeCount * updatesPerNode))
        let finalSnapshot = applier.currentSnapshot
        #expect(finalSnapshot.revision == expectedRevision)
        #expect(finalSnapshot.store.revision == expectedRevision)

        for nodeIndex in 0..<nodeCount {
            let nodeID = NodeId(UInt64(nodeIndex + 2))
            let node = finalSnapshot.store.getNode(nodeID)
            #expect(node?.getProperty(.value) == .signedInt(Int64(updatesPerNode)))
        }
    }
}
