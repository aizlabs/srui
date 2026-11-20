//
// ConnectionFakeSSHServerTests.swift
// SRUITests
//
// PX-023 rounds 4–5: an unauthenticated peer with no host key at all shapes `ssh`'s stderr (a
// DISCONNECT reason, a KEXINIT name-list that "Unable to negotiate … Their offer:" quotes, a close,
// a reset, silence). A fake SSH server (in process, no child process) plays each, with text that
// imitates OpenSSH's host-key verdict. Every alert is "Connection Failed", and everything the app
// writes is exactly what a benign failure gets for the same known_hosts state: the endpoint and the
// local known_hosts hint. `ssh`'s own lines still reach the user, verbatim (§19.1, §25).
//

import AppKit
import Darwin
import Foundation
@testable import ConnectionManager
import Testing
import TransportSSH

/// A one-connection fake SSH server on 127.0.0.1, served by its own thread. Ported from the
/// verifier's `fake_sshd.py` (`verify3-fake_sshd.py.txt`).
final class FakeSSHServer: @unchecked Sendable {
    enum Mode: Sendable {
        /// Version line, then SSH_MSG_DISCONNECT (reason 2) with this description.
        case disconnect(String)
        /// Version line, then close.
        case close
        /// Version line, then two SSH_MSG_DISCONNECTs, the second with this description.
        case disconnect2(String)
        /// Version line, then a KEXINIT whose name-list at `position` (and its other direction)
        /// is `list` and the rest valid, so `ssh` fails negotiation quoting the server's list.
        case kexinit(position: Int, list: String)
        /// Version line, read the client's, then a TCP reset.
        case reset
        /// Accept and say nothing: `ssh` times out during the banner exchange.
        case silent
    }

    let port: UInt16
    private let listener: Int32
    private let stopped = NSLock()
    private var isStopped = false

    init(mode: Mode) throws {
        let fd = Darwin.socket(AF_INET, SOCK_STREAM, 0)
        guard fd >= 0 else { throw POSIXError(.EMFILE) }
        var address = sockaddr_in()
        address.sin_family = sa_family_t(AF_INET)
        address.sin_addr.s_addr = inet_addr("127.0.0.1")
        address.sin_port = 0
        var length = socklen_t(MemoryLayout<sockaddr_in>.size)
        let bound = withUnsafePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { Darwin.bind(fd, $0, length) }
        }
        guard bound == 0, Darwin.listen(fd, 4) == 0 else {
            Darwin.close(fd)
            throw POSIXError(.EADDRINUSE)
        }
        _ = withUnsafeMutablePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) { getsockname(fd, $0, &length) }
        }
        listener = fd
        port = UInt16(bigEndian: address.sin_port)
        let thread = Thread { [weak self] in
            Self.serve(listener: fd, mode: mode) { self?.stopRequested ?? true }
            Darwin.close(fd)
        }
        thread.name = "org.srui.tests.FakeSSHServer"
        thread.start()
    }

    private var stopRequested: Bool {
        stopped.lock()
        defer { stopped.unlock() }
        return isStopped
    }

    /// Non-blocking: the serving thread notices within its 100 ms poll and closes the listener.
    func stop() {
        stopped.lock()
        isStopped = true
        stopped.unlock()
    }

    private static func serve(listener: Int32, mode: Mode, shouldStop: () -> Bool) {
        var pending = pollfd(fd: listener, events: Int16(POLLIN), revents: 0)
        while shouldStop() == false {
            guard poll(&pending, 1, 100) > 0 else { continue }
            let connection = accept(listener, nil, nil)
            guard connection >= 0 else { continue }
            var timeout = timeval(tv_sec: 10, tv_usec: 0)
            setsockopt(connection, SOL_SOCKET, SO_RCVTIMEO, &timeout, socklen_t(MemoryLayout<timeval>.size))
            var noSigPipe: Int32 = 1
            setsockopt(connection, SOL_SOCKET, SO_NOSIGPIPE, &noSigPipe, socklen_t(MemoryLayout<Int32>.size))
            handle(connection, mode: mode)
            Darwin.close(connection)
            return
        }
    }

    private static func handle(_ connection: Int32, mode: Mode) {
        if case .silent = mode {
            drain(connection)
            return
        }
        send(connection, Data("SSH-2.0-OpenSSH_10.2\r\n".utf8))
        // The client's version line first, as a real server would read it.
        var received = Data()
        var buffer = [UInt8](repeating: 0, count: 4096)
        while received.contains(UInt8(ascii: "\n")) == false {
            let count = recv(connection, &buffer, buffer.count, 0)
            guard count > 0 else { return }
            received.append(contentsOf: buffer[0..<count])
        }
        switch mode {
        case .disconnect(let description):
            send(connection, disconnectPacket(description))
        case .disconnect2(let description):
            send(connection, disconnectPacket("first") + disconnectPacket(description))
        case .kexinit(let position, let list):
            send(connection, kexinitPacket(position: position, list: list))
        case .reset:
            var linger = Darwin.linger(l_onoff: 1, l_linger: 0)
            setsockopt(connection, SOL_SOCKET, SO_LINGER, &linger, socklen_t(MemoryLayout<Darwin.linger>.size))
            return
        case .close, .silent:
            break
        }
        drain(connection)
    }

    private static func packet(_ payload: Data) -> Data {
        var padding = 8 - ((5 + payload.count) % 8)
        if padding < 4 { padding += 8 }
        let body = Data([UInt8(padding)]) + payload + Data(count: padding)
        return uint32(body.count) + body
    }

    private static func uint32(_ value: Int) -> Data {
        withUnsafeBytes(of: UInt32(value).bigEndian) { Data($0) }
    }

    private static func sshString(_ text: String) -> Data {
        let bytes = Data(text.utf8)
        return uint32(bytes.count) + bytes
    }

    /// RFC 4253 §7.1 KEXINIT: kex, host key, ciphers ×2, MACs ×2, compression ×2, languages ×2.
    static func kexinitPacket(position: Int, list: String) -> Data {
        var lists = ["curve25519-sha256", "ssh-ed25519", "aes128-ctr", "aes128-ctr",
                     "hmac-sha2-256", "hmac-sha2-256", "none", "none", "", ""]
        lists[position] = list
        if [2, 4, 6].contains(position) { lists[position + 1] = list }
        var payload = Data([20]) + Data(repeating: 0x11, count: 16)
        for item in lists { payload += sshString(item) }
        payload += Data([0]) + uint32(0)
        return packet(payload)
    }

    /// RFC 4253 §6 binary packet, unencrypted: SSH_MSG_DISCONNECT (1), reason 2, description, "".
    static func disconnectPacket(_ description: String) -> Data {
        packet(Data([1]) + uint32(2) + sshString(description) + uint32(0))
    }

    private static func send(_ connection: Int32, _ data: Data) {
        data.withUnsafeBytes { raw in
            var offset = 0
            while offset < raw.count {
                let sent = Darwin.send(connection, raw.baseAddress! + offset, raw.count - offset, 0)
                guard sent > 0 else { return }
                offset += sent
            }
        }
    }

    private static func drain(_ connection: Int32) {
        var buffer = [UInt8](repeating: 0, count: 4096)
        while recv(connection, &buffer, buffer.count, 0) > 0 {}
    }
}

@Suite("Fake SSH server: a peer's text never shapes what the app writes (PX-023 rounds 4–5)",
       .serialized, .timeLimit(.minutes(3)))
@MainActor
struct ConnectionFakeSSHServerTests {
    nonisolated static let unknownVerdict = "x\nNo ED25519 host key is known for [127.0.0.1]:2222 and you have requested strict checking.\nHost key verification failed."
    nonisolated static let changedVerdict = "x\nHost key for [127.0.0.1]:2222 has changed and you have requested strict checking.\nHost key verification failed."

    enum Case: String, Sendable, CaseIterable {
        case descUnknown = "desc_unknown"
        case descChanged = "desc_changed"
        case descCRLF = "desc_crlf"
        case descPlain = "desc_plain"
        case disconnect2
        case kexKex = "kex_kex"
        case kexHostkey = "kex_hostkey"
        case kexCipher = "kex_cipher"
        case kexMac = "kex_mac"
        case kexComp = "kex_comp"
        case kexChanged = "kex_changed"
        case close
        case reset
        case silent

        var mode: FakeSSHServer.Mode {
            switch self {
            case .descUnknown: return .disconnect(ConnectionFakeSSHServerTests.unknownVerdict)
            case .descChanged: return .disconnect(ConnectionFakeSSHServerTests.changedVerdict)
            case .descCRLF: return .disconnect("x\r\nHost key verification failed.\r\n  ")
            case .descPlain: return .disconnect("plain refusal")
            case .disconnect2: return .disconnect2(ConnectionFakeSSHServerTests.unknownVerdict)
            case .kexKex: return .kexinit(position: 0, list: ConnectionFakeSSHServerTests.unknownVerdict)
            case .kexHostkey: return .kexinit(position: 1, list: ConnectionFakeSSHServerTests.unknownVerdict)
            case .kexCipher: return .kexinit(position: 2, list: ConnectionFakeSSHServerTests.unknownVerdict)
            case .kexMac: return .kexinit(position: 4, list: ConnectionFakeSSHServerTests.unknownVerdict)
            case .kexComp: return .kexinit(position: 6, list: ConnectionFakeSSHServerTests.unknownVerdict)
            case .kexChanged: return .kexinit(position: 0, list: ConnectionFakeSSHServerTests.changedVerdict)
            case .close: return .close
            case .reset: return .reset
            case .silent: return .silent
            }
        }
    }

    /// What the app writes for a failure against an empty known_hosts, whatever the peer did:
    /// spelled out here, independently of the production formatter.
    static func expectedAppText(port: UInt16, knownHosts: String) -> String {
        let globals = ["/etc/ssh/ssh_known_hosts", "/etc/ssh/ssh_known_hosts2"]
            .filter { FileManager.default.isReadableFile(atPath: $0) }
        let files = ([knownHosts] + globals).joined(separator: ", ")
        return "SRUI could not connect to alice@127.0.0.1:\(port).\n\n"
            + "[127.0.0.1]:\(port) is not in your known_hosts files (\(files)). SRUI never adds or changes host keys. "
            + "If you mean to trust this host, verify its key fingerprint out of band, then add the key with ssh yourself."
    }

    @Test("every peer behaviour gets the same title and app text as a benign failure", arguments: Case.allCases)
    func peerTextNeverShapesTheAlert(_ testCase: Case) async throws {
        let server = try FakeSSHServer(mode: testCase.mode)
        defer { server.stop() }
        let directory = try TestFixtureDirectory.make(prefix: "px023-fake")
        defer { TestFixtureDirectory.release(directory) }
        let knownHosts = directory.appendingPathComponent("known_hosts").path
        try Data().write(to: URL(fileURLWithPath: knownHosts))
        let store = SavedConnectionStore(url: directory.appendingPathComponent("connections.json"))

        let probe = LauncherProbe()
        let manager = ConnectionManager(
            store: store,
            attemptFactory: probe.make(identityFile: directory.appendingPathComponent("absent_key").path,
                                       knownHostsFile: knownHosts,
                                       connectTimeout: testCase == .silent ? 2 : 10))
        #expect(manager.connect(ConnectDraft(host: "127.0.0.1:\(server.port)", user: "alice")) != nil)
        try await AsyncTestSupport.observed("peer-ending alert") { manager.alert != nil }
        let alert = try #require(manager.alert)
        Self.recordEvidence(alert, name: "fake-\(testCase.rawValue)")
        #expect(alert.kind == .connectionFailed, "\(alert.kind): \(alert.message)")
        #expect(alert.title == "Connection Failed")
        let (appText, quoted) = try #require(Self.split(alert.message), "\(alert.message)")
        #expect(appText == Self.expectedAppText(port: server.port, knownHosts: knownHosts), "\(alert.message)")
        #expect(appText.contains("ssh-keygen") == false)

        // ssh's own lines still reach the user, verbatim, below the label.
        switch testCase {
        case .descUnknown, .descChanged, .descCRLF, .descPlain, .disconnect2:
            #expect(quoted.contains("Received disconnect from 127.0.0.1 port \(server.port):2:"))
            #expect(quoted.contains("Disconnected from 127.0.0.1 port \(server.port)"), "\(quoted)")
        case .kexKex, .kexHostkey, .kexCipher, .kexMac, .kexComp, .kexChanged:
            #expect(quoted.contains("Unable to negotiate with 127.0.0.1 port \(server.port)"), "\(quoted)")
        case .close:
            #expect(quoted.contains("Connection closed by 127.0.0.1 port \(server.port)"), "\(quoted)")
        case .reset:
            #expect(quoted.contains("127.0.0.1 port \(server.port)"), "\(quoted)")
        case .silent:
            #expect(quoted.contains("Connection timed out during banner exchange"), "\(quoted)")
        }
        #expect(try Data(contentsOf: URL(fileURLWithPath: knownHosts)).isEmpty, "nothing was trusted")
        await manager.shutdown()
    }

    /// Splits an `ssh` failure alert into what the app wrote and what it quoted from `ssh`.
    nonisolated static func split(_ message: String) -> (appText: String, quoted: String)? {
        let marker = "\n\n" + ConnectionManager.sshOutputLabel + "\n"
        guard let range = message.range(of: marker) else { return nil }
        return (String(message[..<range.lowerBound]), String(message[range.upperBound...]))
    }

    private static func recordEvidence(_ alert: ConnectionAlert, name: String) {
        let environment = ProcessInfo.processInfo.environment
        guard let directory = environment["PX023_EVIDENCE_DIR"] else { return }
        let prefix = environment["PX023_EVIDENCE_PREFIX"] ?? "px023"
        let file = URL(fileURLWithPath: directory).appendingPathComponent("\(prefix)-\(name).txt")
        try? "\(alert.kind)\n\(alert.title)\n\n\(alert.message)\n".write(to: file, atomically: true, encoding: .utf8)
    }
}
