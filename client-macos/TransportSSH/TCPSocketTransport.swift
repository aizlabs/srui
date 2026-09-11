//
// TCPSocketTransport.swift
// TransportSSH
//
// POSIX TCP stream socket transport adapter (§20.2, §22).
//

import Foundation
#if canImport(Darwin)
import Darwin
#elseif canImport(Glibc)
import Glibc
#endif

/// Transport adapter actor communicating over a TCP stream socket (e.g. `127.0.0.1:port`, §20.2).
public actor TCPSocketTransport: Transport {
    public let host: String
    public let port: UInt16

    private var socketFD: Int32 = -1
    private var isClosed = false
    private var readThread: Thread?
    private var readLatch = SocketReadLatch()
    /// Writes run off the actor so `close()` can preempt a peer that stopped reading (§22.2).
    private let writer = SocketWriter(label: "org.srui.TCPSocketTransport.write")
    /// Bounds inbound read-ahead so a stalled consumer cannot grow memory without limit (§26).
    private let backlogGate = InboundBacklogGate()
    private let stream: AsyncThrowingStream<Data, Error>
    private let continuation: AsyncThrowingStream<Data, Error>.Continuation

    public init(host: String = "127.0.0.1", port: UInt16) {
        self.host = host
        self.port = port
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
        // The latch owns the descriptor and closes it exactly once, deferring to whichever of the
        // reader thread or writer queue still holds a claim.
        writer.stop()
        readLatch.stop()
        backlogGate.release()
        // Otherwise a consumer still iterating `receiveStream()` would hang forever. Drop the
        // termination handler first: it captures `self` weakly, and forming that reference while
        // the actor is mid-deallocation traps.
        continuation.onTermination = nil
        continuation.finish()
    }

    /// Establishes the TCP socket connection to the server.
    public func connect() throws {
        // A closed transport stays closed: its stream continuation is already finished, so silently
        // reconnecting here would open a socket whose every inbound byte is discarded.
        guard !isClosed else {
            throw TransportError.closed
        }
        guard socketFD == -1 else {
            return
        }

        let fd = Darwin.socket(AF_INET, SOCK_STREAM, 0)
        guard fd >= 0 else {
            throw TransportError.connectionFailed("Failed to create TCP socket: \(String(cString: strerror(errno)))")
        }

        var addr = sockaddr_in()
        addr.sin_family = sa_family_t(AF_INET)
        addr.sin_port = port.bigEndian

        let inetResult = inet_pton(AF_INET, host, &addr.sin_addr)
        guard inetResult == 1 else {
            Darwin.close(fd)
            throw TransportError.connectionFailed("Invalid IP address: \(host)")
        }

        let addrLen = socklen_t(MemoryLayout<sockaddr_in>.size)
        let connectResult = withUnsafePointer(to: &addr) { addrPtr in
            addrPtr.withMemoryRebound(to: sockaddr.self, capacity: 1) { saPtr in
                Darwin.connect(fd, saPtr, addrLen)
            }
        }

        guard connectResult == 0 else {
            let err = errno
            Darwin.close(fd)
            throw TransportError.connectionFailed("Failed to connect to \(host):\(port): \(String(cString: strerror(err)))")
        }

        self.socketFD = fd
        readLatch.adopt(descriptor: fd)
        startReadingLoop()
    }

    public func send(data: Data, logicalClass: LogicalChannelClass) async throws {
        guard !isClosed else {
            throw TransportError.closed
        }
        if socketFD == -1 {
            try connect()
        }
        guard socketFD >= 0 else {
            throw TransportError.closed
        }

        // Off the actor executor: see `SocketWriter` for why a blocking write here would make
        // `close()` unreachable while a peer is not draining its buffer (§22.2).
        try await writer.write(data, logicalClass: logicalClass, claiming: readLatch)
    }

    public func acknowledgeReceived(byteCount: Int) async {
        backlogGate.recordConsumed(byteCount)
    }

    /// Bytes read from the socket that the consumer has not acknowledged yet (§26).
    public var pendingInboundBytes: Int {
        backlogGate.outstandingBytes
    }

    public nonisolated func receiveStream() -> AsyncThrowingStream<Data, Error> {
        Task { [weak self] in
            try? await self?.ensureConnected()
        }
        return stream
    }

    private func ensureConnected() throws {
        guard !isClosed else {
            throw TransportError.closed
        }
        if socketFD == -1 {
            try connect()
        }
    }

    /// Runs the blocking `read(2)` loop on a dedicated thread rather than a `Task.detached`, which
    /// would park one of the cooperative pool's threads for the lifetime of the connection (§22.2).
    private func startReadingLoop() {
        guard readThread == nil else { return }
        let cont = self.continuation
        let latch = self.readLatch

        let gate = self.backlogGate

        let thread = Thread {
            var buffer = [UInt8](repeating: 0, count: 65536)

            readLoop: while true {
                // §26: stop pulling from the socket while the consumer is behind, rather than
                // buffering committed transactions without bound.
                guard gate.waitForCapacity() else { break readLoop }

                // The latch hands out the descriptor only while it is guaranteed open, so the
                // number can never be recycled underneath this read.
                switch readAvailable(from: latch, into: &buffer) {
                case .bytes(let count):
                    gate.recordDelivered(count)
                    cont.yield(Data(buffer[0..<count]))
                case .retry:
                    continue readLoop
                case .stopped:
                    break readLoop
                case .endOfStream:
                    cont.finish()
                    return
                case .failed(let err):
                    if err == EBADF || err == ECONNRESET || err == ENOTCONN {
                        cont.finish()
                    } else {
                        cont.finish(throwing: TransportError.ioError(
                            "TCP read failed: \(String(cString: strerror(err)))"
                        ))
                    }
                    return
                }
            }
            cont.finish()
        }
        thread.name = "org.srui.TCPSocketTransport.read"
        thread.stackSize = 512 * 1024
        self.readThread = thread
        thread.start()
    }

    public func close() async {
        guard !isClosed else { return }
        isClosed = true

        // The latch stops both loops, shuts the socket down to wake a blocked `read` or `write`,
        // and closes the descriptor — or hands that close to whichever side still holds a claim.
        // Releasing the gate additionally wakes a reader parked on consumer backpressure, so
        // teardown cannot deadlock on it.
        writer.stop()
        readLatch.stop()
        backlogGate.release()
        readThread = nil
        socketFD = -1
        continuation.finish()
    }
}
