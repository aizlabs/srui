import Foundation
import XCTest
@testable import Protocol

final class FramingStreamDecoderTests: XCTestCase {
    func testFragmentedPrefixAndPayloadAreRetainedUntilComplete() throws {
        let message = makeClientHello(
            version: String(repeating: "v", count: 256),
            instanceByte: 1
        )
        let framed = try SRUIFraming.encodeFramed(message)
        XCTAssertNotEqual(framed[framed.startIndex] & 0x80, 0)

        var decoder = SRUIMessageStreamDecoder()

        let firstBoundary = framed.index(after: framed.startIndex)
        let secondBoundary = framed.index(
            firstBoundary,
            offsetBy: 10,
            limitedBy: framed.endIndex
        ) ?? framed.endIndex

        XCTAssertEqual(
            try decoder.appendAndExtract(
                incoming: Data(framed[..<firstBoundary])
            ),
            []
        )
        XCTAssertEqual(decoder.bufferedByteCount, 1)

        XCTAssertEqual(
            try decoder.appendAndExtract(
                incoming: Data(framed[firstBoundary..<secondBoundary])
            ),
            []
        )
        XCTAssertEqual(decoder.bufferedByteCount, secondBoundary)

        XCTAssertEqual(
            try decoder.appendAndExtract(
                incoming: Data(framed[secondBoundary...])
            ),
            [message]
        )
        XCTAssertEqual(decoder.bufferedByteCount, 0)
    }

    func testCoalescedFramesAreExtractedInWireOrder() throws {
        let first = makeClientHello(version: "0.4-first", instanceByte: 1)
        let second = makeClientHello(version: "0.4-second", instanceByte: 2)

        var incoming = try SRUIFraming.encodeFramed(first)
        incoming.append(try SRUIFraming.encodeFramed(second))

        var decoder = SRUIMessageStreamDecoder()
        XCTAssertEqual(
            try decoder.appendAndExtract(incoming: incoming),
            [first, second]
        )
        XCTAssertEqual(decoder.bufferedByteCount, 0)
    }

    func testOversizedDeclaredLengthIsRejectedBeforePayloadArrives() {
        var decoder = SRUIMessageStreamDecoder(maxFrameSize: 127)

        XCTAssertThrowsError(
            try decoder.appendAndExtract(incoming: Data([0x80, 0x01]))
        ) { error in
            let streamError = error as? SRUIStreamDecodeError
            XCTAssertEqual(streamError?.decodedMessages, [])
            XCTAssertEqual(
                streamError?.framingError,
                .frameSizeLimitExceeded(limit: 127, actual: 128)
            )
        }
        XCTAssertEqual(decoder.bufferedByteCount, 0)
    }

    func testMalformedVarintClearsBufferAndDecoderCanBeReused() throws {
        var decoder = SRUIMessageStreamDecoder()

        XCTAssertThrowsError(
            try decoder.appendAndExtract(
                incoming: Data(repeating: 0x80, count: 10)
            )
        ) { error in
            let streamError = error as? SRUIStreamDecodeError
            XCTAssertEqual(streamError?.decodedMessages, [])
            XCTAssertEqual(streamError?.framingError, .malformedVarint)
        }
        XCTAssertEqual(decoder.bufferedByteCount, 0)

        let valid = makeClientHello(version: "0.4-recovered", instanceByte: 3)
        XCTAssertEqual(
            try decoder.appendAndExtract(
                incoming: SRUIFraming.encodeFramed(valid)
            ),
            [valid]
        )
    }

    func testSingleFrameDecoderPreservesTrailingPaddingCompatibility() throws {
        let message = makeClientHello(version: "0.4-padding", instanceByte: 4)
        var framed = try SRUIFraming.encodeFramed(message)
        framed.append(Data(repeating: 0, count: 32))

        let decoded = try SRUIFraming.decodeFramed(
            SRUIMessage.self,
            from: framed
        )
        XCTAssertEqual(decoded, message)
    }

    func testSingleFrameDecoderReportsTruncatedPayload() throws {
        let message = makeClientHello(version: "0.4-truncated", instanceByte: 5)
        var framed = try SRUIFraming.encodeFramed(message)
        framed.removeLast()

        XCTAssertThrowsError(
            try SRUIFraming.decodeFramed(SRUIMessage.self, from: framed)
        ) { error in
            guard case .truncatedPayload(let expected, let actual) = error as? SRUIFramingError else {
                return XCTFail("Expected truncatedPayload, got \(error)")
            }
            XCTAssertGreaterThan(expected, actual)
        }
    }

    func testIncompleteLengthPrefixIsDistinguishedFromTruncatedPayload() {
        XCTAssertThrowsError(
            try SRUIFraming.decodeFramed(SRUIMessage.self, from: Data([0x80]))
        ) { error in
            XCTAssertEqual(
                error as? SRUIFramingError,
                .incompleteLengthPrefix(buffered: 1)
            )
        }
    }

    func testDecodedPrefixIsReportedWhenALaterFrameFails() throws {
        let first = makeClientHello(version: "0.4-first", instanceByte: 1)

        var incoming = try SRUIFraming.encodeFramed(first)
        // Frame two declares one payload byte that is not valid protobuf.
        incoming.append(Data([0x01, 0x08]))

        var decoder = SRUIMessageStreamDecoder()
        XCTAssertThrowsError(
            try decoder.appendAndExtract(incoming: incoming)
        ) { error in
            guard let streamError = error as? SRUIStreamDecodeError else {
                return XCTFail("Expected SRUIStreamDecodeError, got \(error)")
            }
            XCTAssertEqual(streamError.decodedMessages, [first])
        }
        XCTAssertEqual(decoder.bufferedByteCount, 0)
    }

    private func makeClientHello(
        version: String,
        instanceByte: UInt8
    ) -> SRUIMessage {
        var hello = SRUIClientHello()
        hello.coreVersion = version
        hello.clientInstanceID = Data([instanceByte])

        var message = SRUIMessage()
        message.clientHello = hello
        return message
    }
}
