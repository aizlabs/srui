//
// ConnectionEndpointTests.swift
// SRUITests
//
// PX-023: the Connect form's endpoint parsing, the `ssh` argv it becomes, and the host-key
// diagnostics (§19.1, §25; D2 T38). Deterministic: nothing here starts a process.
//

import Foundation
@testable import ConnectionManager
import Protocol
@testable import Session
import Testing
import TransportSSH

/// An attempt that must never exist: every factory call is a test failure.
private actor UnreachableAttempt: ConnectionAttempt {
    nonisolated let events: AsyncStream<ConnectionAttemptEvent>
    private let continuation: AsyncStream<ConnectionAttemptEvent>.Continuation

    init() {
        let pair = AsyncStream.makeStream(of: ConnectionAttemptEvent.self)
        events = pair.stream
        continuation = pair.continuation
    }

    func start() async throws {}

    func stop() async {
        continuation.finish()
    }
}

@MainActor
private final class FactoryProbe {
    private(set) var configurations: [SSHConfiguration] = []

    func make(_ request: ConnectionAttemptRequest) -> any ConnectionAttempt {
        configurations.append(request.configuration)
        return UnreachableAttempt()
    }
}

private func temporaryStore() -> (SavedConnectionStore, URL) {
    let directory = FileManager.default.temporaryDirectory
        .appendingPathComponent("srui-px023-\(UUID().uuidString)", isDirectory: true)
    return (SavedConnectionStore(url: directory.appendingPathComponent("connections.json")), directory)
}

/// The posture every connection's argv starts with (§19.1): fixed, whatever the endpoint.
private let posturePrefix = [
    "-T", "-x", "-a",
    "-o", "ClearAllForwardings=yes",
    "-o", "ExitOnForwardFailure=yes",
    "-o", "StrictHostKeyChecking=yes",
    "-o", "LogLevel=INFO",
    "-o", "UpdateHostKeys=no",
    "-o", "BatchMode=yes",
    "-o", "ConnectTimeout=30",
]

/// The argv the manager's one SSH configuration builds for an accepted draft.
private func argv(_ host: String, _ user: String, _ port: String = "") throws -> [String] {
    let draft = ConnectDraft(host: host, port: port, user: user)
    let connection = try #require(draft.savedConnection, "\(host) / \(user) / \(port) should be accepted")
    return ConnectionManager.sshConfiguration(for: connection).buildArguments()
}

private func parseError(_ host: String, _ user: String = "alice", _ port: String = "") -> ConnectionEndpoint.ParseError? {
    if case .failure(let error) = ConnectionEndpoint.parse(host: host, user: user, port: port) {
        return error
    }
    return nil
}

@Suite("Connect form endpoint parsing and argv (PX-023, §19.1, §25)", .timeLimit(.minutes(1)))
struct ConnectionEndpointTests {
    struct Accepted: Sendable, CustomTestStringConvertible {
        let host: String
        let user: String
        let port: String
        let argvTail: [String]
        var testDescription: String { "\(host) | \(user) | \(port)" }
    }

    /// Each accepted endpoint and the exact argv after the posture options. The destination is one
    /// element, after `--`, followed only by the fixed subsystem name.
    static let accepted: [Accepted] = [
        Accepted(host: "host.example", user: "alice", port: "",
                 argvTail: ["-l", "alice", "-s", "--", "host.example", "srui"]),
        Accepted(host: "alice@host.example", user: "", port: "",
                 argvTail: ["-l", "alice", "-s", "--", "host.example", "srui"]),
        Accepted(host: "alice@host.example", user: "alice", port: "",
                 argvTail: ["-l", "alice", "-s", "--", "host.example", "srui"]),
        Accepted(host: "host.example:2222", user: "alice", port: "",
                 argvTail: ["-p", "2222", "-l", "alice", "-s", "--", "host.example", "srui"]),
        Accepted(host: "host.example:2222", user: "alice", port: "2222",
                 argvTail: ["-p", "2222", "-l", "alice", "-s", "--", "host.example", "srui"]),
        Accepted(host: "host.example", user: "alice", port: "2222",
                 argvTail: ["-p", "2222", "-l", "alice", "-s", "--", "host.example", "srui"]),
        Accepted(host: "bob@host.example:22", user: "", port: "",
                 argvTail: ["-p", "22", "-l", "bob", "-s", "--", "host.example", "srui"]),
        Accepted(host: "[::1]", user: "alice", port: "",
                 argvTail: ["-l", "alice", "-s", "--", "::1", "srui"]),
        Accepted(host: "[::1]:2222", user: "alice", port: "",
                 argvTail: ["-p", "2222", "-l", "alice", "-s", "--", "::1", "srui"]),
        Accepted(host: "alice@[fe80::1%en0]:22", user: "", port: "",
                 argvTail: ["-p", "22", "-l", "alice", "-s", "--", "fe80::1%en0", "srui"]),
        Accepted(host: "[::ffff:127.0.0.1]", user: "alice", port: "",
                 argvTail: ["-l", "alice", "-s", "--", "::ffff:127.0.0.1", "srui"]),
        Accepted(host: "127.0.0.1", user: "alice", port: "1",
                 argvTail: ["-p", "1", "-l", "alice", "-s", "--", "127.0.0.1", "srui"]),
        Accepted(host: "127.0.0.1", user: "alice", port: "65535",
                 argvTail: ["-p", "65535", "-l", "alice", "-s", "--", "127.0.0.1", "srui"]),
        Accepted(host: "127.0.0.1", user: "alice", port: "00022",
                 argvTail: ["-p", "22", "-l", "alice", "-s", "--", "127.0.0.1", "srui"]),
        // Leading and trailing whitespace (a pasted value) is dropped; nothing else is.
        Accepted(host: "  host.example\t", user: " alice\n", port: " 2222 ",
                 argvTail: ["-p", "2222", "-l", "alice", "-s", "--", "host.example", "srui"]),
        // An alias from the user's own ssh_config, and the user names Linux accepts.
        Accepted(host: "build_box-2", user: "svc.deploy_1-a", port: "",
                 argvTail: ["-l", "svc.deploy_1-a", "-s", "--", "build_box-2", "srui"]),
    ]

    @Test("accepted endpoints become exactly this argv", arguments: accepted)
    func acceptedEndpointArgv(_ input: Accepted) throws {
        let arguments = try argv(input.host, input.user, input.port)
        #expect(arguments == posturePrefix + input.argvTail)
        // The destination is the only element after `--` but the subsystem.
        let separator = try #require(arguments.firstIndex(of: "--"))
        #expect(arguments.count == separator + 3)
        #expect(arguments.last == "srui")
    }

    struct Rejected: Sendable, CustomTestStringConvertible {
        let host: String
        let user: String
        let port: String
        let error: ConnectionEndpoint.ParseError
        var testDescription: String { "\(host.debugDescription) | \(user.debugDescription) | \(port.debugDescription)" }
    }

    static let rejected: [Rejected] = [
        // Empty and whitespace-only fields.
        Rejected(host: "", user: "alice", port: "", error: .emptyHost),
        Rejected(host: "   \t", user: "alice", port: "", error: .emptyHost),
        Rejected(host: ":2222", user: "alice", port: "", error: .emptyHost),
        Rejected(host: "alice@", user: "", port: "", error: .emptyHost),
        Rejected(host: "host.example", user: "", port: "", error: .emptyUser),
        Rejected(host: "host.example", user: "  ", port: "", error: .emptyUser),
        Rejected(host: "@host.example", user: "", port: "", error: .emptyUser),
        // Ports: 0, 65536, non-numeric, signed, spaced, too many digits, empty after a colon.
        Rejected(host: "host.example", user: "alice", port: "0", error: .invalidPort),
        Rejected(host: "host.example", user: "alice", port: "65536", error: .invalidPort),
        Rejected(host: "host.example", user: "alice", port: "99999", error: .invalidPort),
        Rejected(host: "host.example", user: "alice", port: "ssh", error: .invalidPort),
        Rejected(host: "host.example", user: "alice", port: "+22", error: .invalidPort),
        Rejected(host: "host.example", user: "alice", port: "-22", error: .invalidPort),
        Rejected(host: "host.example", user: "alice", port: "2 2", error: .invalidPort),
        Rejected(host: "host.example", user: "alice", port: "000022", error: .invalidPort),
        Rejected(host: "host.example", user: "alice", port: "２２", error: .invalidPort),
        Rejected(host: "host.example:", user: "alice", port: "", error: .invalidPort),
        Rejected(host: "host.example:0", user: "alice", port: "", error: .invalidPort),
        Rejected(host: "host.example:65536", user: "alice", port: "", error: .invalidPort),
        Rejected(host: "host.example:22;id", user: "alice", port: "", error: .invalidPort),
        // IPv6: bare, unterminated, invalid, trailing junk, bad zone.
        Rejected(host: "::1", user: "alice", port: "", error: .bareIPv6),
        Rejected(host: "fe80::1:22", user: "alice", port: "", error: .bareIPv6),
        Rejected(host: "[::1", user: "alice", port: "", error: .invalidIPv6("::1")),
        Rejected(host: "[not-v6]", user: "alice", port: "", error: .invalidIPv6("not-v6")),
        Rejected(host: "[127.0.0.1]", user: "alice", port: "", error: .invalidIPv6("127.0.0.1")),
        Rejected(host: "[]", user: "alice", port: "", error: .invalidIPv6("")),
        Rejected(host: "[::1]x", user: "alice", port: "", error: .invalidHostCharacter("x")),
        Rejected(host: "[::1]:", user: "alice", port: "", error: .invalidPort),
        Rejected(host: "[::1%]", user: "alice", port: "", error: .invalidIPv6("::1%")),
        Rejected(host: "[::1%en0;id]", user: "alice", port: "", error: .invalidIPv6("::1%en0;id")),
        // A URL or a second destination.
        Rejected(host: "ssh://alice@host.example:22", user: "", port: "", error: .hostIsURL),
        // Shell metacharacters and anything `ssh` could read as an option or a token.
        Rejected(host: "host.example;rm -rf ~", user: "alice", port: "", error: .invalidHostCharacter(";")),
        Rejected(host: "$(touch /tmp/px023)", user: "alice", port: "", error: .invalidHostCharacter("$")),
        Rejected(host: "`id`", user: "alice", port: "", error: .invalidHostCharacter("`")),
        Rejected(host: "host example", user: "alice", port: "", error: .invalidHostCharacter(" ")),
        Rejected(host: "host\nexample", user: "alice", port: "", error: .controlCharacter(.host, "\n")),
        Rejected(host: "host|cat", user: "alice", port: "", error: .invalidHostCharacter("|")),
        Rejected(host: "host&", user: "alice", port: "", error: .invalidHostCharacter("&")),
        Rejected(host: "host>out", user: "alice", port: "", error: .invalidHostCharacter(">")),
        Rejected(host: "host'x", user: "alice", port: "", error: .invalidHostCharacter("'")),
        Rejected(host: "host\"x", user: "alice", port: "", error: .invalidHostCharacter("\"")),
        Rejected(host: "host\\x", user: "alice", port: "", error: .invalidHostCharacter("\\")),
        Rejected(host: "host*", user: "alice", port: "", error: .invalidHostCharacter("*")),
        Rejected(host: "%h", user: "alice", port: "", error: .invalidHostCharacter("%")),
        Rejected(host: "h\u{043E}st", user: "alice", port: "", error: .invalidHostCharacter("\u{043E}")),
        Rejected(host: "-oProxyCommand=touch /tmp/px023", user: "alice", port: "", error: .invalidHostCharacter("=")),
        Rejected(host: "-oProxyCommand", user: "alice", port: "", error: .hostStartsWith("-")),
        Rejected(host: "-p", user: "alice", port: "", error: .hostStartsWith("-")),
        Rejected(host: ".host", user: "alice", port: "", error: .hostStartsWith(".")),
        // Users: an option, metacharacters, a second `@`.
        Rejected(host: "host.example", user: "-oProxyCommand", port: "", error: .userStartsWith("-")),
        Rejected(host: "host.example", user: "-l", port: "", error: .userStartsWith("-")),
        Rejected(host: "host.example", user: "alice;id", port: "", error: .invalidUserCharacter(";")),
        Rejected(host: "host.example", user: "$(id)", port: "", error: .invalidUserCharacter("$")),
        Rejected(host: "host.example", user: "`id`", port: "", error: .invalidUserCharacter("`")),
        Rejected(host: "host.example", user: "al ice", port: "", error: .invalidUserCharacter(" ")),
        Rejected(host: "host.example", user: "al@ice", port: "", error: .invalidUserCharacter("@")),
        Rejected(host: "a@b@host.example", user: "", port: "", error: .invalidUserCharacter("@")),
        Rejected(host: "-x@host.example", user: "", port: "", error: .userStartsWith("-")),
        // PX-023 round 2: NUL, C0, DEL, C1 and invisible format scalars, in every field, first. A
        // NUL inside brackets used to pass: `inet_pton` read only the prefix before it, and
        // `Process.run` then aborted the client on the whole string.
        Rejected(host: "[::\u{0};id -oProxyCommand=x]", user: "alice", port: "", error: .controlCharacter(.host, "\u{0}")),
        Rejected(host: "[::1\u{0} -F /tmp/x]", user: "alice", port: "", error: .controlCharacter(.host, "\u{0}")),
        Rejected(host: "alice@[fe80::1\u{0}$(id)]:22", user: "", port: "", error: .controlCharacter(.host, "\u{0}")),
        Rejected(host: "[::\u{0}1]", user: "alice", port: "", error: .controlCharacter(.host, "\u{0}")),
        Rejected(host: "[fe80::1%en0\u{0}x]", user: "alice", port: "", error: .controlCharacter(.host, "\u{0}")),
        Rejected(host: "host.example\u{0}", user: "alice", port: "", error: .controlCharacter(.host, "\u{0}")),
        Rejected(host: "\u{0}host.example", user: "alice", port: "", error: .controlCharacter(.host, "\u{0}")),
        Rejected(host: "host\u{1}", user: "alice", port: "", error: .controlCharacter(.host, "\u{1}")),
        Rejected(host: "host\u{1B}[31m", user: "alice", port: "", error: .controlCharacter(.host, "\u{1B}")),
        Rejected(host: "host\u{7F}", user: "alice", port: "", error: .controlCharacter(.host, "\u{7F}")),
        Rejected(host: "host\u{85}x", user: "alice", port: "", error: .controlCharacter(.host, "\u{85}")),
        Rejected(host: "host\u{9F}", user: "alice", port: "", error: .controlCharacter(.host, "\u{9F}")),
        Rejected(host: "host\u{200B}", user: "alice", port: "", error: .controlCharacter(.host, "\u{200B}")),
        Rejected(host: "host\u{202E}", user: "alice", port: "", error: .controlCharacter(.host, "\u{202E}")),
        Rejected(host: "host.example", user: "al\u{0}ice", port: "", error: .controlCharacter(.user, "\u{0}")),
        Rejected(host: "host.example", user: "alice\u{0}-oX", port: "", error: .controlCharacter(.user, "\u{0}")),
        Rejected(host: "host.example", user: "al\u{9B}ice", port: "", error: .controlCharacter(.user, "\u{9B}")),
        Rejected(host: "host.example", user: "alice", port: "22\u{0}", error: .controlCharacter(.port, "\u{0}")),
        Rejected(host: "host.example", user: "alice", port: "\u{0}22", error: .controlCharacter(.port, "\u{0}")),
        Rejected(host: "host.example", user: "alice", port: "2\u{FEFF}2", error: .controlCharacter(.port, "\u{FEFF}")),
        // The user or port named twice, differently.
        Rejected(host: "bob@host.example", user: "alice", port: "",
                 error: .conflictingUser(inHost: "bob", inField: "alice")),
        Rejected(host: "host.example:22", user: "alice", port: "2222",
                 error: .conflictingPort(inHost: "22", inField: "2222")),
    ]

    @Test("malformed endpoints are rejected with their reason", arguments: rejected)
    func rejectedEndpoint(_ input: Rejected) {
        #expect(parseError(input.host, input.user, input.port) == input.error)
        let draft = ConnectDraft(host: input.host, port: input.port, user: input.user)
        #expect(draft.savedConnection == nil)
        #expect(draft.validationMessage == input.error.description)
    }

    @Test("field-length bounds hold at and just past each bound")
    func fieldLengthBounds() throws {
        // A 253-character name made of 63-character labels, and one character more.
        let label = String(repeating: "a", count: 63)
        let host253 = [label, label, label, String(repeating: "b", count: 61)].joined(separator: ".")
        #expect(host253.count == 253)
        #expect(try argv(host253, "alice").contains(host253))
        #expect(parseError(host253 + "c") == .hostTooLong)
        #expect(parseError("alice@" + host253 + "c:22", "") == .hostTooLong)
        #expect(parseError(String(repeating: "a", count: 100_000)) == .hostTooLong)

        let user64 = String(repeating: "u", count: 64)
        #expect(try argv("host.example", user64).contains(user64))
        #expect(parseError("host.example", user64 + "u") == .userTooLong)
        #expect(parseError(user64 + "u@host.example", "") == .userTooLong)

        let name128 = String(repeating: "n", count: 128)
        var draft = ConnectDraft(label: name128, host: "host.example", user: "alice")
        #expect(draft.validationMessage == nil)
        #expect(draft.savedConnection?.label == name128)
        draft.label = name128 + "n"
        #expect(draft.validationMessage == ConnectionEndpoint.ParseError.labelTooLong.description)
        #expect(draft.savedConnection == nil)

        // Ports: 1 and 65535 accepted, 0 and 65536 refused, in either place.
        #expect(try argv("h", "alice", "1").contains("1"))
        #expect(try argv("h:65535", "alice").contains("65535"))
        #expect(parseError("h", "alice", "0") == .invalidPort)
        #expect(parseError("h:65536") == .invalidPort)
    }

    @Test("every argv element of an accepted endpoint reaches exec whole: no NUL anywhere")
    func acceptedArgvHasNoNUL() throws {
        for input in Self.accepted {
            for element in try argv(input.host, input.user, input.port) {
                #expect(element.utf8.contains(0) == false, "\(element.debugDescription)")
                // What `Process.run` hands to exec: the C string must be the whole element.
                #expect(element.withCString { strlen($0) } == element.utf8.count)
            }
        }
    }

    @Test("the IPv6 check never passes on a prefix of the literal")
    func ipv6CheckSeesWholeLiteral() {
        // Called directly, below the field-wide control check, so each layer is pinned on its own.
        for literal in ["::\u{0};id", "::1\u{0}", "::\u{0}1", "fe80::1%en0\u{0}x", "\u{0}::1"] {
            #expect(ConnectionEndpoint.validateIPv6(literal) == .invalidIPv6(literal), "\(literal.debugDescription)")
        }
        #expect(ConnectionEndpoint.validateIPv6("::1") == nil)
        #expect(ConnectionEndpoint.validateIPv6("fe80::1%en0") == nil)
    }

    @Test("the name refuses control and invisible characters too")
    func nameControlCharacters() {
        for (label, scalar) in [("Lab\u{0}", "\u{0}"), ("La\u{202E}b", "\u{202E}"), ("L\u{7F}ab", "\u{7F}")] as [(String, Unicode.Scalar)] {
            let draft = ConnectDraft(label: label, host: "host.example", user: "alice")
            #expect(draft.validationMessage == ConnectionEndpoint.ParseError.controlCharacter(.name, scalar).description)
            #expect(draft.savedConnection == nil)
        }
        #expect(ConnectionEndpoint.ParseError.controlCharacter(.host, "\u{0}").description
            == "Host can’t contain a control or invisible character (U+0000).")
    }

    /// A fixed-seed generator, so a failure names an input that reproduces.
    private struct SplitMix64: RandomNumberGenerator {
        var state: UInt64
        mutating func next() -> UInt64 {
            state &+= 0x9E37_79B9_7F4A_7C15
            var z = state
            z = (z ^ (z >> 30)) &* 0xBF58_476D_1CE4_E5B9
            z = (z ^ (z >> 27)) &* 0x94D0_49BB_1331_11EB
            return z ^ (z >> 31)
        }
    }

    /// The safety property, stated independently of the parser: what an accepted endpoint hands
    /// `ssh` is a printable-ASCII name or a whole IPv6 literal, a portable user and a valid port.
    private static func referenceSafe(_ endpoint: ConnectionEndpoint) -> Bool {
        let nameCharacters = Set("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._-")
        func portable(_ text: String, max: Int) -> Bool {
            !text.isEmpty && text.count <= max && text.allSatisfy(nameCharacters.contains)
                && text.first != "-" && text.first != "."
        }
        let hostSafe: Bool
        if endpoint.host.contains(":") {
            let address = endpoint.host.split(separator: "%", maxSplits: 1).first.map(String.init) ?? ""
            let v6Characters = Set("0123456789abcdefABCDEF:.")
            var storage = in6_addr()
            hostSafe = address.allSatisfy(v6Characters.contains)
                && address.withCString { inet_pton(AF_INET6, $0, &storage) } == 1
                && endpoint.host.allSatisfy { nameCharacters.contains($0) || $0 == ":" || $0 == "%" }
        } else {
            hostSafe = portable(endpoint.host, max: 253)
        }
        return hostSafe && portable(endpoint.user, max: 64) && endpoint.port != 0
    }

    @Test("randomized: no hostile input is accepted unsafely, and generated valid endpoints are accepted")
    func randomizedDifferential() throws {
        var random = SplitMix64(state: 0x5058_3032_3352_3200) // fixed seed
        let alphabet: [String] = ["a", "Z", "0", "9", ".", "-", "_", ":", "::", "[", "]", "@", "%", "1",
                                  "fe80", "::1", "en0", " ", "\t", "\u{0}", "\u{1}", "\u{7F}", "\u{85}",
                                  "\u{9F}", "\u{200B}", "\u{202E}", "\u{FEFF}", ";", "$(", ")", "`", "|", "&",
                                  "'", "\"", "\\", "/", "=", "-o", "é", "\u{043E}", "２", "+", "*"]
        var accepted = 0
        for _ in 0..<20_000 {
            func field(_ maxPieces: Int) -> String {
                (0..<Int.random(in: 0...maxPieces, using: &random))
                    .map { _ in alphabet.randomElement(using: &random)! }.joined()
            }
            let host = field(8), user = field(3), port = field(2)
            guard case .success(let endpoint) = ConnectionEndpoint.parse(host: host, user: user, port: port) else {
                continue
            }
            accepted += 1
            #expect(Self.referenceSafe(endpoint), "accepted unsafely: \(host.debugDescription) \(user.debugDescription) \(port.debugDescription)")
            let arguments = ConnectionManager.sshConfiguration(
                for: SavedConnection(label: "x", host: endpoint.host, port: endpoint.port, user: endpoint.user)
            ).buildArguments()
            #expect(arguments.allSatisfy { $0.utf8.contains(0) == false })
        }
        #expect(accepted > 0, "the generator must reach the accepting side too")

        // Fail-safe direction: generated valid endpoints are all accepted, decomposed as built.
        let labelCharacters = Array("abcdefghijklmnopqrstuvwxyz0123456789-")
        for _ in 0..<2_000 {
            let labels = (0..<Int.random(in: 1...4, using: &random)).map { _ in
                "h" + String((0..<Int.random(in: 0...10, using: &random)).map { _ in labelCharacters.randomElement(using: &random)! })
            }
            let host = labels.joined(separator: ".")
            let user = "u" + String((0..<Int.random(in: 0...8, using: &random)).map { _ in labelCharacters.randomElement(using: &random)! })
            let port = UInt16.random(in: 1...65535, using: &random)
            let v6 = Bool.random(using: &random)
            let hostText = v6 ? "[fe80::\(String(port, radix: 16))]" : host
            let result = ConnectionEndpoint.parse(host: "\(user)@\(hostText):\(port)", user: "", port: "")
            #expect(result == .success(ConnectionEndpoint(
                host: v6 ? "fe80::\(String(port, radix: 16))" : host, port: port, user: user)))
        }
    }

    @Test("error messages echo a bounded excerpt, at and past the cap")
    func errorEchoIsBounded() {
        let cap = ConnectionEndpoint.maximumEchoedScalars
        let atCap = String(repeating: "x", count: cap)
        #expect(ConnectionEndpoint.ParseError.excerpt(atCap) == atCap)
        #expect(ConnectionEndpoint.ParseError.excerpt(atCap + "y") == atCap + "…")
        #expect(ConnectionEndpoint.ParseError.excerpt("") == "")

        // One Character carrying many combining scalars is still bounded.
        let grapheme = Character("a" + String(repeating: "\u{301}", count: 10_000))
        let echoed = ConnectionEndpoint.ParseError.invalidHostCharacter(grapheme).description
        #expect(echoed.unicodeScalars.count < 200, "\(echoed.unicodeScalars.count) scalars")
        #expect(echoed.contains("…"))
        let longLiteral = String(repeating: "f", count: 10_000)
        #expect(ConnectionEndpoint.ParseError.invalidIPv6(longLiteral).description.unicodeScalars.count < 100)
        #expect(ConnectionEndpoint.ParseError.conflictingUser(inHost: longLiteral, inField: longLiteral)
            .description.unicodeScalars.count < 200)
        #expect(ConnectionEndpoint.ParseError.conflictingPort(inHost: longLiteral, inField: longLiteral)
            .description.unicodeScalars.count < 200)
    }

    @Test("bounds count Unicode scalars and raw bytes, so one huge grapheme cannot pass them")
    func scalarAndByteBounds() throws {
        let mark = "\u{301}"
        // Host: 253 scalars as one grapheme passes the length bound and fails the character rule
        // with a bounded echo; 254 scalars fail the length bound.
        let host253 = "a" + String(repeating: mark, count: 252)
        #expect(host253.count == 1 && host253.unicodeScalars.count == 253)
        if case .invalidHostCharacter(let character)? = parseError(host253) {
            #expect(ConnectionEndpoint.ParseError.invalidHostCharacter(character).description.unicodeScalars.count < 200)
        } else {
            Issue.record("expected invalidHostCharacter, got \(String(describing: parseError(host253)))")
        }
        #expect(parseError("a" + String(repeating: mark, count: 253)) == .hostTooLong)
        // User: 64 scalars reach the character rule, 65 do not.
        if case .invalidUserCharacter? = parseError("h", "u" + String(repeating: mark, count: 63)) {} else {
            Issue.record("expected invalidUserCharacter at 64 scalars")
        }
        #expect(parseError("h", "u" + String(repeating: mark, count: 64)) == .userTooLong)
        // Name: 128 scalars accepted (combining marks are fine in a local name), 129 refused.
        #expect(ConnectDraft(label: "n" + String(repeating: mark, count: 127), host: "h", user: "alice").validationMessage == nil)
        #expect(ConnectDraft(label: "n" + String(repeating: mark, count: 128), host: "h", user: "alice").validationMessage
            == ConnectionEndpoint.ParseError.labelTooLong.description)
        // Raw size: 4096 UTF-8 bytes are trimmed and parsed, 4097 are refused before any scan.
        let pad = ConnectionEndpoint.maximumRawFieldBytes - "alice".utf8.count
        #expect(parseError("h", "alice" + String(repeating: " ", count: pad)) == nil)
        #expect(parseError("h", "alice" + String(repeating: " ", count: pad + 1)) == .userTooLong)
        #expect(parseError("h", "alice", String(repeating: " ", count: 4097)) == .invalidPort)
        #expect(ConnectDraft(label: String(repeating: " ", count: 4097), host: "h", user: "alice").validationMessage
            == ConnectionEndpoint.ParseError.labelTooLong.description)
        // A million combining scalars in one grapheme: refused by size, with a short message.
        let huge = "a" + String(repeating: mark, count: 1_000_000)
        let error = try #require(parseError(huge))
        #expect(error == .hostTooLong)
        #expect(error.description.count < 100)
        #expect(ConnectionEndpoint.validate(savedHost: huge, user: "alice", port: nil) == .hostTooLong)
        #expect(ConnectionEndpoint.validate(savedHost: "h", user: huge, port: nil) == .userTooLong)
    }

    @Test("an empty name defaults to the endpoint, bracketing IPv6")
    func defaultLabel() throws {
        let plain = try #require(ConnectDraft(host: "host.example:2222", user: "alice").savedConnection)
        #expect(plain.label == "alice@host.example:2222")
        let v6 = try #require(ConnectDraft(host: "[::1]:2222", user: "alice").savedConnection)
        #expect(v6.host == "::1")
        #expect(v6.label == "alice@[::1]:2222")
        let named = try #require(ConnectDraft(label: "  Lab  ", host: "h", user: "alice").savedConnection)
        #expect(named.label == "Lab")
    }

    @Test("a rejected draft never reaches the SSH factory")
    @MainActor
    func rejectedDraftNeverStartsSSH() async throws {
        let (store, directory) = temporaryStore()
        defer { try? FileManager.default.removeItem(at: directory) }
        let probe = FactoryProbe()
        let manager = ConnectionManager(store: store, attemptFactory: { probe.make($0) })

        for input in Self.rejected {
            manager.dismissAlert()
            #expect(manager.connect(ConnectDraft(host: input.host, port: input.port, user: input.user)) == nil)
            #expect(manager.alert?.kind == .invalidInput)
            #expect(manager.alert?.message == input.error.description)
        }
        #expect(probe.configurations.isEmpty)
        #expect(manager.entries.isEmpty)
        await manager.shutdown()
    }

    @Test("a tampered saved entry is refused before it can become argv")
    @MainActor
    func tamperedSavedEntryIsRefused() async throws {
        let (store, directory) = temporaryStore()
        defer { try? FileManager.default.removeItem(at: directory) }
        // The saved-connections file is local input a cold launch reads without the form.
        let tampered = [
            SavedConnection(label: "option", host: "-oProxyCommand=touch /tmp/px023", user: "alice"),
            SavedConnection(label: "shell", host: "host;id", user: "alice"),
            SavedConnection(label: "user", host: "host.example", user: "-oProxyCommand=id"),
            SavedConnection(label: "v6", host: "::1;id", user: "alice"),
            // PX-023 round 2: a NUL the IPv6 check used to read past, and other invisible scalars.
            SavedConnection(label: "v6-nul", host: "::\u{0};id", user: "alice"),
            SavedConnection(label: "v6-nul-tail", host: "::1\u{0} -F /tmp/x", user: "alice"),
            SavedConnection(label: "user-nul", host: "host.example", user: "al\u{0}ice"),
            SavedConnection(label: "bidi", host: "host\u{202E}.example", user: "alice"),
            // Entries saved before PX-023 that the form no longer produces.
            SavedConnection(label: "legacy-user-at", host: "alice@host.example", user: "alice"),
            SavedConnection(label: "legacy-space", host: "host.example ", user: "alice"),
            // Port 0 used to reach `-p 0`.
            SavedConnection(label: "port-zero", host: "host.example", port: 0, user: "alice"),
        ]
        let good = SavedConnection(label: "good", host: "fe80::1%en0", port: 2222, user: "alice")
        try await store.save(tampered + [good])

        let probe = FactoryProbe()
        let manager = ConnectionManager(store: store, attemptFactory: { probe.make($0) })
        await manager.load()
        for entry in tampered {
            manager.dismissAlert()
            manager.connect(id: entry.id)
            #expect(manager.alert?.kind == .invalidInput, "\(entry.label)")
            #expect(manager.alert?.message.hasPrefix("This saved connection’s endpoint is invalid:") == true)
            #expect(manager.status(for: entry.id) == .unknown)
        }
        #expect(probe.configurations.isEmpty)

        manager.dismissAlert()
        manager.connect(id: good.id)
        let configuration = try #require(probe.configurations.first)
        #expect(configuration == ConnectionManager.sshConfiguration(for: good))
        #expect(configuration.buildArguments() == posturePrefix
            + ["-p", "2222", "-l", "alice", "-s", "--", "fe80::1%en0", "srui"])
        await manager.shutdown()
    }

    @Test("a saved port is checked at and past each bound")
    @MainActor
    func savedPortBounds() async throws {
        #expect(ConnectionEndpoint.validate(savedHost: "h", user: "alice", port: 0) == .invalidPort)
        #expect(ConnectionEndpoint.validate(savedHost: "h", user: "alice", port: 1) == nil)
        #expect(ConnectionEndpoint.validate(savedHost: "h", user: "alice", port: 65535) == nil)
        #expect(ConnectionEndpoint.validate(savedHost: "h", user: "alice", port: nil) == nil)

        let (store, directory) = temporaryStore()
        defer { try? FileManager.default.removeItem(at: directory) }
        let zero = SavedConnection(label: "zero", host: "h", port: 0, user: "alice")
        let one = SavedConnection(label: "one", host: "h", port: 1, user: "alice")
        let top = SavedConnection(label: "top", host: "h", port: 65535, user: "alice")
        try await store.save([zero, one, top])
        let probe = FactoryProbe()
        let manager = ConnectionManager(store: store, attemptFactory: { probe.make($0) })
        await manager.load()
        manager.connect(id: zero.id)
        #expect(manager.alert?.message == "This saved connection’s endpoint is invalid: Port must be a number from 1 through 65535. Remove it and add it again.")
        #expect(probe.configurations.isEmpty)
        manager.dismissAlert()
        manager.connect(id: one.id)
        manager.connect(id: top.id)
        #expect(probe.configurations.map { Array($0.buildArguments().suffix(8)) } == [
            ["-p", "1", "-l", "alice", "-s", "--", "h", "srui"],
            ["-p", "65535", "-l", "alice", "-s", "--", "h", "srui"],
        ])
        await manager.shutdown()

        // 65536 cannot be stored as a port at all: the file is refused on load, nothing connects.
        let (bad, badDirectory) = temporaryStore()
        defer { try? FileManager.default.removeItem(at: badDirectory) }
        try FileManager.default.createDirectory(at: bad.url.deletingLastPathComponent(), withIntermediateDirectories: true)
        try Data("""
        [{"id":"\(UUID().uuidString)","label":"over","host":"h","port":65536,"user":"alice","lastKnownRevision":0}]
        """.utf8).write(to: bad.url)
        let badProbe = FactoryProbe()
        let badManager = ConnectionManager(store: bad, attemptFactory: { badProbe.make($0) })
        await badManager.load()
        #expect(badManager.entries.isEmpty)
        #expect(badManager.alert?.kind == .persistenceFailed)
        #expect(badProbe.configurations.isEmpty)
        await badManager.shutdown()
    }

    @Test("the one SSH configuration keeps OpenSSH's strict, non-interactive trust policy")
    func strictTrustPolicy() throws {
        let configuration = ConnectionManager.sshConfiguration(
            for: SavedConnection(label: "x", host: "host.example", user: "alice"))
        #expect(configuration.strictHostKeyChecking == .yes)
        #expect(configuration.batchMode)
        #expect(configuration.knownHostsFile == nil)
        #expect(configuration.identityFile == nil)
        #expect(configuration.extraOptions.isEmpty)
        #expect(configuration.subsystem == "srui")
        #expect(configuration.sshBinaryPath == "/usr/bin/ssh")
    }
}

@Suite("Alert text comes from local facts only (PX-023 round 5, §19.1, §25)", .timeLimit(.minutes(1)))
struct LocalFactAlertTests {
    @MainActor
    private func refusalAlert(detail: String) async throws -> ConnectionAlert {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("srui-px023-spoof-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let pair = await PipeTransport.createPair()
        let manager = ConnectionManager(
            store: SavedConnectionStore(url: directory.appendingPathComponent("connections.json")),
            attemptFactory: { request in
                ConnectionManager.makeSessionControllerAttempt(request: request, transport: pair.client)
            }
        )
        let serverInbound = pair.server.receiveStream()
        _ = try #require(manager.connect(ConnectDraft(host: "trusted.example", user: "alice")))
        // The client's hello reaches the server; then the server refuses with `detail`.
        var iterator = serverInbound.makeAsyncIterator()
        _ = try await iterator.next()
        var refusal = SRUIServerHandshakeRefused()
        refusal.reason = .snapshotUndeliverable
        refusal.detail = detail
        var message = SRUIMessage()
        message.serverHandshakeRefused = refusal
        try await pair.server.send(data: try SRUIFraming.encodeFramed(message))
        try await AsyncTestSupport.observed("refusal alert") { manager.alert != nil }
        let alert = try #require(manager.alert)
        await manager.shutdown()
        await pair.server.close()
        return alert
    }

    @Test("a server refusal that imitates OpenSSH gets exactly what a benign refusal gets (SERVER HANDSHAKE_REFUSED)")
    @MainActor
    func serverRefusalSpoofMatchesBenignRefusal() async throws {
        let spoof = """
        @    WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!     @
        Host key for [trusted.example]:22 has changed and you have requested strict checking.
        Host key verification failed.
        """
        let benign = "snapshot too large"
        let spoofed = try await refusalAlert(detail: spoof)
        let plain = try await refusalAlert(detail: benign)
        #expect(spoofed.kind == .connectionFailed && plain.kind == .connectionFailed)
        #expect(spoofed.title == plain.title)
        #expect(spoofed.title == "Connection Failed")
        // Everything but the server's own words is identical.
        #expect(spoofed.message.replacingOccurrences(of: spoof, with: "<detail>")
            == plain.message.replacingOccurrences(of: benign, with: "<detail>"))
        #expect(spoofed.message.contains("ssh-keygen") == false)
    }

    @Test("the hint states only the local fact, with no trust-changing advice")
    func hintText() {
        let absent = KnownHostsHint(name: "[h.example]:2222", presence: .absent(checkedFiles: ["/k1", "/k2"]))
        // Facts only (round 7): no guidance sentence.
        #expect(absent.text == "[h.example]:2222 is not in your known_hosts files (/k1, /k2). SRUI never adds or changes host keys.")
        let none = KnownHostsHint(name: "h.example", presence: .absent(checkedFiles: []))
        #expect(none.text.hasPrefix("h.example is not in your known_hosts files (none of them exists yet)."))
        let present = KnownHostsHint(name: "h.example", presence: .present(file: "/Users/a/.ssh/known_hosts"))
        #expect(present.text == "A key for h.example is in /Users/a/.ssh/known_hosts. SRUI will not change it.")
        for hint in [absent, none, present] {
            #expect(hint.text.contains("ssh-keygen") == false)
            #expect(hint.text.contains("-R") == false)
            #expect(hint.text.lowercased().contains("remove") == false)
        }
        #expect(KnownHostsHint.knownHostsName(host: "h", port: 22) == "h")
        #expect(KnownHostsHint.knownHostsName(host: "h", port: 2222) == "[h]:2222")
        #expect(KnownHostsHint.knownHostsName(host: "::1", port: 23) == "[::1]:23")
    }

    @Test("the alert text: app text first, then ssh's stderr verbatim under the label")
    func messageFormat() {
        let hint = KnownHostsHint(name: "h", presence: .present(file: "/k"))
        let stderr = "No ED25519 host key is known for h and you have requested strict checking.\r\nHost key verification failed.\r\n"
        #expect(ConnectionManager.sshFailureMessage(endpoint: "alice@h", hint: hint, stderr: stderr)
            == "SRUI could not connect to alice@h.\n\nA key for h is in /k. SRUI will not change it.\n\n"
            + "ssh reported (this text can include messages from the server):\n" + stderr)
        #expect(ConnectionManager.sshFailureMessage(endpoint: "alice@h", hint: nil, stderr: "")
            == "SRUI could not connect to alice@h.\n\nssh reported (this text can include messages from the server):\n")
    }

    @Test("ssh -G output is read for the name and files, and any unrecoverable path abandons the lookup")
    func parseEffectiveConfiguration() {
        let parse = SSHKnownHostsLookup.parseEffectiveConfiguration
        #expect(parse("""
        hostname 127.0.0.1
        port 2222
        userknownhostsfile /a/known_hosts /a/known_hosts2
        globalknownhostsfile /etc/ssh/ssh_known_hosts
        """) == .init(name: "[127.0.0.1]:2222", files: ["/a/known_hosts", "/a/known_hosts2", "/etc/ssh/ssh_known_hosts"]))
        #expect(parse("hostname h\nport 22\nuserknownhostsfile /k\n") == .init(name: "h", files: ["/k"]))
        #expect(parse("hostname h\nport 2222\nhostkeyalias alias.example\nuserknownhostsfile /k\n")
            == .init(name: "alias.example", files: ["/k"]))
        #expect(parse("hostname h\nport 22\nhostkeyalias none\nuserknownhostsfile none\n") == .init(name: "h", files: []))
        // `ssh -G` does not quote: "/tmp/a b/kh" would split into "/tmp/a" and "b/kh".
        #expect(parse("hostname h\nport 22\nuserknownhostsfile /tmp/a b/kh\n") == nil)
        #expect(parse("port 22\n") == nil)
        #expect(parse("hostname h\nport 0\n") == nil)
        #expect(parse("hostname h\nport x\n") == nil)
    }
}

@Suite("Known-hosts lookup through ssh -G and ssh-keygen -F (PX-023 round 5)", .serialized, .timeLimit(.minutes(1)))
struct KnownHostsLookupTests {
    /// A real key line for `name`.
    private static func entry(_ name: String, key: String) throws -> String {
        let publicKey = try String(contentsOf: URL(fileURLWithPath: "\(key).pub"), encoding: .utf8)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return "\(name) \(publicKey)\n"
    }

    /// The production argv for 127.0.0.1, with this test's known_hosts and a global file that
    /// does not exist, so nothing outside the fixture is consulted.
    private static func configuration(port: UInt16?, knownHosts: String, directory: URL,
                                      extra: [String: String] = [:]) -> SSHConfiguration {
        var configuration = ConnectionManager.sshConfiguration(
            for: SavedConnection(label: "x", host: "127.0.0.1", port: port, user: "alice"))
        configuration.knownHostsFile = knownHosts
        configuration.extraOptions = ["GlobalKnownHostsFile": directory.appendingPathComponent("absent_global").path]
            .merging(extra) { $1 }
        return configuration
    }

    @Test("present, absent, port formatting, hashed entries, HostKeyAlias")
    func lookups() async throws {
        let directory = try TestFixtureDirectory.make(prefix: "px023-kh")
        defer { TestFixtureDirectory.release(directory) }
        let key = directory.appendingPathComponent("host_key").path
        try await BoundedProcessTestSupport.generateEd25519Key(at: key)
        let plain = directory.appendingPathComponent("plain_known_hosts").path
        try (try Self.entry("[127.0.0.1]:2222", key: key) + Self.entry("alias.example", key: key))
            .write(toFile: plain, atomically: true, encoding: .utf8)
        let port22 = directory.appendingPathComponent("port22_known_hosts").path
        try Self.entry("127.0.0.1", key: key).write(toFile: port22, atomically: true, encoding: .utf8)
        let hashed = directory.appendingPathComponent("hashed_known_hosts").path
        try Self.entry("[127.0.0.1]:2222", key: key).write(toFile: hashed, atomically: true, encoding: .utf8)
        let hashing = try await BoundedProcessTestSupport.run("/usr/bin/ssh-keygen", ["-H", "-f", hashed])
        #expect(hashing == 0)
        #expect(try String(contentsOfFile: hashed, encoding: .utf8).hasPrefix("|1|"), "entry is hashed")

        let lookup = SSHKnownHostsLookup()
        #expect(await lookup.hint(for: Self.configuration(port: 2222, knownHosts: plain, directory: directory))
            == KnownHostsHint(name: "[127.0.0.1]:2222", presence: .present(file: plain)))
        #expect(await lookup.hint(for: Self.configuration(port: 2200, knownHosts: plain, directory: directory))
            == KnownHostsHint(name: "[127.0.0.1]:2200", presence: .absent(checkedFiles: [plain])))
        // Port 22 is filed under the bare host, and [host]:2222 is not the bare host.
        #expect(await lookup.hint(for: Self.configuration(port: nil, knownHosts: port22, directory: directory))
            == KnownHostsHint(name: "127.0.0.1", presence: .present(file: port22)))
        #expect(await lookup.hint(for: Self.configuration(port: 2222, knownHosts: port22, directory: directory))
            == KnownHostsHint(name: "[127.0.0.1]:2222", presence: .absent(checkedFiles: [port22])))
        #expect(await lookup.hint(for: Self.configuration(port: 2222, knownHosts: hashed, directory: directory))
            == KnownHostsHint(name: "[127.0.0.1]:2222", presence: .present(file: hashed)))
        #expect(await lookup.hint(for: Self.configuration(
            port: 2200, knownHosts: plain, directory: directory, extra: ["HostKeyAlias": "alias.example"]))
            == KnownHostsHint(name: "alias.example", presence: .present(file: plain)))
        // A known_hosts file that does not exist is not listed as checked.
        let missing = directory.appendingPathComponent("missing").path
        #expect(await lookup.hint(for: Self.configuration(port: 2222, knownHosts: missing, directory: directory))
            == KnownHostsHint(name: "[127.0.0.1]:2222", presence: .absent(checkedFiles: [])))
    }

    @Test("a failed or slow lookup gives no hint at all")
    func failures() async throws {
        let directory = try TestFixtureDirectory.make(prefix: "px023-khf")
        defer { TestFixtureDirectory.release(directory) }
        let knownHosts = directory.appendingPathComponent("known_hosts").path
        try Data().write(to: URL(fileURLWithPath: knownHosts))
        var configuration = Self.configuration(port: 2222, knownHosts: knownHosts, directory: directory)

        func lookup(_ lookup: SSHKnownHostsLookup, _ configuration: SSHConfiguration) async -> KnownHostsHint?? {
            await TestDeadline.race(.seconds(20)) { await lookup.hint(for: configuration) }
        }

        configuration.sshBinaryPath = "/nonexistent/ssh"
        #expect(await lookup(SSHKnownHostsLookup(), configuration) == .some(nil), "ssh cannot be launched")
        configuration.sshBinaryPath = "/usr/bin/false"
        #expect(await lookup(SSHKnownHostsLookup(), configuration) == .some(nil), "ssh -G fails")
        // `yes` writes without end: the output cap stops it long before the deadline.
        configuration.sshBinaryPath = "/usr/bin/yes"
        let clock = ContinuousClock()
        let started = clock.now
        #expect(await lookup(SSHKnownHostsLookup(timeout: .seconds(5)), configuration) == .some(nil), "output cap")
        #expect(clock.now - started < .seconds(3), "the cap, not the 5 s deadline, ended it")

        configuration.sshBinaryPath = "/usr/bin/ssh"
        #expect(await lookup(SSHKnownHostsLookup(sshKeygenPath: "/nonexistent/ssh-keygen"), configuration) == .some(nil),
                "ssh-keygen cannot be launched")
    }

    /// A stub that records its PID, optionally ignores SIGTERM, and then sleeps as that same PID.
    private static func hungStub(ignoringSIGTERM: Bool, pidFile: String) async throws -> String {
        let stub = try await SSHTestSupport.assessedStub(for: "KnownHostsLookupTests")
        try """
        echo $$ > "\(pidFile)"
        \(ignoringSIGTERM ? "trap '' TERM" : ":")
        exec /bin/sleep 30
        """.write(to: URL(fileURLWithPath: stub.behavior), atomically: true, encoding: .utf8)
        return stub.path
    }

    private static func waitUntilGone(pidFile: String) async throws -> pid_t {
        let text = try String(contentsOfFile: pidFile, encoding: .utf8).trimmingCharacters(in: .whitespacesAndNewlines)
        let pid = try #require(pid_t(text))
        try await AsyncTestSupport.eventuallyAsync(timeout: .seconds(5), description: "PID \(pid) gone") {
            kill(pid, 0) == -1 && errno == ESRCH
        }
        return pid
    }

    @Test("a hung ssh -G ends at the deadline, and the test cannot hang if the deadline is missing")
    func hungSSHEndsAtDeadline() async throws {
        let directory = try TestFixtureDirectory.make(prefix: "px023-khh")
        defer { TestFixtureDirectory.release(directory) }
        let knownHosts = directory.appendingPathComponent("known_hosts").path
        try Data().write(to: URL(fileURLWithPath: knownHosts))
        let pidFile = directory.appendingPathComponent("pid").path
        var configuration = Self.configuration(port: 2222, knownHosts: knownHosts, directory: directory)
        configuration.sshBinaryPath = try await Self.hungStub(ignoringSIGTERM: false, pidFile: pidFile)

        let clock = ContinuousClock()
        let started = clock.now
        let stubbed = configuration
        let outcome = await TestDeadline.race(.seconds(15)) {
            await SSHKnownHostsLookup(timeout: .milliseconds(300)).hint(for: stubbed)
        }
        #expect(outcome != nil, "the production deadline never fired")
        #expect(outcome == .some(nil))
        #expect(clock.now - started < .seconds(10))
        _ = try await Self.waitUntilGone(pidFile: pidFile)
    }

    @Test("a hung ssh-keygen that ignores SIGTERM is SIGKILLed: its exact PID is gone")
    func hungKeygenIsKilled() async throws {
        let directory = try TestFixtureDirectory.make(prefix: "px023-khk")
        defer { TestFixtureDirectory.release(directory) }
        let knownHosts = directory.appendingPathComponent("known_hosts").path
        try Data().write(to: URL(fileURLWithPath: knownHosts))
        let pidFile = directory.appendingPathComponent("pid").path
        let keygen = try await Self.hungStub(ignoringSIGTERM: true, pidFile: pidFile)
        let configuration = Self.configuration(port: 2222, knownHosts: knownHosts, directory: directory)

        let outcome = await TestDeadline.race(.seconds(15)) {
            await SSHKnownHostsLookup(sshKeygenPath: keygen, timeout: .milliseconds(300)).hint(for: configuration)
        }
        #expect(outcome == .some(nil), "no hint when ssh-keygen times out")
        // SIGTERM is ignored; only the SIGKILL after the grace period ends it.
        _ = try await Self.waitUntilGone(pidFile: pidFile)
    }

    @Test("thousands of matching entries still answer present, quickly", arguments: [500, 3000])
    func manyMatchingEntries(_ count: Int) async throws {
        let directory = try TestFixtureDirectory.make(prefix: "px023-khn")
        defer { TestFixtureDirectory.release(directory) }
        let key = directory.appendingPathComponent("host_key").path
        try await BoundedProcessTestSupport.generateEd25519Key(at: key)
        let line = try Self.entry("[127.0.0.1]:2222", key: key)
        let knownHosts = directory.appendingPathComponent("known_hosts").path
        // Past one 64 KiB pipe buffer of `ssh-keygen -F` output at 500 entries, far past at 3000.
        try String(repeating: line, count: count).write(toFile: knownHosts, atomically: true, encoding: .utf8)
        #expect(line.utf8.count * count > 64 * 1024)
        let configuration = Self.configuration(port: 2222, knownHosts: knownHosts, directory: directory)

        let clock = ContinuousClock()
        let started = clock.now
        let outcome = await TestDeadline.race(.seconds(15)) {
            await SSHKnownHostsLookup().hint(for: configuration)
        }
        #expect(outcome == .some(KnownHostsHint(name: "[127.0.0.1]:2222", presence: .present(file: knownHosts))))
        #expect(clock.now - started < .seconds(3), "answered well before the 5 s deadline")
    }
}

@Suite("Bounded child processes: drain, cap, deadline, SIGKILL (PX-023 round 6)", .serialized, .timeLimit(.minutes(1)))
struct BoundedProcessTests {
    private func run(_ path: String, _ arguments: [String], timeout: Duration = .seconds(5)) async -> BoundedProcess.Result?? {
        await TestDeadline.race(.seconds(20)) {
            await BoundedProcess.run(path, arguments, timeout: timeout)
        }
    }

    @Test("output past one pipe buffer is drained while the child runs")
    func drainsPastPipeBuffer() async {
        let clock = ContinuousClock()
        let started = clock.now
        let outcome = await run("/usr/bin/head", ["-c", "102400", "/dev/zero"])
        #expect(outcome??.status == 0)
        #expect(outcome??.output.count == 102_400)
        #expect(clock.now - started < .seconds(3), "not held until the deadline")
    }

    @Test("output exactly at the cap is kept; one byte more is refused at once")
    func cap() async {
        let cap = BoundedProcess.maximumOutputBytes
        let at = await run("/usr/bin/head", ["-c", "\(cap)", "/dev/zero"])
        #expect(at??.status == 0)
        #expect(at??.output.count == cap)

        let clock = ContinuousClock()
        let started = clock.now
        let past = await run("/usr/bin/head", ["-c", "\(cap + 1)", "/dev/zero"])
        #expect(past == .some(nil), "past the cap there is no result")
        #expect(clock.now - started < .seconds(3), "the cap, not the deadline, ended it")
    }
}

/// Records every signal `BoundedProcess` sends, then delivers it.
final class SignalRecorder: @unchecked Sendable {
    private let lock = NSLock()
    private var recorded: [Int32] = []

    var signals: [Int32] {
        lock.lock()
        defer { lock.unlock() }
        return recorded
    }

    var signal: BoundedProcess.Signal {
        { [self] pid, number in
            lock.lock()
            recorded.append(number)
            lock.unlock()
            return kill(pid, number)
        }
    }
}

/// Weak references to the objects a run launched, to check they are released.
final class LaunchedObjects: @unchecked Sendable {
    private let lock = NSLock()
    private var processes: [() -> AnyObject?] = []

    func add(_ process: Process, _ pipe: Pipe?) {
        weak var weakProcess = process
        weak var weakPipe = pipe
        lock.lock()
        processes.append { weakProcess }
        if pipe != nil { processes.append { weakPipe } }
        lock.unlock()
    }

    var liveCount: Int {
        lock.lock()
        defer { lock.unlock() }
        return processes.filter { $0() != nil }.count
    }

    var count: Int {
        lock.lock()
        defer { lock.unlock() }
        return processes.count
    }
}

@Suite("Bounded child processes: signals, drain ordering, release (PX-023 round 7)", .serialized, .timeLimit(.minutes(2)))
struct BoundedProcessLifecycleTests {
    @Test("a child that exits on SIGTERM receives exactly [SIGTERM]")
    func exitOnSIGTERMGetsNoSIGKILL() async throws {
        let recorder = SignalRecorder()
        let signal = recorder.signal
        let outcome = await TestDeadline.race(.seconds(20)) {
            await BoundedProcess.run(
                "/bin/sh", ["-c", "trap 'kill $!; exit 0' TERM; /bin/sleep 30 & wait"],
                timeout: .milliseconds(300), killGrace: .milliseconds(300), signal: signal)
        }
        #expect(outcome == .some(nil), "the deadline ended the run")
        // Past the grace period: a SIGKILL, if one were coming, would have been sent by now.
        try await Task.sleep(for: .milliseconds(900))
        #expect(recorder.signals == [SIGTERM])
    }

    @Test("a child that ignores SIGTERM receives [SIGTERM, SIGKILL]")
    func trappedSIGTERMGetsSIGKILL() async throws {
        let recorder = SignalRecorder()
        let signal = recorder.signal
        let outcome = await TestDeadline.race(.seconds(20)) {
            await BoundedProcess.run(
                "/bin/sh", ["-c", "trap '' TERM; exec /bin/sleep 30"],
                timeout: .milliseconds(300), killGrace: .milliseconds(300), signal: signal)
        }
        #expect(outcome == .some(nil))
        try await AsyncTestSupport.eventuallyAsync(timeout: .seconds(5), description: "SIGKILL sent") {
            recorder.signals.count >= 2
        }
        try await Task.sleep(for: .milliseconds(300))
        #expect(recorder.signals == [SIGTERM, SIGKILL])
    }

    @Test("no signal at all once the exit is recorded, even if the deadline fires later")
    func noSignalAfterExit() async throws {
        let recorder = SignalRecorder()
        let signal = recorder.signal
        // The shell exits at once; its background sleep keeps stdout open, so the drain cannot end
        // and the deadline fires with the exit already recorded.
        let outcome = await TestDeadline.race(.seconds(20)) {
            await BoundedProcess.run(
                "/bin/sh", ["-c", "/bin/sleep 2 & exit 0"],
                timeout: .milliseconds(300), killGrace: .milliseconds(200), signal: signal)
        }
        #expect(outcome == .some(nil), "no complete drain, no result")
        try await Task.sleep(for: .milliseconds(700))
        #expect(recorder.signals == [])
    }

    @Test("the result waits for the end of the drain and is byte-exact", arguments: [1, 65_537, 131_072])
    func resultWaitsForDrain(_ bytes: Int) async throws {
        // Every read, the EOF read included, is held back 150 ms, so the child has long exited
        // before the drain ends. A result delivered at the exit would be short.
        let hooks = BoundedProcess.Hooks(beforeRead: { usleep(150_000) })
        let outcome = await TestDeadline.race(.seconds(20)) {
            await BoundedProcess.run("/usr/bin/head", ["-c", "\(bytes)", "/dev/zero"],
                                     timeout: .seconds(10), hooks: hooks)
        }
        let result = try #require(outcome ?? nil, "a result")
        #expect(result.status == 0)
        #expect(result.output.count == bytes)
        #expect(result.output.allSatisfy { $0 == 0 })
    }

    @Test("the result for output past the cap is never delivered, however the drain is timed")
    func overCapNeverDelivered() async throws {
        let hooks = BoundedProcess.Hooks(beforeRead: { usleep(50_000) })
        let outcome = await TestDeadline.race(.seconds(20)) {
            await BoundedProcess.run("/usr/bin/head", ["-c", "\(BoundedProcess.maximumOutputBytes + 1)", "/dev/zero"],
                                     timeout: .seconds(10), hooks: hooks)
        }
        #expect(outcome == .some(nil))
    }

    private static func openDescriptorCount() -> Int {
        (try? FileManager.default.contentsOfDirectory(atPath: "/dev/fd").count) ?? 0
    }

    @Test("a finished run releases its Process, Pipe and descriptors at once, not at the deadline")
    func releasedPromptly() async throws {
        let objects = LaunchedObjects()
        let baseline = Self.openDescriptorCount()
        let runs = 40
        for _ in 0..<runs {
            let hooks = BoundedProcess.Hooks(launched: { objects.add($0, $1) })
            let outcome = await TestDeadline.race(.seconds(20)) {
                // A 60 s deadline: if its task kept the run alive, everything would stay held.
                await BoundedProcess.run("/usr/bin/true", [], timeout: .seconds(60), hooks: hooks)
            }
            #expect(outcome??.status == 0)
        }
        #expect(objects.count == runs * 2)
        try await AsyncTestSupport.eventuallyAsync(timeout: .seconds(5), description: "launched objects released") {
            objects.liveCount == 0
        }
        // Tolerance for descriptors other suites open meanwhile; a held run keeps at least one
        // pipe descriptor each, so a leak shows as 40 or more.
        try await AsyncTestSupport.eventuallyAsync(timeout: .seconds(5), description: "descriptors back to baseline") {
            Self.openDescriptorCount() < baseline + runs / 2
        }
    }
}

/// Races `operation` against a test-side deadline without awaiting the loser: if the code under
/// test never resumes (a missing production deadline), the test fails in `limit` instead of
/// hanging the runner (a `.timeLimit` cannot cancel a continuation that is never resumed).
enum TestDeadline {
    private final class Once: @unchecked Sendable {
        private let lock = NSLock()
        private var done = false
        func claim() -> Bool {
            lock.lock()
            defer { lock.unlock() }
            if done { return false }
            done = true
            return true
        }
    }

    /// `.some(value)` when `operation` finished first, `nil` when the deadline did.
    static func race<T: Sendable>(_ limit: Duration, _ operation: @escaping @Sendable () async -> T) async -> T? {
        await withCheckedContinuation { (continuation: CheckedContinuation<T?, Never>) in
            let once = Once()
            Task.detached {
                let value = await operation()
                if once.claim() { continuation.resume(returning: .some(value)) }
            }
            Task.detached {
                try? await Task.sleep(for: limit)
                if once.claim() { continuation.resume(returning: nil) }
            }
        }
    }
}

/// Runs a fixture command to completion without blocking a thread on its exit.
enum BoundedProcessTestSupport {
    /// `SSHTestSupport.generateEd25519Key` without its `waitUntilExit()`, which services the
    /// calling thread's run loop: on the main actor that nests the executor's run loop, and on a
    /// cooperative thread it holds a thread every test shares (PX-023 round 7).
    static func generateEd25519Key(at path: String) async throws {
        let status = try await run("/usr/bin/ssh-keygen", ["-q", "-t", "ed25519", "-N", "", "-f", path])
        guard status == 0 else { throw SSHTestSupportError.keyGenerationFailed(path) }
    }

    static func run(_ path: String, _ arguments: [String]) async throws -> Int32 {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: path)
        process.arguments = arguments
        process.standardInput = FileHandle.nullDevice
        process.standardOutput = FileHandle.nullDevice
        process.standardError = FileHandle.nullDevice
        return try await withCheckedThrowingContinuation { continuation in
            process.terminationHandler = { continuation.resume(returning: $0.terminationStatus) }
            do { try process.run() } catch {
                process.terminationHandler = nil
                continuation.resume(throwing: error)
            }
        }
    }
}
