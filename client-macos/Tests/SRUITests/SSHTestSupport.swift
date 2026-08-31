//
// SSHTestSupport.swift
// SRUITests
//
// Shared helpers for ephemeral OpenSSH test fixtures (§19, §19.1).
//

import Foundation
#if canImport(Darwin)
import Darwin
#endif

enum SSHTestSupport {
    static func findFreePort() -> UInt16 {
        let fd = Darwin.socket(AF_INET, SOCK_STREAM, 0)
        guard fd >= 0 else { return UInt16.random(in: 23000...28000) }
        defer { Darwin.close(fd) }
        var addr = sockaddr_in()
        addr.sin_family = sa_family_t(AF_INET)
        addr.sin_addr.s_addr = inet_addr("127.0.0.1")
        addr.sin_port = 0
        let len = socklen_t(MemoryLayout<sockaddr_in>.size)
        let bindRes = withUnsafePointer(to: &addr) { ptr in
            ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sa in
                Darwin.bind(fd, sa, len)
            }
        }
        if bindRes == 0 {
            var actualAddr = sockaddr_in()
            var actualLen = len
            let getRes = withUnsafeMutablePointer(to: &actualAddr) { ptr in
                ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sa in
                    Darwin.getsockname(fd, sa, &actualLen)
                }
            }
            if getRes == 0 {
                return UInt16(bigEndian: actualAddr.sin_port)
            }
        }
        return UInt16.random(in: 23000...28000)
    }

    static func waitForPort(port: UInt16, timeoutSeconds: TimeInterval = 5.0) async throws {
        let deadline = Date().addingTimeInterval(timeoutSeconds)
        while Date() < deadline {
            let fd = Darwin.socket(AF_INET, SOCK_STREAM, 0)
            if fd >= 0 {
                var addr = sockaddr_in()
                addr.sin_family = sa_family_t(AF_INET)
                addr.sin_addr.s_addr = inet_addr("127.0.0.1")
                addr.sin_port = port.bigEndian
                let len = socklen_t(MemoryLayout<sockaddr_in>.size)
                let res = withUnsafePointer(to: &addr) { ptr in
                    ptr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sa in
                        Darwin.connect(fd, sa, len)
                    }
                }
                Darwin.close(fd)
                if res == 0 {
                    return
                }
            }
            try await Task.sleep(nanoseconds: 50_000_000)
        }
        throw SSHTestSupportError.portTimeout(port)
    }

    static func generateEd25519Key(at path: String) throws {
        let gen = Process()
        gen.executableURL = URL(fileURLWithPath: "/usr/bin/ssh-keygen")
        gen.arguments = ["-t", "ed25519", "-N", "", "-f", path]
        try gen.run()
        gen.waitUntilExit()
        guard gen.terminationStatus == 0 else {
            throw SSHTestSupportError.keyGenerationFailed(path)
        }
    }

    static func writeKnownHosts(port: UInt16, hostPublicKeyPath: String, to knownHostsPath: String) throws {
        let hostPubStr = try String(
            contentsOf: URL(fileURLWithPath: "\(hostPublicKeyPath).pub"),
            encoding: .utf8
        ).trimmingCharacters(in: .whitespacesAndNewlines)
        let entry = "[127.0.0.1]:\(port) \(hostPubStr)\n"
        try entry.write(to: URL(fileURLWithPath: knownHostsPath), atomically: true, encoding: .utf8)
    }
}

enum SSHTestSupportError: Error, CustomStringConvertible {
    case keyGenerationFailed(String)
    case portTimeout(UInt16)

    var description: String {
        switch self {
        case .keyGenerationFailed(let path):
            return "ssh-keygen failed for \(path)"
        case .portTimeout(let port):
            return "timed out waiting for port \(port) to open"
        }
    }
}
