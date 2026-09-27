//
// LogicalChannelSchedulerTests.swift
// SRUITests
//
// SocketWriter drain and Session/EventOutbox classification tests (§18.2, §19.2).
//
// Pure scheduler algorithm tests live in LogicalChannelSchedulingTests and run on Linux.
//

import Testing
import Foundation
import SemanticModel
import Protocol
@testable import Session
@testable import TransportSSH

private let resourceTokenSize = 16 * 1024

private func resourceToken(_ id: UInt32) -> Data {
    var data = Data(count: resourceTokenSize)
    data[0] = 0xAB
    var bigEndian = id.bigEndian
    withUnsafeBytes(of: &bigEndian) { bytes in
        data.replaceSubrange(1..<5, with: bytes)
    }
    return data
}

private func tokenId(_ data: Data) -> UInt32? {
    guard data.count == resourceTokenSize, data[0] == 0xAB else { return nil }
    return data.subdata(in: 1..<5).withUnsafeBytes { $0.load(as: UInt32.self).bigEndian }
}

/// Gates `SocketWriter`'s drain queue so a test can inspect scheduler ordering mid-flight.
///
/// The park inside `write` stays blocking: it runs on the writer's private `DispatchQueue`, which is
/// the stalled-peer behaviour under test. The *waiters* may not block, because they run on the
/// test's own cooperative thread and the writes they are waiting for are child tasks that need a
/// cooperative thread of their own - blocking here deadlocks the test outright wherever the pool has
/// no spare thread. See `AsyncTestSignal`.
private final class GatedByteSink: @unchecked Sendable {
    private let condition = NSCondition()
    private var permits = 0
    private var failure: (any Error)?
    private(set) var dispatched: [Data] = []
    private let entries = AsyncTestSignal()
    private let dispatches = AsyncTestSignal()

    func write(_ data: Data) throws {
        entries.signal()
        condition.lock()
        while permits == 0 && failure == nil {
            condition.wait()
        }
        if let failure {
            condition.unlock()
            throw failure
        }
        permits -= 1
        dispatched.append(data)
        condition.broadcast()
        condition.unlock()
        dispatches.signal()
    }

    private var hasFailed: Bool {
        condition.lock()
        defer { condition.unlock() }
        return failure != nil
    }

    func waitUntilDispatched(_ count: Int) async {
        while snapshot().count < count, !hasFailed {
            try? await Task.sleep(for: .milliseconds(2))
        }
    }

    func snapshot() -> [Data] {
        condition.lock()
        defer { condition.unlock() }
        return dispatched
    }

    func waitUntilEntered(_ count: Int) async {
        await entries.wait(until: count)
    }

    func release(_ count: Int = 1) {
        condition.lock()
        permits += count
        condition.broadcast()
        condition.unlock()
    }

    func fail(_ error: any Error) {
        condition.lock()
        failure = error
        condition.broadcast()
        condition.unlock()
    }
}

private actor RecordingTransport: Transport {
    struct RecordedWrite: Sendable {
        let data: Data
        let logicalClass: LogicalChannelClass
    }

    private(set) var writes: [RecordedWrite] = []
    private let stream: AsyncThrowingStream<Data, Error>
    private let continuation: AsyncThrowingStream<Data, Error>.Continuation

    init() {
        let (stream, continuation) = AsyncThrowingStream<Data, Error>.makeStream()
        self.stream = stream
        self.continuation = continuation
    }

    func send(data: Data, logicalClass: LogicalChannelClass) async throws {
        writes.append(RecordedWrite(data: data, logicalClass: logicalClass))
    }

    func recordedWrites() -> [RecordedWrite] { writes }

    nonisolated func receiveStream() -> AsyncThrowingStream<Data, Error> { stream }

    func close() async {
        continuation.finish()
    }
}

@Suite("Logical Channel Transport Classification (§19.2)")
struct LogicalChannelTransportTests {

    @Test("Resource backlog yields to newly ready control, input, and UI")
    func resourceBacklogYieldsToHigherClasses() async throws {
        let sink = GatedByteSink()
        let writer = SocketWriter(label: "test.scheduler-gate", testSink: { try sink.write($0) })
        let latch = SocketReadLatch()

        try await withThrowingTaskGroup(of: Void.self) { group in
            defer {
                writer.stop()
                sink.fail(TransportError.closed)
                latch.stop()
            }

            group.addTask {
                _ = try? await writer.write(
                    resourceToken(0),
                    logicalClass: .resource,
                    claiming: latch
                )
            }
            await sink.waitUntilEntered(1)

            for id in UInt32(1)..<UInt32(200) {
                let payload = resourceToken(id)
                group.addTask {
                    _ = try? await writer.write(
                        payload,
                        logicalClass: .resource,
                        claiming: latch
                    )
                }
            }
            try await waitUntilQueued(199, in: .resource, writer: writer)

            let control = Data("control-probe".utf8)
            let input = Data("input-probe".utf8)
            let ui = Data("ui-probe".utf8)

            group.addTask {
                _ = try? await writer.write(control, logicalClass: .control, claiming: latch)
            }
            try await waitUntilQueued(1, in: .control, writer: writer)

            group.addTask {
                _ = try? await writer.write(input, logicalClass: .input, claiming: latch)
            }
            try await waitUntilQueued(1, in: .input, writer: writer)

            group.addTask {
                _ = try? await writer.write(ui, logicalClass: .ui, claiming: latch)
            }
            try await waitUntilQueued(1, in: .ui, writer: writer)

            sink.release(8)
            await sink.waitUntilDispatched(4)

            let dispatched = sink.snapshot()
            #expect(dispatched.count >= 4)
            #expect(tokenId(dispatched[0]) == 0)

            let rest = dispatched.dropFirst()
            let controlAt = try #require(rest.firstIndex(of: control))
            let inputAt = try #require(rest.firstIndex(of: input))
            let uiAt = try #require(rest.firstIndex(of: ui))
            let probeEnd = try #require([controlAt, inputAt, uiAt].max())
            let resourceBeforeProbes = rest.prefix(through: probeEnd).filter {
                tokenId($0) != nil
            }.count
            #expect(
                resourceBeforeProbes == 0,
                "no second resource token may precede control/input/UI probes"
            )
        }
    }

    private func waitUntilQueued(
        _ expectedCount: Int,
        in logicalClass: LogicalChannelClass,
        writer: SocketWriter
    ) async throws {
        var spins = 0
        while writer.queuedCount(for: logicalClass) < expectedCount {
            spins += 1
            try #require(spins < 100_000, "writes never entered the expected scheduler queue")
            await Task.yield()
        }
    }

    @Test("SessionController sends HELLO and RESUME as control")
    func sessionControllerClassifiesHandshakeAsControl() async throws {
        let helloTransport = RecordingTransport()
        let helloController = SessionController(transport: helloTransport)
        try await helloController.start()
        let helloWrites = await helloTransport.recordedWrites()
        await helloController.stop()
        await helloTransport.close()

        #expect(helloWrites.count == 1)
        #expect(helloWrites[0].logicalClass == .control)
        let helloMessage = try decodeFramedMessage(from: helloWrites[0].data)
        guard case .clientHello = helloMessage.msg else {
            Issue.record("expected CLIENT HELLO, got \(String(describing: helloMessage.msg))")
            return
        }

        let resumeTransport = RecordingTransport()
        let resumeController = SessionController(transport: resumeTransport, sessionId: "session-resume")
        try await resumeController.start()
        let resumeWrites = await resumeTransport.recordedWrites()
        await resumeController.stop()
        await resumeTransport.close()

        #expect(resumeWrites.count == 1)
        #expect(resumeWrites[0].logicalClass == .control)
        let resumeMessage = try decodeFramedMessage(from: resumeWrites[0].data)
        guard case .clientResume = resumeMessage.msg else {
            Issue.record("expected CLIENT RESUME, got \(String(describing: resumeMessage.msg))")
            return
        }
    }

    @Test("EventOutbox sends original and replayed events as input")
    func eventOutboxClassifiesEventsAsInput() async throws {
        let transport = RecordingTransport()
        let outbox = EventOutbox()
        let binding = await outbox.beginConnectionBinding()
        #expect(await outbox.allowNewEvents(binding: binding))
        _ = try await outbox.sendActivate(
            nodeId: NodeId(1),
            observedRevision: Revision(1),
            binding: binding,
            via: transport
        )
        _ = try await outbox.sendActivate(
            nodeId: NodeId(2),
            observedRevision: Revision(1),
            binding: binding,
            via: transport
        )
        try await outbox.resendPendingEvents(binding: binding, via: transport)

        let writes = await transport.recordedWrites()
        #expect(writes.count == 4)
        for write in writes {
            #expect(write.logicalClass == .input)
            let message = try decodeFramedMessage(from: write.data)
            guard case .event = message.msg else {
                Issue.record("expected semantic event, got \(String(describing: message.msg))")
                return
            }
        }
        await transport.close()
    }
}
