import Foundation
import SwiftProtobuf

/// Default maximum allowable wire frame size in bytes (16 MiB, §26).
public let defaultMaxFrameSize: Int = 16 * 1024 * 1024

/// Errors returned by length-delimited wire framing operations (§16, §26).
public enum SRUIFramingError: Error, Equatable, Sendable, CustomStringConvertible {
    case frameSizeLimitExceeded(limit: Int, actual: Int)
    case malformedVarint
    case truncatedPayload(expected: Int, actual: Int)

    public var description: String {
        switch self {
        case .frameSizeLimitExceeded(let limit, let actual):
            return "Wire frame size limit exceeded: max allowed is \(limit) bytes, actual is \(actual) bytes (§26)"
        case .malformedVarint:
            return "Malformed varint length prefix in wire frame"
        case .truncatedPayload(let expected, let actual):
            return "Truncated frame payload: expected \(expected) bytes, got \(actual) bytes"
        }
    }
}

private enum VarintDecodeResult {
    case incomplete
    case complete(value: UInt64, nextOffset: Int)
}

private func encodeVarint(_ value: UInt64, into data: inout Data) {
    var remaining = value
    while remaining >= 0x80 {
        data.append(UInt8((remaining & 0x7f) | 0x80))
        remaining >>= 7
    }
    data.append(UInt8(remaining))
}

private func decodeVarintIfComplete(
    from data: Data,
    offset: Int
) throws -> VarintDecodeResult {
    var cursor = offset
    var value: UInt64 = 0

    for byteIndex in 0..<10 {
        guard cursor < data.endIndex else {
            return .incomplete
        }

        let byte = data[cursor]
        cursor = data.index(after: cursor)

        // A UInt64 varint's tenth byte may contain only bit 0.
        if byteIndex == 9, byte > 0x01 {
            throw SRUIFramingError.malformedVarint
        }

        value |= UInt64(byte & 0x7f) << UInt64(byteIndex * 7)
        if byte & 0x80 == 0 {
            return .complete(value: value, nextOffset: cursor)
        }
    }

    throw SRUIFramingError.malformedVarint
}

private func checkedFrameLength(
    _ length: UInt64,
    maxFrameSize: Int
) throws -> Int {
    let actual = length > UInt64(Int.max) ? Int.max : Int(length)

    guard maxFrameSize >= 0, length <= UInt64(maxFrameSize) else {
        throw SRUIFramingError.frameSizeLimitExceeded(
            limit: maxFrameSize,
            actual: actual
        )
    }

    return actual
}

/// Length-delimited wire framing helpers implementing §16 reference Protobuf envelope encoding.
public enum SRUIFraming {
    /// Serializes a message with a varint length prefix, enforcing `defaultMaxFrameSize` (§16, §26).
    public static func encodeFramed<M: SwiftProtobuf.Message>(_ message: M) throws -> Data {
        try encodeFramed(message, maxFrameSize: defaultMaxFrameSize)
    }

    /// Serializes a message with a varint length prefix, enforcing a custom `maxFrameSize` (§16, §26).
    public static func encodeFramed<M: SwiftProtobuf.Message>(
        _ message: M,
        maxFrameSize: Int
    ) throws -> Data {
        let payload = try message.serializedData()
        guard payload.count <= maxFrameSize else {
            throw SRUIFramingError.frameSizeLimitExceeded(
                limit: maxFrameSize,
                actual: payload.count
            )
        }

        var framed = Data()
        framed.reserveCapacity(payload.count + 10)
        encodeVarint(UInt64(payload.count), into: &framed)
        framed.append(payload)
        return framed
    }

    /// Decodes one length-delimited message (§16), ignoring bytes after its declared payload.
    public static func decodeFramed<M: SwiftProtobuf.Message>(
        _ type: M.Type,
        from data: Data
    ) throws -> M {
        try decodeFramed(type, from: data, maxFrameSize: defaultMaxFrameSize)
    }

    /// Decodes one length-delimited message while enforcing a custom `maxFrameSize` (§16, §26).
    ///
    /// This buffer-oriented API intentionally ignores trailing bytes, matching the Rust
    /// reference decoder. Use `SRUIMessageStreamDecoder` to consume every frame in a transport
    /// chunk while preserving incomplete data for the next chunk.
    public static func decodeFramed<M: SwiftProtobuf.Message>(
        _ type: M.Type,
        from data: Data,
        maxFrameSize: Int
    ) throws -> M {
        let prefix = try decodeVarintIfComplete(
            from: data,
            offset: data.startIndex
        )

        guard case .complete(let length, let payloadStart) = prefix else {
            let expected = data.count == Int.max ? Int.max : data.count + 1
            throw SRUIFramingError.truncatedPayload(
                expected: expected,
                actual: data.count
            )
        }

        let payloadLength = try checkedFrameLength(
            length,
            maxFrameSize: maxFrameSize
        )
        let availablePayloadBytes = data.distance(
            from: payloadStart,
            to: data.endIndex
        )

        guard availablePayloadBytes >= payloadLength else {
            throw SRUIFramingError.truncatedPayload(
                expected: payloadLength,
                actual: availablePayloadBytes
            )
        }

        let payloadEnd = data.index(payloadStart, offsetBy: payloadLength)
        return try M(serializedBytes: data[payloadStart..<payloadEnd])
    }
}

/// Stateful decoder for length-delimited `SRUIMessage` data arriving in arbitrary chunks.
///
/// The decoder is a mutable value type: transfer it between concurrency domains as needed, but
/// serialize calls to `appendAndExtract(incoming:)` on a single instance. Incomplete prefixes and
/// payloads remain buffered. Complete coalesced frames are returned in wire order. Any malformed,
/// oversized, or invalid protobuf frame clears the buffer and throws so decoding fails closed.
public struct SRUIMessageStreamDecoder: Sendable {
    private var buffer = Data()

    public let maxFrameSize: Int

    public init(maxFrameSize: Int = defaultMaxFrameSize) {
        self.maxFrameSize = maxFrameSize
    }

    /// Number of incomplete bytes currently retained for a later chunk.
    public var bufferedByteCount: Int {
        buffer.count
    }

    /// Appends transport bytes and extracts every complete message now available.
    public mutating func appendAndExtract(incoming: Data) throws -> [SRUIMessage] {
        buffer.append(incoming)

        do {
            var messages: [SRUIMessage] = []
            var frameStart = buffer.startIndex

            while frameStart < buffer.endIndex {
                let prefix = try decodeVarintIfComplete(
                    from: buffer,
                    offset: frameStart
                )

                guard case .complete(let length, let payloadStart) = prefix else {
                    discardConsumedBytes(before: frameStart)
                    return messages
                }

                let payloadLength = try checkedFrameLength(
                    length,
                    maxFrameSize: maxFrameSize
                )
                let availablePayloadBytes = buffer.distance(
                    from: payloadStart,
                    to: buffer.endIndex
                )

                guard availablePayloadBytes >= payloadLength else {
                    discardConsumedBytes(before: frameStart)
                    return messages
                }

                let payloadEnd = buffer.index(
                    payloadStart,
                    offsetBy: payloadLength
                )
                let message = try SRUIMessage(
                    serializedBytes: buffer[payloadStart..<payloadEnd]
                )
                messages.append(message)
                frameStart = payloadEnd
            }

            buffer.removeAll(keepingCapacity: true)
            return messages
        } catch {
            buffer.removeAll(keepingCapacity: false)
            throw error
        }
    }

    private mutating func discardConsumedBytes(before index: Int) {
        guard index > buffer.startIndex else {
            return
        }
        buffer.removeSubrange(buffer.startIndex..<index)
    }
}
