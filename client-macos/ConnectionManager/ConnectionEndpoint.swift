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
// PX-023 round 2: before any per-field rule, every field is refused if it still holds a NUL or any
// other control, format or otherwise invisible scalar. A NUL is the dangerous one: C APIs
// (`inet_pton`, `Process.run`'s file-system representation) read a string only up to it, so a
// check made through them sees a prefix while `ssh` would be handed the whole string, and
// `Process.run` raises an Objective-C exception that aborts the client.
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
    /// Every bound counts Unicode scalars, not `Character`s: one grapheme can carry any number of
    /// combining scalars, so a `Character` count bounds nothing (PX-023 round 3). The composite
    /// Host field holds at most `user@[host]:port`.
    static let maximumHostFieldScalars = maximumHostLength + maximumUserLength + 1 + 2 + 1 + maximumPortDigits
    /// Any field larger than this many UTF-8 bytes is refused before it is trimmed or scanned
    /// (`String.utf8.count` is constant time for native strings).
    static let maximumRawFieldBytes = 4096
    /// An error message repeats at most this many scalars of the input, then "…".
    static let maximumEchoedScalars = 32

    /// The destination `ssh` receives after `--`: a name, an IPv4 address, or an IPv6 address
    /// without its brackets.
    public let host: String
    public let port: UInt16?
    public let user: String

    /// A Connect-form field, as an error names it.
    public enum Field: String, Equatable, Sendable {
        case host = "Host"
        case user = "Remote user"
        case port = "Port"
        case name = "Name"
    }

    public enum ParseError: Error, Equatable, Sendable, CustomStringConvertible {
        case controlCharacter(Field, Unicode.Scalar)
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
            case .controlCharacter(let field, let scalar):
                let code = String(scalar.value, radix: 16, uppercase: true)
                return "\(field.rawValue) can’t contain a control or invisible character (U+\(String(repeating: "0", count: max(0, 4 - code.count)))\(code))."
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
                return "[\(Self.excerpt(literal))] is not a valid IPv6 address."
            case .emptyUser:
                return "Enter a remote user."
            case .userTooLong:
                return "Remote user must be at most \(ConnectionEndpoint.maximumUserLength) characters."
            case .invalidUserCharacter(let character):
                return "Remote user can’t contain \(Self.quoted(character)). Use letters, digits, “.”, “_” and “-”."
            case .userStartsWith(let character):
                return "Remote user can’t start with \(Self.quoted(character))."
            case .conflictingUser(let inHost, let inField):
                return "Host names user “\(Self.excerpt(inHost))” but Remote User is “\(Self.excerpt(inField))”. Enter the user once."
            case .invalidPort:
                return "Port must be a number from 1 through 65535."
            case .conflictingPort(let inHost, let inField):
                return "Host names port \(Self.excerpt(inHost)) but Port is \(Self.excerpt(inField)). Enter the port once."
            case .labelTooLong:
                return "Name must be at most \(ConnectionEndpoint.maximumLabelLength) characters."
            }
        }

        /// At most `maximumEchoedScalars` scalars of `text`, then "…": a message never repeats an
        /// unbounded input (one `Character` can hold millions of combining scalars).
        static func excerpt(_ text: String) -> String {
            let limit = ConnectionEndpoint.maximumEchoedScalars
            var scalars = String.UnicodeScalarView()
            var iterator = text.unicodeScalars.makeIterator()
            while scalars.count < limit, let scalar = iterator.next() {
                scalars.append(scalar)
            }
            return iterator.next() == nil ? String(scalars) : String(scalars) + "…"
        }

        private static func quoted(_ character: Character) -> String {
            let text = String(character)
            if character.isWhitespace || character.unicodeScalars.contains(where: { $0.value < 0x20 || $0.value == 0x7F }) {
                let scalars = excerpt(text).unicodeScalars
                    .map { $0 == "…" ? "…" : "U+" + String($0.value, radix: 16, uppercase: true) }
                return "a space or control character (\(scalars.joined(separator: " ")))"
            }
            return "“\(excerpt(text))”"
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
        // Size first, in constant time, so nothing below scans or copies an unbounded field.
        if hostField.utf8.count > maximumRawFieldBytes { return .failure(.hostTooLong) }
        if userField.utf8.count > maximumRawFieldBytes { return .failure(.userTooLong) }
        if portField.utf8.count > maximumRawFieldBytes { return .failure(.invalidPort) }

        var rest = trimmed(hostField)
        let userField = trimmed(userField)
        let portField = trimmed(portField)

        // Then, before any per-field rule: no NUL, control or invisible scalar anywhere.
        for (field, text) in [(Field.host, rest), (.user, userField), (.port, portField)] {
            if let scalar = firstDisallowedScalar(in: text) {
                return .failure(.controlCharacter(field, scalar))
            }
        }

        guard rest.isEmpty == false else { return .failure(.emptyHost) }
        guard rest.unicodeScalars.count <= maximumHostFieldScalars else {
            return .failure(.hostTooLong)
        }
        guard userField.unicodeScalars.count <= maximumUserLength else { return .failure(.userTooLong) }
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
    /// too, and a cold launch connects from it without passing through the form. The same rules as
    /// the form, port included (a stored `0` would otherwise become `-p 0`). Stored values are
    /// never trimmed: an entry the form could not have produced is refused.
    public static func validate(savedHost host: String, user: String, port: UInt16?) -> ParseError? {
        if host.utf8.count > maximumRawFieldBytes { return .hostTooLong }
        if user.utf8.count > maximumRawFieldBytes { return .userTooLong }
        if let scalar = firstDisallowedScalar(in: host) {
            return .controlCharacter(.host, scalar)
        }
        if let scalar = firstDisallowedScalar(in: user) {
            return .controlCharacter(.user, scalar)
        }
        let hostError = host.contains(":") ? validateIPv6(host) : validateName(host)
        if let error = hostError ?? validateUser(user) {
            return error
        }
        if let port, port == 0 {
            return .invalidPort
        }
        return nil
    }

    /// The Name field: local only, but held to the same no-invisible-character rule and its bound.
    static func validate(label: String) -> ParseError? {
        guard label.utf8.count <= maximumRawFieldBytes else { return .labelTooLong }
        let label = trimmed(label)
        if let scalar = firstDisallowedScalar(in: label) {
            return .controlCharacter(.name, scalar)
        }
        return label.unicodeScalars.count > maximumLabelLength ? .labelTooLong : nil
    }

    /// The first NUL, control (C0, DEL, C1), format (zero-width, bidirectional override),
    /// separator, surrogate, private-use or unassigned scalar in `text`.
    static func firstDisallowedScalar(in text: String) -> Unicode.Scalar? {
        text.unicodeScalars.first { scalar in
            switch scalar.properties.generalCategory {
            case .control, .format, .lineSeparator, .paragraphSeparator, .surrogate, .privateUse, .unassigned:
                return true
            default:
                return false
            }
        }
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

    /// Drops leading and trailing spaces (Unicode `Zs`), tabs and line feeds, as a paste brings
    /// them. Nothing invisible is trimmed: a zero-width or other format scalar, or any other
    /// control, stays and is refused by `firstDisallowedScalar(in:)`.
    /// (`CharacterSet.whitespaces` is not used: it also holds U+200B ZERO WIDTH SPACE.)
    static func trimmed(_ text: String) -> String {
        func trimmable(_ scalar: Unicode.Scalar) -> Bool {
            scalar == "\t" || scalar == "\n" || scalar == "\r"
                || scalar.properties.generalCategory == .spaceSeparator
        }
        let scalars = text.unicodeScalars
        guard let first = scalars.firstIndex(where: { !trimmable($0) }),
              let last = scalars.lastIndex(where: { !trimmable($0) }) else {
            return ""
        }
        return String(scalars[first...last])
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
        guard name.unicodeScalars.count <= maximumHostLength else { return .hostTooLong }
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
    ///
    /// The literal must reach `inet_pton` whole: one containing a NUL is refused outright, and the
    /// C string handed over must be exactly as long as the Swift string, so the check can never
    /// pass on a prefix (PX-023 round 2).
    static func validateIPv6(_ literal: String) -> ParseError? {
        guard literal.unicodeScalars.count <= maximumHostLength else { return .hostTooLong }
        guard literal.utf8.contains(0) == false else { return .invalidIPv6(literal) }
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
        let utf8Count = address.utf8.count
        let parsed = address.withCString { cString -> Int32 in
            guard strlen(cString) == utf8Count else { return 0 }
            return inet_pton(AF_INET6, cString, &storage)
        }
        return parsed == 1 ? nil : .invalidIPv6(literal)
    }

    /// A portable user name: ASCII letters, digits, `.`, `_` and `-`, not starting with `-` or `.`.
    private static func validateUser(_ user: String) -> ParseError? {
        guard user.isEmpty == false else { return .emptyUser }
        guard user.unicodeScalars.count <= maximumUserLength else { return .userTooLong }
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
              text.unicodeScalars.count <= maximumPortDigits,
              text.unicodeScalars.allSatisfy({ $0.value >= 0x30 && $0.value <= 0x39 }),
              let value = UInt32(text),
              (1...65535).contains(value) else {
            return nil
        }
        return UInt16(value)
    }
}
