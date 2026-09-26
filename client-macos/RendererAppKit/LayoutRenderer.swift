import AppKit
import SemanticModel
import Text

public enum LayoutRendererError: Error, Equatable, Sendable {
    case missingSemanticNode(NodeId)
    case missingRenderHandle(NodeId)
    case missingParentHandle(NodeId)
}

/// Retained hierarchy coordinator. Scalar mutations and model content updates update existing
/// handles in place; graph mutations and model creation take a conservative full-remount path.
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

    public func mount(store: SemanticStore, preserveLocalText: Bool = false) throws {
        RendererDiagnostics.log(
            "mount begin revision=\(store.revision) roots=\(store.rootIDs.count)"
        )
        let remount: () throws -> Void = {
            // Window close emits end-editing synchronously. That callback must not flush
            // pending drafts before rebuild, or the new adapter reapplies the stale store string (§22.6).
            self.tearDown()
            for rootID in store.rootIDs {
                try self.mount(nodeID: rootID, from: store)
            }
        }
        if preserveLocalText {
            try controlFactory.textEditingSession.withPreservedLocalText(remount)
        } else {
            try remount()
        }
        // tearDown() closed the previous surfaces; a remount must not leave the UI invisible.
        if surfacesShown {
            showWindows()
        }
        let present = Set(registry.allHandles.map(\.nodeID))
        controlFactory.textEditingSession.syncPresentNodes(present)
        Task {
            await controlFactory.terminalSession.syncPresentNodes(present)
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
            try mount(store: newStore, preserveLocalText: true)
            return classifications
        }

        RendererDiagnostics.log(
            "transaction revision=\(transaction.newRevision) non-structural operations=\(transaction.operations.count)"
        )

        let changedModelIDs = Set(classifications.compactMap { classification -> ModelId? in
            if case .modelContent(let modelID) = classification {
                return modelID
            }
            return nil
        })

        var affectedCollectionNodeIDs: Set<NodeId> = []
        for operation in transaction.operations {
            switch operation {
            case .setProperty(let nodeID, let property, _),
                 .clearProperty(let nodeID, let property):
                guard !controlFactory.extensionMountResolver.isSuppressedFallbackNode(
                    nodeID,
                    in: newStore
                ) else { continue }
                if isCollectionProperty(property) {
                    affectedCollectionNodeIDs.insert(nodeID)
                } else {
                    try apply(property: property, nodeID: nodeID, store: newStore)
                }

            case .batchPropertySet(let nodeID, let properties):
                guard !controlFactory.extensionMountResolver.isSuppressedFallbackNode(
                    nodeID,
                    in: newStore
                ) else { continue }
                for property in properties {
                    if isCollectionProperty(property.property) {
                        affectedCollectionNodeIDs.insert(nodeID)
                    } else {
                        try apply(property: property.property, nodeID: nodeID, store: newStore)
                    }
                }

            default:
                break
            }
        }

        if !changedModelIDs.isEmpty {
            for handle in registry.allHandles {
                if let node = newStore.getNode(handle.nodeID),
                   let modelRef = node.modelRef,
                   changedModelIDs.contains(modelRef) {
                    affectedCollectionNodeIDs.insert(handle.nodeID)
                }
            }
        }

        for nodeID in affectedCollectionNodeIDs {
            guard let handle = registry.handle(for: nodeID) else {
                throw LayoutRendererError.missingRenderHandle(nodeID)
            }
            guard let node = newStore.getNode(nodeID) else {
                throw LayoutRendererError.missingSemanticNode(nodeID)
            }
            controlFactory.refreshCollection(in: handle, for: node, store: newStore)
            configureCollectionScrolling(for: handle)
            RendererDiagnostics.log(
                "refreshed collection node=\(nodeID) view=\(ObjectIdentifier(handle.view))"
            )
        }

        return classifications
    }

    public func showWindows() {
        surfacesShown = true
        let presentation = SurfacePresentation.forHostApplication()
        for handle in registry.surfaceHandles {
            guard let window = handle.window else { continue }
            presentation.present(window)
        }
        RendererDiagnostics.log(
            "showWindows presentation=\(presentation) surfaces=\(registry.surfaceHandles.count)"
        )
    }

    /// Validates extension negotiation and fallback structure without mutating AppKit state.
    public func validateExtensionMounts(in store: SemanticStore) throws {
        try controlFactory.extensionMountResolver.validateMountableExtensions(in: store)
    }
    private func mount(nodeID: NodeId, from store: SemanticStore) throws {
        guard let node = store.getNode(nodeID) else {
            throw LayoutRendererError.missingSemanticNode(nodeID)
        }

        let mountDecision = controlFactory.extensionMountResolver.decision(for: node, in: store)
        let handle = try controlFactory.makeHandle(
            for: node,
            store: store,
            mountDecision: mountDecision
        )
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
        if case .native = mountDecision {
            return
        }
        for childID in node.orderedChildren {
            try mount(nodeID: childID, from: store)
        }
        configureCollectionScrolling(for: handle)
    }

    private func attach(_ child: NSView, to parent: RenderHandle) {
        if let stack = parent.view as? NSStackView {
            stack.addArrangedSubview(child)
            reconcileFillConstraints(for: parent)
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
        if property == .text, ControlFactory.shouldSkipTextFallback(for: handle, node: node) {
            return
        }
        let appliedValue: Value?
        if property == .value, handle.textAdapter != nil {
            appliedValue = ControlFactory.displayedEditorText(for: node)
        } else {
            appliedValue = node.getProperty(property)
        }
        controlFactory.apply(
            property: property,
            value: appliedValue,
            to: handle,
            store: store
        )
        // `.paddingRole` rewrites the stack's `edgeInsets`, and the fill constraints hold that
        // inset as a constant: without a rebuild children stay sized for the previous padding.
        if property == .horizontalAlignment
            || property == .verticalAlignment
            || property == .paddingRole {
            reconcileFillConstraints(for: handle)
        }
        RendererDiagnostics.log(
            "updated node=\(nodeID) property=\(property) view=\(ObjectIdentifier(handle.view))"
        )
    }

    private func reconcileFillConstraints(for handle: RenderHandle) {
        guard let stack = handle.view as? NSStackView else { return }
        let property: PropertyRef
        let shouldFill: Bool
        let inset: CGFloat
        if stack.orientation == .vertical {
            property = .horizontalAlignment
            shouldFill = handle.layoutMetadata.horizontalAlignment == .horizontalAlignmentFill
                || !handle.nodeType.isStandard
            inset = stack.edgeInsets.left + stack.edgeInsets.right
        } else {
            property = .verticalAlignment
            shouldFill = handle.layoutMetadata.verticalAlignment == .verticalAlignmentFill
            inset = stack.edgeInsets.top + stack.edgeInsets.bottom
        }

        handle.propertyConstraints[property]?.forEach { $0.isActive = false }
        guard shouldFill else {
            handle.propertyConstraints[property] = []
            return
        }

        let constraints = stack.arrangedSubviews.map { child in
            let constraint: NSLayoutConstraint
            if stack.orientation == .vertical {
                constraint = child.widthAnchor.constraint(
                    equalTo: stack.widthAnchor,
                    constant: -inset
                )
            } else {
                constraint = child.heightAnchor.constraint(
                    equalTo: stack.heightAnchor,
                    constant: -inset
                )
            }
            constraint.priority = .init(999)
            return constraint
        }
        NSLayoutConstraint.activate(constraints)
        handle.propertyConstraints[property] = constraints
    }

    private func isCollectionProperty(_ property: PropertyRef) -> Bool {
        property == .items || property == .modelRef || property == .columns || property == .selectionMode
    }

    private func configureCollectionScrolling(for handle: RenderHandle) {
        guard handle.nodeType == .list || handle.nodeType == .table || handle.nodeType == .tree else {
            return
        }
        controlFactory.configureCollectionScrolling(
            nested: isInsideScrollContainer(handle),
            handle: handle
        )
    }

    private func isInsideScrollContainer(_ handle: RenderHandle) -> Bool {
        var parentID = handle.parentID
        while let id = parentID, let parent = registry.handle(for: id) {
            if parent.nodeType == .scroll {
                return true
            }
            parentID = parent.parentID
        }
        return false
    }

    private func tearDown() {
        let handles = registry.removeAll()
        for handle in handles {
            handle.view.removeFromSuperview()
            handle.window?.close()
        }
    }
}
