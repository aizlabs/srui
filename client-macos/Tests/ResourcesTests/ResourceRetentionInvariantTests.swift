//
// ResourceRetentionInvariantTests.swift
// ResourcesTests
//
// Retention invariants for the resource cache (§14, §26).
//
// These tests are a different shape from the rest of the suite. Every other test here drives one
// operation and asserts its outcome — committed, rejected, this many entries. That shape cannot
// see an unbounded *value* behind a bounded entry count: a cache capped at 64 entries stays at 64
// entries however large each entry's metadata grows, so the count assertions keep passing while
// memory does not stop growing. `media_type` sat in exactly that blind spot.
//
// What follows asserts a byte budget over an operation *sequence* instead. It needs
// `ResourceCache.retainedBytes()` to exist at all: a budget nothing computes cannot be asserted,
// which is the structural reason the gap survived review.
//

import Foundation
import CryptoKit
import CoreGraphics
import ImageIO
import Testing
import SemanticModel
@testable import Resources

/// Deterministic sequence driver: a seeded LCG keeps a "random" operation order reproducible, so
/// a failing run names a seed that replays the exact sequence.
private struct Lcg {
    private var state: UInt64

    init(seed: UInt64) {
        self.state = seed
    }

    mutating func next() -> UInt64 {
        state = state &* 6_364_136_223_846_793_005 &+ 1_442_695_040_888_963_407
        return state >> 33
    }

    mutating func next(upperBound: UInt64) -> UInt64 {
        next() % upperBound
    }
}

/// Metadata string size a hostile-but-well-formed server can send.
///
/// The wire permits anything up to the 16 MiB frame ceiling. 256 KiB is far past any real media
/// type while keeping the test fast, and — importantly — large enough that a single retained copy
/// blows the budget. Sizing it just past `maxMediaTypeBytes` would not work: a handful of 256-byte
/// strings still fits, so the test would pass against the very bug it exists to catch.
private let hostileMediaTypeBytes = 256 * 1024

private func hostileMediaType() -> String {
    "image/png; profile=" + String(repeating: "x", count: hostileMediaTypeBytes)
}

/// Builds a distinct, valid PNG per `marker`, so each ingest is a different hash.
private func makePNG(width: Int, marker: UInt8) throws -> [UInt8] {
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

private func sha256(_ bytes: [UInt8]) throws -> ResourceHash {
    try ResourceHash(rawBytes: Array(SHA256.hash(data: Data(bytes))))
}

/// Limits small enough that a few hundred operations saturate every table under test.
private func constrainedLimits() -> ResourceLimits {
    ResourceLimits(
        maxEncodedBytes: 64 * 1024,
        maxChunkBytes: 4 * 1024,
        maxConcurrentAssemblies: 2,
        maxInFlightBytes: 64 * 1024,
        maxCommittedEntries: 4,
        maxCommittedDecodedBytes: 64 * 1024,
        maxMediaTypeBytes: 64
    )
}

@Suite("ResourceCache retention invariants")
struct ResourceRetentionInvariantTests {
    /// After any sequence of ingests, rejections, pins and discards, retained bytes must still fit
    /// the budget composed from the declared limits.
    @Test(arguments: [0x9E37_79B9_7F4A_7C15, 0x0123_4567_89AB_CDEF, 0xDEAD_BEEF_CAFE_F00D] as [UInt64])
    func retainedBytesStayWithinBudgetAcrossAnOperationSequence(seed: UInt64) async throws {
        let cache = ResourceCache(limits: constrainedLimits())
        let budget = cache.limits.maxRetainedBytes
        var rng = Lcg(seed: seed)
        var offeredMetadataBytes = 0
        var committedSomething = false

        for step in 0..<160 {
            let marker = UInt8(truncatingIfNeeded: step)
            let png = try makePNG(width: 4 + (step % 12), marker: marker)
            let hash = try sha256(png)
            let hostile = rng.next(upperBound: 2) == 0
            let mediaType = hostile ? hostileMediaType() : "image/png"
            offeredMetadataBytes += mediaType.utf8.count

            let metadata = ResourceMetadataInput(
                resourceHash: hash,
                mediaType: mediaType,
                encodedLength: UInt64(png.count),
                decodedWidth: 0,
                decodedHeight: 0,
                priority: .normal
            )

            // Outcomes are deliberately not asserted: this test is about the budget holding over
            // the sequence, so it keeps failing for the right reason if a rejection is ever
            // relaxed into an acceptance. `do`/`catch` rather than `try?`, because a successful
            // announce returns nil and `try?` would flatten that into "rejected".
            var announced = true
            do {
                _ = try await cache.ingestMetadata(metadata)
            } catch {
                announced = false
            }

            if announced {
                switch rng.next(upperBound: 3) {
                case 0:
                    // Complete the transfer.
                    do {
                        let commit = try await cache.ingestChunk(
                            ResourceChunkInput(resourceHash: hash, byteOffset: 0, data: Data(png))
                        )
                        committedSomething = committedSomething || commit != nil
                    } catch {
                        // A rejected payload is a legitimate outcome; the budget still applies.
                    }
                case 1:
                    // Abandon mid-transfer, leaving a partial assembly retained.
                    _ = try? await cache.ingestChunk(
                        ResourceChunkInput(
                            resourceHash: hash,
                            byteOffset: 0,
                            data: Data(png.prefix(png.count / 2))
                        )
                    )
                default:
                    // Corrupt payload: assembles fully, then fails the hash check.
                    _ = try? await cache.ingestChunk(
                        ResourceChunkInput(
                            resourceHash: hash,
                            byteOffset: 0,
                            data: Data(repeating: 0x7F, count: png.count)
                        )
                    )
                }
            }

            switch rng.next(upperBound: 8) {
            case 0:
                await cache.clearPartials()
            case 1:
                let known = await cache.knownHashes()
                await cache.setLiveReferences(Set(known.prefix(2)))
            case 2:
                await cache.setLiveReferences([])
            default:
                break
            }

            // Asserted after *every* operation, not once at the end: a cache that overshoots
            // mid-sequence and is trimmed later would otherwise pass.
            let retained = await cache.retainedBytes()
            let overshoot = "seed \(String(seed, radix: 16)) step \(step): retained \(retained)"
                + " bytes exceeds the \(budget) byte budget"
            #expect(retained <= budget, Comment(rawValue: overshoot))
        }

        // Not vacuous: content was really retained, and the sequence offered far more metadata
        // than the budget, so the assertion above was load-bearing.
        #expect(committedSomething, "the sequence must commit at least one image")
        #expect(await cache.retainedBytes() > 0)
        let coverage = "offered \(offeredMetadataBytes) metadata bytes against a \(budget) byte"
            + " budget: the sequence is too short to put the cap under test"
        #expect(offeredMetadataBytes > budget, Comment(rawValue: coverage))
    }

    /// The same property stated as growth: more distinct resources must not mean more retained
    /// bytes once the cache is saturated.
    @Test
    func retainedBytesDoNotGrowWithResourceCount() async throws {
        let cache = ResourceCache(limits: constrainedLimits())

        func ingest(_ range: Range<Int>) async throws {
            for step in range {
                let png = try makePNG(width: 4 + (step % 12), marker: UInt8(truncatingIfNeeded: step))
                let hash = try sha256(png)
                // Alternate a normal media type with one a hostile server could send.
                let mediaType = step.isMultiple(of: 2) ? "image/png" : hostileMediaType()
                _ = try? await cache.ingestMetadata(
                    ResourceMetadataInput(
                        resourceHash: hash,
                        mediaType: mediaType,
                        encodedLength: UInt64(png.count)
                    )
                )
                _ = try? await cache.ingestChunk(
                    ResourceChunkInput(resourceHash: hash, byteOffset: 0, data: Data(png))
                )
            }
        }

        try await ingest(0..<40)
        let afterFirst = await cache.retainedBytes()
        try await ingest(40..<80)
        let afterSecond = await cache.retainedBytes()

        #expect(afterFirst > 0, "the cache must actually retain content")
        #expect(afterFirst <= cache.limits.maxRetainedBytes)
        #expect(afterSecond <= cache.limits.maxRetainedBytes)
        #expect(
            afterSecond <= afterFirst,
            "retained bytes must plateau at the caps, not track the number of resources seen"
        )
    }
}
