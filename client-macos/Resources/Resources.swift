//
// Resources.swift
// Resources
//
// Client-side content-addressed resource cache for chunked image delivery (§14, §19.2, §26).
//
// Spec sections implemented:
// - §14 Resource model: SHA-256 content addressing, metadata + contiguous chunks, verify-before-
//   commit; never expose unverified bytes to the renderer.
// - §19.2 Priority classes: chunk payloads are bounded (≤32 KiB) so resource traffic can interleave
//   with UI/control frames.
// - §26 Attack-surface controls: encoded size, decoded axis/total pixels, concurrent assemblies,
//   in-flight byte budgets, and bounded committed CAS (entry + decoded-byte ceilings); ImageIO
//   decoding runs off the main actor.
//
// Platform note: ImageIO + CoreGraphics only. AppKit conversion belongs in RendererAppKit.
// SemanticModel must never import AppKit — this target imports neither AppKit nor Cocoa.
//

import Foundation
import CryptoKit
import CoreGraphics
import ImageIO
import SemanticModel

// MARK: - Limits (§14, §26)

/// Configurable ceilings for resource assembly and decoded-image acceptance (§14, §26).
public struct ResourceLimits: Sendable, Equatable {
    /// Maximum encoded byte length of a single resource. Default: 50 MiB.
    public var maxEncodedBytes: Int
    /// Maximum width or height in pixels after decode. Default: 16_384.
    public var maxAxisPixels: Int
    /// Maximum width × height (overflow-safe). Default: 100_000_000.
    public var maxTotalPixels: Int
    /// Maximum payload size of one chunk. Default: 32 KiB.
    public var maxChunkBytes: Int
    /// Maximum number of distinct hashes assembling at once.
    public var maxConcurrentAssemblies: Int
    /// Maximum aggregate encoded bytes across all in-flight assemblies.
    public var maxInFlightBytes: Int
    /// Maximum number of committed decoded images retained in the CAS.
    public var maxCommittedEntries: Int
    /// Maximum aggregate decoded backing-store bytes (`bytesPerRow × height`) across committed images.
    public var maxCommittedDecodedBytes: Int
    /// Maximum UTF-8 length of a resource's `media_type`. The string is retained per partial
    /// assembly and per committed image, outside both byte budgets. Default: 255.
    public var maxMediaTypeBytes: Int

    public init(
        maxEncodedBytes: Int = 50 * 1024 * 1024,
        maxAxisPixels: Int = 16_384,
        maxTotalPixels: Int = 100_000_000,
        maxChunkBytes: Int = 32 * 1024,
        maxConcurrentAssemblies: Int = 16,
        maxInFlightBytes: Int = 64 * 1024 * 1024,
        maxCommittedEntries: Int = 64,
        maxCommittedDecodedBytes: Int = 256 * 1024 * 1024,
        maxMediaTypeBytes: Int = 255
    ) {
        self.maxEncodedBytes = maxEncodedBytes
        self.maxAxisPixels = maxAxisPixels
        self.maxTotalPixels = maxTotalPixels
        self.maxChunkBytes = maxChunkBytes
        self.maxConcurrentAssemblies = maxConcurrentAssemblies
        self.maxInFlightBytes = maxInFlightBytes
        self.maxCommittedEntries = maxCommittedEntries
        self.maxCommittedDecodedBytes = maxCommittedDecodedBytes
        self.maxMediaTypeBytes = maxMediaTypeBytes
    }

    /// Ceiling that `ResourceCache.retainedBytes()` must respect, composed from the individual
    /// limits rather than written as a literal (§26).
    ///
    /// One media type is retained per committed entry and per in-flight assembly, which is the
    /// term that was missing while `media_type` had no cap: every other limit could read as
    /// satisfied while retention grew without bound.
    public var maxRetainedBytes: Int {
        let entriesHoldingMediaTypes = maxCommittedEntries + maxConcurrentAssemblies
        return maxCommittedDecodedBytes
            + maxInFlightBytes
            + entriesHoldingMediaTypes * maxMediaTypeBytes
    }
}

// MARK: - Domain input (NOT protobuf)

/// Transfer priority for a resource, independent of the wire enum (§14, §19.2).
public enum ResourceTransferPriority: Sendable, Equatable, Hashable {
    case unspecified
    case normal
    case low
}

/// Domain metadata announcement used by the cache (mapped from wire by Session) (§14).
public struct ResourceMetadataInput: Sendable, Equatable {
    public var resourceHash: ResourceHash
    public var mediaType: String
    public var encodedLength: UInt64
    public var decodedWidth: UInt32
    public var decodedHeight: UInt32
    public var priority: ResourceTransferPriority

    public init(
        resourceHash: ResourceHash,
        mediaType: String,
        encodedLength: UInt64,
        decodedWidth: UInt32 = 0,
        decodedHeight: UInt32 = 0,
        priority: ResourceTransferPriority = .normal
    ) {
        self.resourceHash = resourceHash
        self.mediaType = mediaType
        self.encodedLength = encodedLength
        self.decodedWidth = decodedWidth
        self.decodedHeight = decodedHeight
        self.priority = priority
    }
}

/// Domain chunk payload used by the cache (mapped from wire by Session) (§14, §19.2).
public struct ResourceChunkInput: Sendable, Equatable {
    public var resourceHash: ResourceHash
    public var byteOffset: UInt64
    public var data: Data

    public init(resourceHash: ResourceHash, byteOffset: UInt64, data: Data) {
        self.resourceHash = resourceHash
        self.byteOffset = byteOffset
        self.data = data
    }
}

// MARK: - Errors

/// Typed rejection reasons for resource assembly; never leave partial bytes visible (§14, §26).
public enum ResourceCacheError: Error, Equatable, Sendable, CustomStringConvertible {
    case invalidHash(Int)
    case unknownResource(ResourceHash)
    case nonContiguousOffset(expected: UInt64, actual: UInt64)
    case oversizedChunk(length: Int, limit: Int)
    case oversizedEncoded(length: UInt64, limit: Int)
    case oversizedMediaType(length: Int, limit: Int)
    case lengthMismatch(expected: UInt64, actual: UInt64)
    case hashMismatch(expected: ResourceHash, actual: ResourceHash)
    case invalidImage(String)
    case dimensionLimit(width: Int, height: Int, reason: String)
    case assemblyLimit(concurrent: Int, limit: Int)
    case inFlightBytesLimit(requested: Int, limit: Int)
    case committedBytesLimit(requested: Int, limit: Int)
    case committedEntryLimit(count: Int, limit: Int)
    case metadataConflict(ResourceHash)

    public var description: String {
        switch self {
        case .invalidHash(let length):
            return "resource hash must be exactly 32 bytes, got \(length)"
        case .unknownResource(let hash):
            return "chunk for unknown resource \(hash)"
        case .nonContiguousOffset(let expected, let actual):
            return "non-contiguous chunk offset: expected \(expected), got \(actual)"
        case .oversizedChunk(let length, let limit):
            return "chunk payload \(length) bytes exceeds limit \(limit)"
        case .oversizedEncoded(let length, let limit):
            return "encoded length \(length) exceeds limit \(limit)"
        case .oversizedMediaType(let length, let limit):
            return "media type of \(length) bytes exceeds limit \(limit)"
        case .lengthMismatch(let expected, let actual):
            return "assembled length \(actual) does not match encoded_length \(expected)"
        case .hashMismatch(let expected, let actual):
            return "SHA-256 mismatch: expected \(expected), got \(actual)"
        case .invalidImage(let reason):
            return "invalid image: \(reason)"
        case .dimensionLimit(let width, let height, let reason):
            return "decoded dimensions \(width)×\(height) rejected: \(reason)"
        case .assemblyLimit(let concurrent, let limit):
            return "concurrent assemblies \(concurrent) exceed limit \(limit)"
        case .inFlightBytesLimit(let requested, let limit):
            return "in-flight bytes would become \(requested), limit \(limit)"
        case .committedBytesLimit(let requested, let limit):
            return "committed decoded bytes would become \(requested), limit \(limit)"
        case .committedEntryLimit(let count, let limit):
            return "committed entry count would become \(count), limit \(limit)"
        case .metadataConflict(let hash):
            return "conflicting metadata for already-assembling resource \(hash)"
        }
    }
}

// MARK: - Committed cache value

/// Immutable validated/decoded raster retained in the committed CAS (§14).
///
/// `CGImage` is treated as immutable after construction; the wrapper is `@unchecked Sendable`
/// so the actor can hand it to MainActor rendering without copying pixels again.
public final class ValidatedDecodedImage: @unchecked Sendable {
    public let hash: ResourceHash
    public let mediaType: String
    public let encodedLength: UInt64
    public let cgImage: CGImage
    public let pixelWidth: Int
    public let pixelHeight: Int

    public init(
        hash: ResourceHash,
        mediaType: String,
        encodedLength: UInt64,
        cgImage: CGImage
    ) {
        self.hash = hash
        self.mediaType = mediaType
        self.encodedLength = encodedLength
        self.cgImage = cgImage
        self.pixelWidth = cgImage.width
        self.pixelHeight = cgImage.height
    }
}

/// Outcome of a successful ingest that newly committed (or reconfirmed) a decoded image (§14).
public struct ResourceCommit: Sendable {
    public let image: ValidatedDecodedImage
    /// `true` when this ingest finalized a new assembly; `false` when the hash was already committed.
    public let newlyCommitted: Bool
    /// Hashes evicted from the committed CAS to stay within retained bounds (§26).
    public let evictedHashes: [ResourceHash]

    public init(
        image: ValidatedDecodedImage,
        newlyCommitted: Bool,
        evictedHashes: [ResourceHash] = []
    ) {
        self.image = image
        self.newlyCommitted = newlyCommitted
        self.evictedHashes = evictedHashes
    }
}

// MARK: - Partial assembly

private struct PartialAssembly {
    var metadata: ResourceMetadataInput
    var buffer: Data
    var nextOffset: UInt64

    var encodedLength: UInt64 { metadata.encodedLength }

    init(metadata: ResourceMetadataInput) {
        self.metadata = metadata
        self.buffer = Data()
        self.buffer.reserveCapacity(Int(clamping: metadata.encodedLength))
        self.nextOffset = 0
    }
}

package typealias ResourceReferenceMutation = @MainActor @Sendable () -> Void

/// Linearizes native cache mutation with connection-owner replacement on MainActor.
///
/// AppKit rendering is synchronous and MainActor-isolated, so owner activation and the complete
/// native mutation form one actor order without holding a blocking lock across a remount.
@MainActor
private final class ResourceReferenceOwnerFence: Sendable {
    private var activeOwnerEpoch: UInt64?

    nonisolated init() {}

    func activate(epoch: UInt64) -> Bool {
        if let activeOwnerEpoch, epoch < activeOwnerEpoch { return false }
        activeOwnerEpoch = epoch
        return true
    }

    func performIfActive<Value: Sendable>(
        epoch: UInt64,
        _ body: @MainActor @Sendable () -> Value
    ) -> Value? {
        guard activeOwnerEpoch == epoch else { return nil }
        return body()
    }
}

private struct RenderReferenceLease {
    var ownerEpoch: UInt64
    var renderToken: UUID
    var hashes: Set<ResourceHash>
}

// MARK: - ResourceCache actor

/// Content-addressed resource cache with separate committed CAS and partial assemblies (§14, §26).
///
/// Invariants:
/// - Partial bytes are never returned by `lookup` / never handed to the renderer.
/// - Completion requires contiguous coverage of `[0, encoded_length)` and a byte-for-byte SHA-256 match.
/// - Any validation failure discards the partial and returns a typed rejection.
public actor ResourceCache {
    public nonisolated let limits: ResourceLimits

    private var committed: [ResourceHash: ValidatedDecodedImage] = [:]
    /// Insertion order for LRU eviction of committed entries (§26).
    private var committedOrder: [ResourceHash] = []
    private var committedDecodedBytes: Int = 0
    private var partials: [ResourceHash: PartialAssembly] = [:]
    private var inFlightBytes: Int = 0
    /// Hashes currently shown by live Image nodes; never evicted while live (§14, §26).
    private var liveReferences: Set<ResourceHash> = []
    /// Process-wide monotonic connection owner for every managed cache mutation.
    private var activeReferenceOwnerEpoch: UInt64?
    /// MainActor counterpart of `activeReferenceOwnerEpoch` for native commit dispatch.
    private nonisolated let referenceOwnerFence = ResourceReferenceOwnerFence()
    /// At most one renderer ownership token is active per bound EventOutbox connection.
    private var renderReferenceLease: RenderReferenceLease?

    public init(limits: ResourceLimits = ResourceLimits()) {
        self.limits = limits
    }

    /// Returns whether `hash` is present in the committed CAS (not merely assembling).
    public func contains(_ hash: ResourceHash) -> Bool {
        committed[hash] != nil
    }

    /// Looks up a committed decoded image. Partials are never visible.
    /// Touches LRU recency so frequently resolved hashes survive eviction (§26).
    public func lookup(_ hash: ResourceHash) -> ValidatedDecodedImage? {
        guard committed[hash] != nil else { return nil }
        touchCommitted(hash)
        return committed[hash]
    }

    /// Number of committed CAS entries (tests / diagnostics).
    public func committedCount() -> Int {
        committed.count
    }

    /// Total variable-size bytes this cache is holding onto (§26).
    ///
    /// Counts decoded backing stores, in-flight assembly buffers, and every retained metadata
    /// string. Fixed-size components (hashes, lengths, enums) are excluded: they are already
    /// bounded by the entry caps, whereas the quantities here are server-controlled and are what a
    /// retention invariant must be asserted against. A budget nothing computes cannot be tested,
    /// which is exactly how an unbounded `media_type` sat behind satisfied byte limits.
    public func retainedBytes() -> Int {
        var total = committedDecodedBytes
        for image in committed.values {
            total += image.mediaType.utf8.count
        }
        for assembly in partials.values {
            total += assembly.buffer.count
            total += assembly.metadata.mediaType.utf8.count
        }
        return total
    }

    /// Verified committed hashes suitable for reconnect negotiation (§14, §18).
    public func knownHashes() -> [ResourceHash] {
        committedOrder
    }

    /// Pins `hashes` and returns matching verified images in cache-recency order.
    ///
    /// This single actor operation prevents a reconnecting renderer from looking up entries
    /// before they are protected from eviction (§14, §18, §26).
    public func setLiveReferencesAndLookup(
        _ hashes: Set<ResourceHash>
    ) -> [ValidatedDecodedImage] {
        guard activeReferenceOwnerEpoch == nil else { return [] }
        liveReferences = hashes
        let hits = committedOrder.filter { hashes.contains($0) }
        for hash in hits {
            touchCommitted(hash)
        }
        return hits.compactMap { committed[$0] }
    }

    /// Returns matching verified images without changing pins or LRU order.
    public func peekCommitted(_ hashes: Set<ResourceHash>) -> [ValidatedDecodedImage] {
        committedOrder.compactMap { hash in
            guard hashes.contains(hash) else { return nil }
            return committed[hash]
        }
    }

    /// Activates a globally newer renderer connection, retires its predecessor's partial
    /// transfers, and replaces temporary pins with the exact renderer/store references observed
    /// after the predecessor's render ownership was invalidated.
    @discardableResult
    package func activateReferenceOwner(
        epoch: UInt64,
        liveReferences: Set<ResourceHash>
    ) async -> Bool {
        if let activeReferenceOwnerEpoch {
            guard epoch >= activeReferenceOwnerEpoch else { return false }
            if epoch == activeReferenceOwnerEpoch {
                guard await referenceOwnerFence.activate(epoch: epoch) else { return false }
                return self.activeReferenceOwnerEpoch == epoch
            }
        }

        // Publish actor ownership before the MainActor hop. A newer activation may supersede this
        // one while it waits, so the post-hop equality check determines which caller may adopt.
        activeReferenceOwnerEpoch = epoch
        self.liveReferences = liveReferences
        renderReferenceLease = nil
        discardAllPartials()

        guard await referenceOwnerFence.activate(epoch: epoch) else { return false }
        return activeReferenceOwnerEpoch == epoch
    }

    /// Reports whether a managed connection still owns every scoped cache mutation.
    package func isReferenceOwnerActive(ownerEpoch: UInt64) -> Bool {
        activeReferenceOwnerEpoch == ownerEpoch
    }

    /// Runs one native resource commit only while the exact cache owner remains active.
    package func performIfReferenceOwnerActive(
        ownerEpoch: UInt64,
        _ body: ResourceReferenceMutation
    ) async -> Bool {
        guard activeReferenceOwnerEpoch == ownerEpoch else { return false }
        return await referenceOwnerFence.performIfActive(
            epoch: ownerEpoch,
            body
        ) != nil
    }

    /// Runs a synchronous AppKit render only while both its cache owner and exact preload lease
    /// remain active. The owner fence and EventOutbox's render-token fence are nested on MainActor,
    /// so neither connection replacement nor same-connection render supersession can interleave
    /// between authorization and native mutation.
    package func performIfRenderReferenceOwnerActive<Value: Sendable>(
        ownerEpoch: UInt64,
        renderToken: UUID,
        _ body: @MainActor @Sendable () -> Value
    ) async -> Value? {
        guard activeReferenceOwnerEpoch == ownerEpoch,
              renderReferenceLease?.ownerEpoch == ownerEpoch,
              renderReferenceLease?.renderToken == renderToken else {
            return nil
        }
        return await referenceOwnerFence.performIfActive(
            epoch: ownerEpoch,
            body
        )
    }

    /// Atomically pins a renderer snapshot and preloads its committed images.
    ///
    /// The exact connection epoch and render token prevent a stale same- or cross-connection
    /// update from changing global pins. EventOutbox guarantees one active renderer token per
    /// connection; a second distinct token fails closed.
    package func beginRenderReferenceLease(
        ownerEpoch: UInt64,
        renderToken: UUID,
        hashes: Set<ResourceHash>
    ) -> [ValidatedDecodedImage]? {
        guard ownerEpoch == activeReferenceOwnerEpoch else { return nil }
        if let lease = renderReferenceLease,
           lease.ownerEpoch != ownerEpoch || lease.renderToken != renderToken {
            return nil
        }
        renderReferenceLease = RenderReferenceLease(
            ownerEpoch: ownerEpoch,
            renderToken: renderToken,
            hashes: hashes
        )
        return peekCommitted(hashes)
    }

    /// Promotes a rendered token's exact current references and releases its temporary pins in one
    /// actor operation, leaving no eviction window between the two states.
    @discardableResult
    package func finishRenderReferenceLease(
        ownerEpoch: UInt64,
        renderToken: UUID,
        liveReferences: Set<ResourceHash>
    ) -> Bool {
        guard ownerEpoch == activeReferenceOwnerEpoch,
              renderReferenceLease?.ownerEpoch == ownerEpoch,
              renderReferenceLease?.renderToken == renderToken else {
            return false
        }
        self.liveReferences = liveReferences
        renderReferenceLease = nil
        return true
    }

    /// Discards unmanaged in-flight assemblies while retaining the committed CAS (§14, §18).
    /// Once connection ownership is activated, callers must use the scoped overload.
    public func clearPartials() {
        guard activeReferenceOwnerEpoch == nil else { return }
        discardAllPartials()
    }

    /// Discards in-flight assemblies only for the active managed connection.
    @discardableResult
    package func clearPartials(ownerEpoch: UInt64) -> Bool {
        guard activeReferenceOwnerEpoch == ownerEpoch else { return false }
        discardAllPartials()
        return true
    }

    private func discardAllPartials() {
        partials.removeAll(keepingCapacity: false)
        inFlightBytes = 0
    }

    /// Announces metadata before managed connection ownership has been activated.
    @discardableResult
    public func ingestMetadata(_ input: ResourceMetadataInput) throws -> ResourceCommit? {
        guard activeReferenceOwnerEpoch == nil else { return nil }
        return try ingestMetadataForActiveOwner(input)
    }

    /// Announces metadata only for the active managed connection.
    @discardableResult
    package func ingestMetadata(
        _ input: ResourceMetadataInput,
        ownerEpoch: UInt64
    ) throws -> ResourceCommit? {
        guard activeReferenceOwnerEpoch == ownerEpoch else { return nil }
        return try ingestMetadataForActiveOwner(input)
    }

    private func ingestMetadataForActiveOwner(
        _ input: ResourceMetadataInput
    ) throws -> ResourceCommit? {
        if let existing = committed[input.resourceHash] {
            touchCommitted(input.resourceHash)
            return ResourceCommit(image: existing, newlyCommitted: false)
        }

        if let existing = partials[input.resourceHash] {
            guard existing.metadata == input else {
                discardPartial(input.resourceHash)
                throw ResourceCacheError.metadataConflict(input.resourceHash)
            }
            return nil
        }

        try validateMetadataBudget(input)

        if input.encodedLength == 0 {
            return try finalizeAssembly(
                hash: input.resourceHash,
                metadata: input,
                bytes: Data()
            )
        }

        if partials.count >= limits.maxConcurrentAssemblies {
            throw ResourceCacheError.assemblyLimit(
                concurrent: partials.count + 1,
                limit: limits.maxConcurrentAssemblies
            )
        }

        let encoded = Int(input.encodedLength)
        let nextInFlight = inFlightBytes + encoded
        if nextInFlight > limits.maxInFlightBytes {
            throw ResourceCacheError.inFlightBytesLimit(
                requested: nextInFlight,
                limit: limits.maxInFlightBytes
            )
        }

        // Optional decoded-dimension hints from metadata are advisory; reject early when set and
        // clearly over limit so we never assemble bytes we already know we cannot accept (§26).
        try validateOptionalDimensionHints(input)

        partials[input.resourceHash] = PartialAssembly(metadata: input)
        inFlightBytes = nextInFlight
        return nil
    }

    /// Appends one contiguous chunk before managed connection ownership has been activated.
    @discardableResult
    public func ingestChunk(_ input: ResourceChunkInput) throws -> ResourceCommit? {
        guard activeReferenceOwnerEpoch == nil else { return nil }
        return try ingestChunkForActiveOwner(input)
    }

    /// Appends one contiguous chunk only for the active managed connection.
    @discardableResult
    package func ingestChunk(
        _ input: ResourceChunkInput,
        ownerEpoch: UInt64
    ) throws -> ResourceCommit? {
        guard activeReferenceOwnerEpoch == ownerEpoch else { return nil }
        return try ingestChunkForActiveOwner(input)
    }

    private func ingestChunkForActiveOwner(
        _ input: ResourceChunkInput
    ) throws -> ResourceCommit? {
        if let existing = committed[input.resourceHash] {
            // Idempotent: late/duplicate chunks for a committed hash are ignored.
            touchCommitted(input.resourceHash)
            return ResourceCommit(image: existing, newlyCommitted: false)
        }

        guard var assembly = partials[input.resourceHash] else {
            throw ResourceCacheError.unknownResource(input.resourceHash)
        }

        if input.data.count > limits.maxChunkBytes {
            discardPartial(input.resourceHash)
            throw ResourceCacheError.oversizedChunk(
                length: input.data.count,
                limit: limits.maxChunkBytes
            )
        }

        if input.byteOffset != assembly.nextOffset {
            let expected = assembly.nextOffset
            discardPartial(input.resourceHash)
            throw ResourceCacheError.nonContiguousOffset(
                expected: expected,
                actual: input.byteOffset
            )
        }

        let remaining = assembly.encodedLength - assembly.nextOffset
        if UInt64(input.data.count) > remaining {
            discardPartial(input.resourceHash)
            throw ResourceCacheError.lengthMismatch(
                expected: assembly.encodedLength,
                actual: assembly.nextOffset + UInt64(input.data.count)
            )
        }

        assembly.buffer.append(input.data)
        assembly.nextOffset += UInt64(input.data.count)
        partials[input.resourceHash] = assembly

        if assembly.nextOffset < assembly.encodedLength {
            return nil
        }

        if assembly.nextOffset != assembly.encodedLength {
            discardPartial(input.resourceHash)
            throw ResourceCacheError.lengthMismatch(
                expected: assembly.encodedLength,
                actual: assembly.nextOffset
            )
        }

        let metadata = assembly.metadata
        let bytes = assembly.buffer
        discardPartial(input.resourceHash)
        return try finalizeAssembly(hash: input.resourceHash, metadata: metadata, bytes: bytes)
    }

    // MARK: - Internals

    private func discardPartial(_ hash: ResourceHash) {
        if let removed = partials.removeValue(forKey: hash) {
            let encoded = Int(clamping: removed.encodedLength)
            inFlightBytes = max(0, inFlightBytes - encoded)
        }
    }

    private func validateMetadataBudget(_ input: ResourceMetadataInput) throws {
        if input.encodedLength > UInt64(limits.maxEncodedBytes) {
            throw ResourceCacheError.oversizedEncoded(
                length: input.encodedLength,
                limit: limits.maxEncodedBytes
            )
        }
        // `media_type` is retained by every partial assembly and every committed image, and is
        // accounted for by neither the encoded nor the decoded byte budget (§26).
        let mediaTypeBytes = input.mediaType.utf8.count
        if mediaTypeBytes > limits.maxMediaTypeBytes {
            throw ResourceCacheError.oversizedMediaType(
                length: mediaTypeBytes,
                limit: limits.maxMediaTypeBytes
            )
        }
    }

    private func validateOptionalDimensionHints(_ input: ResourceMetadataInput) throws {
        let width = Int(input.decodedWidth)
        let height = Int(input.decodedHeight)
        if width > limits.maxAxisPixels || height > limits.maxAxisPixels {
            throw ResourceCacheError.dimensionLimit(
                width: width,
                height: height,
                reason: "metadata axis hint exceeds \(limits.maxAxisPixels)"
            )
        }
        if width > 0 && height > 0 {
            try enforceDimensionLimits(width: width, height: height, stage: "metadata hint")
        }
    }

    private func finalizeAssembly(
        hash: ResourceHash,
        metadata: ResourceMetadataInput,
        bytes: Data
    ) throws -> ResourceCommit {
        if UInt64(bytes.count) != metadata.encodedLength {
            throw ResourceCacheError.lengthMismatch(
                expected: metadata.encodedLength,
                actual: UInt64(bytes.count)
            )
        }

        let digest = Array(SHA256.hash(data: bytes))
        let actualHash: ResourceHash
        do {
            actualHash = try ResourceHash(rawBytes: digest)
        } catch {
            throw ResourceCacheError.invalidHash(digest.count)
        }

        guard actualHash == hash else {
            throw ResourceCacheError.hashMismatch(expected: hash, actual: actualHash)
        }

        // ImageIO decode runs on this actor (off MainActor) (§22.2, §26).
        let cgImage = try decodeRasterImage(bytes: bytes, mediaType: metadata.mediaType)

        let validated = ValidatedDecodedImage(
            hash: hash,
            mediaType: metadata.mediaType,
            encodedLength: metadata.encodedLength,
            cgImage: cgImage
        )
        let evicted = try insertCommitted(validated)
        return ResourceCommit(image: validated, newlyCommitted: true, evictedHashes: evicted)
    }

    /// Updates the set of hashes currently referenced by live Image nodes. Eviction never drops
    /// these entries, so visible content is not permanently replaced with placeholders (§14, §26).
    public func setLiveReferences(_ hashes: Set<ResourceHash>) {
        guard activeReferenceOwnerEpoch == nil else { return }
        liveReferences = hashes
    }

    /// Binding-scoped variant used by SessionController resource ingestion.
    @discardableResult
    package func setLiveReferences(
        _ hashes: Set<ResourceHash>,
        ownerEpoch: UInt64
    ) -> Bool {
        guard ownerEpoch == activeReferenceOwnerEpoch else { return false }
        liveReferences = hashes
        return true
    }

    private func isLiveReference(_ hash: ResourceHash) -> Bool {
        liveReferences.contains(hash) || renderReferenceLease?.hashes.contains(hash) == true
    }

    /// Inserts into the committed CAS, evicting oldest *non-live* entries to honor bounds (§26).
    private func insertCommitted(_ image: ValidatedDecodedImage) throws -> [ResourceHash] {
        if committed[image.hash] != nil {
            touchCommitted(image.hash)
            return []
        }

        let decodedBytes = Self.estimatedDecodedBytes(image.cgImage)
        // A single image larger than the committed budget must be rejected, not force-inserted
        // after emptying the CAS (§26).
        if decodedBytes > limits.maxCommittedDecodedBytes {
            throw ResourceCacheError.committedBytesLimit(
                requested: decodedBytes,
                limit: limits.maxCommittedDecodedBytes
            )
        }

        // Plan every eviction before mutating the cache. If live entries make the insertion
        // impossible, a failure must not discard unrelated verified content (§14, §26).
        var evicted: [ResourceHash] = []
        var projectedCount = committed.count
        var projectedBytes = committedDecodedBytes
        for candidate in committedOrder where !isLiveReference(candidate) {
            let next = projectedBytes.addingReportingOverflow(decodedBytes)
            if projectedCount < limits.maxCommittedEntries
                && !next.overflow
                && next.partialValue <= limits.maxCommittedDecodedBytes {
                break
            }

            guard let existing = committed[candidate] else { continue }
            evicted.append(candidate)
            projectedCount = max(0, projectedCount - 1)
            projectedBytes = max(
                0,
                projectedBytes - Self.estimatedDecodedBytes(existing.cgImage)
            )
        }

        if projectedCount >= limits.maxCommittedEntries {
            throw ResourceCacheError.committedEntryLimit(
                count: projectedCount + 1,
                limit: limits.maxCommittedEntries
            )
        }
        let next = projectedBytes.addingReportingOverflow(decodedBytes)
        if next.overflow || next.partialValue > limits.maxCommittedDecodedBytes {
            throw ResourceCacheError.committedBytesLimit(
                requested: next.overflow ? Int.max : next.partialValue,
                limit: limits.maxCommittedDecodedBytes
            )
        }

        for hash in evicted {
            removeCommitted(hash)
        }
        committed[image.hash] = image
        committedOrder.append(image.hash)
        committedDecodedBytes = next.partialValue
        return evicted
    }

    private func touchCommitted(_ hash: ResourceHash) {
        if let idx = committedOrder.firstIndex(of: hash) {
            committedOrder.remove(at: idx)
            committedOrder.append(hash)
        }
    }

    private func removeCommitted(_ hash: ResourceHash) {
        if let index = committedOrder.firstIndex(of: hash) {
            committedOrder.remove(at: index)
        }
        if let removed = committed.removeValue(forKey: hash) {
            committedDecodedBytes = max(
                0,
                committedDecodedBytes - Self.estimatedDecodedBytes(removed.cgImage)
            )
        }
    }

    private static func estimatedDecodedBytes(_ image: CGImage) -> Int {
        let bytes = image.bytesPerRow.multipliedReportingOverflow(by: image.height)
        return bytes.overflow ? Int.max : bytes.partialValue
    }

    private func decodeRasterImage(bytes: Data, mediaType: String) throws -> CGImage {
        guard !bytes.isEmpty else {
            throw ResourceCacheError.invalidImage("empty payload")
        }

        guard let source = CGImageSourceCreateWithData(bytes as CFData, nil) else {
            throw ResourceCacheError.invalidImage("CGImageSourceCreateWithData failed")
        }

        guard CGImageSourceGetCount(source) >= 1 else {
            throw ResourceCacheError.invalidImage("no frames in image source")
        }

        if let type = CGImageSourceGetType(source) as String? {
            try confirmRasterType(type, declaredMediaType: mediaType)
        }

        // Inspect dimensions before full decode (§14, §26).
        let props = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [CFString: Any]
        let hintedWidth = (props?[kCGImagePropertyPixelWidth] as? NSNumber)?.intValue
        let hintedHeight = (props?[kCGImagePropertyPixelHeight] as? NSNumber)?.intValue
        if let hintedWidth, let hintedHeight {
            try enforceDimensionLimits(width: hintedWidth, height: hintedHeight, stage: "pre-decode")
        }

        guard let image = CGImageSourceCreateImageAtIndex(source, 0, nil) else {
            throw ResourceCacheError.invalidImage("CGImageSourceCreateImageAtIndex failed")
        }

        try enforceDimensionLimits(
            width: image.width,
            height: image.height,
            stage: "post-decode"
        )
        return image
    }

    private func confirmRasterType(_ uti: String, declaredMediaType: String) throws {
        let lowered = uti.lowercased()
        // PDF and other non-raster document types must not enter the image CAS (§14).
        if lowered.contains("pdf") || lowered == "com.adobe.pdf" {
            throw ResourceCacheError.invalidImage("non-raster type \(uti)")
        }
        let allowedSubstrings = [
            "png", "jpeg", "jpg", "tiff", "gif", "bmp", "webp", "heic", "heif", "image",
        ]
        let media = declaredMediaType.lowercased()
        let utiLooksRaster = allowedSubstrings.contains { lowered.contains($0) }
        let mediaLooksRaster = media.hasPrefix("image/") || media.isEmpty
        guard utiLooksRaster || mediaLooksRaster else {
            throw ResourceCacheError.invalidImage("unsupported type \(uti) media=\(declaredMediaType)")
        }
    }

    private func enforceDimensionLimits(width: Int, height: Int, stage: String) throws {
        guard width > 0, height > 0 else {
            throw ResourceCacheError.dimensionLimit(
                width: width,
                height: height,
                reason: "non-positive dimensions at \(stage)"
            )
        }
        if width > limits.maxAxisPixels || height > limits.maxAxisPixels {
            throw ResourceCacheError.dimensionLimit(
                width: width,
                height: height,
                reason: "axis exceeds \(limits.maxAxisPixels) at \(stage)"
            )
        }
        let (product, overflow) = width.multipliedReportingOverflow(by: height)
        if overflow || product > limits.maxTotalPixels {
            throw ResourceCacheError.dimensionLimit(
                width: width,
                height: height,
                reason: "total pixels exceed \(limits.maxTotalPixels) at \(stage)"
            )
        }
    }
}
