import Foundation
import SwiftProtobuf

/// Length-delimited wire framing helpers implementing §16 reference Protobuf envelope encoding.
public enum SRUIFraming {
    /// Serializes a message with a varint length prefix per §16 reference wire framing.
    public static func encodeFramed<M: SwiftProtobuf.Message>(_ message: M) throws -> Data {
        let serialized = try message.serializedData()
        var varintData = Data()
        var value = UInt64(serialized.count)
        while value >= 0x80 {
            varintData.append(UInt8((value & 0x7F) | 0x80))
            value >>= 7
        }
        varintData.append(UInt8(value & 0x7F))
        return varintData + serialized
    }

    /// Decodes a length-delimited message (§16) from Data.
    public static func decodeFramed<M: SwiftProtobuf.Message>(_ type: M.Type, from data: Data) throws -> M {
        let stream = InputStream(data: data)
        stream.open()
        defer { stream.close() }
        return try BinaryDelimited.parse(messageType: type, from: stream)
    }
}
