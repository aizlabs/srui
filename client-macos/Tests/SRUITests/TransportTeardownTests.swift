//
// TransportTeardownTests.swift
// SRUITests
//
// A dropped transport must terminate its receive stream instead of stranding consumers (§20.2, §22).
//

import Foundation
import Testing
import TransportSSH

@Suite("Transport Teardown Tests")
struct TransportTeardownTests {
    @Test(
        "Dropping a unix socket transport terminates its receive stream",
        .timeLimit(.minutes(1))
    )
    func droppedUnixSocketTransportFinishesReceiveStream() async {
        let path = NSTemporaryDirectory() + "srui-dropped-\(UUID().uuidString).sock"
        var transport: UnixSocketTransport? = UnixSocketTransport(socketPath: path)
        let stream = transport!.receiveStream()
        transport = nil

        await expectTerminated(stream)
    }

    @Test(
        "Dropping a TCP transport terminates its receive stream",
        .timeLimit(.minutes(1))
    )
    func droppedTCPSocketTransportFinishesReceiveStream() async {
        var transport: TCPSocketTransport? = TCPSocketTransport(host: "127.0.0.1", port: 1)
        let stream = transport!.receiveStream()
        transport = nil

        await expectTerminated(stream)
    }

    @Test(
        "Dropping an SSH transport terminates its receive stream",
        .timeLimit(.minutes(1))
    )
    func droppedSSHTransportFinishesReceiveStream() async {
        var transport: SSHTransport? = SSHTransport(
            configuration: SSHConfiguration(
                host: "127.0.0.1",
                port: 1,
                batchMode: true,
                connectTimeout: 1.0
            )
        )
        let stream = transport!.receiveStream()
        transport = nil

        await expectTerminated(stream)
    }

    private func expectTerminated(_ stream: AsyncThrowingStream<Data, Error>) async {
        var iterator = stream.makeAsyncIterator()
        do {
            let unexpectedElement = try await iterator.next()
            #expect(unexpectedElement == nil, "a dropped transport must finish without yielding data")
        } catch {
            // Error completion is also a valid terminal state; the key contract is no stranded waiter.
        }
    }
}
