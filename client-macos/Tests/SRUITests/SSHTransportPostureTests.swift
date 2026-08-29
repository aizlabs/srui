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

        // §19.1 Rule 6: Fixed subsystem request, not a shell command string
        #expect(args.contains("-s"), "Expected -s flag for subsystem mode (§19.1)")
        if let sIndex = args.firstIndex(of: "-s") {
            #expect(sIndex + 1 < args.count)
            #expect(args[sIndex + 1] == "srui", "Expected fixed subsystem 'srui' following -s (§19.1)")
        }

        // Host destination must be present
        #expect(args.contains("remote.example.com"))

        // Ensure no shell command injection wrappers are present
        #expect(!args.contains("/bin/sh"))
        #expect(!args.contains("/bin/bash"))
        #expect(!args.contains("-c"))
        #expect(!args.contains("-L"))
        #expect(!args.contains("-R"))
        #expect(!args.contains("-D"))
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
}
