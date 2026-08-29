import Foundation
import XCTest
@testable import Protocol

final class FramingDecoderMatrixTests: XCTestCase {
    private var goldenFramed: Data = Data()
    private var goldenMessage: SRUIMessage = SRUIMessage()

    override func setUpWithError() throws {
        let vectorsDir = try XCTUnwrap(conformanceVectorsDirectory())
        goldenFramed = try Data(
            contentsOf: vectorsDir.appendingPathComponent("golden_framed_message.bin")
        )
        goldenMessage = try SRUIFraming.decodeFramed(
            SRUIMessage.self,
            from: goldenFramed
        )
    }

    private func conformanceVectorsDirectory() -> URL? {
        var current = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
        while current.path != "/" {
            let candidate = current.appendingPathComponent("protocol/conformance-vectors")
            if FileManager.default.fileExists(atPath: candidate.path) {
                return candidate
            }
            current.deleteLastPathComponent()
        }
        return nil
    }

    func testGoldenFrameSplitAtEveryByteBoundary() throws {
        XCTAssertGreaterThan(goldenFramed.count, 2)

        for split in 1..<goldenFramed.count {
            var decoder = SRUIMessageStreamDecoder()

            XCTAssertEqual(
                try decoder.appendAndExtract(
                    incoming: goldenFramed.prefix(split)
                ),
                [],
                "split at \(split) should be incomplete after first chunk"
            )
            XCTAssertGreaterThan(decoder.bufferedByteCount, 0)

            XCTAssertEqual(
                try decoder.appendAndExtract(
                    incoming: goldenFramed.suffix(from: split)
                ),
                [goldenMessage],
                "split at \(split) should complete on second chunk"
            )
            XCTAssertEqual(decoder.bufferedByteCount, 0)
        }
    }

    func testGoldenFrameOneByteFeeds() throws {
        var decoder = SRUIMessageStreamDecoder()
        var decoded: [SRUIMessage] = []

        for byte in goldenFramed {
            decoded.append(
                contentsOf: try decoder.appendAndExtract(incoming: Data([byte]))
            )
        }

        XCTAssertEqual(decoded, [goldenMessage])
        XCTAssertEqual(decoder.bufferedByteCount, 0)
    }

    func testCoalescedGoldenFramesDecodeInWireOrder() throws {
        var incoming = goldenFramed
        incoming.append(goldenFramed)

        var decoder = SRUIMessageStreamDecoder()
        XCTAssertEqual(
            try decoder.appendAndExtract(incoming: incoming),
            [goldenMessage, goldenMessage]
        )
        XCTAssertEqual(decoder.bufferedByteCount, 0)
    }

    func testVarintLengthPrefixBoundariesWaitForPayload() throws {
        for declaredLength in [1, 127, 128] {
            var frame = Data()
            encodeVarint(UInt64(declaredLength), into: &frame)
            frame.append(Data(repeating: 0, count: declaredLength))
            let prefixLength = varintPrefixLength(declaredLength)

            var decoder = SRUIMessageStreamDecoder()
            XCTAssertEqual(
                try decoder.appendAndExtract(
                    incoming: frame.prefix(prefixLength)
                ),
                [],
                "declared length \(declaredLength) should wait for payload"
            )

            XCTAssertThrowsError(
                try decoder.appendAndExtract(
                    incoming: frame.suffix(from: prefixLength)
                ),
                "declared length \(declaredLength) with zero-filled payload should fail protobuf decode"
            )
            XCTAssertEqual(decoder.bufferedByteCount, 0)
        }

        var emptyFrame = Data()
        encodeVarint(0, into: &emptyFrame)
        var decoder = SRUIMessageStreamDecoder()
        XCTAssertEqual(
            try decoder.appendAndExtract(incoming: emptyFrame),
            [SRUIMessage()],
            "declared length 0 is complete with only the prefix byte"
        )
    }

    func testMaxDeclaredLengthAcceptedAndMaxPlusOneRejected() {
        var atLimit = Data()
        encodeVarint(UInt64(defaultMaxFrameSize), into: &atLimit)

        var decoder = SRUIMessageStreamDecoder()
        XCTAssertEqual(
            try decoder.appendAndExtract(incoming: atLimit),
            [],
            "max declared length should wait for payload"
        )
        XCTAssertEqual(decoder.bufferedByteCount, atLimit.count)

        var overLimit = Data()
        encodeVarint(UInt64(defaultMaxFrameSize + 1), into: &overLimit)

        decoder = SRUIMessageStreamDecoder()
        XCTAssertThrowsError(
            try decoder.appendAndExtract(incoming: overLimit)
        ) { error in
            let streamError = error as? SRUIStreamDecodeError
            XCTAssertEqual(streamError?.decodedMessages, [])
            XCTAssertEqual(
                streamError?.framingError,
                .frameSizeLimitExceeded(
                    limit: defaultMaxFrameSize,
                    actual: defaultMaxFrameSize + 1
                )
            )
        }
        XCTAssertEqual(decoder.bufferedByteCount, 0)
    }

    func testIncompleteVarintPrefixIsRetained() throws {
        var decoder = SRUIMessageStreamDecoder()
        XCTAssertEqual(
            try decoder.appendAndExtract(incoming: Data([0x80])),
            [],
            "single-byte continuation prefix should wait for more data"
        )
        XCTAssertEqual(decoder.bufferedByteCount, 1)
    }

    func testOverlongVarintClearsBufferAndCanRecover() throws {
        var decoder = SRUIMessageStreamDecoder()
        XCTAssertThrowsError(
            try decoder.appendAndExtract(
                incoming: Data(repeating: 0x80, count: 11)
            )
        ) { error in
            let streamError = error as? SRUIStreamDecodeError
            XCTAssertEqual(streamError?.decodedMessages, [])
            XCTAssertEqual(streamError?.framingError, .malformedVarint)
        }
        XCTAssertEqual(decoder.bufferedByteCount, 0)

        let valid = makeSampleMessage()
        XCTAssertEqual(
            try decoder.appendAndExtract(
                incoming: try SRUIFraming.encodeFramed(valid)
            ),
            [valid]
        )
    }

    func testDeclaredLengthExceedsAvailablePayloadWaitsThenRecovers() throws {
        let truncated = Data([0x64, 0x01, 0x02, 0x03, 0x04])
        let valid = makeSampleMessage()
        let validFramed = try SRUIFraming.encodeFramed(valid)

        var decoder = SRUIMessageStreamDecoder()
        XCTAssertEqual(
            try decoder.appendAndExtract(incoming: truncated),
            [],
            "declared length greater than payload should wait"
        )
        XCTAssertEqual(decoder.bufferedByteCount, truncated.count)

        decoder = SRUIMessageStreamDecoder()
        XCTAssertEqual(
            try decoder.appendAndExtract(incoming: validFramed),
            [valid]
        )
    }

    func testDecoderRecoversAfterMaxPlusOneDeclaredLength() throws {
        var overLimit = Data()
        encodeVarint(UInt64(defaultMaxFrameSize + 1), into: &overLimit)
        let valid = makeSampleMessage()
        let validFramed = try SRUIFraming.encodeFramed(valid)

        var decoder = SRUIMessageStreamDecoder()
        XCTAssertThrowsError(
            try decoder.appendAndExtract(incoming: overLimit)
        ) { error in
            XCTAssertNotNil(error as? SRUIStreamDecodeError)
        }
        XCTAssertEqual(decoder.bufferedByteCount, 0)

        XCTAssertEqual(
            try decoder.appendAndExtract(incoming: validFramed),
            [valid]
        )
    }

    func testDecoderRecoversAfterInvalidProtobufPayload() throws {
        var invalid = Data()
        encodeVarint(1, into: &invalid)
        invalid.append(0x08)

        let valid = makeSampleMessage()
        let validFramed = try SRUIFraming.encodeFramed(valid)

        var decoder = SRUIMessageStreamDecoder()
        XCTAssertThrowsError(
            try decoder.appendAndExtract(incoming: invalid)
        ) { error in
            XCTAssertNotNil(error as? SRUIStreamDecodeError)
        }
        XCTAssertEqual(decoder.bufferedByteCount, 0)

        XCTAssertEqual(
            try decoder.appendAndExtract(incoming: validFramed),
            [valid]
        )
    }

    func testDecoderRecoversAfterIncompletePrefixWithoutClearing() throws {
        let prefixLength = varintPrefixLength(104)
        var decoder = SRUIMessageStreamDecoder()

        XCTAssertEqual(
            try decoder.appendAndExtract(
                incoming: goldenFramed.prefix(prefixLength - 1)
            ),
            [],
            "partial varint prefix should wait"
        )

        XCTAssertEqual(
            try decoder.appendAndExtract(
                incoming: goldenFramed.suffix(from: prefixLength - 1)
            ),
            [goldenMessage]
        )
        XCTAssertEqual(decoder.bufferedByteCount, 0)
    }

    private func makeSampleMessage() -> SRUIMessage {
        var transaction = SRUITransaction()
        transaction.baseRevision = 1
        transaction.newRevision = 2
        transaction.priority = 1

        var message = SRUIMessage()
        message.transaction = transaction
        return message
    }

    private func encodeVarint(_ value: UInt64, into data: inout Data) {
        var remaining = value
        while remaining >= 0x80 {
            data.append(UInt8((remaining & 0x7f) | 0x80))
            remaining >>= 7
        }
        data.append(UInt8(remaining))
    }

    private func varintPrefixLength(_ value: Int) -> Int {
        var data = Data()
        encodeVarint(UInt64(value), into: &data)
        return data.count
    }
}
