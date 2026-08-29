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
    /// The peer's continuation, held strongly: only this end feeds the peer's stream, so releasing
    /// this end must be able to hand the survivor an EOF even after `peer` has been zeroed.
    private var peerContinuation: AsyncThrowingStream<Data, Error>.Continuation?
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

    deinit {
        // `peer` is weak, so releasing one end no longer tears the other down through `close()`.
        // Without finishing here, a consumer iterating either side's `receiveStream()` would never
        // observe EOF and would hang forever. Drop the termination handler first: it captures
        // `self` weakly, and forming that reference mid-deallocation traps.
        continuation.onTermination = nil
        continuation.finish()
        peerContinuation?.finish()
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
        self.peerContinuation = peer.streamContinuation
    }

    /// The continuation feeding this end's `receiveStream()`. Shared with the peer so either side
    /// can terminate the other's stream when it goes away.
    nonisolated var streamContinuation: AsyncThrowingStream<Data, Error>.Continuation {
        continuation
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
        peerContinuation = nil
        if let targetPeer {
            await targetPeer.close()
        }
    }
}
