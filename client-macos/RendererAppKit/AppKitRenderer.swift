import SemanticModel

/// Public main-actor entry point for mounting committed semantic state and applying render deltas.
@MainActor
public final class AppKitRenderer {
    public let registry: RenderRegistry
    public let controlFactory: ControlFactory
    public let layoutRenderer: LayoutRenderer

    public var onInteraction: (@MainActor (SemanticInteraction) -> Void)? {
        get { controlFactory.onInteraction }
        set { controlFactory.onInteraction = newValue }
    }

    public init() {
        let registry = RenderRegistry()
        let controlFactory = ControlFactory()
        self.registry = registry
        self.controlFactory = controlFactory
        self.layoutRenderer = LayoutRenderer(
            registry: registry,
            controlFactory: controlFactory
        )
    }

    public func attach(store: SemanticStore) throws {
        try layoutRenderer.mount(store: store)
    }

    @discardableResult
    public func apply(
        transaction: Transaction,
        newStore: SemanticStore
    ) throws -> [DirtyClassification] {
        try layoutRenderer.apply(transaction: transaction, newStore: newStore)
    }

    public func showWindows() {
        layoutRenderer.showWindows()
    }
}
