//
// ResourceCacheTests.swift
// ResourcesTests
//
// Assembly, hashing, corruption, and decoded-image limit coverage for ResourceCache (§14, §26).
//

import Foundation
import CryptoKit
import CoreGraphics
import ImageIO
import Testing
import SemanticModel
@testable import Resources

private enum FixturePNG {
    /// Deterministic valid 1×1 RGB PNG (69 bytes).
    static let bytes: [UInt8] = [
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
        0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x60,
        0x60, 0x60, 0x00, 0x00, 0x00, 0x04, 0x00, 0x01, 0xF6, 0x17, 0x38, 0x55, 0x00, 0x00, 0x00,
        0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ]

    /// Distinct valid 1×1 green RGB PNG (different IDAT → different SHA-256).
    static let greenBytes: [UInt8] = [
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
        0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00,
        0x00, 0x90, 0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x78,
        0xDA, 0x63, 0x60, 0xF8, 0xCF, 0x00, 0x00, 0x02, 0x02, 0x01, 0x00, 0x45, 0xF4, 0x52,
        0xD4, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ]

    /// Distinct valid 1×1 blue RGB PNG.
    static let blueBytes: [UInt8] = [
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
        0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00,
        0x00, 0x90, 0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x78,
        0xDA, 0x63, 0x60, 0x60, 0xF8, 0x0F, 0x00, 0x01, 0x03, 0x01, 0x00, 0x36, 0x74, 0x11,
        0x40, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ]

    static var data: Data { Data(bytes) }

    static func hash() throws -> ResourceHash {
        try hash(of: bytes)
    }

    static func hash(of bytes: [UInt8]) throws -> ResourceHash {
        let digest = SHA256.hash(data: Data(bytes))
        return try ResourceHash(rawBytes: Array(digest))
    }
}

private func ingestPNG(_ cache: ResourceCache, bytes: [UInt8]) async throws -> ResourceCommit {
    let hash = try FixturePNG.hash(of: bytes)
    _ = try await cache.ingestMetadata(
        ResourceMetadataInput(
            resourceHash: hash,
            mediaType: "image/png",
            encodedLength: UInt64(bytes.count),
            decodedWidth: 1,
            decodedHeight: 1,
            priority: .normal
        )
    )
    return try #require(
        try await cache.ingestChunk(
            ResourceChunkInput(resourceHash: hash, byteOffset: 0, data: Data(bytes))
        )
    )
}

private func makeFixturePNG(width: Int, marker: UInt8) throws -> [UInt8] {
    let bytesPerRow = width * 4
    let pixels = Data(repeating: marker, count: bytesPerRow)
    let provider = try #require(CGDataProvider(data: pixels as CFData))
    let image = try #require(
        CGImage(
            width: width,
            height: 1,
            bitsPerComponent: 8,
            bitsPerPixel: 32,
            bytesPerRow: bytesPerRow,
            space: CGColorSpaceCreateDeviceRGB(),
            bitmapInfo: CGBitmapInfo(rawValue: CGImageAlphaInfo.premultipliedLast.rawValue),
            provider: provider,
            decode: nil,
            shouldInterpolate: false,
            intent: .defaultIntent
        )
    )
    let output = NSMutableData()
    let destination = try #require(
        CGImageDestinationCreateWithData(output, "public.png" as CFString, 1, nil)
    )
    CGImageDestinationAddImage(destination, image, nil)
    #expect(CGImageDestinationFinalize(destination))
    return [UInt8](output as Data)
}

@Suite("ResourceCache")
struct ResourceCacheTests {
    @Test
    func validFragmentedInputCommitsDecodedImage() async throws {
        let cache = ResourceCache()
        let hash = try FixturePNG.hash()
        let meta = ResourceMetadataInput(
            resourceHash: hash,
            mediaType: "image/png",
            encodedLength: UInt64(FixturePNG.bytes.count),
            decodedWidth: 1,
            decodedHeight: 1,
            priority: .normal
        )
        #expect(try await cache.ingestMetadata(meta) == nil)

        let mid = FixturePNG.bytes.count / 2
        let first = ResourceChunkInput(
            resourceHash: hash,
            byteOffset: 0,
            data: Data(FixturePNG.bytes[..<mid])
        )
        #expect(try await cache.ingestChunk(first) == nil)

        let second = ResourceChunkInput(
            resourceHash: hash,
            byteOffset: UInt64(mid),
            data: Data(FixturePNG.bytes[mid...])
        )
        let commit = try #require(try await cache.ingestChunk(second))
        #expect(commit.newlyCommitted)
        #expect(commit.image.pixelWidth == 1)
        #expect(commit.image.pixelHeight == 1)
        #expect(await cache.contains(hash))
        #expect(await cache.lookup(hash)?.hash == hash)
    }

    @Test
    func outOfOrderChunkIsRejectedAndPartialDiscarded() async throws {
        let cache = ResourceCache()
        let hash = try FixturePNG.hash()
        let meta = ResourceMetadataInput(
            resourceHash: hash,
            mediaType: "image/png",
            encodedLength: UInt64(FixturePNG.bytes.count),
            decodedWidth: 0,
            decodedHeight: 0,
            priority: .normal
        )
        _ = try await cache.ingestMetadata(meta)

        let bad = ResourceChunkInput(
            resourceHash: hash,
            byteOffset: 10,
            data: Data(FixturePNG.bytes[10..<20])
        )
        await #expect(throws: ResourceCacheError.self) {
            _ = try await cache.ingestChunk(bad)
        }
        // Partial must be gone so a later contiguous transfer can start cleanly.
        _ = try await cache.ingestMetadata(meta)
    }

    @Test
    func oversizedChunkRejected() async throws {
        let limits = ResourceLimits(maxChunkBytes: 8)
        let cache = ResourceCache(limits: limits)
        let hash = try FixturePNG.hash()
        _ = try await cache.ingestMetadata(
            ResourceMetadataInput(
                resourceHash: hash,
                mediaType: "image/png",
                encodedLength: UInt64(FixturePNG.bytes.count),
                decodedWidth: 0,
                decodedHeight: 0,
                priority: .normal
            )
        )
        let chunk = ResourceChunkInput(
            resourceHash: hash,
            byteOffset: 0,
            data: Data(repeating: 0xAB, count: 16)
        )
        do {
            _ = try await cache.ingestChunk(chunk)
            Issue.record("expected oversizedChunk")
        } catch let ResourceCacheError.oversizedChunk(length, limit) {
            #expect(length == 16)
            #expect(limit == 8)
        }
    }

    @Test
    func declaredLengthMismatchRejected() async throws {
        let cache = ResourceCache()
        let hash = try FixturePNG.hash()
        let encoded = UInt64(FixturePNG.bytes.count)
        _ = try await cache.ingestMetadata(
            ResourceMetadataInput(
                resourceHash: hash,
                mediaType: "image/png",
                encodedLength: encoded,
                decodedWidth: 0,
                decodedHeight: 0,
                priority: .normal
            )
        )
        // Deliver one extra byte past encoded_length.
        var oversized = FixturePNG.data
        oversized.append(0xFF)
        await #expect(throws: ResourceCacheError.self) {
            _ = try await cache.ingestChunk(
                ResourceChunkInput(resourceHash: hash, byteOffset: 0, data: oversized)
            )
        }
        #expect(await cache.contains(hash) == false)
    }

    @Test
    func hashMismatchNeverCommits() async throws {
        let cache = ResourceCache()
        var bytes = FixturePNG.bytes
        let realHash = try FixturePNG.hash()
        // Advertise the real hash but deliver corrupted bytes.
        bytes[bytes.count - 5] ^= 0xFF
        _ = try await cache.ingestMetadata(
            ResourceMetadataInput(
                resourceHash: realHash,
                mediaType: "image/png",
                encodedLength: UInt64(bytes.count),
                decodedWidth: 0,
                decodedHeight: 0,
                priority: .normal
            )
        )
        do {
            _ = try await cache.ingestChunk(
                ResourceChunkInput(resourceHash: realHash, byteOffset: 0, data: Data(bytes))
            )
            Issue.record("expected hashMismatch")
        } catch ResourceCacheError.hashMismatch {
            // expected
        }
        #expect(await cache.contains(realHash) == false)
        #expect(await cache.lookup(realHash) == nil)
    }

    @Test
    func invalidImageDataRejected() async throws {
        let cache = ResourceCache()
        let payload = Data(repeating: 0x00, count: 32)
        let digest = SHA256.hash(data: payload)
        let hash = try ResourceHash(rawBytes: Array(digest))
        _ = try await cache.ingestMetadata(
            ResourceMetadataInput(
                resourceHash: hash,
                mediaType: "image/png",
                encodedLength: UInt64(payload.count),
                decodedWidth: 0,
                decodedHeight: 0,
                priority: .normal
            )
        )
        await #expect(throws: ResourceCacheError.self) {
            _ = try await cache.ingestChunk(
                ResourceChunkInput(resourceHash: hash, byteOffset: 0, data: payload)
            )
        }
        #expect(await cache.contains(hash) == false)
    }

    @Test
    func maximumAxisLimitIsConfigurable() async throws {
        let limits = ResourceLimits(maxAxisPixels: 0) // reject any positive dimension
        let cache = ResourceCache(limits: limits)
        let hash = try FixturePNG.hash()
        _ = try await cache.ingestMetadata(
            ResourceMetadataInput(
                resourceHash: hash,
                mediaType: "image/png",
                encodedLength: UInt64(FixturePNG.bytes.count),
                decodedWidth: 0,
                decodedHeight: 0,
                priority: .normal
            )
        )
        do {
            _ = try await cache.ingestChunk(
                ResourceChunkInput(resourceHash: hash, byteOffset: 0, data: FixturePNG.data)
            )
            Issue.record("expected dimensionLimit")
        } catch ResourceCacheError.dimensionLimit {
            // expected
        }
        #expect(await cache.contains(hash) == false)
    }

    @Test
    func clearPartialsRetainsCommittedEntries() async throws {
        let cache = ResourceCache()
        let hash = try FixturePNG.hash()
        _ = try await cache.ingestMetadata(
            ResourceMetadataInput(
                resourceHash: hash,
                mediaType: "image/png",
                encodedLength: UInt64(FixturePNG.bytes.count),
                decodedWidth: 1,
                decodedHeight: 1,
                priority: .normal
            )
        )
        _ = try await cache.ingestChunk(
            ResourceChunkInput(resourceHash: hash, byteOffset: 0, data: FixturePNG.data)
        )
        #expect(await cache.contains(hash))

        // Start a second assembly then clear partials.
        let otherPayload = Data("partial-only".utf8)
        let otherDigest = SHA256.hash(data: otherPayload)
        let otherHash = try ResourceHash(rawBytes: Array(otherDigest))
        _ = try await cache.ingestMetadata(
            ResourceMetadataInput(
                resourceHash: otherHash,
                mediaType: "application/octet-stream",
                encodedLength: UInt64(otherPayload.count),
                decodedWidth: 0,
                decodedHeight: 0,
                priority: .low
            )
        )
        await cache.clearPartials()
        #expect(await cache.contains(hash))
        // Re-announcing after clear must work (partial was dropped).
        _ = try await cache.ingestMetadata(
            ResourceMetadataInput(
                resourceHash: otherHash,
                mediaType: "application/octet-stream",
                encodedLength: UInt64(otherPayload.count),
                decodedWidth: 0,
                decodedHeight: 0,
                priority: .low
            )
        )
    }

    @Test
    func concurrentAssemblyLimitIsEnforced() async throws {
        let cache = ResourceCache(
            limits: ResourceLimits(maxConcurrentAssemblies: 1, maxInFlightBytes: 1_048_576)
        )
        let first = try ResourceHash(rawBytes: Array(repeating: 0x44, count: 32))
        _ = try await cache.ingestMetadata(
            ResourceMetadataInput(resourceHash: first, mediaType: "image/png", encodedLength: 100)
        )
        let second = try ResourceHash(rawBytes: Array(repeating: 0x55, count: 32))
        await #expect(throws: ResourceCacheError.self) {
            _ = try await cache.ingestMetadata(
                ResourceMetadataInput(resourceHash: second, mediaType: "image/png", encodedLength: 100)
            )
        }
    }

    @Test
    func chunkWithoutMetadataIsUnknownResource() async throws {
        let cache = ResourceCache()
        let hash = try FixturePNG.hash()
        await #expect(throws: ResourceCacheError.self) {
            _ = try await cache.ingestChunk(
                ResourceChunkInput(resourceHash: hash, byteOffset: 0, data: FixturePNG.data)
            )
        }
    }

    @Test
    func committedCacheEvictsOldestWhenEntryLimitExceeded() async throws {
        let cache = ResourceCache(
            limits: ResourceLimits(maxCommittedEntries: 1, maxCommittedDecodedBytes: 1_048_576)
        )
        let first = try await ingestPNG(cache, bytes: FixturePNG.bytes)
        #expect(first.newlyCommitted)
        #expect(first.evictedHashes.isEmpty)
        #expect(await cache.committedCount() == 1)

        let secondHash = try FixturePNG.hash(of: FixturePNG.greenBytes)
        #expect(secondHash != first.image.hash)

        let second = try await ingestPNG(cache, bytes: FixturePNG.greenBytes)
        #expect(second.newlyCommitted)
        #expect(second.evictedHashes == [first.image.hash])
        #expect(await cache.contains(secondHash))
        #expect(await cache.contains(first.image.hash) == false)
        #expect(await cache.committedCount() == 1)
    }

    @Test
    func committedCacheEvictsLeastRecentlyUsedOnLookup() async throws {
        let cache = ResourceCache(
            limits: ResourceLimits(maxCommittedEntries: 2, maxCommittedDecodedBytes: 1_048_576)
        )
        let first = try await ingestPNG(cache, bytes: FixturePNG.bytes)
        let second = try await ingestPNG(cache, bytes: FixturePNG.greenBytes)
        #expect(await cache.committedCount() == 2)

        // Touch the older entry so insertion-order eviction would be wrong.
        #expect(await cache.lookup(first.image.hash) != nil)

        let third = try await ingestPNG(cache, bytes: FixturePNG.blueBytes)
        #expect(third.newlyCommitted)
        #expect(third.evictedHashes == [second.image.hash])
        #expect(await cache.contains(first.image.hash))
        #expect(await cache.contains(second.image.hash) == false)
        #expect(await cache.contains(third.image.hash))
        #expect(await cache.committedCount() == 2)
    }

    @Test
    func failedLiveReferenceInsertionDoesNotPartiallyEvict() async throws {
        let largeBytes = try makeFixturePNG(width: 3, marker: 0x7F)
        let probe = ResourceCache()
        let smallProbe = try await ingestPNG(probe, bytes: FixturePNG.bytes)
        let largeProbe = try await ingestPNG(probe, bytes: largeBytes)
        let smallBackingBytes = smallProbe.image.cgImage.bytesPerRow
        let largeBackingBytes = largeProbe.image.cgImage.bytesPerRow
        #expect(largeBackingBytes > smallBackingBytes)

        let cache = ResourceCache(
            limits: ResourceLimits(
                maxCommittedEntries: 4,
                maxCommittedDecodedBytes: smallBackingBytes * 3
            )
        )
        let first = try await ingestPNG(cache, bytes: FixturePNG.bytes)
        let second = try await ingestPNG(cache, bytes: FixturePNG.greenBytes)
        let third = try await ingestPNG(cache, bytes: FixturePNG.blueBytes)
        await cache.setLiveReferences([second.image.hash, third.image.hash])

        await #expect(throws: ResourceCacheError.self) {
            _ = try await ingestPNG(cache, bytes: largeBytes)
        }
        #expect(await cache.contains(first.image.hash))
        #expect(await cache.contains(second.image.hash))
        #expect(await cache.contains(third.image.hash))
        #expect(await cache.committedCount() == 3)
    }

    @Test
    func committedBudgetUsesDecodedBackingStoreBytes() async throws {
        let cache = ResourceCache(
            limits: ResourceLimits(maxCommittedDecodedBytes: 3)
        )

        await #expect(throws: ResourceCacheError.self) {
            _ = try await ingestPNG(cache, bytes: FixturePNG.bytes)
        }
        #expect(await cache.committedCount() == 0)
    }

    @Test
    func liveReferencesAreNotEvicted() async throws {
        let cache = ResourceCache(
            limits: ResourceLimits(maxCommittedEntries: 1, maxCommittedDecodedBytes: 1_048_576)
        )
        let first = try await ingestPNG(cache, bytes: FixturePNG.bytes)
        #expect(await cache.knownHashes() == [first.image.hash])
        let hydrated = await cache.setLiveReferencesAndLookup([first.image.hash])
        #expect(hydrated.map(\.hash) == [first.image.hash])

        await #expect(throws: ResourceCacheError.self) {
            _ = try await ingestPNG(cache, bytes: FixturePNG.greenBytes)
        }
        #expect(await cache.contains(first.image.hash))
        #expect(await cache.committedCount() == 1)

        await cache.setLiveReferences([])
        let second = try await ingestPNG(cache, bytes: FixturePNG.greenBytes)
        #expect(second.evictedHashes == [first.image.hash])
        #expect(await cache.contains(first.image.hash) == false)
        #expect(await cache.contains(second.image.hash))
    }

    /// `media_type` is retained per assembly and per committed image but is counted by neither the
    /// encoded nor the decoded byte budget, so a hostile server could retain ≈ the frame ceiling
    /// per entry while every advertised limit still reads as satisfied (§26).
    @Test
    func oversizedMediaTypeIsRejectedBeforeRetainingMetadata() async throws {
        let cache = ResourceCache(limits: ResourceLimits(maxMediaTypeBytes: 32))
        let hash = try FixturePNG.hash()
        let metadata = ResourceMetadataInput(
            resourceHash: hash,
            mediaType: "image/" + String(repeating: "x", count: 64),
            encodedLength: UInt64(FixturePNG.bytes.count),
            decodedWidth: 1,
            decodedHeight: 1,
            priority: .normal
        )

        await #expect(throws: ResourceCacheError.oversizedMediaType(length: 70, limit: 32)) {
            _ = try await cache.ingestMetadata(metadata)
        }

        // Nothing was retained: the follow-up chunk has no assembly to append to.
        await #expect(throws: ResourceCacheError.unknownResource(hash)) {
            _ = try await cache.ingestChunk(
                ResourceChunkInput(resourceHash: hash, byteOffset: 0, data: FixturePNG.data)
            )
        }
        #expect(await cache.committedCount() == 0)
    }
}
