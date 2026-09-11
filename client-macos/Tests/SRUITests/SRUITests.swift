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
        var current = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
        while current.path != "/" {
            let candidate = current.appendingPathComponent("protocol/conformance-vectors")
            if FileManager.default.fileExists(atPath: candidate.path) {
                return candidate
            }
            current = current.deletingLastPathComponent()
        }
        fatalError("Could not locate protocol/conformance-vectors from \(#filePath)")
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

        // 4. Cross-language TEXT_EDIT check, including event framing and edit_seq.
        let swiftTextEditBytes = try SRUIFraming.encodeFramed(createAuthoredTextEditEvent())
        let rustTextEditBytes = try Data(
            contentsOf: conformanceVectorsDir.appendingPathComponent("golden_text_edit_event.bin")
        )
        XCTAssertEqual(
            swiftTextEditBytes,
            rustTextEditBytes,
            "Cross-language mismatch: Swift-encoded TEXT_EDIT does not match Rust-authored bytes"
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

    /// Authors the golden `ServerEventAck` envelope from scratch (§18.2), independently of the fixture.
    private func createAuthoredEventAck() -> Srui_Protocol_SruiMessage {
        var ack = Srui_Protocol_ServerEventAck()
        ack.clientInstanceID = Data("c17".utf8)
        ack.eventID = Data("e123".utf8)
        ack.lastProcessedEventSeq = 593
        ack.status = .processed
        ack.revisionAfterEffect = 1843
        ack.rejectReason = ""
        ack.sessionID = "s-91c"
        ack.settledEventSeq = 593

        var msg = Srui_Protocol_SruiMessage()
        msg.serverEventAck = ack
        return msg
    }

    func testDecodeGoldenEventAckAgainstExpectedJSON() throws {
        let spec = try loadExpectedSpec()
        guard let vectors = spec["vectors"] as? [String: Any],
              let ackSpec = vectors["golden_event_ack"] as? [String: Any],
              let filename = ackSpec["file"] as? String,
              let expectedHex = ackSpec["hex"] as? String,
              let expectedSHA256 = ackSpec["sha256"] as? String,
              let expectedByteLen = ackSpec["byte_length"] as? Int,
              let expected = ackSpec["expected"] as? [String: Any] else {
            XCTFail("Malformed expected.json structure for golden_event_ack")
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
        guard case .serverEventAck(let ack)? = decoded.msg else {
            XCTFail("Expected serverEventAck in framed message, got \(String(describing: decoded.msg))")
            return
        }
        XCTAssertEqual(ack.clientInstanceID, Data((expected["client_instance_id"] as? String ?? "").utf8))
        XCTAssertEqual(ack.eventID, Data((expected["event_id"] as? String ?? "").utf8))
        XCTAssertEqual(ack.lastProcessedEventSeq, UInt64(expected["last_processed_event_seq"] as? Int ?? -1))
        XCTAssertEqual(ack.status.rawValue, expected["status"] as? Int ?? -1)
        XCTAssertEqual(ack.status, .processed, "status_name \(expected["status_name"] as? String ?? "?")")
        XCTAssertEqual(ack.revisionAfterEffect, UInt64(expected["revision_after_effect"] as? Int ?? -1))
        XCTAssertEqual(ack.rejectReason, expected["reject_reason"] as? String ?? "<missing>")
        // §18.2: the settling incarnation is required, so the oracle carries a real one.
        XCTAssertEqual(ack.sessionID, expected["session_id"] as? String ?? "<missing>")
        XCTAssertFalse(ack.sessionID.isEmpty)
        XCTAssertEqual(ack.settledEventSeq, UInt64(expected["settled_event_seq"] as? Int ?? -1))

        // 3. Re-encode and verify identical wire bytes
        let roundtripData = try SRUIFraming.encodeFramed(decoded)
        XCTAssertEqual(roundtripData, data, "Roundtrip re-encode framed mismatch")
    }

    func testDirectEncodeGoldenEventAckMatchesWireBytes() throws {
        let spec = try loadExpectedSpec()
        guard let vectors = spec["vectors"] as? [String: Any],
              let ackSpec = vectors["golden_event_ack"] as? [String: Any],
              let filename = ackSpec["file"] as? String,
              let expectedHex = ackSpec["hex"] as? String else {
            XCTFail("Malformed expected.json structure for golden_event_ack")
            return
        }

        let fixtureData = try Data(contentsOf: conformanceVectorsDir.appendingPathComponent(filename))
        let encodedData = try SRUIFraming.encodeFramed(createAuthoredEventAck())

        XCTAssertEqual(hexString(from: encodedData), expectedHex, "Authored Swift Framed ServerEventAck hex mismatch")
        XCTAssertEqual(encodedData, fixtureData, "Authored Swift Framed ServerEventAck byte mismatch against golden fixture")
    }

    /// Authors the golden `ClientModelRangeRequest` envelope from scratch (§8, §22.7).
    private func createAuthoredClientModelRangeRequest() -> Srui_Protocol_SruiMessage {
        var request = Srui_Protocol_ClientModelRangeRequest()
        request.nodeID = 7
        request.modelID = 11
        request.startIndex = 128
        request.count = 64
        request.observedRevision = 5

        var msg = Srui_Protocol_SruiMessage()
        msg.clientModelRangeRequest = request
        return msg
    }

    /// Authors the golden `TEXT_EDIT` envelope from scratch (§18.3, §22.6).
    private func createAuthoredTextEditEvent() -> Srui_Protocol_SruiMessage {
        var event = Srui_Protocol_Event()
        event.clientInstanceID = Data("client-29".utf8)
        event.eventSeq = 29
        event.eventID = Data("event-text-29".utf8)
        event.observedRevision = 41
        event.nodeID = 7
        event.eventType = Srui_Protocol_TypeRef.with {
            $0.namespaceID = standardNamespaceID
            $0.localID = UInt32(Srui_Protocol_StandardEvent.eventTextEdit.rawValue)
        }
        event.arguments = [
            Srui_Protocol_Property.with {
                $0.property = Srui_Protocol_PropertyRef.with {
                    $0.namespaceID = standardNamespaceID
                    $0.localID = UInt32(Srui_Protocol_StandardProperty.propertyText.rawValue)
                }
                $0.value = Srui_Protocol_Value.with {
                    $0.stringValue = "composed text"
                }
            }
        ]
        event.editSeq = 3

        var message = Srui_Protocol_SruiMessage()
        message.event = event
        return message
    }

    func testDecodeGoldenClientModelRangeRequestAgainstExpectedJSON() throws {
        let spec = try loadExpectedSpec()
        guard let vectors = spec["vectors"] as? [String: Any],
              let rangeSpec = vectors["golden_client_model_range_request"] as? [String: Any],
              let filename = rangeSpec["file"] as? String,
              let expectedHex = rangeSpec["hex"] as? String,
              let expectedSHA256 = rangeSpec["sha256"] as? String,
              let expectedByteLen = rangeSpec["byte_length"] as? Int,
              let expected = rangeSpec["expected"] as? [String: Any] else {
            XCTFail("Malformed expected.json structure for golden_client_model_range_request")
            return
        }

        let fileURL = conformanceVectorsDir.appendingPathComponent(filename)
        let data = try Data(contentsOf: fileURL)

        XCTAssertEqual(data.count, expectedByteLen, "Fixture byte length mismatch")
        XCTAssertEqual(hexString(from: data), expectedHex, "Fixture hex mismatch")
        XCTAssertEqual(sha256String(from: data), expectedSHA256, "Fixture SHA256 mismatch")

        let decoded = try SRUIFraming.decodeFramed(Srui_Protocol_SruiMessage.self, from: data)
        guard case .clientModelRangeRequest(let request)? = decoded.msg else {
            XCTFail("Expected clientModelRangeRequest in framed message, got \(String(describing: decoded.msg))")
            return
        }
        XCTAssertEqual(request.nodeID, UInt64(expected["node_id"] as? Int ?? -1))
        XCTAssertEqual(request.modelID, UInt64(expected["model_id"] as? Int ?? -1))
        XCTAssertEqual(request.startIndex, UInt64(expected["start_index"] as? Int ?? -1))
        XCTAssertEqual(request.count, UInt64(expected["count"] as? Int ?? -1))
        XCTAssertEqual(request.observedRevision, UInt64(expected["observed_revision"] as? Int ?? -1))

        let roundtripData = try SRUIFraming.encodeFramed(decoded)
        XCTAssertEqual(roundtripData, data, "Roundtrip re-encode framed mismatch")
    }

    func testDirectEncodeGoldenClientModelRangeRequestMatchesWireBytes() throws {
        let spec = try loadExpectedSpec()
        guard let vectors = spec["vectors"] as? [String: Any],
              let rangeSpec = vectors["golden_client_model_range_request"] as? [String: Any],
              let filename = rangeSpec["file"] as? String,
              let expectedHex = rangeSpec["hex"] as? String else {
            XCTFail("Malformed expected.json structure for golden_client_model_range_request")
            return
        }

        let fixtureData = try Data(contentsOf: conformanceVectorsDir.appendingPathComponent(filename))
        let encodedData = try SRUIFraming.encodeFramed(createAuthoredClientModelRangeRequest())

        XCTAssertEqual(hexString(from: encodedData), expectedHex, "Authored Swift Framed ClientModelRangeRequest hex mismatch")
        XCTAssertEqual(encodedData, fixtureData, "Authored Swift Framed ClientModelRangeRequest byte mismatch against golden fixture")
    }

    func testDecodeGoldenTextEditEventAgainstExpectedJSON() throws {
        let spec = try loadExpectedSpec()
        guard let vectors = spec["vectors"] as? [String: Any],
              let eventSpec = vectors["golden_text_edit_event"] as? [String: Any],
              let filename = eventSpec["file"] as? String,
              let expectedHex = eventSpec["hex"] as? String,
              let expectedSHA256 = eventSpec["sha256"] as? String,
              let expectedByteLen = eventSpec["byte_length"] as? Int,
              let expected = eventSpec["expected"] as? [String: Any],
              let expectedType = expected["event_type"] as? [String: Any],
              let expectedArguments = expected["arguments"] as? [[String: Any]],
              let expectedArgument = expectedArguments.first,
              let expectedProperty = expectedArgument["property"] as? [String: Any],
              let expectedValue = expectedArgument["value"] as? [String: Any] else {
            XCTFail("Malformed expected.json structure for golden_text_edit_event")
            return
        }

        let data = try Data(contentsOf: conformanceVectorsDir.appendingPathComponent(filename))
        XCTAssertEqual(data.count, expectedByteLen)
        XCTAssertEqual(hexString(from: data), expectedHex)
        XCTAssertEqual(sha256String(from: data), expectedSHA256)

        let decoded = try SRUIFraming.decodeFramed(Srui_Protocol_SruiMessage.self, from: data)
        guard case .event(let event)? = decoded.msg else {
            XCTFail("Expected event in framed message, got \(String(describing: decoded.msg))")
            return
        }
        XCTAssertEqual(
            event.clientInstanceID,
            Data((expected["client_instance_id"] as? String ?? "").utf8)
        )
        XCTAssertEqual(event.eventSeq, UInt64(expected["event_seq"] as? Int ?? -1))
        XCTAssertEqual(event.eventID, Data((expected["event_id"] as? String ?? "").utf8))
        XCTAssertEqual(event.observedRevision, UInt64(expected["observed_revision"] as? Int ?? -1))
        XCTAssertEqual(event.nodeID, UInt64(expected["node_id"] as? Int ?? -1))
        XCTAssertTrue(event.hasEventType)
        XCTAssertEqual(
            event.eventType.namespaceID,
            UInt32(expectedType["namespace_id"] as? Int ?? -1)
        )
        XCTAssertEqual(event.eventType.localID, UInt32(expectedType["local_id"] as? Int ?? -1))
        XCTAssertEqual(event.arguments.count, expectedArguments.count)
        XCTAssertEqual(
            event.arguments[0].property.namespaceID,
            UInt32(expectedProperty["namespace_id"] as? Int ?? -1)
        )
        XCTAssertEqual(
            event.arguments[0].property.localID,
            UInt32(expectedProperty["local_id"] as? Int ?? -1)
        )
        guard case .stringValue(let text) = event.arguments[0].value.value else {
            XCTFail("Expected stringValue TEXT_EDIT argument")
            return
        }
        XCTAssertEqual(text, expectedValue["string_value"] as? String)
        XCTAssertEqual(event.editSeq, UInt64(expected["edit_seq"] as? Int ?? -1))

        let roundtripData = try SRUIFraming.encodeFramed(decoded)
        XCTAssertEqual(roundtripData, data)
    }

    func testDirectEncodeGoldenTextEditEventMatchesWireBytes() throws {
        let spec = try loadExpectedSpec()
        guard let vectors = spec["vectors"] as? [String: Any],
              let eventSpec = vectors["golden_text_edit_event"] as? [String: Any],
              let filename = eventSpec["file"] as? String,
              let expectedHex = eventSpec["hex"] as? String else {
            XCTFail("Malformed expected.json structure for golden_text_edit_event")
            return
        }

        let fixtureData = try Data(contentsOf: conformanceVectorsDir.appendingPathComponent(filename))
        let encodedData = try SRUIFraming.encodeFramed(createAuthoredTextEditEvent())
        XCTAssertEqual(hexString(from: encodedData), expectedHex)
        XCTAssertEqual(encodedData, fixtureData)
    }

    private func createAuthoredTerminalData() -> Srui_Protocol_SruiMessage {
        var data = Srui_Protocol_TerminalData()
        data.streamID = 7
        data.byteOffset = 4096
        data.data = Data("pty-ok".utf8)
        var msg = Srui_Protocol_SruiMessage()
        msg.terminalData = data
        return msg
    }

    private func createAuthoredTerminalInput() -> Srui_Protocol_SruiMessage {
        var input = Srui_Protocol_TerminalInput()
        input.streamID = 7
        input.data = Data("ls\n".utf8)
        var msg = Srui_Protocol_SruiMessage()
        msg.terminalInput = input
        return msg
    }

    private func createAuthoredTerminalResize() -> Srui_Protocol_SruiMessage {
        var resize = Srui_Protocol_TerminalResize()
        resize.streamID = 7
        resize.columns = 80
        resize.rows = 24
        resize.pixelWidth = 1280
        resize.pixelHeight = 720
        var msg = Srui_Protocol_SruiMessage()
        msg.terminalResize = resize
        return msg
    }

    private func createAuthoredTerminalResyncRequired() -> Srui_Protocol_SruiMessage {
        var resync = Srui_Protocol_TerminalResyncRequired()
        resync.streamID = 7
        resync.requestedOffset = 100
        resync.retainedFromOffset = 64
        resync.resumeAtOffset = 240
        resync.reason = .retentionLoss
        var msg = Srui_Protocol_SruiMessage()
        msg.terminalResyncRequired = resync
        return msg
    }

    func testDecodeGoldenTerminalDataAgainstExpectedJSON() throws {
        try assertFramedTerminalVector(
            key: "golden_terminal_data",
            authored: createAuthoredTerminalData()
        ) { decoded in
            guard case .terminalData(let data)? = decoded.msg else {
                XCTFail("Expected terminalData, got \(String(describing: decoded.msg))")
                return
            }
            XCTAssertEqual(data.streamID, 7)
            XCTAssertEqual(data.byteOffset, 4096)
            XCTAssertEqual(data.data, Data("pty-ok".utf8))
        }
    }

    func testDirectEncodeGoldenTerminalDataMatchesWireBytes() throws {
        try assertAuthoredTerminalMatchesFixture("golden_terminal_data", createAuthoredTerminalData())
    }

    func testDecodeGoldenTerminalInputAgainstExpectedJSON() throws {
        try assertFramedTerminalVector(
            key: "golden_terminal_input",
            authored: createAuthoredTerminalInput()
        ) { decoded in
            guard case .terminalInput(let input)? = decoded.msg else {
                XCTFail("Expected terminalInput, got \(String(describing: decoded.msg))")
                return
            }
            XCTAssertEqual(input.streamID, 7)
            XCTAssertEqual(input.data, Data("ls\n".utf8))
        }
    }

    func testDirectEncodeGoldenTerminalInputMatchesWireBytes() throws {
        try assertAuthoredTerminalMatchesFixture("golden_terminal_input", createAuthoredTerminalInput())
    }

    func testDecodeGoldenTerminalResizeAgainstExpectedJSON() throws {
        try assertFramedTerminalVector(
            key: "golden_terminal_resize",
            authored: createAuthoredTerminalResize()
        ) { decoded in
            guard case .terminalResize(let resize)? = decoded.msg else {
                XCTFail("Expected terminalResize, got \(String(describing: decoded.msg))")
                return
            }
            XCTAssertEqual(resize.streamID, 7)
            XCTAssertEqual(resize.columns, 80)
            XCTAssertEqual(resize.rows, 24)
            XCTAssertEqual(resize.pixelWidth, 1280)
            XCTAssertEqual(resize.pixelHeight, 720)
        }
    }

    func testDirectEncodeGoldenTerminalResizeMatchesWireBytes() throws {
        try assertAuthoredTerminalMatchesFixture("golden_terminal_resize", createAuthoredTerminalResize())
    }

    func testDecodeGoldenTerminalResyncRequiredAgainstExpectedJSON() throws {
        try assertFramedTerminalVector(
            key: "golden_terminal_resync_required",
            authored: createAuthoredTerminalResyncRequired()
        ) { decoded in
            guard case .terminalResyncRequired(let resync)? = decoded.msg else {
                XCTFail("Expected terminalResyncRequired, got \(String(describing: decoded.msg))")
                return
            }
            XCTAssertEqual(resync.streamID, 7)
            XCTAssertEqual(resync.requestedOffset, 100)
            XCTAssertEqual(resync.retainedFromOffset, 64)
            XCTAssertEqual(resync.resumeAtOffset, 240)
            XCTAssertEqual(resync.reason, .retentionLoss)
        }
    }

    func testDirectEncodeGoldenTerminalResyncRequiredMatchesWireBytes() throws {
        try assertAuthoredTerminalMatchesFixture(
            "golden_terminal_resync_required",
            createAuthoredTerminalResyncRequired()
        )
    }

    /// Negative decode vectors for the terminal envelopes (§21, §26; CLAUDE.md decode-path rule).
    func testMalformedTerminalVectorsRejectedBySwiftDecodePath() async throws {
        let spec = try loadExpectedSpec()
        guard let malformed = spec["malformed_vectors"] as? [String: Any],
              let inputSpec = malformed["malformed_terminal_input_empty"] as? [String: Any],
              let dataSpec = malformed["malformed_terminal_data_empty"] as? [String: Any] else {
            XCTFail("Missing malformed terminal vectors in expected.json")
            return
        }

        for vector in [inputSpec, dataSpec] {
            guard let filename = vector["file"] as? String,
                  let expectedHex = vector["hex"] as? String,
                  let expectedSHA256 = vector["sha256"] as? String,
                  let expectedByteLen = vector["byte_length"] as? Int else {
                XCTFail("Malformed expected.json structure for a terminal vector")
                return
            }
            let bytes = try Data(contentsOf: conformanceVectorsDir.appendingPathComponent(filename))
            XCTAssertEqual(bytes.count, expectedByteLen, "\(filename) byte length mismatch")
            XCTAssertEqual(hexString(from: bytes), expectedHex, "\(filename) hex mismatch")
            XCTAssertEqual(sha256String(from: bytes), expectedSHA256, "\(filename) SHA256 mismatch")
        }

        // TERMINAL_INPUT: protobuf-valid, semantically forbidden (no payload).
        let inputBytes = try Data(
            contentsOf: conformanceVectorsDir.appendingPathComponent(inputSpec["file"] as! String)
        )
        let decodedInput = try SRUIFraming.decodeFramed(
            Srui_Protocol_SruiMessage.self,
            from: inputBytes
        )
        guard case .terminalInput(let input)? = decodedInput.msg else {
            XCTFail("Expected terminalInput vector")
            return
        }
        XCTAssertTrue(input.data.isEmpty, "vector must carry an empty payload")

        // TERMINAL_DATA: the client apply path must refuse it instead of advancing an offset.
        let dataBytes = try Data(
            contentsOf: conformanceVectorsDir.appendingPathComponent(dataSpec["file"] as! String)
        )
        let decodedData = try SRUIFraming.decodeFramed(
            Srui_Protocol_SruiMessage.self,
            from: dataBytes
        )
        guard case .terminalData(let frame)? = decodedData.msg else {
            XCTFail("Expected terminalData vector")
            return
        }
        XCTAssertTrue(frame.data.isEmpty, "vector must carry an empty payload")

        let session = TerminalSession()
        do {
            _ = try await session.applyData(
                streamID: NodeId(frame.streamID),
                byteOffset: frame.byteOffset,
                data: frame.data
            )
            XCTFail("empty TerminalData frame must be rejected")
        } catch let error as TerminalApplyError {
            XCTAssertEqual(error, .emptyFrame)
        }
        let offsets = await session.streamOffsets()
        XCTAssertTrue(offsets.isEmpty, "rejected frame must not create stream state")
    }

    private func assertFramedTerminalVector(
        key: String,
        authored: Srui_Protocol_SruiMessage,
        check: (Srui_Protocol_SruiMessage) -> Void
    ) throws {
        let spec = try loadExpectedSpec()
        guard let vectors = spec["vectors"] as? [String: Any],
              let vector = vectors[key] as? [String: Any],
              let filename = vector["file"] as? String,
              let expectedHex = vector["hex"] as? String,
              let expectedSHA256 = vector["sha256"] as? String,
              let expectedByteLen = vector["byte_length"] as? Int else {
            XCTFail("Malformed expected.json structure for \(key)")
            return
        }
        let data = try Data(contentsOf: conformanceVectorsDir.appendingPathComponent(filename))
        XCTAssertEqual(data.count, expectedByteLen)
        XCTAssertEqual(hexString(from: data), expectedHex)
        XCTAssertEqual(sha256String(from: data), expectedSHA256)
        let decoded = try SRUIFraming.decodeFramed(Srui_Protocol_SruiMessage.self, from: data)
        check(decoded)
        XCTAssertEqual(try SRUIFraming.encodeFramed(decoded), data)
        _ = authored
    }

    private func assertAuthoredTerminalMatchesFixture(
        _ key: String,
        _ authored: Srui_Protocol_SruiMessage
    ) throws {
        let spec = try loadExpectedSpec()
        guard let vectors = spec["vectors"] as? [String: Any],
              let vector = vectors[key] as? [String: Any],
              let filename = vector["file"] as? String,
              let expectedHex = vector["hex"] as? String else {
            XCTFail("Malformed expected.json structure for \(key)")
            return
        }
        let fixture = try Data(contentsOf: conformanceVectorsDir.appendingPathComponent(filename))
        let encoded = try SRUIFraming.encodeFramed(authored)
        XCTAssertEqual(hexString(from: encoded), expectedHex)
        XCTAssertEqual(encoded, fixture)
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

    func testMalformedFramingVectorsRejectedCleanly() throws {
        let spec = try loadExpectedSpec()
        guard let malformed = spec["malformed_vectors"] as? [String: Any],
              let overlongSpec = malformed["malformed_overlong_varint"] as? [String: Any],
              let truncatedSpec = malformed["malformed_truncated_frame"] as? [String: Any] else {
            XCTFail("Missing malformed_vectors in expected.json")
            return
        }

        // 1. Overlong varint
        let overlongFile = overlongSpec["file"] as! String
        let overlongData = try Data(contentsOf: conformanceVectorsDir.appendingPathComponent(overlongFile))
        XCTAssertEqual(hexString(from: overlongData), overlongSpec["hex"] as! String)
        XCTAssertThrowsError(try SRUIFraming.decodeFramed(Srui_Protocol_SruiMessage.self, from: overlongData))

        // 2. Truncated frame
        let truncatedFile = truncatedSpec["file"] as! String
        let truncatedData = try Data(contentsOf: conformanceVectorsDir.appendingPathComponent(truncatedFile))
        XCTAssertEqual(hexString(from: truncatedData), truncatedSpec["hex"] as! String)
        XCTAssertThrowsError(try SRUIFraming.decodeFramed(Srui_Protocol_SruiMessage.self, from: truncatedData))
    }
}
