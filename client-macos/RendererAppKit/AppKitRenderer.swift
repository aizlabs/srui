import AppKit
import SemanticModel
import Resources

/// Public main-actor entry point for mounting committed semantic state and applying render deltas.
@MainActor
public final class AppKitRenderer {
    public let registry: RenderRegistry
    public let controlFactory: ControlFactory
    public let layoutRenderer: LayoutRenderer

    /// Decoded resource images retained by content hash for in-place NSImageView updates (§14).
    private var imagesByHash: [ResourceHash: NSImage] = [:]

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
        // Synchronous main-actor resolver so ControlFactory can paint a cached hash immediately (§14).
        self.controlFactory.resolveResourceImage = { [weak self] hash in
            self?.imagesByHash[hash]
        }
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

    /// Looks up a previously committed resource image on the main actor (§14).
    public func resolveResourceImage(_ hash: ResourceHash) -> NSImage? {
        imagesByHash[hash]
    }

    /// Commits a validated decoded image into the renderer's hash → NSImage table and refreshes
    /// every Image handle whose `pendingResourceHash` matches, preserving the existing
    /// `NSImageView` identity (§14, §22.3).
    public func commitResourceImage(_ image: ValidatedDecodedImage) {
        let size = NSSize(width: image.pixelWidth, height: image.pixelHeight)
        let nsImage = NSImage(cgImage: image.cgImage, size: size)
        imagesByHash[image.hash] = nsImage
        refreshImageHandles(matching: image.hash, with: nsImage)
    }

    /// Drops retained images for hashes evicted from the client resource CAS and restores
    /// placeholders on any Image handles still pointing at those hashes (§14, §26).
    public func evictResourceImages(_ hashes: [ResourceHash]) {
        guard !hashes.isEmpty else { return }
        let evicted = Set(hashes)
        for hash in hashes {
            imagesByHash.removeValue(forKey: hash)
        }
        for handle in registry.allHandles {
            guard let pending = handle.pendingResourceHash, evicted.contains(pending) else {
                continue
            }
            guard handle.nodeType == .image,
                  let imageView = handle.view as? NSImageView else {
                continue
            }
            imageView.image = NSImage(systemSymbolName: "photo", accessibilityDescription: nil)
        }
    }

    /// Convenience overload accepting a raw `CGImage` already verified by the resource cache (§14).
    public func commitResourceImage(hash: ResourceHash, cgImage: CGImage, pixelWidth: Int, pixelHeight: Int) {
        let size = NSSize(width: pixelWidth, height: pixelHeight)
        let nsImage = NSImage(cgImage: cgImage, size: size)
        imagesByHash[hash] = nsImage
        refreshImageHandles(matching: hash, with: nsImage)
    }

    private func refreshImageHandles(matching hash: ResourceHash, with nsImage: NSImage) {
        for handle in registry.allHandles where handle.pendingResourceHash == hash {
            guard handle.nodeType == .image,
                  let imageView = handle.view as? NSImageView else {
                continue
            }
            imageView.image = nsImage
        }
    }
}
