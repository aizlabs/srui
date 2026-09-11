//
// Capability.swift
// SemanticModel
//
// Semantic capability profiles, sets, and negotiation logic (§4 inv. 13, §6.1, §6.4, §11, §15).
// Normative requirement: this target must NEVER import AppKit or Cocoa.
//

import Foundation

/// Canonical standard and extension profile identifiers (§6.4, §7, §9, §11, §21, §30).
public enum StandardProfiles {
    public static let standardWidgets = "org.srui.standard-widgets"
    public static let terminal = "org.srui.terminal"
    public static let richtext = "org.srui.richtext"
    public static let vectorScene = "org.srui.vector-scene"
    public static let mediaSurface = "org.srui.media-surface"
    public static let coding = "org.srui.coding"
}

/// Error returned when parsing a capability profile string fails (§6.4, §15).
public enum ParseProfileError: Error, Equatable, Sendable, CustomStringConvertible {
    /// String is empty or contains only whitespace.
    case emptyString
    /// Profile name portion before the version delimiter is empty.
    case emptyName
    /// Missing `'/'` delimiter separating profile name and version number.
    case missingVersionDelimiter(String)
    /// Version portion is not a valid positive integer.
    case invalidVersion(String)

    public var description: String {
        switch self {
        case .emptyString:
            return "profile string is empty"
        case .emptyName:
            return "profile name is empty"
        case .missingVersionDelimiter(let s):
            return "missing version delimiter '/' in profile string: \"\(s)\""
        case .invalidVersion(let v):
            return "invalid profile version number: \"\(v)\""
        }
    }
}

/// Strongly-typed capability profile identifier with a major version (§6.4, §15).
///
/// Formatted on the wire and in configuration as `"<name>/<version>"`, for example:
/// `"org.srui.standard-widgets/1"`.
public struct Profile: Hashable, Equatable, Comparable, Sendable, CustomStringConvertible {
    public let name: String
    public let version: UInt32

    /// Constructs a new `Profile` with name and major version.
    public init(name: String, version: UInt32) throws {
        let trimmed = name.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else {
            throw ParseProfileError.emptyName
        }
        guard version >= 1 else {
            throw ParseProfileError.invalidVersion("version must be >= 1")
        }
        self.name = trimmed
        self.version = version
    }

    /// Constructs a `Profile` unchecked from known-valid static constants.
    public init(uncheckedName name: String, version: UInt32) {
        self.name = name
        self.version = version
    }

    /// Parses a profile identifier string formatted as `"<name>/<version>"`.
    public static func parse(_ string: String) throws -> Profile {
        let trimmed = string.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else {
            throw ParseProfileError.emptyString
        }
        guard let slashIndex = trimmed.lastIndex(of: "/") else {
            throw ParseProfileError.missingVersionDelimiter(string)
        }

        let namePart = String(trimmed[..<slashIndex]).trimmingCharacters(in: .whitespacesAndNewlines)
        guard !namePart.isEmpty else {
            throw ParseProfileError.emptyName
        }

        let versionPart = String(trimmed[trimmed.index(after: slashIndex)...]).trimmingCharacters(in: .whitespacesAndNewlines)
        guard let version = UInt32(versionPart), version >= 1 else {
            throw ParseProfileError.invalidVersion(versionPart)
        }

        return Profile(uncheckedName: namePart, version: version)
    }

    public var description: String {
        "\(name)/\(version)"
    }

    public static func < (lhs: Profile, rhs: Profile) -> Bool {
        if lhs.name != rhs.name {
            return lhs.name < rhs.name
        }
        return lhs.version < rhs.version
    }

    /// Standard Widget Profile v1 (`org.srui.standard-widgets/1`, §7).
    public static let standardWidgetsV1 = Profile(uncheckedName: StandardProfiles.standardWidgets, version: 1)

    /// Terminal Compatibility Extension Profile v1 (`org.srui.terminal/1`, §21).
    public static let terminalV1 = Profile(uncheckedName: StandardProfiles.terminal, version: 1)

    /// RichText Profile v1 (`org.srui.richtext/1`, §9).
    public static let richtextV1 = Profile(uncheckedName: StandardProfiles.richtext, version: 1)

    /// VectorScene Retained Graphics Profile v1 (`org.srui.vector-scene/1`, §11.2).
    public static let vectorSceneV1 = Profile(uncheckedName: StandardProfiles.vectorScene, version: 1)

    /// MediaSurface Video/Surface Streaming Profile v1 (`org.srui.media-surface/1`, §5.2).
    public static let mediaSurfaceV1 = Profile(uncheckedName: StandardProfiles.mediaSurface, version: 1)

    /// Coding Domain Extension Profile v1 (`org.srui.coding/1`, §30).
    public static let codingV1 = Profile(uncheckedName: StandardProfiles.coding, version: 1)
}

/// Error returned when capability negotiation fails (§4 invariant 13, §15).
public enum NegotiationError: Error, Equatable, Sendable, CustomStringConvertible {
    /// One or more required profiles (§15) were not offered by the client.
    case unsatisfiedRequiredProfiles(missing: [Profile])

    public var description: String {
        switch self {
        case .unsatisfiedRequiredProfiles(let missing):
            let formatted = missing.map { "\"\($0)\"" }.joined(separator: ", ")
            return "unsatisfied required capabilities: client did not offer required profile(s): [\(formatted)]"
        }
    }
}

/// Set of capability profiles negotiated between client and server (§15).
///
/// This is `Set<Profile>` so Swift set operations come from the standard library; profile parsing
/// and §15 negotiation live in the extension below.
public typealias CapabilitySet = Set<Profile>

extension Set where Element == Profile {
    /// Constructs a `CapabilitySet` by parsing an array of profile strings.
    ///
    /// Required-profile lists on the wire must use this path: a malformed entry is unknown
    /// required semantics and must fail explicitly (§4 inv. 13).
    public static func fromStrings(_ strings: [String]) throws -> CapabilitySet {
        var set = CapabilitySet()
        for s in strings {
            try set.insert(string: s)
        }
        return set
    }

    /// Parses valid profile strings and ignores invalid entries.
    ///
    /// Use this only for optional / client-offered lists, where unknown profiles are omitted
    /// without error (§15). Never use it for server-required profiles.
    public static func fromValidStrings(_ strings: [String]) -> CapabilitySet {
        var set = CapabilitySet()
        for s in strings {
            if let profile = try? Profile.parse(s) {
                set.insert(profile)
            }
        }
        return set
    }

    /// Parses and inserts a profile string into the set.
    @discardableResult
    public mutating func insert(string: String) throws -> Bool {
        insert(try Profile.parse(string)).inserted
    }

    /// Returns `true` if the set contains the exact profile matching the string.
    public func contains(string: String) -> Bool {
        guard let profile = try? Profile.parse(string) else { return false }
        return contains(profile)
    }

    /// Returns `true` if the set contains any version of the profile with the given name.
    public func contains(name: String) -> Bool {
        contains { $0.name == name }
    }

    /// Returns the first `Profile` matching the given name, if present.
    public func getProfile(named name: String) -> Profile? {
        first { $0.name == name }
    }

    /// Returns the version number of the profile with the given name, if present.
    public func getVersion(named name: String) -> UInt32? {
        getProfile(named: name)?.version
    }

    /// Returns a sorted array of formatted profile strings in this set.
    public func toStringArray() -> [String] {
        sorted().map(\.description)
    }

    /// Returns an array of profiles in sorted order.
    public var sortedProfiles: [Profile] {
        sorted()
    }

    /// Computes the negotiated capability set between client-offered profiles and server requirements (§15).
    ///
    /// # Invariants & Matching Rules
    ///
    /// - **§4 Invariant 13**: Unknown required semantics must fail explicitly. If any profile in `serverRequired`
    ///   is not present in `clientOffered`, negotiation fails with `NegotiationError.unsatisfiedRequiredProfiles`.
    /// - **Optional Profiles**: If an optional profile in `serverOptional` is offered by the client, it is enabled
    ///   in the negotiated set. If not offered by the client, it is omitted without error.
    /// - **Unknown Client Profiles**: If the client offers profiles not requested or supported by the server,
    ///   they are ignored and omitted from the negotiated set.
    public static func negotiate(
        clientOffered: CapabilitySet,
        serverRequired: CapabilitySet,
        serverOptional: CapabilitySet
    ) throws -> CapabilitySet {
        let missingRequired = serverRequired.sortedProfiles.filter { !clientOffered.contains($0) }
        if !missingRequired.isEmpty {
            throw NegotiationError.unsatisfiedRequiredProfiles(missing: missingRequired)
        }

        var negotiated = serverRequired
        for opt in serverOptional where clientOffered.contains(opt) {
            negotiated.insert(opt)
        }
        return negotiated
    }

    /// Convenience instance method: negotiates this client capability set against server requirements.
    public func negotiateWith(
        serverRequired: CapabilitySet,
        serverOptional: CapabilitySet
    ) throws -> CapabilitySet {
        try CapabilitySet.negotiate(
            clientOffered: self,
            serverRequired: serverRequired,
            serverOptional: serverOptional
        )
    }
}

/// Server-side capability configuration specifying required and optional profiles (§15).
public struct ServerCapabilities: Equatable, Sendable {
    public var required: CapabilitySet
    public var optional: CapabilitySet

    public init(required: CapabilitySet = CapabilitySet(), optional: CapabilitySet = CapabilitySet()) {
        self.required = required
        self.optional = optional
    }

    /// Standard-only server capability specification requiring standard widgets v1.
    public static var standardWidgets: ServerCapabilities {
        ServerCapabilities(required: [Profile.standardWidgetsV1])
    }

    /// Computes the negotiated capability set for a connecting client's offered profiles (§15).
    public func negotiate(clientOffered: CapabilitySet) throws -> CapabilitySet {
        try CapabilitySet.negotiate(
            clientOffered: clientOffered,
            serverRequired: required,
            serverOptional: optional
        )
    }
}
