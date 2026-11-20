//
// ConnectionEndpointTests.swift
// SRUITests
//
// PX-023: the Connect form's endpoint parsing, the `ssh` argv it becomes, and the host-key
// diagnostics (§19.1, §25; D2 T38). Deterministic: nothing here starts a process.
//

import Foundation
@testable import ConnectionManager
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

@Suite("Connect form endpoint parsing and argv (PX-023, §19.1, §25)")
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
        Rejected(host: "host\nexample", user: "alice", port: "", error: .invalidHostCharacter("\n")),
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

@Suite("Host-key rejection diagnostics (PX-023, §19.1, §25)")
struct HostKeyRejectionTests {
    // OpenSSH's own text (sshconnect.c), as `SSHTransport` reports a child exit; the live suite
    // records the same text from OpenSSH_10.2p1.
    static let unknownMessage = "transport ended: SSH process terminated with exit code 255: "
        + "No ED25519 host key is known for [127.0.0.1]:2222 and you have requested strict checking.\n"
        + "Host key verification failed."
    static let changedMessage = "transport ended: SSH process terminated with exit code 255: "
        + "@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@\n"
        + "@    WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!     @\n"
        + "@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@@\n"
        + "Host key for [127.0.0.1]:2222 has changed and you have requested strict checking.\n"
        + "Host key verification failed."

    @Test("OpenSSH's failures are classified, and other failures are not host-key failures")
    func classification() {
        #expect(HostKeyRejection(sshMessage: Self.unknownMessage) == .unknown)
        #expect(HostKeyRejection(sshMessage: Self.changedMessage) == .changed)
        #expect(HostKeyRejection(sshMessage: "@ WARNING: POSSIBLE DNS SPOOFING DETECTED! @") == .changed)
        #expect(HostKeyRejection(sshMessage: "@ WARNING: REVOKED HOST KEY DETECTED! @\nHost key verification failed.") == .revoked)
        #expect(HostKeyRejection(sshMessage: "Host key verification failed.") == .unverified)
        #expect(HostKeyRejection(sshMessage: "Permission denied (publickey).") == nil)
        #expect(HostKeyRejection(sshMessage: "ssh: connect to host h port 22: Connection refused") == nil)
        #expect(HostKeyRejection(sshMessage: "") == nil)
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
