//
// TransportTests.swift
// SRUITests
//
// Unit tests for Transport adapters (§19, §20.2, §22).
//

import Testing
import Foundation
import TransportSSH

/// Minimal test double that implements only the required classified-send API.
private actor ClassifiedOnlyTransport: Transport {
    private var logicalClasses: [LogicalChannelClass] = []

    func send(data: Data, logicalClass: LogicalChannelClass) async throws {
        _ = data
        logicalClasses.append(logicalClass)
    }

    var recordedLogicalClasses: [LogicalChannelClass] {
        logicalClasses
    }

    nonisolated func receiveStream() -> AsyncThrowingStream<Data, Error> {
        AsyncThrowingStream { continuation in
            continuation.finish()
        }
    }

    func close() async {}
}

@Suite("Transport Tests")
struct TransportTests {

    @Test("Unclassified sends default to control without erasing explicit classes")
    func unclassifiedSendDefaultsToControl() async throws {
        let transport = ClassifiedOnlyTransport()
        try await transport.send(data: Data([0x01]), logicalClass: .input)
        try await transport.send(data: Data([0x02]))

        let logicalClasses = await transport.recordedLogicalClasses
        #expect(logicalClasses == [.input, .control])
    }

    @Test("PipeTransport bidirectional in-memory message delivery")
    func pipeTransportBidirectionalDelivery() async throws {
        let (client, server) = await PipeTransport.createPair()

        let clientPayload = "hello from client".data(using: .utf8)!
        let serverPayload = "hello from server".data(using: .utf8)!

        let serverStream = server.receiveStream()
        let clientStream = client.receiveStream()

        try await client.send(data: clientPayload)
        try await server.send(data: serverPayload)

        var serverReceived: Data?
        for try await chunk in serverStream {
            serverReceived = chunk
            break
        }

        var clientReceived: Data?
        for try await chunk in clientStream {
            clientReceived = chunk
            break
        }

        #expect(serverReceived == clientPayload)
        #expect(clientReceived == serverPayload)

        await client.close()
        await server.close()
    }

    @Test("UnixSocketTransport local communication")
    func unixSocketTransportLocalLoopback() async throws {
        let tempSocketPath = "/tmp/test-srui-transport-\(UUID().uuidString).sock"
        defer {
            unlink(tempSocketPath)
        }

        // Create a basic Unix domain server socket
        let serverFD = Darwin.socket(AF_UNIX, SOCK_STREAM, 0)
        try #require(serverFD >= 0)
        defer {
            Darwin.close(serverFD)
        }

        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)
        tempSocketPath.utf8CString.withUnsafeBytes { src in
            withUnsafeMutablePointer(to: &addr.sun_path) { dst in
                UnsafeMutableRawPointer(dst).copyMemory(from: src.baseAddress!, byteCount: src.count)
            }
        }

        let addrLen = socklen_t(MemoryLayout<sockaddr_un>.size)
        let bindRes = withUnsafePointer(to: &addr) { addrPtr in
            addrPtr.withMemoryRebound(to: sockaddr.self, capacity: 1) { sa in
                Darwin.bind(serverFD, sa, addrLen)
            }
        }
        try #require(bindRes == 0)
        try #require(Darwin.listen(serverFD, 5) == 0)

        // Connect with UnixSocketTransport
        let transport = UnixSocketTransport(socketPath: tempSocketPath)
        let receiveStream = transport.receiveStream()

        // Accept connection on server
        let clientFD = Darwin.accept(serverFD, nil, nil)
        try #require(clientFD >= 0)
        defer {
            Darwin.close(clientFD)
        }

        // Send data from client to server
        let testMsg = "hello from unix socket client".data(using: .utf8)!
        try await transport.send(data: testMsg)

        var serverBuffer = [UInt8](repeating: 0, count: 1024)
        let bytesRead = Darwin.read(clientFD, &serverBuffer, serverBuffer.count)
        #expect(bytesRead == testMsg.count)
        #expect(Data(serverBuffer[0..<bytesRead]) == testMsg)

        // Send data from server to client
        let responseMsg = "hello from unix socket server".data(using: .utf8)!
        responseMsg.withUnsafeBytes { raw in
            _ = Darwin.write(clientFD, raw.baseAddress!, raw.count)
        }

        var clientReceived: Data?
        for try await chunk in receiveStream {
            clientReceived = chunk
            break
        }

        #expect(clientReceived == responseMsg)
        await transport.close()
    }
}
