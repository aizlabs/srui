//
// ConnectionEndpoint.swift
// ConnectionManager
//
// Validation of the endpoint a user types into the Connect form (§19.1, §25; D2 T38, PX-023).
//
// The endpoint becomes `ssh` argv elements (`-l <user>`, `-p <port>`, `-- <host>`), never a shell
// command line, and `SSHConfiguration.buildArguments()` already puts `--` before the host. This is
// the second, independent guard: what reaches `ssh` is a host name, an IPv4 address or an IPv6
// literal, a portable user name and a port from 1 through 65535 — nothing `ssh` could read as an
// option, a URL, a `%` token or a second destination, and nothing a shell would treat specially if
// one were ever involved. Rejected input is never trimmed into something acceptable: only leading
// and trailing whitespace is dropped.
//

import Foundation

/// A validated SSH destination: what `SSHConfiguration` receives for one connection.
public struct ConnectionEndpoint: Equatable, Sendable {
    /// RFC 1035 §2.3.4: a full domain name is at most 253 characters in text form.
    public static let maximumHostLength = 253
    /// Longer than the 32 bytes of Linux `useradd` and the 31 of older systems; still bounded.
    public static let maximumUserLength = 64
    /// The local display name of a saved connection. It never reaches `ssh`.
    public static let maximumLabelLength = 128
    /// Five digits hold every valid port; more digits are rejected before any conversion.
    static let maximumPortDigits = 5

    /// The destination `ssh` receives after `--`: a name, an IPv4 address, or an IPv6 address
    /// without its brackets.
    public let host: String
    public let port: UInt16?
    public let user: String

    public enum ParseError: Error, Equatable, Sendable, CustomStringConvertible {
        case emptyHost
        case hostTooLong
        case hostIsURL
        case invalidHostCharacter(Character)
        case hostStartsWith(Character)
        case bareIPv6
        case invalidIPv6(String)
        case emptyUser
        case userTooLong
        case invalidUserCharacter(Character)
        case userStartsWith(Character)
        case conflictingUser(inHost: String, inField: String)
        case invalidPort
        case conflictingPort(inHost: String, inField: String)
        case labelTooLong

        public var description: String {
            switch self {
            case .emptyHost:
                return "Enter a host."
            case .hostTooLong:
                return "Host must be at most \(ConnectionEndpoint.maximumHostLength) characters."
            case .hostIsURL:
                return "Enter a host name or address, not a URL."
            case .invalidHostCharacter(let character):
                return "Host can’t contain \(Self.quoted(character)). Use a host name, an IPv4 address, or an IPv6 address in brackets."
            case .hostStartsWith(let character):
                return "Host can’t start with \(Self.quoted(character))."
            case .bareIPv6:
                return "Put an IPv6 address in brackets, for example [::1] or [::1]:2222."
            case .invalidIPv6(let literal):
                return "[\(literal)] is not a valid IPv6 address."
            case .emptyUser:
                return "Enter a remote user."
            case .userTooLong:
                return "Remote user must be at most \(ConnectionEndpoint.maximumUserLength) characters."
            case .invalidUserCharacter(let character):
                return "Remote user can’t contain \(Self.quoted(character)). Use letters, digits, “.”, “_” and “-”."
            case .userStartsWith(let character):
                return "Remote user can’t start with \(Self.quoted(character))."
            case .conflictingUser(let inHost, let inField):
                return "Host names user “\(inHost)” but Remote User is “\(inField)”. Enter the user once."
            case .invalidPort:
                return "Port must be a number from 1 through 65535."
            case .conflictingPort(let inHost, let inField):
                return "Host names port \(inHost) but Port is \(inField). Enter the port once."
            case .labelTooLong:
                return "Name must be at most \(ConnectionEndpoint.maximumLabelLength) characters."
            }
        }

        private static func quoted(_ character: Character) -> String {
            if character.isWhitespace || character.unicodeScalars.contains(where: { $0.value < 0x20 || $0.value == 0x7F }) {
                let scalars = character.unicodeScalars.map { "U+" + String($0.value, radix: 16, uppercase: true) }
                return "a space or control character (\(scalars.joined(separator: " ")))"
            }
            return "“\(character)”"
        }
    }

    init(host: String, port: UInt16?, user: String) {
        self.host = host
        self.port = port
        self.user = user
    }

    /// Parses the three Connect-form fields.
    ///
    /// `hostField` may also carry the user and the port, as `ssh` users type them: `user@host`,
    /// `host:port`, `[v6]` and `user@[v6]:port`. A user or port given in both places must agree. A
    /// bare IPv6 address is refused, because `::1:22` cannot say where the address ends.
    public static func parse(
        host hostField: String,
        user userField: String,
        port portField: String
    ) -> Result<ConnectionEndpoint, ParseError> {
        var rest = trimmed(hostField)
        let userField = trimmed(userField)
        let portField = trimmed(portField)

        guard rest.isEmpty == false else { return .failure(.emptyHost) }
        guard rest.count <= maximumHostLength + maximumUserLength + 1 + 2 + 1 + maximumPortDigits else {
            return .failure(.hostTooLong)
        }
        if rest.contains("://") {
            return .failure(.hostIsURL)
        }

        var userInHost: String?
        if let at = rest.lastIndex(of: "@") {
            userInHost = String(rest[..<at])
            rest = String(rest[rest.index(after: at)...])
        }

        let hostPart: String
        var portInHost: String?
        if rest.hasPrefix("[") {
            guard let close = rest.firstIndex(of: "]") else {
                return .failure(.invalidIPv6(String(rest.dropFirst())))
            }
            let literal = String(rest[rest.index(after: rest.startIndex)..<close])
            if let error = validateIPv6(literal) { return .failure(error) }
            hostPart = literal
            let after = rest[rest.index(after: close)...]
            if after.isEmpty == false {
                guard after.first == ":" else {
                    return .failure(.invalidHostCharacter(after.first!))
                }
                portInHost = String(after.dropFirst())
            }
        } else {
            let colons = rest.filter { $0 == ":" }.count
            if colons > 1 {
                return .failure(.bareIPv6)
            }
            if let colon = rest.firstIndex(of: ":") {
                hostPart = String(rest[..<colon])
                portInHost = String(rest[rest.index(after: colon)...])
            } else {
                hostPart = rest
            }
            if let error = validateName(hostPart) { return .failure(error) }
        }

        let user: String
        switch (userInHost, userField.isEmpty) {
        case (nil, true):
            return .failure(.emptyUser)
        case (nil, false):
            user = userField
        case (let inHost?, true):
            user = inHost
        case (let inHost?, false):
            guard inHost == userField else {
                return .failure(.conflictingUser(inHost: inHost, inField: userField))
            }
            user = inHost
        }
        if let error = validateUser(user) { return .failure(error) }

        let portText: String?
        switch (portInHost, portField.isEmpty) {
        case (nil, true):
            portText = nil
        case (nil, false):
            portText = portField
        case (let inHost?, true):
            portText = inHost
        case (let inHost?, false):
            guard let hostPort = parsePort(inHost), let fieldPort = parsePort(portField) else {
                return .failure(.invalidPort)
            }
            guard hostPort == fieldPort else {
                return .failure(.conflictingPort(inHost: inHost, inField: portField))
            }
            portText = portField
        }
        var port: UInt16?
        if let portText {
            guard let parsed = parsePort(portText) else { return .failure(.invalidPort) }
            port = parsed
        }

        return .success(ConnectionEndpoint(host: hostPart, port: port, user: user))
    }

    /// Re-checks a saved entry before it reaches `ssh`: the saved-connections file is local input
    /// too, and a cold launch connects from it without passing through the form.
    public static func validate(savedHost host: String, user: String) -> ParseError? {
        let hostError = host.contains(":") ? validateIPv6(host) : validateName(host)
        return hostError ?? validateUser(user)
    }

    /// `user@host[:port]`, bracketing an IPv6 address so the port stays unambiguous.
    public var displayString: String {
        Self.displayString(host: host, port: port, user: user)
    }

    static func displayString(host: String, port: UInt16?, user: String) -> String {
        let shownHost = host.contains(":") ? "[\(host)]" : host
        let portSuffix = port.map { ":\($0)" } ?? ""
        return "\(user)@\(shownHost)\(portSuffix)"
    }

    static func trimmed(_ text: String) -> String {
        text.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private static func isASCIIAlphanumeric(_ scalar: Unicode.Scalar) -> Bool {
        switch scalar.value {
        case 0x30...0x39, 0x41...0x5A, 0x61...0x7A:
            return true
        default:
            return false
        }
    }

    /// A host name, an alias from the user's own `ssh_config`, or an IPv4 address: ASCII letters,
    /// digits, `-`, `.` and `_`, not starting with `-` or `.`.
    private static func validateName(_ name: String) -> ParseError? {
        guard name.isEmpty == false else { return .emptyHost }
        guard name.count <= maximumHostLength else { return .hostTooLong }
        for character in name {
            guard character.unicodeScalars.count == 1,
                  let scalar = character.unicodeScalars.first,
                  isASCIIAlphanumeric(scalar) || scalar == "-" || scalar == "." || scalar == "_" else {
                return .invalidHostCharacter(character)
            }
        }
        if let first = name.first, first == "-" || first == "." {
            return .hostStartsWith(first)
        }
        return nil
    }

    /// An IPv6 address `inet_pton` accepts, optionally with a `%zone` of ASCII letters, digits, `.`,
    /// `_` and `-` (an interface name such as `en0`).
    private static func validateIPv6(_ literal: String) -> ParseError? {
        guard literal.count <= maximumHostLength else { return .hostTooLong }
        var address = literal
        if let percent = literal.firstIndex(of: "%") {
            address = String(literal[..<percent])
            let zone = literal[literal.index(after: percent)...]
            guard zone.isEmpty == false,
                  zone.allSatisfy({ character in
                      character.unicodeScalars.count == 1
                          && character.unicodeScalars.first.map {
                              isASCIIAlphanumeric($0) || $0 == "." || $0 == "_" || $0 == "-"
                          } == true
                  }) else {
                return .invalidIPv6(literal)
            }
        }
        guard address.unicodeScalars.allSatisfy({ $0.isASCII }) else {
            return .invalidIPv6(literal)
        }
        var storage = in6_addr()
        let parsed = address.withCString { inet_pton(AF_INET6, $0, &storage) }
        return parsed == 1 ? nil : .invalidIPv6(literal)
    }

    /// A portable user name: ASCII letters, digits, `.`, `_` and `-`, not starting with `-` or `.`.
    private static func validateUser(_ user: String) -> ParseError? {
        guard user.isEmpty == false else { return .emptyUser }
        guard user.count <= maximumUserLength else { return .userTooLong }
        for character in user {
            guard character.unicodeScalars.count == 1,
                  let scalar = character.unicodeScalars.first,
                  isASCIIAlphanumeric(scalar) || scalar == "-" || scalar == "." || scalar == "_" else {
                return .invalidUserCharacter(character)
            }
        }
        if let first = user.first, first == "-" || first == "." {
            return .userStartsWith(first)
        }
        return nil
    }

    /// One to five ASCII digits naming 1 through 65535. `UInt16(_:)` alone would accept `+22`.
    private static func parsePort(_ text: String) -> UInt16? {
        guard text.isEmpty == false,
              text.count <= maximumPortDigits,
              text.unicodeScalars.allSatisfy({ $0.value >= 0x30 && $0.value <= 0x39 }),
              let value = UInt32(text),
              (1...65535).contains(value) else {
            return nil
        }
        return UInt16(value)
    }
}
