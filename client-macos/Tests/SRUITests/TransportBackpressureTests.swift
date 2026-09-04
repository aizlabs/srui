//
// TransportBackpressureTests.swift
// SRUITests
//
// Inbound backpressure and off-actor write preemption for socket transports (§14, §20.4, §22.2, §26).
//

import Testing
import Foundation
@testable import TransportSSH

#if canImport(Darwin)
import Darwin
#endif

/// Minimal cross-thread cell; the semaphores in each test provide the ordering.
private final class Box<T>: @unchecked Sendable {
    private let lock = NSLock()
    private var storage: T

    init(_ value: T) { storage = value }

    var value: T {
        get { lock.lock(); defer { lock.unlock() }; return storage }
        set { lock.lock(); storage = newValue; lock.unlock() }
    }
}

@Suite("Transport Backpressure & Write Preemption (§20.4, §22.2, §26)")
struct TransportBackpressureTests {

    // MARK: - Inbound backlog gate (§26)

    /// §26: the reader must stop pulling from the socket while the consumer is behind.
    ///
    /// `AsyncThrowingStream.makeStream()` buffers `.unbounded`, so without this gate a reader that
    /// yields every read regardless of consumption lets a fast server grow client memory without
    /// limit. Dropping is not an alternative in this direction: committed transactions may never
    /// be silently discarded (§20.4).
    @Test("A reader parks at the backlog limit and resumes only once the consumer acknowledges")
    func readerParksUntilConsumerCatchesUp() throws {
        let gate = InboundBacklogGate(limitBytes: 1024)
        gate.recordDelivered(1024)
        #expect(gate.outstandingBytes == 1024)

        let entered = DispatchSemaphore(value: 0)
        let admitted = DispatchSemaphore(value: 0)
        let capacityGranted = Box(false)

        let reader = Thread {
            entered.signal()
            let granted = gate.waitForCapacity()
            capacityGranted.value = granted
            admitted.signal()
        }
        reader.start()
        entered.wait()

        #expect(
            admitted.wait(timeout: .now() + 0.25) == .timedOut,
            "the reader must stay parked while the consumer is at the limit"
        )

        gate.recordConsumed(1024)
        #expect(
            admitted.wait(timeout: .now() + 5) == .success,
            "acknowledging consumption must release the reader"
        )
        #expect(capacityGranted.value)
        #expect(gate.outstandingBytes == 0)
    }

    /// Teardown must never deadlock on the gate: a parked reader is released, not left waiting for
    /// a consumer that has gone away.
    @Test("Releasing the gate wakes a parked reader and tells it to stop")
    func releaseWakesParkedReader() throws {
        let gate = InboundBacklogGate(limitBytes: 16)
        gate.recordDelivered(64)

        let admitted = DispatchSemaphore(value: 0)
        let capacityGranted = Box(true)

        let reader = Thread {
            capacityGranted.value = gate.waitForCapacity()
            admitted.signal()
        }
        reader.start()

        #expect(admitted.wait(timeout: .now() + 0.25) == .timedOut)
        gate.release()
        #expect(admitted.wait(timeout: .now() + 5) == .success)
        #expect(
            capacityGranted.value == false,
            "a released gate must tell the reader to exit rather than to read again"
        )
    }

    // MARK: - Off-actor writes (§22.2)

    /// Creates a connected `AF_UNIX` socket pair with a deliberately tiny send buffer, so a peer
    /// that never reads fills it within one payload.
    private func makeStalledSocketPair() throws -> (writable: Int32, unread: Int32) {
        var fds: [Int32] = [-1, -1]
        #expect(socketpair(AF_UNIX, SOCK_STREAM, 0, &fds) == 0)
        var sendBuffer: Int32 = 2048
        _ = setsockopt(fds[0], SOL_SOCKET, SO_SNDBUF, &sendBuffer, socklen_t(MemoryLayout<Int32>.size))
        var receiveBuffer: Int32 = 2048
        _ = setsockopt(fds[1], SOL_SOCKET, SO_RCVBUF, &receiveBuffer, socklen_t(MemoryLayout<Int32>.size))
        return (fds[0], fds[1])
    }

    /// §22.2: a stalled peer must not be able to pin the write path forever.
    ///
    /// Before the fix the blocking `write(2)` ran on the transport actor's executor, so `close()`
    /// — also actor-isolated — was queued behind the very write it was meant to abort, and
    /// `SessionController.stop()` awaited it forever.
    @Test("A write to a peer that never reads fails on its deadline instead of blocking forever")
    func stalledWriteFailsOnDeadline() async throws {
        let (writable, unread) = try makeStalledSocketPair()
        defer {
            Darwin.close(unread)
        }

        let latch = SocketReadLatch()
        latch.adopt(descriptor: writable)
        let writer = SocketWriter(label: "test.stalled-write", timeout: 0.3)

        // Far larger than both socket buffers, and nothing ever reads from `unread`.
        let payload = Data(repeating: 0xAB, count: 4 * 1024 * 1024)
        let started = Date()
        await #expect(throws: TransportError.self) {
            try await writer.write(payload, claiming: latch)
        }
        #expect(
            Date().timeIntervalSince(started) < 10,
            "the write must surface as a timeout rather than parking indefinitely"
        )
        latch.stop()
    }

    /// The other half of preemption: teardown unblocks an already-parked write immediately, which
    /// is what makes `close()` reachable while a peer is stalled.
    @Test("Stopping the descriptor latch unblocks a write parked on a full socket buffer")
    func stoppingLatchUnblocksParkedWrite() async throws {
        let (writable, unread) = try makeStalledSocketPair()
        defer { Darwin.close(unread) }

        let latch = SocketReadLatch()
        latch.adopt(descriptor: writable)
        // Long deadline: only the teardown can end this write in time.
        let writer = SocketWriter(label: "test.preempted-write", timeout: 120)

        let payload = Data(repeating: 0xCD, count: 4 * 1024 * 1024)
        let write = Task { try await writer.write(payload, claiming: latch) }

        // Let the writer reach the full buffer before tearing the descriptor down.
        try await Task.sleep(nanoseconds: 200_000_000)

        writer.stop()
        latch.stop()

        let outcome = await write.result
        #expect(
            (try? outcome.get()) == nil,
            "a write preempted by teardown must fail rather than report success"
        )
    }

    /// Queued writes across classes each resume exactly once on teardown.
    @Test("Stopping the writer resumes every queued continuation exactly once")
    func stopResumesEveryQueuedContinuationOnce() async throws {
        let sink = GatedFailingSink()
        let writer = SocketWriter(label: "test.multi-class-stop", testSink: { try sink.write($0) })
        let latch = SocketReadLatch()

        let classes: [LogicalChannelClass] = [.control, .input, .ui, .resource, .terminalHigh, .terminalNormal]
        let tasks = classes.map { logicalClass in
            Task {
                try await writer.write(Data("\(logicalClass)".utf8), logicalClass: logicalClass, claiming: latch)
            }
        }

        sink.waitUntilEntered(1)
        writer.stop()
        sink.fail(TransportError.closed)
        latch.stop()

        var finished = 0
        for task in tasks {
            let result = await task.result
            #expect((try? result.get()) == nil)
            finished += 1
        }
        #expect(finished == classes.count)
    }

    /// A write failure fails every queued class continuation once; no hang after teardown.
    @Test("A write failure resumes queued continuations exactly once")
    func writeFailureResumesQueuedContinuationsOnce() async throws {
        let sink = GatedFailingSink()
        let writer = SocketWriter(label: "test.multi-class-fail", testSink: { try sink.write($0) })
        let latch = SocketReadLatch()

        let classes: [LogicalChannelClass] = [.resource, .control, .input, .ui]
        let tasks = classes.map { logicalClass in
            Task {
                try await writer.write(Data("\(logicalClass)".utf8), logicalClass: logicalClass, claiming: latch)
            }
        }

        sink.waitUntilEntered(1)
        sink.fail(TransportError.ioError("injected failure"))

        for task in tasks {
            let result = await task.result
            #expect((try? result.get()) == nil)
        }
        latch.stop()
    }

    /// The latch must not release a descriptor number while an I/O call still holds it, or the
    /// kernel can recycle it under a parked `read`/`write`.
    @Test("A stop during an outstanding claim defers the close to the claim holder")
    func stopDefersCloseWhileClaimed() throws {
        let (writable, unread) = try makeStalledSocketPair()
        defer { Darwin.close(unread) }

        let latch = SocketReadLatch()
        latch.adopt(descriptor: writable)

        let claimed = try #require(latch.beginIO())
        #expect(claimed == writable)

        latch.stop()
        #expect(latch.isStopped)
        // Still open: the claim holder has not finished, so the number cannot be recycled yet.
        #expect(fcntl(writable, F_GETFD) != -1)

        latch.endIO()
        #expect(fcntl(writable, F_GETFD) == -1, "the last claim release must perform the close")
        #expect(latch.beginIO() == nil, "a stopped latch hands out no further claims")
    }
}

private final class GatedFailingSink: @unchecked Sendable {
    private let condition = NSCondition()
    private var entered = 0
    private var failure: (any Error)?

    func write(_ data: Data) throws {
        _ = data
        condition.lock()
        entered += 1
        condition.broadcast()
        while failure == nil {
            condition.wait()
        }
        let error = failure!
        condition.unlock()
        throw error
    }

    func waitUntilEntered(_ count: Int) {
        condition.lock()
        while entered < count {
            condition.wait()
        }
        condition.unlock()
    }

    func fail(_ error: any Error) {
        condition.lock()
        failure = error
        condition.broadcast()
        condition.unlock()
    }
}
