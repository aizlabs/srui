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

    static var data: Data { Data(bytes) }

    static func hash() throws -> ResourceHash {
        let digest = SHA256.hash(data: data)
        return try ResourceHash(rawBytes: Array(digest))
    }
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
}
