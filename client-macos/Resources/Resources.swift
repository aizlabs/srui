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
//   and in-flight byte budgets; ImageIO decoding runs off the main actor.
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

    public init(
        maxEncodedBytes: Int = 50 * 1024 * 1024,
        maxAxisPixels: Int = 16_384,
        maxTotalPixels: Int = 100_000_000,
        maxChunkBytes: Int = 32 * 1024,
        maxConcurrentAssemblies: Int = 16,
        maxInFlightBytes: Int = 64 * 1024 * 1024
    ) {
        self.maxEncodedBytes = maxEncodedBytes
        self.maxAxisPixels = maxAxisPixels
        self.maxTotalPixels = maxTotalPixels
        self.maxChunkBytes = maxChunkBytes
        self.maxConcurrentAssemblies = maxConcurrentAssemblies
        self.maxInFlightBytes = maxInFlightBytes
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
    case lengthMismatch(expected: UInt64, actual: UInt64)
    case hashMismatch(expected: ResourceHash, actual: ResourceHash)
    case invalidImage(String)
    case dimensionLimit(width: Int, height: Int, reason: String)
    case assemblyLimit(concurrent: Int, limit: Int)
    case inFlightBytesLimit(requested: Int, limit: Int)
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

    public init(image: ValidatedDecodedImage, newlyCommitted: Bool) {
        self.image = image
        self.newlyCommitted = newlyCommitted
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
    private var partials: [ResourceHash: PartialAssembly] = [:]
    private var inFlightBytes: Int = 0

    public init(limits: ResourceLimits = ResourceLimits()) {
        self.limits = limits
    }

    /// Returns whether `hash` is present in the committed CAS (not merely assembling).
    public func contains(_ hash: ResourceHash) -> Bool {
        committed[hash] != nil
    }

    /// Looks up a committed decoded image. Partials are never visible.
    public func lookup(_ hash: ResourceHash) -> ValidatedDecodedImage? {
        committed[hash]
    }

    /// Discards all in-flight assemblies while retaining the committed CAS (§14, §18).
    public func clearPartials() {
        partials.removeAll(keepingCapacity: false)
        inFlightBytes = 0
    }

    /// Announces metadata for a forthcoming (or empty) resource transfer (§14).
    @discardableResult
    public func ingestMetadata(_ input: ResourceMetadataInput) throws -> ResourceCommit? {
        if let existing = committed[input.resourceHash] {
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

    /// Appends one contiguous chunk; may complete and commit the resource (§14, §19.2).
    @discardableResult
    public func ingestChunk(_ input: ResourceChunkInput) throws -> ResourceCommit? {
        if let existing = committed[input.resourceHash] {
            // Idempotent: late/duplicate chunks for a committed hash are ignored.
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

        let digest = SHA256.hash(data: bytes)
        let actualHash: ResourceHash
        do {
            actualHash = try ResourceHash(rawBytes: Array(digest))
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
        committed[hash] = validated
        return ResourceCommit(image: validated, newlyCommitted: true)
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
