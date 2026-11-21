//
// KnownHostsHint.swift
// ConnectionManager
//
// A hint built only from local facts (§19.1, §25; PX-023 round 5).
//
// Four review rounds showed that a peer can put text of its choosing on `ssh`'s stderr in shapes
// no classifier anticipates (a DISCONNECT reason, a KEXINIT name-list quoted by "Unable to
// negotiate", a banner). So no alert content is derived from `ssh`'s output: every SSH failure is
// "Connection Failed", and `ssh`'s stderr is quoted verbatim under a label that says it can carry
// the server's words. The only extra the app writes is this hint, and it states what the local
// known_hosts files hold for the destination, which is true whatever the server sent:
//
// - the files are the ones `ssh` itself would use, read from `ssh -G` with the same argv;
// - each is searched with `ssh-keygen -F <name> -f <file>`, which handles hashed entries;
// - both are argv launches (no shell), with stdin, stdout and stderr set before `run()`, each
//   bounded by its own 5 s deadline (SIGTERM, then SIGKILL), `ssh -G`'s stdout drained while it
//   runs and capped, and awaited without blocking any thread on the main actor. The worst case is
//   5 s × (1 + the number of readable known_hosts files), which only the user's own ssh_config
//   (a slow `Match exec`, many files) can approach;
// - any failure, timeout or unparseable answer yields no hint at all.
//
// SRUI never adds, replaces or removes a host key, and never suggests a command that would.
//

import Foundation
import TransportSSH

/// What the local known_hosts files hold for a destination.
public struct KnownHostsHint: Equatable, Sendable {
    public enum Presence: Equatable, Sendable {
        /// No checked file holds a key for the name. `checkedFiles` lists the files that exist.
        case absent(checkedFiles: [String])
        /// `file` holds a key for the name.
        case present(file: String)
    }

    /// The name `ssh` files the key under: `host`, or `[host]:port` for a port other than 22.
    public let name: String
    public let presence: Presence

    public var text: String {
        switch presence {
        case .absent(let checkedFiles):
            let files = checkedFiles.isEmpty ? "none of them exists yet" : checkedFiles.joined(separator: ", ")
            return "\(name) is not in your known_hosts files (\(files)). SRUI never adds or changes host keys."
        case .present(let file):
            return "A key for \(name) is in \(file). SRUI will not change it."
        }
    }

    /// The known_hosts name for `host` and `port`, as `ssh` and `ssh-keygen -F` use it.
    public static func knownHostsName(host: String, port: Int) -> String {
        port == 22 ? host : "[\(host)]:\(port)"
    }
}

/// Looks up the local known_hosts state for a connection's SSH configuration.
public protocol KnownHostsLookup: Sendable {
    /// The hint for `configuration`, or `nil` when the lookup failed or timed out.
    func hint(for configuration: SSHConfiguration) async -> KnownHostsHint?
}

/// The production lookup: `ssh -G` for the files and the name, `ssh-keygen -F` for each file.
public struct SSHKnownHostsLookup: KnownHostsLookup {
    public var sshKeygenPath: String
    public var timeout: Duration

    public init(sshKeygenPath: String = "/usr/bin/ssh-keygen", timeout: Duration = .seconds(5)) {
        self.sshKeygenPath = sshKeygenPath
        self.timeout = timeout
    }

    public func hint(for configuration: SSHConfiguration) async -> KnownHostsHint? {
        // The same argv the connection used, so the same ssh_config, options and known_hosts files.
        guard let resolved = await BoundedProcess.run(
            configuration.sshBinaryPath, ["-G"] + configuration.buildArguments(), timeout: timeout
        ), resolved.status == 0,
            let settings = Self.parseEffectiveConfiguration(resolved.text) else {
            return nil
        }
        let name = settings.name
        var checked: [String] = []
        for file in settings.files where FileManager.default.isReadableFile(atPath: file) {
            checked.append(file)
            // Exit status only: 0 found, 1 not found. Its stdout (the matching lines) is not needed.
            guard let search = await BoundedProcess.run(
                sshKeygenPath, ["-F", name, "-f", file], timeout: timeout, captureOutput: false
            ) else {
                return nil
            }
            switch search.status {
            case 0:
                return KnownHostsHint(name: name, presence: .present(file: file))
            case 1:
                continue
            default:
                return nil
            }
        }
        return KnownHostsHint(name: name, presence: .absent(checkedFiles: checked))
    }

    struct EffectiveSettings: Equatable {
        /// The name `ssh` looks the key up under: `HostKeyAlias` verbatim when set, otherwise
        /// `hostname` with `[ ]:port` for a port other than 22 (`sshconnect.c`).
        let name: String
        let files: [String]
    }

    /// Reads `hostname`, `hostkeyalias`, `port`, `userknownhostsfile` and `globalknownhostsfile`
    /// from `ssh -G` output. `ssh -G` prints file lists space-separated and unquoted, so a path
    /// containing a space cannot be recovered: every listed path must be absolute, or the whole
    /// lookup is abandoned (no hint rather than a wrong one).
    static func parseEffectiveConfiguration(_ output: String) -> EffectiveSettings? {
        var values: [String: String] = [:]
        for line in output.split(whereSeparator: \.isNewline) {
            let parts = line.split(separator: " ", maxSplits: 1, omittingEmptySubsequences: true)
            guard parts.count == 2 else { continue }
            let key = parts[0].lowercased()
            if values[key] == nil { values[key] = String(parts[1]) }
        }
        guard let hostname = values["hostname"], hostname.isEmpty == false,
              let portText = values["port"], let port = Int(portText), (1...65535).contains(port) else {
            return nil
        }
        var name = KnownHostsHint.knownHostsName(host: hostname, port: port)
        if let alias = values["hostkeyalias"], alias.isEmpty == false, alias.lowercased() != "none" {
            name = alias
        }
        var files: [String] = []
        for key in ["userknownhostsfile", "globalknownhostsfile"] {
            guard let list = values[key] else { continue }
            for path in list.split(separator: " ").map(String.init) where path.lowercased() != "none" {
                guard path.hasPrefix("/") else { return nil }
                files.append(path)
            }
        }
        return EffectiveSettings(name: name, files: files)
    }
}

/// Runs one program with an argument vector (never a shell), its stdin, stdout and stderr set
/// before launch, and a deadline. No thread waits on a run loop: the exit arrives through the
/// termination handler and stdout is drained on its own thread while the child runs (PX-023 round
/// 6: read only at exit, a child writing more than one pipe buffer blocked until the deadline).
///
/// A result is delivered only once **both** the exit and the end of the drain have been seen, so
/// it is never a truncated read (round 7). The result is `nil` (and the child is stopped) when it
/// cannot be launched, outlives `timeout`, or writes more than `maximumOutputBytes`. Stopping sends
/// SIGTERM to the child's exact PID, then SIGKILL to the same PID after `killGrace` if it has not
/// exited, so a child that ignores SIGTERM cannot outlive the lookup; neither signal is sent once
/// the exit is recorded. The deadline task is cancelled as soon as the run ends, so the `Process`
/// and its `Pipe` are released at once rather than when the deadline would have expired.
enum BoundedProcess {
    struct Result: Sendable, Equatable {
        let status: Int32
        let output: Data
        var text: String { String(decoding: output, as: UTF8.self) }
    }

    /// The most stdout read from one child; `ssh -G` prints a few kilobytes.
    static let maximumOutputBytes = 256 * 1024
    /// How long a child gets between SIGTERM and SIGKILL.
    static let killGrace: Duration = .milliseconds(500)

    /// Sends a signal; `kill(2)` in production, a recorder in tests.
    typealias Signal = @Sendable (pid_t, Int32) -> Int32

    /// Test seams. `beforeRead` runs on the drain thread before every read, the one that sees EOF
    /// included, so a test can hold the drain back after the child has exited. `launched` sees the
    /// launched `Process` (and its `Pipe`) so a test can check they are released.
    struct Hooks: Sendable {
        var beforeRead: (@Sendable () -> Void)?
        var launched: (@Sendable (Process, Pipe?) -> Void)?

        static let none = Hooks()
    }

    /// Shared between the termination handler, the stdout drain and the deadline; every field is
    /// read and written under `lock`, and the continuation is resumed exactly once.
    private final class Run: @unchecked Sendable {
        private let lock = NSLock()
        private var continuation: CheckedContinuation<Result?, Never>?
        private var status: Int32?
        private var output = Data()
        private var drained: Bool
        private var exited = false
        private var deadline: Task<Void, Never>?

        init(continuation: CheckedContinuation<Result?, Never>, drains: Bool) {
            self.continuation = continuation
            self.drained = drains == false
        }

        var hasExited: Bool {
            lock.lock()
            defer { lock.unlock() }
            return exited
        }

        /// Keeps the deadline task so the end of the run can cancel it; cancels it at once if the
        /// run has already ended.
        func setDeadline(_ task: Task<Void, Never>) {
            lock.lock()
            let ended = continuation == nil
            if ended == false { deadline = task }
            lock.unlock()
            if ended { task.cancel() }
        }

        /// Appends drained bytes; false once the cap is exceeded.
        func append(_ data: Data, cap: Int) -> Bool {
            lock.lock()
            defer { lock.unlock() }
            output.append(data)
            return output.count <= cap
        }

        func drainFinished() {
            lock.lock()
            drained = true
            let ready = takeIfComplete()
            lock.unlock()
            finish(ready)
        }

        func exit(_ code: Int32) {
            lock.lock()
            exited = true
            status = code
            let ready = takeIfComplete()
            lock.unlock()
            finish(ready)
        }

        /// Ends the run with no result (deadline, cap or launch failure); true if this call did.
        @discardableResult
        func fail() -> Bool {
            lock.lock()
            let taken = continuation
            continuation = nil
            let task = deadline
            deadline = nil
            lock.unlock()
            task?.cancel()
            taken?.resume(returning: nil)
            return taken != nil
        }

        private func finish(_ ready: (CheckedContinuation<Result?, Never>, Result, Task<Void, Never>?)?) {
            guard let ready else { return }
            ready.2?.cancel()
            ready.0.resume(returning: ready.1)
        }

        /// Called with `lock` held: the result exists only once both the exit and the end of the
        /// drain are recorded.
        private func takeIfComplete() -> (CheckedContinuation<Result?, Never>, Result, Task<Void, Never>?)? {
            guard drained, let status, let taken = continuation else { return nil }
            continuation = nil
            let task = deadline
            deadline = nil
            return (taken, Result(status: status, output: output), task)
        }
    }

    /// SIGTERM now, SIGKILL after `grace`, each only while the child's exit is unrecorded and
    /// `Process` still reports it running.
    // lc-debt: Foundation reaps the child before calling the termination handler, so between that
    // reap and `exit(_:)` recording it a signal could reach a recycled PID. The window is
    // microseconds and applies to both the first SIGTERM and the later SIGKILL; `isRunning` narrows
    // it (it turns false at the reap) but cannot close it. Upgrade path: launch with posix_spawn and
    // reap with our own waitpid, so the PID stays ours until we have finished signalling.
    private static func stop(_ process: Process, run: Run, grace: Duration, signal: @escaping Signal) {
        let pid = process.processIdentifier
        guard pid > 0, run.hasExited == false, process.isRunning else { return }
        _ = signal(pid, SIGTERM)
        Task.detached {
            try? await Task.sleep(for: grace)
            if run.hasExited == false, process.isRunning {
                _ = signal(pid, SIGKILL)
            }
        }
    }

    /// `captureOutput: false` sends stdout to /dev/null: `ssh-keygen -F` is read by exit status
    /// only, so a file with thousands of matching entries costs nothing.
    static func run(
        _ path: String,
        _ arguments: [String],
        timeout: Duration,
        captureOutput: Bool = true,
        maximumOutputBytes: Int = maximumOutputBytes,
        killGrace: Duration = killGrace,
        signal: @escaping Signal = { kill($0, $1) },
        hooks: Hooks = .none
    ) async -> Result? {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: path)
        process.arguments = arguments
        process.standardInput = FileHandle.nullDevice
        process.standardError = FileHandle.nullDevice
        let pipe = captureOutput ? Pipe() : nil
        process.standardOutput = pipe ?? FileHandle.nullDevice

        return await withCheckedContinuation { continuation in
            let run = Run(continuation: continuation, drains: pipe != nil)
            process.terminationHandler = { finished in
                run.exit(finished.terminationStatus)
            }
            do {
                try process.run()
            } catch {
                process.terminationHandler = nil
                run.fail()
                return
            }
            hooks.launched?(process, pipe)
            if let reader = pipe?.fileHandleForReading {
                // Its own thread, so a blocking read never occupies the cooperative pool. It ends
                // at EOF, which the child's exit (or its SIGKILL) delivers.
                let drain = Thread {
                    while true {
                        hooks.beforeRead?()
                        guard let chunk = try? reader.read(upToCount: 64 * 1024), chunk.isEmpty == false else {
                            break
                        }
                        if run.append(chunk, cap: maximumOutputBytes) == false {
                            if run.fail() { stop(process, run: run, grace: killGrace, signal: signal) }
                            break
                        }
                    }
                    run.drainFinished()
                }
                drain.name = "org.srui.ConnectionManager.BoundedProcess.stdout"
                drain.start()
            }
            run.setDeadline(Task.detached {
                try? await Task.sleep(for: timeout)
                guard Task.isCancelled == false else { return }
                if run.fail() { stop(process, run: run, grace: killGrace, signal: signal) }
            })
        }
    }
}
