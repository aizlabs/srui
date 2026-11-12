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

    /// The kernel pipe handle and its peer for one descriptor of `pid`, or `nil` if it is no pipe.
    private func pipeEnd(of pid: pid_t, fd: Int32) -> (handle: UInt64, peer: UInt64)? {
        var info = pipe_fdinfo()
        let size = Int32(MemoryLayout<pipe_fdinfo>.size)
        guard proc_pidfdinfo(pid, fd, PROC_PIDFDPIPEINFO, &info, size) == size else { return nil }
        return (info.pipeinfo.pipe_handle, info.pipeinfo.pipe_peerhandle)
    }

    /// Handles of every pipe end this process currently has open.
    private func openPipeHandles() -> Set<UInt64> {
        var handles: Set<UInt64> = []
        for fd in Int32(0)..<Int32(1024) {
            if let end = pipeEnd(of: getpid(), fd: fd) {
                handles.insert(end.handle)
            }
        }
        return handles
    }

    /// The parent-side ends of the stub child's stdin, stdout and stderr: the peers of the child's
    /// own descriptors 0, 1 and 2.
    ///
    /// Identity taken from the child, not "every pipe opened since": suites that run alongside this
    /// one open and close pipes of their own, and a before/after difference counted those as this
    /// transport's — a child-exit test holding its stub's pipes for a second failed this suite
    /// 9 times in 30 under load.
    private func transportPipeHandles(childPIDFile: String) async throws -> Set<UInt64> {
        try await AsyncTestSupport.eventuallyAsync(timeout: .roundTrip, description: "stub wrote its pid") {
            (try? String(contentsOfFile: childPIDFile, encoding: .utf8))?.hasSuffix("\n") == true
        }
        let text = try String(contentsOfFile: childPIDFile, encoding: .utf8)
        let pid = try #require(pid_t(text.trimmingCharacters(in: .whitespacesAndNewlines)))
        return Set((Int32(0)...2).compactMap { pipeEnd(of: pid, fd: $0)?.peer })
    }

    /// Writes a stand-in for `ssh` that ignores the posture flags, chatters on stderr, and stays
    /// alive until it is terminated — i.e. a child that will not close the pipes for us.
    ///
    /// `exec sleep` rather than a polling loop: the stub must be idle, because these tests run
    /// alongside suites whose deadlines are measured on a shared executor.
    private func writeStubSSH(at path: String, pidFile: String) async throws {
        try await SSHTestSupport.writeAssessedStub(at: path, body: """
        echo "stub ssh started" >&2
        echo $$ > "\(pidFile)"
        exec sleep 3600
        """)
    }

    private func stubConfiguration(in tempDir: URL, pidFile: String) async throws -> SSHConfiguration {
        let stubPath = tempDir.appendingPathComponent("ssh-stub").path
        try await writeStubSSH(at: stubPath, pidFile: pidFile)
        return SSHConfiguration(
            host: "127.0.0.1",
            subsystem: "srui",
            batchMode: true,
            sshBinaryPath: stubPath
        )
    }

    /// Waits, bounded, for `handles` to drain — a reader thread performs its release on its own
    /// thread after `close()` has returned.
    private func waitForRelease(of handles: Set<UInt64>) async throws -> Set<UInt64> {
        var remaining = openPipeHandles().intersection(handles)
        for _ in 0..<60 where !remaining.isEmpty {
            try await Task.sleep(nanoseconds: 50_000_000)
            remaining = openPipeHandles().intersection(handles)
        }
        return remaining
    }

    @Test("close() releases every pipe it opened and stops both reader threads")
    func closeReleasesEveryPipeItOpened() async throws {
        let tempDir = try TestFixtureDirectory.make(prefix: "srui-fd")
        defer { TestFixtureDirectory.release(tempDir) }

        let pidFile = tempDir.appendingPathComponent("stub.pid").path
        let configuration = try await stubConfiguration(in: tempDir, pidFile: pidFile)

        let transport = SSHTransport(configuration: configuration)
        _ = transport.receiveStream()

        // Forces `connect()`, which spawns the child and latches its three pipe duplicates.
        try await transport.send(data: Data("probe".utf8))

        let ours = try await transportPipeHandles(childPIDFile: pidFile)
        let held = openPipeHandles().intersection(ours)
        #expect(
            ours.count == 3 && held == ours,
            "connect() must hold the three child pipes, otherwise this test proves nothing (held \(held.count) of \(ours.count))"
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
        let tempDir = try TestFixtureDirectory.make(prefix: "srui-fd")
        defer { TestFixtureDirectory.release(tempDir) }

        let pidFile = tempDir.appendingPathComponent("stub.pid").path
        let configuration = try await stubConfiguration(in: tempDir, pidFile: pidFile)

        for cycle in 0..<4 {
            try? FileManager.default.removeItem(atPath: pidFile)
            let transport = SSHTransport(configuration: configuration)
            _ = transport.receiveStream()
            try await transport.send(data: Data("probe".utf8))
            let ours = try await transportPipeHandles(childPIDFile: pidFile)
            #expect(ours.count == 3, "cycle \(cycle): the stub's three pipes were found")
            await transport.close()

            let leaked = try await waitForRelease(of: ours)
            #expect(
                leaked.isEmpty,
                "cycle \(cycle): pipes survived teardown: \(leaked.sorted())"
            )
        }
    }
}
