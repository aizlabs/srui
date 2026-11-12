//
// SSHTransportChildExitTests.swift
// SRUITests
//
// A write that fails because the `ssh` child exited reports the child's exit, not the order in
// which the writer and the reaper happened to see it (§19.1, §25, §4 inv. 13).
//
// `SSHTransportHostKeyTests` hit this as a race: a host-key rejection came back as
// `.connectionFailed` when the child had been reaped before the write, as
// `Transport I/O error: ... write failed: Broken pipe` when the write found its stdin already
// closed, and as `.closed` when the child was gone before `send`. These stubs force each ordering.
//

import Testing
import Foundation
@testable import TransportSSH

@Suite("SSH Transport child-exit classification (§19.1, §25)", .timeLimit(.minutes(1)))
struct SSHTransportChildExitTests {

    /// A stand-in for `ssh` that closes its stdin, says so through `marker`, then runs `tail`.
    /// No polling loop: the stubs must stay idle while other suites run against deadlines.
    private func writeStub(at path: String, marker: String, tail: String) async throws {
        try await SSHTestSupport.writeAssessedStub(at: path, body: """
        exec 0<&-
        : > "\(marker)"
        \(tail)
        """)
    }

    private func transport(
        in directory: URL,
        tail: String,
        classificationBound: Duration = SSHTransport.defaultChildExitClassificationBound
    ) async throws -> (SSHTransport, marker: String) {
        let stub = directory.appendingPathComponent("ssh-stub").path
        let marker = directory.appendingPathComponent("stdin-closed").path
        try await writeStub(at: stub, marker: marker, tail: tail)
        let configuration = SSHConfiguration(
            host: "127.0.0.1",
            subsystem: "srui",
            batchMode: true,
            sshBinaryPath: stub
        )
        return (SSHTransport(configuration: configuration, childExitClassificationBound: classificationBound), marker)
    }

    /// Spawns the child (through `receiveStream()`, which connects without writing) and waits
    /// until it has closed its stdin.
    private func connectUntilStdinClosed(
        _ transport: SSHTransport,
        marker: String
    ) async throws -> AsyncThrowingStream<Data, Error> {
        let stream = transport.receiveStream()
        // Process start-up, not main-actor progress: the shared cross-task budget.
        try await AsyncTestSupport.eventuallyAsync(timeout: .roundTrip, description: "stub closed its stdin") {
            FileManager.default.fileExists(atPath: marker)
        }
        return stream
    }

    private static func connectionFailure(of error: Error) -> String? {
        guard case TransportError.connectionFailed(let message) = error else { return nil }
        return message
    }

    @Test("A write that meets EPIPE before the child is reaped reports the child's exit")
    func brokenPipeBeforeExitReportsTheExit() async throws {
        let directory = try TestFixtureDirectory.make(prefix: "srui-exit")
        defer { TestFixtureDirectory.release(directory) }
        // Still running when the write fails, exiting the way `ssh` exits on a host-key rejection.
        let (transport, marker) = try await transport(
            in: directory,
            tail: "sleep 1\necho 'Host key verification failed.' >&2\nexit 255"
        )
        _ = try await connectUntilStdinClosed(transport, marker: marker)

        let error = await #expect(throws: TransportError.self) {
            try await transport.send(data: Data("hello".utf8))
        }
        let message = error.flatMap(Self.connectionFailure)
        #expect(message?.contains("exit code 255") == true, "got \(String(describing: error))")
        await transport.close()
    }

    @Test("A send after the child has exited reports the child's exit, not a bare close")
    func sendAfterExitReportsTheExit() async throws {
        let directory = try TestFixtureDirectory.make(prefix: "srui-exit")
        defer { TestFixtureDirectory.release(directory) }
        let (transport, marker) = try await transport(in: directory, tail: "exit 255")
        let stream = try await connectUntilStdinClosed(transport, marker: marker)
        // The stream ends only after the reader has seen the child reaped, and its termination
        // closes the transport: the send below finds it closed by the child's failure.
        do {
            for try await _ in stream {}
            Issue.record("the stream must report the child's failure")
        } catch {
            #expect(Self.connectionFailure(of: error)?.contains("exit code 255") == true, "got \(error)")
        }

        let error = await #expect(throws: TransportError.self) {
            try await transport.send(data: Data("hello".utf8))
        }
        #expect(error.flatMap(Self.connectionFailure)?.contains("exit code 255") == true,
                "got \(String(describing: error))")
        await transport.close()
    }

    @Test("A broken pipe to a child that keeps running stays an I/O error, within the bound")
    func brokenPipeToARunningChildStaysAnIOError() async throws {
        let directory = try TestFixtureDirectory.make(prefix: "srui-exit")
        defer { TestFixtureDirectory.release(directory) }
        // A short bound: the child's pipes stay open while it runs, and suites that count the
        // process's pipes run alongside this one.
        let bound = Duration.milliseconds(500)
        let (transport, marker) = try await transport(in: directory, tail: "exec sleep 3600", classificationBound: bound)
        _ = try await connectUntilStdinClosed(transport, marker: marker)

        let clock = ContinuousClock()
        let started = clock.now
        let error = await #expect(throws: TransportError.self) {
            try await transport.send(data: Data("hello".utf8))
        }
        let elapsed = clock.now - started
        guard case .ioError(let message)? = error else {
            Issue.record("expected an I/O error, got \(String(describing: error))")
            await transport.close()
            return
        }
        #expect(message.contains("Broken pipe"), "got \(message)")
        #expect(elapsed >= bound, "the write waited for the child's exit before giving up (took \(elapsed))")
        #expect(elapsed < bound + .seconds(5),
                "the wait for the child's exit is bounded (took \(elapsed))")
        await transport.close()
    }
}
