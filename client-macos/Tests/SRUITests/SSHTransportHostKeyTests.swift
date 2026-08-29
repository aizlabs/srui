//
// SSHTransportHostKeyTests.swift
// SRUITests
//
// Automated test verifying that SSHTransport fails closed on host-key mismatch (§19.1, §25).
//

import Testing
import Foundation
import TransportSSH

@Suite("SSH Host Key Verification Tests (§19.1, §25)")
struct SSHTransportHostKeyTests {

    @Test("SSHTransport fails closed on host key verification mismatch")
    func badHostKeyFailsClosed() async throws {
        let tempDir = URL(fileURLWithPath: "/tmp/srui-hk-\(UUID().uuidString.prefix(8))")
        try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
        defer {
            try? FileManager.default.removeItem(at: tempDir)
        }

        let realHostKeyPath = tempDir.appendingPathComponent("real_host_key").path
        let spoofedHostKeyPath = tempDir.appendingPathComponent("spoofed_host_key").path
        let userKeyPath = tempDir.appendingPathComponent("user_key").path
        let authKeysPath = tempDir.appendingPathComponent("authorized_keys").path
        let knownHostsPath = tempDir.appendingPathComponent("known_hosts").path
        let sshdConfigPath = tempDir.appendingPathComponent("sshd_config").path

        // Generate real server host key
        let genRealHostKey = Process()
        genRealHostKey.executableURL = URL(fileURLWithPath: "/usr/bin/ssh-keygen")
        genRealHostKey.arguments = ["-t", "ed25519", "-N", "", "-f", realHostKeyPath]
        try genRealHostKey.run()
        genRealHostKey.waitUntilExit()

        // Generate a different spoofed host key to put in known_hosts
        let genSpoofHostKey = Process()
        genSpoofHostKey.executableURL = URL(fileURLWithPath: "/usr/bin/ssh-keygen")
        genSpoofHostKey.arguments = ["-t", "ed25519", "-N", "", "-f", spoofedHostKeyPath]
        try genSpoofHostKey.run()
        genSpoofHostKey.waitUntilExit()

        // Generate client user key
        let genUserKey = Process()
        genUserKey.executableURL = URL(fileURLWithPath: "/usr/bin/ssh-keygen")
        genUserKey.arguments = ["-t", "ed25519", "-N", "", "-f", userKeyPath]
        try genUserKey.run()
        genUserKey.waitUntilExit()

        // Install user key in authorized_keys
        let userPubData = try Data(contentsOf: URL(fileURLWithPath: "\(userKeyPath).pub"))
        try userPubData.write(to: URL(fileURLWithPath: authKeysPath))

        // Write intentionally MISMATCHED spoofed host key to known_hosts
        let port: UInt16 = Self.findFreePort()
        let spoofPubStr = try String(contentsOf: URL(fileURLWithPath: "\(spoofedHostKeyPath).pub"), encoding: .utf8).trimmingCharacters(in: .whitespacesAndNewlines)
        let knownHostsEntry = "[127.0.0.1]:\(port) \(spoofPubStr)\n"
        try knownHostsEntry.write(to: URL(fileURLWithPath: knownHostsPath), atomically: true, encoding: .utf8)

        // Generate sshd_config
        let configContent = """
        Port \(port)
        HostKey \(realHostKeyPath)
        AuthorizedKeysFile \(authKeysPath)
        StrictModes no
        UsePAM no
        PidFile \(tempDir.appendingPathComponent("sshd.pid").path)
        Subsystem srui /bin/echo "srui-subsystem"
        """
        try configContent.write(to: URL(fileURLWithPath: sshdConfigPath), atomically: true, encoding: .utf8)

        // Launch ephemeral sshd in single-connection debug mode (-d)
        let sshdProc = Process()
        sshdProc.executableURL = URL(fileURLWithPath: "/usr/sbin/sshd")
        sshdProc.arguments = [
            "-f", sshdConfigPath,
            "-h", realHostKeyPath,
            "-d",
            "-p", String(port)
        ]
        try sshdProc.run()
        defer {
            if sshdProc.isRunning {
                sshdProc.terminate()
            }
        }

        // Allow sshd to bind socket
        try await Task.sleep(nanoseconds: 200_000_000)

        // Connect with SSHTransport configured with mismatched known_hosts
        let sshConfig = SSHConfiguration(
            host: "127.0.0.1",
            port: port,
            subsystem: "srui",
            identityFile: userKeyPath,
            knownHostsFile: knownHostsPath,
            strictHostKeyChecking: .yes,
            batchMode: true,
            connectTimeout: 3.0
        )
        let transport = SSHTransport(configuration: sshConfig)

        var connectionFailed = false
        do {
            let stream = transport.receiveStream()
            try await transport.send(data: "test".data(using: .utf8)!)

            for try await _ in stream {
                Issue.record("Should not receive stream data on host key verification failure")
            }
        } catch let error as TransportError {
            connectionFailed = true
            // Confirm the error is connectionFailed indicating fail-closed behavior
            switch error {
            case .connectionFailed(let message):
                #expect(message.contains("exit code") || message.contains("Host key verification failed") || message.contains("status"))
            default:
                break
            }
        } catch {
            connectionFailed = true
        }

        #expect(connectionFailed, "SSHTransport must fail closed when host key verification fails (§19.1, §25)")
        await transport.close()
    }

    private static func findFreePort() -> UInt16 {
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
}
