import AppKit
import SemanticModel
import Testing
@testable import RendererAppKit

@MainActor
struct LayoutRendererTests {
    @Test
    func surfaceWindowIsNotReleasedWhenClosed() throws {
        let handle = try ControlFactory().makeHandle(
            for: Node(id: 1, nodeType: .surface)
        )

        // tearDown() closes surface windows while RenderHandle still holds a strong
        // reference; AppKit's default isReleasedWhenClosed would over-release them.
        #expect(handle.window?.isReleasedWhenClosed == false)
    }

    @Test
    func structuralRemountKeepsSurfaceWindowsVisible() throws {
        let base: [SemanticModel.Operation] = [
            .createNode(id: 1, nodeType: .surface),
            .createNode(id: 2, nodeType: .text, parentID: 1),
        ]
        let store = try makeStore(base)
        let renderer = AppKitRenderer()

        try renderer.attach(store: store)
        renderer.showWindows()
        #expect(renderer.registry.surfaceHandles.allSatisfy { $0.window?.isVisible == true })

        let structural = SemanticModel.Operation.createNode(id: 3, nodeType: .text, parentID: 1)
        let newStore = try makeStore(base + [structural])
        try renderer.apply(
            transaction: Transaction(baseRevision: store.revision, operations: [structural]),
            newStore: newStore
        )

        #expect(renderer.registry.surfaceHandles.count == 1)
        #expect(renderer.registry.surfaceHandles.allSatisfy { $0.window?.isVisible == true })
    }

    @Test
    func scalarPropertyUpdatePreservesViewIdentityWithoutRemounting() throws {
        let base: [SemanticModel.Operation] = [
            .createNode(id: 1, nodeType: .surface),
            .createNode(
                id: 2,
                nodeType: .text,
                parentID: 1,
                properties: [(.text, .string("Before"))]
            ),
        ]
        let store = try makeStore(base)
        let renderer = LayoutRenderer()

        try renderer.mount(store: store)
        let textBefore = try #require(renderer.registry.view(for: 2))
        let surfaceWindowBefore = try #require(renderer.registry.handle(for: 1)?.window)

        let scalar = SemanticModel.Operation.setProperty(
            id: 2,
            property: .text,
            value: .string("After")
        )
        let newStore = try makeStore(base + [scalar])

        _ = try renderer.apply(
            transaction: Transaction(baseRevision: store.revision, operations: [scalar]),
            newStore: newStore
        )

        let textAfter = try #require(renderer.registry.view(for: 2))
        let surfaceWindowAfter = try #require(renderer.registry.handle(for: 1)?.window)

        #expect(textBefore === textAfter)
        #expect(surfaceWindowBefore === surfaceWindowAfter)
        #expect((textAfter as? NSTextField)?.stringValue == "After")
    }

    @Test
    func reorderChildrenRemountsWithoutOrphanSubviews() throws {
        let base: [SemanticModel.Operation] = [
            .createNode(id: 1, nodeType: .surface),
            .createNode(id: 2, nodeType: .column, parentID: 1),
            .createNode(
                id: 3,
                nodeType: .text,
                parentID: 2,
                properties: [(.text, .string("First"))]
            ),
            .createNode(
                id: 4,
                nodeType: .text,
                parentID: 2,
                properties: [(.text, .string("Second"))]
            ),
        ]
        let store = try makeStore(base)
        let renderer = LayoutRenderer()

        try renderer.mount(store: store)
        let firstBefore = try #require(renderer.registry.view(for: 3))
        let secondBefore = try #require(renderer.registry.view(for: 4))

        let structural = SemanticModel.Operation.reorderChildren(parentID: 2, newOrder: [4, 3])
        let newStore = try makeStore(base + [structural])

        try renderer.apply(
            transaction: Transaction(baseRevision: store.revision, operations: [structural]),
            newStore: newStore
        )

        let column = try #require(renderer.registry.view(for: 2) as? NSStackView)
        let arranged = column.arrangedSubviews
        #expect(arranged.count == 2)
        #expect((arranged[0] as? NSTextField)?.stringValue == "Second")
        #expect((arranged[1] as? NSTextField)?.stringValue == "First")
        #expect(firstBefore.superview == nil)
        #expect(secondBefore.superview == nil)
        assertEveryHandleIsMounted(in: renderer)
    }

    @Test
    func moveNodeRemountsChildUnderNewParent() throws {
        let base: [SemanticModel.Operation] = [
            .createNode(id: 1, nodeType: .surface),
            .createNode(id: 2, nodeType: .column, parentID: 1),
            .createNode(id: 3, nodeType: .column, parentID: 1),
            .createNode(
                id: 4,
                nodeType: .text,
                parentID: 2,
                properties: [(.text, .string("Movable"))]
            ),
        ]
        let store = try makeStore(base)
        let renderer = LayoutRenderer()

        try renderer.mount(store: store)
        let movedBefore = try #require(renderer.registry.view(for: 4))

        let structural = SemanticModel.Operation.moveNode(id: 4, newParentID: 3, newChildIndex: 0)
        let newStore = try makeStore(base + [structural])

        try renderer.apply(
            transaction: Transaction(baseRevision: store.revision, operations: [structural]),
            newStore: newStore
        )

        let destination = try #require(renderer.registry.view(for: 3) as? NSStackView)
        let movedAfter = try #require(renderer.registry.view(for: 4))
        #expect(destination.arrangedSubviews.contains { $0 === movedAfter })
        #expect(movedBefore.superview == nil)
        assertEveryHandleIsMounted(in: renderer)
    }

    @Test
    func remountIsIdempotentAfterShowWindows() throws {
        let store = try makeStore([
            .createNode(id: 1, nodeType: .surface),
            .createNode(id: 2, nodeType: .text, parentID: 1),
        ])
        let renderer = LayoutRenderer()

        try renderer.mount(store: store)
        renderer.showWindows()
        #expect(renderer.registry.surfaceHandles.allSatisfy { $0.window?.isVisible == true })

        try renderer.mount(store: store)

        #expect(renderer.registry.count == 2)
        #expect(renderer.registry.surfaceHandles.count == 1)
        #expect(renderer.registry.surfaceHandles.allSatisfy { $0.window?.isVisible == true })
        assertEveryHandleIsMounted(in: renderer)
    }

    @Test
    func scrollNodeRetainsEveryChildInTheViewHierarchy() throws {
        let store = try makeStore([
            SemanticModel.Operation.createNode(id: 1, nodeType: .surface),
            .createNode(id: 2, nodeType: .scroll, parentID: 1),
            .createNode(id: 3, nodeType: .text, parentID: 2),
            .createNode(id: 4, nodeType: .text, parentID: 2),
        ])
        let renderer = AppKitRenderer()

        try renderer.attach(store: store)

        let first = try #require(renderer.registry.view(for: 3))
        let second = try #require(renderer.registry.view(for: 4))
        let scrollView = try #require(renderer.registry.view(for: 2) as? NSScrollView)

        #expect(first.superview != nil)
        #expect(second.superview != nil)
        #expect(first.superview === second.superview)
        #expect(first.isDescendant(of: scrollView))
        #expect(second.isDescendant(of: scrollView))
    }

    @Test
    func allFourModelMutationsUpdateTableInPlaceAndPreserveIdentity() throws {
        let modelID = ModelId(10)
        var store = SemanticStore()
        try store.createModel(id: modelID, modelType: .table, itemCount: 0)
        try store.modelInsert(
            id: modelID,
            index: 0,
            items: [
                ModelItem(itemID: ItemId(1), value: .string("Alpha")),
                ModelItem(itemID: ItemId(2), value: .string("Beta")),
            ]
        )
        try store.createNode(
            id: 1,
            nodeType: .surface,
            properties: [Property(property: .label, value: .string("Test Window"))]
        )
        try store.createNode(
            id: 2,
            nodeType: .table,
            parentID: 1,
            properties: [
                Property(property: .modelRef, value: .unsignedInt(modelID.value)),
                Property(property: .selectionMode, value: .enumToken(.selectionModeSingle)),
            ]
        )
        try store.createNode(
            id: 3,
            nodeType: .text,
            parentID: 1,
            properties: [Property(property: .text, value: .string("Status"))]
        )

        let renderer = LayoutRenderer()
        try renderer.mount(store: store)

        let windowBefore = try #require(renderer.registry.handle(for: 1)?.window)
        let tableBefore = try #require(renderer.registry.view(for: 2))
        let textBefore = try #require(renderer.registry.view(for: 3))

        let adapter = try #require(renderer.registry.handle(for: 2)?.modelAdapter as? TableCollectionAdapter)
        #expect(adapter.rows.map(\.cells) == [["Alpha"], ["Beta"]])

        // 1. MODEL_INSERT
        let insertOp = Operation.modelInsert(
            id: modelID,
            index: 1,
            items: [ModelItem(itemID: ItemId(3), value: .string("Gamma"))]
        )
        var storeAfterInsert = store
        try insertOp.apply(to: &storeAfterInsert)
        try renderer.apply(
            transaction: Transaction(baseRevision: store.revision, operations: [insertOp]),
            newStore: storeAfterInsert
        )
        #expect(adapter.rows.map(\.cells) == [["Alpha"], ["Gamma"], ["Beta"]])
        #expect(renderer.registry.handle(for: 1)?.window === windowBefore)
        #expect(renderer.registry.view(for: 2) === tableBefore)
        #expect(renderer.registry.view(for: 3) === textBefore)

        // 2. MODEL_UPDATE
        let updateOp = Operation.modelUpdate(
            id: modelID,
            index: 0,
            items: [ModelItem(itemID: ItemId(1), value: .string("Alpha Prime"))]
        )
        var storeAfterUpdate = storeAfterInsert
        try updateOp.apply(to: &storeAfterUpdate)
        try renderer.apply(
            transaction: Transaction(baseRevision: storeAfterInsert.revision, operations: [updateOp]),
            newStore: storeAfterUpdate
        )
        #expect(adapter.rows.map(\.cells) == [["Alpha Prime"], ["Gamma"], ["Beta"]])
        #expect(renderer.registry.view(for: 2) === tableBefore)

        // 3. MODEL_DELETE
        let deleteOp = Operation.modelDelete(id: modelID, index: 2, count: 1, itemIds: [])
        var storeAfterDelete = storeAfterUpdate
        try deleteOp.apply(to: &storeAfterDelete)
        try renderer.apply(
            transaction: Transaction(baseRevision: storeAfterUpdate.revision, operations: [deleteOp]),
            newStore: storeAfterDelete
        )
        #expect(adapter.rows.map(\.cells) == [["Alpha Prime"], ["Gamma"]])
        #expect(renderer.registry.view(for: 2) === tableBefore)

        // 4. MODEL_RESET_RANGE
        let resetOp = Operation.modelResetRange(
            id: modelID,
            startIndex: 0,
            items: [
                ModelItem(itemID: ItemId(10), value: .string("Fresh 1")),
                ModelItem(itemID: ItemId(20), value: .string("Fresh 2")),
            ],
            totalCount: 2
        )
        var storeAfterReset = storeAfterDelete
        try resetOp.apply(to: &storeAfterReset)
        try renderer.apply(
            transaction: Transaction(baseRevision: storeAfterDelete.revision, operations: [resetOp]),
            newStore: storeAfterReset
        )
        #expect(adapter.rows.map(\.cells) == [["Fresh 1"], ["Fresh 2"]])
        #expect(renderer.registry.view(for: 2) === tableBefore)
    }

    @Test
    func insertionDrivenIndexShiftsPreserveSelectionByItemId() throws {
        let modelID = ModelId(20)
        var store = SemanticStore()
        try store.createModel(id: modelID, modelType: .table, itemCount: 0)
        try store.modelInsert(
            id: modelID,
            index: 0,
            items: [
                ModelItem(itemID: ItemId(10), value: .string("Item 10")),
                ModelItem(itemID: ItemId(20), value: .string("Item 20")),
                ModelItem(itemID: ItemId(30), value: .string("Item 30")),
            ]
        )
        try store.createNode(id: 1, nodeType: .surface)
        try store.createNode(
            id: 2,
            nodeType: .table,
            parentID: 1,
            properties: [
                Property(property: .modelRef, value: .unsignedInt(modelID.value)),
                Property(property: .selectionMode, value: .enumToken(.selectionModeSingle)),
            ]
        )

        let renderer = LayoutRenderer()
        try renderer.mount(store: store)

        let tableHandle = try #require(renderer.registry.handle(for: 2))
        let tableView = try #require((tableHandle.view as? NSScrollView)?.documentView as? NSTableView)

        // User selects row 1 (ItemId 20)
        tableView.selectRowIndexes(IndexSet(integer: 1), byExtendingSelection: false)
        #expect(tableView.selectedRow == 1)

        // Insert item at index 0 -> item 20 moves to index 2
        let insertOp = Operation.modelInsert(
            id: modelID,
            index: 0,
            items: [ModelItem(itemID: ItemId(5), value: .string("Item 5"))]
        )
        var newStore = store
        try insertOp.apply(to: &newStore)
        try renderer.apply(
            transaction: Transaction(baseRevision: store.revision, operations: [insertOp]),
            newStore: newStore
        )

        // Selection should be preserved on row index 2 (ItemId 20)
        #expect(tableView.selectedRow == 2)
    }

    @Test
    func inPlaceChangesToColumnsModelRefAndSelectionMode() throws {
        let model1 = ModelId(100)
        let model2 = ModelId(200)

        var store = SemanticStore()
        try store.createModel(id: model1, modelType: .table, itemCount: 0)
        try store.modelInsert(id: model1, index: 0, items: [ModelItem(itemID: ItemId(1), value: .list([.string("M1-A"), .string("M1-B")]))])
        try store.createModel(id: model2, modelType: .table, itemCount: 0)
        try store.modelInsert(id: model2, index: 0, items: [ModelItem(itemID: ItemId(2), value: .list([.string("M2-A"), .string("M2-B")]))])

        try store.createNode(id: 1, nodeType: .surface)
        try store.createNode(
            id: 2,
            nodeType: .table,
            parentID: 1,
            properties: [
                Property(property: .columns, value: .list([.string("Col 1"), .string("Col 2")])),
                Property(property: .modelRef, value: .unsignedInt(model1.value)),
                Property(property: .selectionMode, value: .enumToken(.selectionModeNone)),
            ]
        )

        let renderer = LayoutRenderer()
        try renderer.mount(store: store)

        let tableHandle = try #require(renderer.registry.handle(for: 2))
        let tableIdentity = ObjectIdentifier(tableHandle.view)
        let tableView = try #require((tableHandle.view as? NSScrollView)?.documentView as? NSTableView)
        let adapter = try #require(tableHandle.modelAdapter as? TableCollectionAdapter)

        #expect(tableView.tableColumns.count == 2)
        #expect(adapter.rows.map(\.cells) == [["M1-A", "M1-B"]])
        #expect(tableView.selectionHighlightStyle == .none)

        // 1. Reconfigure columns in place
        let reconfigColumnsOp = Operation.setProperty(
            id: 2,
            property: .columns,
            value: .list([.string("Header A"), .string("Header B"), .string("Header C"), .string("Header D")])
        )
        var storeColumns = store
        try reconfigColumnsOp.apply(to: &storeColumns)
        try renderer.apply(
            transaction: Transaction(baseRevision: store.revision, operations: [reconfigColumnsOp]),
            newStore: storeColumns
        )
        #expect(ObjectIdentifier(tableHandle.view) == tableIdentity)
        #expect(tableView.tableColumns.count == 4)
        #expect(tableView.tableColumns[0].title == "Header A")
        #expect(tableView.tableColumns[3].title == "Header D")

        // 2. Reconfigure selectionMode in place
        let reconfigSelectionOp = Operation.setProperty(
            id: 2,
            property: .selectionMode,
            value: .enumToken(.selectionModeSingle)
        )
        var storeSelection = storeColumns
        try reconfigSelectionOp.apply(to: &storeSelection)
        try renderer.apply(
            transaction: Transaction(baseRevision: storeColumns.revision, operations: [reconfigSelectionOp]),
            newStore: storeSelection
        )
        #expect(ObjectIdentifier(tableHandle.view) == tableIdentity)
        #expect(tableView.selectionHighlightStyle == .regular)
        #expect(tableView.allowsMultipleSelection == false)

        // 3. Reconfigure modelRef in place
        let reconfigModelOp = Operation.setProperty(
            id: 2,
            property: .modelRef,
            value: .unsignedInt(model2.value)
        )
        var storeModel = storeSelection
        try reconfigModelOp.apply(to: &storeModel)
        try renderer.apply(
            transaction: Transaction(baseRevision: storeSelection.revision, operations: [reconfigModelOp]),
            newStore: storeModel
        )
        #expect(ObjectIdentifier(tableHandle.view) == tableIdentity)
        #expect(adapter.rows.map(\.cells) == [["M2-A", "M2-B"]])
    }

    @Test
    func multipleTablesReferencingSameModelAreBothUpdatedInPlace() throws {
        let modelID = ModelId(50)
        var store = SemanticStore()
        try store.createModel(id: modelID, modelType: .table, itemCount: 0)
        try store.modelInsert(id: modelID, index: 0, items: [ModelItem(itemID: ItemId(1), value: .string("Initial"))])

        try store.createNode(id: 1, nodeType: .surface)
        try store.createNode(id: 2, nodeType: .table, parentID: 1, properties: [
            Property(property: .modelRef, value: .unsignedInt(modelID.value)),
        ])
        try store.createNode(id: 3, nodeType: .table, parentID: 1, properties: [
            Property(property: .modelRef, value: .unsignedInt(modelID.value)),
        ])

        let renderer = LayoutRenderer()
        try renderer.mount(store: store)

        let table2View = try #require(renderer.registry.view(for: 2))
        let table3View = try #require(renderer.registry.view(for: 3))
        let adapter2 = try #require(renderer.registry.handle(for: 2)?.modelAdapter as? TableCollectionAdapter)
        let adapter3 = try #require(renderer.registry.handle(for: 3)?.modelAdapter as? TableCollectionAdapter)

        #expect(adapter2.rows.map(\.cells) == [["Initial"]])
        #expect(adapter3.rows.map(\.cells) == [["Initial"]])

        let insertOp = Operation.modelInsert(
            id: modelID,
            index: 1,
            items: [ModelItem(itemID: ItemId(2), value: .string("Second Item"))]
        )
        var newStore = store
        try insertOp.apply(to: &newStore)
        try renderer.apply(
            transaction: Transaction(baseRevision: store.revision, operations: [insertOp]),
            newStore: newStore
        )

        // Both tables updated in place with preserved view identity
        #expect(renderer.registry.view(for: 2) === table2View)
        #expect(renderer.registry.view(for: 3) === table3View)
        #expect(adapter2.rows.map(\.cells) == [["Initial"], ["Second Item"]])
        #expect(adapter3.rows.map(\.cells) == [["Initial"], ["Second Item"]])
    }

    @Test
    func tablesReferencingDifferentModelsOnlyUpdateReferencingHandle() throws {
        let model1 = ModelId(10)
        let model2 = ModelId(20)
        var store = SemanticStore()
        try store.createModel(id: model1, modelType: .table, itemCount: 0)
        try store.modelInsert(id: model1, index: 0, items: [ModelItem(itemID: ItemId(1), value: .string("M1 Item"))])
        try store.createModel(id: model2, modelType: .table, itemCount: 0)
        try store.modelInsert(id: model2, index: 0, items: [ModelItem(itemID: ItemId(2), value: .string("M2 Item"))])

        try store.createNode(id: 1, nodeType: .surface)
        try store.createNode(id: 2, nodeType: .table, parentID: 1, properties: [
            Property(property: .modelRef, value: .unsignedInt(model1.value)),
        ])
        try store.createNode(id: 3, nodeType: .table, parentID: 1, properties: [
            Property(property: .modelRef, value: .unsignedInt(model2.value)),
        ])

        let renderer = LayoutRenderer()
        try renderer.mount(store: store)

        let adapter2 = try #require(renderer.registry.handle(for: 2)?.modelAdapter as? TableCollectionAdapter)
        let adapter3 = try #require(renderer.registry.handle(for: 3)?.modelAdapter as? TableCollectionAdapter)

        // Mutate model 1 only
        let insertOp = Operation.modelInsert(
            id: model1,
            index: 1,
            items: [ModelItem(itemID: ItemId(10), value: .string("M1 New"))]
        )
        var newStore = store
        try insertOp.apply(to: &newStore)
        try renderer.apply(
            transaction: Transaction(baseRevision: store.revision, operations: [insertOp]),
            newStore: newStore
        )

        #expect(adapter2.rows.map(\.cells) == [["M1 Item"], ["M1 New"]])
        #expect(adapter3.rows.map(\.cells) == [["M2 Item"]])
    }

    @Test
    func mixedStructuralAndModelTransactionRemountsCleanly() throws {
        let modelID = ModelId(30)
        var store = SemanticStore()
        try store.createModel(id: modelID, modelType: .table, itemCount: 0)
        try store.modelInsert(id: modelID, index: 0, items: [ModelItem(itemID: ItemId(1), value: .string("Row 1"))])

        try store.createNode(id: 1, nodeType: .surface)
        try store.createNode(id: 2, nodeType: .table, parentID: 1, properties: [
            Property(property: .modelRef, value: .unsignedInt(modelID.value)),
        ])

        let renderer = LayoutRenderer()
        try renderer.mount(store: store)

        // Mixed transaction: createNode (structural) + modelInsert
        let createOp = Operation.createNode(id: 3, nodeType: .text, parentID: 1, properties: [
            Property(property: .text, value: .string("Footer Text"))
        ])
        let insertOp = Operation.modelInsert(
            id: modelID,
            index: 1,
            items: [ModelItem(itemID: ItemId(2), value: .string("Row 2"))]
        )

        var newStore = store
        try createOp.apply(to: &newStore)
        try insertOp.apply(to: &newStore)

        try renderer.apply(
            transaction: Transaction(baseRevision: store.revision, operations: [createOp, insertOp]),
            newStore: newStore
        )

        #expect(renderer.registry.count == 3)
        #expect(renderer.registry.handle(for: 3) != nil)
        let adapter = try #require(renderer.registry.handle(for: 2)?.modelAdapter as? TableCollectionAdapter)
        #expect(adapter.rows.map(\.cells) == [["Row 1"], ["Row 2"]])
    }

    @Test
    func columnReconciliationPreservesColumnObjectsWhileUpdatingTitles() throws {
        let node = Node(
            id: 1,
            nodeType: .table,
            properties: [
                .columns: .list([.string("Col A"), .string("Col B")]),
            ]
        )
        let handle = try ControlFactory().makeHandle(for: node)
        let tableView = try #require((handle.view as? NSScrollView)?.documentView as? NSTableView)

        #expect(tableView.tableColumns.count == 2)
        let col0Before = tableView.tableColumns[0]
        let col1Before = tableView.tableColumns[1]
        #expect(col0Before.title == "Col A")
        #expect(col1Before.title == "Col B")

        // Update columns: rename Col A -> First, rename Col B -> Second, add Third
        ControlFactory().reconcileColumns(
            in: tableView,
            columns: ["First", "Second", "Third"],
            fallbackTitle: "Table"
        )

        #expect(tableView.tableColumns.count == 3)
        #expect(tableView.tableColumns[0] === col0Before)
        #expect(tableView.tableColumns[0].title == "First")
        #expect(tableView.tableColumns[1] === col1Before)
        #expect(tableView.tableColumns[1].title == "Second")
        #expect(tableView.tableColumns[2].title == "Third")

        // Shrink columns down to 1
        ControlFactory().reconcileColumns(
            in: tableView,
            columns: ["Only One"],
            fallbackTitle: "Table"
        )
        #expect(tableView.tableColumns.count == 1)
        #expect(tableView.tableColumns[0] === col0Before)
        #expect(tableView.tableColumns[0].title == "Only One")
    }

    private func makeStore(_ operations: [SemanticModel.Operation]) throws -> SemanticStore {
        var store = SemanticStore()
        for operation in operations {
            try operation.apply(to: &store)
        }
        return store
    }

    private func assertEveryHandleIsMounted(in renderer: LayoutRenderer) {
        for handle in renderer.registry.allHandles {
            if handle.nodeType == .surface {
                #expect(handle.window != nil)
                continue
            }
            #expect(handle.view.superview != nil || handle.view.window != nil)
        }
    }
}
