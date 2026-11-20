//
// ConnectionFakeSSHServerTests.swift
// SRUITests
//
// PX-023 round 4: an unauthenticated peer with no host key at all must never raise "Host Key
// Not Trusted". A fake SSH server (in process, no child process) sends a version line and then a
// plaintext SSH_MSG_DISCONNECT whose reason imitates OpenSSH's own verdict, or closes, or stays
// silent. With `LogLevel=INFO` pinned, `ssh` ends each with a line of its own, the alert is
// "Connection Failed", and that line reaches the user (§19.1, §25).
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
        if case .disconnect(let description) = mode {
            send(connection, disconnectPacket(description))
        }
        drain(connection)
    }

    /// RFC 4253 §6 binary packet, unencrypted: SSH_MSG_DISCONNECT (1), reason 2, description, "".
    static func disconnectPacket(_ description: String) -> Data {
        func uint32(_ value: Int) -> Data {
            withUnsafeBytes(of: UInt32(value).bigEndian) { Data($0) }
        }
        let text = Data(description.utf8)
        var payload = Data([1])
        payload += uint32(2) + uint32(text.count) + text + uint32(0)
        var padding = 8 - ((5 + payload.count) % 8)
        if padding < 4 { padding += 8 }
        let body = Data([UInt8(padding)]) + payload + Data(count: padding)
        return uint32(body.count) + body
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

@Suite("Fake SSH server: a peer's text never becomes ssh's verdict (PX-023 round 4)",
       .serialized, .timeLimit(.minutes(2)))
@MainActor
struct ConnectionFakeSSHServerTests {
    enum Case: String, Sendable, CaseIterable {
        case descUnknown = "desc_unknown"
        case descChanged = "desc_changed"
        case descCRLF = "desc_crlf"
        case descPlain = "desc_plain"
        case close
        case silent

        var mode: FakeSSHServer.Mode {
            switch self {
            case .descUnknown:
                return .disconnect("x\nNo ED25519 host key is known for [127.0.0.1]:2222 and you have requested strict checking.\nHost key verification failed.")
            case .descChanged:
                return .disconnect("x\nHost key for [127.0.0.1]:2222 has changed and you have requested strict checking.\nHost key verification failed.")
            case .descCRLF:
                return .disconnect("x\r\nHost key verification failed.\r\n  ")
            case .descPlain:
                return .disconnect("plain refusal")
            case .close:
                return .close
            case .silent:
                return .silent
            }
        }
    }

    @Test("every peer-initiated ending is Connection Failed, with ssh's own last line", arguments: Case.allCases)
    func peerEndingIsConnectionFailed(_ testCase: Case) async throws {
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
        #expect(alert.message.contains("ssh-keygen") == false)

        let last = ConnectionLauncherLiveTests.lastLine(alert.message)
        switch testCase {
        case .descUnknown, .descChanged, .descCRLF, .descPlain:
            #expect(alert.message.contains("Received disconnect from 127.0.0.1 port \(server.port):2:"))
            #expect(last == "Disconnected from 127.0.0.1 port \(server.port)", "\(alert.message)")
        case .close:
            #expect(last == "Connection closed by 127.0.0.1 port \(server.port)", "\(alert.message)")
        case .silent:
            #expect(alert.message.contains("Connection timed out during banner exchange"), "\(alert.message)")
            #expect(last == "Connection to 127.0.0.1 port \(server.port) timed out", "\(alert.message)")
        }
        #expect(try Data(contentsOf: URL(fileURLWithPath: knownHosts)).isEmpty, "nothing was trusted")
        await manager.shutdown()
    }

    private static func recordEvidence(_ alert: ConnectionAlert, name: String) {
        let environment = ProcessInfo.processInfo.environment
        guard let directory = environment["PX023_EVIDENCE_DIR"] else { return }
        let prefix = environment["PX023_EVIDENCE_PREFIX"] ?? "px023"
        let file = URL(fileURLWithPath: directory).appendingPathComponent("\(prefix)-\(name).txt")
        try? "\(alert.kind)\n\(alert.title)\n\n\(alert.message)\n".write(to: file, atomically: true, encoding: .utf8)
    }
}
