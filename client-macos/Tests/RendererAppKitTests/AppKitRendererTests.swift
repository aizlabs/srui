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
