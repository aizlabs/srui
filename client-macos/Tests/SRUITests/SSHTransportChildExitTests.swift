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
// closed, and as `.closed` when the child was gone before `send`. These stubs force each ordering,
// and pin what the classification must not do: call a clean exit a failure, call our own `close()`
// a failure, or outlive a cancelled send.
//

import Testing
import Foundation
@testable import TransportSSH

// Serialized: the suite's tests share one assessed stub and its behaviour file.
@Suite("SSH Transport child-exit classification (§19.1, §25)", .serialized, .timeLimit(.minutes(1)))
struct SSHTransportChildExitTests {

    /// A transport whose `ssh` closes its stdin, says so through a marker file, then runs `tail`.
    /// No polling loop in any tail: the stubs must stay idle while other suites run against
    /// deadlines.
    private func transport(
        in directory: URL,
        tail: String,
        classificationBound: Duration = SSHTransport.defaultChildExitClassificationBound
    ) async throws -> (SSHTransport, marker: String) {
        let stub = try await SSHTestSupport.assessedStub(for: "SSHTransportChildExitTests")
        let marker = directory.appendingPathComponent("stdin-closed").path
        try """
        exec 0<&-
        : > "\(marker)"
        \(tail)
        """.write(to: URL(fileURLWithPath: stub.behavior), atomically: true, encoding: .utf8)
        let configuration = SSHConfiguration(
            host: "127.0.0.1",
            subsystem: "srui",
            batchMode: true,
            sshBinaryPath: stub.path
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

    private static func sendError(_ transport: SSHTransport) async -> Error? {
        do {
            try await transport.send(data: Data("hello".utf8))
            return nil
        } catch {
            return error
        }
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

        let error = await Self.sendError(transport)
        #expect(error.flatMap(Self.connectionFailure)?.contains("exit code 255") == true,
                "got \(String(describing: error))")
        await transport.close()
    }

    @Test("A clean exit after closing stdin is not a connection failure")
    func cleanExitIsNotAConnectionFailure() async throws {
        let directory = try TestFixtureDirectory.make(prefix: "srui-exit")
        defer { TestFixtureDirectory.release(directory) }
        let (transport, marker) = try await transport(in: directory, tail: "sleep 1\nexit 0")
        _ = try await connectUntilStdinClosed(transport, marker: marker)

        // The write's own I/O error, or `.closed` once the clean exit's end of stream has closed
        // the transport (whichever the reaper and this send see first); never a failure.
        let error = await Self.sendError(transport)
        #expect(error != nil, "the write to a closed stdin fails")
        #expect(error.flatMap(Self.connectionFailure) == nil,
                "a clean exit is not a connection failure, got \(String(describing: error))")
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

        let error = await Self.sendError(transport)
        #expect(error.flatMap(Self.connectionFailure)?.contains("exit code 255") == true,
                "got \(String(describing: error))")
        await transport.close()
    }

    @Test("A send after our own close() of a running child is .closed")
    func sendAfterOurCloseIsClosed() async throws {
        let directory = try TestFixtureDirectory.make(prefix: "srui-exit")
        defer { TestFixtureDirectory.release(directory) }
        let (transport, marker) = try await transport(in: directory, tail: "exec sleep 3600")
        let stream = try await connectUntilStdinClosed(transport, marker: marker)
        await transport.close()
        // Let the reader see the child our close() terminated (SIGTERM) before sending.
        do {
            for try await _ in stream {}
        } catch {}

        let error = await Self.sendError(transport)
        guard case TransportError.closed? = error else {
            Issue.record("our own close() is not the child's failure, got \(String(describing: error))")
            return
        }
    }

    @Test("A send waiting to classify its failed write when we close() reports .closed, promptly")
    func closeDuringTheClassificationWaitIsClosed() async throws {
        let directory = try TestFixtureDirectory.make(prefix: "srui-exit")
        defer { TestFixtureDirectory.release(directory) }
        let (transport, marker) = try await transport(in: directory, tail: "exec sleep 3600")
        _ = try await connectUntilStdinClosed(transport, marker: marker)

        let clock = ContinuousClock()
        let started = clock.now
        let send = Task { await Self.sendError(transport) }
        // Whether the send is already waiting or not yet writing, our close() makes it `.closed`.
        try await Task.sleep(for: .milliseconds(300))
        await transport.close()
        let error = await send.value
        let elapsed = clock.now - started
        guard case TransportError.closed? = error else {
            Issue.record("a close() we made is not the child's failure, got \(String(describing: error))")
            return
        }
        #expect(elapsed < SSHTransport.defaultChildExitClassificationBound,
                "close() ended the wait instead of the bound (took \(elapsed))")
    }

    @Test("Cancelling a send during the classification wait ends the wait")
    func cancellingASendEndsTheWait() async throws {
        let directory = try TestFixtureDirectory.make(prefix: "srui-exit")
        defer { TestFixtureDirectory.release(directory) }
        // A bound far beyond the suite's time limit: only cancellation can end this wait in time.
        let bound = Duration.seconds(600)
        let (transport, marker) = try await transport(in: directory, tail: "exec sleep 3600", classificationBound: bound)
        _ = try await connectUntilStdinClosed(transport, marker: marker)

        let clock = ContinuousClock()
        let started = clock.now
        let send = Task { await Self.sendError(transport) }
        try await Task.sleep(for: .milliseconds(300))
        send.cancel()
        let error = await send.value
        let elapsed = clock.now - started
        #expect(elapsed < .roundTrip, "cancellation ended the wait (took \(elapsed))")
        guard case TransportError.ioError(let message)? = error else {
            Issue.record("a cancelled wait keeps the write's I/O error, got \(String(describing: error))")
            await transport.close()
            return
        }
        #expect(message.contains("Broken pipe"), "got \(message)")
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
        let error = await Self.sendError(transport)
        let elapsed = clock.now - started
        guard case TransportError.ioError(let message)? = error else {
            Issue.record("expected an I/O error, got \(String(describing: error))")
            await transport.close()
            return
        }
        #expect(message.contains("Broken pipe"), "got \(message)")
        #expect(elapsed >= bound, "the write waited for the child's exit before giving up (took \(elapsed))")
        #expect(elapsed < bound + .seconds(5), "the wait for the child's exit is bounded (took \(elapsed))")
        await transport.close()
    }
}
