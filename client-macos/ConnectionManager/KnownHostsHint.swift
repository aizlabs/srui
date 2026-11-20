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
// - both are argv launches (no shell), with stdin, stdout and stderr set before `run()`, bounded
//   by a timeout, and awaited without blocking any thread on the main actor;
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
            return "\(name) is not in your known_hosts files (\(files)). SRUI never adds or changes host keys. "
                + "If you mean to trust this host, verify its key fingerprint out of band, then add the key with ssh yourself."
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
            let settings = Self.parseEffectiveConfiguration(resolved.output) else {
            return nil
        }
        let name = settings.name
        var checked: [String] = []
        for file in settings.files where FileManager.default.isReadableFile(atPath: file) {
            checked.append(file)
            guard let search = await BoundedProcess.run(
                sshKeygenPath, ["-F", name, "-f", file], timeout: timeout
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
/// before launch, and a deadline. Waits through the termination handler, so no thread runs a run
/// loop for it; past the deadline the child is sent SIGTERM and the result is `nil`.
enum BoundedProcess {
    struct Result: Sendable, Equatable {
        let status: Int32
        let output: String
    }

    /// Output beyond this is not read; the lookups print a few kilobytes.
    static let maximumOutputBytes = 256 * 1024

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

    static func run(_ path: String, _ arguments: [String], timeout: Duration) async -> Result? {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: path)
        process.arguments = arguments
        let pipe = Pipe()
        process.standardInput = FileHandle.nullDevice
        process.standardOutput = pipe
        process.standardError = FileHandle.nullDevice
        let reader = pipe.fileHandleForReading
        let once = Once()

        return await withCheckedContinuation { continuation in
            process.terminationHandler = { finished in
                guard once.claim() else { return }
                let data = (try? reader.read(upToCount: maximumOutputBytes)) ?? nil
                continuation.resume(returning: Result(
                    status: finished.terminationStatus,
                    output: String(decoding: data ?? Data(), as: UTF8.self)
                ))
            }
            do {
                try process.run()
            } catch {
                process.terminationHandler = nil
                if once.claim() { continuation.resume(returning: nil) }
                return
            }
            Task.detached {
                try? await Task.sleep(for: timeout)
                guard once.claim() else { return }
                process.terminate()
                continuation.resume(returning: nil)
            }
        }
    }
}
