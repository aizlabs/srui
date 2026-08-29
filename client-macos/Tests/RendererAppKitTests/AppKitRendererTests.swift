import AppKit
import SemanticModel
import Testing
@testable import RendererAppKit

@MainActor
struct AppKitRendererTests {
    @Test
    func scalarUpdatePreservesViewAndSurfaceWindowIdentity() throws {
        let base: [SemanticModel.Operation] = [
            .createNode(id: 1, nodeType: .surface),
            .createNode(
                id: 2,
                nodeType: .text,
                parentID: 1,
                properties: [(.text, .string("Hello"))]
            ),
        ]
        let store = try makeStore(base)
        let renderer = AppKitRenderer()

        try renderer.attach(store: store)
        let textBefore = try #require(renderer.registry.view(for: 2))
        let surfaceWindowBefore = try #require(renderer.registry.handle(for: 1)?.window)

        let scalar = SemanticModel.Operation.setProperty(id: 2, property: .text, value: .string("Updated"))
        let newStore = try makeStore(base + [scalar])

        let classifications = try renderer.apply(
            transaction: Transaction(baseRevision: store.revision, operations: [scalar]),
            newStore: newStore
        )

        #expect(
            classifications.contains {
                if case .contentOnly(nodeID: 2, property: .text) = $0 { return true }
                return false
            }
        )
        let textAfter = try #require(renderer.registry.view(for: 2))
        let surfaceWindowAfter = try #require(renderer.registry.handle(for: 1)?.window)

        #expect(textBefore === textAfter)
        #expect(surfaceWindowBefore === surfaceWindowAfter)
        #expect((textAfter as? NSTextField)?.stringValue == "Updated")
    }

    @Test
    func attachIsIdempotentForTheSameCommittedStore() throws {
        let store = try makeStore([
            .createNode(id: 1, nodeType: .surface),
            .createNode(id: 2, nodeType: .text, parentID: 1),
        ])
        let renderer = AppKitRenderer()

        try renderer.attach(store: store)
        let firstHandles = Set(renderer.registry.allHandles.map { ObjectIdentifier($0.view) })

        try renderer.attach(store: store)
        let secondHandles = Set(renderer.registry.allHandles.map { ObjectIdentifier($0.view) })

        #expect(renderer.registry.count == 2)
        #expect(renderer.registry.view(for: 1) != nil)
        #expect(renderer.registry.view(for: 2) != nil)
        #expect(firstHandles.isDisjoint(with: secondHandles))
        assertEveryHandleIsMounted(in: renderer)
    }

    @Test
    func structuralApplyRemovesDetachedViewsFromTheHierarchy() throws {
        let base: [SemanticModel.Operation] = [
            .createNode(id: 1, nodeType: .surface),
            .createNode(id: 2, nodeType: .text, parentID: 1),
        ]
        let store = try makeStore(base)
        let renderer = AppKitRenderer()

        try renderer.attach(store: store)
        let detachedText = try #require(renderer.registry.view(for: 2))

        let structural = SemanticModel.Operation.deleteNode(id: 2)
        let newStore = try makeStore([
            .createNode(id: 1, nodeType: .surface),
        ])

        try renderer.apply(
            transaction: Transaction(baseRevision: store.revision, operations: [structural]),
            newStore: newStore
        )

        #expect(renderer.registry.view(for: 2) == nil)
        #expect(detachedText.superview == nil)
        #expect(detachedText.window == nil)
        assertEveryHandleIsMounted(in: renderer)
    }

    @Test
    func scalarApplyThrowsWhenRenderHandleIsMissing() throws {
        let store = try makeStore([
            .createNode(id: 1, nodeType: .surface),
        ])
        let renderer = AppKitRenderer()
        try renderer.attach(store: store)

        let scalar = SemanticModel.Operation.setProperty(id: 2, property: .text, value: .string("orphan"))
        let newStore = try makeStore([
            .createNode(id: 1, nodeType: .surface),
            .createNode(id: 2, nodeType: .text, parentID: 1),
            scalar,
        ])

        #expect(throws: LayoutRendererError.missingRenderHandle(2)) {
            try renderer.apply(
                transaction: Transaction(baseRevision: store.revision, operations: [scalar]),
                newStore: newStore
            )
        }
    }

    @Test
    func scalarApplyThrowsWhenSemanticNodeIsMissing() throws {
        let store = try makeStore([
            .createNode(id: 1, nodeType: .surface),
            .createNode(id: 2, nodeType: .text, parentID: 1),
        ])
        let renderer = AppKitRenderer()
        try renderer.attach(store: store)

        let scalar = SemanticModel.Operation.setProperty(id: 2, property: .text, value: .string("gone"))
        var newStore = store
        _ = try newStore.deleteNode(2)

        #expect(throws: LayoutRendererError.missingSemanticNode(2)) {
            try renderer.apply(
                transaction: Transaction(baseRevision: store.revision, operations: [scalar]),
                newStore: newStore
            )
        }
    }

    @Test
    func appKitRendererForwardsAllThreeSemanticInteractions() throws {
        var receivedInteractions: [SemanticInteraction] = []
        let renderer = AppKitRenderer()
        renderer.onInteraction = { interaction in
            receivedInteractions.append(interaction)
        }

        let modelID = ModelId(100)
        var store = SemanticStore()
        try store.createModel(id: modelID, modelType: .table, itemCount: 2)
        try store.modelInsert(
            id: modelID,
            index: 0,
            items: [
                ModelItem(itemID: ItemId(501), value: .string("Item 501")),
                ModelItem(itemID: ItemId(502), value: .string("Item 502")),
            ]
        )

        try store.createNode(id: 1, nodeType: .surface)
        try store.createNode(id: 10, nodeType: .button, parentID: 1, properties: [
            Property(property: .label, value: .string("Click"))
        ])
        try store.createNode(id: 20, nodeType: .toggle, parentID: 1, properties: [
            Property(property: .label, value: .string("Toggle"))
        ])
        try store.createNode(id: 30, nodeType: .table, parentID: 1, properties: [
            Property(property: .modelRef, value: .unsignedInt(modelID.value)),
            Property(property: .selectionMode, value: .enumToken(.selectionModeSingle)),
        ])

        try renderer.attach(store: store)

        // 1. Button click
        let buttonHandle = try #require(renderer.registry.handle(for: 10))
        let button = try #require(buttonHandle.view as? NSButton)
        let buttonTrampoline = try #require(buttonHandle.actionTrampoline as? ActionTrampoline)
        buttonTrampoline.performButtonAction(button)
        #expect(receivedInteractions == [.activate(nodeID: 10)])
        receivedInteractions.removeAll()

        // 2. Toggle click
        let toggleHandle = try #require(renderer.registry.handle(for: 20))
        let toggle = try #require(toggleHandle.view as? NSButton)
        let toggleTrampoline = try #require(toggleHandle.actionTrampoline as? ActionTrampoline)
        toggle.state = .on
        toggleTrampoline.performToggleAction(toggle)
        #expect(receivedInteractions == [.valueChanged(nodeID: 20, value: .bool(true))])
        receivedInteractions.removeAll()

        // 3. Table row selection
        let tableHandle = try #require(renderer.registry.handle(for: 30))
        let tableView = try #require((tableHandle.view as? NSScrollView)?.documentView as? NSTableView)
        tableView.selectRowIndexes(IndexSet(integer: 1), byExtendingSelection: false)
        #expect(receivedInteractions == [.selectionChanged(nodeID: 30, itemID: ItemId(502))])
    }

    @Test
    func appKitRendererAppliesModelMutationsIncrementally() throws {
        let modelID = ModelId(200)
        var store = SemanticStore()
        try store.createModel(id: modelID, modelType: .table, itemCount: 1)
        try store.modelInsert(id: modelID, index: 0, items: [ModelItem(itemID: ItemId(1), value: .string("Initial Item"))])

        try store.createNode(id: 1, nodeType: .surface)
        try store.createNode(id: 2, nodeType: .table, parentID: 1, properties: [
            Property(property: .modelRef, value: .unsignedInt(modelID.value)),
        ])

        let renderer = AppKitRenderer()
        try renderer.attach(store: store)

        let tableBefore = try #require(renderer.registry.view(for: 2))
        let windowBefore = try #require(renderer.registry.handle(for: 1)?.window)
        let adapter = try #require(renderer.registry.handle(for: 2)?.modelAdapter as? TableCollectionAdapter)
        #expect(adapter.rows.map(\.cells) == [["Initial Item"]])

        let insertOp = Operation.modelInsert(
            id: modelID,
            index: 1,
            items: [ModelItem(itemID: ItemId(2), value: .string("Second Item"))]
        )
        var newStore = store
        try insertOp.apply(to: &newStore)

        let classifications = try renderer.apply(
            transaction: Transaction(baseRevision: store.revision, operations: [insertOp]),
            newStore: newStore
        )

        #expect(classifications == [.modelContent(modelID: modelID)])
        #expect(renderer.registry.view(for: 2) === tableBefore)
        #expect(renderer.registry.handle(for: 1)?.window === windowBefore)
        #expect(adapter.rows.map(\.cells) == [["Initial Item"], ["Second Item"]])
    }

    private func makeStore(_ operations: [SemanticModel.Operation]) throws -> SemanticStore {
        var store = SemanticStore()
        for operation in operations {
            try operation.apply(to: &store)
        }
        return store
    }

    private func assertEveryHandleIsMounted(in renderer: AppKitRenderer) {
        for handle in renderer.registry.allHandles {
            if handle.nodeType == .surface {
                #expect(handle.window != nil)
                continue
            }
            #expect(handle.view.superview != nil || handle.view.window != nil)
        }
    }
}
