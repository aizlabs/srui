//
// TransportTeardownTests.swift
// SRUITests
//
// A dropped transport must terminate its receive stream instead of stranding consumers (§20.2, §22).
//

import Testing
import Foundation
import TransportSSH

@Suite("Transport Teardown Tests")
struct TransportTeardownTests {

    @Test("Dropping a unix socket transport terminates its receive stream")
    func droppedUnixSocketTransportFinishesReceiveStream() async {
        let path = NSTemporaryDirectory() + "srui-dropped-\(UUID().uuidString).sock"
        var transport: UnixSocketTransport? = UnixSocketTransport(socketPath: path)
        let stream = transport!.receiveStream()
        transport = nil

        #expect(await streamTerminates(stream, within: 2.0))
    }

    @Test("Dropping a TCP transport terminates its receive stream")
    func droppedTCPSocketTransportFinishesReceiveStream() async {
        var transport: TCPSocketTransport? = TCPSocketTransport(host: "127.0.0.1", port: 1)
        let stream = transport!.receiveStream()
        transport = nil

        #expect(await streamTerminates(stream, within: 2.0))
    }

    private func streamTerminates(
        _ stream: AsyncThrowingStream<Data, Error>,
        within seconds: Double
    ) async -> Bool {
        await withTaskGroup(of: Bool.self) { group in
            group.addTask {
                do {
                    for try await _ in stream {}
                } catch {
                    // A thrown termination still terminates the stream.
                }
                return true
            }
            group.addTask {
                try? await Task.sleep(nanoseconds: UInt64(seconds * 1_000_000_000))
                return false
            }
            let first = await group.next() ?? false
            group.cancelAll()
            return first
        }
    }
}
