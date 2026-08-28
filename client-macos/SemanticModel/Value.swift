//
// Value.swift
// SemanticModel
//
// Semantic value types representing the 17 dynamic value variants in SRUI (§6.5).
// Platform-neutral: NEVER import AppKit or Cocoa in this file.
//

import Foundation
import Protocol

// MARK: - Geometric & Semantic Value Tuples

/// Strongly-typed 2D size tuple (width, height) in logical points.
public struct Size: Hashable, Equatable, Sendable, CustomStringConvertible {
    public var width: Double
    public var height: Double

    public init(width: Double, height: Double) {
        self.width = width
        self.height = height
    }

    public var description: String {
        "Size(\(width)x\(height))"
    }
}

/// Strongly-typed 2D point tuple (x, y) in logical points.
public struct Point: Hashable, Equatable, Sendable, CustomStringConvertible {
    public var x: Double
    public var y: Double

    public init(x: Double, y: Double) {
        self.x = x
        self.y = y
    }

    public var description: String {
        "Point(\(x), \(y))"
    }
}

/// Strongly-typed 1D continuous range tuple (start, length).
public struct SemanticRange: Hashable, Equatable, Sendable, CustomStringConvertible {
    public var start: UInt64
    public var length: UInt64

    public var location: UInt64 {
        get { start }
        set { start = newValue }
    }

    public init(start: UInt64, length: UInt64) {
        self.start = start
        self.length = length
    }

    public init(location: UInt64, length: UInt64) {
        self.start = location
        self.length = length
    }

    public var description: String {
        "Range(start: \(start), length: \(length))"
    }
}

public typealias RangeVal = SemanticRange

/// Strongly-typed 2D rectangle tuple (x, y, width, height) in logical points.
public struct Rect: Hashable, Equatable, Sendable, CustomStringConvertible {
    public var x: Double
    public var y: Double
    public var width: Double
    public var height: Double

    public init(x: Double, y: Double, width: Double, height: Double) {
        self.x = x
        self.y = y
        self.width = width
        self.height = height
    }

    public var description: String {
        "Rect(\(x), \(y), \(width)x\(height))"
    }
}

/// Strongly-typed 4-edge insets tuple (top, leading, bottom, trailing).
public struct EdgeInsets: Hashable, Equatable, Sendable, CustomStringConvertible {
    public var top: Double
    public var leading: Double
    public var bottom: Double
    public var trailing: Double

    public init(top: Double, leading: Double, bottom: Double, trailing: Double) {
        self.top = top
        self.leading = leading
        self.bottom = bottom
        self.trailing = trailing
    }

    public var description: String {
        "EdgeInsets(top: \(top), leading: \(leading), bottom: \(bottom), trailing: \(trailing))"
    }
}

// MARK: - Property & SmallRecord

/// A property key-value pair.
public struct Property: Hashable, Equatable, Sendable, CustomStringConvertible {
    public var property: PropertyRef
    public var value: Value

    public init(property: PropertyRef, value: Value) {
        self.property = property
        self.value = value
    }

    public var description: String {
        "Property(\(property), \(value))"
    }

    public init(wire: SRUIProperty) throws {
        guard wire.hasProperty else {
            throw ValueConversionError.missingField("property")
        }
        self.property = PropertyRef(wire: wire.property)
        if wire.hasValue {
            self.value = try Value(wire: wire.value)
        } else {
            self.value = .null
        }
    }

    public func toWire() -> SRUIProperty {
        SRUIProperty.with {
            $0.property = self.property.toWire()
            $0.value = self.value.toWire()
        }
    }
}

/// Small typed record representing a structured non-node composite value (§7.6).
public struct SmallRecord: Hashable, Equatable, Sendable, CustomStringConvertible {
    public var typeRef: TypeRef
    public var properties: [Property]

    public init(typeRef: TypeRef, properties: [Property]) {
        self.typeRef = typeRef
        self.properties = properties
    }

    public var description: String {
        "SmallRecord(\(typeRef), \(properties.count) properties)"
    }
}

// MARK: - Value Conversion Error

/// Error converting between protobuf wire representations and in-memory Value types.
public enum ValueConversionError: Error, Hashable, Equatable, Sendable, CustomStringConvertible {
    case missingField(String)
    case invalidResourceHashLength(Int)
    case invalidNestedStructure(String)

    public var description: String {
        switch self {
        case .missingField(let field):
            return "missing expected protobuf wire field: \(field)"
        case .invalidResourceHashLength(let len):
            return "expected 32-byte resource hash, got \(len) bytes"
        case .invalidNestedStructure(let msg):
            return "invalid nested structure: \(msg)"
        }
    }
}

// MARK: - Value Enum (§6.5)

/// Dynamic value holding any of the 17 SRUI value types (§6.5).
public enum Value: Hashable, Equatable, Sendable, CustomStringConvertible {
    /// 1. Null / absence of value
    case null
    /// 2. Boolean (true / false)
    case bool(Bool)
    /// 3. 64-bit signed integer
    case signedInt(Int64)
    /// 4. 64-bit unsigned integer
    case unsignedInt(UInt64)
    /// 5. 64-bit floating point number
    case float64(Double)
    /// 6. UTF-8 string
    case string(String)
    /// 7. Strongly-typed node graph reference
    case nodeID(NodeId)
    /// 8. Strongly-typed collection item reference
    case itemID(ItemId)
    /// 9. 256-bit SHA-256 binary resource hash
    case resourceHash(ResourceHash)
    /// 10. Namespace-qualified enum token
    case enumToken(EnumToken)
    /// 11. 2D size tuple (width, height)
    case size(Size)
    /// 12. 2D point tuple (x, y)
    case point(Point)
    /// 13. 1D continuous range tuple (start, length)
    case range(SemanticRange)
    /// 14. 2D rectangle tuple (x, y, width, height)
    case rect(Rect)
    /// 15. 4-edge insets tuple (top, leading, bottom, trailing)
    case edgeInsets(EdgeInsets)
    /// 16. Homogeneous/heterogeneous list of scalar values
    indirect case list([Value])
    /// 17. Small typed composite record
    indirect case record(SmallRecord)

    /// Returns `true` if this value is scalar (§6.5).
    public var isScalar: Bool {
        switch self {
        case .list, .record:
            return false
        default:
            return true
        }
    }

    /// Returns `true` if this value is `Value.null`.
    public var isNull: Bool {
        if case .null = self { return true }
        return false
    }

    public var asBool: Bool? {
        if case .bool(let b) = self { return b }
        return nil
    }

    public var asSignedInt: Int64? {
        if case .signedInt(let i) = self { return i }
        return nil
    }

    public var asUnsignedInt: UInt64? {
        if case .unsignedInt(let u) = self { return u }
        return nil
    }

    public var asFloat64: Double? {
        if case .float64(let f) = self { return f }
        return nil
    }

    public var asString: String? {
        if case .string(let s) = self { return s }
        return nil
    }

    public var asNodeID: NodeId? {
        if case .nodeID(let id) = self { return id }
        return nil
    }

    public var asItemID: ItemId? {
        if case .itemID(let id) = self { return id }
        return nil
    }

    public var asResourceHash: ResourceHash? {
        if case .resourceHash(let h) = self { return h }
        return nil
    }

    public var asEnumToken: EnumToken? {
        if case .enumToken(let e) = self { return e }
        return nil
    }

    public var asSize: Size? {
        if case .size(let s) = self { return s }
        return nil
    }

    public var asPoint: Point? {
        if case .point(let p) = self { return p }
        return nil
    }

    public var asRange: SemanticRange? {
        if case .range(let r) = self { return r }
        return nil
    }

    public var asRect: Rect? {
        if case .rect(let r) = self { return r }
        return nil
    }

    public var asEdgeInsets: EdgeInsets? {
        if case .edgeInsets(let i) = self { return i }
        return nil
    }

    public var asList: [Value]? {
        if case .list(let l) = self { return l }
        return nil
    }

    public var asRecord: SmallRecord? {
        if case .record(let r) = self { return r }
        return nil
    }

    public var description: String {
        switch self {
        case .null:
            return "null"
        case .bool(let b):
            return "\(b)"
        case .signedInt(let i):
            return "\(i)"
        case .unsignedInt(let u):
            return "\(u)"
        case .float64(let f):
            return "\(f)"
        case .string(let s):
            return "\"\(s)\""
        case .nodeID(let id):
            return "\(id)"
        case .itemID(let id):
            return "\(id)"
        case .resourceHash(let h):
            return "\(h)"
        case .enumToken(let e):
            return "\(e)"
        case .size(let s):
            return "\(s)"
        case .point(let p):
            return "\(p)"
        case .range(let r):
            return "\(r)"
        case .rect(let r):
            return "\(r)"
        case .edgeInsets(let i):
            return "\(i)"
        case .list(let l):
            return "List(\(l.count) items)"
        case .record(let r):
            return "\(r)"
        }
    }
}

// MARK: - ExpressibleBy Literals

extension Value: ExpressibleByNilLiteral {
    public init(nilLiteral: ()) {
        self = .null
    }
}

extension Value: ExpressibleByBooleanLiteral {
    public init(booleanLiteral value: Bool) {
        self = .bool(value)
    }
}

extension Value: ExpressibleByIntegerLiteral {
    public init(integerLiteral value: Int64) {
        self = .signedInt(value)
    }
}

extension Value: ExpressibleByFloatLiteral {
    public init(floatLiteral value: Double) {
        self = .float64(value)
    }
}

extension Value: ExpressibleByStringLiteral {
    public init(stringLiteral value: String) {
        self = .string(value)
    }
}

extension Value: ExpressibleByArrayLiteral {
    public init(arrayLiteral elements: Value...) {
        self = .list(elements)
    }
}

// MARK: - Wire Protobuf Conversion

extension Value {
    public init(wire: SRUIValue) throws {
        guard let wireVal = wire.value else {
            throw ValueConversionError.missingField("value")
        }

        switch wireVal {
        case .nullValue:
            self = .null
        case .boolValue(let b):
            self = .bool(b)
        case .intValue(let i):
            self = .signedInt(i)
        case .uintValue(let u):
            self = .unsignedInt(u)
        case .floatValue(let f):
            self = .float64(f)
        case .stringValue(let s):
            self = .string(s)
        case .nodeIDValue(let id):
            self = .nodeID(NodeId(id))
        case .itemIDValue(let id):
            self = .itemID(ItemId(id))
        case .resourceHash(let bytes):
            guard bytes.count == 32 else {
                throw ValueConversionError.invalidResourceHashLength(bytes.count)
            }
            self = .resourceHash(try ResourceHash(bytes: bytes))
        case .enumValue(let e):
            self = .enumToken(EnumToken(enumID: e.enumID, valueID: e.valueID))
        case .sizeValue(let s):
            self = .size(Size(width: s.width, height: s.height))
        case .pointValue(let p):
            self = .point(Point(x: p.x, y: p.y))
        case .rangeValue(let r):
            self = .range(SemanticRange(start: r.location, length: r.length))
        case .rectValue(let r):
            self = .rect(Rect(x: r.x, y: r.y, width: r.width, height: r.height))
        case .insetsValue(let i):
            self = .edgeInsets(EdgeInsets(top: i.top, leading: i.leading, bottom: i.bottom, trailing: i.trailing))
        case .listValue(let l):
            var items: [Value] = []
            items.reserveCapacity(l.values.count)
            for v in l.values {
                items.append(try Value(wire: v))
            }
            self = .list(items)
        case .recordValue(let r):
            guard r.hasType else {
                throw ValueConversionError.missingField("record.type")
            }
            let typeRef = TypeRef(wire: r.type)
            var props: [Property] = []
            props.reserveCapacity(r.properties.count)
            for p in r.properties {
                props.append(try Property(wire: p))
            }
            self = .record(SmallRecord(typeRef: typeRef, properties: props))
        }
    }

    public func toWire() -> SRUIValue {
        var wire = SRUIValue()
        switch self {
        case .null:
            wire.nullValue = .nullValue
        case .bool(let b):
            wire.boolValue = b
        case .signedInt(let i):
            wire.intValue = i
        case .unsignedInt(let u):
            wire.uintValue = u
        case .float64(let f):
            wire.floatValue = f
        case .string(let s):
            wire.stringValue = s
        case .nodeID(let id):
            wire.nodeIDValue = id.value
        case .itemID(let id):
            wire.itemIDValue = id.value
        case .resourceHash(let h):
            wire.resourceHash = h.bytes
        case .enumToken(let e):
            wire.enumValue = Srui_Protocol_EnumValue.with {
                $0.enumID = e.enumID
                $0.valueID = e.valueID
            }
        case .size(let s):
            wire.sizeValue = Srui_Protocol_SizeVal.with {
                $0.width = s.width
                $0.height = s.height
            }
        case .point(let p):
            wire.pointValue = Srui_Protocol_PointVal.with {
                $0.x = p.x
                $0.y = p.y
            }
        case .range(let r):
            wire.rangeValue = Srui_Protocol_RangeVal.with {
                $0.location = r.start
                $0.length = r.length
            }
        case .rect(let r):
            wire.rectValue = Srui_Protocol_RectVal.with {
                $0.x = r.x
                $0.y = r.y
                $0.width = r.width
                $0.height = r.height
            }
        case .edgeInsets(let i):
            wire.insetsValue = Srui_Protocol_EdgeInsetsVal.with {
                $0.top = i.top
                $0.leading = i.leading
                $0.bottom = i.bottom
                $0.trailing = i.trailing
            }
        case .list(let list):
            wire.listValue = Srui_Protocol_ValueList.with {
                $0.values = list.map { $0.toWire() }
            }
        case .record(let rec):
            wire.recordValue = Srui_Protocol_SmallRecord.with {
                $0.type = rec.typeRef.toWire()
                $0.properties = rec.properties.map { $0.toWire() }
            }
        }
        return wire
    }
}
