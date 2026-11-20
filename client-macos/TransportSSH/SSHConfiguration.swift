//
// SSHConfiguration.swift
// TransportSSH
//
// Configuration and argument construction for SSH transport binding (§19, §19.1).
//

import Foundation

/// Host key checking mode for OpenSSH (§19.1).
public enum StrictHostKeyCheckingMode: String, Sendable, Equatable {
    case yes = "yes"
    case ask = "ask"
    case acceptNew = "accept-new"
}

/// Configuration options for establishing an SSH transport session (§19, §19.1, §25).
public struct SSHConfiguration: Sendable, Equatable {
    /// OpenSSH `-o` keys that must not appear in `extraOptions`, in any case, because they can weaken
    /// §19.1 posture or change what `ssh` writes to the stderr the client reads as `ssh`'s own.
    public static let blockedExtraOptionKeys: Set<String> = [
        "StrictHostKeyChecking",
        "ClearAllForwardings",
        "ExitOnForwardFailure",
        "ForwardAgent",
        "LocalForward",
        "RemoteForward",
        "DynamicForward",
        "ProxyCommand",
        "PermitLocalCommand",
        "LogLevel",
        "UpdateHostKeys",
    ]

    /// Remote host name or IP address.
    public var host: String

    /// Optional SSH server port (defaults to standard SSH port 22 if nil).
    public var port: UInt16?

    /// Optional remote username.
    public var user: String?

    /// Subsystem name to request (defaults to fixed subsystem "srui", §19, §19.1).
    public var subsystem: String

    /// Optional identity file path (`-i identity_file`).
    public var identityFile: String?

    /// Optional custom known_hosts file path (`-o UserKnownHostsFile=path`).
    public var knownHostsFile: String?

    /// Strict host key checking mode (defaults to `.yes`, §19.1).
    public var strictHostKeyChecking: StrictHostKeyCheckingMode

    /// Batch mode flag (`-o BatchMode=yes`, disables interactive passphrase/host-key prompts).
    public var batchMode: Bool

    /// Connection timeout in seconds (`-o ConnectTimeout=N`).
    public var connectTimeout: TimeInterval?

    /// Additional `-o` options, emitted after every posture option (OpenSSH keeps the first value).
    /// Keys in `blockedExtraOptionKeys`, matched case-insensitively, are ignored.
    public var extraOptions: [String: String]

    /// Path to system SSH binary (defaults to `/usr/bin/ssh`).
    public var sshBinaryPath: String

    public init(
        host: String,
        port: UInt16? = nil,
        user: String? = nil,
        subsystem: String = "srui",
        identityFile: String? = nil,
        knownHostsFile: String? = nil,
        strictHostKeyChecking: StrictHostKeyCheckingMode = .yes,
        batchMode: Bool = false,
        connectTimeout: TimeInterval? = nil,
        extraOptions: [String: String] = [:],
        sshBinaryPath: String = "/usr/bin/ssh"
    ) {
        self.host = host
        self.port = port
        self.user = user
        self.subsystem = subsystem
        self.identityFile = identityFile
        self.knownHostsFile = knownHostsFile
        self.strictHostKeyChecking = strictHostKeyChecking
        self.batchMode = batchMode
        self.connectTimeout = connectTimeout
        self.extraOptions = extraOptions
        self.sshBinaryPath = sshBinaryPath
    }

    /// Whether `key` is one of `blockedExtraOptionKeys`. OpenSSH matches option keywords without
    /// regard to case (`loglevel`, `LOGLEVEL`), so the blocklist does too (PX-023 round 4).
    public static func isBlockedExtraOptionKey(_ key: String) -> Bool {
        let folded = key.lowercased()
        return blockedExtraOptionKeys.contains { $0.lowercased() == folded }
    }

    /// Builds the argument vector for executing `/usr/bin/ssh` conforming strictly to §19.1:
    /// - `-T`: Request no pseudo-terminal (PTY) allocation.
    /// - `-x`: Disable X11 forwarding.
    /// - `-a`: Disable authentication agent forwarding.
    /// - `-o ClearAllForwardings=yes`: Prevent ad hoc local/remote port forwards.
    /// - `-o ExitOnForwardFailure=yes`: Exit immediately if any forwarding fails.
    /// - `-o StrictHostKeyChecking=...`: Enforce strict host-key verification with fail-closed semantics.
    /// - `-o LogLevel=INFO`, pinned (PX-023 round 4): `ssh` ends every peer-initiated failure with
    ///   a line of its own at INFO ("Disconnected from …", "Connection closed by …"), so a
    ///   server's text is never the last line of `ssh`'s stderr before the stream starts, and the
    ///   user's `ssh_config` can neither lower it (hiding those lines and the diagnostics they
    ///   carry) nor raise it (adding debug lines after `ssh`'s verdict).
    /// - `-o UpdateHostKeys=no` (PX-023 round 6): OpenSSH's default (`yes` when the user's own
    ///   known_hosts is in use) lets a trusted server's extra or rotated keys be written into
    ///   known_hosts after login. SRUI's connections never change known_hosts; trust stays explicit.
    /// - `-s <subsystem>`: Request fixed subsystem, preventing shell command interpolation.
    ///
    /// OpenSSH takes the **first** value given for an option. Every posture and configuration
    /// option is therefore emitted **before** `extraOptions`, which can only add options nothing
    /// above sets; keys in `blockedExtraOptionKeys` (any case) are dropped outright.
    public func buildArguments() -> [String] {
        var args: [String] = [
            "-T", // §19.1: No PTY for SRUI protocol channel
            "-x", // §19.1: No X11 forwarding
            "-a", // §19.1: No agent forwarding by default
            "-o", "ClearAllForwardings=yes", // §19.1: No ad hoc port forwards
            "-o", "ExitOnForwardFailure=yes",
            "-o", "StrictHostKeyChecking=\(strictHostKeyChecking.rawValue)", // §19.1: Fail closed on host-key changes
            "-o", "LogLevel=INFO", // pinned: ssh's own final line always ends a peer-initiated failure
            "-o", "UpdateHostKeys=no", // §25 explicit trust: ssh never rewrites known_hosts for SRUI
        ]

        if let knownHostsFile {
            args.append(contentsOf: ["-o", "UserKnownHostsFile=\(knownHostsFile)"])
        }

        if batchMode {
            args.append(contentsOf: ["-o", "BatchMode=yes"])
        }

        if let connectTimeout {
            let seconds = max(1, Int(connectTimeout))
            args.append(contentsOf: ["-o", "ConnectTimeout=\(seconds)"])
        }

        if identityFile != nil {
            args.append(contentsOf: ["-o", "IdentitiesOnly=yes"])
        }

        for (key, value) in extraOptions.sorted(by: { $0.key < $1.key }) {
            guard !Self.isBlockedExtraOptionKey(key) else { continue }
            args.append(contentsOf: ["-o", "\(key)=\(value)"])
        }

        if let port {
            args.append(contentsOf: ["-p", String(port)])
        }

        if let user {
            args.append(contentsOf: ["-l", user])
        }

        if let identityFile {
            args.append(contentsOf: ["-i", identityFile])
        }

        // §19.1: Fixed subsystem request rather than a shell-interpolated command string.
        // `-s` takes no argument: it makes the remote command (the operand after the host) a
        // subsystem name. `--` ends option parsing, so a host that begins with `-` (for example
        // `-oProxyCommand=…`) is a destination, never an ssh option.
        args.append(contentsOf: ["-s", "--", host, subsystem])

        return args
    }
}
