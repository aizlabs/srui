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

/// Tracks sshd processes that `launchSSHD` started and `terminate` has not yet reaped.
///
/// swift-testing exits the test binary as soon as the last test finishes, which can cut a `defer`
/// in a still-unwinding test short and reliably strands a listener or two per run. They are inert
/// once their stdio is detached, but each holds a port and a /tmp fixture directory and they
/// accumulate across runs. `atexit` runs on that normal exit, so it is the one hook that sees
/// every straggler regardless of which test lost the race.
private final class SSHDRegistry: @unchecked Sendable {
    static let shared = SSHDRegistry()

    private let lock = NSLock()
    private var pids: Set<pid_t> = []
    private var hookInstalled = false

    func add(_ pid: pid_t) {
        lock.lock()
        defer { lock.unlock() }
        pids.insert(pid)
        guard !hookInstalled else { return }
        hookInstalled = true
        atexit { SSHDRegistry.shared.killAll() }
    }

    /// Drops a pid the caller has already killed and waited on, so the exit hook can never signal
    /// a number the kernel has since handed to an unrelated process.
    func remove(_ pid: pid_t) {
        lock.lock()
        defer { lock.unlock() }
        pids.remove(pid)
    }

    func killAll() {
        lock.lock()
        defer { lock.unlock() }
        for pid in pids {
            kill(pid, SIGKILL)
        }
        pids.removeAll()
    }
}

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

    /// sshd start-up is process launch under whatever load the run is under: the shared budget.
    static func waitForPort(port: UInt16, timeoutSeconds: TimeInterval = AsyncTestSupport.roundTripSeconds) async throws {
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
        SSHDRegistry.shared.add(sshd.processIdentifier)
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
        let pid = sshd.processIdentifier
        sshd.terminate()
        kill(pid, SIGKILL)
        sshd.waitUntilExit()
        SSHDRegistry.shared.remove(pid)
    }

    /// Writes an executable `/bin/sh` stand-in at `path` and runs it once, outside every test
    /// deadline, before returning.
    ///
    /// macOS assesses a newly written executable on its first exec, before it runs a line: right
    /// after a loaded run, a fresh two-line script took 6-13 s to start and 0.03 s the next time.
    /// A stub's first real run sits inside a test's wait, so that assessment failed
    /// `SSHTransportDescriptorOwnershipTests` ("Timed out waiting for stub wrote its pid") with
    /// nothing wrong. The warm-up run passes `--srui-warm-up`, which the prologue added here
    /// answers by exiting; no `ssh` argument vector starts with it.
    static func writeAssessedStub(at path: String, body: String) async throws {
        let script = "#!/bin/sh\n[ \"$1\" = \"--srui-warm-up\" ] && exit 0\n" + body + "\n"
        try script.write(to: URL(fileURLWithPath: path), atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: path)
        let warmUp = Process()
        warmUp.executableURL = URL(fileURLWithPath: path)
        warmUp.arguments = ["--srui-warm-up"]
        warmUp.standardInput = FileHandle.nullDevice
        warmUp.standardOutput = FileHandle.nullDevice
        warmUp.standardError = FileHandle.nullDevice
        // Awaited through the termination handler, not `waitUntilExit()`, which would service the
        // main run loop of a main-actor caller.
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            warmUp.terminationHandler = { _ in continuation.resume() }
            do {
                try warmUp.run()
            } catch {
                warmUp.terminationHandler = nil
                continuation.resume(throwing: error)
            }
        }
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
