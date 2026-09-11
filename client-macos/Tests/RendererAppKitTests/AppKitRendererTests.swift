import AppKit
import SemanticModel
import Testing
@testable import RendererAppKit
@testable import Collections
import CoreGraphics

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
    func nativeValueChangeGatesCadenceRasterAndIdleDoesNotRaster() throws {
        let base: [SemanticModel.Operation] = [
            .createNode(id: 1, nodeType: .surface),
            .createNode(
                id: 5,
                nodeType: .progress,
                parentID: 1,
                properties: [(.value, .float64(0))]
            ),
        ]
        let renderer = AppKitRenderer()
        let store = try makeStore(base)
        try renderer.attach(store: store)

        let root = try #require(
            renderer.registry.handle(for: 1)?.window?.contentView
        )
        let progress = try #require(
            renderer.registry.view(for: 5) as? NSProgressIndicator
        )
        var lastPaintedValue = progress.doubleValue

        func cadenceTick() throws -> Bool {
            guard progress.doubleValue != lastPaintedValue else {
                return false
            }
            root.layoutSubtreeIfNeeded()
            let rect = root.bounds.integral
            let representation = try #require(
                root.bitmapImageRepForCachingDisplay(in: rect)
            )
            root.cacheDisplay(in: rect, to: representation)
            guard representation.bitmapData != nil else {
                return false
            }
            lastPaintedValue = progress.doubleValue
            return true
        }

        #expect(progress.doubleValue == 0)
        #expect(root.bounds.width > 0)
        #expect(root.bounds.height > 0)

        let first = SemanticModel.Operation.setProperty(
            id: 5,
            property: .value,
            value: .float64(0.5)
        )
        let firstStore = try makeStore(base + [first])
        try renderer.apply(
            transaction: Transaction(
                baseRevision: store.revision,
                operations: [first]
            ),
            newStore: firstStore
        )
        #expect(progress.doubleValue == 0.5)
        #expect(try cadenceTick())
        #expect(lastPaintedValue == 0.5)

        var repaintCount = 1
        #expect(try cadenceTick() == false)
        if try cadenceTick() {
            repaintCount += 1
        }
        #expect(repaintCount == 1, "An idle cadence tick must not rasterize.")

        let second = SemanticModel.Operation.setProperty(
            id: 5,
            property: .value,
            value: .float64(0.75)
        )
        let secondStore = try makeStore(base + [first, second])
        try renderer.apply(
            transaction: Transaction(
                baseRevision: firstStore.revision,
                operations: [second]
            ),
            newStore: secondStore
        )
        #expect(progress.doubleValue == 0.75)
        #expect(try cadenceTick())
        #expect(lastPaintedValue == 0.75)
        repaintCount += 1
        #expect(repaintCount == 2)
        #expect(try cadenceTick() == false)
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
    func collectionPropertyApplyThrowsWhenRenderHandleIsMissing() throws {
        let store = try makeStore([
            .createNode(id: 1, nodeType: .surface),
        ])
        let renderer = AppKitRenderer()
        try renderer.attach(store: store)

        let collectionUpdate = SemanticModel.Operation.setProperty(
            id: 2,
            property: .columns,
            value: .list([.string("Name")])
        )
        let newStore = try makeStore([
            .createNode(id: 1, nodeType: .surface),
            .createNode(id: 2, nodeType: .table, parentID: 1),
            collectionUpdate,
        ])

        #expect(throws: LayoutRendererError.missingRenderHandle(2)) {
            try renderer.apply(
                transaction: Transaction(
                    baseRevision: store.revision,
                    operations: [collectionUpdate]
                ),
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
        try store.createModel(id: modelID, modelType: .table, itemCount: 0)
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
        try store.createModel(id: modelID, modelType: .table, itemCount: 0)
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

    @Test
    func resourceCommitRefreshesMatchingImageHandleInPlace() throws {
        let hash = try ResourceHash(rawBytes: Array(repeating: 0x42, count: 32))
        let store = try makeStore([
            .createNode(id: 1, nodeType: .surface),
            .createNode(
                id: 2,
                nodeType: .image,
                parentID: 1,
                properties: [(.resource, .resourceHash(hash))]
            ),
        ])
        let renderer = AppKitRenderer()
        try renderer.attach(store: store)

        let handle = try #require(renderer.registry.handle(for: 2))
        let imageView = try #require(handle.view as? NSImageView)
        let viewIdentityBefore = ObjectIdentifier(imageView)
        #expect(handle.pendingResourceHash == hash)

        // Build a tiny CGImage without going through ResourceCache.
        let colorSpace = CGColorSpaceCreateDeviceRGB()
        let bitmapInfo = CGImageAlphaInfo.premultipliedLast.rawValue
        let context = try #require(
            CGContext(
                data: nil,
                width: 5,
                height: 7,
                bitsPerComponent: 8,
                bytesPerRow: 0,
                space: colorSpace,
                bitmapInfo: bitmapInfo
            )
        )
        context.setFillColor(red: 0, green: 1, blue: 0, alpha: 1)
        context.fill(CGRect(x: 0, y: 0, width: 5, height: 7))
        let cgImage = try #require(context.makeImage())

        renderer.commitResourceImage(hash: hash, cgImage: cgImage, pixelWidth: 5, pixelHeight: 7)

        let imageViewAfter = try #require(renderer.registry.view(for: 2) as? NSImageView)
        #expect(ObjectIdentifier(imageViewAfter) == viewIdentityBefore)
        #expect(imageViewAfter.image?.size.width == 5)
        #expect(imageViewAfter.image?.size.height == 7)
        #expect(renderer.resolveResourceImage(hash) != nil)
    }


    @Test
    func preCachedResourceMountsImmediately() throws {
        let hash = try ResourceHash(rawBytes: Array(repeating: 0x77, count: 32))
        let renderer = AppKitRenderer()
        let colorSpace = CGColorSpaceCreateDeviceRGB()
        let context = try #require(
            CGContext(
                data: nil,
                width: 4,
                height: 5,
                bitsPerComponent: 8,
                bytesPerRow: 0,
                space: colorSpace,
                bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
            )
        )
        context.setFillColor(red: 1, green: 0, blue: 0, alpha: 1)
        context.fill(CGRect(x: 0, y: 0, width: 4, height: 5))
        let cgImage = try #require(context.makeImage())
        renderer.commitResourceImage(hash: hash, cgImage: cgImage, pixelWidth: 4, pixelHeight: 5)

        let store = try makeStore([
            .createNode(id: 1, nodeType: .surface),
            .createNode(
                id: 2,
                nodeType: .image,
                parentID: 1,
                properties: [(.resource, .resourceHash(hash))]
            ),
        ])
        try renderer.attach(store: store)

        let imageView = try #require(renderer.registry.view(for: 2) as? NSImageView)
        #expect(imageView.image?.size.width == 4)
        #expect(imageView.image?.size.height == 5)
    }

    @Test
    func unresolvedResourceNeverBecomesVisible() throws {
        let hash = try ResourceHash(rawBytes: Array(repeating: 0x88, count: 32))
        let store = try makeStore([
            .createNode(id: 1, nodeType: .surface),
            .createNode(
                id: 2,
                nodeType: .image,
                parentID: 1,
                properties: [(.resource, .resourceHash(hash))]
            ),
        ])
        let renderer = AppKitRenderer()
        try renderer.attach(store: store)
        let imageView = try #require(renderer.registry.view(for: 2) as? NSImageView)
        let placeholder = imageView.image
        #expect(renderer.resolveResourceImage(hash) == nil)
        #expect(imageView.image === placeholder)
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
