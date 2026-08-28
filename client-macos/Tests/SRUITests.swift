import XCTest
import Foundation
import CryptoKit
@testable import TransportSSH
@testable import Protocol
@testable import SemanticModel
@testable import Session
@testable import RendererAppKit
@testable import Collections
@testable import Text
@testable import Terminal
@testable import Resources
@testable import Accessibility

final class SRUITests: XCTestCase {
    private var conformanceVectorsDir: URL {
        let testFileURL = URL(fileURLWithPath: #filePath)
        let repoRoot = testFileURL
            .deletingLastPathComponent() // Tests
            .deletingLastPathComponent() // client-macos
            .deletingLastPathComponent() // repo root
        return repoRoot.appendingPathComponent("protocol/conformance-vectors")
    }

    private func loadExpectedSpec() throws -> [String: Any] {
        let specURL = conformanceVectorsDir.appendingPathComponent("expected.json")
        let data = try Data(contentsOf: specURL)
        let json = try JSONSerialization.jsonObject(with: data) as? [String: Any]
        return json ?? [:]
    }

    private func hexString(from data: Data) -> String {
        return data.map { String(format: "%02x", $0) }.joined()
    }

    private func sha256String(from data: Data) -> String {
        let digest = SHA256.hash(data: data)
        return digest.map { String(format: "%02x", $0) }.joined()
    }

    private func createAuthoredNodeRecord() -> SRUINodeRecord {
        var node = SRUINodeRecord()
        node.nodeID = 42
        node.type = SRUITypeRef.with {
            $0.namespaceID = standardNamespaceID
            $0.localID = UInt32(Srui_Protocol_StandardNodeType.nodeTypeButton.rawValue)
        }
        node.parentID = 1
        node.childIndex = 0

        let p0 = SRUIProperty.with {
            $0.property = SRUIPropertyRef.with {
                $0.namespaceID = standardNamespaceID
                $0.localID = UInt32(Srui_Protocol_StandardProperty.propertyLabel.rawValue)
            }
            $0.value = SRUIValue.with {
                $0.stringValue = "Delete"
            }
        }

        let p1 = SRUIProperty.with {
            $0.property = SRUIPropertyRef.with {
                $0.namespaceID = standardNamespaceID
                $0.localID = UInt32(Srui_Protocol_StandardProperty.propertyRole.rawValue)
            }
            $0.value = SRUIValue.with {
                $0.enumValue = Srui_Protocol_EnumValue.with {
                    $0.enumID = UInt32(Srui_Protocol_StandardEnum.enumActionRole.rawValue)
                    $0.valueID = UInt32(Srui_Protocol_ActionRole.destructive.rawValue)
                }
            }
        }

        let p2 = SRUIProperty.with {
            $0.property = SRUIPropertyRef.with {
                $0.namespaceID = standardNamespaceID
                $0.localID = UInt32(Srui_Protocol_StandardProperty.propertyEnabled.rawValue)
            }
            $0.value = SRUIValue.with {
                $0.boolValue = true
            }
        }

        node.properties = [p0, p1, p2]
        return node
    }

    private func createAuthoredTransaction() -> SRUITransaction {
        var tx = SRUITransaction()
        tx.baseRevision = 104
        tx.newRevision = 105
        tx.priority = 1

        let createOp = SRUIOperation.with {
            $0.createNode = Srui_Protocol_CreateNodeOp.with {
                $0.node = SRUINodeRecord.with {
                    $0.nodeID = 19
                    $0.type = SRUITypeRef.with {
                        $0.namespaceID = standardNamespaceID
                        $0.localID = UInt32(Srui_Protocol_StandardNodeType.nodeTypeText.rawValue)
                    }
                    $0.parentID = 2
                    $0.childIndex = 3
                    $0.properties = [
                        SRUIProperty.with {
                            $0.property = SRUIPropertyRef.with {
                                $0.namespaceID = standardNamespaceID
                                $0.localID = UInt32(Srui_Protocol_StandardProperty.propertyText.rawValue)
                            }
                            $0.value = SRUIValue.with {
                                $0.stringValue = "27 tests passed"
                            }
                        }
                    ]
                }
            }
        }

        let setOp = SRUIOperation.with {
            $0.setProperty = Srui_Protocol_SetPropertyOp.with {
                $0.nodeID = 4
                $0.property = SRUIPropertyRef.with {
                    $0.namespaceID = standardNamespaceID
                    $0.localID = UInt32(Srui_Protocol_StandardProperty.propertyValue.rawValue)
                }
                $0.value = SRUIValue.with {
                    $0.floatValue = 0.71
                }
            }
        }

        let batchOp = SRUIOperation.with {
            $0.batchPropertySet = Srui_Protocol_BatchPropertySetOp.with {
                $0.nodeID = 19
                $0.properties = [
                    SRUIProperty.with {
                        $0.property = SRUIPropertyRef.with {
                            $0.namespaceID = standardNamespaceID
                            $0.localID = UInt32(Srui_Protocol_StandardProperty.propertyMinimumSize.rawValue)
                        }
                        $0.value = SRUIValue.with {
                            $0.sizeValue = Srui_Protocol_SizeVal.with {
                                $0.width = 120.0
                                $0.height = 24.0
                            }
                        }
                    }
                ]
            }
        }

        tx.operations = [createOp, setOp, batchOp]
        return tx
    }

    func testDecodeGoldenNodeRecordAgainstExpectedJSON() throws {
        let spec = try loadExpectedSpec()
        guard let vectors = spec["vectors"] as? [String: Any],
              let nodeSpec = vectors["golden_node_record"] as? [String: Any],
              let filename = nodeSpec["file"] as? String,
              let expectedHex = nodeSpec["hex"] as? String,
              let expectedSHA256 = nodeSpec["sha256"] as? String,
              let expectedByteLen = nodeSpec["byte_length"] as? Int,
              let expected = nodeSpec["expected"] as? [String: Any] else {
            XCTFail("Malformed expected.json structure for golden_node_record")
            return
        }

        let fileURL = conformanceVectorsDir.appendingPathComponent(filename)
        let data = try Data(contentsOf: fileURL)

        // 1. Assert raw bytes match canonical specification
        XCTAssertEqual(data.count, expectedByteLen, "Fixture byte length mismatch")
        XCTAssertEqual(hexString(from: data), expectedHex, "Fixture hex mismatch")
        XCTAssertEqual(sha256String(from: data), expectedSHA256, "Fixture SHA256 mismatch")

        // 2. Decode and assert against JSON oracle
        let node = try SRUINodeRecord(serializedBytes: data)

        XCTAssertEqual(node.nodeID, UInt64(expected["node_id"] as? Int ?? -1))
        XCTAssertEqual(node.parentID, UInt64(expected["parent_id"] as? Int ?? -1))
        XCTAssertEqual(node.childIndex, UInt32(expected["child_index"] as? Int ?? -1))

        let typeSpec = expected["type"] as? [String: Any] ?? [:]
        XCTAssertTrue(node.hasType)
        XCTAssertEqual(node.type.namespaceID, UInt32(typeSpec["namespace_id"] as? Int ?? -1))
        XCTAssertEqual(node.type.localID, UInt32(typeSpec["local_id"] as? Int ?? -1))

        let expectedProps = expected["properties"] as? [[String: Any]] ?? []
        XCTAssertEqual(node.properties.count, expectedProps.count)

        // Property 0: label
        let p0 = node.properties[0]
        let p0Spec = expectedProps[0]
        let p0PropSpec = p0Spec["property"] as? [String: Any] ?? [:]
        let p0ValSpec = p0Spec["value"] as? [String: Any] ?? [:]
        XCTAssertEqual(p0.property.namespaceID, UInt32(p0PropSpec["namespace_id"] as? Int ?? -1))
        XCTAssertEqual(p0.property.localID, UInt32(p0PropSpec["local_id"] as? Int ?? -1))
        guard case .stringValue(let str) = p0.value.value else {
            XCTFail("Expected stringValue for property 0")
            return
        }
        XCTAssertEqual(str, p0ValSpec["string_value"] as? String)

        // Property 1: role
        let p1 = node.properties[1]
        let p1Spec = expectedProps[1]
        let p1PropSpec = p1Spec["property"] as? [String: Any] ?? [:]
        let p1ValSpec = p1Spec["value"] as? [String: Any] ?? [:]
        let p1EnumSpec = p1ValSpec["enum_value"] as? [String: Any] ?? [:]
        XCTAssertEqual(p1.property.namespaceID, UInt32(p1PropSpec["namespace_id"] as? Int ?? -1))
        XCTAssertEqual(p1.property.localID, UInt32(p1PropSpec["local_id"] as? Int ?? -1))
        guard case .enumValue(let ev) = p1.value.value else {
            XCTFail("Expected enumValue for property 1")
            return
        }
        XCTAssertEqual(ev.enumID, UInt32(p1EnumSpec["enum_id"] as? Int ?? -1))
        XCTAssertEqual(ev.valueID, UInt32(p1EnumSpec["value_id"] as? Int ?? -1))

        // Property 2: enabled
        let p2 = node.properties[2]
        let p2Spec = expectedProps[2]
        let p2PropSpec = p2Spec["property"] as? [String: Any] ?? [:]
        let p2ValSpec = p2Spec["value"] as? [String: Any] ?? [:]
        XCTAssertEqual(p2.property.namespaceID, UInt32(p2PropSpec["namespace_id"] as? Int ?? -1))
        XCTAssertEqual(p2.property.localID, UInt32(p2PropSpec["local_id"] as? Int ?? -1))
        guard case .boolValue(let b) = p2.value.value else {
            XCTFail("Expected boolValue for property 2")
            return
        }
        XCTAssertEqual(b, p2ValSpec["bool_value"] as? Bool)

        // 3. Re-encode and verify identical wire bytes
        let roundtripData = try node.serializedData()
        XCTAssertEqual(roundtripData, data, "Roundtrip re-encode mismatch")
    }

    func testDecodeGoldenTransactionAgainstExpectedJSON() throws {
        let spec = try loadExpectedSpec()
        guard let vectors = spec["vectors"] as? [String: Any],
              let txSpec = vectors["golden_transaction"] as? [String: Any],
              let filename = txSpec["file"] as? String,
              let expectedHex = txSpec["hex"] as? String,
              let expectedSHA256 = txSpec["sha256"] as? String,
              let expectedByteLen = txSpec["byte_length"] as? Int,
              let expected = txSpec["expected"] as? [String: Any] else {
            XCTFail("Malformed expected.json structure for golden_transaction")
            return
        }

        let fileURL = conformanceVectorsDir.appendingPathComponent(filename)
        let data = try Data(contentsOf: fileURL)

        // 1. Assert raw bytes match canonical specification
        XCTAssertEqual(data.count, expectedByteLen, "Fixture byte length mismatch")
        XCTAssertEqual(hexString(from: data), expectedHex, "Fixture hex mismatch")
        XCTAssertEqual(sha256String(from: data), expectedSHA256, "Fixture SHA256 mismatch")

        // 2. Decode and assert against JSON oracle
        let tx = try SRUITransaction(serializedBytes: data)

        XCTAssertEqual(tx.baseRevision, UInt64(expected["base_revision"] as? Int ?? -1))
        XCTAssertEqual(tx.newRevision, UInt64(expected["new_revision"] as? Int ?? -1))
        XCTAssertEqual(tx.priority, UInt32(expected["priority"] as? Int ?? -1))

        let expectedOps = expected["operations"] as? [[String: Any]] ?? []
        XCTAssertEqual(tx.operations.count, expectedOps.count)

        // Op 0: CREATE_NODE
        guard case .createNode(let createOp) = tx.operations[0].op else {
            XCTFail("Expected createNode operation for op 0")
            return
        }
        let expCreate = (expectedOps[0]["create_node"] as? [String: Any])?["node"] as? [String: Any] ?? [:]
        let createdNode = createOp.node
        XCTAssertEqual(createdNode.nodeID, UInt64(expCreate["node_id"] as? Int ?? -1))
        let expCreateType = expCreate["type"] as? [String: Any] ?? [:]
        XCTAssertEqual(createdNode.type.localID, UInt32(expCreateType["local_id"] as? Int ?? -1))
        XCTAssertEqual(createdNode.parentID, UInt64(expCreate["parent_id"] as? Int ?? -1))
        XCTAssertEqual(createdNode.childIndex, UInt32(expCreate["child_index"] as? Int ?? -1))

        let expCreateProps = expCreate["properties"] as? [[String: Any]] ?? []
        XCTAssertEqual(createdNode.properties.count, expCreateProps.count)
        let expProp0 = expCreateProps[0]
        let expProp0Ref = expProp0["property"] as? [String: Any] ?? [:]
        let expProp0Val = expProp0["value"] as? [String: Any] ?? [:]
        XCTAssertEqual(createdNode.properties[0].property.localID, UInt32(expProp0Ref["local_id"] as? Int ?? -1))
        guard case .stringValue(let text) = createdNode.properties[0].value.value else {
            XCTFail("Expected stringValue for text property")
            return
        }
        XCTAssertEqual(text, expProp0Val["string_value"] as? String)

        // Op 1: SET_PROPERTY
        guard case .setProperty(let setOp) = tx.operations[1].op else {
            XCTFail("Expected setProperty operation for op 1")
            return
        }
        let expSet = expectedOps[1]["set_property"] as? [String: Any] ?? [:]
        let expSetProp = expSet["property"] as? [String: Any] ?? [:]
        let expSetVal = expSet["value"] as? [String: Any] ?? [:]
        XCTAssertEqual(setOp.nodeID, UInt64(expSet["node_id"] as? Int ?? -1))
        XCTAssertEqual(setOp.property.localID, UInt32(expSetProp["local_id"] as? Int ?? -1))
        guard case .floatValue(let f) = setOp.value.value else {
            XCTFail("Expected floatValue for setOp value")
            return
        }
        let expectedFloat = expSetVal["float_value"] as? Double ?? 0.0
        XCTAssertEqual(f, expectedFloat, accuracy: 0.0001)

        // Op 2: BATCH_PROPERTY_SET
        guard case .batchPropertySet(let batchOp) = tx.operations[2].op else {
            XCTFail("Expected batchPropertySet operation for op 2")
            return
        }
        let expBatch = expectedOps[2]["batch_property_set"] as? [String: Any] ?? [:]
        let expBatchProps = expBatch["properties"] as? [[String: Any]] ?? []
        XCTAssertEqual(batchOp.nodeID, UInt64(expBatch["node_id"] as? Int ?? -1))
        XCTAssertEqual(batchOp.properties.count, expBatchProps.count)

        let expBatchProp0 = expBatchProps[0]
        let expBatchProp0Ref = expBatchProp0["property"] as? [String: Any] ?? [:]
        let expBatchProp0Val = expBatchProp0["value"] as? [String: Any] ?? [:]
        let expSize = expBatchProp0Val["size_value"] as? [String: Any] ?? [:]

        let batchProp = batchOp.properties[0]
        XCTAssertEqual(batchProp.property.localID, UInt32(expBatchProp0Ref["local_id"] as? Int ?? -1))
        guard case .sizeValue(let size) = batchProp.value.value else {
            XCTFail("Expected sizeValue for batchProp")
            return
        }
        XCTAssertEqual(size.width, expSize["width"] as? Double ?? 0.0)
        XCTAssertEqual(size.height, expSize["height"] as? Double ?? 0.0)

        // 3. Re-encode and verify identical wire bytes
        let roundtripData = try tx.serializedData()
        XCTAssertEqual(roundtripData, data, "Roundtrip re-encode mismatch")
    }

    func testDirectEncodeGoldenNodeRecordMatchesWireBytes() throws {
        let spec = try loadExpectedSpec()
        guard let vectors = spec["vectors"] as? [String: Any],
              let nodeSpec = vectors["golden_node_record"] as? [String: Any],
              let filename = nodeSpec["file"] as? String,
              let expectedHex = nodeSpec["hex"] as? String else {
            XCTFail("Malformed expected.json structure for golden_node_record")
            return
        }

        let fixtureData = try Data(contentsOf: conformanceVectorsDir.appendingPathComponent(filename))
        let authoredNode = createAuthoredNodeRecord()
        let encodedData = try authoredNode.serializedData()

        XCTAssertEqual(hexString(from: encodedData), expectedHex, "Authored Swift NodeRecord hex mismatch")
        XCTAssertEqual(encodedData, fixtureData, "Authored Swift NodeRecord byte mismatch against golden fixture")
    }

    func testDirectEncodeGoldenTransactionMatchesWireBytes() throws {
        let spec = try loadExpectedSpec()
        guard let vectors = spec["vectors"] as? [String: Any],
              let txSpec = vectors["golden_transaction"] as? [String: Any],
              let filename = txSpec["file"] as? String,
              let expectedHex = txSpec["hex"] as? String else {
            XCTFail("Malformed expected.json structure for golden_transaction")
            return
        }

        let fixtureData = try Data(contentsOf: conformanceVectorsDir.appendingPathComponent(filename))
        let authoredTx = createAuthoredTransaction()
        let encodedData = try authoredTx.serializedData()

        XCTAssertEqual(hexString(from: encodedData), expectedHex, "Authored Swift Transaction hex mismatch")
        XCTAssertEqual(encodedData, fixtureData, "Authored Swift Transaction byte mismatch against golden fixture")
    }

    func testCrossLanguageRustSwiftByteEquality() throws {
        // 1. Cross-language NodeRecord check: Swift encoding must be bit-for-bit identical to Rust-authored fixture
        let authoredNode = createAuthoredNodeRecord()
        let swiftNodeBytes = try authoredNode.serializedData()
        let rustNodeBytes = try Data(contentsOf: conformanceVectorsDir.appendingPathComponent("golden_node_record.bin"))
        XCTAssertEqual(
            swiftNodeBytes,
            rustNodeBytes,
            "Cross-language mismatch: Swift-encoded NodeRecord does not match Rust-authored bytes"
        )

        // 2. Cross-language Transaction check: Swift encoding must be bit-for-bit identical to Rust-authored fixture
        let authoredTx = createAuthoredTransaction()
        let swiftTxBytes = try authoredTx.serializedData()
        let rustTxBytes = try Data(contentsOf: conformanceVectorsDir.appendingPathComponent("golden_transaction.bin"))
        XCTAssertEqual(
            swiftTxBytes,
            rustTxBytes,
            "Cross-language mismatch: Swift-encoded Transaction does not match Rust-authored bytes"
        )

        // 3. Cross-language Framed SruiMessage check: Swift framing must be bit-for-bit identical to Rust-authored fixture
        var msg = Srui_Protocol_SruiMessage()
        msg.transaction = authoredTx
        let swiftFramedBytes = try SRUIFraming.encodeFramed(msg)
        let rustFramedBytes = try Data(contentsOf: conformanceVectorsDir.appendingPathComponent("golden_framed_message.bin"))
        XCTAssertEqual(
            swiftFramedBytes,
            rustFramedBytes,
            "Cross-language mismatch: Swift-encoded Framed SruiMessage does not match Rust-authored bytes"
        )
    }

    func testDecodeGoldenFramedMessageAgainstExpectedJSON() throws {
        let spec = try loadExpectedSpec()
        guard let vectors = spec["vectors"] as? [String: Any],
              let framedSpec = vectors["golden_framed_message"] as? [String: Any],
              let filename = framedSpec["file"] as? String,
              let expectedHex = framedSpec["hex"] as? String,
              let expectedSHA256 = framedSpec["sha256"] as? String,
              let expectedByteLen = framedSpec["byte_length"] as? Int,
              let expected = framedSpec["expected"] as? [String: Any] else {
            XCTFail("Malformed expected.json structure for golden_framed_message")
            return
        }

        let fileURL = conformanceVectorsDir.appendingPathComponent(filename)
        let data = try Data(contentsOf: fileURL)

        // 1. Assert raw bytes match canonical specification
        XCTAssertEqual(data.count, expectedByteLen, "Fixture byte length mismatch")
        XCTAssertEqual(hexString(from: data), expectedHex, "Fixture hex mismatch")
        XCTAssertEqual(sha256String(from: data), expectedSHA256, "Fixture SHA256 mismatch")

        // 2. Decode framed message and assert against JSON oracle
        let decoded = try SRUIFraming.decodeFramed(Srui_Protocol_SruiMessage.self, from: data)
        XCTAssertEqual(decoded.transaction.baseRevision, UInt64(expected["base_revision"] as? Int ?? -1))
        XCTAssertEqual(decoded.transaction.newRevision, UInt64(expected["new_revision"] as? Int ?? -1))
        XCTAssertEqual(decoded.transaction.priority, UInt32(expected["priority"] as? Int ?? -1))
        XCTAssertEqual(decoded.transaction.operations.count, expected["operation_count"] as? Int ?? -1)

        // 3. Re-encode and verify identical wire bytes
        let roundtripData = try SRUIFraming.encodeFramed(decoded)
        XCTAssertEqual(roundtripData, data, "Roundtrip re-encode framed mismatch")
    }

    func testDirectEncodeGoldenFramedMessageMatchesWireBytes() throws {
        let spec = try loadExpectedSpec()
        guard let vectors = spec["vectors"] as? [String: Any],
              let framedSpec = vectors["golden_framed_message"] as? [String: Any],
              let filename = framedSpec["file"] as? String,
              let expectedHex = framedSpec["hex"] as? String else {
            XCTFail("Malformed expected.json structure for golden_framed_message")
            return
        }

        let fixtureData = try Data(contentsOf: conformanceVectorsDir.appendingPathComponent(filename))
        var msg = Srui_Protocol_SruiMessage()
        msg.transaction = createAuthoredTransaction()
        let encodedData = try SRUIFraming.encodeFramed(msg)

        XCTAssertEqual(hexString(from: encodedData), expectedHex, "Authored Swift Framed SruiMessage hex mismatch")
        XCTAssertEqual(encodedData, fixtureData, "Authored Swift Framed SruiMessage byte mismatch against golden fixture")
    }

    func testLengthDelimitedFraming() throws {
        var msg = Srui_Protocol_SruiMessage()
        msg.transaction = createAuthoredTransaction()

        let framed = try SRUIFraming.encodeFramed(msg)
        XCTAssertFalse(framed.isEmpty)

        let decoded = try SRUIFraming.decodeFramed(Srui_Protocol_SruiMessage.self, from: framed)
        XCTAssertEqual(decoded.transaction.baseRevision, 104)
        XCTAssertEqual(decoded.transaction.newRevision, 105)
        XCTAssertEqual(decoded.transaction.operations.count, 3)
    }

    func testMaxFrameSizeLimitEnforced() throws {
        var msg = Srui_Protocol_SruiMessage()
        msg.transaction = createAuthoredTransaction()

        // Encoding exceeding frame size should throw
        XCTAssertThrowsError(try SRUIFraming.encodeFramed(msg, maxFrameSize: 2)) { error in
            guard case SRUIFramingError.frameSizeLimitExceeded(let limit, let actual) = error else {
                XCTFail("Expected frameSizeLimitExceeded, got \(error)")
                return
            }
            XCTAssertEqual(limit, 2)
            XCTAssertGreaterThan(actual, 2)
        }

        // Decoding exceeding frame size should throw
        let validFramed = try SRUIFraming.encodeFramed(msg)
        XCTAssertThrowsError(try SRUIFraming.decodeFramed(Srui_Protocol_SruiMessage.self, from: validFramed, maxFrameSize: 2)) { error in
            guard case SRUIFramingError.frameSizeLimitExceeded(let limit, _) = error else {
                XCTFail("Expected frameSizeLimitExceeded, got \(error)")
                return
            }
            XCTAssertEqual(limit, 2)
        }
    }
}
