//
// Transport.swift
// TransportSSH
//
// Transport protocol abstraction for SRUI network and IPC channels (§19, §19.2, §20.2, §22).
//
// Logical classes are a transport scheduling concern, not Core message semantics. SSH/TCP
// serialize selected frames onto one byte stream; a future QUIC binding may map the same
// classes to independent streams without changing protobuf messages.
//

import Foundation
#if canImport(Darwin)
import Darwin
#elseif canImport(Glibc)
import Glibc
#endif
@_exported import LogicalChannelScheduling

/// Errors produced during transport operations.
public enum TransportError: Error, CustomStringConvertible, Sendable {
    case connectionFailed(String)
    case closed
    case ioError(String)
    case timeout

    public var description: String {
        switch self {
        case .connectionFailed(let msg):
            return "Transport connection failed: \(msg)"
        case .closed:
            return "Transport is closed"
        case .ioError(let msg):
            return "Transport I/O error: \(msg)"
        case .timeout:
            return "Transport operation timed out"
        }
    }
}

// MARK: - Inbound backpressure (§14, §20.4, §26)

/// Bounds how far a socket reader may run ahead of the consumer of `receiveStream()`.
///
/// `AsyncThrowingStream.makeStream()` buffers `.unbounded` by default, so a reader thread that
/// yields every 64 KiB read regardless of consumption lets a fast server grow client memory
/// without limit whenever the consumer stalls — and the consumer stalls routinely, because it
/// hops to the main actor to render each transaction (§22.2).
///
/// Dropping or coalescing is not an option in this direction: committed transactions may never be
/// silently discarded (§20.4). So the reader simply stops pulling from the socket while the
/// consumer is behind, which turns the bound into real transport-level backpressure and pushes it
/// back to the peer through the TCP/pipe window.
final class InboundBacklogGate: @unchecked Sendable {
    /// Default ceiling on bytes yielded but not yet acknowledged by the consumer.
    ///
    /// Comfortably above one `max_frame_size` frame's worth of reads so a single large snapshot
    /// still streams, and far below anything that matters for process memory.
    static let defaultLimitBytes = 8 * 1024 * 1024

    private let condition = NSCondition()
    private let limitBytes: Int
    private var outstanding = 0
    private var released = false

    init(limitBytes: Int = InboundBacklogGate.defaultLimitBytes) {
        self.limitBytes = Swift.max(1, limitBytes)
    }

    /// Bytes delivered to the stream that the consumer has not acknowledged yet.
    var outstandingBytes: Int {
        condition.lock()
        defer { condition.unlock() }
        return outstanding
    }

    /// Parks the reader thread while the consumer is at or beyond the limit.
    ///
    /// Returns `false` once the transport has been torn down, so the reader exits its loop.
    func waitForCapacity() -> Bool {
        condition.lock()
        defer { condition.unlock() }
        while !released && outstanding >= limitBytes {
            condition.wait()
        }
        return !released
    }

    func recordDelivered(_ byteCount: Int) {
        condition.lock()
        outstanding += byteCount
        condition.unlock()
    }

    func recordConsumed(_ byteCount: Int) {
        condition.lock()
        outstanding = Swift.max(0, outstanding - byteCount)
        condition.broadcast()
        condition.unlock()
    }

    /// Wakes a parked reader permanently; used on close so teardown never deadlocks on the gate.
    func release() {
        condition.lock()
        released = true
        condition.broadcast()
        condition.unlock()
    }
}

// MARK: - Non-blocking reads (§22.2)

/// Outcome of one `read(2)` attempt against a latched non-blocking descriptor.
enum SocketReadResult {
    case bytes(Int)
    case endOfStream
    /// Interrupted or not ready yet; the caller should loop, having already waited for readability.
    case retry
    /// The latch was torn down; the reader must exit.
    case stopped
    case failed(errno: Int32)
}

/// Reads whatever is available, waiting for readability rather than parking in `read(2)`.
///
/// The descriptor is non-blocking (see `SocketReadLatch.adopt`), so readiness is awaited through
/// `poll(2)` with a short slice. That keeps the reader responsive to teardown — `stop()` shuts the
/// descriptor down, which makes `poll` return immediately — without spinning on `EAGAIN`.
func readAvailable(from latch: SocketReadLatch, into buffer: inout [UInt8]) -> SocketReadResult {
    guard let fd = latch.beginIO() else { return .stopped }
    defer { latch.endIO() }

    let bytesRead = buffer.withUnsafeMutableBytes { raw -> Int in
        guard let base = raw.baseAddress else { return 0 }
        return Darwin.read(fd, base, raw.count)
    }
    if bytesRead > 0 { return .bytes(bytesRead) }
    if bytesRead == 0 { return .endOfStream }

    let err = errno
    if err == EINTR { return .retry }
    guard err == EAGAIN || err == EWOULDBLOCK else { return .failed(errno: err) }

    var descriptor = pollfd(fd: fd, events: Int16(POLLIN), revents: 0)
    let ready = Darwin.poll(&descriptor, 1, 250)
    if ready < 0 && errno != EINTR {
        return .failed(errno: errno)
    }
    return .retry
}

// MARK: - Off-actor socket writes (§22.2)

/// Performs blocking descriptor writes on a dedicated serial queue instead of an actor executor.
///
/// A `write(2)` issued from inside an actor method occupies that actor for its whole duration: a
/// peer that stops reading fills the socket buffer, the write pends, and `close()` — also
/// actor-isolated — can never run, so `SessionController.stop()` awaits a `transport.close()` that
/// is queued behind the very write it is supposed to abort. Moving the write here keeps the actor
/// free, so `close()` runs, shuts the descriptor down, and the parked write fails out.
///
/// Concurrent `write` calls enter per-class FIFO queues. The serial queue drains them through
/// [`LogicalChannelScheduler`] without holding `NSLock` during `write(2)`, `poll(2)`, or an
/// async suspension (§19.2, §22.2). `EventOutbox.sendTail` still serializes event allocation
/// order into the input lane; the scheduler does not replace that §18.2 guarantee.
final class SocketWriter: @unchecked Sendable {
    /// How long one payload may stay unwritable before the peer is treated as unreachable.
    static let defaultTimeout: TimeInterval = 30

    /// Largest single `write(2)` issued between writability polls.
    private static let chunkSize = 64 * 1024

    fileprivate struct PendingWrite {
        let data: Data
        let continuation: CheckedContinuation<Void, any Error>
    }

    private let queue: DispatchQueue
    private let timeout: TimeInterval
    private let label: String
    private let lock = NSLock()
    private var stopped = false
    private var drainScheduled = false
    private var scheduler = LogicalChannelScheduler()
    private var queues: [LogicalChannelClass: [PendingWrite]] = Dictionary(
        uniqueKeysWithValues: LogicalChannelClass.allCases.map { ($0, []) }
    )
    /// Optional test sink; when set, the drain loop calls this instead of `write(2)`.
    private let testSink: ((Data) throws -> Void)?

    init(label: String, timeout: TimeInterval = SocketWriter.defaultTimeout) {
        self.label = label
        self.queue = DispatchQueue(label: label)
        self.timeout = timeout
        self.testSink = nil
    }

    /// Test-only writer that drains through `sink` instead of a socket descriptor.
    init(
        label: String,
        timeout: TimeInterval = SocketWriter.defaultTimeout,
        testSink: @escaping (Data) throws -> Void
    ) {
        self.label = label
        self.queue = DispatchQueue(label: label)
        self.timeout = timeout
        self.testSink = testSink
    }

    /// Refuses further writes and resumes every queued continuation exactly once.
    func stop() {
        let pending = takeAllAndStop()
        resumeAll(pending, throwing: TransportError.closed)
    }

    /// Frames waiting in per-class queues, excluding a write already popped for I/O.
    func queuedCount() -> Int {
        lock.lock()
        defer { lock.unlock() }
        return LogicalChannelClass.allCases.reduce(0) { partial, logicalClass in
            partial + (queues[logicalClass]?.count ?? 0)
        }
    }

    /// Frames waiting in one logical-class queue, excluding a write already popped for I/O.
    func queuedCount(for logicalClass: LogicalChannelClass) -> Int {
        lock.lock()
        defer { lock.unlock() }
        return queues[logicalClass]?.count ?? 0
    }

    private var isStopped: Bool {
        lock.lock()
        defer { lock.unlock() }
        return stopped
    }

    /// Writes `data` as control-class traffic. Compatibility path for unclassified callers.
    func write(_ data: Data, claiming latch: SocketReadLatch) async throws {
        try await write(data, logicalClass: .control, claiming: latch)
    }

    /// Enqueues `data` on `logicalClass` and resumes when the scheduler drains it.
    func write(
        _ data: Data,
        logicalClass: LogicalChannelClass,
        claiming latch: SocketReadLatch
    ) async throws {
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, any Error>) in
            lock.lock()
            if stopped {
                lock.unlock()
                continuation.resume(throwing: TransportError.closed)
                return
            }
            var lane = queues[logicalClass] ?? []
            lane.append(PendingWrite(data: data, continuation: continuation))
            queues[logicalClass] = lane
            let shouldSchedule = !drainScheduled
            if shouldSchedule {
                drainScheduled = true
            }
            lock.unlock()
            if shouldSchedule {
                queue.async {
                    self.drain(claiming: latch)
                }
            }
        }
    }

    private func drain(claiming latch: SocketReadLatch) {
        while true {
            lock.lock()
            if stopped {
                let pending = takeAllLocked()
                drainScheduled = false
                lock.unlock()
                resumeAll(pending, throwing: TransportError.closed)
                return
            }
            guard let logicalClass = scheduler.selectNext(ready: { candidate in
                !(self.queues[candidate] ?? []).isEmpty
            }) else {
                drainScheduled = false
                lock.unlock()
                return
            }
            var lane = queues[logicalClass] ?? []
            let item = lane.removeFirst()
            queues[logicalClass] = lane
            lock.unlock()

            do {
                if let testSink {
                    try testSink(item.data)
                } else {
                    try writeSynchronously(item.data, claiming: latch)
                }
                item.continuation.resume()
            } catch {
                lock.lock()
                stopped = true
                let rest = takeAllLocked()
                drainScheduled = false
                lock.unlock()
                item.continuation.resume(throwing: error)
                resumeAll(rest, throwing: error)
                return
            }
        }
    }

    private func takeAllAndStop() -> [PendingWrite] {
        lock.lock()
        stopped = true
        let pending = takeAllLocked()
        drainScheduled = false
        lock.unlock()
        return pending
    }

    private func takeAllLocked() -> [PendingWrite] {
        var pending: [PendingWrite] = []
        for logicalClass in LogicalChannelClass.allCases {
            pending.append(contentsOf: queues[logicalClass] ?? [])
            queues[logicalClass] = []
        }
        return pending
    }

    private func resumeAll(_ items: [PendingWrite], throwing error: Error) {
        for item in items {
            item.continuation.resume(throwing: error)
        }
    }

    private func writeSynchronously(_ data: Data, claiming latch: SocketReadLatch) throws {
        guard !isStopped, let fd = latch.beginIO() else {
            throw TransportError.closed
        }
        defer { latch.endIO() }

        let deadline = Date().addingTimeInterval(timeout)
        try data.withUnsafeBytes { rawBuffer in
            guard let baseAddress = rawBuffer.baseAddress else { return }
            var bytesWritten = 0

            while bytesWritten < rawBuffer.count {
                if isStopped {
                    throw TransportError.closed
                }

                // Bounded by a deadline, so a peer that never drains its buffer surfaces as a
                // timeout instead of parking this queue until the transport is closed.
                try waitUntilWritable(fd: fd, deadline: deadline)

                let remaining = Swift.min(rawBuffer.count - bytesWritten, Self.chunkSize)
                let written = Darwin.write(fd, baseAddress.advanced(by: bytesWritten), remaining)

                if written > 0 {
                    bytesWritten += written
                    continue
                }
                if written == 0 {
                    throw TransportError.closed
                }
                let err = errno
                if err == EINTR || err == EAGAIN {
                    continue
                }
                throw TransportError.ioError(
                    "\(label) write failed: \(String(cString: strerror(err)))"
                )
            }
        }
    }

    private func waitUntilWritable(fd: Int32, deadline: Date) throws {
        while true {
            let remaining = deadline.timeIntervalSinceNow
            guard remaining > 0 else {
                throw TransportError.timeout
            }
            // Capped so the stop flag and the deadline are both re-checked promptly.
            let sliceMilliseconds = Int32(Swift.min(remaining, 0.25) * 1000) + 1
            var descriptor = pollfd(fd: fd, events: Int16(POLLOUT), revents: 0)
            let ready = Darwin.poll(&descriptor, 1, sliceMilliseconds)

            if ready < 0 {
                if errno == EINTR { continue }
                throw TransportError.ioError(
                    "\(label) poll failed: \(String(cString: strerror(errno)))"
                )
            }
            if ready == 0 {
                if isStopped { throw TransportError.closed }
                continue
            }
            if descriptor.revents & Int16(POLLNVAL) != 0 {
                throw TransportError.closed
            }
            return
        }
    }
}

/// Abstract stream transport interface decoupling the protocol and session layers
/// from the underlying socket or SSH channel implementation (§19, §20.2, §22).
public protocol Transport: Sendable {
    /// Transmits raw framed bytes on an explicit logical class (§19.2).
    func send(data: Data, logicalClass: LogicalChannelClass) async throws

    /// Returns an asynchronous throwing stream of incoming raw byte chunks from the remote peer.
    func receiveStream() -> AsyncThrowingStream<Data, Error>

    /// Gracefully closes the transport connection and releases underlying resources.
    func close() async

    /// Reports that `byteCount` bytes taken from `receiveStream()` have been fully processed.
    ///
    /// This is the release half of inbound backpressure (§14, §26): a transport that reads ahead
    /// of its consumer must stop reading until the consumer catches up, and only the consumer
    /// knows when a chunk is done. Consumers MUST call this after handling each chunk; transports
    /// with no read-ahead buffer of their own may keep the default no-op.
    func acknowledgeReceived(byteCount: Int) async
}

extension Transport {
    /// Convenience path: unclassified writes are control-class traffic.
    public func send(data: Data) async throws {
        try await send(data: data, logicalClass: .control)
    }

    /// In-memory transports have no socket to throttle, so acknowledgement is a no-op for them.
    public func acknowledgeReceived(byteCount: Int) async {}
}
