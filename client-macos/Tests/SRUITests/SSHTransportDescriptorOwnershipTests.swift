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

// Serialized: the suite's tests share one assessed stub and its behaviour file.
@Suite("SSH Transport Descriptor Ownership (§22.2)", .serialized, .timeLimit(.minutes(1)))
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

    /// The stub child, alive until `kill()`: it ignores the SIGTERM `close()` sends.
    ///
    /// Why it must outlive `close()`: a released pipe end's kernel handle is reused by the next
    /// `pipe()` anywhere in the process (a verifier saw it in 10 of 10 cycles), so "is this handle
    /// still open here?" could count another suite's new pipe as a leak. While the child holds
    /// its ends, the pipe objects — and so the handles of our ends — stay allocated and cannot be
    /// handed out again; an open handle in this set is then ours and nothing else's.
    private struct StubChild {
        let pid: pid_t
        /// The parent-side ends of the child's stdin, stdout and stderr: the peers of the
        /// child's own descriptors 0, 1 and 2.
        let parentEnds: Set<UInt64>

        func kill() {
            _ = Darwin.kill(pid, SIGKILL)
        }
    }

    private func stubConfiguration(pidFile: String) async throws -> SSHConfiguration {
        let stub = try await SSHTestSupport.assessedStub(for: "SSHTransportDescriptorOwnershipTests")
        // Chatters on stderr, ignores SIGTERM (inherited across `exec`), and stays alive until
        // killed — a child that will not close the pipes for us. `exec sleep` rather than a
        // polling loop: the stub must be idle, because these tests run alongside suites whose
        // deadlines are measured on a shared executor.
        try """
        echo "stub ssh started" >&2
        trap '' TERM
        echo $$ > "\(pidFile)"
        exec sleep 3600
        """.write(to: URL(fileURLWithPath: stub.behavior), atomically: true, encoding: .utf8)
        return SSHConfiguration(
            host: "127.0.0.1",
            subsystem: "srui",
            batchMode: true,
            sshBinaryPath: stub.path
        )
    }

    private func stubChild(pidFile: String) async throws -> StubChild {
        try await AsyncTestSupport.eventuallyAsync(timeout: .roundTrip, description: "stub wrote its pid") {
            (try? String(contentsOfFile: pidFile, encoding: .utf8))?.hasSuffix("\n") == true
        }
        let text = try String(contentsOfFile: pidFile, encoding: .utf8)
        let pid = try #require(pid_t(text.trimmingCharacters(in: .whitespacesAndNewlines)))
        return StubChild(pid: pid, parentEnds: Set((Int32(0)...2).compactMap { pipeEnd(of: pid, fd: $0)?.peer }))
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
        let configuration = try await stubConfiguration(pidFile: pidFile)

        let transport = SSHTransport(configuration: configuration)
        _ = transport.receiveStream()

        // Forces `connect()`, which spawns the child and latches its three pipe duplicates.
        try await transport.send(data: Data("probe".utf8))

        let child = try await stubChild(pidFile: pidFile)
        defer { child.kill() }
        let held = openPipeHandles().intersection(child.parentEnds)
        #expect(
            child.parentEnds.count == 3 && held == child.parentEnds,
            "connect() must hold the three child pipes, otherwise this test proves nothing (held \(held.count) of \(child.parentEnds.count))"
        )

        // The stub ignores the SIGTERM close() sends and never closes the pipes: if `close()`
        // depended on the child closing them, or on a reader thread noticing EOF, they would leak.
        await transport.close()

        let leaked = try await waitForRelease(of: child.parentEnds)
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
        let configuration = try await stubConfiguration(pidFile: pidFile)

        for cycle in 0..<4 {
            try? FileManager.default.removeItem(atPath: pidFile)
            let transport = SSHTransport(configuration: configuration)
            _ = transport.receiveStream()
            try await transport.send(data: Data("probe".utf8))
            let child = try await stubChild(pidFile: pidFile)
            defer { child.kill() }
            #expect(child.parentEnds.count == 3, "cycle \(cycle): the stub's three pipes were found")
            await transport.close()

            let leaked = try await waitForRelease(of: child.parentEnds)
            #expect(
                leaked.isEmpty,
                "cycle \(cycle): pipes survived teardown: \(leaked.sorted())"
            )
        }
    }
}
