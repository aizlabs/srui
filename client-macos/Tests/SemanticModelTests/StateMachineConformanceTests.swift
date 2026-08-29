//
// StateMachineConformanceTests.swift
// SemanticModelTests
//
// Cross-language conformance test runner replaying JSON state machine vectors (§32).
//

import XCTest
import Foundation
@testable import Protocol
@testable import SemanticModel

final class StateMachineConformanceTests: XCTestCase {

    private func findConformanceVectorsDirectory() -> URL? {
        // Look relative to this source file
        let currentFile = URL(fileURLWithPath: #filePath)
        let repoRoot = currentFile
            .deletingLastPathComponent() // Tests/SemanticModelTests
            .deletingLastPathComponent() // Tests
            .deletingLastPathComponent() // client-macos
            .deletingLastPathComponent() // repo root (srui)

        let vectorsDir = repoRoot.appendingPathComponent("protocol/conformance-vectors/state-machine")
        if FileManager.default.fileExists(atPath: vectorsDir.path) {
            return vectorsDir
        }

        // Fallback: search relative to process working directory
        let cwd = URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
        let cwdVectors = cwd.appendingPathComponent("protocol/conformance-vectors/state-machine")
        if FileManager.default.fileExists(atPath: cwdVectors.path) {
            return cwdVectors
        }

        let parentVectors = cwd.appendingPathComponent("../protocol/conformance-vectors/state-machine")
        if FileManager.default.fileExists(atPath: parentVectors.path) {
            return parentVectors
        }

        return nil
    }

    func testReplayAllStateMachineConformanceVectors() throws {
        guard let vectorsDir = findConformanceVectorsDirectory() else {
            XCTFail("Could not locate protocol/conformance-vectors/state-machine directory")
            return
        }

        let fileManager = FileManager.default
        let fileURLs = try fileManager.contentsOfDirectory(at: vectorsDir, includingPropertiesForKeys: nil)
            .filter { $0.pathExtension == "json" }
            .filter { fileURL in
                let name = fileURL.lastPathComponent
                if let filter = ProcessInfo.processInfo.environment["SRUI_CONFORMANCE_VECTOR"] {
                    return name == filter
                }
                if let from = ProcessInfo.processInfo.environment["SRUI_CONFORMANCE_FROM"] {
                    if name < from { return false }
                }
                if let to = ProcessInfo.processInfo.environment["SRUI_CONFORMANCE_TO"] {
                    if name > to { return false }
                }
                return true
            }
            .sorted { $0.lastPathComponent < $1.lastPathComponent }

        XCTAssertFalse(fileURLs.isEmpty, "Found 0 conformance vector JSON files in \(vectorsDir.path)")
        print("Found \(fileURLs.count) conformance vector fixtures to replay:")

        for fileURL in fileURLs {
            print("  Replaying \(fileURL.lastPathComponent)...")
            try replayVectorFile(at: fileURL)
        }
    }

    private func replayVectorFile(at url: URL) throws {
        let data = try Data(contentsOf: url)
        let jsonObject = try JSONSerialization.jsonObject(with: data) as? [String: Any]
        let json = try XCTUnwrap(jsonObject, "Invalid JSON in \(url.lastPathComponent)")

        let fileName = url.lastPathComponent
        let initialRevision = (json["initial_revision"] as? NSNumber)?.uint64Value ?? 0

        // 1. Configure store limits
        let limits = parseLimits(json["initial_limits"] as? [String: Any] ?? json["store_limits"] as? [String: Any])
        let applier = TransactionApplier(limits: limits, initialRevision: Revision(initialRevision))

        // 2. Run setup transactions
        if let setupTxns = json["setup_transactions"] as? [[String: Any]] {
            for (idx, setupDict) in setupTxns.enumerated() {
                let txn = try parseTransaction(setupDict, fileName: fileName)
                let res = applier.apply(record: txn)
                guard case .success(let committedRev) = res else {
                    XCTFail("[\(fileName)] Setup transaction #\(idx) failed: \(res)")
                    return
                }
                XCTAssertEqual(
                    committedRev,
                    txn.newRevision,
                    "[\(fileName)] Setup txn #\(idx) committed revision mismatch"
                )
            }
        }

        // Snapshot pre-transaction state for rollback verification (matches Rust take_snapshot)
        let preTxnSnapshot = takeSnapshot(applier.store)

        // 3. Parse and apply test transaction
        let txnDict = try XCTUnwrap(json["transaction"] as? [String: Any], "[\(fileName)] Missing 'transaction' object")
        let transaction = try parseTransaction(txnDict, fileName: fileName)

        let outcomeDict = try XCTUnwrap(json["expected_outcome"] as? [String: Any], "[\(fileName)] Missing 'expected_outcome' object")
        let status = try XCTUnwrap(outcomeDict["status"] as? String, "[\(fileName)] Missing 'status' in outcome")

        let result = applier.apply(record: transaction)

        switch status {
        case "success":
            guard case .success(let committedRev) = result else {
                XCTFail("[\(fileName)] Expected success, got rejection: \(result)")
                return
            }

            if let expRev = (outcomeDict["committed_revision"] as? NSNumber)?.uint64Value {
                XCTAssertEqual(committedRev.value, expRev, "[\(fileName)] Committed revision mismatch")
                XCTAssertEqual(applier.store.revision.value, expRev, "[\(fileName)] Store revision mismatch")
            }

            if let stateDict = outcomeDict["store_state"] as? [String: Any] {
                try verifyStoreState(applier.store, expectedState: stateDict, fileName: fileName)
            }

        case "rejected":
            guard case .failure(let txnError) = result else {
                XCTFail("[\(fileName)] Expected rejection, got success: \(result)")
                return
            }

            let expErrorCode = try XCTUnwrap(outcomeDict["error_code"] as? String, "[\(fileName)] Missing error_code in rejected outcome")
            let actualErrorCode = txnError.conformanceCode

            XCTAssertEqual(
                actualErrorCode,
                expErrorCode,
                "[\(fileName)] Error code mismatch (expected \(expErrorCode), got \(String(describing: actualErrorCode)))"
            )

            if let expFailedOp = outcomeDict["failed_op_index"] as? Int {
                if case .opFailed(let opIdx, _) = txnError {
                    XCTAssertEqual(opIdx, expFailedOp, "[\(fileName)] Failed operation index mismatch")
                }
            }

            if let expStoreRev = (outcomeDict["expected_store_revision"] as? NSNumber)?.uint64Value {
                XCTAssertEqual(
                    applier.store.revision.value,
                    expStoreRev,
                    "[\(fileName)] Store revision after rejection mismatch"
                )
            }

            if let rollbackVerified = outcomeDict["rollback_verified"] as? Bool, rollbackVerified {
                let postTxnSnapshot = takeSnapshot(applier.store)
                XCTAssertEqual(
                    postTxnSnapshot,
                    preTxnSnapshot,
                    "[\(fileName)] Store state was mutated despite transaction rollback"
                )
            }

        default:
            XCTFail("[\(fileName)] Unknown outcome status: \(status)")
        }
    }

    // MARK: - Store Snapshot (rollback parity with Rust take_snapshot)

    private struct NodeSnapshot: Equatable {
        let nodeType: TypeRef
        let parentID: NodeId?
        let orderedChildren: [NodeId]
        let properties: [PropertyRef: Value]
    }

    private struct ModelSnapshot: Equatable {
        let modelType: TypeRef
        let itemCount: UInt64
        let cachedItemCount: Int
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
                models[modelID] = ModelSnapshot(
                    modelType: model.modelType,
                    itemCount: model.itemCount,
                    cachedItemCount: model.cachedItemCount
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

    // MARK: - State Verifications

    private func verifyStoreState(_ store: SemanticStore, expectedState: [String: Any], fileName: String) throws {
        if let expNodeCount = expectedState["node_count"] as? Int {
            XCTAssertEqual(store.nodeCount, expNodeCount, "[\(fileName)] Store nodeCount mismatch")
        }

        if let expRoots = expectedState["roots"] as? [NSNumber] {
            let actualRoots = store.rootIDs.map { $0.value }
            let expectedRootValues = expRoots.map { $0.uint64Value }
            XCTAssertEqual(actualRoots, expectedRootValues, "[\(fileName)] Roots mismatch")
        }

        if let nodesDict = expectedState["nodes"] as? [String: [String: Any]] {
            for (nodeIDStr, expNode) in nodesDict {
                guard let nodeIDVal = UInt64(nodeIDStr) else { continue }
                let nodeID = NodeId(nodeIDVal)
                let node = try XCTUnwrap(store.getNode(nodeID), "[\(fileName)] Node \(nodeID) missing from store")

                if let expTypeStr = expNode["node_type"] as? String {
                    let expType = try resolveNodeType(expTypeStr)
                    XCTAssertEqual(node.nodeType, expType, "[\(fileName)] Node \(nodeID) type mismatch")
                }

                if let hasParent = expNode["parent_id"] {
                    if let pidNum = hasParent as? NSNumber {
                        XCTAssertEqual(node.parentID, NodeId(pidNum.uint64Value), "[\(fileName)] Node \(nodeID) parent mismatch")
                    } else if hasParent is NSNull {
                        XCTAssertNil(node.parentID, "[\(fileName)] Node \(nodeID) should have nil parent")
                    }
                }

                if let expChildren = expNode["ordered_children"] as? [NSNumber] {
                    let expChildIDs = expChildren.map { NodeId($0.uint64Value) }
                    XCTAssertEqual(node.orderedChildren, expChildIDs, "[\(fileName)] Node \(nodeID) ordered_children mismatch")
                }

                if let expProps = expNode["properties"] as? [String: Any] {
                    for (propName, propValJson) in expProps {
                        let propRef = try resolvePropertyName(propName)
                        let expVal = try convertValue(propValJson)
                        let actualVal = try XCTUnwrap(node.getProperty(propRef), "[\(fileName)] Property \(propName) missing on node \(nodeID)")
                        XCTAssertEqual(actualVal, expVal, "[\(fileName)] Property \(propName) value mismatch on node \(nodeID)")
                    }
                }
            }
        }

        if let expModelCount = expectedState["model_count"] as? Int {
            XCTAssertEqual(store.modelCount, expModelCount, "[\(fileName)] Store modelCount mismatch")
        }

        if let modelsDict = expectedState["models"] as? [String: [String: Any]] {
            for (modelIDStr, expModel) in modelsDict {
                guard let modelIDVal = UInt64(modelIDStr) else { continue }
                let modelID = ModelId(modelIDVal)
                let model = try XCTUnwrap(store.getModel(modelID), "[\(fileName)] Model \(modelID) missing from store")

                if let expItemCount = (expModel["item_count"] as? NSNumber)?.uint64Value {
                    XCTAssertEqual(model.itemCount, expItemCount, "[\(fileName)] Model \(modelID) itemCount mismatch")
                }

                if let expCachedCount = expModel["cached_item_count"] as? Int {
                    XCTAssertEqual(model.cachedItemCount, expCachedCount, "[\(fileName)] Model \(modelID) cachedItemCount mismatch")
                }

                if let expRanges = expModel["cached_ranges"] as? [[String: Any]] {
                    let actualRanges = model.cachedRanges()
                    XCTAssertEqual(actualRanges.count, expRanges.count, "[\(fileName)] Model \(modelID) cached_ranges count mismatch")
                    for (i, rJson) in expRanges.enumerated() {
                        if i < actualRanges.count {
                            let start = (rJson["start"] as? NSNumber)?.uint64Value ?? 0
                            let length = (rJson["length"] as? NSNumber)?.uint64Value ?? 0
                            XCTAssertEqual(actualRanges[i].start, start, "[\(fileName)] Model \(modelID) range [\(i)] start mismatch")
                            XCTAssertEqual(actualRanges[i].length, length, "[\(fileName)] Model \(modelID) range [\(i)] length mismatch")
                        }
                    }
                }

                if let itemsDict = expModel["items"] as? [String: [String: Any]] {
                    for (idxStr, expItem) in itemsDict {
                        guard let idx = UInt64(idxStr) else { continue }
                        let item = try XCTUnwrap(model.getItemByIndex(idx), "[\(fileName)] Item at index \(idx) missing in model \(modelID)")
                        if let expItemId = (expItem["item_id"] as? NSNumber)?.uint64Value {
                            XCTAssertEqual(item.itemID.value, expItemId, "[\(fileName)] Item ID mismatch at index \(idx)")
                        }
                        if let expValJson = expItem["value"] {
                            let expVal = try convertValue(expValJson)
                            XCTAssertEqual(item.value, expVal, "[\(fileName)] Item value mismatch at index \(idx)")
                        }
                    }
                }
            }
        }
    }

    // MARK: - Parsers and Converters

    private func parseLimits(_ dict: [String: Any]?) -> StoreLimits {
        guard let d = dict else { return StoreLimits() }
        var limits = StoreLimits()
        if let v = d["max_tree_depth"] as? Int { limits.maxTreeDepth = v }
        if let v = d["max_node_count"] as? Int { limits.maxNodeCount = v }
        if let v = d["max_transaction_operations"] as? Int { limits.maxTransactionOperations = v }
        if let v = d["max_string_length"] as? Int { limits.maxStringLength = v }
        if let v = d["max_value_depth"] as? Int { limits.maxValueDepth = v }
        if let v = d["max_list_length"] as? Int ?? d["max_list_elements"] as? Int { limits.maxListElements = v }
        if let v = d["max_record_properties"] as? Int { limits.maxRecordProperties = v }
        if let v = d["max_model_count"] as? Int { limits.maxModelCount = v }
        if let v = d["max_cached_items_per_model"] as? Int { limits.maxCachedItemsPerModel = v }
        if let v = d["max_items_per_model_operation"] as? Int { limits.maxItemsPerModelOperation = v }
        return limits
    }

    private func parseTransaction(_ dict: [String: Any], fileName: String) throws -> Transaction {
        let baseRev = (dict["base_revision"] as? NSNumber)?.uint64Value ?? 0
        let newRev = (dict["new_revision"] as? NSNumber)?.uint64Value ?? (baseRev + 1)
        let priority = (dict["priority"] as? NSNumber)?.uint32Value ?? 0

        var ops: [StoreOperation] = []
        if let rawOps = dict["operations"] as? [[String: Any]] {
            for rawOp in rawOps {
                ops.append(try parseOperation(rawOp, fileName: fileName))
            }
        }

        return Transaction(
            baseRevision: Revision(baseRev),
            newRevision: Revision(newRev),
            operations: ops,
            priority: priority
        )
    }

    private func parseOperation(_ dict: [String: Any], fileName: String) throws -> StoreOperation {
        let opType = try XCTUnwrap(dict["type"] as? String, "[\(fileName)] Operation missing 'type'")

        switch opType {
        case "CREATE_NODE":
            let nodeID = NodeId(try getUInt64(dict, key: "node_id", fileName: fileName))
            let nodeTypeStr = try XCTUnwrap(dict["node_type"] as? String, "[\(fileName)] CREATE_NODE missing node_type")
            let nodeType = try resolveNodeType(nodeTypeStr)
            let parentID = (dict["parent_id"] as? NSNumber).map { NodeId($0.uint64Value) }
            let childIndex = dict["child_index"] as? Int

            let properties = try parseFixtureProperties(dict["properties"], fileName: fileName)
            return .createNode(
                id: nodeID,
                nodeType: nodeType,
                parentID: parentID,
                childIndex: childIndex,
                properties: properties
            )

        case "DELETE_NODE":
            let nodeID = NodeId(try getUInt64(dict, key: "node_id", fileName: fileName))
            return .deleteNode(id: nodeID)

        case "SET_PROPERTY":
            let nodeID = NodeId(try getUInt64(dict, key: "node_id", fileName: fileName))
            let propName = try XCTUnwrap(dict["property"] as? String, "[\(fileName)] SET_PROPERTY missing property")
            let propRef = try resolvePropertyName(propName)
            let val = try convertValue(dict["value"])
            return .setProperty(id: nodeID, property: propRef, value: val)

        case "CLEAR_PROPERTY":
            let nodeID = NodeId(try getUInt64(dict, key: "node_id", fileName: fileName))
            let propName = try XCTUnwrap(dict["property"] as? String, "[\(fileName)] CLEAR_PROPERTY missing property")
            let propRef = try resolvePropertyName(propName)
            return .clearProperty(id: nodeID, property: propRef)

        case "MOVE_NODE":
            let nodeID = NodeId(try getUInt64(dict, key: "node_id", fileName: fileName))
            let newParentID = (dict["new_parent_id"] as? NSNumber).map { NodeId($0.uint64Value) }
            let newChildIndex = dict["new_child_index"] as? Int
            return .moveNode(id: nodeID, newParentID: newParentID, newChildIndex: newChildIndex)

        case "REORDER_CHILDREN":
            let parentID = NodeId(try getUInt64(dict, key: "parent_id", fileName: fileName))
            let newOrderNums = try XCTUnwrap(dict["new_order"] as? [NSNumber], "[\(fileName)] REORDER_CHILDREN missing new_order")
            let newOrder = newOrderNums.map { NodeId($0.uint64Value) }
            return .reorderChildren(parentID: parentID, newOrder: newOrder)

        case "BATCH_PROPERTY_SET":
            let nodeID = NodeId(try getUInt64(dict, key: "node_id", fileName: fileName))
            let properties = try parseFixtureProperties(dict["properties"], fileName: fileName)
            return .batchPropertySet(id: nodeID, properties: properties)

        case "CREATE_MODEL":
            let modelID = ModelId(try getUInt64(dict, key: "model_id", fileName: fileName))
            let modelTypeStr = try XCTUnwrap(dict["model_type"] as? String, "[\(fileName)] CREATE_MODEL missing model_type")
            let modelType = try resolveNodeType(modelTypeStr)
            let itemCount = try getUInt64(dict, key: "item_count", fileName: fileName)
            return .createModel(id: modelID, modelType: modelType, itemCount: itemCount)

        case "MODEL_INSERT":
            let modelID = ModelId(try getUInt64(dict, key: "model_id", fileName: fileName))
            let index = try getUInt64(dict, key: "index", fileName: fileName)
            let rawItems = try XCTUnwrap(dict["items"] as? [[String: Any]], "[\(fileName)] MODEL_INSERT missing items")
            let items = try rawItems.map { try convertModelItem($0) }
            return .modelInsert(id: modelID, index: index, items: items)

        case "MODEL_DELETE":
            let modelID = ModelId(try getUInt64(dict, key: "model_id", fileName: fileName))
            let index = (dict["index"] as? NSNumber)?.uint64Value
            let count = (dict["count"] as? NSNumber)?.uint64Value
            let itemIds = (dict["item_ids"] as? [NSNumber])?.map { ItemId($0.uint64Value) } ?? []
            return .modelDelete(id: modelID, index: index, count: count, itemIds: itemIds)

        case "MODEL_UPDATE":
            let modelID = ModelId(try getUInt64(dict, key: "model_id", fileName: fileName))
            let index = (dict["index"] as? NSNumber)?.uint64Value
            let rawItems = try XCTUnwrap(dict["items"] as? [[String: Any]], "[\(fileName)] MODEL_UPDATE missing items")
            let items = try rawItems.map { try convertModelItem($0) }
            return .modelUpdate(id: modelID, index: index, items: items)

        case "MODEL_RESET_RANGE":
            let modelID = ModelId(try getUInt64(dict, key: "model_id", fileName: fileName))
            let startIndex = try getUInt64(dict, key: "start_index", fileName: fileName)
            let totalCount = (dict["total_count"] as? NSNumber)?.uint64Value
            let rawItems = try XCTUnwrap(dict["items"] as? [[String: Any]], "[\(fileName)] MODEL_RESET_RANGE missing items")
            let items = try rawItems.map { try convertModelItem($0) }
            return .modelResetRange(id: modelID, startIndex: startIndex, items: items, totalCount: totalCount)

        default:
            throw StoreError.operationError("Unsupported operation type in fixture: \(opType)")
        }
    }

    private func convertModelItem(_ dict: [String: Any]) throws -> ModelItem {
        let itemIDNum = try XCTUnwrap(dict["item_id"] as? NSNumber, "ModelItem missing item_id")
        let itemID = ItemId(itemIDNum.uint64Value)
        let val = try convertValue(dict["value"])
        var properties: [PropertyRef: Value] = [:]
        if let propsDict = dict["properties"] as? [String: Any] {
            for (k, v) in propsDict {
                let propRef = try resolvePropertyName(k)
                properties[propRef] = try convertValue(v)
            }
        }
        return ModelItem(itemID: itemID, value: val, properties: properties)
    }

    private func getUInt64(_ dict: [String: Any], key: String, fileName: String) throws -> UInt64 {
        guard let num = dict[key] as? NSNumber else {
            throw StoreError.operationError("[\(fileName)] Missing or invalid numeric field: \(key)")
        }
        return num.uint64Value
    }

    private func resolveNodeType(_ name: String) throws -> TypeRef {
        if let standard = try? resolveStandardNodeType(name).get() {
            return standard
        }
        if let num = UInt32(name) {
            return TypeRef.standard(num)
        }
        throw StoreError.operationError("Unknown node type: \(name)")
    }

    private func resolvePropertyName(_ name: String) throws -> PropertyRef {
        try resolvePropertyRef(name)
    }

    private func resolvePropertyRef(_ raw: Any) throws -> PropertyRef {
        if let name = raw as? String {
            if let standard = try? resolveStandardProperty(name).get() {
                return standard
            }
            if let num = UInt32(name) {
                return PropertyRef.standard(num)
            }
            if name == "model_ref" {
                return PropertyRef.modelRef
            }
            return PropertyRef(namespaceID: standardNamespaceID, localID: UInt32(bitPattern: Int32(name.hashValue)))
        }

        if let dict = raw as? [String: Any] {
            let namespaceID = (dict["namespace_id"] as? NSNumber)?.uint32Value ?? standardNamespaceID
            guard let localID = (dict["local_id"] as? NSNumber)?.uint32Value else {
                throw StoreError.operationError("property_ref object missing numeric local_id field")
            }
            return PropertyRef(namespaceID: namespaceID, localID: localID)
        }

        if let num = raw as? NSNumber {
            return PropertyRef.standard(num.uint32Value)
        }

        throw StoreError.operationError("Invalid property_ref JSON: \(raw)")
    }

    private func parseFixtureProperties(_ raw: Any?, fileName: String) throws -> [Property] {
        guard let raw else { return [] }

        if let propsDict = raw as? [String: Any] {
            var properties: [Property] = []
            for (k, v) in propsDict {
                let propRef = try resolvePropertyName(k)
                let val = try convertValue(v)
                properties.append(Property(property: propRef, value: val))
            }
            return properties
        }

        if let propsArray = raw as? [[String: Any]] {
            var properties: [Property] = []
            for item in propsArray {
                guard let propertyRaw = item["property"] else {
                    throw StoreError.operationError("[\(fileName)] property field missing in property item")
                }
                guard let valueRaw = item["value"] else {
                    throw StoreError.operationError("[\(fileName)] value field missing in property item")
                }
                let propRef = try resolvePropertyRef(propertyRaw)
                let val = try convertValue(valueRaw)
                properties.append(Property(property: propRef, value: val))
            }
            return properties
        }

        throw StoreError.operationError("[\(fileName)] Invalid properties JSON: \(raw)")
    }

    private func convertValue(_ raw: Any?) throws -> Value {
        guard let raw = raw, !(raw is NSNull) else {
            return .null
        }

        if let b = raw as? Bool {
            return .bool(b)
        }

        if let s = raw as? String {
            return .string(s)
        }

        if let num = raw as? NSNumber {
            if CFGetTypeID(num) == CFBooleanGetTypeID() {
                return .bool(num.boolValue)
            }
            let objCType = String(cString: num.objCType)
            if objCType == "d" || objCType == "f" {
                return .float64(num.doubleValue)
            }
            let stringValue = num.stringValue
            if stringValue.contains(".") || stringValue.contains("e") || stringValue.contains("E") {
                return .float64(num.doubleValue)
            }
            // Avoid `intValue` — it traps on arm64 when the magnitude exceeds Int.max.
            if stringValue.hasPrefix("-") {
                return .signedInt(num.int64Value)
            }
            return .unsignedInt(num.uint64Value)
        }

        if let dict = raw as? [String: Any] {
            return try convertStructuredValue(dict)
        }

        if let list = raw as? [Any] {
            let items = try list.map { try convertValue($0) }
            return .list(items)
        }

        return .string(String(describing: raw))
    }

    private func convertStructuredValue(_ dict: [String: Any]) throws -> Value {
        var map = dict

        if let enumName = map.removeValue(forKey: "enum") as? String,
           let variant = map.removeValue(forKey: "value") as? String {
            if let token = EnumToken.resolveStandard(enumName: enumName, valueName: variant) {
                return .enumToken(token)
            }
            return .string(variant)
        }

        if let enumID = map.removeValue(forKey: "enum_id") as? NSNumber,
           let valueID = map.removeValue(forKey: "value_id") as? NSNumber {
            return .enumToken(EnumToken(enumID: enumID.uint32Value, valueID: valueID.uint32Value))
        }

        if let nodeID = map.removeValue(forKey: "node_id") as? NSNumber {
            return .nodeID(NodeId(nodeID.uint64Value))
        }

        if let itemID = map.removeValue(forKey: "item_id") as? NSNumber {
            return .itemID(ItemId(itemID.uint64Value))
        }

        if let hashStr = map.removeValue(forKey: "resource_hash") as? String {
            return .resourceHash(try ResourceHash(hex: hashStr))
        }

        if let x = map["x"] as? NSNumber,
           let y = map["y"] as? NSNumber,
           let width = map["width"] as? NSNumber,
           let height = map["height"] as? NSNumber {
            return .rect(Rect(
                x: x.doubleValue,
                y: y.doubleValue,
                width: width.doubleValue,
                height: height.doubleValue
            ))
        }

        if let width = map.removeValue(forKey: "width") as? NSNumber,
           let height = map.removeValue(forKey: "height") as? NSNumber {
            return .size(Size(width: width.doubleValue, height: height.doubleValue))
        }

        if let x = map.removeValue(forKey: "x") as? NSNumber,
           let y = map.removeValue(forKey: "y") as? NSNumber {
            return .point(Point(x: x.doubleValue, y: y.doubleValue))
        }

        if let start = map.removeValue(forKey: "start") as? NSNumber,
           let length = map.removeValue(forKey: "length") as? NSNumber {
            return .range(SemanticRange(start: start.uint64Value, length: length.uint64Value))
        }

        if let top = map.removeValue(forKey: "top") as? NSNumber,
           let leading = map.removeValue(forKey: "leading") as? NSNumber,
           let bottom = map.removeValue(forKey: "bottom") as? NSNumber,
           let trailing = map.removeValue(forKey: "trailing") as? NSNumber {
            return .edgeInsets(EdgeInsets(
                top: top.doubleValue,
                leading: leading.doubleValue,
                bottom: bottom.doubleValue,
                trailing: trailing.doubleValue
            ))
        }

        if let recordTypeRaw = map.removeValue(forKey: "record_type"),
           let propsRaw = map.removeValue(forKey: "properties") {
            let typeRef = try resolveNodeType(recordTypeRaw as? String ?? String(describing: recordTypeRaw))
            guard let propsDict = propsRaw as? [String: Any] else {
                throw StoreError.operationError("record properties must be an object")
            }
            let properties = try propsDict
                .map { key, value in
                    Property(
                        property: try resolvePropertyName(key),
                        value: try convertValue(value)
                    )
                }
                .sorted { $0.property < $1.property }
            return .record(SmallRecord(typeRef: typeRef, properties: properties))
        }
        throw StoreError.operationError("Unrecognized structured value object in fixture: \(dict)")
    }
    // MARK: - Invariant Verification (§4.7, §4.16, §4.17, §7.1, §10, §32.3)

    /// Verifies the semantic-not-paint invariant: standard widget profile trees define portable meaning,
    /// roles, and layout intent without display frames, paint instructions, or mandatory absolute pixel geometry.
    func testSemanticNotPaintArchitecturalInvariants() {
        // Assert standard registry tables contain zero display frame, paint command, or absolute pixel coordinate concepts
        let forbiddenConcepts = [
            "paint", "draw_rect", "draw_line", "fill_path", "rasterize",
            "framebuffer", "pixel_buffer", "render_pass", "gpu_texture",
            "display_list", "paint_layer", "skia_canvas"
        ]

        for entry in standardNodeTypesTable {
            let name = entry.name.lowercased()
            for forbidden in forbiddenConcepts {
                XCTAssertFalse(
                    name.contains(forbidden),
                    "Standard node type '\(entry.name)' violates semantic-not-paint invariant by containing forbidden paint keyword '\(forbidden)' (§4.7, §32.3)"
                )
            }
        }

        for entry in standardPropertiesTable {
            let name = entry.name.lowercased()
            for forbidden in forbiddenConcepts {
                XCTAssertFalse(
                    name.contains(forbidden),
                    "Standard property '\(entry.name)' violates semantic-not-paint invariant by containing forbidden paint keyword '\(forbidden)' (§4.7, §32.3)"
                )
            }
        }
    }
}
