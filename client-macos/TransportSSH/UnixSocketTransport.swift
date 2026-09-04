//
// UnixSocketTransport.swift
// TransportSSH
//
// POSIX Unix Domain Socket transport adapter for local sessiond connections (§20.1, §20.2, §22).
//

import Foundation
#if canImport(Darwin)
import Darwin
#elseif canImport(Glibc)
import Glibc
#endif

/// Owns a descriptor used concurrently by a blocking reader thread and a serial writer queue, and
/// closes it exactly once.
///
/// A stop flag alone is not enough: `while !isStopped { read(fd) }` is check-then-act, so the
/// reader can pass the check, be descheduled while `close()` runs to completion, and then call
/// `read` on a descriptor number the kernel has already recycled to an unrelated `open` elsewhere
/// in the process — silently consuming another owner's bytes. The latch therefore keeps the
/// descriptor: a stop requested while an I/O call is in flight only shuts the socket down (to wake
/// it) and defers the `close(2)` to the last claim holder, so the number can never be recycled
/// while the reader or writer might still touch it.
///
/// Claims are counted rather than boolean because reads and writes overlap: the reader parks in
/// `read(2)` for the whole connection while the writer issues independent `write(2)` calls.
final class SocketReadLatch: @unchecked Sendable {
    private let lock = NSLock()
    private var _isStopped = false
    private var fd: Int32 = -1
    private var activeClaims = 0

    /// Transfers ownership of `descriptor` to the latch. After this only the latch closes it.
    ///
    /// Two descriptor flags are set here because both are load-bearing for teardown (§22.2):
    ///
    /// - `F_SETNOSIGPIPE`: a write to a descriptor whose peer has gone away must return `EPIPE`,
    ///   not raise SIGPIPE and take the process down. `stop()` deliberately shuts the descriptor
    ///   down underneath an in-flight write — that is the mechanism by which `close()` preempts a
    ///   peer that stopped reading — so this is a normal path, not an exceptional one.
    /// - `O_NONBLOCK`: a blocking `write(2)` cannot honour a deadline, because a single call parks
    ///   inside the kernel until the peer drains. Non-blocking I/O plus an explicit `poll(2)` wait
    ///   is what lets both the reader and the writer re-check the stop flag and the deadline while
    ///   a peer is unresponsive.
    func adopt(descriptor: Int32) {
        _ = fcntl(descriptor, F_SETNOSIGPIPE, 1)
        let flags = fcntl(descriptor, F_GETFL, 0)
        if flags >= 0 {
            _ = fcntl(descriptor, F_SETFL, flags | O_NONBLOCK)
        }
        lock.lock()
        fd = descriptor
        lock.unlock()
    }

    /// Whether teardown has been requested.
    var isStopped: Bool {
        lock.lock()
        defer { lock.unlock() }
        return _isStopped
    }

    /// Claims the descriptor for one I/O call, or returns `nil` once stopped.
    func beginIO() -> Int32? {
        lock.lock()
        defer { lock.unlock() }
        guard !_isStopped, fd >= 0 else { return nil }
        activeClaims += 1
        return fd
    }

    /// Releases one claim, performing the deferred close if a stop landed while it was held.
    func endIO() {
        lock.lock()
        activeClaims = Swift.max(0, activeClaims - 1)
        let doomed = (_isStopped && activeClaims == 0) ? fd : -1
        if doomed >= 0 { fd = -1 }
        lock.unlock()

        if doomed >= 0 {
            Darwin.close(doomed)
        }
    }

    func beginRead() -> Int32? { beginIO() }

    func endRead() { endIO() }

    /// Stops all I/O, wakes a blocked `read(2)` or `write(2)` by shutting the socket down, and
    /// releases the descriptor unless a claim is outstanding, in which case `endIO()` closes it.
    func stop() {
        lock.lock()
        _isStopped = true
        let current = fd
        let busy = activeClaims > 0
        if current >= 0 && !busy { fd = -1 }
        lock.unlock()

        guard current >= 0 else { return }
        Darwin.shutdown(current, SHUT_RDWR)
        if !busy {
            Darwin.close(current)
        }
    }
}

/// Transport adapter actor communicating with a local Unix domain socket daemon (`srui-sessiond`, §20.2).
public actor UnixSocketTransport: Transport {
    public let socketPath: String

    private var socketFD: Int32 = -1
    private var isClosed = false
    private var readThread: Thread?
    private var readLatch = SocketReadLatch()
    /// Writes run off the actor so `close()` can preempt a peer that stopped reading (§22.2).
    private let writer = SocketWriter(label: "org.srui.UnixSocketTransport.write")
    /// Bounds inbound read-ahead so a stalled consumer cannot grow memory without limit (§26).
    private let backlogGate = InboundBacklogGate()
    private let stream: AsyncThrowingStream<Data, Error>
    private let continuation: AsyncThrowingStream<Data, Error>.Continuation

    public init(socketPath: String) {
        self.socketPath = socketPath
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

    /// Establishes the Unix domain socket connection to the server.
    public func connect() throws {
        // A closed transport stays closed: its stream continuation is already finished, so silently
        // reconnecting here would open a socket whose every inbound byte is discarded.
        guard !isClosed else {
            throw TransportError.closed
        }
        guard socketFD == -1 else {
            return
        }

        let fd = Darwin.socket(AF_UNIX, SOCK_STREAM, 0)
        guard fd >= 0 else {
            throw TransportError.connectionFailed("Failed to create socket: \(String(cString: strerror(errno)))")
        }

        var addr = sockaddr_un()
        addr.sun_family = sa_family_t(AF_UNIX)

        let pathBytes = socketPath.utf8CString
        let maxPathLen = MemoryLayout.size(ofValue: addr.sun_path)

        guard pathBytes.count <= maxPathLen else {
            Darwin.close(fd)
            throw TransportError.connectionFailed("Socket path length (\(pathBytes.count) bytes) exceeds maximum allowable length (\(maxPathLen) bytes)")
        }

        withUnsafeMutablePointer(to: &addr.sun_path) { sunPathPtr in
            let rawPtr = UnsafeMutableRawPointer(sunPathPtr)
            pathBytes.withUnsafeBytes { srcBytes in
                if let base = srcBytes.baseAddress {
                    rawPtr.copyMemory(from: base, byteCount: srcBytes.count)
                }
            }
        }

        let addrLen = socklen_t(MemoryLayout<sockaddr_un>.size)
        let connectResult = withUnsafePointer(to: &addr) { addrPtr in
            addrPtr.withMemoryRebound(to: sockaddr.self, capacity: 1) { saPtr in
                Darwin.connect(fd, saPtr, addrLen)
            }
        }

        guard connectResult == 0 else {
            let err = errno
            Darwin.close(fd)
            throw TransportError.connectionFailed("Failed to connect to \(socketPath): \(String(cString: strerror(err)))")
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

        // Handed to the serial writer queue rather than performed here: a blocking `write(2)` on
        // the actor executor holds this actor for its whole duration, and `close()` is
        // actor-isolated, so a peer that stops reading would make teardown unreachable (§22.2).
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

    /// Runs the blocking `read(2)` loop on a dedicated thread.
    ///
    /// This deliberately does not use `Task.detached`: a task parked in a blocking syscall occupies
    /// one of the Swift cooperative pool's threads (sized to the core count) for the whole lifetime
    /// of the connection, starving the same pool that decodes and applies transactions (§22.2).
    private func startReadingLoop() {
        guard readThread == nil else { return }
        let cont = self.continuation
        let latch = self.readLatch

        let gate = self.backlogGate

        let thread = Thread {
            var buffer = [UInt8](repeating: 0, count: 65536)

            readLoop: while true {
                // §26: stop pulling from the socket while the consumer is behind. Committed
                // transactions may not be dropped, so the bound has to be real backpressure.
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
                            "Socket read failed: \(String(cString: strerror(err)))"
                        ))
                    }
                    return
                }
            }
            cont.finish()
        }
        thread.name = "org.srui.UnixSocketTransport.read"
        thread.stackSize = 512 * 1024
        self.readThread = thread
        thread.start()
    }

    public func close() async {
        guard !isClosed else { return }
        isClosed = true

        // The latch stops both loops, shuts the socket down to wake a blocked `read` or `write`,
        // and closes the descriptor — or hands that close to whichever side still holds a claim.
        // That ordering is what makes joining the reader thread unnecessary: the number is never
        // released while the reader or writer might still use it. Releasing the gate additionally
        // wakes a reader parked on consumer backpressure so teardown cannot deadlock on it.
        writer.stop()
        readLatch.stop()
        backlogGate.release()
        readThread = nil
        socketFD = -1
        continuation.finish()
    }
}
