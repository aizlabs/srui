//
// SSHTransportPostureTests.swift
// SRUITests
//
// Unit tests inspecting SSH transport invocation arguments against §19.1 normative posture rules.
//

import Testing
import Foundation
import TransportSSH

@Suite("SSH Transport Posture Tests (§19.1)")
struct SSHTransportPostureTests {

    @Test("Default configuration generates strict §19.1 posture arguments")
    func defaultConfigurationPosture() {
        let config = SSHConfiguration(host: "remote.example.com")
        let args = config.buildArguments()

        // §19.1 Rule 1: Request no PTY for the SRUI protocol channel
        #expect(args.contains("-T"), "Expected -T to disable pseudo-terminal allocation (§19.1)")

        // §19.1 Rule 2: No X11 forwarding
        #expect(args.contains("-x"), "Expected -x to disable X11 forwarding (§19.1)")

        // §19.1 Rule 3: No agent forwarding by default
        #expect(args.contains("-a"), "Expected -a to disable authentication agent forwarding (§19.1)")

        // §19.1 Rule 4: No ad hoc port forwards
        #expect(args.contains("ClearAllForwardings=yes"), "Expected ClearAllForwardings=yes to prevent ad hoc port forwards (§19.1)")
        #expect(args.contains("ExitOnForwardFailure=yes"), "Expected ExitOnForwardFailure=yes (§19.1)")

        // §19.1 Rule 5: Strict host key checking
        #expect(args.contains("StrictHostKeyChecking=yes"), "Expected StrictHostKeyChecking=yes (§19.1)")

        // §19.1 / PX-023: ssh's stderr holds only ssh's own errors, never a server's banner
        // pinned to INFO (round 4), so ssh's own last line ends every peer-initiated failure
        if let index = args.firstIndex(of: "LogLevel=INFO") {
            #expect(args[index - 1] == "-o")
        } else {
            Issue.record("Expected -o LogLevel=INFO (PX-023)")
        }

        // §19.1 Rule 6: Fixed subsystem request, not a shell command string
        // `-s` is a flag; the subsystem name is the operand after the host, and `--` ends
        // option parsing before the host.
        #expect(args.contains("-s"), "Expected -s flag for subsystem mode (§19.1)")
        #expect(Array(args.suffix(4)) == ["-s", "--", "remote.example.com", "srui"],
                "Expected `-s -- <host> <subsystem>` at the end of the invocation (§19.1)")

        // Ensure no shell command injection wrappers are present
        #expect(!args.contains("/bin/sh"))
        #expect(!args.contains("/bin/bash"))
        #expect(!args.contains("-c"))
        #expect(!args.contains("-L"))
        #expect(!args.contains("-R"))
        #expect(!args.contains("-D"))
    }

    @Test("A host that begins with a dash is a destination after --, never an ssh option")
    func dashLeadingHostIsNotAnOption() {
        let host = "-oProxyCommand=touch /tmp/srui-injected"
        let args = SSHConfiguration(host: host).buildArguments()

        guard let endOfOptions = args.firstIndex(of: "--") else {
            Issue.record("Missing -- before the host")
            return
        }
        #expect(Array(args[(endOfOptions + 1)...]) == [host, "srui"])
        #expect(args.firstIndex(of: host) == endOfOptions + 1, "The host must appear only after --")
    }

    @Test("Custom configuration options are formatted correctly")
    func customConfigurationOptions() {
        let config = SSHConfiguration(
            host: "127.0.0.1",
            port: 2222,
            user: "sruiuser",
            subsystem: "srui",
            identityFile: "/tmp/test_id_ed25519",
            knownHostsFile: "/tmp/test_known_hosts",
            strictHostKeyChecking: .yes,
            batchMode: true,
            connectTimeout: 5.0,
            extraOptions: ["Compression": "no"]
        )
        let args = config.buildArguments()

        // Check port
        if let pIdx = args.firstIndex(of: "-p") {
            #expect(args[pIdx + 1] == "2222")
        } else {
            Issue.record("Missing -p flag")
        }

        // Check user
        if let uIdx = args.firstIndex(of: "-l") {
            #expect(args[uIdx + 1] == "sruiuser")
        } else {
            Issue.record("Missing -l flag")
        }

        // Check identity
        if let iIdx = args.firstIndex(of: "-i") {
            #expect(args[iIdx + 1] == "/tmp/test_id_ed25519")
        } else {
            Issue.record("Missing -i flag")
        }

        // Check known_hosts and batch options
        #expect(args.contains("UserKnownHostsFile=/tmp/test_known_hosts"))
        #expect(args.contains("BatchMode=yes"))
        #expect(args.contains("ConnectTimeout=5"))
        #expect(args.contains("Compression=no"))
        #expect(args.contains("StrictHostKeyChecking=yes"))
        #expect(args.contains("127.0.0.1"))
    }

    @Test("Blocked extraOptions cannot weaken normative §19.1 posture")
    func blockedExtraOptionsCannotWeakenPosture() {
        let config = SSHConfiguration(
            host: "remote.example.com",
            extraOptions: [
                "StrictHostKeyChecking": "no",
                "ClearAllForwardings": "no",
                "ForwardAgent": "yes",
                "LocalForward": "8080:127.0.0.1:80",
                "LogLevel": "DEBUG3",
                "Compression": "no",
            ]
        )
        let args = config.buildArguments()

        #expect(!args.contains("StrictHostKeyChecking=no"))
        #expect(args.contains("StrictHostKeyChecking=yes"))
        #expect(!args.contains("ClearAllForwardings=no"))
        #expect(args.contains("ClearAllForwardings=yes"))
        #expect(!args.contains("ForwardAgent=yes"))
        #expect(!args.contains("LocalForward=8080:127.0.0.1:80"))
        #expect(!args.contains("LogLevel=DEBUG3"))
        #expect(args.filter { $0.hasPrefix("LogLevel=") } == ["LogLevel=INFO"])
        #expect(args.contains("Compression=no"))

        if let strictIdx = args.firstIndex(of: "StrictHostKeyChecking=yes") {
            if let compressionIdx = args.firstIndex(of: "Compression=no") {
                // PX-023 round 4: OpenSSH keeps the first value, so posture comes first.
                #expect(strictIdx < compressionIdx, "Posture flags must precede filtered extraOptions")
            }
        } else {
            Issue.record("Missing StrictHostKeyChecking=yes")
        }
    }

    /// Every case spelling of every blocked key, plus non-blocked keys the posture also sets.
    static let hostileExtraOptions: [String: String] = [
        "stricthostkeychecking": "no",
        "STRICTHOSTKEYCHECKING": "no",
        "loglevel": "DEBUG3",
        "LOGLEVEL": "QUIET",
        "logLevel": "ERROR",
        "forwardagent": "yes",
        "ProxyCommand": "/usr/bin/false",
        "proxycommand": "/usr/bin/false",
        "clearallforwardings": "no",
        "exitonforwardfailure": "no",
        "permitlocalcommand": "yes",
        "localforward": "8080:127.0.0.1:80",
        "BatchMode": "no",
        "batchmode": "no",
        "UserKnownHostsFile": "/tmp/px023-hostile-known-hosts",
        "IdentitiesOnly": "no",
        "ConnectTimeout": "999",
        "Compression": "yes",
    ]

    static let hostileConfiguration = SSHConfiguration(
        host: "127.0.0.1", port: 2222, user: "alice", subsystem: "srui",
        identityFile: "/tmp/px023-id", knownHostsFile: "/tmp/px023-known-hosts",
        strictHostKeyChecking: .yes, batchMode: true, connectTimeout: 5,
        extraOptions: hostileExtraOptions)

    @Test("blocked keys are dropped in any case, and every posture option precedes every extra option")
    func blockedKeysAnyCaseAndPostureFirst() {
        let args = Self.hostileConfiguration.buildArguments()
        let options = args.indices.filter { $0 > 0 && args[$0 - 1] == "-o" }.map { args[$0] }
        let lowered = options.map { $0.lowercased() }
        for key in SSHConfiguration.blockedExtraOptionKeys {
            let prefix = key.lowercased() + "="
            let posture = lowered.filter { $0.hasPrefix(prefix) }
            #expect(posture.count <= 1, "\(key) appears \(posture.count) times: \(options)")
        }
        #expect(options.contains("LogLevel=INFO"))
        #expect(lowered.filter { $0.hasPrefix("loglevel=") } == ["loglevel=info"])
        #expect(lowered.filter { $0.hasPrefix("stricthostkeychecking=") } == ["stricthostkeychecking=yes"])
        #expect(lowered.contains { $0.hasPrefix("proxycommand=") } == false)
        #expect(lowered.contains { $0.hasPrefix("forwardagent=") } == false)
        // OpenSSH keeps the first value: every posture option comes before the first extra.
        let firstExtra = try? #require(options.firstIndex(of: "Compression=yes"))
        for posture in ["ClearAllForwardings=yes", "ExitOnForwardFailure=yes", "StrictHostKeyChecking=yes",
                        "LogLevel=INFO", "UserKnownHostsFile=/tmp/px023-known-hosts", "BatchMode=yes",
                        "ConnectTimeout=5", "IdentitiesOnly=yes"] {
            let index = options.firstIndex(of: posture)
            #expect(index != nil, "missing \(posture)")
            if let index, let firstExtra {
                #expect(index < firstExtra, "\(posture) must precede the extra options")
            }
            #expect(options.firstIndex { $0.lowercased().hasPrefix(posture.split(separator: "=")[0].lowercased() + "=") } == index,
                    "\(posture) must be the first value for its key")
        }
    }

    /// `ssh -G` prints the configuration `ssh` would use, without connecting. Run with a hostile
    /// `-F` config too, standing in for the user's `~/.ssh/config`.
    private static func effectiveConfiguration(_ args: [String], configFile: String) async throws -> [String: String] {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/ssh")
        // `-G` before the posture; the trailing `-s -- host srui` is ignored by -G.
        process.arguments = ["-G", "-F", configFile] + args
        let pipe = Pipe()
        process.standardInput = FileHandle.nullDevice
        process.standardOutput = pipe
        process.standardError = FileHandle.nullDevice
        let reader = pipe.fileHandleForReading
        let output: String = try await withCheckedThrowingContinuation { continuation in
            process.terminationHandler = { _ in
                continuation.resume(returning: String(decoding: reader.readDataToEndOfFile(), as: UTF8.self))
            }
            do { try process.run() } catch {
                process.terminationHandler = nil
                continuation.resume(throwing: error)
            }
        }
        var values: [String: String] = [:]
        for line in output.split(whereSeparator: \.isNewline) {
            let parts = line.split(separator: " ", maxSplits: 1).map(String.init)
            if parts.count == 2, values[parts[0]] == nil { values[parts[0]] = parts[1] }
        }
        return values
    }

    @Test("ssh -G: no extra option and no ssh_config can override the posture")
    func sshEffectiveConfigurationKeepsPosture() async throws {
        let directory = try TestFixtureDirectory.make(prefix: "px023-sshG")
        defer { TestFixtureDirectory.release(directory) }
        let config = directory.appendingPathComponent("hostile_config")
        try """
        Host *
            StrictHostKeyChecking no
            LogLevel QUIET
            BatchMode no
            ForwardAgent yes
            ClearAllForwardings no
            UserKnownHostsFile /tmp/px023-hostile-config-known-hosts
            IdentitiesOnly no
        """.write(to: config, atomically: true, encoding: .utf8)

        let effective = try await Self.effectiveConfiguration(
            Self.hostileConfiguration.buildArguments(), configFile: config.path)
        #expect(effective["stricthostkeychecking"] == "true", "\(effective)")
        #expect(effective["loglevel"] == "INFO")
        #expect(effective["batchmode"] == "yes")
        #expect(effective["forwardagent"] == "no")
        #expect(effective["clearallforwardings"] == "yes")
        #expect(effective["exitonforwardfailure"] == "yes")
        #expect(effective["identitiesonly"] == "yes")
        #expect(effective["userknownhostsfile"] == "/tmp/px023-known-hosts")
        #expect(effective["connecttimeout"] == "5")
        #expect(effective["proxycommand"] == nil || effective["proxycommand"] == "none")
        #expect(effective["compression"] == "yes", "a harmless extra option still applies")
    }
}
