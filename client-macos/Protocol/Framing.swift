import Foundation
import SwiftProtobuf

/// Default maximum allowable wire frame size in bytes (16 MiB, §26).
public let defaultMaxFrameSize: Int = 16 * 1024 * 1024

/// Errors returned by length-delimited wire framing operations (§16, §26).
public enum SRUIFramingError: Error, Equatable, CustomStringConvertible {
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

/// Length-delimited wire framing helpers implementing §16 reference Protobuf envelope encoding.
public enum SRUIFraming {
    /// Serializes a message with a varint length prefix, enforcing `defaultMaxFrameSize` (§16, §26).
    public static func encodeFramed<M: SwiftProtobuf.Message>(_ message: M) throws -> Data {
        try encodeFramed(message, maxFrameSize: defaultMaxFrameSize)
    }

    /// Serializes a message with a varint length prefix, enforcing a custom `maxFrameSize` (§16, §26).
    public static func encodeFramed<M: SwiftProtobuf.Message>(_ message: M, maxFrameSize: Int) throws -> Data {
        let serialized = try message.serializedData()
        if serialized.count > maxFrameSize {
            throw SRUIFramingError.frameSizeLimitExceeded(limit: maxFrameSize, actual: serialized.count)
        }
        var varintData = Data()
        var value = UInt64(serialized.count)
        while value >= 0x80 {
            varintData.append(UInt8((value & 0x7F) | 0x80))
            value >>= 7
        }
        varintData.append(UInt8(value & 0x7F))
        return varintData + serialized
    }

    /// Decodes a length-delimited message (§16) from Data, enforcing `defaultMaxFrameSize` (§26).
    public static func decodeFramed<M: SwiftProtobuf.Message>(_ type: M.Type, from data: Data) throws -> M {
        try decodeFramed(type, from: data, maxFrameSize: defaultMaxFrameSize)
    }

    /// Decodes a length-delimited message (§16) from Data, enforcing a custom `maxFrameSize` (§26).
    public static func decodeFramed<M: SwiftProtobuf.Message>(_ type: M.Type, from data: Data, maxFrameSize: Int) throws -> M {
        // Read varint length prefix
        var offset = 0
        var length: UInt64 = 0
        var shift: UInt64 = 0
        while offset < data.count {
            let byte = data[offset]
            offset += 1
            length |= UInt64(byte & 0x7F) << shift
            if (byte & 0x80) == 0 {
                break
            }
            shift += 7
            if shift >= 64 {
                throw SRUIFramingError.malformedVarint
            }
        }
        if length > UInt64(maxFrameSize) {
            throw SRUIFramingError.frameSizeLimitExceeded(limit: maxFrameSize, actual: Int(length))
        }
        let stream = InputStream(data: data)
        stream.open()
        defer { stream.close() }
        return try BinaryDelimited.parse(messageType: type, from: stream)
    }
}
