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

    /// A `/bin/sh` stand-in for `ssh`, written and run once per suite per process, that runs
    /// whatever its suite last wrote to `behavior`.
    ///
    /// macOS assesses a newly written executable on its first exec, before it runs a line: right
    /// after a loaded run, a fresh two-line script took 6-13 s to start and 0.03 s the next time.
    /// A stub's first real run sits inside a test's wait, so that assessment failed
    /// `SSHTransportDescriptorOwnershipTests` ("Timed out waiting for stub wrote its pid"), and a
    /// fresh stub per test paid it again inside each test's time limit. One stub per suite pays it
    /// once, here, outside every deadline; `behavior` is sourced, not executed, so changing it
    /// costs nothing. The suite must be `.serialized`: its tests share `behavior`.
    static func assessedStub(for suite: String) async throws -> (path: String, behavior: String) {
        try await AssessedStubs.shared.stub(for: suite)
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

/// One assessed stub per suite, per process. The directories are `TestFixtureDirectory`s, removed
/// by its exit hook.
private actor AssessedStubs {
    static let shared = AssessedStubs()
    private var stubs: [String: Task<(path: String, behavior: String), Error>] = [:]

    func stub(for suite: String) async throws -> (path: String, behavior: String) {
        if let existing = stubs[suite] {
            return try await existing.value
        }
        let task = Task<(path: String, behavior: String), Error> {
            let directory = try TestFixtureDirectory.make(prefix: "srui-stub")
            let path = directory.appendingPathComponent("ssh-stub").path
            let behavior = directory.appendingPathComponent("behavior.sh").path
            // The warm-up passes `--srui-warm-up`, which no `ssh` argument vector starts with.
            let script = "#!/bin/sh\n[ \"$1\" = \"--srui-warm-up\" ] && exit 0\n. \"\(behavior)\"\n"
            try script.write(to: URL(fileURLWithPath: path), atomically: true, encoding: .utf8)
            try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: path)
            try await FixtureBinaries.absorbFirstLaunchAssessment(path, arguments: ["--srui-warm-up"])
            return (path, behavior)
        }
        stubs[suite] = task
        return try await task.value
    }
}

/// Fixture executables launched by tests: each is run once per process, outside every test
/// deadline, before its first real launch.
///
/// macOS assesses a newly linked executable on its first exec, before `main`. Measured on a busy
/// machine: 12-19 s for the first exec of a just-relinked `srtop`, 0.02 s for the next (sampled
/// stuck in `_dyld_start`). A fixture's first launch sits inside a socket or port wait of 5-10 s,
/// so a rebuild of `srui-sessiond`, `counter` or the bridge failed tests with nothing wrong in
/// them (`SSHTransportPersistenceIntegrationTests` failed 2 of 2 on its 5 s socket wait right after
/// a sessiond relink). Each warm-up passes arguments that make the binary refuse and exit.
enum FixtureBinaries {
    nonisolated(unsafe) private static var assessed: Set<String> = []
    /// Guards `assessed`.
    private static let lock = NSLock()

    static let sessiondRefusal = ["--socket"]
    static let bridgeRefusal = ["--socket"]
    static let counterRefusal = ["--port", "srui-warm-up"]
    static let codingAgentRefusal: [String] = []
    static let srtopRefusal: [String] = []

    static func absorbFirstLaunchAssessment(_ path: String, arguments: [String]) async throws {
        guard !lock.withLock({ assessed.contains(path) }) else { return }
        let warmUp = Process()
        warmUp.executableURL = URL(fileURLWithPath: path)
        warmUp.arguments = arguments
        warmUp.standardInput = FileHandle.nullDevice
        warmUp.standardOutput = FileHandle.nullDevice
        warmUp.standardError = FileHandle.nullDevice
        // Awaited through the termination handler, not `waitUntilExit()`, which would service the
        // main run loop of a main-actor caller. The calling test's `.timeLimit` bounds it.
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            warmUp.terminationHandler = { _ in continuation.resume() }
            do {
                try warmUp.run()
            } catch {
                warmUp.terminationHandler = nil
                continuation.resume(throwing: error)
            }
        }
        _ = lock.withLock { assessed.insert(path) }
    }
}
