//
// StoreTraceTests.swift
// SemanticModelTests
//
// Deterministic seeded store trace tests (§6.2, §12.1, §13, §26, §32 item 1).
// Mirrors server-rust/semantic-tree/tests/trace_test.rs.
//

import XCTest
@testable import SemanticModel

final class StoreTraceTests: XCTestCase {

    // MARK: - Shared trace parameters (must match trace_test.rs)

    private static let traceSeeds: [UInt64] = [0x5EED_0001, 0x5EED_0002, 0x5EED_CAFE]
    private static let stepsPerSeed = 120

    private static func traceLimits() -> StoreLimits {
        var limits = StoreLimits()
        limits.maxTreeDepth = 8
        limits.maxNodeCount = 48
        limits.maxStringLength = 256
        limits.maxValueDepth = 8
        limits.maxListElements = 16
        limits.maxRecordProperties = 16
        limits.maxTransactionOperations = 6
        limits.maxModelCount = 4
        limits.maxCachedItemsPerModel = 32
        limits.maxItemsPerModelOperation = 8
        return limits
    }

    // MARK: - Tests

    func testDeterministicStoreTraceSeededSequences() {
        for seed in Self.traceSeeds {
            do {
                try runSeededTrace(seed: seed)
            } catch let mismatch as TraceMismatch {
                persistFailingTrace(mismatch)
                XCTFail(
                    "store trace mismatch: seed=\(String(format: "%#x", mismatch.seed)) " +
                    "step=\(mismatch.step) rejected=\(mismatch.rejected) " +
                    "liveRev=\(mismatch.liveSnapshot.revision.value) " +
                    "refRev=\(mismatch.referenceSnapshot.revision.value)"
                )
            } catch {
                XCTFail("unexpected error for seed \(seed): \(error)")
            }
        }
    }

    func testTraceReferenceModelMatchesReplayOnEmptyCommitLog() {
        let limits = Self.traceLimits()
        let store = SemanticStore(limits: limits)
        XCTAssertEqual(takeSnapshot(store), referenceSnapshot(committed: [], limits: limits))
    }

    // MARK: - Portable deterministic PRNG (LCG; identical constants in Rust)

    private struct LcgRng {
        private var state: UInt64

        init(seed: UInt64) {
            state = seed
        }

        mutating func next() -> UInt64 {
            state = state &* 6_364_136_223_846_793_005 &+ 1
            return state
        }

        mutating func genUsize(upperExclusive: Int) -> Int {
            guard upperExclusive > 0 else { return 0 }
            return Int(next() % UInt64(upperExclusive))
        }

        mutating func genBool(numer: UInt64, denom: UInt64) -> Bool {
            next() % denom < numer
        }
    }

    // MARK: - Store snapshot

    private struct NodeSnapshot: Equatable {
        let nodeType: TypeRef
        let parentID: NodeId?
        let orderedChildren: [NodeId]
        let properties: [PropertyRef: Value]
    }

    private struct ItemEntry: Equatable {
        let itemID: ItemId
        let value: Value
    }

    private struct ModelSnapshot: Equatable {
        let modelType: TypeRef
        let itemCount: UInt64
        let cachedItemCount: Int
        let cachedRanges: [SemanticRange]
        let itemsByIndex: [UInt64: ItemEntry]
    }

    private struct StoreSnapshot: Equatable {
        let revision: Revision
        let nodeCount: Int
        let roots: [NodeId]
        let nodes: [NodeId: NodeSnapshot]
        let modelCount: Int
        let models: [ModelId: ModelSnapshot]
    }

    private func takeSnapshot(_ store: SemanticStore) -> StoreSnapshot {
        var nodes: [NodeId: NodeSnapshot] = [:]
        for root in store.rootIDs {
            collectNodesSnapshot(store, id: root, into: &nodes)
        }

        var models: [ModelId: ModelSnapshot] = [:]
        for modelID in store.modelIDs {
            if let model = store.getModel(modelID) {
                var itemsByIndex: [UInt64: ItemEntry] = [:]
                for (idx, item) in model.items {
                    itemsByIndex[idx] = ItemEntry(itemID: item.itemID, value: item.value)
                }
                models[modelID] = ModelSnapshot(
                    modelType: model.modelType,
                    itemCount: model.itemCount,
                    cachedItemCount: model.cachedItemCount,
                    cachedRanges: model.cachedRanges(),
                    itemsByIndex: itemsByIndex
                )
            }
        }

        return StoreSnapshot(
            revision: store.revision,
            nodeCount: store.nodeCount,
            roots: store.rootIDs,
            nodes: nodes,
            modelCount: store.modelCount,
            models: models
        )
    }

    private func collectNodesSnapshot(
        _ store: SemanticStore,
        id: NodeId,
        into nodes: inout [NodeId: NodeSnapshot]
    ) {
        guard let node = store.getNode(id) else { return }
        nodes[id] = NodeSnapshot(
            nodeType: node.nodeType,
            parentID: node.parentID,
            orderedChildren: node.orderedChildren,
            properties: node.properties
        )
        for child in node.orderedChildren {
            collectNodesSnapshot(store, id: child, into: &nodes)
        }
    }

    private func referenceSnapshot(committed: [Transaction], limits: StoreLimits) -> StoreSnapshot {
        let applier = TransactionApplier(limits: limits, initialRevision: .initial)
        for txn in committed {
            let result = applier.apply(record: txn)
            guard case .success = result else {
                XCTFail("committed transaction must replay cleanly: \(result)")
                return takeSnapshot(applier.store)
            }
        }
        return takeSnapshot(applier.store)
    }

    // MARK: - Trace generator

    private struct TraceGen {
        var nextNodeID: UInt64 = 1
        var nextModelID: UInt64 = 100
        var nextItemID: UInt64 = 1_000
        var nodeIDs: [NodeId] = []
        var modelIDs: [ModelId] = []

        mutating func allocNodeID() -> NodeId {
            let id = NodeId(nextNodeID)
            nextNodeID += 1
            return id
        }

        mutating func allocModelID() -> ModelId {
            let id = ModelId(nextModelID)
            nextModelID += 1
            return id
        }

        mutating func allocItemID() -> ItemId {
            let id = ItemId(nextItemID)
            nextItemID += 1
            return id
        }

        func leafNodes(store: SemanticStore) -> [NodeId] {
            nodeIDs.filter { id in
                store.getNode(id)?.orderedChildren.isEmpty ?? false
            }
        }

        func pickNode(rng: inout LcgRng) -> NodeId? {
            guard !nodeIDs.isEmpty else { return nil }
            return nodeIDs[rng.genUsize(upperExclusive: nodeIDs.count)]
        }

        func pickModel(rng: inout LcgRng) -> ModelId? {
            guard !modelIDs.isEmpty else { return nil }
            return modelIDs[rng.genUsize(upperExclusive: modelIDs.count)]
        }

        static let nodeTypes: [TypeRef] = [.surface, .column, .row, .text, .button]
        static let propertyRefs: [PropertyRef] = [.label, .text, .enabled, .busy]

        static func stringValue(rng: inout LcgRng, tag: String) -> Value {
            let n = rng.next() % 10_000
            return .string("\(tag)-\(n)")
        }

        mutating func generateTransaction(
            rng: inout LcgRng,
            store: SemanticStore,
            step: Int
        ) -> Transaction {
            let base = store.revision
            let stale = step > 0 && rng.genBool(numer: 1, denom: 20)
            let baseRevision = stale ? Revision(base.value &- 1) : base

            let opCount = 1 + rng.genUsize(upperExclusive: 3)
            var ops: [StoreOperation] = []
            ops.reserveCapacity(opCount)

            for _ in 0..<opCount {
                if nodeIDs.isEmpty {
                    ops.append(genCreateRoot(rng: &rng))
                    continue
                }

                let choice = rng.genUsize(upperExclusive: 10)
                let op: StoreOperation
                switch choice {
                case 0, 1:
                    op = genCreateNode(rng: &rng, store: store)
                case 2, 3:
                    op = genSetProperty(rng: &rng) ?? genCreateNode(rng: &rng, store: store)
                case 4:
                    op = genClearProperty(rng: &rng, store: store) ?? genCreateNode(rng: &rng, store: store)
                case 5:
                    op = genDeleteLeaf(rng: &rng, store: store) ?? genCreateNode(rng: &rng, store: store)
                case 6:
                    op = genMoveNode(rng: &rng, store: store) ?? genCreateNode(rng: &rng, store: store)
                case 7:
                    op = genBatchPropertySet(rng: &rng) ?? genCreateNode(rng: &rng, store: store)
                case 8:
                    op = genCreateModel(rng: &rng)
                default:
                    op = genModelInsert(rng: &rng, store: store)
                }
                ops.append(op)
            }

            if ops.isEmpty {
                ops.append(genCreateRoot(rng: &rng))
            }

            return Transaction(
                baseRevision: baseRevision,
                newRevision: baseRevision.next,
                operations: ops
            )
        }

        mutating func genCreateRoot(rng: inout LcgRng) -> StoreOperation {
            let id = allocNodeID()
            nodeIDs.append(id)
            return .create(
                id: id,
                nodeType: .surface,
                properties: [(PropertyRef.label, Self.stringValue(rng: &rng, tag: "root"))]
            )
        }

        mutating func genCreateNode(rng: inout LcgRng, store: SemanticStore) -> StoreOperation {
            let id = allocNodeID()
            let parent = pickNode(rng: &rng)
            let nodeType = Self.nodeTypes[rng.genUsize(upperExclusive: Self.nodeTypes.count)]
            let childIndex: Int?
            if rng.genBool(numer: 1, denom: 3), let parent {
                let kids = store.children(of: parent) ?? []
                childIndex = rng.genUsize(upperExclusive: kids.count + 1)
            } else {
                childIndex = nil
            }
            nodeIDs.append(id)
            return .create(
                id: id,
                nodeType: nodeType,
                parentID: parent,
                childIndex: childIndex,
                properties: [(PropertyRef.label, Self.stringValue(rng: &rng, tag: "node"))]
            )
        }

        mutating func genSetProperty(rng: inout LcgRng) -> StoreOperation? {
            guard let id = pickNode(rng: &rng) else { return nil }
            let prop = Self.propertyRefs[rng.genUsize(upperExclusive: Self.propertyRefs.count)]
            let value: Value
            switch prop {
            case PropertyRef.enabled, PropertyRef.busy:
                value = .bool(rng.genBool(numer: 1, denom: 2))
            default:
                value = Self.stringValue(rng: &rng, tag: "prop")
            }
            return .setProperty(id: id, property: prop, value: value)
        }

        mutating func genClearProperty(rng: inout LcgRng, store: SemanticStore) -> StoreOperation? {
            guard let id = pickNode(rng: &rng),
                  let node = store.getNode(id),
                  !node.properties.isEmpty else {
                return genSetProperty(rng: &rng)
            }
            let props = Array(node.properties.keys)
            let prop = props[rng.genUsize(upperExclusive: props.count)]
            return .clearProperty(id: id, property: prop)
        }

        mutating func genDeleteLeaf(rng: inout LcgRng, store: SemanticStore) -> StoreOperation? {
            let leaves = leafNodes(store: store)
            guard leaves.count > 1 else { return genSetProperty(rng: &rng) }
            let id = leaves[rng.genUsize(upperExclusive: leaves.count)]
            nodeIDs.removeAll { $0 == id }
            return .deleteNode(id: id)
        }

        mutating func genMoveNode(rng: inout LcgRng, store: SemanticStore) -> StoreOperation? {
            guard nodeIDs.count >= 2,
                  let id = pickNode(rng: &rng),
                  let newParent = pickNode(rng: &rng),
                  id != newParent else {
                return genSetProperty(rng: &rng)
            }
            if store.rootIDs.count == 1, store.rootIDs[0] == id {
                return genSetProperty(rng: &rng)
            }
            let childIndex: Int?
            if rng.genBool(numer: 1, denom: 2) {
                let kids = store.children(of: newParent) ?? []
                childIndex = rng.genUsize(upperExclusive: kids.count + 1)
            } else {
                childIndex = nil
            }
            return .moveNode(id: id, newParentID: newParent, newChildIndex: childIndex)
        }

        mutating func genBatchPropertySet(rng: inout LcgRng) -> StoreOperation? {
            guard let id = pickNode(rng: &rng) else { return nil }
            return .batchPropertySet(
                id: id,
                properties: [
                    Property(property: .label, value: Self.stringValue(rng: &rng, tag: "batch")),
                    Property(property: .enabled, value: .bool(rng.genBool(numer: 1, denom: 2))),
                ]
            )
        }

        mutating func genCreateModel(rng: inout LcgRng) -> StoreOperation {
            let id = allocModelID()
            modelIDs.append(id)
            let itemCount = 50 + (rng.next() % 200)
            return .createModel(id: id, modelType: .list, itemCount: itemCount)
        }

        mutating func genModelInsert(rng: inout LcgRng, store: SemanticStore) -> StoreOperation {
            if let modelID = pickModel(rng: &rng),
               let model = store.getModel(modelID) {
                let index = rng.next() % (model.itemCount + 1)
                let item = ModelItem(
                    itemID: allocItemID(),
                    value: Self.stringValue(rng: &rng, tag: "item")
                )
                return .modelInsert(id: modelID, index: index, items: [item])
            }
            return genCreateModel(rng: &rng)
        }
    }

    // MARK: - Trace runner

    private struct TraceMismatch: Error {
        let seed: UInt64
        let step: Int
        let committed: [Transaction]
        let failingTxn: Transaction
        let liveSnapshot: StoreSnapshot
        let referenceSnapshot: StoreSnapshot
        let preSnapshot: StoreSnapshot
        let rejected: Bool
    }

    private func runSeededTrace(seed: UInt64) throws {
        let limits = Self.traceLimits()
        let applier = TransactionApplier(limits: limits, initialRevision: .initial)
        var gen = TraceGen()
        var rng = LcgRng(seed: seed)
        var committed: [Transaction] = []

        for step in 0..<Self.stepsPerSeed {
            let txn = gen.generateTransaction(rng: &rng, store: applier.store, step: step)
            let preSnapshot = takeSnapshot(applier.store)
            let referenceBefore = referenceSnapshot(committed: committed, limits: limits)

            XCTAssertEqual(
                preSnapshot,
                referenceBefore,
                "seed \(String(format: "%#x", seed)) step \(step): live store diverged from reference before txn"
            )

            let result = applier.apply(record: txn)

            switch result {
            case .success(let newRev):
                XCTAssertEqual(newRev, txn.newRevision)
                committed.append(txn)
                let live = takeSnapshot(applier.store)
                let reference = referenceSnapshot(committed: committed, limits: limits)
                if live != reference {
                    throw TraceMismatch(
                        seed: seed,
                        step: step,
                        committed: Array(committed.dropLast()),
                        failingTxn: txn,
                        liveSnapshot: live,
                        referenceSnapshot: reference,
                        preSnapshot: preSnapshot,
                        rejected: false
                    )
                }

            case .failure:
                let postSnapshot = takeSnapshot(applier.store)
                if postSnapshot != preSnapshot {
                    throw TraceMismatch(
                        seed: seed,
                        step: step,
                        committed: committed,
                        failingTxn: txn,
                        liveSnapshot: postSnapshot,
                        referenceSnapshot: referenceBefore,
                        preSnapshot: preSnapshot,
                        rejected: true
                    )
                }
                let reference = referenceSnapshot(committed: committed, limits: limits)
                if postSnapshot != reference {
                    throw TraceMismatch(
                        seed: seed,
                        step: step,
                        committed: committed,
                        failingTxn: txn,
                        liveSnapshot: postSnapshot,
                        referenceSnapshot: reference,
                        preSnapshot: preSnapshot,
                        rejected: true
                    )
                }
            }
        }
    }

    private func persistFailingTrace(_ mismatch: TraceMismatch) {
        guard ProcessInfo.processInfo.environment["SRUI_WRITE_TRACE_FIXTURE"] == "1" else { return }

        let currentFile = URL(fileURLWithPath: #filePath)
        let repoRoot = currentFile
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()

        let outDir = repoRoot.appendingPathComponent(
            "protocol/conformance-vectors/suites/01-core-state-machine/vectors")
        let fileName = String(format: "99_trace_seed_%llx_step_%d.json", mismatch.seed, mismatch.step)
        let path = outDir.appendingPathComponent(fileName)

        let fixture: [String: Any] = [
            "name": "trace_seed_\(String(format: "%llx", mismatch.seed))_step_\(mismatch.step)",
            "description": "Auto-generated minimal failing store trace (T35)",
            "spec_sections": ["§6.2", "§12.1", "§13", "§26"],
            "initial_limits": limitsJSON(Self.traceLimits()),
            "setup_transactions": mismatch.committed.map { transactionJSON($0) },
            "transaction": transactionJSON(mismatch.failingTxn),
            "expected_outcome": [
                "status": mismatch.rejected ? "rejected" : "success",
                "rollback_verified": mismatch.rejected,
                "expected_store_revision": mismatch.preSnapshot.revision.value,
            ] as [String: Any],
        ]

        guard let data = try? JSONSerialization.data(withJSONObject: fixture, options: [.prettyPrinted, .sortedKeys]) else {
            return
        }
        try? data.write(to: path)
        fputs("Wrote failing trace fixture to \(path.path)\n", stderr)
    }

    private func limitsJSON(_ limits: StoreLimits) -> [String: Any] {
        [
            "max_tree_depth": limits.maxTreeDepth,
            "max_node_count": limits.maxNodeCount,
            "max_transaction_operations": limits.maxTransactionOperations,
            "max_string_length": limits.maxStringLength,
            "max_value_depth": limits.maxValueDepth,
            "max_list_elements": limits.maxListElements,
            "max_record_properties": limits.maxRecordProperties,
            "max_model_count": limits.maxModelCount,
            "max_cached_items_per_model": limits.maxCachedItemsPerModel,
            "max_items_per_model_operation": limits.maxItemsPerModelOperation,
        ]
    }

    private func transactionJSON(_ txn: Transaction) -> [String: Any] {
        [
            "base_revision": txn.baseRevision.value,
            "new_revision": txn.newRevision.value,
            "operations": txn.operations.map { operationJSON($0) },
        ]
    }

    private func operationJSON(_ op: StoreOperation) -> [String: Any] {
        switch op {
        case .createNode(let id, let nodeType, let parentID, let childIndex, let properties):
            var props: [String: Any] = [:]
            for p in properties {
                if let name = p.property.standardName {
                    props[name] = valueJSON(p.value)
                }
            }
            return [
                "type": "CREATE_NODE",
                "node_id": id.value,
                "node_type": nodeType.standardName ?? "unknown",
                "parent_id": parentID.map { $0.value } as Any,
                "child_index": childIndex as Any,
                "properties": props,
            ]
        case .deleteNode(let id):
            return ["type": "DELETE_NODE", "node_id": id.value]
        case .setProperty(let id, let property, let value):
            return [
                "type": "SET_PROPERTY",
                "node_id": id.value,
                "property": property.standardName ?? "unknown",
                "value": valueJSON(value),
            ]
        case .clearProperty(let id, let property):
            return [
                "type": "CLEAR_PROPERTY",
                "node_id": id.value,
                "property": property.standardName ?? "unknown",
            ]
        case .moveNode(let id, let newParentID, let newChildIndex):
            return [
                "type": "MOVE_NODE",
                "node_id": id.value,
                "new_parent_id": newParentID.map { $0.value } as Any,
                "new_child_index": newChildIndex as Any,
            ]
        case .reorderChildren(let parentID, let newOrder):
            return [
                "type": "REORDER_CHILDREN",
                "parent_id": parentID.value,
                "new_order": newOrder.map { $0.value },
            ]
        case .batchPropertySet(let id, let properties):
            var props: [String: Any] = [:]
            for p in properties {
                if let name = p.property.standardName {
                    props[name] = valueJSON(p.value)
                }
            }
            return [
                "type": "BATCH_PROPERTY_SET",
                "node_id": id.value,
                "properties": props,
            ]
        case .createModel(let id, let modelType, let itemCount):
            return [
                "type": "CREATE_MODEL",
                "model_id": id.value,
                "model_type": modelType.standardName ?? "unknown",
                "item_count": itemCount,
            ]
        case .modelInsert(let id, let index, let items):
            return [
                "type": "MODEL_INSERT",
                "model_id": id.value,
                "index": index,
                "items": items.map { itemJSON($0) },
            ]
        case .modelDelete(let id, let index, let count, let itemIds):
            return [
                "type": "MODEL_DELETE",
                "model_id": id.value,
                "index": index as Any,
                "count": count as Any,
                "item_ids": itemIds.map { $0.value },
            ]
        case .modelUpdate(let id, let index, let items):
            return [
                "type": "MODEL_UPDATE",
                "model_id": id.value,
                "index": index as Any,
                "items": items.map { itemJSON($0) },
            ]
        case .modelResetRange(let id, let startIndex, let items, let totalCount):
            return [
                "type": "MODEL_RESET_RANGE",
                "model_id": id.value,
                "start_index": startIndex,
                "items": items.map { itemJSON($0) },
                "total_count": totalCount as Any,
            ]
        }
    }

    private func itemJSON(_ item: ModelItem) -> [String: Any] {
        ["item_id": item.itemID.value, "value": valueJSON(item.value)]
    }

    private func valueJSON(_ value: Value) -> Any {
        switch value {
        case .null: return NSNull()
        case .bool(let b): return b
        case .signedInt(let i): return i
        case .unsignedInt(let u): return u
        case .float64(let f): return f
        case .string(let s): return s
        default: return String(describing: value)
        }
    }
}
