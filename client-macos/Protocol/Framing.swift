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

    /// Serializes a message with a varint length prefix via `BinaryDelimited`, enforcing a custom `maxFrameSize` (§16, §26).
    public static func encodeFramed<M: SwiftProtobuf.Message>(_ message: M, maxFrameSize: Int) throws -> Data {
        let serializedSize = try message.serializedData().count
        if serializedSize > maxFrameSize {
            throw SRUIFramingError.frameSizeLimitExceeded(limit: maxFrameSize, actual: serializedSize)
        }
        let stream = OutputStream.toMemory()
        stream.open()
        defer { stream.close() }
        try BinaryDelimited.serialize(message: message, to: stream)
        guard let data = stream.property(forKey: .dataWrittenToMemoryStreamKey) as? Data else {
            throw SRUIFramingError.truncatedPayload(expected: serializedSize, actual: 0)
        }
        return data
    }

    /// Decodes a length-delimited message (§16) from Data via `BinaryDelimited`, enforcing `defaultMaxFrameSize` (§26).
    public static func decodeFramed<M: SwiftProtobuf.Message>(_ type: M.Type, from data: Data) throws -> M {
        try decodeFramed(type, from: data, maxFrameSize: defaultMaxFrameSize)
    }

    /// Decodes a length-delimited message (§16) from Data via `BinaryDelimited`, enforcing a custom `maxFrameSize` (§26).
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
