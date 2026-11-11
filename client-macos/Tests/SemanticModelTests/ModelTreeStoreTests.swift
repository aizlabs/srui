//
// ModelTreeStoreTests.swift
// SemanticModelTests
//
// Replica-side rules for hierarchical collection models (PX-018-G01), in parity with
// `server-rust/semantic-tree/tests/model_tree_test.rs`: a Tree model's index space is the
// depth-first preorder of the forest, shaped by the standard item properties `tree_depth` and
// `descendant_count`, checked after the last operation of a transaction that touches the model,
// and bounded by the §26 limit `maxModelTreeDepth`. Implements: §4 inv. 13, §8, §12.1, §26.
// §32 suites 1 and 9 run this file; suite 1 vectors 70–84 replay the same rules in both languages,
// and `protocol/conformance-vectors/model_tree_shape_cases.json` is the shared parity table.
//

import XCTest
@testable import SemanticModel

final class ModelTreeStoreTests: XCTestCase {

    private let model = ModelId(10)

    private func treeItem(_ id: UInt64, _ depth: UInt64, _ descendants: UInt64) -> ModelItem {
        ModelItem(
            itemID: ItemId(id),
            value: .string("item \(id)"),
            properties: [.treeDepth: .unsignedInt(depth), .descendantCount: .unsignedInt(descendants)]
        )
    }

    /// A, B, C, D, E, F in preorder: A{B{C}, D}, E{F}.
    private var nested: [ModelItem] {
        [treeItem(1, 0, 3), treeItem(2, 1, 1), treeItem(3, 2, 0), treeItem(4, 1, 0),
         treeItem(5, 0, 1), treeItem(6, 1, 0)]
    }

    private func applierWithNested(_ limits: StoreLimits = StoreLimits()) throws -> TransactionApplier {
        let applier = TransactionApplier(limits: limits)
        let result = applier.apply(baseRevision: .initial, operations: [
            .createModel(id: model, modelType: .tree, itemCount: 0),
            .modelInsert(id: model, index: 0, items: nested),
        ])
        guard case .success = result else {
            XCTFail("setup failed: \(result)")
            throw StoreError.operationError("nested setup")
        }
        return applier
    }

    private func codeAndIndex(_ result: Result<Revision, TxnError>) -> (String, Int?)? {
        guard case .failure(let error) = result else { return nil }
        if case .opFailed(let index, let source) = error { return (source.conformanceCode, index) }
        return (error.conformanceCode ?? "unknown", nil)
    }

    // MARK: - Shared case table (Rust / Swift / Python parity)

    func testTheSharedShapeTableMatchesRowForRow() throws {
        var dir = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
        while !FileManager.default.fileExists(
            atPath: dir.appendingPathComponent("protocol/conformance-vectors").path
        ) {
            dir = dir.deletingLastPathComponent()
            guard dir.path != "/" else { return XCTFail("conformance-vectors not found") }
        }
        let url = dir.appendingPathComponent("protocol/conformance-vectors/model_tree_shape_cases.json")
        let cases = try loadCases(url)
        XCTAssertEqual(cases.count, 323, "pinned with the Rust and Python loaders")
        try assertCases(cases)
    }

    /// Differential fuzz hook: `SRUI_MODEL_TREE_FUZZ_CASES=<file>` replays a larger generated table
    /// in the same format against this replica, exactly as `model_tree_test.rs` does. Unset, it
    /// does nothing.
    func testOptionalFuzzTableMatchesRowForRow() throws {
        guard let path = ProcessInfo.processInfo.environment["SRUI_MODEL_TREE_FUZZ_CASES"] else { return }
        let cases = try loadCases(URL(fileURLWithPath: path))
        try assertCases(cases)
        print("fuzz table: \(cases.count) cases agreed")
    }

    /// Exhaustive small-forest sweep hook: `SRUI_MODEL_TREE_SWEEP_OUT=<file>` writes one verdict
    /// letter per cache (`.` accepted, `L`, `C`, `J`, `S`) for every cache of at most five items,
    /// depths 0...3 and every descendant count, full and sparse, under limits 3 and 4 — the same
    /// enumeration and format as `model_tree_test.rs` and
    /// `protocol/tests/test_model_tree_shape_cases.py::enumerate_sweep`. Unset, it does nothing.
    func testOptionalExhaustiveSweepWritesVerdicts() throws {
        guard let path = ProcessInfo.processInfo.environment["SRUI_MODEL_TREE_SWEEP_OUT"] else { return }
        let depths: [UInt64] = [0, 1, 2, 3]
        var out = [UInt8]()
        for maxDepth in [3, 4] {
            for n in UInt64(1)...5 {
                let base = UInt64(depths.count) * n
                for mask in 1..<(1 << n) {
                    let cached = (0..<n).filter { mask >> $0 & 1 == 1 }
                    var total: UInt64 = 1
                    for _ in cached { total *= base }
                    for t in 0..<total {
                        var built = Model(id: model, modelType: .tree, itemCount: n)
                        var x = t
                        for index in cached {
                            let digit = x % base
                            x /= base
                            let item = treeItem(1000 + index, depths[Int(digit / n)], digit % n)
                            built.items[index] = item
                            built.idToIndex[item.itemID] = index
                        }
                        built.treeShaped = true
                        do {
                            try built.validateTreeShape(maxModelTreeDepth: maxDepth)
                            out.append(UInt8(ascii: "."))
                        } catch StoreError.maxModelTreeDepthExceeded {
                            out.append(UInt8(ascii: "L"))
                        } catch StoreError.invalidModelTree(_, _, let violation) {
                            switch violation {
                            case .descendantCountOutOfRange: out.append(UInt8(ascii: "C"))
                            case .depthJump: out.append(UInt8(ascii: "J"))
                            case .outsideParentSpan: out.append(UInt8(ascii: "S"))
                            case .invalidProperty: out.append(UInt8(ascii: "P"))
                            }
                        }
                    }
                }
            }
        }
        try Data(out).write(to: URL(fileURLWithPath: path))
        print("sweep: \(out.count) caches written")
    }

    /// A deeper item directly after a cached leaf would be that leaf's child outside its empty
    /// span: refused (PX-018-G01 round 1, F1), as are the sparse forms no forest can produce.
    func testADeeperItemAfterACachedLeafIsRefused() {
        let apply = { (items: [ModelItem], count: UInt64) -> (String, Int?)? in
            self.codeAndIndex(TransactionApplier().apply(baseRevision: .initial, operations: [
                .createModel(id: self.model, modelType: .tree, itemCount: count),
                .modelResetRange(id: self.model, startIndex: 0, items: items, totalCount: nil),
            ]))
        }
        XCTAssertEqual(apply([treeItem(1, 0, 0), treeItem(2, 1, 0)], 2)?.0, "model_tree_outside_parent_span")
        XCTAssertEqual(apply([treeItem(1, 0, 1), treeItem(2, 1, 0), treeItem(3, 2, 0)], 3)?.0, "model_tree_outside_parent_span")
        XCTAssertNil(apply([treeItem(1, 0, 1), treeItem(2, 1, 0), treeItem(3, 0, 0)], 3))
    }

    /// A flat model that gains the shape properties only through MODEL_UPDATE is checked from
    /// that transaction on (round 1, F3): `updateItems` marks the model shaped.
    func testAFlatModelGainingShapeThroughModelUpdateIsChecked() throws {
        let applier = TransactionApplier()
        let flat = (0..<3).map { ModelItem(itemID: ItemId(UInt64($0)), value: .null) }
        XCTAssertNil(codeAndIndex(applier.apply(baseRevision: .initial, operations: [
            .createModel(id: model, modelType: .tree, itemCount: 0),
            .modelInsert(id: model, index: 0, items: flat),
        ])))
        XCTAssertEqual(applier.store.getModel(model)?.treeShaped, false)
        let bad = ModelItem(itemID: ItemId(1), value: .null, properties: [.treeDepth: .unsignedInt(2)])
        let result = applier.apply(baseRevision: applier.lastAppliedRevision, operations: [
            .modelUpdate(id: model, index: nil, items: [bad]),
        ])
        XCTAssertEqual(codeAndIndex(result)?.0, "model_tree_depth_jump")
        XCTAssertEqual(applier.store.getModel(model)?.treeShaped, false, "rolled back")
        let good = ModelItem(itemID: ItemId(0), value: .null, properties: [.descendantCount: .unsignedInt(2)])
        let child = ModelItem(itemID: ItemId(1), value: .null, properties: [.treeDepth: .unsignedInt(1)])
        let child2 = ModelItem(itemID: ItemId(2), value: .null, properties: [.treeDepth: .unsignedInt(1)])
        XCTAssertNil(codeAndIndex(applier.apply(baseRevision: applier.lastAppliedRevision, operations: [
            .modelUpdate(id: model, index: nil, items: [good, child, child2]),
        ])))
        XCTAssertEqual(applier.store.getModel(model)?.treeShaped, true)
    }

    private func loadCases(_ url: URL) throws -> [[String: Any]] {
        let table = try XCTUnwrap(
            JSONSerialization.jsonObject(with: Data(contentsOf: url)) as? [String: Any]
        )
        return try XCTUnwrap(table["cases"] as? [[String: Any]])
    }

    private func assertCases(_ cases: [[String: Any]]) throws {
        for row in cases {
            let name = try XCTUnwrap(row["name"] as? String)
            let itemCount = try XCTUnwrap((row["item_count"] as? NSNumber)?.uint64Value)
            let maxDepth = try XCTUnwrap((row["max_model_tree_depth"] as? NSNumber)?.intValue)
            let rows = try XCTUnwrap(row["items"] as? [[Any]])
            var built = Model(id: model, modelType: .tree, itemCount: itemCount)
            // Contiguous runs go in through the ordinary reset path, as in the Rust loader.
            var runs: [(UInt64, [ModelItem])] = []
            for cell in rows {
                let index = try XCTUnwrap((cell[0] as? NSNumber)?.uint64Value)
                var props: [PropertyRef: Value] = [:]
                if let v = shapeValue(cell[1]) { props[.treeDepth] = v }
                if let v = shapeValue(cell[2]) { props[.descendantCount] = v }
                let item = ModelItem(itemID: ItemId(1000 + index), value: .null, properties: props)
                if let last = runs.last, last.0 + UInt64(last.1.count) == index {
                    runs[runs.count - 1].1.append(item)
                } else {
                    runs.append((index, [item]))
                }
            }
            for (start, items) in runs {
                try built.resetRange(startIndex: start, items: items, totalCount: nil)
            }
            var actual: [String: Any]? = nil
            do {
                try built.validateTreeShape(maxModelTreeDepth: maxDepth)
            } catch StoreError.maxModelTreeDepthExceeded(_, let index, _, _) {
                actual = ["code": "max_model_tree_depth_exceeded", "index": index]
            } catch StoreError.invalidModelTree(_, let index, let violation) {
                actual = ["code": violation.conformanceCode, "index": index]
            }
            if let expected = row["expected"] as? [String: Any] {
                XCTAssertEqual(actual?["code"] as? String, expected["code"] as? String, name)
                XCTAssertEqual(actual?["index"] as? UInt64, (expected["index"] as? NSNumber)?.uint64Value, name)
            } else {
                XCTAssertNil(actual, "\(name) must be accepted")
            }
        }
    }

    /// Case-table cells: `null` is absent, a non-negative JSON integer is a shape, a negative one is
    /// a signed integer, and a string stays a string — exactly as the Rust loader builds them.
    private func shapeValue(_ raw: Any) -> Value? {
        if raw is NSNull { return nil }
        if let s = raw as? String { return .string(s) }
        guard let n = raw as? NSNumber else { return .null }
        return n.int64Value < 0 ? .signedInt(n.int64Value) : .unsignedInt(n.uint64Value)
    }

    // MARK: - Transaction semantics

    func testAFlatModelWithoutTreePropertiesIsNeverShapeChecked() throws {
        let applier = TransactionApplier()
        let flat = (0..<5).map { ModelItem(itemID: ItemId(UInt64($0)), value: .null) }
        _ = applier.apply(baseRevision: .initial, operations: [
            .createModel(id: model, modelType: .tree, itemCount: 0),
            .modelInsert(id: model, index: 0, items: flat),
        ])
        XCTAssertEqual(applier.store.getModel(model)?.treeShaped, false)
    }

    func testSubtreeInsertWithAncestorUpdateCommitsAsOneEdit() throws {
        let applier = try applierWithNested()
        let result = applier.apply(baseRevision: applier.lastAppliedRevision, operations: [
            .modelInsert(id: model, index: 4, items: [treeItem(7, 1, 0)]),
            .modelUpdate(id: model, index: nil, items: [treeItem(1, 0, 4)]),
        ])
        XCTAssertNil(codeAndIndex(result))
        XCTAssertEqual(applier.store.getModel(model)?.indexOf(ItemId(7)), 4)
    }

    func testTheFailureIsReportedAtTheModelsLastOperationAndRollsBack() throws {
        let applier = try applierWithNested()
        let before = applier.store
        let result = applier.apply(baseRevision: applier.lastAppliedRevision, operations: [
            .modelInsert(id: model, index: 4, items: [treeItem(7, 1, 0)]),
            .modelUpdate(id: model, index: nil, items: [treeItem(1, 0, 3)]),
        ])
        XCTAssertEqual(codeAndIndex(result)?.0, "model_tree_outside_parent_span")
        XCTAssertEqual(codeAndIndex(result)?.1, 1)
        XCTAssertEqual(applier.store, before)
    }

    func testAnUnrelatedLaterOperationDoesNotMoveTheCheckpoint() throws {
        let applier = try applierWithNested()
        let result = applier.apply(baseRevision: applier.lastAppliedRevision, operations: [
            .modelInsert(id: model, index: 4, items: [treeItem(7, 1, 0)]),
            .createModel(id: ModelId(11), modelType: .list, itemCount: 0),
        ])
        XCTAssertEqual(codeAndIndex(result)?.0, "model_tree_outside_parent_span")
        XCTAssertEqual(codeAndIndex(result)?.1, 0)
    }

    func testEveryApplyPathEnforcesTheShape() throws {
        let bad: [StoreOperation] = [
            .createModel(id: model, modelType: .tree, itemCount: 0),
            .modelInsert(id: model, index: 0, items: [treeItem(1, 0, 1), treeItem(2, 2, 0)]),
        ]
        XCTAssertEqual(codeAndIndex(TransactionApplier().apply(baseRevision: .initial, operations: bad))?.0,
                       "model_tree_depth_jump")
        XCTAssertEqual(codeAndIndex(TransactionApplier().apply(record: Transaction(baseRevision: .initial, operations: bad)))?.1, 1)
        switch TransactionApplier().applyDelivered(record: Transaction(baseRevision: .initial, operations: bad)) {
        case .failure(.opFailed(1, let source)): XCTAssertEqual(source.conformanceCode, "model_tree_depth_jump")
        default: XCTFail("delivered path must refuse the shape")
        }
        let snapshot = Transaction(baseRevision: .initial, newRevision: Revision(5), operations: bad)
        switch TransactionApplier().prepareResyncSnapshot(record: snapshot) {
        case .failure(.opFailed(1, let source)): XCTAssertEqual(source.conformanceCode, "model_tree_depth_jump")
        default: XCTFail("snapshot path must refuse the shape")
        }
        var store = SemanticStore()
        XCTAssertThrowsError(try store.apply(bad)) { error in
            XCTAssertEqual((error as? StoreError)?.conformanceCode, "model_tree_depth_jump")
        }
        XCTAssertNil(store.getModel(model), "the batch apply is all-or-nothing")
        // A lone operation is its own last touch.
        var single = SemanticStore()
        try single.apply(bad[0])
        XCTAssertThrowsError(try single.apply(bad[1])) { error in
            XCTAssertEqual((error as? StoreError)?.conformanceCode, "model_tree_depth_jump")
        }
    }

    // MARK: - §26 limit at and past its bound

    private func chain(_ levels: UInt64) -> [ModelItem] {
        (0..<levels).map { treeItem(100 + $0, $0, levels - 1 - $0) }
    }

    private func applyChain(_ limits: StoreLimits, _ levels: UInt64) -> Result<Revision, TxnError> {
        TransactionApplier(limits: limits).apply(baseRevision: .initial, operations: [
            .createModel(id: model, modelType: .tree, itemCount: 0),
            .modelInsert(id: model, index: 0, items: chain(levels)),
        ])
    }

    func testDefaultLimitAdmits256LevelsAndRefuses257() {
        XCTAssertEqual(defaultMaxModelTreeDepth, 256)
        XCTAssertEqual(StoreLimits().maxModelTreeDepth, 256)
        XCTAssertNil(codeAndIndex(applyChain(StoreLimits(), 256)))
        // Depth 255 is the deepest level, so the item there cannot hold the 257th level.
        guard case .failure(.opFailed(1, .maxModelTreeDepthExceeded(_, 255, 256, 255))) = applyChain(StoreLimits(), 257) else {
            return XCTFail("expected the depth limit at index 255")
        }
    }

    func testCustomLimitIsEnforcedAtAndPastItsBound() {
        let limits = StoreLimits().withMaxModelTreeDepth(3)
        XCTAssertNil(codeAndIndex(applyChain(limits, 3)))
        XCTAssertEqual(codeAndIndex(applyChain(limits, 4))?.0, "max_model_tree_depth_exceeded")
    }

    func testTheDepthCheckPrecedesStructuralChecksForTheSameItem() {
        let result = TransactionApplier().apply(baseRevision: .initial, operations: [
            .createModel(id: model, modelType: .tree, itemCount: 0),
            .modelInsert(id: model, index: 0, items: [treeItem(1, 300, 0)]),
        ])
        XCTAssertEqual(codeAndIndex(result)?.0, "max_model_tree_depth_exceeded")
    }

    func testAStringDepthIsAnInvalidProperty() {
        let item = ModelItem(itemID: ItemId(1), value: .null, properties: [.treeDepth: .string("0")])
        let result = TransactionApplier().apply(baseRevision: .initial, operations: [
            .createModel(id: model, modelType: .tree, itemCount: 0),
            .modelInsert(id: model, index: 0, items: [item]),
        ])
        guard case .failure(.opFailed(1, .invalidModelTree(_, 0, .invalidProperty(let property)))) = result else {
            return XCTFail("expected invalid_model_tree_property, got \(result)")
        }
        XCTAssertEqual(property, "tree_depth")
    }

    func testItemAccessorsReadTheShape() {
        let item = treeItem(1, 3, 2)
        XCTAssertEqual(item.treeDepth, 3)
        XCTAssertEqual(item.descendantCount, 2)
        XCTAssertEqual(ModelItem(itemID: ItemId(2), value: .null).treeDepth, 0)
    }
}
