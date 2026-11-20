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
    "-o", "LogLevel=ERROR",
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

@Suite("Host-key rejection diagnostics (PX-023, §19.1, §25)", .timeLimit(.minutes(1)))
struct HostKeyRejectionTests {
    // OpenSSH's own text (sshconnect.c), as `SSHTransport` reports a child exit; the live suite
    // records the same text from OpenSSH_10.2p1.
    // `ssh`'s raw stderr, as `SSHTransport.localFailureBeforeStream()` reports it.
    static let unknownMessage = "No ED25519 host key is known for [127.0.0.1]:2222 and you have requested strict checking.\n"
        + "Host key verification failed."
    static let changedMessage = "@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@\n"
        + "@    WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!     @\n"
        + "@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@\n"
        + "Host key for [127.0.0.1]:2222 has changed and you have requested strict checking.\n"
        + "Host key verification failed."

    @Test("OpenSSH's failures are classified, and other failures are not host-key failures")
    func classification() {
        #expect(HostKeyRejection(sshMessage: Self.unknownMessage) == .unknown)
        #expect(HostKeyRejection(sshMessage: Self.changedMessage) == .changed)
        #expect(HostKeyRejection(sshMessage: Self.unknownMessage.replacingOccurrences(of: "\n", with: "\r\n") + "\r\n") == .unknown)
        #expect(HostKeyRejection(sshMessage: """
        @@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
        @       WARNING: REVOKED HOST KEY DETECTED!               @
        @@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@
        The ED25519 host key for host.example is marked as revoked.
        This could mean that a stolen key is being used to
        impersonate this host.
        Host key verification failed.
        """) == .revoked)
        #expect(HostKeyRejection(sshMessage: "Host key verification failed.") == .unverified)
        #expect(HostKeyRejection(sshMessage: "@ WARNING: POSSIBLE DNS SPOOFING DETECTED! @") == nil)

        // Structural (PX-023 round 3): the verdict must be ssh's last line. Server-authored text
        // logged before it (a pre-auth banner, a disconnect reason) never ends ssh's stderr.
        #expect(HostKeyRejection(sshMessage: Self.changedMessage + "\nalice@127.0.0.1: Permission denied (publickey).") == nil)
        #expect(HostKeyRejection(sshMessage: """
        Received disconnect from 127.0.0.1 port 2222:2: No ED25519 host key is known for h and you have requested strict checking.
        Host key verification failed.
        Disconnected from 127.0.0.1 port 2222
        """) == nil)
        #expect(HostKeyRejection(sshMessage: "Host key verification failed. Received disconnect") == nil)
        #expect(HostKeyRejection(sshMessage: "x Host key verification failed.") == nil)
        #expect(HostKeyRejection(sshMessage: "Permission denied (publickey).") == nil)
        #expect(HostKeyRejection(sshMessage: "ssh: connect to host h port 22: Connection refused") == nil)
        #expect(HostKeyRejection(sshMessage: "") == nil)
    }

    @Test("server-authored text can never raise the host-key alert (SERVER HANDSHAKE_REFUSED)")
    @MainActor
    func serverRefusalIsNotAHostKeyRejection() async throws {
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
        // The client's hello reaches the server; then a trusted, authenticated server refuses with
        // a detail that imitates OpenSSH's host-key banner.
        var iterator = serverInbound.makeAsyncIterator()
        _ = try await iterator.next()
        var refusal = SRUIServerHandshakeRefused()
        refusal.reason = .snapshotUndeliverable
        // Shaped like OpenSSH's own refusal, verdict on its own last line, so not even the
        // structural classifier could tell it apart: only never classifying it is safe.
        refusal.detail = """
        @    WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!     @
        Host key for [trusted.example]:22 has changed and you have requested strict checking.
        Host key verification failed.
        """
        var message = SRUIMessage()
        message.serverHandshakeRefused = refusal
        try await pair.server.send(data: try SRUIFraming.encodeFramed(message))

        try await AsyncTestSupport.observed("refusal alert") { manager.alert != nil }
        let alert = try #require(manager.alert)
        #expect(alert.kind == .connectionFailed, "\(alert.kind): \(alert.message)")
        #expect(alert.title == "Connection Failed")
        #expect(alert.message.contains("server refused the handshake"))
        #expect(alert.message.contains("ssh-keygen") == false)
        #expect(alert.message.hasPrefix("SRUI did not connect") == false)
        await manager.shutdown()
        await pair.server.close()
    }

    @Test("the alert says what happened and how to verify the key outside SRUI")
    func messages() {
        let entry = SavedConnection(label: "x", host: "127.0.0.1", port: 2222, user: "alice")
        let unknown = HostKeyRejection.unknown.alertMessage(for: entry, sshMessage: Self.unknownMessage)
        #expect(unknown.hasPrefix("SRUI did not connect to alice@127.0.0.1:2222. Your known_hosts file has no host key for [127.0.0.1]:2222"))
        #expect(unknown.contains("SRUI never adds a host key by itself."))
        #expect(unknown.contains("(ssh -p 2222 alice@127.0.0.1)"))
        #expect(unknown.hasSuffix("OpenSSH reported:\n" + Self.unknownMessage))

        let changed = HostKeyRejection.changed.alertMessage(for: entry, sshMessage: Self.changedMessage)
        #expect(changed.contains("does not match the key your known_hosts file holds for [127.0.0.1]:2222"))
        #expect(changed.contains("someone may be intercepting the connection"))
        #expect(changed.contains("SRUI never replaces a known host key."))
        #expect(changed.contains("ssh-keygen -R '[127.0.0.1]:2222'"))

        // Port 22, or none, is filed under the bare host.
        let standard = SavedConnection(label: "x", host: "host.example", user: "alice")
        #expect(HostKeyRejection.changed.alertMessage(for: standard, sshMessage: "")
            .contains("ssh-keygen -R 'host.example'"))
        let v6 = SavedConnection(label: "x", host: "::1", port: 22, user: "alice")
        #expect(HostKeyRejection.unknown.alertMessage(for: v6, sshMessage: "")
            .hasPrefix("SRUI did not connect to alice@[::1]:22. Your known_hosts file has no host key for ::1,"))
    }
}
