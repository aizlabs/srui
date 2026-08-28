//
// PipeTransport.swift
// TransportSSH
//
// In-memory duplex pipe transport for unit testing and deterministic simulation (§22, §32).
//

import Foundation

/// In-memory bidirectional stream transport actor for tests and local simulation.
public actor PipeTransport: Transport {
    private var isClosed = false
    /// Weak so that `createPair` does not build a retain cycle between the two ends. The caller of
    /// `createPair` holds both, which is what keeps them alive.
    private weak var peer: PipeTransport?
    private let stream: AsyncThrowingStream<Data, Error>
    private let continuation: AsyncThrowingStream<Data, Error>.Continuation

    public init() {
        let (stream, continuation) = AsyncThrowingStream<Data, Error>.makeStream()
        self.stream = stream
        self.continuation = continuation

        continuation.onTermination = { [weak self] _ in
            Task {
                await self?.close()
            }
        }
    }

    /// Creates a connected bidirectional pair of in-memory pipe transports.
    public static func createPair() async -> (client: PipeTransport, server: PipeTransport) {
        let client = PipeTransport()
        let server = PipeTransport()
        await client.setPeer(server)
        await server.setPeer(client)
        return (client, server)
    }

    public func setPeer(_ peer: PipeTransport) {
        self.peer = peer
    }

    public func send(data: Data) async throws {
        guard !isClosed else {
            throw TransportError.closed
        }
        guard let targetPeer = peer else {
            throw TransportError.closed
        }
        await targetPeer.receiveIncoming(data: data)
    }

    public nonisolated func receiveStream() -> AsyncThrowingStream<Data, Error> {
        stream
    }

    public func receiveIncoming(data: Data) {
        guard !isClosed else { return }
        continuation.yield(data)
    }

    public func close() async {
        guard !isClosed else { return }
        isClosed = true
        continuation.finish()
        let targetPeer = peer
        peer = nil
        if let targetPeer {
            await targetPeer.close()
        }
    }
}
