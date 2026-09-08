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

    /// Launches an ephemeral sshd whose stdio is detached from this process.
    ///
    /// Every sshd spawn in the test suite must go through here. swift-test reads the test
    /// binary's stdout/stderr through pipes and only returns once *every* descriptor on the write
    /// end is closed. sshd that inherited those pipes and outlived the test therefore wedges the
    /// whole run after the tests have already passed — a silent, minutes-long stall with no
    /// failing test to point at.
    ///
    /// Setting `Process.standardOutput`/`standardError` is not enough: sshd re-execs itself at
    /// startup and the redirection does not survive into the re-exec'd listener (confirmed with
    /// lsof — fd 1/2 stay on the inherited pipes while the replacement descriptors land on spare
    /// fds). Redirecting in the shell before `exec` applies to the descriptors themselves, so
    /// sshd and every connection child it forks inherit /dev/null instead.
    ///
    /// `exec` means the returned `Process` still refers to sshd itself, so `terminate()` and
    /// `waitUntilExit()` keep their usual meaning for callers.
    static func launchSSHD(
        configPath: String,
        hostKeyPath: String,
        port: UInt16,
        debug: Bool = false
    ) throws -> Process {
        let sshd = Process()
        sshd.executableURL = URL(fileURLWithPath: "/bin/sh")
        // Paths arrive as positional parameters so they are never re-parsed by the shell.
        sshd.arguments = [
            "-c",
            "exec /usr/sbin/sshd -f \"$1\" -h \"$2\" \(debug ? "-d" : "-D") -p \"$3\" </dev/null >/dev/null 2>&1",
            "sshd",
            configPath,
            hostKeyPath,
            String(port),
        ]
        try sshd.run()  // stdio: detached by the shell redirection above
        return sshd
    }

    /// Tears down an sshd launched by `launchSSHD`.
    ///
    /// Guarding the kill with `Process.isRunning` is what leaked a listener pair per run: when it
    /// reports false the signal is skipped entirely and the daemon survives the test binary,
    /// accumulating ports and /tmp fixtures across runs. The SIGKILL here is unconditional and
    /// safe — Foundation has not reaped the child yet, so its pid cannot have been recycled, and
    /// signalling an already-dead pid just returns ESRCH.
    static func terminate(_ sshd: Process) {
        sshd.terminate()
        kill(sshd.processIdentifier, SIGKILL)
        sshd.waitUntilExit()
    }

    static func generateEd25519Key(at path: String) throws {
        let gen = Process()
        gen.executableURL = URL(fileURLWithPath: "/usr/bin/ssh-keygen")
        gen.arguments = ["-t", "ed25519", "-N", "", "-f", path]
        // Keeps ssh-keygen's progress banner off the test binary's stdout/stderr, so a spawn that
        // outlives its `waitUntilExit()` cannot hold those pipes open (see `launchSSHD`).
        gen.standardOutput = FileHandle.nullDevice
        gen.standardError = FileHandle.nullDevice
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
