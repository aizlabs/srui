//
// Ids.swift
// SemanticModel
//
// Semantic graph identifier types and standard registry constants (§6.2, §6.4).
// Platform-neutral: NEVER import AppKit or Cocoa in this file.
//

import Foundation

/// Namespace 0 is permanently reserved for the canonical SRUI standard registry (§6.4).
public let standardNamespaceID: UInt32 = 0


// MARK: - Node Identifier (§6.2)

/// Strongly-typed identifier for a node in the semantic graph (§6.2).
public struct NodeId: Hashable, Equatable, Comparable, Sendable, CustomStringConvertible, ExpressibleByIntegerLiteral {
    public let value: UInt64

    public init(_ value: UInt64) {
        self.value = value
    }

    public init(integerLiteral value: UInt64) {
        self.value = value
    }

    public var description: String {
        "NodeId(\(value))"
    }

    public static func < (lhs: NodeId, rhs: NodeId) -> Bool {
        lhs.value < rhs.value
    }
}

// MARK: - Item Identifier (§6.2)

/// Strongly-typed identifier for an item within a collection / list / table (§6.2).
public struct ItemId: Hashable, Equatable, Comparable, Sendable, CustomStringConvertible, ExpressibleByIntegerLiteral {
    public let value: UInt64

    public init(_ value: UInt64) {
        self.value = value
    }

    public init(integerLiteral value: UInt64) {
        self.value = value
    }

    public var description: String {
        "ItemId(\(value))"
    }

    public static func < (lhs: ItemId, rhs: ItemId) -> Bool {
        lhs.value < rhs.value
    }
}

// MARK: - Model Identifier (§6.2, §8)

/// Strongly-typed identifier for a collection model in the semantic graph (§6.2, §8).
public struct ModelId: Hashable, Equatable, Comparable, Sendable, CustomStringConvertible, ExpressibleByIntegerLiteral {
    public let value: UInt64

    public init(_ value: UInt64) {
        self.value = value
    }

    public init(integerLiteral value: UInt64) {
        self.value = value
    }

    public var description: String {
        "ModelId(\(value))"
    }

    public static func < (lhs: ModelId, rhs: ModelId) -> Bool {
        lhs.value < rhs.value
    }
}

// MARK: - Resource Hash (§7.4, §8, §18)

/// Error parsing a resource hash from string format.
public enum ParseResourceHashError: Error, Hashable, Equatable, Sendable, CustomStringConvertible {
    case invalidLength(Int)
    case invalidHexCharacter

    public var description: String {
        switch self {
        case .invalidLength(let len):
            return "expected 64 hex characters, got length \(len)"
        case .invalidHexCharacter:
            return "invalid hex character in resource hash"
        }
    }
}

/// 256-bit SHA-256 binary digest identifying content-addressed resources (§7.4, §8, §18).
public struct ResourceHash: Hashable, Equatable, Sendable, CustomStringConvertible {
    public let bytes: Data

    public init(bytes: Data) throws {
        guard bytes.count == 32 else {
            throw ParseResourceHashError.invalidLength(bytes.count)
        }
        self.bytes = bytes
    }

    public init(rawBytes: [UInt8]) throws {
        guard rawBytes.count == 32 else {
            throw ParseResourceHashError.invalidLength(rawBytes.count)
        }
        self.bytes = Data(rawBytes)
    }

    public init(hex: String) throws {
        var clean = hex
        if clean.hasPrefix("sha256:") {
            clean = String(clean.dropFirst(7))
        }
        guard clean.count == 64 else {
            throw ParseResourceHashError.invalidLength(clean.count)
        }

        var data = Data(capacity: 32)
        var index = clean.startIndex
        while index < clean.endIndex {
            let nextIndex = clean.index(index, offsetBy: 2)
            let byteString = clean[index..<nextIndex]
            guard let byte = UInt8(byteString, radix: 16) else {
                throw ParseResourceHashError.invalidHexCharacter
            }
            data.append(byte)
            index = nextIndex
        }
        self.bytes = data
    }

    public func toHex() -> String {
        bytes.map { String(format: "%02x", $0) }.joined()
    }

    public var asBytes: [UInt8] {
        Array(bytes)
    }

    public var description: String {
        "sha256:\(toHex())"
    }
}

// MARK: - Registry Lookup Error

/// Error returned when resolving a registry symbol name fails.
public enum RegistryLookupError: Error, Hashable, Equatable, Sendable, CustomStringConvertible {
    case unknownNodeType(String)
    case unknownProperty(String)
    case unknownEnum(String)
    case unknownEvent(String)
    case unknownOperation(String)

    public var description: String {
        switch self {
        case .unknownNodeType(let name):
            return "unknown standard node type: \"\(name)\""
        case .unknownProperty(let name):
            return "unknown standard property: \"\(name)\""
        case .unknownEnum(let name):
            return "unknown standard enum: \"\(name)\""
        case .unknownEvent(let name):
            return "unknown standard event: \"\(name)\""
        case .unknownOperation(let name):
            return "unknown standard operation: \"\(name)\""
        }
    }
}

// MARK: - Type Reference (§6.4, §7.2)

/// Strongly-typed reference to a node or event type in a namespace registry (§6.4, §7.2).
public struct TypeRef: Hashable, Equatable, Comparable, Sendable, CustomStringConvertible {
    public var namespaceID: UInt32
    public var localID: UInt32

    public init(namespaceID: UInt32, localID: UInt32) {
        self.namespaceID = namespaceID
        self.localID = localID
    }

    public static func standard(_ localID: UInt32) -> TypeRef {
        TypeRef(namespaceID: standardNamespaceID, localID: localID)
    }

    public var isStandard: Bool {
        namespaceID == standardNamespaceID
    }

    public var standardName: String? {
        isStandard ? lookupStandardNodeTypeName(localID) : nil
    }

    public var standardEventName: String? {
        isStandard ? lookupStandardEventName(localID) : nil
    }

    public static func resolveStandard(_ name: String) -> Result<TypeRef, RegistryLookupError> {
        if let id = lookupStandardNodeType(name) {
            return .success(TypeRef.standard(id))
        }
        return .failure(.unknownNodeType(name))
    }

    public static func resolveStandardEvent(_ name: String) -> Result<TypeRef, RegistryLookupError> {
        if let id = lookupStandardEvent(name) {
            return .success(TypeRef.standard(id))
        }
        return .failure(.unknownEvent(name))
    }

    public var description: String {
        if isStandard {
            if let name = standardName {
                return "TypeRef(standard:\(localID)/\(name))"
            }
            if let evName = standardEventName {
                return "TypeRef(standard:\(localID)/\(evName))"
            }
        }
        return "TypeRef(\(namespaceID):\(localID))"
    }

    public static func < (lhs: TypeRef, rhs: TypeRef) -> Bool {
        if lhs.namespaceID != rhs.namespaceID {
            return lhs.namespaceID < rhs.namespaceID
        }
        return lhs.localID < rhs.localID
    }
}

// MARK: - Property Reference (§6.4, §7.4)

/// Strongly-typed reference to a property definition in a namespace registry (§6.4, §7.4).
public struct PropertyRef: Hashable, Equatable, Comparable, Sendable, CustomStringConvertible {
    public var namespaceID: UInt32
    public var localID: UInt32

    public init(namespaceID: UInt32, localID: UInt32) {
        self.namespaceID = namespaceID
        self.localID = localID
    }

    public static func standard(_ localID: UInt32) -> PropertyRef {
        PropertyRef(namespaceID: standardNamespaceID, localID: localID)
    }

    public var isStandard: Bool {
        namespaceID == standardNamespaceID
    }

    public var standardName: String? {
        isStandard ? lookupStandardPropertyName(localID) : nil
    }

    public static func resolveStandard(_ name: String) -> Result<PropertyRef, RegistryLookupError> {
        if let id = lookupStandardProperty(name) {
            return .success(PropertyRef.standard(id))
        }
        return .failure(.unknownProperty(name))
    }

    public var description: String {
        if isStandard {
            if let name = standardName {
                return "PropertyRef(standard:\(localID)/\(name))"
            }
        }
        return "PropertyRef(\(namespaceID):\(localID))"
    }

    public static func < (lhs: PropertyRef, rhs: PropertyRef) -> Bool {
        if lhs.namespaceID != rhs.namespaceID {
            return lhs.namespaceID < rhs.namespaceID
        }
        return lhs.localID < rhs.localID
    }
}

// MARK: - Enum Token (§7.5)

/// Strongly-typed standard or custom enum token pair (enum_id, value_id) (§7.5).
public struct EnumToken: Hashable, Equatable, Comparable, Sendable, CustomStringConvertible {
    public var enumID: UInt32
    public var valueID: UInt32

    public init(enumID: UInt32, valueID: UInt32) {
        self.enumID = enumID
        self.valueID = valueID
    }

    public static func resolveStandard(enumName: String, valueName: String) -> EnumToken? {
        resolveStandardEnumValue(enumName: enumName, valueName: valueName)
    }

    public var standardEnumName: String? {
        lookupStandardEnumName(enumID)
    }

    public var standardValueName: String? {
        lookupStandardEnumValueName(enumID: enumID, valueID: valueID)
    }

    public var description: String {
        if let eName = standardEnumName, let vName = standardValueName {
            return "EnumToken(\(eName).\(vName))"
        }
        return "EnumToken(\(enumID):\(valueID))"
    }

    public static func < (lhs: EnumToken, rhs: EnumToken) -> Bool {
        if lhs.enumID != rhs.enumID {
            return lhs.enumID < rhs.enumID
        }
        return lhs.valueID < rhs.valueID
    }
}

// MARK: - Top-level Resolvers

/// Resolves a standard node type name to a standard namespace 0 `TypeRef`.
public func resolveStandardNodeType(_ name: String) -> Result<TypeRef, RegistryLookupError> {
    TypeRef.resolveStandard(name)
}

/// Resolves a standard property name to a standard namespace 0 `PropertyRef`.
public func resolveStandardProperty(_ name: String) -> Result<PropertyRef, RegistryLookupError> {
    PropertyRef.resolveStandard(name)
}

/// Resolves a standard event name to a standard namespace 0 `TypeRef`.
public func resolveStandardEvent(_ name: String) -> Result<TypeRef, RegistryLookupError> {
    TypeRef.resolveStandardEvent(name)
}
