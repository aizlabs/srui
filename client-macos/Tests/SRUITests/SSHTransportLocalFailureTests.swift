//
// SSHTransportLocalFailureTests.swift
// SRUITests
//
// PX-023 round 2: `SSHTransport.localFailureBeforeStream()` reports `ssh`'s own failure only when
// the child exited 255 before any SRUI byte arrived. That is the only input the Connect form's
// host-key alert reads, so nothing a server sends can raise it (§19.1, §25).
//

import Foundation
import Testing
@testable import TransportSSH

// Serialized: the suite's tests share one assessed stub and its behaviour file.
@Suite("SSH Transport local failure before the stream (PX-023)", .serialized, .timeLimit(.minutes(1)))
struct SSHTransportLocalFailureTests {
    /// Runs a stub `ssh` whose body is `behavior`, drains its stream to the end, and returns the
    /// transport (still open, so its record can be read).
    private func run(_ behavior: String) async throws -> SSHTransport {
        let stub = try await SSHTestSupport.assessedStub(for: "SSHTransportLocalFailureTests")
        try behavior.write(to: URL(fileURLWithPath: stub.behavior), atomically: true, encoding: .utf8)
        let transport = SSHTransport(configuration: SSHConfiguration(
            host: "127.0.0.1", subsystem: "srui", batchMode: true, sshBinaryPath: stub.path))
        do {
            for try await _ in transport.receiveStream() {}
        } catch {
            // The exit is a failure for every non-zero status; the record is what is under test.
        }
        return transport
    }

    @Test("ssh's own exit 255 before any byte is reported with its stderr")
    func exit255BeforeStream() async throws {
        let transport = try await run("""
        printf 'No ED25519 host key is known for [127.0.0.1]:2222 and you have requested strict checking.\\nHost key verification failed.\\n' >&2
        exit 255
        """)
        // stderr is read on its own thread; it can trail the stdout end by a moment.
        try await AsyncTestSupport.eventuallyAsync(description: "stderr captured") {
            await transport.localFailureBeforeStream()?.stderr.contains("Host key verification failed.") == true
        }
        let local = try #require(await transport.localFailureBeforeStream())
        #expect(local.status == 255)
        #expect(local.stderr == """
        No ED25519 host key is known for [127.0.0.1]:2222 and you have requested strict checking.
        Host key verification failed.
        """)
        await transport.close()
    }

    @Test("an exit after the first stream byte is never ssh's own failure")
    func exitAfterStreamByte() async throws {
        let transport = try await run("""
        printf 'x'
        printf 'Host key verification failed.\\n' >&2
        exit 255
        """)
        #expect(await transport.localFailureBeforeStream() == nil)
        await transport.close()
    }

    @Test("any status but 255 is not ssh's own failure")
    func otherStatuses() async throws {
        for status in [0, 1, 254] {
            let transport = try await run("""
            printf 'Host key verification failed.\\n' >&2
            exit \(status)
            """)
            #expect(await transport.localFailureBeforeStream() == nil, "status \(status)")
            await transport.close()
        }
    }
}
