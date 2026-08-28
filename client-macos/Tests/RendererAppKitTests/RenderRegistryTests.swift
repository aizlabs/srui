import SemanticModel
import Testing
@testable import RendererAppKit

@MainActor
struct RenderRegistryTests {
    @Test
    func registersLooksUpAndRemovesHandleByNodeID() throws {
        let registry = RenderRegistry()
        let handle = try ControlFactory().makeHandle(
            for: Node(id: 41, nodeType: .button)
        )

        try registry.register(handle)

        #expect(registry.count == 1)
        #expect(registry.handle(for: 41) === handle)
        #expect(registry.view(for: 41) === handle.view)
        #expect(registry.remove(41) === handle)
        #expect(registry.isEmpty)
    }

    @Test
    func rejectsDuplicateNodeID() throws {
        let registry = RenderRegistry()
        let factory = ControlFactory()
        let first = try factory.makeHandle(for: Node(id: 7, nodeType: .text))
        let second = try factory.makeHandle(for: Node(id: 7, nodeType: .button))
        try registry.register(first)

        #expect(throws: RenderRegistryError.duplicateNodeID(7)) {
            try registry.register(second)
        }
        #expect(registry.handle(for: 7) === first)
    }

    @Test
    func removesRegisteredSubtree() throws {
        let registry = RenderRegistry()
        let factory = ControlFactory()
        let root = try factory.makeHandle(
            for: Node(id: 1, nodeType: .column, orderedChildren: [2])
        )
        let child = try factory.makeHandle(
            for: Node(id: 2, nodeType: .text, parentID: 1)
        )
        try registry.register(root)
        try registry.register(child)

        let removed = registry.removeSubtree(rootID: 1)

        #expect(removed.count == 2)
        #expect(registry.isEmpty)
    }
}
