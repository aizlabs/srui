import AppKit
import SemanticModel

public enum LayoutRendererError: Error, Equatable, Sendable {
    case missingSemanticNode(NodeId)
    case missingRenderHandle(NodeId)
    case missingParentHandle(NodeId)
}

/// Retained hierarchy coordinator. Scalar mutations update existing handles; structural
/// mutations currently take a conservative full-remount path.
@MainActor
public final class LayoutRenderer {
    public let registry: RenderRegistry
    public let controlFactory: ControlFactory

    /// Whether surfaces have been ordered on screen, so a remount can restore visibility.
    private var surfacesShown = false

    public init(
        registry: RenderRegistry = RenderRegistry(),
        controlFactory: ControlFactory = ControlFactory()
    ) {
        self.registry = registry
        self.controlFactory = controlFactory
    }

    public func mount(store: SemanticStore) throws {
        RendererDiagnostics.log(
            "mount begin revision=\(store.revision) roots=\(store.rootIDs.count)"
        )
        tearDown()
        for rootID in store.rootIDs {
            try mount(nodeID: rootID, from: store)
        }
        // tearDown() closed the previous surfaces; a remount must not leave the UI invisible.
        if surfacesShown {
            showWindows()
        }
        RendererDiagnostics.log("mount complete handles=\(registry.count)")
    }

    @discardableResult
    public func apply(
        transaction: Transaction,
        newStore: SemanticStore
    ) throws -> [DirtyClassification] {
        let classifications = DirtyClassifier.classify(transaction)
        if classifications.contains(where: \.isStructureAffecting) {
            RendererDiagnostics.log(
                "transaction revision=\(transaction.newRevision) structural; remounting"
            )
            try mount(store: newStore)
            return classifications
        }

        RendererDiagnostics.log(
            "transaction revision=\(transaction.newRevision) scalar operations=\(transaction.operations.count)"
        )
        for operation in transaction.operations {
            switch operation {
            case .setProperty(let nodeID, let property, _),
                 .clearProperty(let nodeID, let property):
                try apply(property: property, nodeID: nodeID, store: newStore)

            case .batchPropertySet(let nodeID, let properties):
                for property in properties {
                    try apply(property: property.property, nodeID: nodeID, store: newStore)
                }

            default:
                break
            }
        }
        return classifications
    }

    public func showWindows() {
        surfacesShown = true
        for handle in registry.surfaceHandles {
            handle.window?.makeKeyAndOrderFront(nil)
        }
    }

    private func mount(nodeID: NodeId, from store: SemanticStore) throws {
        guard let node = store.getNode(nodeID) else {
            throw LayoutRendererError.missingSemanticNode(nodeID)
        }

        let handle = try controlFactory.makeHandle(for: node)
        try registry.register(handle)
        RendererDiagnostics.log(
            "mounted node=\(node.id) type=\(node.nodeType) parent=\(node.parentID?.description ?? "root")"
        )

        if let parentID = node.parentID {
            guard let parentHandle = registry.handle(for: parentID) else {
                throw LayoutRendererError.missingParentHandle(parentID)
            }
            attach(handle.view, to: parentHandle)
        }

        for childID in node.orderedChildren {
            try mount(nodeID: childID, from: store)
        }
    }

    private func attach(_ child: NSView, to parent: RenderHandle) {
        if let stack = parent.view as? NSStackView {
            stack.addArrangedSubview(child)
            return
        }

        if let grid = parent.view as? NSGridView {
            grid.addRow(with: [child])
            return
        }

        if parent.nodeType == .scroll,
           let documentStack = (parent.view as? NSScrollView)?.documentView as? NSStackView {
            documentStack.addArrangedSubview(child)
            return
        }

        guard parent.nodeType != .list,
              parent.nodeType != .table,
              parent.nodeType != .tree else {
            return
        }

        parent.view.addSubview(child)
        let inset: CGFloat = parent.nodeType == .surface ? 16 : 0
        NSLayoutConstraint.activate([
            child.leadingAnchor.constraint(equalTo: parent.view.leadingAnchor, constant: inset),
            child.trailingAnchor.constraint(equalTo: parent.view.trailingAnchor, constant: -inset),
            child.topAnchor.constraint(equalTo: parent.view.topAnchor, constant: inset),
            child.bottomAnchor.constraint(equalTo: parent.view.bottomAnchor, constant: -inset),
        ])
    }

    private func apply(
        property: PropertyRef,
        nodeID: NodeId,
        store: SemanticStore
    ) throws {
        guard let handle = registry.handle(for: nodeID) else {
            throw LayoutRendererError.missingRenderHandle(nodeID)
        }
        guard let node = store.getNode(nodeID) else {
            throw LayoutRendererError.missingSemanticNode(nodeID)
        }
        controlFactory.apply(
            property: property,
            value: node.getProperty(property),
            to: handle
        )
        RendererDiagnostics.log(
            "updated node=\(nodeID) property=\(property) view=\(ObjectIdentifier(handle.view))"
        )
    }

    private func tearDown() {
        let handles = registry.removeAll()
        for handle in handles {
            handle.view.removeFromSuperview()
            handle.window?.close()
        }
    }
}
