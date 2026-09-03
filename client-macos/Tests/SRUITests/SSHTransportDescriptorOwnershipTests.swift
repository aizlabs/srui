//
// SSHTransportDescriptorOwnershipTests.swift
// SRUITests
//
// Verifies that SSHTransport owns private duplicates of the child's pipe descriptors and releases
// every one of them on close, so teardown can neither double-close a descriptor nor strand a
// reader thread on one (§19.1, §22.2).
//

import Testing
import Foundation
import TransportSSH

@Suite("SSH Transport Descriptor Ownership (§22.2)", .serialized)
struct SSHTransportDescriptorOwnershipTests {

    /// Inodes of every FIFO this process currently has open.
    ///
    /// Identity, not a count: the suite runs alongside others that open and close descriptors of
    /// their own, so a process-wide tally is noise. A pipe's inode is stable and unique while it
    /// exists, which makes "the pipes this transport created" a set we can name and then require
    /// to be empty after `close()`.
    private func openPipeInodes() -> Set<UInt64> {
        var inodes: Set<UInt64> = []
        for fd in Int32(0)..<Int32(1024) {
            var info = stat()
            guard fstat(fd, &info) == 0 else { continue }
            if info.st_mode & S_IFMT == S_IFIFO {
                inodes.insert(UInt64(info.st_ino))
            }
        }
        return inodes
    }

    /// Writes a stand-in for `ssh` that ignores the posture flags, chatters on stderr, and stays
    /// alive until it is terminated — i.e. a child that will not close the pipes for us.
    ///
    /// `exec sleep` rather than a polling loop: the stub must be idle, because these tests run
    /// alongside suites whose deadlines are measured on a shared executor.
    private func writeStubSSH(at path: String) throws {
        let script = """
        #!/bin/sh
        echo "stub ssh started" >&2
        exec sleep 3600
        """
        try script.write(to: URL(fileURLWithPath: path), atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes(
            [.posixPermissions: 0o755],
            ofItemAtPath: path
        )
    }

    private func stubConfiguration(in tempDir: URL) throws -> SSHConfiguration {
        let stubPath = tempDir.appendingPathComponent("ssh-stub").path
        try writeStubSSH(at: stubPath)
        return SSHConfiguration(
            host: "127.0.0.1",
            subsystem: "srui",
            batchMode: true,
            sshBinaryPath: stubPath
        )
    }

    /// Waits, bounded, for `inodes` to drain — a reader thread performs its release on its own
    /// thread after `close()` has returned.
    private func waitForRelease(of inodes: Set<UInt64>) async throws -> Set<UInt64> {
        var remaining = openPipeInodes().intersection(inodes)
        for _ in 0..<60 where !remaining.isEmpty {
            try await Task.sleep(nanoseconds: 50_000_000)
            remaining = openPipeInodes().intersection(inodes)
        }
        return remaining
    }

    @Test("close() releases every pipe it opened and stops both reader threads")
    func closeReleasesEveryPipeItOpened() async throws {
        let tempDir = URL(fileURLWithPath: "/tmp/srui-fd-\(UUID().uuidString.prefix(8))")
        try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tempDir) }

        let configuration = try stubConfiguration(in: tempDir)
        let before = openPipeInodes()

        let transport = SSHTransport(configuration: configuration)
        _ = transport.receiveStream()

        // Forces `connect()`, which spawns the child and latches its three pipe duplicates.
        try await transport.send(data: Data("probe".utf8))

        let ours = openPipeInodes().subtracting(before)
        #expect(
            ours.count >= 3,
            "connect() must open the three child pipes, otherwise this test proves nothing (saw \(ours.count))"
        )

        // The stub never exits on its own: if `close()` depended on the child closing the pipes,
        // or on a reader thread noticing EOF, it would hang here.
        await transport.close()

        let leaked = try await waitForRelease(of: ours)
        #expect(
            leaked.isEmpty,
            "close() must release every pipe it opened; still open: \(leaked.sorted())"
        )
    }

    @Test("repeated connect/close cycles do not accumulate pipes")
    func repeatedCyclesDoNotAccumulatePipes() async throws {
        let tempDir = URL(fileURLWithPath: "/tmp/srui-fd-\(UUID().uuidString.prefix(8))")
        try FileManager.default.createDirectory(at: tempDir, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: tempDir) }

        let configuration = try stubConfiguration(in: tempDir)

        for cycle in 0..<4 {
            let before = openPipeInodes()

            let transport = SSHTransport(configuration: configuration)
            _ = transport.receiveStream()
            try await transport.send(data: Data("probe".utf8))
            let ours = openPipeInodes().subtracting(before)
            await transport.close()

            let leaked = try await waitForRelease(of: ours)
            #expect(
                leaked.isEmpty,
                "cycle \(cycle): pipes survived teardown: \(leaked.sorted())"
            )
        }
    }
}
