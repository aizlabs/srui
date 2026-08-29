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
