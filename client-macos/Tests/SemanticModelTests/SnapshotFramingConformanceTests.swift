//
// SnapshotFramingConformanceTests.swift
// SemanticModelTests
//
// §32 suite 8 (reconnect): multi-envelope snapshot delivery vectors (§12.1, §18, §26, §32.8).
//
// Replays protocol/conformance-vectors/suites/08-reconnect/vectors through the Swift replica:
// `SnapshotAssembler` stages the announced envelopes, and the reassembled snapshot is decoded under
// the client's §26 limits and applied through `prepareResyncSnapshot`/`publishResyncSnapshot`, the
// same entry points `SessionController` uses. The Rust replica replays the same files in
// server-rust/semantic-tree/tests/conformance_snapshot_framing_test.rs, so both apply a delivered
// snapshot identically or reject it with the same error at the same envelope.
//

import Foundation
import XCTest

@testable import Protocol
@testable import SemanticModel

final class SnapshotFramingConformanceTests: XCTestCase {
    private enum Observed {
        case applied(SemanticStore)
        case rejected(code: String, atEnvelope: Int?)
        case incomplete(received: Int)
    }

    func testReplaySnapshotFramingVectors() throws {
        let files = try ConformanceVectors.vectors(forSuite: 8)
        XCTAssertFalse(files.isEmpty)
        for url in files {
            let vector = try XCTUnwrap(
                JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: Any])
            let name = try XCTUnwrap(vector["name"] as? String)
            let expected = try XCTUnwrap(vector["expected_outcome"] as? [String: Any])
            let status = try XCTUnwrap(expected["status"] as? String)

            switch (status, try replay(vector)) {
            case ("applied", .applied(let store)):
                try assertStore(name, store, expected)
            case ("rejected", .rejected(let code, let atEnvelope)):
                XCTAssertEqual(code, expected["error"] as? String, "\(name): error")
                XCTAssertEqual(
                    atEnvelope, (expected["at_envelope"] as? NSNumber)?.intValue,
                    "\(name): rejected envelope")
            case ("incomplete", .incomplete(let received)):
                XCTAssertEqual(
                    received, (expected["received_envelopes"] as? NSNumber)?.intValue, name)
            case (_, let observed):
                XCTFail("\(name): expected \(status), observed \(observed)")
            }
        }
    }

    // MARK: - Replay

    private func replay(_ vector: [String: Any]) throws -> Observed {
        let limits = try XCTUnwrap(vector["client_limits"] as? [String: Any])
        let decision = try XCTUnwrap(vector["decision"] as? [String: Any])
        let maxOperations = try int(limits, "max_transaction_operations")
        var assembler: SnapshotAssembler
        do {
            assembler = try SnapshotAssembler(
                snapshotRevision: UInt64(try int(decision, "snapshot_revision")),
                announcedParts: UInt32(try int(decision, "snapshot_parts")),
                maxParts: UInt32(try int(limits, "max_snapshot_parts")),
                maxOperations: maxOperations
            )
        } catch let error as SnapshotAssemblyError {
            return .rejected(code: error.code, atEnvelope: nil)
        }

        let envelopes = try XCTUnwrap(vector["envelopes"] as? [[String: Any]])
        for (index, envelope) in envelopes.enumerated() {
            let snapshot: SRUITransaction?
            do {
                snapshot = try assembler.accept(try wireEnvelope(envelope))
            } catch let error as SnapshotAssemblyError {
                return .rejected(code: error.code, atEnvelope: index)
            }
            guard let snapshot else { continue }
            XCTAssertEqual(index + 1, envelopes.count, "snapshot completed before its last envelope")

            let storeLimits = StoreLimits(maxTransactionOperations: maxOperations)
            let record = try ProtocolDecoder(limits: storeLimits)
                .validateAndConvertTransaction(wire: snapshot)
            let applier = TransactionApplier(limits: storeLimits)
            let prepared = try applier.prepareResyncSnapshot(record: record).get()
            let committed = try applier.publishResyncSnapshot(prepared).get()
            return .applied(committed.store)
        }
        return .incomplete(received: envelopes.count)
    }

    private func int(_ object: [String: Any], _ key: String) throws -> Int {
        try XCTUnwrap((object[key] as? NSNumber)?.intValue, "missing integer \(key)")
    }

    private func wireString(_ text: String) -> SRUIValue {
        var value = SRUIValue()
        value.stringValue = text
        return value
    }

    private func standardType(_ localID: Int) -> SRUITypeRef {
        var type = SRUITypeRef()
        type.namespaceID = 0
        type.localID = UInt32(localID)
        return type
    }

    private func wireOperation(_ json: [String: Any]) throws -> SRUIOperation {
        var op = SRUIOperation()
        if let node = json["create_node"] as? [String: Any] {
            var record = SRUINodeRecord()
            record.nodeID = UInt64(try int(node, "node_id"))
            record.type = standardType(try int(node, "type"))
            record.parentID = UInt64(try int(node, "parent_id"))
            record.childIndex = UInt32(try int(node, "child_index"))
            record.properties = try XCTUnwrap(node["properties"] as? [[String: Any]]).map {
                var property = SRUIProperty()
                property.property.namespaceID = 0
                property.property.localID = UInt32(try int($0, "property"))
                property.value = wireString(try XCTUnwrap($0["string"] as? String))
                return property
            }
            var create = Srui_Protocol_CreateNodeOp()
            create.node = record
            op.createNode = create
        } else if let model = json["create_model"] as? [String: Any] {
            var create = SRUICreateModelOp()
            create.modelID = UInt64(try int(model, "model_id"))
            create.modelType = standardType(try int(model, "model_type"))
            create.itemCount = UInt64(try int(model, "item_count"))
            op.createModel = create
        } else if let range = json["model_reset_range"] as? [String: Any] {
            var reset = SRUIModelResetRangeOp()
            reset.modelID = UInt64(try int(range, "model_id"))
            reset.startIndex = UInt64(try int(range, "start_index"))
            reset.items = try XCTUnwrap(range["items"] as? [[String: Any]]).map {
                var item = SRUIModelItem()
                item.itemID = UInt64(try int($0, "item_id"))
                item.value = wireString(try XCTUnwrap($0["string"] as? String))
                return item
            }
            op.modelResetRange = reset
        } else {
            XCTFail("unknown vector operation \(json)")
        }
        return op
    }

    private func wireEnvelope(_ json: [String: Any]) throws -> SRUITransaction {
        var tx = SRUITransaction()
        tx.baseRevision = UInt64(try int(json, "base_revision"))
        tx.newRevision = UInt64(try int(json, "new_revision"))
        tx.operations = try XCTUnwrap(json["operations"] as? [[String: Any]]).map(wireOperation)
        return tx
    }

    // MARK: - Expected state

    private func stringProperties(_ json: Any?) throws -> [PropertyRef: Value] {
        var properties: [PropertyRef: Value] = [:]
        for entry in try XCTUnwrap(json as? [[String: Any]]) {
            properties[PropertyRef(namespaceID: 0, localID: UInt32(try int(entry, "property")))] =
                .string(try XCTUnwrap(entry["string"] as? String))
        }
        return properties
    }

    private func ids(_ json: Any?) throws -> [UInt64] {
        try XCTUnwrap(json as? [NSNumber]).map(\.uint64Value)
    }

    private func assertStore(
        _ name: String, _ store: SemanticStore, _ expected: [String: Any]
    ) throws {
        XCTAssertEqual(store.revision.value, UInt64(try int(expected, "revision")), "\(name): revision")
        XCTAssertEqual(store.rootIDs.map(\.value), try ids(expected["root_ids"]), "\(name): roots")

        let nodes = try XCTUnwrap(expected["nodes"] as? [[String: Any]])
        XCTAssertEqual(store.nodeCount, nodes.count, "\(name): node count")
        for want in nodes {
            let id = NodeId(UInt64(try int(want, "node_id")))
            let node = try XCTUnwrap(store.getNode(id), "\(name): node \(id)")
            XCTAssertEqual(node.nodeType, TypeRef(namespaceID: 0, localID: UInt32(try int(want, "type"))))
            XCTAssertEqual(node.parentID?.value ?? 0, UInt64(try int(want, "parent_id")), "\(name)")
            XCTAssertEqual(node.orderedChildren.map(\.value), try ids(want["children"]), "\(name)")
            XCTAssertEqual(node.properties, try stringProperties(want["properties"]), "\(name)")
        }

        let models = try XCTUnwrap(expected["models"] as? [[String: Any]])
        XCTAssertEqual(store.modelCount, models.count, "\(name): model count")
        for want in models {
            let model = try XCTUnwrap(store.getModel(ModelId(UInt64(try int(want, "model_id")))))
            XCTAssertEqual(
                model.modelType, TypeRef(namespaceID: 0, localID: UInt32(try int(want, "model_type"))))
            XCTAssertEqual(model.itemCount, UInt64(try int(want, "item_count")), "\(name)")
            let items = try XCTUnwrap(want["items"] as? [[String: Any]])
            XCTAssertEqual(model.cachedItemCount, items.count, "\(name): cached items")
            for item in items {
                let got = try XCTUnwrap(model.getItemByIndex(UInt64(try int(item, "index"))))
                XCTAssertEqual(got.itemID, ItemId(UInt64(try int(item, "item_id"))), "\(name)")
                XCTAssertEqual(got.value, .string(try XCTUnwrap(item["string"] as? String)), "\(name)")
            }
        }
    }
}
