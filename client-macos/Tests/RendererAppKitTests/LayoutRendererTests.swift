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
}
