//
// Shortcut.swift
// SemanticModel
//
// Platform-neutral semantic keyboard shortcuts: standard property 31 `shortcut`
// (§4 inv. 13, §7.4, §7.6, §7.7, §22.5, §22.8, §22.9, §26).
// Platform-neutral: NEVER import AppKit or Cocoa in this file. The native key-equivalent mapping
// lives in RendererAppKit; this file holds only the grammar, admission, display text and the
// reserved-combination tables, which are pure data.
//
// Grammar (identical to `server-rust/semantic-tree/src/shortcut.rs`, same error categories):
//
//     shortcut = [ "primary+" ] [ "shift+" ] [ "alt+" ] key        ; at most 32 bytes
//     key      = "a".."z" | "0".."9" | "return" | "escape" | "tab" | "space" | "delete"
//              | "up" | "down" | "left" | "right" | "home" | "end" | "page_up" | "page_down"
//              | "f1".."f12"
//
// `primary` is Command on macOS and Control elsewhere. Every key except f1–f12 requires `primary`.
//

import Foundation

/// Maximum encoded length of a `shortcut` value in bytes (§26).
public let maxShortcutBytes = 32

/// A named, non-character key admitted by the grammar.
public enum ShortcutNamedKey: Hashable, Sendable {
    case `return`, escape, tab, space, delete
    case up, down, left, right, home, end, pageUp, pageDown
    /// Function key f1–f12.
    case function(Int)

    static let table: [(String, ShortcutNamedKey)] = [
        ("return", .return), ("escape", .escape), ("tab", .tab), ("space", .space),
        ("delete", .delete), ("up", .up), ("down", .down), ("left", .left), ("right", .right),
        ("home", .home), ("end", .end), ("page_up", .pageUp), ("page_down", .pageDown),
    ]

    /// Matches UTF-8 bytes exactly, never by Unicode canonical equivalence, as Rust does.
    static func parse(_ token: [UInt8]) -> ShortcutNamedKey? {
        if let entry = table.first(where: { Array($0.0.utf8) == token }) {
            return entry.1
        }
        guard token.first == UInt8(ascii: "f") else { return nil }
        let digits = token.dropFirst()
        guard !digits.isEmpty, digits.first != UInt8(ascii: "0"),
              digits.allSatisfy({ (UInt8(ascii: "0")...UInt8(ascii: "9")).contains($0) }),
              digits.count <= 2 else {
            return nil
        }
        let n = digits.reduce(0) { $0 * 10 + Int($1 - UInt8(ascii: "0")) }
        return (1...12).contains(n) ? .function(n) : nil
    }

    /// Canonical token.
    public var token: String {
        if case .function(let n) = self { return "f\(n)" }
        return Self.table.first(where: { $0.1 == self })!.0
    }
}

/// The key of a shortcut.
public enum ShortcutKey: Hashable, Sendable {
    /// A lowercase ASCII letter a–z or digit 0–9.
    case character(Character)
    case named(ShortcutNamedKey)

    /// Whether the key may be bound without `primary` (only f1–f12).
    public var isFunctionKey: Bool {
        if case .named(.function) = self { return true }
        return false
    }
}

/// Why a `shortcut` value is not in the canonical grammar.
public enum ShortcutError: Error, Equatable, Sendable, CustomStringConvertible {
    case notAString
    case empty
    case tooLong(length: Int)
    case unknownToken(String)
    case duplicateModifier(String)
    case misorderedModifier(String)
    case missingKey
    case requiresPrimary(String)

    /// Same wording as the Rust `ShortcutError` Display.
    public var description: String {
        switch self {
        case .notAString: return "a shortcut must be a string"
        case .empty: return "a shortcut must not be empty"
        case .tooLong(let length):
            return "a shortcut is at most \(maxShortcutBytes) bytes, got \(length)"
        case .unknownToken(let token):
            return "unknown shortcut token \"\(escapedShortcutToken(token))\""
        case .duplicateModifier(let m): return "modifier \"\(m)\" appears more than once"
        case .misorderedModifier(let m):
            return "modifier \"\(m)\" is out of the canonical primary, shift, alt order"
        case .missingKey: return "a shortcut must end in a key"
        case .requiresPrimary(let key):
            return "key \"\(escapedShortcutToken(key))\" requires the primary modifier; only f1-f12 may be bare"
        }
    }
}

/// Renders a token for an error message exactly as the Rust `escape_token` does: printable ASCII
/// as itself, `"` and `\` backslash-escaped, every other Unicode scalar as `\u{hex}`.
public func escapedShortcutToken(_ token: String) -> String {
    var out = ""
    for scalar in token.unicodeScalars {
        switch scalar.value {
        case 0x22: out += "\\\""
        case 0x5C: out += "\\\\"
        case 0x20...0x7E: out.unicodeScalars.append(scalar)
        default: out += "\\u{" + String(scalar.value, radix: 16) + "}"
        }
    }
    return out
}

/// What a shortcut does on an admitted node type.
public enum ShortcutBehavior: Equatable, Sendable {
    /// Button: ACTIVATE exactly as a click (§7.6).
    case activate
    /// Toggle: flip the value and send VALUE_CHANGED exactly as a click.
    case toggle
    /// TextInput, TextArea, List, Table, Tree, RichText: local focus only, no event (§22.5).
    case focus

    /// The behavior on `nodeType`, or `nil` when the type does not admit a shortcut.
    public init?(nodeType: TypeRef) {
        switch nodeType {
        case .button: self = .activate
        case .toggle: self = .toggle
        case .textInput, .textArea, .list, .table, .tree, .richText: self = .focus
        default: return nil
        }
    }
}

/// The host platform family whose conventions a shortcut is presented in.
public enum ShortcutPlatform: Equatable, Sendable {
    /// `primary` is Command; display uses the macOS modifier glyphs.
    case macOS
    /// `primary` is Control; display uses `Ctrl+Shift+Alt+Key`.
    case other

    /// The platform this process runs on.
    public static var current: ShortcutPlatform {
        #if os(macOS)
        return .macOS
        #else
        return .other
        #endif
    }
}

/// One entry of a platform's reserved-combination table.
public struct ReservedShortcut: Equatable, Sendable {
    /// The platform spelling, e.g. `Cmd-Q`.
    public let name: String
    /// The canonical value it reserves, or `nil` when the grammar cannot express it.
    public let canonical: String?
}

/// A validated canonical shortcut.
public struct Shortcut: Hashable, Sendable, CustomStringConvertible {
    public let primary: Bool
    public let shift: Bool
    public let alt: Bool
    public let key: ShortcutKey

    private static let modifiers = ["primary", "shift", "alt"]
    private static let modifierBytes = modifiers.map { Array($0.utf8) }

    /// Parses a value in the closed canonical grammar, failing explicitly otherwise (§4 inv. 13).
    ///
    /// Works on UTF-8 bytes and splits on the `+` byte, exactly as the Rust parser does, so a
    /// combining mark after `+` (which Swift would otherwise fold into a `+` grapheme cluster)
    /// yields the same tokens, error category and message in both languages.
    public init(parsing value: String) throws {
        let bytes = Array(value.utf8)
        if bytes.count > maxShortcutBytes { throw ShortcutError.tooLong(length: bytes.count) }
        if bytes.isEmpty { throw ShortcutError.empty }
        let tokens = bytes.split(separator: UInt8(ascii: "+"), omittingEmptySubsequences: false)
            .map { Array($0) }
        let keyToken = tokens[tokens.count - 1]
        // Splitting valid UTF-8 on an ASCII byte keeps every token valid UTF-8.
        func text(_ token: [UInt8]) -> String { String(decoding: token, as: UTF8.self) }
        var seen = [false, false, false]
        var lastRank: Int?
        for token in tokens.dropLast() {
            guard let rank = Self.modifierBytes.firstIndex(of: token) else {
                throw ShortcutError.unknownToken(text(token))
            }
            if seen[rank] { throw ShortcutError.duplicateModifier(Self.modifiers[rank]) }
            if let lastRank, rank < lastRank {
                throw ShortcutError.misorderedModifier(Self.modifiers[rank])
            }
            seen[rank] = true
            lastRank = rank
        }
        if let rank = Self.modifierBytes.firstIndex(of: keyToken) {
            if seen[rank] { throw ShortcutError.duplicateModifier(Self.modifiers[rank]) }
            throw ShortcutError.missingKey
        }
        guard let key = Self.parseKey(keyToken) else {
            throw ShortcutError.unknownToken(text(keyToken))
        }
        if !seen[0] && !key.isFunctionKey {
            throw ShortcutError.requiresPrimary(text(keyToken))
        }
        self.primary = seen[0]
        self.shift = seen[1]
        self.alt = seen[2]
        self.key = key
    }

    /// Parses a standard property value; a non-string is refused (§6.5).
    public init(value: Value) throws {
        guard let string = value.asString else { throw ShortcutError.notAString }
        try self.init(parsing: string)
    }

    private static func parseKey(_ token: [UInt8]) -> ShortcutKey? {
        if token.count == 1,
           (UInt8(ascii: "a")...UInt8(ascii: "z")).contains(token[0])
            || (UInt8(ascii: "0")...UInt8(ascii: "9")).contains(token[0]) {
            return .character(Character(UnicodeScalar(token[0])))
        }
        return ShortcutNamedKey.parse(token).map(ShortcutKey.named)
    }

    /// The canonical spelling; `Shortcut(parsing: s).canonical == s` for every valid `s`.
    public var canonical: String {
        var parts: [String] = []
        if primary { parts.append("primary") }
        if shift { parts.append("shift") }
        if alt { parts.append("alt") }
        switch key {
        case .character(let c): parts.append(String(c))
        case .named(let named): parts.append(named.token)
        }
        return parts.joined(separator: "+")
    }

    public var description: String { canonical }

    /// Human-readable shortcut in `platform`'s conventions, e.g. `⇧⌘R` on macOS and
    /// `Ctrl+Shift+R` elsewhere (§22.8, §22.9).
    public func displayString(for platform: ShortcutPlatform = .current) -> String {
        switch platform {
        case .macOS:
            // Apple HIG order: Control, Option, Shift, Command.
            var glyphs = ""
            if alt { glyphs += "⌥" }
            if shift { glyphs += "⇧" }
            if primary { glyphs += "⌘" }
            return glyphs + keyDisplay(macOS: true)
        case .other:
            var parts: [String] = []
            if primary { parts.append("Ctrl") }
            if shift { parts.append("Shift") }
            if alt { parts.append("Alt") }
            parts.append(keyDisplay(macOS: false))
            return parts.joined(separator: "+")
        }
    }

    private func keyDisplay(macOS: Bool) -> String {
        switch key {
        case .character(let c): return String(c).uppercased()
        case .named(let named):
            switch named {
            case .return: return macOS ? "↩" : "Enter"
            case .escape: return macOS ? "⎋" : "Esc"
            case .tab: return macOS ? "⇥" : "Tab"
            case .space: return "Space"
            case .delete: return macOS ? "⌫" : "Backspace"
            case .up: return macOS ? "↑" : "Up"
            case .down: return macOS ? "↓" : "Down"
            case .left: return macOS ? "←" : "Left"
            case .right: return macOS ? "→" : "Right"
            case .home: return macOS ? "↖" : "Home"
            case .end: return macOS ? "↘" : "End"
            case .pageUp: return macOS ? "⇞" : "Page Up"
            case .pageDown: return macOS ? "⇟" : "Page Down"
            case .function(let n): return "F\(n)"
            }
        }
    }

    /// The macOS reserved-combination table. A matching shortcut is never bound and yields a
    /// renderer diagnostic. Entries the grammar cannot express carry no canonical value.
    public static let macOSReserved: [ReservedShortcut] = [
        ReservedShortcut(name: "Cmd-Q", canonical: "primary+q"),
        ReservedShortcut(name: "Cmd-W", canonical: "primary+w"),
        ReservedShortcut(name: "Cmd-H", canonical: "primary+h"),
        ReservedShortcut(name: "Opt-Cmd-H", canonical: "primary+alt+h"),
        ReservedShortcut(name: "Cmd-M", canonical: "primary+m"),
        ReservedShortcut(name: "Cmd-Tab", canonical: "primary+tab"),
        ReservedShortcut(name: "Cmd-`", canonical: nil),
        ReservedShortcut(name: "Cmd-comma", canonical: nil),
        ReservedShortcut(name: "Cmd-C", canonical: "primary+c"),
        ReservedShortcut(name: "Cmd-V", canonical: "primary+v"),
        ReservedShortcut(name: "Cmd-X", canonical: "primary+x"),
        ReservedShortcut(name: "Cmd-A", canonical: "primary+a"),
        ReservedShortcut(name: "Cmd-Z", canonical: "primary+z"),
        ReservedShortcut(name: "Shift-Cmd-Z", canonical: "primary+shift+z"),
        ReservedShortcut(name: "Cmd-Space", canonical: "primary+space"),
        ReservedShortcut(name: "Ctrl-Cmd-F", canonical: nil),
        ReservedShortcut(name: "Cmd-?", canonical: nil),
    ]

    /// The reserved-table entry this shortcut matches on `platform`, if any.
    public func reservedEntry(on platform: ShortcutPlatform = .current) -> ReservedShortcut? {
        switch platform {
        case .macOS:
            let canonical = canonical
            return Self.macOSReserved.first { $0.canonical == canonical }
        case .other:
            return nil
        }
    }
}

/// Validates one `shortcut` property value against the grammar and the node type (§7.4, §26).
///
/// Shared by `SemanticStore` (apply) and the Protocol decoders (grammar only, via
/// `Shortcut(value:)`), so every path refuses the same values with the same reason.
func validateShortcutValue(nodeID: NodeId, nodeType: TypeRef, value: Value) throws {
    do {
        _ = try Shortcut(value: value)
    } catch let error as ShortcutError {
        throw StoreError.invalidShortcut(nodeID: nodeID, reason: error.description)
    }
    guard ShortcutBehavior(nodeType: nodeType) != nil else {
        throw StoreError.shortcutNotAdmitted(nodeID: nodeID, nodeType: nodeType)
    }
}
